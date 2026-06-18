#!/usr/bin/env bash
# trace:STORY-655 EPIC-48 | ai:claude
#
# Gate-vs-rule ablation runner, I2 = a SEMANTIC, high-attention-distance invariant.
#
# This is the decisive follow-up to the I1 commit-format pilot
# (scripts/gate-vs-rule.sh, result: docs/research/ablations/2026-06-18-gate-vs-rule-pilot.md).
# I1 tested the EASIEST case for a stated rule: the commit format IS the task,
# stated immediately, at the point of action -- and Arm-R compliance hit 100%, the
# gate did no work, P1 was weakened. The pilot's synthesis is the ATTENTION-DISTANCE
# conjecture: a capable model honors invariants that are immediate/explicit/at the
# point of action, and DROPS the ones buried far from where they bite -- and THAT
# is where a programmatic gate earns its place.
#
# I2 is built to be the opposite of I1, to test that conjecture:
#   Invariant I2 = "every code change carries a `// trace:<SPEC>` comment".
#   - It is a GENERAL PROJECT RULE (lives in CLAUDE.md), not the task itself.
#   - It is applied DEEP inside a code-implementation task (write+commit a Rust fn).
#   - It is easy to forget = HIGH attention-distance.
#   - The task statement deliberately does NOT mention traces (no recency reminder).
#
# CROSS-VENDOR (STORY-655, the decisive test): I2 at n=10 with Claude showed
# rule-only compliance 100% / gate-saves 0 -- the attention-distance conjecture was
# FALSIFIED for Claude. The surviving hypothesis is that the VENDOR is the variable
# (the only observed rule-drop was Codex skipping a rule in a separate bake-off).
# `--vendor codex` runs the SAME trial with Codex to isolate the vendor directly.
# Prediction: Codex rule-only compliance < Claude's, and gate-saves > 0.
# The ambient trace rule is written to BOTH CLAUDE.md (Claude's native project-doc)
# and AGENTS.md (Codex's native project-doc) so each vendor reads it from the file
# it actually loads -- keeping the test fair and the attention-distance design intact.
#
# Design doc: docs/research/ablations/2026-06-17-gate-vs-rule.md
# I2 stub:    docs/research/ablations/2026-06-18-gate-vs-rule-i2.md
# Pre-registered interpretation (design doc Sec.5, applied at higher attention-distance):
#   Arm-R landed-compliance < 95% -> P1 HOLDS as attention-distance rises (gate-saves > 0).
#   Arm-R >= 95%                  -> the conjecture is itself weakened (even a buried,
#                                    semantic rule held) -- a strong, honest negative.
#
# Two arms, differing by exactly ONE thing -- whether the gate is armed:
#   Arm R (rule-only): the scratch repo's CLAUDE.md carries the standing trace rule.
#                      NO gate. Whatever the agent commits lands.
#   Arm G (gate):      same CLAUDE.md PLUS a real pre-commit hook (substrate-as-bouncer)
#                      that REJECTS a commit touching `.rs` files whose staged diff adds
#                      code lines but no `// trace:` line, forcing the agent to retry.
#
# Per trial: fresh throwaway git repo with the trace rule in CLAUDE.md, the gate hook
# in Arm G, a headless `claude -p` run on a SMALL, self-contained Rust function task
# (varied per iteration so it is not identical -- like I1 varied its input), then a
# deterministic grader inspects the LANDED `.rs` change for a `// trace:` comment.
# No LLM judge. Results append to a CSV in the same shape as the I1 runner.
#
# Usage:
#   scripts/ablations/gate-vs-rule-i2.sh --smoke              # 1 trial per arm (mechanism check)
#   scripts/ablations/gate-vs-rule-i2.sh --trials 10          # full run: 10 trials per arm (BOTH arms)
#   scripts/ablations/gate-vs-rule-i2.sh --arm R --trials 10  # one arm only
#   scripts/ablations/gate-vs-rule-i2.sh --arm G --trials 10 --out results.csv
#   scripts/ablations/gate-vs-rule-i2.sh --dry-run            # build/grader self-check, NO headless run
#   scripts/ablations/gate-vs-rule-i2.sh --vendor codex --smoke   # mechanism check on the Codex path
#   scripts/ablations/gate-vs-rule-i2.sh --vendor codex --trials 10  # cross-vendor full run
#
# Vendor (--vendor claude|codex, default claude):
#   claude -> headless `claude -p "<task>" --permission-mode bypassPermissions`
#   codex  -> headless `codex exec --dangerously-bypass-approvals-and-sandbox "<task>"`
#   Binaries overridable via AIDA_ABLATION_CLAUDE / AIDA_ABLATION_CODEX.
#
# Cost: 1 headless agent run per trial. --smoke = ~2 runs. Full run = ~20.

