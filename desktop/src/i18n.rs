//! The app's own words in the user's language: the tray menu, the password and permission
//! prompts, and the fixed sentences it writes onto the setup page (setup steps, its own errors).
//! The setup page itself has its catalog in `ui/strings.js`; both follow docs/i18n-glossary.md.
//!
//! The language is the one picked on the setup page (`locale` in config.json), set once at launch
//! and again whenever it changes (see `commands::set_locale`). Until one is picked, English.
//! What other programs say (Docker, Tailscale, the installers, the network) stays as it is.

use std::sync::RwLock;

/// The locales offered on the setup page, in its order.
pub const LOCALES: [&str; 10] = [
    "en", "en-US", "en-GB", "de-DE", "de-AT", "de-CH", "fr-FR", "fr-BE", "fr-CH", "fr-CA",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lang {
    En = 0,
    De = 1,
    Fr = 2,
}

/// The locale in use; empty means English.
static CURRENT: RwLock<String> = RwLock::new(String::new());

/// `raw` as one of [`LOCALES`] (`de-ch` → `de-CH`), or `None`.
pub fn normalize(raw: &str) -> Option<&'static str> {
    let raw = raw.trim().replace('_', "-");
    LOCALES.into_iter().find(|l| l.eq_ignore_ascii_case(&raw))
}

/// Switches the app's own words to `locale` (anything not in [`LOCALES`] means English).
pub fn set_locale(locale: &str) {
    let tag = normalize(locale).unwrap_or("");
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = tag.to_string();
}

fn current() -> String {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn lang_of(locale: &str) -> Lang {
    match locale.split('-').next().unwrap_or("") {
        "de" => Lang::De,
        "fr" => Lang::Fr,
        _ => Lang::En,
    }
}

/// The text for `key` in the current language.
pub fn t(key: &str) -> String {
    t_in(&current(), key)
}

/// The text for `key` with its `{name}` placeholders filled in.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    fill(t(key), args)
}

