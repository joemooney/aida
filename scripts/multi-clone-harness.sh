#!/usr/bin/env bash
# trace:STORY-636 EPIC-46
# trace:STORY-642 (multi-host: cross-host TTL coordination, AIDA_HOST_OVERRIDE)
#
# Same-host multi-clone test harness for the AIDA multi-user catalog.
#
# Makes docs/testing/multi-user-test-cases.md RUNNABLE: it spins up a throwaway
# bare origin + two clones on this host (with an isolated $HOME so the real
# ~/.aida is never touched), runs the MU-### cases, and asserts each one against
# the catalog's documented Expected behavior.
#
# Cases declare EXPECT=pass or EXPECT=known-gap:
#   - EXPECT=pass that FAILS  -> the suite FAILS (exit non-zero).
#   - EXPECT=known-gap        -> reported as GAP (expected until shared
#                                coordination lands); does NOT fail the suite.
#   - EXPECT=known-gap that PASSES -> "GAP CLOSED -- flip EXPECT to pass"
#                                (does not fail; a signal to update this script).
#
# The cross-clone coordination cases from EPIC-46 — MU-504 (leases, STORY-637),
# MU-505 (drain lock) + MU-506 (solo lock, both STORY-638) — were the original
# red-by-design gaps: leases and drain/solo locks were per-clone-local, so they
# provided zero cross-clone safety. The shared coordination registry on the
# aida-store branch (coordination/leases/<scope>.toml + coordination/{drain,solo}
# .lock.toml) closed all three; they are now EXPECT=pass.
#
# Usage:
#   scripts/multi-clone-harness.sh                 # run all cases
#   scripts/multi-clone-harness.sh all             # run all cases
#   scripts/multi-clone-harness.sh MU-101 MU-203   # run specific cases
#   scripts/multi-clone-harness.sh --list          # list cases
#   scripts/multi-clone-harness.sh --keep all      # keep the workdir for debugging
#
# Environment:
#   AIDA_BIN  path to the aida binary (default: ./target/release/aida).

set -euo pipefail

# --------------------------------------------------------------------------
# Resolve the aida binary to an absolute path; build it if missing.
# --------------------------------------------------------------------------
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AIDA_BIN="${AIDA_BIN:-$REPO_ROOT/target/release/aida}"

if [[ ! -x "$AIDA_BIN" ]]; then
    echo "aida binary not found at $AIDA_BIN -- building..." >&2
    (cd "$REPO_ROOT" && cargo build --release -p aida-cli --bin aida)
fi
# Absolutize.
AIDA_BIN="$(cd "$(dirname "$AIDA_BIN")" && pwd)/$(basename "$AIDA_BIN")"
if [[ ! -x "$AIDA_BIN" ]]; then
    echo "FATAL: aida binary still not executable at $AIDA_BIN" >&2
    exit 1
fi

# --------------------------------------------------------------------------
# Colors (only when stdout is a tty).
# --------------------------------------------------------------------------
if [[ -t 1 ]]; then
    C_RED=$'\033[31m'; C_GRN=$'\033[32m'; C_YEL=$'\033[33m'
    C_BLU=$'\033[34m'; C_DIM=$'\033[2m'; C_BOLD=$'\033[1m'; C_RST=$'\033[0m'
else
    C_RED=""; C_GRN=""; C_YEL=""; C_BLU=""; C_DIM=""; C_BOLD=""; C_RST=""
fi

# --------------------------------------------------------------------------
# Workdir + isolation. Everything (origin, clones, fake HOME) lives under a
# single mktemp -d so the real ~/.aida (node registry, agents.toml, roles) is
# NEVER touched.
# --------------------------------------------------------------------------
KEEP=0
WORKDIR=""
ORIGIN=""
CLONE_A=""
CLONE_B=""

cleanup() {
    if [[ "$KEEP" == "1" ]]; then
        echo
        echo "${C_DIM}--keep set; workdir preserved at: $WORKDIR${C_RST}"
        return
    fi
    if [[ -n "$WORKDIR" && -d "$WORKDIR" ]]; then
        rm -rf "$WORKDIR"
    fi
}
trap cleanup EXIT

# --------------------------------------------------------------------------
# Result accounting.
# --------------------------------------------------------------------------
SUITE_FAIL=0     # set to 1 when an EXPECT=pass case fails -> suite exits non-zero
COUNT_PASS=0
COUNT_FAIL=0
COUNT_GAP=0
COUNT_GAP_CLOSED=0

# Per-case scratch: the first assertion failure detail.
CASE_FAIL_DETAIL=""

# --------------------------------------------------------------------------
# Printers.
# --------------------------------------------------------------------------
log()  { echo "${C_DIM}    $*${C_RST}"; }
pass() { echo "  ${C_GRN}PASS${C_RST}  ${C_BOLD}$1${C_RST}  ${C_DIM}$2${C_RST}"; }
fail() { echo "  ${C_RED}FAIL${C_RST}  ${C_BOLD}$1${C_RST}  ${C_DIM}$2${C_RST}"; }
gap()  { echo "  ${C_YEL}GAP ${C_RST}  ${C_BOLD}$1${C_RST}  ${C_DIM}$2 (expected until shared coordination lands)${C_RST}"; }
gapclosed() { echo "  ${C_BLU}GAP CLOSED${C_RST}  ${C_BOLD}$1${C_RST}  ${C_DIM}$2 -- flip EXPECT to pass${C_RST}"; }

# --------------------------------------------------------------------------
# run_in <dir> <cmd...>  -- run a command inside a clone with HOME isolated and
# the aida binary on hand. cd-chained so a failed cd aborts. stdout/stderr of
# the command flow through; the caller captures with $(...).
# --------------------------------------------------------------------------
run_in() {
    local dir="$1"; shift
    ( cd "$dir" && HOME="$WORKDIR/home" "$@" )
}

aida_in() {
    local dir="$1"; shift
    ( cd "$dir" && HOME="$WORKDIR/home" "$AIDA_BIN" "$@" )
}

# --------------------------------------------------------------------------
# Assertions. Each records CASE_FAIL_DETAIL on first failure and returns 1.
# A case function chains them with && so the first failure short-circuits.
# --------------------------------------------------------------------------
assert_eq() {
    local got="$1" want="$2" msg="${3:-}"
    if [[ "$got" == "$want" ]]; then
        return 0
    fi
    CASE_FAIL_DETAIL="${msg:-assert_eq}: got [$got] want [$want]"
    return 1
}

assert_ne() {
    local got="$1" notwant="$2" msg="${3:-}"
    if [[ "$got" != "$notwant" ]]; then
        return 0
    fi
    CASE_FAIL_DETAIL="${msg:-assert_ne}: got [$got] which should differ from [$notwant]"
    return 1
}

assert_contains() {
    local haystack="$1" needle="$2" msg="${3:-}"
    if [[ "$haystack" == *"$needle"* ]]; then
        return 0
    fi
    CASE_FAIL_DETAIL="${msg:-assert_contains}: [$needle] not found"
    return 1
}

assert_not_contains() {
    local haystack="$1" needle="$2" msg="${3:-}"
    if [[ "$haystack" != *"$needle"* ]]; then
        return 0
    fi
    CASE_FAIL_DETAIL="${msg:-assert_not_contains}: [$needle] unexpectedly present"
    return 1
}

# --------------------------------------------------------------------------
# Shared setup: bare origin + clone A (aida init) + clone B (git clone +
# fresh-clone auto-attach + node acquire). Run once before any case.
# --------------------------------------------------------------------------
SETUP_DONE=0

do_setup() {
    [[ "$SETUP_DONE" == "1" ]] && return 0

    WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/aida-mc-harness.XXXXXX")"
    mkdir -p "$WORKDIR/home"
    ORIGIN="$WORKDIR/origin.git"
    CLONE_A="$WORKDIR/cloneA"
    CLONE_B="$WORKDIR/cloneB"

    echo "${C_BOLD}=== setup ===${C_RST}"
    log "workdir : $WORKDIR"
    log "HOME    : $WORKDIR/home  (real ~/.aida untouched)"
    log "aida    : $AIDA_BIN"

    # git identity for the isolated HOME (so commits work).
    export GIT_AUTHOR_NAME="MC Harness"
    export GIT_AUTHOR_EMAIL="harness@example.com"
    export GIT_COMMITTER_NAME="MC Harness"
    export GIT_COMMITTER_EMAIL="harness@example.com"
    HOME="$WORKDIR/home" git config --global init.defaultBranch main >/dev/null 2>&1 || true
    HOME="$WORKDIR/home" git config --global user.name "MC Harness" >/dev/null 2>&1 || true
    HOME="$WORKDIR/home" git config --global user.email "harness@example.com" >/dev/null 2>&1 || true
    HOME="$WORKDIR/home" git config --global pull.rebase true >/dev/null 2>&1 || true
    HOME="$WORKDIR/home" git config --global advice.diverging false >/dev/null 2>&1 || true

    # Bare origin.
    HOME="$WORKDIR/home" git init --bare -q "$ORIGIN"

    # Clone A: a normal repo wired to origin, with a seed commit on main.
    HOME="$WORKDIR/home" git clone -q "$ORIGIN" "$CLONE_A"
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" git commit -q --allow-empty -m "chore: seed" \
        && HOME="$WORKDIR/home" git branch -M main \
        && HOME="$WORKDIR/home" git push -q -u origin main )

    # aida init in clone A (distributed default): creates + pushes aida-store.
    log "clone A: aida init (distributed)"
    aida_in "$CLONE_A" init --no-skills --no-hooks --no-agent-config >/dev/null 2>&1 \
        || { echo "${C_RED}FATAL: aida init failed in clone A${C_RST}"; aida_in "$CLONE_A" init --no-skills --no-hooks --no-agent-config; exit 1; }
    # Push the store (and any code changes init made) to origin.
    aida_in "$CLONE_A" push >/dev/null 2>&1 || aida_in "$CLONE_A" db sync --push >/dev/null 2>&1 || true
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" git push -q origin main 2>/dev/null ) || true

    # Clone B: fresh git clone of the same origin. First store-reading command
    # auto-attaches the .aida-store worktree (TASK-621). That worktree carries
    # A's per-clone node.toml (it rode the aida-store branch), so on a same-host
    # same-store clone B inherits node id 1; `node acquire --force` allocates B
    # its own distinct id (2) + spec-id block so the two clones don't collide.
    log "clone B: git clone + fresh-clone auto-attach + node acquire --force"
    HOME="$WORKDIR/home" git clone -q "$ORIGIN" "$CLONE_B"
    # Trigger auto-attach via a read.
    aida_in "$CLONE_B" list >/dev/null 2>&1 || true
    aida_in "$CLONE_B" node acquire --force >/dev/null 2>&1 \
        || { echo "${C_RED}FATAL: node acquire --force failed in clone B${C_RST}"; aida_in "$CLONE_B" node acquire --force; exit 1; }

    # Sanity: both clones can list.
    if ! aida_in "$CLONE_A" list >/dev/null 2>&1; then
        echo "${C_RED}FATAL: clone A cannot 'aida list'${C_RST}"; exit 1
    fi
    if ! aida_in "$CLONE_B" list >/dev/null 2>&1; then
        echo "${C_RED}FATAL: clone B cannot 'aida list'${C_RST}"; exit 1
    fi
    log "both clones can 'aida list' OK"
    echo

    SETUP_DONE=1
}

