import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from stage_root_test import select_executable


class RootArtifactTests(unittest.TestCase):
    def artifact(self, path="test-binary", test=True, name="ltbox_patch"):
        return json.dumps({"reason": "compiler-artifact", "target": {"name": name},
                           "profile": {"test": test}, "executable": path})

    def test_selects_only_current_test_artifact(self):
        lines = [self.artifact(test=False), self.artifact(name="other"),
                 json.dumps({"reason": "build-finished", "success": True}), self.artifact()]
        self.assertEqual(select_executable(lines), Path("test-binary"))

    def test_missing_or_ambiguous_artifacts_fail(self):
        for lines in [[], [self.artifact(path=None)],
                      [self.artifact(), self.artifact(path="another")]]:
            with self.assertRaises(ValueError):
                select_executable(lines)


class CacheMetricTests(unittest.TestCase):
    def test_timing_preserves_command_failures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            environment = dict(os.environ, CI_METRICS_DIR=directory)
            environment.pop("RUNNER_TEMP", None)
            result = subprocess.run(
                [sys.executable, str(Path(__file__).with_name("cache_metrics.py")),
                 "run", "failed-test", sys.executable, "-c", "raise SystemExit(7)"],
                env=environment, check=False,
            )
            self.assertEqual(result.returncode, 7)
            row = json.loads((root / "failed-test.json").read_text())
            self.assertEqual(row["exit_code"], 7)
            self.assertGreaterEqual(row["seconds"], 0)


if __name__ == "__main__":
    unittest.main()
