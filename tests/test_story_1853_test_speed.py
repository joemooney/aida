#!/usr/bin/env python3
"""Unit tests for the STORY-1853 test-speed tooling (no cargo).

- scripts/test-timings.py: per-test timings out of nextest JUnit reports,
  per-binary timings out of a `cargo test` log, and the slow-test budget gate;
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

    def test_child_reexec_result_does_not_overwrite_an_earlier_finished_binary(self):
        # A later binary's child re-exec ran 1 test; the newest 1-test row is
        # slow_one, which already has its result and is no longer running.
        log = [
            HDR + "tests/slow_one.rs (target/debug/deps/slow_one-aa)",
            "running 1 test", result(1, "45.00"),
            HDR + "tests/reexec.rs (target/debug/deps/reexec-bb)",
            "running 3 tests",
            "running 1 test", result(1, "0.05", filtered=2),
            result(3, "2.00"),
        ]
        rows = {r.key: r for r in tt.parse_log(log)}
        self.assertEqual(rows["slow_one:tests/slow_one.rs"].seconds, 45.0)
        self.assertEqual(rows["reexec:tests/reexec.rs"].seconds, 2.0)
        v, _ = tt.check(list(rows.values()), tt.Budget(10, {}))
        self.assertEqual(len(v), 1)
        self.assertIn("slow_one:tests/slow_one.rs: 45.0s with no budget entry", v[0])

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

    def test_committed_budget_file_loads(self):
        b = tt.load_budget(str(REPO / "scripts/test-time-budget.toml"))
        self.assertEqual(b.default_limit, 10)
        self.assertTrue(all(v > b.default_limit for v in b.entries.values()))
        # Per-test keys (STORY-1853 AC4): `<nextest binary id> <test name>`.
        # trace:TASK-1988 | ai:claude
        self.assertTrue(all(k.startswith("doctests:") or " " in k for k in b.entries), b.entries)

    # trace:TASK-1988 | ai:claude
    def test_overlapping_over_budget_tests_are_a_stall_and_a_lone_one_fails(self):
        def row(key, start, secs):
            return tt.Binary(key, 1, 0, 0, secs, f"2026-10-20T20:39:{start:02d}.000+00:00")
        b = tt.Budget(10, {})
        # Run 39492854022: three tests that take 1-2 s went to 18 s together.
        rows = [row("x a", 10, 18.7), row("x b", 11, 17.7), row("x c", 11, 17.9), row("x fine", 12, 1.0)]
        v, notes = tt.check(rows, b)
        self.assertEqual(v, [])
        self.assertEqual(sum(n.startswith("runner stall") for n in notes), 3)
        # Over budget at different times: each is a regression, not a stall.
        v, _ = tt.check([row("x a", 0, 12.0), row("x b", 30, 12.0)], b)
        self.assertEqual(len(v), 2)
        # A log row has no timestamp and never clusters.
        v, _ = tt.check([tt.Binary("doctests:x", 1, 0, 0, 12.0), tt.Binary("doctests:y", 1, 0, 0, 12.0)], b)
        self.assertEqual(len(v), 2)

    def test_note_only_for_a_budgeted_test_whose_binary_ran(self):
        b = tt.Budget(10, {"bin-a gone": 20, "bin-b other_lane": 20, "aida-cli-lib other::half": 20})
        # The lib binary runs split across two lanes: its other half is silent.
        v, notes = tt.check([tt.Binary("bin-a kept", 1, 0, 0, 1.0),
                             tt.Binary("aida-cli-lib this::half", 1, 0, 0, 1.0)], b)
        self.assertEqual(v, [])
        self.assertEqual(notes, ["bin-a gone: budgeted but did not run in this log"])

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



JUNIT = """<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="3" failures="1" errors="0">
  <testsuite name="aida-cli::journey" tests="2" failures="1">
    <testcase name="slow_one" classname="aida-cli::journey" time="12.500"/>
    <testcase name="broken" classname="aida-cli::journey" time="0.200"><failure type="test failure"/></testcase>
  </testsuite>
  <testsuite name="aida-cli-lib" tests="1">
    <testcase name="cli::tests::fast" classname="aida-cli-lib" time="0.010"/>
  </testsuite>
</testsuites>
"""


class JunitTest(unittest.TestCase):
    """trace:TASK-1988 | ai:claude"""

    def test_rows_are_per_test_with_failures(self):
        with tempfile.TemporaryDirectory() as d:
            path = pathlib.Path(d) / "junit-cli-0-0.xml"
            path.write_text(JUNIT)
            rows = tt.parse_junit(str(path))
        self.assertEqual([(r.key, r.passed, r.failed, r.seconds) for r in rows], [
            ("aida-cli::journey slow_one", 1, 0, 12.5),
            ("aida-cli::journey broken", 0, 1, 0.2),
            ("aida-cli-lib cli::tests::fast", 1, 0, 0.01),
        ])

    def test_main_budgets_junit_rows_and_keeps_only_doctests_from_the_log(self):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d)
            (p / "junit.xml").write_text(JUNIT)
            # A nextest step log: the doctest run is cargo test's, and a
            # child's libtest lines in captured output are not a binary.
            (p / "log").write_text("\n".join([
                "+ cargo nextest run --no-fail-fast --profile ci -p aida-cli",
                HDR + "tests/x.rs (target/debug/deps/x-ff)", "running 1 test", result(1, "50.00"),
                "+ cargo test --no-fail-fast --doc -p aida-cli-lib",
                "   Doc-tests aida_cli_lib", "running 2 tests", result(2, "0.30"),
            ]) + "\n")
            (p / "budget.toml").write_text('default_limit = 10\n[budget]\n"aida-cli::journey slow_one" = 15\n')
            args = [str(p / "log"), "--junit", str(p / "junit.xml"), str(p / "missing.xml"),
                    "--json", str(p / "t.json"), "--budget", str(p / "budget.toml")]
            self.assertEqual(tt.main(args), 0)
            keys = [r["key"] for r in __import__("json").loads((p / "t.json").read_text())["rows"]]
            self.assertEqual(sorted(keys), sorted([
                "doctests:aida_cli_lib", "aida-cli::journey slow_one",
                "aida-cli::journey broken", "aida-cli-lib cli::tests::fast"]))
            (p / "budget.toml").write_text("default_limit = 10\n")
            self.assertEqual(tt.main(args), 1)
            (p / "bad.xml").write_text("<testsuites>")
            self.assertEqual(tt.main([str(p / "log"), "--junit", str(p / "bad.xml")]), 2)


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
