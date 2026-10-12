#!/usr/bin/env python3
"""A headless Chromium that keeps its state between calls, for cedar's `browser` tool. It is
installed in the browser image as `cedar-browser`:

    cedar-browser serve             start Chromium and wait for calls (the container's command)
    cedar-browser call JSON         send one request to the running server and print the reply

A request is a JSON object with an `action`:

    {"action": "navigate", "url": "https://..."}    load a page, then take a snapshot
    {"action": "snapshot"}                          snapshot the current page again

The reply is one JSON object on stdout: `{"url", "title", "status", "text", "visible_chars"}`. `status` is the HTTP
status of the last navigation's main document (null if there was none). `text` is the page as
readable text, with interactive elements numbered like `[12] button "Show all reviews"`; the
numbers are stored on the elements (`data-cedar-ref`) so later actions can address them. When a
modal dialog is open, only the dialog is described. A failure prints its reason to stderr and
exits 1.

The server holds one page in one fresh browser context (no cookies or sign-ins from anywhere
else) and handles one call at a time over a Unix socket. Only http(s) navigations are allowed,
downloads are refused, and images, media, and fonts are not loaded. If CEDAR_BROWSER_PROXY is
set, all traffic goes through that proxy.
"""

import asyncio
import json
import os
import sys

SOCKET = os.environ.get("CEDAR_BROWSER_SOCKET", "/tmp/cedar-browser.sock")
NAVIGATE_TIMEOUT_MS = 30_000
# After the load event, wait this long at most for the page's own requests to finish, since many
# sites render their content from API calls made after load and some never go fully idle.
SETTLE_TIMEOUT_MS = 3_000
MAX_REQUEST_BYTES = 1_000_000
BLOCKED_RESOURCES = {"image", "media", "font"}