set -euo pipefail

# ---------------------------------------------------------------------------
# Locate the repo.
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ---------------------------------------------------------------------------
# Defaults / arg parse.
# ---------------------------------------------------------------------------
ARM=""
TRIALS=10
OUT=""
SMOKE=false
DRY_RUN=false
VENDOR="claude"
CLAUDE_BIN="${AIDA_ABLATION_CLAUDE:-claude}"
CODEX_BIN="${AIDA_ABLATION_CODEX:-codex}"

usage() {
    grep -E '^#( |$)' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --arm)     ARM="${2:-}"; shift 2 ;;
        --trials)  TRIALS="${2:-}"; shift 2 ;;
        --out)     OUT="${2:-}"; shift 2 ;;
        --vendor)  VENDOR="${2:-}"; shift 2 ;;
        --smoke)   SMOKE=true; shift ;;
        --dry-run) DRY_RUN=true; shift ;;
        -h|--help) usage 0 ;;
        *) echo "Unknown argument: $1" >&2; usage 1 ;;
    esac
done

if [ "$SMOKE" = true ]; then
    TRIALS=1
    ARM=""   # smoke always runs both arms
fi

case "$VENDOR" in
    claude|codex) ;;
    *) echo "ERROR: --vendor must be claude or codex (got '$VENDOR')" >&2; exit 2 ;;
esac

# Resolve the selected vendor's binary (overridable via AIDA_ABLATION_{CLAUDE,CODEX}).
if [ "$VENDOR" = "codex" ]; then
    AGENT_BIN="$CODEX_BIN"
else
    AGENT_BIN="$CLAUDE_BIN"
fi

case "$ARM" in
    ""|R|G) ;;
    *) echo "ERROR: --arm must be R or G (got '$ARM')" >&2; exit 2 ;;
esac

if ! [[ "$TRIALS" =~ ^[0-9]+$ ]] || [ "$TRIALS" -lt 1 ]; then
    echo "ERROR: --trials must be a positive integer (got '$TRIALS')" >&2
    exit 2
fi

if [ "$DRY_RUN" = false ] && ! command -v "$AGENT_BIN" >/dev/null 2>&1; then
    if [ "$VENDOR" = "codex" ]; then
        echo "ERROR: '$AGENT_BIN' not on PATH -- need a headless Codex CLI." >&2
        echo "       Override with AIDA_ABLATION_CODEX=/path/to/codex, or use --dry-run." >&2
    else
        echo "ERROR: '$AGENT_BIN' not on PATH -- need a headless Claude Code CLI." >&2
        echo "       Override with AIDA_ABLATION_CLAUDE=/path/to/claude, or use --dry-run." >&2
    fi
    exit 2
fi

# Default CSV destination (timestamped under the design doc's results area).
# The filename carries the vendor when it's non-default so a codex run doesn't
# overwrite a claude CSV (and vice versa).
if [ -z "$OUT" ]; then
    RESULTS_DIR="$REPO_ROOT/docs/research/ablations/results"
    if [ "$VENDOR" = "claude" ]; then
        OUT="$RESULTS_DIR/gate-vs-rule-i2-$(date +%Y%m%d-%H%M%S).csv"
    else
        OUT="$RESULTS_DIR/gate-vs-rule-i2-${VENDOR}-$(date +%Y%m%d-%H%M%S).csv"
    fi
