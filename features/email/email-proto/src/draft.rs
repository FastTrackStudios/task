//! Compose-side payloads — [`Draft`] for `append_draft`/`send`,
//! [`Attachment`] / [`AttachmentMeta`] for inline + post-fetch
//! attachment shapes.

use crate::Addr;
use facet::Facet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct AttachmentMeta {
    /// Backend-specific part address used by
    /// [`crate::EmailSync::fetch_attachment`].
    pub part: String,
    pub filename: Option<String>,
    pub mime: String,
    pub size: u64,
    /// `Content-ID` for an **inline** part, without angle brackets.
    ///
    /// This is how HTML mail embeds an image: the part carries
    /// `Content-ID: <logo>` and the body references `src="cid:logo"`.
    /// `None` = an ordinary attachment the client lists at the bottom.
    /// Borrowed from Resend's `CreateAttachment::with_content_id`,
    /// which is the piece our compose path had no way to express.
    #[serde(default)]
    pub content_id: Option<String>,
}

/// In-memory attachment carried on a [`Draft`] before send.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct Attachment {
    pub meta: AttachmentMeta,
    pub data: Vec<u8>,
}

impl Attachment {
    /// An attachment from bytes.
    ///
    /// `part` is empty until a backend assigns one — it addresses a
    /// part of a *fetched* message, and this is an outgoing one.
    /// `mime` defaults to `application/octet-stream`, which every
    /// client will at least offer to download.
    #[must_use]
    pub fn new(filename: impl Into<String>, data: Vec<u8>) -> Self {
        let size = data.len() as u64;
        Self {
            meta: AttachmentMeta {
                part: String::new(),
                filename: Some(filename.into()),
                mime: "application/octet-stream".to_owned(),
                size,
                content_id: None,
            },
            data,
        }
    }

    #[must_use]
    pub fn with_content_type(mut self, mime: impl Into<String>) -> Self {
        self.meta.mime = mime.into();
        self
    }

    /// Mark this part inline under `cid`, for `src="cid:…"` in the
    /// HTML body. Angle brackets are stripped if present.
    #[must_use]
    pub fn with_content_id(mut self, cid: impl Into<String>) -> Self {
        let cid = cid.into();
        let bare = cid.trim_start_matches('<').trim_end_matches('>').to_owned();
        self.meta.content_id = Some(bare);
        self
    }

    /// Is this part meant to be rendered inside the body rather than
    /// listed as an attachment?
    #[must_use]
    pub fn is_inline(&self) -> bool {
        self.meta.content_id.is_some()
    }
}

/// Outgoing message. Empty `references` + `in_reply_to` =
/// fresh thread; populated = a reply.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct Draft {
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<Attachment>,
}

impl Draft {
    /// A plain-text message. The three things a message cannot be sent
    /// without are the arguments; everything else is opt-in.
    ///
    /// Shape borrowed from Resend's `CreateEmailBaseOptions` — required
    /// fields positional, the rest chained. Ours had ten public fields
    /// and every call site (compose form, CLI, agents) wrote a struct
    /// literal spelling out four empty vecs and three `None`s, which is
    /// both noisy and easy to get subtly wrong on a reply.
    #[must_use]
    pub fn new(from: Addr, to: Vec<Addr>, subject: impl Into<String>) -> Self {
        Self {
            from,
            to,
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: subject.into(),
            body_text: String::new(),
            body_html: None,
            in_reply_to: None,
            references: Vec::new(),
            attachments: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.body_text = text.into();
        self
    }

    /// Add an HTML alternative. The text body is kept as the fallback —
    /// send both, never HTML alone, or text-only clients get nothing.
    #[must_use]
    pub fn with_html(mut self, html: impl Into<String>) -> Self {
        self.body_html = Some(html.into());
        self
    }

    #[must_use]
    pub fn with_cc(mut self, addr: Addr) -> Self {
        self.cc.push(addr);
        self
    }

    #[must_use]
    pub fn with_bcc(mut self, addr: Addr) -> Self {
        self.bcc.push(addr);
        self
    }

    /// Make this a reply to `message_id`, threading it correctly.
    ///
    /// Sets `In-Reply-To` **and** appends to `References`. Clients
    /// thread on `References`; setting only `In-Reply-To` is the
    /// classic way to produce a reply that shows up as its own
    /// conversation, so the two are deliberately not separable here.
    #[must_use]
    pub fn in_reply_to(mut self, message_id: impl Into<String>) -> Self {
        let id = message_id.into();
        if !self.references.contains(&id) {
            self.references.push(id.clone());
        }
        self.in_reply_to = Some(id);
        self
    }

    /// Carry a parent's `References` chain onto this reply, ahead of
    /// the parent's own id.
    #[must_use]
    pub fn with_references(mut self, refs: impl IntoIterator<Item = String>) -> Self {
        let mut chain: Vec<String> = refs.into_iter().collect();
        for existing in std::mem::take(&mut self.references) {
            if !chain.contains(&existing) {
                chain.push(existing);
            }
        }
        self.references = chain;
        self
    }

    #[must_use]
    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    #[must_use]
    pub fn with_attachments(mut self, items: impl IntoIterator<Item = Attachment>) -> Self {
        self.attachments.extend(items);
        self
    }

    /// Every recipient the message will actually go to.
    #[must_use]
    pub fn recipients(&self) -> Vec<String> {
        self.to
            .iter()
            .chain(&self.cc)
            .chain(&self.bcc)
            .map(|a| a.email.clone())
            .collect()
    }
}

impl Addr {
    /// A bare address with no display name.
    #[must_use]
    pub fn mailbox(email: impl Into<String>) -> Self {
        Self {
            name: None,
            email: email.into(),
        }
    }

