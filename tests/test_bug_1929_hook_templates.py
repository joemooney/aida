#!/usr/bin/env python3
"""No-build behavioural preflight for shipped store hooks and planted managers.

Rust's store_hook_tests drive commit_paths and its test-only mutation. This
fixture exercises the actual template scripts with real git before CI builds.
trace:BUG-1929 | ai:codex
"""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class StoreHookTemplates(unittest.TestCase):
    def test_managers_and_drop_pin_mutations(self):
        for manager in ("husky", "pre-commit", "lefthook"):
            for mutant in (False, True):
                with self.subTest(manager=manager, mutant=mutant), tempfile.TemporaryDirectory() as tmp:
                    base = Path(tmp)
                    store = base / ".aida-store"
                    hooks = base / "host-hooks"
                    project = base / "project-hooks"
                    marker = base / "host-exec-plant"
                    for directory in (store, hooks, project):
                        directory.mkdir()
                    proc_env = dict(os.environ)
                    proc_env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                                    GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.com",
                                    GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.com",
                                    AIDA_STORE_WRITE_GUARD="test", AIDA_STORE_HOOKS_ONLY="1",
                                    PATH="/usr/bin:/bin", AIDA_COMMIT_STRICT="true")
                    def git(*args, check=True, pin=True):
                        argv = ["git", "-c", "core.fsmonitor=false"]
                        if pin:
                            argv.extend(["-c", f"core.hooksPath={hooks}"])
                        return subprocess.run(argv + list(args), cwd=store, env=proc_env,
                                              check=check, capture_output=True, text=True)
                    def executable(path, body):
                        path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_text(body)
                        path.chmod(0o700)
                    git("init", "-q", "-b", "aida-store")
                    git("config", "core.hooksPath", str(project))
                    for name, master in (("pre-commit", "aida-pre-commit.sh"),
                                         ("commit-msg", "aida-commit-msg")):
                        executable(hooks / name, (ROOT / "aida-core/templates/hooks" / master).read_text())
                    executable(store / "planted-tool", f"#!/bin/sh\necho {manager} > '{marker}'\n")
                    if manager == "husky":
                        executable(store / "node_modules/.bin/lint-staged", (store / "planted-tool").read_text())
                        executable(store / ".husky/pre-commit", "#!/bin/sh\nlint-staged\n")
                        executable(project / "h", '#!/bin/sh\nn=$(basename "$0")\nexport PATH="node_modules/.bin:$PATH"\nsh -e ".husky/$n" "$@"\n')
                        executable(project / "pre-commit", f"#!/bin/sh\n. '{project / 'h'}'\n")
                    else:
                        config, key = (".pre-commit-config.yaml", "entry") if manager == "pre-commit" else ("lefthook.yml", "run")
                        (store / config).write_text(f"  {key}: ./planted-tool\n")
                        executable(project / "pre-commit", f"#!/bin/sh\ncmd=$(sed -n 's/^[[:space:]]*{key}: //p' {config})\nsh -c \"$cmd\"\n")
                    (store / "notes.txt").write_text("note\n")
                    git("add", "notes.txt")
                    invalid = git("commit", "-m", "invalid", check=False)
                    self.assertNotEqual(invalid.returncode, 0)
                    self.assertIn("conventional", invalid.stderr)
                    sentinel = (store / ".git/aida-precommit-sentinel").read_text().strip()
                    self.assertEqual(sentinel, git("write-tree").stdout.strip())
                    self.assertFalse(marker.exists())
                    git("commit", "-m", "chore: store write", pin=not mutant)
                    self.assertEqual(marker.exists(), mutant)
                    if mutant:
                        self.assertEqual(marker.read_text().strip(), manager)
                        print(f"MUTATION KILLED: {manager}: drop pin executes planted host command")


if __name__ == "__main__":
    unittest.main()
