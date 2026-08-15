// Settings — the Task server base URL, org slug, and device token. Entered
// once (dictation/scribble); persisted in UserDefaults. The token maps to
// your org + user on the server (TASK_WATCH_TOKEN).

import SwiftUI

struct SettingsView: View {
    @Environment(TaskStore.self) private var store

    var body: some View {
        @Bindable var store = store
        Form {
            Section("Server") {
                TextField("host", text: $store.baseURL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                TextField("org slug", text: $store.orgSlug)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                TextField("device token", text: $store.token)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            }
            Section {
                LabeledContent("Status",
                               value: store.isConfigured ? "Configured" : "Incomplete")
                Button("Test connection") {
                    Task { await store.refreshActive() }
                }
                .disabled(!store.isConfigured)
                if let err = store.lastError {
                    Text(err).font(.caption2).foregroundStyle(.red)
                }
            }
        }
        .navigationTitle("Settings")
    }
}
