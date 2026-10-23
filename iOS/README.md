# cedar for iOS

A native SwiftUI client for [cedar-chat](../chat/README.md). The app is a thin client: it talks
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
- The agent's images (charts, pictures it saved or drew) shown under the reply, with sharing
- Clarifying questions (`ask_user`) as a form: pick options, add your own words, send
- Plans to agree to and tool calls to allow or deny, when the server asks (`TOOL_APPROVALS`)
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
- Scheduled agents: a **Chats | Scheduled** switch at the top of the sidebar lists each
  schedule with its timing, next run and last result; expand one to see its runs and open any of
  them as a conversation. **Run now**, **Pause** and **Resume** from a swipe or a long press, and
  read the prompt; creating and editing schedules is done on the web
- Live notices: a response that ends in another conversation marks it unread at once
- iPhone push notifications when a response ends while you are away (see below)
- A model and reasoning level picked from what cedar has registered
- Markdown rendering: headings, lists, quotes, tables, and code blocks with a copy button
- Connecting by scanning the QR code the chat's Settings → iPhone page, the Cedar desktop app or
  `scripts/cedar-stack.sh --pair` shows
- iPhone and iPad layouts, light and dark mode

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
3. On first launch, enter the server URL and the access token in Settings and tap
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
| `Cedar/Features` | SwiftUI screens: conversation list, chat (turns, questions, approvals, images), and settings (with the QR scanner) |
| `CedarTests` | Unit tests for the API types, the reducer, the SSE parser, Markdown, and the connection code |
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

- **Build number.** The workflow sets `CURRENT_PROJECT_VERSION` to the run number, so every
  upload is higher than the last. Change `MARKETING_VERSION` in the project for a new version.
- **Export compliance.** The app declares `ITSAppUsesNonExemptEncryption = NO` (it only uses
  HTTPS), so builds don't wait on the encryption question.
- **Push.** TestFlight builds use the production APNs environment, so set the server's iPhone
  push setting to **Production** (see [Push notifications](#push-notifications)).

## Limitations

- Events arrive per model turn and per tool call, not per token, so text appears a turn at a
  time.
- Document cards open on the web rather than being drawn in the app, as does a map, recipe,
  route or stock card whose result the app can't read.
- Attachments (images and PDFs with a message), scheduled agents and the server's settings are
  managed in the web UI.
- iOS suspends network connections in the background. When you return to the app, running
  responses reconnect and catch up from the event log, and the list is refreshed.
