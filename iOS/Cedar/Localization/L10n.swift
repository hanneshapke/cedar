import Foundation

/// Text and formats in the interface language, for what isn't a SwiftUI string literal: strings
/// built in models, errors, labels kept in variables. (`Text("…")` and the like look themselves up
/// through the `locale` the root view sets.)
///
/// Keys are the English text, as in `Localizable.xcstrings`; ones used only here are marked
/// `manual` there, since Xcode can't find them in the code. The String Catalog compiles to
/// `<language>.lproj/Localizable.strings`, which is read directly, so a pick takes effect without
/// a relaunch: `de-CH` reads its own text and falls back to `de`, then to English.
enum L10n {
    /// The locale to format dates, numbers and currency for.
    static var locale: Locale { LanguageStore.shared.locale }

    /// Whether the interface is in French, which puts a no-break space before `:`.
    static var isFrench: Bool { AppLanguage.languageCode(of: LanguageStore.shared.effectiveTag) == "fr" }

    /// The text for `key` in the interface language, or `key` itself when it has none.
    static func string(_ key: String) -> String {
        string(key, in: LanguageStore.shared.effectiveTag)
    }

    /// The text for `key` in the language `tag` names.
    static func string(_ key: String, in tag: String) -> String {
        for bundle in bundles(AppLanguage.lookupChain(for: tag)) {
            let value = bundle.localizedString(forKey: key, value: missing, table: nil)
            if value != missing { return value }
        }
        return key
    }

    /// The format `key` looks up, filled in with `arguments` (`%@` for strings, `%lld` for `Int`).
    static func format(_ key: String, _ arguments: CVarArg...) -> String {
        String(format: string(key), locale: locale, arguments: arguments)
    }

    /// "label: value", with the space French puts before the colon.
    static func labeled(_ label: String, _ value: String) -> String {
        isFrench ? "\(label)\u{00A0}: \(value)" : "\(label): \(value)"
    }

    /// What `localizedString` returns for a key a table doesn't have.
    private static let missing = "\u{1}missing\u{1}"
    private static let lock = NSLock()
    /// Each language's `.lproj` bundle, or `nil` when the app has none for it.
    private static var cache: [String: Bundle?] = [:]

    private static func bundles(_ chain: [String]) -> [Bundle] {
        lock.lock()
        defer { lock.unlock() }
        return chain.compactMap { name -> Bundle? in
            if let cached = cache[name] { return cached }
            let bundle = Bundle.main.path(forResource: name, ofType: "lproj").flatMap { Bundle(path: $0) }
            cache.updateValue(bundle, forKey: name)
            return bundle
        }
    }
}
