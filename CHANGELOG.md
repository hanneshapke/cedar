# Changelog

## 0.1.3

- **Starred conversations.** Star a chat or a scheduled run from its header; the starred ones are listed under Starred at the top of the sidebar. Stars are kept by the chat server, so the list is the same on the web, in the Mac and Windows app and in the iOS app (#72).
- **Settings → iPhone.** The chat's HTTPS address, access token and the QR code the iOS app scans, with a check of what Tailscale on this computer still needs (#71).
- iOS: switch the sidebar between Chats, Starred and Scheduled; scheduled agents list their runs, with Run now, Pause and Resume (#69, #72). Fix: opening a scheduled run no longer shows an empty chat marked "cancelled" (#69).
- Landing page: Cedar for Windows, download buttons that follow the visitor's OS (#66), and a Cedar vs. comparison picker (#68).

## 0.1.2

- **Fix: Cedar for Windows starts its servers on PCs without the Visual C++ Redistributable.** In 0.1.1, cedar.exe and cedar-chat.exe loaded `VCRUNTIME140.dll`, which Windows doesn't ship, so they failed with "VCRUNTIME140.dll was not found". The C runtime is now linked in, and the release checks that no executable needs the DLL.
- **Fix: the Windows window no longer goes blank.** Opening Cedar from the tray (or starting it again) while the servers weren't running navigated the window to `about:blank` instead of the setup page.

## 0.1.1

- **Fix: the Mac app's chat now starts.** In 0.1.0, cedar-chat loaded OpenSSL from Homebrew, which macOS refused (or which wasn't there), so the chat didn't come up. OpenSSL is now built into it (#63).
- **Cedar for Windows:** a tray app with an MSI installer for x64, alongside the Mac app. It is not code-signed yet, so SmartScreen warns on first run (#62).
- The app's source moved from `macos/` to `desktop/` (#61).
- iOS: attach photos and PDFs to a message (#35), and answer the agent's clarifying questions in a form (#53).
- Linux: pair the iOS app by QR code from `cedar-stack.sh --pair` (#54).
- Registry: new tools `dep_audit`, `arxiv_search` and `air_quality` (#57).

## 0.1.0

First release: the Cedar Mac app (Apple Silicon), the sandbox, browser images, Python tools, iOS app connection.
