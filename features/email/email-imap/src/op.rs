//! High-level operations the backend supports, modeled as
//! data. Each variant carries everything the protocol layer
//! needs; `OpResult` carries what comes back.
//!
//! This is the seam pimalaya/io-imap turns into a full sans-io
//! coroutine — for now we just use it as a typed boundary
//! between "what we ask IMAP to do" and "how it actually
//! happens." The `backend` module drives `Op`s through an
//! `async-imap` session; tests can drive the same `Op`s
//! through a recorded transcript.

use email_proto::{Envelope, Folder, Message, SeqRange};

#[derive(Debug, Clone)]
pub enum Op {
    ListFolders,
    FetchEnvelopes {
        folder: String,
        range: SeqRange,
    },
    FetchMessage {
        folder: String,
        message_id: String,
    },
    FetchAttachment {
        folder: String,
        message_id: String,
        part: String,
    },
}

#[derive(Debug, Clone)]
pub enum OpResult {
    Folders(Vec<Folder>),
    Envelopes(Vec<Envelope>),
    Message(Box<Message>),
    Attachment(Vec<u8>),
}
