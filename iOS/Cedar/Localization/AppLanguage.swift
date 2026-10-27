import Foundation
import Observation

/// A language the app offers, as a BCP 47 tag with its name in that language. The text comes in
/// English, German, French and Spanish; the region picks the formats (dates, numbers, currency) and
/// tells the server which variant to write in. Swiss German also has its own text, with ss for ß,
/// and so does Spain's Spanish where its words differ (the Spanish text is Latin American).
struct AppLanguage: Identifiable, Hashable, Sendable {
    let tag: String
    let name: String

    var id: String { tag }

    static let all: [AppLanguage] = [
        AppLanguage(tag: "en", name: "English"),
        AppLanguage(tag: "en-US", name: "English (United States)"),
        AppLanguage(tag: "en-GB", name: "English (United Kingdom)"),
        AppLanguage(tag: "de-DE", name: "Deutsch (Deutschland)"),
        AppLanguage(tag: "de-AT", name: "Deutsch (Österreich)"),
        AppLanguage(tag: "de-CH", name: "Deutsch (Schweiz)"),
        AppLanguage(tag: "fr-FR", name: "Français (France)"),
        AppLanguage(tag: "fr-BE", name: "Français (Belgique)"),
        AppLanguage(tag: "fr-CH", name: "Français (Suisse)"),
        AppLanguage(tag: "fr-CA", name: "Français (Canada)"),
        AppLanguage(tag: "es-ES", name: "Español (España)"),
        AppLanguage(tag: "es-MX", name: "Español (México)"),
        AppLanguage(tag: "es-419", name: "Español (Latinoamérica)"),
        AppLanguage(tag: "es-US", name: "Español (Estados Unidos)"),
    ]

    /// The languages the text is translated into.
    static let languageCodes: Set<String> = ["en", "de", "fr", "es"]

    /// The variant a language starts on when the device's region isn't offered (`de-LU` → `de-DE`):
    /// its first one in `all`, unless named here. Spanish from elsewhere (`es-AR`) is Latin American.
    private static let mainVariants: [String: String] = ["es": "es-419"]

    /// The variant to offer for language `code`, if the text is translated into it.
    static func mainVariant(of code: String) -> AppLanguage? {
        guard languageCodes.contains(code) else { return nil }
        if let tag = mainVariants[code], let main = offered(tag) { return main }
        return all.first { languageCode(of: $0.tag) == code }
    }

    /// `tag` the BCP 47 way, `de-CH` for `de_ch`: the language lowercased and the region, if any,
    /// uppercased. Scripts and extensions are dropped (`en-Latn-GB-u-ca-gregory` → `en-GB`).
    static func canonical(_ tag: String) -> String? {
        let parts = tag.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "_", with: "-")
            .split(separator: "-")
            .map(String.init)
        guard let first = parts.first, (2...3).contains(first.count), first.allSatisfy({ $0.isASCII && $0.isLetter })
        else { return nil }
        var region: String?
        for part in parts.dropFirst() {
            // A single letter starts an extension; nothing after it is a region.
            if part.count == 1 { break }
            if (part.count == 2 && part.allSatisfy({ $0.isASCII && $0.isLetter }))
                || (part.count == 3 && part.allSatisfy({ $0.isASCII && $0.isNumber })) {
                region = part.uppercased()
                break
            }
        }
        let language = first.lowercased()
        return region.map { "\(language)-\($0)" } ?? language
    }

    /// The language part of `tag`: `de` for `de-CH`, or empty when it isn't a tag.
    static func languageCode(of tag: String) -> String {
        guard let normalized = canonical(tag) else { return "" }
        return String(normalized.prefix { $0 != "-" })
    }

    /// The offered language `tag` names, if any.
    static func offered(_ tag: String) -> AppLanguage? {
        guard let normalized = canonical(tag) else { return nil }
        return all.first { $0.tag == normalized }
    }

    /// The language to start the picker on for the device's languages, most preferred first: the
    /// first one offered as it is, else the first one translated, in its main variant (`de-LU` →
    /// `de-DE`, `es-AR` → `es-419`); English when none is.
    static func suggested(for preferred: [String]) -> AppLanguage {
        for tag in preferred {
            if let exact = offered(tag) { return exact }
            if let main = mainVariant(of: languageCode(of: tag)) { return main }
        }
        return all[0]
    }

    /// The tag the device's settings come to when the language is automatic: the first of its
    /// languages the app is translated into, with its region (or the device's region when it names
    /// none), as the system picks the app's language; `en` when none is translated.
    static func automaticTag(preferred: [String], region: String?) -> String {
        for tag in preferred {
            guard let normalized = canonical(tag) else { continue }
            let code = languageCode(of: normalized)
            guard languageCodes.contains(code) else { continue }
            if normalized != code { return normalized }
            if let region, let withRegion = canonical("\(code)-\(region)"), withRegion != code {
                return withRegion
            }
            return code
        }
        return "en"
    }

    /// Where text for `tag` is looked up, most specific first: `de-CH`, `de`, then English, the
    /// source. Only the variants with text of their own (`de-CH`, `es-ES`) have a translation to find.
    static func lookupChain(for tag: String) -> [String] {
        guard let normalized = canonical(tag) else { return ["en"] }
        var chain = [normalized]
        let code = languageCode(of: normalized)
        if code != normalized { chain.append(code) }
        if !chain.contains("en") { chain.append("en") }
        return chain
    }
}

