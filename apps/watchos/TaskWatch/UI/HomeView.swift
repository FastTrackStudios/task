// The watch app's one screen. It is for two things — getting a fleeting
// thought down and keeping time — so both are a single tap from the
// wrist: Capture opens dictation straight away (TextFieldLink), Start
// starts the clock now and asks what it is for only if you want to say.
// While a timer runs, the clock is the screen. The Suggested card (what
// the plan says is next) sits under the actions; Settings is the gear.

import SwiftUI

let brand = Color(red: 0.49, green: 0.52, blue: 0.98)

struct HomeView: View {
    @Environment(TaskStore.self) private var store
    @State private var saved = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 10) {
                    if let active = store.active, active.running {
                        RunningCard(active: active)
                    } else {
                        actions
                    }
                    if saved {
                        Label("Captured", systemImage: "checkmark.circle.fill")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(.green)
                            .transition(.opacity)
                    }
                    if let s = store.suggestion {
                        SuggestionCard(s: s)
                    }
                    if !store.isConfigured {
                        Text("Open Task on your iPhone to connect.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    } else if let err = store.lastError {
                        Text(err)
                            .font(.caption2)
                            .foregroundStyle(.red)
                            .multilineTextAlignment(.center)
                            .lineLimit(3)
                    }
                }
                .padding(.horizontal, 2)
            }
            .navigationTitle("Task")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    NavigationLink { SettingsView() } label: {
                        Image(systemName: "gearshape")
                    }
                }
            }
        }
        .task {
            await store.refreshActive()
            await store.refreshSuggestion()
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

    /// Nothing running: the two things the watch is for.
    private var actions: some View {
        HStack(spacing: 8) {
            // One tap: dictation opens; what is said lands in the inbox.
            TextFieldLink(prompt: Text("What's on your mind?")) {
                BigAction(symbol: "mic.fill", label: "Capture", tint: brand)
            } onSubmit: { text in
                Task {
                    if await store.capture(text) {
                        withAnimation { saved = true }
                        try? await Task.sleep(for: .seconds(2))
                        withAnimation { saved = false }
                    }
                }
            }
            .buttonStyle(.plain)
            .disabled(store.busy || !store.isConfigured)

            // Start now; name it on the phone or never.
            Button {
                Task { await store.start(description: "") }
            } label: {
                BigAction(symbol: "play.fill", label: "Timer", tint: .green)
            }
            .buttonStyle(.plain)
            .disabled(store.busy || !store.isConfigured)
        }
    }
}

/// A big square action: a symbol in a tinted disc, a word under it.
struct BigAction: View {
    let symbol: String
    let label: String
    let tint: Color

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                Circle().fill(tint.gradient)
                Image(systemName: symbol)
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundStyle(.white)
            }
            .frame(width: 62, height: 62)
            Text(label).font(.footnote.weight(.semibold))
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 10)
        .background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
}

/// The running timer: what it is for, the clock, Done.
struct RunningCard: View {
    @Environment(TaskStore.self) private var store
    let active: TimerDto

    var body: some View {
        let _ = store.tick
        VStack(spacing: 6) {
            HStack(spacing: 5) {
                Circle().fill(.red).frame(width: 7, height: 7)
                Text(active.description.isEmpty ? "Timer" : active.description)
                    .font(.footnote.weight(.semibold))
                    .lineLimit(1)
            }
            .foregroundStyle(.secondary)
            Text(hms(active.elapsed()))
                .font(.system(size: 38, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .minimumScaleFactor(0.6)
                .lineLimit(1)
            Button {
                Task { await store.stop() }
            } label: {
                Label("Done", systemImage: "checkmark")
                    .font(.headline)
                    .frame(maxWidth: .infinity)
            }
            .tint(brand)
            .disabled(store.busy)
        }
        .padding(10)
        .background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }

    private func hms(_ secs: TimeInterval) -> String {
        let s = Int(secs)
        return s >= 3600
            ? String(format: "%d:%02d:%02d", s / 3600, (s % 3600) / 60, s % 60)
            : String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// What the plan says is next: why now, the thing, one action, Not now.
struct SuggestionCard: View {
    @Environment(TaskStore.self) private var store
    let s: SuggestionDto

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(s.eyebrow).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            Text(s.title).font(.headline).lineLimit(2)
            HStack(spacing: 6) {
                if s.action != "Process" {
                    Button(s.action) { Task { await store.actOnSuggestion() } }
                        .font(.caption.weight(.semibold))
                        .tint(brand)
                        .disabled(store.busy)
                } else {
                    Text("On your phone").font(.caption2).foregroundStyle(.secondary)
                    Spacer()
                }
                Button("Not now") { Task { await store.notNow() } }
                    .font(.caption2)
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(brand.opacity(0.16), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
    }
}