    /// A named address — `Cody <cody@example.com>`.
    #[must_use]
    pub fn named(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            email: email.into(),
        }
    }
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Attachment, AttachmentMeta, Draft};
    unsafe impl vox_types::Reborrow for AttachmentMeta {
        type Ref<'a> = AttachmentMeta;
    }
    unsafe impl vox_types::Reborrow for Attachment {
        type Ref<'a> = Attachment;
    }
    unsafe impl vox_types::Reborrow for Draft {
        type Ref<'a> = Draft;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_draft_has_no_stray_fields() {
        let d = Draft::new(
            Addr::mailbox("me@example.com"),
            vec![Addr::named("Alice", "alice@example.com")],
            "Hello",
        )
        .with_text("hi");
        assert_eq!(d.subject, "Hello");
        assert_eq!(d.body_text, "hi");
        assert!(d.body_html.is_none());
        assert!(d.cc.is_empty() && d.bcc.is_empty());
        assert!(d.in_reply_to.is_none() && d.references.is_empty());
    }

    #[test]
    fn replies_thread_on_references_not_just_in_reply_to() {
        // Setting only In-Reply-To is how you get a reply that clients
        // render as a brand new conversation. `in_reply_to` therefore
        // does both.
        let d = Draft::new(
            Addr::mailbox("me@x.com"),
            vec![Addr::mailbox("a@x.com")],
            "Re: hi",
        )
        .in_reply_to("parent@x.com");
        assert_eq!(d.in_reply_to.as_deref(), Some("parent@x.com"));
        assert_eq!(d.references, vec!["parent@x.com".to_owned()]);
    }

    #[test]
    fn a_parents_chain_is_carried_ahead_of_the_parent() {
        let d = Draft::new(
            Addr::mailbox("me@x.com"),
            vec![Addr::mailbox("a@x.com")],
            "Re: hi",
        )
        .in_reply_to("parent@x.com")
        .with_references(["root@x.com".to_owned(), "mid@x.com".to_owned()]);
        assert_eq!(
            d.references,
            vec![
                "root@x.com".to_owned(),
                "mid@x.com".to_owned(),
                "parent@x.com".to_owned()
            ],
            "oldest first, parent last"
        );
    }

    #[test]
    fn repeated_in_reply_to_does_not_duplicate_the_reference() {
        let d = Draft::new(Addr::mailbox("me@x.com"), vec![], "s")
            .in_reply_to("p@x.com")
            .in_reply_to("p@x.com");
        assert_eq!(d.references, vec!["p@x.com".to_owned()]);
    }

    #[test]
    fn recipients_covers_to_cc_and_bcc() {
        let d = Draft::new(
            Addr::mailbox("me@x.com"),
            vec![Addr::mailbox("a@x.com")],
            "s",
        )
        .with_cc(Addr::mailbox("c@x.com"))
        .with_bcc(Addr::mailbox("b@x.com"));
        assert_eq!(d.recipients(), vec!["a@x.com", "c@x.com", "b@x.com"]);
    }

    #[test]
    fn inline_attachments_are_distinguishable_and_cid_is_bare() {
        let plain =
            Attachment::new("notes.pdf", b"pdf".to_vec()).with_content_type("application/pdf");
        assert!(!plain.is_inline());
        assert_eq!(plain.meta.size, 3);

        // Angle brackets come off — the body references `cid:logo`,
        // and the header adds its own brackets.
        let inline = Attachment::new("logo.png", b"png".to_vec())
            .with_content_type("image/png")
            .with_content_id("<logo>");
        assert!(inline.is_inline());
        assert_eq!(inline.meta.content_id.as_deref(), Some("logo"));
    }
}
