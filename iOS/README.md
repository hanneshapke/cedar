# cedar for iOS

A native SwiftUI client for [cedar-chat](../chat/README.md); [android/](../android/README.md) is the same client for Android. The app is a thin client: it talks
to cedar-chat's `/api`, the same routes the web page uses, so your conversations, their unread
state, scheduled agents, tool and model settings, and notifications are the same in the app and
in the browser. cedar's own API key never leaves the server.

```
iPhone  ──/api (Bearer CHAT_ACCESS_TOKEN)──▶  cedar-chat  ──/v1──▶  cedar  ──▶  vLLM + sandbox
```

## Features

- The conversation list as cedar-chat keeps it, with a dot and a bold title on an unread
  response and a **Needs answer** chip when the agent asked a question; pull to refresh,
  rename, delete, and open on the web
- Replies stream in as the agent works: the model's reasoning (folded), text, tool calls with
  their input, output, duration and sources, token usage and cost
- The agent's images (charts, pictures it saved or drew) shown under the reply, with sharing; tap
  one to see it full screen and zoom in. PDFs it wrote are listed under the reply too
- The conversation's workspace files from the folder button in the chat's toolbar, with their
  sizes: images and PDFs open in a viewer (pinch to zoom, share), other files are left to the web
- Clarifying questions (`ask_user`) as a form: pick options, add your own words, send
- Plans to agree to and tool calls to allow or deny, when the server asks (`TOOL_APPROVALS`); a
  plan Cedar chose to make partway through (`propose_plan`) is marked as such
- A Plans setting for your messages: the server's, when Cedar thinks it helps, for longer
  requests, always, or never
- Charts from `generate_chart` as native Swift Charts cards, drawn from the call's data: a legend
  that hides and shows series, touch to read the values, and the data as a table
- Maps, recipes, routes, stock comparisons and product searches as native cards: Apple Maps
  with numbered pins and route lines (tap for full screen), a recipe checklist with steps to
  tick off, stock charts with ratings, returns and news, and product tiles with photos, prices,
  the reading of their reviews and comparable options to step through
- Follow-ups continue the conversation; a stop button cancels the running response
- Attach PDFs (any model) and photos or images (models registered as multimodal) to a
  message from the paperclip: the photo library or Files. Photos in a type cedar doesn't take,
  such as HEIC, are sent as JPEG; the limits are the server's (10 files, 20 MB each, 32 MB in all)
- Starred conversations: star a chat or a scheduled run with the star in the chat's toolbar, a
  swipe or a long press, and find them under **Starred** in the sidebar's switch, most recently
  starred first. Stars are kept by cedar-chat, so they are the same here, on the web and in the
  desktop app
- Scheduled agents: a **Chats | Starred | Scheduled** switch at the top of the sidebar lists each
  schedule with its timing, next run and last result; expand one to see its runs and open any of
  them as a conversation. **Run now**, **Pause** and **Resume** from a swipe or a long press, and
  read the prompt; creating and editing schedules is done on the web
