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
    Es,
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
            "es" => Lang::Es,
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
            Lang::Es => "Nueva conversación",
        }
    }

    pub fn open_in_cedar(self) -> &'static str {
        match self {
            Lang::En => "Open in Cedar",
            Lang::De => "In Cedar öffnen",
            Lang::Fr => "Ouvrir dans Cedar",
            Lang::Es => "Abrir en Cedar",
        }
    }

    pub fn test_title(self) -> &'static str {
        match self {
            Lang::En => "Cedar: test notification",
            Lang::De => "Cedar: Testbenachrichtigung",
            Lang::Fr => "Cedar : notification de test",
            Lang::Es => "Cedar: notificación de prueba",
        }
    }

    pub fn test_body(self) -> &'static str {
        match self {
            Lang::En => "This is how Cedar will tell you when a response finishes while you are away.",
            Lang::De => "So sagt dir Cedar Bescheid, wenn eine Antwort fertig ist, während du weg bist.",
            Lang::Fr => "C'est ainsi que Cedar vous préviendra lorsqu'une réponse sera prête pendant votre absence.",
            Lang::Es => "Así te avisará Cedar cuando una respuesta esté lista mientras no estás.",
        }
    }
}

/// What started an agent's run, in a line: shown with the run, and told to the model (in
/// English, with no locale). Names, subjects and values are the trigger's own data, cut short
/// where they were read.
pub fn event_summary(locale: Option<&str>, event: &crate::trigger::Event) -> String {
    use crate::trigger::{Event, Op};
    let lang = Lang::of(locale);
    let text = match event {
        Event::Clock => match lang {
            Lang::En => "on schedule".to_string(),
            Lang::De => "nach Zeitplan".to_string(),
            Lang::Fr => "selon la planification".to_string(),
            Lang::Es => "según lo programado".to_string(),
        },
        Event::Changed { from, to } => match lang {
            Lang::En => format!("the watched value changed from {from:?} to {to:?}"),
            Lang::De => format!("der beobachtete Wert hat sich von „{from}“ zu „{to}“ geändert"),
            Lang::Fr => format!("la valeur surveillée est passée de « {from} » à « {to} »"),
            Lang::Es => format!("el valor vigilado cambió de «{from}» a «{to}»"),
        },
        Event::Holds { value, op, target } => {
            let sign = match op {
                Op::Gt => ">",
                Op::Lt => "<",
                Op::Gte => "≥",
                Op::Lte => "≤",
                Op::Equals => "=",
                Op::Contains | Op::Changes => "",
            };
            match (lang, op) {
                (Lang::En, Op::Contains) => format!("the watched value contains {target:?}"),
                (Lang::De, Op::Contains) => format!("der beobachtete Wert enthält „{target}“"),
                (Lang::Fr, Op::Contains) => format!("la valeur surveillée contient « {target} »"),
                (Lang::Es, Op::Contains) => format!("el valor vigilado contiene «{target}»"),
                (Lang::En, _) => format!("the watched value is {value} ({sign} {target})"),
                (Lang::De, _) => format!("der beobachtete Wert ist {value} ({sign} {target})"),
                (Lang::Fr, _) => format!("la valeur surveillée est {value} ({sign} {target})"),
                (Lang::Es, _) => format!("el valor vigilado es {value} ({sign} {target})"),
            }
        }
        Event::Webhook { dropped: 0, .. } => match lang {
            Lang::En => "a webhook was called".to_string(),
            Lang::De => "ein Webhook wurde aufgerufen".to_string(),
            Lang::Fr => "un webhook a été appelé".to_string(),
            Lang::Es => "se llamó a un webhook".to_string(),
        },
        Event::Webhook { dropped, .. } => match lang {
            Lang::En => format!("a webhook was called ({dropped} earlier calls arrived while the agent was busy and were dropped)"),
            Lang::De => format!("ein Webhook wurde aufgerufen ({dropped} frühere Aufrufe kamen, während der Agent beschäftigt war, und wurden verworfen)"),
            Lang::Fr => format!("un webhook a été appelé ({dropped} appels précédents, arrivés pendant que l'agent était occupé, ont été ignorés)"),
            Lang::Es => format!("se llamó a un webhook ({dropped} llamadas anteriores llegaron mientras el agente estaba ocupado y se descartaron)"),
        },
        Event::Rows { set, added, changed } => match (lang, *added, *changed) {
            (Lang::En, a, 0) => format!("{a} new in the record set {set}"),
            (Lang::En, 0, c) => format!("{c} changed in the record set {set}"),
            (Lang::En, a, c) => format!("{a} new and {c} changed in the record set {set}"),
            (Lang::De, a, 0) => format!("{a} neu im Datensatz {set}"),
            (Lang::De, 0, c) => format!("{c} geändert im Datensatz {set}"),
            (Lang::De, a, c) => format!("{a} neu und {c} geändert im Datensatz {set}"),
            (Lang::Fr, a, 0) => format!("{a} ajout(s) dans le jeu de données {set}"),
            (Lang::Fr, 0, c) => format!("{c} modification(s) dans le jeu de données {set}"),
            (Lang::Fr, a, c) => format!("{a} ajout(s) et {c} modification(s) dans le jeu de données {set}"),
            (Lang::Es, a, 0) => format!("{a} nuevo(s) en el conjunto {set}"),
            (Lang::Es, 0, c) => format!("{c} cambiado(s) en el conjunto {set}"),
            (Lang::Es, a, c) => format!("{a} nuevo(s) y {c} cambiado(s) en el conjunto {set}"),
        },
        Event::Device { source } => {
            // What on the device asked; anything else is named as it came.
            let what = match (lang, source.as_str()) {
                (Lang::En, "shortcut") => "a shortcut",
                (Lang::De, "shortcut") => "ein Kurzbefehl",
                (Lang::Fr, "shortcut") => "un raccourci",
                (Lang::Es, "shortcut") => "un atajo",
                (Lang::En, "share") => "something shared from another app",
                (Lang::De, "share") => "etwas aus einer anderen App Geteiltes",
                (Lang::Fr, "share") => "un partage depuis une autre application",
                (Lang::Es, "share") => "algo compartido desde otra aplicación",
                (Lang::En, "automation") => "an automation",
                (Lang::De, "automation") => "eine Automation",
                (Lang::Fr, "automation") => "une automatisation",
                (Lang::Es, "automation") => "una automatización",
                (_, other) => other,
            };
            match lang {
                Lang::En => format!("started from your device by {what}"),
                Lang::De => format!("von deinem Gerät gestartet durch {what}"),
                Lang::Fr => format!("lancé depuis votre appareil par {what}"),
                Lang::Es => format!("iniciado desde tu dispositivo por {what}"),
            }
        }
        Event::Files { count: 1, name } => match lang {
            Lang::En => format!("a new file in the watched folder: {name}"),
            Lang::De => format!("eine neue Datei im beobachteten Ordner: {name}"),
            Lang::Fr => format!("un nouveau fichier dans le dossier surveillé : {name}"),
            Lang::Es => format!("un archivo nuevo en la carpeta vigilada: {name}"),
        },
        Event::Files { count, name } => match lang {
            Lang::En => format!("{count} new files in the watched folder, the latest {name}"),
            Lang::De => format!("{count} neue Dateien im beobachteten Ordner, die neueste {name}"),
            Lang::Fr => format!("{count} nouveaux fichiers dans le dossier surveillé, le dernier {name}"),
            Lang::Es => format!("{count} archivos nuevos en la carpeta vigilada, el último {name}"),
        },
        Event::Mail { count: 1, from, subject } => match lang {
            Lang::En => format!("new mail from {from}: {subject}"),
            Lang::De => format!("neue E-Mail von {from}: {subject}"),
            Lang::Fr => format!("nouveau message de {from} : {subject}"),
            Lang::Es => format!("nuevo correo de {from}: {subject}"),
        },
        Event::Mail { count, from, subject } => match lang {
            Lang::En => format!("{count} new emails, the latest from {from}: {subject}"),
            Lang::De => format!("{count} neue E-Mails, die neueste von {from}: {subject}"),
            Lang::Fr => format!("{count} nouveaux messages, le dernier de {from} : {subject}"),
            Lang::Es => format!("{count} correos nuevos, el último de {from}: {subject}"),
        },
        Event::Soon { title, minutes, count } => {
            let more = count.saturating_sub(1);
            let base = match lang {
                Lang::En => format!("\"{title}\" starts in {minutes} min"),
                Lang::De => format!("„{title}“ beginnt in {minutes} Min."),
                Lang::Fr => format!("« {title} » commence dans {minutes} min"),
                Lang::Es => format!("«{title}» empieza en {minutes} min"),
            };
            match (lang, more) {
                (_, 0) => base,
                (Lang::En, n) => format!("{base} (and {n} more)"),
                (Lang::De, n) => format!("{base} (und {n} weitere)"),
                (Lang::Fr, n) => format!("{base} (et {n} de plus)"),
                (Lang::Es, n) => format!("{base} (y {n} más)"),
            }
        }
        Event::NewEvent { title, count } => {
            let more = count.saturating_sub(1);
            let base = match lang {
                Lang::En => format!("new calendar event: {title}"),
                Lang::De => format!("neuer Kalendertermin: {title}"),
                Lang::Fr => format!("nouvel événement dans l'agenda : {title}"),
                Lang::Es => format!("nuevo evento en el calendario: {title}"),
            };
            match (lang, more) {
                (_, 0) => base,
                (Lang::En, n) => format!("{base} (and {n} more)"),
                (Lang::De, n) => format!("{base} (und {n} weitere)"),
                (Lang::Fr, n) => format!("{base} (et {n} de plus)"),
                (Lang::Es, n) => format!("{base} (y {n} más)"),
            }
        }
    };
    Lang::swiss(locale, text)
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
        (Lang::Es, Outcome::NeedsAnswer) => ("necesita tu respuesta", "necesita tu respuesta"),
        (Lang::Es, Outcome::Failed) => ("falló", "falló"),
        (Lang::Es, Outcome::Finished) => ("lista", "está lista"),
    };
    let kind = match (lang, scheduled) {
        (Lang::En, true) => "An agent's run",
        (Lang::En, false) => "A response",
        (Lang::De, true) => "Ein Agentenlauf",
        (Lang::De, false) => "Eine Antwort",
        (Lang::Fr, true) => "Une exécution d'agent",
        (Lang::Fr, false) => "Une réponse",
        (Lang::Es, true) => "Una ejecución de agente",
        (Lang::Es, false) => "Una respuesta",
    };
    // How each language quotes the conversation's name.
    let quoted = match lang {
        Lang::En => format!("in \"{name}\""),
        Lang::De => format!("in „{name}“"),
        Lang::Fr => format!("dans « {name} »"),
        Lang::Es => format!("en «{name}»"),
    };
    let colon = if lang == Lang::Fr { " :" } else { ":" };
    let body = match (error, outcome, excerpt) {
        (Some(error), _, _) => format!("{kind} {quoted} {what}{colon} {error}"),
        (None, Outcome::NeedsAnswer, Some(text)) => {
            let asks = match lang {
                Lang::En => "Cedar asks",
                Lang::De => "Cedar fragt",
                Lang::Fr => "Cedar demande",
                Lang::Es => "Cedar pregunta",
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
        assert_eq!(Lang::of(Some("es-419")), Lang::Es);
        assert_eq!(Lang::of(Some("pt")), Lang::En);
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
        assert_eq!(body, "Ein Agentenlauf in „Strasse“ ist fertig.");
        assert_eq!(
            msg(Some("es-MX"), Outcome::Finished, None, Some("Listo.")),
            ("Reise: lista".into(), "Una respuesta en «Reise» está lista:\nListo.".into())
        );
        let (title, _) = turn_message(Some("de"), None, Outcome::Finished, false, None, None);
        assert_eq!(title, "Neuer Chat: fertig");
    }
}