# Sync helper: push from one clone, pull into the other (store + code legs).
push_from() { aida_in "$1" push >/dev/null 2>&1 || aida_in "$1" db sync --push >/dev/null 2>&1 || true; }
pull_into() { aida_in "$1" pull >/dev/null 2>&1 || aida_in "$1" db sync --pull >/dev/null 2>&1 || true; }

# Abort an in-progress store-leg rebase in a clone and hard-reset the store
# worktree onto the fetched origin/aida-store so it's clean for later cases.
recover_store_rebase() {
    local dir="$1"
    ( cd "$dir" && HOME="$WORKDIR/home" git -C .aida-store rebase --abort >/dev/null 2>&1 ) || true
    ( cd "$dir" && HOME="$WORKDIR/home" git -C .aida-store fetch -q origin aida-store >/dev/null 2>&1 ) || true
    ( cd "$dir" && HOME="$WORKDIR/home" git -C .aida-store reset --hard FETCH_HEAD >/dev/null 2>&1 ) || true
}

# Extract a freshly-created spec id from `aida add` output (e.g. "TASK-1-001").
# Falls back to scanning the store objects if stdout doesn't carry it.
add_spec() {
    # $1 dir, $2 title, $3 type
    # Filing as Approved needs advisor authority (TASK-647); a plain non-TTY
    # add --status approved is downgraded to Draft. Set the advisor role so the
    # spec lands Approved (and is therefore lease-able by session start).
    local dir="$1" title="$2" stype="$3" out
    out="$( cd "$dir" && HOME="$WORKDIR/home" AIDA_SESSION_ROLE=advisor \
        "$AIDA_BIN" add --title "$title" --type "$stype" --status approved 2>&1 || true )"
    # Match a spec id like FOO-1-001 / TASK-2-003 / BUG-1.
    grep -oE '[A-Z]+-[0-9]+(-[0-9]+)*' <<<"$out" | head -1
}

# =========================================================================
# CASES
# Each case: declare EXPECT, run logic, set CASE_OK=1 on success (assertions
# passed) or 0 on failure, set CASE_DETAIL to a one-line summary.
# =========================================================================

CASE_OK=0
CASE_DETAIL=""
EXPECT=""

