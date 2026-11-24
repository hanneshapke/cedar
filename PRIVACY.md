# Privacy Policy

_Last updated: November 24, 2026_

**cedar does not collect any data. Period.**

This applies to the cedar server and to every cedar app: iOS, Android, desktop and the web chat. The one service we run, Cedar Private Inference, is optional: you choose it instead of running a model yourself, and it keeps only what it needs to sell you credit (see [Cedar Private Inference](#cedar-private-inference-optional)).

## What that means

- We don't collect, store, sell or share any personal data.
- There are no analytics, no telemetry, no crash reporting, no ads and no tracking of any kind.
- Using cedar needs no account with us. The only service we run is the optional Cedar Private Inference, and your conversations don't pass through it either.

## Where your data goes

cedar is self-hosted. The apps talk only to the cedar server **you** run and pair them with. Your conversations, runs, files and settings stay on that server and on your own devices. We never see them.

To find out whether a newer version is out, the Mac and Windows app and the web chat ask GitHub, where cedar's releases are published, every few hours. The request carries the installed version and nothing about you or your conversations; GitHub sees it like any visit to a public page. The setup page (desktop) or `CHAT_UPDATE_CHECK=off` (chat server) turns it off.

Only when you click *Set up Docker for me* or *Set up Tailscale for me*, the Mac and Windows app downloads that program's installer from its maker (Docker Desktop from desktop.docker.com, Tailscale from pkgs.tailscale.com) and installs it. Like any download, the request carries nothing about you or your conversations. `scripts/cedar-stack.sh` does the same on Linux (get.docker.com, tailscale.com) when you answer yes. Docker and Tailscale are then governed by their own privacy policies.

Anything you choose to connect cedar to (for example your LLM server, or a third-party tool you install from the community registry) is governed by that service's own privacy policy, not by us.

## Cedar Private Inference (optional)

Instead of running a model yourself, you can connect Cedar Private Inference on the desktop app's setup page, in the web chat's Settings, or with `cedar private-inference connect`. It sells prepaid credit for a hosted model and starts you with a free trial. If you never connect it, nothing in this section applies to you.

- **Your conversations go straight to the inference provider, never through us.** The service hands your cedar server a key for the provider, Doubleword (doubleword.ai), and from then on your cedar server sends every model call to Doubleword directly: your messages, the system prompt, your memory as far as a run is shown it, and what the agent's tools return. Doubleword runs the models on GPUs it rents from hosting providers. The key's account has zero data retention turned on, so Doubleword does not keep the prompts and replies of these calls; its own privacy policy applies to what it receives.
- **What the service keeps:** your email address (to sign you in, with a link sent to it), your credit (the trial, your payments and what you have spent), and your key. Payments go through Stripe; we never see your card. What you spent comes from Doubleword as totals per key, in tokens and money, without any content. The service keeps none of your conversations, prompts, replies, files or memory.
- **What a cedar app sends it**, only while you connect: a random one-time code and a name for the computer or server you connect (its computer name, the chat's address or the server's host name), shown on the page where you approve the connection.
- **Its website** sets a cookie that keeps you signed in, and no other.
- **Ending it.** Disconnecting removes the models and the key from your cedar. The key stays valid until you revoke it on your account page. To have your account and everything kept with it deleted, ask us (see [Contact](#contact)).

This is not yet verified by hardware: today it rests on Doubleword's zero-retention commitment. Cedar will say so on the model when a provider can prove, through attestation that you can check yourself, that your prompts are processed only inside verified hardware.

## Memory

cedar can remember facts about you across conversations, such as your name, your preferences or a project you're working on, so you don't have to repeat them. **You stay in control of your memory, and it is never shared with a third party.**

- Your memory is stored only in the database of the cedar server **you** run. We never see it, and cedar never sends it to us or to anyone else.
- The only place it goes is the LLM server you configured, as part of each request, like the rest of your conversation. If that is a hosted model provider rather than your own server, its privacy policy applies to what it receives.
- cedar keeps facts, preferences and standing instructions you stated, each with the words of yours it was taken from, a note on what each conversation was about, ways of working that went well, and a short profile of you. A fact is kept only when it quotes your own words from the conversation, never something a tool, a web page, an email or a document said, and text that looks like a password, a key or a card, account or government number is refused.
- You can see all of it, the conversation each item came from, when it was last used and how it changed over time, and edit, pin, archive or delete one item or everything at any time in Settings → Memory (web chat and desktop; the iOS and Android apps list and delete) or through cedar's API (`/v1/memory`). A run's trace says exactly which memories it was shown.
- A deleted memory is removed from the database right away, with its history, and is not used again, starting with your very next message. An archived one leaves every prompt and stays until you delete it.
- You can turn memory off under Settings → Tools → Memory, and whoever runs the server can turn it off altogether (`MEMORY=off`).

## Contributing a conversation (opt-in)

cedar can let you *give* one conversation to a shared corpus used to train open-weight agent models. This is a proposal in progress (see `docs/design/contributions.md`); what exists today is described here so the first sentence of this policy stays true as it is built.

- **Nothing is sent unless you send it.** Your cedar-chat server knows one corpus, the project's own, unless whoever runs it named another or turned the feature off (`CHAT_CONTRIBUTIONS_URL`); knowing it sends nothing. You choose, once, in Settings → Contribute, whether to contribute at all, a license and whether to use a random pseudonym. After that, every contribution is one conversation, chosen by you, after a page that shows exactly what will leave your server. There is no "share everything" switch, no background collection, and no reminders.
- **What a contribution contains.** Your prompts, the agent's replies and tool calls, the system prompt and tool definitions it ran with, how the run ended, and the outcome you reported. Before you see it, cedar-chat removes attachments' contents and names, your memory list, run and key identifiers, timestamps (keeping the day), and your region (keeping the language), and replaces emails, phone and account numbers, IP addresses, keys and home-directory names with made-up stand-ins of the same shape (an address at example.com, a 555 number, a documentation IP address, an invented name), the same stand-in for the same value within that conversation and never derived from the value. The contribution lists which values are stand-ins. Conversations that used your Gmail or Calendar, or a health tool, cannot be contributed at all.
- **Redaction is best effort.** The scan catches what has a shape. A fact stated in your own words is not caught; the review page is where you read for that, and it lets you redact more and restore what the scan got wrong.
- **No identity.** A contribution carries no account, email, key, device or address of yours. Its pseudonym, if you chose one, is a random id generated on your server that you can rotate or drop. The corpus operator sees the connection like any web server does and commits in its own policy not to keep request logs.
- **You can withdraw.** Your server keeps a receipt for each contribution, with a token only it knows, and Settings → Contribute lets you withdraw one. A withdrawn contribution is deleted from the corpus's intake and left out of every later release; a release already published is an immutable snapshot and cannot be recalled from people who downloaded it.
- **Licensing.** You license what you wrote under the license you chose (CC0 or CC BY 4.0). The agent's replies are the output of the model your server ran; the corpus publishes only models whose terms allow it.

## Device permissions

The apps ask for a few permissions, used only on your device and only for the following:

- **Camera** – to scan the QR code that pairs the app with your cedar server. Images are never stored or sent anywhere.
- **Local network / internet** – to reach your own cedar server.
- **Notifications** (Android) – to tell you when a run finishes.

## Children

The cedar software collects no data, so it collects none from children either. Cedar Private Inference is meant for adults, and we don't knowingly keep a child's email address.

## Changes

If this policy ever changes, the update will be published in this file in the repository.

## Contact

Questions? Open an issue at https://github.com/hanneshapke/cedar/issues.
