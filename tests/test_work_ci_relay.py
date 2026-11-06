#!/usr/bin/env python3
"""Local bare repos + fake GitLab API. trace:STORY-2480 | ai:codex

Run AIDA_TEST_BIN=/absolute/path/to/aida python3 tests/test_work_ci_relay.py.
No production configuration, credentials, runners, store, or network used.
"""
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("AIDA_TEST_BIN", str(ROOT / "target/debug/aida"))
HOOK = ROOT / "scripts/ci-relay/pre-receive.py"


class Api(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, value, status=200):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        state = self.server.fixture
        path = urlsplit(self.path).path
        if path.startswith("/home/"):
            assert self.headers["PRIVATE-TOKEN"] == "home-only"
            self.reply(state.statuses)
        else:
            assert self.headers["PRIVATE-TOKEN"] == "work-only"
            if path.endswith("/pipelines"):
                self.reply([{"id": state.pipeline, "sha": state.sha, "ref": f"ci/{state.attempt}"}])
            elif path.endswith("/jobs"):
                self.reply([{"name": "build", "status": state.result, "duration": 1.25}])
            elif path.endswith("/test_report"):
                self.reply({"test_suites": [{"test_cases": [{"name": "checks", "status": "failed"}]}] if state.result == "failed" else []})
            else:
                self.reply({"id": state.pipeline, "sha": state.sha, "ref": f"ci/{state.attempt}", "status": state.result})

    def do_POST(self):
        state = self.server.fixture
        assert self.path.startswith("/home/")
        assert self.headers["PRIVATE-TOKEN"] == "home-only"
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        body["status"] = body["state"]
        state.statuses.append(body)
        self.reply(body)


class Fixture:
    def __init__(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aida-ci-test-")
        self.root = Path(self.temp.name)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("AIDA_", "GIT_", "REQ_"))}
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_AUTHOR_NAME="test", GIT_AUTHOR_EMAIL="test@invalid", GIT_COMMITTER_NAME="test", GIT_COMMITTER_EMAIL="test@invalid", WORK_CI_TOKEN="work-only", HOME_CI_TOKEN="home-only")
        self.source = self.root / "source"
        self.source.mkdir()
        self.git(self.source, "init", "-q", "-b", "main")
        (self.source / ".gitlab-ci.yml").write_text("build:\n  script: cargo test\n")
        (self.source / "lib.rs").write_text("#[test]\nfn checks() {}\n")
        self.git(self.source, "add", ".")
        self.git(self.source, "commit", "-qm", "source")
        self.sha = self.git(self.source, "rev-parse", "HEAD").stdout.strip()
        self.attempt, self.pipeline, self.result, self.statuses = 1, 41, "success", []
        self.home, self.work = self.root / "home.git", self.root / "work.git"
        for repo in (self.home, self.work):
            self.git(self.root, "init", "--bare", "-q", str(repo))
            self.git(self.source, "push", "-q", str(repo), f"{self.sha}:refs/aida/ci/1")
        self.git(self.source, "push", "-q", str(self.home), "HEAD:refs/heads/main", "HEAD:refs/heads/aida-store")
        for repo in (self.home, self.work):
            # In production each URL/key has a trusted server-side identity.
            # Local transport inherits env; wrappers model those distinct keys.
            hook = repo / "hooks/pre-receive"
            mapping = 'if [ "$AIDA_CI_PRINCIPAL" = mirror ]; then export AIDA_CI_PRINCIPAL=return; fi\n' if repo == self.home else ""
            hook.write_text(f'#!/bin/sh\n{mapping}exec python3 "{HOOK}"\n')
            hook.chmod(0o700)
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Api)
        self.server.fixture = self
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.home_clone = self.clone("home-clone", "home")
        self.work_clone = self.clone("work-clone", "work")

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.temp.cleanup()

    def git(self, cwd, *args, ok=True, principal=None):
        env = dict(self.env)
        if principal:
            env["AIDA_CI_PRINCIPAL"] = principal
        result = subprocess.run(["git", *args], cwd=cwd, env=env, text=True, capture_output=True, timeout=40)
        if ok and result.returncode:
            raise AssertionError(result.stderr)
        return result

    def cli(self, clone, *args, ok=True, principal="return"):
        home = clone / "isolated-home"
        home.mkdir(exist_ok=True)
        env = dict(self.env, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_CACHE_HOME=str(home / "cache"), AIDA_CI_PRINCIPAL=principal, AIDA_TELEMETRY="0")
        result = subprocess.run([BINARY, "intent", *args], cwd=clone, env=env, text=True, capture_output=True, timeout=50)
        if ok and result.returncode:
            raise AssertionError(result.stderr + result.stdout)
        return result

    def clone(self, name, role):
        clone = self.root / name
        clone.mkdir()
        self.git(clone, "init", "-q", "-b", "main")
        self.cli(clone, "enroll", "--remote", str(self.home), "--reviewed")
        port = self.server.server_port
        profile = {"version": 1, "role": role, "home_remote": str(self.home), "work_remote": str(self.work),
                   "work_api": {"url": f"http://127.0.0.1:{port}/work", "project": "1", "token_env": "WORK_CI_TOKEN", "forge": "gitlab"},
                   "home_api": {"url": f"http://127.0.0.1:{port}/home", "project": "2", "token_env": "HOME_CI_TOKEN", "forge": "gitlab"} if role == "home" else None,
                   "timeout_seconds": 600, "max_polls": 8}
        file = clone / "profile.json"
        file.write_text(json.dumps(profile))
        self.cli(clone, "ci", "enroll", "--file", str(file), "--reviewed")
        return clone

    def private(self, clone):
        return clone / ".git/aida-intake"

    def receipt(self, n=1):
        path = f"ci-results/{self.sha}/{n}.json"
        value = self.git(self.home, "show", f"refs/heads/aida-intent:{path}").stdout
        return json.loads(value)

    def poll(self):
        return self.cli(self.work_clone, "ci", "poll", principal="mirror")

    def inject(self, value):
        """Build hostile intake bytes with otherwise valid immutable metadata."""
        repo = self.private(self.work_clone) / "worktree"
        path = f"ci-results/{value['sha']}/{value['attempt']}.json"
        file = repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(json.dumps(value))
        self.git(repo, "add", path)
        env = dict(self.env, GIT_AUTHOR_NAME="AIDA Intake", GIT_AUTHOR_EMAIL="intake@invalid", GIT_COMMITTER_NAME="AIDA Intake", GIT_COMMITTER_EMAIL="intake@invalid")
        result = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "Record reviewed intake data"], cwd=repo, env=env, capture_output=True, timeout=20)
        assert result.returncode == 0, result.stderr
        self.git(repo, "push", "-q", str(self.home), "HEAD:refs/heads/aida-intent", principal="return")


