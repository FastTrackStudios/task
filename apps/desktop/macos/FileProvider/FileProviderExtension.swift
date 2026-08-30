// The macOS half of the cloud folder.
//
// Linux mounts a FUSE filesystem and the kernel asks it for everything.
// macOS has no such seam: the system loads this extension from the app
// bundle and asks *it*, through `NSFileProviderReplicatedExtension`.
// The behaviour on the other side is meant to be identical — a tree
// that lists at real sizes, and a file whose bytes come back because
// somebody opened it.
//
// # One domain, many roots
//
// This machine holds forty-six roots. A domain each would put
// forty-six unrelated folders in Finder's sidebar, which is the layout
// the `place` mechanism exists to replace. So there is one domain and
// the hierarchy above each root is synthesised from the places — see
// `Tree`. The Linux mount does the same thing by mounting each root at
// its place and letting the kernel invent the parents; there is no
// kernel to hand that to here, so the extension invents them itself.
//
// # What this does and does not own
//
// It does not own the store. The sync agent does, because hydration
// writes into a jj repo whose locking assumes a single writer, and two
// processes holding it is the bug avoided here by construction. So
// `fetchContents` asks the agent, over the same control socket the CLI
// and the app use, and the agent materializes into the live tree.

import FileProvider
import Foundation
import os

private let log = Logger(subsystem: "app.fasttrackstudio.task", category: "fileprovider")

