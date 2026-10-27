//! The few texts this service writes itself (notifications), in the user's language.
//!
//! The pages and apps translate their own text; cedar tells the model which language to answer in.
//! What is left here is what goes out by push, email or Pushover while the user is away, which
//! follows the language saved in Settings (`locale` in the preferences).

/// The languages the clients are translated into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
    Fr,
}

/// The longest locale tag accepted (as cedar takes them).
const MAX_TAG: usize = 35;

/// A locale as the user picked it: a BCP 47 tag such as `de` or `fr-CH`, trimmed. cedar
/// normalizes it; this only refuses what can't be one, so a typo is reported before it is saved.
pub fn clean_locale(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(tag) = raw.map(str::trim).filter(|t| !t.is_empty()) else { return Ok(None) };
    let language = tag.split(['-', '_']).next().unwrap_or_default();
    let ok = tag.len() <= MAX_TAG
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && (2..=3).contains(&language.len())
        && language.bytes().all(|b| b.is_ascii_alphabetic())
        && !tag.ends_with(['-', '_']);
    if !ok {
        return Err(format!("locale {tag:?} is not a language tag such as \"de\" or \"fr-CH\""));
    }
    Ok(Some(tag.to_string()))
}

impl Lang {
    /// The language of a locale; English for none or one the clients aren't translated into.
    pub fn of(locale: Option<&str>) -> Self {
        let language = locale.unwrap_or_default().split(['-', '_']).next().unwrap_or_default();
        match language.to_ascii_lowercase().as_str() {
            "de" => Lang::De,
            "fr" => Lang::Fr,
            _ => Lang::En,
        }
    }

    /// Swiss German writes ss for ß.
    fn swiss(locale: Option<&str>, text: String) -> String {
        let region = locale.unwrap_or_default().split(['-', '_']).nth(1).unwrap_or_default();
        if Lang::of(locale) == Lang::De && region.eq_ignore_ascii_case("CH") {
            text.replace('ß', "ss")
        } else {
            text
        }
    }

    pub fn new_chat(self) -> &'static str {
        match self {
            Lang::En => "New chat",
            Lang::De => "Neuer Chat",
            Lang::Fr => "Nouvelle conversation",
        }
    }

    pub fn open_in_cedar(self) -> &'static str {
        match self {
            Lang::En => "Open in Cedar",
            Lang::De => "In Cedar öffnen",
            Lang::Fr => "Ouvrir dans Cedar",
        }
    }

    pub fn test_title(self) -> &'static str {
        match self {
            Lang::En => "Cedar: test notification",
            Lang::De => "Cedar: Testbenachrichtigung",
            Lang::Fr => "Cedar : notification de test",
        }
    }

    pub fn test_body(self) -> &'static str {
        match self {
            Lang::En => "This is how Cedar will tell you when a response finishes while you are away.",
            Lang::De => "So sagt dir Cedar Bescheid, wenn eine Antwort fertig ist, während du weg bist.",
            Lang::Fr => "C'est ainsi que Cedar vous préviendra lorsqu'une réponse sera prête pendant votre absence.",
        }
    }
}

/// How a finished turn ended, for its notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    NeedsAnswer,
    Failed,
    Finished,
}