class WorkCiTests(unittest.TestCase):
    def setUp(self):
        self.f = Fixture()
        self.addCleanup(self.f.close)

    def test_pass_fail_restart_and_observation_coexistence(self):
        f = self.f
        before = f.git(f.home, "show-ref", "refs/heads/main", "refs/heads/aida-store").stdout
        self.assertIn("published", f.poll().stdout)
        receipt = f.receipt()
        self.assertEqual(receipt["status"], "success")
        self.assertEqual(receipt["jobs"][0]["duration_seconds"], 2)
        self.assertEqual(set(receipt), {"version", "kind", "sha", "attempt", "status", "jobs", "failing_tests", "digest"})
        tip = f.git(f.home, "rev-parse", "refs/heads/aida-intent").stdout
        # Simulate a crash after accepted push but before journal completion.
        journal = f.private(f.work_clone) / "ci-attempt-1.json"
        state = json.loads(journal.read_text())
        state["done"] = False
        journal.write_text(json.dumps(state))
        f.poll()
        self.assertEqual(tip, f.git(f.home, "rev-parse", "refs/heads/aida-intent").stdout)
        f.cli(f.home_clone, "ci", "process")
        f.cli(f.home_clone, "ci", "process")
        self.assertEqual(len(f.statuses), 1)
        self.assertEqual(f.statuses[0]["name"], "aida/work-ci")
        # Fresh home clone recovers admission and processing from immutable refs
        # plus native status evidence; local relay journals grant no authority.
        fresh = f.clone("fresh-home", "home")
        f.cli(fresh, "ci", "process")
        self.assertEqual(len(f.statuses), 1)
        # Ordinary observation publication still validates retained CI history.
        body = fresh / "body"
        body.write_text("A safe observation")
        note = f.cli(fresh, "note", "--category", "observation", "--title", "Example", "--file", str(body)).stdout
        file = fresh / "note.json"
        file.write_text(note)
        f.cli(fresh, "publish", "--file", str(file), "--reviewed")
        # A new home-assigned attempt for the same SHA supersedes the old result.
        f.cli(f.home_clone, "ci", "request", "--attempt", "2", "--sha", f.sha, principal="request")
        f.cli(f.home_clone, "ci", "process")
        self.assertEqual([s["state"] for s in f.statuses], ["success", "pending"])
        f.attempt, f.pipeline, f.result = 2, 42, "failed"
        f.poll()
        f.cli(f.home_clone, "ci", "process")
        self.assertEqual(f.receipt(2)["failing_tests"], ["checks"])
        self.assertEqual([s["state"] for s in f.statuses], ["success", "pending", "failed"])
        self.assertEqual(before, f.git(f.home, "show-ref", "refs/heads/main", "refs/heads/aida-store").stdout)
        self.assertFalse((fresh / ".aida").exists())

    def test_server_ref_restrictions_independent_of_client(self):
        f = self.f
        f.git(f.source, "commit", "--allow-empty", "-qm", "next")
        new = f.git(f.source, "rev-parse", "HEAD").stdout.strip()
        for ref in ("refs/heads/evil", "refs/tags/evil", "refs/heads/main", "refs/aida/ci/9", "refs/heads/aida-intent"):
            self.assertNotEqual(f.git(f.source, "push", str(f.home), f"{new}:{ref}", ok=False, principal="return").returncode, 0, ref)
        for target in (f"{new}:refs/aida/ci/1", ":refs/aida/ci/1", f"{new}:refs/aida/ci/01"):
            self.assertNotEqual(f.git(f.source, "push", str(f.work), target, ok=False, principal="request").returncode, 0)
        self.assertEqual(f.git(f.work, "rev-parse", "refs/aida/ci/1").stdout.strip(), f.sha)

    def test_tampered_receipt_and_unknown_fields_fail_closed(self):
        f = self.f
        f.poll()
        bad = f.receipt()
        bad["attempt"] = 2
        bad["jobs"][0]["name"] = "not-in-home"
        bad["digest"] = hashlib.sha256(json.dumps({k: v for k, v in bad.items() if k != "digest"}, separators=(",", ":")).encode()).hexdigest()
        # Attempt 2 is admitted, but its recomputed hash cannot authorize names.
        f.cli(f.home_clone, "ci", "request", "--attempt", "2", "--sha", f.sha, principal="request")
        f.inject(bad)
        refused = f.cli(f.home_clone, "pull", ok=False)
        self.assertNotEqual(refused.returncode, 0)
        self.assertNotIn("not-in-home", refused.stderr)
        self.assertEqual([s["state"] for s in f.statuses], ["pending"])
        bad["attempt"] = 3
        bad["unknown"] = "remote-secret"
        f.cli(f.home_clone, "ci", "request", "--attempt", "3", "--sha", f.sha, principal="request")
        f.inject(bad)
        refused = f.cli(f.home_clone, "ci", "process", ok=False)
        self.assertNotEqual(refused.returncode, 0)
        self.assertNotIn("remote-secret", refused.stderr)
        self.assertEqual([s["state"] for s in f.statuses], ["pending", "pending"])

    def test_missing_admission_and_work_selected_attempt_refuse(self):
        f = self.f
        f.poll()
        bad = f.receipt()
        bad["attempt"] = 9
        bad["digest"] = hashlib.sha256(json.dumps({k: v for k, v in bad.items() if k != "digest"}, separators=(",", ":")).encode()).hexdigest()
        f.inject(bad)
        self.assertNotEqual(f.cli(f.home_clone, "pull", ok=False).returncode, 0)
        # Administrative corruption bypasses the server policy only in this fixture.
        f.git(f.home, "update-ref", "-d", "refs/aida/ci/1")
        self.assertNotEqual(f.cli(f.home_clone, "ci", "process", ok=False).returncode, 0)
        self.assertFalse(f.statuses)

    def test_uncertain_accepted_native_status_reconciles(self):
        f = self.f
        f.poll()
        receipt = f.receipt()
        f.cli(f.home_clone, "ci", "process")
        private = f.private(f.home_clone)
        (private / "ci-status-1.json").unlink()
        pending = private / "ci-status-pending-1.json"
        pending.write_text(receipt["digest"])
        pending.chmod(0o600)
        f.cli(f.home_clone, "ci", "process")
        self.assertEqual(len(f.statuses), 1)
        self.assertFalse(pending.exists())
        # A journal without native evidence is uncertain, never blindly replayed.
        (private / "ci-status-1.json").unlink()
        pending.write_text(receipt["digest"])
        pending.chmod(0o600)
        f.statuses.clear()
        self.assertNotEqual(f.cli(f.home_clone, "ci", "process", ok=False).returncode, 0)
        self.assertFalse(f.statuses)

    def test_deadline_and_uncertain_status_recovery(self):
        f = self.f
        f.result = "running"
        f.poll()
        journal = f.private(f.work_clone) / "ci-attempt-1.json"
        j = json.loads(journal.read_text())
        original = j["deadline"]
        f.poll()
        self.assertEqual(json.loads(journal.read_text())["deadline"], original)
        j["deadline"] = 1
        journal.write_text(json.dumps(j))
        self.assertIn("expired", f.poll().stdout)
        self.assertFalse(f.statuses)
        self.assertNotEqual(f.git(f.home, "rev-parse", "--verify", "refs/heads/aida-intent", ok=False).returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
