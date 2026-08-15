//! Generic state-file helpers (load / atomic-save) used by
//! [`queue`](crate::queue), and any future stateful module.

use std::fs;
use std::io::Write;

use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::vault::WikiLive;

/// JSON state files live under `Wiki/_state/<FILENAME>`.
pub(crate) trait StateFile: Serialize + DeserializeOwned + Default {
    const FILENAME: &'static str;
}

impl WikiLive {
    pub(crate) fn load_state<S: StateFile>(&self) -> Result<S, WikiLiveError> {
        let path = self.wiki_root().join(paths::STATE_DIR).join(S::FILENAME);
        if !path.is_file() {
            return Ok(S::default());
        }
        let bytes = fs::read(&path)?;
        if bytes.is_empty() {
            return Ok(S::default());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub(crate) fn save_state<S: StateFile>(&self, state: &S) -> Result<(), WikiLiveError> {
        let dir = self.wiki_root().join(paths::STATE_DIR);
        fs::create_dir_all(&dir)?;
        let path = dir.join(S::FILENAME);
        let body = serde_json::to_vec_pretty(state)?;
        atomic_write::<S>(&path, &body)
    }
}

/// Atomic write via temp+rename. Type-erased helper so
/// multiple state files share one impl.
pub(crate) fn atomic_write<S>(path: &std::path::Path, bytes: &[u8]) -> Result<(), WikiLiveError> {
    let _ = std::marker::PhantomData::<S>;
    let tmp = path.with_extension(format!("tmp.{}", Uuid::new_v4().simple()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
