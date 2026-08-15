//! `recipe-import` — webpage → cooklang `.cook` source.
//!
//! The pipeline behind `task recipe import <url>` (issue 44fe504f):
//!
//! 1. **Fetch** ([`fetch_html`]) — reqwest with a browser UA. Bot
//!    protection (Cloudflare & friends) is detected and surfaced as
//!    [`ImportError::BotProtected`] with a `--from-file <saved.html>`
//!    escape hatch in the message.
//! 2. **Extract** ([`extract`]) — schema.org/Recipe structured data →
//!    [`NormalizedRecipe`], via cooklang-import's extractor chain
//!    (JSON-LD → microdata → HTML classes). The NormalizedRecipe is
//!    the pinned source of truth for everything downstream.
//! 3. **Synthesize** — [`synthesize_llm`] (LLM-primary: Anthropic
//!    Messages API behind the [`LlmClient`] trait, output validated by
//!    the in-tree cooklang parser, one retry with the parser's
//!    diagnostics) or [`synthesize_heuristic`] (`--offline` / LLM
//!    fallback: deterministic qty/unit parse + first-occurrence
//!    ingredient weaving; never drops an ingredient, always
//!    parser-valid, frontmatter `import: needs-review` so a later
//!    `task recipe polish` can find it).
//! 4. **Save** — NOT here. The CLI feeds the synthesized source
//!    through its existing validate-and-create path
//!    (`CookbookServiceClient::create/update`), so imported recipes
//!    flow into pantry/shopping exactly like hand-written ones.
//!
//! # V2 design note — `--from {mealie|paprika|tandoor}`
//!
//! Alternate front-ends are alternate producers of stage 2's output:
//! a Mealie/Paprika/Tandoor export is parsed straight into a
//! [`NormalizedRecipe`] (they are all JSON with name / ingredient
//! lines / step texts / metadata — strictly easier than HTML), and
//! stages 3–4 run unchanged. Implement as
//! `fn from_mealie(json: &str) -> Result<NormalizedRecipe, ImportError>`
//! (etc.) next to [`extract`]; nothing else needs to change.

#![cfg(not(target_arch = "wasm32"))]

pub mod error;
pub mod extract;
pub mod fetch;
pub mod heuristic;
pub mod llm;
pub mod normalized;
pub mod synthesize;
pub mod validate;

pub use error::ImportError;
pub use extract::extract;
pub use fetch::fetch_html;
pub use heuristic::synthesize_heuristic;
pub use llm::{AnthropicClient, LlmClient};
pub use normalized::NormalizedRecipe;
pub use synthesize::{Coverage, SynthesisOutcome, coverage, synthesize_llm};
pub use validate::validate_cook;
