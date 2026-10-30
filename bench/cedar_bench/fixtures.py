"""One HTTP server standing in for every external service cedar's tools call.

Each service lives under its own path prefix; `python -m cedar_bench env` prints the cedar settings
that point at them:

    /brave/search                Brave web search        (BRAVE_API_BASE_URL)
    /web/<path>                  pages and files         (fetch_url, download_file; search results
                                                          link here)
    /weather/forecast, /geocode  Open-Meteo              (WEATHER_BASE_URL, WEATHER_GEOCODING_BASE_URL)
    /stocks/chart/<sym>, /news   Yahoo Finance           (STOCKS_CHART_URL, STOCKS_NEWS_URL)
    /google/token, /gmail, /calendar   Google OAuth and REST APIs over the active persona
    /replay/<service>/<path>     recorded answers, for services too intricate to synthesize
                                 (transit, driving, ...); `--record service=URL` fills them

and the harness's own routes:

    POST /_admin/reset  {"persona", "anchor"?}   load a persona and forget drafts and the log
    GET  /_admin/state                            drafts, the request log, and fixture misses
    GET  /_admin/health

The server holds one persona at a time, so trials against one server run one after another.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import math
import mimetypes
import random
import re
import threading
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import yaml

from . import personas, timeutil
from .google_mock import GoogleMock, NotFound

TOKEN = "cedar-bench-access-token"
RANGES = {"5d": 5, "1mo": 22, "3mo": 66, "6mo": 126, "1y": 252, "2y": 504, "5y": 1260,
          "10y": 1260, "max": 1260}


class Fixtures:
    """Everything the server serves, loaded from the fixtures and personas directories."""

    def __init__(self, root: Path, personas_dir: Path, public_url: str,
                 record: dict[str, str] | None = None):
        self.root = root
        self.personas_dir = personas_dir
        self.public_url = public_url.rstrip("/")
        self.record = record or {}
        self.lock = threading.Lock()
        self.anchor = dt.date.today()
        self.persona_id: str | None = None
        self.google: GoogleMock | None = None
        self.log: list[dict] = []
        self.search = self._yaml("web/search.yaml") or []
        self.locations = self._yaml("weather/locations.yaml") or []
        self.symbols = {s["symbol"].upper(): s for s in self._yaml("stocks/symbols.yaml") or []}

    def _yaml(self, rel: str) -> Any:
        path = self.root / rel
        return yaml.safe_load(path.read_text()) if path.exists() else None

    def reset(self, persona_id: str | None, anchor: str | None) -> dict:
        with self.lock:
            self.anchor = timeutil.parse_anchor(anchor)
            self.log = []
            self.persona_id = persona_id
            self.google = None
            if persona_id:
                p = personas.load(personas.find(self.personas_dir, persona_id), self.anchor)
                self.google = GoogleMock(p)
            return self.state()

    def state(self) -> dict:
        return {
            "persona": self.persona_id,
            "anchor": self.anchor.isoformat(),
            "base": self.public_url,
            "drafts": self.google.drafts if self.google else [],
            "requests": self.log,
            "misses": [r for r in self.log if r["status"] == 404],
        }

    def link(self, url: str) -> str:
        """Search results and pages write `{base}` for the server's own address."""
        return url.replace("{base}", self.public_url)

    # -- Brave ---------------------------------------------------------------------------------

    def brave(self, query: dict[str, str]) -> dict:
        words = set(re.findall(r"[a-z0-9]+", query.get("q", "").lower()))
        count = int(query.get("count", 10))
        scored = []
        for i, r in enumerate(self.search):
            text = " ".join([r["title"], r.get("description", ""), " ".join(r.get("keywords", []))])
            score = len(words & set(re.findall(r"[a-z0-9]+", text.lower())))
            if score:
                scored.append((-score, i, r))
        results = [{"title": r["title"], "url": self.link(r["url"]),
                    "description": r.get("description", ""),
                    **({"age": timeutil.resolve_str(r["age"], self.anchor)} if "age" in r else {})}
                   for _, _, r in sorted(scored)[:count]]
        return {"type": "search", "query": {"original": query.get("q", "")},
                "web": {"type": "search", "results": results}}

    # -- web pages ------------------------------------------------------------------------------

    def page(self, rel: str) -> tuple[bytes, str] | None:
        site = (self.root / "web" / "site").resolve()
        path = (site / rel).resolve()
        if site not in path.parents or not path.is_file():
            return None
        ctype = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        data = path.read_bytes()
        if ctype.startswith("text/") or ctype in ("application/json",):
            text = timeutil.resolve_str(self.link(data.decode()), self.anchor)
            data = text.encode()
            ctype += "; charset=utf-8"
        return data, ctype

    # -- weather --------------------------------------------------------------------------------

    def geocode(self, query: dict[str, str]) -> dict:
        name = query.get("name", "").split(",")[0].strip().lower()
        for loc in self.locations:
            if name in [loc["name"].lower(), *[a.lower() for a in loc.get("aliases", [])]]:
                return {"results": [{"name": loc["name"], "latitude": loc["latitude"],
                                     "longitude": loc["longitude"], "country": loc.get("country", ""),
                                     "admin1": loc.get("admin1", ""), "timezone": loc["timezone"]}]}
        return {"generationtime_ms": 0.1}  # Open-Meteo's "no match": no `results`

    def forecast(self, query: dict[str, str]) -> dict:
        lat, lon = float(query.get("latitude", 0)), float(query.get("longitude", 0))
        loc = min(self.locations, key=lambda l: math.hypot(l["latitude"] - lat, l["longitude"] - lon))
        days = int(query.get("forecast_days", 7))
        imperial = query.get("temperature_unit") == "fahrenheit"
        rng = random.Random(f"{loc['name']}:{self.anchor}")
        # `days` maps date tokens ("{{sat}}", "{{today+1}}") to what differs from the seeded default.
        given = {timeutil.resolve_str(k, self.anchor): v for k, v in (loc.get("days") or {}).items()}
        dates = [(self.anchor + dt.timedelta(days=i)).isoformat() for i in range(days)]
        rows = []
        for date in dates:
            d = dict(given.get(date) or {})
            base = loc.get("base", {"max": 18, "min": 9})
            d.setdefault("code", rng.choice([0, 1, 2, 3]))
            d.setdefault("max", round(base["max"] + rng.uniform(-2, 2), 1))
            d.setdefault("min", round(base["min"] + rng.uniform(-2, 2), 1))
            d.setdefault("precip", 0.0 if d["code"] < 50 else round(rng.uniform(1, 12), 1))
            d.setdefault("precip_prob", 5 if d["code"] < 50 else rng.randint(60, 95))
            d.setdefault("wind", round(rng.uniform(5, 25), 1))
            rows.append(d)
        t = (lambda c: round(c * 9 / 5 + 32, 1)) if imperial else (lambda c: c)
        w = (lambda k: round(k / 1.609, 1)) if imperial else (lambda k: k)
        mm = (lambda v: round(v / 25.4, 2)) if imperial else (lambda v: v)
        now = rows[0]
        return {
            "latitude": loc["latitude"], "longitude": loc["longitude"], "timezone": loc["timezone"],
            "current_units": {"time": "iso8601", "temperature_2m": "°F" if imperial else "°C",
                              "apparent_temperature": "°F" if imperial else "°C",
                              "relative_humidity_2m": "%", "precipitation": "inch" if imperial else "mm",
                              "weather_code": "wmo code", "wind_speed_10m": "mp/h" if imperial else "km/h",
                              "wind_direction_10m": "°", "wind_gusts_10m": "mp/h" if imperial else "km/h"},
            "current": {"time": f"{dates[0]}T12:00", "temperature_2m": t(now["max"] - 1),
                        "apparent_temperature": t(now["max"] - 2), "relative_humidity_2m": 65,
                        "precipitation": mm(now["precip"] / 6), "weather_code": now["code"],
                        "wind_speed_10m": w(now["wind"]), "wind_direction_10m": 250,
                        "wind_gusts_10m": w(now["wind"] * 1.8)},
            "daily_units": {"time": "iso8601", "weather_code": "wmo code",
                            "temperature_2m_max": "°F" if imperial else "°C",
                            "temperature_2m_min": "°F" if imperial else "°C",
                            "precipitation_sum": "inch" if imperial else "mm",
                            "precipitation_probability_max": "%",
                            "wind_speed_10m_max": "mp/h" if imperial else "km/h",
                            "wind_gusts_10m_max": "mp/h" if imperial else "km/h",
                            "uv_index_max": "", "sunrise": "iso8601", "sunset": "iso8601"},
            "daily": {"time": dates, "weather_code": [r["code"] for r in rows],
                      "temperature_2m_max": [t(r["max"]) for r in rows],
                      "temperature_2m_min": [t(r["min"]) for r in rows],
                      "precipitation_sum": [mm(r["precip"]) for r in rows],
                      "precipitation_probability_max": [r["precip_prob"] for r in rows],
                      "wind_speed_10m_max": [w(r["wind"]) for r in rows],
                      "wind_gusts_10m_max": [w(r["wind"] * 1.8) for r in rows],
                      "uv_index_max": [3.0 for _ in rows],
                      "sunrise": [f"{d}T07:10" for d in dates],
                      "sunset": [f"{d}T18:05" for d in dates]},
        }

    # -- stocks ---------------------------------------------------------------------------------

    def chart(self, symbol: str, query: dict[str, str]) -> tuple[int, dict]:
        s = self.symbols.get(symbol.upper())
        if not s:
            return 404, {"chart": {"result": None, "error": {
                "code": "Not Found", "description": "No data found, symbol may be delisted"}}}
        n = RANGES.get(query.get("range", "1y"), 252)
        days, closes = self._series(s)
        days, closes = days[-n:], closes[-n:]
        offset = int(s.get("gmtoffset", -18000))
        stamps = [int(dt.datetime.combine(d, dt.time(14, 30), dt.timezone.utc).timestamp())
                  for d in days]
        meta = {"symbol": s["symbol"], "longName": s["name"], "currency": s.get("currency", "USD"),
                "fullExchangeName": s.get("exchange", "NasdaqGS"),
                "instrumentType": s.get("type", "EQUITY"), "gmtoffset": offset,
                "regularMarketPrice": closes[-1], "regularMarketTime": stamps[-1] + 6 * 3600,
                "previousClose": closes[-2]}
        return 200, {"chart": {"error": None, "result": [{
            "meta": meta, "timestamp": stamps,
            "indicators": {"quote": [{"close": closes}], "adjclose": [{"adjclose": closes}]}}]}}

    def _series(self, s: dict) -> tuple[list[dt.date], list[float]]:
        """Trading days up to the anchor and closes ending at `price`: a seeded random walk with
        the drift that makes the yearly return about `annual_return`."""
        end = self.anchor
        while end.weekday() >= 5:
            end -= dt.timedelta(days=1)
        days = []
        d = end
        while len(days) < RANGES["5y"]:
            if d.weekday() < 5:
                days.append(d)
            d -= dt.timedelta(days=1)
        days.reverse()
        seed = int(hashlib.sha256(f"{s['symbol']}:{s.get('seed', 0)}".encode()).hexdigest()[:8], 16)
        rng = random.Random(seed)
        vol = s.get("volatility", 0.2) / math.sqrt(252)
        drift = math.log(1 + s.get("annual_return", 0.08)) / 252
        logs = [0.0]
        for _ in days[1:]:
            logs.append(logs[-1] + drift + rng.gauss(0, vol))
        shift = math.log(s["price"]) - logs[-1]
        return days, [round(math.exp(v + shift), 2) for v in logs]

    def news(self, query: dict[str, str]) -> dict:
        q = query.get("q", "").upper()
        count = int(query.get("newsCount", 5))
        items = []
        for item in self.symbols.get(q, {}).get("news", [])[:count]:
            when = timeutil.local_datetime(timeutil.resolve_str(item["date"], self.anchor), "UTC")
            items.append({"title": item["title"], "publisher": item.get("publisher", "Bench Wire"),
                          "link": self.link(item.get("link", "{base}/web/news")),
                          "providerPublishTime": int(when.timestamp())})
        return {"news": items}

    # -- replay ---------------------------------------------------------------------------------

    def replay(self, service: str, path: str, query: dict[str, str], method: str,
               body: bytes) -> tuple[int, bytes, str] | None:
        folder = self.root / "replay" / service
        for f in sorted(folder.glob("*.yaml")) if folder.exists() else []:
            rec = yaml.safe_load(f.read_text())
            if rec.get("method", "GET") != method or rec["path"].strip("/") != path.strip("/"):
                continue
            if all(query.get(k) == str(v) for k, v in (rec.get("query") or {}).items()):
                out = rec["body"]
                data = out.encode() if isinstance(out, str) else json.dumps(out).encode()
                return rec.get("status", 200), data, rec.get("content_type", "application/json")
        upstream = self.record.get(service)
        if not upstream:
            return None
        url = f"{upstream.rstrip('/')}/{path}"
        if query:
            url += "?" + urllib.parse.urlencode(query)
        req = urllib.request.Request(url, data=body or None, method=method,
                                     headers={"User-Agent": "cedar-bench-recorder"})
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                status, data, ctype = resp.status, resp.read(), resp.headers.get_content_type()
        except urllib.error.HTTPError as e:
            status, data, ctype = e.code, e.read(), e.headers.get_content_type()
        folder.mkdir(parents=True, exist_ok=True)
        name = hashlib.sha256(f"{method} {path} {sorted(query.items())}".encode()).hexdigest()[:12]
        try:
            stored: Any = json.loads(data)
        except ValueError:
            stored = data.decode(errors="replace")
        (folder / f"{name}.yaml").write_text(yaml.safe_dump(
            {"method": method, "path": path, "query": query, "status": status,
             "content_type": ctype, "body": stored}, sort_keys=False, allow_unicode=True))
        return status, data, ctype


