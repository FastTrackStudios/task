//! Mozilla ISPDB + provider-served autoconfig fetcher.
//!
//! ISPDB serves the same XML schema as the per-provider
//! `autoconfig.<domain>` endpoints, just from a curated central
//! source. Both go through [`parse`](crate::xml::parse).
//!
//! We deliberately keep this thin: build a URL, GET it, parse
//! the body. No retries, no caching here — callers can wrap
//! [`lookup`] with their own policy.

use crate::xml;
use crate::{AutoconfigResult, Error};

#[derive(Debug, Clone, Copy)]
pub enum ProviderSource {
    /// `https://autoconfig.thunderbird.net/v1.1/<domain>`.
    /// Curated by Mozilla; covers most major providers.
    Mozilla,
    /// `https://autoconfig.<domain>/mail/config-v1.1.xml` and
    /// `https://<domain>/.well-known/mail-config`. Tried in
    /// that order; first 200 wins.
    Provider,
}

pub async fn lookup(
    domain: &str,
    source: ProviderSource,
) -> Result<Option<AutoconfigResult>, Error> {
    let urls = candidate_urls(domain, source);
    let client = reqwest::Client::builder()
        .user_agent("task-email-autoconfig/0.1")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;

    for url in urls {
        tracing::trace!(%url, "autoconfig GET");
        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::trace!(%url, error = %e, "autoconfig request failed");
                continue;
            }
        };
        if !resp.status().is_success() {
            tracing::trace!(%url, status = ?resp.status(), "non-2xx");
            continue;
        }
        let body = resp.text().await.map_err(|e| Error::Http(e.to_string()))?;
        let mut parsed = xml::parse(&body)?;
        if parsed.is_empty() {
            continue;
        }
        parsed.source = Some(url);
        return Ok(Some(parsed));
    }
    Ok(None)
}

fn candidate_urls(domain: &str, source: ProviderSource) -> Vec<String> {
    match source {
        ProviderSource::Mozilla => {
            vec![format!("https://autoconfig.thunderbird.net/v1.1/{domain}")]
        }
        ProviderSource::Provider => vec![
            format!("https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress=user@{domain}"),
            format!("https://{domain}/.well-known/mail-config"),
            format!("http://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress=user@{domain}"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_shapes_match_thunderbird() {
        let urls = candidate_urls("example.com", ProviderSource::Mozilla);
        assert_eq!(
            urls,
            vec!["https://autoconfig.thunderbird.net/v1.1/example.com"]
        );

        let urls = candidate_urls("example.com", ProviderSource::Provider);
        assert!(urls[0].starts_with("https://autoconfig.example.com/"));
        assert!(urls[1].contains(".well-known/mail-config"));
    }
}
