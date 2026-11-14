# cedar-chat

A web chat for a cedar server, with two selectable looks. It is a separate Rust service
(axum + SQLite) that serves a single-page UI and sits between the browser and cedar:

```
browser  ──/api──▶  cedar-chat  ──/v1 (Bearer CEDAR_API_KEY)──▶  cedar  ──▶  vLLM + sandbox
```

The cedar API key stays on the server. The browser only talks to cedar-chat.

## Four interfaces

The settings button (sliders icon, top right) switches between:

| | Style | Layout |
| --- | --- | --- |
| `/v3/` | **Ledger** | The Cedar-style split, with the plan, approvals, citations and a sources rail |
| `/v4/` | **Dossier** | One column; each turn is a brief of plan, work, result and sources |
| `/v2/` | **Cedar style** | Say on the left, show on the right: the conversation (prompts, Cedar's prose, its questions, and a step row per tool call) in a narrow column, and everything a tool produced as full-width cards on a "stage" beside it, grouped by turn |
| `/v1/` | **Claude style** | Conversations in a sidebar, one thread with tool calls inline |

Your choice is saved in this browser (`localStorage`, key `cedar-chat-ui`) and decides where `/`
goes. Without a saved choice, `/` opens Ledger. Switching keeps the open conversation and
your access token. Opening `/v1/` to `/v4/` directly always shows that interface and does not
change the saved choice.

## Features

Both interfaces have all of these, except where noted:

- A conversation list (a sidebar in Claude style, a drawer behind the menu button in Cedar style)
  with new chat, rename (click the title), and delete
- Replies stream in as the agent works. Text, tool calls (`bash`, `run_python`, the file,
  document, chart, web, and weather tools, and `web_search` when cedar has a Brave key) with their input and
  output, errors, and token usage are shown as they happen
- A stop button cancels the running response
- Expert mode (Settings → Advanced settings → Expert mode, saved in this browser) adds a Trace button to each message
  you send. It opens a dialog with everything the agent did for that response, from cedar's
  `GET /v1/runs/{id}/trace` (`/api/turns/{id}/trace` here): a timeline of each model call (the
  parameters and messages it was sent, with the ones new since the previous call marked, and the
  response with its reasoning, tool calls, stop reason, tokens and time) and each tool call (input,
  output, time) in between, with the plan, approvals and blocked connections where they happened;
  the conversation as the model sees it; the system prompt and tool definitions; the event log;
  and the whole trace as JSON to copy or download (`trace-view.js` and `trace-view.css`)
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
- A `weather_forecast` tool call is drawn as a weather card, read from the same text report the
  model gets: the place and its local time, the current conditions with a large icon (a moon at
  night, between the day's sunset and sunrise), the temperature, feels like and the day's high and
  low, then tiles for humidity, wind with an arrow for its direction, gusts, precipitation, UV and
  daylight. Under it one row per day: its icon, chance of rain, and the low and high as a bar on a
  scale shared by all days (coloured from cold to warm; a dot marks the current temperature on
  today's bar). Click a day to see its wind, precipitation, UV and sunrise to sunset. Conditions are
  translated; the Open-Meteo credit its license asks for is at the bottom. If the report can't be
  read, only the tool call's raw output is shown
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
- A `search_flights` tool call (when cedar has a SerpApi key) is drawn as a flights card: the route,
  the day (one way, round trip with the day back, or the return flight), the passengers and the
  cabin; Google's price insight (low, typical or high for the route, the typical range, the cheapest
  price now) with a sparkline of the price history; then a row per flight with its price (for the
  whole trip on a round trip), a Top pick badge, the airline's logo and names, the departure and
  arrival times as the airports' local times (with "+1" when it lands a day later), the duration,
  the stops, and flags for flights that are often delayed, overnight, or emit notably less or more
  CO₂. Click a row to open its segments (flight number, aircraft, legroom) and layovers. After the
  model picks a flight, the card lists where to book it: each seller with its price, fare and bag
  fees, and whether it is a separate ticket for one way, with a Book on Google Flights button. Cedar
  style puts it on the stage as a card, Claude style under the tool call (`flight-view.js` and
  `flight-view.css`; see [Flight data and privacy](#flight-data-and-privacy))
- An `ask_user` tool call, when the agent needs a clarification before it goes on, is drawn as a
  form: each question with its options as radio buttons (pick one) or checkboxes (pick any), and
  always a field for an answer in your own words (for an open question, that field alone). Send
  answer sends what you picked and wrote as your next message, with each question repeated so the
  agent knows which answer is which; you can also just reply in the message box. Once a later
  message is in, the form is closed. Claude style puts it under the tool call, Cedar style in the
  conversation column (`clarify-view.js` and `clarify-view.css`). An [agent](#agents)'s run is
  told nobody is there to answer, so it assumes instead of asking
- Markdown rendering: headings, lists, tables, quotes, links, and code blocks with a copy button
- Light and dark themes (follows the system), plus a mobile layout
- The send button names the model and reasoning level the message goes with, and opens a menu
  of the registered models with the levels each declares; an optional access token
- [Tools and settings](#tools-and-settings): the tools, model, turn limit and system prompt every
  conversation gets, a Tools button to change the tools of one conversation, and (for admins)
  [the models](#models) cedar can use, its Brave Search, Best Buy and SerpApi keys, its Slack token and its Google account
- [Attachments](#attachments): PDFs, CSV and JSON files go with a message to any model, and images to a model registered as
  accepting them, by the paperclip button, by dropping them on the message box, or by pasting an image
- Starred conversations: the star button in the header stars the open conversation, a chat or a
  run of an agent alike, and a **Starred** list at the top of the sidebar (the drawer in the other
  styles) lists them, most recently starred first, with a clock on agents' runs. Stars are kept
  by this server, so the same list shows in every browser, the
  [Cedar app for macOS and Windows](../desktop/README.md) and the Cedar iOS and Android apps; a page picks up
  stars set elsewhere when it comes back into view, and every minute while it is shown
  (`star.js` and `star.css`)
- Search: a search box at the top of the sidebar (the drawer in the other styles) finds
  conversations, chats and agents' runs alike, by their title and by what was said in them, your
  messages and Cedar's answers. A conversation matches when it has every word you typed, in any
  case; the results take the place of the sidebar's lists, most recently active first, each with
  the passage that matched and the words marked. Ctrl+K (⌘K on a Mac) puts the cursor in the box,
  the arrow keys walk the results, Enter opens the first one and Escape empties the box
  (`search.js` and `search.css`)
- [Agents](#agents): prompts that run on their own, on a schedule, when something changes, when a
  webhook is called, or when mail or a calendar event comes up, made from scratch or from a
  finished conversation; a page lists them and every run's result, with what started it
- [Notifications](#notifications): a response that finishes while you are in another conversation,
  another tab, or away is marked unread in the conversation list and counted in the page title and
  on the bell button, whose inbox lists it (agents' runs too); a question the agent asked is
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
`stock-view.css` for stocks, `product-view.js` and `product-view.css` for products, `flight-view.js` and `flight-view.css` for flights, `clarify-view.js` and `clarify-view.css` for clarifying questions, `followup-view.js` and `followup-view.css` for suggested follow-ups,
`agents.js` and `agents.css` for the Agents page, `star.js` and `star.css` for starred
conversations, `search.js` and `search.css` for the search over the conversations, `notify.js` and `notify.css` for the inbox
of finished responses and the Notifications settings, `sw.js` for the service worker behind browser
push, and
`models.js` and `models.css` for the send button's model menu, `settings-shell.js` for the Settings
dialog's frame (its left nav, one page at a time, and the Save footer), `settings.js` and `settings.css` for the
Tools, Models, Advanced settings and Server pages, which every interface shares, `vendor/leaflet/` for the map
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

### Flight data and privacy

Airline logos are the one thing on a flights card that comes from elsewhere: the browser loads them
from the `https` addresses in the result (Google's `gstatic.com`), without a referrer; a logo that
fails to load leaves a plane icon. The Google Flights button is a link that opens in a new tab
without a referrer. Everything else is set as text, so nothing in the tool's result runs in the
page. The chat server makes no requests for flights; cedar asks SerpApi's Google Flights API (see
the top-level README). Remove the SerpApi key (Settings → Server) or set `FLIGHTS=off` on cedar to
remove the tool.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `CEDAR_API_KEY` | required | One of cedar's `CEDAR_API_KEYS` |
| `CEDAR_URL` | `http://127.0.0.1:8080` | cedar's base URL |
| `CHAT_BIND_ADDR` | `127.0.0.1:3000` | Where the chat server listens |
| `CHAT_DATABASE_URL` | `sqlite://cedar-chat.db` | Conversation storage |
| `CHAT_ACCESS_TOKEN` | unset | If set, the page asks for this token and the API requires it |
| `CHAT_ADMIN_TOKEN` | unset | If set, Settings offers [the models](#models) and the [integrations](#integrations) (cedar's API keys and Google account) to whoever enters it. Must differ from `CHAT_ACCESS_TOKEN` |
| `CHAT_MODELS_ACCESS` | `users` | Who may add, change and remove [the models](#models): `users` (everyone the access token lets in) or `admins` (the admin token; set this on a server several people use). API keys and the Google account stay with admins either way |
| `CHAT_PUBLIC_URL` | `http://<CHAT_BIND_ADDR>` | This server's address as the browser reaches it; "Connect Google" sends Google `<CHAT_PUBLIC_URL>/api/oauth/google/callback`, except for a browser that opened the chat at a loopback address (`127.0.0.1`, `localhost`), which is sent back to that address |
| `CHAT_HISTORY_MAX_BYTES` | `40000` | How much earlier conversation is inlined if the previous run is gone from cedar, or compacted for an agent in that case (at most 60000) |
| `CHAT_PUSHOVER_URL` | `https://api.pushover.net/1/messages.json` | Where Pushover messages go (for tests) |
| `CHAT_APNS_URL` | unset | Where iPhone pushes go instead of Apple's production or sandbox APNs (for tests) |
| `CHAT_FCM_URL` | `https://fcm.googleapis.com` | Where Android pushes through Firebase go (`…/v1/projects/{project}/messages:send`; for tests) |
| `CHAT_GOOGLE_TOKEN_URL` | `https://oauth2.googleapis.com/token` | Where the Firebase service account's signed assertion is traded for an access token (for tests) |
| `CHAT_TAILSCALE_CLI` | found | The `tailscale` CLI that Settings → Phone asks about Tailscale, when it isn't in the usual places or on `PATH` |
| `CHAT_UPDATE_CHECK` | `on` | Look up the latest GitHub Release a minute after the start and every six hours, and show a banner when it is newer (see [Updates](#updates)). `off` makes no requests to GitHub. The desktop app sets it `off`, it updates itself |
| `CHAT_UPDATE_DIR` | unset | The directory the root `cedar-update` unit watches (`/var/lib/cedar/update` in [DEPLOYMENT.md](../DEPLOYMENT.md#updates)). When set, an admin's *Update* in the banner installs the release; otherwise the banner shows the commands to run |
| `CHAT_UPDATE_URL` | `https://api.github.com/repos/hanneshapke/cedar/releases/latest` | Where the latest release is looked up (for tests) |
| `CHAT_CONTRIBUTIONS_URL` | the project's corpus | The base URL of a [cedar-contrib](../contrib/README.md) ingest that a conversation can be [contributed](#contributing-a-conversation) to; unset, the project's own ingest. `off`, and nothing can be sent; the review dialog still exports a bundle to a file |

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
- The bell button in the header counts the unseen responses, agents' runs included, and the
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
  agent asked, an agent's run) and every response that ends while you are not watching it.
- Each [agent](#agents) has a **Notify me by** picker in its form: the
  default, only in the app, or any set-up method, for that agent alone. An agent with a method of
  its own is announced on every finished or failed run, whatever **Send it for** says. The choice is
  shown with the agent in the list.

The last delivery, test or real, is shown with what the service answered. The credentials stay in
this server's database (`CHAT_DATABASE_URL`), as they are, and are never sent back to the page;
keep that file as private as the cedar key.

## Agents

An agent is a prompt that Cedar runs on its own, with a **trigger** that says when. The **Agents**
page (`#/agents`, linked from the sidebar in every style) lists them, grouped by what starts them:

| Trigger | Starts a run |
| --- | --- |
| On a schedule | Every day, on weekdays, every week on a weekday, every month on a day (the last day in a shorter month), every N minutes, hours or days (at least 15 minutes), or once at a date and time. Times are local: the browser's time zone is sent along (an IANA name like `Europe/Berlin`), so daylight-saving changes are handled |
| When something changes | Every few minutes (5 unless you say) the chat server looks at a value **without calling a model**, and starts a run only when the value passes a test. The value comes from a web address (a JSON API's reply, or a page's text), one of cedar's tools that can be called outside a run, or one of your [record sets](#records); a path such as `price.eur` picks it out of a JSON result. The test is *changes*, *is below / at most / above / at least* a number, *is exactly*, or *contains* |
| When a webhook is called | Another app or device sends a `POST` to the agent's address with its secret (see [Webhooks](#webhooks)) |
| When mail arrives | New mail matching a Gmail search (`from:billing@example.com has:attachment`), or any mail with an empty search |
| Around calendar events | Some minutes before an event starts, when a new event appears, or both, optionally only for events matching a text |
| When records are added | A row is added to one of your [record sets](#records), or added or changed. The run is handed those rows, so "a new row in `expenses`" can start a budget check. Nothing leaves your machine for this one |
| When a file arrives | A new file appears in a Google Drive folder (give the folder's address from the browser, or its ID). The run is handed the files' names and IDs and reads them with the Drive tools. Folders inside the folder don't count |
| From your phone | The Cedar app on your phone asks for a run (see [Your phone](#your-phone)): a Shortcuts action on an iPhone, the share sheet or an automation app on Android. Text or a link sent along goes to the run as data |

**New agent** makes one from scratch: a name, the prompt, the trigger, the model and its reasoning
level, and how you want to be told about its runs. **Make an agent** in a conversation's header
(once it has a finished response) starts from that chat instead: cedar reviews the whole
conversation, tool calls included, and writes one self-contained prompt for it (`POST /v1/compact`
in cedar's API), which you then edit. If cedar no longer has the run, the exchanges this service
remembers are compacted instead, up to `CHAT_HISTORY_MAX_BYTES`.

A trigger that watches something (a condition, records, mail, the calendar, a Drive folder) has a **Try it** button in the
form: one check, with nothing kept, that shows the value it found, whether a run would start, and
what came back, so you can find the path to the value you mean.

Each run is a **fresh** cedar run: a new conversation, a new workspace, no memory of the earlier
runs, and no files (an agent's prompt is text only). The chat server looks every 30 seconds, so a
run starts within half a minute of its time. Runs are not interactive, so they read what cedar
[remembers](#memory) about you (your units, your language, your projects) but cannot add to it or
change it: nobody is there to see a memory being saved, and an agent that reads mail or web pages
unattended must not be able to plant a standing instruction.

Every run lives in a conversation of its own, named after the agent and the time, with the full
tool trace and files panel. These conversations stay out of the conversation list. An agent's page
(`#/agents/{id}`) shows its trigger, where it stands (its next run, when it last checked, why a
check failed), its prompt, and every run with what started it, its status and its result; **Open**
shows a run as a conversation, whose header carries a badge that links back to the agent. There you
can also **Pause** and **Resume** it, **Run now**, **Edit** it and **Delete** it. Deleting an agent
deletes its runs' conversations and their cedar runs too. The **Activity** tab (`#/agents/activity`)
is the other way in: the latest runs of all agents, newest first, each with its agent and what
started it.

How triggers behave:

- **A first look only learns.** A *changes* condition and the mail, "new event", records and
  Drive triggers start nothing at their first check: what is already there is the starting point, and only what
  differs or arrives afterwards counts. A threshold (*is below 40*) starts a run when it is first
  found to hold, and again only after it has stopped holding in between.
- **A check that fails starts nothing.** The agent's page says why (the address returned an error,
  nothing is at that path, Google is not connected), and the check is tried again less and less
  often, up to every six hours, until it works.
- **Missed times do not pile up.** If the chat server was down when a run was due, the run starts
  once at the next start and the agent moves on to its next time. If cedar refuses the run because
  it is busy (its per-key active-run cap) or can't be reached, a clock's slot is retried every tick
  for an hour, after which it is recorded as a failed run and skipped. Resuming a paused agent
  skips what was missed while it was paused: a clock moves to its next time, and a watcher learns
  anew what is there.
- **A one-off switches itself off** after its run.
- **Mail, calendar and Drive need Google** connected under Settings → Integrations, with cedar's built-in
  Google backend (not `GOOGLE_BACKEND=mcp`). A calendar trigger looks at timed events; an all-day
  event has no start to be minutes before.
- **A personal tool as the thing to watch** runs its code on every check, in a container made for
  the call, with nobody approving it. It can be checked every 5 minutes at most.

At most 100 agents can exist.

### Runs that start from outside

A run on a clock is given your prompt and nothing else. A run started by a condition, records, a
webhook, your phone, mail, the calendar or a Drive folder is also given **what happened**: the
value, the new rows, the mail's subject and sender, the event, the files' names, the body of the
webhook call, what your phone shared. That is text other people wrote, and nobody is there to
approve what the agent does with it (cedar asks for approval only in interactive runs). Three
things limit what can go wrong:

- **The data is fenced off.** It is appended to the prompt in a block marked with a random number,
  under a note that it is data and not instructions (at most 16 KiB). The conversation keeps your
  prompt as you wrote it; the block is in the run's trace. This lowers the odds of an injected
  instruction being followed. It does not rule it out.
- **A careful choice of tools.** Unless you choose otherwise, such an agent gets every tool group
  cedar offers except personal tools, and from the Google and Slack groups only the tools that read. The
  form says so and lets you pick your default tools instead.
- **Limits on how often.** One run at a time per agent; at most one run every 5 minutes; at most
  24 runs in any 24 hours. Both numbers can be changed per agent (at least a minute apart, at most
  500 a day). An event that comes too soon waits: a check decides again the next time round, and a
  webhook call is kept, the latest in place of any earlier one, until the agent may run. The run
  is then told how many calls it replaced. **Run now** is your own doing and not held to these.

### Your phone

`POST /api/agents/{id}/trigger` starts an agent's run because your own device asked. It takes the
access token like every other route (it is you asking, not a stranger with a secret), works for
any agent that isn't paused, and can bring something along: `{"source"?, "text"?, "title"?,
"url"?}`, at most 16 KiB together, handed to the run as data. `source` says what on the device
asked (`shortcut`, `share`, `automation`) and shows with the run.

- On an **iPhone**, the Cedar app adds a **Run Cedar Agent** action to the Shortcuts app, with the
  agent to run and an optional text. Shortcuts' own automations can run it when you arrive at or
  leave a place, connect to CarPlay, at a time, or from the share sheet through a shortcut.
- On **Android**, the app is a **share target** (share text or a link to Cedar and pick the
  agent), and other apps such as Tasker can start an agent that has the *From your phone*
  trigger with an intent, once you allow that in the app's settings (it is off by default).

An automation can ask often, so these runs are held to the agent's limits like any other that
starts from outside; a request that comes too soon is answered `{"started": false, "held":
"too_soon"}` (or `busy`, `daily_limit`, `cedar_busy`) instead of being queued. An agent with the
*From your phone* trigger has no other way to start, besides **Run now**.

### Follow-ups the agent suggests

"Remind me on Friday", "check again tomorrow", "tell me when it ships": cedar's
`suggest_follow_up` tool lets the model answer these honestly. It **schedules nothing**. The chat
shows what the model suggested as a card under the reply: the title, when, the prompt the
follow-up would run, and **Set up** and **Dismiss**.

- **Set up** makes a one-off agent that runs the suggested prompt at the suggested time, in a new
  conversation, and notifies you like any agent. It shows under Agents, where you can change or
  delete it. Because the model wrote the prompt, perhaps after reading a page or a mail, the agent
  gets the [careful choice of tools](#runs-that-start-from-outside).
- **Dismiss** only records that you saw it. A suggestion whose time has passed can only be dismissed.
- The first decision stands, on every device, so the same suggestion can't be set up twice.
- To keep watching something, the follow-up's own run can suggest the next check, which again
  waits for you: nothing chains by itself.

This is why a prompt injection can at most put a suggestion in front of you: an agent only comes
to exist when you press the button, and you see its prompt first.

### Webhooks

Saving a webhook agent gives it an address, `{CHAT_PUBLIC_URL}/api/hooks/<32 hex characters>`, and
a secret. The secret is shown **once**; afterwards the page shows its last four characters and can
make a new one, which stops the old one at once. Whatever calls the address with the secret starts
a run, and the body of the request is handed to the agent as data:

```sh
curl -X POST 'https://chat.example/api/hooks/6be059e30e9f97a82578455b139de8cb' \
  -H 'Authorization: Bearer <secret>' \
  -H 'Content-Type: application/json' \
  -d '{"door": "open"}'
# 202 {"started": true, "queued": false, "conversation_id": "…"}
```

- The route takes **no access token**: the hook's secret is what lets a call in, and it gives
  access to nothing else on this server. The address is a random id, not the agent's.
- Instead of the bearer header, a call can carry a signature of its body made with the secret:
  `X-Cedar-Signature: t=<unix seconds>,v1=<hex>` (HMAC-SHA256 over `<t>.<body>`, the scheme
  [cedar's own callbacks](../README.md#being-told-when-a-run-ends) use; a timestamp more than five
  minutes off is refused), or GitHub's `X-Hub-Signature-256: sha256=<hex>` (HMAC-SHA256 over the
  body), so a repository webhook can point straight at an agent.
- Anything that doesn't check out (an unknown address, a wrong or missing secret, a paused agent)
  gets the same `404`. The body may be up to 64 KiB, of any content type; with it, the run is told
  the `Content-Type` and any `X-…-Event` header. More than 60 calls a minute to one hook (600 to
  all) get a `429`.
- `202 {"started": false, "queued": true}` means the call is kept until the agent may run (see the
  limits above).
- The sender has to be able to reach this server. On the default `127.0.0.1` that is only this
  machine; behind Tailscale, only your tailnet. The secret is kept in this server's database
  (`CHAT_DATABASE_URL`) as it is, like the notification credentials.

## Tools and settings

**Settings → Tools** sets which of cedar's tool groups every new conversation gets (web search,
reading web pages, weather, maps, the browser, Gmail and Calendar, the last two down to single
tools), and **Settings → Models** the default model (one of those cedar has
[registered](#models); unset means cedar's own default), turn limit, and system prompt. The
system prompt reads as a document showing cedar's own default text until you press **Edit**, and
"Use the default" puts it back; the default text is saved as "unset", so it keeps following cedar's
`SYSTEM_PROMPT`. The groups come from cedar's `GET /v1/tools`, so a group cedar doesn't offer is shown
greyed out with the reason (no Brave key, `BROWSER=off`, no Google account). Tools that send or
change things (sending mail, editing events) ask before they are switched on. The sandbox tools
(files, shell, Python, documents, charts) are always on.

The **Tools** button next to the message box changes the tools for the open conversation only
(in a new chat, for the conversation the first message creates); a dot on it shows the
conversation has its own choice. An agent on a schedule made from a conversation keeps that conversation's
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

### Memory

cedar remembers you across conversations (see [Memory](../README.md#memory)): the facts,
preferences and standing instructions you state, saved when you tell it something lasting or ask
it to remember something, and noted from your message once the reply is done, so a fact mentioned
in passing ("I'm vegetarian, by the way") is kept too; what each conversation was about; and ways
of working that went well. Each fact or procedure saved shows as a *remember* step under the reply.
**Settings → Memory** shows the profile at the top (the short summary cedar reads before every
reply, written by cedar after a few conversations, or by you: edit it and it stays as you wrote
it), then the memories by kind (*About you*, *Conversations*, *Ways of working*) and the
*Archive*, newest first, each with the words it was taken from, the date, a link to the
conversation it came from and how often it was used, with **Pin** (always shown to cedar),
**Archive** or **Restore**, and **Remove** (for good) on each, **Export** for a backup, and
**Forget all**. A change applies from the next message on, in every conversation. The thumbs on a
reply are passed on to cedar, which counts them for or against what it learned in that run. The
iOS and Android apps have the same page in Settings: the profile, the kinds and the archive, with
pin, archive, restore and delete on each memory.

Memory is the `memory` tool group, so **Settings → Tools** turns it off for new conversations and
the conversation's **Tools** button for one conversation. If you ever unticked a group there,
your saved choice predates memory and leaves it off; the Memory page says so. Everyone using this
chat server shares one memory, since the chat calls cedar with one `CEDAR_API_KEY`. Memory stays
on your own cedar server and is never shared with a third party (see [PRIVACY.md](../PRIVACY.md)).

### Records

Memory holds a few facts; for data you will come back to, cedar keeps [records](../README.md#records):
named sets of rows (a running log, weekly measurements, an export) that outlive conversations and
that the agent reads back with a query, so a weekly check-in can compare with every earlier week
from a fresh conversation. **Settings → Records** lists the sets, most recently saved first, each
with its description, how many rows it holds, its key, its columns and when it was last saved,
with a *Health data* badge where the agent marked it so. **Show rows** opens the set's rows in a
table, newest first, with the date each was saved and a link to the conversation that saved it;
**Delete** removes a set, **Delete all** every set. The list is exactly what the agent is told
about, and a deletion applies from the next message on.

Records are the `records` tool group, so **Settings → Tools** turns them off for new
conversations and the conversation's **Tools** button for one conversation; a saved choice from
before records leaves them off, and the Records page says so. Everyone using this chat server
shares the same records, since the chat calls cedar with one `CEDAR_API_KEY`. Records stay on
your own cedar server and are never shared with a third party.

### Contributing a conversation

A conversation can be given, one at a time and on purpose, to a corpus for training open-weight
agent models (the design is in [docs/design/contributions.md](../docs/design/contributions.md);
what it means for you is in [PRIVACY.md](../PRIVACY.md#contributing-a-conversation-opt-in)).
Three things have to be true before anything leaves this server:

1. The server has a corpus to send to: the project's own unless whoever runs it named another
   (`CHAT_CONTRIBUTIONS_URL`) or set it to `off`. Off, the More menu offers *Export for
   contribution…* only in expert mode, and only as a download.
2. You have read **Settings → Contribute** and turned it on there. The page says what a
   contribution contains, names the corpus, and takes your one-time choices: the license for
   what you wrote (CC0 or CC BY 4.0), a pseudonym (a random id made here, which you can rotate or
   drop; without one each contribution stands alone), and whether the model's reasoning and the
   pages the agent fetched are included by default (both off).
3. You chose *Contribute this conversation…* in the More menu and clicked *Send* in the review
   dialog.

The review dialog shows exactly what would be sent, built from the traces of the conversation's
runs: your prompts, the agent's replies and tool calls, the system prompt and tool definitions,
the model calls' counts and tokens, and the events that matter for training (plans and your edits
to them, approvals and denials, how each run ended). Before you see it, the chat cuts your memory
list out of the system prompt, drops attached files (their bytes and names), replaces the bodies
of fetched pages with their size and hash, leaves reasoning out, drops identifiers and exact
times, and replaces emails, phone and account numbers, IP addresses, keys and home-directory names
with made-up stand-ins of the same shape (`maya.lindqvist@example.net`, `+1 555 0137`,
`/Users/theo`, `sk-stand1n…`), the same stand-in for the same value throughout the conversation and
random otherwise, so a trajectory stays coherent and readable rather than full of bracket tokens.
The bundle lists which values are stand-ins (`redactions`), and the dialog marks each. You can
include reasoning or fetched pages for this one conversation, redact any phrase you type and say
what to put in its place (a made-up name reads best; without one, a neutral `redacted-…`), rate each response with a thumb (saved on the turn, sent as its rating),
and say whether the agent did what you asked. The *Exactly what is sent* tab is the document that
*Send* posts, byte for byte, and *Download JSON* saves it.

Some conversations can't be contributed, and the dialog says why: one still being answered, an
agent's run, one that called a Gmail or Calendar tool (other people's data), one whose run
cedar no longer has, one from a model the corpus doesn't publish, or one in which a secret is still
in the text after redaction. The checks the corpus runs are run here first, so a bundle the dialog
shows as ready is one the corpus will take.

Each contribution leaves a receipt in **Settings → Contribute**, with *Withdraw*: the corpus deletes
it from its intake and leaves it out of every later release. *Refresh status* asks the corpus what
became of each: received, accepted by its screening, held for a reviewer, rejected, or released
(with the release's version). Deleting the conversation keeps the receipt. The withdraw token is
kept in the chat's database like its other credentials.

### Pairing the iOS and Android apps

**Settings → Phone** has what the [iOS app](../iOS/README.md) and the Android app need to connect: the chat's
HTTPS address, the access token (and the admin token, if this browser has entered it), and the QR
code both apps scan under Settings → **Scan QR Code** (`cedar://connect?server=…&token=…`, drawn by
the server and shown only on request, since it holds the token). *Copy connection link* copies
the same link as text, which the [desktop app](../desktop/README.md#using-cedar-that-runs-on-another-computer)
takes to use this server from another computer. The apps connect over HTTPS only,
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
the box or pasted (a screenshot, say). PDFs, CSV and JSON files go to any model: cedar saves them
in the run's workspace under `attachments/` and tells the agent, which reads a PDF with its
`read_document` tool; a CSV or JSON file is also imported into a [record set](#records) named
after the file, unless the cedar server has that off, and the agent is told where its rows went. Images go only to a model registered as accepting images (Cedar works that out when the model is added; see [Models](#models))), and only while the send button names it (the default model, until another is
picked); such a model is also shown the first pages of each PDF as images. PNG, JPEG, GIF, WebP,
PDF, CSV and JSON files are taken, at most 10 per message, 20 MB each and 32 MB together; the chosen files
are shown as chips, with a thumbnail for images, until the message is sent, and a message still
needs some text. Picking a model without the box drops chosen images and keeps the other files. The browser
sends the files as base64 with the message, and cedar puts them in front of the prompt as
[attachments](../README.md#attachments) of the run, so the agent sees them for the rest of the
conversation. A sent message shows what went with it (name, type and size; the bytes stay in
cedar's run). cedar passes images to the model as they are and does not check that it can read
them: with a text-only model the response fails with the model server's error, so leave the box
off for one. A message with images to a model registered without the box is refused by cedar,
and the error is relayed.

### Integrations

Settings → **Server** is where the API keys go. Without `CHAT_ADMIN_TOKEN` it only explains how
to turn it on and which tool groups lack a key. With it, the page is unlocked with that token for
the browser session and manages cedar's Brave Search key (which also turns on product search), its Best Buy key, which adds Best Buy's catalog to it, and its SerpApi key, which turns on flight search (a key from serpapi.com, whose free plan includes a limited number of searches a month; it is checked against SerpApi's account page, which uses up no search), its Slack bot token, which turns on the Slack tools (the `xoxb-…` token of a Slack app installed to your workspace; it is checked with Slack, and the row then names the workspace and the app it posts as) (the others are each checked with one search before they are saved, then shown
as its last four characters), its Google OAuth client (the ID and secret of a client made in your
own Google Cloud project, or the JSON file the console downloads pasted into the form; the page
walks through making one) and its Google account ("Connect Google" goes through Google's consent
page and back to `CHAT_PUBLIC_URL/api/oauth/google/callback`, or, for a browser on the chat's own
computer that opened it at a loopback address, back to that address, which a *Desktop app* client
accepts without registration). They are stored by cedar,
encrypted, and take effect on the next message without a restart. This needs this server's
`CEDAR_API_KEY` in cedar's `CEDAR_ADMIN_KEYS`, and cedar's `CEDAR_SECRET_KEY` for anything that is
a secret; see [Models](../README.md#models) and [Integrations](../README.md#integrations). Keys set
in cedar's environment are shown as "Managed by environment" and can't be changed here.

Settings → **Models** holds cedar's [models](#models) (each with its **Max tokens per reply**, 16384
unless set) above your own defaults for new conversations and the system prompt. By default
(`CHAT_MODELS_ACCESS=users`) the list is open to everyone with the access token, admin token or
not, since on a personal setup whoever chats also picks the models: the page, the phone apps and
any browser can add a server and its model. With `CHAT_MODELS_ACCESS=admins`, for a server several
people use as [DEPLOYMENT.md](../DEPLOYMENT.md) sets it up, the page shows the models read-only
until the Server page is unlocked with the admin token. The API keys, the Google account, the
server settings and the personal tools need the admin token either way.

With cedar's `TOOLS_DIR` set, the page also lists the **Personal tools**: each one's source (local
or community), version, what it asks for (internet hosts, secrets, packages, health data), and a
field per secret it declares, saved by cedar like the API keys and never shown again. With
`TOOLS_REGISTRY_URL` set too, **Community tools** lists the registry's tools, with Install,
Update and Uninstall. Installing first spells out what the tool will be able to do and asks to
confirm; see [Community tools](../README.md#community-tools).

### Models

The **Models** list in that section shows every model cedar has registered: its label, the model
name and the server it is on, whether an API key is saved, its reasoning levels, and which one is
the default. **Add a model**, in the heading of the list, offers three ways, the same as the
desktop app's setup page:

- **Run a model here with llama.cpp.** Only the desktop app does this: it sends the window to its
  setup page, which downloads llama.cpp and a model for the GPU and adds it to the list as an
  entry of its own. The option is greyed out, with the reason, in a browser or on a phone, with
  no GPU that can run a model, when the app shows the chat of another computer, and while the
  app already runs a model (the GPU's memory holds one). That entry has **Manage…** in place of
  **Edit** and **Remove**, which leads to the same page.
- **Use a hosted provider.** A list of providers (Baseten, Nebius, Doubleword and others; *Other…*
  takes any address) fills in the address, with a link to the provider's documentation. *Find a
  model* narrows a long model list.
- **Use a model server you already run.** A preset (Ollama, LM Studio, vLLM, llama.cpp, SGLang)
  fills in the address, says what that server needs for Cedar, and lists its models.

The providers and presets come from `static/model-servers.json`, served at
`/model-servers.json`, which the desktop app compiles in too. The form under the last two asks
for the server address
(`http://10.0.0.5:8000/v1`, say; `/v1` is added if the path is left off), an API key if the server
needs one, an id (filled in from the label or the model), a label, and the model: **List models** asks the server which models it serves (with the key
typed, else the entry's saved one) and the model is picked from that list, never typed. Changing
the address or key empties the list until it is listed again. Then come the reasoning levels it takes as `reasoning_effort`
(they follow the chosen model where `static/suggested-models.json` names it and show as one line; *Advanced* opens
a dropdown of the usual sets, like `none, low, high, max`, with *Other…* for a list typed by hand, and the level to use when a message picks none),
and whether it is the
default for new chats. Whether the model reads images (see [Attachments](#attachments)) is not asked: a list of known
models in `static/suggested-models.json` says, a model it does not name is tried with one small image when
it is saved, and *Advanced* has a dropdown to say it outright. cedar checks the address by listing the server's models
again before saving; the edit form starts with the names it listed last time, with a warning on the entry if
the model name is not one of them. **Edit**, **Make default** and **Remove** change an entry; a removed model's
conversations fail at their next message until another model is picked. The first model
registered is the default, and a fresh cedar has none: the message box says so until an admin adds
one. Saving an API key needs cedar's `CEDAR_SECRET_KEY`; a server without one can be registered
without it. The send button's menu, the default model on the Models page, and the agent form
all follow the list.

## API

All routes are under `/api`. When `CHAT_ACCESS_TOKEN` is set, they need
`Authorization: Bearer <token>` (except `/api/config`, and `/api/hooks/{hook_id}`, which takes a [webhook](#webhooks)'s own secret). Errors look like cedar's:
`{"error": {"code": ..., "message": ...}}`.

| Method and path | Purpose |
| --- | --- |
| `GET /api/config` | `{auth_required, admin_enabled, models_access, memory, contributions}`; `models_access` is `admins` or `users` (`CHAT_MODELS_ACCESS`), `contributions` is whether `CHAT_CONTRIBUTIONS_URL` is set, and `memory` is `true` on a chat server with the `/api/memory` routes, so the apps can hide Memory on an older one |
| `GET /api/update` | The [update](#updates) banner's state: `{enabled, current, latest: {version, notes_url, published_at, tarball_url, checksums_url}, available, checked_at, error, can_install, install: {state, version, detail}, commands}`. `install.state` is `idle`, `requested`, `downloading`, `verifying`, `installing`, `restarting`, `done` or `failed` |
| `POST /api/update/check` | Look the latest release up now (at most once a minute); answers like `GET` |
| `POST /api/update` | Admin (`X-Admin-Token`): ask the `cedar-update` unit to install the latest release. 202, or 403 without `CHAT_UPDATE_DIR`, 409 when nothing newer is known or an update is running |
| `GET /api/conversations` | Conversations, most recently active first, each with `unread` and `needs_answer` (see [Notifications](#notifications)). Conversations that are an agent's runs are left out |
| `POST /api/conversations` | Create one. Body `{}` or `{"title": "..."}` |
| `GET /api/conversations/{id}` | `{conversation, turns, agent?, followups}`. Unfinished turns are refreshed from cedar; each turn carries `stop_reason`, `finished_at` and `seen`. `agent` is `{id, title, trigger_kind}` when the conversation is an [agent](#agents)'s run; `followups` lists what was decided about the [follow-ups suggested](#follow-ups-the-agent-suggests) in its turns (one not listed is still open) |
| `PATCH /api/conversations/{id}` | Rename: `{"title": "..."}` |
| `DELETE /api/conversations/{id}` | Delete. Returns 409 while a response is running |
| `POST /api/conversations/{id}/messages` | Send `{"content": "...", "model"?: "<registry id>", "reasoning"?: "<level>", "plan"?: "off"\|"on"\|"auto"\|"tool", "locale"?: "de-CH", "attachments"?: [{"name", "media_type", "data"}]}` (`data` is base64; see [Attachments](#attachments) for the types and limits, and the body may be up to 48 MiB). Starts a cedar run with the [settings](#tools-and-settings) (`plan`, the phone apps' Plans setting, overrides the Settings one for this message; `locale`, the language the page or app is shown in, overrides the saved one) and returns the turn, whose `attachments` lists `{name, media_type, size}` for each file. `model` and `reasoning` go to cedar as they are and its verdict is relayed (400 for an unregistered model or a level it doesn't offer); the conversation then reports them as `model` and `reasoning`. Images to a model registered without "accepts images" are a 400 from cedar, relayed. Returns 409 if a response is still running |
| `PUT /api/conversations/{id}/tools` | `{"tools": [...]}` gives the conversation its own tools; `{"tools": null}` returns it to the defaults |
| `GET /api/conversations/{id}/files` | The workspace files of the latest turn's run, as cedar reports them (`{workspace_id, files, truncated}`); empty for a conversation without turns |
| `GET /api/conversations/{id}/files/{path}` | Download one file, relayed from cedar with its type, length, and file name |
| `POST /api/conversations/{id}/compact` | Have cedar write one prompt for the conversation so far: `{prompt, source}` where `source` is `run` or, when cedar has lost the run, `history`. Returns 409 while a response is running, 404 without a finished response |
| `GET /api/conversations/{id}/contribution` | The conversation as it would be [contributed](#contributing-a-conversation): `{bundle, blocked, secrets, problems, can_send, accepted, private_tools, turn_ids}`. `?reasoning=true` and `?fetched=true` include what is left out by default. `blocked` lists why it can't be sent (`scheduled`, `active`, `no_turns`, `private_tools`, `run_gone`, `secrets`, `not_consistent`, `model_not_allowed`), empty when it can |
| `POST /api/conversations/{id}/contribution` | Send `{"bundle", "outcome"?}` (the bundle as the dialog holds it; `outcome` is `{result: succeeded\|partly\|failed, notes?}`) to `CHAT_CONTRIBUTIONS_URL`. The license and pseudonym come from Settings whatever the body says; the checks run again first. `201` with the receipt; `409` without a corpus or while contributing is off in Settings; `400` for a bundle that fails the checks or still holds a secret, or that the corpus refused (its code and message) |
| `GET /api/contributions` | `{contributions}`: every receipt, newest first, each `{id, conversation_id, title, content_sha256, license, url, submitted_at, withdrawn_at}` (never the withdraw token) |
| `POST /api/contributions/refresh` | Ask the corpus what became of each contribution not withdrawn (its screening and release status, with the receipt's token); answers like `GET`, with `status` (`quarantined`, `accepted`, `held`, `rejected`, `released`, or `unknown` when the corpus no longer knows it), `released_in` and `checked_at` filled in |
| `DELETE /api/contributions/{id}` | Withdraw it from the corpus; answers the receipt with `withdrawn_at`. `404` for an unknown receipt |
| `GET` / `PUT /api/settings/contributions` | The one-time choices: `{license, pseudonym, include_reasoning, include_fetched, accepted_at, available, host, licenses}`. `PUT` takes any of `license`, `pseudonym` (`"new"` makes one, `""` drops it), `include_reasoning`, `include_fetched` and `enabled` (sets or clears `accepted_at`) |
| `PUT /api/turns/{id}/rating` | `{"rating": 1 \| -1 \| null}`: your thumb on a response; answers the turn |
| `PUT` / `DELETE /api/conversations/{id}/star` | Star or unstar the conversation (a chat or an agent's run) for every client. Both are idempotent; starring again keeps the first `starred_at`. Returns the conversation |
| `GET /api/conversations/search?q=` | `{conversations}`: up to 50 conversations that have every word of `q` (at most 200 characters; the first 8 words count) in their title, a message or a response, whatever the case, most recently active first, agents' runs included. Each has `schedule_title` like the starred ones and `snippet`: the passage of the first message that matched, or `null` when only the title did. An empty `q` finds nothing |
| `GET /api/starred` | `{conversations}`: the starred conversations, most recently starred first, agents' runs included, each with `schedule_title` (the agent a run belongs to, else `null`) |
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
| `GET /api/agents` | `{agents, limits}`: every [agent](#agents), enabled ones first, each `{id, title, prompt, model, reasoning, tools, notify, enabled, source_conversation_id, timezone, trigger, next_run_at, next_check_at, last_run_at, last_checked_at, last_error, waiting, min_interval_secs, max_runs_per_day, hook, last_run, created_at, updated_at}`. `next_run_at` is set for an agent on a clock and `next_check_at` for one that watches; `last_error` says why its last check failed; `waiting` that a webhook call is held until it may run; `hook` is `{url, path, secret_last4}` for a webhook agent. `limits` has the defaults (`max_agents`, `min_interval_secs`, `max_runs_per_day`, `min_interval_minutes`, `check_minutes`, `personal_check_minutes`) |
| `POST /api/agents` | Create one: `{"title", "prompt", "trigger", "timezone"?, "model"?, "reasoning"?, "conversation_id"?, "tools"?, "notify"?, "enabled"?, "min_interval_secs"?, "max_runs_per_day"?}`. `trigger` is one of the shapes below. Without `tools`, an agent that events start gets the [careful choice](#runs-that-start-from-outside), and one on a clock the conversation's tools or the defaults; `null` means the defaults. `notify` is how its finished runs are announced: a method set up under Notifications (`desktop`, `browser`, `ios`, `android`, `pushover`, `email`; 400 if it isn't set up), `none`, or `null` (the default) for the default method. Returns `201` with the agent; a webhook agent's `hook.secret` is in this response only. 409 at 100 agents |
| `GET /api/agents/{id}` | `{agent, runs}`, newest run first (at most 100), each `{id, schedule_id, conversation_id, turn_id, due_at, started_at, manual, error, cause, turn}`. `cause` is what started it, `{kind, summary, excerpt?}` with `kind` one of `clock`, `changed`, `holds`, `rows`, `webhook`, `device`, `mail`, `soon`, `new_event`, `files`, and `summary` a line in the saved language; `null` for "run now". `turn` is refreshed while unfinished; `error` says why a run could not be started |
| `PATCH /api/agents/{id}` | Change any of the fields above, or `{"enabled": false}` to pause. A `trigger` replaces the whole trigger and forgets what the old one had seen; resuming skips what was missed. A webhook agent that gets another trigger loses its address |
| `DELETE /api/agents/{id}` | Delete the agent with its runs' conversations and cedar runs. Returns 409 while one of its runs is going |
| `POST /api/agents/{id}/run` | Start a run now, whatever the trigger says. Returns 409 while its latest run is still going |
| `POST /api/agents/{id}/hook/rotate` | A new secret for a webhook agent, returned in `hook.secret` this once; the old one stops working. 409 if the agent has no webhook |
| `POST /api/agents/{id}/trigger` | [Your phone](#your-phone) asks for a run of any agent that isn't paused: `{"source"?, "text"?, "title"?, "url"?}` (the body may be left out; at most 16 KiB, else `413`). `202 {started: true, conversation_id}` or `202 {started: false, held: "busy" \| "too_soon" \| "daily_limit" \| "cedar_busy"}`; `409` for a paused agent |
| `PUT /api/turns/{id}/followups/{call_id}` | Decide about [a follow-up the model suggested](#follow-ups-the-agent-suggests) in that turn (`call_id` is the `suggest_follow_up` call's id in the run's events): `{"decision": "accept", "title", "prompt", "at", "timezone"}` makes a one-off agent, `{"decision": "dismiss"}` records that. Returns `{followup: {turn_id, call_id, status, agent_id, decided_at}, agent}`. The first decision stands; deciding again returns it. `400` if the time has passed |
| `POST /api/agents/test` | Try a watching trigger out, keeping nothing: `{"trigger"}` → `{ok: true, value, would_fire, summary?, sample: {text, cut, json}}`, or `{ok: false, error, sample?}` when the check fails. 400 for a trigger that doesn't watch |
| `GET /api/agents/activity` | The latest runs of all agents, newest first: `{items, next_before}`, each item a run as above plus `agent: {id, title, trigger_kind}`. `?limit=` (30, at most 100) and `?before=` (the previous page's `next_before`) page back |
| `POST /api/hooks/{hook_id}` | **No access token**: [a webhook agent's address](#webhooks). `202 {started, queued, conversation_id?}`; `404` for anything that doesn't check out; `413` over 64 KiB; `429` over 60 calls a minute |
| `GET /api/schedules`, `POST /api/schedules`, `GET` / `PATCH` / `DELETE /api/schedules/{id}`, `POST /api/schedules/{id}/run` | The agents API from before triggers, kept for the iPhone and Android apps of that time: the same agents, but only those on a daily, weekday or weekly clock, in the old shape (`frequency`, `weekday`, `time_of_day`, `timezone`, `next_run_at` on the agent itself). Any other agent is a `404` here. New clients use `/api/agents` |
| `GET /api/settings` | The defaults for new runs: `{tools, model, system, max_turns, plan, locale}` (`null` means cedar's own), plus `defaults`: cedar's own model (`model` and `model_id`, `null` until one is registered), system prompt and turn limit (`null` if cedar can't be reached) |
| `PUT /api/settings/locale` | `{"locale": "de-CH"}` saves the user's language and region (`null` goes back to cedar's `DEFAULT_LOCALE`). Scheduled runs and messages that don't name a locale use it, and notifications are written in it. The pages and apps ask for it first and keep it in Settings → Language |
| `PUT /api/settings` | Replace them (all but `locale`, which has its own route). Tool names are checked against cedar's `/v1/tools` and `model` against its `/v1/models`. A default model cedar no longer has is dropped when a message is sent, so cedar's own default applies |
| `GET /api/pairing` | What the iOS and Android apps connect with (Settings → Phone): `{platform, server, on_tailnet, access_token, link, qr_svg, tailscale: {state, name, detail}}`. `server` (and `link`, `qr_svg`) is `null` unless `CHAT_PUBLIC_URL` is `https://`; `tailscale.state` is `missing`, `unavailable`, `logged_out`, `stopped`, `connecting`, `no_magic_dns`, `no_https` or `ready` |
| `GET /api/tools` | cedar's tool groups (`GET /v1/tools`), relayed |
| `GET /api/models` | cedar's registered models (`GET /v1/models`), relayed, for the pickers |
| `GET /api/memory` | What cedar remembers about the user (`GET /v1/memory`): `{supported, items, limits}`, each item `{id, text, run_id, created_at, updated_at}` plus the `conversation_id` and `title` of the conversation it was saved in (`null` when there is none here). A cedar from before memory gives `supported: false` and no items. See [Memory](#memory) |
| `POST /api/memory` | `{"text"}`: remember something by hand, relayed to `POST /v1/memory` |
| `DELETE /api/memory/{id}` | Forget one memory (`404` if cedar has no such item) |
| `DELETE /api/memory` | Forget everything cedar remembers about the user |
| `GET /api/integrations` | Admin (`X-Admin-Token`): cedar's integrations, plus the Google `redirect_uri` to register |
| `GET /api/integrations/models` | Admin: cedar's registered models with their servers (`GET /v1/admin/models`), relayed |
| `POST /api/integrations/server-models` | Admin: the models a server lists (cedar's `POST /v1/admin/server-models`, body `{"base_url", "api_key"?, "id"?}`), relayed, for the model picker in the form |
| `PUT` / `DELETE /api/integrations/models/{id}` | Admin: register or change a model (the body is cedar's `{"base_url", "model", "label"?, "api_key"?, "reasoning_levels"?, "default_reasoning"?, "is_default"?}`, passed through as it is) or remove it |
| `GET` / `PUT /api/integrations/settings` | Admin: cedar's server-wide settings (`GET` / `PUT /v1/admin/settings`, body `{"max_image_side"?, "sandbox_idle_minutes"?, "sandbox_runtime"?}`), relayed; the Server page's "Max image size" and "Pause when idle", and the Advanced settings page's "Run sandboxes with OpenShell" |
| `PUT` / `DELETE /api/integrations/brave` | Admin: save (`{"api_key"}`) or remove cedar's Brave Search key |
| `PUT` / `DELETE /api/integrations/bestbuy` | Admin: save (`{"api_key"}`) or remove cedar's Best Buy key |
| `PUT` / `DELETE /api/integrations/serpapi` | Admin: save (`{"api_key"}`) or remove cedar's SerpApi key, which turns on `search_flights` |
| `PUT` / `DELETE /api/integrations/slack` | Admin: save (`{"api_key"}`) or remove cedar's Slack bot token, which turns on the `slack_*` tools |
| `GET /api/integrations/tools` | Admin: cedar's personal tools with their permissions and which secrets are saved (`GET /v1/admin/tools`), relayed |
| `PUT` / `DELETE /api/integrations/tools/{name}/secrets/{secret}` | Admin: save (`{"value"}`) or remove a secret a personal tool declares |
| `POST /api/integrations/tools/{name}/setup` | Admin: install a personal tool's packages again |
| `GET /api/integrations/marketplace` | Admin: the [community tool](../README.md#community-tools) registry's tools (`?refresh=true` to skip cedar's cache) |
| `POST /api/integrations/marketplace/{name}/install` | Admin: install or update a community tool; the body is cedar's `{"accept": {...}}`, passed through |
| `DELETE /api/integrations/marketplace/{name}` | Admin: uninstall a community tool |
| `PUT` / `DELETE /api/integrations/google/client` | Admin: save (`{"client_id", "client_secret"}`) or remove cedar's Google OAuth client; the answer carries the `redirect_uri` like the status does |
| `POST /api/integrations/google/connect` | Admin: `{url}` of Google's consent page |
| `DELETE /api/integrations/google` | Admin: disconnect the Google account |
| `GET /api/oauth/google/callback` | Where Google sends the browser back. Needs no token: the single-use `state` cedar issued for the admin's "Connect Google" is the check. Redirects to `/?integrations=connected` or `/?integrations=error&message=…` |

A `trigger` is `{"kind", ...}`:

| `kind` | Fields |
| --- | --- |
| `time` | `frequency`: `daily` \| `weekdays` \| `weekly`, `weekday`?: 0–6 (Monday–Sunday, weekly only), `time`: `HH:MM`, `timezone`: an IANA name |
| `once` | `at`: `YYYY-MM-DDTHH:MM`, local to `timezone`; `timezone`. In the future |
| `interval` | `every_minutes`: 15 to 10080 |
| `monthly` | `day`: 1–31, `time`, `timezone` |
| `condition` | `source`: `{"type": "url", "url", "as"?: "json" \| "text"}`, `{"type": "tool", "name", "input"}` (a tool `GET /api/tools` marks `pollable`) or `{"type": "records", "set"}` (seen as `{count, updated_at, records}`); `extract`?: a JSON pointer (`/price/eur`) or dotted path (`price.eur`); `predicate`: `{"op": "changes" \| "gt" \| "lt" \| "gte" \| "lte" \| "equals" \| "contains", "value"?}`; `every_minutes`: 1 to 10080 (5 unless given; at least 5 for a personal tool) |
| `webhook` | none |
| `email` | `query`: a Gmail search; `every_minutes` |
| `calendar` | `calendar_id`?, `filter`?, `minutes_before`?, `new_events`?: at least one of the last two; `every_minutes` |
| `records` | `set`: a record set's name; `on`: `added` (the default) or `changed` (added or changed); `every_minutes` |
| `drive` | `folder`: a Google Drive folder's ID or its address; `every_minutes` |
| `device` | none; started through `POST /api/agents/{id}/trigger` |

## Tests

```sh
cargo test -p cedar-chat
```

The integration tests in `tests/api.rs` run the chat API against a fake cedar server.
