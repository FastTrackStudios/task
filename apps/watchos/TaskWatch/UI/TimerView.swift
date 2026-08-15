// The Timer tab — start / stop a time-tracking work session. When a timer is
// running it shows the description + a live HH:MM:SS clock and a Stop button;
// otherwise a (dictatable) description field + a Start button.

import SwiftUI

struct TimerView: View {
    @Environment(TaskStore.self) private var store
    @State private var description: String = ""

    var body: some View {
        VStack(spacing: 8) {
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
        .navigationTitle("Timer")
        // Poll while visible so the running state stays fresh.
        .task {
            await store.refreshActive()
            // 1 Hz tick to advance the clock; also re-poll every ~10s.
            var n: UInt64 = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                store.tick &+= 1
                n &+= 1
                if n % 10 == 0 { await store.refreshActive() }
            }
        }
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
