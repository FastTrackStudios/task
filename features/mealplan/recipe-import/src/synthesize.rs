//! Stage 3a — LLM-primary synthesis, plus the ingredient-coverage
//! report both synthesizers share.
//!
//! Flow: prompt (NormalizedRecipe pinned as source of truth + cooklang
//! syntax card + annotate-first-occurrence rules) → completion →
//! validate with the in-tree cooklang parser → on failure, ONE retry
//! carrying the failed output + the parser's rendered diagnostics →
//! on a second failure, `Err(ImportError::LlmValidation)` so the
//! caller falls back to [`crate::synthesize_heuristic`].

use crate::error::ImportError;
use crate::llm::{ChatMessage, LlmClient};
use crate::normalized::NormalizedRecipe;
use crate::validate::validate_cook;

/// The system prompt: cooklang syntax card + conversion rules.
const SYSTEM: &str = r#"You convert recipes into cooklang, the plain-text recipe markup. You output ONLY the cooklang source — no markdown fences, no commentary, no explanations.

COOKLANG SYNTAX CARD
- The file starts with YAML frontmatter between `---` lines. Use these keys when data exists: title, description, servings, prep time, cook time, course, cuisine, tags, source, image.
- The body is steps: one paragraph per step, separated by ONE blank line.
- Ingredients are annotated inline: @name{quantity%unit}. Multi-word names work because the braces terminate the name: @olive oil{2%tbsp}. No amount: @salt{}. Quantity without unit: @eggs{3}. Fractions are fine: @flour{1/2%cup}.
- Cookware: #pan{} or #cast iron skillet{}. Timers: ~{10%minutes}.
- `--` starts a comment to end of line; avoid emitting it inside step text.

CONVERSION RULES
1. The JSON recipe you are given is the source of truth. Use its wording. Do not invent ingredients, quantities, or steps; do not drop any.
2. Annotate each ingredient at its FIRST natural mention in the steps, weaving the @...{} annotation into the existing words. Later mentions stay plain text.
3. Quantities and units come from the ingredient list, not the step text.
4. If an ingredient is never mentioned in any step, work it into the step where it would actually be used (e.g. "Fold in the @walnuts{1%cup}"), keeping the step truthful.
5. Every ingredient from the list must appear exactly once as an @ annotation.
6. Annotate cookware (#) and durations (~) where they appear naturally; don't force them.
7. Frontmatter: copy title/description/servings/times/course/cuisine/tags/source/image from the JSON metadata when present. Do not fabricate values."#;

/// Outcome of LLM synthesis, for the CLI's summary line.
#[derive(Debug)]
pub struct SynthesisOutcome {
    /// Validated cooklang source.
    pub source: String,
    /// 1 = first attempt validated; 2 = needed the diagnostic retry.
    pub attempts: u8,
}

/// Synthesize cooklang via the LLM, validating with the in-tree
/// parser and retrying once with diagnostics. Two validation failures
/// → [`ImportError::LlmValidation`] (caller falls back to the
/// heuristic). Transport errors surface as [`ImportError::Llm`].
pub async fn synthesize_llm<C: LlmClient>(
    client: &C,
    recipe: &NormalizedRecipe,
) -> Result<SynthesisOutcome, ImportError> {
    let user = format!(
        "Convert this recipe to cooklang.\n\nRECIPE (source of truth):\n{}",
        recipe.to_prompt_json()
    );
    let mut transcript = vec![ChatMessage::user(user)];

    let first = strip_fences(&client.complete(SYSTEM, &transcript).await?);
    let label = format!("{}.cook", recipe.name);
    let first_report = match validate_cook(&label, &first) {
        Ok(()) => {
            return Ok(SynthesisOutcome {
                source: first,
                attempts: 1,
            });
        }
        Err(report) => report,
    };
    tracing::warn!("llm cooklang failed validation, retrying with diagnostics");

    transcript.push(ChatMessage::assistant(first.clone()));
    transcript.push(ChatMessage::user(format!(
        "That cooklang failed to parse. Fix the errors and output the FULL corrected cooklang \
         source (only the source, no commentary).\n\nPARSER DIAGNOSTICS:\n{first_report}"
    )));

    let second = strip_fences(&client.complete(SYSTEM, &transcript).await?);
    match validate_cook(&label, &second) {
        Ok(()) => Ok(SynthesisOutcome {
            source: second,
            attempts: 2,
        }),
        Err(report) => Err(ImportError::LlmValidation(report)),
    }
}

/// Models love markdown fences no matter how firmly told otherwise.
fn strip_fences(s: &str) -> String {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t.to_string();
    };
    // Drop the info string ("cooklang", "cook", …) on the fence line.
    let rest = rest.split_once('\n').map_or("", |(_, body)| body);
    rest.trim_end()
        .trim_end_matches("```")
        .trim_end()
        .to_string()
}

