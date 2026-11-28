#!/usr/bin/env python3
"""Run a libtest selection one test per process, each under its own timeout.

The CI live-bwrap gate used to run its selection as one `cargo test | tee`:
when one test stalled, the step hung to the job timeout and its logs were
lost. This harness builds the test binary once, lists the tests matching the
filters, and runs each in its own process group with a per-test timeout. A
test over its budget is killed and named, with the running tests and the log
tail printed before the run fails. An overall deadline bounds the whole run.

Result lines use libtest's --logfile format (`ok <name>`, `failed <name>`,
`ignored <name>`, plus `timeout <name>`), so callers keep counting `^ok `.
Test output is appended to --log; a run without "1 passed" is a failure.
trace:BUG-2788 | ai:claude
"""
import argparse
import itertools
import json
import os
import shlex
import signal
import subprocess
import sys
import tempfile
import time


def build(cargo_args):
    cmd = ["cargo", "test", "--no-run", "--message-format=json-render-diagnostics", *cargo_args]
    print("watchdog: building:", shlex.join(cmd), flush=True)
    started = time.monotonic()
    out = subprocess.run(cmd, stdout=subprocess.PIPE, text=True, check=True).stdout
    exes = []
    for line in out.splitlines():
        msg = json.loads(line) if line.startswith("{") else {}
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") and msg["profile"]["test"]:
            exes.append((msg["executable"], os.path.dirname(msg["manifest_path"])))
    print(f"watchdog: build took {time.monotonic() - started:.0f}s", flush=True)
    if len(exes) != 1:
        sys.exit(f"watchdog: expected one test executable, got {exes}")
    return exes[0]


def list_tests(binary, filters, cwd, ignored=False):
    cmd = [binary, "--list", "--format", "terse", *(["--ignored"] if ignored else []), *filters]
    out = subprocess.run(cmd, cwd=cwd, stdout=subprocess.PIPE, text=True, check=True).stdout
    return [line[: -len(": test")] for line in out.splitlines() if line.endswith(": test")]


def tail(path, lines):
    try:
        with open(path, errors="replace") as f:
            return "".join(f.readlines()[-lines:])
    except OSError:
        return ""


def kill(proc):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(proc.pid, sig)
        except ProcessLookupError:
            return
        try:
            proc.wait(timeout=3)
            return
        except subprocess.TimeoutExpired:
            continue


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--cargo-test-args", help="arguments for `cargo test --no-run`, e.g. '-p aida-cli-lib --lib'")
    p.add_argument("--binary", help="run this test binary instead of building one")
    p.add_argument("--log", required=True, help="append each test's output here")
    p.add_argument("--results", required=True, help="write libtest-format result lines here")
    p.add_argument("--test-timeout", type=float, default=120)
    p.add_argument("--overall-timeout", type=float, default=480)
    p.add_argument("--jobs", type=int, default=os.cpu_count() or 2)
    p.add_argument("--tail-lines", type=int, default=40)
    p.add_argument("filters", nargs="+")
    args = p.parse_args(argv)
    if bool(args.binary) == bool(args.cargo_test_args):
        p.error("give exactly one of --binary or --cargo-test-args")
    if args.binary:
        binary, cwd = os.path.abspath(args.binary), os.getcwd()
    else:
        binary, cwd = build(shlex.split(args.cargo_test_args))
    env = dict(os.environ, CARGO_MANIFEST_DIR=cwd)
    ignored = set(list_tests(binary, args.filters, cwd, ignored=True))
    pending = [t for t in list_tests(binary, args.filters, cwd) if t not in ignored]
    total = len(pending)
    print(f"watchdog: {total} tests (+{len(ignored)} ignored), {args.jobs} at a time, "
          f"{args.test_timeout:.0f}s each, {args.overall_timeout:.0f}s overall", flush=True)
    if not pending:
        sys.exit("watchdog: no tests match the filters")
    results = {t: "ignored" for t in ignored}
    running = {}  # name -> (proc, output path, start)
    tmp = tempfile.mkdtemp(prefix="live-test-watchdog-")
    deadline = time.monotonic() + args.overall_timeout
    log = open(args.log, "a")
    serial = itertools.count()

    def report_stall(why):
        names = ", ".join(f"{n} ({time.monotonic() - s:.0f}s)" for n, (_, _, s) in running.items()) or "none"
        print(f"::error::{why}\nwatchdog: running tests: {names}", flush=True)
        log.flush()
        print(f"watchdog: last {args.tail_lines} lines of {args.log}:\n{tail(args.log, args.tail_lines)}", flush=True)

    def finish(name, status, proc_out, started):
        results[name] = status
        log.write(f"--- {name}: {status} in {time.monotonic() - started:.1f}s\n{tail(proc_out, 10**6)}")
        log.flush()
        print(f"{status} {name} ({time.monotonic() - started:.1f}s)", flush=True)

    while pending or running:
        while pending and len(running) < args.jobs:
            name = pending.pop(0)
            out = os.path.join(tmp, f"{next(serial)}.out")
            with open(out, "w") as f:
                proc = subprocess.Popen([binary, "--exact", name, "--nocapture", "--test-threads", "1"],
                                        cwd=cwd, env=env, stdout=f, stderr=subprocess.STDOUT,
                                        stdin=subprocess.DEVNULL, start_new_session=True)
            running[name] = (proc, out, time.monotonic())
        time.sleep(0.1)
        now = time.monotonic()
        for name, (proc, out, started) in list(running.items()):
            if proc.poll() is not None:
                del running[name]
                passed = proc.returncode == 0 and "test result: ok. 1 passed;" in tail(out, 5)
                finish(name, "ok" if passed else "failed", out, started)
            elif now - started > args.test_timeout:
                report_stall(f"test {name} exceeded its {args.test_timeout:.0f}s timeout; killed")
                print(f"watchdog: last {args.tail_lines} lines of {name}:\n{tail(out, args.tail_lines)}", flush=True)
                kill(proc)
                del running[name]
                finish(name, "timeout", out, started)
        if now > deadline and (pending or running):
            report_stall(f"overall {args.overall_timeout:.0f}s deadline passed with "
                         f"{len(running)} running and {len(pending)} not started")
            for name, (proc, out, started) in list(running.items()):
                kill(proc)
                del running[name]
                finish(name, "timeout", out, started)
            for name in pending:
                results[name] = "notrun"
            pending = []
    log.close()
    with open(args.results, "w") as f:
        f.writelines(f"{status} {name}\n" for name, status in sorted(results.items()))
    bad = sorted(n for n, s in results.items() if s not in ("ok", "ignored"))
    print(f"watchdog: {total - len(bad)} of {total} ok", flush=True)
    for name in bad:
        print(f"watchdog: {results[name]} {name}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
