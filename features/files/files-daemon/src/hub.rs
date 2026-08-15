//! The daemon's status-event fan-out hub — behind
//! [`crate::service::DaemonControlService::status_events`]. Sliding
//! mailbox (a slow subscriber loses its oldest queued events), correct
//! for these state-shaped `Changed(snapshot)` payloads: a subscriber
//! that fell behind only needs the latest.

use crate::model::DaemonStatus;
use crate::service::DaemonEvent;

/// Wraps `architect::PubSub` so the daemon and the service layer share
/// one publisher without every caller naming the vox generics.
#[derive(Clone)]
pub struct EventHub {
    inner: architect::PubSub<DaemonEvent>,
}

impl Default for EventHub {
    fn default() -> Self {
        // Sliding mailbox: a slow subscriber loses its oldest queued
        // snapshots, correct for these state-shaped payloads.
        Self {
            inner: architect::PubSub::sliding(256),
        }
    }
}

impl EventHub {
    /// Publish a fresh status snapshot to every subscriber.
    pub fn publish(&self, status: DaemonStatus) {
        self.inner.publish(DaemonEvent::Changed(status));
    }

    /// The underlying hub, for the service's stream source to attach to.
    #[must_use]
    pub fn pubsub(&self) -> &architect::PubSub<DaemonEvent> {
        &self.inner
    }
}