class Handler(BaseHTTPRequestHandler):
    server: "FixtureServer"
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # the request log is in /_admin/state
        pass

    def do_GET(self):
        self._handle("GET")

    def do_POST(self):
        self._handle("POST")

    def _handle(self, method: str):
        fx = self.server.fixtures
        url = urllib.parse.urlsplit(self.path)
        path = urllib.parse.unquote(url.path)
        query = dict(urllib.parse.parse_qsl(url.query, keep_blank_values=True))
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            status, data, ctype = self._route(fx, method, path, query, body)
        except NotFound:
            status, (data, ctype) = 404, _json({"error": {"code": 404, "message": "Not Found"}})
        except Exception as e:  # a fixture bug must show up as a harness error, not a hang
            status, (data, ctype) = 500, _json({"error": {"code": 500, "message": repr(e)}})
        if not path.startswith("/_admin"):
            with fx.lock:
                fx.log.append({"method": method, "path": path, "query": query, "status": status})
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _route(self, fx: Fixtures, method: str, path: str, query: dict, body: bytes):
        parts = [p for p in path.split("/") if p]
        head = parts[0] if parts else ""
        if head == "_admin":
            if parts[1:] == ["reset"] and method == "POST":
                req = json.loads(body or b"{}")
                return 200, *_json(fx.reset(req.get("persona"), req.get("anchor")))
            if parts[1:] == ["state"]:
                with fx.lock:
                    return 200, *_json(fx.state())
            if parts[1:] == ["health"]:
                return 200, *_json({"ok": True, "persona": fx.persona_id})
        elif head == "brave":
            return 200, *_json(fx.brave(query))
        elif head == "web":
            found = fx.page("/".join(parts[1:]))
            if found:
                return 200, *found
        elif head == "weather":
            if parts[1:] == ["forecast"]:
                return 200, *_json(fx.forecast(query))
            if parts[1:] == ["geocode"]:
                return 200, *_json(fx.geocode(query))
        elif head == "stocks":
            if len(parts) == 3 and parts[1] == "chart":
                status, out = fx.chart(parts[2], query)
                return status, *_json(out)
            if parts[1:] == ["news"]:
                return 200, *_json(fx.news(query))
        elif head == "google":
            return self._google(fx, method, parts[1:], query, body)
        elif head == "replay" and len(parts) >= 2:
            hit = fx.replay(parts[1], "/".join(parts[2:]), query, method, body)
            if hit:
                return hit
        raise NotFound(path)

    def _google(self, fx: Fixtures, method: str, parts: list[str], query: dict, body: bytes):
        if parts == ["token"] and method == "POST":
            return 200, *_json({"access_token": TOKEN, "expires_in": 3600, "token_type": "Bearer"})
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            return 401, *_json({"error": {"code": 401, "status": "UNAUTHENTICATED",
                                          "message": "Request had invalid credentials."}})
        g = fx.google
        if g is None:
            return 403, *_json({"error": {"code": 403, "status": "PERMISSION_DENIED",
                                          "message": "No persona is loaded (POST /_admin/reset)."}})
        with fx.lock:
            if parts[:1] == ["gmail"]:
                if method == "POST" and parts[1:] == ["drafts"]:
                    return 200, *_json(g.gmail_create_draft(json.loads(body)))
                if method == "GET":
                    return 200, *_json(g.gmail_get(parts[1:], query))
            if parts[:1] == ["calendar"]:
                if method == "POST" and parts[1:] == ["freeBusy"]:
                    return 200, *_json(g.free_busy(json.loads(body)))
                if method == "GET":
                    return 200, *_json(g.calendar_get(parts[1:], query))
        raise NotFound("/".join(parts))