# --- MU-101: distinct node ids for the two clones ------------------------
case_MU-101() {
    EXPECT=pass
    CASE_DETAIL=""
    # Per-clone node identity lives in the attached store worktree at
    # .aida-store/.aida/node.toml (node_id = "N").
    local a_node b_node
    a_node="$(run_in "$CLONE_A" cat .aida-store/.aida/node.toml 2>/dev/null | grep -oE 'node_id *= *"[^"]+"' | head -1 | sed 's/.*"\(.*\)"/\1/')"
    b_node="$(run_in "$CLONE_B" cat .aida-store/.aida/node.toml 2>/dev/null | grep -oE 'node_id *= *"[^"]+"' | head -1 | sed 's/.*"\(.*\)"/\1/')"
    # Both registered in the shared registry/nodes.toml.
    local reg
    reg="$(run_in "$CLONE_A" cat .aida-store/registry/nodes.toml 2>/dev/null || true)"
    CASE_DETAIL="A=$a_node B=$b_node"
    if assert_ne "" "$a_node" "A node id present" \
        && assert_ne "" "$b_node" "B node id present" \
        && assert_ne "$a_node" "$b_node" "node ids distinct" \
        && assert_contains "$reg" "$a_node" "A node in registry" \
        && assert_contains "$reg" "$b_node" "B node in registry"; then
        CASE_OK=1
    else
        CASE_OK=0
    fi
}

# --- MU-103: independent adds yield non-colliding (node-namespaced) ids ---
case_MU-103() {
    EXPECT=pass
    local a_id b_id
    a_id="$(add_spec "$CLONE_A" "mu103 from A" task)"
    b_id="$(add_spec "$CLONE_B" "mu103 from B" task)"
    CASE_DETAIL="A=$a_id B=$b_id"
    if assert_ne "" "$a_id" "A produced a spec id" \
        && assert_ne "" "$b_id" "B produced a spec id" \
        && assert_ne "$a_id" "$b_id" "ids do not collide"; then
        CASE_OK=1
    else
        CASE_OK=0
    fi
}

# --- MU-201: A adds + push; B pull -> B's list shows it ------------------
case_MU-201() {
    EXPECT=pass
    local a_id listing
    a_id="$(add_spec "$CLONE_A" "mu201 shared spec" story)"
    push_from "$CLONE_A"
    pull_into "$CLONE_B"
    listing="$(aida_in "$CLONE_B" list 2>&1 || true)"
    CASE_DETAIL="spec=$a_id visible in B"
    if assert_ne "" "$a_id" "A produced a spec id" \
        && assert_contains "$listing" "$a_id" "B sees A's spec after pull"; then
        CASE_OK=1
    else
        CASE_OK=0
    fi
    # stash for MU-301
    MU201_SPEC="$a_id"
}
MU201_SPEC=""

# --- MU-202: edits to DIFFERENT specs reconcile cleanly -----------------
case_MU-202() {
    EXPECT=pass
    # Create two distinct specs, get both clones in sync first.
    local id_a id_b
    id_a="$(add_spec "$CLONE_A" "mu202 spec A" task)"
    id_b="$(add_spec "$CLONE_B" "mu202 spec B" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    push_from "$CLONE_B"; pull_into "$CLONE_A"
    # A edits its spec; B edits its (different) spec; both push/pull.
    aida_in "$CLONE_A" edit "$id_a" --status in-progress >/dev/null 2>&1 || true
    aida_in "$CLONE_B" edit "$id_b" --status in-progress >/dev/null 2>&1 || true
    push_from "$CLONE_A"
    # B pushes second -> must rebase cleanly (different files, no conflict).
    push_from "$CLONE_B"
    pull_into "$CLONE_A"
    pull_into "$CLONE_B"
    local la lb
    la="$(aida_in "$CLONE_A" list 2>&1 || true)"
    lb="$(aida_in "$CLONE_B" list 2>&1 || true)"
    CASE_DETAIL="both specs present in both clones, no conflict ($id_a,$id_b)"
    if assert_ne "" "$id_a" "A spec id" && assert_ne "" "$id_b" "B spec id" \
        && assert_contains "$la" "$id_a" "A sees own spec" \
        && assert_contains "$la" "$id_b" "A sees B's spec" \
        && assert_contains "$lb" "$id_a" "B sees A's spec" \
        && assert_contains "$lb" "$id_b" "B sees own spec"; then
        CASE_OK=1
    else
        CASE_OK=0
    fi
}

# --- MU-203: NON-MERGEABLE store conflict still surfaces for manual resolve -
# Before STORY-641, same-spec edits surfaced a manual conflict. STORY-641 now
# AUTO-MERGES spec objects + the oplog (see MU-204), so the same-spec status
# case no longer conflicts. MU-203 now pins the COMPLEMENT: a conflict in a
# store file with NO known union rule (here `metadata.yaml`) must STILL fall
# back to the manual-resolution path (non-zero exit / rebase hint), proving the
# auto-merger never force-resolves unknown files. trace:STORY-641
case_MU-203() {
    EXPECT=pass
    # Force a textual conflict on a non-mergeable tracked store file by writing
    # divergent content from each clone directly on the aida-store branch.
    # (We bypass `aida edit` here because edits target spec objects + oplog,
    # which now auto-merge — the point is the *unknown-file* fallback.)
    run_in "$CLONE_A" sh -c 'printf "harness_marker: A-%s\n" "$$" >> .aida-store/metadata.yaml \
        && git -C .aida-store add metadata.yaml \
        && git -C .aida-store commit -q -m "mu203: A edits metadata"' >/dev/null 2>&1 || true
    run_in "$CLONE_B" sh -c 'printf "harness_marker: B-%s\n" "$$" >> .aida-store/metadata.yaml \
        && git -C .aida-store add metadata.yaml \
        && git -C .aida-store commit -q -m "mu203: B edits metadata"' >/dev/null 2>&1 || true
    # A pushes first (wins the push race).
    push_from "$CLONE_A"
    # B pulls -> store-leg rebase hits a conflict on metadata.yaml that the
    # auto-merger refuses (no union rule) -> falls back to manual resolution.
    local pull_out pull_rc
    set +e
    pull_out="$(aida_in "$CLONE_B" pull 2>&1)"
    pull_rc=$?
    set -e
    # The conflict must be SURFACED (non-zero rc and/or a conflict/rebase hint).
    local conflict_marker=""
    if [[ $pull_rc -ne 0 ]]; then conflict_marker="rc=$pull_rc"; fi
    if [[ "$pull_out" == *[Cc]onflict* || "$pull_out" == *rebase* || "$pull_out" == *CONFLICT* || "$pull_out" == *non-mergeable* ]]; then
        conflict_marker="${conflict_marker} text-hint"
    fi
    # Crucially it must NOT claim an auto-merge for the unknown file.
    CASE_DETAIL="non-mergeable conflict surfaced: ${conflict_marker:-none}"
    if assert_ne "" "$conflict_marker" "non-mergeable store conflict surfaces (non-zero exit or hint)" \
        && assert_not_contains "$pull_out" "auto-merged metadata" "unknown file is NOT force-resolved"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="expected manual-fallback; pull rc=$pull_rc out=[${pull_out:0:140}]"
    fi
    # Recover B (abort the rebase, reset onto A's store head) so later cases
    # aren't wedged. A won the push, so adopt A's metadata.
    recover_store_rebase "$CLONE_B"
    push_from "$CLONE_A"
    pull_into "$CLONE_B"
    if run_in "$CLONE_B" git -C .aida-store status 2>/dev/null | grep -q "rebase in progress"; then
        CASE_OK=0
        CASE_DETAIL="conflict surfaced but B's store could not be recovered (still mid-rebase)"
    fi
}

# --- MU-204: concurrent SAME-spec edits AUTO-MERGE on pull (no conflict) --
# STORY-641: a same-spec edit writes BOTH the spec object YAML and the
# append-only oplog. Both are union-mergeable (oplog operations by id + lamport
# reconcile; spec scalars by LWW; spec history/comments by id). B's `aida pull`
# reconciles WITHOUT a manual conflict (rc==0), BOTH clones' edits survive in
# the unioned oplog, and the scalar status is the deterministic LWW winner.
case_MU-204() {
    EXPECT=pass
    # One spec, sync both clones onto it.
    local id
    id="$(add_spec "$CLONE_A" "mu204 auto-merge spec" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # Both edit the SAME spec's status (each appends an oplog SetStatus op),
    # both commit locally. Different targets so the LWW winner is observable.
    aida_in "$CLONE_A" edit "$id" --status in-progress >/dev/null 2>&1 || true
    aida_in "$CLONE_B" edit "$id" --status planned >/dev/null 2>&1 || true
    # A pushes first (wins the push race).
    push_from "$CLONE_A"
    # B pulls -> should AUTO-MERGE the spec YAML + oplog (no manual conflict).
    local pull_out pull_rc
    set +e
    pull_out="$(aida_in "$CLONE_B" pull 2>&1)"
    pull_rc=$?
    set -e
    # Assert: B is NOT left mid-rebase (auto-merge completed it).
    local mid_rebase="no"
    if run_in "$CLONE_B" git -C .aida-store status 2>/dev/null | grep -q "rebase in progress"; then
        mid_rebase="yes"
    fi
    # BOTH clones' edits must survive: the unioned oplog carries A's SetStatus
    # (In Progress) AND B's SetStatus (Planned) — neither is dropped. Count the
    # SetStatus ops for this spec's window: both target-statuses must appear.
    local has_a has_b op_count
    op_count="$(run_in "$CLONE_B" sh -c "grep -c '^- id:' .aida-store/oplog.yaml 2>/dev/null")"
    has_a="$(run_in "$CLONE_B" sh -c "grep -c 'status: In Progress' .aida-store/oplog.yaml 2>/dev/null")"
    has_b="$(run_in "$CLONE_B" sh -c "grep -c 'status: Planned' .aida-store/oplog.yaml 2>/dev/null")"
    # Scalar LWW winner in the merged spec object (B edited later -> Planned).
    local obj_path status_line
    obj_path="$(run_in "$CLONE_B" sh -c "find .aida-store/objects -name '${id}.yaml' 2>/dev/null | head -1")"
    status_line="$(run_in "$CLONE_B" sh -c "grep -E '^status:' \"$obj_path\" 2>/dev/null | head -1")"
    CASE_DETAIL="auto-merged ($id): rc=$pull_rc ops=$op_count A=$has_a B=$has_b status=[${status_line}]"
    if assert_ne "" "$id" "spec id" \
        && assert_eq "0" "$pull_rc" "B pull exits 0 (auto-merged, no manual conflict)" \
        && assert_eq "no" "$mid_rebase" "B's store is NOT left mid-rebase" \
        && assert_ne "0" "$has_a" "A's edit (In Progress) survived the oplog union" \
        && assert_ne "0" "$has_b" "B's edit (Planned) survived the oplog union" \
        && assert_eq "$status_line" "status: Planned" "scalar status is the LWW winner (B, later)" ; then
        CASE_OK=1
    else
        CASE_OK=0
    fi
    # Re-sync so later cases see a clean, converged store in both clones.
    push_from "$CLONE_B"
    pull_into "$CLONE_A"
    pull_into "$CLONE_B"
    # Safety: ensure neither clone is wedged mid-rebase for later cases.
    if run_in "$CLONE_B" git -C .aida-store status 2>/dev/null | grep -q "rebase in progress"; then
        recover_store_rebase "$CLONE_B"
    fi
}

# --- MU-208: concurrent SAME-spec COMMENTS auto-merge on pull -----------
# STORY-645 finishes MU-203 for structurally-mergeable fields: two clones each
# add a DIFFERENT comment to the SAME spec, both push/pull -> merge_spec_three_way
# unions the comments array by id, so B's pull auto-merges (rc==0, no manual
# conflict) and BOTH comments survive in the resulting spec object.
case_MU-208() {
    EXPECT=pass
    local id
    id="$(add_spec "$CLONE_A" "mu208 comment auto-merge spec" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # Each clone adds a DIFFERENT comment to the SAME spec (each rewrites the
    # spec object YAML's comments: array), both commit locally.
    local marker_a="MU208-COMMENT-FROM-A" marker_b="MU208-COMMENT-FROM-B"
    aida_in "$CLONE_A" comment add "$id" "$marker_a" >/dev/null 2>&1 || true
    aida_in "$CLONE_B" comment add "$id" "$marker_b" >/dev/null 2>&1 || true
    # A pushes first (wins the race); B pulls -> should AUTO-MERGE (comment union).
    push_from "$CLONE_A"
    local pull_out pull_rc
    set +e
    pull_out="$(aida_in "$CLONE_B" pull 2>&1)"
    pull_rc=$?
    set -e
    # B must NOT be left mid-rebase.
    local mid_rebase="no"
    if run_in "$CLONE_B" git -C .aida-store status 2>/dev/null | grep -q "rebase in progress"; then
        mid_rebase="yes"
    fi
    # BOTH comments must be present in the merged spec object.
    local obj_path has_a has_b
    obj_path="$(run_in "$CLONE_B" sh -c "find .aida-store/objects -name '${id}.yaml' 2>/dev/null | head -1")"
    has_a="$(run_in "$CLONE_B" sh -c "grep -c '$marker_a' \"$obj_path\" 2>/dev/null")"
    has_b="$(run_in "$CLONE_B" sh -c "grep -c '$marker_b' \"$obj_path\" 2>/dev/null")"
    CASE_DETAIL="comments auto-merged ($id): rc=$pull_rc A=$has_a B=$has_b"
    if assert_ne "" "$id" "spec id" \
        && assert_eq "0" "$pull_rc" "B pull exits 0 (auto-merged, no manual conflict)" \
        && assert_eq "no" "$mid_rebase" "B's store is NOT left mid-rebase" \
        && assert_ne "0" "$has_a" "A's comment survived the comment union" \
        && assert_ne "0" "$has_b" "B's comment survived the comment union" ; then
        CASE_OK=1
    else
        CASE_OK=0
    fi
    # Re-sync so later cases see a clean, converged store in both clones.
    push_from "$CLONE_B"
    pull_into "$CLONE_A"
    pull_into "$CLONE_B"
    if run_in "$CLONE_B" git -C .aida-store status 2>/dev/null | grep -q "rebase in progress"; then
        recover_store_rebase "$CLONE_B"
    fi
}

# --- MU-301: B's cache reflects the new spec after MU-201's pull ---------
case_MU-301() {
    EXPECT=pass
    # Ensure MU-201 ran (the shared spec exists); if not, make one.
    if [[ -z "$MU201_SPEC" ]]; then
        MU201_SPEC="$(add_spec "$CLONE_A" "mu301 shared spec" story)"
        push_from "$CLONE_A"; pull_into "$CLONE_B"
    fi
    # After a pull that advanced the store HEAD, the cache must rebuild so the
    # cache-backed `aida list` includes the spec. cache status should report
    # in-sync (cache HEAD == store HEAD) after the read rebuilt it.
    local listing cache_status
    listing="$(aida_in "$CLONE_B" list 2>&1 || true)"
    cache_status="$(aida_in "$CLONE_B" cache status 2>&1 || true)"
    CASE_DETAIL="cache reflects $MU201_SPEC"
    if assert_contains "$listing" "$MU201_SPEC" "cache-backed list shows pulled spec"; then
        CASE_OK=1
    else
        CASE_OK=0
    fi
}

