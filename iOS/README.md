# cedar for iOS

A native SwiftUI client for cedar. It talks to cedar's HTTP API directly, the same way
[cedar-chat](../chat/README.md) does from the server side: start a run, follow its events live,
continue it with follow-up messages, stop it, and delete it.

## Features

- A conversation list, rebuilt from your runs on the server, with pull to refresh, rename, and delete
- Replies stream in as the agent works: text, tool calls (`bash`, `read_file`, `write_file`) with
  their input, output, and duration, errors, and token usage
- Follow-ups continue the previous run (`parent_id`), so the agent keeps its conversation and its
  `/workspace` files
- A stop button that cancels the running response
- Markdown rendering: headings, lists, quotes, tables, and code blocks with a copy button
- iPhone and iPad layouts, light and dark mode
- An optional model and system prompt for new conversations

## Requirements

- Xcode 16 or newer (the project uses folder-synchronized groups)
- iOS 17 or newer
- A cedar server the device can reach, and one of its `CEDAR_API_KEYS`

## Running it

1. Start cedar (see the [top-level README](../README.md)). To reach it from a phone, bind it to an
   address the phone can reach, for example `BIND_ADDR=0.0.0.0:8080`. In the simulator,
   `http://127.0.0.1:8080` works as is.
2. Open `iOS/Cedar.xcodeproj` in Xcode, choose the **Cedar** scheme, and run it. To run on a device,
   pick your team under *Signing & Capabilities*.
3. On first launch, enter the server URL and API key in Settings and tap **Test Connection**.

The API key is stored in the Keychain. Everything else is stored in `UserDefaults`.

cedar speaks plain HTTP, so the app allows cleartext connections (`NSAllowsArbitraryLoads` in
`Config/Info.plist`). If your cedar is only reachable over HTTPS, behind a TLS-terminating proxy,
you can remove that exception. Anyone with the API key can run the agent, so do not expose cedar on
a public network without TLS.

## How conversations map onto cedar

cedar has no conversation object. A conversation is a chain of runs, each continuing the one
before it, so the app rebuilds conversations from `GET /v1/runs` (the newest 500 runs) by following
`parent_id`. This means conversations started on another device, or from cedar-chat's fallback,
show up too. Custom titles are kept on the device, keyed by the conversation's first run.

- The first message starts a run. Each later message continues the latest run.
- Each exchange is rebuilt by replaying the run's event stream (`GET /v1/runs/{id}/events`). A run
  that is still going is followed live, and the stream reconnects from the last event it saw.
- If the previous run is gone from cedar (`404`), the message starts a new run with the earlier
  exchanges inlined in the prompt, like cedar-chat does.
- Deleting a conversation deletes all of its runs on cedar, newest first, which frees the
  workspace. cedar refuses while a run is active, so stop it first.

## Layout

| Path | Contents |
| --- | --- |
| `Cedar/App` | The app entry point, the root split view, and `AppModel` (settings, conversation list, open chats) |
| `Cedar/Models` | API types, the run-to-conversation grouping, the event-to-transcript reducer, and a Markdown block parser |
| `Cedar/Networking` | `CedarClient` for `/v1`, and the Server-Sent Events parser |
| `Cedar/Storage` | Settings, Keychain access, and custom titles |
| `Cedar/Features` | SwiftUI screens: conversation list, chat, and settings |
| `CedarTests` | Unit tests for the parser, models, and Markdown |
| `Config/Info.plist` | App Transport Security settings, merged with the generated Info.plist |

## Tests

Run them in Xcode with ⌘U, or from the command line:

```sh
cd iOS
xcodebuild test -project Cedar.xcodeproj -scheme Cedar \
  -destination 'platform=iOS Simulator,name=iPhone 16'
```

## Limitations

- cedar has no API for listing models, so the model is a free-text setting.
- Only the newest 500 runs are loaded. Older conversations, and older parts of long chains, are
  not shown.
- Events arrive per model turn and per tool call, not per token, so text appears a turn at a time.
- iOS suspends network connections in the background. When you return to the app, running
  responses reconnect and catch up from the event log.
