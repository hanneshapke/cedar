import Foundation

/// A chain of runs, each continuing the one before (`parent_id`). cedar has no conversation
/// object of its own, so conversations are rebuilt from the run list. The first run's id
/// identifies the conversation.
struct Conversation: Identifiable, Hashable, Sendable {
    /// Oldest first.
    var runs: [Run]

    var id: String { runs[0].id }
    var latest: Run { runs[runs.count - 1] }
    var updatedAt: Date { latest.createdAt }
    var isActive: Bool { runs.contains { !$0.status.isTerminal } }

    /// A title derived from the first prompt, used until the conversation is renamed.
    var defaultTitle: String {
        let firstLine = runs[0].prompt
            .split(whereSeparator: \.isNewline)
            .first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            .map { $0.trimmingCharacters(in: .whitespaces) } ?? "Untitled"
        return firstLine.count > 80 ? String(firstLine.prefix(80)) + "…" : firstLine
    }

    /// Groups runs into conversations, most recently active first. A run whose parent is not in
    /// `runs` (deleted, or past the fetched pages) starts its own conversation.
    static func group(_ runs: [Run]) -> [Conversation] {
        let ids = Set(runs.map(\.id))
        var childOf: [String: Run] = [:]
        for run in runs {
            if let parent = run.parentId, ids.contains(parent) {
                // cedar allows a run to be continued once, so there is at most one child.
                childOf[parent] = run
            }
        }
        var conversations: [Conversation] = []
        for root in runs where root.parentId.map({ !ids.contains($0) }) ?? true {
            var chain = [root]
            var seen: Set<String> = [root.id]
            while let next = childOf[chain[chain.count - 1].id], seen.insert(next.id).inserted {
                chain.append(next)
            }
            conversations.append(Conversation(runs: chain))
        }
        return conversations.sorted { $0.updatedAt > $1.updatedAt }
    }
}
