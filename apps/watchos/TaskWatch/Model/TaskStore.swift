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
            await refreshSuggestion()
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
            await refreshSuggestion()
        } catch {
            lastError = (error as? LocalizedError)?.errorDescription ?? "\(error)"
            WKInterfaceDevice.current().play(.failure)
        }
    }

    // ── Suggested ──
    //
    // The one thing worth doing now — the same pick as the Tasks page's
    // Suggested slot, computed by the server at the watch's local date and
    // minute. "Not now" is kept here, per day, like the web keeps it per
    // device: `<date>|key|key…` under `suggested.declined`.

    /// What to show at the top of the Timer tab, or nil.
    var suggestion: SuggestionDto?

    private static let declinedKey = "suggested.declined"

    /// The watch's local date (YYYY-MM-DD) and minute since midnight.
    private func localNow() -> (date: String, minute: Int) {
        let now = Date()
        let cal = Calendar.current
        let c = cal.dateComponents([.year, .month, .day, .hour, .minute], from: now)
        let date = String(format: "%04d-%02d-%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
        return (date, (c.hour ?? 0) * 60 + (c.minute ?? 0))
    }

    private func declined(on date: String) -> [String] {
        let raw = UserDefaults.standard.string(forKey: Self.declinedKey) ?? ""
        var parts = raw.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
        guard parts.first == date else { return [] }
        parts.removeFirst()
        return parts.filter { !$0.isEmpty }
    }

    private func saveDeclined(_ keys: [String], on date: String) {
        UserDefaults.standard.set(([date] + keys).joined(separator: "|"), forKey: Self.declinedKey)
    }

    func refreshSuggestion() async {
        guard isConfigured else { return }
        let now = localNow()
        do {
            suggestion = try await client.suggested(
                date: now.date, minute: now.minute, declined: declined(on: now.date))
        } catch {
            // The glance is a nicety: keep the last one, don't shout.
        }
    }

    /// "Not now": set it aside for today and show the next one.
    func notNow() async {
        guard let s = suggestion else { return }
        let now = localNow()
        saveDeclined(declined(on: now.date) + [s.key], on: now.date)
        suggestion = nil
        await refreshSuggestion()
    }

    /// The card's primary action (Start / Done). Process is the phone's.
    func actOnSuggestion() async {
        guard isConfigured, !busy, let s = suggestion, s.action != "Process" else { return }
        busy = true
        defer { busy = false }
        let now = localNow()
        do {
            let out = try await client.actOnSuggestion(
                key: s.key, date: now.date, minute: now.minute, declined: declined(on: now.date))
            if let t = out.timer { active = t }
            suggestion = out.next
            lastError = nil
            WKInterfaceDevice.current().play(out.timer != nil ? .start : .success)
        } catch TaskClientError.http(409, _) {
            // It stopped applying (another device acted, the hour moved on).
            await refreshSuggestion()
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
