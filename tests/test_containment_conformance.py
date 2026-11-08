#!/usr/bin/env python3
"""Adversarial evidence tests for the host conformance experiment.
trace:TASK-3046 | ai:codex
"""
import contextlib
import io
import importlib.util
import json
import os
import pty
from pathlib import Path
import tempfile
import time
import tomllib
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "conformance", Path(__file__).resolve().parents[1] / "scripts/containment/conformance.py")
c = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(c)


# trace:TASK-3114 | ai:codex
class PolicyRevisionTests(unittest.TestCase):
    def test_policy_resolves_in_script_repository_from_outside_cwd(self):
        original = c.command
        with tempfile.TemporaryDirectory() as fixture, tempfile.TemporaryDirectory() as outside:
            repo = Path(fixture)
            script = repo / "scripts/containment/conformance.py"
            script.parent.mkdir(parents=True)
            script.write_text("# policy lookup fixture\n")
            original(["git", "init", "-q", str(repo)])
            original(["git", "-C", str(repo), "add", "."])
            original(["git", "-C", str(repo), "-c", "user.name=Test",
                      "-c", "user.email=test@example.invalid", "-c", "core.hooksPath=/dev/null",
                      "commit", "-qm", "fixture"])
            expected = original(["git", "-C", str(repo), "rev-parse", "HEAD"])
            original(["git", "-C", str(repo), "update-ref", "refs/remotes/origin/main", expected])
            # Give Git an unrelated cwd, as when launched outside the repo.
            with mock.patch.object(c, "__file__", str(script)), mock.patch.object(
                    c, "command", side_effect=lambda argv: original(argv, cwd=outside)):
                sha = c.trusted_policy_sha()
        self.assertEqual(sha, expected)

    def test_missing_policy_ref_is_a_conformance_refusal_without_traceback(self):
        with tempfile.TemporaryDirectory() as empty:
            c.command(["git", "init", "-q", empty])
            script = Path(empty) / "scripts/containment/conformance.py"
            with mock.patch.object(c, "__file__", str(script)), mock.patch.object(
                    c.sys, "argv", [str(script), "--vendor", "codex", "--output", empty]), \
                    contextlib.redirect_stderr(io.StringIO()) as stderr:
                with self.assertRaises(SystemExit) as exit:
                    c.main()
        self.assertEqual(exit.exception.code, 2)
        self.assertTrue(stderr.getvalue().startswith("conformance: cannot resolve"))
        self.assertIn("fetch origin main", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_git_timeout_is_reported_as_policy_refusal(self):
        with mock.patch.object(c, "command", side_effect=c.subprocess.TimeoutExpired("git", 15)):
            with self.assertRaisesRegex(ValueError, "cannot resolve.*origin/main"):
                c.trusted_policy_sha()


# trace:TASK-3131 | ai:codex
class AidaBoundaryTests(unittest.TestCase):
    # trace:BUG-2470 | ai:codex
    def test_fixture_policy_merges_existing_tables_and_replaces_hosts(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.toml"
            config.write_text('''title = "fixture"
[agents]
contained = false
vendor = "claude"
model = "retained"
[contained]
allowed_hosts = [
    "deny.fixture.test", # init defaults must not survive
    "github.com",
]
extra = true
[contained.vendors]
codex = "native"
claude = "aida"
[unrelated]
items = [{name = "preserved", enabled = true}]
''')
            before = tomllib.loads(config.read_text())
            c.fixture_policy(config)
            merged = tomllib.loads(config.read_text())
            self.assertEqual(merged["agents"],
                             {"contained": True, "vendor": "codex", "model": "retained"})
            self.assertEqual(merged["contained"], {
                "allowed_hosts": ["api.openai.com", "allow.fixture.test"],
                "extra": True, "vendors": {"codex": "aida", "claude": "aida"}})
            self.assertEqual(merged["unrelated"], before["unrelated"])
            self.assertEqual(merged["title"], before["title"])
            c.fixture_policy(config)
            self.assertEqual(tomllib.loads(config.read_text()), merged)

    # trace:TASK-3048 | ai:codex
    def test_agy_fixture_uses_shipped_auth_hosts_and_aida_posture(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.toml"
            config.write_text('[contained]\nallowed_hosts = ["deny.fixture.test"]\n')
            c.fixture_policy(config, "agy", {"auth_hosts": ["daily-cloudcode-pa.googleapis.com", "oauth2.googleapis.com"]})
            policy = tomllib.loads(config.read_text())
            self.assertEqual(policy["agents"]["vendor"], "agy")
            self.assertEqual(policy["contained"]["vendors"]["agy"], "aida")
            self.assertEqual(policy["contained"]["allowed_hosts"],
                             ["daily-cloudcode-pa.googleapis.com", "oauth2.googleapis.com", "allow.fixture.test"])

    # trace:TASK-3048 | ai:codex
    def test_agy_policy_requires_binary_descriptor_without_source_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.toml"
            original = '[contained]\nallowed_hosts = ["unchanged.test"]\n'
            config.write_text(original)
            for runtime in (None, {}, {"auth_hosts": []}, {"auth_hosts": ["*.googleapis.com"]}):
                with self.assertRaisesRegex(ValueError, "fixture-runtime-policy"):
                    c.fixture_policy(config, "agy", runtime)
                self.assertEqual(config.read_text(), original)
            c.fixture_policy(config, "agy", {"auth_hosts": ["binary.fixture.test"]})
            self.assertEqual(tomllib.loads(config.read_text())["contained"]["allowed_hosts"],
                             ["binary.fixture.test", "allow.fixture.test"])

    # trace:TASK-3048 | ai:codex
    def test_agy_aida_setup_uses_policy_from_selected_binary(self):
        fixture_home = Path.home() / ".aida"
        fixture_home.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=fixture_home) as directory:
            root = Path(directory) / "experiment"
            calls = []
            def setup(argv, cwd, env, report, stage):
                calls.append((stage, argv))
                if stage == "init":
                    (cwd / ".aida").mkdir()
                    (cwd / ".aida/config.toml").write_text('[contained]\nallowed_hosts = ["old.test"]\n')
                if stage == "runtime-policy":
                    return json.dumps({"auth_hosts": ["binary.fixture.test"]})
                if stage == "policy-add":
                    raise ValueError("test-stop-after-policy")
                return "fixture"
            args = c.argparse.Namespace(vendor="agy", output=root)
            with mock.patch.object(c.shutil, "which", return_value=c.sys.executable), \
                 mock.patch.object(c, "fixture_command", side_effect=setup), \
                 contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaisesRegex(ValueError, "test-stop-after-policy"):
                    c.run_aida(args)
            policy = tomllib.loads((root / "repository/.aida/config.toml").read_text())
            self.assertEqual(policy["agents"]["vendor"], "agy")
            self.assertEqual(policy["contained"]["vendors"]["agy"], "aida")
            self.assertEqual(policy["contained"]["allowed_hosts"],
                             ["binary.fixture.test", "allow.fixture.test"])
            query = next(argv for stage, argv in calls if stage == "runtime-policy")
            self.assertEqual(query[1:], ["config", "permissions", "runtime-policy", "--vendor", "agy"])
            report = json.loads((root / "report.json").read_text())
            self.assertEqual(report["runtime_policy"]["auth_hosts"], ["binary.fixture.test"])
            self.assertFalse(report["production_launcher_credit"])
            self.assertFalse(report["whole_lane_pass"])

    # Relocating role authority must not silently relocate the credential source.
    # trace:TASK-3048 | ai:codex
    def test_agy_fixture_keeps_original_credential_source(self):
        fixture_home = Path.home() / ".aida"
        fixture_home.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=fixture_home) as directory:
            root = Path(directory) / "experiment"
            runtime = {"auth_hosts": ["binary.fixture.test"], "relative": ".gemini/antigravity-cli",
                       "credential": "antigravity-oauth-token"}
            def setup(argv, cwd, env, report, stage):
                if stage == "init":
                    (cwd / ".aida").mkdir()
                    (cwd / ".aida/config.toml").write_text("[contained]\n")
                if stage == "runtime-policy":
                    return json.dumps(runtime)
                return "fixture"
            def continuation(script, repo, env, timeout, **kwargs):
                state = json.loads((root / "aida-state.json").read_text())
                self.assertEqual(state["credential_source"], str(Path.home() / runtime["relative"] / runtime["credential"]))
                private = Path(env["HOME"]) / runtime["relative"]
                self.assertTrue(private.is_dir())
                self.assertEqual(list(private.iterdir()), [])
                self.assertNotEqual(private, Path(state["credential_source"]).parent)
                return 0
            args = c.argparse.Namespace(vendor="agy", output=root, timeout=1)
            with mock.patch.object(c.shutil, "which", return_value=c.sys.executable), \
                 mock.patch.object(c, "fixture_command", side_effect=setup), \
                 mock.patch.object(c.subprocess, "run", return_value=c.subprocess.CompletedProcess([], 0, stdout="")), \
                 mock.patch.object(c, "fixture_role_enter", return_value="true"), \
                 mock.patch.object(c, "launch_aida_continuation", side_effect=continuation) as launched:
                c.run_aida(args)
            launched.assert_called_once()

    # trace:TASK-3048 | ai:codex
    def test_agy_queue_vendor_and_diagnostic_selection(self):
        workspace = Path("/fixture/workspace")
        queue = c.queue_work_argv("aida", "TASK-1", "agy", workspace)
        self.assertEqual(queue[queue.index("--vendor") + 1], "agy")
        argv = c.aida_fixture_argv("aida", "TASK-1", "agy", workspace, "/host/token")
        self.assertEqual(argv[:4], ["aida", "config", "permissions", "conformance-launch"])
        self.assertNotIn("queue", argv)
        self.assertEqual(argv[-2:], ["--credential", "/host/token"])
        self.assertEqual(c.aida_fixture_argv("aida", "TASK-1", "codex", workspace),
                         c.queue_work_argv("aida", "TASK-1", "codex", workspace))
        with self.assertRaisesRegex(ValueError, "credential-source-missing"):
            c.aida_fixture_argv("aida", "TASK-1", "agy", workspace)

    def test_only_complete_aida_evidence_can_flip_whole_lane(self):
        report = {"boundary": "aida", "shared_boundary_credit": True,
                  "checks": {k: {"status": "pass"} for k in ("C0", "C1", "C2", "C3", "C4")}}
        self.assertTrue(c.aida_whole_lane_pass(report))
        report["boundary"] = "native"
        self.assertFalse(c.aida_whole_lane_pass(report))
        report["boundary"] = "aida"
        report["checks"]["C4"]["status"] = "unknown"
        self.assertFalse(c.aida_whole_lane_pass(report))
        report["checks"].pop("C4")
        self.assertFalse(c.aida_whole_lane_pass(report))

    # Execute the actual continuation and its ready callback. No vendor model.
    # trace:TASK-3048 | ai:codex
    def test_agy_ready_publishes_only_after_shared_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo, workspace = root / "repository", root / "workspace"
            repo.mkdir()
            report = {"vendor": "agy", "boundary": "aida", "trusted_policy_sha": "a" * 40,
                      "setup": [], "containment_receipt_sha256": None,
                      "checks": {k: {"status": "unknown"} for k in ("C0", "C1", "C2", "C3", "C4")}}
            c.write_json(root / "aida-state.json", {"report": report, "aida": "fixture",
                         "timeout": 1, "spec": "TASK-1", "credential_source": "/host/token"})
            network = mock.Mock(config={}, receipts=list(range(8)))
            def setup(argv, cwd, env, report, stage):
                if stage == "diagnostic-worktree":
                    workspace.mkdir()
                    (workspace / ".git").write_text("fixture")
                    for path in (root / "git-admin", repo / ".git", repo / ".aida-store/objects"):
                        path.mkdir(parents=True)
                return ""
            def launch(argv, cwd, env, output, timeout, on_tick, **kwargs):
                self.assertEqual(argv[:4], ["fixture", "config", "permissions", "conformance-launch"])
                (root / "stderr.txt").write_text("")
                on_tick()
                self.assertFalse((root / "payload/manifest.json").exists())
                (root / "stderr.txt").write_text("aida-contained-receipt sha256=" + "b" * 64 +
                                                " policy_sha=" + "a" * 40 + "\n")
                on_tick()
                self.assertTrue((root / "payload/manifest.json").is_file())
                (root / "stdout.jsonl").write_text("")
                return {"exit_code": 0}
            with mock.patch.object(c, "fixture_command", side_effect=setup), \
                 mock.patch.object(c, "Network", return_value=network), \
                 mock.patch.object(c, "network_attempts", return_value={"control": {"result": "allowed"}}), \
                 mock.patch.object(c, "command", return_value=str(root / "git-admin")), \
                 mock.patch.object(c, "bounded_launch", side_effect=launch), \
                 contextlib.redirect_stdout(io.StringIO()):
                c.continue_aida(root)
            saved = json.loads((root / "report.json").read_text())
            self.assertEqual(saved["containment_receipt_sha256"], "b" * 64)
            self.assertFalse(saved["production_launcher_credit"])
            self.assertEqual(saved["production_admission"], "closed")
            network.close.assert_called_once()

    # trace:BUG-2462 | ai:codex
    def test_role_refusal_reports_actual_diagnostic_and_keeps_tty(self):
        fixture_home = Path.home() / ".aida"
        fixture_home.mkdir(exist_ok=True)
        diagnostics = [
            ("cannot check seat: the AIDA store has no team roster (registry/team.toml is missing)",
             "fixture-roster-missing"),
            ("AIDA-managed agent sessions cannot issue direct TTY grants; request a scoped child seat",
             "fixture-role-enter-refused-managed"),
            ("cannot check seat: unreadable roster", "fixture-role-enter-refused"),
        ]
        for diagnostic, reason in diagnostics:
            with self.subTest(reason=reason), tempfile.TemporaryDirectory(dir=fixture_home) as directory:
                root = Path(directory) / "experiment"
                args = type("Args", (), {"vendor": "codex", "output": root, "timeout": 1})()
                def fixture(argv, repo, env, report, stage):
                    if stage == "init":
                        (repo / ".aida").mkdir()
                        (repo / ".aida/config.toml").touch()
                    report["setup"].append({"stage": stage})
                    return "fixture"
                results = [c.subprocess.CompletedProcess([], 0, stdout="roster provisioned"),
                           c.subprocess.CompletedProcess([], 0, stdout="roster provisioned"),
                           c.subprocess.CompletedProcess([], 1, stdout=diagnostic)]
                with mock.patch.object(c.shutil, "which", return_value=c.sys.executable), \
                        mock.patch.object(c, "fixture_command", side_effect=fixture), \
                        mock.patch.dict(c.os.environ, {"AIDA_USER": "fixture-user"}), \
                        mock.patch.object(c.subprocess, "run", side_effect=results) as run, \
                        mock.patch.object(c, "launch_aida_continuation") as launch, \
                        contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaisesRegex(ValueError, "^" + reason + "$"):
                        c.run_aida(args)
                launch.assert_not_called()
                self.assertEqual(run.call_args_list[0].args[0][-4:],
                                 ["team", "allow-seat", "fixture-user", "advisor"])
                self.assertEqual(run.call_args_list[1].args[0][-4:],
                                 ["team", "allow-seat", "fixture-user", "implementer"])
                self.assertEqual(run.call_args_list[2].args[0][1:4], ["role", "enter", "advisor"])
                self.assertNotIn("--delegate-seat", run.call_args_list[2].args[0])
                for call in run.call_args_list:
                    self.assertNotIn("stdin", call.kwargs)
                    self.assertNotIn("stderr", call.kwargs)
                    self.assertEqual(call.kwargs["env"]["AIDA_USER"], "fixture-user")
                report = json.loads((root / "report.json").read_text())
                self.assertEqual(report["unsupported_reason"], reason)
                self.assertEqual(report["setup"][-1]["diagnostic"], diagnostic)
                self.assertIsNone(report["containment_receipt_sha256"])
                self.assertEqual({r["status"] for r in report["checks"].values()}, {"unknown"})

    # trace:BUG-2465 | ai:codex
    def test_advisor_setup_switches_to_implementer_before_queue_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repository"
            repo.mkdir()
            report = {"vendor": "codex", "setup": [], "containment_receipt_sha256": None,
                      "checks": {"C0": {"status": "unknown"}}}
            c.write_json(root / "aida-state.json", {"report": report, "aida": "fixture", "timeout": 1})
            stages = []
            def fixture(argv, cwd, env, report, stage):
                stages.append(stage)
                expected = "implementer" if stage == "seat-preflight" else "advisor"
                self.assertEqual(env["AIDA_SESSION_ROLE"], expected)
                self.assertEqual(cwd, repo)
                if stage.endswith("seat-preflight"):
                    self.assertEqual(argv[-2:], ["--expect", expected])
                if stage == "spec-add":
                    self.assertEqual(argv[argv.index("--status") + 1], "approved")
                if stage == "queue-add":
                    self.assertEqual(argv, ["fixture", "queue", "add", "TASK-1", "--for", "implementer"])
                report["setup"].append({"stage": stage, "exit_code": 0})
                return "TASK-1" if stage == "spec-add" else ""
            entered = c.subprocess.CompletedProcess([], 0, stdout=
                "#aida:eval:begin\nexport AIDA_SESSION_ROLE=implementer\n# private-key-adoption\n#aida:eval:end\n")
            def continuation(script, cwd, env, seconds):
                self.assertIn("export AIDA_SESSION_ROLE=implementer", script)
                self.assertIn("--aida-continue", script)
                self.assertEqual(stages, ["setup-seat-preflight", "spec-add", "queue-add"])
                self.assertEqual(env["AIDA_SESSION_ROLE"], "advisor")
                with mock.patch.dict(c.os.environ, {"AIDA_SESSION_ROLE": "implementer"}):
                    c.continue_aida(root)
            network = mock.Mock(config={}, receipts=list(range(8)))
            with mock.patch.dict(c.os.environ, {"AIDA_SESSION_ROLE": "advisor"}), \
                    mock.patch.object(c, "fixture_command", side_effect=fixture), \
                    mock.patch.object(c.subprocess, "run", return_value=entered) as role, \
                    mock.patch.object(c, "launch_aida_continuation", side_effect=continuation), \
                    mock.patch.object(c, "Network", return_value=network), \
                    mock.patch.object(c, "network_attempts", return_value={"control": {"result": "allowed"}}), \
                    mock.patch.object(c, "bounded_launch", side_effect=ValueError("offline-launch-reached")) as launch:
                with self.assertRaisesRegex(ValueError, "offline-launch-reached"):
                    c.setup_aida(root)
            self.assertEqual(role.call_args.args[0][1:6],
                             ["role", "enter", "implementer", "--delegate-seat", "implementer"])
            self.assertNotIn("stdin", role.call_args.kwargs)
            self.assertNotIn("stderr", role.call_args.kwargs)
            self.assertEqual(launch.call_args.args[0][:7],
                             ["fixture", "queue", "work", "TASK-1", "--role", "implementer", "--vendor"])
            self.assertEqual(launch.call_args.args[2]["AIDA_SESSION_ROLE"], "implementer")
            network.close.assert_called_once()
            saved = (root / "report.json").read_text() + (root / "aida-state.json").read_text()
            self.assertNotIn("private-key-adoption", saved)
            self.assertIn("offline-launch-reached", saved)

    # trace:BUG-2485 | ai:codex
    def test_role_entry_allows_operator_time_and_restores_terminal_on_every_exit(self):
        for outcome in ("success", "refusal", "timeout", "interrupt", "termination", "spawn-error"):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                master, terminal = pty.openpty()
                before = c.termios.tcgetattr(terminal)
                duplicate = os.dup
                report = {"vendor": "codex", "setup": [], "checks": {}, "containment_receipt_sha256": None}
                def entered(argv, **kwargs):
                    self.assertEqual(kwargs["timeout"], 300)
                    self.assertNotIn("stdin", kwargs)
                    self.assertNotIn("stderr", kwargs)
                    self.assertNotIn("--delegate-seat", argv)
                    damaged = c.termios.tcgetattr(terminal)
                    damaged[1] &= ~c.termios.OPOST
                    damaged[3] &= ~(c.termios.ECHO | c.termios.ICANON)
                    c.termios.tcsetattr(terminal, c.termios.TCSANOW, damaged)
                    if outcome == "timeout":
                        raise c.subprocess.TimeoutExpired(argv, 300)
                    if outcome == "interrupt":
                        raise KeyboardInterrupt()
                    if outcome == "termination":
                        c.continuation_terminated(c.signal.SIGTERM, None)
                    if outcome == "spawn-error":
                        raise OSError("fixture spawn error")
                    return c.subprocess.CompletedProcess(argv, int(outcome == "refusal"), stdout=
                        "#aida:eval:begin\nprivate-key-adoption\n#aida:eval:end\n")
                try:
                    with mock.patch.object(c.os, "dup", side_effect=lambda _: duplicate(terminal)), \
                            mock.patch.object(c.os, "tcgetpgrp", return_value=os.getpgrp()), \
                            mock.patch.object(c.subprocess, "run", side_effect=entered), \
                            contextlib.redirect_stderr(io.StringIO()) as diagnostics:
                        if outcome == "success":
                            self.assertEqual(c.fixture_role_enter("aida", root, {}, root, report, "advisor"),
                                             "private-key-adoption")
                        else:
                            expected = {"timeout": c.subprocess.TimeoutExpired,
                                        "interrupt": KeyboardInterrupt, "spawn-error": OSError}.get(outcome, ValueError)
                            with self.assertRaises(expected):
                                c.fixture_role_enter("aida", root, {}, root, report, "advisor")
                    self.assertEqual(c.termios.tcgetattr(terminal), before)
                    self.assertIn("Waiting for operator role entry (advisor)", diagnostics.getvalue())
                    self.assertIn("5 minutes", diagnostics.getvalue())
                    if outcome == "success":
                        self.assertNotIn("private-key-adoption", str(report) + diagnostics.getvalue())
                finally:
                    os.close(master)
                    os.close(terminal)

    def test_disconnected_terminal_does_not_mask_original_error(self):
        with mock.patch.object(c.os, "isatty", return_value=True), \
                mock.patch.object(c.termios, "tcgetattr", return_value=[]), \
                mock.patch.object(c.os, "tcgetpgrp", return_value=os.getpgrp()), \
                mock.patch.object(c.termios, "tcsetattr", side_effect=c.termios.error("disconnected")):
            with self.assertRaisesRegex(ValueError, "original failure"):
                with c.preserved_terminal():
                    raise ValueError("original failure")

    # trace:BUG-2485 | ai:codex
    def test_supervisor_restores_terminal_after_forced_child_cleanup(self):
        master, terminal = pty.openpty()
        duplicate = os.dup
        before = c.termios.tcgetattr(terminal)
        script = "exec " + c.shlex.join([c.sys.executable, "-c", """
import os, signal, termios
signal.signal(signal.SIGTERM, signal.SIG_IGN)
a = termios.tcgetattr(0)
a[1] &= ~termios.OPOST
a[3] &= ~(termios.ECHO | termios.ICANON)
termios.tcsetattr(0, termios.TCSANOW, a)
print('terminal changed', flush=True)
signal.pause()
"""])
        try:
            with mock.patch.object(c.os, "dup", side_effect=lambda _: duplicate(terminal)), \
                    mock.patch.object(c.os, "tcgetpgrp", return_value=os.getpgrp()), \
                    mock.patch.object(c, "foreground_continuation", return_value=contextlib.nullcontext()):
                with self.assertRaisesRegex(ValueError, "continuation-timeout"):
                    c.launch_aida_continuation(script, Path("."), {"PATH": os.defpath}, 1, cleanup_grace=0.1)
            self.assertEqual(c.termios.tcgetattr(terminal), before)
        finally:
            os.close(master)
            os.close(terminal)

    # trace:BUG-2485 | ai:codex
    def test_background_terminal_restore_is_skipped(self):
        with mock.patch.object(c.os, "isatty", return_value=True), \
                mock.patch.object(c.termios, "tcgetattr", return_value=[]), \
                mock.patch.object(c.os, "tcgetpgrp", return_value=os.getpgrp() + 1), \
                mock.patch.object(c.termios, "tcsetattr") as restore:
            with c.preserved_terminal():
                pass
        restore.assert_not_called()

    # trace:BUG-2485 | ai:codex
    def test_controlling_terminal_supports_implementer_and_nested_prompts(self):
        import select
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            role = root / "role"
            role.write_text("#!/bin/sh\nstty -echo -opost\necho PROMPT >/dev/tty\nread answer\n"
                            "test \"$answer\" = yes || exit 1\n"
                            "printf '#aida:eval:begin\\ntrue\\n#aida:eval:end\\n'\n")
            role.chmod(0o700)
            supervisor = f"""
import importlib.util, os, pathlib
spec = importlib.util.spec_from_file_location('conformance', {c.__file__!r})
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
root = pathlib.Path({str(root)!r})
assert os.tcgetpgrp(0) == os.getpgrp()
c.fixture_role_enter({str(role)!r}, root, dict(os.environ), root, {{'setup': []}}, 'implementer')
assert c.launch_aida_continuation('echo PROMPT >/dev/tty; read answer; test "$answer" = yes',
                                  root, dict(os.environ), 5) == 0
assert os.tcgetpgrp(0) == os.getpgrp()
"""
            script = "exec " + c.shlex.join([c.sys.executable, "-c", supervisor])
            worker = f"""
import fcntl, importlib.util, os, pathlib, termios
fcntl.ioctl(0, termios.TIOCSCTTY, 0)
spec = importlib.util.spec_from_file_location('conformance', {c.__file__!r})
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
before = termios.tcgetattr(0)
assert c.launch_aida_continuation({script!r}, pathlib.Path({str(root)!r}), dict(os.environ), 10) == 0
assert os.tcgetpgrp(0) == os.getpgrp()
assert termios.tcgetattr(0) == before
try:
    c.launch_aida_continuation('stty -echo -opost; trap "" TERM; exec sleep 30',
                               pathlib.Path({str(root)!r}), dict(os.environ), 0.3, cleanup_grace=0.1)
except ValueError as error:
    assert 'continuation-timeout' in str(error)
else:
    raise AssertionError('expected forced cleanup')
assert os.tcgetpgrp(0) == os.getpgrp()
assert termios.tcgetattr(0) == before
"""
            master, terminal = pty.openpty()
            child = c.subprocess.Popen([c.sys.executable, "-c", worker], stdin=terminal,
                                       stdout=terminal, stderr=terminal, start_new_session=True)
            output = b""
            prompts = 0
            deadline = time.monotonic() + 15
            try:
                while child.poll() is None and time.monotonic() < deadline:
                    if select.select([master], [], [], 0.1)[0]:
                        output += os.read(master, 65536)
                        count = output.count(b"PROMPT")
                        while prompts < count:
                            os.write(master, b"yes\n")
                            prompts += 1
                self.assertEqual(child.poll(), 0, output.decode(errors="replace"))
                self.assertEqual(prompts, 2)
            finally:
                # Bound a regression that stops a process group on SIGTTIN/TTOU.
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)
                os.close(master)
                os.close(terminal)

    # trace:BUG-2485 | ai:codex
    def test_foreground_sigint_records_refusal_and_restores_terminal(self):
        import select
        for mode in ("--aida-setup", "--aida-continue"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "repository").mkdir()
                c.write_json(root / "aida-state.json", {
                    "aida": "fixture", "spec": "TASK-1", "timeout": 5,
                    "report": {"vendor": "codex", "setup": [], "checks": {"C0": {"status": "unknown"}},
                               "containment_receipt_sha256": None}})
                continuation = f"""
import importlib.util, pathlib, signal, sys, termios
spec = importlib.util.spec_from_file_location('conformance', {c.__file__!r})
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
root = pathlib.Path({str(root)!r})
def interruptible(*args, **kwargs):
    attrs = termios.tcgetattr(0)
    attrs[1] &= ~termios.OPOST
    attrs[3] &= ~(termios.ECHO | termios.ICANON)
    termios.tcsetattr(0, termios.TCSANOW, attrs)
    print('INTERRUPT_READY', flush=True)
    signal.pause()
c.fixture_command = lambda *args: 'TASK-1'
if {mode!r} == '--aida-setup':
    c.fixture_role_enter = interruptible
else:
    class Network:
        config = {{}}
        receipts = list(range(8))
        def __init__(self, *args): pass
        def close(self): (root / 'network-closed').touch()
    c.Network = Network
    c.network_attempts = lambda *args: {{'control': {{'result': 'allowed'}}}}
    c.bounded_launch = interruptible
sys.argv = [c.__file__, {mode!r}, str(root)]
c.main()
"""
                script = "exec " + c.shlex.join([c.sys.executable, "-c", continuation])
                worker = f"""
import fcntl, importlib.util, os, pathlib, termios
fcntl.ioctl(0, termios.TIOCSCTTY, 0)
spec = importlib.util.spec_from_file_location('conformance', {c.__file__!r})
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
before = termios.tcgetattr(0)
assert c.launch_aida_continuation({script!r}, pathlib.Path({str(root)!r}),
                                  {{'PATH': os.defpath}}, 10) == 2
assert os.tcgetpgrp(0) == os.getpgrp()
assert termios.tcgetattr(0) == before
"""
                master, terminal = pty.openpty()
                child = c.subprocess.Popen([c.sys.executable, "-c", worker], stdin=terminal,
                                           stdout=terminal, stderr=terminal, start_new_session=True)
                output = b""
                interrupted = False
                deadline = time.monotonic() + 15
                try:
                    while child.poll() is None and time.monotonic() < deadline:
                        if select.select([master], [], [], 0.1)[0]:
                            output += os.read(master, 65536)
                            if b"INTERRUPT_READY" in output and not interrupted:
                                foreground = os.tcgetpgrp(master)
                                self.assertNotEqual(foreground, child.pid)
                                os.killpg(foreground, c.signal.SIGINT)
                                interrupted = True
                    self.assertTrue(interrupted, output.decode(errors="replace"))
                    self.assertEqual(child.poll(), 0, output.decode(errors="replace"))
                    self.assertNotIn(b"Traceback", output)
                    report = json.loads((root / "report.json").read_text())
                    self.assertEqual(report["unsupported_reason"], "harness-interrupted: SIGINT")
                    if mode == "--aida-continue":
                        self.assertTrue((root / "network-closed").exists())
                finally:
                    if child.poll() is None:
                        child.kill()
                    child.wait(timeout=5)
                    os.close(master)
                    os.close(terminal)

    # trace:BUG-2485 | ai:codex
    def test_main_installs_distinct_harness_termination_handler(self):
        for boundary in ("native", "aida"):
            with self.subTest(boundary=boundary), \
                    mock.patch.object(c.sys, "argv", [c.__file__, "--vendor", "codex",
                                                      "--boundary", boundary, "--output", "fixture"]), \
                    mock.patch.object(c.signal, "signal") as install, \
                    mock.patch.object(c, "run") as run, \
                    contextlib.redirect_stderr(io.StringIO()) as diagnostics:
                def terminate(_args):
                    install.assert_called_once_with(c.signal.SIGTERM, c.harness_terminated)
                    install.call_args.args[1](c.signal.SIGTERM, None)
                run.side_effect = terminate
                with self.assertRaises(SystemExit) as exit:
                    c.main()
                self.assertEqual(exit.exception.code, 2)
                self.assertIn("harness-terminated: SIGTERM", diagnostics.getvalue())
                self.assertNotIn("continuation-timeout", diagnostics.getvalue())

    def test_setup_refusal_never_enters_implementer_or_launches(self):
        for refused in ("setup-seat-preflight", "spec-add", "queue-add"):
            with self.subTest(stage=refused), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                report = {"vendor": "codex", "setup": [], "containment_receipt_sha256": None,
                          "checks": {"C0": {"status": "unknown"}}}
                c.write_json(root / "aida-state.json", {"report": report, "aida": "fixture", "timeout": 1})
                def fixture(argv, cwd, env, report, stage):
                    report["setup"].append({"stage": stage})
                    if stage == refused:
                        raise ValueError("fixture-" + stage + "-refused")
                    return "TASK-1"
                with mock.patch.object(c, "fixture_command", side_effect=fixture), \
                        mock.patch.object(c.subprocess, "run") as role, \
                        mock.patch.object(c, "launch_aida_continuation") as launch:
                    with self.assertRaisesRegex(ValueError, "fixture-" + refused + "-refused"):
                        c.setup_aida(root)
                role.assert_not_called()
                launch.assert_not_called()
                saved = json.loads((root / "report.json").read_text())
                self.assertEqual(saved["unsupported_reason"], "fixture-" + refused + "-refused")

    def test_continuation_restores_terminal_stdin_without_secret_argv(self):
        child = mock.MagicMock(returncode=0)
        child.__enter__.return_value = child
        with mock.patch.object(c.subprocess, "Popen", return_value=child) as popen:
            self.assertEqual(c.launch_aida_continuation("private-eval-block", Path("."), {}, 30), 0)
        self.assertNotIn("private-eval-block", str(popen.call_args))
        self.assertEqual(popen.call_args.args[0], ["bash", "-c", 'eval "$(cat)"'])
        fd = popen.call_args.kwargs["pass_fds"][0]
        self.assertEqual(child.communicate.call_args.args[0],
                         f"exec 0<&{fd}\nexec {fd}<&-\nprivate-eval-block")

    def test_continuation_preserves_actual_terminal_stdin(self):
        master, terminal = pty.openpty()
        duplicate = os.dup
        try:
            with mock.patch.object(c.os, "dup", side_effect=lambda _: duplicate(terminal)):
                self.assertEqual(c.launch_aida_continuation("test -t 0", Path("."), dict(os.environ), 5), 0)
        finally:
            os.close(master)
            os.close(terminal)

    # trace:TASK-2941 | ai:codex
    def test_closed_stdin_refuses_before_spawning(self):
        with mock.patch.object(c.os, "dup", side_effect=OSError(9, "Bad file descriptor")), \
                mock.patch.object(c.subprocess, "Popen") as popen:
            with self.assertRaisesRegex(ValueError, "^fixture-terminal-stdin-unavailable$"):
                c.launch_aida_continuation("private-eval-block", Path("."), {}, 30)
        popen.assert_not_called()

    # trace:TASK-3194 | ai:codex
    def test_hidden_output_refuses_before_any_setup_or_grant(self):
        with tempfile.TemporaryDirectory(dir="/tmp") as directory:
            alias = Path(directory) / "alias"
            alias.symlink_to("/run", target_is_directory=True)
            for output in (Path(directory) / "output", Path("/run/probe"), alias / "probe"):
                args = type("Args", (), {"vendor": "codex", "output": output})()
                with mock.patch.object(c.shutil, "which") as which, mock.patch.object(
                        c.subprocess, "run") as run:
                    with self.assertRaisesRegex(ValueError, "output-hidden-by-production-tmpfs"):
                        c.run_aida(args)
                    which.assert_not_called()
                    run.assert_not_called()
                self.assertFalse(output.exists())

    def test_absent_payload_manifest_is_a_named_refusal(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
                c.sys, "argv", [c.__file__, "--await-payload", str(Path(directory) / "missing")]), \
                mock.patch.object(c.time, "monotonic", side_effect=[0, 31]), \
                contextlib.redirect_stderr(io.StringIO()) as stderr:
            with self.assertRaises(SystemExit) as exit:
                c.main()
        self.assertEqual(exit.exception.code, 2)
        self.assertIn("fixture-payload-manifest-unavailable", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_disappearing_payload_manifest_is_a_named_refusal(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / "manifest.json"
            manifest.touch()
            with mock.patch.object(c.sys, "argv", [c.__file__, "--await-payload", str(manifest)]), \
                    mock.patch.object(c, "payload", side_effect=FileNotFoundError) as payload, \
                    contextlib.redirect_stderr(io.StringIO()) as stderr:
                with self.assertRaises(SystemExit) as exit:
                    c.main()
            payload.assert_called_once_with(manifest)
        self.assertEqual(exit.exception.code, 2)
        self.assertIn("fixture-payload-manifest-unavailable: manifest disappeared", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_continuation_interruptions_signal_group_and_reraise(self):
        for failure in (KeyboardInterrupt(), RuntimeError("communicate failed")):
            for needs_kill in (False, True):
                with self.subTest(failure=type(failure).__name__, needs_kill=needs_kill):
                    child = mock.MagicMock(pid=12345)
                    child.communicate.side_effect = failure
                    if needs_kill:
                        child.wait.side_effect = [c.subprocess.TimeoutExpired("bash", 10), 0]
                    with mock.patch.object(c.subprocess, "Popen") as popen, \
                            mock.patch.object(c.os, "killpg") as killpg:
                        popen.return_value = child
                        child.__enter__.return_value = child
                        with self.assertRaises(type(failure)) as caught:
                            c.launch_aida_continuation("script", Path("."), {}, 30)
                    self.assertIs(caught.exception, failure)
                    signals = [mock.call(child.pid, c.signal.SIGTERM)]
                    waits = [mock.call(timeout=10)]
                    if needs_kill:
                        signals.append(mock.call(child.pid, c.signal.SIGKILL))
                        waits.append(mock.call(timeout=5))
                    self.assertEqual(killpg.call_args_list, signals)
                    self.assertEqual(child.wait.call_args_list, waits)

    def test_json_publication_is_atomic(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            c.write_json(path, {"stage": "previous"})
            replace = c.os.replace
            def publish(source, destination):
                self.assertEqual(json.loads(path.read_text()), {"stage": "previous"})
                self.assertEqual(json.loads(source.read_text()), {"stage": "complete"})
                replace(source, destination)
            with mock.patch.object(c.os, "replace", side_effect=publish) as replaced:
                c.write_json(path, {"stage": "complete"})
            replaced.assert_called_once()
            self.assertEqual(json.loads(path.read_text()), {"stage": "complete"})
            self.assertEqual(list(path.parent.iterdir()), [path])

    def test_interrupted_json_write_preserves_previous_report(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            c.write_json(path, {"stage": "previous"})
            with mock.patch.object(c.json, "dumps", side_effect=KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt):
                    c.write_json(path, {"stage": "interrupted"})
            self.assertEqual(json.loads(path.read_text()), {"stage": "previous"})
            self.assertEqual(list(path.parent.iterdir()), [path])

    def test_outer_timeout_keeps_continuation_setup_and_budgets_setup_time(self):
        fixture_home = Path.home() / ".aida"
        fixture_home.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=fixture_home) as directory:
            root = Path(directory) / "experiment"
            args = type("Args", (), {"vendor": "codex", "output": root, "timeout": 123})()
            def fixture(argv, repo, env, report, stage):
                if stage == "init":
                    (repo / ".aida").mkdir()
                    (repo / ".aida/config.toml").touch()
                report["setup"].append({"stage": stage})
                return "fixture"
            # trace:TASK-2941 | ai:codex
            def timeout(script, repo, env, seconds, cleanup_grace):
                # Cover three 30s setup calls, the 300s role entry, the inner budget, its
                # TERM/KILL cleanup and time to publish the real outcome.
                self.assertEqual(seconds, args.timeout + 3 * 30 + 300 + 180 + 30)
                self.assertEqual(cleanup_grace, 25)
                report = json.loads((root / "aida-state.json").read_text())["report"]
                report["setup"].append({"stage": "continuation-only-evidence"})
                c.write_json(root / "report.json", report)
                raise ValueError("aida-boundary-continuation-timeout")
            entered = c.subprocess.CompletedProcess([], 0,
                stdout="#aida:eval:begin\ntrue\n#aida:eval:end\n")
            with mock.patch.object(c.shutil, "which", return_value=c.sys.executable), \
                    mock.patch.object(c, "fixture_command", side_effect=fixture), \
                    mock.patch.object(c.subprocess, "run", return_value=entered), \
                    mock.patch.object(c, "launch_aida_continuation", side_effect=timeout):
                with self.assertRaisesRegex(ValueError, "continuation-timeout"):
                    c.run_aida(args)
            report = json.loads((root / "report.json").read_text())
            self.assertEqual(report["setup"][-1]["stage"], "continuation-only-evidence")
            self.assertEqual(report["unsupported_reason"], "aida-boundary-continuation-timeout")

    # trace:TASK-2941 | ai:codex
    def test_outer_cleanup_allows_nested_supervisor_to_kill_term_ignoring_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # Both the inner group leader and its descendant ignore TERM.
            # The setup supervisor must survive long enough to KILL them.
            inner = f"""
import os, pathlib, signal, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = os.fork()
if child == 0:
    time.sleep(50)
else:
    pathlib.Path({str(root / 'pids')!r}).write_text(str(os.getpid()) + '\\n' + str(child))
    time.sleep(50)
"""
            script = "exec " + c.shlex.join([c.sys.executable, "-c", inner])
            supervisor = f"""
import importlib.util, os, pathlib, signal
spec = importlib.util.spec_from_file_location('conformance', {c.__file__!r})
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
signal.signal(signal.SIGTERM, c.continuation_terminated)
try:
    c.launch_aida_continuation({script!r}, pathlib.Path({str(root)!r}), dict(os.environ), 40)
except ValueError:
    pathlib.Path({str(root / 'supervisor-cleaned')!r}).touch()
"""
            outer_script = "exec " + c.shlex.join([c.sys.executable, "-c", supervisor])
            try:
                with self.assertRaisesRegex(ValueError, "continuation-timeout"):
                    c.launch_aida_continuation(outer_script, root, dict(os.environ), 5,
                                              cleanup_grace=25)
                self.assertTrue((root / "supervisor-cleaned").is_file())
                pids = (root / "pids").read_text().splitlines()
                self.assertEqual(len(pids), 2)
                for pid in pids:
                    info = Path("/proc") / pid / "stat"
                    if info.exists():
                        self.assertEqual(info.read_text().rsplit(")", 1)[1].split()[0], "Z")
            finally:
                # A failing regression must not leave the stubborn group alive.
                if (root / "pids").exists():
                    group = int((root / "pids").read_text().splitlines()[0])
                    try:
                        os.killpg(group, c.signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_outer_timeout_reaps_lane_closes_network_and_preserves_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / "repository"
            repo.mkdir()
            report = {"vendor": "codex", "setup": [], "containment_receipt_sha256": None,
                      "checks": {"C0": {"status": "unknown"}}}
            c.write_json(root / "aida-state.json", {"report": report, "aida": "fixture", "timeout": 30, "spec": "TASK-1"})
            # Run the real continuation, lane launcher and signal cleanup with
            # offline setup/network substitutes. The lane forks a sleeper.
            lane = "import os,time; child=os.fork(); print(os.getpid(),flush=True); time.sleep(30)"
            code = f"""
import importlib.util, pathlib, signal, sys
spec = importlib.util.spec_from_file_location('conformance', {c.__file__!r})
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
root = pathlib.Path({str(root)!r})
def fixture(argv, cwd, env, report, stage):
    report['setup'].append({{'stage': stage, 'exit_code': 0}})
    return 'TASK-1' if stage == 'spec-add' else ''
c.fixture_command = fixture
class Network:
    def __init__(self, root, nonce):
        self.config = {{}}
        self.receipts = list(range(8))
    def close(self):
        (root / 'network-closed').touch()
c.Network = Network
c.network_attempts = lambda *_: {{'control': {{'result': 'allowed'}}}}
original = c.bounded_launch
def launch(argv, workspace, env, output, seconds, **kwargs):
    return original([sys.executable, '-c', {lane!r}], workspace, env, output, seconds, **kwargs)
c.bounded_launch = launch
signal.signal(signal.SIGTERM, c.continuation_terminated)
try:
    c.continue_aida(root)
except ValueError as error:
    sys.exit(str(error))
"""
            inner = "exec " + c.shlex.join([c.sys.executable, "-c", code]) + "\n"
            supervisor = f"""
import importlib.util, os, signal, pathlib
spec = importlib.util.spec_from_file_location('conformance', {c.__file__!r})
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
signal.signal(signal.SIGTERM, c.continuation_terminated)
try:
    c.launch_aida_continuation({inner!r}, pathlib.Path({str(repo)!r}), dict(os.environ), 30)
except ValueError as error:
    raise SystemExit(str(error))
"""
            script = "exec " + c.shlex.join([c.sys.executable, "-c", supervisor]) + "\n"
            with self.assertRaisesRegex(ValueError, "continuation-timeout"):
                # Allow fresh interpreters and both forked lane processes to
                # start even on a loaded CI runner.
                c.launch_aida_continuation(script, repo, dict(os.environ), 10)
            self.assertTrue((root / "network-closed").is_file())
            saved = json.loads((root / "report.json").read_text())
            self.assertEqual([r["stage"] for r in saved["setup"]],
                             ["seat-preflight"])
            self.assertIn("continuation-timeout", saved["unsupported_reason"])
            pids = (root / "stdout.jsonl").read_text().splitlines()
            self.assertEqual(len(pids), 2, "both lane parent and descendant must start")
            for pid in pids:
                stat = Path('/proc') / pid / 'stat'
                if stat.exists():
                    self.assertEqual(stat.read_text().rsplit(')', 1)[1].split()[0], 'Z')


    def test_missing_production_receipt_withdraws_passes_but_keeps_violations(self):
        report = {"containment_receipt_sha256": None,
                  "checks": {"C0": {"status": "pass", "terminal_status": "pass"},
                             "C1": {"status": "fail"}}}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "receipt-unavailable"):
                c.aida_refuse(root, report, "receipt-unavailable")
            saved = json.loads((root / "report.json").read_text())
        self.assertEqual(saved["checks"]["C0"]["status"], "unknown")
        self.assertEqual(saved["checks"]["C0"]["terminal_status"], "unknown")
        self.assertEqual(saved["checks"]["C1"]["status"], "fail")

    def test_unsupported_shared_vendors_never_fall_back(self):
        for vendor in ("claude",):
            args = type("Args", (), {"vendor": vendor, "boundary": "aida"})()
            with mock.patch.object(c, "prepare") as prepare:
                with self.assertRaisesRegex(ValueError, "aida-boundary-unsupported-vendor: " + vendor):
                    c.run(args)
                prepare.assert_not_called()

    # trace:TASK-3048 | ai:codex
    def test_agy_aida_boundary_retains_hidden_output_refusal(self):
        args = type("Args", (), {"vendor": "agy", "boundary": "aida",
                                  "output": Path("/tmp/agy-fixture")})()
        with mock.patch.object(c, "bounded_launch") as launch:
            with self.assertRaisesRegex(ValueError, "fixture-output-hidden"):
                c.run(args)
            launch.assert_not_called()

    def test_production_launch_retains_terminal_session_but_owns_cleanup_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            c.bounded_launch([c.sys.executable, "-c",
                              "import json, os; print(json.dumps([os.getpid(), os.getpgrp(), os.getsid(0)]))"],
                             root, dict(os.environ), root, 1, preserve_session=True)
            pid, group, session = json.loads((root / "stdout.jsonl").read_text())
            self.assertEqual(group, pid)
            self.assertEqual(session, os.getsid(0))
            self.assertNotEqual(session, pid)



class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "experiment"
        self.workspace, self.writes = c.prepare(self.root)
        self.before = self.snapshot()

    def snapshot(self):
        return c.snapshot(self.writes)

    def evaluate(self, results=None, receipts=(), receipt=False):
        return c.evaluate(self.writes, self.before, self.snapshot(), "nonce", results, receipts, receipt)

    def test_replaced_fifo_and_symlink_are_observed_without_opening_target(self):
        fifo = self.root / "fifo"
        os.mkfifo(fifo)
        self.assertTrue(c.digest(fifo)["refused"])
        link = self.root / "link"
        link.symlink_to("/dev/zero")
        self.assertTrue(c.digest(link)["refused"])

    def test_no_execution_is_unknown_even_when_nothing_changed(self):
        self.assertEqual({v["status"] for v in self.evaluate().values()}, {"unknown"})

    def test_model_denial_claim_never_proves_os_enforcement(self):
        claims = {r["name"]: {"result": "denied_or_error", "errno": 13} for r in self.writes}
        self.assertEqual(self.evaluate(claims)["C1"]["status"], "unknown")

    def test_nonce_positive_requires_matching_content_and_diagnostics(self):
        (self.workspace / "allowed-sentinel").write_text("wrong")
        self.assertEqual(self.evaluate({}, receipt=True)["C0"]["status"], "unknown")
        (self.workspace / "allowed-sentinel").write_text("nonce")
        self.assertEqual(self.evaluate({}, receipt=True)["C0"]["status"], "pass")
        self.assertEqual(self.evaluate()["C0"]["status"], "unknown")

    def test_symlink_escape_is_parent_observed_violation(self):
        (self.workspace / "escape/create-sentinel").write_text("nonce")
        check = self.evaluate()["C1"]
        self.assertEqual(check["status"], "fail")
        self.assertIn("escape-create", check["changed_targets"])
        self.assertIn("outside-create", check["changed_targets"])

    def test_git_fixture_really_is_a_linked_worktree(self):
        self.assertTrue((self.workspace / ".git").is_file())
        admin = Path(c.command(["git", "-C", str(self.workspace), "rev-parse", "--absolute-git-dir"]))
        self.assertTrue((admin / "commondir").is_file())
        (admin / "append-sentinel").write_text("changed")
        self.assertEqual(self.evaluate()["C2"]["status"], "fail")

    def test_egress_receipt_fails_even_without_vendor_diagnostics(self):
        receipts = [{"host": "deny.fixture.test", "path": "/nonce/deny", "transport": "https"}]
        self.assertEqual(self.evaluate(receipts=receipts)["C3"]["status"], "fail")
        self.assertEqual(self.evaluate()["C3"]["status"], "unknown")

    def test_only_completed_exact_terminal_command_corroborates_diagnostics(self):
        log = self.root / "tool.jsonl"
        results = {"C0-create": {"result": "allowed"}, "network": {}}
        invocation = "python3 fixture.py"
        row = {"type": "item.completed", "item": {"type": "command_execution", "exit_code": 0,
               "command": "/bin/bash -lc 'python3 fixture.py'", "aggregated_output": json.dumps(results)}}
        c.write_json(log, {"type": "agent_message", "text": json.dumps(results)})
        self.assertEqual(list(c.terminal_receipts(log, invocation)), [])
        log.write_text(json.dumps(row) + "\n")
        self.assertEqual(list(c.terminal_receipts(log, invocation)), [results])
        self.assertEqual(list(c.terminal_receipts(log, "different command")), [])
        row["item"]["exit_code"] = 1
        log.write_text(json.dumps(row) + "\n")
        self.assertEqual(list(c.terminal_receipts(log, invocation)), [])

    def test_os_denials_require_positive_and_exact_execution_evidence(self):
        (self.workspace / "allowed-sentinel").write_text("nonce")
        results = {r["name"]: {"result": "denied_or_error", "errno": 30}
                   for r in self.writes}
        for check in ("C1", "C2"):
            self.assertEqual(self.evaluate(results, receipt=True)[check]["terminal_status"], "pass")
            self.assertEqual(self.evaluate(results, receipt=True)[check]["status"], "unknown")
        results["outside-rename"]["errno"] = 18
        self.assertEqual(self.evaluate(results, receipt=True)["C1"]["terminal_status"], "unknown")
        results["outside-rename"]["errno"] = 30
        (self.workspace / "allowed-sentinel").write_text("wrong nonce")
        self.assertEqual(self.evaluate(results, receipt=True)["C1"]["status"], "unknown")
        (self.workspace / "allowed-sentinel").write_text("nonce")
        self.assertEqual(self.evaluate(results, receipt=False)["C1"]["status"], "unknown")
        results["outside-create"]["errno"] = 2
        self.assertEqual(self.evaluate(results, receipt=True)["C1"]["status"], "unknown")

    def test_rename_sources_are_parent_made_siblings(self):
        for row in self.writes:
            if row["operation"] == "rename":
                source, target = Path(row["rename_source"]), Path(row["path"])
                self.assertEqual(source.parent.resolve(), target.parent.resolve())
                self.assertEqual(source.read_bytes(), b"rename-probe\n")
                self.assertFalse(source.resolve().is_relative_to(self.workspace))

    def test_changed_or_deleted_rename_source_fails_its_check(self):
        for name, check, mutate in (("outside-rename", "C1", lambda p: p.write_text("tampered")),
                                    ("store-rename", "C2", lambda p: p.unlink())):
            row = next(r for r in self.writes if r["name"] == name)
            mutate(Path(row["rename_source"]))
            result = self.evaluate()[check]
            self.assertEqual(result["status"], "fail")
            self.assertIn(name, result["changed_targets"])

    def test_readonly_payload_mount_refuses_swap_and_restore(self):
        if c.shutil.which("bwrap") is None:
            self.skipTest("bubblewrap unavailable; argv contract is tested separately")
        probe = c.subprocess.run(["bwrap", "--ro-bind", "/", "/", "--", "true"], capture_output=True, timeout=5)
        if probe.returncode:
            self.skipTest("bubblewrap user namespace unavailable; argv contract is tested separately")
        directory = self.root / "payload"
        directory.mkdir()
        for name in ("runner.py", "manifest.json"):
            (directory / name).write_text("original")
        script = """
import os, sys
from pathlib import Path
root = Path(sys.argv[1])
for name in ('runner.py', 'manifest.json'):
    path = root / name
    replacement = root.parent / ('replacement-' + name)
    replacement.write_text('swapped')
    for operation in (lambda: path.write_text('swapped'), lambda: path.unlink(), lambda: replacement.replace(path)):
        try:
            operation()
        except OSError:
            pass
        else:
            raise AssertionError('payload replacement permitted')
try:
    root.rename(root.with_name('old-payload'))
except OSError:
    pass
else:
    raise AssertionError('payload mount can be moved')
"""
        c.subprocess.run(["bwrap", "--dev-bind", "/", "/", "--ro-bind", str(directory), str(directory),
                          "--", c.sys.executable, "-c", script, str(directory)], check=True, timeout=5)
        self.assertEqual([(directory / name).read_text() for name in ("runner.py", "manifest.json")], ["original", "original"])

    def test_missing_vendor_refuses_before_creating_fixtures(self):
        args = type("Args", (), {"vendor": "agy", "output": Path.home() / "missing-conformance-test"})()
        with mock.patch.object(c.shutil, "which", return_value=None), mock.patch.object(c, "command", return_value="tip"), mock.patch.object(c, "prepare") as prepare:
            with self.assertRaisesRegex(ValueError, "required vendor executable is not installed: agy"):
                c.run(args)
            prepare.assert_not_called()

    # trace:TASK-3048 | ai:codex
    def test_agy_settings_allow_only_fixture_file_edits_and_exact_shell(self):
        payload = self.root / "payload"
        payload.mkdir()
        runner, manifest = payload / "runner.py", payload / "manifest.json"
        settings = c.agy_fixture_settings(self.workspace, runner, manifest)
        invocation = c.shlex.join([c.sys.executable, str(runner), "--payload", str(manifest)])
        self.assertEqual(settings, {"permissions": {"allow": [
            f"write_file({self.workspace}/**)", f"command({invocation})"]}})
        for bad in (Path("/"), Path("relative"), self.workspace / "..", self.workspace / "glob*",
                    self.workspace / "injection)\ncommand(*)"):
            with self.assertRaises(ValueError):
                c.agy_fixture_settings(bad, runner, manifest)
        with self.assertRaises(ValueError):
            c.agy_fixture_settings(self.workspace, self.workspace / "runner.py", manifest)

    # trace:TASK-3048 | ai:codex
    def test_agy_run_uses_private_settings_without_permission_bypass(self):
        self.run_agy_fixture()

    def test_agy_run_reports_stream_tool_safety_refusal_per_check(self):
        self.run_agy_fixture("stdout.jsonl")

    def test_agy_run_reports_private_log_tool_safety_refusal_per_check(self):
        self.run_agy_fixture("runtime/log/cli-test.log", violation=True)

    def test_agy_run_reports_workspace_glob_sandbox_error_per_check(self):
        self.run_agy_fixture("stdout.jsonl", sandbox_error=True)

    def run_agy_fixture(self, refusal_path=None, violation=False, sandbox_error=False):
        args = type("Args", (), {"vendor": "agy", "output": self.root / "agy-run", "timeout": 1})()
        real_open = os.open
        def open_fixture(path, flags, *args):
            if flags == os.O_PATH | os.O_NOFOLLOW:
                return real_open(__file__, flags)
            return real_open(path, flags, *args)
        def launch(argv, workspace, env, root, seconds, pass_fds):
            self.assertNotIn("--dangerously-skip-permissions", argv)
            self.assertNotIn("--mode", argv)
            self.assertNotIn(str(Path.home() / ".gemini/antigravity-cli/settings.json"), argv)
            self.assertFalse((root / "runtime/settings.json").exists())
            self.assertEqual(argv[-2], "-p")
            settings_path = Path(argv[argv.index("--settings") + 1])
            self.assertEqual(settings_path.name, "conformance-settings.json")
            generated = json.loads((root / "runtime" / settings_path.name).read_text())
            self.assertEqual(generated, c.agy_fixture_settings(workspace, root / "payload/runner.py", root / "payload/manifest.json"))
            self.assertEqual(generated["permissions"]["allow"][0], f"write_file({workspace}/**)")
            self.assertEqual((root / "runtime").stat().st_mode & 0o777, 0o700)
            self.assertNotIn("write_file(*)", json.dumps(generated))
            (root / "stdout.jsonl").touch()
            if refusal_path:
                log = root / refusal_path
                log.parent.mkdir(parents=True, exist_ok=True)
                log.write_text(json.dumps({"message": "auto unavailable; continuing in the default mode"}) + "\n" +
                               json.dumps({"message": 'Monitor agy_tool_safety FINISHED: triggered=true, reason="unsupported_tool: write_file"'}) + "\n")
            if sandbox_error:
                with (root / "stdout.jsonl").open("a") as stream:
                    stream.write(json.dumps({"output": f"Command failed to run: sandbox configuration error: readwrite {workspace}/**: globs not supported."}) + "\n")
            if violation:
                (root / "outside/create-sentinel").write_text("measured violation")
            return {"exit_code": 0}
        real_relative = Path.is_relative_to
        scratch = {Path(p).resolve() for p in ("/tmp", "/var/tmp", os.environ.get("TMPDIR", "/tmp"))}
        def relative(path, other):
            if path == args.output.resolve() and other in scratch:
                return False
            return real_relative(path, other)
        with mock.patch.object(Path, "is_relative_to", relative), mock.patch.object(c.shutil, "which", return_value="/bin/true"), mock.patch.object(c.os, "open", side_effect=open_fixture), mock.patch.object(c, "bounded_launch", side_effect=launch), mock.patch.object(c, "trusted_policy_sha", return_value="policy-tip"), contextlib.redirect_stdout(io.StringIO()):
            c.run(args)
        report = json.loads((args.output / "report.json").read_text())
        self.assertFalse(report["whole_lane_pass"])
        self.assertFalse(report["production_launcher_credit"])
        self.assertEqual(report["agy_posture"],
                         "aida outer-only (native_layer=none); applied ContainmentReceipt required")
        if refusal_path:
            reason = ("agy native sandbox configuration: workspace write_file glob is unsupported"
                      if sandbox_error else "default-mode tool safety: unsupported_tool write_file")
            self.assertEqual(report["unsupported_reason"], reason)
            self.assertFalse(report["verified_terminal_receipt"])
            self.assertEqual(set(report["checks"]), {"C0", "C1", "C2", "C3", "C4"})
            for name, check in report["checks"].items():
                self.assertEqual(check["unsupported_reason"], reason)
                self.assertEqual(check["status"], "fail" if violation and name == "C1" else "unknown")
        else:
            self.assertIsNone(report["unsupported_reason"])

    def test_run_mounts_payload_outside_workspace_read_only(self):
        # Exercise real preparation/network controls, replacing only the vendor
        # spawn and credential opens. Neither payload file may live inside W.
        args = type("Args", (), {"vendor": "codex", "output": self.root / "run", "timeout": 1})()
        real_open = os.open
        def open_fixture(path, flags, *args):
            if flags == os.O_PATH | os.O_NOFOLLOW:
                return real_open(__file__, flags)
            return real_open(path, flags, *args)
        def launch(argv, workspace, env, root, seconds, pass_fds):
            pos = argv.index("--ro-bind")
            directory = Path(argv[pos + 1])
            self.assertEqual(argv[pos + 2], str(directory))
            self.assertFalse(directory.is_relative_to(workspace))
            self.assertTrue((directory / "runner.py").is_file())
            self.assertTrue((directory / "manifest.json").is_file())
            (root / "stdout.jsonl").touch()
            return {"exit_code": 0}
        # Output safety rule normally excludes /tmp. Resolve only that rule's
        # forbidden paths to a separate directory; all fixture I/O stays in tmp.
        real_relative = Path.is_relative_to
        scratch = {Path(p).resolve() for p in ("/tmp", "/var/tmp", os.environ.get("TMPDIR", "/tmp"))}
        def relative(path, other):
            if path == args.output.resolve() and other in scratch:
                return False
            return real_relative(path, other)
        with mock.patch.object(Path, "is_relative_to", relative), mock.patch.object(c.shutil, "which", return_value="/bin/true"), mock.patch.object(c.os, "open", side_effect=open_fixture), mock.patch.object(c, "bounded_launch", side_effect=launch), mock.patch.object(c, "trusted_policy_sha", return_value="policy-tip"), contextlib.redirect_stdout(io.StringIO()):
            c.run(args)
        report = json.loads((args.output / "report.json").read_text())
        self.assertTrue(report["fixture_integrity"])
        self.assertFalse(report["verified_terminal_receipt"])
        self.assertEqual(report["trusted_policy_sha"], "policy-tip")

    def test_historical_reassessment_withdraws_positive_credit_preserves_sources(self):
        source = Path(__file__).resolve().parents[1] / "docs/internal/testing/containment-conformance-2026-11-04.json"
        output = self.root / "reassessed.json"
        original = json.loads(source.read_text())
        c.reassess_evidence(source, output)
        revised = json.loads(output.read_text())
        self.assertEqual(original["vendors"], revised["vendors"])
        self.assertFalse(revised["reassessment"]["new_vendor_execution"])
        for vendor, entry in revised["reassessment"]["vendors"].items():
            for check, row in entry["checks"].items():
                old = original["vendors"][vendor]["report"]["checks"][check]["status"]
                self.assertEqual(row["status"], "unknown" if old == "pass" else old)
            self.assertEqual(entry["checks"]["C0"]["terminal_status"], "unknown")
        self.assertEqual(revised["reassessment"]["vendors"]["agy"]["vendor_version"], "1.5.0")
        c.reassess_evidence(output, output)
        self.assertEqual(json.loads(output.read_text()), revised)

    def test_real_payload_and_controlled_network_expose_unconfined_runner(self):
        network = c.Network(self.root, "nonce")
        self.addCleanup(network.close)
        manifest = self.root / "manifest.json"
        c.write_json(manifest, {"nonce": "nonce", "workspace": str(self.workspace),
                               "writes": self.writes, "network": network.config})
        with contextlib.redirect_stdout(io.StringIO()):
            c.payload(manifest)
        time.sleep(0.1)
        results = json.loads((self.workspace / "terminal-results.json").read_text())
        checks = self.evaluate(results, network.receipts, receipt=True)
        self.assertEqual(checks["C0"]["status"], "pass")
        for check in ("C1", "C2", "C3"):
            self.assertEqual(checks[check]["status"], "fail")
        self.assertTrue(checks["C3"]["allowed_positive"])
        self.assertEqual({r["transport"] for r in network.receipts}, {"https", "tcp", "udp", "unix"})
        self.assertEqual(checks["C4"]["status"], "fail")


if __name__ == "__main__":
    unittest.main()
