#!/usr/bin/env python3
"""Local bare repos + fake GitLab API. trace:STORY-2480 | ai:codex

Run AIDA_TEST_BIN=/absolute/path/to/aida python3 tests/test_work_ci_relay.py.
No production configuration, credentials, runners, store, or network used.
"""
import hashlib
import http.server
import json
import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import unittest
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
BINARY = str(Path(os.environ.get("AIDA_TEST_BIN", str(ROOT / "target/debug/aida"))).resolve())
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
                self.reply([{"name": name, "status": state.result, "duration": 1.25} for name in state.job_names])
            elif path.endswith("/test_report"):
                self.reply({"test_suites": [{"test_cases": [{"name": "module::checks", "status": "failed"}]}] if state.result == "failed" else []})
            else:
                self.reply({"id": state.pipeline, "sha": state.sha, "ref": f"ci/{state.attempt}", "status": state.result})

    def do_POST(self):
        state = self.server.fixture
        assert self.path.startswith("/home/")
        assert self.headers["PRIVATE-TOKEN"] == "home-only"
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        body["status"] = body["state"]
        body["sha"] = urlsplit(self.path).path.rsplit("/", 1)[-1]
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
        self.job_names = ["build"]
        self.cli_timeout = 50
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
        result = subprocess.run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", *args], cwd=cwd, env=env, text=True, capture_output=True, timeout=40)
        if ok and result.returncode:
            raise AssertionError(result.stderr)
        return result

    def cli(self, clone, *args, ok=True, principal="return"):
        home = clone / "isolated-home"
        home.mkdir(exist_ok=True)
        env = dict(self.env, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_CACHE_HOME=str(home / "cache"), AIDA_CI_PRINCIPAL=principal, AIDA_TELEMETRY="0")
        result = subprocess.run([BINARY, "intent", *args], cwd=clone, env=env, text=True, capture_output=True, timeout=self.cli_timeout, umask=0o022)
        if ok and result.returncode:
            raise AssertionError(result.stderr + result.stdout)
        return result

    def clone(self, name, role, ci=True):
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
        if ci:
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
        result = subprocess.run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "Record reviewed intake data"], cwd=repo, env=env, capture_output=True, timeout=20)
        assert result.returncode == 0, result.stderr
        self.git(repo, "push", "-q", str(self.home), "HEAD:refs/heads/aida-intent", principal="return")


