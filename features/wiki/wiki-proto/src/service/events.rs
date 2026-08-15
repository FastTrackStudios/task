//! Live change subscription — one `#[subscribe]` stream served
//! from the backend's `architect::PubSub` hub.

use crate::event::WikiChange;

#[architect::rpc]
pub trait Events {
    /// Every wiki change, as it happens — page writes, ingest
    /// queue transitions, lint / review / federation news.
    /// Unfiltered across wiki ids; each [`WikiChange`] carries its
    /// `wiki_id` so subscribers keep the one they browse. See
    /// [`WikiChange`] for the fetch-once-then-fold subscriber
    /// contract.
    #[subscribe]
    fn changes(&self) -> WikiChange;
}
