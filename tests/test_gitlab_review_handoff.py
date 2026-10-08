#!/usr/bin/env python3
"""Isolated CLI reproduction of MR-35 handoff (and GitHub parity).
trace:BUG-1807 | ai:codex
Run: python3 tests/test_gitlab_review_handoff.py --aida /path/to/aida
"""
import argparse
import json
import os
from pathlib import Path
import re
import tempfile

from test_cache_refresh_outputs import require, run


def check(binary, forge, fail_first=False):
    with tempfile.TemporaryDirectory(prefix="aida-review-handoff-") as temp:
        root, home, bins = (Path(temp) / n for n in ("project", "home", "bin"))
        for path in (root, home, bins):
            path.mkdir()
        # Init/automatic maintenance must never reach the user's service manager.
        service = bins / "systemctl"
        service.write_text("#!/bin/sh\nexit 1\n")
        service.chmod(0o755)
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("AIDA_", "GIT_", "REQ_", "XDG_"))}
        env.update(HOME=str(home), AIDA_HOME=str(home),
                   XDG_CONFIG_HOME=str(home / ".config"),
                   XDG_RUNTIME_DIR=str(home / "runtime"),
                   GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null",
                   GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
                   GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid",
                   AIDA_TELEMETRY="0", AIDA_USER="fixture", USER="fixture",
                   AIDA_TEST_GLAB_BINARY=str(bins / "glab"),
                   AIDA_TEST_GH_BINARY=str(bins / "gh"), PATH=str(bins) + os.pathsep + env["PATH"])
        def git(*args):
            return require(run(["git", *args], root, env))
        def aida(*args):
            return require(run([binary, *args], root, env))
        git("init", "-q", "-b", "main")
        git("commit", "--allow-empty", "-qm", "base")
        aida("init", "--no-skills", "--no-hooks")
        added = aida("add", "--type", "bug", "--title", "handoff fixture")
        spec = re.search(r"BUG-\d+(?:-\d+)?", added.stdout).group()
        git("checkout", "-qb", "bug-fixture")
        git("commit", "--allow-empty", "-qm", f"fix: fixture ({spec})")
        # URL only selects the forge. Fake CLIs answer every request, and no
        # command fetches/pushes this unreachable origin.
        git("remote", "add", "origin", f"https://{forge}.invalid/fixture/repo.git")
        config = root / ".aida/config.toml"
        config.write_text(config.read_text().replace('provider = "pure-git"', f'provider = "{forge}"'))
        noun = "MR" if forge == "gitlab" else "PR"
        mr = dict(iid=35, source_branch="bug-fixture", target_branch="main",
                  title="fixture", state="opened", web_url="https://gitlab.invalid/fixture/-/merge_requests/35")
        pr = dict(number=35, headRefName="bug-fixture", baseRefName="main",
                  title="fixture", state="OPEN", url="https://github.invalid/fixture/pull/35")
        for cli in ("glab", "gh"):
            script = bins / cli
            # Permit only the expected read operations; a wrong forge fails.
            script.write_text("#!/usr/bin/env python3\nimport json,sys\na=sys.argv[1:]\n" +
                (f"obj={repr(mr)}\nif a[:1]==['api']:\n print(json.dumps(obj if any('/merge_requests/35' in x for x in a) else [obj]))\nelse: sys.exit(91)\n"
                 if cli == "glab" and forge == "gitlab" else
                 f"obj={repr(pr)}\nif a[:2]==['pr','list']: print('35\\tfixture\\thttps://github.invalid/fixture/pull/35\\tbug-fixture')\nelif a[:2]==['pr','view']: print(json.dumps(obj))\nelse: sys.exit(92)\n"
                 if cli == "gh" and forge == "github" else "sys.exit(93)\n"))
            script.chmod(0o755)
        queue = root / ".aida-store/registry/queues/fixture.yaml"
        if fail_first:
            queue.parent.mkdir(parents=True, exist_ok=True)
            queue.write_text("[invalid yaml\n")
            first = run([binary, "pr", "auto-queue-review", "--branch", "bug-fixture"], root, env)
            assert first.returncode != 0, (first.stdout, first.stderr)
            assert "queue insertion failed" in first.stderr, first.stderr
            assert "→ reviewer queue" not in first.stdout
            queue.unlink()  # fixture recovery; permanent lock sidecar stays
        else:
            first = aida("pr", "auto-queue-review", "--branch", "bug-fixture")
            assert f"reviewer queue ({noun}-35)" in first.stdout, (first.stdout, first.stderr)
        objects = list((root / ".aida-store/objects").rglob("*.yaml"))
        stories = [p for p in objects if f"Review {noun}-35:" in p.read_text()]
        assert len(stories) == 1, stories
        story = stories[0].stem
        # The creation gate must reject an incomplete canonical inventory even
        # when its stale cache still holds the valid pre-corruption story.
        # trace:BUG-1807 | ai:codex
        original = stories[0].read_bytes()
        def refused_inventory(label):
            before = {p: p.read_bytes() for p in objects if p != stories[0]}
            result = run([binary, "pr", "auto-queue-review", "--branch", "bug-fixture"], root, env)
            output = result.stdout + result.stderr
            assert result.returncode != 0, (label, output)
            assert "cannot read review stories" in output, (label, output)
            assert "reviewer handoff not confirmed" in output, (label, output)
            assert "→ reviewer queue" not in output, (label, output)
            assert "reuses canonical" not in output, (label, output)
            assert "✓ filed" not in output, (label, output)
            assert f"aida queue work {noun}-35 --role reviewer" not in output
            assert set((root / ".aida-store/objects").rglob("*.yaml")) == set(objects)
            assert all(p.read_bytes() == data for p, data in before.items())
            print(f"PASS {noun} {label}: exit={result.returncode}, objects={len(objects)} unchanged", flush=True)
            print(output, flush=True)

        malformed = original + b"\ninvalid: [unterminated\n"
        stories[0].write_bytes(malformed)
        refused_inventory("malformed canonical story")
        assert stories[0].read_bytes() == malformed
        stories[0].write_bytes(original)

        # chmod is not evidence of unreadability under root. Verify denial,
        # otherwise explicitly skip this permission-specific scenario.
        mode = stories[0].stat().st_mode
        try:
            stories[0].chmod(0)
            try:
                stories[0].read_bytes()
            except PermissionError:
                refused_inventory("unreadable canonical story")
            else:
                print("SKIP permission denial: this user can read chmod-000 files", flush=True)
        finally:
            stories[0].chmod(mode)
        assert stories[0].read_bytes() == original

        objects_dir = root / ".aida-store/objects"
        mode = objects_dir.stat().st_mode
        try:
            objects_dir.chmod(0)
            try:
                list(objects_dir.iterdir())
            except PermissionError:
                result = run([binary, "pr", "auto-queue-review", "--branch", "bug-fixture"], root, env)
                output = result.stdout + result.stderr
                assert result.returncode != 0, output
                assert "reviewer handoff not confirmed" in output, output
                assert "→ reviewer queue" not in output and "reuses canonical" not in output
                print(f"PASS {noun} unreadable objects directory: exit={result.returncode}", output, flush=True)
            else:
                print("SKIP directory denial: this user can read chmod-000 directories", flush=True)
        finally:
            objects_dir.chmod(mode)
        assert set(objects_dir.rglob("*.yaml")) == set(objects)
        assert stories[0].read_bytes() == original
        second = aida("pr", "auto-queue-review", "--branch", "bug-fixture")
        assert f"reuses canonical review story {story}" in second.stdout, second.stdout
        assert len(list((root / ".aida-store/objects").rglob("*.yaml"))) == len(objects)
        # Corruption must fail closed, including the existing-story path.
        queues = list((root / ".aida-store/registry/queues").glob("*.yaml"))
        assert len(queues) == 1, queues
        queues[0].write_text("[invalid yaml\n")
        failed = run([binary, "pr", "auto-queue-review", "--branch", "bug-fixture"], root, env)
        assert failed.returncode != 0, (failed.stdout, failed.stderr)
        assert "queue insertion failed" in failed.stderr, failed.stderr
        assert "reuses canonical" not in failed.stdout
        assert "→ reviewer queue" not in failed.stdout
        print(f"PASS {noun}-35 creation, existing recognition, queue failure (initial failure={fail_first})", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aida", type=Path, required=True)
    binary = parser.parse_args().aida.resolve()
    for forge in ("gitlab", "github"):
        check(binary, forge)

    check(binary, "gitlab", fail_first=True)
