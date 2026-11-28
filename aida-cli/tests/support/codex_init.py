"""Exercise interactive init and launch with an isolated home and a recording Codex."""
# trace:BUG-2752 | ai:codex
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import tomllib


def exercise(root, aida):
    repo, home = root / "repo", root / "home"
    repo.mkdir()
    home.mkdir()
    env = dict(HOME=str(home), USER="fixture", PATH=f"{root}:{os.environ['PATH']}",
               TERM="dumb", NO_COLOR="1", AIDA_TELEMETRY="0")
    vendor = root / "codex"
    vendor.write_text('#!/bin/sh\n[ "$1" = --version ] && { echo "codex-cli 1.0"; exit; }\n'
                      'case " $* " in *" --probe-seat "*) while [ "$1" != -- ]; do shift; done; shift; exec "$@";; esac\n'
                      'printf "%s\\n" "$@" > "$HOME/launch"\n')
    vendor.chmod(0o755)

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=repo, env=env, text=True)

    def terminal(*args):
        # script gives inline-grant issuance a controlling terminal.
        argv = ["timeout", "45", "script", "-qec", shlex.join([aida, *args]), "/dev/null"]
        with subprocess.Popen(argv, cwd=repo, env=env, stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
            output = pending = b""
            while chunk := child.stdout.read1(8192):
                output += chunk
                pending += chunk
                if re.search(rb"\[(?:[yY]/[nN](?:/\?)?|1)\][:?]? *$", pending):
                    prompt = pending.splitlines()[-1]
                    answer = (b"y" if b'codex = "native"' in prompt or b"one-time grant" in prompt
                              else b"2" if b"Choose [1]" in prompt else b"")
                    child.stdin.write(answer + b"\n")
                    child.stdin.flush()
                    pending = b""
            assert child.wait(timeout=5) == 0, output.decode(errors="replace")
            return output.decode()

    git("init", "-qb", "main")
    git("config", "user.email", "fixture@example.org")
    git("config", "user.name", "fixture")
    git("commit", "-q", "--allow-empty", "-m", "initial")
    init = ("init", "--footprint", "full", "--agent", "codex", "--node-name", "fixture",
            "--no-hooks", "--no-skills", "--no-post-hooks", "--no-schedule")
    explanation = "Codex native sandbox opt-in written. Codex uses its native sandbox, not AIDA containment"
    result = terminal(*init)
    assert explanation in result, result
    path = ".aida/config.toml"
    assert tomllib.loads(git("show", f"main:{path}"))["contained"]["vendors"]["codex"] == "native"
    terminal("team", "allow-seat", "fixture", "advisor")
    terminal("agent", "new", "codex", "--role", "advisor", "--description", "fixture", "--no-resume", "--no-mail-wake", "--no-prompt")
    assert "--sandbox\nworkspace-write\n" in (home / "launch").read_text()

    # Remove the committed opt-in to exercise ordinary distributed re-init.
    config = repo / path
    body, count = re.subn(r'(?m)^codex = "native"$', "", config.read_text())
    assert count == 1
    config.write_text(body)
    git("commit", "-qm", "remove native policy", "--", path)
    git("switch", "-qc", "topic")
    git("commit", "-q", "--allow-empty", "-m", "topic tip")
    result = terminal(*init)
    assert "trusted default branch (main)" in result and "Opt Codex into" not in result, result
    assert "native" != tomllib.loads(config.read_text())["contained"]["vendors"].get("codex")
    git("switch", "-q", "main")
    unpublished_base = git("rev-parse", "main").strip()
    result = terminal(*init)
    assert explanation in result, result
    assert tomllib.loads(git("show", f"main:{path}"))["contained"]["vendors"]["codex"] == "native"

    git("update-ref", "refs/remotes/origin/main", unpublished_base)
    git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main")
    committed = git("rev-parse", "HEAD")
    for state in ("clean", "unstaged", "staged"):
        if state == "unstaged":
            with config.open("a") as stream:
                stream.write("\n# unrelated operator edit\n")
        elif state == "staged":
            git("add", "--", path)
        worktree, index = config.read_bytes(), git("show", f":{path}")
        result = terminal(*init)
        assert "policy committed locally but not pushed: run `git push origin main`" in result, result
        assert "Opt Codex into" not in result and "review and commit .aida/config.toml" not in result, result
        assert config.read_bytes() == worktree and git("show", f":{path}") == index, state
        assert git("rev-parse", "HEAD") == committed, state

    # A valid inline table that the editor declines must not abort role setup.
    config.write_text("contained = {}\n" + re.sub(r"(?ms)^\[contained(?:\.vendors)?\].*?(?=^\[|\Z)", "", config.read_text()))
    git("commit", "-qm", "inline policy", "--", path)
    # Role setup reports only under --verbose (short default output). trace:TASK-3961 | ai:claude
    result = terminal(*init, "--verbose")
    assert "Native sandbox opt-in skipped" in result and "global roles installed" in result, result


with tempfile.TemporaryDirectory() as directory:
    exercise(Path(directory), sys.argv[1])
