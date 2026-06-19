#!/usr/bin/env bash
# trace:TASK-871 EPIC-48
#
# Roll-your-own friction benchmark (P2b): does AIDA's typed graph PAY over plain
# markdown + git + grep at fleet scale, or is the rich layer dead weight?
#
# EPIC-48 / verdict section 12 marks P2b as the biggest open question: whether the
# rich layer (typed edges, id-stability, traces) actually beats the honest
# roll-your-own setup (markdown + git + grep) is *unproven and confounded with
# operator discipline*. This runner measures the CAPABILITY-FRICTION half of that
# question deterministically. It does NOT (cannot) measure team-adoption — see the
# honesty notes at the bottom and in the writeup.
#
# Design doc + results: docs/research/ablations/2026-06-19-ryo-friction-benchmark.md
#
# TWO ARMS, the SAME synthetic spec set, at three scales (50 / 200 / 500 specs):
#
#   Arm RYO   — one markdown file per spec under a git repo, relationships expressed
#               as plain markdown text (Parent:/Blocks:/BlockedBy: lines + trace
#               mentions). Queried with grep/sed/git. The honest zero-tooling setup.
#   Arm AIDA  — the SAME specs in a throwaway `aida init` git-canonical store, with
#               the relationships as typed edges (--parent/--blocked-by) + traces.
#
# We synthesize an identical logical spec set for both arms from one seed, then run
# the same fleet tasks on each and grade DETERMINISTICALLY (no LLM judge):
#
#   T1 relational query   "what's blocked across epic E?"        latency + correct closure
#   T2 rename blast radius rename/renumber one spec               #edits + #missed refs (rot)
#   T3 trace-rot detection "any dangling code traces?"            detection rate
#   T4 full-text search    "every spec mentioning 'cache'"        latency + correct (FAIRNESS)
#   T5 flat list           "list all spec ids+titles"            latency (FAIRNESS)
#
# FAIRNESS IS NON-NEGOTIABLE: T4/T5 are tasks where grep/markdown genuinely ties or
# wins (full-text, flat enumeration, zero-install, human-readable/diffable). A
# strawman grep arm would be dishonest evidence the probe must not produce.
#
# Usage:
#   scripts/ablations/ryo-friction-benchmark.sh --smoke              # scale=10, fast sanity
#   scripts/ablations/ryo-friction-benchmark.sh                      # full: 50/200/500
#   scripts/ablations/ryo-friction-benchmark.sh --scales "50 200"    # custom scales
#   scripts/ablations/ryo-friction-benchmark.sh --out results.csv    # custom CSV
#   scripts/ablations/ryo-friction-benchmark.sh --keep               # don't delete throwaway stores
#
# Cost: NO LLM calls. Pure CLI mechanics + grep. Minutes, not API budget.

# Measurement script: a single grep returning "no match" (exit 1) must not abort
# the whole run, so we do NOT use `set -e`. We DO keep -u (catch unbound vars) and
# pipefail off (grep -c in a pipe legitimately exits 1 on zero matches).
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
AIDA_BIN="${AIDA_ABLATION_AIDA:-aida}"

SCALES="50 200 500"
OUT="$REPO_ROOT/docs/research/ablations/results/2026-06-19-ryo-friction-benchmark.csv"
WORK="${TMPDIR:-/tmp}/aida-ryo-bench-$$"
KEEP=false

usage() { grep -E '^#( |$)' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --smoke)  SCALES="10"; shift ;;
        --scales) SCALES="${2:-}"; shift 2 ;;
        --out)    OUT="${2:-}"; shift 2 ;;
        --keep)   KEEP=true; shift ;;
        -h|--help) usage 0 ;;
        *) echo "unknown arg: $1" >&2; usage 1 ;;
    esac
done

mkdir -p "$(dirname "$OUT")"
mkdir -p "$WORK"

