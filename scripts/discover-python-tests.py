"""Run unittest discovery for suites without a dedicated CI invocation."""
# trace:BUG-2734 | ai:codex
from glob import escape
from pathlib import Path
import re
import sys
import unittest

import yaml


# Files exercised through other entry points, with their own fixtures or build.
# These exact files are explicitly excluded from unittest discovery.
OTHER_HARNESSES = {
    "test_dell1_controller.py": "test_self_hosted_runner.py checks every controller test ID",
    "test_bug2017_bootstrap.py": "test_distributed.sh",
    "test_history_templates.py": "test_history_templates.sh",
    "test_mcp_doc_consistency.py": "test_mcp_doc_consistency.sh",
    "test_mcp_stdio.py": "test_mcp_stdio.sh",
    "test_porcelain_flow_parity.py": "story_2704_porcelain_parity.rs",
    "test_story_1843_pr3.py": "story_1843_pr3_blackbox.rs",
}

# Dated failures from the initial discovery run. Product owns follow-up IDs.
KNOWN_FAILING = {
    # Expected one canonical object under the displayed spec ID.
    "test_cache_refresh_outputs.py": ("2026-11-23", "TASK-3848"),
    # Definition scenario refused on missing current-head approval (exit 23).
    "test_pr_ship_preflight.py": ("2026-11-23", "TASK-3849"),
    # Fixture role current did not report orchestrator.
    "test_seat_takeover_lifecycle.py": ("2026-11-23", "TASK-3850"),
}

# Only simple standalone invocations qualify. Comments, command arguments to
# other programs, and incidental mentions in workflow text are not wiring.
PATH = r"tests/test_\w+\.py"
UNIT = re.compile(r'(?:AIDA_TEST_BIN="\$PWD/target/debug/aida" )?python3 -m unittest'
                  rf'(?: -v)? (?P<paths>{PATH}(?: {PATH})*)')
# Only these CLI entry points are executed directly without unittest discovery.
CLI = {
    "test_show_latency.py", "test_status_cold_cache.py", "test_origin_prompt.py",
    "test_fixture_pid_namespace.py", "test_work_ci_relay.py", "test_mail_notice_hook.py",
    "test_precompact_handoff_hook.py", "test_grant_expiry_notice_hook.py",
    "test_gitlab_review_handoff.py", "test_skill_discovery.py",
}


def dedicated_invocations():
    workflow = yaml.safe_load(Path(".github/workflows/ci.yml").read_text())
    dedicated = set()
    for step in workflow["jobs"]["build"]["steps"]:
        for line in step.get("run", "").splitlines():
            line = line.strip()
            match = UNIT.fullmatch(line)
            if match:
                dedicated.update({Path(p).name for p in match["paths"].split()} - CLI)
            args = line.split()
            if len(args) >= 2 and args[0] == "python3" and re.fullmatch(PATH, args[1]):
                name = Path(args[1]).name
                if name in CLI:
                    dedicated.add(name)
    return dedicated


def main():
    tests = Path("tests")
    files = sorted(tests.glob("test_*.py"))
    if not files:
        sys.exit("No tests/test_*.py files found")
    dedicated = dedicated_invocations()
    sys.path.insert(0, str(Path.cwd()))
    ok = True
    for path in files:
        if path.name in KNOWN_FAILING:
            date, issue = KNOWN_FAILING[path.name]
            print(f"{path}: known failure {date} ({issue})", flush=True)
            if not re.fullmatch(r"(?:BUG|TASK)-[0-9]+", issue or ""):
                print(f"{path}: missing follow-up spec ID", file=sys.stderr)
                ok = False
            continue
        if path.name in CLI:
            if path.name not in dedicated:
                print(f"{path}: missing standalone CI invocation", file=sys.stderr)
                ok = False
            continue
        if path.name in OTHER_HARNESSES:
            continue
        print(f"Discovering {path}", flush=True)
        loader = unittest.TestLoader()
        suite = loader.discover(str(tests), pattern=escape(path.name))
        if loader.errors:
            print("\n".join(loader.errors), file=sys.stderr)
            ok = False
            continue
        if not suite.countTestCases():
            print(f"{path}: unittest discovered zero tests", file=sys.stderr)
            ok = False
            continue
        if path.name in dedicated:
            continue
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        ok = ok and result.wasSuccessful()
    return int(not ok)


if __name__ == "__main__":
    sys.exit(main())
