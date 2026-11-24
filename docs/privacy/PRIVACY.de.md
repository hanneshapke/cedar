<!-- translated-from: PRIVACY.md 4dabea162098 -->
# Datenschutzerklärung

_Stand: 24. November 2026_

> Dies ist eine Übersetzung. Maßgeblich ist die [englische Fassung](https://github.com/hanneshapke/cedar/blob/main/PRIVACY.md).

**cedar sammelt keinerlei Daten. Punkt.**

Das gilt für den cedar-Server und für jede cedar-App: iOS, Android, Desktop und den Web-Chat. Der einzige Dienst, den wir betreiben, Cedar Private Inference, ist freiwillig: Du wählst ihn, statt selbst ein Modell zu betreiben, und er speichert nur, was er braucht, um dir Guthaben zu verkaufen (siehe [Cedar Private Inference](#cedar-private-inference-freiwillig)).

## Was das bedeutet

- Wir sammeln, speichern, verkaufen oder teilen keine personenbezogenen Daten.
- Es gibt keine Analyse, keine Telemetrie, keine Absturzberichte, keine Werbung und kein Tracking jeglicher Art.
- Um cedar zu nutzen, brauchst du kein Konto bei uns. Der einzige Dienst, den wir betreiben, ist das freiwillige Cedar Private Inference, und auch durch ihn laufen deine Unterhaltungen nicht.

## Wohin deine Daten gehen

cedar wird selbst gehostet. Die Apps sprechen nur mit dem cedar-Server, den **du** betreibst und mit dem du sie koppelst. Deine Unterhaltungen, Läufe, Dateien und Einstellungen bleiben auf diesem Server und auf deinen eigenen Geräten. Wir sehen sie nie.

Um herauszufinden, ob eine neuere Version erschienen ist, fragen die Mac- und Windows-App und der Web-Chat alle paar Stunden bei GitHub nach, wo cedars Releases veröffentlicht werden. Die Anfrage enthält die installierte Version und nichts über dich oder deine Unterhaltungen; GitHub sieht sie wie jeden Besuch einer öffentlichen Seite. Die Einrichtungsseite (Desktop) oder `CHAT_UPDATE_CHECK=off` (Chat-Server) schaltet das ab.

Nur wenn du auf *Docker für mich einrichten* oder *Tailscale für mich einrichten* klickst, lädt die Mac- und Windows-App das Installationsprogramm dieses Programms von seinem Hersteller herunter (Docker Desktop von desktop.docker.com, Tailscale von pkgs.tailscale.com) und installiert es. Wie jeder Download enthält die Anfrage nichts über dich oder deine Unterhaltungen. `scripts/cedar-stack.sh` macht unter Linux dasselbe (get.docker.com, tailscale.com), wenn du mit Ja antwortest. Für Docker und Tailscale gelten dann deren eigene Datenschutzerklärungen.

Für alles, womit du cedar verbindest (zum Beispiel deinen LLM-Server oder ein Werkzeug eines Drittanbieters, das du aus der Community-Registry installierst), gilt die Datenschutzerklärung dieses Dienstes, nicht unsere.

## Cedar Private Inference (freiwillig)

Statt selbst ein Modell zu betreiben, kannst du Cedar Private Inference verbinden: auf der Einrichtungsseite der Desktop-App, in den Einstellungen des Web-Chats oder mit `cedar private-inference connect`. Der Dienst verkauft Prepaid-Guthaben für ein gehostetes Modell und startet mit einem kostenlosen Testguthaben. Wenn du ihn nie verbindest, betrifft dich nichts in diesem Abschnitt.

- **Deine Unterhaltungen gehen direkt an den Inferenzanbieter, nie über uns.** Der Dienst übergibt deinem cedar-Server einen Schlüssel für den Anbieter Doubleword (doubleword.ai), und von da an schickt dein cedar-Server jeden Modellaufruf direkt an Doubleword: deine Nachrichten, den System-Prompt, dein Gedächtnis, soweit ein Lauf es gezeigt bekommt, und was die Werkzeuge des Agenten zurückgeben. Doubleword betreibt die Modelle auf GPUs, die es bei Hosting-Anbietern mietet. Für das Konto des Schlüssels ist Zero Data Retention eingeschaltet, Doubleword speichert die Prompts und Antworten dieser Aufrufe also nicht; für das, was Doubleword erhält, gilt seine eigene Datenschutzerklärung.
- **Was der Dienst speichert:** deine E-Mail-Adresse (um dich anzumelden, mit einem Link, der an sie geschickt wird), dein Guthaben (das Testguthaben, deine Zahlungen und was du verbraucht hast) und deinen Schlüssel. Zahlungen laufen über Stripe; deine Karte sehen wir nie. Was du verbraucht hast, meldet Doubleword als Summen je Schlüssel, in Tokens und Geld, ohne jeden Inhalt. Der Dienst speichert keine deiner Unterhaltungen, Prompts, Antworten, Dateien und nichts aus deinem Gedächtnis.
- **Was eine cedar-App ihm schickt**, nur während du verbindest: einen zufälligen Einmalcode und einen Namen für den Computer oder Server, den du verbindest (seinen Computernamen, die Adresse des Chats oder den Hostnamen des Servers), der auf der Seite angezeigt wird, auf der du die Verbindung freigibst.
- **Seine Website** setzt ein Cookie, das dich angemeldet hält, und sonst keines.
- **Beenden.** Trennen entfernt die Modelle und den Schlüssel aus deinem cedar. Der Schlüssel bleibt gültig, bis du ihn auf deiner Kontoseite widerrufst. Wenn du dein Konto und alles, was damit gespeichert ist, löschen lassen willst, bitte uns darum (siehe [Kontakt](#kontakt)).

Das ist noch nicht durch Hardware überprüft: Heute beruht es auf Doublewords Zusage, nichts zu speichern. Cedar zeigt es am Modell an, sobald ein Anbieter durch eine Attestierung, die du selbst prüfen kannst, nachweisen kann, dass deine Prompts nur in überprüfter Hardware verarbeitet werden.

## Gedächtnis

cedar kann sich über Unterhaltungen hinweg Dinge über dich merken, etwa deinen Namen, deine Vorlieben oder ein Projekt, an dem du arbeitest, damit du sie nicht wiederholen musst. **Du behältst die Kontrolle über dein Gedächtnis, und es wird nie an Dritte weitergegeben.**

- Dein Gedächtnis wird nur in der Datenbank des cedar-Servers gespeichert, den **du** betreibst. Wir sehen es nie, und cedar schickt es nie an uns oder an sonst jemanden.
- Der einzige Ort, an den es geht, ist der LLM-Server, den du eingerichtet hast, als Teil jeder Anfrage, wie der Rest deiner Unterhaltung. Ist das statt deines eigenen Servers ein gehosteter Modellanbieter, gilt dessen Datenschutzerklärung für das, was er erhält.
- cedar behält Fakten, Vorlieben und dauerhafte Anweisungen, die du genannt hast, jeweils mit deinen Worten, aus denen sie stammen, eine Notiz dazu, worum es in jeder Unterhaltung ging, Arbeitsweisen, die gut funktioniert haben, und ein kurzes Profil von dir. Ein Fakt wird nur behalten, wenn er deine eigenen Worte aus der Unterhaltung zitiert, nie etwas, das ein Werkzeug, eine Webseite, eine E-Mail oder ein Dokument gesagt hat, und Text, der wie ein Passwort, ein Schlüssel oder eine Karten-, Konto- oder Ausweisnummer aussieht, wird abgelehnt.
- Du kannst alles davon sehen, die Unterhaltung, aus der jede Erinnerung stammt, wann sie zuletzt verwendet wurde und wie sie sich mit der Zeit verändert hat, und jederzeit eine einzelne Erinnerung oder alles bearbeiten, anheften, archivieren oder löschen, unter Einstellungen → Gedächtnis (Web-Chat und Desktop; die iOS- und Android-Apps zeigen an und löschen) oder über cedars API (`/v1/memory`). Die Ablaufspur eines Laufs zeigt genau, welche Erinnerungen ihm gezeigt wurden.
- Eine gelöschte Erinnerung wird sofort samt ihrem Verlauf aus der Datenbank entfernt und ab deiner nächsten Nachricht nicht mehr verwendet. Eine archivierte erscheint in keinem Prompt mehr und bleibt, bis du sie löschst.
- Du kannst das Gedächtnis unter Einstellungen → Werkzeuge → Gedächtnis abschalten, und wer den Server betreibt, kann es ganz abschalten (`MEMORY=off`).

## Eine Unterhaltung beisteuern (freiwillig)

Mit cedar kannst du eine Unterhaltung einem gemeinsamen Korpus *schenken*, mit dem offene Agent-Modelle trainiert werden. Das ist ein Vorschlag in Arbeit (siehe `docs/design/contributions.md`); was es heute gibt, steht hier, damit der erste Satz dieser Erklärung wahr bleibt, während es entsteht.

- **Nichts wird gesendet, wenn du es nicht sendest.** Dein cedar-Chat-Server kennt einen Korpus, den des Projekts, es sei denn, wer ihn betreibt, hat einen anderen angegeben oder die Funktion abgeschaltet (`CHAT_CONTRIBUTIONS_URL`); ihn zu kennen, sendet nichts. Du entscheidest einmal unter Einstellungen → Beisteuern, ob du überhaupt beisteuerst, welche Lizenz du wählst und ob du ein zufälliges Pseudonym verwendest. Danach ist jeder Beitrag eine Unterhaltung, von dir ausgewählt, nach einer Seite, die genau zeigt, was deinen Server verlassen wird. Es gibt keinen Schalter „alles teilen“, kein Sammeln im Hintergrund und keine Erinnerungen.
- **Was ein Beitrag enthält.** Deine Prompts, die Antworten und Werkzeugaufrufe des Agenten, den System-Prompt und die Werkzeugdefinitionen, mit denen er lief, wie der Lauf endete und das Ergebnis, das du gemeldet hast. Bevor du ihn siehst, entfernt cedar-Chat die Inhalte und Namen von Anhängen, deine Liste der Erinnerungen, Lauf- und Schlüsselkennungen, Zeitstempel (der Tag bleibt) und deine Region (die Sprache bleibt), und ersetzt E-Mail-Adressen, Telefon- und Kontonummern, IP-Adressen, Schlüssel und Namen von Benutzerverzeichnissen durch erfundene Platzhalter derselben Form (eine Adresse bei example.com, eine 555-Nummer, eine Dokumentations-IP-Adresse, ein erfundener Name), innerhalb dieser Unterhaltung derselbe Platzhalter für denselben Wert und nie aus dem Wert abgeleitet. Der Beitrag listet auf, welche Werte Platzhalter sind. Unterhaltungen, die dein Gmail oder deinen Kalender oder ein Gesundheitswerkzeug verwendet haben, können gar nicht beigesteuert werden.
- **Das Schwärzen ist nach bestem Bemühen.** Die Suche findet, was eine Form hat. Eine Tatsache in deinen eigenen Worten wird nicht erkannt; dafür ist die Prüfseite da, auf der du mehr schwärzen und wiederherstellen kannst, was die Suche falsch erkannt hat.
- **Keine Identität.** Ein Beitrag enthält kein Konto, keine E-Mail-Adresse, keinen Schlüssel, kein Gerät und keine Adresse von dir. Sein Pseudonym, falls du eins gewählt hast, ist eine zufällige Kennung, die auf deinem Server erzeugt wird und die du wechseln oder weglassen kannst. Wer den Korpus betreibt, sieht die Verbindung wie jeder Webserver und verpflichtet sich in seiner eigenen Erklärung, keine Anfrage-Logs aufzubewahren.
- **Du kannst zurückziehen.** Dein Server bewahrt für jeden Beitrag einen Beleg auf, mit einem Token, das nur er kennt, und unter Einstellungen → Beisteuern kannst du einen Beitrag zurückziehen. Ein zurückgezogener Beitrag wird aus dem Eingang des Korpus gelöscht und in keiner späteren Veröffentlichung mehr enthalten sein; eine bereits veröffentlichte Version ist ein unveränderlicher Stand und kann von denen, die sie heruntergeladen haben, nicht zurückgeholt werden.
- **Lizenz.** Was du geschrieben hast, lizenzierst du unter der Lizenz, die du gewählt hast (CC0 oder CC BY 4.0). Die Antworten des Agenten sind die Ausgabe des Modells, das dein Server ausgeführt hat; der Korpus veröffentlicht nur Modelle, deren Bedingungen das erlauben.

## Geräteberechtigungen

Die Apps fragen nach einigen Berechtigungen, die nur auf deinem Gerät und nur für Folgendes verwendet werden:

- **Kamera** – um den QR-Code zu scannen, der die App mit deinem cedar-Server koppelt. Bilder werden nie gespeichert oder irgendwohin gesendet.
- **Lokales Netzwerk / Internet** – um deinen eigenen cedar-Server zu erreichen.
- **Benachrichtigungen** (Android) – um dir mitzuteilen, wenn ein Lauf fertig ist.


## Kinder

Die cedar-Software sammelt keine Daten, also auch keine von Kindern. Cedar Private Inference richtet sich an Erwachsene, und wir speichern nicht wissentlich die E-Mail-Adresse eines Kindes.

## Änderungen

Sollte sich diese Erklärung je ändern, wird die neue Fassung in der Datei PRIVACY.md im Repository veröffentlicht.

## Kontakt

Fragen? Eröffne ein Issue unter https://github.com/hanneshapke/cedar/issues.