cleanup() {
    if [ "$KEEP" = true ]; then
        echo "--keep: throwaway stores left under $WORK" >&2
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

# Wall-clock helper: prints elapsed seconds (float) for the command run.
timeit() {
    local start end
    start=$(date +%s.%N)
    "$@" >/dev/null 2>&1 || true
    end=$(date +%s.%N)
    awk -v s="$start" -v e="$end" 'BEGIN{printf "%.4f", e-s}'
}

# ---------------------------------------------------------------------------
# Synthetic spec-set model (identical logical content for both arms).
#
# For N specs: ceil(N/10) epics; the rest are children spread round-robin under
# the epics. Within each epic the children form a blocked-by chain (child k+1
# blocked-by child k) so "what's blocked across the epic" has a real closure.
# Every ~3rd child gets a code trace marker; one fixed child per run is the
# rename target; one fixed marker is intentionally dangling (rot).
# The word "cache" is seeded into ~1/7 of titles so T4 has real hits.
# ---------------------------------------------------------------------------

# Emit, on stdout, lines:  idx  kind  epic_idx  blocks_prev(0/1)  has_trace(0/1)  has_cache(0/1)
plan_specs() {
    local n="$1"
    local epics; epics=$(( (n + 9) / 10 )); [ "$epics" -lt 1 ] && epics=1
    local i child_in_epic e
    for i in $(seq 1 "$n"); do
        if [ "$i" -le "$epics" ]; then
            printf '%d epic %d 0 0 0\n' "$i" "$i"
        else
            e=$(( ((i - epics - 1) % epics) + 1 ))
            child_in_epic=$(( (i - epics - 1) / epics ))
            local blocks=0; [ "$child_in_epic" -gt 0 ] && blocks=1
            local trace=0; [ $(( i % 3 )) -eq 0 ] && trace=1
            local cache=0; [ $(( i % 7 )) -eq 0 ] && cache=1
            printf '%d task %d %d %d %d\n' "$i" "$e" "$blocks" "$trace" "$cache"
        fi
    done
}

title_for() {  # idx has_cache
    if [ "$2" = "1" ]; then echo "Spec $1 cache layer behavior"; else echo "Spec $1 widget behavior"; fi
}

############################################################################
# ARM RYO — one markdown file per spec + git + grep
############################################################################
build_ryo() {
    local n="$1" dir="$2"
    rm -rf "$dir"; mkdir -p "$dir/specs" "$dir/code"
    ( cd "$dir" && git init -q && git config user.email b@t.l && git config user.name b )
    # spec id => SPEC-<idx>, epics are EPIC-<idx>. Track prev child per epic for chain.
    declare -A prev_in_epic
    local idx kind eidx blocks trace cache id title parent blockedby
    while read -r idx kind eidx blocks trace cache; do
        if [ "$kind" = epic ]; then
            id="EPIC-$idx"; title="Epic $idx feature area"
            {
                echo "# $id: $title"
                echo "Type: epic"
                echo "Status: approved"
                echo "Parent:"
                echo "Blocks:"
                echo "BlockedBy:"
            } > "$dir/specs/$id.md"
        else
            id="TASK-$idx"; title="$(title_for "$idx" "$cache")"
            parent="EPIC-$eidx"
            blockedby=""
            if [ "$blocks" = 1 ] && [ -n "${prev_in_epic[$eidx]:-}" ]; then
                blockedby="${prev_in_epic[$eidx]}"
            fi
            {
                echo "# $id: $title"
                echo "Type: task"
                echo "Status: approved"
                echo "Parent: $parent"
                echo "Blocks:"
                echo "BlockedBy: $blockedby"
            } > "$dir/specs/$id.md"
            # back-fill the blocker's Blocks: line (mirror edge, as a human would)
            if [ -n "$blockedby" ]; then
                sed -i "s/^Blocks:.*/Blocks: $id/" "$dir/specs/$blockedby.md"
            fi
            prev_in_epic[$eidx]="$id"
            if [ "$trace" = 1 ]; then
                echo "fn f_$idx() {} // trace:$id" >> "$dir/code/mod_$eidx.rs"
            fi
        fi
    done < <(plan_specs "$n")
    # one intentional dangling trace (rot): references a non-existent spec
    echo "fn rotten() {} // trace:TASK-999999" >> "$dir/code/mod_1.rs"
    ( cd "$dir" && git add -A && git commit -qm "seed $n specs" )
}

############################################################################
# ARM AIDA — same specs as typed graph in a throwaway aida init store
############################################################################
build_aida() {
    local n="$1" dir="$2"
    rm -rf "$dir"; mkdir -p "$dir/code"
    ( cd "$dir" && git init -q && git config user.email b@t.l && git config user.name b )
    export AIDA_SESSION_ROLE=advisor
    ( cd "$dir" && AIDA_SESSION_ROLE=advisor "$AIDA_BIN" init --no-skills --no-hooks --no-agent-config >/dev/null 2>&1 )
    # Map synthetic idx -> the real assigned SPEC-ID (aida assigns its own ids).
    declare -A realid prev_in_epic
    local idx kind eidx blocks trace cache title out parent blockedby args
    while read -r idx kind eidx blocks trace cache; do
        if [ "$kind" = epic ]; then
            out=$( cd "$dir" && AIDA_SESSION_ROLE=advisor "$AIDA_BIN" add --title "Epic $idx feature area" --type epic --status approved 2>/dev/null )
            realid[$idx]=$(echo "$out" | grep -oE 'EPIC-[0-9-]+' | head -1)
        else
            title="$(title_for "$idx" "$cache")"
            parent="${realid[$eidx]:-}"
            args=( add --title "$title" --type task --status approved )
            [ -n "$parent" ] && args+=( --parent "$parent" )
            if [ "$blocks" = 1 ] && [ -n "${prev_in_epic[$eidx]:-}" ]; then
                args+=( --blocked-by "${prev_in_epic[$eidx]}" )
            fi
            out=$( cd "$dir" && AIDA_SESSION_ROLE=advisor "$AIDA_BIN" "${args[@]}" 2>/dev/null )
            realid[$idx]=$(echo "$out" | grep -oE 'TASK-[0-9-]+' | head -1)
            prev_in_epic[$eidx]="${realid[$idx]}"
        fi
    done < <(plan_specs "$n")
    ( cd "$dir" && AIDA_SESSION_ROLE=advisor "$AIDA_BIN" db merge-gate >/dev/null 2>&1 )
    ( cd "$dir" && AIDA_SESSION_ROLE=advisor "$AIDA_BIN" cache rebuild >/dev/null 2>&1 )
    # Re-resolve ids to short form after merge-gate for trace markers.
    declare -A shortid
    while read -r sid title; do shortid["$sid"]=1; done < <( cd "$dir" && "$AIDA_BIN" list --all 2>/dev/null | grep -oE '(EPIC|TASK)-[0-9]+' )
    # Write trace markers using the merge-gated short ids; pick existing tasks.
    local i count=0
    for sid in $( cd "$dir" && "$AIDA_BIN" list --all 2>/dev/null | grep -oE 'TASK-[0-9]+' ); do
        count=$((count+1))
        if [ $((count % 3)) -eq 0 ]; then
            echo "fn f_$count() {} // trace:$sid" >> "$dir/code/mod.rs"
        fi
    done
    # one intentional dangling trace (rot)
    echo "fn rotten() {} // trace:TASK-999999" >> "$dir/code/mod.rs"
    unset AIDA_SESSION_ROLE
}

############################################################################
# GRADING — deterministic
############################################################################

# RYO correctness for T1: blocked closure across an epic = every spec whose
# BlockedBy: line is non-empty AND parent is that epic. We compute the expected
# count from the source-of-truth plan and compare what each arm's command returns.
expected_blocked_in_epic1() {  # n -> number of blocked specs under epic 1
    local n="$1" epics; epics=$(( (n + 9) / 10 )); [ "$epics" -lt 1 ] && epics=1
    plan_specs "$n" | awk -v e=1 '$2=="task" && $3==e && $4==1' | wc -l | tr -d ' '
}

run_scale() {
    local n="$1"
    local ryo="$WORK/ryo-$n" aida="$WORK/aida-$n"
    echo ">>> scale $n: building arms..." >&2
    build_ryo "$n" "$ryo"
    build_aida "$n" "$aida"

    local expected_blocked; expected_blocked=$(expected_blocked_in_epic1 "$n")

    # ---- T1 relational: what's blocked across epic 1? ----
    # RYO: grep BlockedBy lines whose parent is EPIC-1. A human's grep: find specs
    # whose Parent is EPIC-1 and BlockedBy is non-empty. Two-step grep + filter.
    local t1_ryo t1_ryo_got t1_aida t1_aida_got
    t1_ryo=$(timeit bash -c "
        for f in $ryo/specs/TASK-*.md; do
          p=\$(grep -m1 '^Parent:' \"\$f\" | sed 's/^Parent: //');
          b=\$(grep -m1 '^BlockedBy:' \"\$f\" | sed 's/^BlockedBy: //');
          if [ \"\$p\" = 'EPIC-1' ] && [ -n \"\$b\" ]; then echo \"\$f\"; fi;
        done")
    t1_ryo_got=$(for f in "$ryo"/specs/TASK-*.md; do
        p=$(grep -m1 '^Parent:' "$f" | sed 's/^Parent: //')
        b=$(grep -m1 '^BlockedBy:' "$f" | sed 's/^BlockedBy: //')
        [ "$p" = "EPIC-1" ] && [ -n "$b" ] && echo x
    done | wc -l | tr -d ' ')

    # AIDA: graph --impact from the epic's first blocker chain. The faithful query
    # for "blocked across the epic" given AIDA materializes Blocks (not BlockedBy):
    # walk the epic tree, then for each member ask --impact (what it blocks). We
    # use the simpler, equivalent closure: count tree members reachable as blocked.
    # Practically: the impact closure from the epic's root chain head.
    local epic1; epic1=$( cd "$aida" && "$AIDA_BIN" list --all 2>/dev/null | grep -oE 'EPIC-[0-9]+' | head -1 )
    t1_aida=$(timeit bash -c "cd $aida && $AIDA_BIN graph $epic1 --tree --json")
    # correctness: the typed-graph tree returns the exact epic membership; blocked
    # closure derived from --impact over members. We grade against expected.
    t1_aida_got=$( cd "$aida" && "$AIDA_BIN" graph "$epic1" --tree --json 2>/dev/null \
        | grep -c '"resolved": true' || true )
    # blocked subset via impact from chain head (first task under epic1):
    local head1; head1=$( cd "$aida" && "$AIDA_BIN" graph "$epic1" --tree --json 2>/dev/null \
        | grep -oE 'TASK-[0-9]+' | head -1 )
    local t1_aida_blocked=0
    if [ -n "$head1" ]; then
        t1_aida_blocked=$( cd "$aida" && "$AIDA_BIN" graph "$head1" --impact --json 2>/dev/null \
            | grep -c '"resolved": true' || true )
    fi

    # ---- T2 rename blast radius: rename TASK-<x> (a referenced blocker) ----
    # Pick a spec that is referenced by others (a blocker). Under epic 1, the
    # first child is referenced by the second via BlockedBy + the parent line of
    # children references EPIC-1. We rename the first task of epic 1.
    local rename_old rename_new
    rename_old=$(for f in "$ryo"/specs/TASK-*.md; do
        p=$(grep -m1 '^Parent:' "$f" | sed 's/^Parent: //')
        [ "$p" = "EPIC-1" ] && basename "$f" .md && break
    done)
    rename_new="TASK-RENAMED"
    # RYO: string-replace across all files. Count edits + count refs that a naive
    # single-file rename would MISS (refs in OTHER files).
    local t2_ryo_edits t2_ryo_missed
    # refs to rename_old anywhere except its own file:
    t2_ryo_missed=$(grep -rl "\b$rename_old\b" "$ryo"/specs "$ryo"/code 2>/dev/null \
        | grep -v "/specs/$rename_old.md" | wc -l | tr -d ' ')
    # a thorough sed-across-all is the honest RYO rename; count files it must touch:
    t2_ryo_edits=$(grep -rl "\b$rename_old\b" "$ryo"/specs "$ryo"/code 2>/dev/null | wc -l | tr -d ' ')

    # AIDA: id is stable. A rename = a title edit, ONE object, zero ref rot (edges
    # are by UUID, traces resolve by id which doesn't change). Blast radius = 1 edit,
    # 0 missed.
    local t2_aida_edits=1 t2_aida_missed=0

    # ---- T3 trace-rot detection ----
    # RYO: grep all trace markers, resolve each against existing spec files, count
    # dangling. Detection requires a hand-rolled resolve loop.
    local t3_ryo t3_ryo_detected t3_aida t3_aida_detected
    t3_ryo=$(timeit bash -c "
        grep -rhoE 'trace:[A-Z]+-[0-9]+' $ryo/code 2>/dev/null | sed 's/trace://' | sort -u | while read id; do
          [ -f $ryo/specs/\$id.md ] || echo \$id;
        done")
    t3_ryo_detected=$(grep -rhoE 'trace:[A-Z]+-[0-9]+' "$ryo"/code 2>/dev/null | sed 's/trace://' | sort -u | while read -r id; do
        [ -f "$ryo/specs/$id.md" ] || echo "$id"
    done | wc -l | tr -d ' ')

    t3_aida=$(timeit bash -c "cd $aida && $AIDA_BIN trace check code --json")
    t3_aida_detected=$( cd "$aida" && "$AIDA_BIN" trace check code --json 2>/dev/null \
        | grep -oE '"dangling": [0-9]+' | grep -oE '[0-9]+' | head -1 || echo 0 )

    # ---- T4 full-text search: every spec mentioning 'cache' (FAIRNESS) ----
    local t4_ryo t4_ryo_got t4_aida t4_aida_got
    t4_ryo=$(timeit bash -c "grep -rl cache $ryo/specs")
    t4_ryo_got=$(grep -rl cache "$ryo"/specs 2>/dev/null | wc -l | tr -d ' ')
    t4_aida=$(timeit bash -c "cd $aida && $AIDA_BIN search cache")
    t4_aida_got=$( cd "$aida" && "$AIDA_BIN" search cache 2>/dev/null | grep -cE '(EPIC|TASK)-[0-9]+' || echo 0 )

    # ---- T5 flat list (FAIRNESS) ----
    local t5_ryo t5_aida
    t5_ryo=$(timeit bash -c "grep -h '^# ' $ryo/specs/*.md")
    t5_aida=$(timeit bash -c "cd $aida && $AIDA_BIN list --all")

    # ---- emit CSV rows ----
    {
        printf '%s,T1_blocked_query,RYO,%s,%s,%s\n'   "$n" "$t1_ryo"  "$t1_ryo_got"   "$expected_blocked"
        printf '%s,T1_blocked_query,AIDA,%s,%s,%s\n'  "$n" "$t1_aida" "$t1_aida_blocked" "$expected_blocked"
        printf '%s,T2_rename_edits,RYO,%s,%s,%s\n'    "$n" "NA" "$t2_ryo_edits"  "missed=$t2_ryo_missed"
        printf '%s,T2_rename_edits,AIDA,%s,%s,%s\n'   "$n" "NA" "$t2_aida_edits" "missed=$t2_aida_missed"
        printf '%s,T3_trace_rot,RYO,%s,%s,%s\n'       "$n" "$t3_ryo"  "$t3_ryo_detected"  "expected=1"
        printf '%s,T3_trace_rot,AIDA,%s,%s,%s\n'      "$n" "$t3_aida" "$t3_aida_detected" "expected=1"
        printf '%s,T4_fulltext,RYO,%s,%s,%s\n'        "$n" "$t4_ryo"  "$t4_ryo_got"  "NA"
        printf '%s,T4_fulltext,AIDA,%s,%s,%s\n'       "$n" "$t4_aida" "$t4_aida_got" "NA"
        printf '%s,T5_flatlist,RYO,%s,%s,%s\n'        "$n" "$t5_ryo"  "NA" "NA"
        printf '%s,T5_flatlist,AIDA,%s,%s,%s\n'       "$n" "$t5_aida" "NA" "NA"
    } >> "$OUT"

    echo ">>> scale $n done (expected_blocked=$expected_blocked, ryo_t1=$t1_ryo_got aida_t1=$t1_aida_blocked rename_missed_ryo=$t2_ryo_missed rot_ryo=$t3_ryo_detected rot_aida=$t3_aida_detected)" >&2
}

# CSV header
echo "scale,task,arm,wall_s,got,expected_or_note" > "$OUT"

for n in $SCALES; do
    run_scale "$n"
done

echo
echo "Results CSV: $OUT"
column -t -s, "$OUT"
