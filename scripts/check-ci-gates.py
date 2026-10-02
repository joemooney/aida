#!/usr/bin/env python3
"""Run CI's Build-job gates locally, in CI's own order.

CI's Build job runs far more gates than the local check set documented in
CLAUDE.md. Every gate an agent cannot run locally costs a full CI cycle plus a
re-push, and on a constrained host that is the dominant cost of a fix.

This runner carries NO gate commands. It reads each `run:` body out of
`.github/workflows/ci.yml`, so a gate whose command changes cannot drift away
from its local runner. The only local file it consults is
`scripts/ci-gate-tiers.toml`, which records the one judgement a workflow cannot
express: whether a gate can run here, and whether it needs a cargo build.

A `run:` step missing from that table is a hard failure, not a silent omission.
That drift is the root cause this exists to close.

trace:TASK-1555 | ai:claude
"""

from __future__ import annotations

import argparse
import contextlib
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

import yaml

# GitHub Actions invokes Bash on Linux as
# `bash --noprofile --norc -e -o pipefail {0}`. Keep this argv in parity with
# `github_actions_bash` in aida-cli-lib/src/implementer_preflight.rs so a
# command CI rejects cannot pass here. trace:BUG-1420
CI_SHELL_CONTRACT = "bash --noprofile --norc -e -o pipefail {0}"
CI_SHELL_ARGV = ["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c"]

BASE_REF_PLACEHOLDER = "${{ github.base_ref }}"
EVENT_NAME_PLACEHOLDER = "${{ github.event_name }}"

# A git ref name reaches the gate text through `bash -c`, and ref names may
# contain `;`, `$()`, backticks and quotes. Substitute a branch name only when
# it is made of shell-inert characters; otherwise refuse the gates that need it
# rather than run something else. Same policy as BUG-1624 on the Rust side.
SHELL_INERT_BRANCH = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$")

TIERS = ("fast", "full")


class GateError(RuntimeError):
    """A problem with the gate definitions themselves, not with the tree."""


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def load_ci_steps(root: Path) -> list[tuple[str, str]]:
    """Every named `run:` step of the Build job, in CI's order."""
    path = root / ".github/workflows/ci.yml"
    try:
        workflow = yaml.safe_load(path.read_text())
    except FileNotFoundError:
        raise GateError(f"{path} is missing; there is no CI contract to mirror")
    except yaml.YAMLError as err:
        raise GateError(f"{path} is unreadable: {err}")

    build = (workflow or {}).get("jobs", {}).get("build")
    if not build:
        raise GateError(f"{path} has no `jobs.build`; nothing to mirror")

    shell = build.get("defaults", {}).get("run", {}).get("shell")
    if shell != CI_SHELL_CONTRACT:
        # Local parity is a claim about CI's shell. If that contract moved, the
        # claim is false, so refuse rather than report a green that means
        # nothing.
        raise GateError(
            "CI Bash contract drifted: expected "
            f"`{CI_SHELL_CONTRACT}`, found `{shell or '<missing>'}`"
        )

    return [
        (step["name"], step["run"])
        for step in build.get("steps", [])
        if step.get("run") and step.get("name")
    ]


def load_tiers(root: Path) -> tuple[dict[str, str], dict[str, str]]:
    path = root / "scripts/ci-gate-tiers.toml"
    try:
        with path.open("rb") as handle:
            table = tomllib.load(handle)
    except FileNotFoundError:
        raise GateError(f"{path} is missing; no gate is classified")
    except tomllib.TOMLDecodeError as err:
        raise GateError(f"{path} is unreadable: {err}")

    tiers = table.get("tiers", {})
    skips = table.get("skips", {})
    bad = sorted(name for name, tier in tiers.items() if tier not in TIERS)
    if bad:
        raise GateError(
            "ci-gate-tiers.toml: tier must be one of "
            f"{', '.join(TIERS)}; got something else for: {', '.join(bad)}"
        )
    return tiers, skips


def audit(steps: list[tuple[str, str]], tiers: dict[str, str], skips: dict[str, str]) -> list[str]:
    """Report every way the tier table and CI can disagree, in both directions."""
    problems: list[str] = []
    ci_names = [name for name, _ in steps]
    classified = set(tiers) | set(skips)

    for name in ci_names:
        if name not in classified:
            problems.append(
                f"CI gate is not classified: {name!r}\n"
                "    Add it to [tiers] (fast|full) in scripts/ci-gate-tiers.toml, or to\n"
                "    [skips] with a one-line reason it cannot run locally."
            )
    for name in sorted(classified):
        if name not in ci_names:
            problems.append(
                f"classified gate is not in CI's Build job: {name!r}\n"
                "    A renamed or deleted CI step leaves a stale entry here, which is how a\n"
                "    gate stops being run locally without anything failing. Fix the name."
            )
    both = sorted(set(tiers) & set(skips))
    for name in both:
        problems.append(f"gate is both tiered and skipped: {name!r}; it must be one or the other")
    return problems


