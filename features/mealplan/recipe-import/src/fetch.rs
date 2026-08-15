//! Stage 1 — fetch the page.
//!
//! Browser UA (recipe sites serve different markup — or a block page —
//! to obvious bots), 30s timeout, and bot-protection classification:
//! a 403/401/429/503, or a 200 whose body is a recognizable challenge
//! page, becomes [`ImportError::BotProtected`] with the `--from-file`
//! escape hatch spelled out.

use std::time::Duration;

use crate::error::ImportError;

/// Same shape as a current desktop Chrome — what cooklang-import's own
/// fetcher sends, and what the smoke-tested sites accept.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Fetch `url` and return the HTML body.
pub async fn fetch_html(url: &str) -> Result<String, ImportError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ImportError::Fetch {
            url: url.to_string(),
            reason: format!("client build: {e}"),
        })?;

    let resp = client
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| ImportError::Fetch {
            url: url.to_string(),
            reason: e.to_string(),
        })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| ImportError::Fetch {
        url: url.to_string(),
        reason: format!("read body: {e}"),
    })?;

    classify(url, status.as_u16(), &body)?;
    Ok(body)
}

/// Pure classification so the error UX is unit-testable without a
/// network. `Ok(())` means "treat the body as page HTML".
pub(crate) fn classify(url: &str, status: u16, body: &str) -> Result<(), ImportError> {
    match status {
        // The classic bot-protection / paywall set.
        401 | 403 | 429 | 503 => Err(ImportError::BotProtected {
            url: url.to_string(),
            status,
            hint: String::new(),
        }),
        200..=299 => {
            if let Some(marker) = challenge_marker(body) {
                return Err(ImportError::BotProtected {
                    url: url.to_string(),
                    status,
                    hint: format!(", challenge page detected: {marker}"),
                });
            }
            Ok(())
        }
        s => Err(ImportError::Fetch {
            url: url.to_string(),
            reason: format!("HTTP {s}"),
        }),
    }
}

/// Detect a challenge interstitial served with a 200. Markers are
/// matched case-insensitively against the first 16KB (challenge pages
/// are tiny; real recipe pages put these strings nowhere near the top).
fn challenge_marker(body: &str) -> Option<&'static str> {
    const MARKERS: [&str; 5] = [
        "just a moment...",        // Cloudflare interstitial <title>
        "cf-browser-verification", // Cloudflare legacy challenge div
        "challenge-platform",      // Cloudflare turnstile script path
        "are you a robot",         // generic
        "px-captcha",              // PerimeterX
    ];
    let head: String = body
        .chars()
        .take(16 * 1024)
        .collect::<String>()
        .to_lowercase();
    MARKERS.into_iter().find(|m| head.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_is_bot_protected_with_escape_hatch() {
        let err = classify("https://x.test/r", 403, "").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("HTTP 403"), "{msg}");
        assert!(msg.contains("--from-file"), "{msg}");
    }

    #[test]
    fn cloudflare_challenge_on_200_is_detected() {
        let body = "<html><title>Just a moment...</title></html>";
        let err = classify("https://x.test/r", 200, body).unwrap_err();
        assert!(
            matches!(err, ImportError::BotProtected { status: 200, .. }),
            "{err}"
        );
    }

    #[test]
    fn plain_200_passes() {
        assert!(classify("https://x.test/r", 200, "<html>recipe</html>").is_ok());
    }

    #[test]
    fn other_errors_are_plain_fetch_errors() {
        let err = classify("https://x.test/r", 500, "").unwrap_err();
        assert!(matches!(err, ImportError::Fetch { .. }), "{err}");
    }
}
