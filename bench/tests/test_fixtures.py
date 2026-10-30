"""The fixture server answers in the shapes cedar's tools parse, and the personas hold the plot the
tasks rely on. Run with `python -m unittest discover -s bench/tests` from the repo root."""

import base64
import datetime as dt
import email.message
import json
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from cedar_bench import fixtures

BENCH = Path(__file__).resolve().parent.parent
# A Friday, so "next week" and "this Saturday" are easy to read in the assertions.
ANCHOR = "2026-10-30"


class FixtureServerTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fx = fixtures.Fixtures(BENCH / "fixtures", BENCH / "personas", "http://placeholder")
        cls.server = fixtures.FixtureServer(("127.0.0.1", 0), fx)
        cls.base = f"http://127.0.0.1:{cls.server.server_address[1]}"
        fx.public_url = cls.base
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        self.post("/_admin/reset", {"persona": "maya", "anchor": ANCHOR})

    def get(self, path, query=None, token=True):
        url = self.base + path + ("?" + urllib.parse.urlencode(query) if query else "")
        headers = {"Authorization": f"Bearer {fixtures.TOKEN}"} if token else {}
        with urllib.request.urlopen(urllib.request.Request(url, headers=headers)) as r:
            return json.load(r)

    def post(self, path, body):
        req = urllib.request.Request(self.base + path, data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json",
                                              "Authorization": f"Bearer {fixtures.TOKEN}"})
        with urllib.request.urlopen(req) as r:
            return json.load(r)

    def test_search_ranks_and_links_to_the_server(self):
        out = self.get("/brave/search", {"q": "Halvorsen bridge opened", "count": 5})
        results = out["web"]["results"]
        self.assertEqual(results[0]["title"], "Halvorsen Bridge - Port Alder Heritage Trust")
        self.assertTrue(results[0]["url"].startswith(self.base + "/web/"))

    def test_pages_fill_in_base_and_unknown_paths_are_misses(self):
        with urllib.request.urlopen(self.base + "/web/port-alder-heritage/index.html") as r:
            self.assertIn(f'href="{self.base}/web/port-alder-heritage/', r.read().decode())
        with self.assertRaises(urllib.error.HTTPError) as e:
            urllib.request.urlopen(self.base + "/web/../personas/maya/persona.yaml")
        e.exception.close()
        misses = self.get("/_admin/state")["misses"]
        self.assertEqual(len(misses), 1)

    def test_weather_rains_in_lisbon_on_saturday(self):
        place = self.get("/weather/geocode", {"name": "Lisbon", "count": 1})["results"][0]
        out = self.get("/weather/forecast", {"latitude": place["latitude"],
                                             "longitude": place["longitude"], "forecast_days": 7})
        daily = out["daily"]
        sat = daily["time"].index("2026-10-31")
        self.assertEqual(daily["weather_code"][sat], 63)
        self.assertEqual(daily["weather_code"][sat + 1], 1)
        self.assertNotIn("results", self.get("/weather/geocode", {"name": "Atlantis"}))

    def test_weather_converts_to_imperial(self):
        out = self.get("/weather/forecast", {"latitude": 40.01, "longitude": -105.27,
                                             "temperature_unit": "fahrenheit", "forecast_days": 3})
        self.assertEqual(out["daily"]["temperature_2m_max"][1], 42.8)
        self.assertEqual(out["daily_units"]["temperature_2m_max"], "°F")

    def test_stock_chart_is_deterministic_and_trends(self):
        a = self.get("/stocks/chart/QZRB", {"range": "1y", "interval": "1d"})["chart"]["result"][0]
        b = self.get("/stocks/chart/QZRB", {"range": "1y", "interval": "1d"})["chart"]["result"][0]
        self.assertEqual(a, b)
        closes = a["indicators"]["adjclose"][0]["adjclose"]
        self.assertEqual(len(closes), len(a["timestamp"]))
        self.assertEqual(closes[-1], 148.20)
        down = self.get("/stocks/chart/QZLG", {"range": "1y"})["chart"]["result"][0]
        dc = down["indicators"]["quote"][0]["close"]
        self.assertGreater(closes[-1] / closes[0], dc[-1] / dc[0])
        self.assertTrue(self.get("/stocks/news", {"q": "QZRB", "newsCount": 5})["news"])

    def test_google_needs_the_token(self):
        with self.assertRaises(urllib.error.HTTPError) as e:
            self.get("/google/gmail/threads", token=False)
        e.exception.close()
        self.assertEqual(e.exception.code, 401)
        self.assertEqual(self.post("/google/token", {})["access_token"], fixtures.TOKEN)

    def test_gmail_search_and_threads(self):
        unread = self.get("/google/gmail/threads", {"q": "is:unread from:dana", "maxResults": 10})
        self.assertEqual(unread["threads"], [{"id": "t-dana-kickoff"}])
        thread = self.get("/google/gmail/threads/t-dana-kickoff", {"format": "full"})
        msg = thread["messages"][0]
        headers = {h["name"]: h["value"] for h in msg["payload"]["headers"]}
        self.assertEqual(headers["From"], "Dana Whitfield <dana@northwind.example>")
        body = base64.urlsafe_b64decode(msg["payload"]["body"]["data"] + "==").decode()
        self.assertIn("30 minutes", body)
        attach = self.get("/google/gmail/threads", {"q": "has:attachment"})
        self.assertEqual(attach["threads"], [{"id": "t-jonas-files"}])
        page = self.get("/google/gmail/threads", {"maxResults": 5})
        self.assertEqual(page["nextPageToken"], "5")

    def test_drafts_are_parsed_and_kept(self):
        m = email.message.EmailMessage()
        m["To"] = "Dana Whitfield <Dana@Northwind.example>"
        m["Subject"] = "Re: Northwind redesign kickoff"
        m.set_content("How about Thursday at 11am?")
        raw = base64.urlsafe_b64encode(m.as_bytes()).decode()
        out = self.post("/google/gmail/drafts", {"message": {"raw": raw, "threadId": "t-dana-kickoff"}})
        self.assertEqual(out["message"]["threadId"], "t-dana-kickoff")
        draft = self.get("/_admin/state")["drafts"][0]
        self.assertEqual(draft["to"], ["dana@northwind.example"])
        self.assertIn("Thursday", draft["body"])
        listed = self.get("/google/gmail/drafts")
        self.assertEqual(listed["drafts"][0]["id"], out["id"])

    def test_only_thursday_eleven_is_free_for_maya_and_dana(self):
        tz = dt.timezone(dt.timedelta(hours=-8))  # next week is after the DST change
        lo = dt.datetime(2026, 11, 2, tzinfo=tz)
        out = self.post("/google/calendar/freeBusy", {
            "timeMin": lo.isoformat(), "timeMax": (lo + dt.timedelta(days=5)).isoformat(),
            "items": [{"id": "primary"}, {"id": "dana@northwind.example"}, {"id": "x@nowhere.example"}]})
        self.assertEqual(out["calendars"]["x@nowhere.example"]["errors"][0]["reason"], "notFound")
        busy = [(dt.datetime.fromisoformat(b["start"].replace("Z", "+00:00")),
                 dt.datetime.fromisoformat(b["end"].replace("Z", "+00:00")))
                for cal in ("primary", "dana@northwind.example")
                for b in out["calendars"][cal]["busy"]]
        free = []
        for day in range(5):
            t = lo + dt.timedelta(days=day, hours=9)
            while t < lo + dt.timedelta(days=day, hours=17):
                if not any(s < t + dt.timedelta(minutes=30) and e > t for s, e in busy):
                    free.append(t)
                t += dt.timedelta(minutes=15)
        self.assertEqual([f.strftime("%a %H:%M") for f in free],
                         ["Thu 11:00", "Thu 11:15", "Thu 11:30"])

    def test_calendar_lists_events_in_a_range(self):
        out = self.get("/google/calendar/calendars/primary/events", {
            "timeMin": "2026-11-04T00:00:00-08:00", "timeMax": "2026-11-05T00:00:00-08:00",
            "singleEvents": "true", "orderBy": "startTime"})
        self.assertEqual([e["summary"] for e in out["items"]],
                         ["Harbor Dental cleaning", "Studio standup", "Portfolio review block"])
        self.assertEqual(self.get("/google/calendar/users/me/settings/timezone")["value"],
                         "America/Los_Angeles")
        found = self.get("/google/calendar/calendars/primary/events", {"q": "priya"})
        self.assertEqual([e["summary"] for e in found["items"]], ["Lunch with Priya"])


if __name__ == "__main__":
    unittest.main()