def default_branch(root: Path) -> str | None:
    """The default branch, or None when it is not safe to substitute."""
    try:
        out = subprocess.run(
            ["git", "symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    name = out.stdout.strip().removeprefix("origin/") if out.returncode == 0 else ""
    if not name:
        name = "main"
    return name if SHELL_INERT_BRANCH.match(name) else None


def substitute(command: str, branch: str | None) -> str:
    command = command.replace(EVENT_NAME_PLACEHOLDER, "pull_request")
    if BASE_REF_PLACEHOLDER in command:
        if branch is None:
            raise GateError(
                "this gate needs the base branch, and the resolved default branch name is "
                "not shell-inert; refusing to substitute it"
            )
        command = command.replace(BASE_REF_PLACEHOLDER, branch)
    return command


def aida_stub_dir(root: Path, stack) -> str:
    """A directory whose `aida` fails loudly, for the no-build tier.

    The fast tier promises no cargo build. `implementer_preflight` deliberately
    refuses to decide "does this gate need the binary?" from the gate's text,
    because a gate can reach `aida` through a script it calls (BUG-1420) — so it
    builds the worktree binary before any gate runs.

    A pre-push tier cannot pay for that build, and consulting whatever `aida` is
    on PATH would answer from a stale install, which is the defect BUG-1420
    exists to prevent. So instead of guessing, this ENFORCES the classification:
    a gate tiered `fast` that resolves `aida` through PATH gets a stub that exits
    non-zero and says so. Mis-classification fails loudly instead of lying.

    This covers PATH lookup only. A gate that runs `target/debug/aida` by path
    walks past the stub -- but it then fails on the absent file, which is also
    loud, and that is how "Monitor contract drift guard" was caught and moved to
    `full`. Neither route can produce a false pass; the stub only makes the
    PATH route say WHY.
    """
    directory = stack.enter_context(tempfile.TemporaryDirectory(prefix="aida-fast-tier-"))
    stub = Path(directory) / "aida"
    stub.write_text(
        "#!/usr/bin/env bash\n"
        "echo \"check-ci-gates: this gate invoked \\`aida\\`, so it is not a no-build\" >&2\n"
        "echo \"  gate. Move it from fast to full in scripts/ci-gate-tiers.toml.\" >&2\n"
        "exit 1\n"
    )
    stub.chmod(stub.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return directory


def run_gate(root: Path, name: str, command: str, env: dict[str, str]) -> tuple[bool, float]:
    started = time.monotonic()
    completed = subprocess.run([*CI_SHELL_ARGV, command], cwd=root, env=env)
    return completed.returncode == 0, time.monotonic() - started


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="check-ci-gates.py",
        description="Run CI's Build-job gates locally, in CI's own order.",
    )
    parser.add_argument(
        "--tier",
        choices=["fast", "full", "all"],
        default="all",
        help="fast = no cargo build, cheap enough before every push; "
        "all = every gate that can run locally (default)",
    )
    parser.add_argument(
        "--audit",
        action="store_true",
        help="check that every CI gate is classified and exit; run nothing",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="print the resolved plan and exit; run nothing",
    )
    args = parser.parse_args(argv)

    root = repo_root()
    try:
        steps = load_ci_steps(root)
        tiers, skips = load_tiers(root)
    except GateError as err:
        print(f"check-ci-gates: {err}", file=sys.stderr)
        return 2

    problems = audit(steps, tiers, skips)
    if problems:
        print(
            f"check-ci-gates: the gate table and CI disagree ({len(problems)} problem(s)):",
            file=sys.stderr,
        )
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    if args.audit:
        print(f"check-ci-gates: all {len(steps)} CI Build gates are classified.")
        return 0

    wanted = TIERS if args.tier == "all" else (args.tier,)
    plan = [(name, command) for name, command in steps if tiers.get(name) in wanted]

    # Every skip is printed with its reason, naming the gate. A skip nobody sees
    # is how the documented check set came to omit ~25 gates.
    for name, _ in steps:
        if name in skips:
            print(f"SKIP  {name}\n        cannot run locally: {skips[name]}")
        elif tiers[name] not in wanted:
            print(f"SKIP  {name}\n        not in the {args.tier} tier (it is {tiers[name]})")

    if args.list:
        for name, _ in plan:
            print(f"PLAN  {name}  [{tiers[name]}]")
        return 0

    branch = default_branch(root)
    with contextlib.ExitStack() as stack:
        env = dict(os.environ)
        if args.tier == "fast":
            env["PATH"] = aida_stub_dir(root, stack) + os.pathsep + env.get("PATH", "")

        for name, command in plan:
            try:
                resolved = substitute(command, branch)
            except GateError as err:
                print(f"FAIL  {name}\n        {err}", file=sys.stderr)
                return 1
            print(f"RUN   {name}  [{tiers[name]}]", flush=True)
            passed, seconds = run_gate(root, name, resolved, env)
            if not passed:
                # Exit on the FIRST failure: CI's order is the cheapest order,
                # and a second failure after the first is noise.
                print(
                    f"FAIL  {name}  ({seconds:.1f}s)\n"
                    f"        this is CI's own `{name}` step, run locally.",
                    file=sys.stderr,
                )
                return 1
            print(f"ok    {name}  ({seconds:.1f}s)")

    print(f"check-ci-gates: {len(plan)} gate(s) passed ({args.tier} tier).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
