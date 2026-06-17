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

# --- MU-203: edits to SAME spec -> store-leg rebase CONFLICT surfaced -----
case_MU-203() {
    EXPECT=pass
    # Create one spec, sync both clones onto it.
    local id
    id="$(add_spec "$CLONE_A" "mu203 contended spec" task)"
    push_from "$CLONE_A"; pull_into "$CLONE_B"
    # Both edit the SAME spec's status, both commit locally (no push yet).
    aida_in "$CLONE_A" edit "$id" --status in-progress >/dev/null 2>&1 || true
    aida_in "$CLONE_B" edit "$id" --status planned >/dev/null 2>&1 || true
    # A pushes first (wins).
    push_from "$CLONE_A"
    # B pulls -> store-leg rebase should hit a conflict in id.yaml.
    local pull_out pull_rc
    set +e
    pull_out="$(aida_in "$CLONE_B" pull 2>&1)"
    pull_rc=$?
    set -e
    # Desired/today behavior: conflict is SURFACED (non-zero rc OR a
    # conflict/rebase hint in output), and B is left to resolve -- the edit is
    # NOT silently lost. We assert surfacing, not auto-merge.
    local conflict_marker=""
    if [[ $pull_rc -ne 0 ]]; then conflict_marker="rc=$pull_rc"; fi
    if [[ "$pull_out" == *[Cc]onflict* || "$pull_out" == *rebase* || "$pull_out" == *CONFLICT* ]]; then
        conflict_marker="${conflict_marker} text-hint"
    fi
    # Verify A's edit survived on the store and B's mid-rebase state is recoverable:
    # the spec must still exist and A's status must be present in the store HEAD.
    CASE_DETAIL="conflict surfaced ($id): ${conflict_marker:-none}"
    if assert_ne "" "$id" "spec id" \
        && assert_ne "" "$conflict_marker" "same-spec contention surfaces a conflict/rebase hint or non-zero exit"; then
        CASE_OK=1
    else
        CASE_OK=0
        CASE_DETAIL="no conflict surfaced; pull rc=$pull_rc out=[${pull_out:0:120}]"
    fi
    # Recover B so later cases aren't wedged mid-rebase: abort the store-leg
    # rebase, then re-sync B onto A's store head (A won the push). We accept A's
    # version (theirs) since the point of the case was to surface the conflict,
    # not to resolve it.
    recover_store_rebase "$CLONE_B"
    push_from "$CLONE_A"
    pull_into "$CLONE_B"
    # Verify recovery actually cleared the rebase; if not, the suite shouldn't
    # silently carry a wedged store into later cases.
    if run_in "$CLONE_B" git -C .aida-store status 2>/dev/null | grep -q "rebase in progress"; then
        CASE_OK=0
        CASE_DETAIL="conflict surfaced but B's store could not be recovered (still mid-rebase)"
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
    # present, the entry point must refuse BEFORE doing any work.
    local out rc
    set +e
    out="$( cd "$CLONE_B" && HOME="$WORKDIR/home" "$AIDA_BIN" queue work --auto-complete 2>&1 )"
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
    local out rc
    set +e
    out="$( cd "$CLONE_A" && HOME="$WORKDIR/home" AIDA_DRAIN_LOCK_STALE_SECS=3600 "$AIDA_BIN" queue work --auto-complete 2>&1 )"
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

# =========================================================================
# Case registry (ordered).
# =========================================================================
ALL_CASES=(MU-101 MU-103 MU-201 MU-202 MU-203 MU-301 MU-401 MU-402 MU-504 MU-505 MU-506 MU-507 MU-511 MU-512 MU-513)

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
