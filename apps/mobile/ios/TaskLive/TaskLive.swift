// The app's native half for the lock screen: the timer's Live Activity
// (Dynamic Island, lock screen, the Apple Watch Smart Stack) and the data
// the widgets read. Built into a static library by apps/mobile/build.rs
// and called from Rust (apps/mobile/src/live.rs) through these C entry
// points. Everything is guarded by availability: the app runs on iOS 15,
// Live Activities need 16.1.

import Foundation
#if canImport(ActivityKit)
import ActivityKit
#endif
#if canImport(WidgetKit)
import WidgetKit
#endif

private func string(_ p: UnsafePointer<CChar>?) -> String {
    guard let p else { return "" }
    return String(cString: p)
}

/// Start the timer's activity, or update it if one is running.
@_cdecl("task_live_timer_show")
public func task_live_timer_show(_ title: UnsafePointer<CChar>?, _ startedAt: Double) {
    #if canImport(ActivityKit)
    guard #available(iOS 16.2, *) else { return }
    let state = TaskTimerAttributes.ContentState(
        title: string(title),
        startedAt: Date(timeIntervalSince1970: startedAt)
    )
    let content = ActivityContent(state: state, staleDate: nil)
    Task {
        if let running = Activity<TaskTimerAttributes>.activities.first {
            await running.update(content)
            // Only one: end any others left from an earlier run.
            for extra in Activity<TaskTimerAttributes>.activities.dropFirst() {
                await extra.end(nil, dismissalPolicy: .immediate)
            }
            return
        }
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        _ = try? Activity.request(attributes: TaskTimerAttributes(), content: content, pushType: nil)
    }
    #endif
}

/// End the timer's activity (stopped or paused).
@_cdecl("task_live_timer_end")
public func task_live_timer_end() {
    #if canImport(ActivityKit)
    guard #available(iOS 16.2, *) else { return }
    Task {
        for activity in Activity<TaskTimerAttributes>.activities {
            await activity.end(nil, dismissalPolicy: .immediate)
        }
    }
    #endif
}

/// What the widgets show: today's open tasks, and the day's current and
/// next block. Reloads the widgets when anything changed.
@_cdecl("task_widgets_publish")
public func task_widgets_publish(
    _ tasksToday: Int32,
    _ now: UnsafePointer<CChar>?,
    _ next: UnsafePointer<CChar>?
) {
    guard let d = TaskShared.defaults else { return }
    let (n, nowText, nextText) = (Int(tasksToday), string(now), string(next))
    let same = d.integer(forKey: TaskShared.tasksToday) == n
        && d.string(forKey: TaskShared.calendarNow) == nowText
        && d.string(forKey: TaskShared.calendarNext) == nextText
    if same { return }
    d.set(n, forKey: TaskShared.tasksToday)
    d.set(nowText, forKey: TaskShared.calendarNow)
    d.set(nextText, forKey: TaskShared.calendarNext)
    d.set(Date().timeIntervalSince1970, forKey: TaskShared.updatedAt)
    #if canImport(WidgetKit)
    if #available(iOS 14.0, *) {
        WidgetCenter.shared.reloadAllTimelines()
    }
    #endif
}