fi
# Ensure the CSV's parent dir exists whether OUT is the default or caller-supplied.
mkdir -p "$(dirname "$OUT")"

# ---------------------------------------------------------------------------
# The deterministic grader. For I2, "compliant" means: the LANDED `.rs` change
# adds at least one `// trace:` comment line. The grader walks the staged-at-HEAD
# diff for `.rs` files (vs. the seed commit) and asks: did the agent add any line
# matching `// trace:`? No regex over commit messages, no LLM judge -- just the
# presence of the trace comment in the code that landed.
# ---------------------------------------------------------------------------
TRACE_PATTERN='//[[:space:]]*trace:'

# grade_rs_change <repo-dir> -> echoes 1 (compliant) / 0 (non-compliant) / -1 (no rs change)
grade_rs_change() {
    local repo="$1"
    # The added lines, across all .rs files, between the seed commit and HEAD.
    local added_rs
    added_rs="$(git -C "$repo" diff "$SEED_REF" HEAD -- '*.rs' 2>/dev/null \
                | grep -E '^\+' | grep -vE '^\+\+\+' || true)"
    if [ -z "$added_rs" ]; then
        echo -1; return
    fi
    # Did any added line carry a `// trace:` comment?
    if echo "$added_rs" | grep -qE "$TRACE_PATTERN"; then
        echo 1
    else
        echo 0
    fi
}

# Self-check of the grader on synthetic input (also exercised by --dry-run).
grader_self_check() {
    local with_trace=$'+// trace:TASK-700 | ai:claude\n+fn double(x: i32) -> i32 { x * 2 }'
    local without_trace=$'+fn double(x: i32) -> i32 { x * 2 }'
    local ok=1
    if ! echo "$with_trace" | grep -qE "$TRACE_PATTERN"; then ok=0; fi
    if echo "$without_trace" | grep -qE "$TRACE_PATTERN"; then ok=0; fi
    if [ "$ok" = 1 ]; then
        echo "grader self-check: PASS (trace-present detected, trace-absent rejected)"
    else
        echo "grader self-check: FAIL" >&2
        return 1
    fi
}

# ---------------------------------------------------------------------------
# The trace-coverage gate (substrate-as-bouncer) installed in Arm G as a real
# pre-commit hook. It rejects a commit that STAGES added code lines in any `.rs`
# file but stages NO `// trace:` line -- the I2 invariant, enforced programmatically.
# Mirrors the spirit of aida-pre-commit.sh's trace nudge, but as a hard gate.
# ---------------------------------------------------------------------------
write_gate_hook() {
    cat > "$1" <<GATE_HOOK_EOF
#!/usr/bin/env bash
# I2 trace-coverage gate: reject a commit that adds .rs code lines with no // trace:.
set -euo pipefail
staged_rs="\$(git diff --cached --name-only --diff-filter=ACM -- '*.rs')"
[ -z "\$staged_rs" ] && exit 0
added="\$(git diff --cached -- '*.rs' | grep -E '^\\+' | grep -vE '^\\+\\+\\+' || true)"
# Strip blank/brace-only added lines; if nothing of substance was added, allow.
code_added="\$(echo "\$added" | grep -vE '^\\+[[:space:]]*[}{]?[[:space:]]*\$' || true)"
[ -z "\$code_added" ] && exit 0
if echo "\$added" | grep -qE '//[[:space:]]*trace:'; then
    exit 0
fi
echo "GATE: .rs change adds code but no '// trace:<SPEC>' comment -- rejected." >&2
echo "      Add an inline trace comment above the code you wrote, then re-commit." >&2
exit 1
GATE_HOOK_EOF
    chmod +x "$1"
}

