"""An installed cedar-bench finds the benchmark data (tasks/, personas/, fixtures/)."""

import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from cedar_bench import paths

BENCH = Path(__file__).resolve().parent.parent


class BenchDirTest(unittest.TestCase):
    def setUp(self):
        paths.bench_dir.cache_clear()
        self.addCleanup(paths.bench_dir.cache_clear)

    def test_the_checkout_beside_the_package(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop(paths.ENV, None)
            self.assertEqual(paths.bench_dir(), BENCH)

    def test_the_environment_variable_wins_and_is_checked(self):
        with mock.patch.dict(os.environ, {paths.ENV: str(BENCH)}):
            self.assertEqual(paths.bench_dir(), BENCH)
        paths.bench_dir.cache_clear()
        with tempfile.TemporaryDirectory() as empty, mock.patch.dict(os.environ, {paths.ENV: empty}):
            with self.assertRaises(SystemExit):
                paths.bench_dir()

    def test_an_installed_package_searches_from_the_working_directory(self):
        site = Path(tempfile.mkdtemp()) / "cedar_bench" / "paths.py"  # not beside any data
        os.environ.pop(paths.ENV, None)
        cwd = os.getcwd()
        try:
            with mock.patch.object(paths, "__file__", str(site)):
                os.chdir(BENCH / "tasks" / "web")
                self.assertEqual(paths.bench_dir(), BENCH)
                paths.bench_dir.cache_clear()
                os.chdir(BENCH.parent)  # the repo root: finds ./bench
                self.assertEqual(paths.bench_dir(), BENCH)
                paths.bench_dir.cache_clear()
                os.chdir(tempfile.mkdtemp())
                with self.assertRaises(SystemExit) as e:
                    paths.bench_dir()
                self.assertIn(paths.ENV, str(e.exception))
        finally:
            os.chdir(cwd)


if __name__ == "__main__":
    unittest.main()
