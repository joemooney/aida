#!/usr/bin/env python3
"""Compare installed built-in and porcelain reports without network or writes.
trace:STORY-2704 | ai:codex
Usage: python3 tests/test_porcelain_flow_parity.py /path/to/aida /path/to/aida-porcelain
"""
import shutil
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    builtin, porcelain = map(lambda s: str(Path(shutil.which(s) or s).resolve()), sys.argv[1:])
    with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as home:
        root = Path(tmp)
        # Inject fixture inputs explicitly; do not inherit agent credentials,
        # Git configuration or ambient output-format overrides.
        env = {"PATH": os.defpath, "HOME": home, "GIT_CONFIG_NOSYSTEM": "1"}
        def git(*args, **kwargs):
            kwargs.setdefault("env", env)
            return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, **kwargs)
        git("init", "-b", "main")
        (root / "tracked.txt").write_text("fixture\n")
        git("add", "tracked.txt")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "subject (#42)",
            env={**env, "GIT_AUTHOR_DATE": "2026-11-09T04:00:00Z", "GIT_COMMITTER_DATE": "2026-11-09T04:00:00Z"})
        git("update-ref", "refs/remotes/origin/main", "HEAD")
        gh = root / "bin/gh"
        gh.parent.mkdir()
        gh.write_text('''#!/bin/sh
case "$1 $2" in
'pr view') echo '{"createdAt":"2026-11-09T00:00:00Z","commits":[{"oid":"abc"}]}' ;;
'run list') echo '[]' ;;
*) exit 19 ;;
esac
''')
        gh.chmod(0o755)
        for name in [".aida/gate-ledger/PR-42.jsonl", ".aida/queue.json", ".aida/sessions/lease.toml", ".aida/cache-db/state", ".aida-store/refs"]:
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('{"kind":"verdict","record":{"verdict":"APPROVED","recorded_at":"2026-11-09T02:00:00Z"}}\n' if name.endswith("jsonl") else "fixture")
        git("-C", str(root / ".aida-store"), "init", "-b", "aida-store")
        git("-C", str(root / ".aida-store"), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "store fixture")
        env = {**env, "PATH": f"{gh.parent}:{os.defpath}", "AIDA_TEST_GH_BINARY": str(gh), "AIDA_NO_UPDATE_CHECK": "1"}
        def snapshot():
            return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob("*") if p.is_file()}
        before = snapshot()
        for suffix in [[], ["--json"], ["--format", "json"]]:
            args = ["flow", "report", "--since", "2026-11-09", "--until", "2026-11-10", *suffix]
            outputs = [subprocess.run([exe, *args], cwd=root, env=env, capture_output=True) for exe in [builtin, porcelain]]
            for result in outputs:
                assert result.returncode == 0, result.stderr.decode()
            assert outputs[0].stdout == outputs[1].stdout, [p.stdout.decode() for p in outputs]
        assert before == snapshot(), "report changed fixture state"
    print("PASS: built-in/porcelain text and JSON parity; fixture state unchanged")


if __name__ == "__main__":
    main()
