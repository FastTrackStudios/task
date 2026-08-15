//! MediaSource-backed `<audio>` fed from `MediaService` over vox.
//!
//! The playback half of media-over-vox (see
//! `apps/task/plans/media-over-vox.md`): instead of pointing an
//! `HTMLAudioElement` at an HTTP URL, the element's media comes from a
//! `MediaSource` whose single `SourceBuffer` is appended from
//! [`MediaChunk`]s streamed through the org's vox lane. Same origin,
//! same auth, one protocol — no signed-URL side-channel.
//!
//! Browser constraint: MSE reliably accepts `audio/webm; codecs="opus"`
//! (Chrome + Firefox) but NOT ogg-opus, so only stems ingested as webm
//! take this path; callers fall back to the signed-URL element for
//! anything else (see `song_session::resolve_front`).
//!
//! v1 strategy: progressively append the WHOLE stem from byte 0 —
//! worship stems at 96 kbps are a few MB each, playback starts as soon
//! as the first chunks land, and seeks stay within the buffered range
//! once the tail arrives. Windowed/eviction-aware buffering is a
//! follow-up (the plan's step 5).

#![cfg(target_arch = "wasm32")]

use media_proto::MediaChunk;
use web_sys::{HtmlAudioElement, MediaSource, MediaSourceReadyState, SourceBuffer, Url};

/// The one MSE mime this path speaks.
pub(crate) const WEBM_OPUS: &str = "audio/webm; codecs=\"opus\"";

/// Whether this browser can take the vox-MSE path at all.
pub(crate) fn mse_supported() -> bool {
    MediaSource::is_type_supported(WEBM_OPUS)
}

/// Create an `<audio>` element whose media streams from `hash` over the
/// org's vox lane. Returns the element immediately; a spawned task
/// establishes the client, opens the `SourceBuffer`, and appends chunks
/// as they arrive. Errors after creation surface as a media error on
/// the element (players already tolerate stalled/failed stems).
pub(crate) fn audio_element_over_vox(
    org: String,
    hash: String,
) -> Result<HtmlAudioElement, String> {
    let el = HtmlAudioElement::new().map_err(|e| format!("audio element: {e:?}"))?;
    el.set_preload("auto");
    el.set_loop(false);

    let ms = MediaSource::new().map_err(|e| format!("MediaSource: {e:?}"))?;
    let src =
        Url::create_object_url_with_source(&ms).map_err(|e| format!("object url: {e:?}"))?;
    el.set_src(&src);

    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = feed(ms, org, hash.clone()).await {
            tracing::warn!(%hash, e, "vox media stream failed");
        }
        // The object URL is only needed until the element resolves it.
        let _ = Url::revoke_object_url(&src);
    });
    Ok(el)
}

/// Wait until `cond` returns true, yielding to the browser between
/// polls. MSE state flips (readyState, updating) have no async API, so
/// short polls beat juggling one-shot event closures.
async fn poll_until(mut cond: impl FnMut() -> bool) {
    while !cond() {
        gloo_timers::future::TimeoutFuture::new(5).await;
    }
}

async fn feed(ms: MediaSource, org: String, hash: String) -> Result<(), String> {
    // The SourceBuffer can only be added once the element has attached
    // the MediaSource (readyState = open).
    poll_until(|| ms.ready_state() == MediaSourceReadyState::Open).await;
    let sb: SourceBuffer = ms
        .add_source_buffer(WEBM_OPUS)
        .map_err(|e| format!("addSourceBuffer: {e:?}"))?;

    let client = crate::media_client(&org).await?;
    let (tx, mut rx) = vox::channel::<MediaChunk>();
    let h = hash.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = client.read(h.clone(), 0, u64::MAX, tx).await {
            tracing::warn!(hash = %h, ?e, "MediaService read ended with error");
        }
    });

    while let Ok(Some(chunk)) = rx.recv().await {
        let mut bytes = chunk.get().bytes.clone();
        poll_until(|| !sb.updating()).await;
        if let Err(e) = sb.append_buffer_with_u8_array(&mut bytes) {
            // QuotaExceeded on very large stems: keep what's buffered
            // and stop appending — playback continues over the buffered
            // range. Anything else is a real failure.
            return Err(format!("appendBuffer: {e:?}"));
        }
    }
    poll_until(|| !sb.updating()).await;
    // Close out the stream so duration/ended semantics work.
    if ms.ready_state() == MediaSourceReadyState::Open {
        let _ = ms.end_of_stream();
    }
    Ok(())
}
