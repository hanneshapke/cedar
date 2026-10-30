"""The reports: the analysis behind them, and report.md / report.pdf / charts written from it."""

import json
import tempfile
import unittest
from pathlib import Path

from pypdf import PdfReader

from cedar_bench import report


def trial(task, model, reasoning, repeat, passed, cost=0.002, category="web", violations=(), error=None):
    return {"task": task, "task_version": 1, "model": model, "reasoning": reasoning, "repeat": repeat,
            "category": category, "passed": passed, "score": 1.0 if passed else 0.5,
            "checks": [{"kind": "final_text", "passed": passed, "detail": "final text lacks '1931'",
                        "weight": 1.0, "required": True, "label": "right year"}],
            "status": "completed", "error": error,
            "usage": {"input_tokens": 1000, "output_tokens": 100, "turns": 3, "cost_usd": cost},
            "safety_violations": list(violations), "fixture_misses": [], "duration_secs": 10.0}


def sample():
    rows = []
    for r in range(3):
        rows.append(trial("web/a", "big", "high", r, True, 0.004))
        rows.append(trial("web/b", "big", "high", r, r != 2, 0.004))
        rows.append(trial("safety/c", "big", "high", r, True, 0.004, "safety"))
        rows.append(trial("web/a", "small", None, r, r == 0, 0.001))
        rows.append(trial("web/b", "small", None, r, False, 0.001))
        rows.append(trial("safety/c", "small", None, r, r != 1, 0.001, "safety",
                          violations=["gmail_send_message"] if r == 1 else ()))
    return rows


class AnalyzeTest(unittest.TestCase):
    def test_leaderboard_reliability_and_cost(self):
        a = report.analyze(sample())
        best, other = a["configs"]
        self.assertEqual(best["label"], "big · high")
        self.assertAlmostEqual(best["success"], 8 / 9)
        self.assertAlmostEqual(best["pass_k"][3], 2 / 3)  # web/b failed once
        self.assertAlmostEqual(best["cost_per_success"], 9 * 0.004 / 8)
        self.assertEqual(other["label"], "small")
        self.assertEqual(a["headline"]["cheapest"]["label"], "small")
        self.assertEqual(a["safety"][0]["tools"], ["gmail_send_message"])

    def test_category_shares_are_floats(self):
        # mean() over bools returns a bool, which a colormap reads as an index, not a fraction
        a = report.analyze(sample())
        self.assertIs(type(a["by_category"]["safety"]["big · high"]), float)
        self.assertEqual(a["by_category"]["safety"]["big · high"], 1.0)

    def test_matrix_puts_the_hardest_task_first(self):
        a = report.analyze(sample())
        self.assertEqual(a["matrix"][0]["task"], "web/b")
        self.assertEqual(a["matrix"][0]["cells"]["small"], (0, 3))

    def test_failures_say_why(self):
        a = report.analyze(sample())
        self.assertIn("right year: final text lacks '1931'", a["failures"]["web/b"]["small"])
        self.assertIn("called forbidden tool(s): gmail_send_message", a["failures"]["safety/c"]["small"])


class WriteReportsTest(unittest.TestCase):
    def write(self, rows, meta=None):
        d = Path(tempfile.mkdtemp())
        (d / "results.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
        if meta:
            (d / "meta.json").write_text(json.dumps(meta))
        return d, report.write_reports(d / "results.jsonl")

    def test_pdf_markdown_and_charts(self):
        d, out = self.write(sample(), {"bench_version": "0.1.0", "commit": "abcdef1234567",
                                       "repeats": 3, "models": [{"id": "big", "model": "vendor/big",
                                                                 "price": {"input_usd_per_m": 1, "output_usd_per_m": 2}}]})
        for key in ("pdf", "markdown", "chart_success", "chart_cost", "chart_categories"):
            self.assertTrue(out[key].exists() and out[key].stat().st_size > 1000, key)
        text = "\n".join(p.extract_text() for p in PdfReader(out["pdf"]).pages)
        for part in ("cedar-bench report", "Leaderboard", "big · high", "88.9%", "Safety",
                     "gmail_send_message", "How it was measured", "abcdef1234"):
            self.assertIn(part, text)
        md = out["markdown"].read_text()
        self.assertIn("![Success and reliability per configuration](charts/success.png)", md)
        self.assertIn("| big | high | 3 | 9 | 88.9% | 66.7% |", md)

    def test_one_trial_without_prices(self):
        rows = [trial("web/a", "m", None, 0, False, cost=None)]
        rows[0]["usage"]["cost_usd"] = None
        d, out = self.write(rows)
        self.assertTrue(out["pdf"].exists())
        self.assertNotIn("chart_cost", out)  # no prices, no cost chart
        self.assertIn("No forbidden tool was called.", out["markdown"].read_text())


if __name__ == "__main__":
    unittest.main()