fn fill(mut text: String, args: &[(&str, &str)]) -> String {
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// The text for `key` in `locale`: English when the key has no entry (the key itself if it has
/// none at all). Swiss German writes ss for ß.
fn t_in(locale: &str, key: &str) -> String {
    let Some((_, texts)) = STRINGS.iter().find(|(k, _)| *k == key) else {
        debug_assert!(false, "no text for {key}");
        return key.to_string();
    };
    let text = texts[lang_of(locale) as usize];
    if locale == "de-CH" {
        text.replace('ß', "ss")
    } else {
        text.to_string()
    }
}

/// A sentence the app's own setup scripts write in English (a `STEP:` or `ERROR:` line), in the
/// current language; anything else (what an installer printed) as it is.
pub fn known(english: &str) -> String {
    let locale = current();
    match STRINGS
        .iter()
        .find(|(k, texts)| k.starts_with("script.") && texts[0] == english)
    {
        Some((key, _)) => t_in(&locale, key),
        None => english.to_string(),
    }
}

/// Every text as `(key, [English, Deutsch, Français])`. German says *du*, French *vous* with a
/// no-break space (U+00A0) before `: ; ? !`.
const STRINGS: &[(&str, [&str; 3])] = &[
    // The tray menu.
    ("tray.open", ["Open Cedar", "Cedar öffnen", "Ouvrir Cedar"]),
    ("tray.browser", ["Open in Browser", "Im Browser öffnen", "Ouvrir dans le navigateur"]),
    ("tray.copy_token", ["Copy Access Token", "Zugriffstoken kopieren", "Copier le jeton d’accès"]),
    ("tray.phone", ["Phone Access…", "Zugriff vom Smartphone…", "Accès depuis le téléphone…"]),
    ("tray.setup", ["Model Server & Setup…", "Modellserver & Einrichtung…", "Serveur de modèles et configuration…"]),
    ("tray.restart", ["Restart Services", "Dienste neu starten", "Redémarrer les services"]),
    ("tray.logs", ["Open Logs Folder", "Protokollordner öffnen", "Ouvrir le dossier des journaux"]),
    ("tray.check_updates", ["Check for Updates…", "Nach Updates suchen…", "Rechercher des mises à jour…"]),
    ("tray.checking_updates", ["Checking for Updates…", "Suche nach Updates…", "Recherche de mises à jour…"]),
    ("tray.installing_update", ["Installing the Update…", "Update wird installiert…", "Installation de la mise à jour…"]),
    ("tray.update_to", ["Update to Cedar {version}…", "Auf Cedar {version} aktualisieren…", "Mettre à jour vers Cedar {version}…"]),
    ("tray.quit", ["Quit Cedar", "Cedar beenden", "Quitter Cedar"]),
    // Commands the setup page calls.
    ("cmd.docker_busy", ["Docker is already being set up", "Docker wird bereits eingerichtet", "Docker est déjà en cours de configuration"]),
    ("cmd.tailscale_busy", ["Tailscale is already being installed", "Tailscale wird bereits installiert", "Tailscale est déjà en cours d’installation"]),
    ("cmd.no_restart", ["the Docker setup doesn't need a restart", "die Docker-Einrichtung braucht keinen Neustart", "la configuration de Docker ne nécessite pas de redémarrage"]),
    ("cmd.pick_model", ["pick a model", "Wähle ein Modell", "Choisissez un modèle"]),
    ("cmd.not_shared", ["the chat is not shared on the tailnet", "der Chat ist nicht im Tailnet freigegeben", "le chat n’est pas partagé sur le tailnet"]),
    ("cmd.bad_locale", ["{locale} is not one of the languages Cedar offers", "{locale} ist keine der Sprachen, die Cedar anbietet", "{locale} ne fait pas partie des langues proposées par Cedar"]),
    // The model server form.
    ("model.no_address", ["enter the model server's address, for example http://127.0.0.1:11434/v1", "Gib die Adresse des Modellservers ein, zum Beispiel http://127.0.0.1:11434/v1", "Saisissez l’adresse du serveur de modèles, par exemple http://127.0.0.1:11434/v1"]),
    ("model.bad_url", ["{raw} is not a valid URL: {error}", "{raw} ist keine gültige URL: {error}", "{raw} n’est pas une URL valide\u{a0}: {error}"]),
    ("model.bad_scheme", ["the address must start with http:// or https://", "die Adresse muss mit http:// oder https:// beginnen", "l’adresse doit commencer par http:// ou https://"]),
    ("model.no_host", ["{raw} has no host", "{raw} hat keinen Host", "{raw} n’a pas d’hôte"]),
    ("model.unreachable", ["could not reach {base}; is the model server running?", "{base} ist nicht erreichbar; läuft der Modellserver?", "impossible de joindre {base}\u{a0}; le serveur de modèles est-il démarré\u{a0}?"]),
    ("model.key_rejected", ["{base} rejected the API key ({status})", "{base} hat den API-Schlüssel abgelehnt ({status})", "{base} a refusé la clé d’API ({status})"]),
    ("model.bad_status", ["{base}/models answered {status}; check the address (it usually ends in /v1)", "{base}/models antwortete mit {status}; prüfe die Adresse (sie endet meist auf /v1)", "{base}/models a répondu {status}\u{a0}; vérifiez l’adresse (elle se termine généralement par /v1)"]),
    ("model.bad_list", ["{base}/models did not return an OpenAI-style model list", "{base}/models hat keine Modellliste im OpenAI-Format geliefert", "{base}/models n’a pas renvoyé de liste de modèles au format OpenAI"]),
    ("model.no_models", ["{base} lists no models; load one in the model server first", "{base} listet keine Modelle; lade zuerst eines im Modellserver", "{base} ne liste aucun modèle\u{a0}; chargez-en d’abord un dans le serveur de modèles"]),
    // Docker and the images.
    ("images.starting", ["starting", "wird gestartet", "démarrage"]),
    ("images.pull_slow", ["pulling {image} took longer than 45 minutes; check the connection and try again", "Das Laden von {image} hat länger als 45 Minuten gedauert; prüfe die Verbindung und versuche es erneut", "le téléchargement de {image} a pris plus de 45\u{a0}minutes\u{a0}; vérifiez la connexion et réessayez"]),
    ("images.unpublished", ["{image} could not be pulled ({stderr}). Either this version's images have not been published yet or the package is private on GitHub; see desktop/README.md", "{image} konnte nicht geladen werden ({stderr}). Entweder sind die Images dieser Version noch nicht veröffentlicht oder das Paket ist auf GitHub privat; siehe desktop/README.md", "{image} n’a pas pu être téléchargée ({stderr}). Soit les images de cette version ne sont pas encore publiées, soit le paquet est privé sur GitHub\u{a0}; voir desktop/README.md"]),
    ("images.pull_failed", ["pulling {image} failed: {stderr}", "Laden von {image} fehlgeschlagen: {stderr}", "échec du téléchargement de {image}\u{a0}: {stderr}"]),
    // The servers.
    ("services.starting_cedar", ["starting cedar", "cedar wird gestartet", "démarrage de cedar"]),
    ("services.starting_chat", ["starting the chat", "der Chat wird gestartet", "démarrage du chat"]),
    ("services.port_in_use", ["port {port} is already in use, so {what} can't listen on it; change the port in {config}", "Port {port} ist schon belegt, daher kann {what} ihn nicht nutzen; ändere den Port in {config}", "le port {port} est déjà utilisé, {what} ne peut donc pas l’écouter\u{a0}; changez le port dans {config}"]),
    ("services.the_chat", ["the chat", "der Chat", "le chat"]),
    ("services.gave_up", ["{detail}. Gave up after {count} crashes in a row; use Restart Services once the cause is fixed", "{detail}. Nach {count} Abstürzen in Folge aufgegeben; nutze „Dienste neu starten“, sobald die Ursache behoben ist", "{detail}. Abandon après {count}\u{a0}plantages consécutifs\u{a0}; utilisez «\u{a0}Redémarrer les services\u{a0}» une fois la cause corrigée"]),
    // Installing Docker Desktop and Tailscale.
    ("setup.starting", ["Starting", "Wird gestartet", "Démarrage"]),
    ("setup.checking_mac", ["Checking this Mac", "Dieser Mac wird geprüft", "Vérification de ce Mac"]),
    ("setup.checking_pc", ["Checking this PC", "Dieser PC wird geprüft", "Vérification de ce PC"]),
    ("setup.downloading_of", ["Downloading {what}: {got} of {total} MB", "{what} wird heruntergeladen: {got} von {total} MB", "Téléchargement de {what}\u{a0}: {got} sur {total}\u{a0}Mo"]),
    ("setup.downloading", ["Downloading {what}: {got} MB", "{what} wird heruntergeladen: {got} MB", "Téléchargement de {what}\u{a0}: {got}\u{a0}Mo"]),
    ("setup.download_failed", ["downloading {what}; check the internet connection", "{what} konnte nicht heruntergeladen werden; prüfe die Internetverbindung", "téléchargement de {what}\u{a0}; vérifiez la connexion Internet"]),
    ("setup.download_stalled", ["the {what} download stalled; check the connection and try again", "Der Download von {what} hängt; prüfe die Verbindung und versuche es erneut", "le téléchargement de {what} est bloqué\u{a0}; vérifiez la connexion et réessayez"]),
    ("setup.download_cut", ["the {what} download was cut off; try again", "Der Download von {what} wurde abgebrochen; versuche es erneut", "le téléchargement de {what} a été interrompu\u{a0}; réessayez"]),
    ("setup.took_an_hour", ["the installation took more than an hour; {log} in the logs folder says how far it got", "Die Installation hat länger als eine Stunde gedauert; {log} im Protokollordner zeigt, wie weit sie gekommen ist", "l’installation a pris plus d’une heure\u{a0}; {log} dans le dossier des journaux indique où elle s’est arrêtée"]),
    ("setup.docker_stopped", ["setting up Docker stopped (exit code {code}); docker-setup.log in the logs folder says how far it got", "Die Einrichtung von Docker wurde abgebrochen (Exit-Code {code}); docker-setup.log im Protokollordner zeigt, wie weit sie gekommen ist", "la configuration de Docker s’est arrêtée (code de sortie {code})\u{a0}; docker-setup.log dans le dossier des journaux indique où elle s’est arrêtée"]),
    ("setup.tailscale_stopped", ["installing Tailscale stopped (exit code {code}); tailscale-setup.log in the logs folder says how far it got", "Die Installation von Tailscale wurde abgebrochen (Exit-Code {code}); tailscale-setup.log im Protokollordner zeigt, wie weit sie gekommen ist", "l’installation de Tailscale s’est arrêtée (code de sortie {code})\u{a0}; tailscale-setup.log dans le dossier des journaux indique où elle s’est arrêtée"]),
    ("setup.none", ["none", "keiner", "aucun"]),
    ("setup.script_changed", ["the setup script was changed before it could run, so it didn't; try again", "Das Einrichtungsskript wurde vor dem Start verändert und deshalb nicht ausgeführt; versuche es erneut", "le script de configuration a été modifié avant de pouvoir s’exécuter, il n’a donc pas été lancé\u{a0}; réessayez"]),
    ("mac.prompt_docker", ["Cedar wants to install Docker Desktop.", "Cedar möchte Docker Desktop installieren.", "Cedar souhaite installer Docker Desktop."]),
    ("mac.prompt_tailscale", ["Cedar wants to install Tailscale.", "Cedar möchte Tailscale installieren.", "Cedar souhaite installer Tailscale."]),
    ("mac.waiting_for_password", [
        "Waiting for permission: macOS asks for an administrator's name and password so Cedar can install it.",
        "Warte auf die Erlaubnis: macOS fragt nach Name und Passwort eines Administrators, damit Cedar es installieren kann.",
        "En attente d’autorisation\u{a0}: macOS demande le nom et le mot de passe d’un administrateur pour que Cedar puisse l’installer.",
    ]),
    ("mac.declined", [
        "macOS did not get an administrator's password, so nothing was installed. Click Try again and enter your password; if you aren't an administrator of this Mac, one can enter theirs.",
        "macOS hat kein Administratorpasswort erhalten, daher wurde nichts installiert. Klicke auf „Erneut versuchen“ und gib dein Passwort ein; wenn du kein Administrator dieses Macs bist, kann ein Administrator sein eigenes eingeben.",
        "macOS n’a pas reçu de mot de passe d’administrateur, rien n’a donc été installé. Cliquez sur «\u{a0}Réessayer\u{a0}» et saisissez votre mot de passe\u{a0}; si vous n’êtes pas administrateur de ce Mac, un administrateur peut saisir le sien.",
    ]),
    ("mac.no_hypervisor", [
        "This Mac can't run virtual machines (macOS reports no Hypervisor support), and Docker runs its containers in one. That happens on Macs from before 2010 and inside a virtual machine. Run Docker on another computer, or use a Mac that supports it.",
        "Dieser Mac kann keine virtuellen Maschinen ausführen (macOS meldet keine Hypervisor-Unterstützung), und Docker führt seine Container in einer aus. Das betrifft Macs von vor 2010 und Macs, die selbst in einer virtuellen Maschine laufen. Führe Docker auf einem anderen Computer aus oder nutze einen Mac, der es unterstützt.",
        "Ce Mac ne peut pas exécuter de machines virtuelles (macOS n’indique aucune prise en charge de l’hyperviseur), or Docker exécute ses conteneurs dans l’une d’elles. C’est le cas des Mac antérieurs à 2010 et à l’intérieur d’une machine virtuelle. Exécutez Docker sur un autre ordinateur ou utilisez un Mac compatible.",
    ]),
    ("win.waiting_for_permission", [
        "Waiting for permission: Windows asks whether Cedar may make changes to this PC. Choose Yes. If no prompt appears, click the flashing shield in the taskbar.",
        "Warte auf die Erlaubnis: Windows fragt, ob Cedar Änderungen an diesem PC vornehmen darf. Wähle „Ja“. Erscheint keine Abfrage, klicke auf das blinkende Schild in der Taskleiste.",
        "En attente d’autorisation\u{a0}: Windows demande si Cedar peut apporter des modifications à ce PC. Choisissez «\u{a0}Oui\u{a0}». Si aucune demande n’apparaît, cliquez sur le bouclier clignotant dans la barre des tâches.",
    ]),
    ("win.declined_docker", [
        "Windows did not get permission to make changes, so nothing was installed. Click Set up Docker again and choose Yes; on a work PC, an administrator may have to enter their password.",
        "Windows hat keine Erlaubnis für Änderungen erhalten, daher wurde nichts installiert. Klicke erneut auf „Docker für mich einrichten“ und wähle „Ja“; auf einem Arbeits-PC muss eventuell ein Administrator sein Passwort eingeben.",
        "Windows n’a pas reçu l’autorisation d’apporter des modifications, rien n’a donc été installé. Cliquez à nouveau sur «\u{a0}Configurer Docker pour moi\u{a0}» et choisissez «\u{a0}Oui\u{a0}»\u{a0}; sur un PC professionnel, un administrateur devra peut-être saisir son mot de passe.",
    ]),
    ("win.declined_tailscale", [
        "Windows did not get permission to make changes, so nothing was installed. Click Try again and choose Yes; on a work PC, an administrator may have to enter their password.",
        "Windows hat keine Erlaubnis für Änderungen erhalten, daher wurde nichts installiert. Klicke auf „Erneut versuchen“ und wähle „Ja“; auf einem Arbeits-PC muss eventuell ein Administrator sein Passwort eingeben.",
        "Windows n’a pas reçu l’autorisation d’apporter des modifications, rien n’a donc été installé. Cliquez sur «\u{a0}Réessayer\u{a0}» et choisissez «\u{a0}Oui\u{a0}»\u{a0}; sur un PC professionnel, un administrateur devra peut-être saisir son mot de passe.",
    ]),
    ("win.too_old", [
        "WSL 2 needs Windows 10 version 2004 (build {min}) or newer, and this PC runs build {build}. Update Windows in Settings → Windows Update, then try again.",
        "WSL 2 braucht Windows 10 Version 2004 (Build {min}) oder neuer, und auf diesem PC läuft Build {build}. Aktualisiere Windows unter Einstellungen → Windows Update und versuche es dann erneut.",
        "WSL 2 nécessite Windows 10 version 2004 (build {min}) ou plus récent, et ce PC exécute le build {build}. Mettez Windows à jour dans Paramètres → Windows Update, puis réessayez.",
    ]),
    // Lines the setup scripts write (they run as root or elevated and write English; see `known`).
    ("script.mac_docker_check", ["Checking the Docker Desktop disk image", "Das Docker-Desktop-Image wird geprüft", "Vérification de l’image disque de Docker Desktop"]),
    ("script.mac_docker_install", ["Installing Docker Desktop into Applications. This takes a minute or two.", "Docker Desktop wird in „Programme“ installiert. Das dauert ein bis zwei Minuten.", "Installation de Docker Desktop dans Applications. Cela prend une à deux minutes."]),
    ("script.mac_dmg_broken", ["the downloaded Docker Desktop disk image could not be opened. It was deleted; click Try again to download it again.", "Das heruntergeladene Docker-Desktop-Image ließ sich nicht öffnen. Es wurde gelöscht; klicke auf „Erneut versuchen“, um es noch einmal herunterzuladen.", "l’image disque de Docker Desktop téléchargée n’a pas pu être ouverte. Elle a été supprimée\u{a0}; cliquez sur «\u{a0}Réessayer\u{a0}» pour la télécharger à nouveau."]),
    ("script.mac_docker_unsigned", ["the downloaded Docker Desktop is not signed by Docker. It was deleted; click Try again to download it again.", "Das heruntergeladene Docker Desktop ist nicht von Docker signiert. Es wurde gelöscht; klicke auf „Erneut versuchen“, um es noch einmal herunterzuladen.", "le Docker Desktop téléchargé n’est pas signé par Docker. Il a été supprimé\u{a0}; cliquez sur «\u{a0}Réessayer\u{a0}» pour le télécharger à nouveau."]),
    ("script.ts_check", ["Checking the Tailscale installer", "Das Tailscale-Installationsprogramm wird geprüft", "Vérification du programme d’installation de Tailscale"]),
    ("script.ts_install_mac", ["Installing Tailscale into Applications", "Tailscale wird in „Programme“ installiert", "Installation de Tailscale dans Applications"]),
    ("script.ts_install", ["Installing Tailscale", "Tailscale wird installiert", "Installation de Tailscale"]),
    ("script.ts_no_developer_id", ["the downloaded Tailscale installer is not signed with a Developer ID. It was deleted; click Try again to download it again.", "Das heruntergeladene Tailscale-Installationsprogramm ist nicht mit einer Developer ID signiert. Es wurde gelöscht; klicke auf „Erneut versuchen“, um es noch einmal herunterzuladen.", "le programme d’installation de Tailscale téléchargé n’est pas signé avec un Developer ID. Il a été supprimé\u{a0}; cliquez sur «\u{a0}Réessayer\u{a0}» pour le télécharger à nouveau."]),
    ("script.ts_unsigned", ["the downloaded Tailscale installer is not signed by Tailscale. It was deleted; click Try again to download it again.", "Das heruntergeladene Tailscale-Installationsprogramm ist nicht von Tailscale signiert. Es wurde gelöscht; klicke auf „Erneut versuchen“, um es noch einmal herunterzuladen.", "le programme d’installation de Tailscale téléchargé n’est pas signé par Tailscale. Il a été supprimé\u{a0}; cliquez sur «\u{a0}Réessayer\u{a0}» pour le télécharger à nouveau."]),
    ("script.win_wsl", ["Turning on the Windows Subsystem for Linux", "Das Windows-Subsystem für Linux wird eingeschaltet", "Activation du sous-système Windows pour Linux"]),
    ("script.win_wsl_update", ["Updating WSL", "WSL wird aktualisiert", "Mise à jour de WSL"]),
    ("script.win_docker_check", ["Checking the Docker Desktop installer", "Das Docker-Desktop-Installationsprogramm wird geprüft", "Vérification du programme d’installation de Docker Desktop"]),
    ("script.win_docker_install", ["Installing Docker Desktop. This takes a few minutes.", "Docker Desktop wird installiert. Das dauert ein paar Minuten.", "Installation de Docker Desktop. Cela prend quelques minutes."]),
    ("script.win_docker_users", ["Letting your account use Docker", "Dein Konto erhält Zugriff auf Docker", "Autorisation de votre compte à utiliser Docker"]),
    // The update banner.
    ("update.downloading", ["Downloading…", "Wird heruntergeladen…", "Téléchargement…"]),
    ("update.downloading_of", ["Downloading: {got} of {total} MB", "Wird heruntergeladen: {got} von {total} MB", "Téléchargement\u{a0}: {got} sur {total}\u{a0}Mo"]),
    ("update.downloading_mb", ["Downloading: {got} MB", "Wird heruntergeladen: {got} MB", "Téléchargement\u{a0}: {got}\u{a0}Mo"]),
    ("update.installing", ["Installing Cedar {version}…", "Cedar {version} wird installiert…", "Installation de Cedar {version}…"]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    fn placeholders(s: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(start) = rest.find('{') {
            let Some(len) = rest[start..].find('}') else { break };
            out.insert(rest[start + 1..start + len].to_string());
            rest = &rest[start + len + 1..];
        }
        out
    }

    #[test]
    fn every_key_is_unique_and_translated() {
        let mut seen = HashSet::new();
        for (key, texts) in STRINGS {
            assert!(seen.insert(*key), "{key} is listed twice");
            for (i, text) in texts.iter().enumerate() {
                assert!(!text.trim().is_empty(), "{key} has no text in language {i}");
            }
            let en = placeholders(texts[0]);
            assert_eq!(placeholders(texts[1]), en, "{key}: German placeholders");
            assert_eq!(placeholders(texts[2]), en, "{key}: French placeholders");
        }
    }

    #[test]
    fn german_has_its_own_words() {
        for (key, texts) in STRINGS {
            assert_ne!(texts[1], texts[0], "{key} is English in German");
        }
    }

    /// French puts a no-break space before `: ; ? !` and inside « ». Punctuation inside a word
    /// (`http://`, `127.0.0.1:11434`) is not punctuation.
    #[test]
    fn french_typography() {
        for (key, texts) in STRINGS {
            let fr: Vec<char> = texts[2].chars().collect();
            for (i, c) in fr.iter().enumerate() {
                let next = fr.get(i + 1).copied();
                let ends_word = next.is_none_or(|n| n.is_whitespace());
                if matches!(c, ':' | ';' | '?' | '!') && ends_word {
                    assert_eq!(fr.get(i.wrapping_sub(1)), Some(&'\u{a0}'), "{key}: before {c}");
                }
                if *c == '«' {
                    assert_eq!(next, Some('\u{a0}'), "{key}: after «");
                }
                if *c == '»' {
                    assert_eq!(fr.get(i.wrapping_sub(1)), Some(&'\u{a0}'), "{key}: before »");
                }
            }
            assert!(!texts[2].contains('"') && !texts[1].contains('"'), "{key}: straight quotes");
        }
    }

    #[test]
    fn lookup_by_locale() {
        assert_eq!(t_in("", "tray.quit"), "Quit Cedar");
        assert_eq!(t_in("en-GB", "tray.quit"), "Quit Cedar");
        assert_eq!(t_in("de-AT", "tray.quit"), "Cedar beenden");
        assert_eq!(t_in("fr-CA", "tray.quit"), "Quitter Cedar");
        assert_eq!(
            fill(t_in("de-DE", "tray.update_to"), &[("version", "0.2.0")]),
            "Auf Cedar 0.2.0 aktualisieren…"
        );
    }

    #[test]
    fn swiss_german_writes_ss() {
        let de = t_in("de-DE", "script.mac_dmg_broken");
        assert!(de.contains("ließ"));
        let ch = t_in("de-CH", "script.mac_dmg_broken");
        assert!(ch.contains("liess") && !ch.contains('ß'));
        for (key, _) in STRINGS {
            assert!(!t_in("de-CH", key).contains('ß'), "{key}");
        }
    }

    #[test]
    fn locales_are_normalized() {
        assert_eq!(normalize("de-ch"), Some("de-CH"));
        assert_eq!(normalize(" fr_CA "), Some("fr-CA"));
        assert_eq!(normalize("en"), Some("en"));
        assert_eq!(normalize("de"), None);
        assert_eq!(normalize("it-IT"), None);
        assert_eq!(normalize(""), None);
        for l in LOCALES {
            assert_eq!(normalize(l), Some(l));
        }
    }

    /// Each script line the table translates is still written, word for word, by a script.
    #[test]
    fn script_lines_match_the_scripts() {
        let scripts = [
            include_str!("macsetup.rs"),
            include_str!("tssetup.rs"),
            include_str!("winsetup.rs"),
        ]
        .concat();
        for (key, texts) in STRINGS.iter().filter(|(k, _)| k.starts_with("script.")) {
            let tagged = ["STEP: ", "ERROR: ", "fail '"]
                .iter()
                .any(|tag| scripts.contains(&format!("{tag}{}'", texts[0])));
            assert!(tagged, "{key}: no script writes {:?}", texts[0]);
        }
    }

    /// The scripts' English lines are found again; anything else passes through. (The current
    /// language is English in tests: nothing sets it.)
    #[test]
    fn script_lines_are_recognized() {
        assert_eq!(known("Updating WSL"), "Updating WSL");
        assert_eq!(known("some installer output"), "some installer output");
        let key = STRINGS
            .iter()
            .find(|(_, texts)| texts[0] == "Updating WSL")
            .map(|(k, _)| *k)
            .unwrap();
        assert_eq!(t_in("de-DE", key), "WSL wird aktualisiert");
    }
}
