// Listing a directory.
//
// The live tree is already on disk — materialize put it there — so
// enumeration is `FileManager` plus one question the bridge answers: is
// this entry a pointer stub, and if so how big is the thing it stands
// for. That is the whole difference between this and a plain folder.

import FileProvider
import Foundation

final class Enumerator: NSObject, NSFileProviderEnumerator {
    private let treeRoot: URL
    private let container: NSFileProviderItemIdentifier

    init(treeRoot: URL, container: NSFileProviderItemIdentifier) {
        self.treeRoot = treeRoot
        self.container = container
    }

    func invalidate() {}

    func enumerateItems(for observer: NSFileProviderEnumerationObserver,
                        startingAt _: NSFileProviderPage) {
        let relative = container == .rootContainer ? "" : container.rawValue
        let directory = relative.isEmpty ? treeRoot : treeRoot.appendingPathComponent(relative)

        let names: [String]
        do {
            names = try FileManager.default.contentsOfDirectory(atPath: directory.path)
        } catch {
            observer.finishEnumeratingWithError(error)
            return
        }

        var items: [Item] = []
        for name in names {
            // The version store's own bookkeeping is not the user's
            // content, and showing it in Finder would invite somebody
            // to delete it.
            if name == ".fts-files" || name == ".fts-root.json" { continue }

            let child = directory.appendingPathComponent(name)
            let childRelative = relative.isEmpty ? name : "\(relative)/\(name)"

            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: child.path, isDirectory: &isDir) else {
                continue  // vanished mid-listing; the next enumeration will agree
            }
            let modified = Disk.modified(of: child.path)

            if isDir.boolValue {
                items.append(Item(relativePath: childRelative, isDirectory: true, size: 0,
                                  modified: modified, resident: true))
                continue
            }

            // The one call that makes this a cloud folder rather than a
            // folder. A failure here is not fatal to the listing: the
            // entry is reported as the ordinary file its bytes are, so
            // one odd file cannot make a directory unlistable.
            let facts = try? Bridge.facts(of: child.path)
            let size = facts?.size ?? Disk.size(of: child.path)
            items.append(Item(relativePath: childRelative, isDirectory: false, size: size,
                              modified: modified,
                              resident: !(facts?.dehydrated ?? false)))
        }

        observer.didEnumerate(items)
        observer.finishEnumerating(upTo: nil)
    }

    /// Changes since an anchor.
    ///
    /// Not implemented yet, and saying so is better than answering
    /// "nothing changed": the system takes an empty change set at face
    /// value and would show a stale tree indefinitely. Reporting the
    /// anchor as expired makes it re-enumerate, which is correct and
    /// merely less efficient — the honest trade until this watches the
    /// tree the way the Linux agent's watcher does.
    func enumerateChanges(for observer: NSFileProviderChangeObserver,
                          from _: NSFileProviderSyncAnchor) {
        observer.finishEnumeratingWithError(
            NSFileProviderError(.syncAnchorExpired))
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        completionHandler(nil)
    }
}
