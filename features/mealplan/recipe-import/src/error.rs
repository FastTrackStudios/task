//! Import pipeline errors. Every variant renders a *user-facing*
//! message — the CLI prints these verbatim, so each carries its own
//! recovery hint.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImportError {
    /// Network-level fetch failure (DNS, TLS, timeout, …).
    #[error("fetch {url}: {reason}")]
    Fetch { url: String, reason: String },

    /// The site answered, but with a bot-protection / paywall shape
    /// (403/401/429/503, or a challenge page). The escape hatch: save
    /// the page from a real browser and re-run with `--from-file`.
    #[error(
        "{url}: blocked by the site (HTTP {status}{hint}) — this usually means bot protection \
         (Cloudflare) or a paywall.\n  escape hatch: open the page in your browser, save it \
         (Ctrl+S → 'Webpage, HTML Only'), then re-run with `--from-file <saved.html>`"
    )]
    BotProtected {
        url: String,
        status: u16,
        /// e.g. ", challenge page detected" — empty when the status
        /// code alone was conclusive.
        hint: String,
    },

    /// Fetched fine, but no schema.org/Recipe (or recognizable recipe
    /// markup) anywhere in the document.
    #[error(
        "no recipe found at {url}: the page has no schema.org/Recipe JSON-LD, microdata, or \
         recognizable recipe markup{detail}"
    )]
    NoRecipeFound { url: String, detail: String },

    /// The structured data was present but unusably incomplete
    /// (e.g. zero ingredients).
    #[error("recipe data at {url} is incomplete: {detail}")]
    IncompleteRecipe { url: String, detail: String },

    /// LLM transport / API error (not a validation failure — those
    /// trigger the retry-then-heuristic path inside synthesis).
    #[error("llm: {0}")]
    Llm(String),

    /// Synthesized cooklang failed parser validation twice. Carries
    /// the final rendered diagnostics; the caller falls back to the
    /// heuristic.
    #[error("llm output failed cooklang validation twice; last report:\n{0}")]
    LlmValidation(String),

    #[error("io: {0}")]
    Io(String),
}
