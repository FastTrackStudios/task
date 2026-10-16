// Task's widgets: four lock-screen / home-screen shortcuts, the Capture
// control, and the running timer's Live Activity.
//
// Every widget opens the app with an `app.fasttrackstudio.task://…` link
// (`ui::deep_link`). Task List and Calendar also show numbers the app
// writes into the App Group (`TaskShared`, apps/mobile/ios/Shared).

import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

let scheme = "app.fasttrackstudio.task"

func link(_ place: String) -> URL { URL(string: "\(scheme)://\(place)")! }

// ── What the app last wrote ──────────────────────────────────────────

struct Snapshot: TimelineEntry {
    let date: Date
    let tasksToday: Int
    let now: String
    let next: String

    static let placeholder = Snapshot(date: .now, tasksToday: 3, now: "Studio block · until 12:30", next: "Lunch · 12:30")

    static func read() -> Snapshot {
        let d = TaskShared.defaults
        return Snapshot(
            date: .now,
            tasksToday: d?.integer(forKey: TaskShared.tasksToday) ?? 0,
            now: d?.string(forKey: TaskShared.calendarNow) ?? "",
            next: d?.string(forKey: TaskShared.calendarNext) ?? ""
        )
    }
}

struct SnapshotProvider: TimelineProvider {
    func placeholder(in context: Context) -> Snapshot { .placeholder }
    func getSnapshot(in context: Context, completion: @escaping (Snapshot) -> Void) {
        completion(context.isPreview ? .placeholder : .read())
    }
    func getTimeline(in context: Context, completion: @escaping (Timeline<Snapshot>) -> Void) {
        // The app reloads the widgets when anything changes; this is the
        // backstop for a day's blocks moving on with the app closed.
        completion(Timeline(entries: [.read()], policy: .after(.now.addingTimeInterval(15 * 60))))
    }
}

struct StaticProvider: TimelineProvider {
    func placeholder(in context: Context) -> Snapshot { .placeholder }
    func getSnapshot(in context: Context, completion: @escaping (Snapshot) -> Void) { completion(.placeholder) }
    func getTimeline(in context: Context, completion: @escaping (Timeline<Snapshot>) -> Void) {
        completion(Timeline(entries: [.placeholder], policy: .never))
    }
}

/// A round launcher: one symbol, one place.
struct Launcher: View {
    let symbol: String
    let label: String
    @Environment(\.widgetFamily) var family

    var body: some View {
        switch family {
        case .accessoryCircular:
            ZStack {
                AccessoryWidgetBackground()
                Image(systemName: symbol).font(.title2.weight(.semibold))
            }
            .widgetAccentable()
        default:
            VStack(alignment: .leading, spacing: 8) {
                Image(systemName: symbol).font(.title.weight(.semibold))
                Spacer()
                Text(label).font(.headline)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
    }
}

// ── Vault Search ─────────────────────────────────────────────────────

struct SearchWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "task.search", provider: StaticProvider()) { _ in
            Launcher(symbol: "magnifyingglass", label: "Search")
                .widgetURL(link("search"))
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Vault Search")
        .description("Search every vault and wiki.")
        .supportedFamilies([.accessoryCircular, .systemSmall])
    }
}

// ── Fleeting Capture ─────────────────────────────────────────────────

struct CaptureWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "task.capture", provider: StaticProvider()) { _ in
            Launcher(symbol: "square.and.pencil", label: "Capture")
                .widgetURL(link("capture"))
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Fleeting Capture")
        .description("Get a thought out of your head.")
        .supportedFamilies([.accessoryCircular, .systemSmall])
    }
}

// ── Task List ────────────────────────────────────────────────────────

struct TasksView: View {
    let entry: Snapshot
    @Environment(\.widgetFamily) var family

    var body: some View {
        switch family {
        case .accessoryCircular:
            ZStack {
                AccessoryWidgetBackground()
                VStack(spacing: 0) {
                    Text("\(entry.tasksToday)").font(.title2.weight(.bold)).monospacedDigit()
                    Image(systemName: "checkmark.circle").font(.caption2)
                }
            }
            .widgetAccentable()
        case .accessoryRectangular:
            VStack(alignment: .leading) {
                Label("Today", systemImage: "checkmark.circle").font(.headline)
                Text(entry.tasksToday == 1 ? "1 task" : "\(entry.tasksToday) tasks")
            }
        default:
            VStack(alignment: .leading, spacing: 4) {
                Label("Today", systemImage: "checkmark.circle").font(.subheadline.weight(.semibold))
                Spacer()
                Text("\(entry.tasksToday)").font(.system(size: 44, weight: .bold)).monospacedDigit()
                Text(entry.tasksToday == 1 ? "task" : "tasks").font(.subheadline).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
    }
}

struct TasksWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "task.tasks", provider: SnapshotProvider()) { entry in
            TasksView(entry: entry)
                .widgetURL(link("tasks"))
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Task List")
        .description("How many tasks today holds.")
        .supportedFamilies([.accessoryCircular, .accessoryRectangular, .systemSmall])
    }
}

