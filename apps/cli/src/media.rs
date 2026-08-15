//! `task media …` — content-addressed media streamed over vox.
//!
//! The binary↔binary E2E surface for `MediaService`: stat a blob,
//! stream it to a file/stdout, or verify every stem on a `type: song`
//! note end-to-end (stream over the real vox wire, sha256 the bytes,
//! compare to the frontmatter `content_hash`, report throughput). No
//! browser required — this is how audio streaming gets smoke-tested
//! against a live task-server:
//!
//! ```bash
//! task song ingest Songs/Praise.md --stems ./stems
//! task media verify-song Songs/Praise.md      # streams + hashes all stems
//! task media get <hash> --out stem.ogg        # or pipe to a player
//! ```

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use clap::Subcommand;
use media_proto::{MediaChunk, MediaServiceClient};
use sha2::{Digest, Sha256};

use crate::errors;

#[derive(Subcommand)]
pub enum MediaCmd {
    /// Size + mime for a content-addressed blob.
    Stat {
        /// sha256 hex content hash.
        hash: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Stream a blob over vox to a file (or stdout with no --out).
    Get {
        /// sha256 hex content hash.
        hash: String,
        /// Write here; stdout when omitted.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Byte offset to start from.
        #[arg(long, default_value_t = 0)]
        start: u64,
        /// Bytes to read (default: to the end).
        #[arg(long)]
        len: Option<u64>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Stream EVERY stem on a `type: song` note over vox and verify
    /// each one's bytes hash back to its frontmatter `content_hash`.
    /// The no-browser audio-streaming E2E.
    VerifySong {
        /// Vault-relative note path (e.g. `Songs/Praise.md`).
        note: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run_media(cmd: MediaCmd) -> eyre::Result<()> {
    match cmd {
        MediaCmd::Stat {
            hash,
            org,
            server,
            json,
        } => {
            let client = client(org.as_deref(), server).await?;
            let info = client
                .stat(hash)
                .await
                .map_err(|e| eyre::eyre!("stat: {e:?}"))?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "content_hash": info.content_hash,
                        "size_bytes": info.size_bytes,
                        "mime_type": info.mime_type,
                        "filename": info.filename,
                    })
                );
            } else {
                println!(
                    "{}  {} bytes  {}  {}",
                    info.content_hash, info.size_bytes, info.mime_type, info.filename
                );
            }
        }
        MediaCmd::Get {
            hash,
            out,
            start,
            len,
            org,
            server,
        } => {
            let client = client(org.as_deref(), server).await?;
            let t0 = Instant::now();
            let bytes = stream_window(&client, &hash, start, len.unwrap_or(u64::MAX)).await?;
            let secs = t0.elapsed().as_secs_f64().max(1e-9);
            match out {
                Some(path) => {
                    std::fs::write(&path, &bytes)
                        .map_err(|e| eyre::eyre!("write {}: {e}", path.display()))?;
                    eprintln!(
                        "{} bytes → {} ({:.1} MB/s)",
                        bytes.len(),
                        path.display(),
                        bytes.len() as f64 / 1e6 / secs,
                    );
                }
                None => std::io::stdout().write_all(&bytes)?,
            }
        }
        MediaCmd::VerifySong {
            note,
            org,
            server,
            json,
        } => {
            let active = crate::org_ctx::resolve_active(org.as_deref())?;
            let note_path = active.root.vault_dir().join(note.trim_start_matches('/'));
            let text = std::fs::read_to_string(&note_path)
                .map_err(|e| eyre::eyre!("read {}: {e}", note_path.display()))?;
            let stems = stems_from_frontmatter(&text);
            if stems.is_empty() {
                return Err(errors::usage("media verify-song")
                    .cause(format!("no `stems:` with content_hash in {note}"))
                    .hint("ingest first: task song ingest <note> --stems DIR")
                    .report());
            }
            let client = client(org.as_deref(), server).await?;
            let mut results = Vec::new();
            let mut all_ok = true;
            for (name, expected) in &stems {
                let t0 = Instant::now();
                let bytes = stream_window(&client, expected, 0, u64::MAX).await?;
                let secs = t0.elapsed().as_secs_f64().max(1e-9);
                let mut h = Sha256::new();
                h.update(&bytes);
                let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
                let ok = &got == expected;
                all_ok &= ok;
                if !json {
                    println!(
                        "{}  {:<24} {:>9} bytes  {:>6.1} MB/s  {}",
                        if ok { "ok " } else { "BAD" },
                        name,
                        bytes.len(),
                        bytes.len() as f64 / 1e6 / secs,
                        &expected[..12.min(expected.len())],
                    );
                }
                results.push(serde_json::json!({
                    "name": name, "content_hash": expected, "ok": ok,
                    "bytes": bytes.len(), "secs": secs,
                }));
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "note": note, "ok": all_ok, "stems": results })
                );
            }
            if !all_ok {
                return Err(eyre::eyre!("stem hash mismatch — see output"));
            }
            if !json {
                println!("{} stems verified over vox", stems.len());
            }
        }
    }
    Ok(())
}

