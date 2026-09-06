//! What `task patch`, `task sample` and `task lighting` share.
//!
//! The three asset lanes ADR 0003 adds beside `task chart` differ only
//! in what their frontmatter says; how a body is read off the shell,
//! and how the client is established, is the same question three times.
//! Answered once here so the lane modules stay about their own kind.

use std::io::Read as _;
use std::path::PathBuf;

use resources_proto::{ContentRef, ResourcesServiceClient};

use crate::{establish_for_url, resolve_active_org, resolve_org_vox_url};

/// `(slug, body)` for a save: `--from` names the body file and the
/// positional argument is the slug; without it the positional argument
/// *is* the body file and its stem is the slug. `--from -` reads stdin,
/// so an asset round-trips through a pipe.
pub fn read_body(slug_or_file: &str, from: Option<&str>) -> eyre::Result<(String, String)> {
    match from {
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Ok((slug_or_file.to_string(), buf))
        }
        Some(path) => Ok((slug_or_file.to_string(), read_file(path.into())?)),
        None => {
            let path = PathBuf::from(slug_or_file);
            let slug = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((slug, read_file(path)?))
        }
    }
}

fn read_file(path: PathBuf) -> eyre::Result<String> {
    std::fs::read_to_string(&path).map_err(|e| eyre::eyre!("reading `{}`: {e}", path.display()))
}

/// The File Root binding a `--content-root` / `--content-path` pair
/// names. Both empty means "no bytes bound", which is the ordinary
/// state of a freshly declared asset — the manifest says what a thing
/// is, the Files layer owns its content.
#[must_use]
pub fn content_ref(root: Option<String>, path: Option<String>) -> ContentRef {
    ContentRef {
        root_id: root.unwrap_or_default(),
        path: path.unwrap_or_default(),
    }
}

/// How a bound asset prints in a list: the root and path, or a dash.
#[must_use]
pub fn content_cell(content: &ContentRef) -> String {
    if content.is_bound() {
        format!("{}:{}", content.root_id, content.path)
    } else {
        "-".to_owned()
    }
}

pub async fn client(
    org: Option<String>,
    server: Option<String>,
) -> eyre::Result<ResourcesServiceClient> {
    let slug = resolve_active_org(org)?;
    establish_for_url(&resolve_org_vox_url(server, &slug)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_file_names_its_own_slug() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("warm-analog-pad.json");
        std::fs::write(&json, "{\"blocks\":[]}").unwrap();
        let (slug, body) = read_body(&json.to_string_lossy(), None).unwrap();
        assert_eq!(slug, "warm-analog-pad");
        assert_eq!(body, "{\"blocks\":[]}");

        // With `--from`, the positional argument is the slug.
        let (slug, _) = read_body("other-name", Some(&json.to_string_lossy())).unwrap();
        assert_eq!(slug, "other-name");
    }

    #[test]
    fn an_unbound_asset_prints_a_dash_rather_than_a_half_path() {
        assert_eq!(content_cell(&content_ref(None, None)), "-");
        assert_eq!(
            content_cell(&content_ref(Some("lib".into()), None)),
            "-",
            "a root with no path names nothing"
        );
        assert_eq!(
            content_cell(&content_ref(Some("lib".into()), Some("a.wav".into()))),
            "lib:a.wav"
        );
    }
}
