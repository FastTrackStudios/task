//! Vector + hybrid search via LanceDB. Behind the
//! `vector` feature flag.
//!
//! ## Storage
//!
//! `<vault>/Wiki/_state/lancedb/` is the on-disk Lance
//! table directory. Schema mirrors llm_wiki's:
//!
//! ```text
//! path        Utf8                       (vault-relative, e.g. `Concepts/Foo.md`)
//! title       Utf8
//! type        Utf8                       (frontmatter `type:`)
//! body        Utf8
//! vector      FixedSizeList<Float32, dim>
//! ```
//!
//! `dim` is set by the embedder. Stored alongside the
//! table as `dim.txt`.
//!
//! ## Indexing
//!
//! [`index`] takes a `&dyn DocEmbedder` + `&[IndexRow]`,
//! builds an Arrow `RecordBatch`, creates the table on
//! first call (overwrites on subsequent calls — full
//! re-index for now; incremental upsert is a follow-up
//! once we track per-row sha256s).
//!
//! ## Querying
//!
//! [`search_hybrid_with`] embeds the user query, runs a
//! Lance vector search, merges with token results via
//! reciprocal-rank fusion.

#![allow(dead_code)]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder};
use arrow_array::{Float32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::connect;
use lancedb::query::{ExecutableQuery, QueryBase};
use wiki_proto::search::{SearchHit, SearchHits, SearchMode, SearchOpts};

use crate::SearchError;

/// Embed a query string into a vector.
pub trait QueryEmbedder: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
    /// Embedding dimension — must match the indexed
    /// table's `dim.txt`.
    fn dim(&self) -> usize;
}

/// Embed a batch of documents for the indexer.
pub trait DocEmbedder: QueryEmbedder {
    fn embed_batch(&self, docs: &[&str]) -> Result<Vec<Vec<f32>>, String>;
}

/// One row to index.
#[derive(Debug, Clone)]
pub struct IndexRow {
    pub path: String,
    pub title: String,
    pub node_type: String,
    pub body: String,
}

const TABLE_NAME: &str = "wiki_pages";

/// On-disk Lance table directory.
pub fn lance_dir(vault_root: &Path) -> PathBuf {
    vault_root
        .join(wiki_proto::paths::WIKI_ROOT)
        .join(wiki_proto::paths::STATE_DIR)
        .join("lancedb")
}

/// Index a batch of rows into the on-disk Lance table.
/// Drops + re-creates the table — incremental upsert
/// follows once row-level sha256 bookkeeping lands.
pub fn index(
    vault_root: &Path,
    embedder: &dyn DocEmbedder,
    rows: &[IndexRow],
) -> Result<usize, SearchError> {
    let dim = embedder.dim();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| SearchError::Vector(format!("tokio: {e}")))?;

    rt.block_on(async {
        let dir = lance_dir(vault_root);
        std::fs::create_dir_all(&dir).map_err(SearchError::Io)?;
        std::fs::write(dir.join("dim.txt"), dim.to_string()).map_err(SearchError::Io)?;

        let texts: Vec<&str> = rows.iter().map(|r| r.body.as_str()).collect();
        let embeddings = embedder
            .embed_batch(&texts)
            .map_err(|e| SearchError::Vector(format!("embed_batch: {e}")))?;

        let schema = Arc::new(Schema::new(vec![
            Field::new("path", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("body", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dim as i32,
                ),
                false,
            ),
        ]));

        let paths = StringArray::from_iter_values(rows.iter().map(|r| r.path.as_str()));
        let titles = StringArray::from_iter_values(rows.iter().map(|r| r.title.as_str()));
        let types = StringArray::from_iter_values(rows.iter().map(|r| r.node_type.as_str()));
        let bodies = StringArray::from_iter_values(rows.iter().map(|r| r.body.as_str()));

        let mut vec_builder =
            FixedSizeListBuilder::new(Float32Builder::with_capacity(rows.len() * dim), dim as i32);
        for emb in &embeddings {
            vec_builder.values().append_slice(emb);
            vec_builder.append(true);
        }
        let vectors = vec_builder.finish();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(paths),
                Arc::new(titles),
                Arc::new(types),
                Arc::new(bodies),
                Arc::new(vectors),
            ],
        )
        .map_err(|e| SearchError::Vector(format!("record batch: {e}")))?;

        let db = connect(
            dir.to_str()
                .ok_or_else(|| SearchError::Vector("non-utf8 path".into()))?,
        )
        .execute()
        .await
        .map_err(|e| SearchError::Vector(format!("connect: {e}")))?;

        // Drop existing table for clean re-index.
        let _ = db.drop_table(TABLE_NAME, &[]).await;
        db.create_table(TABLE_NAME, vec![batch])
            .execute()
            .await
            .map_err(|e| SearchError::Vector(format!("create_table: {e}")))?;
        Ok::<_, SearchError>(rows.len())
    })
}

