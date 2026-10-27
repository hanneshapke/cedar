# Translation glossary

cedar's clients (web chat, desktop app, iOS, Android) are translated into German and French. English is
the source. Use these terms everywhere so the same thing has the same name on every screen.

## Tone

- **German** addresses the user informally: *du*, *dein*, imperative *Wähle …*. Capitalize *Du* only at the
  start of a sentence.
- **French** addresses the user formally: *vous*, *votre*, imperative *Choisissez …*.
- **Swiss German** (`de-CH`) writes *ss* instead of *ß*. The clients derive it from the German text; don't
  keep a separate copy unless a word differs otherwise.
- Keep it short and plain, like the English. Buttons are verbs (*Speichern*, *Enregistrer*).

## Typography

- German quotes: „so“. French quotes: « ainsi », with a no-break space (U+00A0) inside.
- French puts a no-break space (U+00A0) before `:`, `;`, `?` and `!`.
- Use the ellipsis character `…`, as the English does.
- Numbers, dates and times are never written into a string by hand: format them for the user's locale
  (`Intl` on the web, `Locale` on iOS, `java.util.Locale` on Android).

## Never translated

*Cedar*, product and service names (Gmail, Google Calendar, Pushover, Tailscale, Docker, vLLM, DeepSeek,
Firebase, Brave Search, Best Buy, iPhone, Android), the interface names *Ledger* and *Dossier*, tool names
as the model calls them (`web_search`, `run_python`), file names, code, and API error messages from the
server (they are for developers).

## Terms

| English | Deutsch | Français |
| --- | --- | --- |
| conversation | Unterhaltung | conversation |
| new chat | Neuer Chat | Nouvelle conversation |
| message | Nachricht | message |
| response, reply | Antwort | réponse |
| run (one agent run) | Lauf | exécution |
| task | Aufgabe | tâche |
| agent | Agent | agent |
| scheduled agent, schedule | geplanter Agent, Zeitplan | agent planifié, planification |
| plan, step | Plan, Schritt | plan, étape |
| approve, approval | freigeben, Freigabe | approuver, approbation |
| Allow / Allow always / Deny | Erlauben / Immer erlauben / Ablehnen | Autoriser / Toujours autoriser / Refuser |
| tool, tool group | Werkzeug, Werkzeuggruppe | outil, groupe d'outils |
| personal tools | Eigene Werkzeuge | Outils personnels |
| community tools | Community-Werkzeuge | Outils de la communauté |
| sandbox | Sandbox | bac à sable |
| workspace | Arbeitsbereich | espace de travail |
| file, attachment, attach | Datei, Anhang, anhängen | fichier, pièce jointe, joindre |
| sources | Quellen | sources |
| model | Modell | modèle |
| reasoning (level) | Denkaufwand | niveau de raisonnement |
| system prompt | System-Prompt | prompt système |
| settings | Einstellungen | Réglages |
| interface | Oberfläche | Interface |
| language | Sprache | Langue |
| notifications | Benachrichtigungen | Notifications |
| inbox | Posteingang | Boîte de réception |
| server | Server | serveur |
| access token | Zugriffstoken | jeton d'accès |
| connect, pair (a phone) | verbinden, koppeln | connecter, associer |
| star, starred | markieren, Markiert | ajouter aux favoris, Favoris |
| send | Senden | Envoyer |
| stop (a run) | Stoppen | Arrêter |
| cancel (a dialog) | Abbrechen | Annuler |
| save | Speichern | Enregistrer |
| delete | Löschen | Supprimer |
| rename | Umbenennen | Renommer |
| retry | Erneut versuchen | Réessayer |
| copy, copied | Kopieren, Kopiert | Copier, Copié |
| update (software) | Update | mise à jour |
| Automatic (language) | Automatisch | Automatique |
