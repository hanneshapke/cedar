# cedar-chat

A web chat for a cedar server, with two selectable looks. It is a separate Rust service
(axum + SQLite) that serves a single-page UI and sits between the browser and cedar:

```
browser  ──/api──▶  cedar-chat  ──/v1 (Bearer CEDAR_API_KEY)──▶  cedar  ──▶  vLLM + sandbox
```

The cedar API key stays on the server. The browser only talks to cedar-chat.

## Two interfaces

The settings button (sliders icon, top right) switches between:

| | Style | Layout |
| --- | --- | --- |
| `/v1/` | **Claude style** | Conversations in a sidebar, one thread with tool calls inline |
| `/v2/` | **Cedar style** | Say on the left, show on the right: the conversation (prompts, Cedar's prose, its questions, and a step row per tool call) in a narrow column, and everything a tool produced as full-width cards on a "stage" beside it, grouped by turn |

Your choice is saved in this browser (`localStorage`, key `cedar-chat-ui`) and decides where `/`
goes. Without a saved choice, `/` opens Cedar style. Switching keeps the open conversation and
your access token. Opening `/v1/` or `/v2/` directly always shows that interface and does not
change the saved choice.

## Features

Both interfaces have all of these, except where noted:

- A conversation list (a sidebar in Claude style, a drawer behind the menu button in Cedar style)
  with new chat, rename (click the title), and delete
- Replies stream in as the agent works. Text, tool calls (`bash`, `run_python`, the file,
  document, chart, web, and weather tools, and `web_search` when cedar has a Brave key) with their input and
  output, errors, and token usage are shown as they happen
- A stop button cancels the running response
- A files panel (the folder button in the header) lists what the agent saved under `/workspace`
  and downloads any of it; finished `write_file`, `generate_pdf`, `generate_chart`, and
  `download_file` calls get a download button too
- A `generate_chart` call is drawn as a chart card from the data the agent gave it, rather than as
  the picture it saved: the title, a legend whose entries hide and show their series, the plot drawn
  to the card's width in a fixed palette with light and dark steps (lines, areas, grouped or stacked
  bars, horizontal bars, scatter points, a histogram's bins, or a donut with its slices listed), a
  tooltip that reads every series where you point (a crosshair over lines, the category over bars,
  the nearest point of a scatter, a slice of a pie; arrow keys move it too), and the data as a table
  folded under it. Very long series are thinned on the card; the saved file has every point. Cedar
  style puts it on the stage with the file's View and Download buttons, Claude style under the tool
  call (`chart-view.js` and `chart-view.css`). If the data can't be drawn, the picture is shown.
  Any other tool can show charts the same way by printing `[chart] {spec}` lines (the spec is
  `generate_chart`'s input without a path): they are drawn under the call, and in Cedar style on
  its card with the rest of its output folded under them (see
  [Personal tools in Python](../README.md#personal-tools-in-python)); `tandem_glucose` does this
- The images the agent produces are shown in the response, once each, after its reply: a chart
  from `generate_chart` that wasn't drawn as a card, a picture `write_file` or `download_file` saved, or one a `bash` command
  or `run_python` script created (from the files the call lists). A file that was rewritten
  several times is shown as it is when the response ends. Claude style puts them under the reply,
  Cedar style in an images card on the stage after the response's other cards. An image the agent refers to in its text as
  `![caption](chart.png)` is shown in place of the reference, and not again below. PNG, JPEG, GIF,
  WebP, BMP, AVIF, and SVG files up to 16 MB are shown; at most 12 per response, loaded as they
  scroll into view. Each has View and Download buttons under it, and a click on the picture opens
  it full size
- In Cedar style, a Markdown or text file the agent writes, or a PDF it makes from Markdown, is shown
  rendered as a document card on the stage, with Copy and Download, and the stage rests on it when the
  response ends. Searches (each result as a link with its snippet), commands and scripts with their
  output, and the pages and files the agent read are cards folded to one line that open on a click;
  a failed call opens on its own. Each tool call is also a step row in the conversation, and clicking
  it jumps to the card. The split between the two columns can be dragged (double-click resets it); on a
  narrow screen they become **Conversation** and **Results** tabs, with a count of results that landed
  while the conversation was showing
- A View button opens a workspace Markdown file (`.md`, `.markdown`) as rendered Markdown, a PDF in
  the browser's own PDF viewer, and an image at full size, in a dialog. It is in the files panel, on
  finished file-tool calls, and on the page of a generated PDF. Markdown files over 2 MB and images
  over 16 MB are not previewed. A file named `.pdf` that is not a PDF, or `.png` that is not a PNG,
  is refused. Workspace images inside a Markdown file (`![caption](chart.png)`, relative to
  `/workspace` as `generate_pdf` resolves them) are shown; images from the web are not fetched
- A `show_map` tool call (when cedar offers the tool) is drawn as a map: a numbered pin for each
  place on an OpenStreetMap map you can pan and zoom (click it first, so the wheel keeps scrolling
  the conversation), a list of the places with their coordinates under it, and a note for any names
  cedar could not find. Cedar style puts it on the stage as a card, Claude style under the tool
  call. The list is also what you see if the tiles can't load. If the result isn't a usable map, only
  the tool call's raw output is shown
- A `show_recipe` tool call (when cedar offers the tool) is drawn as a recipe card: the photo of the
  dish cedar found (loaded from Wikimedia, see [Recipe photos and privacy](#recipe-photos-and-privacy); credited with a link to its page; the title alone if there
  is none or it fails to load), the times and servings, the ingredients as a checklist, and the steps
  as a timeline. The steps appear one after another the first time the card is on screen. Tick a
  step's number to mark it done, or use the button under the timeline to finish the current step
  and move on: the line fills in and the next step is highlighted. Cedar style puts it on the stage
  as a card, Claude style under the tool call
- A `plan_trip` tool call is drawn as a trip card: the recommended way to go and why, a map of the
  chosen option's route between an A and a B pin (each transit line in its own color, walks dotted),
  a row per option (transit with its line badges, times and transfers; drive; rideshare with its
  estimated fare; walk), and the chosen option's steps on a timeline. Click a row to see that option
  on the map and in the steps. Times are shown in the trip's time zone when the planner gives one.
  It is placed like a map (`transit-view.js` and `transit-view.css`, which draw with the map view)
- A `compare_stocks` tool call is drawn as a stock card: a quote tile per symbol (price, the day's
  move, and its buy/hold/sell call), a line chart of the period (1M to 5Y) as % change against the
  benchmark index (dashed) or as one symbol's price, with a crosshair that lists every line's value
  (arrow keys move it too), then the selected symbol's rating: the call on a sell–hold–buy gauge with
  its confidence and summary, every factor with its score bar and the reasoning behind it, what would
  change the view, and key numbers (52-week and day ranges, moving averages, RSI, volatility,
  drawdown, beta). Pick a tile to see another symbol. Below are the returns of every symbol side by
  side and the news, which can be filtered by symbol; headlines open in a new tab. Cedar style puts
  it on the stage as a card, Claude style under the tool call (`stock-view.js` and
  `stock-view.css`; see [Stock news and privacy](#stock-news-and-privacy))
- A `search_products` tool call (when cedar offers the tool) is drawn as a product card: the
  model's recommendation in a few sentences, then a tile per option with its photo, its badges
  (top pick, best value, premium or budget pick, best rated), the price with the regular price and
  the savings, its stars with the number of reviews, and the verdict on them. A price that comes
  from review text rather than a store page is marked "≈", and one no page stated reads "Price at
  the store". Pick a tile to see the product below: a photo gallery (click a thumbnail, or use the
  arrow keys), the price, its stock when the store said, a View at button that opens the shop's
  page in a new tab, why it fits and who it suits, its pros and cons, the reading of its reviews
  (the stars, the rating adjusted for how many reviews there are on a bar, the verdict and how much
  to trust it, and a sentence), the description, key features, specs, and the numbered sources
  behind it, then a strip of comparable options with how much more or less each costs. Click one to
  see it in turn, with a button back to the option it was compared to. Cedar style puts it on the
  stage as a card, Claude style under the tool call (`product-view.js` and `product-view.css`; see
  [Product photos and privacy](#product-photos-and-privacy))
- An `ask_user` tool call, when the agent needs a clarification before it goes on, is drawn as a
  form: each question with its options as radio buttons (pick one) or checkboxes (pick any), and
  always a field for an answer in your own words (for an open question, that field alone). Send
  answer sends what you picked and wrote as your next message, with each question repeated so the
  agent knows which answer is which; you can also just reply in the message box. Once a later
  message is in, the form is closed. Claude style puts it under the tool call, Cedar style in the
  conversation column (`clarify-view.js` and `clarify-view.css`). A [scheduled agent](#scheduled-agents)'s run is
  told nobody is there to answer, so it assumes instead of asking
- Markdown rendering: headings, lists, tables, quotes, links, and code blocks with a copy button
- Light and dark themes (follows the system), plus a mobile layout
- The send button names the model and reasoning level the message goes with, and opens a menu
  of the registered models with the levels each declares; an optional access token
- [Tools and settings](#tools-and-settings): the tools, model, turn limit and system prompt every
  conversation gets, a Tools button to change the tools of one conversation, and (for admins)
  [the models](#models) cedar can use, its Brave Search and Best Buy keys and its Google account
- [Attachments](#attachments): PDFs go with a message to any model, and images to a model registered as
  accepting them, by the paperclip button, by dropping them on the message box, or by pasting an image
- Starred conversations: the star button in the header stars the open conversation, a chat or a
  scheduled run alike, and a **Starred** list at the top of the sidebar (the drawer in the other
  styles) lists them, most recently starred first, with a clock on scheduled runs. Stars are kept
  by this server, so the same list shows in every browser, the
  [Cedar app for macOS and Windows](../desktop/README.md) and the Cedar iOS and Android apps; a page picks up
  stars set elsewhere when it comes back into view, and every minute while it is shown
  (`star.js` and `star.css`)
- [Scheduled agents](#scheduled-agents): a finished conversation can be turned into one prompt that
  runs on its own every day, on weekdays, or weekly at a local time, listed in the sidebar with
  every run's result
- [Notifications](#notifications): a response that finishes while you are in another conversation,
  another tab, or away is marked unread in the conversation list and counted in the page title and
  on the bell button, whose inbox lists it (scheduled runs too); a question the agent asked is
  marked "Needs answer". Out of the app, by the method you pick in Settings: a desktop
  notification from the [Cedar app for macOS and Windows](../desktop/README.md), a browser push (tab open or not), a push to the
  Cedar iOS or Android app, a phone push through Pushover, or an email

## Running it

```sh
# 1. Start cedar (see the top-level README), e.g.
CEDAR_API_KEYS=devkey cargo run -p cedar

# 2. Start the chat server and open http://127.0.0.1:3000
CEDAR_API_KEY=devkey cargo run -p cedar-chat
```

The UI is plain HTML, CSS, and JavaScript in `static/` (`v1/` and `v2/` for the two interfaces,
`ui-pref.js` for the saved choice, `file-viewer.js` for the file preview, `image-view.js` and
`image-view.css` for the images the agent produces, `map-view.js` and
`map-view.css` for the maps, `transit-view.js` and `transit-view.css` for trips, `stock-view.js` and
`stock-view.css` for stocks, `product-view.js` and `product-view.css` for products, `clarify-view.js` and `clarify-view.css` for clarifying questions,
`schedule.js` and `schedule.css` for scheduled agents, `star.js` and `star.css` for starred
conversations, `notify.js` and `notify.css` for the inbox
of finished responses and the Notifications settings, `sw.js` for the service worker behind browser
push, and
`models.js` and `models.css` for the send button's model menu, `settings-shell.js` for the Settings
dialog's frame (its left nav, one page at a time, and the Save footer), `settings.js` and `settings.css` for the
Tools, Model & prompt, Advanced settings and Server pages, which every interface shares, `vendor/leaflet/` for the map
library, and
`index.html`, which sends `/` to the chosen one). It is
compiled into the binary, so there is no Node build step. Restart the server after you edit those
files. The logo and icons are not in `static/`: they come from `../docs/assets/` (shared with the
root README and the landing page) and are served under `/assets/`.

### Images

Every image the page shows comes through this server's `/api/conversations/{id}/files/{path}`
with the access token, like a download, so nothing is fetched from elsewhere: a `![caption](url)`
that points at the web stays a link. The bytes are shown from a `blob:` URL whose type is taken
from the file's extension, never from the content, so a file named `.png` is decoded as a PNG or
not at all, and an SVG is drawn by an `<img>`, which runs none of its scripts and loads nothing it
references. "Open in new tab" in the viewer is offered for PDFs and raster images only, since a
tab of its own would run an SVG's scripts with the page's origin.

### Maps and privacy

[Leaflet](https://leafletjs.com) 1.9.4 (BSD 2-Clause, license in `static/vendor/leaflet/LICENSE`) is
served from this server and loaded the first time a map is shown, so no third-party code runs in
the page, which holds your access token. The map tiles are the one thing that comes from elsewhere:
the browser fetches them from `tile.openstreetmap.org`, which sees the viewer's IP address and the
area shown. OpenStreetMap's [tile usage policy](https://operations.osmfoundation.org/policies/tiles/)
allows light use like this and asks for the attribution the map displays; a busy deployment should
use its own tile server (the URL is `TILES` in `static/map-view.js`). The chat server itself makes
no requests for maps; cedar looks the places up (see the top-level README).

### Recipe photos and privacy

The photo on a recipe card is the other thing that comes from elsewhere: the browser loads it from
the `https` address cedar found, normally `upload.wikimedia.org`, without a referrer, so Wikimedia
sees the viewer's IP address and which photo was shown, but not this page. The photo is an `<img>`
and the rest of the card is set as text, so nothing in the tool's result runs in the page. The chat
server itself makes no requests for recipes; cedar searches Wikipedia and Commons (see the top-level
README). Set `RECIPES=off` on cedar to remove the tool.

### Stock news and privacy

The stock chart is drawn in the page from the prices in the tool's result; nothing is loaded for it.
Headline thumbnails are the one thing on a stock card that comes from elsewhere: the browser loads
them from the `https` addresses Yahoo Finance gave, without a referrer, and leaves them out if they
fail. Headlines are links that open in a new tab without a referrer. Everything else is set as text,
so nothing in the tool's result runs in the page. The chat server makes no requests for stocks;
cedar asks Yahoo Finance (see the top-level README). Set `STOCKS=off` on cedar to remove the tool.

### Product photos and privacy

Product photos are the one thing on a product card that comes from elsewhere: the browser loads
them from the `https` addresses cedar found on the shops' pages (or Brave's thumbnail server),
without a referrer, so each shop sees the viewer's IP address and which photos were shown, but
not this page; a photo that fails to load leaves a placeholder. The View at button and the sources
are links that open in a new tab without a referrer. Everything else is set as text, so nothing in
the tool's result runs in the page. The chat server makes no requests for products; cedar searches
and reads the pages (see the top-level README). Remove the Brave and Best Buy keys (Settings →
Server) or set `PRODUCTS=off` on cedar to remove the tool.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `CEDAR_API_KEY` | required | One of cedar's `CEDAR_API_KEYS` |
| `CEDAR_URL` | `http://127.0.0.1:8080` | cedar's base URL |
| `CHAT_BIND_ADDR` | `127.0.0.1:3000` | Where the chat server listens |
| `CHAT_DATABASE_URL` | `sqlite://cedar-chat.db` | Conversation storage |
| `CHAT_ACCESS_TOKEN` | unset | If set, the page asks for this token and the API requires it |
| `CHAT_ADMIN_TOKEN` | unset | If set, Settings offers [the models](#models) and the [integrations](#integrations) (cedar's API keys and Google account) to whoever enters it. Must differ from `CHAT_ACCESS_TOKEN` |
| `CHAT_PUBLIC_URL` | `http://<CHAT_BIND_ADDR>` | This server's address as the browser reaches it; "Connect Google" sends Google `<CHAT_PUBLIC_URL>/api/oauth/google/callback` |
| `CHAT_HISTORY_MAX_BYTES` | `40000` | How much earlier conversation is inlined if the previous run is gone from cedar, or compacted for a schedule in that case (at most 60000) |
| `CHAT_PUSHOVER_URL` | `https://api.pushover.net/1/messages.json` | Where Pushover messages go (for tests) |
| `CHAT_APNS_URL` | unset | Where iPhone pushes go instead of Apple's production or sandbox APNs (for tests) |
| `CHAT_FCM_URL` | `https://fcm.googleapis.com` | Where Android pushes through Firebase go (`…/v1/projects/{project}/messages:send`; for tests) |
| `CHAT_GOOGLE_TOKEN_URL` | `https://oauth2.googleapis.com/token` | Where the Firebase service account's signed assertion is traded for an access token (for tests) |
| `CHAT_TAILSCALE_CLI` | found | The `tailscale` CLI that Settings → Phone asks about Tailscale, when it isn't in the usual places or on `PATH` |
| `CHAT_UPDATE_CHECK` | `on` | Look up the latest GitHub Release a minute after the start and every six hours, and show a banner when it is newer (see [Updates](#updates)). `off` makes no requests to GitHub. The desktop app sets it `off`, it updates itself |
| `CHAT_UPDATE_DIR` | unset | The directory the root `cedar-update` unit watches (`/var/lib/cedar/update` in [DEPLOYMENT.md](../DEPLOYMENT.md#updates)). When set, an admin's *Update* in the banner installs the release; otherwise the banner shows the commands to run |
| `CHAT_UPDATE_URL` | `https://api.github.com/repos/hanneshapke/cedar/releases/latest` | Where the latest release is looked up (for tests) |

Without `CHAT_ACCESS_TOKEN`, anyone who can reach the chat server can run the agent with your
cedar key. Keep the default loopback bind, or set a token before listening on a public address.

## Updates

When a release newer than this cedar-chat is out, every UI shows a banner at the top with its
release notes, **Update** and **Dismiss** (dismissed for that version, in that browser). The
check is a `GET` of GitHub's latest-release API, with the ETag of the last answer so an unchanged
release costs nothing; it sends nothing about this server but its version in the User-Agent.

What *Update* does depends on where the chat runs:

- **Linux under systemd, with the `cedar-update` unit** ([DEPLOYMENT.md](../DEPLOYMENT.md#updates)):
  it asks for the admin token if the tab doesn't have it, writes `{"version": …}` to
  `$CHAT_UPDATE_DIR/request`, and the root unit downloads the release's tarball for this CPU and
  `checksums.txt` from GitHub, checks the tarball against it, installs both binaries, restarts
  both services, and puts the old binaries back if cedar doesn't come up. The banner follows its
  progress (`status.json` in the same directory) and reloads the page once the new version
  answers. Restarting ends a response that is still running, and the banner says so before it
  starts.
- **Linux without it**, or built from source: *How to update* shows the commands to run.
- **The Mac and Windows app**: the app updates itself (see
  [desktop/README.md](../desktop/README.md#updates)) and drives this banner.

## How conversations map onto cedar

The first message in a conversation starts a new cedar run. Each later message starts a run that
**continues** the previous one (`parent_id` in cedar's API), so the agent keeps:

- the whole conversation, including its earlier tool calls and their results, and
- its sandbox workspace: files written under `/workspace` are still there, as are packages the
  agent `pip install`ed (they go to `/workspace/.local`). Running processes are not, since each
  run gets a new container.

cedar removes a workspace once it has been idle for `WORKSPACE_TTL_SECS` (24 hours by default).
After that, a follow-up still has the conversation but starts with an empty workspace, and the
model is told so.

If cedar no longer has the previous run (it was deleted, or cedar's database was reset), cedar-chat
falls back to starting a new run with the earlier exchanges inlined in the prompt, up to
`CHAT_HISTORY_MAX_BYTES`. [Attachments](#attachments) live in cedar's run, so earlier ones are gone
with it; only the new message's files go along.

Only one response per conversation runs at a time. The UI replays each turn from cedar's event log,
so reloading the page or opening a conversation shows the full tool trace. Deleting a conversation
also deletes its runs on cedar (best effort), which frees the workspace.

## Notifications

A response that ends while nobody is looking at its conversation is not lost in the list. The
server follows cedar's stream of the key's finished runs (`GET /v1/events`, see [Being told when
a run ends](../README.md#being-told-when-a-run-ends)) in the background, from the id of the last
notification it handled, which it keeps in its database: a restart of either service loses
nothing. When a run ends it brings the turn up to date, marks it unseen, and tells every open tab
over `GET /api/events`.

What the page does with that:

- The conversation list marks a conversation with an unseen finished (or failed) response with a
  dot and a bold title, and one whose last response was a question (`ask_user`) with a **Needs
  answer** chip, until the answer is sent.
- The bell button in the header counts the unseen responses, scheduled runs included, and the
  page title carries the count (`(2) Cedar`). Clicking the bell lists them, newest first, with
  what happened (finished, failed, needs your answer) and when; a row opens the conversation,
  and **Mark all read** clears the list.
- A response that finishes in another conversation while the tab is visible is announced in a
  toast.
- Looking at a conversation marks its responses seen: opening it, watching its response end with
  the tab visible, or coming back to the tab. A response that ends while the conversation is open
  but the tab is hidden stays unread until then. The page tells the server with
  `POST /api/conversations/{id}/seen`.

A run the user stopped is not announced. Responses from before this feature count as seen.

### Out of the app

Settings → **Notifications** sets up the ways to be told when you are not looking at the page at
all, as one list of methods: pick one as the default and its setup unfolds (a **Set up** link opens
another's without picking it); each has its credentials, a **Set up** badge once it can send, and
**Send a test**, which saves first. The server sends a message a few seconds after cedar says the run ended,
unless the response was looked at in the meantime (a page that watched it end marks it seen first).

- **Desktop notification**: a notification in Notification Center (a toast on Windows) from the
  [Cedar desktop app](../desktop/README.md), its window open or not; clicking it is meant to open the conversation. The app follows
  `GET /api/notifications/desktop/events`, and its first connection sets the method up (and makes
  it the default if none was chosen). It is offered once the app has connected; a send while the app
  isn't running fails and says so. The app's web view can't take browser push, so the app hides
  that method.
- **Browser push**: a notification from this browser even when the tab is closed, through a
  service worker (`sw.js`) and the browser's push service, signed with VAPID keys the server makes
  on first use. **Turn on in this browser** asks the browser for permission and subscribes it; every
  subscribed browser is listed and can be removed, and a browser the push service reports gone is
  dropped. Browsers only allow this on `https` or on `localhost`, so it works over the SSH tunnel
  of the [deployment guide](../DEPLOYMENT.md) and behind a TLS proxy. A push that arrives while a
  Cedar tab is in front is left to that tab. An optional contact address goes with each push, as
  some push services ask for one.
- **iPhone push**: a notification from the [Cedar iOS app](../iOS/README.md), through Apple's
  push service (APNs). Needs an APNs key from your Apple developer account (Certificates,
  Identifiers & Profiles → Keys: make a key with *Apple Push Notifications service* on, download
  its `.p8` file, note the key id), your team id (under Membership), the app's bundle id
  (`dev.cedar.Cedar` unless you changed it in Xcode), and whether the app was installed from Xcode
  (**Sandbox**) or the App Store or TestFlight (**Production**). Every iPhone that opens the app
  against this server and allows notifications registers itself and is listed, and can be removed;
  one Apple reports gone is dropped. A tap on the notification opens the conversation in the app.
  `CHAT_APNS_URL` points the pushes somewhere else (a mock in tests).
- **Android push**: a notification from the Cedar Android app, through Firebase Cloud Messaging
  (FCM), or through [UnifiedPush](https://unifiedpush.org) on a phone without Google Play
  services (the [ntfy](https://ntfy.sh) app as the distributor). Every phone that opens the app
  against this server and allows notifications registers itself with its FCM token or its
  UnifiedPush endpoint (https only) and is listed, and can be removed; one the service reports
  gone (FCM: 404, `UNREGISTERED`, or a 400 that blames the token; UnifiedPush: 404 or 410) is
  dropped. UnifiedPush needs nothing set up here. FCM needs the key of a service account of the
  Firebase project the app was built with (Firebase console → Project settings → Service
  accounts → Generate new private key): paste the JSON file; the server signs a token request
  with it (RS256) and trades it at Google for an access token, kept for 50 minutes. The push
  carries data only, `{"kind": "turn"|"test", "conversation_id": "<id, empty for a test>",
  "tag": "<the turn's id>"}`, never the conversation's text: the app fetches what to show from
  this API, so nothing is sent unencrypted to a UnifiedPush server or through Google.
  `CHAT_FCM_URL` and `CHAT_GOOGLE_TOKEN_URL` point it somewhere else (a mock in tests).
- **Phone push via Pushover**: a push to the [Pushover](https://pushover.net) app on your phone.
  Needs an application token (make an application at pushover.net/apps) and your user key.
- **Email**: a mail through an SMTP server: host, port (left empty, the connection's usual one),
  TLS, STARTTLS or plain, a user name and password if the server wants them, and the sender and
  recipient addresses. A Gmail account takes an app password with STARTTLS on `smtp.gmail.com`.

Which method is used, and for what:

- **Default method** (only in the app, or one of the set-up methods) covers responses in
  conversations, and **Send it for** chooses between what needs you (a failure, a question the
  agent asked, a scheduled run) and every response that ends while you are not watching it.
- Each [scheduled agent](#scheduled-agents) has a **Notify me by** picker in its dialog: the
  default, only in the app, or any set-up method, for that agent alone. An agent with a method of
  its own is announced on every finished or failed run, whatever **Send it for** says. The choice is
  shown with the agent in the list.

The last delivery, test or real, is shown with what the service answered. The credentials stay in
this server's database (`CHAT_DATABASE_URL`), as they are, and are never sent back to the page;
keep that file as private as the cedar key.

## Scheduled agents

Once a conversation has a finished response, the **Schedule** button (the clock in Claude style,
"Schedule" in Cedar style) turns it into an agent that runs on its own:

1. cedar reviews the whole conversation, tool calls included, and writes one self-contained prompt
   for it (`POST /v1/compact` in cedar's API). If cedar no longer has the run, the exchanges this
   service remembers are compacted instead, up to `CHAT_HISTORY_MAX_BYTES`.
2. You edit the prompt until it stands on its own, name the schedule, and pick a frequency (every
   day, weekdays, or every week on a weekday), a time of day, the model and its reasoning level.
   Times are local: the
   browser's time zone is sent along (an IANA name like `Europe/Berlin`), so daylight-saving
   changes are handled, and the next run is confirmed in your local time.
3. From then on the chat server starts the prompt as a **fresh** cedar run each time it is due: a
   new conversation, a new workspace, no memory of the earlier runs, and no files (a schedule's
   prompt is text only). It checks every 30 seconds, so a run starts within half a minute of its time.

Every run lives in a conversation of its own, named after the schedule and the time it was due,
with the full tool trace and files panel. These conversations stay out of the conversation list;
**Scheduled agents** in the conversations sidebar lists each schedule with its next run; clicking
one opens its detail with every run's status and result, an **Open** button that shows a run as a
conversation, and the prompt that was used. There you can also **Pause** and **Resume** a schedule,
**Run now**, **Edit** the prompt and timing, and **Delete** it. Deleting a schedule deletes its runs' conversations and
their cedar runs too.

If the chat server was down when a run was due, the run starts once at the next start, and the
schedule then moves on to its next slot (missed slots do not pile up). If cedar refuses the run
because it is busy (its per-key active-run cap) or can't be reached, the slot is retried every tick
for an hour, after which it is recorded as a failed run and skipped. Resuming a paused schedule
skips the slots missed while it was paused. At most 100 schedules can exist.

## Tools and settings

**Settings → Tools** sets which of cedar's tool groups every new conversation gets (web search,
reading web pages, weather, maps, the browser, Gmail and Calendar, the last two down to single
tools), and **Settings → Model & prompt** the default model (one of those cedar has
[registered](#models); unset means cedar's own default), turn limit, and system prompt. The
system prompt reads as a document showing cedar's own default text until you press **Edit**, and
"Use the default" puts it back; the default text is saved as "unset", so it keeps following cedar's
`SYSTEM_PROMPT`. The groups come from cedar's `GET /v1/tools`, so a group cedar doesn't offer is shown
greyed out with the reason (no Brave key, `BROWSER=off`, no Google account). Tools that send or
change things (sending mail, editing events) ask before they are switched on. The sandbox tools
(files, shell, Python, documents, charts) are always on.

The **Tools** button next to the message box changes the tools for the open conversation only
(in a new chat, for the conversation the first message creates); a dot on it shows the
conversation has its own choice. A schedule made from a conversation keeps that conversation's
tools.

Every message sends cedar the resolved choice, `*` when everything is on, so changing the defaults
also applies to follow-ups in existing conversations that don't have their own tools.

The label on the **send button** names the model the next message goes to and, after a dot, its
reasoning level (`DeepSeek V4 · high`, say). Clicking it opens a menu of the registered models,
each with the levels it declares (`none`, `low`, `high`, `max` for DeepSeek V4) and a "default"
chip for the model's own default level; a model with no levels is just picked. The label is shown
once there is something to choose: more than one model, or a model with levels. Both choices are
sent with every message (as the entry's id and the level), cedar checks them against its registry,
and the conversation remembers them, so opening it again shows the choice its last message used.
Until cedar has a model registered, the message box says so and sending is off.

### Pairing the iOS and Android apps

**Settings → Phone** has what the [iOS app](../iOS/README.md) and the Android app need to connect: the chat's
HTTPS address, the access token (and the admin token, if this browser has entered it), and the QR
code both apps scan under Settings → **Scan QR Code** (`cedar://connect?server=…&token=…`, drawn by
the server and shown only on request, since it holds the token). The apps connect over HTTPS only,
so the address is `CHAT_PUBLIC_URL` when it is an `https://` one: the tailnet address that the
[desktop app](../desktop/README.md#using-it-from-an-iphone-or-ipad) and `scripts/cedar-stack.sh`
start the chat with when they serve it with `tailscale serve`, or your own. The page also runs
`tailscale status` on the chat's computer and lists what is missing (Tailscale installed, logged
in, MagicDNS, HTTPS Certificates, the chat served), with how to serve it on this platform, and the
steps and apps on the phone. It only reports: serving the chat is up to the desktop app or the
script. The QR code and the Tailscale checks are shared with the desktop app's setup page
(`../pairing`).

### Attachments

A paperclip button by the message box attaches files to a message; they can also be dropped on
the box or pasted (a screenshot, say). PDFs go to any model: cedar saves them in the run's
workspace under `attachments/` and tells the agent, which reads them with its `read_document`
tool. Images go only to a model registered with **Accepts images** (a checkbox on its entry under
[Models](#models)), and only while the send button names it (the default model, until another is
picked); such a model is also shown the first pages of each PDF as images. PNG, JPEG, GIF, WebP
and PDF files are taken, at most 10 per message, 20 MB each and 32 MB together; the chosen files
are shown as chips, with a thumbnail for images, until the message is sent, and a message still
needs some text. Picking a model without the box drops chosen images and keeps PDFs. The browser
sends the files as base64 with the message, and cedar puts them in front of the prompt as
[attachments](../README.md#attachments) of the run, so the agent sees them for the rest of the
conversation. A sent message shows what went with it (name, type and size; the bytes stay in
cedar's run). cedar passes images to the model as they are and does not check that it can read
them: with a text-only model the response fails with the model server's error, so leave the box
off for one. A message with images to a model registered without the box is refused by cedar,
and the error is relayed.

### Integrations

Settings → **Server** is where the models and the API keys go. Without
`CHAT_ADMIN_TOKEN` it only explains how to turn it on and which tool groups lack a key. With it,
the page is unlocked with that token for the browser session and manages cedar's
[models](#models) (each with its **Max tokens per reply**, 16384 unless set), its Brave Search key (which also turns on product search) and its Best Buy key, which adds Best Buy's catalog to it (each checked with one search before it is saved, then shown
as its last four characters) and its Google account ("Connect Google" goes through Google's
consent page and back to `CHAT_PUBLIC_URL/api/oauth/google/callback`). They are stored by cedar,
encrypted, and take effect on the next message without a restart. This needs this server's
`CEDAR_API_KEY` in cedar's `CEDAR_ADMIN_KEYS`, and cedar's `CEDAR_SECRET_KEY` for anything that is
a secret; see [Models](../README.md#models) and [Integrations](../README.md#integrations). Keys set
in cedar's environment are shown as "Managed by environment" and can't be changed here.

With cedar's `TOOLS_DIR` set, the page also lists the **Personal tools**: each one's source (local
or community), version, what it asks for (internet hosts, secrets, packages, health data), and a
field per secret it declares, saved by cedar like the API keys and never shown again. With
`TOOLS_REGISTRY_URL` set too, **Community tools** lists the registry's tools, with Install,
Update and Uninstall. Installing first spells out what the tool will be able to do and asks to
confirm; see [Community tools](../README.md#community-tools).

### Models

The **Models** list in that section shows every model cedar has registered: its label, the model
name and the server it is on, whether an API key is saved, its reasoning levels, and which one is
the default. **Add a model**, in the heading of the list, asks for an id (filled in from the label), a label, the server address
(`http://10.0.0.5:8000/v1`, say; `/v1` is added if the path is left off), an API key if the server
needs one, and the model: **List models** asks the server which models it serves (with the key
typed, else the entry's saved one) and the model is picked from that list, never typed. Changing
the address or key empties the list until it is listed again. Then come the reasoning levels it takes as `reasoning_effort`
(comma-separated, like `none, low, high, max`), the level to use when a message picks none,
whether it accepts images (see [Attachments](#attachments)), and whether it is the
default for new chats. cedar checks the address by listing the server's models
again before saving; the edit form starts with the names it listed last time, with a warning on the entry if
the model name is not one of them. **Edit**, **Make default** and **Remove** change an entry; a removed model's
conversations fail at their next message until another model is picked. The first model
registered is the default, and a fresh cedar has none: the message box says so until an admin adds
one. Saving an API key needs cedar's `CEDAR_SECRET_KEY`; a server without one can be registered
without it. The send button's menu, the default model in Model & prompt, and the schedule dialog
all follow the list.

## API

All routes are under `/api`. When `CHAT_ACCESS_TOKEN` is set, they need
`Authorization: Bearer <token>` (except `/api/config`). Errors look like cedar's:
`{"error": {"code": ..., "message": ...}}`.

| Method and path | Purpose |
| --- | --- |
| `GET /api/config` | `{auth_required, admin_enabled}` |
| `GET /api/update` | The [update](#updates) banner's state: `{enabled, current, latest: {version, notes_url, published_at, tarball_url, checksums_url}, available, checked_at, error, can_install, install: {state, version, detail}, commands}`. `install.state` is `idle`, `requested`, `downloading`, `verifying`, `installing`, `restarting`, `done` or `failed` |
| `POST /api/update/check` | Look the latest release up now (at most once a minute); answers like `GET` |
| `POST /api/update` | Admin (`X-Admin-Token`): ask the `cedar-update` unit to install the latest release. 202, or 403 without `CHAT_UPDATE_DIR`, 409 when nothing newer is known or an update is running |
| `GET /api/conversations` | Conversations, most recently active first, each with `unread` and `needs_answer` (see [Notifications](#notifications)). Conversations a schedule created are left out |
| `POST /api/conversations` | Create one. Body `{}` or `{"title": "..."}` |
| `GET /api/conversations/{id}` | `{conversation, turns}`. Unfinished turns are refreshed from cedar; each turn carries `stop_reason`, `finished_at` and `seen` |
| `PATCH /api/conversations/{id}` | Rename: `{"title": "..."}` |
| `DELETE /api/conversations/{id}` | Delete. Returns 409 while a response is running |
| `POST /api/conversations/{id}/messages` | Send `{"content": "...", "model"?: "<registry id>", "reasoning"?: "<level>", "plan"?: "off"\|"on"\|"auto"\|"tool", "locale"?: "de-CH", "attachments"?: [{"name", "media_type", "data"}]}` (`data` is base64; see [Attachments](#attachments) for the types and limits, and the body may be up to 48 MiB). Starts a cedar run with the [settings](#tools-and-settings) (`plan`, the phone apps' Plans setting, overrides the Settings one for this message; `locale`, the language the page or app is shown in, overrides the saved one) and returns the turn, whose `attachments` lists `{name, media_type, size}` for each file. `model` and `reasoning` go to cedar as they are and its verdict is relayed (400 for an unregistered model or a level it doesn't offer); the conversation then reports them as `model` and `reasoning`. Images to a model registered without "accepts images" are a 400 from cedar, relayed. Returns 409 if a response is still running |
| `PUT /api/conversations/{id}/tools` | `{"tools": [...]}` gives the conversation its own tools; `{"tools": null}` returns it to the defaults |
| `GET /api/conversations/{id}/files` | The workspace files of the latest turn's run, as cedar reports them (`{workspace_id, files, truncated}`); empty for a conversation without turns |
| `GET /api/conversations/{id}/files/{path}` | Download one file, relayed from cedar with its type, length, and file name |
| `POST /api/conversations/{id}/compact` | Have cedar write one prompt for the conversation so far: `{prompt, source}` where `source` is `run` or, when cedar has lost the run, `history`. Returns 409 while a response is running, 404 without a finished response |
| `PUT` / `DELETE /api/conversations/{id}/star` | Star or unstar the conversation (a chat or a scheduled run) for every client. Both are idempotent; starring again keeps the first `starred_at`. Returns the conversation |
| `GET /api/starred` | `{conversations}`: the starred conversations, most recently starred first, scheduled runs included, each with `schedule_title` (the schedule a run belongs to, else `null`) |
| `POST /api/conversations/{id}/seen` | The conversation has been looked at: its finished responses leave the inbox. Returns the conversation (see [Notifications](#notifications)) |
| `GET /api/inbox` | `{items, unread}`: the finished or failed responses nobody has looked at, newest first (at most 50), each `{turn_id, conversation_id, title, schedule_id, status, stop_reason, needs_answer, finished_at}`, and how many there are |
| `GET /api/events` | Server-Sent Events for the page: a `turn.finished` event (`{turn_id, conversation_id, run_id, status, stop_reason, error, needs_answer, title, schedule_id, finished_at}`) for every response that ends. No replay: read `/api/inbox` on connecting |
| `GET /api/notifications` | How to be told [out of the app](#out-of-the-app): `{default_method, configured: [the methods that can send], scope, pushover: {configured, token_set, user_set, user_last4}, email: {configured, host, port, security, username, password_set, from, to}, browser: {public_key, contact, subscriptions: [{id, user_agent, created_at, last_used_at}]}, desktop: {configured, connected_at, listening}, ios: {configured, team_id, key_id, key_set, topic, environment, devices: [{id, name, created_at, last_used_at}]}, android: {configured (FCM set up, or a UnifiedPush phone registered), fcm_configured, project_id, client_email, key_set, devices: [{id, transport: "fcm"\|"unifiedpush", name, created_at, last_used_at}]}, last_delivery}`. Never the secrets. Makes the push keys on first read |
| `PUT /api/notifications` | Change any of `{"default_method": "none"\|"desktop"\|"browser"\|"ios"\|"android"\|"pushover"\|"email", "scope": "attention"\|"all", "pushover": {"token"?, "user"?}, "email": {"host"?, "port"?, "security"?: "tls"\|"starttls"\|"none", "username"?, "password"?, "from"?, "to"?}, "ios": {"team_id"?, "key_id"?, "private_key"? (the `.p8` file's contents), "topic"?, "environment"?: "production"\|"sandbox"}, "android": {"service_account"? (the Firebase service account's JSON file; checked for `project_id`, `client_email` and an RSA `private_key`)}, "contact"?}`. A secret left out is kept, an empty one cleared. 400 if the default method can't send (missing credentials, no subscribed browser, no registered iPhone or Android phone, only FCM phones and no service account, the Mac app never connected) |
| `POST /api/notifications/test` | `{"method"}`: send a test message with the saved settings. `{ok, detail}`, or 502 with what went wrong; recorded as `last_delivery` either way |
| `GET /api/notifications/desktop/events` | For Cedar for macOS: SSE, a `notification` event `{type, title, body, url, tag}` for every message sent by the `desktop` method. Connecting sets the method up |
| `POST /api/notifications/push/subscriptions` | A browser's `PushSubscription` (as `JSON.stringify` gives it). Returns the settings plus `subscription_id` |
| `DELETE /api/notifications/push/subscriptions` | `{"endpoint"}`: the browser turned push off |
| `DELETE /api/notifications/push/subscriptions/{id}` | Remove a subscribed browser |
| `POST /api/notifications/apns/devices` | The iOS app registered for push: `{"token": "<the device token, hex>", "name"?}`. Returns the settings plus `device_id`; the same token again renames the device |
| `DELETE /api/notifications/apns/devices` | `{"token"}`: the app turned push off, or left for another server |
| `DELETE /api/notifications/apns/devices/{id}` | Remove a registered iPhone |
| `POST /api/notifications/android/devices` | The Android app registered for push: `{"transport": "fcm"\|"unifiedpush", "token": "<the FCM registration token, or the UnifiedPush endpoint URL, https only>", "name"?}`. Returns the settings plus `device: {id, transport, name, created_at, last_used_at}` and `device_id`; the same token again updates the device |
| `DELETE /api/notifications/android/devices` | `{"token"}`: the app turned push off, or left for another server |
| `DELETE /api/notifications/android/devices/{id}` | Remove a registered Android phone |
| `GET /api/turns/{id}/events?after=N` | The turn's cedar event stream (SSE), relayed |
| `POST /api/turns/{id}/cancel` | Stop the turn's run |
| `GET /api/schedules` | Every schedule, enabled ones first, with its `last_run` |
| `POST /api/schedules` | Create one: `{"title", "prompt", "frequency": "daily"\|"weekdays"\|"weekly", "weekday"?: 0–6 (Monday–Sunday, weekly only), "time": "HH:MM", "timezone": "<IANA name>", "model"?, "reasoning"?, "conversation_id"?, "tools"?, "notify"?}`. `model` must be registered with cedar and `reasoning` one of its levels (or the default model's, without `model`); blank means the default. Without `tools` the schedule takes the conversation's own tools, if it has any, else it follows the defaults. `notify` is how its finished runs are announced: a method set up under [Notifications](#out-of-the-app) (`desktop`, `browser`, `ios`, `android`, `pushover`, `email`; 400 if it isn't set up), `none`, or `null` (the default) for the default method |
| `GET /api/schedules/{id}` | `{schedule, runs}`, newest run first, each with its `turn` (refreshed while unfinished) or an `error` if the run could not be started |
| `PATCH /api/schedules/{id}` | Change any of the fields above, or `{"enabled": false}` to pause. A changed timing (or resuming) recomputes the next run |
| `DELETE /api/schedules/{id}` | Delete the schedule with its runs' conversations and cedar runs. Returns 409 while one of its runs is going |
| `POST /api/schedules/{id}/run` | Start a run now, without moving the next scheduled one. Returns 409 while its latest run is still going |
| `GET /api/settings` | The defaults for new runs: `{tools, model, system, max_turns, plan, locale}` (`null` means cedar's own), plus `defaults`: cedar's own model (`model` and `model_id`, `null` until one is registered), system prompt and turn limit (`null` if cedar can't be reached) |
| `PUT /api/settings/locale` | `{"locale": "de-CH"}` saves the user's language and region (`null` goes back to cedar's `DEFAULT_LOCALE`). Scheduled runs and messages that don't name a locale use it, and notifications are written in it. The pages and apps ask for it first and keep it in Settings → Language |
| `PUT /api/settings` | Replace them (all but `locale`, which has its own route). Tool names are checked against cedar's `/v1/tools` and `model` against its `/v1/models`. A default model cedar no longer has is dropped when a message is sent, so cedar's own default applies |
| `GET /api/pairing` | What the iOS and Android apps connect with (Settings → Phone): `{platform, server, on_tailnet, access_token, link, qr_svg, tailscale: {state, name, detail}}`. `server` (and `link`, `qr_svg`) is `null` unless `CHAT_PUBLIC_URL` is `https://`; `tailscale.state` is `missing`, `unavailable`, `logged_out`, `stopped`, `connecting`, `no_magic_dns`, `no_https` or `ready` |
| `GET /api/tools` | cedar's tool groups (`GET /v1/tools`), relayed |
| `GET /api/models` | cedar's registered models (`GET /v1/models`), relayed, for the pickers |
| `GET /api/integrations` | Admin (`X-Admin-Token`): cedar's integrations, plus the Google `redirect_uri` to register |
| `GET /api/integrations/models` | Admin: cedar's registered models with their servers (`GET /v1/admin/models`), relayed |
| `POST /api/integrations/server-models` | Admin: the models a server lists (cedar's `POST /v1/admin/server-models`, body `{"base_url", "api_key"?, "id"?}`), relayed, for the model picker in the form |
| `PUT` / `DELETE /api/integrations/models/{id}` | Admin: register or change a model (the body is cedar's `{"base_url", "model", "label"?, "api_key"?, "reasoning_levels"?, "default_reasoning"?, "is_default"?}`, passed through as it is) or remove it |
| `GET` / `PUT /api/integrations/settings` | Admin: cedar's server-wide settings (`GET` / `PUT /v1/admin/settings`, body `{"max_image_side"?, "sandbox_idle_minutes"?, "sandbox_runtime"?}`), relayed; the Server page's "Max image size" and "Pause when idle", and the Advanced settings page's "Run sandboxes with OpenShell" |
| `PUT` / `DELETE /api/integrations/brave` | Admin: save (`{"api_key"}`) or remove cedar's Brave Search key |
| `PUT` / `DELETE /api/integrations/bestbuy` | Admin: save (`{"api_key"}`) or remove cedar's Best Buy key |
| `GET /api/integrations/tools` | Admin: cedar's personal tools with their permissions and which secrets are saved (`GET /v1/admin/tools`), relayed |
| `PUT` / `DELETE /api/integrations/tools/{name}/secrets/{secret}` | Admin: save (`{"value"}`) or remove a secret a personal tool declares |
| `POST /api/integrations/tools/{name}/setup` | Admin: install a personal tool's packages again |
| `GET /api/integrations/marketplace` | Admin: the [community tool](../README.md#community-tools) registry's tools (`?refresh=true` to skip cedar's cache) |
| `POST /api/integrations/marketplace/{name}/install` | Admin: install or update a community tool; the body is cedar's `{"accept": {...}}`, passed through |
| `DELETE /api/integrations/marketplace/{name}` | Admin: uninstall a community tool |
| `POST /api/integrations/google/connect` | Admin: `{url}` of Google's consent page |
| `DELETE /api/integrations/google` | Admin: disconnect the Google account |
| `GET /api/oauth/google/callback` | Where Google sends the browser back. Needs no token: the single-use `state` cedar issued for the admin's "Connect Google" is the check. Redirects to `/?integrations=connected` or `/?integrations=error&message=…` |

## Tests

```sh
cargo test -p cedar-chat
```

The integration tests in `tests/api.rs` run the chat API against a fake cedar server.
