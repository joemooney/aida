#!/usr/bin/env python3
"""Compare the core reader, external flow and porcelain without network or writes.
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
    with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as home, tempfile.TemporaryDirectory() as installed:
        # Exercise the shipped layout independently of Cargo's output directory
        # and any host-installed companions. A symlink to aida would canonicalize
        # back to the build directory during coordinating-bin discovery.
        # trace:STORY-2704 | ai:codex
        for source, name in [(builtin, "aida"), (porcelain, "aida-porcelain")]:
            destination = Path(installed) / name
            try:
                os.link(source, destination)
            except OSError:
                shutil.copy2(source, destination)
        (Path(installed) / "aida-flow").symlink_to("aida-porcelain")
        builtin = str(Path(installed) / "aida")
        porcelain = str(Path(installed) / "aida-porcelain")
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
        env = {**env, "PATH": f"{gh.parent}:{os.defpath}", "AIDA_TEST_GH_BINARY": str(gh), "AIDA_NO_UPDATE_CHECK": "1", "AIDA_BIN": builtin}
        def snapshot():
            return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob("*") if p.is_file()}
        before = snapshot()
        def invoke(exe, args, overrides=None):
            result = subprocess.run([exe, *args], cwd=root, env={**env, **(overrides or {})}, capture_output=True)
            assert before == snapshot(), f"{exe} {args} changed fixture state"
            return result

        for suffix in [[], ["--json"], ["--format", "json"], ["--format", "human"], ["--format", "toon"]]:
            args = ["flow", "report", "--since", "2026-11-09", "--until", "2026-11-10", *suffix]
            outputs = [invoke(exe, args) for exe in [builtin, porcelain]]
            for result in outputs:
                assert result.returncode == 0, result.stderr.decode()
            assert outputs[0].stdout == outputs[1].stdout, [p.stdout.decode() for p in outputs]
            # Keep an independent reference: comparing only public flow with
            # porcelain would compare the same renderer against itself.
            reference = invoke(builtin, ["flow-data", *args[1:]])
            assert reference.returncode == 0, reference.stderr.decode()
            assert outputs[0].stdout == reference.stdout, [outputs[0].stdout, reference.stdout]
        for bounds, message in [
            (["--since", "bad"], b"expected RFC3339"),
            (["--since", "2026-11-10", "--until", "2026-11-09"], b"since must not be later than until"),
        ]:
            for suffix in [[], ["--format", "human"], ["--format", "toon"], ["--format", "json"]]:
                outputs = [invoke(exe, ["flow", "report", *bounds, *suffix]) for exe in [builtin, porcelain]]
                assert [p.returncode for p in outputs] == [1, 1]
                assert outputs[0].stdout == outputs[1].stdout, [p.stdout for p in outputs]
                assert outputs[0].stderr == outputs[1].stderr, [p.stderr for p in outputs]
                assert all(message in p.stdout + p.stderr for p in outputs)
        # Environment aliases must pin both error channels even when the agent
        # preference would otherwise select the opposite format.
        def check_alias_errors(bounds):
            for alias, canonical, agent in [
                ("table", "human", "0"),
                ("table", "human", "1"),
                (" TABLE ", "human", "1"),
                ("agent", "toon", "0"),
                ("agent", "toon", "1"),
                (" AgEnT ", "toon", "0"),
            ]:
                args = ["flow", "report", *bounds]
                outputs = [invoke(exe, args, {"AIDA_OUTPUT_FORMAT": alias, "AIDA_AGENT_OUTPUT": agent})
                           for exe in [builtin, porcelain]]
                expected = invoke(builtin, ["flow-data", *args[1:], "--format", canonical])
                for result in outputs:
                    assert result.returncode == expected.returncode != 0
                    assert result.stdout == expected.stdout, [result.stdout, expected.stdout]
                    assert result.stderr == expected.stderr, [result.stderr, expected.stderr]
                # An explicit format must override the environment alias for
                # validation failures and failures returned by the reader.
                opposite = "toon" if canonical == "human" else "human"
                explicit_args = [*args, "--format", opposite]
                expected = invoke(builtin, ["flow-data", *explicit_args[1:]])
                for exe in [builtin, porcelain]:
                    result = invoke(exe, explicit_args,
                                    {"AIDA_OUTPUT_FORMAT": alias, "AIDA_AGENT_OUTPUT": agent})
                    assert result.returncode == expected.returncode != 0
                    assert result.stdout == expected.stdout, [result.stdout, expected.stdout]
                    assert result.stderr == expected.stderr, [result.stderr, expected.stderr]
        check_alias_errors(["--since", "bad"])
        check_alias_errors(["--since", "2026-11-10", "--until", "2026-11-09"])
        # A missing history ref fails inside the reader, after argument parsing.
        git("update-ref", "-d", "refs/remotes/origin/main")
        before = snapshot()
        for suffix in [[], ["--format", "human"], ["--format", "toon"], ["--format", "json"]]:
            args = ["flow", "report", "--since", "2026-11-09", *suffix]
            outputs = [invoke(exe, args) for exe in [builtin, porcelain]]
            assert outputs[0].returncode != 0
            assert outputs[0].returncode == outputs[1].returncode
            assert outputs[0].stdout == outputs[1].stdout, [p.stdout for p in outputs]
            assert outputs[0].stderr == outputs[1].stderr, [p.stderr for p in outputs]
        check_alias_errors(["--since", "2026-11-09"])
        assert before == snapshot(), "report changed fixture state"
    print("PASS: core/external/porcelain text and JSON parity; fixture state unchanged")


if __name__ == "__main__":
    main()
