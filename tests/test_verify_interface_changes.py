#!/usr/bin/env python3
"""Regression tests for the doc-intent surface detector."""

import importlib.util
from pathlib import Path
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

    def test_legacy_cli_path_still_arms_gate(self):
        changed = ["aida-cli/src/cli.rs"]

        self.assertEqual(verify_interface_changes.touches_surface(changed), changed)


if __name__ == "__main__":
    unittest.main()
