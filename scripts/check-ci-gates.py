#!/usr/bin/env python3
"""Run CI's Build-job gates locally; fast source gates precede compilation.

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
import codecs
import contextlib
from concurrent.futures import ThreadPoolExecutor
import errno
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

# A local `make check-ci` runs CI's `cargo test` steps on the agent profile.
# Automatic cache seeding builds that profile, while CI's default (debug) would
# rebuild the workspace cold in each worktree and run unoptimized tests: the
# 44-80 min local runs of 2026-10-18. Tests and checks use agent; the
# `cargo build` steps stay debug because later gates run `target/debug/aida`.
# trace:STORY-1853 | ai:claude
LOCAL_TEST_PROFILE = "agent"
# trace:STORY-2640 | ai:codex
CARGO_CHECK_RE = re.compile(r"\bcargo check\b(?![^\n]*--profile)")
CARGO_TEST_RE = re.compile(r"\bcargo test\b(?![^\n]*--profile)")
RUN_ONLY_ARGS = {"--no-fail-fast"}
RUN_ONLY_REDIRECT = ["<", "/dev/null"]
# CI tees the run into a log for the per-binary timings (STORY-1853 S1); a
# compile-only run has nothing to log. trace:STORY-1853 | ai:claude
RUN_ONLY_TEE = ["2>&1", "|", "tee"]
# CI's `Run tests` body: every test lane through one planner, under nextest.
# Locally it runs all lanes on the agent profile, with the same runner.
# trace:TASK-1988 | ai:claude
CI_TEST_LANES = ('python3 scripts/ci-test-shards.py --runner nextest --shard "${{ matrix.shard }}" '
                 '< /dev/null 2>&1 | tee target/test-run.log')


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
    # The workflow's matrix selector covers four disjoint lanes. Compile their
    # workspace union through the very same planner. trace:TASK-1949 | ai:codex
    # Under nextest (TASK-1988) the compile set is the same test targets, so
    # the planner's `cargo test --no-run` still covers it. trace:TASK-1988 | ai:claude
    if source_command.strip() == CI_TEST_LANES:
        return "python3 scripts/ci-test-shards.py --compile --profile agent"
    try:
        argv = shlex.split(source_command.strip())
    except ValueError as err:
        raise GateError(f"cannot derive a compile gate from {source_name!r}: {err}")

    if len(argv) >= 4 and argv[-4:-1] == RUN_ONLY_TEE:
        argv = argv[:-4]
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


def local_test_command(command: str) -> str:
    """CI's step body with Cargo tests and checks on the seeded agent profile."""
    # Resolve the CI-only matrix context to all lanes for explicit local full
    # runs; the compile tier uses --compile instead. trace:TASK-1949 | ai:codex
    if command.strip() == CI_TEST_LANES:
        return command.replace('"${{ matrix.shard }}"', 'all --profile agent')
    command = CARGO_TEST_RE.sub(f"cargo test --profile {LOCAL_TEST_PROFILE}", command)
    return CARGO_CHECK_RE.sub(f"cargo check --profile {LOCAL_TEST_PROFILE}", command)


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
    # The lane wrapper's PID scope belongs to real procfs, not an explicitly
    # injected synthetic process tree used by the detection fixtures.
    # trace:STORY-2640 | ai:codex
    if pids_env and proc == Path("/proc"):
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


# trace:TASK-2772 | ai:claude
SCCACHE_SERVER_TIMEOUT = 15.0


