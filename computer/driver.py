#!/usr/bin/env python3
"""A desktop on a virtual screen, for cedar's `computer` tool. It is installed in the computer
image as `cedar-computer`:

    cedar-computer serve            start the screen and the desktop, and keep them running (the
                                    container's command)
    cedar-computer call JSON        do one thing on the screen and print what it looks like after

A request is a JSON object with an `action`. Positions are pixels on the screen, from its top
left corner:

    {"action": "screenshot"}                            only look
    {"action": "click", "x": 640, "y": 400}             also "button": "left" (default), "right",
                                                        "middle", and "count": 1 to 3
    {"action": "move", "x": 640, "y": 400}              move the pointer, e.g. to open a tooltip
    {"action": "drag", "x": 100, "y": 100, "to_x": 300, "to_y": 200}
                                                        press the left button at x,y, release at to
    {"action": "type", "text": "..."}                   type into whatever has the keyboard
    {"action": "key", "keys": "ctrl+s"}                 press keys: xdotool names, several
                                                        separated by spaces ("ctrl+a BackSpace")
    {"action": "scroll", "direction": "down", "amount": 3, "x": 640, "y": 400}
                                                        turn the wheel (also "up", "left",
                                                        "right"), at x,y if given
    {"action": "wait", "seconds": 2}                    let the screen catch up
    {"action": "open", "path": "report.ods"}            open a workspace file with its application

Any request may carry `width` and `height`: the size the screenshot is scaled to.

The reply is one JSON object on stdout: `{"image", "media_type", "width", "height", "screen",
"pointer", "window"}`. `image` is the screen after the action as a base64 PNG, pointer included;
`width` and `height` are its size, `screen` the screen's own `[width, height]`, `pointer` where
the pointer is on the screen, and `window` the title of the window that has the keyboard (null
if none has). A failure prints its reason to stderr and exits 1.

The screen is an Xvfb display of CEDAR_SCREEN (1280x800 unless set); the desktop on it is
CEDAR_COMPUTER_SESSION (Xfce unless set). Nothing is kept between containers.
"""

import base64
import io
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import time

DISPLAY = os.environ.setdefault("DISPLAY", ":1")
SESSION = os.environ.get("CEDAR_COMPUTER_SESSION", "dbus-run-session -- startxfce4")
WORKSPACE = os.environ.get("CEDAR_WORKSPACE", "/workspace")
STARTUP_WAIT_SECS = 15
# How long the screen gets to react before it is photographed.
SETTLE_SECS = 0.6
OPEN_SETTLE_SECS = 2.5
MAX_WAIT_SECS = 10
MAX_TEXT_CHARS = 10_000
MAX_KEYS = 20
MAX_SCROLL = 30
COMMAND_TIMEOUT_SECS = 30
BUTTONS = {"left": "1", "middle": "2", "right": "3"}
WHEEL = {"up": "4", "down": "5", "left": "6", "right": "7"}
KEY = re.compile(r"^[A-Za-z0-9_]+(\+[A-Za-z0-9_]+)*$")


class Refused(Exception):
    """The request can't be carried out; the message is for the model."""


def screen_size():
    """The configured size of the screen as (width, height)."""
    match = re.fullmatch(r"(\d+)x(\d+)", os.environ.get("CEDAR_SCREEN", "1280x800"))
    if not match:
        raise Refused("CEDAR_SCREEN must look like 1280x800")
    return int(match.group(1)), int(match.group(2))


def xdotool(*args, stdin=None):
    done = subprocess.run(
        ["xdotool", *args],
        input=stdin,
        capture_output=True,
        text=True,
        timeout=COMMAND_TIMEOUT_SECS,
    )
    if done.returncode != 0:
        raise Refused(done.stderr.strip() or f"xdotool {args[0]} failed")
    return done.stdout.strip()


def integer(request, name, low, high, default=None):
    value = request.get(name, default)
    if isinstance(value, bool) or not isinstance(value, int) or not low <= value <= high:
        raise Refused(f"`{name}` must be a whole number from {low} to {high}")
    return value


def point(request, screen, names=("x", "y")):
    """A position on the screen from the request, as two strings for xdotool."""
    x = integer(request, names[0], 0, screen[0] - 1)
    y = integer(request, names[1], 0, screen[1] - 1)
    return str(x), str(y)


