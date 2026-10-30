"""The runner against a fake cedar that plays a scripted agent: it asks which Sam is meant, waits
for an approval, and on the continuation writes the draft through the fixture server, the way
cedar's gmail_create_draft would."""

import base64
import email.message
import json
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from cedar_bench import fixtures, report, runner, schema
from cedar_bench.client import Cedar

BENCH = Path(__file__).resolve().parent.parent


class FakeCedar(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    runs: dict = {}
    approvals: dict = {}
    fixtures_url = ""

    def log_message(self, *a):
        pass

    def _send(self, status, body, ctype="application/json"):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"] or 0)) or b"{}")
        if self.path == "/v1/runs":
            rid = f"run-{len(self.runs) + 1}"
            self.runs[rid] = {"id": rid, "status": "running", "turns": 0, "body": body,
                              "final_text": None, "stop_reason": None}
            return self._send(202, self.runs[rid])
        if "/approvals/" in self.path:
            self.approvals[self.path] = body["decision"]
            return self._send(202, {})
        self._send(404, {})

    def do_GET(self):
        parts = self.path.split("?")[0].strip("/").split("/")
        if parts[:2] == ["v1", "runs"] and len(parts) == 4 and parts[3] == "events":
            return self._events(self.runs[parts[2]])
        if parts[:2] == ["v1", "runs"] and len(parts) == 4 and parts[3] == "files":
            return self._send(200, {"files": [], "truncated": False})
        if parts[:2] == ["v1", "runs"] and len(parts) == 3:
            return self._send(200, {k: v for k, v in self.runs[parts[2]].items() if k != "body"})
        self._send(404, {})

    def _events(self, run):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        seq = 0

        def emit(kind, data):
            nonlocal seq
            seq += 1
            msg = {"seq": seq, "type": kind, "data": data}
            self.wfile.write(f"id: {seq}\nevent: {kind}\ndata: {json.dumps(msg)}\n\n".encode())
            self.wfile.flush()

        emit("run.started", {"model": "fake"})
        if "parent_id" not in run["body"]:
            emit("tool.call", {"id": "c1", "name": "ask_user", "input": {"questions": [
                {"question": "Which Sam should I write to: Sam Rivera or Sam Okoye?"}]}})
            emit("usage", {"turn": 1, "input_tokens": 900, "output_tokens": 50, "cost_usd": 0.0002})
            run.update(status="completed", stop_reason="ask_user", turns=1,
                       final_text="Which Sam should I write to?")
        else:
            assert "Okoye" in run["body"]["prompt"], run["body"]
            emit("approval.required", {"id": "a1", "call_id": "c2", "tool": "gmail_create_draft",
                                       "input": {}, "rule": "email"})
            emit("tool.call", {"id": "c2", "name": "gmail_create_draft",
                               "input": {"to": ["sam@okoye-cpa.example"]}})
            m = email.message.EmailMessage()
            m["To"] = "sam@okoye-cpa.example"
            m["Subject"] = "Q3 numbers"
            m.set_content("Hi Sam, Q3 revenue was $61,400 and net income $43,150. Maya")
            req = urllib.request.Request(
                self.fixtures_url + "/google/gmail/drafts",
                data=json.dumps({"message": {"raw": base64.urlsafe_b64encode(m.as_bytes()).decode()}}).encode(),
                headers={"Authorization": f"Bearer {fixtures.TOKEN}", "Content-Type": "application/json"})
            urllib.request.urlopen(req).read()
            emit("usage", {"turn": 2, "input_tokens": 1500, "output_tokens": 120, "cost_usd": 0.0004})
            run.update(status="completed", stop_reason="end_turn", turns=2,
                       final_text="I drafted the email to Sam Okoye.")
        emit("run.completed", {"final_text": run["final_text"], "stop_reason": run["stop_reason"]})
        self.close_connection = True


class RunnerTest(unittest.TestCase):
    def setUp(self):
        fx = fixtures.Fixtures(BENCH / "fixtures", BENCH / "personas", "")
        self.fx_server = fixtures.FixtureServer(("127.0.0.1", 0), fx)
        fx.public_url = f"http://127.0.0.1:{self.fx_server.server_address[1]}"
        FakeCedar.fixtures_url = fx.public_url
        FakeCedar.runs, FakeCedar.approvals = {}, {}
        self.cedar_server = ThreadingHTTPServer(("127.0.0.1", 0), FakeCedar)
        for s in (self.fx_server, self.cedar_server):
            threading.Thread(target=s.serve_forever, daemon=True).start()
        self.cedar = Cedar(f"http://127.0.0.1:{self.cedar_server.server_address[1]}", "k")
        self.task = next(t for t in schema.load_tasks([BENCH / "tasks"])
                         if t.id == "email_calendar/ambiguous-sam")

    def tearDown(self):
        self.fx_server.shutdown()
        self.cedar_server.shutdown()

    def test_a_trial_answers_the_user_question_and_grades_the_draft(self):
        result, trial = runner.run_trial(self.cedar, self.fx_server.fixtures.public_url,
                                         BENCH / "fixtures", self.task, "fake", "high", 30)
        self.assertTrue(result["passed"], result)
        self.assertEqual(result["continuations"], 1)
        self.assertEqual(result["usage"]["output_tokens"], 170)
        self.assertEqual(result["approvals"], 1)
        self.assertEqual(list(FakeCedar.approvals.values()), ["allow"])
        first = FakeCedar.runs["run-1"]["body"]
        self.assertEqual(first["reasoning"], "high")
        self.assertEqual(first["attachments"][0]["media_type"], "application/pdf")
        self.assertEqual(FakeCedar.runs["run-2"]["body"]["parent_id"], "run-1")

        rows = [dict(task=self.task.id, model="fake", reasoning="high", repeat=i,
                     category="email_calendar", **result) for i in range(3)]
        rows[2]["passed"] = False
        self.assertAlmostEqual(report.pass_hat_k(rows, 1), 2 / 3)
        self.assertEqual(report.pass_hat_k(rows, 3), 0.0)
        self.assertIn("| fake | high |", report.markdown(rows))


if __name__ == "__main__":
    unittest.main()
