#!/usr/bin/env python3
"""Regression tests for the doc-intent surface detector."""

import importlib.util
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / "docs" / "cli" / "verify-interface-changes.py"

spec = importlib.util.spec_from_file_location("verify_interface_changes", SCRIPT)
verify_interface_changes = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(verify_interface_changes)


class SurfacePathTests(unittest.TestCase):
    # trace:BUG-1238 | ai:codex
    def test_cli_lib_path_arms_gate(self):
        changed = ["aida-cli-lib/src/cli.rs"]

        self.assertEqual(verify_interface_changes.touches_surface(changed), changed)


class GateBehaviorTests(unittest.TestCase):
    # trace:BUG-1582 | ai:codex
    def test_agreed_id_resolves_filename_with_different_spec_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._create_synthetic_repo(
                repo,
                "spec_id: TASK-1-164\nagreed_id: TASK-185\ntags:\n  - docs:impacted\n",
                filename="TASK-1-164.yaml",
                referenced_id="TASK-185",
            )

            result = self._run_gate(repo)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("doc-impact marked on: TASK-185", result.stdout)

    # trace:BUG-1582 | ai:codex
    def test_native_spec_id_still_resolves(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._create_synthetic_repo(
                repo,
                "spec_id: BUG-9001\nagreed_id: BUG-42\ntags:\n  - docs:impacted\n",
                referenced_id="BUG-9001",
            )

            result = self._run_gate(repo)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    # trace:BUG-1582 | ai:codex
    def test_identifier_collision_fails_closed_as_ambiguous(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._create_synthetic_repo(
                repo,
                "spec_id: TASK-1-164\nagreed_id: TASK-185\ntags:\n  - docs:impacted\n",
                filename="TASK-1-164.yaml",
                referenced_id="TASK-185",
                extra_specs={
                    "TASK-185.yaml": (
                        "spec_id: TASK-185\nagreed_id: TASK-999\n"
                        "tags:\n  - docs:impacted\n"
                    )
                },
            )

            result = self._run_gate(repo)

            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("ambiguous spec identifier", result.stdout)
            self.assertIn("TASK-1-164.yaml", result.stdout)
            self.assertIn("TASK-185.yaml", result.stdout)

    # trace:BUG-1582 | ai:codex
    def test_missing_spec_fails_closed_and_is_distinct_from_unmarked(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._create_synthetic_repo(
                repo,
                "spec_id: BUG-9001\nagreed_id: BUG-9001\ntags:\n  - docs:impacted\n",
                referenced_id="BUG-9999",
            )

            result = self._run_gate(repo)

            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("spec not found: BUG-9999", result.stdout)
            self.assertNotIn("NO referenced spec marks doc-impact", result.stdout)

    # trace:BUG-1238 | ai:codex
    def test_cli_lib_diff_enforces_doc_impact_from_store(self):
        cases = {
            "unmarked": ("tags:\n  - ci\ninterface_changes:\n", 1),
            "docs_impacted": ("tags:\n  - docs:impacted\n", 0),
            "interface_changes": (
                "tags:\n  - ci\ninterface_changes:\n  cli:\n    - Added aida synthetic\n",
                0,
            ),
        }

        for name, (metadata, expected_exit) in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                repo = Path(tmp)
                self._create_synthetic_repo(repo, metadata)

                result = subprocess.run(
                    [
                        sys.executable,
                        "docs/cli/verify-interface-changes.py",
                        "HEAD~1",
                        "HEAD",
                    ],
                    cwd=repo,
                    capture_output=True,
                    text=True,
                    check=False,
                )

                self.assertEqual(
                    result.returncode,
                    expected_exit,
                    msg=f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}",
                )
                self.assertIn("aida-cli-lib/src/cli.rs", result.stdout)

    @staticmethod
    def _run_gate(repo):
        return subprocess.run(
            [sys.executable, "docs/cli/verify-interface-changes.py", "HEAD~1", "HEAD"],
            cwd=repo,
            capture_output=True,
            text=True,
            check=False,
        )

    @staticmethod
    def _create_synthetic_repo(
        repo,
        metadata,
        filename="BUG-9001.yaml",
        referenced_id="BUG-9001",
        extra_specs=None,
    ):
        def git(*args):
            subprocess.run(
                ["git", *args], cwd=repo, check=True, capture_output=True, text=True
            )

        git("init", "-b", "main")
        git("config", "user.name", "AIDA test")
        git("config", "user.email", "aida-test@example.invalid")

        script = repo / "docs" / "cli" / "verify-interface-changes.py"
        script.parent.mkdir(parents=True)
        shutil.copy2(SCRIPT, script)
        cli = repo / "aida-cli-lib" / "src" / "cli.rs"
        cli.parent.mkdir(parents=True)
        cli.write_text("// base CLI surface\n")
        git("add", ".")
        git("commit", "-m", "test: establish base")

        git("checkout", "--orphan", "aida-store")
        git("rm", "-rf", ".")
        spec_path = repo / "objects" / "bug" / filename
        spec_path.parent.mkdir(parents=True)
        spec_path.write_text(metadata)
        for extra_filename, extra_metadata in (extra_specs or {}).items():
            (spec_path.parent / extra_filename).write_text(extra_metadata)
        git("add", ".")
        git("commit", "-m", "test: add synthetic spec")

        git("checkout", "main")
        cli.write_text("// base CLI surface\n// synthetic flag added\n")
        git("add", "aida-cli-lib/src/cli.rs")
        git("commit", "-m", f"test: change CLI surface ({referenced_id})")

    def test_legacy_cli_path_still_arms_gate(self):
        changed = ["aida-cli/src/cli.rs"]

        self.assertEqual(verify_interface_changes.touches_surface(changed), changed)


if __name__ == "__main__":
    unittest.main()