# --- MU-601: concurrent SAME-CLONE cache contention (BUG-636 / BUG-627) ----
# The harness's original blind spot: it covered cross-clone sync (MU-2xx) and a
# single-clone cache rebuild after a pull (MU-301), but NEVER two agents hitting
# ONE shared .aida/cache.db at the same time. A real session hit exactly that --
# two concurrent `aida` invocations in the same clone racing reads + writes on
# the one cache db. This case reproduces that contention and pins three
# guarantees the cache-concurrency work (the SQLite busy-lock + retry, BUG-636's
# incremental update, BUG-627's schema self-heal) must hold:
#   1. no HARD `SQLITE_BUSY` / "database is locked" error -- the lock + retry must
#      serialize concurrent writers/readers, not crash one of them;
#   2. no "no such column" / schema-drift hard error -- BUG-627's open-time column
#      verification self-heals a drifted cache instead of erroring;
#   3. the cache stays consistent / self-heals -- after the storm, `aida cache
#      status` reports FRESH and a cache-backed `aida list` succeeds and shows
#      the writes.
#
# Mechanism: TWO concurrent worker processes each loop N times in CLONE A doing
# add (write) + list/search/cache-status (reads), all against the SAME clone's
# single .aida/cache.db. Both workers append their combined stdout+stderr to a
# per-worker log; after they join, we scan the merged logs for the forbidden
# hard-error substrings and assert the post-storm cache is FRESH + queryable. We
# do NOT run a real drain or anything that parks specs -- just concurrent
# reads+writes.
#
# Two same-build invocations suffice to exercise the lock; a different-SHA pair
# (the ideal: one agent mid-upgrade) is a strict superset and noted as a followup
# (would also exercise BUG-627's schema self-heal under contention, but needs a
# second binary the harness doesn't build). trace:TASK-938
case_MU-601() {
    EXPECT=pass
    local iters="${MU601_ITERS:-12}"
    local log_dir="$WORKDIR/mu601-logs"
    mkdir -p "$log_dir"
    rm -f "$log_dir"/worker-*.log 2>/dev/null || true

    # One concurrent worker: loop `iters` times, each iteration writing a spec
    # then doing a spread of cache-backed reads, all in CLONE A against the one
    # shared cache db. add lands Approved via the inline advisor role (TASK-647)
    # so a non-TTY add is not downgraded to Draft. Combined stdout+stderr is
    # captured so the post-join scan sees any hard error text.
    mu601_worker() {
        local tag="$1" n="$2" log="$3"
        local i
        for (( i = 0; i < n; i++ )); do
            ( cd "$CLONE_A" && HOME="$WORKDIR/home" AIDA_SESSION_ROLE=advisor \
                "$AIDA_BIN" add --title "mu601-$tag-$i" --type task --status approved ) >>"$log" 2>&1 || true
            ( cd "$CLONE_A" && HOME="$WORKDIR/home" "$AIDA_BIN" list ) >>"$log" 2>&1 || true
            ( cd "$CLONE_A" && HOME="$WORKDIR/home" "$AIDA_BIN" search "mu601" ) >>"$log" 2>&1 || true
            ( cd "$CLONE_A" && HOME="$WORKDIR/home" "$AIDA_BIN" cache status ) >>"$log" 2>&1 || true
        done
    }

    # Fire two workers concurrently against the SAME clone's cache db.
    local log_a="$log_dir/worker-a.log" log_b="$log_dir/worker-b.log"
    mu601_worker A "$iters" "$log_a" &
    local pid_a=$!
    mu601_worker B "$iters" "$log_b" &
    local pid_b=$!
    # Join both; never let a worker non-zero abort the harness (set -e).
    wait "$pid_a" 2>/dev/null || true
    wait "$pid_b" 2>/dev/null || true

    # Merge the two worker logs and scan for the forbidden HARD errors.
    local merged
    merged="$(cat "$log_a" "$log_b" 2>/dev/null || true)"

    # 1+2: no hard SQLITE_BUSY / lock error, no schema-drift "no such column".
    local busy_hit=0 schema_hit=0
    if [[ "$merged" == *"database is locked"* || "$merged" == *"SQLITE_BUSY"* || "$merged" == *"database table is locked"* ]]; then
        busy_hit=1
    fi
    if [[ "$merged" == *"no such column"* || "$merged" == *"no such table"* ]]; then
        schema_hit=1
    fi

    # 3: after the storm the cache must be FRESH and a read must succeed + show
    # the concurrent writes. `aida cache status` prints an up-to-date / in-sync
    # marker when cache HEAD == store HEAD; accept the common phrasings.
    local post_status post_list
    post_status="$(aida_in "$CLONE_A" cache status 2>&1 || true)"
    post_list="$(aida_in "$CLONE_A" list 2>&1 || true)"
    local cache_fresh=0
    if [[ "$post_status" == *"up to date"* || "$post_status" == *"up-to-date"* \
        || "$post_status" == *"in sync"* || "$post_status" == *"in-sync"* \
        || "$post_status" == *FRESH* || "$post_status" == *fresh* ]]; then
        cache_fresh=1
    fi
    # At least one of each worker's writes must be queryable post-storm.
    local list_has_writes=0
    if [[ "$post_list" == *"mu601-A-"* && "$post_list" == *"mu601-B-"* ]]; then
        list_has_writes=1
    fi

    CASE_DETAIL="iters=$iters busy=$busy_hit schema=$schema_hit fresh=$cache_fresh writes=$list_has_writes"
    if assert_eq "$busy_hit" "0" "no SQLITE_BUSY/database-is-locked hard error under same-clone contention" \
        && assert_eq "$schema_hit" "0" "no schema-drift (no such column/table) hard error (BUG-627 self-heal)" \
        && assert_eq "$cache_fresh" "1" "cache self-heals: cache status FRESH after the storm" \
        && assert_eq "$list_has_writes" "1" "cache-backed list shows both concurrent workers' writes"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="contention left a hard error or stale cache: $CASE_DETAIL; status=[${post_status:0:160}]"
    fi
}

# --- MU-401: same OS user, two clones -> shared queue -------------------
case_MU-401() {
    EXPECT=pass
    # Same user (no AIDA_USER -> both resolve current_user_id() identically).
    local id qlist
    id="$(add_spec "$CLONE_A" "mu401 queued spec" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # A queues it (advisor role lets non-TTY queue add through).
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" AIDA_SESSION_ROLE=advisor "$AIDA_BIN" queue add "$id" >/dev/null 2>&1 ) || true
    push_from "$CLONE_A"
    pull_into "$CLONE_B"
    qlist="$(aida_in "$CLONE_B" queue list 2>&1 || true)"
    CASE_DETAIL="B's queue shows A's add ($id), shared by OS user"
    if assert_ne "" "$id" "spec id" \
        && assert_contains "$qlist" "$id" "B's queue list shows the shared item"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="B's queue did not show $id; out=[${qlist:0:120}]"
    fi
}

# --- MU-402: different AIDA_USER -> separate queues ----------------------
case_MU-402() {
    EXPECT=pass
    local id qlist_bob
    id="$(add_spec "$CLONE_A" "mu402 alice spec" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # alice queues in A.
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" AIDA_USER=alice AIDA_SESSION_ROLE=advisor "$AIDA_BIN" queue add "$id" >/dev/null 2>&1 ) || true
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # bob lists in B -> must NOT see alice's item.
    qlist_bob="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER=bob "$AIDA_BIN" queue list 2>&1 || true )"
    CASE_DETAIL="bob does not see alice's queued $id"
    if assert_ne "" "$id" "spec id" \
        && assert_not_contains "$qlist_bob" "$id" "bob's queue excludes alice's item"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="bob saw alice's item $id; out=[${qlist_bob:0:120}]"
    fi
}

# --- MU-504: two clones lease the SAME spec -> REFUSED (STORY-637) --------
case_MU-504() {
    # STORY-637 closed this gap: a shared lease registry on the aida-store
    # branch (coordination/leases/<scope>.toml) makes B's cross-clone
    # `session start --owns <same spec>` refuse a lease A already holds.
    EXPECT=pass
    # A creates a spec + leases it; B attempts to lease the same spec.
    local id
    id="$(add_spec "$CLONE_A" "mu504 contended lease" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # A takes a lease (worktree directed into the workdir to stay contained;
    # no --launch flag means Claude is NOT spawned).
    aida_in "$CLONE_A" session start --owns "$id" --path "$WORKDIR/wtA-$id" >/dev/null 2>&1 || true
    # Structural fact (the root cause): A's lease file is NOT visible in B.
    local a_leases b_leases_before
    a_leases="$(run_in "$CLONE_A" ls .aida/sessions/ 2>/dev/null | grep -c '\.toml$' || true)"
    b_leases_before="$(run_in "$CLONE_B" ls .aida/sessions/ 2>/dev/null | grep -c '\.toml$' || true)"
    # B attempts the same lease. DESIRED: refused (non-zero / "already owned").
    local b_out b_rc
    set +e
    b_out="$(aida_in "$CLONE_B" session start --owns "$id" --path "$WORKDIR/wtB-$id" 2>&1)"
    b_rc=$?
    set -e
    # Desired pass condition: B is REFUSED (cross-clone lease coordination).
    local desired_refused=0
    if [[ $b_rc -ne 0 || "$b_out" == *"already owned"* ]]; then desired_refused=1; fi
    CASE_DETAIL="A leases=$a_leases, B saw=$b_leases_before before; B refused=$desired_refused (desired=1)"
    if assert_eq "$desired_refused" "1" "B refuses a cross-clone lease on the same spec"; then
        CASE_OK=1     # gap closed
    else
        CASE_OK=0     # still a gap (expected)
    fi
    # Clean up any worktrees/leases we created so later cases aren't disturbed.
    aida_in "$CLONE_A" session end "$id" >/dev/null 2>&1 || true
    aida_in "$CLONE_B" session end "$id" >/dev/null 2>&1 || true
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" git worktree prune >/dev/null 2>&1 ) || true
    ( cd "$CLONE_B" && HOME="$WORKDIR/home" git worktree prune >/dev/null 2>&1 ) || true
    rm -rf "$WORKDIR/wtA-$id" "$WORKDIR/wtB-$id" 2>/dev/null || true
}

