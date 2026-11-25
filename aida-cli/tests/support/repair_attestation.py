"""Exercise the real doctor command with an isolated repository and terminal.

Set AIDA_BIN to the candidate CLI binary before running this test.
"""
# trace:BUG-2722 | ai:codex
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import time
import unittest


class RepairAttestation(unittest.TestCase):
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
                master, slave = pty.openpty()
                try:
                    os.write(master, (id_ + "\n").encode())
                    result = subprocess.run(args + flags, stdin=slave, stderr=slave,
                                            stdout=slave, env=env | extra,
                                            cwd=tmp, timeout=10)
                    self.assertNotEqual(result.returncode, 0)
                    transcript = bytearray()
                    while select.select([master], [], [], 0)[0]:
                        transcript.extend(os.read(master, 65536))
                    self.assertIn(expected, transcript.decode())
                    self.assertEqual(json.loads(journal.read_text())["entries"], [entry])
                finally:
                    os.close(master)
                    os.close(slave)

            for change in (True, False):
                master, slave = pty.openpty()
                proc = subprocess.Popen(args, stdin=slave, stderr=slave, cwd=tmp, env=env,
                                        stdout=slave)
                try:
                    preview = b""
                    deadline = time.monotonic() + 10
                    while b"to release:" not in preview:
                        self.assertTrue(select.select([master], [], [], max(0, deadline - time.monotonic()))[0], preview)
                        preview += os.read(master, 65536)
                    with (directory / "repair-journal.lock").open("a") as lock:
                        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        if change:
                            state = json.loads(journal.read_text())
                            state["entries"][0]["reserved_at"] = "new generation"
                            journal.write_text(json.dumps(state))
                    os.write(master, (id_ + "\n").encode())
                    proc.communicate(timeout=10)
                    state = json.loads(journal.read_text())
                    if change:
                        self.assertNotEqual(proc.returncode, 0)
                        self.assertFalse(state.get("releases"))
                    else:
                        self.assertEqual(proc.returncode, 0)
                        self.assertEqual(state["releases"][0]["id"], id_)
                finally:
                    proc.kill()
                    proc.wait()
                    os.close(master)
                    os.close(slave)


if __name__ == "__main__":
    unittest.main()
