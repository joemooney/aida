#!/usr/bin/env python3
"""Fixture tests for scripts/analyze_code_growth.py (TASK-1605).

trace:TASK-1605 | ai:claude

Everything runs in private temp dirs with a fake HOME/AIDA_HOME; the fixture
repository is created from scratch and never touches the real repo or AIDA
state.  Tokei-backed tests are skipped when no ``tokei`` binary is on PATH.
"""

import csv
import datetime as dt
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).parents[1] / "scripts/analyze_code_growth.py"
SPEC = importlib.util.spec_from_file_location("analyze_code_growth", SCRIPT)
mod = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(mod)

TOKEI = shutil.which("tokei")


def utc(y, m, d):
    return dt.datetime(y, m, d, 12, tzinfo=dt.timezone.utc)


def fake_commits(dates):
    return [{"sha": f"sha{i}", "when": d, "subject": f"c{i}"} for i, d in enumerate(dates)]


class ClassifyTest(unittest.TestCase):
    def check(self, expected):
        for path, bucket in expected.items():
            self.assertEqual(mod.classify(path), bucket, path)

    def test_components_are_exclusive_and_include_webui_and_tui(self):
        self.check({
            "aida-core/src/lib.rs": "core",
            "aida-cli-lib/src/lib.rs": "cli",
            "aida-cli/src/main.rs": "cli",
            "aida-server/src/main.rs": "server",
            "aida-tui/src/app.rs": "tui",
            "aida-gui/src/app.rs": "desktop_gui",
            "aida-web-react/src/App.tsx": "web_ui",
            "aida-web-react/index.html": "web_ui",
            "shared/types.ts": "web_ui",
            "aida-generate-types/src/main.rs": "developer_tooling",
            "proto/aida.proto": "core",
            "mystery/file.rs": "uncategorized",
        })

    def test_precedence_tests_beat_components(self):
        self.check({
            "aida-tui/tests/snap.rs": "tests_automation",
            "aida-cli-lib/src/tests/fixtures/a.json": "tests_automation",
            "aida-web-react/src/__tests__/a.tsx": "tests_automation",
            "aida-web-react/src/a.test.tsx": "tests_automation",
            "aida-core/src/test_helpers.rs": "tests_automation",
            "aida-core/src/store_tests.rs": "tests_automation",
            "scripts/check.py": "tests_automation",
            ".github/workflows/ci.yml": "tests_automation",
            "Makefile": "tests_automation",
            "docs/monitor-contract-fixtures/a.json": "tests_automation",
        })

    def test_precedence_artifacts_beat_everything(self):
        self.check({
            "Cargo.lock": "artifacts",
            "aida-web-react/package-lock.json": "artifacts",
            "aida-server/src/generated/api.rs": "artifacts",
            "docs/presentation/vendor/lib.js": "artifacts",
            "aida-store/objects/FR/000/FR-1.yaml": "artifacts",
            "requirements-manager/requirements.yaml": "artifacts",
            "default_requirements.yaml": "artifacts",
            "tests/generated/x.rs": "artifacts",
            "aida-web/dist/app.wasm": "artifacts",
            "data/token-rates/v1.toml": "artifacts",
        })

    def test_docs_and_integration_precedence(self):
        self.check({
            "docs/cli/01.md": "docs",
            "README.md": "docs",
            "aida-cli/README.md": "docs",
            "ai-integration-report.html": "docs",
            ".aida/discipline/README.md": "docs",
            ".aida/config.toml": "packaging_integration",
            ".claude/skills/x/SKILL.md": "packaging_integration",
            "aida-core/templates/skill.md": "packaging_integration",
            "plugins/aida/plugin.json": "packaging_integration",
            "docker/Dockerfile.server": "packaging_integration",
            "Dockerfile": "packaging_integration",
            "Cargo.toml": "packaging_integration",
            "aida-cli/Cargo.toml": "cli",
            "./aida-tui/src/lib.rs": "tui",
        })

    def test_generated_analytics_exports_are_artifacts_but_readme_is_docs(self):
        base = "docs/analytics/code-growth/"
        for n in ("code-growth.json", "code-growth.csv", "code-growth-embedded.csv", "delivery-cadence.csv",
                  "rust-test-markers.csv", "production-growth.png"):
            self.assertEqual(mod.classify(base + n), "artifacts", n)
        self.assertEqual(mod.classify(base + "README.md"), "docs")
        self.assertNotEqual(mod.classify("docs/analytics/other/code-growth.csv"), "artifacts")

    def test_every_rule_target_is_a_declared_bucket(self):
        for bucket, _ in mod.RULE_DOCS:
            self.assertTrue(bucket in mod.BUCKET_INFO or bucket == "<component>", bucket)
        for _, bucket in mod.COMPONENT_PREFIXES:
            self.assertIn(bucket, mod.BUCKET_INFO)


