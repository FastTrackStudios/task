// One entry in a synced root, as the File Provider system wants it.
//
// # Identifiers are paths
//
// The system needs a stable identifier per item, and the obvious
// candidates are both wrong here: an inode changes when a file is
// replaced (which is what an atomic save does), and a jj file id is the
// *content*, so it changes on every edit and is shared by every copy.
//
// The path relative to the root is what actually identifies a file to
// the person using it, it survives edits, and it is what the sync
// engine names things by — the same key the reconcile engine, the CLI
// and the daemon's `hydrate` all use. Renames are the price: a rename
// is a delete plus a create to the system, which is exactly what it is
// to the version store too.

import FileProvider
import UniformTypeIdentifiers

/// The root container's identifier, spelled as the empty relative path.
let rootRelativePath = ""

final class Item: NSObject, NSFileProviderItem {
    private let relativePath: String
    private let isDirectory: Bool
    private let contentSize: UInt64
    private let modified: Date
    private let resident: Bool

    init(relativePath: String, isDirectory: Bool, size: UInt64, modified: Date, resident: Bool) {
        self.relativePath = relativePath
        self.isDirectory = isDirectory
        self.contentSize = size
        self.modified = modified
        self.resident = resident
    }

    var itemIdentifier: NSFileProviderItemIdentifier {
        relativePath.isEmpty ? .rootContainer : NSFileProviderItemIdentifier(relativePath)
    }

    var parentItemIdentifier: NSFileProviderItemIdentifier {
        let parent = (relativePath as NSString).deletingLastPathComponent
        return parent.isEmpty ? .rootContainer : NSFileProviderItemIdentifier(parent)
    }

    var filename: String {
        relativePath.isEmpty ? "Task" : (relativePath as NSString).lastPathComponent
    }

    var contentType: UTType {
        if isDirectory { return .folder }
        let ext = (relativePath as NSString).pathExtension
        return UTType(filenameExtension: ext) ?? .data
    }

    /// The size of the content — for a dehydrated file, what its stub
    /// records. Reporting the placeholder's size here is the bug this
    /// whole extension exists to avoid: it would tell every app on the
    /// machine that a two-gigabyte take is empty.
    var documentSize: NSNumber? { isDirectory ? nil : NSNumber(value: contentSize) }

    var contentModificationDate: Date? { modified }

    var capabilities: NSFileProviderItemCapabilities {
        isDirectory ? [.allowsAddingSubItems, .allowsContentEnumerating, .allowsReading,
                       .allowsRenaming, .allowsDeleting]
                    : [.allowsReading, .allowsWriting, .allowsRenaming, .allowsDeleting,
                       .allowsReparenting]
    }

    /// What Finder puts the cloud badge on. A dehydrated file is not
    /// "not downloaded yet" in the sense of a transfer in flight — it is
    /// a file whose bytes this machine gave back and can get again — but
    /// this is the flag the system has for it, and it is the one that
    /// makes "Download Now" and "Remove Download" appear in the menu.
    var isMostRecentVersionDownloaded: Bool { resident }

    var itemVersion: NSFileProviderItemVersion {
        // Content version tracks size + mtime: the pair changes on any
        // edit that matters, and neither requires reading the file,
        // which for a dehydrated one would defeat the point.
        let stamp = "\(contentSize)-\(modified.timeIntervalSince1970)"
        let data = Data(stamp.utf8)
        return NSFileProviderItemVersion(contentVersion: data, metadataVersion: data)
    }
}