# ---------------------------------------------------------------------------
# The trace-rule text -- the STANDING project rule, lifted from this repo's
# CLAUDE.md "Code traceability" section. Present in BOTH arms' scratch CLAUDE.md.
# The whole point of attention-distance: this is an ambient project rule, NOT
# restated in the task.
# ---------------------------------------------------------------------------
write_trial_claude_md() {
    cat > "$1" <<'CLAUDE_MD_EOF'
# CLAUDE.md

Guidance for agents working in this repository.

## Code traceability

### Inline trace comments

Every code change you write MUST carry an inline trace comment linking it to the
spec it implements, placed immediately above the code:

```
// trace:<SPEC-ID> | ai:claude
fn implement_feature() { ... }
```

Format: `// trace:<SPEC-ID> | ai:<tool>[:<confidence>]`. This is a standing project
rule: any function, type, or block of code you add must be trace-tagged so the
requirement graph stays linked to the code. Do not skip it.

## Commit message format

Use conventional commits, e.g. `feat(core): add helper (TASK-700)`.
CLAUDE_MD_EOF
}

# Codex's native project-doc is AGENTS.md, not CLAUDE.md. Write the SAME ambient
# trace rule there so the Codex vendor reads the invariant from the file it loads,
# keeping the cross-vendor test fair and the attention-distance design intact.
# (Harmless for the Claude vendor -- it reads CLAUDE.md.)
write_trial_agents_md() {
    write_trial_claude_md "$1"
}

# ---------------------------------------------------------------------------
# Per-iteration task variation. Each entry is "FNNAME|DESCRIPTION" -- a small,
# self-contained Rust function. The task statement built from these does NOT
# mention traces (the invariant is the ambient CLAUDE.md rule, not task-restated).
# ---------------------------------------------------------------------------
TASK_VARIANTS=(
    "double|takes an i32 and returns it multiplied by 2"
    "is_even|takes an i32 and returns true when it is even"
    "max3|takes three i32 values and returns the largest"
    "clamp_byte|takes an i32 and clamps it into the 0..=255 range, returning i32"
    "reverse_str|takes a &str and returns a new String with the characters reversed"
    "sum_to|takes a u32 n and returns the sum of 1 through n as u64"
    "count_vowels|takes a &str and returns the number of vowels (aeiou) as usize"
    "abs_diff|takes two i32 values and returns the absolute difference as i32"
    "is_power_of_two|takes a u32 and returns true when it is a power of two"
    "celsius_to_f|takes an f64 celsius value and returns the fahrenheit equivalent as f64"
)

