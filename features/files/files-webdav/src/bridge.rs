//! [`WebdavBridge`] — one org's WebDAV mount.
//!
//! ## The URL space
//!
//! ```text
//! <mount>/                        the visible roots, as folders   (RootsFs)
//! <mount>/<root>/…                that root's live tree, rw       (LiveTreeFs)
//! ```
//!
//! `<mount>` is whatever the host route mounted this at (task-server
//! uses `/org/{slug}/dav`), passed in per request rather than baked in,
//! so the bridge stays a plain library and the org slug stays the
//! router's business.
//!
//! There is deliberately no third level of URL space. A version-
//! addressed path (`…/@v/<commit>/…`) is exactly what issue #274 rules
//! out — "only current heads are visible; version history is not
//! exposed" — and the version chain already has a first-class surface in
//! `FilesService::chain`. This bridge is the compat path for an OS file
//! manager, never the sync path.
//!
//! ## Dispatch
//!
//! Each request's path is normalized once by [`DavPath`] (which
//! percent-decodes and rejects `..` escapes), the mount prefix is
//! stripped, and the first remaining segment selects the filesystem:
//! empty → the roots collection, otherwise the named root. The handler
//! is then given `strip_prefix` down to that root, so every path the
//! root's view sees is root-relative — the same coordinate system
//! `FilesService::browse` uses.
//!
//! ## Locks
//!
//! macOS and Windows will not mount a share read-write without
//! `LOCK`/`UNLOCK`, so each root gets its own [`MemLs`], created on
//! first touch and kept for the process's life. Per root, not one
//! shared: paths are root-relative by the time the lock system sees
//! them, so a single instance would have `/Mix.wav` in two different
//! projects contending for one lock.

use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};

use bytes::Buf;
use dav_server::body::Body;
use dav_server::davpath::DavPath;
use dav_server::memls::MemLs;
use dav_server::{DavConfig, DavHandler, DavMethodSet};
use files::{FilesBackend, FilesService as _};
use files_proto::{FileRootInfo, FilesError};
use http::{Request, Response, StatusCode};
use http_body::Body as HttpBody;
use uuid::Uuid;

use crate::live_tree_fs::LiveTreeFs;
use crate::naming;
use crate::policy::WebdavPolicy;
use crate::roots_fs::RootsFs;

/// One org's WebDAV surface over its File Roots.
#[derive(Clone)]
pub struct WebdavBridge {
    backend: FilesBackend,
    policy: Arc<WebdavPolicy>,
    handler: DavHandler,
    locks: Arc<Mutex<HashMap<Uuid, MemLs>>>,
}

impl std::fmt::Debug for WebdavBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebdavBridge")
            .field("policy", &self.policy.path())
            .finish_non_exhaustive()
    }
}

