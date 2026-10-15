#!/usr/bin/env python3
"""Unit tests for scripts/test-touched.py (no git, no cargo).

Two fixtures: a small synthetic workspace written to a temp dir, and the real
repository's sources (its `#[path]` test modules, `include!`d `lib_part*.rs`,
build-script inputs and `include_str!` guard tests), so the mapping is checked
against the shapes it must handle here.

trace:TASK-1780 | ai:claude
"""

import importlib.util
import os
import pathlib
import re
import sys
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO / "scripts/test-touched.py"
SPEC = importlib.util.spec_from_file_location("test_touched", SCRIPT)
tt = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
sys.modules[SPEC.name] = tt  # dataclasses resolve their module by name
SPEC.loader.exec_module(tt)

BASE = ["cargo", "test", "--profile", "agent"]


def meta(root, packages):
    """cargo-metadata-shaped dict: packages = [(name, dir, [(kinds, tname, src)])]."""
    return {
        "workspace_root": str(root),
        "workspace_members": [n for n, _, _ in packages],
        "packages": [
            {"id": n, "name": n, "manifest_path": f"{root}/{d}/Cargo.toml",
             "targets": [{"kind": k, "name": t, "src_path": f"{root}/{d}/{s}"} for k, t, s in targets]}
            for n, d, targets in packages
        ] + [{"id": "dep", "name": "serde", "manifest_path": "/reg/serde/Cargo.toml", "targets": []}],
    }


SYNTH_FILES = {
    "aida-core/build.rs": (
        'fn main() { println!("cargo:rerun-if-changed=templates/");\n'
        '  code.push_str("pub static EMBEDDED_TEMPLATES: X = Y;"); }\n'
    ),
    "aida-core/templates/skills/x.md": "x\n",
    "aida-core/src/lib.rs": 'pub mod db;\npub mod ai;\npub mod embed;\n#[path = "tests/x_tests.rs"]\nmod x_tests;\n',
    "aida-core/src/tests/x_tests.rs": "// tests went missing\n",
    "aida-core/src/db/mod.rs": "pub mod cache;\n",
    "aida-core/src/db/cache.rs": "#[cfg(test)]\nmod tests {}\n",
    "aida-core/src/ai/mod.rs": "\n",
    "aida-core/src/orphan.rs": "// declared nowhere\n",
    "aida-core/src/embed.rs": 'const S: &str = include_str!("embed/schema.sql");\n',
    "aida-core/src/embed/schema.sql": "create table t();\n",
    "aida-core/tests/context_budget.rs": "\n",
    "aida-cli/src/main.rs": "mod grpc;\nmod scaffold;\nmod refresh;\n",
    "aida-cli/src/scaffold.rs": "use aida_core::EMBEDDED_TEMPLATES;\n#[cfg(test)]\nmod tests {}\n",
    "aida-cli/src/refresh.rs": "fn f() { aida_core::EMBEDDED_TEMPLATES.len(); }\n",
    "aida-cli/src/grpc.rs": "\n",
    "aida-cli/tests/agent_write_confirm.rs": "\n",
    "aida-cli/tests/common/mod.rs": "\n",
    "aida-types/build.rs": "fn main() {}\n",
    "docs/a.md": "\n",
}


class SynthTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(cls.tmp.name)
        for rel, text in SYNTH_FILES.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        cls.pkgs = tt.load_packages(meta(root, [
            ("aida-core", "aida-core", [
                (["lib"], "aida_core", "src/lib.rs"),
                (["test"], "context_budget", "tests/context_budget.rs"),
                (["custom-build"], "build-script-build", "build.rs"),
            ]),
            ("aida-cli", "aida-cli", [
                (["bin"], "aida", "src/main.rs"),
                (["test"], "agent_write_confirm", "tests/agent_write_confirm.rs"),
            ]),
            ("aida-types", "aida-types", [(["custom-build"], "build-script-build", "build.rs")]),
        ]))
        cls.idx = tt.build_index(str(root), cls.pkgs)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_plan(self, changed, extra=()):
        return tt.plan(changed, self.pkgs, "agent", list(extra), self.idx)

    def test_docs_only_plan_nothing(self):
        p = self.run_plan(["docs/a.md", "README.md"])
        self.assertEqual(p.commands, [])
        self.assertEqual(p.untargetable, [])
        self.assertTrue(any("2 non-Rust" in n for n in p.notes))

    def test_declared_module_files_become_filters(self):
        p = self.run_plan(["aida-core/src/db/cache.rs", "aida-core/src/ai/mod.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib", "--", "ai", "db::cache"]])
        self.assertEqual(p.derived, {"aida-core": {"ai", "db::cache"}})

    def test_undeclared_module_falls_back_to_unfiltered(self):
        p = self.run_plan(["aida-core/src/orphan.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib"]])
        self.assertTrue(any("not resolvable" in n for n in p.notes))

    def test_crate_root_runs_unit_tests_unfiltered(self):
        p = self.run_plan(["aida-core/src/lib.rs", "aida-core/src/db/cache.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib"]])
        self.assertEqual(p.derived, {})

    def test_integration_test_file_runs_its_binary(self):
        p = self.run_plan(["aida-cli/tests/agent_write_confirm.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-cli", "--test", "agent_write_confirm"]])

    def test_test_helper_runs_all_integration_tests(self):
        p = self.run_plan(["aida-cli/tests/common/mod.rs", "aida-cli/tests/agent_write_confirm.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-cli", "--test", "*"]])

    def test_bin_only_package_uses_bins(self):
        p = self.run_plan(["aida-cli/src/grpc.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-cli", "--bins", "--", "grpc"]])

    def test_package_build_inputs_run_unfiltered(self):
        p = self.run_plan(["aida-core/Cargo.toml"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib"]])

    def test_build_inputs_also_run_generated_symbol_consumers(self):
        for f in ["aida-core/build.rs", "aida-core/templates/skills/x.md"]:
            with self.subTest(f=f):
                p = self.run_plan([f])
                self.assertEqual(p.commands, [
                    BASE + ["-p", "aida-cli", "--bins", "--", "refresh", "scaffold"],
                    BASE + ["-p", "aida-core", "--lib"],
                ])
                self.assertEqual(p.consumer, {"aida-cli": {"refresh", "scaffold"}})
                self.assertEqual(p.suspect, {"aida-cli": {"scaffold"}})

    def test_non_rust_include_str_input_runs_unfiltered(self):
        p = self.run_plan(["aida-core/src/embed/schema.sql"])
        self.assertEqual(p.consumer, {})
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib"]])

    def test_workspace_root_config_is_untargetable(self):
        for f in ["Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml", "rust-toolchain"]:
            with self.subTest(f=f):
                p = self.run_plan([f, "aida-core/src/db/cache.rs"])
                self.assertEqual(p.untargetable, [f])
                self.assertEqual(tt.execute(p, [], False, self.fail_run, self.fail_list), 4)

    def test_package_without_lib_or_bin_is_noted_when_skipped(self):
        p = self.run_plan(["aida-types/build.rs"])
        self.assertEqual(p.commands, [])
        self.assertTrue(any("aida-types: no lib or bin target" in n for n in p.notes))

    def test_outside_rust_file_is_a_note(self):
        p = self.run_plan(["tools/x.rs"])
        self.assertEqual(p.commands, [])
        self.assertTrue(any("not in a workspace package" in n for n in p.notes))

    def test_extra_filters_are_appended(self):
        p = self.run_plan(["aida-core/src/db/cache.rs"], extra=["story_1463"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib", "--", "db::cache", "story_1463"]])

    def test_non_member_packages_are_ignored(self):
        self.assertNotIn("serde", [p.name for p in self.pkgs])

    # execute(): the zero-match refusal
    def fail_run(self, cmd):
        self.fail(f"must not run {cmd}")

    def fail_list(self, cmd):
        self.fail(f"must not list {cmd}")

    def listing(self, *names):
        return lambda cmd: (0, "".join(f"{n}: test\n" for n in names) + "\n9 tests, 0 benchmarks\n")

    def test_untested_clean_module_falls_back_to_unfiltered(self):
        # ai/mod.rs holds no tests and was mapped conventionally: not suspect.
        p = self.run_plan(["aida-core/src/ai/mod.rs", "aida-core/src/db/cache.rs"])
        self.assertEqual(p.suspect, {"aida-core": {"db::cache"}})
        ran, listed = [], []

        def list_tests(cmd):
            listed.append(cmd)
            return self.listing("db::cache::tests::t1", "other::t")(cmd)
        rc = tt.execute(p, [], False, lambda c: ran.append(c) or 0, list_tests)
        self.assertEqual(rc, 0)
        self.assertEqual(listed, [BASE + ["-p", "aida-core", "--lib", "--", "--list"]])
        self.assertEqual(ran, [BASE + ["-p", "aida-core", "--lib"]])

    def test_zero_match_on_a_file_holding_tests_fails(self):
        p = self.run_plan(["aida-core/src/db/cache.rs"])
        self.assertEqual(tt.execute(p, [], False, self.fail_run, self.listing("other::t")), 3)

    def test_zero_match_on_a_path_mapped_file_fails(self):
        p = self.run_plan(["aida-core/src/tests/x_tests.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-core", "--lib", "--", "x_tests"]])
        self.assertEqual(tt.execute(p, [], False, self.fail_run, self.listing("other::t")), 3)

    def test_untested_consumer_is_dropped(self):
        p = self.run_plan(["aida-core/templates/skills/x.md"])
        ran = []
        rc = tt.execute(p, [], False, lambda c: ran.append(c) or 0, self.listing("scaffold::tests::t"))
        self.assertEqual(rc, 0)
        self.assertEqual(ran, [BASE + ["-p", "aida-cli", "--bins", "--", "scaffold"],
                               BASE + ["-p", "aida-core", "--lib"]])

    def test_consumer_holding_tests_with_zero_match_fails(self):
        p = self.run_plan(["aida-core/templates/skills/x.md"])
        self.assertEqual(tt.execute(p, [], False, self.fail_run, self.listing("other::t")), 3)

    def test_filters_that_all_match_run(self):
        p = self.run_plan(["aida-core/src/db/cache.rs"])
        ran = []
        rc = tt.execute(p, [], False, lambda c: ran.append(c) or 0,
                        lambda c: (0, "db::cache::tests::t1: test\n"))
        self.assertEqual(rc, 0)
        self.assertEqual(ran, p.commands)

    def test_extra_only_filters_matching_nothing_fail(self):
        p = self.run_plan(["aida-core/src/lib.rs"], extra=["nope"])
        self.assertEqual(tt.execute(p, ["nope"], False, self.fail_run,
                                    lambda c: (0, "db::cache::tests::t1: test\n")), 3)

    def test_unfiltered_runs_skip_the_listing(self):
        p = self.run_plan(["aida-core/src/lib.rs"])
        self.assertEqual(tt.execute(p, [], False, lambda c: 0, self.fail_list), 0)

    def test_parse_test_list(self):
        out = "a::b: test\nc: benchmark\nrunning\nd::e: test\n"
        self.assertEqual(tt.parse_test_list(out), ["a::b", "d::e"])


def real_packages():
    return tt.load_packages(meta(REPO, [
        ("aida-core", "aida-core", [
            (["lib"], "aida_core", "src/lib.rs"),
            (["custom-build"], "build-script-build", "build.rs"),
        ]),
        ("aida-cli-lib", "aida-cli-lib", [
            (["lib"], "aida_cli_lib", "src/lib.rs"),
            (["custom-build"], "build-script-build", "build.rs"),
        ]),
        ("aida-server", "aida-server", [
            (["bin"], "aida-server", "src/main.rs"),
            (["custom-build"], "build-script-build", "build.rs"),
        ]),
    ]))


class RealRepoTest(unittest.TestCase):
    """The shapes the advisor found mis-mapped (TASK-1780 round 1), on the real tree."""

    @classmethod
    def setUpClass(cls):
        cls.pkgs = real_packages()
        cls.idx = tt.build_index(str(REPO), cls.pkgs)
        cls.cli = next(p for p in cls.pkgs if p.name == "aida-cli-lib")

    def run_plan(self, changed):
        return tt.plan(changed, self.pkgs, "agent", [], self.idx)

    def test_path_declared_test_maps_to_its_declaring_module(self):
        # role.rs: #[path = "tests/role_identity_tests.rs"] mod role_identity_tests;
        p = self.run_plan(["aida-cli-lib/src/tests/role_identity_tests.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-cli-lib", "--lib", "--", "role::role_identity_tests"]])

    def test_path_declared_in_an_included_part_maps_to_the_crate_root(self):
        # lib_part2.rs (include!d into lib.rs): #[path = "tests/glossary_render_tests.rs"]
        p = self.run_plan(["aida-cli-lib/src/tests/glossary_render_tests.rs"])
        self.assertEqual(p.commands, [BASE + ["-p", "aida-cli-lib", "--lib", "--", "glossary_render_tests"]])

    def test_included_lib_parts_run_unfiltered(self):
        for n in range(1, 7):
            with self.subTest(part=n):
                p = self.run_plan([f"aida-cli-lib/src/lib_part{n}.rs"])
                self.assertEqual(p.commands, [BASE + ["-p", "aida-cli-lib", "--lib"]])

    def test_every_path_declared_file_resolves_to_its_declared_name(self):
        decl = re.compile(r'^#\[path = "([^"]+)"\]\s*\n(?:pub(?:\([^)]*\))? )?mod (\w+);', re.M)
        seen = 0
        for f in sorted((REPO / "aida-cli-lib/src").rglob("*.rs")):
            for target, name in decl.findall(f.read_text(errors="replace")):
                rel = os.path.normpath(os.path.relpath(f.parent / target, REPO))
                mod = tt.module_path(rel, self.cli, self.idx)
                self.assertIsNotNone(mod, rel)
                self.assertEqual(mod[-1], name, rel)
                self.assertNotEqual(mod[0], "tests", f"{rel}: path-derived filter {mod}")
                seen += 1
        self.assertGreater(seen, 300)

    def test_inline_mod_tests_dir_maps_conventionally(self):
        # git_ops.rs: mod tests { mod store_hook_tests; }
        core = next(p for p in self.pkgs if p.name == "aida-core")
        self.assertEqual(tt.module_path("aida-core/src/git_ops/tests/store_hook_tests.rs", core, self.idx),
                         ["git_ops", "tests", "store_hook_tests"])

    def test_source_read_by_a_guard_test_adds_the_guard_module(self):
        p = self.run_plan(["aida-cli-lib/src/queue_cmd.rs"])
        self.assertEqual(len(p.commands), 1)
        self.assertIn("queue_cmd", p.derived["aida-cli-lib"])
        self.assertIn("headless_hint_tests", " ".join(p.derived["aida-cli-lib"]))

    def test_templates_run_aida_core_unit_tests_unfiltered(self):
        p = self.run_plan(["aida-core/templates/skills/aida-pr/SKILL.md"])
        self.assertIn(BASE + ["-p", "aida-core", "--lib"], p.commands)
        self.assertEqual(p.untargetable, [])

    def test_template_read_by_a_test_module_adds_that_module(self):
        # story_1350_review_approach_tests.rs include_str!s the aida-pickup skill.
        p = self.run_plan(["aida-core/templates/skills/aida-pickup.md"])
        self.assertIn(BASE + ["-p", "aida-core", "--lib"], p.commands)
        self.assertIn("story_1350_review_approach_tests", p.derived["aida-cli-lib"])

    def test_discipline_pack_change_runs_embedded_template_consumers(self):
        # #2671 round 1 broke these aida-cli-lib tests while aida-core's were green.
        p = self.run_plan(["aida-core/templates/.aida/discipline/README.md"])
        self.assertIn(BASE + ["-p", "aida-core", "--lib"], p.commands)
        cli = [c for c in p.commands if c[5] == "aida-cli-lib"]
        self.assertEqual(len(cli), 1)
        self.assertIn("story_255_discipline_pack_tests", cli[0])
        self.assertIn("scaffold_refresh", cli[0])  # reaches scaffold_refresh::tests
        self.assertIn("--", cli[0])

    def test_proto_runs_both_generated_client_packages(self):
        p = self.run_plan(["proto/aida.proto"])
        self.assertEqual(p.commands, [
            BASE + ["-p", "aida-cli-lib", "--lib"],
            BASE + ["-p", "aida-server", "--bins"],
        ])

    def test_build_scripts_and_manifests_run_unfiltered(self):
        p = self.run_plan(["aida-cli-lib/build.rs", "aida-core/Cargo.toml"])
        self.assertEqual(p.commands, [
            BASE + ["-p", "aida-cli-lib", "--lib"],
            BASE + ["-p", "aida-core", "--lib"],
        ])

    def test_root_build_config_is_untargetable(self):
        p = self.run_plan(["Cargo.lock"])
        self.assertEqual(p.untargetable, ["Cargo.lock"])


if __name__ == "__main__":
    unittest.main()
