"""Exercise the operator cron using disposable Git repos and launcher commands."""
# trace:TASK-3812 | ai:codex
import fcntl
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/aida-autobuild.sh"
CONFIG = ".aida/config.toml"


class AutobuildTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.home = Path(scratch.name)
        self.repo = self.home / "primary"
        self.upstream = self.home / "upstream"
        self.config = self.repo / CONFIG
        self.log = self.home / "autobuild.log"
        self.env = dict(
            PATH="/usr/bin:/bin",
            LANG="C.UTF-8",
            HOME=str(self.home),
            AIDA_REPO_DIR=str(self.repo),
            AIDA_AUTOBUILD_LOG=str(self.log),
            GIT_CONFIG_NOSYSTEM="1",
            GIT_CONFIG_GLOBAL="/dev/null",
            GIT_AUTHOR_NAME="test",
            GIT_AUTHOR_EMAIL="test@example.org",
            GIT_COMMITTER_NAME="test",
            GIT_COMMITTER_EMAIL="test@example.org",
        )
        self.upstream.mkdir()
        self.git(self.upstream, "init", "-q", "-b", "main")
        (self.upstream / CONFIG).parent.mkdir()
        (self.upstream / CONFIG).write_text("cap = 3\n\n# policy\npolicy = 1\n")
        self.git(self.upstream, "add", CONFIG)
        self.git(self.upstream, "commit", "-qm", "base")
        self.git(self.home, "clone", "-q", str(self.upstream), str(self.repo))
        self.head = self.git(self.repo, "rev-parse", "HEAD")
        self.config.write_text(self.config.read_text().replace("cap = 3", "cap = 8"))
        self.git(self.repo, "update-index", "--skip-worktree", CONFIG)
        (self.home / "live").write_text(self.head)
        (self.home / ".local/bin").mkdir(parents=True)
        self.stub("gh", """
            if test -f "$HOME/pending"; then
                rm "$HOME/pending"
                echo 'in_progress '
            else
                echo 'completed success'
            fi
        """)
        self.stub("sleep", 'echo poll >> "$HOME/calls"')
        self.stub("aida", """
            case "$*" in
                --version) echo "aida 0.16.0 (built 2024-01-01 00:00:00 UTC, sha $(cat "$HOME/live"))" ;;
                "dev build")
                    test "$AIDA_AUTOBUILD_LOCK_FD" = 9
                    flock -n 9
                    if flock -n -E 75 .git true; then
                        exit 1
                    else
                        test "$?" = 75
                    fi
                    test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)"
                    test -z "$(git status --porcelain --untracked-files=no)"
                    grep -q '^cap = 8$' .aida/config.toml
                    echo build >> "$HOME/calls"
                    if ! test -f "$HOME/no-promotion"; then git rev-parse HEAD > "$HOME/live"; fi ;;
                *) exit 2 ;;
            esac
        """)
        self.stub(self.home / "lane-start.sh", """
            test "$*" = --check
            echo launcher >> "$HOME/calls"
        """)

    def stub(self, path, body):
        executable = self.home / ".local/bin" / path
        executable.write_text("#!/bin/bash\nset -euo pipefail\n" + body)
        executable.chmod(0o700)

    def git(self, root, *args):
        return subprocess.check_output(
            ["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", *args],
            cwd=root, env=self.env, text=True, stderr=subprocess.PIPE
        ).strip()

    def advance(self, old, new):
        path = self.upstream / CONFIG
        path.write_text(path.read_text().replace(old, new))
        self.git(self.upstream, "commit", "-qam", "incoming config")
        return self.git(self.upstream, "rev-parse", "HEAD")

    def run_cron(self, *args, code=0):
        result = subprocess.run(
            ["bash", str(SCRIPT), *args], cwd=self.repo, env=self.env,
            capture_output=True, text=True, timeout=20,
        )
        self.assertEqual(result.returncode, code, result.stderr + (self.log.read_text() if self.log.exists() else ""))
        return self.log.read_text() if self.log.exists() else ""

    def test_waits_rebases_override_promotes_and_checks_launcher(self):
        tip = self.advance("policy = 1", "policy = 2")
        (self.home / "pending").touch()
        self.assertIn("promoted and verified", self.run_cron())
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), tip)
        self.assertEqual(self.config.read_text(), "cap = 8\n\n# policy\npolicy = 2\n")
        self.assertEqual(self.git(self.repo, "ls-files", "-v", CONFIG), f"S {CONFIG}")
        self.assertEqual((self.home / "calls").read_text(), "poll\nbuild\nlauncher\n")

    def test_conflicting_override_refuses_without_mutating_checkout(self):
        self.advance("cap = 3", "cap = 4")
        original = self.config.read_bytes()
        self.assertIn("conflicts with the host override", self.run_cron())
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertEqual(self.config.read_bytes(), original)
        self.assertEqual(self.git(self.repo, "ls-files", "-v", CONFIG), f"S {CONFIG}")
        self.assertFalse((self.home / "calls").exists())

    def test_dry_run_preserves_checkout_and_override(self):
        self.advance("policy = 1", "policy = 2")
        original = self.config.read_bytes()
        self.assertIn("dry run would fast-forward", self.run_cron("--dry-run"))
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertEqual(self.config.read_bytes(), original)
        self.assertFalse((self.home / "calls").exists())

    def test_hidden_entry_probe_failure_refuses_before_fetch(self):
        self.advance("policy = 1", "policy = 2")
        self.stub("awk", "exit 2")
        self.assertIn("no visible tracked changes or other hidden entries", self.run_cron())
        self.assertEqual(self.git(self.repo, "rev-parse", "origin/main"), self.head)
        self.assertFalse((self.home / "calls").exists())

    def test_red_ci_leaves_checkout_and_binary_alone(self):
        self.advance("policy = 1", "policy = 2")
        self.stub("gh", "echo 'completed failure'")
        self.assertIn("completed failure", self.run_cron())
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertEqual((self.home / "live").read_text(), self.head)
        self.assertFalse((self.home / "calls").exists())

    def test_hidden_symlink_is_refused(self):
        self.advance("policy = 1", "policy = 2")
        secret = self.home / "operator-config"
        secret.write_text(self.config.read_text())
        self.config.unlink()
        self.config.symlink_to(secret)
        self.assertIn("hidden config must be a regular file", self.run_cron(code=1))
        self.assertTrue(self.config.is_symlink())
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertFalse((self.home / "calls").exists())

    def test_busy_lock_skips_without_fetching_or_building(self):
        self.advance("policy = 1", "policy = 2")
        lock = os.open(self.repo / ".git", os.O_RDONLY | os.O_DIRECTORY)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.assertIn("build in progress, skipped", self.run_cron())
        finally:
            os.close(lock)
        self.assertEqual(self.git(self.repo, "rev-parse", "origin/main"), self.head)
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertFalse((self.home / "calls").exists())

    def test_failed_fast_forward_restores_original_config(self):
        self.advance("policy = 1", "policy = 2")
        (self.upstream / "collision").write_text("tracked upstream")
        self.git(self.upstream, "add", "collision")
        self.git(self.upstream, "commit", "-qm", "add colliding file")
        (self.repo / "collision").write_text("operator file")
        original = self.config.read_bytes()
        self.assertIn("could not fast-forward", self.run_cron(code=1))
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertEqual(self.config.read_bytes(), original)
        self.assertEqual(self.git(self.repo, "ls-files", "-v", CONFIG), f"S {CONFIG}")
        self.assertEqual((self.repo / "collision").read_text(), "operator file")
        self.assertFalse((self.home / "calls").exists())

    def test_tip_movement_while_waiting_defers_promotion(self):
        self.advance("policy = 1", "policy = 2")
        self.git(self.upstream, "branch", "pending-tip")
        moved = self.advance("policy = 2", "policy = 3")
        self.git(self.upstream, "checkout", "-q", "pending-tip")
        self.git(self.upstream, "branch", "-f", "main", "HEAD")
        self.stub("sleep", f'git -C "$HOME/upstream" branch -f main {moved}')
        (self.home / "pending").touch()
        self.assertIn("HEAD or origin/main moved", self.run_cron())
        self.assertEqual(self.git(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertEqual(self.git(self.repo, "rev-parse", "origin/main"), moved)
        self.assertFalse((self.home / "calls").exists())

    def test_a_hex_named_git_ref_does_not_count_as_a_binary_sha(self):
        self.git(self.repo, "tag", "deadbeef", "HEAD")
        (self.home / "live").write_text("deadbeef")
        self.assertIn("promoted and verified", self.run_cron())
        self.assertEqual((self.home / "calls").read_text(), "build\nlauncher\n")

    def test_successful_build_exit_without_promotion_cannot_pass_verification(self):
        self.advance("policy = 1", "policy = 2")
        (self.home / "no-promotion").touch()
        self.assertIn("does not match", self.run_cron(code=1))
        self.assertEqual((self.home / "calls").read_text(), "build\n")


if __name__ == "__main__":
    unittest.main()