def _json(value: Any) -> tuple[bytes, str]:
    return json.dumps(value, ensure_ascii=False).encode(), "application/json; charset=utf-8"


class FixtureServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, addr: tuple[str, int], fixtures: Fixtures):
        super().__init__(addr, Handler)
        self.fixtures = fixtures


def cedar_env(base: str) -> dict[str, str]:
    """The cedar settings that point every tool at the fixture server at `base`."""
    base = base.rstrip("/")
    return {
        "WEB_FETCH_ALLOW_PRIVATE": "on",
        "BRAVE_API_KEY": "cedar-bench",
        "BRAVE_API_BASE_URL": f"{base}/brave/search",
        "WEATHER_BASE_URL": f"{base}/weather/forecast",
        "WEATHER_GEOCODING_BASE_URL": f"{base}/weather/geocode",
        "STOCKS_CHART_URL": f"{base}/stocks/chart",
        "STOCKS_NEWS_URL": f"{base}/stocks/news",
        "TRANSIT_BASE_URL": f"{base}/replay/transit",
        "DRIVING_BASE_URL": f"{base}/replay/driving",
        "RECIPE_WIKIPEDIA_URL": f"{base}/replay/wikipedia",
        "RECIPE_COMMONS_URL": f"{base}/replay/commons",
        "GMAIL": "on",
        "CALENDAR": "on",
        "GOOGLE_BACKEND": "api",
        "GOOGLE_CLIENT_ID": "cedar-bench",
        "GOOGLE_CLIENT_SECRET": "cedar-bench",
        "GOOGLE_REFRESH_TOKEN": "cedar-bench",
        "GOOGLE_TOKEN_URL": f"{base}/google/token",
        "GMAIL_API_URL": f"{base}/google/gmail",
        "CALENDAR_API_URL": f"{base}/google/calendar",
        # Live services the hermetic track has no fixtures for yet.
        "FLIGHTS": "off",
        "PRODUCTS": "off",
        # The memory list would carry facts from one trial into the next.
        "MEMORY": "off",
    }


def serve(root: Path, personas_dir: Path, host: str, port: int, public_url: str | None,
          record: dict[str, str]) -> None:
    public = public_url or f"http://{host}:{port}"
    server = FixtureServer((host, port), Fixtures(root, personas_dir, public, record))
    print(f"cedar-bench fixtures on http://{host}:{port} (links as {public})", flush=True)
    server.serve_forever()