/// The title and body of a finished turn's notification. `excerpt` is the start of the reply, and
/// `error` what went wrong.
pub fn turn_message(
    locale: Option<&str>,
    name: Option<&str>,
    outcome: Outcome,
    scheduled: bool,
    error: Option<&str>,
    excerpt: Option<&str>,
) -> (String, String) {
    let lang = Lang::of(locale);
    let name = name.unwrap_or(lang.new_chat());
    let (title_what, what) = match (lang, outcome) {
        (Lang::En, Outcome::NeedsAnswer) => ("needs your answer", "needs your answer"),
        (Lang::En, Outcome::Failed) => ("failed", "failed"),
        (Lang::En, Outcome::Finished) => ("finished", "finished"),
        (Lang::De, Outcome::NeedsAnswer) => ("braucht deine Antwort", "braucht deine Antwort"),
        (Lang::De, Outcome::Failed) => ("fehlgeschlagen", "ist fehlgeschlagen"),
        (Lang::De, Outcome::Finished) => ("fertig", "ist fertig"),
        (Lang::Fr, Outcome::NeedsAnswer) => ("attend votre réponse", "attend votre réponse"),
        (Lang::Fr, Outcome::Failed) => ("échec", "a échoué"),
        (Lang::Fr, Outcome::Finished) => ("terminé", "est terminée"),
    };
    let kind = match (lang, scheduled) {
        (Lang::En, true) => "A scheduled run",
        (Lang::En, false) => "A response",
        (Lang::De, true) => "Ein geplanter Lauf",
        (Lang::De, false) => "Eine Antwort",
        (Lang::Fr, true) => "Une exécution planifiée",
        (Lang::Fr, false) => "Une réponse",
    };
    // How each language quotes the conversation's name.
    let quoted = match lang {
        Lang::En => format!("in \"{name}\""),
        Lang::De => format!("in „{name}“"),
        Lang::Fr => format!("dans « {name} »"),
    };
    let colon = if lang == Lang::Fr { " :" } else { ":" };
    let body = match (error, outcome, excerpt) {
        (Some(error), _, _) => format!("{kind} {quoted} {what}{colon} {error}"),
        (None, Outcome::NeedsAnswer, Some(text)) => {
            let asks = match lang {
                Lang::En => "Cedar asks",
                Lang::De => "Cedar fragt",
                Lang::Fr => "Cedar demande",
            };
            format!("{asks} {quoted}{colon}\n{text}")
        }
        (None, _, Some(text)) => format!("{kind} {quoted} {what}{colon}\n{text}"),
        (None, _, None) => format!("{kind} {quoted} {what}."),
    };
    let title = format!("{name}{colon} {title_what}");
    (Lang::swiss(locale, title), Lang::swiss(locale, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_are_checked_not_rewritten() {
        assert_eq!(clean_locale(None), Ok(None));
        assert_eq!(clean_locale(Some("  ")), Ok(None));
        assert_eq!(clean_locale(Some(" de-CH ")), Ok(Some("de-CH".into())));
        assert_eq!(clean_locale(Some("fr_ca")), Ok(Some("fr_ca".into())));
        for bad in ["d", "de-", "de CH", "12", "de;q=1"] {
            assert!(clean_locale(Some(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn languages_fall_back_to_english() {
        assert_eq!(Lang::of(Some("de-AT")), Lang::De);
        assert_eq!(Lang::of(Some("FR")), Lang::Fr);
        assert_eq!(Lang::of(Some("es")), Lang::En);
        assert_eq!(Lang::of(None), Lang::En);
    }

    #[test]
    fn messages_read_naturally_in_each_language() {
        let msg = |locale, outcome, error, excerpt| {
            turn_message(locale, Some("Reise"), outcome, false, error, excerpt)
        };
        assert_eq!(
            msg(None, Outcome::Finished, None, Some("Done.")),
            ("Reise: finished".into(), "A response in \"Reise\" finished:\nDone.".into())
        );
        assert_eq!(
            msg(Some("de"), Outcome::Failed, Some("Zeitüberschreitung"), None),
            (
                "Reise: fehlgeschlagen".into(),
                "Eine Antwort in „Reise“ ist fehlgeschlagen: Zeitüberschreitung".into()
            )
        );
        assert_eq!(
            msg(Some("fr-CA"), Outcome::NeedsAnswer, None, Some("Quel jour ?")),
            (
                "Reise : attend votre réponse".into(),
                "Cedar demande dans « Reise » :\nQuel jour ?".into()
            )
        );
        let (_, body) = turn_message(Some("de-CH"), Some("Straße"), Outcome::Finished, true, None, None);
        assert_eq!(body, "Ein geplanter Lauf in „Strasse“ ist fertig.");
        let (title, _) = turn_message(Some("de"), None, Outcome::Finished, false, None, None);
        assert_eq!(title, "Neuer Chat: fertig");
    }
}
