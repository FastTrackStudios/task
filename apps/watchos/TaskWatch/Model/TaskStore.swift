// The watch app's single observable state: server config + the current
// timer, plus the actions the two tabs call. All server IO funnels through
// TaskClient (the /watch/v1 HTTP bridge).

import Foundation
import SwiftUI
import WatchKit

@MainActor
@Observable
final class TaskStore {
    // ── Config (persisted) ──
    var baseURL: String { didSet { persist(); rebuild() } }
    var orgSlug: String { didSet { persist(); rebuild() } }
    var token: String { didSet { persist(); rebuild() } }

    // ── Live state ──
    /// The currently-running session, or nil. Refreshed on appear + poll.
    var active: TimerDto?
    var busy = false
    var lastError: String?
    /// Bumped once a second so running clocks re-render.
    var tick: UInt64 = 0

    private var client: TaskClient

    var config: TaskServerConfig { TaskServerConfig(baseURL: baseURL, orgSlug: orgSlug, token: token) }
    var isConfigured: Bool { config.isComplete }

    init() {
        let d = UserDefaults.standard
        let b = d.string(forKey: "baseURL") ?? ""
        let s = d.string(forKey: "orgSlug") ?? ""
        let t = d.string(forKey: "token") ?? ""
        baseURL = b
        orgSlug = s
        token = t
        client = TaskClient(config: TaskServerConfig(baseURL: b, orgSlug: s, token: t))
    }

    private func persist() {
        let d = UserDefaults.standard
        d.set(baseURL, forKey: "baseURL")
        d.set(orgSlug, forKey: "orgSlug")
        d.set(token, forKey: "token")
    }

    private func rebuild() { client = TaskClient(config: config) }

    // ── Timer actions ──

    func refreshActive() async {
        guard isConfigured else { return }
        do {
            active = try await client.activeTimer()
            lastError = nil
        } catch {
            lastError = (error as? LocalizedError)?.errorDescription ?? "\(error)"
        }
    }

    func start(description: String) async {
        guard isConfigured, !busy else { return }
        busy = true
        defer { busy = false }
        do {
            active = try await client.startTimer(description: description)
            lastError = nil
            WKInterfaceDevice.current().play(.start)
        } catch {
            lastError = (error as? LocalizedError)?.errorDescription ?? "\(error)"
            WKInterfaceDevice.current().play(.failure)
        }
    }

    func stop() async {
        guard isConfigured, !busy else { return }
        busy = true
        defer { busy = false }
        do {
            _ = try await client.stopTimer()
            active = nil
            lastError = nil
            WKInterfaceDevice.current().play(.stop)
        } catch {
            lastError = (error as? LocalizedError)?.errorDescription ?? "\(error)"
            WKInterfaceDevice.current().play(.failure)
        }
    }

    // ── Capture ──

    /// Returns true on success (so the view can clear its field + confirm).
    func capture(_ body: String) async -> Bool {
        let text = body.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isConfigured, !text.isEmpty, !busy else { return false }
        busy = true
        defer { busy = false }
        do {
            try await client.capture(body: text)
            lastError = nil
            WKInterfaceDevice.current().play(.success)
            return true
        } catch {
            lastError = (error as? LocalizedError)?.errorDescription ?? "\(error)"
            WKInterfaceDevice.current().play(.failure)
            return false
        }
    }
}
