"""Hermetic notification adapter tests: no real mail, Codex, or user services."""
# trace:TASK-2347 | ai:codex
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

MODULE = Path(__file__).resolve().parents[1] / "scripts/codex-mail-wake.py"
spec = importlib.util.spec_from_file_location("mail_wake", MODULE)
wake = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wake)
THREAD = "01a1700b-717d-71f2-b7e0-0569b05f3dee"


class WatcherTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        home = self.root / "home"
        home.mkdir()
        env = patch.dict(os.environ, {"HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"),
                                      "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": str(home / "gitconfig")})
        env.start()
        self.addCleanup(env.stop)
        self.state = wake.State(self.root / "state")
        self.addCleanup(self.state.close)
        self.ledger = wake.initial(self.root, THREAD)

    def tick(self, inventory, now=100, **kwargs):
        return wake.tick(self.state, self.ledger, self.root, THREAD, "fixture-aida", "fixture-codex",
                         kwargs.pop("batch_seconds", 5), now=now, inventory=inventory, **kwargs)

    def test_batch_restart_and_unchanged_unread_never_duplicate(self):
        ids = {"mail-a": {"/main", "/sibling"}, "mail-b": {"/sibling"}}
        with patch.object(wake, "command", return_value=b"accepted") as cmd:
            self.assertFalse(self.tick(ids)["queue_submitted"])
            self.assertFalse(self.tick(ids, now=104)["queue_submitted"])
            self.assertTrue(self.tick(ids, now=105)["queue_submitted"])
            argv = cmd.call_args.args[0]
            self.assertEqual(argv[:4], ["fixture-codex", "queue", "--thread", THREAD])
            self.assertIn("mail-a, mail-b", argv[-1])
            self.assertNotIn("PRIVATE BODY", argv[-1])
            self.ledger = self.state.read()
            wake.validate(self.ledger, self.root, THREAD)
            self.assertFalse(self.tick(ids, now=200)["queue_submitted"])
            self.assertEqual(cmd.call_count, 1)
            self.assertEqual(self.ledger["accepted"], ["mail-a", "mail-b"])

    def test_no_mail_and_dry_run_do_not_queue_or_write_delivery(self):
        with patch.object(wake, "command") as cmd:
            self.assertFalse(self.tick({})["queue_submitted"])
            before = self.state.read()
            result = self.tick({"new-id": {"/sibling"}}, dry_run=True)
            self.assertTrue(result["dry_run"])
            self.assertEqual(self.state.read(), before)
            cmd.assert_not_called()

    def test_pre_spawn_failure_retries_with_backoff_without_ack(self):
        with patch.object(wake, "command", side_effect=wake.CreationFailure):
            result = self.tick({"new-id": {"/sibling"}}, batch_seconds=0)
        self.assertFalse(result["queue_submitted"])
        self.assertEqual(self.ledger["accepted"], [])
        self.assertIsNone(self.ledger["inflight"])
        with patch.object(wake, "command", return_value=b"accepted") as cmd:
            self.assertFalse(self.tick({}, now=105, batch_seconds=0)["queue_submitted"])
            self.assertTrue(self.tick({}, now=110, batch_seconds=0)["queue_submitted"])
            self.assertEqual(cmd.call_count, 1)

    def test_uncertain_submission_is_not_automatically_duplicated(self):
        with patch.object(wake, "command", side_effect=wake.Refusal("failed")):
            with self.assertRaises(wake.Refusal):
                self.tick({"new-id": {"/sibling"}}, batch_seconds=0)
        ledger = self.state.read()
        self.assertEqual(ledger["accepted"], [])
        self.assertEqual(ledger["inflight"]["ids"], ["new-id"])
        with self.assertRaises(wake.Refusal):
            wake.validate(ledger, self.root, THREAD)

    def test_duplicate_process_lock_and_live_status(self):
        with self.assertRaises(wake.Refusal):
            wake.State(self.root / "state")
        status = wake.State(self.root / "state", exclusive=False)
        self.addCleanup(status.close)
        self.assertTrue(status.running)

    def test_state_symlink_hardlink_and_identity_refused(self):
        self.state.write(self.ledger)
        outside = self.root / "outside"
        outside.write_text("private")
        ledger = self.root / "state/ledger.json"
        ledger.unlink()
        ledger.symlink_to(outside)
        with self.assertRaises(OSError):
            self.state.read()
        ledger.unlink()
        os.link(outside, ledger)
        with self.assertRaises(wake.Refusal):
            self.state.read()
        with self.assertRaises(wake.Refusal):
            wake.validate(self.ledger, self.root, "another-thread")

    def test_worktree_scan_uses_machine_envelope_dedups_and_refuses_bad_root(self):
        sibling = self.root / "sibling"
        (sibling / ".aida/mailbox").mkdir(parents=True)
        calls = []
        def fake(argv, **kwargs):
            calls.append(argv)
            if argv[0] == "git":
                return (f"worktree {self.root}\0\0worktree {sibling}\0\0").encode()
            self.assertIn("wake-snapshot", argv)
            self.assertEqual(kwargs.get("cwd"), self.root)
            return json.dumps({"version": 1, "messages": [{"id": "shared-id"}]}).encode()
        with patch.object(wake, "command", side_effect=fake):
            result = wake.scan(self.root, "fixture-aida")
        self.assertEqual(result, {"shared-id": {str(self.root), str(sibling)}})
        self.assertEqual(len(calls), 3)
        with patch.object(wake, "command", side_effect=[b"worktree /one\0", wake.Refusal("scan failed")]):
            # project root is always inspected; a missing sibling is skipped.
            with self.assertRaises(wake.Refusal):
                wake.scan(Path("/one"), "fixture-aida")

    def test_service_unit_refuses_links_without_overwriting_private_target(self):
        directory = self.root / "units"
        directory.mkdir()
        outside = self.root / "private-unit-target"
        outside.write_text("PRIVATE")
        unit = directory / "watch.service"
        unit.symlink_to(outside)
        with self.assertRaises(wake.Refusal):
            wake.write_unit(unit, "PUBLIC")
        unit.unlink()
        os.link(outside, unit)
        with self.assertRaises(wake.Refusal):
            wake.write_unit(unit, "PUBLIC")
        self.assertEqual(outside.read_text(), "PRIVATE")
        unit.unlink()
        wake.write_unit(unit, "PUBLIC")
        self.assertEqual(unit.read_text(), "PUBLIC")
        self.assertEqual(unit.stat().st_mode & 0o777, 0o600)

    def test_service_dry_run_cannot_install_or_start_anything(self):
        state = self.root / "not-created"
        result = subprocess.run(["python3", str(MODULE), "install", "--dry-run", "--project", str(self.root),
                                 "--thread", THREAD, "--state-dir", str(state)], capture_output=True, check=True)
        self.assertTrue(json.loads(result.stdout)["dry_run"])
        self.assertFalse(state.exists())
        self.assertFalse((Path(os.environ["XDG_CONFIG_HOME"]) / "systemd").exists())

    def test_inherited_repo_override_cannot_redirect_explicit_project(self):
        with patch.dict(os.environ, {"AIDA_STORE": "/private/other", "GIT_DIR": "/other/git", "AIDA_SESSION_GRANT": "preserved"}):
            env = wake.repo_environment()
        self.assertNotIn("AIDA_STORE", env)
        self.assertNotIn("GIT_DIR", env)
        self.assertEqual(env["AIDA_SESSION_GRANT"], "preserved")

    def test_systemd_preserves_literal_dollar_percent_quotes_and_newline(self):
        self.assertEqual(wake.systemd_escape('/fixture/${HOME}/%user/"a"\n'),
                         '"/fixture/$${HOME}/%%user/\\"a\\"\\n"')

    def test_empty_existing_ledger_is_refused(self):
        for value in ({}, [], None, {"version": 1}):
            with self.assertRaises(wake.Refusal):
                wake.validate(value, self.root, THREAD)
        directory = self.root / "corrupt-state"
        directory.mkdir(mode=0o700)
        ledger = directory / "ledger.json"
        ledger.write_text("{}")
        ledger.chmod(0o600)
        result = subprocess.run(["python3", str(MODULE), "once", "--project", str(self.root),
                                 "--thread", THREAD, "--state-dir", str(directory)], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"invalid schema", result.stderr)
        self.assertEqual(ledger.read_text(), "{}")

    def test_explicit_recovery_preserves_read_state_and_can_restart(self):
        directory = self.root / "recover-state"
        state = wake.State(directory)
        ledger = wake.initial(self.root, THREAD)
        ledger["pending"] = {"new-id": ["/sibling"]}
        ledger["first_pending"] = 100
        ledger["inflight"] = {"ids": ["new-id"], "submitted_at": 100}
        state.write(ledger)
        state.close()
        args = ["python3", str(MODULE), "recover", "--project", str(self.root),
                "--thread", THREAD, "--state-dir", str(directory), "--accepted"]
        result = json.loads(subprocess.check_output(args))
        self.assertTrue(result["recovered"])
        state = wake.State(directory)
        self.addCleanup(state.close)
        recovered = state.read()
        wake.validate(recovered, self.root, THREAD)
        self.assertEqual(recovered["accepted"], ["new-id"])
        self.assertIsNone(recovered["inflight"])
        self.assertFalse((self.root / ".aida").exists())

    def test_post_spawn_pipe_failure_keeps_uncertain_claim(self):
        with patch.object(wake.os, "set_blocking", side_effect=OSError("fixture pipe failure")):
            with self.assertRaises(wake.Refusal):
                wake.command(["python3", "-c", "print('accepted')"])
        with patch.object(wake, "command", side_effect=wake.Refusal("post-spawn failure")):
            with self.assertRaises(wake.Refusal):
                self.tick({"new-id": {"/sibling"}}, batch_seconds=0)
        self.assertIsNotNone(self.state.read()["inflight"])

    def test_command_output_and_time_are_bounded(self):
        with self.assertRaises(wake.Refusal):
            wake.command(["python3", "-c", "import time; time.sleep(5)"], timeout=0.05)
        with patch.object(wake, "LIMIT", 100):
            with self.assertRaises(wake.Refusal):
                wake.command(["python3", "-c", "print('x'*200)"])

    def test_real_git_inventory_and_fake_cli_end_to_end(self):
        repo = self.root / "repo"
        repo.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                        "commit", "--allow-empty", "-qm", "fixture"], check=True)
        sibling = self.root / "linked"
        subprocess.run(["git", "-C", str(repo), "worktree", "add", "-qb", "sibling", str(sibling)], check=True)
        (sibling / ".aida/mailbox").mkdir(parents=True)
        (repo / ".aida/mailbox").mkdir(parents=True)
        fake_aida = self.root / "fixture-aida"
        fake_aida.write_text("#!/usr/bin/env python3\nimport json\nprint(json.dumps({'version':1,'messages':[{'id':'fixture-message'}]}))\n")
        fake_aida.chmod(0o700)
        capture = self.root / "queue.json"
        fake_codex = self.root / "fixture-codex"
        fake_codex.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\nPath(" + repr(str(capture)) + ").write_text(json.dumps(sys.argv[1:]))\n")
        fake_codex.chmod(0o700)
        argv = ["python3", str(MODULE), "once", "--project", str(repo), "--thread", THREAD,
                "--state-dir", str(self.root / "integrated"), "--aida", str(fake_aida),
                "--codex", str(fake_codex), "--batch-seconds", "0"]
        first = json.loads(subprocess.check_output(argv, cwd=self.root))
        self.assertTrue(first["queue_submitted"])
        self.assertEqual(first["ids"], ["fixture-message"])
        self.assertEqual(json.loads(capture.read_text())[:3], ["queue", "--thread", THREAD])
        before = capture.stat().st_mtime_ns
        second = json.loads(subprocess.check_output(argv, cwd=self.root))
        self.assertFalse(second["queue_submitted"])
        self.assertEqual(capture.stat().st_mtime_ns, before)
        self.assertFalse((repo / ".aida/mailbox/.read").exists())
        self.assertFalse((sibling / ".aida/mailbox/.read").exists())


if __name__ == "__main__":
    unittest.main()