// ── Ingredient coverage ──────────────────────────────────────

/// How many of the normalized ingredient lines made it into the final
/// `.cook` as `@` annotations. Printed by the CLI after every import.
#[derive(Debug)]
pub struct Coverage {
    pub total: usize,
    pub matched: usize,
    /// Normalized ingredient lines with no corresponding annotation.
    pub unmatched: Vec<String>,
}

impl Coverage {
    #[must_use]
    pub fn complete(&self) -> bool {
        self.matched == self.total
    }
}

/// Compare the normalized ingredient lines against the `@` annotations
/// the parser actually sees in `source`.
#[must_use]
pub fn coverage(recipe: &NormalizedRecipe, source: &str) -> Coverage {
    let parser =
        cooklang::CooklangParser::new(cooklang::Extensions::all(), cooklang::Converter::bundled());
    let annotated: Vec<String> = parser
        .parse(source)
        .into_result()
        .map(|(parsed, _)| {
            parsed
                .ingredients
                .iter()
                .map(|i| i.name.to_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let mut matched = 0usize;
    let mut unmatched = Vec::new();
    for line in &recipe.ingredients {
        let line_lc = line.to_lowercase();
        let hit = annotated.iter().any(|name| {
            name.len() >= 3 && (line_lc.contains(name.as_str()) || name.contains(line_lc.as_str()))
        });
        if hit {
            matched += 1;
        } else {
            unmatched.push(line.clone());
        }
    }
    Coverage {
        total: recipe.ingredients.len(),
        matched,
        unmatched,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::error::ImportError;

    /// Scripted mock: pops canned responses in order.
    struct MockLlm {
        responses: Mutex<Vec<String>>,
        calls: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl MockLlm {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().map(String::from).collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl LlmClient for MockLlm {
        async fn complete(
            &self,
            _system: &str,
            messages: &[ChatMessage],
        ) -> Result<String, ImportError> {
            self.calls.lock().unwrap().push(messages.to_vec());
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| ImportError::Llm("mock exhausted".into()))
        }
    }

    fn sample() -> NormalizedRecipe {
        NormalizedRecipe {
            name: "Toast".into(),
            ingredients: vec!["2 slices bread".into(), "1 tbsp butter".into()],
            steps: vec!["Toast the bread.".into(), "Spread the butter.".into()],
            ..Default::default()
        }
    }

    const GOOD: &str =
        "---\ntitle: Toast\n---\n\nToast the @bread{2%slices}.\n\nSpread the @butter{1%tbsp}.\n";
    // Empty quantity before `%` → guaranteed hard parse error.
    const BAD: &str = "Toast the @bread{%}.\n\nSpread the @butter{1%tbsp}.\n";

    #[tokio::test]
    async fn first_attempt_valid() {
        let llm = MockLlm::new(vec![GOOD]);
        let out = synthesize_llm(&llm, &sample()).await.unwrap();
        assert_eq!(out.attempts, 1);
        assert!(out.source.contains("@bread"));
        assert_eq!(llm.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn fenced_output_is_unwrapped() {
        let fenced = format!("```cooklang\n{GOOD}\n```");
        let llm = MockLlm::new(vec![&fenced]);
        let out = synthesize_llm(&llm, &sample()).await.unwrap();
        assert!(!out.source.contains("```"));
        assert!(out.source.starts_with("---"));
    }

    #[tokio::test]
    async fn invalid_then_valid_retries_with_diagnostics() {
        let llm = MockLlm::new(vec![BAD, GOOD]);
        let out = synthesize_llm(&llm, &sample()).await.unwrap();
        assert_eq!(out.attempts, 2);
        let calls = llm.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        // Retry transcript carries the failed attempt + diagnostics.
        assert_eq!(calls[1].len(), 3);
        assert!(calls[1][2].content.contains("PARSER DIAGNOSTICS"));
    }

    #[tokio::test]
    async fn two_failures_signal_heuristic_fallback() {
        let llm = MockLlm::new(vec![BAD, BAD]);
        let err = synthesize_llm(&llm, &sample()).await.unwrap_err();
        assert!(matches!(err, ImportError::LlmValidation(_)), "{err}");
    }

    #[test]
    fn coverage_counts_annotations() {
        let cov = coverage(&sample(), GOOD);
        assert_eq!(cov.total, 2);
        assert_eq!(cov.matched, 2);
        assert!(cov.complete());

        let cov = coverage(
            &sample(),
            "---\ntitle: Toast\n---\n\nToast the @bread{2%slices}.\n",
        );
        assert_eq!(cov.matched, 1);
        assert_eq!(cov.unmatched, vec!["1 tbsp butter".to_string()]);
    }
}
