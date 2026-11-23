"""Regression coverage for the shared CLI/server build stamp."""
# trace:BUG-2728 | ai:codex

from pathlib import Path
import subprocess
import tempfile
import unittest


class BuildGitStampTest(unittest.TestCase):
    def test_dirty_stamp_names_only_tracked_changes(self):
        helper = Path(__file__).resolve().parents[1] / "aida-cli-lib/build_git_stamp.rs"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / "repo"
            repo.mkdir()
            probe = root / "probe.rs"
            probe.write_text(helper.read_text() + "\nfn main() { emit_git_stamps(); }\n")
            binary = root / "probe"
            subprocess.run(["rustc", "--edition=2021", probe, "-o", binary], check=True)

            def git(*args):
                subprocess.run(["git", *args], cwd=repo, check=True)

            def stamp():
                return subprocess.check_output(
                    [binary], cwd=repo, text=True,
                ).splitlines()

            git("init", "-q")
            git("config", "commit.gpgsign", "false")
            tracked = repo / "tracked file"
            tracked.touch()
            git("add", ".")
            git("-c", "user.name=Test", "-c", "user.email=test@example.org",
                "commit", "-qm", "initial")
            (repo / "DU.txt").touch()
            clean = stamp()
            self.assertIn("cargo:rustc-env=AIDA_BUILD_GIT_DIRTY=0", clean)
            self.assertIn("cargo:rustc-env=AIDA_BUILD_GIT_STATUS=", clean)

            tracked.write_text("modified")
            (repo / "second").touch()
            git("add", "second")
            dirty = stamp()
            self.assertIn("cargo:rustc-env=AIDA_BUILD_GIT_DIRTY=1", dirty)
            self.assertIn(
                'cargo:rustc-env=AIDA_BUILD_GIT_STATUS=A  second |  M "tracked file"',
                dirty,
            )

            unusual = repo / "new\nline"
            unusual.touch()
            git("add", unusual.name)
            self.assertIn(
                r'cargo:rustc-env=AIDA_BUILD_GIT_STATUS=A  "new\nline" | A  second |  M "tracked file"',
                stamp(),
            )


if __name__ == "__main__":
    unittest.main()