/// Default `search_hybrid` entry — no embedder available,
/// so we tag the response and fall back to token.
pub(crate) fn search_hybrid(
    vault_root: &Path,
    opts: &SearchOpts,
) -> Result<SearchHits, SearchError> {
    let mut hits = crate::token::search_token(vault_root, opts)?;
    hits.mode = SearchMode::Hybrid;
    Ok(hits)
}

/// Hybrid search with a caller-supplied embedder. Runs
/// token + vector in sequence and fuses results via
/// reciprocal-rank.
pub fn search_hybrid_with(
    vault_root: &Path,
    embedder: &dyn QueryEmbedder,
    opts: &SearchOpts,
) -> Result<SearchHits, SearchError> {
    let dir = lance_dir(vault_root);
    if !dir.join("dim.txt").is_file() {
        return search_hybrid(vault_root, opts);
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| SearchError::Vector(format!("tokio: {e}")))?;

    let q_embedding = embedder
        .embed(&opts.query)
        .map_err(|e| SearchError::Vector(format!("embed query: {e}")))?;

    let vector_hits: Vec<SearchHit> = rt.block_on(async {
        let db = connect(
            dir.to_str()
                .ok_or_else(|| SearchError::Vector("non-utf8 path".into()))?,
        )
        .execute()
        .await
        .map_err(|e| SearchError::Vector(format!("connect: {e}")))?;
        let table = db
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| SearchError::Vector(format!("open_table: {e}")))?;
        let top_k = if opts.top_k == 0 {
            20
        } else {
            opts.top_k as usize
        };
        let mut stream = table
            .query()
            .nearest_to(q_embedding)
            .map_err(|e| SearchError::Vector(format!("nearest_to: {e}")))?
            .limit(top_k)
            .execute()
            .await
            .map_err(|e| SearchError::Vector(format!("execute query: {e}")))?;
        let mut out = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|e| SearchError::Vector(format!("stream: {e}")))?
        {
            let path_col = batch
                .column_by_name("path")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| SearchError::Vector("path col".into()))?;
            let title_col = batch
                .column_by_name("title")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| SearchError::Vector("title col".into()))?;
            let body_col = batch
                .column_by_name("body")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| SearchError::Vector("body col".into()))?;
            let distance_col = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
            for i in 0..batch.num_rows() {
                let dist = distance_col.map_or(0.0, |c| c.value(i));
                // Convert distance → score (higher is
                // better). Caller may further re-rank.
                let score = 1.0 / (1.0 + dist);
                out.push(SearchHit {
                    path: path_col.value(i).to_string(),
                    title: title_col.value(i).to_string(),
                    snippet: body_col.value(i).chars().take(200).collect(),
                    content: if opts.include_content {
                        body_col.value(i).to_string()
                    } else {
                        String::new()
                    },
                    score,
                    matched_terms: Vec::new(),
                });
            }
        }
        Ok::<_, SearchError>(out)
    })?;

    let token_hits = crate::token::search_token(vault_root, opts)?;

    // Reciprocal-rank fusion: score(p) = Σ 1/(k + rank_i).
    // k = 60 (Cormack et al.'s default).
    const K: f32 = 60.0;
    let mut fused: std::collections::HashMap<String, SearchHit> = Default::default();
    for (rank, h) in token_hits.hits.iter().enumerate() {
        let bonus = 1.0 / (K + rank as f32 + 1.0);
        let entry = fused.entry(h.path.clone()).or_insert_with(|| SearchHit {
            path: h.path.clone(),
            title: h.title.clone(),
            snippet: h.snippet.clone(),
            content: h.content.clone(),
            score: 0.0,
            matched_terms: h.matched_terms.clone(),
        });
        entry.score += bonus;
    }
    for (rank, h) in vector_hits.iter().enumerate() {
        let bonus = 1.0 / (K + rank as f32 + 1.0);
        let entry = fused.entry(h.path.clone()).or_insert_with(|| SearchHit {
            path: h.path.clone(),
            title: h.title.clone(),
            snippet: h.snippet.clone(),
            content: h.content.clone(),
            score: 0.0,
            matched_terms: Vec::new(),
        });
        entry.score += bonus;
    }
    let mut merged: Vec<SearchHit> = fused.into_values().collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if opts.top_k > 0 {
        merged.truncate(opts.top_k as usize);
    }

    Ok(SearchHits {
        mode: SearchMode::Hybrid,
        token_count: token_hits.hits.len() as u32,
        vector_count: vector_hits.len() as u32,
        hits: merged,
    })
}
