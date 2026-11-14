#!/usr/bin/env python3
"""A scripted OpenAI-compatible model server for the Cedar intro screencast.

Cedar itself, its chat UI and the sandbox are real; this stands in for the language model so the
recording is the same every time. It answers /v1/models and /v1/chat/completions (not streamed,
as Cedar calls it) and picks each reply from the latest user message and how many tool results
have come back since. Every request is appended to requests.jsonl next to it.
"""
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL = "deepseek-ai/DeepSeek-V4-Flash"
sys.path.insert(0, HERE)
import scenes  # noqa: E402


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(p.get("text", "") for p in content if isinstance(p, dict))
    return ""


def completion(msg, finish, n):
    return {"id": f"chatcmpl-{n}", "object": "chat.completion", "created": int(time.time()),
            "model": MODEL, "choices": [{"index": 0, "message": msg, "finish_reason": finish}],
            "usage": {"prompt_tokens": 2400 + 310 * n % 900, "completion_tokens": 180 + 37 * n % 200,
                      "total_tokens": 0}}


class H(BaseHTTPRequestHandler):
    n = 0

    def log_message(self, *a):
        pass

    def send_json(self, obj, code=200):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/").endswith("/models"):
            return self.send_json({"object": "list", "data": [
                {"id": MODEL, "object": "model", "owned_by": "vllm"}]})
        self.send_json({"error": {"message": "not found"}}, 404)

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        H.n += 1
        msgs = req.get("messages", [])
        system = text_of(msgs[0]["content"]) if msgs and msgs[0]["role"] == "system" else ""
        last_user = max((i for i, m in enumerate(msgs) if m["role"] == "user"), default=-1)
        prompt = text_of(msgs[last_user]["content"]) if last_user >= 0 else ""
        results = [text_of(m["content"]) for m in msgs[last_user + 1:] if m["role"] == "tool"]
        reply = scenes.reply(system, prompt, results, req)
        with open(os.path.join(HERE, "requests.jsonl"), "a") as f:
            f.write(json.dumps({"n": H.n, "system": system[:300], "prompt": prompt[:400],
                                "results": [r[:300] for r in results],
                                "tools": [t["function"]["name"] for t in req.get("tools", [])],
                                "extra": {k: v for k, v in req.items() if k not in ("messages", "tools")},
                                "reply": reply}) + "\n")
        time.sleep(reply.pop("delay", 1.2))
        msg = {"role": "assistant", "content": reply.get("text") or None}
        if reply.get("reasoning"):
            msg["reasoning_content"] = reply["reasoning"]
        calls = reply.get("calls") or []
        if calls:
            msg["tool_calls"] = [{"id": f"call_{H.n}_{i}", "type": "function",
                                  "function": {"name": name, "arguments": json.dumps(args)}}
                                 for i, (name, args) in enumerate(calls)]
        self.send_json(completion(msg, "tool_calls" if calls else "stop", H.n))


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
