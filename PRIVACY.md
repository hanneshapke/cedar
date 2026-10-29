# Privacy Policy

_Last updated: October 29, 2026_

**cedar does not collect any data. Period.**

This applies to the cedar server and to every cedar app: iOS, Android, desktop and the web chat.

## What that means

- We don't collect, store, sell or share any personal data.
- There are no analytics, no telemetry, no crash reporting, no ads and no tracking of any kind.
- There are no accounts with us, and there is no cedar cloud service. We don't run any servers that your data passes through.

## Where your data goes

cedar is self-hosted. The apps talk only to the cedar server **you** run and pair them with. Your conversations, runs, files and settings stay on that server and on your own devices. We never see them.

To find out whether a newer version is out, the Mac and Windows app and the web chat ask GitHub, where cedar's releases are published, every few hours. The request carries the installed version and nothing about you or your conversations; GitHub sees it like any visit to a public page. The setup page (desktop) or `CHAT_UPDATE_CHECK=off` (chat server) turns it off.

Only when you click *Set up Docker for me* or *Set up Tailscale for me*, the Mac and Windows app downloads that program's installer from its maker (Docker Desktop from desktop.docker.com, Tailscale from pkgs.tailscale.com) and installs it. Like any download, the request carries nothing about you or your conversations. `scripts/cedar-stack.sh` does the same on Linux (get.docker.com, tailscale.com) when you answer yes. Docker and Tailscale are then governed by their own privacy policies.

Anything you choose to connect cedar to (for example your LLM server, or a third-party tool you install from the community registry) is governed by that service's own privacy policy, not by us.

## Memory

cedar can remember facts about you across conversations, such as your name, your preferences or a project you're working on, so you don't have to repeat them. **You stay in control of your memory, and it is never shared with a third party.**

- Your memory is stored only in the database of the cedar server **you** run. We never see it, and cedar never sends it to us or to anyone else.
- The only place it goes is the LLM server you configured, as part of each request, like the rest of your conversation. If that is a hosted model provider rather than your own server, its privacy policy applies to what it receives.
- You can see every remembered fact, and the conversation it came from, and delete one or all of them at any time in Settings → Memory (web chat, desktop, iOS and Android) or through cedar's API (`GET` and `DELETE /v1/memory`). What you see there is exactly what the agent sees.
- A deleted memory is removed from the database right away and is not used again, starting with your very next message.
- You can turn memory off under Settings → Tools → Memory, and whoever runs the server can turn it off altogether (`MEMORY=off`).

## Device permissions

The apps ask for a few permissions, used only on your device and only for the following:

- **Camera** – to scan the QR code that pairs the app with your cedar server. Images are never stored or sent anywhere.
- **Local network / internet** – to reach your own cedar server.
- **Notifications** (Android) – to tell you when a run finishes.

## Children

Because we collect no data, we collect no data from children either.

## Changes

If this policy ever changes, the update will be published in this file in the repository.

## Contact

Questions? Open an issue at https://github.com/hanneshapke/cedar/issues.