// ── Calendar ─────────────────────────────────────────────────────────

struct CalendarView: View {
    let entry: Snapshot
    @Environment(\.widgetFamily) var family

    var body: some View {
        switch family {
        case .accessoryCircular:
            ZStack {
                AccessoryWidgetBackground()
                Image(systemName: "calendar").font(.title2.weight(.semibold))
            }
            .widgetAccentable()
        default:
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.now.isEmpty ? "Nothing planned" : entry.now)
                    .font(.headline).lineLimit(2)
                if !entry.next.isEmpty {
                    Text("Next: \(entry.next)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
    }
}

struct CalendarWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "task.calendar", provider: SnapshotProvider()) { entry in
            CalendarView(entry: entry)
                .widgetURL(link("calendar"))
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Calendar")
        .description("What is on now, and what is next.")
        .supportedFamilies([.accessoryRectangular, .accessoryCircular, .systemSmall])
    }
}

// ── The Capture control (Control Center, the Action button) ──────────

@available(iOS 18.0, *)
struct CaptureControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "task.capture.control") {
            ControlWidgetButton(action: OpenURLIntent(link("capture"))) {
                Label("Capture", systemImage: "square.and.pencil")
            }
        }
        .displayName("Fleeting Capture")
        .description("Open Task straight to capture.")
    }
}

@available(iOS 18.0, *)
struct TimerControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "task.timer.control") {
            ControlWidgetButton(action: OpenURLIntent(link("timer"))) {
                Label("Start Timer", systemImage: "timer")
            }
        }
        .displayName("Start a Timer")
        .description("Start a timer now; name it later.")
    }
}

// ── The running timer ────────────────────────────────────────────────

struct TimerTitle: View {
    let state: TaskTimerAttributes.ContentState
    var body: some View {
        Text(state.title.isEmpty ? "Timer" : state.title).lineLimit(1)
    }
}

struct TimerActivity: Widget {
    var body: some WidgetConfiguration {
        let config = ActivityConfiguration(for: TaskTimerAttributes.self) { context in
            // Lock screen (and the watch's Smart Stack).
            HStack(spacing: 12) {
                Image(systemName: "timer").font(.title2).foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 2) {
                    TimerTitle(state: context.state).font(.headline)
                    Text("Task").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Text(context.state.startedAt, style: .timer)
                    .font(.title2.weight(.semibold)).monospacedDigit()
                    .multilineTextAlignment(.trailing).frame(maxWidth: 110)
            }
            .padding()
            .widgetURL(link("timer"))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Image(systemName: "timer").font(.title2).foregroundStyle(.tint)
                }
                DynamicIslandExpandedRegion(.center) {
                    TimerTitle(state: context.state).font(.headline)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(context.state.startedAt, style: .timer)
                        .monospacedDigit().frame(maxWidth: 70).multilineTextAlignment(.trailing)
                }
            } compactLeading: {
                Image(systemName: "timer").foregroundStyle(.tint)
            } compactTrailing: {
                Text(context.state.startedAt, style: .timer)
                    .monospacedDigit().frame(maxWidth: 56).multilineTextAlignment(.trailing)
            } minimal: {
                Image(systemName: "timer").foregroundStyle(.tint)
            }
            .widgetURL(link("timer"))
        }
        if #available(iOS 18.0, *) {
            // The small family is what the Apple Watch's Smart Stack shows.
            return config.supplementalActivityFamilies([.small])
        } else {
            return config
        }
    }
}

// ── The bundle ───────────────────────────────────────────────────────

@main
struct TaskWidgetBundle: WidgetBundle {
    @WidgetBundleBuilder
    var body: some Widget {
        SearchWidget()
        CaptureWidget()
        TasksWidget()
        CalendarWidget()
        TimerActivity()
        if #available(iOS 18.0, *) {
            CaptureControl()
            TimerControl()
        }
    }
}