@objc(FileProviderExtension)
final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension {
    required init(domain: NSFileProviderDomain) {
        super.init()
    }

    func invalidate() {}

    /// The roots, freshly asked for.
    ///
    /// **Nothing may block in `init`** — the system constructs the
    /// extension while launching it and kills a launch that takes too
    /// long, which cost this a respawn every fifteen seconds with no log
    /// line to show for it. So the agent is asked here, on a call that
    /// is allowed to be slow, and not before.
    private func tree() -> Tree {
        do {
            return Tree(roots: try Bridge.roots())
        } catch {
            log.error("cannot reach the sync agent: \(error.localizedDescription, privacy: .public)")
            return Tree(roots: [])
        }
    }

    /// A path in the composed tree, from an identifier.
    private func path(of identifier: NSFileProviderItemIdentifier) -> String {
        identifier == .rootContainer ? "" : identifier.rawValue
    }

    // ── Reading ────────────────────────────────────────────────────

    func item(for identifier: NSFileProviderItemIdentifier,
              request _: NSFileProviderRequest,
              completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void)
        -> Progress {
        let progress = Progress(totalUnitCount: 1)
        defer { progress.completedUnitCount = 1 }

        let shown = path(of: identifier)
        guard let resolved = tree().resolve(shown) else {
            completionHandler(nil, NSFileProviderError(.noSuchItem))
            return progress
        }

        switch resolved {
        case .synthetic:
            // A directory that exists only in the layout. It has no
            // mtime of its own to report, so it reports now — which is
            // honest for something that is a view rather than a file.
            completionHandler(
                Item(relativePath: shown, isDirectory: true, size: 0,
                     modified: Date(), resident: true), nil)
        case let .inRoot(root, relative):
            let disk = relative.isEmpty
                ? root.path
                : root.path + "/" + relative
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: disk, isDirectory: &isDir) else {
                completionHandler(nil, NSFileProviderError(.noSuchItem))
                return progress
            }
            let facts = isDir.boolValue ? nil : try? Bridge.facts(of: disk)
            completionHandler(
                Item(relativePath: shown, isDirectory: isDir.boolValue,
                     size: facts?.size ?? 0, modified: Disk.modified(of: disk),
                     resident: !(facts?.dehydrated ?? false)), nil)
        }
        return progress
    }

    func enumerator(for containerItemIdentifier: NSFileProviderItemIdentifier,
                    request _: NSFileProviderRequest) throws -> NSFileProviderEnumerator {
        Enumerator(tree: tree(), container: path(of: containerItemIdentifier))
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
        let shown = path(of: itemIdentifier)

        guard case let .inRoot(root, relative)? = tree().resolve(shown) else {
            // A synthetic directory has no contents to fetch.
            completionHandler(nil, nil, NSFileProviderError(.noSuchItem))
            return progress
        }
        let source = root.path + "/" + relative

        DispatchQueue.global(qos: .userInitiated).async {
            do {
                if (try? Bridge.facts(of: source))?.dehydrated == true {
                    log.info("fetching \(shown, privacy: .public)")
                    try Bridge.hydrate(root: root.id, path: relative)
                }
                progress.completedUnitCount = 90

                // The system takes ownership of the file at the URL it
                // is handed, so this must be a copy: handing over the
                // tree's own file would let the system move the user's
                // content out from under the sync engine.
                let staged = FileManager.default.temporaryDirectory
                    .appendingPathComponent(UUID().uuidString)
                try FileManager.default.copyItem(
                    at: URL(fileURLWithPath: source), to: staged)

                let size = (try? Bridge.facts(of: source).size) ?? 0
                let item = Item(relativePath: shown, isDirectory: false, size: size,
                                modified: Disk.modified(of: source), resident: true)
                progress.completedUnitCount = 100
                completionHandler(staged, item, nil)
            } catch {
                log.error("fetching \(shown, privacy: .public): \(error.localizedDescription, privacy: .public)")
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
    //
    // A synthetic directory is not a place anything can be written: it
    // is a segment of a layout, not a folder on a disk, so creating
    // there is refused rather than silently landing somewhere.

    func createItem(basedOn itemTemplate: NSFileProviderItem,
                    fields _: NSFileProviderItemFields,
                    contents url: URL?,
                    options _: NSFileProviderCreateItemOptions = [],
                    request _: NSFileProviderRequest,
                    completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void)
        -> Progress {
        let progress = Progress(totalUnitCount: 1)
        defer { progress.completedUnitCount = 1 }

        let parent = path(of: itemTemplate.parentItemIdentifier)
        let shown = parent.isEmpty
            ? itemTemplate.filename : "\(parent)/\(itemTemplate.filename)"

        guard case let .inRoot(root, relative)? = tree().resolve(shown) else {
            completionHandler(nil, [], false, NSFileProviderError(.noSuchItem))
            return progress
        }
        let destination = URL(fileURLWithPath: root.path + "/" + relative)

        do {
            if itemTemplate.contentType == .folder {
                try FileManager.default.createDirectory(
                    at: destination, withIntermediateDirectories: true)
            } else if let url {
                try? FileManager.default.removeItem(at: destination)
                try FileManager.default.copyItem(at: url, to: destination)
            } else {
                FileManager.default.createFile(atPath: destination.path, contents: nil)
            }
            completionHandler(
                Item(relativePath: shown,
                     isDirectory: itemTemplate.contentType == .folder,
                     size: Disk.size(of: destination.path),
                     modified: Date(), resident: true),
                [], false, nil)
        } catch {
            completionHandler(nil, [], false, error)
        }
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
        defer { progress.completedUnitCount = 1 }

        let tree = tree()
        var shown = path(of: item.itemIdentifier)
        guard case let .inRoot(root, relative)? = tree.resolve(shown) else {
            completionHandler(nil, [], false, NSFileProviderError(.noSuchItem))
            return progress
        }
        let path = URL(fileURLWithPath: root.path + "/" + relative)

        do {
            if changedFields.contains(.contents), let url {
                try? FileManager.default.removeItem(at: path)
                try FileManager.default.copyItem(at: url, to: path)
            }
            if changedFields.contains(.filename) || changedFields.contains(.parentItemIdentifier) {
                let parent = self.path(of: item.parentItemIdentifier)
                let moved = parent.isEmpty ? item.filename : "\(parent)/\(item.filename)"
                // A move must land inside a root — across roots it would
                // be a different tree with a different history, which is
                // a sync decision and not a rename.
                guard case let .inRoot(destRoot, destRelative)? = tree.resolve(moved),
                      destRoot.id == root.id
                else {
                    completionHandler(nil, [], false, NSFileProviderError(.noSuchItem))
                    return progress
                }
                try FileManager.default.moveItem(
                    at: path, to: URL(fileURLWithPath: destRoot.path + "/" + destRelative))
                shown = moved
            }
            completionHandler(
                Item(relativePath: shown, isDirectory: item.contentType == .folder,
                     size: Disk.size(of: path.path), modified: Date(), resident: true),
                [], false, nil)
        } catch {
            completionHandler(nil, [], false, error)
        }
        return progress
    }

    func deleteItem(identifier: NSFileProviderItemIdentifier,
                    baseVersion _: NSFileProviderItemVersion,
                    options _: NSFileProviderDeleteItemOptions = [],
                    request _: NSFileProviderRequest,
                    completionHandler: @escaping (Error?) -> Void) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        defer { progress.completedUnitCount = 1 }

        guard case let .inRoot(root, relative)? = tree().resolve(path(of: identifier)),
              !relative.isEmpty
        else {
            // Refusing to delete a root through Finder is deliberate:
            // that is "stop holding this project", a decision with sync
            // consequences, and it belongs to `unshare`.
            completionHandler(NSFileProviderError(.noSuchItem))
            return progress
        }
        do {
            try FileManager.default.removeItem(atPath: root.path + "/" + relative)
            completionHandler(nil)
        } catch {
            completionHandler(error)
        }
        return progress
    }
}