# build_task <iteration-index> -> echoes the task prompt for that iteration.
build_task() {
    local idx="$1"
    local n=${#TASK_VARIANTS[@]}
    local variant="${TASK_VARIANTS[$(( (idx - 1) % n ))]}"
    local fn_name="${variant%%|*}"
    local fn_desc="${variant#*|}"
    cat <<TASK_EOF
Add a small Rust function named \`${fn_name}\` to a new file \`src/lib.rs\` (create the directory if needed). The function ${fn_desc}. Write a correct, idiomatic implementation. Then stage the file and create exactly one git commit following the project's commit conventions in CLAUDE.md. Do not push. Do not run any other commands. Do not run cargo.
TASK_EOF
}

# ---------------------------------------------------------------------------
# Temp-dir bookkeeping + cleanup trap.
# ---------------------------------------------------------------------------
TRIAL_DIRS=()
cleanup() {
    for d in "${TRIAL_DIRS[@]:-}"; do
        [ -n "$d" ] && [ -d "$d" ] && rm -rf "$d"
    done
}
trap cleanup EXIT INT TERM

SEED_REF=""   # set per trial to the seed commit SHA, used by the grader.

# ---------------------------------------------------------------------------
# run_headless_agent <task> <trial_dir>
# The single per-vendor headless invocation. Both vendors run with cwd =
# <trial_dir> (so the commit lands in the right repo) and have permissions /
# sandbox bypassed for the unattended run. Failures are tolerated by the caller
# (the grader reads the LANDED change, whatever the agent's exit code). The
# agent's stdout+stderr is captured to <trial_dir>/.agent.log.
# ---------------------------------------------------------------------------
run_headless_agent() {
    local task="$1" trial_dir="$2"
    case "$VENDOR" in
        codex)
            # codex-cli 0.139.0: `codex exec [PROMPT]` is the non-interactive form;
            # --dangerously-bypass-approvals-and-sandbox skips all approval prompts and
            # sandboxing (intended for externally-sandboxed automation). Verified via
            # `codex exec --help`. cwd = trial_dir so the commit lands in the right repo.
            # </dev/null is REQUIRED: codex exec reads/appends piped stdin as a <stdin>
            # block and BLOCKS waiting on it if stdin is a non-/dev/null fd -- redirecting
            # from /dev/null makes the headless run deterministic (verified: with an open
            # stdin it hangs on "Reading additional input from stdin...").
            (
                cd "$trial_dir"
                "$AGENT_BIN" exec \
                    --dangerously-bypass-approvals-and-sandbox \
                    "$task" \
                    </dev/null >"$trial_dir/.agent.log" 2>&1
            ) || true
            ;;
        claude|*)
            (
                cd "$trial_dir"
                "$AGENT_BIN" -p "$task" \
                    --permission-mode bypassPermissions \
                    </dev/null >"$trial_dir/.agent.log" 2>&1
            ) || true
            ;;
    esac
}

# ---------------------------------------------------------------------------
# run_trial <arm> <iteration-index> -> appends one CSV row, echoes a summary line.
# CSV columns: timestamp,vendor,arm,landed_compliant,gate_save,had_commit,fn_name
# (mirrors the I1 runner's shape; subject -> fn_name for I2; vendor added for the
# cross-vendor test -- existing columns keep their meaning).
# ---------------------------------------------------------------------------
run_trial() {
    local arm="$1" idx="$2"
    local variant="${TASK_VARIANTS[$(( (idx - 1) % ${#TASK_VARIANTS[@]} ))]}"
    local fn_name="${variant%%|*}"

    local trial_dir
    trial_dir="$(mktemp -d "${TMPDIR:-/tmp}/aida-ablation-i2-${arm}-XXXXXX")"
    TRIAL_DIRS+=("$trial_dir")

    # Fresh throwaway repo.
    git -C "$trial_dir" init -q -b main
    git -C "$trial_dir" config user.email "ablation@aida.test"
    git -C "$trial_dir" config user.name "AIDA Ablation"
    git -C "$trial_dir" config commit.gpgsign false

    write_trial_claude_md "$trial_dir/CLAUDE.md"
    write_trial_agents_md "$trial_dir/AGENTS.md"
    git -C "$trial_dir" add CLAUDE.md AGENTS.md
    git -C "$trial_dir" commit -q -m "chore: seed repo" --no-verify
    SEED_REF="$(git -C "$trial_dir" rev-parse HEAD)"

    # Install the gate + a rejection counter so we can detect gate saves. The
    # wrapper records each rejection (hook exit != 0) to a sidecar file, then
    # delegates to the genuine gate hook unchanged. Arm R: NO gate installed.
    local reject_log="$trial_dir/.gate-rejections"
    : > "$reject_log"
    if [ "$arm" = "G" ]; then
        mkdir -p "$trial_dir/.git/hooks"
        write_gate_hook "$trial_dir/.git/hooks/pre-commit.real"
        cat > "$trial_dir/.git/hooks/pre-commit" <<HOOK_WRAPPER
#!/usr/bin/env bash
# Ablation wrapper: delegate to the real trace-coverage gate, recording rejections.
"\$(dirname "\$0")/pre-commit.real" "\$@"
rc=\$?
if [ "\$rc" -ne 0 ]; then
    echo "reject" >> "$reject_log"
fi
exit \$rc
HOOK_WRAPPER
        chmod +x "$trial_dir/.git/hooks/pre-commit"
    fi

    local task
    task="$(build_task "$idx")"

    # Run the selected vendor's headless agent in the trial repo. Permissions /
    # sandbox bypassed so the unattended run can write files + commit; failures
    # are tolerated (the grader reads the landed change, whatever the exit code).
    run_headless_agent "$task" "$trial_dir"

    # ---- deterministic grading of the LANDED .rs change ----
    local had_commit=0 landed_compliant=0 gate_save=0 grade=-1
    local head_subject
    head_subject="$(git -C "$trial_dir" log -1 --pretty=%s 2>/dev/null || true)"
    if [ "$head_subject" != "chore: seed repo" ] && [ -n "$head_subject" ]; then
        had_commit=1
    fi
    grade="$(grade_rs_change "$trial_dir")"
    [ "$grade" = "1" ] && landed_compliant=1

    # Gate save: Arm G only -- the gate rejected at least one attempt AND a
    # compliant .rs change ultimately landed (the gate forced the fix).
    local rejections=0
    [ -f "$reject_log" ] && rejections="$(wc -l < "$reject_log" | tr -d ' ')"
    if [ "$arm" = "G" ] && [ "$rejections" -gt 0 ] && [ "$landed_compliant" = "1" ]; then
        gate_save=1
    fi

    # CSV row.
    local ts
    ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s,%s,%s,%s,%s,%s,"%s"\n' \
        "$ts" "$VENDOR" "$arm" "$landed_compliant" "$gate_save" "$had_commit" "$fn_name" >> "$OUT"

    printf '  [%s arm %s] fn=%s commit=%s compliant=%s gate_save=%s rejections=%s\n' \
        "$VENDOR" "$arm" "$fn_name" "$had_commit" "$landed_compliant" "$gate_save" "$rejections"
}

# ---------------------------------------------------------------------------
# --dry-run: prove the harness is correct without a headless run. Exercises the
# grader self-check and the gate hook against a synthetic staged repo, so a
# BLOCKED-on-run situation can still demonstrate a correct, runnable harness.
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    echo "gate-vs-rule I2 (trace-coverage) -- DRY RUN (no headless agent)"
    echo "  vendor: $VENDOR  (agent binary: $AGENT_BIN)"
    echo
    grader_self_check
    echo
    echo "gate hook self-check:"
    dry_dir="$(mktemp -d "${TMPDIR:-/tmp}/aida-ablation-i2-dry-XXXXXX")"
    TRIAL_DIRS+=("$dry_dir")
    git -C "$dry_dir" init -q -b main
    git -C "$dry_dir" config user.email "ablation@aida.test"
    git -C "$dry_dir" config user.name "AIDA Ablation"
    git -C "$dry_dir" config commit.gpgsign false
    write_gate_hook "$dry_dir/.git/hooks/pre-commit"
    # Case 1: .rs change WITHOUT a trace comment -> gate must REJECT.
    printf 'fn double(x: i32) -> i32 { x * 2 }\n' > "$dry_dir/lib.rs"
    git -C "$dry_dir" add lib.rs
    if git -C "$dry_dir" commit -q -m "feat: no trace" 2>/dev/null; then
        echo "  case untagged-change: FAIL (gate allowed an untagged .rs change)"
    else
        echo "  case untagged-change: PASS (gate rejected the untagged .rs change)"
    fi
    # Case 2: .rs change WITH a trace comment -> gate must ALLOW.
    printf '// trace:TASK-700 | ai:claude\nfn double(x: i32) -> i32 { x * 2 }\n' > "$dry_dir/lib.rs"
    git -C "$dry_dir" add lib.rs
    if git -C "$dry_dir" commit -q -m "feat: tagged (TASK-700)" 2>/dev/null; then
        echo "  case tagged-change:   PASS (gate allowed the trace-tagged .rs change)"
    else
        echo "  case tagged-change:   FAIL (gate rejected a properly tagged .rs change)"
    fi
    echo
    echo "Task variants available: ${#TASK_VARIANTS[@]}"
    echo "Sample task (iteration 1):"
    build_task 1 | sed 's/^/  | /'
    echo
    echo "DRY RUN complete -- harness is wired. Run without --dry-run for a real trial."
    exit 0
fi

# ---------------------------------------------------------------------------
# Drive the trials.
# ---------------------------------------------------------------------------
if [ ! -f "$OUT" ]; then
    echo "timestamp,vendor,arm,landed_compliant,gate_save,had_commit,fn_name" > "$OUT"
fi

declare -a ARMS_TO_RUN
if [ -n "$ARM" ]; then
    ARMS_TO_RUN=("$ARM")
else
    ARMS_TO_RUN=(R G)
fi

echo "gate-vs-rule I2 ablation (trace-coverage, high attention-distance) -- $TRIALS trial(s) per arm"
echo "  vendor: $VENDOR"
echo "  arms  : ${ARMS_TO_RUN[*]}"
echo "  out   : $OUT"
echo "  agent : $AGENT_BIN"
echo

for arm in "${ARMS_TO_RUN[@]}"; do
    echo "Arm $arm:"
    for ((i = 1; i <= TRIALS; i++)); do
        echo "  trial $i/$TRIALS ..."
        run_trial "$arm" "$i"
    done
    echo
done

# ---------------------------------------------------------------------------
# Summary + pre-registered interpretation (design doc Sec.5, at high attention-distance).
# ---------------------------------------------------------------------------
pct() {
    local n="$1" d="$2"
    if [ "$d" -eq 0 ]; then echo 0; else echo $(( (n * 100 + d / 2) / d )); fi
}

r_total=0 r_compliant=0
g_total=0 g_compliant=0 g_saves=0
while IFS=, read -r _ts vend arm comp save _had _fn; do
    [ "$arm" = "arm" ] && continue          # header
    [ "$vend" != "$VENDOR" ] && continue    # only summarize the selected vendor's rows
    case "$arm" in
        R) r_total=$((r_total + 1)); [ "$comp" = "1" ] && r_compliant=$((r_compliant + 1)) ;;
        G) g_total=$((g_total + 1)); [ "$comp" = "1" ] && g_compliant=$((g_compliant + 1))
           [ "$save" = "1" ] && g_saves=$((g_saves + 1)) ;;
    esac
done < "$OUT"

r_rate="$(pct "$r_compliant" "$r_total")"
g_rate="$(pct "$g_compliant" "$g_total")"
save_rate="$(pct "$g_saves" "$g_total")"

echo "=============================================================="
echo " RESULTS for vendor=$VENDOR (rows in $OUT)"
echo "=============================================================="
echo "  Arm R (rule-only) landed-compliance : ${r_rate}%  (${r_compliant}/${r_total})"
echo "  Arm G (gate)      landed-compliance : ${g_rate}%  (${g_compliant}/${g_total})"
echo "  Arm G gate-save rate                : ${save_rate}%  (${g_saves}/${g_total})"
echo
echo "  Pre-registered interpretation (design doc Sec.5, at HIGH attention-distance),"
echo "  keyed on Arm-R landed-compliance for this semantic/buried invariant:"
echo "    < 95%  -> P1 HOLDS as attention-distance rises (the conjecture is supported;"
echo "              the gate does real work -- gate-saves should be > 0)"
echo "    >= 95% -> the attention-distance conjecture is itself WEAKENED (even a buried,"
echo "              semantic rule held) -- a strong, honest negative."
echo

if [ "$r_total" -gt 0 ]; then
    if [ "$r_rate" -ge 95 ]; then
        verdict="CONJECTURE WEAKENED (Arm-R ${r_rate}% >= 95% even at high attention-distance)"
    else
        verdict="P1 HOLDS as attention-distance rises (Arm-R ${r_rate}% < 95%)"
    fi
    echo "  >>> Verdict from this CSV: $verdict"
else
    echo "  >>> No Arm-R trials in CSV -- run with --arm R (or both) for a verdict."
fi
echo
echo "  NOTE: a smoke run (1 trial/arm) is a MECHANISM check, not evidence."
echo "  Full run (operator opt-in, ~20 headless runs):"
echo "    scripts/ablations/gate-vs-rule-i2.sh --trials 10"
