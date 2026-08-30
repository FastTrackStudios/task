// The Rust half, as Swift sees it.
//
// `files-fileprovider` answers the two questions Swift has no business
// answering itself: how big a dehydrated file really is, and how to get
// its bytes back. Everything below is a thin, typed wrapper over that C
// ABI — no logic, so that the interesting decisions stay in one place
// and are tested there.

import Foundation

/// A call into the bridge that failed, carrying the reason the Rust
/// side left behind.
struct BridgeError: LocalizedError {
    let reason: String
    var errorDescription: String? { reason }

    /// Whatever the last call on this thread went wrong with, or a
    /// stand-in — a failure with no reason is still a failure, and
    /// swallowing it would leave Finder showing a spinner forever.
    static func last(_ fallback: String) -> BridgeError {
        guard let raw = fts_fp_last_error() else {
            return BridgeError(reason: fallback)
        }
        defer { fts_fp_free(raw) }
        return BridgeError(reason: String(cString: raw))
    }
}

/// What a path in a live tree really is.
struct Facts: Decodable {
    /// The size of the content, which for a dehydrated file is what its
    /// stub records rather than what `stat` reports.
    let size: UInt64
    /// Is this a placeholder rather than the content?
    let dehydrated: Bool
    let executable: Bool
}

/// One root the agent holds — the thing a File Provider domain is a
/// window onto.
struct Root: Decodable {
    let id: String
    let name: String
    let path: String
    /// Where it appears in the composed tree — `org/Projects/Name`.
    /// What `Tree` builds the hierarchy from.
    let place: String
}

/// The two stat questions this extension asks over and over, with the
/// optional-chaining spelled once instead of at every call site — and
/// with an answer rather than a crash when the file is gone, which mid
/// enumeration it sometimes is.
enum Disk {
    static func size(of path: String) -> UInt64 {
        let attrs = try? FileManager.default.attributesOfItem(atPath: path)
        return (attrs?[.size] as? NSNumber)?.uint64Value ?? 0
    }

    static func modified(of path: String) -> Date {
        let attrs = try? FileManager.default.attributesOfItem(atPath: path)
        return (attrs?[.modificationDate] as? Date) ?? Date()
    }
}

enum Bridge {
    static func facts(of path: String) throws -> Facts {
        guard let raw = fts_fp_facts(path) else {
            throw BridgeError.last("could not read \(path)")
        }
        defer { fts_fp_free(raw) }
        return try JSONDecoder().decode(Facts.self, from: Data(String(cString: raw).utf8))
    }

    static func roots() throws -> [Root] {
        guard let raw = fts_fp_roots() else {
            throw BridgeError.last("could not reach the sync agent")
        }
        defer { fts_fp_free(raw) }
        return try JSONDecoder().decode([Root].self, from: Data(String(cString: raw).utf8))
    }

    /// Bring one path's bytes back. Blocks — the caller is already on a
    /// background queue, and the system shows progress while it waits.
    static func hydrate(root: String, path: String) throws {
        if fts_fp_hydrate(root, path) != 0 {
            throw BridgeError.last("could not fetch \(path)")
        }
    }

    /// Give one path's bytes back to the disk.
    static func evict(root: String, path: String) throws {
        if fts_fp_evict(root, path) != 0 {
            throw BridgeError.last("could not evict \(path)")
        }
    }
}
