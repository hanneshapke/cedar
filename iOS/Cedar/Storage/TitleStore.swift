import Foundation

/// Custom conversation titles, keyed by the conversation's first run id. cedar has nowhere to
/// keep them, so they stay on this device.
struct TitleStore {
    private let defaults: UserDefaults
    private let key = "conversationTitles"

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    var all: [String: String] {
        defaults.dictionary(forKey: key) as? [String: String] ?? [:]
    }

    func set(_ title: String?, for conversationId: String) {
        var titles = all
        let trimmed = title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        titles[conversationId] = trimmed.isEmpty ? nil : trimmed
        defaults.set(titles, forKey: key)
    }
}