# Walks the DOM (and open shadow roots) and returns the page as text: block elements on their own
# lines, headings as Markdown headings, list items as "- ", and each visible interactive element
# as "[n] role "name"", numbered in document order. An interactive element with a short label is
# described on one line; one that wraps a lot of content (a listing card that is one big link) gets
# a line of its own and its content follows as ordinary text.
SNAPSHOT_JS = r"""
() => {
  const SKIP = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "SVG", "CANVAS", "IFRAME", "HEAD"]);
  const ROLES = new Set(["button", "link", "checkbox", "radio", "switch", "tab", "menuitem",
                         "option", "combobox", "textbox", "searchbox", "slider"]);
  const MAX_NAME = 150;
  const clean = (s) => (s || "").replace(/\s+/g, " ").trim();

  document.querySelectorAll("[data-cedar-ref]").forEach((el) => el.removeAttribute("data-cedar-ref"));

  // What is on screen, not what screen readers get: sites set aria-hidden on their whole app while
  // an overlay (a cookie banner, a dialog) is open, and on visible text that has a spoken twin.
  // An element that isn't rendered (display: none) takes its subtree with it, but one with
  // `visibility: hidden` doesn't: its children can be visible again, so that is checked per element.
  // checkVisibility() is false for `display: contents`, which has no box of its own although its
  // children render normally (Airbnb wraps its whole app in one), so those are walked into.
  const rendered = (el) =>
    !el.checkVisibility || el.checkVisibility() || getComputedStyle(el).display === "contents";
  const shown = (el) => rendered(el) && getComputedStyle(el).visibility === "visible";

  const roleOf = (el) => {
    const explicit = (el.getAttribute("role") || "").split(" ")[0];
    if (ROLES.has(explicit)) return explicit;
    const tag = el.tagName;
    if (tag === "A" && el.hasAttribute("href")) return "link";
    if (tag === "BUTTON" || tag === "SUMMARY") return "button";
    if (tag === "SELECT") return "combobox";
    if (tag === "TEXTAREA") return "textbox";
    if (tag === "INPUT") {
      const type = (el.getAttribute("type") || "text").toLowerCase();
      if (type === "hidden") return null;
      if (["button", "submit", "reset", "image"].includes(type)) return "button";
      if (type === "checkbox" || type === "radio") return type;
      if (type === "range") return "slider";
      return type === "search" ? "searchbox" : "textbox";
    }
    if (el.isContentEditable && !el.parentElement?.isContentEditable) return "textbox";
    return null;
  };

  const labelOf = (el) => {
    const labelledBy = el.getAttribute("aria-labelledby");
    if (labelledBy) {
      const text = labelledBy.split(/\s+/).map((id) => document.getElementById(id)?.innerText || "").join(" ");
      if (clean(text)) return clean(text);
    }
    if (el.labels && el.labels.length) return clean(el.labels[0].innerText);
    return clean(el.getAttribute("aria-label") || "");
  };

  // The dialog to describe instead of the page, if a modal one is open (the last one wins).
  const modal = [...document.querySelectorAll('dialog:modal, [aria-modal="true"]')].filter(shown).pop();
  const root = modal || document.body;

  const lines = [];
  let line = "";
  let next = 1;
  const flush = () => {
    const text = clean(line);
    if (text && lines[lines.length - 1] !== text) lines.push(text);
    line = "";
  };
  const emit = (text) => { flush(); line = text; flush(); };
  const href = (el) => {
    const raw = el.getAttribute("href") || "";
    if (!raw || raw.startsWith("javascript:") || raw === "#") return "";
    try {
      const url = new URL(raw, location.href);
      return url.origin === location.origin ? url.pathname + url.search + url.hash : url.href;
    } catch { return ""; }
  };

  // `visible`: whether text directly inside `node` is visible (`visibility` of its element).
  const walk = (node, visible) => {
    if (node.nodeType === Node.TEXT_NODE) {
      if (visible) line += node.nodeValue;
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE && node.nodeType !== Node.DOCUMENT_FRAGMENT_NODE) return;
    if (node.nodeType === Node.ELEMENT_NODE) {
      const el = node;
      if (SKIP.has(el.tagName.toUpperCase()) || !rendered(el)) return;
      const style = getComputedStyle(el);
      visible = style.visibility === "visible";
      const role = visible ? roleOf(el) : null;
      if (role) {
        const ref = next++;
        el.setAttribute("data-cedar-ref", String(ref));
        const text = clean(el.innerText || el.textContent);
        let name = labelOf(el) || (text.length <= MAX_NAME ? text : "")
          || clean(el.getAttribute("placeholder") || el.getAttribute("title") || el.getAttribute("alt") || "");
        let desc = `[${ref}] ${role}` + (name ? ` "${name}"` : "");
        if (role === "link" && href(el)) desc += ` -> ${href(el)}`;
        if (["textbox", "searchbox", "combobox", "slider"].includes(role) && "value" in el && el.value) {
          desc += ` value="${clean(String(el.value)).slice(0, MAX_NAME)}"`;
        }
        if ((role === "checkbox" || role === "radio") && "checked" in el) desc += el.checked ? " (checked)" : "";
        if (el.getAttribute("aria-expanded")) desc += el.getAttribute("aria-expanded") === "true" ? " (expanded)" : " (collapsed)";
        if (el.disabled || el.getAttribute("aria-disabled") === "true") desc += " (disabled)";
        emit(desc);
        // Short labels are already in the description; long content is described on below.
        if (text.length <= MAX_NAME || ["textbox", "searchbox", "combobox"].includes(role)) return;
      }
      const display = style.display;
      const block = !display.startsWith("inline") && display !== "contents";
      const heading = /^H[1-6]$/.test(el.tagName) ? Number(el.tagName[1]) : 0;
      if (heading && !role && visible) {
        const text = clean(el.innerText);
        if (text) emit("#".repeat(heading) + " " + text);
        return;
      }
      if (block || el.tagName === "BR") flush();
      if (el.tagName === "LI") line += "- ";
      if (el.tagName === "IMG" && visible && !role && clean(el.getAttribute("alt"))) line += ` [image: ${clean(el.getAttribute("alt"))}] `;
      if (el.shadowRoot) walk(el.shadowRoot, visible);
      for (const child of el.childNodes) walk(child, visible);
      if (block) flush();
      else line += " ";
      return;
    }
    for (const child of node.childNodes) walk(child, visible);
  };

  walk(root, shown(root));
  flush();
  return { dialog: Boolean(modal), text: lines.join("\n"), visibleChars: (document.body?.innerText || "").length };
}
"""


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