async fn client(
    org: Option<&str>,
    server: Option<String>,
) -> eyre::Result<MediaServiceClient> {
    let active = crate::org_ctx::resolve_active(org)?;
    let slug = active.root.slug().to_string();
    crate::establish_client::<MediaServiceClient>(server, &slug).await
}

/// Stream one read window into a contiguous buffer, asserting the
/// chunks arrive in order.
async fn stream_window(
    client: &MediaServiceClient,
    hash: &str,
    start: u64,
    len: u64,
) -> eyre::Result<Vec<u8>> {
    let (tx, mut rx) = vox::channel::<MediaChunk>();
    let c = client.clone();
    let h = hash.to_string();
    let reader = tokio::spawn(async move { c.read(h, start, len, tx).await });
    let mut got: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = rx.recv().await {
        let c = chunk.get();
        eyre::ensure!(
            c.offset == start + got.len() as u64,
            "out-of-order chunk at offset {}",
            c.offset
        );
        got.extend_from_slice(&c.bytes);
    }
    reader
        .await?
        .map_err(|e| eyre::eyre!("media read: {e:?}"))?;
    Ok(got)
}

/// Minimal `(name, content_hash)` scan of a song note's `stems:`
/// frontmatter block (same shape `task song ingest` writes).
fn stems_from_frontmatter(text: &str) -> Vec<(String, String)> {
    let Some(rest) = text.strip_prefix("---") else {
        return Vec::new();
    };
    let Some((front, _)) = rest.split_once("\n---") else {
        return Vec::new();
    };
    let clean = |s: &str| s.trim().trim_matches(['"', '\'']).trim().to_owned();
    let mut out = Vec::new();
    let mut in_stems = false;
    let mut name: Option<String> = None;
    for line in front.lines() {
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if line.trim_end() == "stems:" {
            in_stems = true;
            continue;
        }
        if in_stems && !indented && !line.trim().is_empty() {
            break;
        }
        if !in_stems {
            continue;
        }
        let t = line.trim_start().trim_start_matches('-').trim_start();
        if let Some(v) = t.strip_prefix("name:") {
            name = Some(clean(v));
        } else if let Some(v) = t.strip_prefix("content_hash:") {
            if let Some(n) = name.take() {
                out.push((n, clean(v)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::stems_from_frontmatter;

    #[test]
    fn scans_ingested_shape() {
        let note = "---\ntype: song\nstems:\n  - name: \"Click\"\n    group: Guide\n    default_muted: true\n    content_hash: aaa\n  - name: \"Bass\"\n    content_hash: bbb\nduration_sec: 12\n---\nbody";
        assert_eq!(
            stems_from_frontmatter(note),
            vec![("Click".into(), "aaa".into()), ("Bass".into(), "bbb".into())]
        );
    }
}
