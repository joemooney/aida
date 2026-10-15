#!/usr/bin/env python3
"""Unit tests for scripts/test-touched.py's planner (no git, no cargo).

trace:TASK-1780 | ai:claude
"""

import importlib.util
import pathlib
import sys
import unittest

SCRIPT = pathlib.Path(__file__).parents[1] / "scripts/test-touched.py"
SPEC = importlib.util.spec_from_file_location("test_touched", SCRIPT)
tt = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
sys.modules[SPEC.name] = tt  # dataclasses resolve their module by name
SPEC.loader.exec_module(tt)

ROOT = "/ws"
META = {
    "workspace_root": ROOT,
    "workspace_members": ["core", "cli", "srv"],
    "packages": [
        {"id": "core", "name": "aida-core", "manifest_path": f"{ROOT}/aida-core/Cargo.toml", "targets": [
            {"kind": ["lib"], "name": "aida_core", "src_path": f"{ROOT}/aida-core/src/lib.rs"},
            {"kind": ["test"], "name": "context_budget", "src_path": f"{ROOT}/aida-core/tests/context_budget.rs"},
            {"kind": ["custom-build"], "name": "build-script-build", "src_path": f"{ROOT}/aida-core/build.rs"},
        ]},
        {"id": "cli", "name": "aida-cli", "manifest_path": f"{ROOT}/aida-cli/Cargo.toml", "targets": [
            {"kind": ["bin"], "name": "aida", "src_path": f"{ROOT}/aida-cli/src/main.rs"},
            {"kind": ["test"], "name": "agent_write_confirm", "src_path": f"{ROOT}/aida-cli/tests/agent_write_confirm.rs"},
        ]},
        {"id": "srv", "name": "aida-server", "manifest_path": f"{ROOT}/aida-server/Cargo.toml", "targets": [
            {"kind": ["bin"], "name": "aida-server", "src_path": f"{ROOT}/aida-server/src/main.rs"},
        ]},
        {"id": "dep", "name": "serde", "manifest_path": "/reg/serde/Cargo.toml", "targets": []},
    ],
}
PKGS = tt.load_packages(META)
BASE = ["cargo", "test", "--profile", "agent"]


def run(changed, extra=()):
    return tt.plan(changed, PKGS, "agent", list(extra))


class PlannerTest(unittest.TestCase):
    def test_docs_and_templates_only_plan_nothing(self):
        p = run(["docs/a.md", "aida-core/templates/skills/x.md"])
        self.assertEqual(p.commands, [])
        self.assertTrue(any("2 non-Rust" in n for n in p.notes))

    def test_module_files_become_filters(self):
        p = run(["aida-core/src/db/cache.rs", "aida-core/src/ai/mod.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib", "--", "ai", "db::cache"]])

    def test_crate_root_runs_unit_tests_unfiltered(self):
        p = run(["aida-core/src/lib.rs", "aida-core/src/db/cache.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib"]])

    def test_integration_test_file_runs_its_binary(self):
        p = run(["aida-cli/tests/agent_write_confirm.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-cli", "--test", "agent_write_confirm"]])

    def test_test_helper_runs_all_integration_tests(self):
        p = run(["aida-cli/tests/common/mod.rs", "aida-cli/tests/agent_write_confirm.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-cli", "--test", "*"]])

    def test_bin_only_package_uses_bins(self):
        p = run(["aida-server/src/grpc.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-server", "--bins", "--", "grpc"]])

    def test_build_script_and_outside_files_are_notes(self):
        p = run(["aida-core/build.rs", "tools/x.rs"])
        self.assertEqual(p.commands, [])
        self.assertEqual(len(p.notes), 2)

    def test_extra_filters_are_appended(self):
        p = run(["aida-core/src/db/cache.rs"], extra=["story_1463"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib", "--", "db::cache", "story_1463"]])

    def test_non_member_packages_are_ignored(self):
        self.assertNotIn("serde", [p.name for p in PKGS])


if __name__ == "__main__":
    unittest.main()
