"""Hermetic notification adapter tests: no real mail, Codex, or user services."""
# trace:TASK-2347 | ai:codex
import importlib.util
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import uuid
from types import SimpleNamespace
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
        env = patch.dict(os.environ, {"HOME": str(home), "AIDA_HOME": "", "XDG_CONFIG_HOME": str(home / ".config"),
                                      "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": str(home / "gitconfig")})
        env.start()
        self.addCleanup(env.stop)
        validator = patch.object(wake, "validate_global_scheduler_snapshot")
        self.native_validator = validator.start()
        self.addCleanup(validator.stop)
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

    def test_shared_read_receipts_remove_stale_replica_but_preserve_private_mail(self):
        sibling = self.root / "sibling"
        (sibling / ".aida/mailbox").mkdir(parents=True)
        envelopes = [
            {"version": 1, "messages": [], "acknowledged_ids": ["replica"]},
            {"version": 1, "messages": [{"id": "replica"}, {"id": "private"}], "acknowledged_ids": []},
        ]
        with patch.object(wake, "worktrees", return_value=[self.root, sibling]), patch.object(
                wake, "command", side_effect=[json.dumps(e).encode() for e in envelopes]):
            inventory, acknowledged = wake.scan(self.root, "fixture-aida")
        self.assertEqual(inventory, {"private": {str(sibling)}})
        self.assertEqual(acknowledged, {"replica"})
        self.tick({"replica": {str(sibling)}})
        with patch.object(wake, "command") as command:
            self.tick({}, now=110, acknowledged=acknowledged)
            command.assert_not_called()
        self.assertEqual(self.ledger["pending"], {})
        self.assertEqual(self.ledger["accepted"], [])

    def test_snapshot_without_read_evidence_refuses(self):
        with patch.object(wake, "worktrees", return_value=[self.root]), patch.object(
                wake, "command", return_value=b'{"version":1,"messages":[]}'):
            with self.assertRaises(wake.Refusal):
                wake.scan(self.root, "fixture-aida")

    def test_read_accept_cycles_prune_history_and_timestamp_metadata(self):
        with patch.object(wake, "command", return_value=b"accepted") as cmd, patch.object(wake.os, "fsync"):
            for n in range(150):
                ident = f"mail-{n}"
                self.tick({ident: {"/sibling"}}, now=100 + n, batch_seconds=0)
                self.assertEqual(self.ledger["accepted"], [ident])
                self.assertEqual(self.ledger["accepted_at"], {ident: 100 + n})
                self.tick({}, now=100 + n, acknowledged={ident})
                self.assertEqual(self.ledger["accepted"], [])
                self.assertEqual(self.ledger["accepted_at"], {})
                wake.validate(self.state.read(), self.root, THREAD)
            self.assertEqual(cmd.call_count, 150)
        self.assertLess(len(json.dumps(self.state.read())), 1024)

    def test_accepted_unread_survives_missing_inventory_and_failed_full_scan(self):
        with patch.object(wake, "command", return_value=b"accepted"):
            self.tick({"unread": {"/removed-sibling"}}, batch_seconds=0)
        before = self.state.read()
        with patch.object(wake, "command") as cmd:
            self.tick({}, now=150)
            self.tick({"unread": {"/main"}}, now=200)
            cmd.assert_not_called()
        self.assertEqual(self.state.read(), before)
        with patch.object(wake, "scan", side_effect=wake.Refusal("later root failed")):
            with self.assertRaises(wake.Refusal):
                wake.tick(self.state, self.ledger, self.root, THREAD, "fixture-aida", "fixture-codex", 0)
        self.assertEqual(self.state.read(), before)

    def test_health_keeps_legacy_ages_unknown_and_oldest_known_across_batches(self):
        self.ledger["accepted"] = ["legacy"]
        self.ledger["last_acceptance"] = {"ids": ["legacy"], "time": 1}
        wake.validate(self.ledger, self.root, THREAD)
        self.assertEqual(wake.health(self.ledger, now=100), {
            "accepted_unacknowledged_count": 1, "accepted_unknown_age_count": 1,
            "oldest_known_notification_age_seconds": None, "recovery_required": False})
        with patch.object(wake, "command", return_value=b"accepted"):
            self.tick({"first": {"/main"}}, now=100, batch_seconds=0)
            self.tick({"second": {"/main"}}, now=150, batch_seconds=0)
        self.assertEqual(wake.health(self.ledger, now=200)["oldest_known_notification_age_seconds"], 100)
        self.tick({}, now=200, acknowledged={"first", "legacy"})
        self.assertEqual(wake.health(self.ledger, now=200), {
            "accepted_unacknowledged_count": 1, "accepted_unknown_age_count": 0,
            "oldest_known_notification_age_seconds": 50, "recovery_required": False})

    def test_timestamp_schema_refuses_orphans_invalid_values_and_unknown_fields(self):
        self.ledger["accepted"] = ["known"]
        for stamps in ({"orphan": 100}, {"known": True}, {"known": -1}, {"known": float("nan")}, []):
            self.ledger["accepted_at"] = stamps
            with self.assertRaises(wake.Refusal):
                wake.validate(self.ledger, self.root, THREAD)
        self.ledger["accepted_at"] = {"known": 100}
        wake.validate(self.ledger, self.root, THREAD)
        self.ledger["extra"] = None
        with self.assertRaises(wake.Refusal):
            wake.validate(self.ledger, self.root, THREAD)

    def test_status_reports_persisted_health_without_scanning_or_queueing(self):
        self.ledger["accepted"] = ["legacy", "known"]
        self.ledger["accepted_at"] = {"known": 100}
        self.state.write(self.ledger)
        before = (self.root / "state/ledger.json").read_bytes()
        result = json.loads(subprocess.check_output([
            "python3", str(MODULE), "status", "--project", str(self.root), "--thread", THREAD,
            "--state-dir", str(self.root / "state"), "--aida", "/nonexistent", "--codex", "/nonexistent"]))
        self.assertTrue(result["running"])
        self.assertEqual(result["health"]["accepted_unacknowledged_count"], 2)
        self.assertEqual(result["health"]["accepted_unknown_age_count"], 1)
        self.assertGreater(result["health"]["oldest_known_notification_age_seconds"], 0)
        self.assertEqual((self.root / "state/ledger.json").read_bytes(), before)

    def test_dense_backlog_batches_once_with_a_hard_byte_bound(self):
        pending = {str(uuid.uuid4()): ["/source"] for _ in range(580)}
        batch, message = wake.notification(pending)
        self.assertEqual(len(batch), 580)
        self.assertLessEqual(len(message.encode()), 24_000)
        larger = {str(uuid.uuid4()): ["/source"] for _ in range(2000)}
        batch, message = wake.notification(larger)
        self.assertGreater(len(batch), 500)
        self.assertLess(len(batch), 1024)
        self.assertLessEqual(len(message.encode()), 24_000)
        self.assertEqual(batch, sorted(larger)[:len(batch)])

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
            return json.dumps({"version": 1, "messages": [{"id": "shared-id"}], "acknowledged_ids": []}).encode()
        with patch.object(wake, "command", side_effect=fake):
            result, acknowledged = wake.scan(self.root, "fixture-aida")
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

    def service_args(self, action="install"):
        args = self.health_args()
        args.action, args.dry_run = action, False
        args.state_dir = self.root / "service-state"
        args.codex, args.batch_seconds = "fixture-codex", 5
        (self.root / ".aida").mkdir(exist_ok=True)
        return args

    def fake_service(self, calls, fail=None, active="inactive", edit_config=False):
        def run(argv, **kwargs):
            calls.append(argv)
            if argv[0] == "fixture-aida":
                return b'{"total":0,"findings":[]}'
            self.assertEqual(argv[:2], ["systemctl", "--user"])
            if argv[2] == "show":
                data = (f"ActiveState={active}\nUnitFileState=disabled\nLoadState=loaded\n").encode()
                return (0, data) if kwargs.get("with_status") else data
            if argv[2] == fail:
                if edit_config:
                    with (self.root / ".aida/config.toml").open("a") as out:
                        out.write('\n[[schedule.jobs]]\nname="intervening-user"\ncommand="cache verify"\nevery="1h"\n')
                raise wake.CreationFailure("fixture failure")
            return b""
        return run

    def test_service_ownership_and_capability_preflight_leave_original_files_unchanged(self):
        args = self.service_args()
        args.state_dir.mkdir(mode=0o700)
        script = args.state_dir / "watcher.py"
        script.write_text("original script")
        config = self.root / ".aida/config.toml"
        name = wake.health_registration(args, self.root)["job"]
        config.write_text('[[schedule.jobs]]\nname=' + json.dumps(name) + '\nprompt="user"\nevery="1h"\n')
        before = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        with patch.object(wake, "worktrees", return_value=[self.root]), patch.object(wake, "command") as command:
            with self.assertRaises(wake.Refusal):
                wake.service(args, self.root)
            command.assert_not_called()
        self.assertEqual({str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}, before)
        config.write_text("# original config\n")
        before = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        with patch.object(wake, "worktrees", return_value=[self.root]), patch.object(
                wake, "command", return_value=b'{"error":"unknown category"}'):
            with self.assertRaises(wake.Refusal):
                wake.service(args, self.root)
        self.assertEqual({str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}, before)
        self.assertFalse((self.root / ".aida/codex-mail-health.lock").exists())

    def test_service_later_install_failures_retain_private_original_evidence_and_user_edits(self):
        for failure in ("unit-write", "daemon-reload", "enable"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory(dir=self.root) as project_dir:
                project = Path(project_dir)
                (project / ".aida").mkdir()
                config = project / ".aida/config.toml"
                original = b'# tracked project config\n[schedule]\nmin_gap="30s"\n'
                config.write_bytes(original)
                config.chmod(0o640)
                args = self.service_args()
                args.state_dir = project / "state"
                args.state_dir.mkdir(mode=0o700)
                (args.state_dir / "watcher.py").write_text("original script")
                calls = []
                unit = Path(os.environ["XDG_CONFIG_HOME"]) / "systemd/user" / wake.service_name(project, args.thread)
                real_write = wake.write_unit
                def write(path, text):
                    if failure == "unit-write" and path == unit:
                        raise wake.Refusal("fixture unit failure")
                    return real_write(path, text)
                run = self.fake_service(calls, fail=failure)
                # Preserve an unrelated edit introduced at the failing stage.
                def command(argv, **kwargs):
                    if failure == "enable" and argv[:3] == ["systemctl", "--user", "enable"]:
                        with config.open("a") as out:
                            out.write('\n[[schedule.jobs]]\nname="intervening"\ncommand="cache verify"\nevery="1h"\n')
                    return run(argv, **kwargs)
                with patch.object(wake, "worktrees", return_value=[project]), patch.object(
                        wake, "write_unit", side_effect=write), patch.object(wake, "command", side_effect=command):
                    with self.assertRaisesRegex(wake.Refusal, failure):
                        wake.service(args, project)
                evidence, = args.state_dir.glob("install-recovery-*.json")
                record = json.loads(evidence.read_bytes())
                self.assertEqual(evidence.stat().st_mode & 0o777, 0o600)
                self.assertEqual(record["stage"], failure)
                saved = record["before"]["config"]
                self.assertEqual(base64.b64decode(saved["bytes_base64"]), original)
                self.assertEqual(saved["mode"], 0o640)
                self.assertEqual(saved["sha256"], hashlib.sha256(original).hexdigest())
                self.assertIsNone(record["before"]["registration"])
                self.assertIsNone(record["before"]["unit"])
                self.assertEqual(base64.b64decode(record["before"]["script"]["bytes_base64"]), b"original script")
                self.assertEqual(record["service_before"]["ActiveState"], "inactive")
                self.assertIn("observed", record)
                self.assertTrue((project / ".aida/codex-mail-health.json").exists())
                self.assertIn("codex-mail-wake --fail-on-findings", config.read_text())
                if failure == "enable":
                    self.assertIn('name="intervening"', config.read_text())
                # Repair/retry is exact-owned and preserves intervening jobs.
                with patch.object(wake, "worktrees", return_value=[project]), patch.object(
                        wake, "command", side_effect=self.fake_service(calls)):
                    wake.service(args, project)
                    after_retry = config.read_bytes()
                    wake.service(args, project)
                    self.assertEqual(config.read_bytes(), after_retry)
                    args.action = "uninstall"
                    wake.service(args, project)
                    wake.service(args, project)
                self.assertEqual(config.stat().st_mode & 0o777, 0o640)
                self.assertEqual(config.read_bytes(), original if failure != "enable" else original +
                    b'\n[[schedule.jobs]]\nname="intervening"\ncommand="cache verify"\nevery="1h"\n')

    def test_service_script_and_registration_failures_preserve_original_recovery(self):
        for failure in ("script-deploy", "health-registration"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory(dir=self.root) as directory:
                project = Path(directory)
                (project / ".aida").mkdir()
                config = project / ".aida/config.toml"
                config.write_text("# user config\n")
                args = self.service_args()
                args.state_dir = project / "state"
                args.state_dir.mkdir(mode=0o700)
                deployed = args.state_dir / "watcher.py"
                deployed.write_text("prior script")
                real_write, real_health = wake.write_unit, wake.own_health_job
                def write(path, text):
                    if failure == "script-deploy" and path == deployed:
                        raise wake.Refusal("fixture script failure")
                    return real_write(path, text)
                def health(*params, **kwargs):
                    if failure == "health-registration" and not kwargs.get("check_only"):
                        raise wake.Refusal("fixture registration failure")
                    return real_health(*params, **kwargs)
                with patch.object(wake, "worktrees", return_value=[project]), patch.object(
                        wake, "command", side_effect=self.fake_service([])), patch.object(
                        wake, "write_unit", side_effect=write), patch.object(wake, "own_health_job", side_effect=health):
                    with self.assertRaisesRegex(wake.Refusal, failure):
                        wake.service(args, project)
                bundle, = args.state_dir.glob("install-recovery-*.json")
                record = json.loads(bundle.read_bytes())
                self.assertEqual(record["stage"], failure)
                self.assertEqual(base64.b64decode(record["before"]["script"]["bytes_base64"]), b"prior script")
                self.assertEqual(config.read_text(), "# user config\n")
                self.assertFalse((project / ".aida/codex-mail-health.json").exists())

    def test_service_rechecks_intervening_unit_before_publication(self):
        args = self.service_args()
        (self.root / ".aida/config.toml").write_text("# user config\n")
        unit = Path(os.environ["XDG_CONFIG_HOME"]) / "systemd/user" / wake.service_name(self.root, args.thread)
        real_health = wake.own_health_job
        def health(*params, **kwargs):
            result = real_health(*params, **kwargs)
            if not kwargs.get("check_only"):
                unit.parent.mkdir(parents=True, exist_ok=True)
                unit.write_text("intervening user unit")
            return result
        with patch.object(wake, "worktrees", return_value=[self.root]), patch.object(
                wake, "command", side_effect=self.fake_service([])), patch.object(wake, "own_health_job", side_effect=health):
            with self.assertRaisesRegex(wake.Refusal, "unit-write"):
                wake.service(args, self.root)
        self.assertEqual(unit.read_text(), "intervening user unit")
        self.assertTrue((self.root / ".aida/codex-mail-health.json").exists())
        bundle, = args.state_dir.glob("install-recovery-*.json")
        self.assertIsNone(json.loads(bundle.read_bytes())["before"]["unit"])

    def test_service_absent_unit_and_nonzero_queries_use_typed_state_only(self):
        inactive = b"ActiveState=inactive\nUnitFileState=\nLoadState=not-found\n"
        for status in (0, 1, 4):
            with patch.object(wake, "command", return_value=(status, inactive)):
                self.assertEqual(wake.service_state("fixture.service")["ActiveState"], "inactive")
        for result in ((1, b""), (1, b"ActiveState=inactive\nUnitFileState=disabled\nLoadState=loaded\n"),
                       (1, b"ActiveState=active\nUnitFileState=\nLoadState=not-found\n")):
            with patch.object(wake, "command", return_value=result):
                with self.assertRaises(wake.Refusal):
                    wake.service_state("fixture.service")
        args = self.service_args("uninstall")
        (self.root / ".aida/config.toml").write_text("# unrelated config\n")
        calls = []
        def absent(argv, **kwargs):
            calls.append(argv)
            if argv[2] == "show":
                return (1, inactive)
            if argv[2] == "disable":
                self.fail("already absent/inactive unit must not require disable")
            return b""
        with patch.object(wake, "command", side_effect=absent):
            wake.service(args, self.root)
            wake.service(args, self.root)
        self.assertEqual((self.root / ".aida/config.toml").read_text(), "# unrelated config\n")

    def test_service_uninstall_requires_confirmed_stop_before_removing_monitoring(self):
        args = self.service_args()
        config = self.root / ".aida/config.toml"
        config.write_text("# user config\n")
        calls = []
        with patch.object(wake, "worktrees", return_value=[self.root]), patch.object(
                wake, "command", side_effect=self.fake_service(calls)):
            wake.service(args, self.root)
        before = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        args.action = "uninstall"
        for fail, active in (("disable", "inactive"), (None, "active")):
            with patch.object(wake, "command", side_effect=self.fake_service(calls, fail=fail, active=active)):
                with self.assertRaisesRegex(wake.Refusal, "monitor|retained"):
                    wake.service(args, self.root)
            self.assertEqual({str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}, before)
        with patch.object(wake, "command", side_effect=self.fake_service(calls)):
            wake.service(args, self.root)
        self.assertEqual(config.read_text(), "# user config\n")
        self.assertTrue(args.state_dir.exists())

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
        self.assertEqual(recovered["accepted_at"], {"new-id": 100})
        self.assertEqual(wake.health(recovered, now=150)["oldest_known_notification_age_seconds"], 50)
        self.assertIsNone(recovered["inflight"])
        self.assertFalse((self.root / ".aida").exists())

    def test_recover_dry_run_rejected_before_state_or_lock_mutation(self):
        for decision in ("--accepted", "--retry"):
            directory = self.root / decision.removeprefix("--")
            state = wake.State(directory)
            ledger = wake.initial(self.root, THREAD)
            ledger["pending"] = {"uncertain": ["/fixture"]}
            ledger["first_pending"] = 100
            ledger["inflight"] = {"ids": ["uncertain"], "submitted_at": 100}
            state.write(ledger)
            state.close()
            before = {p.name: p.read_bytes() for p in directory.iterdir()}
            argv = ["python3", str(MODULE), "recover", "--project", str(self.root),
                    "--thread", THREAD, "--state-dir", str(directory), decision, "--dry-run"]
            result = subprocess.run(argv, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"recover --dry-run is unsupported", result.stderr)
            self.assertEqual({p.name: p.read_bytes() for p in directory.iterdir()}, before)
            missing = self.root / (directory.name + "-missing")
            argv[argv.index(str(directory))] = str(missing)
            self.assertNotEqual(subprocess.run(argv, capture_output=True).returncode, 0)
            self.assertFalse(missing.exists())

    def test_status_missing_state_and_missing_lock_creates_nothing(self):
        directory = self.root / "absent-status"
        argv = ["python3", str(MODULE), "status", "--project", str(self.root),
                "--thread", THREAD, "--state-dir", str(directory)]
        result = json.loads(subprocess.check_output(argv))
        self.assertFalse(result["running"])
        self.assertFalse(directory.exists())
        directory.mkdir(mode=0o700)
        path = directory / "ledger.json"
        path.write_text(json.dumps(self.ledger))
        path.chmod(0o600)
        before = path.read_bytes()
        subprocess.check_output(argv)
        self.assertEqual(path.read_bytes(), before)
        self.assertEqual([p.name for p in directory.iterdir()], ["ledger.json"])

    def test_normal_retry_recovery_keeps_pending_and_does_not_claim_acceptance(self):
        directory = self.root / "retry-state"
        state = wake.State(directory)
        ledger = wake.initial(self.root, THREAD)
        ledger["pending"] = {"uncertain": ["/fixture"]}
        ledger["first_pending"] = 100
        ledger["inflight"] = {"ids": ["uncertain"], "submitted_at": 100}
        state.write(ledger)
        state.close()
        result = subprocess.run(["python3", str(MODULE), "recover", "--project", str(self.root),
                                "--thread", THREAD, "--state-dir", str(directory), "--retry"],
                                capture_output=True, check=True)
        self.assertTrue(json.loads(result.stdout)["recovered"])
        after = json.loads((directory / "ledger.json").read_text())
        self.assertEqual(after["pending"], ledger["pending"])
        self.assertEqual(after["accepted"], [])
        self.assertIsNone(after["inflight"])
        self.assertFalse(json.loads((directory / "health.json").read_text())["recovery_required"])

    def test_health_snapshot_contains_counts_only_and_tracks_uncertainty_recovery(self):
        self.ledger["accepted"] = ["legacy", "known"]
        self.ledger["accepted_at"] = {"known": 100}
        self.state.write(self.ledger)
        path = self.root / "state/health.json"
        snapshot = json.loads(path.read_text())
        self.assertEqual(snapshot["accepted_unacknowledged_count"], 2)
        self.assertEqual(snapshot["accepted_unknown_age_count"], 1)
        self.assertEqual(snapshot["oldest_known_notification_at"], 100)
        self.assertNotIn("known", snapshot.values())
        self.assertNotIn("legacy", snapshot.values())
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.ledger["pending"] = {"uncertain": ["/fixture"]}
        self.ledger["first_pending"] = 100
        self.ledger["inflight"] = {"ids": ["uncertain"], "submitted_at": 100}
        self.state.write(self.ledger)
        self.assertTrue(json.loads(path.read_text())["recovery_required"])
        self.ledger["inflight"] = None
        self.state.write(self.ledger)
        self.assertFalse(json.loads(path.read_text())["recovery_required"])

    def health_args(self):
        return SimpleNamespace(thread=THREAD, aida="fixture-aida", state_dir=self.root / "state",
                               health_max_age=3600, interval=30)

    def test_owned_health_job_install_idempotent_and_remove_preserves_user_jobs(self):
        directory = self.root / ".aida"
        directory.mkdir()
        original = '[schedule]\nmin_gap="30s"\n[[schedule.jobs]]\nname="user-job"\ncommand="cache verify"\nevery="1h"\nenabled=true\n'
        config = directory / "config.toml"
        config.write_text(original)
        args = self.health_args()
        wake.own_health_job(args, self.root)
        first = config.read_bytes()
        registration = json.loads((directory / "codex-mail-health.json").read_text())
        self.assertEqual(registration["thread"], THREAD)
        self.assertEqual(registration["max_age_seconds"], 3600)
        self.assertIn(b"doctor check codex-mail-wake --fail-on-findings", first)
        self.assertIn(("CronJobFailed:" + registration["job"]).encode(), first)
        wake.own_health_job(args, self.root)
        self.assertEqual(config.read_bytes(), first)
        wake.own_health_job(args, self.root, remove=True)
        self.assertEqual(config.read_text(), original)
        self.assertFalse((directory / "codex-mail-health.json").exists())
        self.assertTrue((directory / "codex-mail-health.lock").exists())

    def test_health_job_refuses_user_collision_edited_block_and_other_target(self):
        directory = self.root / ".aida"
        directory.mkdir()
        args = self.health_args()
        name = wake.health_registration(args, self.root)["job"]
        config = directory / "config.toml"
        config.write_text('[[schedule.jobs]]\nname=' + json.dumps(name) + '\nprompt="user-owned"\nevery="1h"\n')
        before = config.read_bytes()
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root)
        self.assertEqual(config.read_bytes(), before)
        self.assertFalse((directory / "codex-mail-health.json").exists())
        config.write_text("")
        wake.own_health_job(args, self.root)
        config.write_text(config.read_text().replace('every = "15m"', 'every = "30m"'))
        before = config.read_bytes()
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root, remove=True)
        self.assertEqual(config.read_bytes(), before)
        other = self.health_args()
        other.thread = str(uuid.uuid4())
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(other, self.root)
        self.assertEqual(config.read_bytes(), before)

    def test_health_job_refuses_duplicate_owned_names_even_with_exact_owned_block(self):
        for suffix in ("", "-route"):
            project = self.root / ("duplicate" + suffix)
            (project / ".aida").mkdir(parents=True)
            args = self.health_args()
            wake.own_health_job(args, project)
            config = project / ".aida/config.toml"
            name = wake.health_registration(args, project)["job"] + suffix
            config.write_text(config.read_text() + '\n[[schedule.jobs]]\nname=' + json.dumps(name) + '\nprompt="user-owned"\nevery="1h"\n')
            before = config.read_bytes()
            registration = (project / ".aida/codex-mail-health.json").read_bytes()
            for remove in (False, True):
                with self.assertRaises(wake.Refusal):
                    wake.own_health_job(args, project, remove=remove)
                self.assertEqual(config.read_bytes(), before)
                self.assertEqual((project / ".aida/codex-mail-health.json").read_bytes(), registration)

    def test_global_health_names_refused_in_both_shapes_and_lists_without_local_changes(self):
        global_dir = Path.home() / ".aida"
        global_dir.mkdir()
        global_config = global_dir / "schedule.toml"
        for wrapped in (False, True):
            for collection in ("jobs", "tasks"):
                for suffix in ("", "-route"):
                    for installed in (False, True):
                        with self.subTest(wrapped=wrapped, collection=collection,
                                          suffix=suffix, installed=installed):
                            project = Path(tempfile.mkdtemp(dir=self.root))
                            args = self.health_args()
                            directory = project / ".aida"
                            directory.mkdir()
                            config = directory / "config.toml"
                            config.write_text("# preserve user project config\n")
                            global_config.unlink(missing_ok=True)
                            if installed:
                                wake.own_health_job(args, project)
                            registration = directory / "codex-mail-health.json"
                            before = config.read_bytes()
                            prior = registration.read_bytes() if installed else None
                            name = wake.health_registration(args, project)["job"] + suffix
                            table = "schedule." if wrapped else ""
                            body = ('[[' + table + collection + ']]\nname=' + json.dumps(name)
                                    + '\ncommand="cache verify"\nevery="1h"\nenabled=false\n')
                            global_config.write_text(body)
                            for remove in (False, True):
                                with self.assertRaises(wake.Refusal):
                                    wake.own_health_job(args, project, remove=remove)
                                self.assertEqual(config.read_bytes(), before)
                                self.assertEqual(registration.read_bytes() if registration.exists() else None, prior)
                                self.assertEqual(global_config.read_text(), body)

    def test_global_health_namespace_honors_aida_home_and_preserves_unrelated_jobs(self):
        args = self.health_args()
        alternate = self.root / "aida-home"
        (alternate / ".aida").mkdir(parents=True)
        config = alternate / ".aida/schedule.toml"
        name = wake.health_registration(args, self.root)["job"]
        config.write_text('[[jobs]]\nname=' + json.dumps(name) + '\n')
        with patch.dict(os.environ, {"AIDA_HOME": str(alternate)}):
            with self.assertRaises(wake.Refusal):
                wake.own_health_job(args, self.root)
            self.assertFalse((self.root / ".aida/config.toml").exists())
            self.assertFalse((self.root / ".aida/codex-mail-health.json").exists())
            config.write_text('[[tasks]]\nname="unrelated-global"\ncommand="cache verify"\nevery="1h"\n')
            before = config.read_bytes()
            wake.own_health_job(args, self.root)
            wake.own_health_job(args, self.root)
            wake.own_health_job(args, self.root, remove=True)
            self.assertEqual(config.read_bytes(), before)
        # Missing global registry remains a supported, healthy installation.
        wake.own_health_job(args, self.root)

    def test_global_health_registry_unknown_or_unsafe_refuses_before_local_registration(self):
        directory = Path.home() / ".aida"
        directory.mkdir()
        config = directory / "schedule.toml"
        args = self.health_args()
        malformed = ('[schedule]\njobs="wrong"\n', '[schedule]\ntasks=[1]\n',
                     '[[jobs]]\nname=7\n', '[[tasks]]\n',
                     'schedule="wrong"\n', 'not toml')
        for body in malformed:
            with self.subTest(body=body):
                config.write_text(body)
                before = config.read_bytes()
                with self.assertRaises(wake.Refusal):
                    wake.own_health_job(args, self.root)
                self.assertEqual(config.read_bytes(), before)
                self.assertFalse((self.root / ".aida/config.toml").exists())
                self.assertFalse((self.root / ".aida/codex-mail-health.json").exists())
        config.write_bytes(b"\xff")
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root)
        config.write_bytes(b" " * (wake.LIMIT + 1))
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root)
        config.unlink()
        target = self.root / "private-global"
        target.write_text("")
        config.symlink_to(target)
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root)
        config.unlink()
        os.link(target, config)
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root)
        config.unlink()
        os.mkfifo(config)
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root)
        config.unlink()
        config.write_text("[schedule]\n")
        config.chmod(0o666)
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(args, self.root)
        config.chmod(0o600)
        real_open = wake.os.open
        def deny_registry(path, *args, **kwargs):
            if path == "schedule.toml":
                raise PermissionError("fixture unreadable")
            return real_open(path, *args, **kwargs)
        with patch.object(wake.os, "open", side_effect=deny_registry):
            with self.assertRaises(wake.Refusal):
                wake.own_health_job(args, self.root)
        self.assertFalse((self.root / ".aida/config.toml").exists())
        self.assertFalse((self.root / ".aida/codex-mail-health.json").exists())
        wake.own_health_job(args, self.root)

    def test_native_global_validation_failure_cannot_publish_local_registration(self):
        directory = Path.home() / ".aida"
        directory.mkdir()
        config = directory / "schedule.toml"
        config.write_text('[[jobs]]\nname="other"\ncommand=7\nevery="1h"\n')
        before = config.read_bytes()
        self.native_validator.side_effect = wake.Refusal("native registry error")
        with self.assertRaises(wake.Refusal):
            wake.own_health_job(self.health_args(), self.root)
        self.assertFalse((self.root / ".aida/config.toml").exists())
        self.assertFalse((self.root / ".aida/codex-mail-health.json").exists())
        self.assertEqual(config.read_bytes(), before)
        self.native_validator.assert_called_once_with(before, "fixture-aida")

    def test_native_global_validation_uses_only_disposable_snapshot(self):
        # Reach the real helper while the other ownership fixtures mock this
        # subprocess boundary. No system binary, live project or HOME is used.
        # Resolve the function from a separate module instance, without patches.
        spec = importlib.util.spec_from_file_location("native_snapshot_fixture", MODULE)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for wrapped in (False, True):
            prefix = "schedule." if wrapped else ""
            body = ('[[' + prefix + 'jobs]]\nname="other"\ncommand="cache verify"\nevery="1h"\n').encode()
            def inspect(argv, **kwargs):
                root = kwargs["cwd"]
                self.assertNotEqual(root, self.root)
                snapshot = root / "home/.aida/schedule.toml"
                self.assertEqual(snapshot.read_bytes(), body)
                self.assertNotEqual(kwargs["env"]["HOME"], str(Path.home()))
                self.assertNotIn("AIDA_HOME", kwargs["env"])
                self.assertEqual(argv, ["fixture-aida", "schedule", "list", "--json"])
                return b"[]"
            with patch.object(module, "command", side_effect=inspect) as command:
                module.validate_global_scheduler_snapshot(body, "fixture-aida")
                root = command.call_args.kwargs["cwd"]
            self.assertFalse(root.exists())
            with patch.object(module, "command", side_effect=module.Refusal("fixture failure")):
                with self.assertRaises(module.Refusal):
                    module.validate_global_scheduler_snapshot(body, "fixture-aida")

    @unittest.skipUnless(os.environ.get("TASK2347_AIDA_BIN"), "requires an explicit native candidate")
    def test_native_candidate_rejects_typed_and_semantic_global_errors(self):
        spec = importlib.util.spec_from_file_location("native_candidate_fixture", MODULE)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        directory = Path.home() / ".aida"
        directory.mkdir()
        config = directory / "schedule.toml"
        cases = ('command=7\nevery="1h"',
                 'command="cache verify"\nevery="nonsense"',
                 'prompt="inspect"\non=["NotAnEvent"]',
                 'prompt="inspect"\nwhen="mail.bad nope"',
                 'command="cache verify"\nevery="1h"\nquiet_hours="impossible"')
        for wrapped in (False, True):
            for index, fields in enumerate(cases + ('command="cache verify"\nevery="1h"',)):
                with self.subTest(wrapped=wrapped, fields=fields):
                    project = Path(tempfile.mkdtemp(dir=self.root))
                    args = self.health_args()
                    args.aida = os.environ["TASK2347_AIDA_BIN"]
                    body = ('[[' + ('schedule.' if wrapped else '') + 'jobs]]\n'
                            'name="unrelated"\n' + fields + '\n')
                    config.write_text(body)
                    if index < len(cases):
                        with self.assertRaises(module.Refusal):
                            module.own_health_job(args, project)
                    else:
                        module.own_health_job(args, project)
                    self.assertEqual((project / ".aida/config.toml").exists(), index == len(cases))
                    self.assertEqual((project / ".aida/codex-mail-health.json").exists(), index == len(cases))
                    self.assertEqual(config.read_text(), body)

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
        fake_aida.write_text("#!/usr/bin/env python3\nimport json\nprint(json.dumps({'version':1,'messages':[{'id':'fixture-message'}],'acknowledged_ids':[]}))\n")
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