class CadenceTest(unittest.TestCase):
    def test_monthly_last_commit_per_period_and_tip_included(self):
        commits = fake_commits([utc(2026, 1, 3), utc(2026, 1, 28), utc(2026, 2, 1), utc(2026, 3, 9), utc(2026, 3, 10)])
        samples = mod.select_samples(commits, "month")
        self.assertEqual([s["sha"] for s in samples], ["sha1", "sha2", "sha4"])
        self.assertEqual([s["period"] for s in samples], ["2026-01", "2026-02", "2026-03"])
        self.assertEqual(samples[-1]["kind"], "tip")
        self.assertTrue(samples[-1]["partial"])
        self.assertEqual(samples[0]["kind"], "period_end")

    def test_period_end_commit_on_last_day_is_not_partial(self):
        samples = mod.select_samples(fake_commits([utc(2026, 1, 31)]), "month")
        self.assertFalse(samples[0]["partial"])

    def test_every_n_thins_but_keeps_tip(self):
        commits = fake_commits([utc(2026, m, 5) for m in range(1, 8)])
        samples = mod.select_samples(commits, "month", every=3)
        self.assertEqual([s["period"] for s in samples], ["2026-01", "2026-04", "2026-07"])

    def test_week_quarter_year_day_labels(self):
        commits = fake_commits([utc(2026, 1, 1), utc(2026, 1, 5), utc(2026, 5, 20), utc(2027, 1, 2)])
        self.assertEqual([s["period"] for s in mod.select_samples(commits, "quarter")], ["2026-Q1", "2026-Q2", "2027-Q1"])
        self.assertEqual([s["period"] for s in mod.select_samples(commits, "year")], ["2026", "2027"])
        self.assertEqual(mod.select_samples(commits, "week")[0]["period"], "2026-W01")
        self.assertEqual(len(mod.select_samples(commits, "day")), 4)
        self.assertEqual(mod.period_of(utc(2026, 12, 31), "month")[1], dt.date(2026, 12, 31))
        self.assertEqual(mod.period_of(utc(2026, 11, 3), "quarter")[1], dt.date(2026, 12, 31))

    def test_out_of_order_dates_still_end_at_tip(self):
        commits = fake_commits([utc(2026, 3, 1), utc(2026, 2, 1)])  # tip carries an older date
        samples = mod.select_samples(commits, "month")
        self.assertEqual(samples[-1]["sha"], "sha1")
        self.assertEqual(samples[-1]["kind"], "tip")


