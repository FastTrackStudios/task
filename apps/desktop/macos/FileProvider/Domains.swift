// Registering one File Provider domain per synced root.
//
// Only the containing app may do this — an extension cannot add its own
// domain — so this is built as a small tool inside the bundle that the
// app runs, rather than as part of the extension. It is idempotent by
// design: it adds domains for roots that have none, removes domains for
// roots the agent no longer holds, and leaves the rest alone, so
// running it on every launch is the intended use.
//
//   TaskFileProviderDomains sync     make the domains match the agent
//   TaskFileProviderDomains list     what is registered right now
//   TaskFileProviderDomains clear    remove them all
//
// A domain's identifier is the root's id, which is what the extension
// reads back in `init(domain:)` to find the tree.

import FileProvider
import Foundation

@main
enum Domains {
    static func main() async {
        let verb = CommandLine.arguments.dropFirst().first ?? "sync"
        do {
            switch verb {
            case "sync": try await sync()
            case "list": try await list()
            case "clear": try await clear()
            case "roots": try roots()
            case "facts":
                guard let path = CommandLine.arguments.dropFirst(2).first else {
                    FileHandle.standardError.write(Data("facts needs a path\n".utf8))
                    exit(2)
                }
                let facts = try Bridge.facts(of: path)
                print("size        \(facts.size)")
                print("dehydrated  \(facts.dehydrated)")
                print("executable  \(facts.executable)")
            default:
                FileHandle.standardError.write(Data("usage: TaskFileProviderDomains sync|list|clear|roots|facts <path>\n".utf8))
                exit(2)
            }
        } catch {
            // The domain and code, not just the sentence. macOS answers
            // most refusals here with "The application cannot be used
            // right now", which is the same string for an unregistered
            // extension, a missing entitlement and a bundle the system
            // will not trust — three different problems with three
            // different fixes, and the code is what tells them apart.
            let ns = error as NSError
            FileHandle.standardError.write(Data(
                "\(ns.localizedDescription) [\(ns.domain) \(ns.code)]\n".utf8))
            for (key, value) in ns.userInfo {
                FileHandle.standardError.write(Data("    \(key): \(value)\n".utf8))
            }
            exit(1)
        }
    }

    /// Make the registered domains match what the agent holds.
    static func sync() async throws {
        let roots = try Bridge.roots()
        let existing = try await NSFileProviderManager.domains()

        let wanted = Set(roots.map(\.id))
        let have = Set(existing.map(\.identifier.rawValue))

        for root in roots where !have.contains(root.id) {
            let domain = NSFileProviderDomain(
                identifier: NSFileProviderDomainIdentifier(root.id),
                displayName: root.name)
            try await NSFileProviderManager.add(domain)
            print("added \(root.name)")
        }

        // A root the agent stopped holding leaves a domain behind that
        // enumerates nothing — worse than absent, because it looks like
        // an empty project rather than a missing one.
        for domain in existing where !wanted.contains(domain.identifier.rawValue) {
            try await NSFileProviderManager.remove(domain)
            print("removed \(domain.displayName)")
        }

        if roots.isEmpty {
            print("the agent holds no roots — nothing to show in Finder yet")
        }
    }

    /// What the bridge can see — the diagnostic that separates "the
    /// extension is broken" from "the agent is not running", which from
    /// Finder look identical.
    ///
    /// Worth having as its own verb because registering a domain needs
    /// a signed bundle and an entitlement, and reaching the agent needs
    /// neither: this half can be checked from a terminal on a machine
    /// where the other half cannot be.
    static func roots() throws {
        let roots = try Bridge.roots()
        if roots.isEmpty {
            print("the agent is answering, and holds no roots")
            return
        }
        for root in roots {
            print("\(root.name)  \(root.id)")
            print("    \(root.path)")
        }
    }

    static func list() async throws {
        let domains = try await NSFileProviderManager.domains()
        if domains.isEmpty {
            print("no domains registered")
            return
        }
        for domain in domains {
            // Whether the user has it switched on, because a domain
            // that is registered and disabled looks exactly like one
            // that is working until you touch it: the folder is there,
            // and everything inside it fails with "Sync is not enabled"
            // — which reaches Finder as a listing that hangs, and says
            // nothing about a switch in System Settings.
            let state = domain.userEnabled
                ? "on"
                : "OFF — switch it on in System Settings › General › Login Items & Extensions › File Providers"
            print("\(domain.displayName)  \(domain.identifier.rawValue)  \(state)")
        }
    }

    static func clear() async throws {
        for domain in try await NSFileProviderManager.domains() {
            try await NSFileProviderManager.remove(domain)
            print("removed \(domain.displayName)")
        }
    }
}
