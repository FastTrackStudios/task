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
    /// This provider does not keep a change journal, so it cannot say
    /// *what* changed — only whether anything did. The framework has a
    /// way to say exactly that: fail with `syncAnchorExpired`, and the
    /// system drops its cache and enumerates the container again.
    ///
    /// The trap is that it is only an answer when something *has*
    /// changed. Returning it unconditionally — which is what this did —
    /// makes every re-enumeration end by asking for another one. The
    /// system obliges, forever: the extension is relaunched every few
    /// seconds, the folder never settles, and `ls` on it times out
    /// rather than failing, because from the outside the enumeration
    /// has not finished, it has restarted.
    ///
    /// So the anchor has to mean something, and [`anchor`] is what it
    /// means here: the directory's own modification time, which the
    /// filesystem updates on every add, remove and rename in it. Same
    /// anchor, nothing to report; different anchor, re-enumerate once
    /// and settle on the new one.
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
    /// The enumerated directory's modification time. It catches what
    /// enumeration reports — entries appearing, disappearing, being
    /// renamed — and not edits to a file's contents, which do not touch
    /// the directory. That is the right granularity for this method:
    /// content changes are the item version's job, and an edit through
    /// the folder is the system's own write, which it already knows
    /// about.
    ///
    /// Cheap on purpose. It is one `stat`, called on a path the system
    /// asks about often, and a deep scan here would make every idle
    /// refresh walk the whole project.
    private func anchor() -> NSFileProviderSyncAnchor {
        let relative = container == .rootContainer ? "" : container.rawValue
        let directory = relative.isEmpty ? treeRoot : treeRoot.appendingPathComponent(relative)
        let stamp = Disk.modified(of: directory.path).timeIntervalSince1970
        return NSFileProviderSyncAnchor(Data(String(stamp).utf8))
    }
}
