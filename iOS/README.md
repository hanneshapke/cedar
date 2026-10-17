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
- Maps, recipes, routes and stock comparisons are drawn by the web UI, so those show as a
  line with an **Open on the web** link
- Follow-ups continue the conversation; a stop button cancels the running response
- Live notices: a response that ends in another conversation marks it unread at once
- iPhone push notifications when a response ends while you are away (see below)
- A model and reasoning level picked from what cedar has registered
- Markdown rendering: headings, lists, quotes, tables, and code blocks with a copy button
- iPhone and iPad layouts, light and dark mode

## Requirements

- Xcode 16 or newer (the project uses folder-synchronized groups)
- iOS 17 or newer
- A cedar-chat server the device can reach (its `CHAT_BIND_ADDR` must not be loopback for a
  phone), and its `CHAT_ACCESS_TOKEN` if one is set

## Running it

1. Start cedar and cedar-chat (see the [top-level README](../README.md) and
   [chat/README.md](../chat/README.md)). To reach the chat server from a phone, bind it to an
   address the phone can reach, for example `CHAT_BIND_ADDR=0.0.0.0:3000`, and set
   `CHAT_ACCESS_TOKEN`. In the simulator, `http://127.0.0.1:3000` works as is.
2. Open `iOS/Cedar.xcodeproj` in Xcode, choose the **Cedar** scheme, and run it. To run on a
   device, pick your team under *Signing & Capabilities*.
3. On first launch, enter the server URL and the access token in Settings and tap
   **Test Connection**. The models cedar has registered appear in the picker once it answers.

The access token is stored in the Keychain. Everything else is stored in `UserDefaults`.

cedar-chat speaks plain HTTP unless you put it behind TLS, so the app allows cleartext
connections (`NSAllowsArbitraryLoads` in `Config/Info.plist`). If your server is only reachable
over HTTPS, you can remove that exception. Anyone with the access token can run the agent, so do
not expose cedar-chat on a public network without TLS.

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
| `Cedar/Storage` | Settings and Keychain access |
| `Cedar/Features` | SwiftUI screens: conversation list, chat (turns, questions, approvals, images), and settings |
| `CedarTests` | Unit tests for the API types, the reducer, the SSE parser, and Markdown |
| `Config/Info.plist` | App Transport Security settings, merged with the generated Info.plist |
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

## Limitations

- Events arrive per model turn and per tool call, not per token, so text appears a turn at a
  time.
- Maps, recipes, routes, stock comparisons and document cards open on the web rather than
  being drawn in the app.
- Attachments (images and PDFs with a message), scheduled agents and the server's settings are
  managed in the web UI.
- iOS suspends network connections in the background. When you return to the app, running
  responses reconnect and catch up from the event log, and the list is refreshed.