- Live notices: a response that ends in another conversation marks it unread at once
- iPhone push notifications when a response ends while you are away (see below)
- A model and reasoning level picked from what cedar has registered
- Settings → Memory: what Cedar remembers about you across conversations (see
  [Memory](../README.md#memory)), newest first, with when each fact was saved and a tap to open
  the conversation it came from. Swipe to delete one, or **Delete All**; deletions apply at once.
  Shown only when the server has memory (`memory` in `GET /api/config`)
- Markdown rendering: headings, lists, quotes, tables, and code blocks with a copy button
- Connecting by scanning the QR code the chat's Settings → Phone page, the Cedar desktop app or
  `scripts/cedar-stack.sh --pair` shows
- iPhone and iPad layouts, light and dark mode
- English, German, French and Spanish, picked on first launch or under Settings → Language (see
  [Languages](#languages))

## Requirements

- Xcode 16 or newer (the project uses folder-synchronized groups)
- iOS 17 or newer
- A cedar-chat server the device can reach over HTTPS (for example through `tailscale serve`),
  and its `CHAT_ACCESS_TOKEN` if one is set

## Running it

1. Start cedar and cedar-chat (see the [top-level README](../README.md) and
   [chat/README.md](../chat/README.md)), set `CHAT_ACCESS_TOKEN`, and serve the chat over HTTPS.
   The app only connects over HTTPS (iOS App Transport Security, with no exception). The
   simplest way is Tailscale: the [Mac app](../desktop/README.md#using-it-from-an-iphone-or-ipad)
   does it with one checkbox, `scripts/cedar-stack.sh` runs `tailscale serve` for you, and
   [DEPLOYMENT.md](../DEPLOYMENT.md#over-https-with-tailscale-the-ios-app) shows the steps for a
   systemd install. The server URL is then `https://<machine>.<tailnet>.ts.net`.
2. Open `iOS/Cedar.xcodeproj` in Xcode, choose the **Cedar** scheme, and run it. To run on a
   device, pick your team under *Signing & Capabilities*.
3. On first launch, pick a language, then enter the server URL and the access token in Settings and tap
   **Test Connection**. With the Cedar Mac app or the stack script, tap **Scan QR Code** instead and scan the code
   under *iPhone & iPad → Show QR code*, or the one `scripts/cedar-stack.sh --pair` prints on
   Linux: it fills in both and tests the connection (it asks for the camera once). The models cedar has registered appear in the picker once it answers.

The access token is stored in the Keychain. Everything else is stored in `UserDefaults`.

cedar-chat itself speaks plain HTTP, so put it behind something that terminates TLS with a
certificate the phone trusts: `tailscale serve`, or a reverse proxy such as Caddy with a Let's
Encrypt certificate. Anyone with the access token can run the agent, so keep the token secret
even on a private tailnet.

## Push notifications

Pushes come from cedar-chat, which already decides what is worth telling you about (Settings →
Notifications in the web UI: failures, questions and scheduled runs, or every response that
ends while nobody is watching). To get them on the phone:

1. In your Apple developer account, under *Certificates, Identifiers & Profiles → Keys*, make a
   key with *Apple Push Notifications service (APNs)* enabled and download its `.p8` file; note
   the key id, and your team id (under *Membership*). The app's bundle id (`dev.cedar.Cedar`,
   or what you changed it to) needs the Push Notifications capability, which Xcode adds when
   you run the app with your team.
2. In the web UI, Settings → **Notifications** → **iPhone push**: paste the key, the key id and
   the team id, check the bundle id, pick **Sandbox** for an app installed from Xcode or
   **Production** for TestFlight or the App Store, and save. Choose it as the default method or
   for a scheduled agent.
3. Open the app and allow notifications when asked (or tap **Allow Notifications** in its
   Settings). The phone registers itself with the server and appears in the web UI's list of
   iPhones, where it can be removed. Changing the server in the app moves the registration.

A tap on a notification opens its conversation. One that arrives while that conversation is
open is not shown; the response is already in front of you.

`Config/Cedar.entitlements` holds the `aps-environment` entitlement; Xcode turns `development`
into `production` for distribution builds.

## Layout

| Path | Contents |
| --- | --- |
| `Cedar/App` | The app entry point, the root split view, `AppModel` (settings, the conversation list, the inbox count, the live notices, open chats) and `PushRegistrar` (APNs) |
| `Cedar/Models` | The `/api` types, the event-to-transcript reducer (text, reasoning, plans, approvals, tool calls, questions, images), and a Markdown block parser |
| `Cedar/Networking` | `ChatClient` for `/api`, and the Server-Sent Events parser |
| `Cedar/Storage` | Settings, Keychain access, and the Mac app's connection code (`cedar://connect?server=…&token=…`) |
| `Cedar/Features` | SwiftUI screens: conversation list, chat (turns, questions, approvals, images), and settings (with the QR scanner and the first launch's language picker) |
| `Cedar/Localization` | The offered languages, the picked one (`LanguageStore`), and `L10n` for text built outside SwiftUI views |
| `Cedar/Localizable.xcstrings`, `Cedar/InfoPlist.xcstrings` | The String Catalogs: the app's text and the permission prompts, in German, French and Spanish |
| `CedarTests` | Unit tests for the API types, the reducer, the SSE parser, Markdown, the connection code, and the languages |
| `Config/Cedar.entitlements` | Push notifications |

## How it talks to the server

- `GET /api/conversations` is the list; `GET /api/conversations/{id}` the turns of one.
- A message is `POST /api/conversations/{id}/messages` (a new chat first creates the
  conversation). The model and reasoning level are the app's settings; tools, the turn limit
  and the system prompt are the server's.
- Each turn is rebuilt by replaying its event stream (`GET /api/turns/{id}/events`). A running
  turn is followed live; the stream reconnects from the last event it saw.
- `GET /api/events` is followed for as long as the app runs: a `turn.finished` notice refreshes
  the list and the open chat. The inbox count comes from `GET /api/inbox`.
- Opening a conversation, or watching its response end with the app in front, calls
  `POST /api/conversations/{id}/seen`, as the web page does.
- Plans and approvals answer through `POST /api/turns/{id}/plan` and
  `POST /api/turns/{id}/approvals/{approval_id}`; a clarifying question is answered with an
  ordinary message in the same text form the web page sends.
- Images are fetched from `GET /api/conversations/{id}/files/{path}` with the access token.
- The device token goes to `POST /api/notifications/apns/devices`.
- Settings → Memory reads `GET /api/memory` and deletes with `DELETE /api/memory/{id}` and
  `DELETE /api/memory`. A server whose `GET /api/config` has no `memory` flag, or that answers
  `/api/memory` with 404, is older than memory, and the section is hidden or says so.
- Every message carries the interface language as `locale` (`de-CH`); a language picked in the
  app also goes to `PUT /api/settings/locale` (`null` for Automatic), so scheduled runs and
  notifications follow it. Before a server is set up, it is sent once one is.

## Languages

The app is in English, German, French and Spanish. A new install asks first, before the server's
settings, starting on the device's language; Settings → Language changes it at once, or sets it to
**Automatic** to follow the device. The offered locales are `en`, `en-US`, `en-GB`, `de-DE`,
`de-AT`, `de-CH`, `fr-FR`, `fr-BE`, `fr-CH`, `fr-CA`, `es-ES`, `es-MX`, `es-419` and `es-US`: the
language picks the text, the region the formats of dates, numbers and prices. A device language
whose region isn't offered starts on the language's main variant: `de-LU` on `de-DE`, and Spanish
from elsewhere (`es-AR`) on `es-419`. Swiss German has its own text only where German writes ß
(`de-CH` in the catalog, with ss); everything else falls back to German. Likewise the Spanish
(`es`) is neutral Latin American (*computadora*, *celular*), and Spain has its own text (`es-ES`)
only for the keys where its words differ (*coche*, *móvil*, *Ajustes* for the system's Settings
app); everything else falls back to `es`. What the system draws (the photo picker, permission
prompts) follows on the next launch.

- The root view sets SwiftUI's `locale` to the picked language, so `Text("…")`, `Button("…")` and
  the like look themselves up in it. Text built elsewhere (models, errors, labels kept in a
  `String`) goes through `L10n.string("…")` or `L10n.format("… %@", value)`, which read the
  compiled catalog's `.lproj` for the picked language (`de-CH`, then `de`, then English;
  `es-ES`, then `es`).
- Format numbers and dates with `.locale(L10n.locale)`, never a fixed locale or the device's.

To add text, write it in English in the code. A SwiftUI string literal is added to
`Cedar/Localizable.xcstrings` by Xcode when it builds; one passed to `L10n` has to be added by
hand, as a manually managed string (Xcode can't find it in the code). Give every key a German,
a French and a Spanish translation, following [the glossary](../docs/i18n-glossary.md) (German
*du*, French *vous*, a no-break space before `:` `;` `?` `!` in French, Spanish *tú* with `¿` `¡`
and «así»), a `de-CH` one where the German has ß, and an `es-ES` one only where Spain says it
differently. Permission prompts are in `Cedar/InfoPlist.xcstrings`.

## Tests

Run them in Xcode with ⌘U, or from the command line:

```sh
cd iOS
xcodebuild test -project Cedar.xcodeproj -scheme Cedar \
  -destination 'platform=iOS Simulator,name=iPhone 16'
```

### Continuous integration

`.github/workflows/ios.yml` runs the same tests on a macOS runner for every pull request and
push to main that touches `iOS/`, and on request (Actions → iOS → Run workflow). It builds the
app for the iPhone 16 simulator and runs the unit tests against it, then builds for a device
unsigned, which checks the entitlements and the device SDK. When a run fails, the `.xcresult`
is kept as an artifact for a week and opens in Xcode.

### TestFlight

`.github/workflows/testflight.yml` (Actions → TestFlight → Run workflow) archives the app signed
for distribution and uploads it to App Store Connect, where it shows up under TestFlight once
processed. It needs these repository secrets:

| Secret | What it is |
| --- | --- |
| `IOS_DIST_CERT_P12`, `IOS_DIST_CERT_PASSWORD` | An Apple Distribution certificate exported as .p12, base64-encoded, and its password |
| `IOS_PROFILE` | An App Store provisioning profile for `dev.cedar.Cedar` named `Cedar App Store`, base64-encoded |
| `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_P8` | An App Store Connect API key (App Manager role) that uploads the build |
| `KEYCHAIN_PASSWORD` | Any random string, for the runner's temporary keychain |

The app record for `dev.cedar.Cedar` must exist in App Store Connect before the first upload.

- **Releases.** `release.yml` calls this workflow for every release, so each version reaches
  TestFlight. `MARKETING_VERSION` in the project is the release version (the release checks it);
  bump it with the Cargo.toml versions on the `release/<version>` branch.
- **Build number.** The workflow sets `CURRENT_PROJECT_VERSION` to the UTC time of the build
  (`YYYYMMDD.HHMMSS`), so every upload is higher than the last, from a release or a manual run.
- **Export compliance.** The app declares `ITSAppUsesNonExemptEncryption = NO` (it only uses
  HTTPS), so builds don't wait on the encryption question.
- **Push.** TestFlight builds use the production APNs environment, so set the server's iPhone
  push setting to **Production** (see [Push notifications](#push-notifications)).

### Sideloading

`.github/workflows/ipa.yml` (Actions → IPA → Run workflow) builds the app as an unsigned IPA,
`Cedar-<version>-ios.ipa`, kept as the run's `ipa-ios` artifact; `release.yml` calls it for every
release and attaches the IPA to the GitHub Release. It needs no secrets. Install it with
[SideStore](https://sidestore.io) or AltStore, which sign it on the phone with your own Apple ID.
The simplest way is to add the project's source, `https://hanneshapke.github.io/cedar/sidestore.json`,
under **Sources → +**: it lists every release's IPA, so SideStore offers updates too (see
[docs/sidestore](../docs/sidestore/README.md)). What changes when the app is sideloaded:

- **No push.** The IPA carries no entitlements, so the sideloaded app doesn't register for iPhone
  push (free Apple IDs can't have the capability anyway). Conversations, the inbox and live
  notices work as usual while the app is open.
- **Weekly refresh.** With a free Apple ID the app is signed for 7 days and counts towards its
  3 active apps; SideStore refreshes it in the background or when you tap **Refresh**.
- **Bundle id.** SideStore may append your team id to `dev.cedar.Cedar`; the app doesn't depend
  on it. The access token is in that install's Keychain, so a fresh install asks for it again.

The run's summary lists the IPA's bundle id, version, build number, minimum iOS version, size,
SHA-256 and MD5, and the IPA comes with `Cedar-<version>-ios.source.json`, its entry in the
source.

## Limitations

- Events arrive per model turn and per tool call, not per token, so text appears a turn at a
  time.
- Document cards open on the web rather than being drawn in the app, as does a map, recipe,
  route or stock card whose result the app can't read.
- Attachments (images and PDFs with a message), scheduled agents and the server's settings are
  managed in the web UI.
- iOS suspends network connections in the background. When you return to the app, running
  responses reconnect and catch up from the event log, and the list is refreshed.
