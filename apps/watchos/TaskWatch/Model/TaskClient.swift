// URLSession client for the Task server's `/watch/v1` HTTP bridge. watchOS
// forbids WebSockets outside audio sessions (Apple TN3135), so the watch
// cannot speak vox — the server exposes a plain HTTP facade over the same
// timer + inbox vox services (see apps/task/server/src/watch_bridge.rs).
// Every request carries `Authorization: Bearer <token>`.

import Foundation

/// A running-or-closed work session, mirroring the server's `TimerDto`.
struct TimerDto: Codable, Equatable, Sendable {
    let id: String
    let description: String
    let startTime: String
    let endTime: String?
    let running: Bool

    enum CodingKeys: String, CodingKey {
        case id, description, running
        case startTime = "start_time"
        case endTime = "end_time"
    }

    /// Seconds elapsed since `startTime` (ISO-8601), clamped at 0.
    func elapsed(now: Date = Date()) -> TimeInterval {
        guard let start = TimerDto.iso.date(from: startTime) else { return 0 }
        return max(0, now.timeIntervalSince(start))
    }

    // Parsing is read-only + internally synchronized; safe to share.
    nonisolated(unsafe) private static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
}

enum TaskClientError: Error, LocalizedError {
    case notConfigured
    case badURL
    case http(Int, String)
    case transport(String)

    var errorDescription: String? {
        switch self {
        case .notConfigured: return "Set the server, org, and token in Settings."
        case .badURL: return "Invalid server URL."
        case .http(let code, let body): return "Server \(code): \(body)"
        case .transport(let m): return m
        }
    }
}

/// Immutable connection config; rebuilt whenever Settings change.
struct TaskServerConfig: Equatable, Sendable {
    var baseURL: String   // e.g. "https://task.starcommand.live"
    var orgSlug: String   // e.g. "starcommand"
    var token: String

    var isComplete: Bool {
        !baseURL.trimmingCharacters(in: .whitespaces).isEmpty
            && !orgSlug.trimmingCharacters(in: .whitespaces).isEmpty
            && !token.isEmpty
    }
}

struct TaskClient {
    let config: TaskServerConfig
    private let session: URLSession

    init(config: TaskServerConfig) {
        self.config = config
        let c = URLSessionConfiguration.default
        c.timeoutIntervalForRequest = 12
        c.waitsForConnectivity = true
        self.session = URLSession(configuration: c)
    }

    // ── Timer ──

    func startTimer(description: String) async throws -> TimerDto {
        try await send("timer/start", method: "POST",
                       body: ["description": description], decode: TimerDto.self)
    }

    /// Returns the closed session, or nil when nothing was running (204).
    @discardableResult
    func stopTimer() async throws -> TimerDto? {
        try await send("timer/stop", method: "POST", body: nil, decode: TimerDto?.self)
    }

    func activeTimer() async throws -> TimerDto? {
        try await send("timer/active", method: "GET", body: nil, decode: TimerDto?.self)
    }

    // ── Inbox ──

    func capture(body: String) async throws {
        struct Ack: Codable { let id: String; let created: String }
        _ = try await send("inbox", method: "POST", body: ["body": body], decode: Ack.self)
    }

    // ── Transport ──

    private func send<T: Decodable>(
        _ path: String, method: String, body: [String: String]?, decode: T.Type
    ) async throws -> T {
        guard config.isComplete else { throw TaskClientError.notConfigured }
        let base = config.baseURL.trimmingCharacters(in: .whitespaces)
        let root = base.contains("://") ? base : "https://\(base)"
        let slug = config.orgSlug.trimmingCharacters(in: .whitespaces)
        guard var url = URL(string: root) else { throw TaskClientError.badURL }
        url.append(path: "org/\(slug)/watch/v1/\(path)")

        var req = URLRequest(url: url)
        req.httpMethod = method
        req.setValue("Bearer \(config.token)", forHTTPHeaderField: "Authorization")
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
        }

        let (data, resp): (Data, URLResponse)
        do {
            (data, resp) = try await session.data(for: req)
        } catch {
            throw TaskClientError.transport(error.localizedDescription)
        }
        let code = (resp as? HTTPURLResponse)?.statusCode ?? 0

        // 204 No Content → nil (the callers that can 204 decode into Optional).
        if code == 204 {
            return try JSONDecoder().decode(T.self, from: Data("null".utf8))
        }
        guard (200...299).contains(code) else {
            let msg = String(data: data, encoding: .utf8) ?? ""
            throw TaskClientError.http(code, msg)
        }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw TaskClientError.transport("decode: \(error.localizedDescription)")
        }
    }
}
