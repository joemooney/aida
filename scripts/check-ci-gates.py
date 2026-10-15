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
import shlex
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

# A DERIVED gate is a local-only reshaping of one CI step, used where running
# the step verbatim is the wrong cost. Its tier is never part of `all`: the
# step it derives from already runs there. trace:TASK-1763 | ai:claude
DERIVED_TIERS = ("compile",)

# `make check-ci-compile` compiles every test target without running one. It is
# derived from CI's `Run tests` step rather than written out, so the flags that
# select WHAT is compiled (`--workspace`, features, packages) can only come from
# CI. Two things are local policy and nothing else:
#   - the agent profile, which every worktree already builds (`aida dev build`),
#     built incrementally with sccache off, exactly as that command builds it;
#   - dropping the run-only parts: `--no-fail-fast` and the stdin redirect.
# Anything else in that step (a pipe, a second command, a different test
# runner) is a shape this derivation does not understand, and it refuses rather
# than compile something CI does not.
COMPILE_LOCAL_ARGS = ["--no-run", "--profile", "agent"]
RUN_ONLY_ARGS = {"--no-fail-fast"}
RUN_ONLY_REDIRECT = ["<", "/dev/null"]


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


def load_tiers(root: Path) -> tuple[dict[str, str], dict[str, str], dict[str, str]]:
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
    derived = table.get("derived", {})
    bad = sorted(name for name, tier in tiers.items() if tier not in TIERS)
    if bad:
        raise GateError(
            "ci-gate-tiers.toml: tier must be one of "
            f"{', '.join(TIERS)}; got something else for: {', '.join(bad)}"
        )
    return tiers, skips, derived


def derive_compile_command(source_name: str, source_command: str) -> str:
    """CI's `cargo test` step, reshaped to compile every test target and run none.

    trace:TASK-1763 | ai:claude
    """
    try:
        argv = shlex.split(source_command.strip())
    except ValueError as err:
        raise GateError(f"cannot derive a compile gate from {source_name!r}: {err}")

    if argv[-2:] == RUN_ONLY_REDIRECT:
        argv = argv[:-2]
    if (
        "\n" in source_command.strip()
        or argv[:2] != ["cargo", "test"]
        or any(token in {"|", "||", "&&", ";", "<", ">", "&"} for token in argv)
    ):
        raise GateError(
            f"CI's {source_name!r} step is no longer a single `cargo test ...` command "
            f"(it is `{source_command.strip()}`), so the compile gate cannot be derived "
            "from it. Update derive_compile_command in scripts/check-ci-gates.py."
        )
    if "--" in argv:
        # Arguments after `--` go to the test binaries, which never run here.
        argv = argv[: argv.index("--")]
    clashes = [token for token in argv if token in COMPILE_LOCAL_ARGS or token.startswith("--profile")]
    if clashes:
        raise GateError(
            f"CI's {source_name!r} step already passes {' '.join(clashes)}; the compile "
            "gate's local profile would conflict with it. Update derive_compile_command."
        )
    kept = [token for token in argv if token not in RUN_ONLY_ARGS]
    return shlex.join(kept + COMPILE_LOCAL_ARGS)


def load_derived(
    steps: list[tuple[str, str]], tiers: dict[str, str], derived: dict[str, str]
) -> list[tuple[str, str, str, str]]:
    """`(name, tier, source step, command)` for each derived gate."""
    commands = dict(steps)
    gates = []
    for name, source in derived.items():
        if name in commands:
            raise GateError(f"derived gate {name!r} shadows a CI step of the same name; rename it")
        if source not in commands:
            raise GateError(
                f"derived gate {name!r} derives from {source!r}, which is not a step in CI's "
                "Build job. A renamed CI step must be renamed here too."
            )
        if tiers.get(source) != "full":
            raise GateError(f"derived gate {name!r} must derive from a `full` CI gate")
        gates.append((name, "compile", source, derive_compile_command(source, commands[source])))
    return gates


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


# trace:BUG-2135 | ai:antigravity
def sccache_server_status() -> tuple[bool, str | None]:
    """Check if any running sccache process has a deleted TMPDIR and whether any sccache is alive."""
    proc = Path(os.environ.get("AIDA_SCCACHE_PROC_DIR", "/proc"))
    if not proc.is_dir():
        return False, None
    pids_env = os.environ.get("AIDA_SCCACHE_PIDS")
    if pids_env:
        candidates = [proc / p for p in pids_env.split() if p.isdigit()]
    else:
        try:
            candidates = [p for p in proc.iterdir() if p.name.isdigit()]
        except OSError:
            return False, None

    is_running = False
    missing_tmp = None
    for pid_dir in candidates:
        try:
            comm_file = pid_dir / "comm"
            if comm_file.exists() and comm_file.read_text().strip() != "sccache":
                continue
            is_running = True
            raw_env = (pid_dir / "environ").read_bytes()
            for entry in raw_env.split(b"\0"):
                if entry.startswith(b"TMPDIR="):
                    tmp = entry[7:].decode("utf-8", errors="replace")
                    if tmp and not os.path.isdir(tmp):
                        missing_tmp = tmp
        except (OSError, UnicodeDecodeError):
            continue
    return is_running, missing_tmp


