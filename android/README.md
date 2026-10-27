# cedar for Android

A native Kotlin and Jetpack Compose client for [cedar-chat](../chat/README.md), the Android
counterpart of the [iOS app](../iOS/README.md). Like that app it is a thin client: it talks to
cedar-chat's `/api`, the same routes the web page uses, so your conversations, their unread state,
stars, scheduled agents and notifications are the same in the app, the iOS app and the browser.
cedar's own API key never leaves the server.

```
Android phone ──/api (Bearer CHAT_ACCESS_TOKEN)──▶ cedar-chat ──/v1──▶ cedar ──▶ model server + sandbox
      ▲                                                │
      ├──── FCM (Google Play services) ◀───────────────┤ push when a response ends
      └──── ntfy (UnifiedPush) ◀───────────────────────┘
```

## Features

The same as the iOS app:

- The conversation list as cedar-chat keeps it, with a dot and a bold title on an unread
  response and a **Needs answer** chip when the agent asked a question; pull to refresh, and
  star, rename, open on the web or delete from a long press
- A **Chats | Starred | Scheduled** switch at the top. Starred lists chats and scheduled runs,
  most recently starred first (stars are kept by cedar-chat, so they are the same everywhere).
  Scheduled lists each scheduled agent with its timing, next run and last result; tap one to see
  its runs and open any of them; **Run now**, **Pause**, **Resume** and **Show prompt** from a long
  press. Creating and editing schedules is done on the web
- Replies stream in as the agent works: the model's reasoning (folded), text, tool calls with
  their input, output, duration and sources, token usage and cost; a stop button cancels
- A Plans setting for your messages: the server's, when Cedar thinks it helps, for longer
  requests, always, or never; a plan Cedar chose to make partway through (`propose_plan`) is
  marked as such
- Clarifying questions (`ask_user`) as a form; plans to agree to and tool calls to allow or deny
- The agent's images under the reply, with the Android share sheet
- Cards drawn natively: charts (legend that hides series, touch to read values, the data as a
  table), maps on **OpenStreetMap** (numbered pins, route lines, full screen when tapped, OSM's
  attribution), recipes to tick off, routes with each leg, stock comparisons with ratings, returns
  and news, and product searches. Anything the app can't draw opens on the web
- Attach PDFs (any model) and photos (models registered as multimodal) from the paperclip. Photos
  in a type cedar doesn't take, such as HEIC, are sent as JPEG; the limits are the server's (10
  files, 20 MB each, 32 MB in all)
- Markdown: headings, lists, quotes, tables, code blocks with a copy button
- Live notices while the app is in front, and push notifications when a response ends while you
  are away, through Google's FCM or through ntfy (UnifiedPush) without Google
- Connecting by scanning the QR code the chat's **Settings → Phone** page, the Cedar desktop app
  or `scripts/cedar-stack.sh --pair` shows, or by opening a `cedar://connect` link
- Phone layout, and list and chat side by side on tablets, foldables and in landscape; light and
  dark theme
