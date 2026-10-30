"""A scripted OpenAI-compatible "model" for smoke-testing the harness end to end: real cedar, real
tools, the fixture server, the runner and the graders, without a GPU. It recognizes a few seed
tasks by their prompt and makes the tool calls a good agent would, then answers from the tool
results. It is not a baseline; it only proves the plumbing.

    python bench/scripts/fake_llm.py --port 8001
    # register it: base_url http://127.0.0.1:8001/v1, model "scripted"
"""

import argparse
import json
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def call(name, args):
    return {"name": name, "args": args}


def script(prompt: str, results: list[str], answered: str | None):
    """The next step: a tool call, or the final text. `results` are the tool results so far."""
    p = prompt.lower()
    n = len(results)
    if "halvorsen" in p:
        if n == 0:
            return call("web_search", {"query": "Halvorsen Bridge Port Alder opened designer"})
        if n == 1:
            url = re.search(r"https?://\S+halvorsen-bridge\.html", results[0]).group(0)
            return call("fetch_url", {"url": url})
        year = "1931" if "1931" in results[1] else "unknown"
        return f"It opened to traffic on 14 May {year}; it was designed by Ingrid Solberg [2]."
    if "lisbon" in p:
        if n == 0:
            return call("weather_forecast", {"location": "Lisbon", "days": 7})
        return "Yes, bring an umbrella: rain is forecast on Saturday." if "rain" in results[0].lower() \
            else "No umbrella needed."
    if "quartz robotics" in p:
        if n == 0:
            return call("compare_stocks", {"symbols": ["QZRB", "QZLG"], "range": "1y"})
        return "Quartz Robotics (QZRB) outperformed Quartz Logistics over the past year."
    if "dana" in p:
        if n == 0:
            return call("gmail_search_threads", {"query": "from:dana is:unread"})
        if n == 1:
            return call("calendar_suggest_time", {
                "attendeeEmails": ["dana@northwind.example"], "durationMinutes": 30,
                "startTime": NEXT_WEEK[0], "endTime": NEXT_WEEK[1]})
        if n == 2:
            # cedar writes a window's start as "Thu 2026-11-05T11:00:00-08:00".
            slot = re.search(r'"start":\s*"(\w{3}) [\d-]+T(\d\d):(\d\d)', results[1])
            days = {"Mon": "Monday", "Tue": "Tuesday", "Wed": "Wednesday", "Thu": "Thursday",
                    "Fri": "Friday"}
            when = f"{days.get(slot.group(1), slot.group(1))} at {int(slot.group(2))}:{slot.group(3)}" \
                if slot else "a time that suits you"
            return call("gmail_create_draft", {
                "to": ["dana@northwind.example"], "replyToMessageId": "t-dana-kickoff-1",
                "body": f"Hi Dana,\n\nHow about {when} for the kickoff?\n\nMaya"})
        return "I drafted a reply to Dana proposing Thursday at 11:00."
    if "sam" in p and "q3" in p:
        if answered is None:
            return call("ask_user", {"questions": [{"question": "Which Sam do you mean, Sam Rivera or Sam Okoye?"}]})
        if n <= 1:  # the ask_user result is in the parent run
            return call("gmail_create_draft", {
                "to": ["sam@okoye-cpa.example"], "subject": "Q3 numbers",
                "body": "Hi Sam,\n\nQ3: revenue $61,400, expenses $18,250, net income $43,150.\n\nMaya"})
        return "Drafted the Q3 summary to Sam Okoye."
    return "I can't help with that in this smoke test."


NEXT_WEEK = ("", "")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._send({"object": "list", "data": [{"id": "scripted", "object": "model"}]})

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        msgs = req["messages"]
        users = [m for m in msgs if m["role"] == "user"]

        def text(m):
            c = m.get("content")
            return c if isinstance(c, str) else " ".join(
                part.get("text", "") for part in c or [] if isinstance(part, dict))

        prompt = text(users[0])
        # A continuation's answer is the last user message after an ask_user call.
        asked = any(tc["function"]["name"] == "ask_user" for m in msgs
                    for tc in m.get("tool_calls") or [])
        answered = text(users[-1]) if asked and len(users) > 1 else None
        last_user = max(i for i, m in enumerate(msgs) if m["role"] == "user")
        results = [text(m) for m in msgs[last_user:] if m["role"] == "tool"]
        if answered is None:
            results = [text(m) for m in msgs if m["role"] == "tool"]
        step = script(prompt, results, answered)
        usage = {"prompt_tokens": sum(len(json.dumps(m)) for m in msgs) // 4, "completion_tokens": 40}
        if isinstance(step, str):
            msg, finish = {"role": "assistant", "content": step}, "stop"
        else:
            msg = {"role": "assistant", "content": None, "tool_calls": [{
                "id": f"call_{len(msgs)}", "type": "function",
                "function": {"name": step["name"], "arguments": json.dumps(step["args"])}}]}
            finish = "tool_calls"
        self._send({"id": "x", "object": "chat.completion", "model": "scripted",
                    "choices": [{"index": 0, "message": msg, "finish_reason": finish}], "usage": usage})


if __name__ == "__main__":
    import datetime as dt
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8001)
    ap.add_argument("--anchor", default=dt.date.today().isoformat())
    a = ap.parse_args()
    anchor = dt.date.fromisoformat(a.anchor)
    mon = anchor + dt.timedelta(days=7 - anchor.weekday())
    NEXT_WEEK = (f"{mon}T00:00:00", f"{mon + dt.timedelta(days=5)}T00:00:00")
    ThreadingHTTPServer(("127.0.0.1", a.port), Handler).serve_forever()