def sccache_missing_tmpdir() -> str | None:
    return sccache_server_status()[1]


def ensure_sccache_server() -> None:
    """Ensure sccache is running with a stable TMPDIR, pre-starting or restarting if broken."""
    if "RUSTC_WRAPPER" in os.environ:
        wrapper = os.environ["RUSTC_WRAPPER"].strip()
        if not wrapper or "sccache" not in wrapper:
            return
    if os.environ.get("AIDA_SCCACHE_DISABLE") == "1":
        return
    sccache_bin = os.environ.get("AIDA_SCCACHE") or shutil.which("sccache")
    if not sccache_bin or not os.path.isfile(sccache_bin) or not os.access(sccache_bin, os.X_OK):
        return
    is_running, missing = sccache_server_status()
    stable_tmp = os.environ.get("AIDA_SCCACHE_TMPDIR", "/tmp")
    env = dict(os.environ, TMPDIR=stable_tmp)
    if missing:
        subprocess.run([sccache_bin, "--stop-server"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run([sccache_bin, "--start-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    elif not is_running:
        subprocess.run([sccache_bin, "--start-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


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
        choices=["fast", "full", "all", *DERIVED_TIERS],
        default="all",
        help="fast = no cargo build, cheap enough before every push; "
        "all = every gate that can run locally (default); "
        "compile = compile every test target, run none (after merging main)",
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
        tiers, skips, derived = load_tiers(root)
        derived_gates = load_derived(steps, tiers, derived)
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

    if args.tier in DERIVED_TIERS:
        plan = [(name, command) for name, tier, _, command in derived_gates if tier == args.tier]
        tiers = {**tiers, **{name: tier for name, tier, _, _ in derived_gates}}
        if args.list:
            for name, tier, source, command in derived_gates:
                print(f"PLAN  {name}  [{tier}]  (from CI's {source!r}: {command})")
            return 0
        return run_plan(root, args.tier, plan, tiers, compile_env())

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
        # Derived gates are not part of `all`, but a listing that hid them would
        # make `make check-ci-compile` undiscoverable, which is the defect the
        # tier table exists to remove.
        for name, tier, source, command in derived_gates:
            print(
                f"ALSO  {name}  [{tier}]  `make check-ci-{tier}` only\n"
                f"        derived from CI's {source!r}: {command}"
            )
        return 0

    ensure_sccache_server()
    with contextlib.ExitStack() as stack:
        env = dict(os.environ)
        if args.tier == "fast":
            env["PATH"] = aida_stub_dir(root, stack) + os.pathsep + env.get("PATH", "")
        return run_plan(root, args.tier, plan, tiers, env)


def compile_env() -> dict[str, str]:
    """The environment for the compile tier: the `aida dev build` convention.

    Incremental compilation, with the sccache wrapper disabled because sccache
    refuses incremental units (BUG-1684) -- the same pair `make build-fast` and
    `aida dev build` use, so this shares their incremental cache in
    target/agent. This gate is re-run after every merge of main, and almost
    every merge recompiles aida-cli-lib; incremental is what keeps that re-run
    cheap. trace:TASK-1763 | ai:claude
    """
    env = dict(os.environ)
    env["RUSTC_WRAPPER"] = ""
    env["CARGO_INCREMENTAL"] = "1"
    return env


def run_plan(
    root: Path, tier: str, plan: list[tuple[str, str]], tiers: dict[str, str], env: dict[str, str]
) -> int:
    branch = default_branch(root)
    for name, command in plan:
        try:
            resolved = substitute(command, branch)
        except GateError as err:
            print(f"FAIL  {name}\n        {err}", file=sys.stderr)
            return 1
        print(f"RUN   {name}  [{tiers[name]}]", flush=True)
        passed, seconds = run_gate(root, name, resolved, env)
        if not passed:
            sccache_active = True
            wrapper_val = env.get("RUSTC_WRAPPER", os.environ.get("RUSTC_WRAPPER"))
            if wrapper_val is not None:
                wrapper_val = wrapper_val.strip()
                sccache_active = bool(wrapper_val and "sccache" in wrapper_val)
            if sccache_active:
                missing = sccache_missing_tmpdir()
                if missing:
                    print(
                        f"check-ci-gates: sccache server temp directory ({missing}) is missing.\n"
                        f"        hint: restart sccache with: sccache --stop-server; TMPDIR=/tmp sccache --start-server",
                        file=sys.stderr,
                    )
            # Exit on the FIRST failure: CI's order is the cheapest order,
            # and a second failure after the first is noise.
            origin = (
                "a local derivation of a CI step; see `make check-ci-list`."
                if tier in DERIVED_TIERS
                else f"this is CI's own `{name}` step, run locally."
            )
            print(f"FAIL  {name}  ({seconds:.1f}s)\n        {origin}", file=sys.stderr)
            return 1
        print(f"ok    {name}  ({seconds:.1f}s)")

    print(f"check-ci-gates: {len(plan)} gate(s) passed ({tier} tier).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