- English, German and French (see [Languages](#languages))

## Requirements

- Android 8.0 (API 26) or newer
- A cedar-chat server the phone can reach over **HTTPS** (for example through `tailscale serve`),
  and its `CHAT_ACCESS_TOKEN` if one is set. The app refuses plain `http://` addresses, as the iOS
  app does: an access token should not travel in the clear

## Installing

1. On the phone, open the latest [GitHub Release](https://github.com/hanneshapke/cedar/releases)
   and download `Cedar-<version>-android.apk`.
2. Open it. Android asks to allow installs from your browser (or file manager) the first time:
   allow it, then install. Later releases install over it and keep your settings; they are signed
   with the same key.
3. Serve the chat over HTTPS. The simplest way is Tailscale: the [desktop app](../desktop/README.md)
   does it with one checkbox, `scripts/cedar-stack.sh` runs `tailscale serve` for you, and
   [DEPLOYMENT.md](../DEPLOYMENT.md#over-https-with-tailscale-the-ios-and-android-apps) shows the
   steps for a systemd install. Install Tailscale on the phone too and log into the same tailnet.
4. Open Cedar, tap **Scan QR Code** in Settings and scan the code under **Settings → Phone** in
   the chat (or the desktop app's, or the one `cedar-stack.sh --pair` prints). It fills in the
   server and the token and tests the connection. Or type them in and tap **Test Connection**.

The access token is stored encrypted with a key in the Android Keystore; everything else in the
app's preferences.

## Languages

The app speaks English, German (informal *du*; Swiss German writes *ss* for *ß*) and French (formal
*vous*), following [the glossary](../docs/i18n-glossary.md). The language is the first thing a new
install asks, with the phone's preselected when the app speaks it; **Settings → Language** changes it
at once, or sets it to **Automatic** to follow the phone. On Android 13 and newer it is also the
app's language in the system settings (`res/xml/locales_config.xml`).

Each message carries the locale (`"locale": "de-CH"`; for Automatic the phone's when it is English,
German or French, else `en`), so the agent answers in it, and the app tells the server
(`PUT /api/settings/locale`, once connected) so that scheduled runs and notifications follow it too.
Dates, times, numbers and prices are written the locale's way.

The text is in `app/src/main/res/values*/strings.xml`: `values` is English and the source,
`values-de` and `values-fr` the translations, and `values-de-rCH` holds only the German strings with
an *ß*, written with *ss*. A new string goes into all three; product names, the server's messages
and the model's text are not translated. The locale logic (the offered locales, the effective one,
the request body) is in `core` (`Locales.kt`) and tested there. The app is an `AppCompatActivity` so
that `AppCompatDelegate.setApplicationLocales` can switch it; before Android 13 AppCompat stores the
choice itself (`AppLocalesMetadataHolderService` in the manifest).

## Push notifications

cedar-chat decides what is worth telling you about (Settings → Notifications in the web UI:
failures, questions and scheduled runs, or every response that ends while nobody is watching).
Choose **Android push** as the method there. The push itself carries only which conversation
finished; the app then fetches the title and text from your server and shows the notification, so
neither Google nor an ntfy server sees what was said. A tap opens the conversation; nothing is shown
for the conversation that is open.

The app picks how pushes reach it, and you can switch in its Settings:

### ntfy (UnifiedPush), without Google

For phones without Google Play services (GrapheneOS, LineageOS, …) or if you'd rather not route
pushes through Google. Nothing to set up on the server.

1. Install the [ntfy app](https://ntfy.sh/docs/subscribe/phone/) (F-Droid or Google Play). It
   uses ntfy.sh unless you point it at your own ntfy server in its settings.
2. In Cedar's Settings choose **ntfy / UnifiedPush** and tap **Allow Notifications**. The app
   registers with ntfy, gets an endpoint (an unguessable topic URL) and sends it to cedar-chat.
3. **Send a Test** checks the whole way.

cedar-chat only pushes to `https://` endpoints and forgets a phone whose endpoint answers 404 or
410. An ntfy.sh topic is protected only by its unguessable name, which is why the payload holds
nothing but ids.

### Google (FCM)

The most reliable delivery under Android's battery limits, on phones with Google Play services.
FCM needs a Firebase project shared by the app and the server:

1. In the [Firebase console](https://console.firebase.google.com), create a project and add an
   Android app with the package name `dev.cedar.cedar`. Download its `google-services.json`.
2. Build the app with that file in `android/app/` (see [Building it yourself](#building-it-yourself)),
   or, for the published APK, store it as the repository secret `ANDROID_GOOGLE_SERVICES_JSON`.
3. In the Firebase console, *Project settings → Service accounts → Generate new private key*, and
   paste the downloaded JSON into the web UI under Settings → Notifications → **Android push**.
4. In the app, choose **Google (FCM)** and tap **Allow Notifications**.

The published APK's FCM only works with the server key of the project it was built with. If you
run your own server and want FCM, build the APK with your own `google-services.json`; otherwise use
ntfy, which needs nothing.

Switching transport in the app moves the registration: the old device disappears from the web UI's
list of Android phones, where phones can also be removed.

## Building it yourself

You need JDK 17 or newer and the Android SDK (Android Studio brings one; the command-line tools do
too). Then:

```sh
cd android
./build-apk.sh            # a debug APK: android/app/build/outputs/apk/debug/
./build-apk.sh install    # build it and install it on the phone adb sees
./build-apk.sh release    # a release APK, signed with your keystore if one is configured
./build-apk.sh test       # only the core module's unit tests
```

The script checks for the JDK, finds the SDK (`ANDROID_HOME`, `ANDROID_SDK_ROOT`,
`local.properties` or the usual install folders) and runs the core module's tests first. Put a
`google-services.json` in `android/app/` to build with FCM; without it the app uses ntfy only.

To sign a release APK, make a keystore once and keep it (an installed app only updates from an APK
signed with the same key):

```sh
keytool -genkeypair -v -keystore cedar.jks -alias cedar -keyalg RSA -keysize 4096 -validity 10000
export CEDAR_KEYSTORE=$PWD/cedar.jks CEDAR_KEY_ALIAS=cedar
export CEDAR_KEYSTORE_PASSWORD=… CEDAR_KEY_PASSWORD=…
./build-apk.sh release
```

or write `storeFile`, `storePassword`, `keyAlias` and `keyPassword` into `android/keystore.properties`
(ignored by git). The version is the workspace version in the repository's `Cargo.toml` (or
`-Pcedar.version=…`); `versionCode` is `major·1 000 000 + minor·1 000 + patch`.

You can also open `android/` in Android Studio and run the `app` configuration.

## Layout

| Path | Contents |
| --- | --- |
| `core/` | Plain Kotlin, no Android: the `/api` types, the event-to-transcript reducer, the cards' parsers, the Markdown block parser, the attachment rules, the connection code, the locales, the SSE parser and the OkHttp client. Builds and tests without the Android SDK |
| `core/src/test` | Unit tests ported from `iOS/CedarTests`, plus the client against a mock server |
| `app/…/AppModel.kt`, `ChatModel.kt` | App state (settings, the lists, the inbox count, live notices, open chats) and one conversation's turns and streams, as the iOS app's models |
| `app/…/ui/` | Compose screens: root (list and chat side by side when wide), list (chats, starred, scheduled), chat (turns, questions, approvals, images, composer), settings (with the QR scanner), cards |
| `app/…/push/` | `PushManager` (FCM or UnifiedPush, registration with the server), the two services, and `Notifier`, which fetches the text and shows the notification |
| `app/…/storage/` | Settings, with the token encrypted by an Android Keystore key |
| `app/…/Language.kt`, `ui/Text.kt` | The app's language (AppCompat per-app locales) and the app's words for what `core` names by key |
| `app/src/main/res/values*/` | The strings: English (source), German, French, Swiss German (ß → ss) |
| `build-apk.sh` | Build an APK locally |

## How it talks to the server

As the iOS app (see its README): `GET /api/conversations`, `GET /api/conversations/{id}`,
`POST /api/conversations/{id}/messages`, each turn rebuilt from `GET /api/turns/{id}/events` and
followed live (reconnecting from the last event), `GET /api/events` while in front,
`POST /api/conversations/{id}/seen`, stars (`PUT`/`DELETE /api/conversations/{id}/star`,
`GET /api/starred`), schedules (`GET /api/schedules`, `GET /api/schedules/{id}`,
`POST /api/schedules/{id}/run`, `PATCH /api/schedules/{id}`), plans and approvals, files, and the
language (`PUT /api/settings/locale`, and `locale` with each message).
The push token or endpoint goes to `POST /api/notifications/android/devices`.

## Continuous integration

`.github/workflows/android.yml` runs the core tests, Android lint and a debug build for every push
and pull request that touches `android/`, and keeps the debug APK as an artifact for a week
(Actions → Android → the run → Artifacts). The release workflow builds the signed APK and attaches
it to the GitHub Release with its checksum. It needs these repository secrets:

| Secret | What it is |
| --- | --- |
| `ANDROID_KEYSTORE` | The release keystore (`.jks`), base64-encoded (`base64 -w0 cedar.jks`) |
| `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | Its password, the key's alias and the key's password |
| `ANDROID_GOOGLE_SERVICES_JSON` | Optional: the Firebase project's `google-services.json`, for FCM |

## Limitations

- Events arrive per model turn and per tool call, not per token, so text appears a turn at a time.
- Document cards open on the web, as does a card whose result the app can't read.
- Creating and editing scheduled agents and the server's settings are done in the web UI.
- Android cuts network connections in the background. When you return to the app, running
  responses reconnect and catch up from the event log, and the list is refreshed. Push
  notifications cover the time away.