class ParsingTest(unittest.TestCase):
    def test_pr_subjects(self):
        self.assertEqual(mod.parse_prs("[AI:codex] fix(x): y (BUG-1) (#2461)"), [2461])
        self.assertEqual(mod.parse_prs("Merge pull request #12 from a/b"), [12])
        self.assertEqual(mod.parse_prs("feat: thing (!34)"), [34])
        self.assertEqual(mod.parse_prs("see issue #99 for context"), [])
        self.assertEqual(mod.parse_prs("plain commit"), [])

    def test_delivery_unique_prs_and_partial_month(self):
        commits = fake_commits([utc(2026, 1, 3), utc(2026, 1, 4), utc(2026, 2, 2)])
        commits[0]["subject"] = "a (#1)"
        commits[1]["subject"] = "b (#1)"  # duplicate PR number counts once
        commits[2]["subject"] = "c (#2)"
        d = mod.delivery_cadence(commits, "month")
        self.assertEqual([(r["period"], r["commits"], r["prs"], r["partial"]) for r in d],
                         [("2026-01", 2, 1, False), ("2026-02", 1, 1, True)])

    def test_delivery_out_of_order_dates_keep_every_commit_and_terminate(self):
        for order in ([(1, 5), (3, 5), (2, 5)], [(3, 5), (2, 5)], [(1, 5), (3, 6), (3, 7), (2, 5)]):
            commits = fake_commits([utc(2026, m, d) for m, d in order])
            for cadence in mod.CADENCES:
                d = mod.delivery_cadence(commits, cadence)
                self.assertEqual(sum(r["commits"] for r in d), len(commits), (order, cadence))
                self.assertEqual(len({r["period"] for r in d}), len(d))
        d = mod.delivery_cadence(fake_commits([utc(2026, 1, 5), utc(2026, 3, 5), utc(2026, 2, 5)]), "month")
        self.assertEqual([(r["period"], r["commits"], r["partial"]) for r in d],
                         [("2026-01", 1, False), ("2026-02", 1, True), ("2026-03", 1, True)])

    def test_early_zero_pr_note(self):
        d = [{"period": "2026-01", "prs": 0}, {"period": "2026-02", "prs": 0}, {"period": "2026-03", "prs": 4}]
        self.assertIn("2026-01 to 2026-02 show 0 PRs", mod.early_zero_pr_note(d))
        self.assertEqual(mod.early_zero_pr_note(d[2:]), "")
        self.assertEqual(mod.early_zero_pr_note(d[:2]), "")

    def test_delivery_fills_idle_periods_with_zero(self):
        commits = fake_commits([utc(2026, 1, 3), utc(2026, 4, 2)])
        d = mod.delivery_cadence(commits, "month")
        self.assertEqual([(r["period"], r["commits"]) for r in d],
                         [("2026-01", 0 + 1), ("2026-02", 0), ("2026-03", 0), ("2026-04", 1)])
        self.assertEqual([r["period"] for r in mod.delivery_cadence(commits, "quarter")], ["2026-Q1", "2026-Q2"])
        self.assertEqual(len(mod.delivery_cadence(commits, "week")), 14)

    def test_rust_marker_heuristic(self):
        src = (
            "#[test]\nfn a() {}\n"
            "    #[tokio::test]\n    async fn b() {}\n"
            "#[tokio::test(flavor = \"multi_thread\")]\nasync fn c() {}\n"
            "#[rstest]\n#[case(1)]\nfn d(#[case] x: u8) {}\n"
            "// #[test]\n"
            "#[cfg(test)]\nmod m {}\n"
            "#[derive(Debug)]\nstruct S;\n"
            "let s = \"#[test]\";\n"
        )
        self.assertEqual(mod.count_rust_markers(src), {"plain": 1, "async": 2, "rstest": 1})


