#!/usr/bin/env python3
"""Unit tests for the STORY-1853 test-speed tooling (no cargo).

- scripts/test-timings.py: per-binary timings out of a `cargo test` log, and
  the slow-test budget gate;
- scripts/check-ci-gates.py: a local `make check-ci` runs CI's `cargo test`
  on the agent profile, and the compile gate still derives from the teed
  `Run tests` step.

trace:STORY-1853 | ai:claude
"""

import importlib.util
import pathlib
import sys
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[1]


def load(name, rel):
    spec = importlib.util.spec_from_file_location(name, REPO / rel)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod  # dataclasses resolve their module by name
    spec.loader.exec_module(mod)
    return mod


tt = load("test_timings", "scripts/test-timings.py")
gates = load("check_ci_gates_1853", "scripts/check-ci-gates.py")

ESC = "\x1b"
HDR = ESC + "[1m" + ESC + "[92m     Running" + ESC + "[0m "


def result(passed, secs, ignored=0, filtered=0):
    return (f"test result: ok. {passed} passed; 0 failed; {ignored} ignored; 0 measured; "
            f"{filtered} filtered out; finished in {secs}s")


class ParseTest(unittest.TestCase):
    def test_plain_log(self):
        log = [
            HDR + "unittests src/lib.rs (target/debug/deps/aida_core-9626726ab6ba6650)",
            "", "running 3 tests", "test a ... ok", "", result(2, "1.50", ignored=1), "",
            HDR + "tests/journey.rs (target/debug/deps/journey-0123abcd)",
            "running 1 test", result(1, "14.31"),
            "   Doc-tests aida_core", "running 2 tests", result(0, "0.00", ignored=2),
        ]
        rows = tt.parse_log(log)
        self.assertEqual([(r.key, r.passed, r.ignored, r.seconds) for r in rows], [
            ("aida_core:src/lib.rs", 2, 1, 1.5),
            ("journey:tests/journey.rs", 1, 0, 14.31),
            ("doctests:aida_core", 0, 2, 0.0),
        ])

    def test_next_header_before_previous_result_is_still_attributed(self):
        # Run 39186737460: cargo's stderr header for context_budget landed
        # before aida_core's stdout result, crediting aida_core's 1,552 tests
        # to context_budget.
        log = [
            HDR + "unittests src/lib.rs (target/debug/deps/aida_core-96)",
            "running 1552 tests",
            HDR + "tests/context_budget.rs (target/debug/deps/context_budget-30)",
            result(1549, "39.75", ignored=3),
            "running 1 test",
            result(1, "0.02"),
        ]
        rows = {r.key: r for r in tt.parse_log(log)}
        self.assertEqual(rows["aida_core:src/lib.rs"].seconds, 39.75)
        self.assertEqual(rows["context_budget:tests/context_budget.rs"].seconds, 0.02)

    def test_child_reexec_results_do_not_replace_the_binarys_own(self):
        log = [
            HDR + "unittests src/lib.rs (target/debug/deps/aida_cli_lib-2f)",
            "running 9183 tests",
            "running 1 test", result(1, "0.00", filtered=9182),
            "running 1 test", result(1, "0.01", filtered=9182),
            result(9180, "274.83", ignored=3),
        ]
        rows = tt.parse_log(log)
        self.assertEqual(len(rows), 1)
        self.assertEqual((rows[0].passed, rows[0].seconds), (9180, 274.83))

    def test_gh_download_prefixes_and_step_filter(self):
        pre = "Build (ubuntu-latest)\t{}\t2026-10-19T01:51:31.5Z "
        log = [
            pre.format("Run tests") + HDR + "tests/a.rs (target/debug/deps/a-ff)",
            pre.format("Run tests") + "running 2 tests",
            pre.format("Run tests") + result(2, "3.00"),
            pre.format("Run other") + HDR + "unittests src/lib.rs (target/debug/deps/aida_cli_lib-ee)",
            pre.format("Run other") + "running 1 test",
            pre.format("Run other") + result(1, "0.10", filtered=9000),
        ]
        self.assertEqual(len(tt.parse_log(log)), 2)
        rows = tt.parse_log(tt.step_lines(log, "Run tests"))
        self.assertEqual([(r.key, r.seconds) for r in rows], [("a:tests/a.rs", 3.0)])


