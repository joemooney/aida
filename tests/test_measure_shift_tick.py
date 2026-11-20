"""BUG-2626: quick/disabled ticks cannot satisfy cron acceptance."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/measure-shift-tick.py"


class ShiftMeasurement(unittest.TestCase):
    def test_requires_a_successful_real_tick_report(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            ledger = root / ".aida/gate-ledger"
            (ledger / "by-spec").mkdir(parents=True)
            (ledger / "PR-1.jsonl").write_text("fixture\n")
            binary = root / "aida"
            for report, status, stderr, accepted in [
                ({"enabled": True, "dry_run": False}, 0, "", True),
                ({"enabled": False, "ran": False}, 0, "", False),
                ({"enabled": True, "dry_run": True}, 0, "", False),
                ({"enabled": True, "dry_run": False}, 1, "", False),
                ({}, 0, "", False),
                ({"enabled": True, "dry_run": False}, 0, "shift hold discovery refused: missing certificate", False),
            ]:
                with self.subTest(report=report, status=status):
                    binary.write_text("#!/bin/sh\nprintf '%s\\n' '" + json.dumps(report) + "'\nprintf '%s\\n' '" + stderr + "' >&2\nexit " + str(status) + "\n")
                    binary.chmod(0o755)
                    result = subprocess.run(
                        ["python3", str(SCRIPT), "--root", str(root),
                         "--binary", str(binary), "--deployed-sha", "fixture"],
                        capture_output=True, text=True, check=False,
                    )
                    evidence = json.loads(result.stdout)
                    self.assertEqual(evidence["accepted"], accepted)
                    self.assertEqual(result.returncode, 0 if accepted else 1)
                    self.assertEqual(evidence["change_ledgers"], 1)
                    self.assertEqual(evidence["ledger_bytes"], 8)
                    self.assertLess(evidence["elapsed_seconds"], 120)


if __name__ == "__main__":
    unittest.main()
