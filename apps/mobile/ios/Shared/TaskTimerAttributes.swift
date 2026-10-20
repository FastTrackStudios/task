// The running timer, as a Live Activity sees it.
//
// Compiled into BOTH the app (apps/mobile/ios/TaskLive, linked into the
// Rust binary) and the widget extension (apps/ios-widgets), because
// ActivityKit matches an activity to its UI by this type. Keep it plain.

import Foundation

#if canImport(ActivityKit)
import ActivityKit

@available(iOS 16.1, *)
public struct TaskTimerAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        /// What the time is for — empty until it is named.
        public var title: String
        /// When the clock started; the views count up from it.
        public var startedAt: Date

        public init(title: String, startedAt: Date) {
            self.title = title
            self.startedAt = startedAt
        }
    }

    public init() {}
}
#endif

/// Where the app leaves what the widgets show (an App Group's defaults).
public enum TaskShared {
    public static let group = "group.app.fasttrackstudio.task"
    public static let tasksToday = "tasks.today"
    public static let calendarNow = "calendar.now"
    public static let calendarNext = "calendar.next"
    public static let updatedAt = "updatedAt"

    public static var defaults: UserDefaults? { UserDefaults(suiteName: group) }
}
