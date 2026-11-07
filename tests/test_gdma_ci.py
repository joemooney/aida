# trace:STORY-2606 | ai:codex
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("gdma_ci", ROOT / "ci/gdma/run-upstream.py")
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)
sources_spec = importlib.util.spec_from_file_location("gdma_sources", ROOT / "ci/gdma/check-sources.py")
sources = importlib.util.module_from_spec(sources_spec)
sources_spec.loader.exec_module(sources)
class ExpressionTests(unittest.TestCase):
    def test_boolean_and_string_values(self):
        context = adapter.context("gates", True)
        self.assertTrue(adapter.condition("matrix.shard == 'gates' && runner.os == 'Linux'", context))
        self.assertEqual(adapter.condition("${{ matrix.shard == 'gates' && '1' || '' }}", context), "1")
        self.assertFalse(adapter.condition("matrix.shard == 'contracts'", context))
        self.assertTrue(adapter.condition("!cancelled() && (true || false)", context))

    def test_string_comparisons_ignore_case(self):
        current = adapter.context("gates", True)
        self.assertTrue(adapter.condition("matrix.shard == 'GATES'", current))
        self.assertFalse(adapter.condition("runner.os != 'LINUX'", current))
        self.assertTrue(adapter.condition("startsWith(runner.os, 'LIN')", current))

    def test_context_uses_injected_gitlab_environment(self):
        current = adapter.context("gates", True, environ={
            "CI_PIPELINE_SOURCE": "merge_request_event",
            "CI_MERGE_REQUEST_DIFF_BASE_SHA": "a" * 40,
            "CI_COMMIT_BEFORE_SHA": "b" * 40,
        })
        self.assertEqual(current["github.event_name"], "pull_request")
        self.assertEqual(current["github.event.pull_request.base.sha"], "a" * 40)
        self.assertEqual(current["github.event.before"], "b" * 40)

    def test_prefix_function(self):
        self.assertTrue(adapter.condition("startsWith(matrix.shard, 'lib-')", adapter.context("lib-0", True)))

    def test_unknown_inputs_fail_closed(self):
        for expression in ("secrets.TOKEN", "unknown()", "true; echo unsafe", "(true", "true + false"):
            with self.subTest(expression=expression), self.assertRaises(ValueError):
                adapter.condition(expression, adapter.context("gates", True))

    def test_shell_substitution_preserves_variable_quoting(self):
        self.assertEqual(adapter.shell('git fetch origin "${{ github.base_ref }}"'),
                         'git fetch origin "${GDMA_BASE_REF}"')
        with self.assertRaises(ValueError):
            adapter.shell("echo ${{ secrets.TOKEN }}")

    def test_docs_and_metadata_only_changes(self):
        result = subprocess.CompletedProcess([], 0, stdout=b'docs/a.txt\0README.md\0.gitignore\0LICENSE\0')
        with patch.object(adapter.subprocess, "run", return_value=result):
            self.assertFalse(adapter.full_ci("base"))
        result.stdout += b'ci/gdma/prepare.sh\0'
        with patch.object(adapter.subprocess, "run", return_value=result):
            self.assertTrue(adapter.full_ci("base"))

    def test_every_current_ci_expression_and_shell_is_supported(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        job = workflow['jobs']['build']
        for lane in ('lib-0', 'lib-1', 'cli-0', 'cli-1', 'rest', 'gates', 'contracts'):
            for full in (True, False):
                for failed in (True, False):
                    context = adapter.context(lane, full, failed)
                    adapter.validate_job(job, context)
                    adapter.step_environment(job, context, {})
                    for step in job['steps']:
                        with self.subTest(lane=lane, full=full, name=step.get('name')):
                            adapter.condition(step.get('if', 'true'), context)
                            adapter.step_environment(step, context, {})
                            if 'uses' in step:
                                self.assertIn(step['uses'].split('@')[0], adapter.ACTIONS)
                            elif 'run' in step and step.get('name') not in adapter.INSTALL_STEPS:
                                adapter.shell(step['run'])



class ExecutionTests(unittest.TestCase):
    def run_workflow(self, steps, lane="gates", full=True, before="a" * 40, job_settings=None, stale=False):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        workflows = root / ".github/workflows"
        workflows.mkdir(parents=True)
        workflow = {"env": {"GLOBAL_VALUE": "global"}, "jobs": {"build": {
            "strategy": {"matrix": {"shard": ["gates"]}},
            "env": {"JOB_VALUE": "${{ matrix.shard }}"}, "steps": steps}}}
        workflow["jobs"]["build"].update(job_settings or {})
        if stale:
            for name in (".ci-artifacts/old.xml", "target/nextest/ci/junit-old.xml",
                         "target/test-run.log", "target/test-timings.json",
                         "target/changed-files.txt", "target/cargo-timings/old.html",
                         "target/ci-summary.md"):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("stale")
        (workflows / "ci.yml").write_text(yaml.safe_dump(workflow))
        with patch.object(adapter, "ROOT", root), \
                patch.object(adapter, "full_ci", return_value=full):
            code = adapter.main(["--lane", lane], environ={
                "CI_COMMIT_BEFORE_SHA": before, "CI_PIPELINE_SOURCE": "push"})
        return code, root

    def test_failed_command_skips_ordinary_steps_but_runs_cleanup_and_collects_junit(self):
        code, root = self.run_workflow([
            {"run": 'test "$GLOBAL_VALUE/$JOB_VALUE/$STEP_VALUE" = global/gates/step; exit 1',
             "env": {"STEP_VALUE": "step"}},
            {"run": "touch should-not-exist"},
            {"if": "!cancelled()", "run": "mkdir -p target/nextest/ci; printf '<testsuites/>' > target/nextest/ci/junit-gates.xml"},
        ])
        self.assertEqual(code, 1)
        self.assertFalse((root / "should-not-exist").exists())
        self.assertEqual((root / ".ci-artifacts/nextest/ci/junit-gates.xml").read_text(), "<testsuites/>")

    def test_shell_context_and_summary_are_available(self):
        code, root = self.run_workflow([{"run":
            'test "$GDMA_LANE/$GDMA_EVENT_NAME" = gates/push; echo summary >> "$GITHUB_STEP_SUMMARY"'}])
        self.assertEqual(code, 0)
        self.assertEqual((root / ".ci-artifacts/ci-summary.md").read_text(), "summary\n")

    def test_new_branch_without_baseline_runs_full_ci(self):
        code, root = self.run_workflow([
            {"if": "steps.changes.outputs.full_ci == 'true'", "run": "touch full-ci-ran"},
        ], full=False, before="0" * 40)
        self.assertEqual(code, 0)
        self.assertTrue((root / "full-ci-ran").exists())

    def test_unknown_lane_is_refused_before_execution(self):
        with patch.object(adapter.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                self.run_workflow([{"run": "true"}], lane="unknown")
            run.assert_not_called()

    def test_docs_only_does_not_run_full_ci_step(self):
        code, root = self.run_workflow([
            {"if": "steps.changes.outputs.full_ci == 'true'", "run": "exit 1"},
            {"run": "touch docs-guard-ran"},
        ], full=False)
        self.assertEqual(code, 0)
        self.assertTrue((root / "docs-guard-ran").exists())

    def test_unsupported_later_input_refuses_before_first_command(self):
        for step in ({"uses": "unmapped/action@v1"},
                     {"if": "false", "run": "echo ${{ secrets.TOKEN }}"},
                     {"run": "true", "continue-on-error": True},
                     {"run": "true", "shell": "python"},
                     {"run": "true", "env": {"VALUE": "prefix-${{ secrets.TOKEN }}"}}):
            with self.subTest(step=step), patch.object(adapter.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    self.run_workflow([{"run": "touch premature"}, step])
                run.assert_not_called()

    def test_unsupported_job_settings_refuse_before_first_command(self):
        settings = [{key: value} for key, value in (
            ("container", "image"), ("services", {}), ("if", "false"),
            ("continue-on-error", True), ("needs", ["other"]),
            ("runs-on", "windows-latest"), ("defaults", {"working-directory": "elsewhere"}),
        )]
        settings += [{"strategy": {"matrix": {"shard": ["gates"], key: value}}}
                     for key, value in (("include", []), ("exclude", []),
                                        ("unknown", []), ("os", ["windows-latest"]))]
        settings.append({"strategy": {"matrix": {"shard": ["gates"]}, "max-parallel": 2}})
        for job_settings in settings:
            with self.subTest(settings=job_settings), patch.object(adapter.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    self.run_workflow([{"run": "touch premature"}], job_settings=job_settings)
                run.assert_not_called()

    def test_failure_before_tests_cannot_collect_previous_run_artifacts(self):
        code, root = self.run_workflow([{"run": "exit 1"}], stale=True)
        self.assertEqual(code, 1)
        self.assertEqual(list((root / ".ci-artifacts").rglob("*")), [])
        self.assertFalse((root / "target/nextest/ci/junit-old.xml").exists())
        self.assertFalse((root / "target/test-run.log").exists())
        self.assertFalse((root / "target/ci-summary.md").exists())
        # GitLab after_script collection must not resurrect old reports either.
        with patch.object(adapter, "ROOT", root):
            self.assertEqual(adapter.main(["--collect"], environ={}), 0)
        self.assertEqual(list((root / ".ci-artifacts").rglob("*")), [])

    def test_timeout_fails_and_allows_cleanup(self):
        with patch.object(adapter.subprocess, "run", side_effect=[
                subprocess.TimeoutExpired("fixture", 1), subprocess.CompletedProcess([], 0)]):
            code, _ = self.run_workflow([
                {"run": "slow", "timeout-minutes": 1},
                {"if": "always ()", "run": "cleanup"},
            ])
        self.assertEqual(code, 1)

    def test_literal_jobs_match_upstream_lane_set(self):
        config = yaml.safe_load((ROOT / ".gitlab/ci/gdma.yml").read_text())
        workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
        lanes = workflow["jobs"]["build"]["strategy"]["matrix"]["shard"]
        self.assertEqual({name for name in config if not name.startswith(".")},
                         {f"gdma-ci: [{lane}]" for lane in lanes})
        for lane in lanes:
            job = config[f"gdma-ci: [{lane}]"]
            self.assertEqual(job["variables"]["GDMA_LANE"], lane)
            self.assertEqual(job["extends"], ".gdma-job")
        self.assertEqual(config[".gdma-job"]["variables"]["GIT_STRATEGY"], "clone")
        self.assertEqual(config[".gdma-job"]["image"], "$GDMA_CI_IMAGE")
        self.assertEqual(config[".gdma-job"]["tags"], ["$GDMA_RUNNER_TAG"])


class SourceTests(unittest.TestCase):
    def test_remote_policy_accepts_https_and_ssh(self):
        for remote in ("https://token@gitlab.example.invalid/group/repo.git",
                       "git@gitlab.example.invalid:group/repo.git"):
            sources.check_remote(remote, "gitlab.example.invalid")
        with self.assertRaises(ValueError) as error:
            sources.check_remote("https://secret@other.invalid/repo", "gitlab.example.invalid")
        self.assertNotIn("secret", str(error.exception))

    def test_registry_policy_uses_exact_host_and_path_boundary(self):
        approved = "https://packages.example.invalid/repository/npm/"
        sources.check_registry(approved, approved)
        sources.check_registry(approved + "pkg/-/pkg.tgz", approved, artifact=True)
        for url in ("http://packages.example.invalid/repository/npm/",
                    "https://packages.example.invalid/repository/npm-evil/pkg.tgz",
                    "https://secret@packages.example.invalid/repository/npm/pkg.tgz",
                    "https://other.invalid/repository/npm/pkg.tgz"):
            with self.subTest(url=url), self.assertRaises(ValueError):
                sources.check_registry(url, approved, artifact=True)

    def test_cargo_canonical_identity_requires_configured_replacement(self):
        approved = "https://packages.example.invalid/repository/cargo/"
        config = {"source": {"crates-io": {"replace-with": "mirror"},
                             "mirror": {"registry": "sparse+" + approved}}}
        lock = {"package": [{"source": "registry+https://github.com/rust-lang/crates.io-index"}]}
        sources.check_cargo(config, lock, "gitlab.example.invalid", approved)
        for invalid in ({}, {"source": {"crates-io": {"replace-with": "missing"}}}):
            with self.assertRaises(ValueError):
                sources.check_cargo(invalid, lock, "gitlab.example.invalid", approved)
        with self.assertRaises(ValueError):
            sources.check_cargo(config, {"package": [{"source": "git+https://other.invalid/repo"}]},
                                "gitlab.example.invalid", approved)

    def test_npm_downloads_cannot_escape_configured_registry(self):
        approved = "https://packages.example.invalid/repository/npm/"
        sources.check_npm({"packages": {"pkg": {"resolved": approved + "pkg/-/pkg.tgz"}}},
                          "gitlab.example.invalid", approved)
        with self.assertRaises(ValueError):
            sources.check_npm({"packages": {"pkg": {"resolved": "https://other.invalid/pkg.tgz"}}},
                              "gitlab.example.invalid", approved)


if __name__ == "__main__":
    unittest.main()
