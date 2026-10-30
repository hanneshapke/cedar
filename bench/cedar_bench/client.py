"""A small client for cedar's HTTP API (see "API" in the top-level README), standard library only."""

from __future__ import annotations

import json
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Iterator


class CedarError(RuntimeError):
    def __init__(self, status: int, body: str):
        super().__init__(f"cedar answered {status}: {body[:500]}")
        self.status = status
        self.body = body


class Cedar:
    def __init__(self, base_url: str, key: str, admin_key: str | None = None, timeout: float = 60):
        self.base = base_url.rstrip("/")
        self.key = key
        self.admin_key = admin_key or key
        self.timeout = timeout

    def _request(self, method: str, path: str, body: Any = None, admin: bool = False,
                 raw: bool = False, timeout: float | None = None):
        headers = {"Authorization": f"Bearer {self.admin_key if admin else self.key}"}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(self.base + path, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=timeout or self.timeout) as resp:
                payload = resp.read()
        except urllib.error.HTTPError as e:
            raise CedarError(e.code, e.read().decode(errors="replace")) from None
        if raw:
            return payload
        return json.loads(payload) if payload else None

    # -- runs ----------------------------------------------------------------------------------

    def create_run(self, body: dict) -> dict:
        # 429 means this key has MAX_ACTIVE_RUNS_PER_KEY runs going; wait for one to finish.
        for attempt in range(60):
            try:
                return self._request("POST", "/v1/runs", body)
            except CedarError as e:
                if e.status != 429:
                    raise
                time.sleep(min(2 + attempt, 10))
        raise CedarError(429, "still too many active runs after waiting")

    def run(self, run_id: str) -> dict:
        return self._request("GET", f"/v1/runs/{run_id}")

    def cancel(self, run_id: str) -> None:
        try:
            self._request("POST", f"/v1/runs/{run_id}/cancel")
        except CedarError as e:
            if e.status != 409:  # already finished
                raise

    def files(self, run_id: str) -> dict:
        try:
            return self._request("GET", f"/v1/runs/{run_id}/files")
        except CedarError as e:
            if e.status == 404:  # workspace expired, or the run never got one
                return {"files": [], "truncated": False}
            raise

    def file(self, run_id: str, path: str) -> bytes:
        return self._request("GET", f"/v1/runs/{run_id}/files/{urllib.parse.quote(path)}", raw=True)

    def answer_plan(self, run_id: str, steps: list[dict]) -> None:
        self._request("POST", f"/v1/runs/{run_id}/plan", {"steps": steps})

    def answer_approval(self, run_id: str, approval_id: str, decision: str) -> None:
        self._request("POST", f"/v1/runs/{run_id}/approvals/{approval_id}", {"decision": decision})

    def events(self, run_id: str, deadline: float) -> Iterator[dict]:
        """The run's events, `{seq, type, data, created_at}`, from the first; ends after the
        terminal one. Reconnects from the last seq seen if the stream drops."""
        after = 0
        while True:
            if time.monotonic() > deadline:
                raise TimeoutError(f"run {run_id} did not finish in time")
            path = f"/v1/runs/{run_id}/events" + (f"?after={after}" if after else "")
            req = urllib.request.Request(self.base + path,
                                         headers={"Authorization": f"Bearer {self.key}",
                                                  "Accept": "text/event-stream"})
            try:
                with urllib.request.urlopen(req, timeout=max(deadline - time.monotonic(), 1)) as resp:
                    for event in _sse(resp):
                        after = max(after, event["seq"])
                        yield event
                        if event["type"] in ("run.completed", "run.failed", "run.cancelled"):
                            return
            except (urllib.error.URLError, ConnectionError, TimeoutError):
                pass
            # Dropped, or closed without a terminal event (cedar restarted?): resume after `after`.
            time.sleep(1)

    def ready(self) -> dict:
        """`GET /readyz`; raises OSError (URLError) when nothing answers at the address."""
        try:
            return self._request("GET", "/readyz", timeout=10)
        except CedarError as e:  # 503: up, but the database or Docker isn't usable
            raise ConnectionError(f"not ready: {e.body[:300]}") from None

    # -- admin ---------------------------------------------------------------------------------

    def register_model(self, model_id: str, entry: dict) -> dict:
        return self._request("PUT", f"/v1/admin/models/{urllib.parse.quote(model_id)}", entry,
                             admin=True, timeout=120)

    def models(self) -> dict:
        return self._request("GET", "/v1/models")

    def tools(self) -> dict:
        return self._request("GET", "/v1/tools")

    def defaults(self) -> dict:
        return self._request("GET", "/v1/defaults")


def _sse(resp) -> Iterator[dict]:
    data: list[str] = []
    for raw in resp:
        line = raw.decode("utf-8").rstrip("\r\n")
        if line.startswith("data:"):
            data.append(line[5:].lstrip(" "))
        elif not line and data:
            yield json.loads("\n".join(data))
            data = []
