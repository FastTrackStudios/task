// The macOS half of the cloud folder.
//
// Linux mounts a FUSE filesystem and the kernel asks it for everything.
// macOS has no such seam: the system loads this extension from the app
// bundle and asks *it*, through `NSFileProviderReplicatedExtension`.
// The behaviour on the other side is meant to be identical — a tree
// that lists at real sizes, and a file whose bytes come back because
// somebody opened it.
//
// # What this does and does not own
//
// It does not own the store. The sync agent does, because hydration
// writes into a jj repo whose locking assumes a single writer, and two
// processes holding it is the bug avoided here by construction. So
// `fetchContents` asks the agent, over the same control socket the CLI
// and the app use, and the agent materializes into the live tree.
//
// The live tree is on disk, so enumerating and writing are ordinary
// FileManager work against it. That is what keeps this extension small
// enough to trust: it is a translation layer, not a second engine.

import FileProvider
import Foundation
import os

private let log = Logger(subsystem: "app.fasttrackstudio.task", category: "fileprovider")

@objc(FileProviderExtension)
final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension {
    /// The domain's identifier is the root's id — see `domains.swift`
    /// in the app, which creates one domain per synced root.
    private let rootID: String
    /// Where the root's live tree is, once somebody has asked.
    ///
    /// **Nothing may block in `init`.** The system constructs the
    /// extension as part of launching it and gives that a short leash;
    /// asking the agent there cost the launch, and the system answered
    /// by killing the process and starting another — a respawn every
    /// fifteen seconds, no log line from us because we never got far
    /// enough to write one, and a folder that timed out in Finder
    /// rather than saying anything. So the lookup happens on the first
    /// call that needs it, where being slow is merely slow.
    private var cachedTree: URL?
    private let treeLock = NSLock()

    required init(domain: NSFileProviderDomain) {
        self.rootID = domain.identifier.rawValue
        super.init()
    }

    /// The agent is the authority on where a root's tree is; the domain
    /// only carries its id. Cached after the first success — the answer
    /// does not change while a domain exists, and every enumeration
    /// would otherwise pay for a fresh connection.
    private var treeRoot: URL {
        treeLock.lock()
        defer { treeLock.unlock() }
        if let cachedTree { return cachedTree }
        do {
            guard let path = try Bridge.roots().first(where: { $0.id == rootID })?.path else {
                log.error("the agent holds no root \(self.rootID, privacy: .public)")
                return URL(fileURLWithPath: "/nonexistent")
            }
            let url = URL(fileURLWithPath: path)
            cachedTree = url
            log.info("root \(self.rootID, privacy: .public) lives at \(path, privacy: .public)")
            return url
        } catch {
            // Not cached: the agent may simply have been starting, and
            // the next call should ask again rather than being wrong
            // for the life of the domain.
            log.error("cannot reach the sync agent: \(error.localizedDescription, privacy: .public)")
            return URL(fileURLWithPath: "/nonexistent")
        }
    }

    func invalidate() {}

    // ── Reading ────────────────────────────────────────────────────

    func item(for identifier: NSFileProviderItemIdentifier,
              request _: NSFileProviderRequest,
              completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void)
        -> Progress {
        let progress = Progress(totalUnitCount: 1)
        let relative = identifier == .rootContainer ? "" : identifier.rawValue
        let path = relative.isEmpty ? treeRoot : treeRoot.appendingPathComponent(relative)

        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path.path, isDirectory: &isDir) else {
            completionHandler(nil, NSFileProviderError(.noSuchItem))
            return progress
        }
        let modified = Disk.modified(of: path.path)

        if isDir.boolValue {
            completionHandler(Item(relativePath: relative, isDirectory: true, size: 0,
                                   modified: modified, resident: true), nil)
        } else {
            let facts = try? Bridge.facts(of: path.path)
            completionHandler(Item(relativePath: relative, isDirectory: false,
                                   size: facts?.size ?? 0, modified: modified,
                                   resident: !(facts?.dehydrated ?? false)), nil)
        }
        progress.completedUnitCount = 1
        return progress
    }

    func enumerator(for containerItemIdentifier: NSFileProviderItemIdentifier,
                    request _: NSFileProviderRequest) throws -> NSFileProviderEnumerator {
        Enumerator(treeRoot: treeRoot, container: containerItemIdentifier)
    }

    /// The call this whole extension exists for: something wants the
    /// bytes of a file this machine may not be holding.
    ///
    /// It hydrates first — through the agent, so the store stays
    /// single-writer — and only then hands over a copy. The caller
    /// waits, which is the honest answer and the same one a slow disk
    /// gives.
    func fetchContents(for itemIdentifier: NSFileProviderItemIdentifier,
                       version _: NSFileProviderItemVersion?,
                       request _: NSFileProviderRequest,
                       completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void)
        -> Progress {
        let progress = Progress(totalUnitCount: 100)
        let relative = itemIdentifier.rawValue
        let source = treeRoot.appendingPathComponent(relative)

        DispatchQueue.global(qos: .userInitiated).async { [rootID, treeRoot] in
            do {
                if (try? Bridge.facts(of: source.path))?.dehydrated == true {
                    log.info("fetching \(relative, privacy: .public)")
                    try Bridge.hydrate(root: rootID, path: relative)
                }
                progress.completedUnitCount = 90

                // The system takes ownership of the file at the URL it
                // is handed, so this must be a copy: handing over the
                // tree's own file would let the system move the user's
                // content out from under the sync engine.
                let staged = FileManager.default.temporaryDirectory
                    .appendingPathComponent(UUID().uuidString)
                try FileManager.default.copyItem(at: source, to: staged)

                let modified = Disk.modified(of: source.path)
                let size = (try? Bridge.facts(of: source.path).size) ?? 0
                let item = Item(relativePath: relative, isDirectory: false, size: size,
                                modified: modified, resident: true)
                progress.completedUnitCount = 100
                completionHandler(staged, item, nil)
                _ = treeRoot
            } catch {
                log.error("fetching \(relative, privacy: .public): \(error.localizedDescription, privacy: .public)")
                completionHandler(nil, nil, error)
            }
        }
        return progress
    }

    // ── Writing ────────────────────────────────────────────────────
    //
    // Writes go straight into the live tree, which is what the Linux
    // mount does too: an edit through the cloud folder is an ordinary
    // edit, so the agent's watcher and the checkpoint path see it the
    // way they see any other.

    func createItem(basedOn itemTemplate: NSFileProviderItem,
                    fields _: NSFileProviderItemFields,
                    contents url: URL?,
                    options _: NSFileProviderCreateItemOptions = [],
                    request _: NSFileProviderRequest,
                    completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void)
        -> Progress {
        let progress = Progress(totalUnitCount: 1)
        let parent = itemTemplate.parentItemIdentifier == .rootContainer
            ? "" : itemTemplate.parentItemIdentifier.rawValue
        let relative = parent.isEmpty
            ? itemTemplate.filename : "\(parent)/\(itemTemplate.filename)"
        let destination = treeRoot.appendingPathComponent(relative)

        do {
            if itemTemplate.contentType == .folder {
                try FileManager.default.createDirectory(at: destination,
                                                        withIntermediateDirectories: true)
            } else if let url {
                try? FileManager.default.removeItem(at: destination)
                try FileManager.default.copyItem(at: url, to: destination)
            } else {
                FileManager.default.createFile(atPath: destination.path, contents: nil)
            }
            let size = Disk.size(of: destination.path)
            let item = Item(relativePath: relative,
                            isDirectory: itemTemplate.contentType == .folder,
                            size: size, modified: Date(), resident: true)
            completionHandler(item, [], false, nil)
        } catch {
            completionHandler(nil, [], false, error)
        }
        progress.completedUnitCount = 1
        return progress
    }

    func modifyItem(_ item: NSFileProviderItem,
                    baseVersion _: NSFileProviderItemVersion,
                    changedFields: NSFileProviderItemFields,
                    contents url: URL?,
                    options _: NSFileProviderModifyItemOptions = [],
                    request _: NSFileProviderRequest,
                    completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void)
        -> Progress {
        let progress = Progress(totalUnitCount: 1)
        var relative = item.itemIdentifier.rawValue
        let path = treeRoot.appendingPathComponent(relative)

        do {
            if changedFields.contains(.contents), let url {
                try? FileManager.default.removeItem(at: path)
                try FileManager.default.copyItem(at: url, to: path)
            }
            if changedFields.contains(.filename) || changedFields.contains(.parentItemIdentifier) {
                let parent = item.parentItemIdentifier == .rootContainer
                    ? "" : item.parentItemIdentifier.rawValue
                let moved = parent.isEmpty ? item.filename : "\(parent)/\(item.filename)"
                let destination = treeRoot.appendingPathComponent(moved)
                try FileManager.default.moveItem(at: path, to: destination)
                relative = moved
            }
            let size = Disk.size(of: treeRoot.appendingPathComponent(relative).path)
            completionHandler(Item(relativePath: relative,
                                   isDirectory: item.contentType == .folder,
                                   size: size, modified: Date(), resident: true),
                              [], false, nil)
        } catch {
            completionHandler(nil, [], false, error)
        }
        progress.completedUnitCount = 1
        return progress
    }

    func deleteItem(identifier: NSFileProviderItemIdentifier,
                    baseVersion _: NSFileProviderItemVersion,
                    options _: NSFileProviderDeleteItemOptions = [],
                    request _: NSFileProviderRequest,
                    completionHandler: @escaping (Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        let path = treeRoot.appendingPathComponent(identifier.rawValue)
        do {
            try FileManager.default.removeItem(at: path)
            completionHandler(nil)
        } catch {
            completionHandler(error)
        }
        progress.completedUnitCount = 1
        return progress
    }
}
