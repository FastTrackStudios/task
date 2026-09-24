//! `task files put / get / render` and `task files root ensure` — moving
//! bytes in and out of a File Root from the shell, the way an app does
//! it through `files-client` (ADR 0005).
//!
//! ```text
//! task files root ensure session/always-on-time --name "Always On Time"
//! task files put <root-id> ~/Sessions/"Always On Time"          # a folder, recursively
//! task files render <root-id> Media --kind audio               # the proxies, ahead of the first play
//! task files get <root-id> "Always On Time.RPP" -o session.RPP
//! ```
//!
//! A save is create-only unless `--save` says otherwise, so a second run
//! over the same folder lands only what is new and reports the rest as
//! already there; `--save replace --etag <etag>` is the safe edit, and
//! refuses when the file moved on since that etag (`Stale`). Files move
//! streamed, never whole in memory.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use files_client::{FilesClient, Save};
use files_proto::error::FilesFault;
use files_proto::id::ContentId;
use files_proto::{RenditionKind, RootFlavor, RootId, RootPath, TreeServiceClient};

use crate::establish_for_url;

/// How a save lands.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum SaveArg {
    /// Only where nothing is — the default; existing files are skipped.
    Create,
    /// Only over the content `--etag` names; refused as stale otherwise.
    Replace,
    /// Always; the previous content stays in the version history.
    Overwrite,
    /// Always, beside whatever is there.
    KeepBoth,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum RenditionArg {
    /// The streaming audio proxy (AAC).
    Audio,
    /// Waveform peaks.
    Peaks,
    Proxy720,
    Proxy1080,
    Filmstrip,
}

impl From<RenditionArg> for RenditionKind {
    fn from(kind: RenditionArg) -> Self {
        match kind {
            RenditionArg::Audio => Self::Audio,
            RenditionArg::Peaks => Self::Peaks,
            RenditionArg::Proxy720 => Self::Proxy720,
            RenditionArg::Proxy1080 => Self::Proxy1080,
            RenditionArg::Filmstrip => Self::Filmstrip,
        }
    }
}

/// `task files put`.
#[derive(Args, Debug)]
pub(crate) struct PutArgs {
    pub root_id: uuid::Uuid,
    /// A file, or a folder to upload recursively.
    pub local: PathBuf,
    /// Where in the root. Defaults to the file's name, or — for a folder
    /// — the root itself, so the folder's contents land at the top.
    #[arg(long)]
    pub to: Option<String>,
    #[arg(long, value_enum, default_value_t = SaveArg::Create)]
    pub save: SaveArg,
    /// The etag `--save replace` expects to find (what `put` printed
    /// last time, or `task files entry`).
    #[arg(long)]
    pub etag: Option<String>,
    #[arg(long)]
    pub json: bool,
}

/// `task files get`.
#[derive(Args, Debug)]
pub(crate) struct GetArgs {
    pub root_id: uuid::Uuid,
    /// Root-relative path.
    pub path: String,
    /// Where to write it; `-` (the default) is stdout.
    #[arg(short, long, default_value = "-")]
    pub output: PathBuf,
}

/// `task files render`.
#[derive(Args, Debug)]
pub(crate) struct RenderArgs {
    pub root_id: uuid::Uuid,
    /// A file, or a folder whose media is rendered recursively.
    #[arg(default_value = "")]
    pub path: String,
    #[arg(long, value_enum, default_value_t = RenditionArg::Audio)]
    pub kind: RenditionArg,
    #[arg(long)]
    pub json: bool,
}

/// The five typed clients `files-client` holds, dialled at the org.
async fn files_at(vox_url: &str) -> eyre::Result<FilesClient> {
    Ok(FilesClient::new(
        establish_for_url(vox_url).await?,
        establish_for_url(vox_url).await?,
        establish_for_url(vox_url).await?,
        establish_for_url(vox_url).await?,
        establish_for_url(vox_url).await?,
    ))
}

/// `task files root ensure`: the root at `dir` in the org's files area,
/// made if it does not exist. Safe to run every time.
pub(crate) async fn ensure_root(
    vox_url: &str,
    dir: &str,
    name: Option<String>,
    flavor: RootFlavor,
    json: bool,
) -> eyre::Result<()> {
    let files = files_at(vox_url).await?;
    let name = name.unwrap_or_else(|| {
        dir.trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(dir)
            .to_owned()
    });
    let root = files
        .ensure_root(dir, &name, flavor)
        .await
        .map_err(|e| eyre::eyre!("ensure root: {e}"))?;
    if json {
        println!(
            "{}",
            facet_json::to_string(&root).map_err(|e| eyre::eyre!("{e}"))?
        );
    } else {
        println!("{}  {}", root.id, root.name);
    }
    Ok(())
}

