"""Exercise the real doctor command with an isolated repository and terminal.

Set AIDA_BIN to the candidate CLI binary before running this test.
"""
# trace:BUG-2722 | ai:codex
from contextlib import contextmanager
import errno
from functools import partial
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import subprocess
import tempfile
import time
import unittest


@contextmanager
def confirming(args, cwd, env, prompt):
    master, slave = pty.openpty()
    proc = subprocess.Popen(args, stdin=slave, stderr=slave, stdout=slave, cwd=cwd, env=env)
    try:
        preview = b""
        deadline = time.monotonic() + 10
        while prompt not in preview:
            assert select.select([master], [], [], max(0, deadline - time.monotonic()))[0], preview
            preview += os.read(master, 65536)
        yield lambda reply: os.write(master, (reply + "\n").encode()), proc
    finally:
        proc.kill()
        proc.wait()
        os.close(master)
        os.close(slave)


class RepairAttestation(unittest.TestCase):
    # trace:BUG-2755 | ai:codex
    def test_clone_release_requires_direct_operator(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "main"
            tree = Path(tmp) / "aida-seat-unbound"
            env = dict(PATH=os.environ["PATH"], HOME=tmp, AIDA_OUTPUT_FORMAT="human")
            subprocess.run(["git", "init", "-q", str(root)], check=True, env=env)
            run = partial(subprocess.run, cwd=root, text=True, capture_output=True, timeout=30)
            binary = os.environ["AIDA_BIN"]
            run([binary, "init", "--no-skills", "--no-hooks"], env=env | dict(GIT_AUTHOR_NAME="test",
                GIT_COMMITTER_NAME="test", GIT_AUTHOR_EMAIL="t@test", GIT_COMMITTER_EMAIL="t@test"), check=True)
            run(["git", "clone", "--shared", str(root), str(tree)], env=env, check=True)
            alternates = Path(".git/objects/info/alternates")
            contents = os.fsencode(root / ".git/objects") + b"\n"
            (tree / alternates).write_bytes(contents)
            id_ = "deadbeef2755"
            lease = root / ".aida" / "sessions" / (id_ + ".toml")
            lease.parent.mkdir(parents=True, exist_ok=True)
            body = (f"id='{id_}'\nscope='TASK-3211'\nslug='test'\nowner='test'\nworktree_path='{tree}'\n"
                    "branch='old'\nstarted_at='2026-10-01T00:00:00Z'\nhostname='test'\ntree_kind='clone'\ncreator_pid=2147483647\n")
            lease.write_text(body)
            args = [binary, "doctor", "heal", "stale-leases", "--id", id_, "--attest", "stopped writers"]
            tty = ["script", "-qec", shlex.join(args), "/dev/null"]
            for command, extra, refusal in [(args, {}, "interactive TTY"), (tty, {"AIDA_AGENT_NAME": "worker"}, "AIDA-managed agent")]:
                result = run(command, env=env | extra, input=id_ + "\n")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(refusal, result.stderr + result.stdout)
                self.assertTrue(tree.is_dir())
                self.assertEqual(lease.read_text(), body)
            ticks = Path("/proc/self/stat").read_text().rsplit(")", 1)[1].split()[19]
            namespace = Path("/proc/self/ns/pid").stat().st_ino
            stamped = body + f"pid_namespace={namespace}\nactive_pid={os.getpid()}\nactive_pid_start_time='linux-starttime:{ticks}'\n"
            lease.write_text(stamped)
            result = run(tty, env=env, input=id_ + "\n")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("holder is not stale", result.stdout)
            self.assertEqual(lease.read_text(), stamped)
            self.assertTrue(tree.is_dir())
            lease.write_text(body)
            events = lambda: {e: json.loads(e.read_text()) for e in lease.parent.glob("*.attestation.json")}
            # trace:BUG-2755 | ai:claude — a present but inaccessible tree refuses and keeps its lease.
            if os.geteuid() != 0:
                locked, prior = Path(tmp) / "locked", events()
                hidden = locked / tree.name
                locked.mkdir()
                tree.rename(hidden)
                lease.write_text(body.replace(str(tree), str(hidden)))
                locked.chmod(0)
                try:
                    result = run(tty, env=env, input=id_ + "\n")
                finally:
                    locked.chmod(0o700)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(os.strerror(errno.EACCES), result.stdout)
                self.assertEqual(lease.read_text(), body.replace(str(tree), str(hidden)))
                self.assertEqual(events(), prior)
                self.assertTrue((hidden / ".git").is_dir())
                hidden.rename(tree)
                lease.write_text(body)
            # An ordinary heal refuses the unverified binding, even with --yes.
            result = run(args[:-2] + ["--yes"], env=env)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("--attest", result.stderr)
            try:
                other = tempfile.TemporaryDirectory(dir="/dev/shm")
            except OSError:
                other = None
            if other is not None:
                with other as home:
                    if Path(home).stat().st_dev != tree.stat().st_dev:
                        prior = events()
                        result = run(tty, env=env | {"HOME": home}, input=id_ + "\n")
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn(os.strerror(errno.EXDEV), result.stdout)
                        self.assertTrue(tree.is_dir() and lease.exists())
                        new = sorted((v for e, v in events().items() if e not in prior), key=lambda v: v["outcome"])
                        self.assertEqual([v["outcome"].split(":")[0] for v in new], ["failed", "pending"])
                        self.assertIn(os.strerror(errno.EXDEV), new[0]["outcome"])
                        self.assertTrue(new[1]["planned"].startswith("salvage destination"))
            (tree / "mail").write_text("pending")
            for change in ("replace", "create", "stamp", "wrong", "salvage", "absent"):
                if change == "create":
                    tree.rmdir()
                lease.write_text(body)
                existing_events = set(lease.parent.glob("*.attestation.json"))
                with confirming(args, root, env, b"to release its lease") as (reply, proc):
                    if change == "replace":
                        tree.rename(Path(tmp) / "original")
                    if change in ("replace", "create"):
                        tree.mkdir()
                    if change == "stamp":
                        lease.write_text(body + "ci_fix_child_grant_id='successor'\n")
                    reply("wrong" if change == "wrong" else id_)
                    success = change in ("salvage", "absent")
                    self.assertEqual(proc.wait(timeout=10) == 0, success, change)
                    self.assertEqual(lease.exists(), not success)
                    self.assertEqual(tree.exists(), not success)
                if change == "create":
                    tree.rmdir()
                    (Path(tmp) / "original").rename(tree)
                if change == "salvage":
                    saved, = (Path(tmp) / "salvage/reaped").iterdir()
                    self.assertEqual((saved / alternates).read_bytes(), contents)
                    self.assertEqual((saved / "mail").read_text(), "pending")
                if success:
                    new = [json.loads(e.read_text()) for e in set(lease.parent.glob("*.attestation.json")) - existing_events]
                    outcome = f"salvage destination {saved}" if change == "salvage" else "tree absent, nothing to salvage"
                    record = dict(lease_id=id_, reason="stopped writers", outcome=outcome)
                    self.assertCountEqual(new, [record, record | dict(outcome="pending", planned=outcome)])

    def test_real_terminal_gates_and_concurrent_change(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(["git", "init", "-q", tmp], check=True)
            directory = root / ".aida"
            directory.mkdir()
            journal = directory / "repair-journal.json"
            entry = dict(key=dict(repository=tmp, pr=4376, original_head="660c0e87"),
                         lane_id="01a2053df3e6", tree_path=tmp + "/moved-tree",
                         branch="repair", base="original-base", spawn="unknown",
                         original_conflict_paths=["src/lib.rs"], reserved_at="earlier",
                         dispatched_by="direct-rework")
            journal.write_text(json.dumps(dict(version=1, entries=[entry])))
            id_ = "pr-4376-660c0e87-01a2053df3e6"
            args = [os.environ["AIDA_BIN"], "doctor", "heal",
                    "repair-reservations", "--id", id_, "--attest", "checked moved tree"]
            env = dict(PATH=os.environ["PATH"], HOME=tmp, AIDA_OUTPUT_FORMAT="human")
            for extra, flags, expected in [
                ({"AIDA_AGENT_NAME": "worker"}, [], "AIDA-managed agent"),
                ({"AIDA_AUTO_COMPLETE": "1"}, [], "requires a human operator"),
                ({"AIDA_DEPUTY_JOB": "unconfirmed"}, [], "requires a human operator"),
                ({}, ["--json"], "refuses --json"),
            ]:
                result = subprocess.run(["script", "-qec", shlex.join(args + flags), "/dev/null"],
                                        input=id_ + "\n", capture_output=True, text=True,
                                        env=env | extra, cwd=tmp, timeout=10)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(expected, result.stdout)
                self.assertEqual(json.loads(journal.read_text())["entries"], [entry])

            for change in (True, False):
                with confirming(args, tmp, env, b"to release:") as (reply, proc):
                    with (directory / "repair-journal.lock").open("a") as lock:
                        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        if change:
                            state = json.loads(journal.read_text())
                            state["entries"][0]["reserved_at"] = "new generation"
                            journal.write_text(json.dumps(state))
                    reply(id_)
                    proc.communicate(timeout=10)
                    state = json.loads(journal.read_text())
                    if change:
                        self.assertNotEqual(proc.returncode, 0)
                        self.assertFalse(state.get("releases"))
                    else:
                        self.assertEqual(proc.returncode, 0)
                        self.assertEqual(state["releases"][0]["id"], id_)

    # The bare sweep releases without a per-ID attestation, so it must refuse
    # every non-operator caller and --yes/--json before it reads the forge.
    # Without the gate this journal's PR (no forge here) is kept and the
    # command exits 0, so each case below fails if its refusal is removed.
    # trace:TASK-3891 | ai:claude
    def test_bare_sweep_refuses_without_operator_authority(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(["git", "init", "-q", tmp], check=True)
            directory = root / ".aida"
            directory.mkdir()
            journal = directory / "repair-journal.json"
            entry = dict(key=dict(repository=tmp, pr=4376, original_head="660c0e87"),
                         lane_id="01a2053df3e6", tree_path=tmp + "/closed-pr-tree",
                         branch="repair", base="original-base", spawn="unknown",
                         original_conflict_paths=["src/lib.rs"], reserved_at="earlier",
                         dispatched_by="direct-rework")
            journal.write_text(json.dumps(dict(version=1, entries=[entry])))
            before = journal.read_text()
            args = [os.environ["AIDA_BIN"], "doctor", "heal", "repair-reservations"]
            env = dict(PATH=os.environ["PATH"], HOME=tmp, AIDA_OUTPUT_FORMAT="human")
            # No terminal at all.
            result = subprocess.run(args, stdin=subprocess.DEVNULL, capture_output=True,
                                    text=True, env=env, cwd=tmp, timeout=30)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("interactive TTY", result.stdout + result.stderr)
            self.assertEqual(journal.read_text(), before)
            for extra, flags, expected in [
                ({"AIDA_AGENT_NAME": "worker"}, [], "AIDA-managed agent"),
                ({"AIDA_AUTO_COMPLETE": "1"}, [], "requires a human operator"),
                ({"AIDA_DEPUTY_JOB": "unconfirmed"}, [], "requires a human operator"),
                ({}, ["--json"], "refuses --json"),
                ({}, ["--yes"], "refuses --yes"),
            ]:
                result = subprocess.run(["script", "-qec", shlex.join(args + flags), "/dev/null"],
                                        stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                        env=env | extra, cwd=tmp, timeout=30)
                self.assertNotEqual(result.returncode, 0, (extra, flags, result.stdout))
                self.assertIn(expected, result.stdout)
                self.assertEqual(journal.read_text(), before)


if __name__ == "__main__":
    unittest.main()