@unittest.skipUnless(TOKEI, "tokei binary not on PATH; skipping Tokei-backed tests")
class EndToEndTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cg-test-"))
        cls.home = cls.tmp / "home"
        cls.home.mkdir()
        cls.repo = cls.tmp / "repo"
        cls.repo.mkdir()
        cls.env = {**os.environ, "HOME": str(cls.home), "AIDA_HOME": str(cls.home), "GIT_CONFIG_GLOBAL": os.devnull,
                   "GIT_CONFIG_NOSYSTEM": "1"}
        cls.git("init", "-q", "-b", "main")
        cls.write("aida-core/src/lib.rs", "// c\npub fn a() {}\n\n#[test]\nfn t() {}\n")
        cls.write("docs/a.md", "# Doc\n\ntext\n")
        cls.commit("first", "2026-01-05T12:00:00+00:00")
        cls.write("aida-cli/src/main.rs", "fn main() {}\n")
        cls.write("tests/t.rs", "#[tokio::test]\nasync fn x() {}\n#[rstest]\nfn y() {}\n")
        cls.commit("add cli (#10)", "2026-01-20T12:00:00+00:00")
        cls.git("checkout", "-q", "-b", "side")
        cls.write("aida-tui/src/lib.rs", "pub fn tui() {}\n")
        cls.commit("side work (#99)", "2026-02-02T12:00:00+00:00")
        cls.git("checkout", "-q", "main")
        cls.commit("Merge pull request #11 from x/side", "2026-02-10T12:00:00+00:00", merge="side")
        cls.write("aida-web-react/index.html",
                  "<html>\n<script>\nvar a = 1;\n</script>\n<style>\na{color:red}\n</style>\n</html>\n")
        cls.write("README.md", "# R\n\n```rust\nfn doc() {}\n```\n")
        cls.write("package-lock.json", "{\n  \"lockfileVersion\": 3\n}\n")
        cls.link = cls.repo / "link-to-live-state"
        os.symlink("/nonexistent/live/.aida", cls.link)
        cls.git("add", "-A")
        cls.commit("final fix (#12)", "2026-03-03T12:00:00+00:00")
        cls.scratch = cls.tmp / "scratch"
        cls.scratch.mkdir()
        cls.result = mod.analyze(str(cls.repo), "main", "month", 1, TOKEI, cls.scratch, progress=lambda m: None)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    @classmethod
    def git(cls, *args):
        return subprocess.run(["git", "-C", str(cls.repo), "-c", "user.name=t", "-c", "user.email=t@example.com", *args],
                              env=cls.env, check=True, capture_output=True, text=True).stdout

    @classmethod
    def write(cls, rel, text):
        p = cls.repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    @classmethod
    def commit(cls, msg, when, merge=None):
        env = {**cls.env, "GIT_AUTHOR_DATE": when, "GIT_COMMITTER_DATE": when}
        base = ["git", "-C", str(cls.repo), "-c", "user.name=t", "-c", "user.email=t@example.com"]
        if merge:
            subprocess.run([*base, "merge", "--no-ff", "-q", "-m", msg, merge], env=env, check=True, capture_output=True)
        else:
            subprocess.run([*base, "add", "-A"], env=env, check=True)
            subprocess.run([*base, "commit", "-q", "-m", msg], env=env, check=True, capture_output=True)

    def test_samples_are_first_parent_calendar_months_ending_at_tip(self):
        r = self.result
        self.assertEqual([s["period"] for s in r["samples"]], ["2026-01", "2026-02", "2026-03"])
        tip = self.git("rev-parse", "main").strip()
        self.assertEqual(r["samples"][-1]["revision"], tip)
        self.assertEqual(r["samples"][-1]["kind"], "tip")
        self.assertTrue(r["samples"][-1]["partial"])
        side = self.git("log", "--format=%H", "--grep=side work").strip()
        self.assertNotIn(side, [s["revision"] for s in r["samples"]])
        self.assertEqual(r["samples"][1]["subject"], "Merge pull request #11 from x/side")

    def test_buckets_are_exclusive_and_tokei_total_reconciles(self):
        rec = self.result["reconciliation"]
        self.assertTrue(rec["ok"], rec)
        self.assertEqual(rec["direct_total"], rec["bucketed_total"])
        tip = self.result["tip"]
        rows = [r for r in self.result["rows"] if r["revision"] == tip]
        self.assertEqual(sum(r["code"] for r in rows), rec["direct_total"]["code"])
        self.assertEqual(sum(r["files"] for r in rows), rec["files"])
        # every (bucket, language) pair appears once per revision
        keys = [(r["bucket"], r["language"]) for r in rows]
        self.assertEqual(len(keys), len(set(keys)))
        self.assertTrue(all(r["lines"] == r["code"] + r["comments"] + r["blanks"] for r in rows))

    def test_mixed_language_rows_and_embedded_not_double_counted(self):
        tip = self.result["tip"]
        by = {(r["bucket"], r["language"]): r for r in self.result["rows"] if r["revision"] == tip}
        self.assertEqual(by[("web_ui", "HTML")]["files"], 1)
        self.assertIn(("core", "Rust"), by)
        self.assertIn(("tests_automation", "Rust"), by)
        self.assertIn(("docs", "Markdown"), by)
        self.assertFalse(any(k[1] in ("JavaScript", "CSS") for k in by), "embedded blobs leaked into own rows")
        emb = {(r["container"], r["language"]): r for r in self.result["embedded_rows"] if r["revision"] == tip}
        self.assertEqual(emb[("HTML", "JavaScript")]["code"], 1)
        self.assertEqual(emb[("Markdown", "Rust")]["code"], 1)
        self.assertEqual(self.result["samples"][-1]["files"], sum(r["files"] for r in self.result["rows"]
                                                                  if r["revision"] == tip))

    def test_artifacts_retained_in_exports_but_not_charted(self):
        tip = self.result["tip"]
        art = [r for r in self.result["rows"] if r["revision"] == tip and r["bucket"] == "artifacts"]
        self.assertTrue(art, "package-lock.json should be kept in the export")
        self.assertNotIn("artifacts", mod.PRODUCT_BUCKETS + mod.SUPPORT_BUCKETS)

    def test_symlinks_are_skipped_not_followed(self):
        self.assertEqual(self.result["samples"][-1]["symlinks_skipped"], 1)

    def test_rust_markers_and_delivery(self):
        rt = {r["period"]: r for r in self.result["rust_tests"]}
        self.assertEqual((rt["2026-01"]["plain"], rt["2026-01"]["async"], rt["2026-01"]["rstest"]), (1, 1, 1))
        self.assertEqual(rt["2026-03"]["total"], 3)
        d = {r["period"]: r for r in self.result["delivery"]}
        self.assertEqual((d["2026-01"]["commits"], d["2026-01"]["prs"]), (2, 1))
        self.assertEqual((d["2026-02"]["commits"], d["2026-02"]["prs"]), (1, 1))  # side commit (#99) not first-parent
        self.assertTrue(d["2026-03"]["partial"])
        self.assertFalse(d["2026-01"]["partial"])

    def test_outputs_csv_json_and_charts(self):
        out = self.tmp / "out"
        paths = mod.write_outputs(self.result, out, charts=True)
        names = {p.name for p in paths}
        self.assertTrue({"code-growth.csv", "code-growth.json", "production-growth.png", "supporting-growth.png",
                         "delivery-cadence.png", "rust-test-growth.png"} <= names)
        with open(out / "code-growth.csv", newline="") as fh:
            reader = csv.DictReader(fh)
            self.assertEqual(reader.fieldnames, mod.ROW_FIELDS)
            rows = list(reader)
        self.assertEqual(len(rows), len(self.result["rows"]))
        for field in ("revision", "date", "bucket", "language", "code", "comments", "blanks", "lines", "files"):
            self.assertIn(field, rows[0])
        data = json.loads((out / "code-growth.json").read_text())
        self.assertEqual(data["reconciliation"]["ok"], True)
        self.assertEqual({b["id"] for b in data["buckets"]}, set(mod.BUCKET_INFO))
        for png in out.glob("*.png"):
            self.assertGreater(png.stat().st_size, 5000)
            self.assertEqual(png.read_bytes()[:8], b"\x89PNG\r\n\x1a\n")

    def test_repeatable(self):
        again = mod.analyze(str(self.repo), "main", "month", 1, TOKEI, self.scratch, progress=lambda m: None)
        self.assertEqual(again["rows"], self.result["rows"])
        self.assertEqual(again["samples"], self.result["samples"])

    def test_repo_is_untouched(self):
        self.assertEqual(self.git("status", "--porcelain").strip(), "")


