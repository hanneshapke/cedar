//! The user's language and region, and what the model is told about them.
//!
//! A run may carry a locale (`locale` on `POST /v1/runs`, a BCP 47 tag such as `de-CH`). The
//! system prompt stays in English whatever it is; the run gets a short directive instead, added to
//! the environment note: reply in the language of the request (the locale's language when that is
//! unclear) and format dates, numbers, units and prices the way the locale expects. Personal tools
//! see the locale as `CEDAR_LOCALE` and `CEDAR_LANGUAGE`.

/// The languages cedar's clients are translated into. A locale in another language is kept as
/// English (`en`).
pub const LANGUAGES: &[&str] = &["en", "de", "fr", "es"];

/// The locales the clients offer, besides the bare languages.
pub const REGIONS: &[&str] = &[
    "en-US", "en-GB", "de-DE", "de-AT", "de-CH", "fr-FR", "fr-BE", "fr-CH", "fr-CA", "es-ES",
    "es-MX", "es-419", "es-US",
];

/// The longest tag accepted, before it is cut to language and region.
const MAX_TAG: usize = 35;

/// `raw` as a locale cedar works with: a lowercase language, optionally followed by an uppercase
/// region (`de`, `de-CH`, `fr-CA`). `_` is taken for `-`, and script or variant subtags are
/// dropped (`zh-Hant-TW` is `zh-TW`). A language cedar isn't translated into becomes `en`. Errors
/// on a tag that isn't BCP 47.
pub fn normalize(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let bad = || format!("locale {raw:?} is not a language tag such as \"de\" or \"fr-CH\"");
    if raw.is_empty()
        || raw.len() > MAX_TAG
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(bad());
    }
    let mut parts = raw.split(['-', '_']);
    let language = parts.next().unwrap_or_default().to_ascii_lowercase();
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(bad());
    }
    let mut region = None;
    for part in parts {
        if part.is_empty() || part.len() > 8 {
            return Err(bad());
        }
        let is_region = (part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()))
            || (part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit()));
        if is_region && region.is_none() {
            region = Some(part.to_ascii_uppercase());
        }
    }
    if !LANGUAGES.contains(&language.as_str()) {
        return Ok("en".into());
    }
    Ok(match region {
        Some(r) => format!("{language}-{r}"),
        None => language,
    })
}

/// The language part of a locale: `de` for `de-CH`.
pub fn language(locale: &str) -> &str {
    locale.split('-').next().unwrap_or(locale)
}

/// How a locale writes things, for the directive.
struct Conventions {
    date: &'static str,
    time: &'static str,
    number: &'static str,
    currency: &'static str,
    units: &'static str,
    /// Anything else worth saying, such as spelling.
    extra: Option<&'static str>,
}

fn conventions(locale: &str) -> Option<Conventions> {
    let metric = "metric units";
    let c = |date, time, number, currency, units, extra| Conventions {
        date,
        time,
        number,
        currency,
        units,
        extra,
    };
    Some(match locale {
        "de" | "de-DE" | "de-LU" | "de-BE" => {
            c("27.10.2026", "14:05", "1.234,5", "EUR", metric, None)
        }
        "de-AT" => c(
            "27.10.2026",
            "14:05",
            "1.234,5",
            "EUR",
            metric,
            Some("use Austrian German words where they differ, such as Jänner for January"),
        ),
        "de-CH" | "de-LI" => c(
            "27.10.2026",
            "14:05",
            "1'234.5",
            "CHF",
            metric,
            Some("write ss instead of ß, as Swiss German does"),
        ),
        "fr" | "fr-FR" | "fr-BE" | "fr-LU" | "fr-MC" => {
            c("27/10/2026", "14:05", "1 234,5", "EUR", metric, None)
        }
        "fr-CH" => c("27.10.2026", "14:05", "1'234.5", "CHF", metric, None),
        "fr-CA" => c("2026-10-27", "14 h 05", "1 234,5", "CAD", metric, None),
        "en-US" => c(
            "10/27/2026",
            "2:05 PM",
            "1,234.5",
            "USD",
            "US customary units",
            None,
        ),
        "en-GB" => c("27/10/2026", "14:05", "1,234.5", "GBP", metric, None),
        "es" | "es-ES" => c(
            "27/10/2026",
            "14:05",
            "1.234,5",
            "EUR",
            metric,
            Some("use the words of Spain where they differ, such as ordenador and móvil"),
        ),
        "es-MX" => c("27/10/2026", "14:05", "1,234.5", "MXN", metric, None),
        "es-US" => c(
            "10/27/2026",
            "2:05 PM",
            "1,234.5",
            "USD",
            "US customary units",
            None,
        ),
        // Another region of a supported language: that language's usual conventions. Latin
        // America (es-419) and its countries differ too much to name one currency or format.
        other => match language(other) {
            "de" => return conventions("de"),
            "fr" => return conventions("fr"),
            _ => return None,
        },
    })
}

