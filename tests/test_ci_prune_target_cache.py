"""Fixture tests for ci/prune-target-cache (the GitLab slot cache bound)."""
# trace:TASK-3910 | ai:claude
import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "ci" / "prune-target-cache"
DAY = 86400


class PruneTargetCacheTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.target = Path(self.tmp.name) / "target-v2-0"

    def tearDown(self):
        self.tmp.cleanup()

    def crate(self, rel, age_days, kib=4):
        """An incremental crate dir holding one session of `kib` KiB, last used `age_days` ago."""
        crate = self.target / rel
        session = crate / "s-session"
        session.mkdir(parents=True)
        (session / "query-cache.bin").write_bytes(b"x" * kib * 1024)
        stamp = time.time() - age_days * DAY
        for path in (session / "query-cache.bin", session, crate):
            os.utime(path, (stamp, stamp))
        return crate

    def run_prune(self, **env):
        # Hermetic: no ambient AIDA_CI_CACHE_* override can reach the script.
        full = {"PATH": "/usr/bin:/bin", "TMPDIR": self.tmp.name,
                "CARGO_TARGET_DIR": str(self.target)}
        full.update({k: str(v) for k, v in env.items()})
        return subprocess.run([str(SCRIPT)], env=full, check=True,
                              capture_output=True, text=True).stdout

    def test_removes_only_sessions_older_than_the_age_limit(self):
        old = self.crate("debug/incremental/aida_core-old", 10)
        new = self.crate("debug/incremental/aida_core-new", 1)
        cross = self.crate("x86_64-unknown-linux-musl/release/incremental/aida-old", 30)
        deps = self.target / "debug" / "deps"
        deps.mkdir(parents=True)
        (deps / "libkeep.rlib").write_bytes(b"k")
        out = self.run_prune()
        self.assertFalse(old.exists())
        self.assertFalse(cross.exists())
        self.assertTrue(new.exists())
        self.assertTrue((deps / "libkeep.rlib").exists(), "only incremental state is pruned")
        self.assertIn("removed 2 aged and 0 LRU", out)

    def test_age_limit_is_configurable(self):
        crate = self.crate("debug/incremental/aida_core-a", 3)
        self.run_prune(AIDA_CI_CACHE_MAX_AGE_DAYS=2)
        self.assertFalse(crate.exists())

    def test_size_cap_removes_least_recently_used_first(self):
        oldest = self.crate("debug/incremental/a-1", 3, kib=256)
        middle = self.crate("debug/incremental/b-2", 2, kib=256)
        newest = self.crate("debug/incremental/c-3", 1, kib=256)
        out = self.run_prune(AIDA_CI_CACHE_SLOT_CAP_KIB=600)
        self.assertFalse(oldest.exists())
        self.assertTrue(middle.exists())
        self.assertTrue(newest.exists())
        self.assertIn("removed 0 aged and 1 LRU", out)

    def test_under_cap_and_fresh_keeps_everything(self):
        crate = self.crate("debug/incremental/a-1", 1)
        out = self.run_prune()
        self.assertTrue(crate.exists())
        self.assertIn("removed 0 aged and 0 LRU", out)

    def test_clears_the_slot_when_non_incremental_state_exceeds_the_cap(self):
        deps = self.target / "debug" / "deps"
        deps.mkdir(parents=True)
        (deps / "libbig.rlib").write_bytes(b"x" * 512 * 1024)
        self.run_prune(AIDA_CI_CACHE_SLOT_CAP_KIB=100)
        self.assertTrue(self.target.is_dir())
        self.assertEqual(list(self.target.iterdir()), [])

    def test_missing_target_is_a_no_op(self):
        out = self.run_prune()
        self.assertIn("nothing to prune", out)
        self.assertFalse(self.target.exists())


if __name__ == "__main__":
    unittest.main()
