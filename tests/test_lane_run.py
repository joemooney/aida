"""The host entry point never starts a second publisher after queue work.

trace:BUG-2648 | ai:codex
"""
from pathlib import Path
import subprocess
import tempfile
import unittest


class LaneRunTests(unittest.TestCase):
    def test_terminal_outcomes_never_trigger_fallback_publication(self):
        wrapper = Path(__file__).resolve().parents[1] / "scripts/lane-run.sh"
        for outcome, code in [("pushed", 0), ("no-change", 0),
                              ("punted", 0), ("refused-before-start", 1),
                              ("failed-after-start", 137)]:
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                stub = root / "aida"
                stub.write_text('#!/bin/bash\n'
                                'printf "%s\\n" "$*" >> "$CALLS"\n'
                                'echo "implementer: $OUTCOME"\n'
                                'exit "$CODE"\n')
                stub.chmod(0o700)
                calls = root / "calls"
                env = dict(PATH=f"{root}:/usr/bin:/bin",
                           CALLS=str(calls), OUTCOME=outcome, CODE=str(code))
                result = subprocess.run(["bash", str(wrapper), "BUG-2648"],
                                        env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, code)
                self.assertEqual(calls.read_text(), "queue work BUG-2648\n")


if __name__ == "__main__":
    unittest.main()
