# Translation glossary

cedar's clients (web chat, desktop app, iOS, Android) and the website are translated into German,
French and Spanish. English is the source. Use these terms everywhere so the same thing has the same
name on every screen.

## Tone

- **German** addresses the user informally: *du*, *dein*, imperative *Wähle …*. Capitalize *Du* only at the
  start of a sentence.
- **French** addresses the user formally: *vous*, *votre*, imperative *Choisissez …*.
- **Spanish** addresses the user informally: *tú*, *tu*, imperative *Elige …*. It is neutral Latin American
  Spanish (*computadora*, *celular*, *carpeta*); Spain (`es-ES`) gets its own text only where a word
  clearly differs (*ordenador*, *móvil*), in an `es-ES` catalog that holds just those keys.
- **Swiss German** (`de-CH`) writes *ss* instead of *ß*. The clients derive it from the German text; don't
  keep a separate copy unless a word differs otherwise.
- Keep it short and plain, like the English. Buttons are verbs (*Speichern*, *Enregistrer*).

## Typography

- German quotes: „so“. French quotes: « ainsi », with a no-break space (U+00A0) inside.
- French puts a no-break space (U+00A0) before `:`, `;`, `?` and `!`.
- Spanish opens questions and exclamations with `¿` and `¡`, and quotes «así» (no spaces inside).
- Use the ellipsis character `…`, as the English does.
- Numbers, dates and times are never written into a string by hand: format them for the user's locale
  (`Intl` on the web, `Locale` on iOS, `java.util.Locale` on Android).

## Never translated

*Cedar*, product and service names (Gmail, Google Calendar, Pushover, Tailscale, Docker, vLLM, DeepSeek,
Firebase, Brave Search, Best Buy, iPhone, Android), the interface names *Ledger* and *Dossier*, tool names
as the model calls them (`web_search`, `run_python`), file names, code, and API error messages from the
server (they are for developers).

## Terms

| English | Deutsch | Français | Español |
| --- | --- | --- | --- |
| conversation | Unterhaltung | conversation | conversación |
| new chat | Neuer Chat | Nouvelle conversation | Nueva conversación |
| message | Nachricht | message | mensaje |
| response, reply | Antwort | réponse | respuesta |
| run (one agent run) | Lauf | exécution | ejecución |
| task | Aufgabe | tâche | tarea |
| agent | Agent | agent | agente |
| scheduled agent, schedule | geplanter Agent, Zeitplan | agent planifié, planification | agente programado, programación |
| plan, step | Plan, Schritt | plan, étape | plan, paso |
| approve, approval | freigeben, Freigabe | approuver, approbation | aprobar, aprobación |
| Allow / Allow always / Deny | Erlauben / Immer erlauben / Ablehnen | Autoriser / Toujours autoriser / Refuser | Permitir / Permitir siempre / Denegar |
| tool, tool group | Werkzeug, Werkzeuggruppe | outil, groupe d'outils | herramienta, grupo de herramientas |
| personal tools | Eigene Werkzeuge | Outils personnels | Herramientas propias |
| community tools | Community-Werkzeuge | Outils de la communauté | Herramientas de la comunidad |
| sandbox | Sandbox | bac à sable | sandbox (entorno aislado) |
| workspace | Arbeitsbereich | espace de travail | espacio de trabajo |
| file, attachment, attach | Datei, Anhang, anhängen | fichier, pièce jointe, joindre | archivo, adjunto, adjuntar |
| sources | Quellen | sources | fuentes |
| model | Modell | modèle | modelo |
| reasoning (level) | Denkaufwand | niveau de raisonnement | nivel de razonamiento |
| system prompt | System-Prompt | prompt système | prompt del sistema |
| settings | Einstellungen | Réglages | Configuración |
| interface | Oberfläche | Interface | Interfaz |
| language | Sprache | Langue | Idioma |
| notifications | Benachrichtigungen | Notifications | Notificaciones |
| inbox | Posteingang | Boîte de réception | Bandeja de entrada |
| server | Server | serveur | servidor |
| computer | Computer | ordinateur | computadora (es-ES: ordenador) |
| phone (the user's) | Handy | téléphone | celular (es-ES: móvil) |
| access token | Zugriffstoken | jeton d'accès | token de acceso |
| connect, pair (a phone) | verbinden, koppeln | connecter, associer | conectar, vincular |
| star, starred | markieren, Markiert | ajouter aux favoris, Favoris | destacar, Destacadas |
| send | Senden | Envoyer | Enviar |
| stop (a run) | Stoppen | Arrêter | Detener |
| cancel (a dialog) | Abbrechen | Annuler | Cancelar |
| save | Speichern | Enregistrer | Guardar |
| delete | Löschen | Supprimer | Eliminar |
| rename | Umbenennen | Renommer | Cambiar nombre |
| retry | Erneut versuchen | Réessayer | Reintentar |
| copy, copied | Kopieren, Kopiert | Copier, Copié | Copiar, Copiado |
| update (software) | Update | mise à jour | actualización |
| Automatic (language) | Automatisch | Automatique | Automático |
