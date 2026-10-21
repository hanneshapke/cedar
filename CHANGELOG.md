# Changelog

## 0.1.1

- **Fix: the Mac app's chat now starts.** In 0.1.0, cedar-chat loaded OpenSSL from Homebrew, which macOS refused (or which wasn't there), so the chat didn't come up. OpenSSL is now built into it (#63).
- **Cedar for Windows:** a tray app with an MSI installer for x64, alongside the Mac app. It is not code-signed yet, so SmartScreen warns on first run (#62).
- The app's source moved from `macos/` to `desktop/` (#61).
- iOS: attach photos and PDFs to a message (#35), and answer the agent's clarifying questions in a form (#53).
- Linux: pair the iOS app by QR code from `cedar-stack.sh --pair` (#54).
- Registry: new tools `dep_audit`, `arxiv_search` and `air_quality` (#57).

## 0.1.0

First release: the Cedar Mac app (Apple Silicon), the sandbox, browser images, Python tools, iOS app connection.