pub(crate) async fn put(vox_url: &str, args: PutArgs) -> eyre::Result<()> {
    let files = files_at(vox_url).await?;
    let root = RootId::new(args.root_id);
    let save = || -> eyre::Result<Save> {
        Ok(match args.save {
            SaveArg::Create => Save::create_only(),
            SaveArg::Replace => {
                let etag = args
                    .etag
                    .clone()
                    .ok_or_else(|| eyre::eyre!("--save replace needs the --etag it replaces"))?;
                Save::replacing(ContentId(etag))
            }
            SaveArg::Overwrite => Save::overwrite(),
            SaveArg::KeepBoth => Save::keep_both(),
        })
    };
    let meta =
        std::fs::metadata(&args.local).map_err(|e| eyre::eyre!("{}: {e}", args.local.display()))?;
    let uploads: Vec<(PathBuf, String)> = if meta.is_dir() {
        let base = args
            .to
            .as_deref()
            .unwrap_or("")
            .trim_matches('/')
            .to_owned();
        let mut found = Vec::new();
        walk(&args.local, &args.local, &mut found)?;
        found
            .into_iter()
            .map(|(local, rel)| {
                let dest = if base.is_empty() {
                    rel
                } else {
                    format!("{base}/{rel}")
                };
                (local, dest)
            })
            .collect()
    } else {
        let name = args
            .local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        vec![(args.local.clone(), args.to.clone().unwrap_or(name))]
    };

    let (mut landed, mut skipped) = (0usize, 0usize);
    for (local, dest) in uploads {
        let size = std::fs::metadata(&local)
            .map_err(|e| eyre::eyre!("{}: {e}", local.display()))?
            .len();
        let reader = futures::io::AllowStdIo::new(
            std::fs::File::open(&local).map_err(|e| eyre::eyre!("{}: {e}", local.display()))?,
        );
        match files.put_reader(root, &dest, size, reader, save()?).await {
            Ok(entry) => {
                landed += 1;
                let etag = entry.content.map(|c| c.0).unwrap_or_default();
                if args.json {
                    println!(r#"{{"path":{dest:?},"size":{size},"etag":{etag:?}}}"#);
                } else {
                    println!("{dest}  {size} B  {etag}");
                }
            }
            Err(files_client::ClientError::Fault(FilesFault::Exists { .. })) => {
                skipped += 1;
                if !args.json {
                    println!("{dest}  already there — skipped (--save overwrite replaces it)");
                }
            }
            Err(e) if e.is_stale() => {
                return Err(eyre::eyre!(
                    "{dest}: changed since {} — fetch it, or pass its current etag",
                    args.etag.as_deref().unwrap_or("that etag")
                ));
            }
            Err(e) => return Err(eyre::eyre!("{dest}: {e}")),
        }
    }
    if !args.json {
        eprintln!("{landed} saved, {skipped} already there");
    }
    Ok(())
}

/// Every file under `dir`, with its path relative to `base` (`/`
/// separated). Dotfiles are left behind — `.DS_Store`, editor swap files.
fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, String)>) -> eyre::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| eyre::eyre!("{}: {e}", dir.display()))?
        .collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map_err(|e| eyre::eyre!("{e}"))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((path, rel));
        }
    }
    Ok(())
}

pub(crate) async fn get(vox_url: &str, args: GetArgs) -> eyre::Result<()> {
    use std::io::Write as _;
    let files = files_at(vox_url).await?;
    let root = RootId::new(args.root_id);
    let mut out: Box<dyn std::io::Write> = if args.output.as_os_str() == "-" {
        Box::new(std::io::stdout().lock())
    } else {
        Box::new(std::io::BufWriter::new(
            std::fs::File::create(&args.output)
                .map_err(|e| eyre::eyre!("{}: {e}", args.output.display()))?,
        ))
    };
    let mut failed = None;
    files
        .read_to(root, &args.path, |chunk| {
            if failed.is_none()
                && let Err(e) = out.write_all(chunk)
            {
                failed = Some(e);
            }
        })
        .await
        .map_err(|e| eyre::eyre!("{}: {e}", args.path))?;
    if let Some(e) = failed {
        return Err(eyre::eyre!("writing {}: {e}", args.output.display()));
    }
    out.flush()?;
    Ok(())
}

/// Media a rendition is made from, by name.
fn is_media(name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "wav"
            | "aif"
            | "aiff"
            | "flac"
            | "mp3"
            | "ogg"
            | "m4a"
            | "aac"
            | "caf"
            | "mp4"
            | "mov"
            | "mkv"
            | "webm"
    )
}

pub(crate) async fn render(vox_url: &str, args: RenderArgs) -> eyre::Result<()> {
    let files = files_at(vox_url).await?;
    let tree: TreeServiceClient = establish_for_url(vox_url).await?;
    let root = RootId::new(args.root_id);
    let path = args.path.trim_matches('/').to_owned();
    // A folder when it browses; otherwise the one file.
    let mut todo = Vec::new();
    let mut folders = vec![path.clone()];
    let mut is_folder = true;
    while let Some(folder) = folders.pop() {
        let parsed = RootPath::parse(&folder).map_err(|e| eyre::eyre!("`{folder}`: {e}"))?;
        match tree.browse(root, parsed).await {
            Ok(entries) => {
                for entry in entries {
                    let child = if folder.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{folder}/{}", entry.name)
                    };
                    if entry.is_dir {
                        folders.push(child);
                    } else if is_media(&entry.name) {
                        todo.push(child);
                    }
                }
            }
            Err(_) if folder == path && !path.is_empty() => {
                is_folder = false;
                todo.push(path.clone());
            }
            Err(e) => return Err(eyre::eyre!("browse {folder}: {e}")),
        }
    }
    if is_folder {
        todo.sort();
    }
    let kind = RenditionKind::from(args.kind);
    for file in todo {
        let parsed = RootPath::parse(&file).map_err(|e| eyre::eyre!("`{file}`: {e}"))?;
        let info = files
            .media
            .rendition_info(root, parsed, kind, None)
            .await
            .map_err(|e| eyre::eyre!("{file}: {e}"))?;
        if args.json {
            println!(
                "{}",
                facet_json::to_string(&info).map_err(|e| eyre::eyre!("{e}"))?
            );
        } else {
            println!("{file}  {} B  {}", info.len, info.mime);
        }
    }
    Ok(())
}
