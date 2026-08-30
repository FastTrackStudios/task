// Listing a directory in the composed tree.
//
// Two kinds of directory arrive here. A **synthetic** one — `codywright`,
// `Projects` — exists only as a segment of somebody's place, and its
// children are the next segments of the places below it. A **real** one
// is inside a root, and its children come from the disk, with the one
// question the filesystem cannot answer for itself: is this entry a
// pointer stub, and if so how big is the thing it stands for.

import FileProvider
import Foundation

final class Enumerator: NSObject, NSFileProviderEnumerator {
    private let tree: Tree
    /// The path being listed, in the composed tree. Empty for the root.
    private let container: String

    init(tree: Tree, container: String) {
        self.tree = tree
        self.container = container
    }

    func invalidate() {}

    func enumerateItems(for observer: NSFileProviderEnumerationObserver,
                        startingAt _: NSFileProviderPage) {
        guard let resolved = tree.resolve(container) else {
            observer.finishEnumeratingWithError(NSFileProviderError(.noSuchItem))
            return
        }

        var items: [Item] = []

        switch resolved {
        case .synthetic:
            for name in tree.syntheticChildren(of: container) {
                let child = container.isEmpty ? name : "\(container)/\(name)"
                items.append(Item(relativePath: child, isDirectory: true, size: 0,
                                  modified: Date(), resident: true))
            }
        case let .inRoot(root, relative):
            let directory = relative.isEmpty ? root.path : root.path + "/" + relative
            let names: [String]
            do {
                names = try FileManager.default.contentsOfDirectory(atPath: directory)
            } catch {
                observer.finishEnumeratingWithError(error)
                return
            }
            for name in names {
                // The version store's own bookkeeping is not the user's
                // content, and showing it in Finder would invite
                // somebody to delete it.
                if name == ".fts-files" || name == ".fts-root.json" { continue }

                let disk = directory + "/" + name
                let child = container.isEmpty ? name : "\(container)/\(name)"

                var isDir: ObjCBool = false
                guard FileManager.default.fileExists(atPath: disk, isDirectory: &isDir) else {
                    continue  // vanished mid-listing; the next pass will agree
                }
                let modified = Disk.modified(of: disk)

                if isDir.boolValue {
                    items.append(Item(relativePath: child, isDirectory: true, size: 0,
                                      modified: modified, resident: true))
                    continue
                }

                // The one call that makes this a cloud folder rather
                // than a folder. A failure is not fatal to the listing:
                // the entry is reported as the ordinary file its bytes
                // are, so one odd file cannot make a directory
                // unlistable.
                let facts = try? Bridge.facts(of: disk)
                items.append(Item(relativePath: child, isDirectory: false,
                                  size: facts?.size ?? Disk.size(of: disk),
                                  modified: modified,
                                  resident: !(facts?.dehydrated ?? false)))
            }
        }

        observer.didEnumerate(items)
        observer.finishEnumerating(upTo: nil)
    }

    /// Changes since an anchor.
    ///
    /// This provider keeps no change journal, so it cannot say *what*
    /// changed — only whether anything did. The framework has a way to
    /// say exactly that: fail with `syncAnchorExpired`, and the system
    /// drops its cache and enumerates the container again.
    ///
    /// The trap is that it is only an answer when something *has*
    /// changed. Returning it unconditionally — which is what this did —
    /// makes every re-enumeration end by asking for another one. The
    /// system obliges, forever: the extension relaunches every few
    /// seconds, the folder never settles, and `ls` on it times out
    /// rather than failing, because from the outside the enumeration has
    /// not finished, it has restarted.
    func enumerateChanges(for observer: NSFileProviderChangeObserver,
                          from syncAnchor: NSFileProviderSyncAnchor) {
        let now = anchor()
        if syncAnchor == now {
            observer.finishEnumeratingChanges(upTo: now, moreComing: false)
        } else {
            observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
        }
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        completionHandler(anchor())
    }

    /// What "nothing has changed in here" is spelled as.
    ///
    /// For a real directory, its own modification time: the filesystem
    /// bumps it on every add, remove and rename in it. That is the right
    /// granularity — content changes are the item version's job, and an
    /// edit through the folder is the system's own write, which it
    /// already knows about.
    ///
    /// For a synthetic one, the set of places below it, which is what
    /// its listing is made of: sharing or unsharing a project changes
    /// the string, and nothing else does.
    private func anchor() -> NSFileProviderSyncAnchor {
        let stamp: String
        switch tree.resolve(container) {
        case let .inRoot(root, relative)?:
            let directory = relative.isEmpty ? root.path : root.path + "/" + relative
            stamp = String(Disk.modified(of: directory).timeIntervalSince1970)
        case .synthetic?:
            stamp = tree.syntheticChildren(of: container).sorted().joined(separator: "\u{1}")
        case nil:
            stamp = ""
        }
        return NSFileProviderSyncAnchor(Data(stamp.utf8))
    }
}
