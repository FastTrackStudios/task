//! Outbound submission seam for the maildir backend.
//!
//! The maildir tree is a local store — it can't deliver mail by
//! itself. [`Submit`] is the transport the backend composes to
//! satisfy `EmailSync::send`: production wires
//! [`email_smtp::SmtpSender`], tests wire a mock that records
//! what would have gone on the wire. Object-safe (boxed future)
//! so one `Arc<dyn Submit>` rides per account.

use std::future::Future;
use std::pin::Pin;

/// One submission transport. `submit_raw` takes the already-built
/// RFC5322 bytes plus the envelope (`from` / `recipients`) and
/// returns the Message-ID of the submitted message.
pub trait Submit: Send + Sync {
    fn submit_raw<'a>(
        &'a self,
        from: &'a str,
        recipients: &'a [String],
        raw: &'a [u8],
        message_id: String,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// The production transport: SMTP submission via `mail-send`.
impl Submit for email_smtp::SmtpSender {
    fn submit_raw<'a>(
        &'a self,
        from: &'a str,
        recipients: &'a [String],
        raw: &'a [u8],
        message_id: String,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            self.send_raw(from, recipients, raw, message_id)
                .await
                .map_err(|e| e.to_string())
        })
    }
}
