//! A File Root's documents, many in one request, for a signed-in member:
//! what an app mirrors a song's session folder by.
//!
//! A prepared session is a few hundred files of a few hundred bytes each
//! (its content-addressed objects) and a waveform per take. Read one by
//! one over the files lane, each read is a ticket and a byte stream — a
//! round trip or two that the server answers in seconds under load — so a
//! song took 19 s to mirror, nearly all of it waiting. This is the share
//! lane's batch (`share::share_documents_batch_handler`, the same records)
//! for the org's own people: the caller is whoever the bearer is, and each
//! path is served only where the files lane would let them read it — their
//! org role, or a grant at that path. A path they may not read answers as
//! not found, as the files lane's own reads do.

use architect_permissions::Principal;
use axum::extract::{Path as AxPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use files::lane::caller::{Caller, baseline_as};
use files_proto::id::{PrincipalId, RootId};
use files_proto::path::RootPath;
use files_proto::service::access::{Capability, Subject};

use crate::AppState;
use crate::share::{BATCH_NOT_FOUND, BATCH_PATHS_MAX, FilesScope, documents_batch, push_record};

/// `POST /org/{slug}/files/{root}/docs` — many of a File Root's documents
/// in one response, for the caller the bearer names. The body is the
/// paths, one per line, relative to the root; the response is one record
/// per path (`u8 status | u32le path length | path | u32le length |
/// bytes`), statuses as the share lane's. Media is never a document.
pub async fn root_documents_batch_handler(
    State(state): State<AppState>,
    AxPath((slug, root)): AxPath<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    use architect_telemetry::wide;
    wide::set("org.slug", slug.clone());
    let Some(org) = state.org(&slug) else {
        wide::set("files.docs_outcome", "no-org");
        return (StatusCode::NOT_FOUND, format!("org `{slug}` not hosted")).into_response();
    };
    let Ok(root_id) = root.parse::<uuid::Uuid>() else {
        wide::set("files.docs_outcome", "bad-root");
        return (StatusCode::NOT_FOUND, "no such root").into_response();
    };
    // Who is asking: the same resolution the org's RPC lane makes of the
    // same bearer, so a token good there is good here.
    let bearer = crate::watch_bridge::bearer(&headers);
    let who = org
        .permissions
        .identity_resolver()
        .resolve(bearer.as_deref())
        .await;
    let person = match &who {
        Principal::User { user_id } => user_id.parse::<uuid::Uuid>().ok().map(PrincipalId::new),
        _ => None,
    };
    let Some(person) = person else {
        wide::set("files.docs_outcome", "unauthenticated");
        tracing::warn!(org.slug = %slug, "files docs: refused a caller with no account");
        return (StatusCode::UNAUTHORIZED, "sign in to read this root").into_response();
    };
    wide::set("auth.principal_kind", "user");
    let paths: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    if paths.len() > BATCH_PATHS_MAX {
        wide::set("files.docs_outcome", "too-many");
        return (StatusCode::PAYLOAD_TOO_LARGE, "too many paths in one batch").into_response();
    }
    let head = match org.files.head_commit_hex(root_id).await {
        Ok(head) => head,
        Err(e) => {
            wide::set("files.docs_outcome", "no-root");
            return (StatusCode::NOT_FOUND, format!("root: {e}")).into_response();
        }
    };
    // Read where the files lane would let them: a role that reads covers
    // the whole root; otherwise each path by its grants.
    let role_reads = baseline_as(&org.files, &Caller::Person(person))
        .await
        .contains(&Capability::Read);
    let subject = Subject::Person(person);
    let (allowed, denied): (Vec<String>, Vec<String>) = paths.into_iter().partition(|path| {
        role_reads
            || RootPath::parse(path).is_ok_and(|p| {
                org.files
                    .authorise(&subject, RootId::new(root_id), &p, Capability::Read)
                    .is_ok()
            })
    });
    let scope = FilesScope {
        root_id,
        subpath: String::new(),
        at: Some(head),
        file_only: None,
    };
    let (mut out, served, refused) = documents_batch(&org, &scope, allowed, |_| {}).await;
    for path in &denied {
        push_record(&mut out, BATCH_NOT_FOUND, path, &[]);
    }
    wide::set("files.docs_outcome", "documents-batch");
    wide::set("files.docs_served", served);
    wide::set(
        "files.docs_refused",
        refused + i64::try_from(denied.len()).unwrap_or(i64::MAX),
    );
    wide::set(
        "files.docs_len",
        i64::try_from(out.len()).unwrap_or(i64::MAX),
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        out,
    )
        .into_response()
}
