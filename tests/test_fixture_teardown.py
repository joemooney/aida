#!/usr/bin/env python3
"""A temp project whose cache-refresh worker is mid-write tears down cleanly.

CI (contracts) once failed with ENOTEMPTY on `.aida/cache-db` because a
detached `cache refresh --worker` was still writing while the fixture was
removed. Run: python3 tests/test_fixture_teardown.py
trace:TASK-3962 | ai:claude
"""
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest

from fixture_teardown import temp_project

# Stands in for the detached worker: same argv shape and cwd (the project
# root), keeps creating sidecars under cache-db after the spawner returned.
# With a plain TemporaryDirectory this writer makes rmtree raise ENOTEMPTY
# on every iteration (checked by hand while writing the fix).
WORKER = r"""
import sys, time
from pathlib import Path
cache_dir = Path(sys.argv[-1]).parent
deadline = time.monotonic() + 0.4
n = 0
while time.monotonic() < deadline:
    try:
        cache_dir.mkdir(parents=True, exist_ok=True)
        (cache_dir / f"cache.db.part-{n}").write_bytes(b"x")
    except OSError:
        pass
    n += 1
"""


@unittest.skipUnless(os.path.isdir("/proc"), "fixture-process scan reads /proc")
class MidWriteTeardown(unittest.TestCase):
    def test_teardown_waits_for_mid_write_worker_in_a_loop(self):
        for iteration in range(25):
            with temp_project("aida-teardown-race-") as temp:
                root = Path(temp) / "project"
                cache = root / ".aida/cache-db/cache.db"
                cache.parent.mkdir(parents=True)
                cache.write_bytes(b"")
                worker = subprocess.Popen(
                    [sys.executable, "-c", WORKER, "cache", "refresh", "--worker",
                     "--store", str(root / ".aida-store"), "--cache", str(cache)],
                    cwd=root, start_new_session=True)
                deadline = time.monotonic() + 10
                while not (cache.parent / "cache.db.part-50").exists():
                    self.assertLess(time.monotonic(), deadline, "worker never wrote")
                    time.sleep(0.005)
                self.assertIsNone(worker.poll(), "worker must still be mid-write at teardown")
            # Leaving the block removed the tree without ENOTEMPTY.
            self.assertEqual(worker.wait(timeout=5), 0)
            self.assertFalse(Path(temp).exists(), iteration)

    def test_teardown_still_waits_when_the_body_raised(self):
        with self.assertRaises(RuntimeError):
            with temp_project("aida-teardown-raise-") as temp:
                worker = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(0.3)"],
                                          cwd=temp, start_new_session=True)
                raise RuntimeError("body failed")
        self.assertEqual(worker.wait(timeout=5), 0)
        self.assertFalse(Path(temp).exists())


if __name__ == "__main__":
    unittest.main()