@unittest.skipUnless(TOKEI, "tokei binary not on PATH; skipping Tokei-backed tests")
class BackdatedAndGeneratedTest(unittest.TestCase):
    """R1/R2: non-chronological first-parent dates and generated-export exclusion."""

    def build(self, tmp, dates):
        repo = tmp / "repo"
        repo.mkdir()
        env = {**os.environ, "HOME": str(tmp / "home"), "AIDA_HOME": str(tmp / "home"),
               "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
        (tmp / "home").mkdir()
        base = ["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@example.com"]
        subprocess.run([*base, "init", "-q", "-b", "main"], env=env, check=True)
        for i, when in enumerate(dates):
            files = {"aida-core/src/lib.rs": f"pub fn a{i}() {{}}\n",
                     "aida-server/src/generated/api.rs": "#[test]\nfn gen() {}\n" * (i + 1),
                     "docs/analytics/code-growth/code-growth.csv": "a,b\n" * (i + 1),
                     "docs/analytics/code-growth/README.md": f"# R{i}\n"}
            for rel, text in files.items():
                p = repo / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(text)
            e = {**env, "GIT_AUTHOR_DATE": when, "GIT_COMMITTER_DATE": when}
            subprocess.run([*base, "add", "-A"], env=e, check=True)
            subprocess.run([*base, "commit", "-q", "-m", f"c{i} (#{i + 1})"], env=e, check=True, capture_output=True)
        return repo

    def test_both_orders_preserve_all_commits_for_all_cadences(self):
        orders = {
            "jan-mar-feb": ["2026-01-05T12:00:00+00:00", "2026-03-05T12:00:00+00:00", "2026-02-05T12:00:00+00:00"],
            "mar-feb": ["2026-03-05T12:00:00+00:00", "2026-02-05T12:00:00+00:00"],
        }
        for name, dates in orders.items():
            with tempfile.TemporaryDirectory(prefix="cg-order-") as t:
                tmp = pathlib.Path(t)
                repo = self.build(tmp, dates)
                for cadence in mod.CADENCES:
                    scratch = tmp / f"s-{cadence}"
                    scratch.mkdir()
                    r = mod.analyze(str(repo), "main", cadence, 1, TOKEI, scratch, progress=lambda m: None)
                    self.assertEqual(sum(x["commits"] for x in r["delivery"]), len(dates), (name, cadence))
                    self.assertEqual(sum(x["prs"] for x in r["delivery"]) >= 1, True)
                    self.assertEqual(r["samples"][-1]["revision"],
                                     subprocess.run(["git", "-C", str(repo), "rev-parse", "main"], capture_output=True,
                                                    text=True, env={**os.environ, "GIT_CONFIG_GLOBAL": os.devnull}).stdout.strip())
                    self.assertTrue(r["reconciliation"]["ok"])

    def test_generated_exports_and_server_markers_are_not_charted(self):
        with tempfile.TemporaryDirectory(prefix="cg-gen-") as t:
            tmp = pathlib.Path(t)
            repo = self.build(tmp, ["2026-01-05T12:00:00+00:00", "2026-02-05T12:00:00+00:00"])
            scratch = tmp / "s"
            scratch.mkdir()
            r = mod.analyze(str(repo), "main", "month", 1, TOKEI, scratch, progress=lambda m: None)
            tip = r["tip"]
            by = {(x["bucket"], x["language"]): x for x in r["rows"] if x["revision"] == tip}
            self.assertEqual({b for b, _ in by if b == "docs"}, {"docs"})  # README only
            self.assertEqual(by[("docs", "Markdown")]["files"], 1)
            self.assertFalse([k for k in by if k[0] == "docs" and k[1] != "Markdown"])
            last = r["rust_tests"][-1]
            self.assertEqual(last["total"], 2)            # raw: both generated #[test] markers kept as evidence
            self.assertEqual(last["by_bucket"]["artifacts"]["total"], 2)
            self.assertEqual(last["charted_total"], 0)    # but not plotted
            out = tmp / "out"
            mod.write_outputs(r, out, charts=True)
            with open(out / "rust-test-markers.csv") as fh:
                row = list(csv.DictReader(fh))[-1]
            self.assertEqual((row["total"], row["charted_total"]), ("2", "0"))
            self.assertEqual(r["reconciliation"]["ok"], True)


if __name__ == "__main__":
    unittest.main()
