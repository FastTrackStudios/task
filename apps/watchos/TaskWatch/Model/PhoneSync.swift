// Watch ← iPhone config inheritance (WCSession).
//
// The watch has no independent way to know which Task server + account it
// should talk to. Instead it INHERITS the paired iPhone's active config
// ({baseURL, orgSlug, token}) over WatchConnectivity: the phone pushes its
// current server/session via `updateApplicationContext(_:)`, and this
// delegate applies it to the shared `TaskStore` — so signing in on the phone
// (and the federated-locker flow that follows) auto-configures the watch with
// no manual Settings entry. Manual Settings still works as a fallback.
//
// Pairs with the iOS-side shim documented in
// `apps/task/mobile/ios/watch-config-bridge.md` (the phone sender). The keys
// here MUST match the keys the phone writes.

import Foundation
import WatchConnectivity

/// Activates a `WCSession` and mirrors the iPhone's application context into
/// the watch's `TaskStore`. Only overwrites a field when the phone actually
/// sent a non-empty value, so a partial/late context never wipes a working
/// manual config.
@MainActor
final class PhoneSync: NSObject, WCSessionDelegate {
    private weak var store: TaskStore?

    /// Wire to the store and activate the session. Call once, on app launch.
    func start(store: TaskStore) {
        self.store = store
        guard WCSession.isSupported() else { return }
        let session = WCSession.default
        session.delegate = self
        session.activate()
        // The phone may have delivered a context before we activated —
        // `receivedApplicationContext` holds the latest one, replayed here.
        let ctx = session.receivedApplicationContext
        apply(
            base: ctx["baseURL"] as? String,
            slug: ctx["orgSlug"] as? String,
            token: ctx["token"] as? String
        )
    }

    /// Apply the three config fields to the store (main-actor). Takes plain
    /// `String?`s (Sendable) rather than the `[String: Any]` context, so the
    /// nonisolated delegate callbacks can extract values on their own actor
    /// and hand only Sendable data across the main-actor hop (Swift 6 strict
    /// concurrency — a captured `[String: Any]` would race).
    private func apply(base: String?, slug: String?, token: String?) {
        guard let store else { return }
        if let base, !base.isEmpty { store.baseURL = base }
        if let slug, !slug.isEmpty { store.orgSlug = slug }
        if let token, !token.isEmpty { store.token = token }
    }

    // ── WCSessionDelegate ──
    // On watchOS the only required callback is activation completion; the
    // inactive/deactivate pair is iOS-only. Delegate callbacks arrive off the
    // main actor, so extract the Sendable values here, then hop to apply them.

    nonisolated func session(
        _ session: WCSession,
        activationDidCompleteWith activationState: WCSessionActivationState,
        error: Error?
    ) {
        let ctx = session.receivedApplicationContext
        let base = ctx["baseURL"] as? String
        let slug = ctx["orgSlug"] as? String
        let token = ctx["token"] as? String
        Task { @MainActor [weak self] in self?.apply(base: base, slug: slug, token: token) }
    }

    nonisolated func session(
        _ session: WCSession,
        didReceiveApplicationContext applicationContext: [String: Any]
    ) {
        let base = applicationContext["baseURL"] as? String
        let slug = applicationContext["orgSlug"] as? String
        let token = applicationContext["token"] as? String
        Task { @MainActor [weak self] in self?.apply(base: base, slug: slug, token: token) }
    }
}