def _sccache_server_cmd(cmd: list[str], env: dict[str, str] | None = None) -> bool:
    """TASK-2772: one bounded sccache server command; False on a timeout or
    failure to run, so a stuck sccache can never hold up the gate runner."""
    try:
        subprocess.run(
            cmd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=SCCACHE_SERVER_TIMEOUT,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return True


def ensure_sccache_server() -> bool:
    """Ensure sccache is running with a stable TMPDIR, pre-starting or restarting if broken.

    Returns False when a start or stop did not finish within
    SCCACHE_SERVER_TIMEOUT (or could not run): the caller then builds without
    sccache. True otherwise, including every opt-out. trace:TASK-2772 | ai:claude
    """
    if "RUSTC_WRAPPER" in os.environ:
        wrapper = os.environ["RUSTC_WRAPPER"].strip()
        if not wrapper or "sccache" not in wrapper:
            return True
    if os.environ.get("AIDA_SCCACHE_DISABLE") == "1":
        return True
    sccache_bin = os.environ.get("AIDA_SCCACHE") or shutil.which("sccache")
    if not sccache_bin or not os.path.isfile(sccache_bin) or not os.access(sccache_bin, os.X_OK):
        return True
    is_running, missing = sccache_server_status()
    stable_tmp = os.environ.get("AIDA_SCCACHE_TMPDIR", "/tmp")
    env = dict(os.environ, TMPDIR=stable_tmp)
    if missing:
        return _sccache_server_cmd([sccache_bin, "--stop-server"]) and _sccache_server_cmd(
            [sccache_bin, "--start-server"], env
        )
    if not is_running:
        return _sccache_server_cmd([sccache_bin, "--start-server"], env)
    return True


# trace:TASK-2772 | ai:claude
SCCACHE_PROBE_TIMEOUT = 15.0


def sccache_wrapper_active(env: dict[str, str]) -> bool:
    """Whether cargo would run rustc through sccache: RUSTC_WRAPPER unset (the
    cargo config may name it) or naming sccache."""
    wrapper = env.get("RUSTC_WRAPPER")
    if wrapper is None:
        return True
    wrapper = wrapper.strip()
    return bool(wrapper) and "sccache" in wrapper


def configured_rustc_wrapper(env: dict[str, str], cwd: Path | None = None) -> str | None:
    """The `build.rustc-wrapper` cargo would use from its config files: the
    nearest `.cargo/config.toml` up from `cwd`, then `$CARGO_HOME/config.toml`.
    trace:TASK-2772 | ai:claude"""
    start = (cwd or Path.cwd()).resolve()
    cargo_home = Path(env.get("CARGO_HOME") or Path.home() / ".cargo")
    configs = [d / ".cargo" / "config.toml" for d in (start, *start.parents)]
    configs.append(cargo_home / "config.toml")
    for config in configs:
        try:
            with open(config, "rb") as fh:
                wrapper = tomllib.load(fh).get("build", {}).get("rustc-wrapper")
        except (OSError, tomllib.TOMLDecodeError):
            continue
        if isinstance(wrapper, str) and wrapper.strip():
            return wrapper.strip()
    return None


def sccache_probe_bin(env: dict[str, str], cwd: Path | None = None) -> str | None:
    """The sccache binary cargo will actually run: AIDA_SCCACHE, else the
    RUSTC_WRAPPER or configured `build.rustc-wrapper` naming sccache, else
    PATH. Probing a different one (a PATH shim that degrades by itself) would
    pass while cargo's own wrapper is refused. trace:TASK-2772 | ai:claude"""
    if env.get("AIDA_SCCACHE"):
        return env["AIDA_SCCACHE"]
    wrapper = (env.get("RUSTC_WRAPPER") or "").strip() or configured_rustc_wrapper(env, cwd)
    if wrapper and "sccache" in wrapper:
        return shutil.which(wrapper, path=env.get("PATH")) or wrapper
    return shutil.which("sccache", path=env.get("PATH"))


def sccache_fallback(env: dict[str, str], probe=None, cwd: Path | None = None) -> dict[str, str]:
    """TASK-2772: ask sccache once, with a deadline, whether it can serve this
    process. When it is refused (a lane sandbox without its socket) or does not
    answer, build without it: RUSTC_WRAPPER= and one note. Never hangs."""
    if not sccache_wrapper_active(env) or env.get("AIDA_SCCACHE_DISABLE") == "1":
        return env
    if probe is None:
        sccache_bin = sccache_probe_bin(env, cwd=cwd)  # trace:TASK-2790 | ai:codex
        if not sccache_bin:
            return env

        def probe() -> bool:
            try:
                # The call cargo itself makes first: `--show-stats` can
                # succeed where a wrapped compile is refused (EPERM).
                done = subprocess.run(
                    [sccache_bin, env.get("RUSTC") or "rustc", "-vV"],
                    env=env,
                    cwd=cwd,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=SCCACHE_PROBE_TIMEOUT,
                )
            except (OSError, subprocess.TimeoutExpired):
                return False
            return done.returncode == 0

    if probe():
        return env
    return without_sccache(env)


def without_sccache(env: dict[str, str]) -> dict[str, str]:
    """TASK-2772: the single fallback note, and RUSTC_WRAPPER=."""
    print(
        "check-ci-gates: sccache is unavailable here; building without it (RUSTC_WRAPPER=).",
        file=sys.stderr,
    )
    return {**env, "RUSTC_WRAPPER": ""}


def gate_env(root: Path | None = None) -> dict[str, str]:
    """TASK-2772: the gate runner's start path. Start (or restart) the sccache
    server under a deadline, then probe it once; a stuck or failed start skips
    the probe and falls back. Every step is bounded."""
    if not ensure_sccache_server():
        return without_sccache(dict(os.environ))
    return sccache_fallback(dict(os.environ), cwd=root)


# trace:BUG-2418 | ai:claude
CARGO_HOME_READ_ONLY_NOTE = "new dependencies are fetched on the host: run cargo fetch outside the lane"


def cargo_home_read_only_note(env: dict[str, str]) -> str | None:
    """BUG-2418: the one-line note when CARGO_HOME exists but is not writable
    (a contained lane binds it read-only, so cargo cannot fetch a new crate)."""
    home = env.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    if not os.path.isdir(home):
        return None
    try:
        with tempfile.NamedTemporaryFile(dir=home, prefix=".aida-write-probe"):
            return None
    except PermissionError:
        return CARGO_HOME_READ_ONLY_NOTE
    except OSError as err:
        return CARGO_HOME_READ_ONLY_NOTE if err.errno == errno.EROFS else None


# trace:TASK-2790 | ai:codex
CARGO_FETCH_ERRORS = (b"read-only file system", b"failed to download", b"failed to update registry")


# trace:TASK-2802 | ai:claude
def cargo_tty_env(env: dict[str, str], parent_is_tty: bool, width: int | None = None) -> dict[str, str]:
    """TASK-2802: the gate env plus cargo's terminal settings when the parent
    stream is a TTY; values already in `env` are left alone. Cargo's progress
    `auto` still checks its own (piped) stderr, so the bar needs `always`, and
    cargo rejects `always` without a width: the progress keys are added only
    when the terminal width is known."""
    if not parent_is_tty:
        return env
    tty = {"CARGO_TERM_COLOR": "always"}
    if width:
        tty |= {"CARGO_TERM_PROGRESS_WHEN": "always", "CARGO_TERM_PROGRESS_WIDTH": str(width)}
    return {**tty, **env}


def stderr_tty_width() -> tuple[bool, int | None]:
    """TASK-2802: whether our stderr is a terminal, and its width if known."""
    try:
        if not sys.stderr.isatty():
            return False, None
    except (AttributeError, ValueError):
        return False, None
    try:
        return True, os.get_terminal_size(sys.stderr.fileno()).columns
    except (AttributeError, OSError, ValueError):
        return True, None


def run_gate(root: Path, name: str, command: str, env: dict[str, str]) -> tuple[bool, float, bool]:
    started = time.monotonic()
    # Preserve both streams and retain only an overlap for split error markers.
    def stream_diagnostics(source, sink):
        fetch_error = False
        tail = b""
        # trace:TASK-2802 | ai:claude
        # A replaced sys.stdout (a StringIO) has no .buffer: write it text.
        binary = getattr(sink, "buffer", None)
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        while chunk := source.read1(8192):
            try:
                if binary is not None:
                    binary.write(chunk)
                    binary.flush()
                else:
                    sink.write(decoder.decode(chunk))
                    sink.flush()
            except (OSError, ValueError, AttributeError, TypeError):
                # Drain the child even when the caller's sink went away or
                # cannot take the write (a broken pipe, a closed file, a full
                # disk, an odd sink), like the Rust reader.
                pass
            evidence = tail + chunk.lower()
            fetch_error |= any(marker in evidence for marker in CARGO_FETCH_ERRORS)
            tail = evidence[-64:]
        return fetch_error

    # Piped streams hide the terminal from cargo, which then drops colour and
    # the progress bar; keep them when our own stderr is a terminal, unless the
    # caller already chose.
    # trace:TASK-2802 | ai:claude
    env = cargo_tty_env(env, *stderr_tty_width())
    # The reads below finish at EOF, not at the child's exit: a grandchild that
    # inherits a pipe and outlives the gate keeps its result() blocked.
    with subprocess.Popen(
        [*CI_SHELL_ARGV, command], cwd=root, env=env,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ) as child, ThreadPoolExecutor(max_workers=2) as readers:
        stdout = readers.submit(stream_diagnostics, child.stdout, sys.stdout)
        stderr = readers.submit(stream_diagnostics, child.stderr, sys.stderr)
        passed = child.wait() == 0
        # Read both results even if the first one already contains evidence.
        fetch_error = any([stdout.result(), stderr.result()])
    return passed, time.monotonic() - started, fetch_error


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="check-ci-gates.py",
        description="Run CI's Build-job gates locally; fast source gates precede compilation.",
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
    plan = [(name, local_test_command(command)) for name, command in steps if tiers.get(name) in wanted]

    # Run source ratchets before potentially cold compilation so a cold target
    # cannot hide the failures the fast gate is meant to catch.
    # trace:STORY-2640 | ai:codex
    if args.tier == "fast":
        plan.sort(key=lambda gate: bool(re.search(r"\bcargo (?:check|test|build)\b", gate[1])))

    # Every skip is printed with its reason, naming the gate. A skip nobody sees
    # is how the documented check set came to omit ~25 gates.
    for name, _ in steps:
        if name in skips:
            print(f"SKIP  {name}\n        cannot run locally: {skips[name]}")
        elif tiers[name] not in wanted:
            print(f"SKIP  {name}\n        not in the {args.tier} tier (it is {tiers[name]})")

    if args.list:
        for name, command in plan:
            # Print the reshaped command itself: it is not always `cargo test`. trace:TASK-1979 | ai:claude
            reshaped = f"  (local: {command})" if command != dict(steps)[name] else ""
            print(f"PLAN  {name}  [{tiers[name]}]{reshaped}")
        # Derived gates are not part of `all`, but a listing that hid them would
        # make `make check-ci-compile` undiscoverable, which is the defect the
        # tier table exists to remove.
        for name, tier, source, command in derived_gates:
            print(
                f"ALSO  {name}  [{tier}]  `make check-ci-{tier}` only\n"
                f"        derived from CI's {source!r}: {command}"
            )
        return 0

    with contextlib.ExitStack() as stack:
        env = gate_env(root)  # trace:TASK-2772 | ai:claude
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
        passed, seconds, fetch_error = run_gate(root, name, resolved, env)
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
            note = cargo_home_read_only_note(env) if fetch_error else None
            if note:
                print(f"check-ci-gates: CARGO_HOME is read-only here; {note}", file=sys.stderr)
            # Exit on the first failure; the fast plan puts source gates
            # first so cold compilation cannot mask their failures.
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
