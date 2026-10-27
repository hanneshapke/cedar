import XCTest
@testable import Cedar

final class LocalizationTests: XCTestCase {
    func testTagsAreNormalized() {
        XCTAssertEqual(AppLanguage.canonical("de_ch"), "de-CH")
        XCTAssertEqual(AppLanguage.canonical("EN-gb"), "en-GB")
        XCTAssertEqual(AppLanguage.canonical("en-Latn-GB"), "en-GB")
        XCTAssertEqual(AppLanguage.canonical("es-419"), "es-419")
        // An extension's parts are no region.
        XCTAssertEqual(AppLanguage.canonical("de-u-ca-gregory"), "de")
        XCTAssertEqual(AppLanguage.canonical(" fr "), "fr")
        XCTAssertNil(AppLanguage.canonical(""))
        XCTAssertNil(AppLanguage.canonical("x"))
        XCTAssertEqual(AppLanguage.languageCode(of: "fr_CA"), "fr")
        XCTAssertEqual(AppLanguage.canonical("ES_mx"), "es-MX")
        XCTAssertEqual(AppLanguage.canonical("es-Latn-419"), "es-419")
        XCTAssertEqual(AppLanguage.canonical("es_419"), "es-419")
        XCTAssertEqual(AppLanguage.languageCode(of: "es-419"), "es")

        XCTAssertEqual(AppLanguage.all.map(\.tag), ["en", "en-US", "en-GB", "de-DE", "de-AT", "de-CH", "fr-FR", "fr-BE", "fr-CH", "fr-CA",
                                                    "es-ES", "es-MX", "es-419", "es-US"])
        XCTAssertEqual(AppLanguage.offered("fr_ca")?.name, "Français (Canada)")
        XCTAssertEqual(AppLanguage.offered("es_419")?.name, "Español (Latinoamérica)")
        XCTAssertEqual(AppLanguage.offered("es-us")?.tag, "es-US")
        XCTAssertNil(AppLanguage.offered("de-LU"))
        XCTAssertNil(AppLanguage.offered("es"))
        XCTAssertNil(AppLanguage.offered("es-AR"))
    }

    func testThePickerStartsOnTheDevicesLanguage() {
        XCTAssertEqual(AppLanguage.suggested(for: ["de-AT", "en-US"]).tag, "de-AT")
        XCTAssertEqual(AppLanguage.suggested(for: ["de-LU"]).tag, "de-DE")
        XCTAssertEqual(AppLanguage.suggested(for: ["it-IT", "fr-CA"]).tag, "fr-CA")
        XCTAssertEqual(AppLanguage.suggested(for: ["fr"]).tag, "fr-FR")
        XCTAssertEqual(AppLanguage.suggested(for: ["es-ES", "fr-CA"]).tag, "es-ES")
        XCTAssertEqual(AppLanguage.suggested(for: ["es-MX"]).tag, "es-MX")
        // Spanish from a region not offered is Latin American, not Spain's.
        XCTAssertEqual(AppLanguage.suggested(for: ["es-AR"]).tag, "es-419")
        XCTAssertEqual(AppLanguage.suggested(for: ["es_CO", "en-US"]).tag, "es-419")
        XCTAssertEqual(AppLanguage.suggested(for: ["es"]).tag, "es-419")
        XCTAssertEqual(AppLanguage.suggested(for: ["en-AU"]).tag, "en")
        XCTAssertEqual(AppLanguage.suggested(for: ["ja-JP"]).tag, "en")
        XCTAssertEqual(AppLanguage.suggested(for: []).tag, "en")
    }

    func testAutomaticComesToTheDevicesLocale() {
        XCTAssertEqual(AppLanguage.automaticTag(preferred: ["de-AT", "en-US"], region: "AT"), "de-AT")
        // A language without a region takes the device's.
        XCTAssertEqual(AppLanguage.automaticTag(preferred: ["fr"], region: "CH"), "fr-CH")
        XCTAssertEqual(AppLanguage.automaticTag(preferred: ["en"], region: nil), "en")
        // The first language the app is translated into, as iOS picks the app's language.
        XCTAssertEqual(AppLanguage.automaticTag(preferred: ["ja-JP", "de-DE"], region: "JP"), "de-DE")
        XCTAssertEqual(AppLanguage.automaticTag(preferred: ["ja-JP", "es-ES"], region: "JP"), "es-ES")
        XCTAssertEqual(AppLanguage.automaticTag(preferred: ["es"], region: "AR"), "es-AR")
        XCTAssertEqual(AppLanguage.automaticTag(preferred: ["ja-JP", "it-IT"], region: "JP"), "en")
        XCTAssertEqual(AppLanguage.automaticTag(preferred: [], region: "US"), "en")
    }