class BudgetTest(unittest.TestCase):
    def rows(self, **secs):
        return [tt.Binary(k, 1, 0, 0, s) for k, s in secs.items()]

    def test_unbudgeted_slow_binary_fails(self):
        v, _ = tt.check(self.rows(slow=12.0, fast=3.0), tt.Budget(10, {}))
        self.assertEqual(len(v), 1)
        self.assertIn("slow: 12.0s with no budget entry", v[0])

    def test_over_budget_fails_and_within_budget_passes(self):
        b = tt.Budget(10, {"slow": 20, "slower": 30})
        v, _ = tt.check(self.rows(slow=21.0, slower=29.0), b)
        self.assertEqual(len(v), 1)
        self.assertIn("slow: 21.0s is over its budget of 20s", v[0])

    def test_budgeted_binary_that_did_not_run_is_a_note(self):
        v, notes = tt.check(self.rows(a=1.0), tt.Budget(10, {"gone": 20}))
        self.assertEqual(v, [])
        self.assertEqual(notes, ["gone: budgeted but did not run in this log"])

    def test_committed_budget_file_loads(self):
        b = tt.load_budget(str(REPO / "scripts/test-time-budget.toml"))
        self.assertEqual(b.default_limit, 10)
        self.assertIn("aida_cli_lib:src/lib.rs", b.entries)
        self.assertTrue(all(v > b.default_limit for v in b.entries.values()))

    def test_main_exit_codes_and_outputs(self):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d)
            (p / "log").write_text("\n".join([
                HDR + "tests/a.rs (target/debug/deps/a-ff)", "running 1 test", result(1, "11.00"),
            ]) + "\n")
            (p / "budget.toml").write_text('default_limit = 10\n[budget]\n"a:tests/a.rs" = 15\n')
            (p / "empty").write_text("nothing here\n")
            args = [str(p / "log"), "--json", str(p / "t.json"), "--summary", str(p / "s.md")]
            self.assertEqual(tt.main(args + ["--budget", str(p / "budget.toml")]), 0)
            self.assertIn('"seconds": 11.0', (p / "t.json").read_text())
            self.assertIn("| `a:tests/a.rs` | 11.0 | 1 | 15 |", (p / "s.md").read_text())
            (p / "budget.toml").write_text("default_limit = 10\n")
            self.assertEqual(tt.main([str(p / "log"), "--budget", str(p / "budget.toml")]), 1)
            self.assertEqual(tt.main([str(p / "empty")]), 2)


class CheckCiProfileTest(unittest.TestCase):
    def test_cargo_test_runs_on_the_agent_profile_locally(self):
        self.assertEqual(
            gates.local_test_command("cargo test --workspace --no-fail-fast < /dev/null 2>&1 | tee target/test-run.log"),
            "cargo test --profile agent --workspace --no-fail-fast < /dev/null 2>&1 | tee target/test-run.log",
        )

    def test_explicit_profile_and_builds_are_left_alone(self):
        for cmd in ["cargo test --profile ci -p x", "cargo build --workspace", "cargo clippy --workspace"]:
            self.assertEqual(gates.local_test_command(cmd), cmd)

    def test_compile_gate_derives_from_the_teed_step(self):
        self.assertEqual(
            gates.derive_compile_command(
                "Run tests", "cargo test --workspace --no-fail-fast < /dev/null 2>&1 | tee target/test-run.log"),
            "cargo test --workspace --no-run --profile agent",
        )

    def test_a_different_pipe_is_still_refused(self):
        with self.assertRaises(gates.GateError):
            gates.derive_compile_command("Run tests", "cargo test --workspace | grep -v ok")


if __name__ == "__main__":
    unittest.main()
