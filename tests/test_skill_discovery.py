#!/usr/bin/env python3
"""CLI/MCP skill discovery against isolated project fixtures.

Run: python3 tests/test_skill_discovery.py --aida /path/to/aida
trace:BUG-2388 | ai:codex
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def check(binary):
    with tempfile.TemporaryDirectory(prefix="aida-skill-discovery-") as temp:
        root = Path(temp) / "project"
        root.mkdir()
        home = Path(temp) / "home"
        home.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith(("AIDA_", "GIT_"))}
        env.update(HOME=str(home), AIDA_HOME=str(home),
                   GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
                   GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid",
                   # Human output suppresses the existing store-backed MCP command's
                   # TOON cache footer so render assertions compare prompt bytes.
                   AIDA_TELEMETRY="0", AIDA_OUTPUT_FORMAT="human",
                   XDG_CONFIG_HOME=str(home / "config"),
                   XDG_CACHE_HOME=str(home / "cache"), NO_COLOR="1")
        subprocess.run(["git", "init", "-q", str(root)], check=True, env=env)

        def write(path, text):
            path = root / path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)

        def run(args, expected=0):
            result = subprocess.run([binary, *args], cwd=root, env=env,
                                    capture_output=True, text=True, timeout=30)
            assert result.returncode == expected, (args, result.returncode, result.stdout, result.stderr)
            return result.stdout

        run(["init", "--footprint", "minimal", "--no-skills", "--no-hooks",
             "--no-roles", "--no-agent-config", "--no-post-hooks", "--no-schedule"])
        write(".agents/skills/demo/SKILL.md", "portable")
        write(".agents/skills/demo/SKILL.local.md", "local\n")
        write(".agents/skills/demo/examples/helper.md", "docs/plans/helper-missing.md")
        assert not (root / ".claude").exists()
        for prefix in (["skill"], ["mcp", "skill"]):
            rendered = run([*prefix, "render", "demo"])
            assert rendered == "portable\nlocal\n", (prefix, repr(rendered))
            lint = json.loads(run([*prefix, "lint", "--json"]))
            assert lint["skills_scanned"] == 1, lint
            assert lint["skills_with_plan_refs"] == 0, lint
            write(".agents/skills/demo/SKILL.md", "docs/plans/missing.md\n")
            lint = json.loads(run([*prefix, "lint", "--json"], expected=1))
            assert lint["skills_with_plan_refs"] == 1, lint
            assert "missing.md" in json.dumps(lint), lint
            write(".agents/skills/demo/SKILL.md", "portable")
        print("PASS: Codex-only CLI/MCP render, overlays, lint discovery and missing-plan failure", flush=True)

        write(".claude/skills/demo/SKILL.md", "claude")
        write(".claude/skills/demo.md", "legacy\n")
        write(".claude/skills/demo.local.md", "claude local")
        for prefix in (["skill"], ["mcp", "skill"]):
            rendered = run([*prefix, "render", "demo"])
            assert rendered == "portable\nlocal\n", (prefix, repr(rendered))
            lint = json.loads(run([*prefix, "lint", "--json"]))
            assert lint["skills_scanned"] == 3, lint
        (root / ".agents/skills/demo/SKILL.md").unlink()
        assert run(["skill", "render", "demo"]) == "claude\nclaude local"
        (root / ".claude/skills/demo/SKILL.md").unlink()
        assert run(["mcp", "skill", "render", "demo"]) == "legacy\nclaude local"
        print("PASS: mixed layouts, deterministic precedence and legacy overlays", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--aida", required=True)
    check(str(Path(parser.parse_args().aida).resolve()))
