// The Timer tab — start / stop a time-tracking work session. When a timer is
// running it shows the description + a live HH:MM:SS clock and a Stop button;
// otherwise a (dictatable) description field + a Start button.

import SwiftUI

struct TimerView: View {
    @Environment(TaskStore.self) private var store
    @State private var description: String = ""

    var body: some View {
        ScrollView {
            VStack(spacing: 8) {
                if let s = store.suggestion {
                    suggested(s)
                }
                if let active = store.active, active.running {
                    running(active)
                } else {
                    idle()
                }
                if let err = store.lastError {
                    Text(err)
                        .font(.caption2)
                        .foregroundStyle(.red)
                        .multilineTextAlignment(.center)
                        .lineLimit(3)
                }
            }
            .padding(.horizontal, 4)
        }
        .navigationTitle("Timer")
        // Poll while visible so the running state stays fresh.
        .task {
            await store.refreshActive()
            await store.refreshSuggestion()
            // 1 Hz tick to advance the clock; re-poll the timer every ~10s
            // and the suggestion every minute (the plan moves by the minute).
            var n: UInt64 = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                store.tick &+= 1
                n &+= 1
                if n % 10 == 0 { await store.refreshActive() }
                if n % 60 == 0 { await store.refreshSuggestion() }
            }
        }
    }

    /// The Suggested card: why now (small), the thing, one action, Not now.
    @ViewBuilder private func suggested(_ s: SuggestionDto) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(s.eyebrow)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Text(s.title)
                .font(.headline)
                .lineLimit(2)
            if !s.detail.isEmpty {
                Text(s.detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            HStack(spacing: 6) {
                if s.action == "Process" {
                    // Deciding what each capture is needs the phone.
                    Text("Open on phone")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    Button {
                        Task { await store.actOnSuggestion() }
                    } label: {
                        Label(s.action, systemImage: s.action == "Start" ? "play.fill" : "checkmark")
                            .font(.caption)
                            .frame(maxWidth: .infinity)
                    }
                    .tint(.accentColor)
                    .disabled(store.busy)
                }
                Button("Not now") {
                    Task { await store.notNow() }
                }
                .font(.caption2)
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.accentColor.opacity(0.15), in: RoundedRectangle(cornerRadius: 10))
    }

    @ViewBuilder private func running(_ active: TimerDto) -> some View {
        // Referencing store.tick re-renders this every second.
        let _ = store.tick
        VStack(spacing: 6) {
            Text(active.description.isEmpty ? "Working" : active.description)
                .font(.headline)
                .lineLimit(2)
                .multilineTextAlignment(.center)
            Text(hms(active.elapsed()))
                .font(.system(.title2, design: .monospaced))
                .foregroundStyle(.green)
                .monospacedDigit()
            Button(role: .destructive) {
                Task { await store.stop() }
            } label: {
                Label("Stop", systemImage: "stop.fill").frame(maxWidth: .infinity)
            }
            .tint(.red)
            .disabled(store.busy)
        }
    }

    @ViewBuilder private func idle() -> some View {
        VStack(spacing: 6) {
            if !store.isConfigured {
                Text("Set up the server in Settings →")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            // Tapping the field brings up watchOS dictation/scribble.
            TextField("What are you working on?", text: $description)
                .textFieldStyle(.plain)
                .font(.caption)
            Button {
                Task {
                    await store.start(description: description)
                    if store.active?.running == true { description = "" }
                }
            } label: {
                Label("Start", systemImage: "play.fill").frame(maxWidth: .infinity)
            }
            .tint(.green)
            .disabled(store.busy || !store.isConfigured)
        }
    }

    private func hms(_ secs: TimeInterval) -> String {
        let s = Int(secs)
        return String(format: "%02d:%02d:%02d", s / 3600, (s % 3600) / 60, s % 60)
    }
}
