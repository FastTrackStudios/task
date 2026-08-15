// The Capture tab — dictate a fleeting note. Tapping the field opens watchOS
// dictation/scribble; "Save" posts it to the inbox as a fleeting note
// (source "watch") for later processing in the daily review.

import SwiftUI

struct CaptureView: View {
    @Environment(TaskStore.self) private var store
    @State private var note: String = ""
    @State private var justSaved = false

    var body: some View {
        VStack(spacing: 8) {
            TextField("Dictate a note…", text: $note, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.body)
                .lineLimit(1...4)

            Button {
                Task {
                    let ok = await store.capture(note)
                    if ok {
                        note = ""
                        justSaved = true
                        try? await Task.sleep(for: .seconds(2))
                        justSaved = false
                    }
                }
            } label: {
                Label("Save note", systemImage: "tray.and.arrow.down.fill")
                    .frame(maxWidth: .infinity)
            }
            .tint(.blue)
            .disabled(store.busy || note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                      || !store.isConfigured)

            if justSaved {
                Label("Saved", systemImage: "checkmark.circle.fill")
                    .font(.caption)
                    .foregroundStyle(.green)
            } else if !store.isConfigured {
                Text("Set up the server in Settings →")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            } else if let err = store.lastError {
                Text(err)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .lineLimit(3)
                    .multilineTextAlignment(.center)
            }
        }
        .padding(.horizontal, 4)
        .navigationTitle("Capture")
    }
}
