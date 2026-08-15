//! The real ffmpeg/ffprobe [`Transcoder`] (issue #269), behind the
//! `ffmpeg` feature. Each rendition is one `ffmpeg` invocation writing
//! to a temp output the pipeline then ingests into the CAS. `probe`
//! uses `ffprobe` to classify the source. Not unit-tested — it needs
//! the ffmpeg toolchain and real media — so the pipeline's caching / GC
//! / recipe logic is proven against the deterministic `FakeTranscoder`
//! instead; this driver is the thin adapter that the server wires in.

use std::path::Path;

use tokio::process::Command;

use crate::error::{Error, Result};
use crate::recipe::{MediaClass, RenditionKind};
use crate::transcoder::Transcoder;

/// Shells out to `ffmpeg` / `ffprobe` on `PATH`.
#[derive(Debug, Default, Clone)]
pub struct FfmpegTranscoder;

#[async_trait::async_trait]
impl Transcoder for FfmpegTranscoder {
    async fn probe(&self, source: &Path) -> Result<MediaClass> {
        // Ask ffprobe for the stream types; a video stream ⇒ Video, else
        // an audio stream ⇒ Audio, else Other.
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "default=nw=1:nk=1",
            ])
            .arg(source)
            .output()
            .await
            .map_err(|e| Error::Transcode(format!("ffprobe: {e}")))?;
        if !out.status.success() {
            return Err(Error::Transcode(format!(
                "ffprobe failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        let types = String::from_utf8_lossy(&out.stdout);
        Ok(if types.lines().any(|l| l.trim() == "video") {
            MediaClass::Video
        } else if types.lines().any(|l| l.trim() == "audio") {
            MediaClass::Audio
        } else {
            MediaClass::Other
        })
    }

    async fn generate(&self, kind: RenditionKind, source: &Path) -> Result<Vec<u8>> {
        let dir = tempfile::tempdir()?;
        let out = dir.path().join(match kind {
            RenditionKind::Proxy1080 | RenditionKind::Proxy720 => "out.mp4",
            RenditionKind::Audio => "out.m4a",
            RenditionKind::Peaks => "out.pcm",
            RenditionKind::Filmstrip => "out.jpg",
        });
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y").arg("-i").arg(source);
        match kind {
            RenditionKind::Proxy1080 => {
                cmd.args([
                    "-vf",
                    "scale=-2:1080",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                ])
                .args(["-crf", "23", "-c:a", "aac", "-movflags", "+faststart"]);
            }
            RenditionKind::Proxy720 => {
                cmd.args([
                    "-vf",
                    "scale=-2:720",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                ])
                .args(["-crf", "23", "-c:a", "aac", "-movflags", "+faststart"]);
            }
            RenditionKind::Audio => {
                cmd.args([
                    "-vn",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "192k",
                    "-movflags",
                    "+faststart",
                ]);
            }
            RenditionKind::Peaks => {
                // Waveform peaks as raw PCM the caller reduces to a peak
                // array — simplest portable extraction; the JSON shaping
                // is the pipeline's job in a later pass. For v1 we emit
                // the loudnorm-normalized mono s16le stream.
                cmd.args(["-vn", "-ac", "1", "-ar", "8000", "-f", "s16le"]);
            }
            RenditionKind::Filmstrip => {
                // One row of thumbnails at a fixed cadence, tiled.
                cmd.args(["-vf", "fps=1/10,scale=160:-1,tile=10x1"])
                    .args(["-frames:v", "1"]);
            }
        }
        cmd.arg(&out);
        let status = cmd
            .status()
            .await
            .map_err(|e| Error::Transcode(format!("ffmpeg: {e}")))?;
        if !status.success() {
            return Err(Error::Transcode(format!(
                "ffmpeg {} exited {status}",
                kind.tag()
            )));
        }
        Ok(tokio::fs::read(&out).await?)
    }
}
