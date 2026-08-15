//! Token search — grep + TF-IDF over page bodies.
//!
//! Cheap, no dependencies. Splits the query into terms;
//! score = Σ over terms (tf × idf), where tf = occurrence
//! count in the page (capped at 8 to bound long-doc bias)
//! and idf = ln(N / (1 + df)). Snippet is the first
//! sentence containing any term, capped at 200 chars.

use std::collections::HashMap;
use std::path::Path;

use wiki_proto::search::{SearchHit, SearchHits, SearchMode, SearchOpts};

use crate::SearchError;
use crate::scan::scan;

pub(crate) fn search_token(
    vault_root: &Path,
    opts: &SearchOpts,
) -> Result<SearchHits, SearchError> {
    let pages = scan(vault_root)?;
    if pages.is_empty() {
        return Ok(SearchHits {
            mode: SearchMode::Token,
            token_count: 0,
            vector_count: 0,
            hits: Vec::new(),
        });
    }
    let terms = tokenize(&opts.query);
    if terms.is_empty() {
        return Ok(SearchHits {
            mode: SearchMode::Token,
            token_count: 0,
            vector_count: 0,
            hits: Vec::new(),
        });
    }

    // Doc frequency per term.
    let mut df: HashMap<&str, u32> = HashMap::new();
    let lowered: Vec<String> = pages.iter().map(|p| p.body.to_lowercase()).collect();
    for t in &terms {
        let term = t.as_str();
        let count = lowered.iter().filter(|body| body.contains(term)).count() as u32;
        df.insert(term, count);
    }
    let n_total = pages.len() as f32;

    let mut hits = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        if !opts.node_type.is_empty() && page.page_type != opts.node_type {
            continue;
        }
        let body = &lowered[i];
        let mut score = 0.0_f32;
        let mut matched = Vec::new();
        for term in &terms {
            let tf = body.matches(term.as_str()).count().min(8) as f32;
            if tf == 0.0 {
                continue;
            }
            matched.push(term.clone());
            let d = *df.get(term.as_str()).unwrap_or(&0) as f32;
            let idf = (n_total / (1.0 + d)).ln().max(0.0);
            score += tf * idf;
        }
        if score <= 0.0 {
            continue;
        }
        let snippet = make_snippet(&page.body, &terms);
        hits.push(SearchHit {
            path: page.rel_path.clone(),
            title: page.title.clone(),
            snippet,
            content: if opts.include_content {
                page.body.clone()
            } else {
                String::new()
            },
            score,
            matched_terms: matched,
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if opts.top_k > 0 {
        hits.truncate(opts.top_k as usize);
    }
    let token_count = hits.len() as u32;
    Ok(SearchHits {
        mode: SearchMode::Token,
        token_count,
        vector_count: 0,
        hits,
    })
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(str::to_lowercase)
        .collect()
}

fn make_snippet(body: &str, terms: &[String]) -> String {
    let lower = body.to_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for line in body.lines() {
        let line_lower = line.to_lowercase();
        let mut hits = 0;
        for t in terms {
            if line_lower.contains(t.as_str()) {
                hits += 1;
            }
        }
        if hits > 0 && best.is_none_or(|(h, _)| hits > h) {
            best = Some((hits, line));
        }
    }
    let snippet = best.map_or_else(
        || {
            
            body.lines().find(|l| !l.is_empty()).unwrap_or("")
        },
        |(_, l)| l,
    );
    let s: String = snippet.chars().take(200).collect();
    let _ = lower;
    s
}