class Browser:
    def __init__(self, page):
        self.page = page
        self.status = None

    async def handle(self, request):
        action = request.get("action")
        if action == "navigate":
            await self.navigate(request.get("url"))
        elif action != "snapshot":
            raise ValueError(f"unknown action {action!r}; expected navigate or snapshot")
        return await self.snapshot()

    async def navigate(self, url):
        if not isinstance(url, str) or not url.strip():
            raise ValueError("navigate needs a url")
        url = url.strip()
        if not url.lower().startswith(("http://", "https://")):
            raise ValueError(f"{url} is not an http or https URL")
        self.status = None
        response = await self.page.goto(url, wait_until="load", timeout=NAVIGATE_TIMEOUT_MS)
        self.status = response.status if response else None
        await self.settle()

    async def settle(self):
        try:
            await self.page.wait_for_load_state("networkidle", timeout=SETTLE_TIMEOUT_MS)
        except Exception:
            pass  # still busy: describe what is there now

    async def snapshot(self):
        result = await self.page.evaluate(SNAPSHOT_JS)
        text = result["text"]
        if result["dialog"]:
            text = "[a dialog is open; only the dialog is shown]\n\n" + text
        return {
            "url": self.page.url,
            "title": await self.page.title(),
            "status": self.status,
            "text": text,
            # The length of the page's rendered text, as a check on the snapshot: a snapshot far
            # shorter than this (with no dialog open) means the walk above missed content.
            "visible_chars": result["visibleChars"],
        }


async def route(route):
    request = route.request
    if not request.url.lower().startswith(("http://", "https://", "data:", "blob:")):
        await route.abort("blockedbyclient")
    elif request.resource_type in BLOCKED_RESOURCES:
        await route.abort("blockedbyclient")
    else:
        await route.continue_()


async def serve():
    from playwright.async_api import async_playwright

    launch = {"headless": True, "args": ["--disable-dev-shm-usage"]}
    if os.environ.get("CEDAR_BROWSER_PROXY"):
        launch["proxy"] = {"server": os.environ["CEDAR_BROWSER_PROXY"], "bypass": ""}
    if os.environ.get("CEDAR_BROWSER_EXECUTABLE"):
        launch["executable_path"] = os.environ["CEDAR_BROWSER_EXECUTABLE"]

    async with async_playwright() as p:
        chromium = await p.chromium.launch(**launch)
        context = await chromium.new_context(
            accept_downloads=False,
            viewport={"width": 1280, "height": 900},
            locale="en-US",
        )
        await context.route("**/*", route)
        page = await context.new_page()
        # Close popups instead of following them: one page per run keeps the refs meaningful.
        context.on("page", lambda popup: asyncio.ensure_future(popup.close()) if popup != page else None)
        browser = Browser(page)
        lock = asyncio.Lock()

        async def client(reader, writer):
            try:
                raw = await reader.readline()
                async with lock:
                    try:
                        request = json.loads(raw)
                        if not isinstance(request, dict):
                            raise ValueError("the request must be a JSON object")
                        reply = {"ok": True, "result": await browser.handle(request)}
                    except Exception as e:  # reported to the caller, the server keeps running
                        reply = {"ok": False, "error": error_text(e)}
                writer.write(json.dumps(reply).encode() + b"\n")
                await writer.drain()
            finally:
                writer.close()

        if os.path.exists(SOCKET):
            os.unlink(SOCKET)
        server = await asyncio.start_unix_server(client, path=SOCKET, limit=MAX_REQUEST_BYTES)
        print(f"cedar-browser: Chromium {chromium.version} listening on {SOCKET}", flush=True)
        async with server:
            await server.serve_forever()


def error_text(e):
    """The first line of an exception, which for Playwright errors is the useful part."""
    text = str(e).strip().splitlines()
    return text[0] if text else type(e).__name__


async def call(raw):
    try:
        reader, writer = await asyncio.open_unix_connection(SOCKET, limit=64 * 1024 * 1024)
    except OSError as e:
        fail(f"the browser is not running ({e})")
    writer.write(raw.encode() + b"\n")
    await writer.drain()
    line = await reader.readline()
    writer.close()
    if not line:
        fail("the browser closed the connection without replying")
    reply = json.loads(line)
    if not reply.get("ok"):
        fail(reply.get("error") or "the browser call failed")
    print(json.dumps(reply["result"], ensure_ascii=False))


def main():
    if sys.argv[1:] == ["serve"]:
        asyncio.run(serve())
    elif len(sys.argv) == 3 and sys.argv[1] == "call":
        try:
            request = json.loads(sys.argv[2])
        except ValueError as e:
            fail(f"the request is not valid JSON: {e}")
        asyncio.run(call(json.dumps(request)))  # one line, whatever the caller's formatting
    else:
        fail("usage: cedar-browser serve | cedar-browser call JSON")


if __name__ == "__main__":
    main()