# --- write a shared process-lock claim (coordination/<kind>.lock.toml) on the
# store as if clone A holds it, and push it so clone B sees it on pull. Pid is
# this script's own ($$, guaranteed alive); clone_path is A's canonicalized
# project root (so it's FOREIGN to B); host is this host (so B's same-host pid
# probe applies). Used by MU-505 (drain) and MU-506 (solo). trace:STORY-638
write_shared_lock_claim() {
    # $1 = kind file stem (drain.lock.toml | solo.lock.toml)
    # $2 = scope label  (drain | solo loop)
    # $3 = command
    local kind="$1" scope="$2" command="$3"
    local store="$CLONE_A/.aida-store"
    local now canon
    now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    canon="$( cd "$CLONE_A" && pwd -P )"
    mkdir -p "$store/coordination"
    cat > "$store/coordination/$kind" <<EOF
scope = "$scope"
node_id = "1"
clone_path = "$canon"
host = "$(hostname)"
pid = $$
agent = "$command"
started_at = "$now"
heartbeat_at = "$now"
ttl_secs = 1800
process_backed = true
review_verb = false
EOF
    ( cd "$store" && HOME="$WORKDIR/home" git add "coordination/$kind" >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git commit -q -m "test: harness $scope claim" >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git push -q origin aida-store >/dev/null 2>&1 ) || true
}

# --- remove a shared process-lock claim from the store (cleanup). -----------
remove_shared_lock_claim() {
    local kind="$1"
    local store="$CLONE_A/.aida-store"
    rm -f "$store/coordination/$kind" 2>/dev/null || true
    ( cd "$store" && HOME="$WORKDIR/home" git add -A "coordination/$kind" >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git commit -q -m "test: harness remove claim" >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git push -q origin aida-store >/dev/null 2>&1 ) || true
}

# --- MU-505: A holds the shared DRAIN claim -> B's drain is REFUSED (STORY-638)
case_MU-505() {
    # STORY-638 closed this gap: the drain lock is promoted to a shared claim on
    # the aida-store branch (coordination/drain.lock.toml). With clone A holding
    # a LIVE drain claim, clone B's drain entry (queue work --auto-complete)
    # consults the shared claim BEFORE the local lock and must REFUSE.
    EXPECT=pass
    # Test LOCK MECHANICS structurally: simulate A's live drain by writing the
    # shared claim with this script's own (alive) pid; do NOT run a real drain.
    write_shared_lock_claim "drain.lock.toml" "drain" "burndown run (harness sim)"
    # B attempts a drain on an EMPTY queue. With a live cross-clone drain claim
    # present, the entry point must refuse BEFORE doing any work. Run as advisor:
    # STORY-647's drain-start RBAC gate (advisor-only by default) would otherwise
    # refuse first; this case tests the LOCK mechanics, which only a legitimate
    # drain-starter (advisor) reaches. trace:STORY-647
    local out rc
    set +e
    out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_SESSION_ROLE=advisor "$AIDA_BIN" queue work --auto-complete 2>&1 )"
    rc=$?
    set -e
    # Refusal = mentions a drain running in another clone / a holder / non-zero.
    local refused=0
    if [[ "$out" == *drain* && ( "$out" == *"another clone"* || "$out" == *"already running"* || "$out" == *holder* || "$out" == *pid* ) ]]; then
        refused=1
    fi
    CASE_DETAIL="B's cross-clone drain refused=$refused (rc=$rc)"
    if assert_eq "$refused" "1" "B refuses a cross-clone drain while A holds the shared claim"; then
        CASE_OK=1     # gap closed
    else
        CASE_OK=0
        CASE_DETAIL="cross-clone drain not refused; rc=$rc out=[${out:0:200}]"
    fi
    remove_shared_lock_claim "drain.lock.toml"
}

# --- MU-506: A holds the shared SOLO claim -> B's solo run is REFUSED (STORY-638)
case_MU-506() {
    # Same class as MU-505 for the solo loop: coordination/solo.lock.toml on the
    # store. With clone A holding a LIVE solo claim, clone B's `aida solo run`
    # consults the shared claim before the local lock and must REFUSE.
    EXPECT=pass
    write_shared_lock_claim "solo.lock.toml" "solo loop" "solo run (harness sim)"
    # B attempts `aida solo run`. A live cross-clone solo claim must refuse it
    # before the loop starts. Use a short interval so a (wrongly) non-refused run
    # can't hang the harness; the refusal happens at lock-acquire, before any
    # cycle. Run with a timeout as a belt-and-braces guard.
    local out rc
    set +e
    out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" timeout 30 "$AIDA_BIN" solo run --interval 1 2>&1 )"
    rc=$?
    set -e
    local refused=0
    if [[ "$out" == *"solo loop"* && ( "$out" == *"another clone"* || "$out" == *"already running"* || "$out" == *holder* || "$out" == *pid* ) ]]; then
        refused=1
    fi
    CASE_DETAIL="B's cross-clone solo run refused=$refused (rc=$rc)"
    if assert_eq "$refused" "1" "B refuses a cross-clone solo run while A holds the shared claim"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="cross-clone solo not refused; rc=$rc out=[${out:0:200}]"
    fi
    remove_shared_lock_claim "solo.lock.toml"
    # Make sure B didn't leave a solo flag/loop running.
    ( cd "$CLONE_B" && HOME="$WORKDIR/home" "$AIDA_BIN" solo stop >/dev/null 2>&1 ) || true
}

# --- MU-507: within ONE clone, a second drain is refused (lock works) ----
case_MU-507() {
    EXPECT=pass
    # Test the lock MECHANICS intra-clone without a real drain: write a live
    # drain.lock (our own pid) in clone A, then trigger a drain ENTRY that
    # acquires the lock. queue work --auto-complete on an EMPTY queue takes the
    # lock first; with a live holder present it must REFUSE before doing work.
    local lock_a="$CLONE_A/.aida/drain.lock" now
    now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    mkdir -p "$CLONE_A/.aida"
    printf '{"pid":%d,"started_at_utc":"%s","command":"burndown run (harness sim)","host":"%s"}\n' \
        "$$" "$now" "$(hostname)" > "$lock_a"
    # Second drain attempt in the SAME clone -> must refuse (holder pid alive).
    # Run as advisor: STORY-647's drain-start RBAC gate (advisor-only by default)
    # would otherwise refuse first; this case tests the LOCK mechanics, reached
    # only by a legitimate drain-starter (advisor). trace:STORY-647
    local out rc
    set +e
    out="$( cd "$CLONE_A" && HOME="$WORKDIR/home" AIDA_SESSION_ROLE=advisor AIDA_DRAIN_LOCK_STALE_SECS=3600 "$AIDA_BIN" queue work --auto-complete 2>&1 )"
    rc=$?
    set -e
    rm -f "$lock_a" 2>/dev/null || true
    # Refusal = mentions another drain holds the lock, or non-zero exit citing
    # the holder. (An empty-queue no-op that did NOT mention the lock means the
    # entry point didn't check it -- treat as not-refused.)
    local refused=0
    if [[ "$out" == *drain* && ( "$out" == *lock* || "$out" == *holder* || "$out" == *pid* || "$out" == *running* || "$out" == *"already"* ) ]]; then
        refused=1
    fi
    CASE_DETAIL="second intra-clone drain refused=$refused (rc=$rc)"
    if assert_eq "$refused" "1" "intra-clone drain lock refuses a second drain"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="lock not surfaced by drain entry; rc=$rc out=[${out:0:160}]"
    fi
}

# =========================================================================
# Multi-host cases (STORY-642 / EPIC-47, Phase 2).
#
# Same-host coordination uses pid liveness; CROSS-host can only use the
# TTL/heartbeat backstop (a remote pid is meaningless on this machine).
# `decide_claim` already gates the pid fast path on `host == our_host`, so a
# foreign-host claim is NEVER pid-reclaimed — only TTL-reclaimed. We prove that
# end-to-end by simulating two distinct hosts on this one machine via the
# `AIDA_HOST_OVERRIDE` test hook (read by coordination::hostname()).
#
# Mechanism: clone A takes a REAL cross-clone lease (`session start --owns`)
# under AIDA_HOST_OVERRIDE=hostX, which writes a correctly-named claim file on
# the store (coordination/leases/<sanitized-scope>.toml). We then drive clone B's
# REAL acquire path under AIDA_HOST_OVERRIDE=hostY and observe the decision.
# For the STALE case we rewrite A's pushed claim with an aged heartbeat + tiny
# TTL (testing the real decision against controlled inputs). No real drain runs.
# trace:STORY-642
# =========================================================================

# Locate clone A's pushed lease claim file for a scope (the FNV-hashed name is
# computed by the binary; we glob it back rather than recompute the hash). Echoes
# the absolute path, or empty if none. $1 = store dir, $2 = scope stem (lowercased).
find_lease_claim_file() {
    local store="$1" stem="$2"
    local f
    for f in "$store"/coordination/leases/"${stem}"-*.toml; do
        [[ -e "$f" ]] && { echo "$f"; return 0; }
    done
    echo ""
}

# Rewrite the heartbeat_at + ttl_secs of an existing claim file in place so the
# real decide_claim treats it as TTL-stale. $1 = file, $2 = heartbeat (RFC3339),
# $3 = ttl_secs.
age_claim_file() {
    local file="$1" hb="$2" ttl="$3"
    [[ -e "$file" ]] || return 1
    # Replace the two fields; leave host/pid/clone_path/process_backed intact.
    HOME="$WORKDIR/home" sed -i \
        -e "s/^heartbeat_at = .*/heartbeat_at = \"$hb\"/" \
        -e "s/^started_at = .*/started_at = \"$hb\"/" \
        -e "s/^ttl_secs = .*/ttl_secs = $ttl/" \
        "$file"
}