def act(request, screen):
    """Carry out the request's action; returns how long the screen gets to settle."""
    action = request.get("action")
    if action in ("screenshot", "wait"):
        if action == "wait":
            seconds = request.get("seconds", 1)
            if isinstance(seconds, bool) or not isinstance(seconds, (int, float)):
                raise Refused("`seconds` must be a number")
            time.sleep(min(max(seconds, 0), MAX_WAIT_SECS))
        return 0
    if action == "click":
        button = BUTTONS.get(request.get("button", "left"))
        if button is None:
            raise Refused("`button` must be left, right or middle")
        count = integer(request, "count", 1, 3, 1)
        x, y = point(request, screen)
        xdotool("mousemove", "--sync", x, y, "click", "--repeat", str(count), "--delay", "80", button)
    elif action == "move":
        xdotool("mousemove", "--sync", *point(request, screen))
    elif action == "drag":
        x, y = point(request, screen)
        to_x, to_y = point(request, screen, ("to_x", "to_y"))
        # A stop on the way, since many applications only start a drag once the pointer has moved
        # with the button down.
        mid = (str((int(x) + int(to_x)) // 2), str((int(y) + int(to_y)) // 2))
        xdotool(
            "mousemove", "--sync", x, y, "mousedown", "1", "sleep", "0.15",
            "mousemove", "--sync", *mid, "sleep", "0.1",
            "mousemove", "--sync", to_x, to_y, "sleep", "0.15", "mouseup", "1",
        )  # fmt: skip
    elif action == "type":
        text = request.get("text")
        if not isinstance(text, str) or not text:
            raise Refused("type needs `text`")
        if len(text) > MAX_TEXT_CHARS:
            raise Refused(f"`text` is longer than {MAX_TEXT_CHARS} characters; type it in parts")
        xdotool("type", "--clearmodifiers", "--delay", "12", "--file", "-", stdin=text)
    elif action == "key":
        keys = request.get("keys")
        keys = keys.split() if isinstance(keys, str) else []
        if not keys or len(keys) > MAX_KEYS or not all(KEY.match(k) for k in keys):
            raise Refused(
                "key needs `keys`: names like Return, Escape, ctrl+s or alt+F4, "
                f"at most {MAX_KEYS}, separated by spaces"
            )
        xdotool("key", "--clearmodifiers", "--delay", "60", "--", *keys)
    elif action == "scroll":
        button = WHEEL.get(request.get("direction", "down"))
        if button is None:
            raise Refused("`direction` must be up, down, left or right")
        amount = integer(request, "amount", 1, MAX_SCROLL, 3)
        if "x" in request or "y" in request:
            xdotool("mousemove", "--sync", *point(request, screen))
        xdotool("click", "--repeat", str(amount), "--delay", "30", button)
    elif action == "open":
        path = request.get("path")
        if not isinstance(path, str) or not path:
            raise Refused("open needs `path`, a file in the workspace")
        full = os.path.realpath(os.path.join(WORKSPACE, path))
        if os.path.commonpath([full, os.path.realpath(WORKSPACE)]) != os.path.realpath(WORKSPACE):
            raise Refused("`path` must be inside the workspace")
        if not os.path.exists(full):
            raise Refused(f"{path} does not exist in the workspace")
        subprocess.Popen(
            ["xdg-open", full],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        return OPEN_SETTLE_SECS
    else:
        raise Refused(
            f"unknown action {action!r}; use screenshot, click, move, drag, type, key, scroll, "
            "wait, or open"
        )
    return SETTLE_SECS


def screenshot(width, height):
    """The screen as PNG bytes with the pointer drawn in, scaled to width x height."""
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "screen.png")
        shot = subprocess.run(
            ["scrot", "--pointer", path],
            capture_output=True,
            text=True,
            timeout=COMMAND_TIMEOUT_SECS,
        )
        if shot.returncode != 0:
            raise Refused(shot.stderr.strip() or "the screen could not be captured")
        with open(path, "rb") as f:
            png = f.read()
    from PIL import Image

    image = Image.open(io.BytesIO(png))
    if image.size == (width, height):
        return png
    out = io.BytesIO()
    image.convert("RGB").resize((width, height), Image.LANCZOS).save(out, "PNG", optimize=True)
    return out.getvalue()


def call(request):
    if not isinstance(request, dict):
        raise Refused("the request must be a JSON object")
    screen = screen_size()
    width = integer(request, "width", 16, screen[0], screen[0])
    height = integer(request, "height", 16, screen[1], screen[1])
    settle = act(request, screen)
    time.sleep(settle)
    png = screenshot(width, height)
    where = xdotool("getmouselocation", "--shell")
    pointer = dict(line.split("=", 1) for line in where.splitlines() if "=" in line)
    try:
        window = xdotool("getactivewindow", "getwindowname") or None
    except (Refused, subprocess.TimeoutExpired):
        window = None
    return {
        "image": base64.b64encode(png).decode(),
        "media_type": "image/png",
        "width": width,
        "height": height,
        "screen": list(screen),
        "pointer": [int(pointer.get("X", 0)), int(pointer.get("Y", 0))],
        "window": window,
    }


def serve():
    width, height = screen_size()
    xvfb = subprocess.Popen(
        ["Xvfb", DISPLAY, "-screen", "0", f"{width}x{height}x24", "-nolisten", "tcp"]
    )
    deadline = time.monotonic() + STARTUP_WAIT_SECS
    while subprocess.run(["xdotool", "getdisplaygeometry"], capture_output=True).returncode != 0:
        if xvfb.poll() is not None or time.monotonic() > deadline:
            sys.exit("the screen did not start")
        time.sleep(0.1)
    subprocess.Popen(shlex.split(SESSION), stdin=subprocess.DEVNULL, cwd=os.path.expanduser("~"))
    # The container lives as long as its screen does.
    sys.exit(xvfb.wait())


def main():
    if sys.argv[1:] == ["serve"]:
        serve()
    elif len(sys.argv) == 3 and sys.argv[1] == "call":
        try:
            reply = call(json.loads(sys.argv[2]))
        except json.JSONDecodeError as e:
            sys.exit(f"the request is not JSON: {e}")
        except Refused as e:
            sys.exit(str(e))
        except subprocess.TimeoutExpired:
            sys.exit("the screen did not respond")
        json.dump(reply, sys.stdout)
    else:
        sys.exit("usage: cedar-computer serve | cedar-computer call JSON")


if __name__ == "__main__":
    main()