    func testLookupFallsBackFromTheRegionToTheLanguageToEnglish() {
        XCTAssertEqual(AppLanguage.lookupChain(for: "de-CH"), ["de-CH", "de", "en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: "de_AT"), ["de-AT", "de", "en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: "en-GB"), ["en-GB", "en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: "en"), ["en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: "fr"), ["fr", "en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: "es-ES"), ["es-ES", "es", "en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: "es-419"), ["es-419", "es", "en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: "es_mx"), ["es-MX", "es", "en"])
        XCTAssertEqual(AppLanguage.lookupChain(for: ""), ["en"])
    }

    /// Reads the String Catalog as built into the app.
    func testTextComesFromTheCatalog() {
        XCTAssertEqual(L10n.string("Settings", in: "de-DE"), "Einstellungen")
        XCTAssertEqual(L10n.string("Settings", in: "fr-CA"), "Réglages")
        XCTAssertEqual(L10n.string("Settings", in: "en-GB"), "Settings")
        // Swiss German has its own text where German writes ß, and is German otherwise.
        XCTAssertEqual(L10n.string("Dismiss", in: "de-DE"), "Schließen")
        XCTAssertEqual(L10n.string("Dismiss", in: "de-CH"), "Schliessen")
        XCTAssertEqual(L10n.string("Settings", in: "de-CH"), "Einstellungen")
        XCTAssertEqual(L10n.string("Settings", in: "es-MX"), "Configuración")
        XCTAssertEqual(L10n.string("Settings", in: "es-419"), "Configuración")
        XCTAssertEqual(L10n.string("Settings", in: "es-AR"), "Configuración")
        // Spain's Spanish has its own text where its words differ, and is the Latin American otherwise.
        XCTAssertEqual(L10n.string("Drive", in: "es-MX"), "Auto")
        XCTAssertEqual(L10n.string("Drive", in: "es-ES"), "Coche")
        XCTAssertEqual(L10n.string("Settings", in: "es-ES"), "Configuración")
        XCTAssertEqual(L10n.string("Open Settings", in: "es-ES"), "Abrir Ajustes")
        XCTAssertEqual(L10n.string("Open Settings", in: "es-US"), "Abrir Configuración")
        XCTAssertEqual(L10n.string("Not a key", in: "de-DE"), "Not a key")
    }

    func testTheStoreKeepsThePick() throws {
        let suite = "LocalizationTests-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        // What was written to the suite itself, not what it inherits from the global domain.
        func stored(_ key: String) -> Any? { defaults.persistentDomain(forName: suite)?[key] }

        let store = LanguageStore(defaults: defaults)
        XCTAssertFalse(store.hasChosen)
        XCTAssertNil(store.chosenTag)

        store.choose("de_ch")
        XCTAssertTrue(store.hasChosen)
        XCTAssertEqual(store.chosenTag, "de-CH")
        XCTAssertEqual(store.effectiveTag, "de-CH")
        XCTAssertEqual(store.locale.language.languageCode?.identifier, "de")
        XCTAssertEqual(store.locale.region?.identifier, "CH")
        XCTAssertEqual(stored("AppleLanguages") as? [String], ["de-CH"])

        let reopened = LanguageStore(defaults: defaults)
        XCTAssertTrue(reopened.hasChosen)
        XCTAssertEqual(reopened.chosenTag, "de-CH")

        // Automatic: the device's language, and the system's own list again.
        store.choose(nil)
        XCTAssertTrue(store.hasChosen)
        XCTAssertNil(store.chosenTag)
        XCTAssertNil(stored("AppleLanguages"))
        XCTAssertEqual(store.effectiveTag,
                       AppLanguage.automaticTag(preferred: Locale.preferredLanguages, region: Locale.current.region?.identifier))

        store.choose("es_419")
        XCTAssertEqual(store.chosenTag, "es-419")
        XCTAssertEqual(store.locale.language.languageCode?.identifier, "es")
        XCTAssertEqual(stored("AppleLanguages") as? [String], ["es-419"])

        // A language the app doesn't offer is automatic.
        store.choose("it-IT")
        XCTAssertNil(store.chosenTag)
    }

    func testMessagesCarryTheLocale() throws {
        let message = ChatClient.SendMessage(content: "hi", locale: "de-CH")
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(message)) as? [String: Any])
        XCTAssertEqual(json["locale"] as? String, "de-CH")
        XCTAssertEqual(json["content"] as? String, "hi")

        let plain = try JSONSerialization.jsonObject(with: JSONEncoder().encode(ChatClient.SendMessage(content: "hi"))) as? [String: Any]
        XCTAssertNil(plain?["locale"])
    }

    func testTheLocaleSettingSendsNullForAutomatic() throws {
        let picked = try JSONSerialization.jsonObject(with: JSONEncoder().encode(ChatClient.LocaleSetting(locale: "fr-BE"))) as? [String: Any]
        XCTAssertEqual(picked?["locale"] as? String, "fr-BE")

        let automatic = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(ChatClient.LocaleSetting(locale: nil))) as? [String: Any])
        XCTAssertEqual(automatic.count, 1)
        XCTAssertTrue(automatic["locale"] is NSNull, "null clears the server's setting, so it is sent rather than left out")
    }
}