# Commit + push the store worktree of clone A (so clone B sees the change).
push_store_A() {
    local store="$CLONE_A/.aida-store"
    ( cd "$store" && HOME="$WORKDIR/home" git add -A coordination >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git commit -q -m "test: harness multi-host claim" >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git push -q origin aida-store >/dev/null 2>&1 ) || true
}

# Clean up any cross-clone lease claim + worktrees a multi-host case created.
cleanup_multihost_lease() {
    local id="$1"
    aida_in "$CLONE_A" session end "$id" >/dev/null 2>&1 || true
    aida_in "$CLONE_B" session end "$id" >/dev/null 2>&1 || true
    # Belt-and-braces: delete any lingering claim file + push.
    local store="$CLONE_A/.aida-store" stem
    stem="$(echo "$id" | tr '[:upper:]' '[:lower:]')"
    rm -f "$store"/coordination/leases/"${stem}"-*.toml 2>/dev/null || true
    push_store_A
    pull_into "$CLONE_B"
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" git worktree prune >/dev/null 2>&1 ) || true
    ( cd "$CLONE_B" && HOME="$WORKDIR/home" git worktree prune >/dev/null 2>&1 ) || true
    rm -rf "$WORKDIR/wtA-$id" "$WORKDIR/wtB-$id" 2>/dev/null || true
}

# --- MU-511: foreign-HOST LIVE claim honored -> B refused (cross-host TTL) -
case_MU-511() {
    EXPECT=pass
    # A takes a REAL lease as host "hostX" with a FRESH heartbeat. B (host
    # "hostY") attempts the same scope: cross-host, so no pid fast path; the
    # heartbeat is well within TTL, so B must REFUSE.
    local id
    id="$(add_spec "$CLONE_A" "mu511 cross-host live lease" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" AIDA_HOST_OVERRIDE=hostX \
        "$AIDA_BIN" session start --owns "$id" --path "$WORKDIR/wtA-$id" >/dev/null 2>&1 ) || true
    push_store_A
    pull_into "$CLONE_B"
    # Confirm the claim landed with the overridden foreign host.
    local store="$CLONE_B/.aida-store" stem claim_file claim_host
    stem="$(echo "$id" | tr '[:upper:]' '[:lower:]')"
    claim_file="$(find_lease_claim_file "$store" "$stem")"
    claim_host="$(grep -E '^host = ' "$claim_file" 2>/dev/null | sed 's/.*"\(.*\)".*/\1/' || true)"
    # B attempts the lease as a DIFFERENT host.
    local b_out b_rc
    set +e
    b_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_HOST_OVERRIDE=hostY \
        "$AIDA_BIN" session start --owns "$id" --path "$WORKDIR/wtB-$id" 2>&1 )"
    b_rc=$?
    set -e
    local refused=0
    if [[ $b_rc -ne 0 || "$b_out" == *"already leased"* || "$b_out" == *"already owned"* ]]; then refused=1; fi
    CASE_DETAIL="A=hostX live, claim host=$claim_host; B=hostY refused=$refused (desired=1)"
    if assert_eq "$claim_host" "hostX" "A's claim recorded the overridden host" \
        && assert_eq "$refused" "1" "B refuses a LIVE foreign-host lease (cross-host TTL not expired)"; then
        CASE_OK=1
    else
        CASE_OK=0
        [[ "$refused" != "1" ]] && CASE_DETAIL="B not refused; rc=$b_rc out=[${b_out:0:160}]"
    fi
    cleanup_multihost_lease "$id"
}

# --- MU-512: foreign-HOST STALE claim reclaimable -> B acquires ----------
case_MU-512() {
    EXPECT=pass
    # A takes a REAL lease as host "hostX", then we age its heartbeat past a tiny
    # TTL and re-push. B (host "hostY") attempts the same scope: cross-host, TTL
    # expired -> B must RECLAIM (acquire succeeds).
    local id
    id="$(add_spec "$CLONE_A" "mu512 cross-host stale lease" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    ( cd "$CLONE_A" && HOME="$WORKDIR/home" AIDA_HOST_OVERRIDE=hostX \
        "$AIDA_BIN" session start --owns "$id" --path "$WORKDIR/wtA-$id" >/dev/null 2>&1 ) || true
    # Age A's claim (foreign host, heartbeat 2h ago, ttl 60s -> well past TTL).
    local store_a="$CLONE_A/.aida-store" stem claim_file old_hb
    stem="$(echo "$id" | tr '[:upper:]' '[:lower:]')"
    claim_file="$(find_lease_claim_file "$store_a" "$stem")"
    old_hb="$(date -u -d '2 hours ago' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v-2H +%Y-%m-%dT%H:%M:%SZ 2>/dev/null)"
    age_claim_file "$claim_file" "$old_hb" 60
    push_store_A
    pull_into "$CLONE_B"
    # A's `session start` bumped the spec to InProgress; a stale/crashed holder
    # leaves it there. That's the orthogonal BUG-379 preflight gate, not the
    # coordination decision under test — reset the spec to Approved (the
    # documented recovery for a known-stale holder) so B's `session start`
    # exercises the cross-host TTL RECLAIM cleanly. trace:STORY-642
    ( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_SESSION_ROLE=advisor \
        "$AIDA_BIN" edit "$id" --status approved >/dev/null 2>&1 ) || true
    push_from "$CLONE_B"; pull_into "$CLONE_B"
    # B attempts the lease as a DIFFERENT host -> should reclaim (acquire).
    local b_out b_rc
    set +e
    b_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_HOST_OVERRIDE=hostY \
        "$AIDA_BIN" session start --owns "$id" --path "$WORKDIR/wtB-$id" 2>&1 )"
    b_rc=$?
    set -e
    # Acquire success = exit 0 AND not refused. A "reclaiming a stale" note is a
    # strong positive signal but not required.
    local acquired=0
    if [[ $b_rc -eq 0 && "$b_out" != *"already leased"* && "$b_out" != *"already owned"* ]]; then acquired=1; fi
    local saw_reclaim=0
    [[ "$b_out" == *"reclaiming a stale"* ]] && saw_reclaim=1
    CASE_DETAIL="A=hostX stale (ttl 60s, hb 2h old); B=hostY acquired=$acquired reclaim-note=$saw_reclaim"
    if assert_eq "$acquired" "1" "B reclaims a STALE foreign-host lease (cross-host TTL backstop)"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="B did not acquire stale foreign-host lease; rc=$b_rc out=[${b_out:0:160}]"
    fi
    cleanup_multihost_lease "$id"
}

# --- MU-513: same-HOST dead-pid claim reclaims immediately (fast path) ----
case_MU-513() {
    EXPECT=pass
    # The same-host fast path must STILL work: a PROCESS-BACKED same-host claim
    # whose pid is dead is reclaimable WITHOUT waiting for TTL. We exercise this
    # via decide_claim's governance of the shared DRAIN lock (process-backed):
    # write a same-host drain claim with a guaranteed-DEAD pid + a FRESH
    # heartbeat (so TTL is NOT the trigger), then B's drain entry must reclaim
    # and proceed (NOT refuse) -- proving pid liveness reclaimed it immediately.
    # A guaranteed-dead pid: spawn `true` and reap it; its pid is now free.
    local dead_pid
    ( sleep 0.01 ) & dead_pid=$!
    wait "$dead_pid" 2>/dev/null || true
    local store="$CLONE_A/.aida-store" now canon
    now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"   # FRESH heartbeat -> TTL is NOT stale
    canon="$( cd "$CLONE_A" && pwd -P )"
    mkdir -p "$store/coordination"
    cat > "$store/coordination/drain.lock.toml" <<EOF
scope = "drain"
node_id = "1"
clone_path = "$canon"
host = "$(hostname)"
pid = $dead_pid
agent = "burndown run (harness sim, dead pid)"
started_at = "$now"
heartbeat_at = "$now"
ttl_secs = 1800
process_backed = true
review_verb = false
EOF
    ( cd "$store" && HOME="$WORKDIR/home" git add coordination/drain.lock.toml >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git commit -q -m "test: harness same-host dead-pid drain claim" >/dev/null 2>&1 \
        && HOME="$WORKDIR/home" git push -q origin aida-store >/dev/null 2>&1 ) || true
    # B attempts a drain on an EMPTY queue. The shared claim is same-host (this
    # host) with a dead pid + fresh heartbeat: the fast path must reclaim it and
    # the drain must NOT refuse on the cross-clone claim.
    local out rc
    set +e
    out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_DRAIN_LOCK_STALE_SECS=3600 \
        "$AIDA_BIN" queue work --auto-complete 2>&1 )"
    rc=$?
    set -e
    # Refused-on-cross-clone-claim = the dead-pid fast path FAILED to reclaim.
    local refused_on_claim=0
    if [[ "$out" == *drain* && ( "$out" == *"another clone"* || "$out" == *"already running"* ) ]]; then
        refused_on_claim=1
    fi
    CASE_DETAIL="same-host dead pid=$dead_pid, fresh hb; B reclaimed (not refused)=$((1-refused_on_claim))"
    if assert_eq "$refused_on_claim" "0" "same-host dead-pid claim is reclaimed immediately (not refused, no TTL wait)"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="same-host dead-pid NOT reclaimed (refused on stale claim); rc=$rc out=[${out:0:160}]"
    fi
    remove_shared_lock_claim "drain.lock.toml"
}

