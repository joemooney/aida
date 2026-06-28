#!/usr/bin/env python3
# AIDA agent-surface benchmark harness.
#
# Reproduces the AXI MCP-vs-token-efficient-CLI methodology
# (github.com/kunchenguid/axi, bench-github/) on AIDA's OWN surfaces, to settle
# whether MCP is over-weighted relative to a plain CLI for agent operations.
#
# Methodology (mirrors AXI):
#   per (condition x task x run):
#     1. ensure a seeded AIDA fixture project exists (seed_fixture.sh)
#     2. run `claude -p` with `--output-format stream-json` against the
#        condition (CLI tools vs MCP tools), cwd = the fixture
#     3. parse the stream-json: tokens / turns / cost / tool calls / errors
#     4. LLM-judge (claude haiku) grades the trajectory vs a per-task
#        grading_hint -> pass/fail
#     5. append one row to results/results.jsonl
#   then `report` aggregates per-condition success / cost / turns / tokens.
#
# Stdlib only. Requires `claude` and `aida` on PATH.
#
# Usage:
#   python3 run_bench.py seed
#   python3 run_bench.py run --condition cli --task next_queue_item [--repeat 2] [--model sonnet]
#   python3 run_bench.py matrix --condition cli,mcp --task next_queue_item,file_spec --repeat 2
#   python3 run_bench.py report

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

BENCH_ROOT = Path(__file__).resolve().parent
RESULTS_DIR = BENCH_ROOT / "results"
FIXTURE_DIR = BENCH_ROOT / "fixture-project"
RESULTS_JSONL = RESULTS_DIR / "results.jsonl"

DEFAULT_AGENT_MODEL = "sonnet"
JUDGE_MODEL = "haiku"
AGENT_TIMEOUT_S = 300
JUDGE_TIMEOUT_S = 90


def load_json(name):
    with open(BENCH_ROOT / name, "r", encoding="utf-8") as f:
        return json.load(f)


# ---------------------------------------------------------------------------
# Fixture
# ---------------------------------------------------------------------------

def ensure_fixture(force=False):
    if (FIXTURE_DIR / ".aida" / "config.toml").exists() and not force:
        return
    seed = BENCH_ROOT / "seed_fixture.sh"
    args = ["bash", str(seed), str(FIXTURE_DIR)]
    if force:
        args.append("--force")
    print("Seeding fixture (this runs `aida init`, ~1 min)...")
    subprocess.run(args, check=True)


