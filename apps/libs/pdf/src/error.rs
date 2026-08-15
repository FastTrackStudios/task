use thiserror::Error;

#[derive(Debug, Error)]
pub enum PdfError {
    /// Anything fulgur raises during render — template parse,
    /// CSS error, font load, krilla emit.
    #[error("fulgur: {0}")]
    Fulgur(String),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid: {0}")]
    Invalid(String),
}