# --- MU-502: auto mailbox sync -- A sends, A push, B pull, B sees it --------
# STORY-643: cross-user mailbox visibility now flows automatically on the normal
# sync legs (no manual `aida mailbox sync`). A `aida mailbox send`s a message to
# an identity B will read, A `aida push` (the push store leg PUBLISHES the local
# mailbox into the canonical store + folds it into the store commit), B `aida
# pull` (the rebase brings the canonical message down), then B `aida mailbox
# inbox <identity>` (the read path merges canonical+local) SHOWS it. trace:STORY-643
case_MU-502() {
    EXPECT=pass
    # Unique recipient id + body marker so we don't collide with other cases.
    local recipient="mu502-teammate"
    local marker="mu502-auto-sync-hello"
    # A sends to an identity B will read (explicit --from so it is not A's own
    # identity, and so the sender-exclusion never hides it from the recipient).
    aida_in "$CLONE_A" mailbox send --to "$recipient" --from clonea "$marker" >/dev/null 2>&1 || true
    # A publishes via the normal push (no manual `aida mailbox sync`).
    push_from "$CLONE_A"
    # B receives via the normal pull (brings the canonical message down).
    pull_into "$CLONE_B"
    # B reads the recipient's inbox; the canonical message must surface.
    local inbox
    inbox="$(aida_in "$CLONE_B" mailbox inbox "$recipient" 2>&1 || true)"
    CASE_DETAIL="B sees A's message in $recipient's inbox after push/pull (no manual digest)"
    if assert_contains "$inbox" "$marker" "B sees A's auto-synced message"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="auto mailbox sync did not surface the message; inbox=[${inbox:0:200}]"
    fi
}

# --- MU-541: assignment notification -- A assigns to B identity, B sees it ----
# STORY-644: `aida assign <spec> --to <user>` ALSO sends a mailbox notice
# addressed to <user> ("You were assigned <SPEC>: <title>"). Composes with the
# STORY-643 auto mailbox sync: A assigns -> A push (publishes the local mailbox
# into the canonical store) -> B pull (brings the canonical message down) -> B
# `aida mailbox inbox <user>` SHOWS the assignment notice. trace:STORY-644
case_MU-541() {
    EXPECT=pass
    # The assignee is an identity B will read; it differs from A's OS-user
    # identity so the self-skip never fires and the notice is sent.
    local recipient="mu541-teammate"
    local id
    id="$(add_spec "$CLONE_A" "mu541 assignment notice spec" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # A assigns the spec to the teammate identity (sends the mailbox notice).
    aida_in "$CLONE_A" assign "$id" --to "$recipient" >/dev/null 2>&1 || true
    # A publishes via the normal push (no manual `aida mailbox sync`).
    push_from "$CLONE_A"
    # B receives via the normal pull.
    pull_into "$CLONE_B"
    # B reads the teammate's inbox; the assignment notice must surface.
    local inbox
    inbox="$(aida_in "$CLONE_B" mailbox inbox "$recipient" 2>&1 || true)"
    CASE_DETAIL="B sees the assignment notice for $id in $recipient's inbox"
    if assert_ne "" "$id" "spec id" \
        && assert_contains "$inbox" "assigned" "inbox carries the assignment notice" \
        && assert_contains "$inbox" "$id" "notice names the assigned spec"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="assignment notice did not surface; inbox=[${inbox:0:200}]"
    fi
}

# --- MU-521: `aida team` in clone B lists BOTH clones' nodes (STORY-640) ----
case_MU-521() {
    # STORY-640: `aida team` is the team roster — it reads registry/nodes.toml on
    # the shared aida-store branch and lists every registered node/clone. After
    # setup both A and B are registered (distinct node ids), so B's `aida team`
    # must surface BOTH node ids. Also assert the --json shape parses + carries
    # both rows. trace:STORY-640
    EXPECT=pass
    local a_node b_node
    a_node="$(run_in "$CLONE_A" cat .aida-store/.aida/node.toml 2>/dev/null | grep -oE 'node_id *= *"[^"]+"' | head -1 | sed 's/.*"\(.*\)"/\1/')"
    b_node="$(run_in "$CLONE_B" cat .aida-store/.aida/node.toml 2>/dev/null | grep -oE 'node_id *= *"[^"]+"' | head -1 | sed 's/.*"\(.*\)"/\1/')"
    local out json_out
    out="$(aida_in "$CLONE_B" team 2>&1 || true)"
    json_out="$(aida_in "$CLONE_B" team --json 2>&1 || true)"
    # JSON must carry both node ids as distinct rows.
    local json_has_both=0
    if [[ "$json_out" == *"\"node_id\""* && "$json_out" == *"\"$a_node\""* && "$json_out" == *"\"$b_node\""* ]]; then
        json_has_both=1
    fi
    CASE_DETAIL="B team lists A=$a_node B=$b_node; json_both=$json_has_both"
    if assert_contains "$out" "$a_node" "team roster lists A's node" \
        && assert_contains "$out" "$b_node" "team roster lists B's node" \
        && assert_eq "$json_has_both" "1" "team --json carries both rows"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="team roster missing a node; out=[${out:0:200}] json=[${json_out:0:200}]"
    fi
}

# --- MU-551: per-user team roles gate advisor-only ops (STORY-646) ----------
case_MU-551() {
    # STORY-646 (team RBAC slice 1): a durable per-user role in the shared
    # roster (registry/team.toml) is the user's EFFECTIVE role even with no
    # AIDA_SESSION_ROLE set — so the advisor guardrail survives a forgotten env
    # var, and a non-advisor rostered user is refused an advisor-only op.
    #
    # Verified gated verb (via `aida edit --help`): `aida edit <spec> --status
    # approved` — promoting Draft → Approved needs advisor authority (TASK-647).
    #
    # Two distinct user ids (set via AIDA_USER so current_user_id is fixed):
    #   - mu551-impl  rostered `implementer` -> approve REFUSED (no env advisor)
    #   - mu551-adv   rostered `advisor`     -> approve ALLOWED (no env advisor)
    # Both run non-TTY with NO AIDA_SESSION_ROLE, so the ONLY thing flipping the
    # verdict is the durable roster role. trace:STORY-646
    EXPECT=pass
    local impl_user="mu551-impl" adv_user="mu551-adv"

    # The MU-51x coordination cases push synthetic claim commits that can leave a
    # clone's store mid-rebase; recover both before we touch the store here.
    recover_store_rebase "$CLONE_A"
    recover_store_rebase "$CLONE_B"

    # A files a Draft spec for the implementer to (try to) approve, and a second
    # for the advisor to approve. add_spec lands Approved, so reset to Draft.
    local id_impl id_adv
    id_impl="$(add_spec "$CLONE_A" "mu551 impl-gated spec" task)"
    id_adv="$(add_spec "$CLONE_A" "mu551 adv-allowed spec" task)"
    aida_in "$CLONE_A" edit "$id_impl" --status draft >/dev/null 2>&1 || true
    aida_in "$CLONE_A" edit "$id_adv" --status draft >/dev/null 2>&1 || true
    push_from "$CLONE_A"; pull_into "$CLONE_B"

    # B rosters the two users (set-role is self-service by guardrail design).
    aida_in "$CLONE_B" team set-role "$impl_user" --role implementer >/dev/null 2>&1 || true
    aida_in "$CLONE_B" team set-role "$adv_user" --role advisor >/dev/null 2>&1 || true
    push_from "$CLONE_B"; pull_into "$CLONE_A"

    # The rostered IMPLEMENTER attempts the advisor-only promotion (no env role,
    # non-TTY): DESIRED -> refused (non-zero exit / advisor-authority message).
    local impl_out impl_rc impl_refused=0
    set +e
    impl_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER="$impl_user" \
        env -u AIDA_SESSION_ROLE "$AIDA_BIN" edit "$id_impl" --status approved 2>&1 )"
    impl_rc=$?
    set -e
    if [[ $impl_rc -ne 0 || "$impl_out" == *"advisor authority"* ]]; then impl_refused=1; fi

    # The rostered ADVISOR attempts the same promotion (no env role, non-TTY):
    # DESIRED -> allowed (exit 0, spec becomes Approved).
    local adv_out adv_rc adv_allowed=0
    set +e
    adv_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER="$adv_user" \
        env -u AIDA_SESSION_ROLE "$AIDA_BIN" edit "$id_adv" --status approved 2>&1 )"
    adv_rc=$?
    set -e
    local adv_status
    adv_status="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER="$adv_user" \
        env -u AIDA_SESSION_ROLE "$AIDA_BIN" show "$id_adv" 2>/dev/null | grep -iE '^Status:' | head -1 || true )"
    if [[ $adv_rc -eq 0 && "$adv_status" == *[Aa]pproved* ]]; then adv_allowed=1; fi

    # The refusal also names the durable team role (the STORY-646 message).
    local names_role=0
    if [[ "$impl_out" == *"team role"* ]]; then names_role=1; fi

    CASE_DETAIL="impl_refused=$impl_refused adv_allowed=$adv_allowed names_role=$names_role"
    if assert_ne "" "$id_impl" "impl spec id" \
        && assert_ne "" "$id_adv" "adv spec id" \
        && assert_eq "$impl_refused" "1" "rostered implementer is refused the approve" \
        && assert_eq "$adv_allowed" "1" "rostered advisor is allowed the approve" \
        && assert_eq "$names_role" "1" "refusal names the durable team role"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="impl_refused=$impl_refused (rc=$impl_rc) adv_allowed=$adv_allowed (rc=$adv_rc, status=[$adv_status]); impl_out=[${impl_out:0:200}]"
    fi
}