def write_mcp_config(run_dir):
    """Write a strict MCP config that registers only the AIDA stdio server."""
    cfg = {"mcpServers": {"aida": {"command": "aida", "args": ["mcp-serve"]}}}
    path = run_dir / "mcp-config.json"
    path.write_text(json.dumps(cfg), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------
# Agent execution
# ---------------------------------------------------------------------------

def build_claude_argv(condition, prompt, model, run_dir):
    argv = [
        "claude",
        "--setting-sources", "",
        "-p", prompt,
        "--model", model,
        "--output-format", "stream-json",
        "--verbose",
        "--dangerously-skip-permissions",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--strict-mcp-config",
    ]
    if condition.get("mcp"):
        mcp_path = write_mcp_config(run_dir)
        argv += ["--mcp-config", str(mcp_path)]
    allowed = condition.get("allowed_tools") or []
    if allowed:
        argv += ["--allowedTools", *allowed]
    disallowed = condition.get("disallowed_tools") or []
    if disallowed:
        argv += ["--disallowedTools", *disallowed]
    return argv


def run_agent(condition, task_prompt, model, run_dir):
    preamble = condition.get("preamble", "")
    full_prompt = (preamble + "\n\n" + task_prompt).strip() if preamble else task_prompt
    argv = build_claude_argv(condition, full_prompt, model, run_dir)
    start = time.time()
    try:
        proc = subprocess.run(
            argv,
            cwd=str(FIXTURE_DIR),
            capture_output=True,
            text=True,
            timeout=AGENT_TIMEOUT_S,
            env=os.environ.copy(),
        )
        raw = proc.stdout
        if proc.stderr:
            (run_dir / "stderr.txt").write_text(proc.stderr, encoding="utf-8")
    except subprocess.TimeoutExpired as e:
        raw = e.stdout.decode("utf-8") if isinstance(e.stdout, bytes) else (e.stdout or "")
        (run_dir / "stderr.txt").write_text("TIMEOUT after %ds\n" % AGENT_TIMEOUT_S, encoding="utf-8")
    wall = time.time() - start
    (run_dir / "agent_output.jsonl").write_text(raw, encoding="utf-8")
    return raw, wall


# ---------------------------------------------------------------------------
# Stream-json parsing  (port of AXI parseClaudeJsonl)
# ---------------------------------------------------------------------------

def _iter_jsonl(raw):
    for line in raw.split("\n"):
        line = line.strip()
        if not line:
            continue
        try:
            yield json.loads(line)
        except (ValueError, TypeError):
            continue


def parse_claude_jsonl(raw, wall_seconds):
    input_tokens = 0
    input_tokens_cached = 0
    output_tokens = 0
    reported_cost = 0.0
    turn_count = 0
    command_count = 0
    tool_call_count = 0
    error_count = 0
    command_log = []

    for entry in _iter_jsonl(raw):
        etype = entry.get("type")

        if etype == "assistant":
            msg = entry.get("message") or {}
            for block in (msg.get("content") or []):
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    tool_call_count += 1
                    name = block.get("name", "")
                    inp = block.get("input") or {}
                    if name == "Bash" and isinstance(inp.get("command"), str):
                        command_count += 1
                        command_log.append(inp["command"])
                    else:
                        command_log.append("%s(%s)" % (name, json.dumps(inp)[:200]))

        elif etype == "user":
            msg = entry.get("message") or {}
            for block in (msg.get("content") or []):
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    if block.get("is_error") is True:
                        error_count += 1

        elif etype == "result":
            reported_cost = float(entry.get("total_cost_usd") or 0.0)
            turn_count = int(entry.get("num_turns") or 0)
            usage = entry.get("usage") or {}
            base = int(usage.get("input_tokens") or 0)
            cache_create = int(usage.get("cache_creation_input_tokens") or 0)
            cache_read = int(usage.get("cache_read_input_tokens") or 0)
            input_tokens = base + cache_create + cache_read
            input_tokens_cached = cache_read
            output_tokens = int(usage.get("output_tokens") or 0)

    return {
        "input_tokens": input_tokens,
        "input_tokens_cached": input_tokens_cached,
        "output_tokens": output_tokens,
        "total_cost_usd": reported_cost,
        "wall_clock_seconds": round(wall_seconds, 2),
        "turn_count": turn_count,
        "command_count": command_count,
        "tool_call_count": tool_call_count,
        "error_count": error_count,
        "command_log": command_log,
    }


# ---------------------------------------------------------------------------
# Grading  (LLM-as-judge, port of AXI grader.ts)
# ---------------------------------------------------------------------------

def format_trajectory(raw):
    parts = []
    for entry in _iter_jsonl(raw):
        etype = entry.get("type")
        if etype == "assistant":
            msg = entry.get("message") or {}
            for block in (msg.get("content") or []):
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "text" and isinstance(block.get("text"), str):
                    parts.append("AGENT: " + block["text"])
                elif block.get("type") == "tool_use":
                    name = block.get("name", "")
                    inp = block.get("input") or {}
                    if name == "Bash" and isinstance(inp.get("command"), str):
                        parts.append("COMMAND: " + inp["command"])
                    else:
                        parts.append("TOOL_CALL: %s(%s)" % (name, json.dumps(inp)))
        elif etype == "user":
            msg = entry.get("message") or {}
            for block in (msg.get("content") or []):
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    content = block.get("content")
                    if isinstance(content, str):
                        parts.append("OUTPUT: " + content)
                    elif isinstance(content, list):
                        txt = "\n".join(
                            c.get("text", json.dumps(c)) if isinstance(c, dict) else str(c)
                            for c in content
                        )
                        parts.append("OUTPUT: " + txt)
        elif etype == "result" and isinstance(entry.get("result"), str):
            parts.append("AGENT: " + entry["result"])
    return "\n".join(parts).strip() or "(empty trajectory)"


def build_grading_prompt(task_prompt, trajectory, grading_hint):
    hint = ("\nKNOWN FACTS: " + grading_hint + "\n") if grading_hint else ""
    return (
        "You are a benchmark grader evaluating whether an AI agent completed a task.\n\n"
        "TASK: " + task_prompt + "\n" + hint +
        "\nAGENT TRAJECTORY:\n" + trajectory + "\n\n"
        "Rules:\n"
        "- PASS if the agent ran appropriate commands/tools AND produced a correct, complete answer\n"
        "- FAIL if the agent hallucinated data without running commands/tools\n"
        "- FAIL if the agent ran commands/tools but misinterpreted the results\n"
        "- FAIL if the agent gave a partial answer when a complete one was requested\n\n"
        'Respond with exactly: {"pass": true, "reason": "..."} or {"pass": false, "reason": "..."}'
    )


def extract_verdict(text):
    stripped = text.replace("```json", "").replace("```", "").strip()
    try:
        d = json.loads(stripped)
        if isinstance(d.get("pass"), bool):
            return {"pass": d["pass"], "reason": d.get("reason", "")}
    except (ValueError, TypeError):
        pass
    import re
    m = re.search(r'\{\s*"pass"\s*:\s*(true|false)\s*,\s*"reason"\s*:\s*".*?"\s*\}', stripped, re.S)
    if not m:
        m = re.search(r'\{\s*"reason"\s*:\s*".*?"\s*,\s*"pass"\s*:\s*(true|false)\s*\}', stripped, re.S)
    if m:
        try:
            d = json.loads(m.group(0))
            if isinstance(d.get("pass"), bool):
                return {"pass": d["pass"], "reason": d.get("reason", "")}
        except (ValueError, TypeError):
            pass
    return None


def grade(task_prompt, grading_hint, raw_jsonl, run_dir):
    trajectory = format_trajectory(raw_jsonl)
    prompt = build_grading_prompt(task_prompt, trajectory, grading_hint)
    argv = [
        "claude", "--setting-sources", "",
        "-p", prompt,
        "--model", JUDGE_MODEL,
        "--output-format", "text",
        "--max-turns", "1",
        "--dangerously-skip-permissions",
        "--no-session-persistence",
        "--strict-mcp-config",
    ]
    try:
        proc = subprocess.run(argv, capture_output=True, text=True, timeout=JUDGE_TIMEOUT_S)
        out = proc.stdout
    except subprocess.TimeoutExpired:
        return {"task_success": False, "details": "judge timeout"}
    (run_dir / "judge_output.txt").write_text(out, encoding="utf-8")
    verdict = extract_verdict(out)
    if not verdict:
        return {"task_success": False, "details": "could not parse judge verdict: " + out[:300]}
    return {"task_success": verdict["pass"], "details": verdict["reason"]}


# ---------------------------------------------------------------------------
# Run orchestration
# ---------------------------------------------------------------------------

def run_one(condition_id, condition, task_id, task, run_no, model):
    run_dir = RESULTS_DIR / condition_id / task_id / ("run%d" % run_no)
    run_dir.mkdir(parents=True, exist_ok=True)

    if condition.get("not_implemented"):
        print("  [skip] condition '%s' is not implemented yet (%s)" % (condition_id, condition["name"]))
        return None

    raw, wall = run_agent(condition, task["prompt"], model, run_dir)
    usage = parse_claude_jsonl(raw, wall)
    grade_result = grade(task["prompt"], task.get("grading_hint"), raw, run_dir)
    (run_dir / "grade.json").write_text(json.dumps(grade_result, indent=2), encoding="utf-8")

    result = {
        "condition": condition_id,
        "condition_name": condition["name"],
        "task": task_id,
        "run": run_no,
        "model": model,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "usage": usage,
        "grade": grade_result,
    }
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    with open(RESULTS_JSONL, "a", encoding="utf-8") as f:
        f.write(json.dumps(result) + "\n")
    return result


def print_run(result):
    if result is None:
        return
    g = "PASS" if result["grade"]["task_success"] else "FAIL"
    u = result["usage"]
    print("  %s | %d turns | %d tools | in %d tok / out %d tok | $%.4f | %.1fs"
          % (g, u["turn_count"], u["tool_call_count"], u["input_tokens"],
             u["output_tokens"], u["total_cost_usd"], u["wall_clock_seconds"]))


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

def load_results():
    if not RESULTS_JSONL.exists():
        return []
    out = []
    for line in RESULTS_JSONL.read_text(encoding="utf-8").split("\n"):
        line = line.strip()
        if line:
            out.append(json.loads(line))
    return out


def _mean(xs):
    return sum(xs) / len(xs) if xs else 0.0


def markdown_report(results):
    if not results:
        return "No results found.\n"
    lines = ["# AIDA agent-surface benchmark results\n"]
    lines.append("Generated: %s\n" % datetime.now(timezone.utc).isoformat())
    by_cond = {}
    for r in results:
        by_cond.setdefault(r["condition"], []).append(r)

    lines.append("## Per-condition summary\n")
    lines.append("| Condition | Runs | Success% | Avg in-tok | Avg out-tok | Avg cost | Total cost | Avg turns | Avg tools | Avg s |")
    lines.append("|---|---|---|---|---|---|---|---|---|---|")
    for cond, rs in by_cond.items():
        succ = sum(1 for r in rs if r["grade"]["task_success"])
        lines.append("| %s | %d | %d%% | %d | %d | $%.4f | $%.4f | %.1f | %.1f | %.1f |" % (
            cond, len(rs), round(100 * succ / len(rs)),
            round(_mean([r["usage"]["input_tokens"] for r in rs])),
            round(_mean([r["usage"]["output_tokens"] for r in rs])),
            _mean([r["usage"]["total_cost_usd"] for r in rs]),
            sum(r["usage"]["total_cost_usd"] for r in rs),
            _mean([r["usage"]["turn_count"] for r in rs]),
            _mean([r["usage"]["tool_call_count"] for r in rs]),
            _mean([r["usage"]["wall_clock_seconds"] for r in rs]),
        ))

    lines.append("\n## Per-task breakdown\n")
    by_task = {}
    for r in results:
        by_task.setdefault(r["task"], []).append(r)
    for task, rs in by_task.items():
        lines.append("### %s\n" % task)
        lines.append("| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |")
        lines.append("|---|---|---|---|---|---|")
        tc = {}
        for r in rs:
            tc.setdefault(r["condition"], []).append(r)
        for cond, crs in tc.items():
            succ = sum(1 for r in crs if r["grade"]["task_success"])
            lines.append("| %s | %d | %d | $%.4f | %.1f | %d/%d |" % (
                cond,
                round(_mean([r["usage"]["input_tokens"] for r in crs])),
                round(_mean([r["usage"]["output_tokens"] for r in crs])),
                _mean([r["usage"]["total_cost_usd"] for r in crs]),
                _mean([r["usage"]["turn_count"] for r in crs]),
                succ, len(crs),
            ))
        lines.append("")
    return "\n".join(lines) + "\n"


def write_reports():
    results = load_results()
    md = markdown_report(results)
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    (RESULTS_DIR / "report.md").write_text(md, encoding="utf-8")
    print(md)
    print("Report written to %s" % (RESULTS_DIR / "report.md"))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def clear_results(condition_ids):
    if not RESULTS_JSONL.exists():
        RESULTS_DIR.mkdir(parents=True, exist_ok=True)
        RESULTS_JSONL.write_text("", encoding="utf-8")
        return
    kept = []
    for line in RESULTS_JSONL.read_text(encoding="utf-8").split("\n"):
        line = line.strip()
        if not line:
            continue
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if r.get("condition") not in condition_ids:
            kept.append(line)
    RESULTS_JSONL.write_text(("\n".join(kept) + "\n") if kept else "", encoding="utf-8")


def cmd_matrix(args):
    conditions = load_json("conditions.json")
    tasks = load_json("tasks.json")
    cond_ids = args.condition.split(",") if args.condition else list(conditions.keys())
    task_ids = args.task.split(",") if args.task else list(tasks.keys())
    ensure_fixture(force=args.reseed)
    clear_results(cond_ids)
    total = len(cond_ids) * len(task_ids) * args.repeat
    n = 0
    for cond_id in cond_ids:
        condition = conditions[cond_id]
        for task_id in task_ids:
            task = tasks[task_id]
            for r in range(1, args.repeat + 1):
                n += 1
                print("\n[%d/%d] %s x %s (run %d)" % (n, total, cond_id, task_id, r))
                print_run(run_one(cond_id, condition, task_id, task, r, args.model))
    print("\nMatrix complete: %d runs." % total)
    write_reports()


def cmd_run(args):
    conditions = load_json("conditions.json")
    tasks = load_json("tasks.json")
    if args.condition not in conditions:
        sys.exit("unknown condition: %s (have: %s)" % (args.condition, ", ".join(conditions)))
    if args.task not in tasks:
        sys.exit("unknown task: %s (have: %s)" % (args.task, ", ".join(tasks)))
    ensure_fixture(force=args.reseed)
    for r in range(1, args.repeat + 1):
        print("\n=== %s x %s (run %d/%d) ===" % (args.condition, args.task, r, args.repeat))
        print_run(run_one(args.condition, conditions[args.condition], args.task, tasks[args.task], r, args.model))


def main():
    p = argparse.ArgumentParser(description="AIDA agent-surface benchmark (CLI vs MCP vs TOON)")
    sub = p.add_subparsers(dest="cmd", required=True)

    sp = sub.add_parser("seed", help="(re)seed the fixture AIDA project")
    sp.add_argument("--force", action="store_true")

    rp = sub.add_parser("run", help="run one condition x task")
    rp.add_argument("--condition", required=True)
    rp.add_argument("--task", required=True)
    rp.add_argument("--repeat", type=int, default=1)
    rp.add_argument("--model", default=DEFAULT_AGENT_MODEL)
    rp.add_argument("--reseed", action="store_true")

    mp = sub.add_parser("matrix", help="run a condition x task matrix")
    mp.add_argument("--condition", help="comma-separated condition ids (default: all)")
    mp.add_argument("--task", help="comma-separated task ids (default: all)")
    mp.add_argument("--repeat", type=int, default=1)
    mp.add_argument("--model", default=DEFAULT_AGENT_MODEL)
    mp.add_argument("--reseed", action="store_true")

    sub.add_parser("report", help="aggregate results.jsonl into report.md")

    args = p.parse_args()
    if args.cmd == "seed":
        ensure_fixture(force=args.force)
    elif args.cmd == "run":
        cmd_run(args)
    elif args.cmd == "matrix":
        cmd_matrix(args)
    elif args.cmd == "report":
        write_reports()


if __name__ == "__main__":
    main()