impl WebdavBridge {
    /// Bridge over `backend`'s roots, with its policy file living
    /// beside that backend's registry.
    #[must_use]
    pub fn new(backend: FilesBackend) -> Self {
        let policy = WebdavPolicy::open(backend.data_dir());
        let handler = DavHandler::builder()
            // Every method the OS clients need, and nothing that would
            // hand out a second, version-shaped view of the tree.
            .methods(DavMethodSet::WEBDAV_RW)
            // No HTML directory index. dav-server will happily render
            // one for a browser `GET`, but a browsable file listing is
            // a *surface*, and the spec puts browsing behind the
            // explorer widgets over the Files RPC surface — not on the
            // compat bridge. WebDAV clients `PROPFIND`; they never
            // wanted it.
            .autoindex(false)
            // A symlink out of the live tree is already refused by
            // `LiveTreeFs`; not listing them keeps the mount honest
            // about what it will actually serve.
            .hide_symlinks(true)
            .build_handler();
        Self {
            backend,
            policy: Arc::new(policy),
            handler,
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The per-root visibility policy — an operator's "hide this root
    /// from WebDAV" switch.
    #[must_use]
    pub fn policy(&self) -> &WebdavPolicy {
        &self.policy
    }

    /// Roots this bridge may expose right now, in registry order,
    /// paired with their URL segments.
    ///
    /// Errors propagate. A registry read that fails transiently —
    /// `roots.json` caught mid-rewrite, EMFILE under load, a
    /// permissions blip — must not be reported to a file manager as an
    /// empty root list: a mounted volume that suddenly lists no
    /// projects reads as *every project deleted*, and a client syncing
    /// or caching against the mount can act on that (PR #287 review).
    /// A 5xx is a mount hiccup; an empty multistatus is a catastrophe.
    async fn visible(&self) -> Result<Vec<naming::RootSegment>, FilesError> {
        let roots = self.backend.list_roots().await?;
        // One policy read for the whole request, not one per root.
        let hidden = self
            .policy
            .hidden_set()
            .map_err(|e| FilesError::Io(e.to_string()))?;
        let visible: Vec<FileRootInfo> = roots
            .into_iter()
            .filter(|r| !hidden.contains(&r.id))
            .collect();
        Ok(naming::segments(&visible))
    }

    fn lock_system(&self, root_id: Uuid) -> MemLs {
        self.locks
            .lock()
            .expect("webdav lock map poisoned")
            .entry(root_id)
            .or_insert_with(|| *MemLs::new())
            .clone()
    }

    /// Serve one WebDAV request. `mount` is this bridge's URL prefix
    /// with no trailing slash (e.g. `/org/acme/dav`); the caller is
    /// responsible for having authenticated and org-scoped the request
    /// before it gets here.
    pub async fn handle<ReqBody, ReqData, ReqError>(
        &self,
        mount: &str,
        req: Request<ReqBody>,
    ) -> Response<Body>
    where
        ReqData: Buf + Send + 'static,
        ReqError: StdError + Send + Sync + 'static,
        ReqBody: HttpBody<Data = ReqData, Error = ReqError>,
    {
        let mount = mount.trim_end_matches('/');
        let Some(segment) = self.segment_of(mount, req.uri()) else {
            return status(StatusCode::NOT_FOUND);
        };

        let entries = match self.visible().await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::error!(
                    target: "files_webdav::bridge",
                    error = %e,
                    "listing this org's roots failed — refusing rather than \
                     reporting an empty mount",
                );
                return status(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        if segment.is_empty() {
            let config = DavConfig::new()
                .strip_prefix(mount.to_string())
                .filesystem(Box::new(RootsFs::new(entries)))
                // The mount point is read-only, but it still needs a
                // lock system: `OPTIONS` on the URL a client is about
                // to mount omits `LOCK` from `Allow` without one, and
                // both Finder and Explorer read that to decide whether
                // the share is writable at all.
                .locksystem(Box::new(self.lock_system(Uuid::nil())));
            return self.handler.handle_with(config, req).await;
        }

        let Some(root) = naming::find(&entries, &segment).map(|e| e.root.clone()) else {
            // Hidden and nonexistent are the same answer on purpose: a
            // hidden root must not be distinguishable from one that was
            // never created.
            return status(StatusCode::NOT_FOUND);
        };

        // Belt and braces on top of the registry's own canonicalization:
        // a root's live tree must still sit inside this org's Files area
        // before we hand a filesystem view of it to a network client.
        //
        // The error *kind* is kept, not collapsed to a bool: `Escapes`
        // is a genuine confinement breach and deserves the alert, while
        // `Io` is a temporarily-unmounted volume or an EIO and is a 5xx.
        // Reporting the second as the first was both a false alarm and
        // the wrong status (PR #287 review).
        let base = match files_store::confine(
            root.local_tree().expect("a placed root"),
            self.backend.confine_root(),
        ) {
            Ok(base) => base,
            Err(files_store::PathError::Io(e)) => {
                tracing::warn!(
                    target: "files_webdav::bridge",
                    root = %root.id,
                    path = ?root.path,
                    error = %e,
                    "a root's live tree could not be resolved — transient, not a breach",
                );
                return status(StatusCode::INTERNAL_SERVER_ERROR);
            }
            Err(e) => {
                tracing::error!(
                    target: "files_webdav::bridge",
                    root = %root.id,
                    path = ?root.path,
                    error = %e,
                    "refusing to mount a root whose live tree is outside the org's files area",
                );
                return status(StatusCode::FORBIDDEN);
            }
        };

        // Destroying a whole project is not a file-manager gesture.
        //
        // A `DELETE`/`MOVE` addressed at the root collection itself
        // resolves to path `/` in the root's own filesystem, and
        // dav-server defaults a header-less DELETE to `Depth: Infinity`
        // — so dragging a mounted root to the Trash in Finder would
        // recurse the entire live tree. (The internals survive the
        // filtered `read_dir`, so the final `remove_dir` fails
        // ENOTEMPTY and the client reports an error *after* the data is
        // gone.) A root is a first-class entity with an identity, a
        // marker and a version store; it is removed through Files, not
        // by a drag (PR #287 review).
        let prefix = format!("{mount}/{segment}");
        let addresses_root = addresses_collection_itself(req.uri(), &prefix);
        if addresses_root && matches!(req.method().as_str(), "DELETE" | "MOVE") {
            tracing::info!(
                target: "files_webdav::bridge",
                root = %root.id,
                method = %req.method(),
                "refusing to delete or move a File Root through the WebDAV mount",
            );
            return status(StatusCode::FORBIDDEN);
        }

        // The prefix below ends in `/`, which `DavPath::set_prefix`
        // requires the path to actually carry. A client addressing the
        // collection as `…/dav/Mix` (no trailing slash) is asking for
        // the same thing as `…/dav/Mix/`, so normalize rather than
        // reject it.
        let req = if addresses_root {
            with_trailing_slash(req)
        } else {
            req
        };

        // Task-mediated hydrate-on-access (issue #263, glossary
        // "Pointer stub"): a WebDAV read of a stub hydrates it first,
        // then serves the real content — this bridge is exactly the
        // kind of Task-mediated surface the glossary names, unlike raw
        // NFS, which keeps reading a stub as a stub. Detection is the
        // platform's stat-bounded check, so an ordinary GET of media
        // costs one stat here and nothing more. A stub that cannot
        // hydrate (store missing its chunks) is a 502, not a silent
        // serve of placeholder bytes a DAW would try to play.
        if matches!(req.method().as_str(), "GET" | "HEAD") && !addresses_root {
            if let Some(rel) = rel_inside(req.uri(), &prefix) {
                let target = base.join(&rel);
                // Fail CLOSED on a stub that can't be read or parsed:
                // `.ok().flatten()` here would serve the ~100-byte
                // placeholder as the media file with a 200 — the exact
                // fail-open shape the stub module's own doc forbids
                // (PR #289 review). A candidate-sized file that errors
                // is refused, not served.
                let is_stub = match std::fs::metadata(&target) {
                    // Regular files only: a directory (including the
                    // hidden store dir itself, which the guard 404s
                    // downstream) is never a stub candidate.
                    Ok(m) if m.is_file() && files::stub::candidate_len(m.len()) => {
                        match files::stub::read(&target) {
                            Ok(found) => found.is_some(),
                            Err(err) => {
                                tracing::warn!(
                                    target: "files_webdav::bridge",
                                    root = %root.id,
                                    path = %rel,
                                    error = %err,
                                    "unreadable stub-sized file; refusing to serve it",
                                );
                                return status(StatusCode::BAD_GATEWAY);
                            }
                        }
                    }
                    _ => false,
                };
                if is_stub {
                    if let Err(err) = self.backend.hydrate(root.id, rel.clone()).await {
                        tracing::warn!(
                            target: "files_webdav::bridge",
                            root = %root.id,
                            path = %rel,
                            error = %err,
                            "hydrate-on-access failed; refusing to serve stub bytes",
                        );
                        return status(StatusCode::BAD_GATEWAY);
                    }
                }
            }
        }

        let config = DavConfig::new()
            // Trailing slash on purpose: `DavPath::set_prefix` is a raw
            // byte `starts_with`, so the prefix `…/dav/Mix` also matches
            // `…/dav/Mix Stems/…` — a `MOVE` between two roots whose
            // segments share a prefix would strip to ` Stems/take.wav`
            // and land the write back inside the *source* root, and a
            // shorter sibling (`…/dav/Mix2`) panics `DavPath::parent`
            // by leaving `pfxlen` past the end of the shortened path.
            // With the slash, dav-server verifies the boundary byte for
            // us (PR #287 review).
            .strip_prefix(format!("{mount}/{segment}/"))
            .filesystem(Box::new(LiveTreeFs::new(base)))
            .locksystem(Box::new(self.lock_system(root.id)));
        self.handler.handle_with(config, req).await
    }

    /// The first path segment below `mount` — `Some("")` for the mount
    /// itself, `None` when the request is not under this mount at all
    /// or its path is unusable.
    ///
    /// Normalizing through [`DavPath`] first is what makes this safe:
    /// it percent-decodes and collapses `.`/`..` *before* the prefix is
    /// compared, so `<mount>/A/../B/x` is matched as `B`, exactly as
    /// the handler will resolve it — no chance of dispatching to one
    /// root's filesystem a path that resolves inside another's.
    fn segment_of(&self, mount: &str, uri: &http::Uri) -> Option<String> {
        let path = DavPath::new(uri.path()).ok()?;
        let full = std::str::from_utf8(path.with_prefix().as_bytes()).ok()?;
        let rest = full.strip_prefix(mount)?;
        let rest = match rest.strip_prefix('/') {
            Some(r) => r,
            // `<mount>` exactly (no trailing slash) still addresses the
            // mount point.
            None if rest.is_empty() => "",
            None => return None,
        };
        Some(rest.split('/').next().unwrap_or("").to_string())
    }
}

/// The root-relative path `uri` addresses inside the root mounted at
/// `prefix`, [`DavPath`]-normalized (percent-decoded, `.`/`..`
/// collapsed) — the same resolution the handler itself will perform.
/// `None` for the collection itself or a path outside this prefix.
fn rel_inside(uri: &http::Uri, prefix: &str) -> Option<String> {
    let path = DavPath::new(uri.path()).ok()?;
    let full = std::str::from_utf8(path.with_prefix().as_bytes())
        .ok()?
        .to_owned();
    let rest = full.strip_prefix(prefix)?.strip_prefix('/')?;
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

/// Does `uri` address the collection at `prefix` itself, rather than
/// something inside it? Compared after [`DavPath`] normalization, so
/// `…/Mix/`, `…/Mix`, and `…/Mix/sub/..` are all the same answer.
fn addresses_collection_itself(uri: &http::Uri, prefix: &str) -> bool {
    let Ok(path) = DavPath::new(uri.path()) else {
        return false;
    };
    let Ok(full) = std::str::from_utf8(path.with_prefix().as_bytes()) else {
        return false;
    };
    full == prefix || full == format!("{prefix}/")
}

/// The same request with a `/` appended to its URI path.
fn with_trailing_slash<B>(req: Request<B>) -> Request<B> {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path();
    if path.ends_with('/') {
        return Request::from_parts(parts, body);
    }
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    if let Ok(uri) = format!("{path}/{query}").parse::<http::Uri>() {
        parts.uri = uri;
    }
    Request::from_parts(parts, body)
}

fn status(code: StatusCode) -> Response<Body> {
    Response::builder()
        .status(code)
        .body(Body::empty())
        .expect("static response builds")
}
