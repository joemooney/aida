#!/usr/bin/env python3
"""Regression tests for the doc-intent surface detector."""

import importlib.util
import io
from pathlib import Path
import unittest
from unittest import mock


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
    # trace:BUG-1238 | ai:codex
    def test_cli_lib_change_without_doc_impact_fails(self):
        with (
            mock.patch.object(
                verify_interface_changes,
                "resolve_range",
                return_value=("base", "head"),
            ),
            mock.patch.object(
                verify_interface_changes,
                "changed_files",
                return_value=["aida-cli-lib/src/cli.rs"],
            ),
            mock.patch.object(
                verify_interface_changes,
                "referenced_specs",
                return_value={"BUG-1238"},
            ),
            mock.patch.object(
                verify_interface_changes,
                "load_spec_yaml",
                return_value="id: BUG-1238\ntags:\n  - ci\ninterface_changes:\n",
            ),
            mock.patch("sys.stdout", new_callable=io.StringIO) as stdout,
        ):
            with self.assertRaises(SystemExit) as exit_context:
                verify_interface_changes.main()

        self.assertEqual(exit_context.exception.code, 1)
        self.assertIn("NO referenced spec marks doc-impact", stdout.getvalue())

    def test_legacy_cli_path_still_arms_gate(self):
        changed = ["aida-cli/src/cli.rs"]

        self.assertEqual(verify_interface_changes.touches_surface(changed), changed)


if __name__ == "__main__":
    unittest.main()