fn language_name(language: &str) -> &'static str {
    match language {
        "de" => "German",
        "fr" => "French",
        "es" => "Spanish",
        _ => "English",
    }
}

/// The note added to a run's system prompt for its locale, or `None` for plain `en` (the
/// behavior without a locale).
pub fn directive(locale: &str) -> Option<String> {
    if locale == "en" {
        return None;
    }
    let lang = language(locale);
    let mut note = format!(
        "Reply in the language the user's message is written in. When that is unclear (a single \
         word, only an attachment, a scheduled task), reply in {}.",
        language_name(lang)
    );
    note.push_str(
        " When you reply in German, address the user informally (du); in French, use vous; in \
         Spanish, use tú.",
    );
    if let Some(c) = conventions(locale) {
        note.push_str(&format!(
            " Format for the user's locale ({locale}): dates like {}, times like {}, numbers like \
             {}, {}, and {} for prices unless another currency is given",
            c.date, c.time, c.number, c.units, c.currency
        ));
        if let Some(extra) = c.extra {
            note.push_str("; ");
            note.push_str(extra);
        }
        note.push('.');
    }
    note.push_str(
        " Keep code, file names, commands and tool arguments as they are. Give safety notices, \
         including any health rules above, in the language you reply in.",
    );
    Some(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_normalized() {
        assert_eq!(normalize("de").unwrap(), "de");
        assert_eq!(normalize(" de_ch ").unwrap(), "de-CH");
        assert_eq!(normalize("FR-ca").unwrap(), "fr-CA");
        assert_eq!(normalize("de-Latn-AT").unwrap(), "de-AT");
        assert_eq!(normalize("en-US").unwrap(), "en-US");
        assert_eq!(normalize("fr-419").unwrap(), "fr-419");
        // Languages without translations are kept as English.
        assert_eq!(normalize("es_mx").unwrap(), "es-MX");
        assert_eq!(normalize("pt-BR").unwrap(), "en");
        assert_eq!(normalize("zh-Hant-TW").unwrap(), "en");
        for bad in [
            "",
            "d",
            "deutsch",
            "de-",
            "de CH",
            "de-CH;q=0.9",
            "12",
            &"x".repeat(40),
        ] {
            assert!(normalize(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_offered_locale_is_its_own_normal_form() {
        for tag in LANGUAGES.iter().chain(REGIONS) {
            assert_eq!(normalize(tag).unwrap(), *tag);
        }
    }

    #[test]
    fn plain_english_needs_no_directive() {
        assert_eq!(directive("en"), None);
        let us = directive("en-US").unwrap();
        assert!(us.contains("10/27/2026") && us.contains("USD"));
    }

    #[test]
    fn the_directive_follows_the_request_and_formats_for_the_locale() {
        let de = directive("de-DE").unwrap();
        assert!(de.contains("language the user's message is written in"));
        assert!(de.contains("reply in German"));
        assert!(de.contains("informally (du)"));
        assert!(de.contains("1.234,5") && de.contains("EUR"));
        assert!(!de.contains('ß'));

        let ch = directive("de-CH").unwrap();
        assert!(ch.contains("1'234.5") && ch.contains("CHF") && ch.contains("ss instead of ß"));

        let ca = directive("fr-CA").unwrap();
        assert!(ca.contains("reply in French") && ca.contains("2026-10-27") && ca.contains("CAD"));

        // An unlisted region uses its language's conventions.
        assert!(directive("de-LU").unwrap().contains("EUR"));
        assert!(directive("fr-SN").unwrap().contains("27/10/2026"));
        let es = directive("es-ES").unwrap();
        assert!(es.contains("reply in Spanish") && es.contains("use tú") && es.contains("EUR"));
        assert!(es.contains("ordenador"));
        assert!(directive("es-MX").unwrap().contains("MXN"));
        // Latin America as a whole: the language rule, and no one currency.
        let latam = directive("es-419").unwrap();
        assert!(latam.contains("reply in Spanish") && !latam.contains("Format for"));
        // An English region without a table entry still gets the language rule.
        let au = directive("en-AU").unwrap();
        assert!(au.contains("reply in English") && !au.contains("Format for"));
    }
}
