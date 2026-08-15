//! Local whisper transcription (feature `whisper`).
//!
//! Builds on whisper-rs — NOTE the repo moved to
//! <https://codeberg.org/tazz4843/whisper-rs> (the GitHub
//! repo is archived); the Cargo dep is pinned to a codeberg
//! rev. Compiling this feature builds whisper.cpp via cmake,
//! which is why it is OFF by default: the cheap paths
//! (`<podcast:transcript>` tag, Groq backfill) cover most
//! episodes without it.
//!
//! ## Model management
//!
//! Models are ggml files from
//! `huggingface.co/ggerganov/whisper.cpp`. `--whisper-model`
//! (or `TASK_WHISPER_MODEL`) takes either a file path or a
//! model name; names are cached under
//! `~/.cache/task/whisper/ggml-<name>.bin` and downloaded on
//! first use.
//!
//! - `small` — the dev default (~466 MB, fine for English).
//! - `large-v3-turbo` — the production pick (~1.6 GB,
//!   near-large quality at ~8× speed). Pass
//!   `--whisper-model large-v3-turbo`.
//!
//! ## Timing
//!
//! whisper.cpp reports segment `t0`/`t1` in 10 ms units —
//! conversion lives in [`crate::transcript::segments_to_cues`]
//! (pure, tested without this feature). Token-level
//! timestamps are enabled (`set_token_timestamps`) which
//! measurably improves segment-boundary accuracy.
//!
//! ## Audio decode
//!
//! whisper wants 16 kHz mono f32 PCM; enclosures are mp3/m4a.
//! Decoding shells out to `ffmpeg` (`TASK_FFMPEG`) — pulling
//! a pure-Rust decoder matrix in for every enclosure codec is
//! not worth it for a feature-gated path.

use std::path::PathBuf;

use crate::ArchiveError;
use crate::youtube::Cue;

/// Default model when none is configured.
pub const DEFAULT_MODEL: &str = "small";

/// Resolve a model spec (name or path) to a local file path
/// WITHOUT downloading. Pure.
#[must_use]
pub fn model_path(spec: &str) -> PathBuf {
    let as_path = PathBuf::from(spec);
    let has_bin_ext = as_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bin"));
    if as_path.is_absolute() || spec.contains('/') || has_bin_ext {
        return as_path;
    }
    cache_dir().join(format!("ggml-{spec}.bin"))
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("task")
        .join("whisper")
}

/// Download URL for a named model.
#[must_use]
pub fn model_url(name: &str) -> String {
    format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{name}.bin")
}

/// Ensure the model exists locally, downloading by name when
/// missing. A path spec that doesn't exist is an error (we
/// never guess what to download for a path).
pub async fn ensure_model(client: &reqwest::Client, spec: &str) -> Result<PathBuf, ArchiveError> {
    let path = model_path(spec);
    if path.exists() {
        return Ok(path);
    }
    let looks_like_name = path.starts_with(cache_dir());
    if !looks_like_name {
        return Err(ArchiveError::Extract {
            url: String::new(),
            message: format!("whisper model not found at {}", path.display()),
        });
    }
    let url = model_url(spec);
    tracing::info!(%url, "downloading whisper model (first use)");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| ArchiveError::Fetch {
            url: url.clone(),
            message: e.to_string(),
        })?;
    if !resp.status().is_success() {
        return Err(ArchiveError::BadResponse {
            url,
            message: format!("HTTP {} (unknown model name `{spec}`?)", resp.status()),
        });
    }
    let bytes = resp.bytes().await.map_err(|e| ArchiveError::Fetch {
        url: url.clone(),
        message: e.to_string(),
    })?;
    let tmp = path.with_extension("bin.partial");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Decode arbitrary enclosure audio to 16 kHz mono f32 PCM
/// via an ffmpeg subprocess.
pub async fn decode_to_pcm(audio: &[u8], ffmpeg: &str) -> Result<Vec<f32>, ArchiveError> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new(ffmpeg)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
            "-f",
            "f32le",
            "-ac",
            "1",
            "-ar",
            "16000",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ArchiveError::Subprocess {
            program: ffmpeg.to_string(),
            message: if e.kind() == std::io::ErrorKind::NotFound {
                "ffmpeg not found — install it or set TASK_FFMPEG".to_string()
            } else {
                format!("spawn: {e}")
            },
        })?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let audio_owned = audio.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&audio_owned).await;
    });
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| ArchiveError::Subprocess {
            program: ffmpeg.to_string(),
            message: format!("wait: {e}"),
        })?;
    writer.abort();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(ArchiveError::Subprocess {
            program: ffmpeg.to_string(),
            message: format!(
                "exit {}: {}",
                out.status,
                stderr.lines().last().unwrap_or("(no stderr)")
            ),
        });
    }
    let pcm: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    if pcm.is_empty() {
        return Err(ArchiveError::Extract {
            url: String::new(),
            message: "ffmpeg produced no samples".into(),
        });
    }
    Ok(pcm)
}

/// Run whisper over PCM samples. Blocking (model inference) —
/// call via `spawn_blocking`. Returns `(t0_10ms, text)`
/// segment pairs for [`crate::transcript::segments_to_cues`].
pub fn transcribe_pcm(
    model: &std::path::Path,
    pcm: &[f32],
) -> Result<Vec<(i64, String)>, ArchiveError> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default()).map_err(
        |e| ArchiveError::Extract {
            url: String::new(),
            message: format!("whisper model load: {e}"),
        },
    )?;
    let mut state = ctx.create_state().map_err(|e| ArchiveError::Extract {
        url: String::new(),
        message: format!("whisper state: {e}"),
    })?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    // Word/token timestamps sharpen segment boundaries — the
    // `^t<sec>` anchors come straight from segment t0.
    params.set_token_timestamps(true);
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);

    state.full(params, pcm).map_err(|e| ArchiveError::Extract {
        url: String::new(),
        message: format!("whisper full: {e}"),
    })?;

    let n = state.full_n_segments();
    let mut segments = Vec::with_capacity(n.max(0) as usize);
    for i in 0..n {
        let Some(seg) = state.get_segment(i) else {
            continue;
        };
        let text = seg
            .to_str_lossy()
            .map_err(|e| ArchiveError::Extract {
                url: String::new(),
                message: format!("whisper segment text: {e}"),
            })?
            .into_owned();
        // `start_timestamp` is centiseconds (10 ms units) —
        // exactly what `segments_to_cues` expects.
        segments.push((seg.start_timestamp(), text));
    }
    Ok(segments)
}

/// Convenience: enclosure bytes → anchored cues.
pub async fn transcribe_enclosure(
    client: &reqwest::Client,
    model_spec: &str,
    ffmpeg: &str,
    audio: Vec<u8>,
) -> Result<Vec<Cue>, ArchiveError> {
    let model = ensure_model(client, model_spec).await?;
    let pcm = decode_to_pcm(&audio, ffmpeg).await?;
    let segments = tokio::task::spawn_blocking(move || transcribe_pcm(&model, &pcm))
        .await
        .map_err(|e| ArchiveError::Extract {
            url: String::new(),
            message: format!("whisper task join: {e}"),
        })??;
    Ok(crate::transcript::segments_to_cues(segments))
}