class WorkCiTests(unittest.TestCase):
    def setUp(self):
        self.f = Fixture()
        self.addCleanup(self.f.close)

    # trace:STORY-2606 | ai:codex
    def test_upstream_adapter_lane_receipts_use_the_committed_inventory(self):
        f = self.f
        f.poll()
        f.cli(f.home_clone, "ci", "process")
        for include in (".gitlab-ci.yml", ".gitlab/ci/gdma.yml",
                        "aida-core/templates/gitlab-ci-merge-hold-gate.yml"):
            path = f.source / include
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((ROOT / include).read_bytes())
        f.git(f.source, "add", ".")
        f.git(f.source, "commit", "-qm", "Upstream adapter fixture")
        f.sha = f.git(f.source, "rev-parse", "HEAD").stdout.strip()
        f.attempt, f.pipeline = 2, 42
        f.job_names = sorted(["bwrap-confinement", "test", "verify"] + [
            f"gdma-ci: [{lane}]" for lane in
            ("lib-0", "lib-1", "cli-0", "cli-1", "rest", "gates", "contracts")])
        f.git(f.source, "push", "-q", str(f.home), f"{f.sha}:refs/aida/ci/2", principal="request")
        f.cli(f.home_clone, "ci", "request", "--attempt", "2", "--sha", f.sha, principal="request")
        self.assertIn("published", f.poll().stdout)
        self.assertEqual([job["name"] for job in f.receipt(2)["jobs"]], f.job_names)
        f.cli(f.home_clone, "ci", "process")
        rows = json.loads(f.cli(f.home_clone, "ci", "read", "--json").stdout)
        self.assertEqual(sum(row["verification"] == "verified" for row in rows), 2)

    # trace:TASK-3232 | ai:codex
    @unittest.skipUnless(os.environ.get("AIDA_CI_GATE_EVIDENCE"),
                         "runs through the Rust native PR/train gate contract")
    def test_aida_sized_multi_sha_templates_and_native_gate_evidence(self):
        f = self.f
        f.cli_timeout = 120
        f.poll()
        f.cli(f.home_clone, "ci", "process")
        (f.source / ".gitlab-ci.yml").write_bytes((ROOT / ".gitlab-ci.yml").read_bytes())
        # trace:STORY-2606 | ai:codex
        for include in ("aida-core/templates/gitlab-ci-merge-hold-gate.yml", ".gitlab/ci/gdma.yml"):
            (f.source / include).parent.mkdir(parents=True, exist_ok=True)
            (f.source / include).write_bytes((ROOT / include).read_bytes())
        # Real AIDA Rust sources, padded above 48 MiB. Three distinct source
        # commits exercise independent pinned inventories, then a cold validator.
        paths = f.git(ROOT, "ls-files", "*.rs").stdout.splitlines()
        total = 0
        for path in paths:
            data = (ROOT / path).read_bytes()
            target = f.source / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            total += len(data)
        padding = f.source / "padding"
        padding.mkdir()
        while total < 48 * 1024 * 1024:
            data = b"//" + b"x" * (512 * 1024 - 3) + b"\n"
            (padding / f"source-{total}.rs").write_bytes(data)
            total += len(data)
        f.job_names = sorted(["bwrap-confinement", "test", "verify"] + [
            f"gdma-ci: [{lane}]" for lane in ("lib-0", "lib-1", "cli-0", "cli-1", "rest", "gates", "contracts")])
        evidence, shas, durations = [], [], []
        for attempt, result in [(2, "success"), (3, "failed"), (4, "success")]:
            (f.source / "lib.rs").write_text(f"#[test] fn checks() {{}} // revision {attempt}\n")
            f.git(f.source, "add", ".")
            f.git(f.source, "commit", "-qm", f"AIDA-sized revision {attempt}")
            f.sha = f.git(f.source, "rev-parse", "HEAD").stdout.strip()
            # Bootstrap the retained home request before the request command fetches it.
            f.git(f.source, "push", "-q", str(f.home), f"{f.sha}:refs/aida/ci/{attempt}", principal="request")
            f.attempt, f.pipeline, f.result = attempt, 40 + attempt, result
            f.cli(f.home_clone, "ci", "request", "--attempt", str(attempt), "--sha", f.sha, principal="request")
            evidence.append({"sha": f.sha, "statuses": [dict(f.statuses[-1])]})
            start = time.monotonic()
            self.assertIn("published", f.poll().stdout)
            self.assertEqual(f.git(f.work, "rev-parse", f"refs/heads/ci/{attempt}").stdout.strip(), f.sha)
            f.cli(f.home_clone, "ci", "process")
            durations.append(time.monotonic() - start)
            receipt = f.receipt(attempt)
            self.assertEqual([job["name"] for job in receipt["jobs"]], f.job_names)
            self.assertEqual(receipt["failing_tests"], ["checks"] if result == "failed" else [])
            evidence[-1]["statuses"].append(dict(f.statuses[-1]))
            shas.append(f.sha)
        self.assertEqual(len(set(shas)), 3)
        fresh = f.clone("aida-sized-fresh-home", "home")
        rows = json.loads(f.cli(fresh, "ci", "read", "--json").stdout)
        self.assertEqual(sum(row["verification"] == "verified" for row in rows), 4)
        # Rust gate-contract test consumes actual posted native status bodies.
        Path(os.environ["AIDA_CI_GATE_EVIDENCE"]).write_text(json.dumps(evidence))
        print(f"48 MiB / three SHA relay validation seconds: {durations}", flush=True)

    # trace:STORY-b6b1278890774ef993d2ebc327862470-408.acc0579e | ai:codex
    # trace:STORY-b6b1278890774ef993d2ebc327862470-408.ac0a4b5e | ai:codex
    # trace:STORY-b6b1278890774ef993d2ebc327862470-408.acadec5a | ai:codex
    # trace:STORY-b6b1278890774ef993d2ebc327862470-408.AC6 | ai:codex
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

    # trace:STORY-b6b1278890774ef993d2ebc327862470-408.acd204af | ai:codex
    def test_server_ref_restrictions_independent_of_client(self):
        f = self.f
        f.git(f.source, "commit", "--allow-empty", "-qm", "next")
        new = f.git(f.source, "rev-parse", "HEAD").stdout.strip()
        for ref in ("refs/heads/evil", "refs/tags/evil", "refs/heads/main", "refs/aida/ci/9", "refs/heads/aida-intent"):
            self.assertNotEqual(f.git(f.source, "push", str(f.home), f"{new}:{ref}", ok=False, principal="return").returncode, 0, ref)
        for target in (f"{new}:refs/aida/ci/1", ":refs/aida/ci/1", f"{new}:refs/aida/ci/01"):
            self.assertNotEqual(f.git(f.source, "push", str(f.work), target, ok=False, principal="request").returncode, 0)
        self.assertEqual(f.git(f.work, "rev-parse", "refs/aida/ci/1").stdout.strip(), f.sha)

    # trace:STORY-b6b1278890774ef993d2ebc327862470-408.ac0a391b | ai:codex
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
        self.assertEqual(f.receipt()["status"], "canceled")
        f.cli(f.home_clone, "ci", "process")
        self.assertEqual([s["state"] for s in f.statuses], ["failed"])

    def test_observation_only_clone_explains_ci_enrollment(self):
        f = self.f
        observer = f.clone("observer", "home", ci=False)
        f.cli(observer, "pull")  # no CI history yet
        f.poll()
        result = f.cli(observer, "pull", ok=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Intake contains CI results", result.stderr)
        self.assertIn("aida intent ci enroll", result.stderr)
        self.assertFalse((f.private(observer) / "validated-tip").exists())

    def test_several_shas_reuse_accepted_history_without_source_fetches(self):
        f = self.f
        for attempt in range(1, 5):
            if attempt > 1:
                f.git(f.source, "commit", "--allow-empty", "-qm", "next source")
                f.sha = f.git(f.source, "rev-parse", "HEAD").stdout.strip()
                f.git(f.source, "-c", "core.hooksPath=/dev/null", "push", "-q", str(f.home), f"{f.sha}:refs/aida/ci/{attempt}", principal="request")
                f.cli(f.home_clone, "ci", "request", "--attempt", str(attempt), "--sha", f.sha, principal="request")
            f.attempt, f.pipeline = attempt, 40 + attempt
            f.poll()
            f.cli(f.home_clone, "ci", "process")
        # Count remote fetches by exact SHA through a transparent Git wrapper.
        # Intake transport deliberately clears GIT_*; PATH is the test seam.
        shim = f.root / "shim"
        shim.mkdir()
        log = f.root / "fetches"
        real_git = shutil.which("git")
        wrapper = shim / "git"
        wrapper.write_text(f"#!/usr/bin/python3\nimport os,sys\nif 'fetch' in sys.argv: open({str(log)!r},'a').write(' '.join(sys.argv[1:])+'\\n')\nos.execv({real_git!r},[{real_git!r},*sys.argv[1:]])\n")
        wrapper.chmod(0o700)
        f.env["PATH"] = str(shim) + os.pathsep + f.env["PATH"]
        for _ in range(2):
            f.cli(f.home_clone, "pull")
            f.cli(f.home_clone, "ci", "process")
        self.assertEqual(sum(s["state"] == "success" for s in f.statuses), 4)
        fetches = log.read_text()
        self.assertNotIn(f.sha, fetches)
        self.assertTrue(fetches)
        for command in fetches.splitlines():
            self.assertTrue(command.endswith("refs/heads/aida-intent:refs/intake/incoming"), command)

    # trace:STORY-2480 | ai:codex
    def test_cold_history_bounds_inventory_and_exact_old_read(self):
        self.check_history_bounds(warm=False)

    # trace:STORY-2480 | ai:codex
    def test_warm_history_bounds_new_inventory_and_exact_old_read(self):
        self.check_history_bounds(warm=True)

    def check_history_bounds(self, warm):
        f = self.f
        f.poll()
        original = f.receipt()
        fresh = f.clone("bounded-history", "home")
        if warm:
            f.cli(fresh, "pull")
        shas = {1: f.sha}
        values = []
        for n in range(2, 36):
            f.git(f.source, "commit", "--allow-empty", "-qm", f"source {n}")
            sha = f.git(f.source, "rev-parse", "HEAD").stdout.strip()
            shas[n] = sha
            value = dict(original, sha=sha, attempt=n)
            if n == 2:
                value["jobs"] = [dict(original["jobs"][0], name="not-in-home")]
            value["digest"] = hashlib.sha256(json.dumps({k: v for k, v in value.items() if k != "digest"}, separators=(",", ":")).encode()).hexdigest()
            values.append(value)
        f.git(f.source, "push", "-q", str(f.home),
              *[f"{sha}:refs/aida/ci/{n}" for n, sha in shas.items() if n > 1], principal="request")
        for value in values:
            f.inject(value)
        shim = f.root / "shim"
        shim.mkdir()
        log = f.root / "fetches"
        real_git = shutil.which("git")
        wrapper = shim / "git"
        wrapper.write_text(f"#!/usr/bin/python3\nimport os,sys\nif 'fetch' in sys.argv: open({str(log)!r},'a').write(sys.argv[-1]+'\\n')\nos.execv({real_git!r},[{real_git!r},*sys.argv[1:]])\n")
        wrapper.chmod(0o700)
        f.env["PATH"] = str(shim) + os.pathsep + f.env["PATH"]

        def source_fetches():
            return [line for line in log.read_text().splitlines() if line in shas.values()]

        start = time.monotonic()
        pulled = f.cli(fresh, "pull")
        self.assertLess(time.monotonic() - start, 30)
        unverified = 2 if warm else 3
        self.assertIn(f"{unverified} unverified-history", pulled.stderr)
        self.assertEqual(set(source_fetches()), set(shas[n] for n in range(4, 36)))
        self.assertEqual(len(source_fetches()), 32)
        checkpoint = f.private(fresh) / "validated-tip"
        self.assertTrue(checkpoint.exists())
        state = json.loads(checkpoint.read_text())
        tip = f.git(f.private(fresh) / "worktree", "rev-parse", "HEAD").stdout.strip()
        self.assertEqual(state["tip"], tip)
        self.assertEqual(state["verified_ci"].count("1"), 35 - unverified)
        log.write_text("")
        rows = json.loads(f.cli(fresh, "ci", "read", "--json").stdout)
        self.assertEqual(sum(row["verification"] == "unverified-history" for row in rows), unverified)
        self.assertFalse(source_fetches())
        # Even when all recent statuses are already handled, processing must
        # never promote old audit bytes into receipt authority implicitly.
        for value in [original, *values]:
            if value["attempt"] >= 4 or (warm and value["attempt"] == 1):
                marker = f.private(fresh) / f"ci-status-{value['attempt']}.json"
                marker.write_text(value["digest"])
                marker.chmod(0o600)
        f.cli(fresh, "ci", "process")
        self.assertFalse(source_fetches())
        self.assertFalse(f.statuses)
        # Exact read revalidates only this old identity and persists its proof.
        old_attempt = 3 if warm else 1
        rows = json.loads(f.cli(fresh, "ci", "read", "--attempt", str(old_attempt), "--sha", shas[old_attempt], "--json").stdout)
        self.assertEqual([row["verification"] for row in rows], ["verified"])
        self.assertEqual(source_fetches(), [shas[old_attempt]])
        log.write_text("")
        f.cli(fresh, "ci", "read", "--attempt", str(old_attempt), "--sha", shas[old_attempt])
        self.assertFalse(source_fetches())
        # A validator fingerprint change invalidates all old proof, but uses
        # exactly the same bounded path and creates a fresh incremental checkpoint.
        state = json.loads(checkpoint.read_text())
        state["validator"] = "different compiled validators"
        checkpoint.write_text(json.dumps(state))
        pulled = f.cli(fresh, "pull")
        self.assertIn("3 unverified-history", pulled.stderr)
        self.assertEqual(len(source_fetches()), 32)
        log.write_text("")
        f.cli(fresh, "pull")
        self.assertFalse(source_fetches())
        self.assertFalse(f.statuses)
        refused = f.cli(fresh, "ci", "read", "--attempt", "2", "--sha", shas[2], ok=False)
        self.assertNotEqual(refused.returncode, 0)
        self.assertNotIn("not-in-home", refused.stderr)
        rows = json.loads(f.cli(fresh, "ci", "read", "--json").stdout)
        old = next(row for row in rows if row["audit"]["attempt"] == 2)
        self.assertEqual(old["verification"], "unverified-history")
        self.assertFalse(f.statuses)

    def test_pending_attempt_does_not_starve_later_attempt(self):
        f = self.f
        f.result = "running"
        f.poll()
        f.cli(f.home_clone, "ci", "request", "--attempt", "2", "--sha", f.sha, principal="request")
        f.attempt, f.pipeline, f.result = 2, 42, "success"
        self.assertIn("attempt 2: receipt published", f.poll().stdout)
        self.assertEqual(f.receipt(2)["status"], "success")
        self.assertFalse(json.loads((f.private(f.work_clone) / "ci-attempt-1.json").read_text())["done"])

    def test_cached_history_still_requires_retained_admission(self):
        f = self.f
        f.poll()
        f.cli(f.home_clone, "pull")
        # Administrative corruption bypasses immutable-ref server policy.
        f.git(f.home, "update-ref", "-d", "refs/aida/ci/1")
        refused = f.cli(f.home_clone, "pull", ok=False)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("no retained home admission", refused.stderr)

    def test_ci_json_output(self):
        f = self.f
        result = f.cli(f.work_clone, "ci", "poll", "--json", principal="mirror")
        self.assertIn("published", json.loads(result.stdout)["message"])
        result = f.cli(f.home_clone, "ci", "process", "--json")
        self.assertIn("native status", json.loads(result.stdout)["message"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