# --- MU-552: strict mode default-denies a non-rostered user a gated op -------
case_MU-552() {
    # STORY-647 (team RBAC slice 2): `[team] strict = true` makes the roster
    # authoritative. A NON-rostered user (no registry/team.toml entry) gets
    # LEAST-PRIVILEGE for gated ops (default-deny) instead of the permissive
    # env/default fallback — and the refusal is NOT bypassable by setting
    # AIDA_SESSION_ROLE. An advisor-ROSTERED user is allowed.
    #
    # Gated op probed: `aida burndown run --dry-run` (drain-start). The gate
    # fires FIRST, before the dry-run preview, so we exercise the gate WITHOUT
    # launching a real drain (no `--auto-complete`, no merge). trace:STORY-647
    EXPECT=pass

    # The MU-51x coordination cases can leave a clone mid-rebase; recover first.
    recover_store_rebase "$CLONE_A"
    recover_store_rebase "$CLONE_B"

    # Enable strict mode in clone B's project config (per-clone .aida/config.toml).
    local cfg="$CLONE_B/.aida/config.toml"
    if ! grep -q '^\[team\]' "$cfg" 2>/dev/null; then
        printf '\n[team]\nstrict = true\n' >> "$cfg"
    fi

    # Roster ONLY the advisor user; the non-rostered user is deliberately absent.
    local rostered_adv="mu552-adv" nonrostered="mu552-ghost"
    aida_in "$CLONE_B" team set-role "$rostered_adv" --role advisor >/dev/null 2>&1 || true
    push_from "$CLONE_B"; pull_into "$CLONE_A"

    # NON-rostered user, strict mode, with AIDA_SESSION_ROLE=advisor set anyway:
    # DESIRED -> still REFUSED (env can't grant authority in strict mode).
    local ghost_out ghost_rc ghost_refused=0
    set +e
    ghost_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER="$nonrostered" \
        AIDA_SESSION_ROLE=advisor "$AIDA_BIN" burndown run --dry-run 2>&1 )"
    ghost_rc=$?
    set -e
    if [[ $ghost_rc -ne 0 && ( "$ghost_out" == *"advisor"* || "$ghost_out" == *"drain"* ) ]]; then
        ghost_refused=1
    fi

    # ADVISOR-rostered user, strict mode, NO env role: DESIRED -> allowed (the
    # dry-run preview runs and exits 0; it never launches a drain).
    local adv_out adv_rc adv_allowed=0
    set +e
    adv_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER="$rostered_adv" \
        env -u AIDA_SESSION_ROLE "$AIDA_BIN" burndown run --dry-run 2>&1 )"
    adv_rc=$?
    set -e
    if [[ $adv_rc -eq 0 ]]; then adv_allowed=1; fi

    CASE_DETAIL="ghost_refused=$ghost_refused adv_allowed=$adv_allowed"
    if assert_eq "$ghost_refused" "1" "strict non-rostered (env=advisor) is default-denied the drain-start" \
        && assert_eq "$adv_allowed" "1" "strict advisor-rostered is allowed the drain-start dry path"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="ghost_refused=$ghost_refused (rc=$ghost_rc) adv_allowed=$adv_allowed (rc=$adv_rc); ghost_out=[${ghost_out:0:200}]; adv_out=[${adv_out:0:160}]"
    fi
}

# --- MU-553: a protected-tag spec requires advisor to transition -------------
case_MU-553() {
    # STORY-647 (team RBAC slice 2): a spec carrying any `[team] protected_tags`
    # entry may only be edited/transitioned by the configured protected_role
    # (advisor by default). A rostered IMPLEMENTER is refused the transition; a
    # rostered ADVISOR is allowed. Both run non-TTY with NO AIDA_SESSION_ROLE so
    # only the durable roster role + the protected tag flip the verdict.
    # trace:STORY-647
    EXPECT=pass

    recover_store_rebase "$CLONE_A"
    recover_store_rebase "$CLONE_B"

    # Configure the protected-tag set in clone B's project config.
    local cfg="$CLONE_B/.aida/config.toml"
    if ! grep -q 'protected_tags' "$cfg" 2>/dev/null; then
        if grep -q '^\[team\]' "$cfg" 2>/dev/null; then
            # Append the key under the existing [team] section header.
            awk '1; /^\[team\]/ && !done { print "protected_tags = [\"protected\"]"; done=1 }' \
                "$cfg" > "$cfg.tmp" && mv "$cfg.tmp" "$cfg"
        else
            printf '\n[team]\nprotected_tags = ["protected"]\n' >> "$cfg"
        fi
    fi

    # A protected spec in In-Progress (a non-advisor-only transition target so
    # the ONLY gate exercised is the protected-tag gate, not TASK-647's
    # approved-pipeline gate). add_spec lands Approved; tag it + move to draft so
    # implementer→approved would also be gated — instead test edit of the TAGGED
    # spec's title, which the protected gate refuses for a non-advisor.
    local id
    id="$(add_spec "$CLONE_A" "mu553 protected spec" task)"
    aida_in "$CLONE_A" edit "$id" --add-tag protected >/dev/null 2>&1 || true
    push_from "$CLONE_A"; pull_into "$CLONE_B"

    # Roster the two users.
    local impl_user="mu553-impl" adv_user="mu553-adv"
    aida_in "$CLONE_B" team set-role "$impl_user" --role implementer >/dev/null 2>&1 || true
    aida_in "$CLONE_B" team set-role "$adv_user" --role advisor >/dev/null 2>&1 || true
    push_from "$CLONE_B"; pull_into "$CLONE_A"

    # Rostered IMPLEMENTER tries to edit the protected spec (title change, a
    # non-status edit so only the protected-tag gate applies): DESIRED -> refused.
    local impl_out impl_rc impl_refused=0
    set +e
    impl_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER="$impl_user" \
        env -u AIDA_SESSION_ROLE "$AIDA_BIN" edit "$id" --title "impl tries to edit" 2>&1 )"
    impl_rc=$?
    set -e
    if [[ $impl_rc -ne 0 && "$impl_out" == *"protected"* ]]; then impl_refused=1; fi

    # Rostered ADVISOR makes the same edit: DESIRED -> allowed (exit 0).
    local adv_out adv_rc adv_allowed=0
    set +e
    adv_out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" AIDA_USER="$adv_user" \
        env -u AIDA_SESSION_ROLE "$AIDA_BIN" edit "$id" --title "advisor edits protected" 2>&1 )"
    adv_rc=$?
    set -e
    if [[ $adv_rc -eq 0 ]]; then adv_allowed=1; fi

    CASE_DETAIL="impl_refused=$impl_refused adv_allowed=$adv_allowed"
    if assert_ne "" "$id" "protected spec id" \
        && assert_eq "$impl_refused" "1" "rostered implementer is refused the protected-spec edit" \
        && assert_eq "$adv_allowed" "1" "rostered advisor is allowed the protected-spec edit"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="impl_refused=$impl_refused (rc=$impl_rc) adv_allowed=$adv_allowed (rc=$adv_rc); impl_out=[${impl_out:0:200}]"
    fi
}

# =========================================================================
# Case registry (ordered).
# =========================================================================
ALL_CASES=(MU-101 MU-103 MU-201 MU-202 MU-203 MU-204 MU-208 MU-301 MU-601 MU-401 MU-402 MU-502 MU-541 MU-504 MU-505 MU-506 MU-507 MU-511 MU-512 MU-513 MU-521 MU-551 MU-552 MU-553)

list_cases() {
    echo "Available cases:"
    for c in "${ALL_CASES[@]}"; do
        echo "  $c"
    done
}

run_case() {
    local name="$1"
    CASE_OK=0
    CASE_DETAIL=""
    CASE_FAIL_DETAIL=""
    EXPECT=""
    local fn="case_${name}"
    if ! declare -F "$fn" >/dev/null; then
        echo "  ${C_RED}???${C_RST}  ${C_BOLD}$name${C_RST}  ${C_DIM}no such case${C_RST}"
        return 0
    fi
    "$fn"
    local detail="$CASE_DETAIL"
    [[ -n "$CASE_FAIL_DETAIL" && "$CASE_OK" == "0" ]] && detail="$CASE_FAIL_DETAIL"

    if [[ "$EXPECT" == "pass" ]]; then
        if [[ "$CASE_OK" == "1" ]]; then
            pass "$name" "$detail"; COUNT_PASS=$((COUNT_PASS+1))
        else
            fail "$name" "$detail"; COUNT_FAIL=$((COUNT_FAIL+1)); SUITE_FAIL=1
        fi
    elif [[ "$EXPECT" == "known-gap" ]]; then
        if [[ "$CASE_OK" == "1" ]]; then
            gapclosed "$name" "$detail"; COUNT_GAP_CLOSED=$((COUNT_GAP_CLOSED+1))
        else
            gap "$name" "$detail"; COUNT_GAP=$((COUNT_GAP+1))
        fi
    else
        echo "  ${C_RED}???${C_RST}  ${C_BOLD}$name${C_RST}  ${C_DIM}case did not declare EXPECT${C_RST}"
        SUITE_FAIL=1
    fi
}

# =========================================================================
# CLI dispatch.
# =========================================================================
main() {
    local requested=()
    for arg in "$@"; do
        case "$arg" in
            --keep) KEEP=1 ;;
            --list) list_cases; exit 0 ;;
            -h|--help)
                grep -E '^#( |!)' "${BASH_SOURCE[0]}" | sed 's/^#//' | sed 's/^!.*//'
                exit 0 ;;
            all) requested=("${ALL_CASES[@]}") ;;
            MU-*) requested+=("$arg") ;;
            *) echo "unknown argument: $arg" >&2; exit 2 ;;
        esac
    done
    if [[ ${#requested[@]} -eq 0 ]]; then
        requested=("${ALL_CASES[@]}")
    fi

    do_setup

    echo "${C_BOLD}=== cases ===${C_RST}"
    for c in "${requested[@]}"; do
        run_case "$c"
    done

    echo
    echo "${C_BOLD}=== summary ===${C_RST}"
    echo "  ${C_GRN}pass${C_RST}=$COUNT_PASS  ${C_RED}fail${C_RST}=$COUNT_FAIL  ${C_YEL}gap${C_RST}=$COUNT_GAP  ${C_BLU}gap-closed${C_RST}=$COUNT_GAP_CLOSED"
    if [[ "$SUITE_FAIL" == "1" ]]; then
        echo "  ${C_RED}${C_BOLD}SUITE FAILED${C_RST} (an EXPECT=pass case did not pass)"
        exit 1
    fi
    echo "  ${C_GRN}${C_BOLD}SUITE OK${C_RST} (gaps are expected until shared coordination lands)"
    exit 0
}

main "$@"
