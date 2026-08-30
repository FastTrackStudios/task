// The composed tree, as the extension resolves it.
//
// A File Provider domain is a window onto *one* thing, and this machine
// holds forty-six roots. One domain each would put forty-six folders in
// Finder's sidebar with no relationship between them — which is the
// layout the `place` mechanism exists to replace.
//
// So there is one domain, and the hierarchy above each root is
// **synthesised**: `codywright` and `Projects` are not directories
// anywhere on disk, they are segments of the places the agent reports.
// Below a root, every path is a real path in that root's live tree.
//
// This is the same trick the Linux mount plays by mounting each root at
// its place and letting the kernel make the parents. macOS gives us no
// kernel to hand that to, so the extension does it itself.

import Foundation

/// What a path in the composed tree turns out to be.
enum Resolved {
    /// A segment of somebody's place — `codywright`, or `Projects`.
    /// Exists only as a way to get further down.
    case synthetic
    /// Inside a root: which root, and where within its live tree.
    /// `relative` is empty for the root's own directory.
    case inRoot(root: Root, relative: String)
}

/// The roots, and the tree their places describe.
///
/// Rebuilt rather than cached across calls: roots are shared and
/// unshared while the extension is loaded, and a tree that remembered
/// the answer would keep showing a project somebody removed. The cost
/// is one call to the agent, which is a local socket.
struct Tree {
    let roots: [Root]

    init(roots: [Root]) {
        self.roots = roots
    }

    /// The roots whose place sits at or below `path`.
    private func below(_ path: String) -> [Root] {
        guard !path.isEmpty else { return roots }
        return roots.filter { $0.place == path || $0.place.hasPrefix(path + "/") }
    }

    /// What `path` is — a synthetic parent, a root, something inside a
    /// root, or nothing at all.
    func resolve(_ path: String) -> Resolved? {
        if path.isEmpty { return .synthetic }

        // The longest place that is a prefix of this path wins: a root
        // placed at `a/b` and one at `a/b/c` are both prefixes of
        // `a/b/c/take.wav`, and the answer is the deeper one.
        let owner = roots
            .filter { path == $0.place || path.hasPrefix($0.place + "/") }
            .max(by: { $0.place.count < $1.place.count })

        if let owner {
            let relative = path == owner.place
                ? ""
                : String(path.dropFirst(owner.place.count + 1))
            return .inRoot(root: owner, relative: relative)
        }

        // Not inside any root, but on the way to one.
        return below(path).isEmpty ? nil : .synthetic
    }

    /// The immediate children of a synthetic directory, as names.
    ///
    /// `codywright` given the places `codywright/Projects/A` and
    /// `codywright/Vault` answers `["Projects", "Vault"]` — each once,
    /// however many roots sit under it.
    func syntheticChildren(of path: String) -> [String] {
        let prefix = path.isEmpty ? "" : path + "/"
        var seen: Set<String> = []
        var names: [String] = []
        for root in roots where root.place.hasPrefix(prefix) {
            let rest = String(root.place.dropFirst(prefix.count))
            guard let first = rest.split(separator: "/").first.map(String.init) else { continue }
            if seen.insert(first).inserted {
                names.append(first)
            }
        }
        return names
    }

    /// Is this synthetic path also a root's own place? Then it is a
    /// real directory and its children come from disk, not from here.
    func isRootPlace(_ path: String) -> Bool {
        roots.contains { $0.place == path }
    }
}
