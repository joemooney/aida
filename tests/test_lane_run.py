"""The host entry point never starts a second publisher after queue work.

trace:BUG-2648 | ai:codex
trace:TASK-3812 | ai:claude (stale-binary refusal)
"""
from pathlib import Path
import subprocess
import tempfile
import unittest

WRAPPER = Path(__file__).resolve().parents[1] / "scripts/lane-run.sh"

# `dev check-fresh` exits $FRESH (default 0); every other call exits $CODE.
STUB = ('#!/bin/bash\n'
        'printf "%s\\n" "$*" >> "$CALLS"\n'
        'if [ "$1 $2" = "dev check-fresh" ]; then exit "${FRESH:-0}"; fi\n'
        'echo "implementer: $OUTCOME"\n'
        'exit "$CODE"\n')


def run_wrapper(root, **extra):
    stub = root / "aida"
    stub.write_text(STUB)
    stub.chmod(0o700)
    calls = root / "calls"
    env = {"PATH": f"{root}:/usr/bin:/bin", "CALLS": str(calls),
           "OUTCOME": "pushed", "CODE": "0", **extra}
    result = subprocess.run(["bash", str(WRAPPER), "BUG-2648"],
                            env=env, capture_output=True, text=True)
    return result, calls.read_text().splitlines()


class LaneRunTests(unittest.TestCase):
    def test_terminal_outcomes_never_trigger_fallback_publication(self):
        for outcome, code in [("pushed", 0), ("no-change", 0),
                              ("punted", 0), ("refused-before-start", 1),
                              ("failed-after-start", 137)]:
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as tmp:
                result, calls = run_wrapper(Path(tmp), OUTCOME=outcome, CODE=str(code))
                self.assertEqual(result.returncode, code)
                self.assertEqual(calls, ["dev check-fresh", "queue work BUG-2648"])

    def test_stale_binary_refuses_before_queue_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            result, calls = run_wrapper(Path(tmp), FRESH="10")
            self.assertEqual(result.returncode, 1)
            self.assertEqual(calls, ["dev check-fresh"])
            self.assertIn("REFUSED", result.stderr)

    def test_unknown_freshness_or_older_binary_warns_and_continues(self):
        for fresh in ("1", "2", "3"):
            with self.subTest(fresh=fresh), tempfile.TemporaryDirectory() as tmp:
                result, calls = run_wrapper(Path(tmp), FRESH=fresh)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(calls, ["dev check-fresh", "queue work BUG-2648"])
                self.assertIn("warning", result.stderr)

    def test_override_skips_the_check_and_limits_pass_through(self):
        with tempfile.TemporaryDirectory() as tmp:
            result, calls = run_wrapper(Path(tmp), FRESH="10", AIDA_LANE_ALLOW_STALE="1")
            self.assertEqual(result.returncode, 0)
            self.assertEqual(calls, ["queue work BUG-2648"])
        with tempfile.TemporaryDirectory() as tmp:
            result, calls = run_wrapper(Path(tmp), AIDA_LANE_FRESH_ARGS="--max-hours 12")
            self.assertEqual(calls, ["dev check-fresh --max-hours 12", "queue work BUG-2648"])


if __name__ == "__main__":
    unittest.main()