/// The interface language: one the user picked, or automatic (the device's). Kept in
/// `UserDefaults`; views that read it follow a change at once.
@Observable
final class LanguageStore {
    static let shared = LanguageStore()

    /// The picked tag, or `nil` to follow the device.
    private(set) var chosenTag: String?
    /// Whether a language was ever picked: a new install asks before anything else.
    private(set) var hasChosen: Bool

    private let defaults: UserDefaults
    /// What automatic comes to, read once (it is asked for with every string looked up): iOS
    /// restarts the app when the device's language changes.
    @ObservationIgnored private var deviceTag: String

    private enum Key {
        static let tag = "languageTag"
        static let chosen = "languageChosen"
        /// The system's own list, read at launch: what iOS draws for the app follows it.
        static let appleLanguages = "AppleLanguages"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        chosenTag = defaults.string(forKey: Key.tag).flatMap { AppLanguage.offered($0)?.tag }
        hasChosen = defaults.bool(forKey: Key.chosen)
        deviceTag = Self.readDeviceTag()
    }

    private static func readDeviceTag() -> String {
        AppLanguage.automaticTag(preferred: Locale.preferredLanguages, region: Locale.current.region?.identifier)
    }

    /// The tag text is looked up for and messages carry: the picked one, or the device's.
    var effectiveTag: String {
        chosenTag ?? deviceTag
    }

    /// The locale dates, numbers and currency are formatted for.
    var locale: Locale {
        if let chosenTag { return Locale(identifier: chosenTag) }
        let tag = effectiveTag
        // The device's own locale keeps its settings (24-hour time, say) when it is in the language
        // the text is shown in.
        if Locale.current.language.languageCode?.identifier == AppLanguage.languageCode(of: tag) {
            return .autoupdatingCurrent
        }
        return Locale(identifier: tag)
    }

    /// Picks a language (`nil` for automatic). The app's own text follows at once; what the
    /// system draws (the photo picker, permission alerts) follows on the next launch.
    func choose(_ tag: String?) {
        let tag = tag.flatMap { AppLanguage.offered($0)?.tag }
        chosenTag = tag
        hasChosen = true
        defaults.set(true, forKey: Key.chosen)
        if let tag {
            defaults.set(tag, forKey: Key.tag)
            defaults.set([tag], forKey: Key.appleLanguages)
        } else {
            defaults.removeObject(forKey: Key.tag)
            defaults.removeObject(forKey: Key.appleLanguages)
        }
        // Without the app's own list, the device's shows through again.
        deviceTag = Self.readDeviceTag()
    }
}
