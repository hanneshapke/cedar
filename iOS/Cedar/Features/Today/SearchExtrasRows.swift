import SwiftUI

/// What a search found beside conversations (`GET /api/search`), under them in the sidebar:
/// agents, which open their detail, then what Cedar remembers and the record sets, as rows to
/// read. Each kind is its own section; one with nothing is left out.
struct SearchExtrasRows: View {
    let extras: SearchExtras
    /// Opens an agent's detail.
    let showAgent: (TimelineAgent) -> Void

    var body: some View {
        if !extras.agents.isEmpty {
            Section {
                ForEach(extras.agents) { agent in
                    Button {
                        showAgent(agent)
                    } label: {
                        SearchExtraRow(title: agent.title, detail: agent.trigger.summary, systemImage: agent.trigger.systemImage)
                    }
                    .buttonStyle(.plain)
                    .sidebarRow()
                }
            } header: {
                Text("Agents")
            }
        }
        if !extras.memories.isEmpty {
            Section {
                ForEach(extras.memories) { memory in
                    SearchExtraRow(title: memory.text, detail: nil, systemImage: "bookmark")
                        .sidebarRow()
                }
            } header: {
                Text("Remembered")
            }
        }
        if !extras.records.isEmpty {
            Section {
                ForEach(extras.records) { set in
                    SearchExtraRow(title: set.name, detail: set.description, systemImage: "tablecells")
                        .sidebarRow()
                }
            } header: {
                Text("Records")
            }
        }
    }
}

/// One thing a search found: what it is, and a line about it.
private struct SearchExtraRow: View {
    let title: String
    let detail: String?
    let systemImage: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: systemImage)
                .foregroundStyle(.secondary)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: title)
                    .font(.body)
                    .foregroundStyle(Color.primary)
                    .lineLimit(3)
                if let detail, !detail.isEmpty {
                    Text(verbatim: detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}
