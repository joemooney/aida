"""Exercise the CI runner against a checkout containing new Python test files."""
# trace:BUG-2734 | ai:codex
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUNNER = Path(__file__).resolve().parents[1] / "scripts/discover-python-tests.py"


class Discovery(unittest.TestCase):
    def test_new_suites_must_run_and_uncollected_files_refuse(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            workflow = root / ".github/workflows/ci.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text("jobs: {build: {steps: []}}")
            tests = root / "tests"
            tests.mkdir()

            def run():
                return subprocess.run([sys.executable, RUNNER], cwd=root, capture_output=True,
                                      text=True, timeout=15)

            self.assertNotEqual(run().returncode, 0)
            (tests / "test_new.py").write_text(
                "import unittest\nclass New(unittest.TestCase):\n"
                "    def test_new_regression(self): pass\n"
            )
            one = run()
            self.assertEqual(one.returncode, 0, one.stderr)
            self.assertIn("test_new_regression", one.stderr)
            (tests / "test_empty.py").touch()
            workflow.write_text("jobs: {build: {steps: "
                                "[{run: 'python3 -m unittest tests/test_empty.py'}]}}")
            many = run()
            self.assertNotEqual(many.returncode, 0)
            self.assertIn("test_empty.py: unittest discovered zero tests", many.stderr)
            (tests / "test_empty.py").unlink()
            (tests / "test_fails.py").write_text(
                "import unittest\nclass Fails(unittest.TestCase):\n"
                "    def test_failure(self): self.fail('regression ran')\n"
            )
            failed = run()
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("AssertionError: regression ran", failed.stderr)

            (tests / "test_fails.py").unlink()
            (tests / "test_show_latency.py").touch()
            workflow.write_text("jobs: {build: {steps: "
                                "[{run: 'python3 -m unittest tests/test_show_latency.py'}]}}")
            unwired = run()
            self.assertNotEqual(unwired.returncode, 0)
            self.assertIn("test_show_latency.py: missing standalone CI invocation", unwired.stderr)

    # trace:TASK-3856 | ai:codex
    # trace:TASK-3321 | ai:codex
    # trace:TASK-b6b1278890774ef993d2ebc327862470-1292.AC1 | ai:codex
    # trace:TASK-b6b1278890774ef993d2ebc327862470-1292.AC2 | ai:codex
    def test_only_runnable_steps_supply_dedicated_invocations(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            workflow = root / ".github/workflows/ci.yml"
            workflow.parent.mkdir(parents=True)
            tests = root / "tests"
            tests.mkdir()
            (tests / "test_show_latency.py").touch()
            for condition, wired in [
                ("false", False), ("${{ false }}", False), ("'false'", False),
                ("${{ true && false }}", False), ("true", True),
                ("matrix.shard == 'gates'", True), ("on", False),
                ("matrix.shard == 'contracts'", False),
            ]:
                with self.subTest(condition=condition):
                    workflow.write_text(
                        "jobs:\n  build:\n    strategy:\n      matrix:\n        shard: [gates]\n"
                        f"    steps:\n      - if: {condition}\n"
                        "        run: python3 tests/test_show_latency.py\n"
                    )
                    result = subprocess.run([sys.executable, RUNNER], cwd=root,
                                            capture_output=True, text=True, timeout=15)
                    self.assertEqual(result.returncode, int(not wired), result.stderr)
                    self.assertEqual("missing standalone CI invocation" in result.stderr,
                                     not wired)
