//! Cross-clone lease registry on the `aida-store` branch (STORY-637, slice 1
//! of the cross-clone coordination design,
//! `docs/plans/2026-06-16-cross-clone-coordination.md`).
//!
//! # The problem (MU-504)
//!
//! Session leases (`.aida/sessions/*.toml`) and the drain/solo locks are
//! **per-clone-local**: two clones sharing one `aida-store` have zero
//! cross-clone coordination. Both can `aida session start --owns FR-1`, both
//! can drain the same shared queue → duplicate PRs, merge races, double-work.
//! Intra-clone coordination already works (`find_scope_lease_conflict`,
//! `drain_lock::decide_lock`); the gap is purely cross-clone visibility.
//!
//! # The registry (substrate-as-bouncer, shared layer)
//!
//! Claim records live on the orphan `aida-store` branch — the existing shared
//! substrate — under a new `coordination/leases/<scope>.toml` tree, one file
//! per leased scope (a spec id or a worktree scope). This is the SAME storage
//! shape as `objects/TYPE/000/SPEC.yaml`: different scopes never git-conflict,
//! and CAS contention only happens on a genuine same-scope race.
//!
//! A claim carries enough to (a) identify the holder and (b) decide liveness:
//! `scope`, `node_id`, `clone_path`, `host`, `pid`, `agent`, `started_at`,
//! `heartbeat_at`, `ttl_secs`, `review_verb`.
//!
//! # The protocol (CAS push-wins, mirrors `git_ops::register_node_full`)
//!
//! On `aida session start --owns <scope>` (and `aida agent new`):
//! 1. `pull_rebase` the store (cheap, coarse event).
//! 2. Read `coordination/leases/<scope>.toml`; run the pure [`decide_claim`].
//! 3. On [`ClaimDecision::Refuse`] → error naming the holder
//!    (host / clone_path / agent / age) unless `--force`.
//! 4. On Acquire/Reclaim → write our claim, commit, push with CAS retry on a
//!    non-ff rejection (bounded).
//!
//! Release ([`release_claim`]) runs on `aida session end`: delete the file,
//! commit, push (best-effort — staleness covers a crash).
//!
//! # Liveness (decides "live" vs "reclaimable")
//!
//! A session-lease claim has NO heartbeat: it is written once by a process
//! that exits immediately, so `heartbeat_at` stays equal to `started_at` for
//! the claim's whole life and "heartbeat age" is really claim age (BUG-1765 —
//! the once-planned slice-2 refresh cannot exist without a long-lived driver).
//! Liveness is therefore decided per claim kind:
//!
//! - **Process-backed claims** (drain/solo locks): the holder IS a long-lived
//!   process, so same host + dead pid → reclaim immediately, and their
//!   heartbeat genuinely refreshes (`drain_lock`'s heartbeat thread).
//! - **Session-lease claims, same host:** probed against the holder clone's
//!   OWN local lease table at `clone_path` with the same worktree/live-claude
//!   matrix the local table trusts ([`probe_session_liveness_with`]). A
//!   verified-live (or dormant-but-standing) session keeps its claim alive
//!   indefinitely — TTL age alone never expires it. The probe can only ADD
//!   liveness, never subtract it: when it affirms nothing, the clauses below
//!   decide exactly as they would have (so a peer racing the claim write
//!   cannot insta-reclaim, and a gone holder still ages out).
//! - **Session-lease claims, remote host:** `now - heartbeat_at > ttl_secs`
//!   is stale and reclaimable — the TTL backstop. Nothing short of a real
//!   heartbeat can see a remote peer, and refusing forever would turn one
//!   crashed remote clone into a permanent deadlock; the honest contract is
//!   that a remote session longer than its TTL (30 min) WILL be told it
//!   expired.
//! - **`--force`** short-circuits to acquire (the escape hatch) — handled at
//!   the call site, not in the pure decision fn.
//!
//! # Best-effort / no-store
//!
//! If there is no `origin` remote or the store can't be reached, we do NOT
//! block work: the call site WARNs ("cross-clone coordination unavailable")
//! and falls through to the existing local-only behavior. `session start`
//! must never become brittle on the network.
//!
//! trace:STORY-637 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Subdir (under the store worktree root) holding one file per leased scope.
const LEASES_SUBDIR: &str = "coordination/leases";

/// Default staleness horizon for a claim whose holder is on a DIFFERENT host
/// (so we can't probe its pid). 30 minutes — matches `drain_lock`'s default,
/// comfortably longer than any single phase, far shorter than a wedged holder
/// a human would want auto-cleared. Heartbeat refresh (slice 2) keeps a live
/// long-running holder above this floor.
pub(crate) const DEFAULT_TTL_SECS: u64 = 1800;

/// Max CAS push attempts before giving up (mirrors `git_ops::MAX_CAS_RETRIES`).
const MAX_CAS_RETRIES: u32 = 5;

/// A cross-clone lease claim, serialized as TOML at
/// `coordination/leases/<sanitized-scope>.toml` on the `aida-store` branch.
/// trace:STORY-637 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Claim {
    // Invocation identity; old records remain readable but cannot authorize launch rollback.
    // trace:BUG-2188 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<String>,
    /// Raw scope string (a SPEC-ID like `FR-1`, or a worktree scope).
    pub scope: String,
    /// Which clone/node holds it (from `.aida-store/.aida/node.toml`).
    pub node_id: String,
    /// Absolute path of the holding clone (the project root). Informational +
    /// recovery breadcrumb in the refusal message.
    pub clone_path: String,
    /// Host the holder runs on. Drives the same-host pid fast path.
    pub host: String,
    /// PID of the process that took the claim (the `session start` shell).
    pub pid: u32,
    /// Kernel process start time paired with `pid`; prevents a recycled PID
    /// from keeping a dead process-backed claim alive.
    // trace:TASK-1284 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start_time: Option<String>,
    /// Agent name/role context, best-effort (informational in the refusal).
    pub agent: String,
    /// RFC3339 UTC when the claim was taken.
    pub started_at: String,
    /// RFC3339 UTC of the last heartbeat. Slice 1 sets this to `started_at`;
    /// slice 2 refreshes it on the drain/session loop tick. The TTL backstop
    /// compares `now - heartbeat_at` against `ttl_secs`.
    pub heartbeat_at: String,
    /// Staleness horizon in seconds for the cross-host TTL backstop.
    pub ttl_secs: u64,
    /// Whether the recorded `pid` is a LONG-LIVED process whose liveness
    /// faithfully tracks the claim (e.g. a drain loop — slice 2). When `true`,
    /// the same-host pid probe is an exact reclaim signal. When `false` (the
    /// slice-1 session-lease case), the `pid` is the EPHEMERAL `aida session
    /// start` shell that exits the instant the lease is written — so its death
    /// says nothing about whether the worktree session is still live. For
    /// session leases the holder is the worktree: on the holder's own host it
    /// is probeable through that clone's local lease table (BUG-1765,
    /// [`probe_session_liveness`]), and the TTL backstop governs whatever the
    /// probe cannot affirm. Defaults to `false` so an old reader / older
    /// binary treats every claim as TTL-governed (never wrongly reclaims via
    /// a dead ephemeral pid).
    // trace:STORY-637 trace:BUG-1765 | ai:claude
    #[serde(default)]
    pub process_backed: bool,
    /// True for advisory PR/MR-review claims (no worktree). Informational —
    /// carried so the record mirrors the local `SessionLease` shape.
    #[serde(default)]
    pub review_verb: bool,
    /// STORY-711 slice 1: the authorizing advisor's session/agent id, mirrors
    /// the same field added to `SessionLeaseLite`. `#[serde(default)]` so an
    /// older claim TOML with no `authorized_by` key still deserializes
    /// (`None`). Slice 1's `aida lock` CLI operates on the LOCAL session
    /// lease, not this cross-clone registry — this field is forward-compat
    /// groundwork only; nothing writes a non-`None` value here yet.
    // trace:STORY-711 | ai:claude
    #[serde(default)]
    pub authorized_by: Option<String>,
}

impl Claim {
    /// Age of the heartbeat in seconds relative to `now`. An unparseable
    /// timestamp returns `None` ("age unknown" → pid-liveness alone decides).
    fn heartbeat_age_secs(&self, now: DateTime<Utc>) -> Option<u64> {
        let hb = DateTime::parse_from_rfc3339(&self.heartbeat_at)
            .ok()?
            .with_timezone(&Utc);
        let secs = now.signed_duration_since(hb).num_seconds();
        Some(secs.max(0) as u64)
    }
}

/// The pure decision: given the claim currently on the store (if any) and the
/// claim we want to write, should we ACQUIRE (no contention), be REFUSED (a
/// live foreign holder), or RECLAIM (a dead/stale foreign holder)?
///
/// Liveness is injected as a closure so the paths are unit-testable without
/// spawning real processes — mirrors `drain_lock::decide_lock`. `--force` is
/// NOT modeled here (the call site short-circuits to acquire), keeping this
/// fn pure and total. trace:STORY-637 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClaimDecision {
    /// No live foreign claim stands in the way — write ours.
    Acquire,
    /// A live, non-stale foreign claim holds the scope — refuse, surfacing it.
    Refuse { holder: Box<Claim> },
    /// A foreign claim exists but is reclaimable (dead pid same-host, or aged
    /// past its TTL) — overwrite it with ours. `stale_reason` is for the note
    /// the call site prints.
    Reclaim { stale_reason: String },
}

/// Decide ACQUIRE / REFUSE / RECLAIM. An absent claim → ACQUIRE. A claim from
/// OUR OWN clone (same `node_id`) → ACQUIRE (re-entrant; we already hold it).
/// A foreign claim is reclaimable when it is `process_backed` and its pid is
/// dead AND on our host, or when its heartbeat is older than its `ttl_secs`;
/// otherwise REFUSE. (Session leases are NOT `process_backed` — the TTL
/// backstop governs them UNLESS `session_live` affirms the holder's session;
/// see [`Claim::process_backed`] and below.)
///
/// `our_host` is this machine's hostname (used to gate the pid fast path —
/// probing a pid only makes sense for a same-host holder). `is_alive` reports
/// whether a pid is currently running on THIS machine.
///
/// BUG-1765: `session_live` is the verified-live probe for a session-lease
/// claim (the real one is [`probe_session_liveness`]; tests inject). It is
/// consulted only for a non-`process_backed` claim, and `true` means the
/// holder's session was POSITIVELY verified live on this host — which REFUSES
/// the acquire even when the claim's heartbeat has aged past its TTL, because
/// nothing ever refreshes a session claim's heartbeat and expiring a live
/// session's claim is how the registry silently stopped coordinating after
/// 30 minutes. `false` means "no affirmative signal" (remote host, holder
/// gone, probe unavailable), NEVER "verified dead" — the remaining clauses
/// then decide exactly as they always have. The probe can only add liveness.
// trace:BUG-1765 | ai:claude
pub(crate) fn decide_claim(
    existing: Option<&Claim>,
    ours: &Claim,
    now: DateTime<Utc>,
    our_host: &str,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
    session_live: impl Fn(&Claim) -> bool,
) -> ClaimDecision {
    let Some(holder) = existing else {
        return ClaimDecision::Acquire;
    };
    // Our own clone already holds it — re-entrant acquire (e.g. a re-run, or
    // the orchestrator re-driving the same spec). The CLONE PATH is the true
    // per-clone discriminator: two clones sharing one store inherit the SAME
    // `node_id` in the store worktree's `.aida/node.toml` (it rides the
    // aida-store branch), so node_id alone can't tell clones apart — but each
    // clone has a distinct project root. Match on the canonicalized clone path,
    // falling back to node_id when a path is missing (older claims).
    // trace:STORY-637 | ai:claude
    let same_clone = if !holder.clone_path.is_empty() && !ours.clone_path.is_empty() {
        holder.clone_path == ours.clone_path
    } else {
        holder.node_id == ours.node_id
    };
    if same_clone {
        return ClaimDecision::Acquire;
    }
    // Same-host fast path: for a PROCESS-BACKED claim (a long-lived holder
    // whose pid faithfully tracks liveness — e.g. a drain loop, slice 2), a
    // dead pid means the holder crashed/exited → reclaim immediately, exactly
    // like drain_lock's pid probe. A session-lease claim is NOT process-backed
    // (its pid is the ephemeral `session start` shell), so its death is
    // meaningless — only the TTL backstop governs it. trace:STORY-637
    let same_host = !holder.host.is_empty() && holder.host == our_host;
    if holder.process_backed && same_host && !is_alive(holder.pid, holder.pid_start_time.as_deref())
    {
        return ClaimDecision::Reclaim {
            stale_reason: format!(
                "holder pid {} on {} is not running",
                holder.pid, holder.host
            ),
        };
    }
    // BUG-1765: a session-lease claim whose holding session is POSITIVELY
    // verified live (same-host probe of the holder clone's own lease table)
    // is live, full stop — the TTL backstop below must not expire it, because
    // no heartbeat ever refreshes a session claim. Gated off `process_backed`
    // (a lock's pid probe above is already exact) and evaluated before the
    // TTL so verified liveness outranks aged-out-ness, never the reverse.
    // trace:BUG-1765 | ai:claude
    if !holder.process_backed && session_live(holder) {
        return ClaimDecision::Refuse {
            holder: Box::new(holder.clone()),
        };
    }
    // Universal TTL backstop (portable across hosts): heartbeat aged out.
    let aged_out = holder
        .heartbeat_age_secs(now)
        .map(|age| age > holder.ttl_secs)
        .unwrap_or(false);
    if aged_out {
        return ClaimDecision::Reclaim {
            stale_reason: format!(
                "holder heartbeat is older than its {}s TTL",
                holder.ttl_secs
            ),
        };
    }
    ClaimDecision::Refuse {
        holder: Box::new(holder.clone()),
    }
}

/// BUG-1764: decide whether a recorded claim is STALE for the purpose of
/// *reporting* it. Returns `Some(reason)` when the claim must not be presented
/// as active, `None` when it is presumed live.
///
/// Every reader of [`list_claims`] must route through this. Before BUG-1764
/// none of them applied any expiry predicate at all, so a claim displayed
/// forever once written: `aida session leases` filtered only on `clone_path`,
/// and `aida status --full` / `aida doctor` / `aida team` took the whole list
/// (measured on one host: 211 claims reported as active, the oldest 93 days
/// past its own 1800s TTL).
///
/// # Why this is TTL-first and not a pid check
///
/// A session lease is **not** [`Claim::process_backed`] — its recorded `pid` is
/// the ephemeral `aida session start` shell, which exits the instant the lease
/// is written, so neither its death nor its survival says anything about the
/// session. The TTL/heartbeat age is therefore the governing signal, and the
/// pid probe can only ever ADD staleness. That ordering is what makes a PID
/// collision harmless: BUG-1764 was filed because a lease recording `pid = 2`
/// (`kthreadd`) or `pid = 202` (a live kworker) looked alive forever, and under
/// a TTL-first predicate no pid value can grant life to an aged-out claim.
///
/// Clause order mirrors [`decide_claim`] so display and reclaim never disagree:
/// a claim this function calls stale is one `decide_claim` would reclaim.
///
/// `is_alive` is injected so every clause is unit-testable without spawning a
/// process — the same shape `decide_claim` uses.
///
/// BUG-1765: `session_live` mirrors [`decide_claim`]'s probe — a session-lease
/// claim whose holding session is positively verified live is reported LIVE
/// even past its TTL (the claim's heartbeat is never refreshed, so TTL age is
/// just claim age). `false` means "no affirmative signal", never "dead": the
/// clauses below then decide unchanged.
// trace:BUG-1764 | ai:claude
// trace:BUG-1765 | ai:claude
pub(crate) fn claim_staleness(
    claim: &Claim,
    now: DateTime<Utc>,
    our_host: &str,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
    session_live: impl Fn(&Claim) -> bool,
) -> Option<String> {
    // Clause 0 (BUG-1765) — a verified-live session outranks TTL age, exactly
    // as in `decide_claim`: a claim this clause spares is one `decide_claim`
    // would refuse to reclaim. trace:BUG-1765 | ai:claude
    if !claim.process_backed && session_live(claim) {
        return None;
    }
    // Clause 1 — the universal TTL backstop, portable across hosts and the only
    // signal that applies to a non-process-backed session lease the probe
    // could not affirm.
    if let Some(age) = claim.heartbeat_age_secs(now) {
        if age > claim.ttl_secs {
            return Some(format!(
                "heartbeat {age}s old, past its {}s TTL",
                claim.ttl_secs
            ));
        }
    }
    // Clause 2 — same-HOST pid evaluation. Gated on `process_backed` for the
    // reason above. Note this is keyed on `host`, NOT `clone_path`: a claim from
    // a different clone on THIS machine is fully probeable against the local
    // process table and must not be excused as unreachable just because its
    // path differs (BUG-1764 clause 3 — all eight leases in the filed
    // measurement were `host = "imac"` queried from `imac`).
    let same_host = !claim.host.is_empty() && claim.host == our_host;
    if claim.process_backed && same_host && !is_alive(claim.pid, claim.pid_start_time.as_deref()) {
        return Some(format!(
            "holder pid {} on {} is not running",
            claim.pid, claim.host
        ));
    }
    None
}

/// BUG-1764: the STRICT process-identity probe used by [`claim_staleness`].
///
/// Deliberately *not* `aida_core::liveness::process_identity_is_alive`, which
/// returns `true` when the record carries no `pid_start_time` — the documented
/// TASK-1284 degrade that keeps legacy drain/solo lock holders from being
/// reclaimed merely because their record predates process identities. That
/// degrade is right for reclaim (wrongly reclaiming a live drain is far more
/// destructive than printing a stale row) and wrong for reporting, which is
/// what BUG-1764 clause 2 asks for: an unverifiable claim must not be trusted
/// as live.
///
/// | recorded start | live start (matching format) | verdict |
/// |---|---|---|
/// | (pid not alive) | — | dead |
/// | absent | — | **unverifiable → not live** |
/// | present | unreadable / unparseable | **unverifiable → not live** |
/// | present | differs | dead (the PID-collision case) |
/// | present | equal | alive |
///
/// Use for display and for the opt-in `--prune-stale` clear only. Do NOT reuse
/// as a reclaim predicate for the drain/solo locks.
// trace:BUG-1764 | ai:claude
// trace:BUG-2130 | ai:agy
pub(crate) fn strictly_alive(pid: u32, recorded_start: Option<&str>) -> bool {
    strictly_alive_with(
        pid,
        recorded_start,
        crate::process_probe::pid_is_alive,
        aida_core::liveness::process_start_identity_legacy,
        aida_core::liveness::process_start_identity_exact,
    )
}

/// Injectable core of [`strictly_alive`], so the collision case is reproducible
/// in a unit test without root and without a real pid 2.
// trace:BUG-1764 | ai:claude
// trace:BUG-2130 | ai:agy
pub(crate) fn strictly_alive_with(
    pid: u32,
    recorded_start: Option<&str>,
    is_pid_alive: impl FnOnce(u32) -> bool,
    live_start: impl FnOnce(u32) -> Option<String>,
    live_exact_start: impl FnOnce(u32) -> Option<String>,
) -> bool {
    // No recorded identity → nothing to compare → unverifiable, not live.
    let Some(recorded) = recorded_start else {
        return false;
    };
    aida_core::liveness::process_identity_matches_strict_with(
        pid,
        recorded,
        is_pid_alive,
        live_start,
        live_exact_start,
    )
}

/// BUG-1765: the real verified-live probe for a session-lease claim, injected
/// into [`decide_claim`] / [`claim_staleness`] as `session_live`.
///
/// `true` iff the claim's holder is on THIS host and the holder clone's OWN
/// local lease table (`<clone_path>/.aida/sessions/`) holds a lease for the
/// claim's scope whose state is `Live` or `Dormant` under the exact matrix the
/// local table trusts (`lease_state_for`: worktree exists + live claude + age
/// — BUG-1764 clause 3 established a same-host claim is fully probeable).
/// `Dormant` counts as live because the worktree and lease still stand:
/// reclaiming over a paused session's WIP is the duplicate-work race this
/// registry exists to prevent (`--force` remains the escape hatch).
///
/// Everything else — remote host, empty/missing `clone_path`, no matching
/// lease, `Stale` state — is `false`, which means "no affirmative signal",
/// never "verified dead": the callers' remaining clauses (TTL backstop) then
/// decide unchanged. The probe can only ADD liveness, so a peer that races
/// the gap between the cross-clone claim write and the local lease write
/// cannot insta-reclaim a fresh claim.
// trace:BUG-1765 | ai:claude
pub(crate) fn probe_session_liveness(claim: &Claim, our_host: &str) -> bool {
    // Cheap gates first so the process-table scan only runs when the answer
    // could matter.
    if claim.process_backed || claim.clone_path.is_empty() {
        return false;
    }
    if claim.host.is_empty() || claim.host != our_host {
        return false;
    }
    let live_sessions = crate::process_probe::probe_live_claude_sessions();
    probe_session_liveness_with(claim, our_host, &live_sessions, Utc::now())
}

/// Injectable-clock core of [`probe_session_liveness`]: the caller supplies
/// the live-claude session listing (one process-table scan can serve many
/// claims — see [`partition_claims`]) and `now`.
// trace:BUG-1765 | ai:claude
pub(crate) fn probe_session_liveness_with(
    claim: &Claim,
    our_host: &str,
    live_sessions: &[crate::process_probe::LiveSession],
    now: DateTime<Utc>,
) -> bool {
    if claim.process_backed || claim.clone_path.is_empty() {
        return false;
    }
    if claim.host.is_empty() || claim.host != our_host {
        return false;
    }
    let clone_root = Path::new(&claim.clone_path);
    if !clone_root.is_dir() {
        return false;
    }
    // Freshest lease matching the claim's scope, case-insensitive — the same
    // selection rule the intra-clone conflict guard uses.
    let leases = crate::list_leases(clone_root);
    let Some(lease) = crate::find_scope_lease_conflict(&leases, &claim.scope) else {
        return false;
    };
    matches!(
        crate::lease_state_for(&lease, live_sessions, now),
        aida_core::liveness::LeaseState::Live | aida_core::liveness::LeaseState::Dormant
    )
}

/// BUG-1764: partition claims into (presumed-live, stale-with-reason) using
/// [`claim_staleness`] and the real strict probe. The single entry point for
/// every reporting surface, so a new reader cannot forget the predicate.
///
/// BUG-1765: the session-liveness probe shares ONE process-table scan across
/// the whole claim list, so `aida session leases` / `status` / `doctor` /
/// `team` inherit verified-live reporting at the cost of a single probe.
// trace:BUG-1764 | ai:claude
// trace:BUG-1765 | ai:claude
pub(crate) fn partition_claims(
    claims: Vec<Claim>,
    now: DateTime<Utc>,
    our_host: &str,
) -> (Vec<Claim>, Vec<(Claim, String)>) {
    // Scan the process table once, and only if some claim could need it
    // (a non-process-backed same-host claim with a clone path to probe).
    let needs_probe = claims.iter().any(|c| {
        !c.process_backed && !c.clone_path.is_empty() && !c.host.is_empty() && c.host == our_host
    });
    let live_sessions = if needs_probe {
        crate::process_probe::probe_live_claude_sessions()
    } else {
        Vec::new()
    };
    let session_live = |c: &Claim| probe_session_liveness_with(c, our_host, &live_sessions, now);
    let mut live = Vec::new();
    let mut stale = Vec::new();
    for claim in claims {
        match claim_staleness(&claim, now, our_host, strictly_alive, &session_live) {
            Some(reason) => stale.push((claim, reason)),
            None => live.push(claim),
        }
    }
    (live, stale)
}
/// Sanitize a scope into a safe, collision-resistant filename stem. Lowercases,
/// keeps `[a-z0-9._-]`, maps every other byte to `_`. A trailing FNV-1a hash of
/// the ORIGINAL scope guarantees two scopes that sanitize to the same stem
/// (e.g. `FR/1` vs `FR_1`) still get distinct files — the same distinctness
/// guarantee as `worktree_lease::lease_id_from_agent_id`. trace:STORY-637
pub(crate) fn sanitize_scope(scope: &str) -> String {
    let stem: String = scope
        .chars()
        .map(|c| {
            let l = c.to_ascii_lowercase();
            if l.is_ascii_alphanumeric() || matches!(l, '.' | '-' | '_') {
                l
            } else {
                '_'
            }
        })
        .collect();
    let stem = if stem.is_empty() {
        "scope".to_string()
    } else {
        stem
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in scope.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{stem}-{hash:016x}")
}

/// Best-effort store refresh for a coordination write. An unreachable store
/// keeps the local-only path (`Ok`); a store-sync refusal (hypothesis evidence
/// or allocation claims) is the one failure that stops the write and its
/// publish (`Err`).
// trace:BUG-2162 | ai:claude
// trace:BUG-2017 | ai:claude
fn refresh_store(store_root: &Path, branch: &str) -> Result<std::result::Result<(), String>> {
    match aida_core::git_ops::pull_rebase(store_root, "origin", branch) {
        Ok(()) => Ok(Ok(())),
        Err(e) if aida_core::git_ops::is_sync_refusal(&e) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// [`refresh_store`] for a void cleanup path: report a refusal and tell the
/// caller to stop.
fn refresh_store_or_stop(store_root: &Path, branch: &str) -> bool {
    match refresh_store(store_root, branch) {
        Ok(_) => false,
        Err(e) => {
            eprintln!("warning: store coordination update skipped: {e:#}");
            true
        }
    }
}

/// Directory under the store worktree holding the lease claims.
fn leases_dir(store_root: &Path) -> PathBuf {
    store_root.join(LEASES_SUBDIR)
}

/// Path of the claim file for `scope` under the store worktree.
pub(crate) fn claim_path(store_root: &Path, scope: &str) -> PathBuf {
    leases_dir(store_root).join(format!("{}.toml", sanitize_scope(scope)))
}

/// Read + parse a claim file, if present and well-formed. A missing file or a
/// parse error both yield `None` — a corrupt claim must never wedge a lease
/// (treated as "no claim", i.e. reclaimable). trace:STORY-637
fn read_claim(path: &Path) -> Option<Claim> {
    let raw = std::fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

/// Best-effort host name (informational; drives the same-host pid fast path).
///
/// `AIDA_HOST_OVERRIDE` (test hook) takes precedence when set non-empty: it lets
/// the multi-clone harness simulate two DISTINCT hosts on one machine so the
/// cross-host TTL/heartbeat path (where pid liveness is meaningless) can be
/// exercised without a second physical machine. Production never sets it.
/// trace:STORY-642 | ai:claude
pub(crate) fn hostname() -> String {
    if let Ok(h) = std::env::var("AIDA_HOST_OVERRIDE") {
        if !h.is_empty() {
            return h;
        }
    }
    sysinfo::System::host_name().unwrap_or_default()
}

/// Canonicalize a clone path to a stable string for the claim's clone-identity
/// field, so the holder's claim and ours compare equal regardless of symlinks
/// or `.`/`..` segments. Falls back to the lexical path when canonicalize fails
/// (e.g. the path is being created).
fn canonical_clone_path(clone_path: &Path) -> String {
    clone_path
        .canonicalize()
        .unwrap_or_else(|_| clone_path.to_path_buf())
        .display()
        .to_string()
}

/// Resolve this clone's node id. The clone's WORK binding wins (the same
/// rule as `allocation_authority::clone_node_id`). Without one: two clones sharing one store inherit the same
/// `node_id` in the store worktree's `.aida/node.toml` (it rides the branch),
/// so that is NOT a per-clone discriminator — the authoritative per-clone id is
/// the `registry/nodes.toml` entry keyed by `clone_path`. Look that up first,
/// falling back to the store node.toml, then `"1"`. Used only for a human-
/// readable label; clone-identity comparisons key off `clone_path`.
/// trace:STORY-637 | ai:claude
fn node_id_for_clone(store_root: &Path, clone_path: &Path) -> String {
    // The clone's WORK binding lives in its own git common dir, so it is
    // per-clone already and outranks every shared-store guess: a clone whose
    // legacy node was retired must not keep stamping the retired id.
    // trace:BUG-2378 | ai:claude
    // An unreadable binding is not "no binding": the fallbacks below may
    // name a retired node, so say so rather than stamp it silently.
    match aida_core::work_binding::load(store_root) {
        Ok(Some(binding)) => return binding.node_id,
        Ok(None) => {}
        Err(e) => eprintln!(
            "warning: could not read this clone's node binding ({e:#}); \
             the lease names the node from the shared store, which may be a retired node"
        ),
    }
    let canon = canonical_clone_path(clone_path);
    let registry_path = store_root.join("registry").join("nodes.toml");
    if let Ok(registry) = aida_core::NodeRegistry::load(&registry_path) {
        for node in &registry.nodes {
            if let Some(p) = &node.clone_path {
                let np = p
                    .canonicalize()
                    .unwrap_or_else(|_| p.clone())
                    .display()
                    .to_string();
                if np == canon {
                    return node.id.clone();
                }
            }
        }
    }
    let node_config_path = store_root.join(".aida").join("node.toml");
    aida_core::NodeConfig::load(&node_config_path)
        .map(|c| c.node_id)
        .unwrap_or_else(|_| "1".to_string())
}

/// Build the claim we want to write for `scope`. `clone_path` is the project
/// root (the parent of the store worktree); `agent` is best-effort role/agent
/// context for the refusal message.
fn build_claim(
    store_root: &Path,
    scope: &str,
    clone_path: &Path,
    agent: &str,
    review_verb: bool,
) -> Claim {
    let now = Utc::now().to_rfc3339();
    Claim {
        claim_id: Some(uuid::Uuid::new_v4().to_string()),
        scope: scope.to_string(),
        node_id: node_id_for_clone(store_root, clone_path),
        clone_path: canonical_clone_path(clone_path),
        host: hostname(),
        pid: std::process::id(),
        pid_start_time: aida_core::liveness::process_start_identity(std::process::id()),
        agent: agent.to_string(),
        started_at: now.clone(),
        // Slice 1: heartbeat == started_at. Slice 2 refreshes periodically.
        heartbeat_at: now,
        ttl_secs: DEFAULT_TTL_SECS,
        // A session lease is worktree-backed, not process-backed: the pid above
        // is the ephemeral `aida session start` shell that exits immediately, so
        // it must NOT drive same-host reclaim. The same-host session probe
        // (BUG-1765), the TTL backstop, an explicit `aida session end` release,
        // or `--force` govern it instead. Drain/solo claims set this `true`.
        // trace:STORY-637
        process_backed: false,
        review_verb,
        // trace:STORY-711 | ai:claude — not wired yet; see the field doc.
        authorized_by: None,
    }
}

/// Outcome of [`acquire_claim_receipted`], so the call site can print the right note.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AcquireOutcome {
    /// Claimed cleanly (no prior claim, or our own clone re-claiming).
    Acquired,
    /// Reclaimed a stale/dead foreign claim. Carries the reason for a note.
    Reclaimed(String),
    /// Cross-clone coordination was unavailable (no remote / store unreachable);
    /// the call site proceeded local-only. Carries a one-line reason to WARN.
    Unavailable(String),
}

/// Acquire a cross-clone lease claim for `scope` on the shared store.
///
/// `store_root` is the `.aida-store` worktree; `clone_path` the project root.
/// `force` short-circuits the decision to acquire (the `--force` / `--force-claim`
/// escape). On a live foreign claim, returns `Err` whose message names the
/// holder (host / clone_path / agent / age) and the recovery paths.
///
/// **Best-effort:** with no `origin` remote, or if the store can't be reached,
/// returns `Ok(AcquireOutcome::Unavailable(..))` rather than erroring — the
/// caller WARNs and proceeds local-only. trace:STORY-637 | ai:claude
// Legacy test entrypoint; production retains the acquisition receipt.
// trace:BUG-2188 | ai:codex
#[cfg(test)]
pub(crate) fn acquire_claim(
    store_root: &Path,
    scope: &str,
    clone_path: &Path,
    agent: &str,
    review_verb: bool,
    force: bool,
) -> Result<AcquireOutcome> {
    acquire_claim_receipted(store_root, scope, clone_path, agent, review_verb, force)
        .map(|(outcome, _)| outcome)
}

// trace:BUG-2188 | ai:codex
pub(crate) fn acquire_claim_receipted(
    store_root: &Path,
    scope: &str,
    clone_path: &Path,
    agent: &str,
    review_verb: bool,
    force: bool,
) -> Result<(AcquireOutcome, Option<Claim>)> {
    if !store_root.exists() {
        return Ok((
            AcquireOutcome::Unavailable("no .aida-store worktree attached".to_string()),
            None,
        ));
    }
    // Cross-clone coordination only matters when clones SHARE a remote store.
    // A solo clone (no origin) has nothing to coordinate against — proceed.
    if !aida_core::git_ops::has_remote(store_root, "origin") {
        return Ok((
            AcquireOutcome::Unavailable("store has no origin remote (solo clone)".to_string()),
            None,
        ));
    }
    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let ours = build_claim(store_root, scope, clone_path, agent, review_verb);
    let our_host = ours.host.clone();
    let path = claim_path(store_root, scope);

    for attempt in 0..MAX_CAS_RETRIES {
        let (outcome, committed, written, adopted, commit_sha) =
            aida_core::db::with_store_write_lock(store_root, || {
                // Keep refresh, decision and write under main's shared writer lock.
                // trace:BUG-2188 | ai:codex
                if let Err(e) = refresh_store(store_root, &branch)? {
                    return Ok((
                        AcquireOutcome::Unavailable(format!("store unreachable ({e})")),
                        false,
                        None,
                        false,
                        None,
                    ));
                }
                // Step 2: decide against the freshly-pulled claim.
                let existing = read_claim(&path);
                // A same-clone re-claim refreshes started_at/heartbeat/pid/
                // session/agent (the TTL and BUG-1765 liveness probe read
                // them) but keeps the existing claim_id: the claim is adopted,
                // so it never yields a rollback receipt.
                // trace:BUG-2188 | ai:claude
                let adopted_id = existing
                    .as_ref()
                    .filter(|c| c.clone_path == ours.clone_path)
                    .map(|c| c.claim_id.clone());
                let adopted = adopted_id.is_some();
                let mine = Claim {
                    claim_id: adopted_id.unwrap_or_else(|| ours.claim_id.clone()),
                    ..ours.clone()
                };
                let outcome_note = if force {
                    // --force: skip the decision, but still report what we overrode.
                    existing
                        .as_ref()
                        .filter(|h| h.node_id != ours.node_id)
                        .map(|h| {
                            format!("--force overriding {}'s claim ({})", h.host, h.clone_path)
                        })
                } else {
                    match decide_claim(
                        existing.as_ref(),
                        &ours,
                        Utc::now(),
                        &our_host,
                        crate::process_probe::process_identity_is_alive,
                        // BUG-1765: a same-host holder whose session is verifiably
                        // live keeps its claim past the TTL. trace:BUG-1765 | ai:claude
                        |c: &Claim| probe_session_liveness(c, &our_host),
                    ) {
                        ClaimDecision::Refuse { holder } => {
                            anyhow::bail!("{}", refusal_message(scope, &holder, &path));
                        }
                        ClaimDecision::Acquire => None,
                        ClaimDecision::Reclaim { stale_reason } => Some(stale_reason),
                    }
                };

                // Step 3: write our claim, commit, push. On non-ff → pull + retry.
                std::fs::create_dir_all(leases_dir(store_root)).ok();
                let toml = toml::to_string_pretty(&mine)
                    .map_err(|e| anyhow::anyhow!("could not serialize lease claim: {e}"))?;
                std::fs::write(&path, toml).map_err(|e| {
                    anyhow::anyhow!("could not write lease claim {}: {e}", path.display())
                })?;

                let rel = format!("{LEASES_SUBDIR}/{}.toml", sanitize_scope(scope));
                aida_core::git_ops::add(store_root, &[&rel])?;
                let msg = format!(
                    "chore(coordination): claim lease {scope} (node {})",
                    ours.node_id
                );
                // `commit` returns Ok(false) for "nothing to commit" — treat as a
                // successful no-op (our claim already matched what was on disk).
                // Preserve main's error cleanup; a failed commit supplies no receipt.
                let committed = match aida_core::git_ops::commit(store_root, &msg) {
                    Ok(committed) => committed,
                    Err(error) => {
                        let _ = aida_core::git_ops::host_git_command(store_root)
                            .args(["reset", "-q", "HEAD", "--", &rel])
                            .status();
                        return Err(error);
                    }
                };
                let outcome = match outcome_note {
                    Some(reason) => AcquireOutcome::Reclaimed(reason),
                    None => AcquireOutcome::Acquired,
                };
                Ok((
                    outcome,
                    committed,
                    Some(mine),
                    adopted,
                    Some(aida_core::git_ops::head_sha(store_root)?),
                ))
            })?;
        let receipt = written.clone().filter(|_| !adopted);
        if !committed {
            return Ok((outcome, receipt));
        }
        let written = written.context("missing written claim identity")?;

        let committed_sha = commit_sha
            .as_deref()
            .context("missing claim commit identity")?;
        match aida_core::git_ops::push_exact(store_root, "origin", committed_sha, &branch) {
            Ok(true) => {
                return Ok((outcome, receipt));
            }
            Ok(false) => {
                // Push rejected — another clone wrote first. Discard our local
                // commit + working-tree change so the next iteration's
                // pull --rebase has a clean tree, then re-decide. Mirrors
                // register_node_full's BUG-1-069 reset.
                drop_unpushed_claim_commit(store_root, committed_sha, &written)?;
                let _ = attempt; // loop continues
                continue;
            }
            Err(e) => {
                // Push errored for a non-contention reason (network mid-flight).
                // Roll back our commit and proceed local-only rather than wedge:
                // a rollback that cannot complete is reported, never fatal.
                // trace:TASK-1955 | ai:claude (TASK-2069)
                if let Err(rollback) =
                    drop_unpushed_claim_commit(store_root, committed_sha, &written)
                {
                    eprintln!("warning: the unpushed lease claim commit was kept: {rollback:#}");
                }
                return Ok((
                    AcquireOutcome::Unavailable(format!("could not push lease claim ({e})")),
                    // The remote may have accepted before the connection failed.
                    // Retain the exact identity for later conditional cleanup.
                    receipt,
                ));
            }
        }
    }

    anyhow::bail!(
        "could not claim lease for `{scope}` after {MAX_CAS_RETRIES} attempts — \
         too much contention on the shared lease registry"
    )
}

// Drop this acquisition's own unpushed claim commit after a rejected or
// failed push. `push_exact` runs outside the store write lock, so another
// writer in this clone may have committed on top meanwhile: then our commit
// is rebased out from under the newer HEAD (TASK-1981's rule), keeping the
// peer's commit. The strict identity check still applies when HEAD is ours.
// trace:TASK-1955 | ai:claude (TASK-2069)
// The HEAD read and the rollback it selects run under one (re-entrant) store
// write lock, so no writer can commit between the decision and the reset.
// trace:TASK-1955 | ai:claude (TASK-2069, PR-2809 r1)
fn drop_unpushed_claim_commit(store_root: &Path, claim_sha: &str, expected: &Claim) -> Result<()> {
    aida_core::db::with_store_write_lock(store_root, || {
        if aida_core::git_ops::head_sha(store_root)? == claim_sha {
            return rollback_claim_commit(store_root, claim_sha, expected);
        }
        aida_core::git_ops::rollback_commit_if_head_matches(store_root, claim_sha)?;
        // `Ok(false)` also covers "nothing removed": the claim commit was
        // rewritten by a peer's rebase (no longer an ancestor), or the replay
        // conflicted and was aborted. Verify the claim really left the store.
        // trace:TASK-1955 | ai:claude (TASK-2101)
        anyhow::ensure!(
            read_claim(&claim_path(store_root, &expected.scope)).as_ref() != Some(expected),
            "the unpushed claim commit {} for {} was not dropped (a rebase rewrote it, or \
             removing it conflicted with a later commit); the claim is still in the local \
             store. Inspect `aida session leases --all` and the store history",
            &claim_sha[..claim_sha.len().min(10)],
            expected.scope
        );
        Ok(())
    })
}

// Drop only the exact clean commit this acquisition just made. A concurrent
// local writer or changed claim requires recovery, never a blanket HEAD~1 reset.
// trace:BUG-2188 | ai:codex
fn rollback_claim_commit(store_root: &Path, expected_head: &str, expected: &Claim) -> Result<()> {
    aida_core::db::with_store_write_lock(store_root, || {
        anyhow::ensure!(aida_core::git_ops::head_sha(store_root)? == expected_head
            && read_claim(&claim_path(store_root, &expected.scope)).as_ref() == Some(expected),
            "claim rollback identity changed; inspect `aida session leases --all` and the store history");
        let status = aida_core::git_ops::host_git_command(store_root)
            .args(["status", "--porcelain"])
            .output()?;
        anyhow::ensure!(
            status.status.success() && status.stdout.is_empty(),
            "claim rollback kept concurrent store changes; inspect the store history"
        );
        let output = aida_core::git_ops::host_git_command(store_root)
            .args(["reset", "--hard", "HEAD~1"])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "claim rollback failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    })
}

/// Compare-and-delete only this invocation's persisted identity. Legacy and
/// adopted claims never produce a rollback receipt.
// trace:BUG-2188 | ai:codex
pub(crate) fn release_claim_receipt(store_root: &Path, expected: &Claim) -> Result<bool> {
    let branch = aida_core::git_ops::current_branch(store_root)?;
    let committed_sha = aida_core::db::with_store_write_lock(store_root, || {
        if let Err(error) = refresh_store(store_root, &branch)? {
            anyhow::bail!("cannot refresh claim before rollback: {error}");
        }
        if delete_claim_receipt_locked(store_root, expected)? {
            Ok(Some(aida_core::git_ops::head_sha(store_root)?))
        } else {
            Ok(None)
        }
    })?;
    if let Some(sha) = committed_sha {
        if !aida_core::git_ops::push_exact(store_root, "origin", &sha, &branch)? {
            anyhow::bail!("claim rollback committed locally; push rejected; run `aida pull` and inspect the shared claim");
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

// trace:BUG-2188 | ai:codex
fn delete_claim_receipt_locked(store_root: &Path, expected: &Claim) -> Result<bool> {
    let path = claim_path(store_root, &expected.scope);
    let Some(current) = read_claim(&path) else {
        return Ok(false);
    };
    if expected.claim_id.is_none() || current.claim_id != expected.claim_id || current != *expected
    {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    let rel = format!("{LEASES_SUBDIR}/{}.toml", sanitize_scope(&expected.scope));
    aida_core::git_ops::add(store_root, &[&rel])?;
    aida_core::git_ops::commit(
        store_root,
        &format!("chore(coordination): rollback lease {}", expected.scope),
    )?;
    Ok(true)
}

/// Compose the refusal message naming the holder, mirroring `drain_lock`'s.
fn refusal_message(scope: &str, holder: &Claim, path: &Path) -> String {
    let age = DateTime::parse_from_rfc3339(&holder.started_at)
        .ok()
        .map(|t| {
            let secs = Utc::now()
                .signed_duration_since(t.with_timezone(&Utc))
                .num_seconds()
                .max(0);
            format!("{secs}s ago")
        })
        .unwrap_or_else(|| holder.started_at.clone());
    let agent = if holder.agent.is_empty() {
        String::new()
    } else {
        format!(", agent `{}`", holder.agent)
    };
    format!(
        "`{scope}` is already leased by another clone (host {}, {}{}, started {age}).\n  \
         That clone is actively working it — wait for it to finish, or pass --force to \
         override (you are then responsible for avoiding double-work).\n  \
         Shared claim: {}",
        if holder.host.is_empty() {
            "?".to_string()
        } else {
            holder.host.clone()
        },
        holder.clone_path,
        agent,
        path.display(),
    )
}

/// Release a cross-clone lease claim for `scope`: delete the file, commit, push.
///
/// Best-effort by design — staleness (pid / TTL) guarantees a crashed holder's
/// claim is always eventually reclaimable, so a failed release never deadlocks.
/// Only deletes a claim recorded by OUR clone (matched by `clone_path`), so we
/// never stomp a claim a successor reclaimed. trace:STORY-637 | ai:claude
pub(crate) fn release_claim(store_root: &Path, scope: &str, clone_path: &Path) {
    if !store_root.exists() || !aida_core::git_ops::has_remote(store_root, "origin") {
        return;
    }
    let path = claim_path(store_root, scope);
    let our_clone = canonical_clone_path(clone_path);
    // Only release if the on-disk claim is ours (or unreadable/already gone).
    if let Some(existing) = read_claim(&path) {
        if !existing.clone_path.is_empty() && existing.clone_path != our_clone {
            // A successor (a different clone) reclaimed it — leave it intact.
            return;
        }
    } else if !path.exists() {
        return;
    }

    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    // trace:TASK-1930 | ai:agy
    let committed_sha =
        aida_core::db::with_store_write_lock(store_root, || -> Result<Option<String>> {
            // Best-effort pull so the delete applies on top of the latest store.
            if refresh_store_or_stop(store_root, &branch) {
                return Ok(None);
            }
            // Re-check ownership after refresh_store so a successor's claim pulled
            // from origin is not deleted.
            if let Some(existing) = read_claim(&path) {
                if !existing.clone_path.is_empty() && existing.clone_path != our_clone {
                    return Ok(None);
                }
            } else if !path.exists() {
                return Ok(None);
            }
            if std::fs::remove_file(&path).is_err() && path.exists() {
                return Ok(None);
            }
            let rel = format!("{LEASES_SUBDIR}/{}.toml", sanitize_scope(scope));
            if aida_core::git_ops::add(store_root, &[&rel]).is_err() {
                return Ok(None);
            }
            let msg = format!("chore(coordination): release lease {scope}");
            let committed = aida_core::git_ops::commit(store_root, &msg).unwrap_or(false);
            Ok(committed
                .then(|| aida_core::git_ops::head_sha(store_root))
                .transpose()?)
        })
        .ok()
        .flatten();
    // Push only when there was actually something to commit; "nothing to
    // commit" (claim already gone) or a commit error is a quiet no-op.
    if let Some(sha) = committed_sha {
        let _ = aida_core::git_ops::push_exact(store_root, "origin", &sha, &branch);
    }
}

/// BUG-1764 clause 4: release a FOREIGN lease claim that [`claim_staleness`]
/// reports dead. Returns `Ok(true)` when a file was removed, `Ok(false)` when
/// the claim had already gone or no longer matches what we judged.
///
/// [`release_claim`] cannot do this: it deliberately refuses a claim whose
/// `clone_path` is not ours, so "a successor reclaimed it" never gets stomped.
/// That guard is right for the normal end path and is why there was previously
/// NO supported way to clear a stale foreign claim — `aida session end --spec`
/// cannot reach one (no local lease) and `aida session reap` requires spec
/// Done + branch merged + process exited.
///
/// The authorization here is the staleness verdict, so the verdict must still
/// hold at delete time. `expect_heartbeat` is the `heartbeat_at` the caller
/// judged: if the on-disk claim has a different one, the holder heartbeated
/// between the listing and the delete and is NOT stale after all — leave it.
/// Same for a changed `clone_path` (a successor took the scope). This closes
/// the read-then-delete window that the `clone_path` check closes for
/// [`release_claim`].
// trace:BUG-1764 | ai:claude
pub(crate) fn release_stale_foreign_claim(
    store_root: &Path,
    scope: &str,
    expect_clone_path: &str,
    expect_heartbeat: &str,
) -> Result<bool> {
    if !store_root.exists() || !aida_core::git_ops::has_remote(store_root, "origin") {
        return Ok(false);
    }
    let path = claim_path(store_root, scope);
    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    // trace:TASK-1930 | ai:agy
    let (removed, committed_sha) =
        aida_core::db::with_store_write_lock(store_root, || -> Result<(bool, Option<String>)> {
            // Pull first so we judge, and delete, against the latest store state — a
            // peer may already have released or refreshed it.
            let _ = refresh_store(store_root, &branch)?;
            let Some(existing) = read_claim(&path) else {
                // Already gone (or unreadable) — nothing to release, not an error.
                return Ok((false, None));
            };
            if existing.clone_path != expect_clone_path || existing.heartbeat_at != expect_heartbeat
            {
                // Refreshed or reclaimed since we listed it → our staleness verdict is
                // no longer about this record. Refuse rather than guess.
                return Ok((false, None));
            }
            std::fs::remove_file(&path)?;
            let rel = format!("{LEASES_SUBDIR}/{}.toml", sanitize_scope(scope));
            aida_core::git_ops::add(store_root, &[&rel])?;
            let msg = format!("chore(coordination): release stale lease {scope} (BUG-1764)");
            let committed = aida_core::git_ops::commit(store_root, &msg).unwrap_or(false);
            let sha = committed
                .then(|| aida_core::git_ops::head_sha(store_root))
                .transpose()?;
            Ok((true, sha))
        })?;
    if !removed {
        return Ok(false);
    }
    if let Some(sha) = committed_sha {
        let _ = aida_core::git_ops::push_exact(store_root, "origin", &sha, &branch);
    }
    Ok(true)
}

/// List the cross-clone lease claims currently recorded on the store. Returns
/// an empty vec when the registry tree doesn't exist (no claims yet).
/// Surfaced by `aida session leases`. trace:STORY-637 | ai:claude
pub(crate) fn list_claims(store_root: &Path) -> Vec<Claim> {
    let dir = leases_dir(store_root);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                if let Some(claim) = read_claim(&path) {
                    out.push(claim);
                }
            }
        }
    }
    out.sort_by(|a, b| a.scope.cmp(&b.scope));
    out
}

/// List the per-repo process-lock claims (drain + solo) currently recorded on
/// the store. Returns the live `Claim` records — empty when neither lock is
/// held. Surfaced alongside the lease claims by the `aida status` cross-clone
/// coordination view (STORY-640). trace:STORY-640 | ai:claude
pub(crate) fn list_lock_claims(store_root: &Path) -> Vec<Claim> {
    let mut out = Vec::new();
    for kind in [LockKind::Drain, LockKind::Solo] {
        if let Some(claim) = read_claim(&lock_claim_path(store_root, kind)) {
            out.push(claim);
        }
    }
    out
}

// =========================================================================
// Process-backed per-repo coordination locks (STORY-638, slice 2).
//
// The drain (`burndown run` / `queue work --auto-complete` / `queue integrate`)
// and the solo loop (`aida solo run`) are SINGLE-DRIVER per repo: they merge to
// the default branch, fan out worktrees, and share `target/`. Intra-clone this
// is enforced by `.aida/drain.lock` / `.aida/solo.lock`; cross-clone those files
// are invisible. These promote the claim to `coordination/<kind>.lock.toml` on
// the shared `aida-store` branch so a second CLONE is refused while one holds it.
//
// Unlike session leases (slice 1), drain/solo claims ARE process-backed: the
// holding process lives for the whole drain/solo, so SAME-HOST pid liveness is
// the authoritative reclaim signal (a dead pid → reclaim now), with the TTL /
// heartbeat as the cross-host / pid-recycle backstop. The TTL folds in the old
// `AIDA_DRAIN_LOCK_STALE_SECS` age-backstop semantics. The drain/solo loops
// refresh `heartbeat_at` on each tick so a long drain never looks stale.
// trace:STORY-638 | ai:claude
// =========================================================================

/// Subdir (under the store worktree root) holding the per-repo process locks.
const LOCKS_SUBDIR: &str = "coordination";

/// Which per-repo process lock — drives the file name + the human label.
/// trace:STORY-638 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockKind {
    /// `coordination/drain.lock.toml` — one active drain across all clones.
    Drain,
    /// `coordination/solo.lock.toml` — one active solo loop across all clones.
    Solo,
}

impl LockKind {
    /// File stem on disk (`drain.lock.toml` / `solo.lock.toml`).
    fn file_name(self) -> &'static str {
        match self {
            LockKind::Drain => "drain.lock.toml",
            LockKind::Solo => "solo.lock.toml",
        }
    }

    /// Human label for refusal / commit messages.
    fn label(self) -> &'static str {
        match self {
            LockKind::Drain => "drain",
            LockKind::Solo => "solo loop",
        }
    }
}

/// Path of the shared process-lock claim file under the store worktree.
pub(crate) fn lock_claim_path(store_root: &Path, kind: LockKind) -> PathBuf {
    store_root.join(LOCKS_SUBDIR).join(kind.file_name())
}

/// Build the process-backed claim we want to write for `kind`. `command` is the
/// launching command (e.g. `burndown run --status approved`), carried in the
/// `agent` field so a refusal can name what is running. `ttl_secs` folds in the
/// drain's age-backstop horizon. trace:STORY-638 | ai:claude
fn build_lock_claim(
    store_root: &Path,
    kind: LockKind,
    clone_path: &Path,
    command: &str,
    ttl_secs: u64,
) -> Claim {
    let now = Utc::now().to_rfc3339();
    Claim {
        claim_id: None,
        scope: kind.label().to_string(),
        node_id: node_id_for_clone(store_root, clone_path),
        clone_path: canonical_clone_path(clone_path),
        host: hostname(),
        pid: std::process::id(),
        pid_start_time: aida_core::liveness::process_start_identity(std::process::id()),
        agent: command.to_string(),
        started_at: now.clone(),
        heartbeat_at: now,
        ttl_secs,
        // Drain/solo ARE process-backed: the pid is the long-lived loop, so a
        // dead pid on our host is an exact reclaim signal. trace:STORY-638
        process_backed: true,
        review_verb: false,
        // trace:STORY-711 | ai:claude — not wired yet; see the field doc.
        authorized_by: None,
    }
}

/// Outcome of [`acquire_lock_claim`] — mirrors [`AcquireOutcome`] but typed for
/// the process-lock path so the call site can print the right note.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LockAcquireOutcome {
    /// Claimed cleanly (no prior claim, or our own clone re-claiming).
    Acquired,
    /// Reclaimed a stale/dead foreign claim. Carries the reason for a note.
    Reclaimed(String),
    /// Cross-clone coordination unavailable (no remote / store unreachable);
    /// the call site WARNs and proceeds local-only. Carries a one-line reason.
    Unavailable(String),
}

/// Acquire the shared per-repo process lock for `kind` on the store.
///
/// `store_root` is the `.aida-store` worktree; `clone_path` the project root;
/// `command` the launching command; `ttl_secs` the staleness horizon (the drain
/// folds in `AIDA_DRAIN_LOCK_STALE_SECS`). `force` short-circuits the decision
/// (the `AIDA_DRAIN_FORCE=1` / `--force` escape).
///
/// On a live foreign claim, returns `Err` whose message names the holder. On no
/// remote / unreachable store, returns `Ok(Unavailable(..))` — the caller WARNs
/// and proceeds local-only (a drain must never be brittle on the network).
/// trace:STORY-638 | ai:claude
pub(crate) fn acquire_lock_claim(
    store_root: &Path,
    kind: LockKind,
    clone_path: &Path,
    command: &str,
    ttl_secs: u64,
    force: bool,
) -> Result<LockAcquireOutcome> {
    if !store_root.exists() {
        return Ok(LockAcquireOutcome::Unavailable(
            "no .aida-store worktree attached".to_string(),
        ));
    }
    if !aida_core::git_ops::has_remote(store_root, "origin") {
        return Ok(LockAcquireOutcome::Unavailable(
            "store has no origin remote (solo clone)".to_string(),
        ));
    }
    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let ours = build_lock_claim(store_root, kind, clone_path, command, ttl_secs);
    let our_host = ours.host.clone();
    let path = lock_claim_path(store_root, kind);
    let rel = format!("{LOCKS_SUBDIR}/{}", kind.file_name());

    for _attempt in 0..MAX_CAS_RETRIES {
        // trace:TASK-1930 | ai:agy
        enum LockClaimStep {
            Unavailable(String),
            Refused(String),
            Ready {
                outcome_note: Option<String>,
                committed_sha: Option<String>,
            },
        }
        let step =
            aida_core::db::with_store_write_lock(store_root, || -> Result<LockClaimStep> {
                if let Err(e) = refresh_store(store_root, &branch)? {
                    return Ok(LockClaimStep::Unavailable(format!(
                        "store unreachable ({e})"
                    )));
                }

                let existing = read_claim(&path);
                let outcome_note = if force {
                    existing
                        .as_ref()
                        .filter(|h| h.clone_path.is_empty() || h.clone_path != ours.clone_path)
                        .map(|h| {
                            format!(
                                "--force overriding {}'s {} ({})",
                                h.host,
                                kind.label(),
                                h.clone_path
                            )
                        })
                } else {
                    match decide_claim(
                        existing.as_ref(),
                        &ours,
                        Utc::now(),
                        &our_host,
                        crate::process_probe::process_identity_is_alive,
                        // A lock claim is process-backed (the pid probe above is the
                        // exact signal); there is no session to probe. A session
                        // claim can never legitimately sit at a LOCK path, so grant
                        // it no liveness either. trace:BUG-1765 | ai:claude
                        |_: &Claim| false,
                    ) {
                        ClaimDecision::Refuse { holder } => {
                            return Ok(LockClaimStep::Refused(lock_refusal_message(
                                kind, &holder, &path,
                            )));
                        }
                        ClaimDecision::Acquire => None,
                        ClaimDecision::Reclaim { stale_reason } => Some(stale_reason),
                    }
                };

                std::fs::create_dir_all(store_root.join(LOCKS_SUBDIR)).ok();
                let toml = toml::to_string_pretty(&ours).map_err(|e| {
                    anyhow::anyhow!("could not serialize {} claim: {e}", kind.label())
                })?;
                std::fs::write(&path, toml).map_err(|e| {
                    anyhow::anyhow!(
                        "could not write {} claim {}: {e}",
                        kind.label(),
                        path.display()
                    )
                })?;

                aida_core::git_ops::add(store_root, &[&rel])?;
                let msg = format!(
                    "chore(coordination): claim {} lock (node {})",
                    kind.label(),
                    ours.node_id
                );
                // trace:TASK-1930 | ai:agy
                let committed = match aida_core::git_ops::commit(store_root, &msg) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = aida_core::git_ops::host_git_command(store_root)
                            .args(["reset", "-q", "HEAD", "--", &rel])
                            .status();
                        return Err(e);
                    }
                };
                let committed_sha = if committed {
                    Some(aida_core::git_ops::head_sha(store_root)?)
                } else {
                    None
                };
                Ok(LockClaimStep::Ready {
                    outcome_note,
                    committed_sha,
                })
            })?;

        let (outcome_note, committed_sha) = match step {
            LockClaimStep::Unavailable(msg) => return Ok(LockAcquireOutcome::Unavailable(msg)),
            LockClaimStep::Refused(msg) => anyhow::bail!("{msg}"),
            LockClaimStep::Ready {
                outcome_note,
                committed_sha: None,
            } => {
                return Ok(match outcome_note {
                    Some(reason) => LockAcquireOutcome::Reclaimed(reason),
                    None => LockAcquireOutcome::Acquired,
                });
            }
            LockClaimStep::Ready {
                outcome_note,
                committed_sha: Some(sha),
            } => (outcome_note, sha),
        };

        match aida_core::git_ops::push_exact(store_root, "origin", &committed_sha, &branch) {
            Ok(true) => {
                return Ok(match outcome_note {
                    Some(reason) => LockAcquireOutcome::Reclaimed(reason),
                    None => LockAcquireOutcome::Acquired,
                });
            }
            Ok(false) => {
                let _ =
                    aida_core::git_ops::rollback_commit_if_head_matches(store_root, &committed_sha);
                continue;
            }
            Err(e) => {
                let _ =
                    aida_core::git_ops::rollback_commit_if_head_matches(store_root, &committed_sha);
                return Ok(LockAcquireOutcome::Unavailable(format!(
                    "could not push {} claim ({e})",
                    kind.label()
                )));
            }
        }
    }

    anyhow::bail!(
        "could not claim the {} lock after {MAX_CAS_RETRIES} attempts — \
         too much contention on the shared coordination registry",
        kind.label()
    )
}

/// STORY-1218 (A10): read-only view of the shared per-repo process lock for
/// a launcher that must not start a drain while ANOTHER clone holds one.
/// Reads the local `.aida-store` checkout only (no pull, no write) and applies
/// the same [`decide_claim`] rules the acquire path uses: our own clone's
/// claim is not foreign, a dead same-host pid or an aged-out heartbeat is not
/// live. Returns the live foreign holder, if any.
// trace:STORY-1218 | ai:claude
pub(crate) fn live_foreign_lock_claim(
    store_root: &Path,
    kind: LockKind,
    clone_path: &Path,
    now: DateTime<Utc>,
    is_alive: impl Fn(u32, Option<&str>) -> bool,
) -> Option<Claim> {
    let holder = read_claim(&lock_claim_path(store_root, kind))?;
    let ours = Claim {
        claim_id: None,
        scope: kind.label().to_string(),
        node_id: String::new(),
        clone_path: canonical_clone_path(clone_path),
        host: hostname(),
        pid: std::process::id(),
        pid_start_time: None,
        agent: String::new(),
        started_at: now.to_rfc3339(),
        heartbeat_at: now.to_rfc3339(),
        ttl_secs: 0,
        process_backed: true,
        review_verb: false,
        authorized_by: None,
    };
    let our_host = ours.host.clone();
    // Lock claims are process-backed — no session to probe (BUG-1765).
    match decide_claim(Some(&holder), &ours, now, &our_host, is_alive, |_| false) {
        ClaimDecision::Refuse { holder } => Some(*holder),
        ClaimDecision::Acquire | ClaimDecision::Reclaim { .. } => None,
    }
}

/// Compose the refusal message naming the holder of a process lock.
fn lock_refusal_message(kind: LockKind, holder: &Claim, path: &Path) -> String {
    let age = DateTime::parse_from_rfc3339(&holder.started_at)
        .ok()
        .map(|t| {
            let secs = Utc::now()
                .signed_duration_since(t.with_timezone(&Utc))
                .num_seconds()
                .max(0);
            format!("{secs}s ago")
        })
        .unwrap_or_else(|| holder.started_at.clone());
    let cmd = if holder.agent.is_empty() {
        String::new()
    } else {
        format!(", cmd `{}`", holder.agent)
    };
    format!(
        "a {} is already running in another clone (host {}, {} (pid {}){}, started {age}).\n  \
         Wait for it to finish, or — if you're certain it's dead — pass --force \
         / set AIDA_DRAIN_FORCE=1 to override (you are then responsible for \
         avoiding double-drive).\n  Shared claim: {}",
        kind.label(),
        if holder.host.is_empty() {
            "?".to_string()
        } else {
            holder.host.clone()
        },
        holder.clone_path,
        holder.pid,
        cmd,
        path.display(),
    )
}

/// Refresh `heartbeat_at` on OUR process-lock claim so a long-running drain/solo
/// never ages past its TTL. Best-effort and CHEAP-ish (one pull + commit + push
/// per tick): only rewrites when the on-disk claim is still ours. A successor
/// that reclaimed it (different clone_path) is left intact — we no longer hold
/// the lock. trace:STORY-638 | ai:claude
pub(crate) fn heartbeat_lock_claim(store_root: &Path, kind: LockKind, clone_path: &Path) {
    if !store_root.exists() || !aida_core::git_ops::has_remote(store_root, "origin") {
        return;
    }
    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    // trace:TASK-1930 | ai:agy
    let committed_sha =
        aida_core::db::with_store_write_lock(store_root, || -> Result<Option<String>> {
            if refresh_store_or_stop(store_root, &branch) {
                return Ok(None);
            }
            let path = lock_claim_path(store_root, kind);
            let our_clone = canonical_clone_path(clone_path);
            let Some(mut claim) = read_claim(&path) else {
                return Ok(None);
            };
            if !claim.clone_path.is_empty() && claim.clone_path != our_clone {
                // A successor reclaimed it — we don't own it anymore; don't stomp.
                return Ok(None);
            }
            claim.heartbeat_at = Utc::now().to_rfc3339();
            let Ok(toml) = toml::to_string_pretty(&claim) else {
                return Ok(None);
            };
            if std::fs::write(&path, toml).is_err() {
                return Ok(None);
            }
            let rel = format!("{LOCKS_SUBDIR}/{}", kind.file_name());
            if aida_core::git_ops::add(store_root, &[&rel]).is_err() {
                return Ok(None);
            }
            let msg = format!("chore(coordination): heartbeat {} lock", kind.label());
            let committed = aida_core::git_ops::commit(store_root, &msg).unwrap_or(false);
            Ok(committed
                .then(|| aida_core::git_ops::head_sha(store_root))
                .transpose()?)
        })
        .ok()
        .flatten();
    if let Some(sha) = committed_sha {
        let _ = aida_core::git_ops::push_exact(store_root, "origin", &sha, &branch);
    }
}

/// Release OUR per-repo process lock claim: delete the file, commit, push.
/// Best-effort (staleness covers a crash). Only deletes a claim recorded by OUR
/// clone (matched by `clone_path`). trace:STORY-638 | ai:claude
pub(crate) fn release_lock_claim(store_root: &Path, kind: LockKind, clone_path: &Path) {
    if !store_root.exists() || !aida_core::git_ops::has_remote(store_root, "origin") {
        return;
    }
    let path = lock_claim_path(store_root, kind);
    let our_clone = canonical_clone_path(clone_path);
    if let Some(existing) = read_claim(&path) {
        if !existing.clone_path.is_empty() && existing.clone_path != our_clone {
            return;
        }
    } else if !path.exists() {
        return;
    }
    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    // trace:TASK-1930 | ai:agy
    let committed_sha =
        aida_core::db::with_store_write_lock(store_root, || -> Result<Option<String>> {
            if refresh_store_or_stop(store_root, &branch) {
                return Ok(None);
            }
            if let Some(existing) = read_claim(&path) {
                if !existing.clone_path.is_empty() && existing.clone_path != our_clone {
                    return Ok(None);
                }
            } else if !path.exists() {
                return Ok(None);
            }
            if std::fs::remove_file(&path).is_err() && path.exists() {
                return Ok(None);
            }
            let rel = format!("{LOCKS_SUBDIR}/{}", kind.file_name());
            if aida_core::git_ops::add(store_root, &[&rel]).is_err() {
                return Ok(None);
            }
            let msg = format!("chore(coordination): release {} lock", kind.label());
            let committed = aida_core::git_ops::commit(store_root, &msg).unwrap_or(false);
            Ok(committed
                .then(|| aida_core::git_ops::head_sha(store_root))
                .transpose()?)
        })
        .ok()
        .flatten();
    if let Some(sha) = committed_sha {
        let _ = aida_core::git_ops::push_exact(store_root, "origin", &sha, &branch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PROCESS-BACKED foreign claim (slice-2 drain/solo shape): its pid
    /// faithfully tracks liveness, so the same-host pid probe is exact.
    fn claim(node_id: &str, host: &str, pid: u32, heartbeat_at: &str) -> Claim {
        Claim {
            claim_id: None,
            scope: "FR-1".to_string(),
            node_id: node_id.to_string(),
            clone_path: "/home/joe/ai/aida-b".to_string(),
            host: host.to_string(),
            pid,
            pid_start_time: Some("2026-06-16T11:59:00+00:00".to_string()),
            agent: "codex-implementer-1".to_string(),
            started_at: heartbeat_at.to_string(),
            heartbeat_at: heartbeat_at.to_string(),
            ttl_secs: 1800,
            process_backed: true,
            review_verb: false,
            authorized_by: None,
        }
    }

    /// A SESSION-LEASE foreign claim (slice-1 shape): NOT process-backed, so a
    /// dead pid must NOT trigger reclaim — only the TTL backstop governs it.
    fn session_claim(node_id: &str, host: &str, pid: u32, heartbeat_at: &str) -> Claim {
        Claim {
            process_backed: false,
            ..claim(node_id, host, pid, heartbeat_at)
        }
    }

    fn ours(node_id: &str, host: &str) -> Claim {
        Claim {
            claim_id: None,
            scope: "FR-1".to_string(),
            node_id: node_id.to_string(),
            clone_path: "/home/joe/ai/aida-a".to_string(),
            host: host.to_string(),
            pid: 1234,
            pid_start_time: Some("2026-06-16T11:59:00+00:00".to_string()),
            agent: "claude".to_string(),
            started_at: "2026-06-16T12:00:00Z".to_string(),
            heartbeat_at: "2026-06-16T12:00:00Z".to_string(),
            ttl_secs: 1800,
            process_backed: false,
            review_verb: false,
            authorized_by: None,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    // ── decide_claim table (mirrors drain_lock's 15-test pattern) ──

    #[test]
    fn no_claim_acquires() {
        let d = decide_claim(
            None,
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        assert_eq!(d, ClaimDecision::Acquire);
    }

    #[test]
    fn own_clone_reacquires_reentrant() {
        // existing claim has the SAME clone_path as ours → re-entrant acquire,
        // even though its pid is alive and heartbeat is fresh. (Two clones
        // sharing a store inherit the same node_id, so clone_path — not node_id
        // — is the per-clone discriminator.)
        let mut existing = claim("2", "imac", 4242, "2026-06-16T11:59:30Z");
        existing.clone_path = "/home/joe/ai/aida-a".to_string(); // same as ours()
        let d = decide_claim(
            Some(&existing),
            &ours("2", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        assert_eq!(d, ClaimDecision::Acquire);
    }

    #[test]
    fn same_node_id_but_different_clone_path_is_foreign() {
        // The MU-504 root cause: two clones sharing one store have the SAME
        // node_id (it rides the aida-store branch) but DISTINCT clone paths.
        // A fresh foreign session lease must still REFUSE despite matching
        // node ids. trace:STORY-637
        let existing = claim("1", "imac", 4242, "2026-06-16T11:59:30Z"); // clone aida-b
        let mine = ours("1", "imac"); // clone aida-a, same node_id "1"
        let d = decide_claim(
            Some(&existing),
            &mine,
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn dead_pid_same_host_reclaims() {
        // PROCESS-BACKED foreign clone, same host, pid dead → reclaim
        // immediately (fast path), even though heartbeat is fresh.
        let existing = claim("2", "imac", 4242, "2026-06-16T11:59:50Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn session_lease_dead_pid_same_host_does_not_reclaim() {
        // THE MU-504 BUG: a session lease is NOT process-backed — its pid is
        // the ephemeral `aida session start` shell that has already exited. A
        // dead pid must therefore NOT reclaim a fresh-heartbeat session lease;
        // it stays REFUSED until its TTL or an explicit release. trace:STORY-637
        let existing = session_claim("2", "imac", 4242, "2026-06-16T11:59:50Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn session_lease_stale_ttl_still_reclaims() {
        // A session lease is still reclaimable once its heartbeat ages past the
        // TTL — the crash backstop that prevents a permanent deadlock.
        let existing = session_claim("2", "imac", 4242, "2026-06-16T11:00:00Z"); // 3600s
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn live_heartbeat_refuses() {
        // foreign clone, pid alive (or cross-host), heartbeat within TTL → refuse.
        let existing = claim("2", "imac", 4242, "2026-06-16T11:59:00Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        match d {
            ClaimDecision::Refuse { holder } => assert_eq!(holder.node_id, "2"),
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    #[test]
    fn stale_ttl_reclaims() {
        // foreign clone, heartbeat older than ttl_secs (1800) → reclaim,
        // even with pid reported alive (covers cross-host where pid is meaningless).
        let existing = claim("2", "imac", 4242, "2026-06-16T11:00:00Z"); // 3600s old
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn cross_host_live_pid_does_not_take_fast_path() {
        // foreign clone on a DIFFERENT host: pid liveness is meaningless (the
        // pid is on another machine), so a dead-pid probe must NOT reclaim a
        // claim whose heartbeat is still fresh → refuse.
        let existing = claim("2", "otherhost", 4242, "2026-06-16T11:59:00Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn cross_host_stale_heartbeat_reclaims() {
        // foreign clone on another host whose heartbeat aged out → reclaim
        // via the universal TTL backstop (no pid probe needed).
        let existing = claim("2", "otherhost", 4242, "2026-06-16T11:00:00Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn unparseable_heartbeat_falls_back_to_pid_liveness_same_host() {
        // age unknown → same-host pid decides. Alive → refuse; dead → reclaim.
        let existing = claim("2", "imac", 4242, "not-a-timestamp");
        let alive = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        assert!(
            matches!(alive, ClaimDecision::Refuse { .. }),
            "got {alive:?}"
        );
        let dead = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(
            matches!(dead, ClaimDecision::Reclaim { .. }),
            "got {dead:?}"
        );
    }

    #[test]
    fn heartbeat_age_clamps_future_to_zero() {
        let c = claim("2", "imac", 1, "2026-06-16T13:00:00Z"); // 1h future
        assert_eq!(c.heartbeat_age_secs(now()), Some(0));
    }

    // ── sanitize_scope ──

    #[test]
    fn sanitize_scope_is_filesystem_safe() {
        let s = sanitize_scope("FR-1");
        assert!(s.starts_with("fr-1-"), "got {s}");
        assert!(s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')));
    }

    #[test]
    fn sanitize_scope_distinct_for_collidable_inputs() {
        // `FR/1` and `FR_1` both sanitize to the same stem but the hash suffix
        // keeps them distinct (no cross-clone lease aliasing).
        let a = sanitize_scope("FR/1");
        let b = sanitize_scope("FR_1");
        assert!(a.starts_with("fr_1-"));
        assert!(b.starts_with("fr_1-"));
        assert_ne!(a, b);
    }

    #[test]
    fn sanitize_scope_is_deterministic() {
        assert_eq!(sanitize_scope("TASK-688"), sanitize_scope("TASK-688"));
    }

    #[test]
    fn sanitize_scope_empty_falls_back() {
        let s = sanitize_scope("///");
        assert!(s.starts_with("___-"), "got {s}");
    }

    // ── acquire/release best-effort no-store paths (real fs) ──

    #[test]
    fn acquire_is_unavailable_without_a_store() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join(".aida-store");
        let out = acquire_claim(&missing, "FR-1", dir.path(), "claude", false, false).unwrap();
        assert!(matches!(out, AcquireOutcome::Unavailable(_)), "got {out:?}");
    }

    #[test]
    fn acquire_is_unavailable_without_origin_remote() {
        // A real git repo with no `origin` remote → solo clone → unavailable
        // (proceeds local-only, never blocks).
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&store)
            .output()
            .unwrap();
        let out = acquire_claim(&store, "FR-1", dir.path(), "claude", false, false).unwrap();
        assert!(matches!(out, AcquireOutcome::Unavailable(_)), "got {out:?}");
    }

    #[test]
    fn list_claims_empty_when_no_registry() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_claims(dir.path()).is_empty());
    }

    #[test]
    fn claim_round_trips_through_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = claim_path(dir.path(), "FR-1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = claim("2", "imac", 99, "2026-06-16T12:00:00Z");
        std::fs::write(&path, toml::to_string_pretty(&c).unwrap()).unwrap();
        let back = read_claim(&path).expect("a parseable claim");
        assert_eq!(back, c);
        let listed = list_claims(dir.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].node_id, "2");
    }

    // trace:STORY-711 | ai:claude
    #[test]
    fn claim_authorized_by_round_trips_through_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = claim_path(dir.path(), "FR-1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut c = claim("2", "imac", 99, "2026-06-16T12:00:00Z");
        c.authorized_by = Some("advisor-abc".to_string());
        std::fs::write(&path, toml::to_string_pretty(&c).unwrap()).unwrap();
        let back = read_claim(&path).expect("a parseable claim");
        assert_eq!(back, c);
        assert_eq!(back.authorized_by.as_deref(), Some("advisor-abc"));
    }

    // trace:STORY-711 | ai:claude
    #[test]
    fn claim_authorized_by_defaults_to_none_for_old_toml_without_the_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = claim_path(dir.path(), "FR-1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Old claim shape, no `authorized_by` key at all.
        std::fs::write(
            &path,
            r#"
scope = "FR-1"
node_id = "2"
clone_path = "/home/joe/ai/aida-b"
host = "imac"
pid = 99
agent = "codex-implementer-1"
started_at = "2026-06-16T12:00:00Z"
heartbeat_at = "2026-06-16T12:00:00Z"
ttl_secs = 1800
"#,
        )
        .unwrap();
        let back = read_claim(&path).expect("a parseable claim");
        assert_eq!(back.authorized_by, None);
    }

    // ── process-lock (drain/solo) decision table (STORY-638) ──
    //
    // Drain/solo claims ARE process-backed, so `decide_claim` is exercised here
    // exactly as it governs the shared drain/solo lock: a fresh foreign claim is
    // built via `build_lock_claim` (process_backed = true) and decided.

    /// A foreign process-lock claim on `host`/`pid`, heartbeat now-ish.
    fn lock_claim(host: &str, pid: u32, heartbeat_at: &str) -> Claim {
        Claim {
            claim_id: None,
            scope: "drain".to_string(),
            node_id: "2".to_string(),
            clone_path: "/home/joe/ai/aida-b".to_string(),
            host: host.to_string(),
            pid,
            pid_start_time: Some("2026-06-16T11:59:00+00:00".to_string()),
            agent: "burndown run".to_string(),
            started_at: heartbeat_at.to_string(),
            heartbeat_at: heartbeat_at.to_string(),
            ttl_secs: 1800,
            process_backed: true,
            review_verb: false,
            authorized_by: None,
        }
    }

    #[test]
    fn drain_dead_pid_same_host_reclaims_immediately() {
        // The slice-2 fast path: a drain holder whose pid is dead on OUR host
        // → reclaim now (the process IS the drain), even with a fresh heartbeat.
        let existing = lock_claim("imac", 4242, "2026-06-16T11:59:55Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn drain_live_pid_same_host_refuses() {
        // A live drain in another clone, same host → refuse.
        let existing = lock_claim("imac", 4242, "2026-06-16T11:59:30Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn drain_same_pid_different_start_time_reclaims() {
        let existing = lock_claim("imac", 4242, "2026-06-16T11:59:30Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |pid, start| pid == 4242 && start == Some("different-start"),
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn drain_stale_ttl_reclaims_even_when_pid_alive() {
        // Heartbeat aged past the TTL (pid-recycle / cross-host backstop) →
        // reclaim even with the pid reported alive.
        let existing = lock_claim("imac", 4242, "2026-06-16T11:00:00Z"); // 3600s
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn drain_cross_host_live_within_ttl_refuses() {
        // A drain holder on a DIFFERENT host within TTL: pid probe is meaningless
        // → refuse (no fast-path reclaim of a remote live drain).
        let existing = lock_claim("otherhost", 4242, "2026-06-16T11:59:00Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn build_lock_claim_is_process_backed_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let c = build_lock_claim(
            dir.path(),
            LockKind::Drain,
            dir.path(),
            "burndown run --status approved",
            1800,
        );
        assert!(c.process_backed, "drain/solo claims must be process-backed");
        assert_eq!(c.scope, "drain");
        assert_eq!(c.agent, "burndown run --status approved");
        assert_eq!(c.pid, std::process::id());
    }

    #[test]
    fn lock_claim_path_names_per_kind() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            lock_claim_path(dir.path(), LockKind::Drain).ends_with("coordination/drain.lock.toml")
        );
        assert!(
            lock_claim_path(dir.path(), LockKind::Solo).ends_with("coordination/solo.lock.toml")
        );
    }

    #[test]
    fn acquire_lock_is_unavailable_without_a_store() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join(".aida-store");
        let out = acquire_lock_claim(
            &missing,
            LockKind::Drain,
            dir.path(),
            "burndown run",
            1800,
            false,
        )
        .unwrap();
        assert!(
            matches!(out, LockAcquireOutcome::Unavailable(_)),
            "got {out:?}"
        );
    }

    #[test]
    fn acquire_lock_is_unavailable_without_origin_remote() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&store)
            .output()
            .unwrap();
        let out = acquire_lock_claim(&store, LockKind::Solo, dir.path(), "solo run", 1800, false)
            .unwrap();
        assert!(
            matches!(out, LockAcquireOutcome::Unavailable(_)),
            "got {out:?}"
        );
    }

    // ── BUG-1764: the reporting-staleness predicate ────────────────────────
    //
    // Before BUG-1764 NO reader of `list_claims` applied an expiry predicate:
    // `print_cross_clone_leases` filtered only on `clone_path`, and
    // `print_status_coordination_section` (`aida status --full`, `aida doctor`)
    // and `team::build_team_view` took the whole list. Measured on the
    // reporter's host: 211 claims reported active, oldest 8_078_324s against
    // its own 1800s TTL.
    //
    // Liveness is injected, so the PID-collision case the spec specifies —
    // `pid = 2` (`kthreadd`) with a `pid_start_time` later than boot — is
    // reproducible without root and without a real pid 2.
    // trace:BUG-1764 | ai:claude

    /// A NON-process-backed foreign claim: the session-lease shape, whose
    /// recorded pid is the ephemeral `aida session start` shell.
    fn lease_claim(host: &str, pid: u32, heartbeat_at: &str) -> Claim {
        Claim {
            claim_id: None,
            scope: "BUG-1817".to_string(),
            node_id: "4".to_string(),
            clone_path: "/home/joe.mooney/ai/aida".to_string(),
            host: host.to_string(),
            pid,
            pid_start_time: Some("2026-10-01T22:05:10+00:00".to_string()),
            agent: "implementer".to_string(),
            started_at: heartbeat_at.to_string(),
            heartbeat_at: heartbeat_at.to_string(),
            ttl_secs: 1800,
            process_backed: false,
            review_verb: false,
            authorized_by: None,
        }
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// Clause 1 — an aged-out claim is stale; a fresh one is not. This is the
    /// clause that catches every lease in the filed measurement, and the
    /// reproduction that survived the operator's hand-clear: BUG-1817 on host
    /// `spock`, age 26055s against its own `ttl_secs = 1800`.
    #[test]
    fn bug_1764_ttl_expiry_is_applied_to_a_foreign_lease() {
        let c = lease_claim("spock", 1134977, "2026-10-01T22:05:14+00:00");
        // 26055s later — 14x past the 1800s TTL the claim itself carries.
        let reason = claim_staleness(
            &c,
            at("2026-10-02T05:19:29+00:00"),
            "imac",
            |_, _| true,
            |_| false,
        )
        .expect("a lease 26055s past its 1800s TTL must read stale");
        assert!(
            reason.contains("1800s TTL"),
            "reason should name the TTL it breached, got {reason:?}"
        );
        // Fresh: 60s old, well inside the TTL → presumed live.
        assert_eq!(
            claim_staleness(
                &c,
                at("2026-10-01T22:06:14+00:00"),
                "imac",
                |_, _| true,
                |_| false
            ),
            None,
            "a lease inside its TTL must stay visible"
        );
    }

    /// Clause 2 — a LIVE pid must never rescue an aged-out claim. This is the
    /// structural fix for the filed defect: `pr-2256` recorded `pid = 2`
    /// (`kthreadd`) and `bug-1692` recorded `pid = 202` (a live kworker), and
    /// any predicate that asked only "does this pid exist" saw them as live
    /// forever. `is_alive` says yes to everything here and the claim is still
    /// stale, because the TTL is evaluated first and independently.
    #[test]
    fn bug_1764_a_live_pid_cannot_rescue_an_aged_out_claim() {
        for pid in [2u32, 202] {
            let c = lease_claim("imac", pid, "2026-09-28T12:26:18+00:00");
            assert!(
                claim_staleness(
                    &c,
                    at("2026-10-01T22:00:00+00:00"),
                    "imac",
                    |_, _| true,
                    |_| false
                )
                .is_some(),
                "pid {pid} colliding with a live kernel thread must not keep an \
                 aged-out lease alive"
            );
        }
    }

    /// Clause 2 — the strict identity probe. A recorded start time that
    /// disagrees with the kernel's means the PID was recycled: the filed
    /// leases recorded Sep 28 while pid 2 and pid 202 actually started Sep 4
    /// at boot.
    #[test]
    fn bug_1764_strict_probe_rejects_a_recycled_pid() {
        let boot = |_| Some("2026-09-04T00:00:00+00:00".to_string());
        let exact = |_| Some("linux-starttime:42".to_string());
        // Recorded Sep 28, kernel says Sep 4 → mismatch → dead.
        assert!(
            !strictly_alive_with(2, Some("2026-09-28T12:26:18+00:00"), |_| true, boot, exact),
            "a recorded start time that differs from the kernel's must read dead"
        );
        // Recorded == actual → alive.
        assert!(
            strictly_alive_with(2, Some("2026-09-04T00:00:00+00:00"), |_| true, boot, exact),
            "a matching start time must read alive"
        );
        // Exact tick record matches exact probe without relying on legacy btime.
        assert!(
            strictly_alive_with(2, Some("linux-starttime:42"), |_| true, |_| None, exact),
            "an exact tick identity must read alive against the exact probe"
        );
        // Pid not alive at all → dead, regardless of the recorded identity.
        assert!(
            !strictly_alive_with(2, Some("2026-09-04T00:00:00+00:00"), |_| false, boot, exact),
            "a dead pid must read dead"
        );
    }

    /// Clause 2 — a claim with NO `pid_start_time` is UNVERIFIABLE, not live.
    /// Three of the eight filed leases (`bug-1692`, `task-1500`, `task-1531`)
    /// carry no `pid_start_time` at all, and `bug-1692` is the case where both
    /// defects land together: no recorded identity AND a live colliding kernel
    /// thread. This is deliberately the OPPOSITE of
    /// `aida_core::liveness::process_identity_is_alive`, which returns `true`
    /// on a missing record (the TASK-1284 reclaim degrade) — see
    /// `strictly_alive`'s doc comment for why reporting must not inherit it.
    #[test]
    fn bug_1764_absent_pid_start_time_is_unverifiable_not_live() {
        assert!(
            !strictly_alive_with(
                202,
                None,
                |_| true,
                |_| Some("2026-09-04T00:00:00+00:00".to_string()),
                |_| Some("linux-starttime:42".to_string()),
            ),
            "no recorded pid_start_time must read NOT live, even for a live pid"
        );
        // And a kernel that cannot report a start time is equally unverifiable.
        assert!(
            !strictly_alive_with(
                202,
                Some("2026-09-28T00:00:00+00:00"),
                |_| true,
                |_| None,
                |_| None,
            ),
            "an unreadable kernel start time must read NOT live"
        );
        // Guard the contrast with the reclaim predicate we deliberately did
        // NOT change: it still trusts a missing record, and must keep doing so.
        assert!(
            aida_core::liveness::process_identity_is_alive(std::process::id(), None),
            "the drain/solo reclaim degrade must be left intact"
        );
    }

    /// Clause 3 — a claim whose `host` is ours is evaluated against the LOCAL
    /// process table even though its `clone_path` differs. All eight leases in
    /// the filed measurement were `host = "imac"` queried from `imac`, yet were
    /// filed as unreachable "cross-clone" purely because the path differed.
    #[test]
    fn bug_1764_same_host_different_clone_is_evaluated_locally() {
        let mut c = lease_claim("imac", 2, "2026-10-01T22:00:00+00:00");
        c.process_backed = true; // a drain/solo claim: pid liveness is exact
        assert_ne!(
            c.clone_path, "/home/joe/ai/aida",
            "fixture is a foreign clone"
        );
        // Inside its TTL, so clause 1 cannot be what decides this. Same host +
        // a dead pid → stale via the local process table.
        let reason = claim_staleness(
            &c,
            at("2026-10-01T22:10:00+00:00"),
            "imac",
            |_, _| false,
            |_| false,
        )
        .expect("same-host claim with a dead pid must read stale");
        assert!(
            reason.contains("not running"),
            "reason should name the pid probe, got {reason:?}"
        );
        // The SAME claim on a different host is not locally probeable, so the
        // pid verdict must not be applied to it.
        let remote = lease_claim("spock", 2, "2026-10-01T22:00:00+00:00");
        assert_eq!(
            claim_staleness(
                &remote,
                at("2026-10-01T22:10:00+00:00"),
                "imac",
                |_, _| false,
                |_| false
            ),
            None,
            "a foreign-HOST claim inside its TTL must not be judged by our pid table"
        );
    }

    /// A non-process-backed session lease must NOT be called stale merely
    /// because its ephemeral `session start` shell has exited — that pid dies
    /// immediately by design. Only the TTL governs it. Guards against the
    /// obvious over-correction while fixing this bug.
    #[test]
    fn bug_1764_ephemeral_session_pid_death_is_not_staleness() {
        let c = lease_claim("imac", 1134977, "2026-10-01T22:00:00+00:00");
        assert!(!c.process_backed);
        assert_eq!(
            claim_staleness(
                &c,
                at("2026-10-01T22:10:00+00:00"),
                "imac",
                |_, _| false,
                |_| false
            ),
            None,
            "a session lease must survive the death of its launching shell"
        );
    }

    /// `partition_claims` is the single entry point every reporting surface
    /// uses, so a new reader cannot forget the predicate. Asserts the split
    /// keeps the live claim and reports the reason for the dead one.
    #[test]
    fn bug_1764_partition_splits_live_from_stale() {
        let fresh = lease_claim("spock", 1, "2026-10-01T22:05:00+00:00");
        let mut aged = lease_claim("spock", 2, "2026-09-01T00:00:00+00:00");
        aged.scope = "ADR-7".to_string();
        let (live, stale) = partition_claims(
            vec![fresh.clone(), aged.clone()],
            at("2026-10-01T22:06:00+00:00"),
            "imac",
        );
        assert_eq!(live.len(), 1, "the fresh claim must survive");
        assert_eq!(live[0].scope, "BUG-1817");
        assert_eq!(stale.len(), 1, "the aged claim must be reported stale");
        assert_eq!(stale[0].0.scope, "ADR-7");
        assert!(stale[0].1.contains("TTL"), "got {:?}", stale[0].1);
    }

    // ── BUG-1765: a verified-live session outranks TTL age ────────────────
    //
    // A session-lease claim's heartbeat is never refreshed (its writer exits
    // immediately), so before BUG-1765 every session claim read reclaimable
    // 1800s after `session start` — while its session was still working —
    // and a peer clone was told to RECLAIM the scope, the exact duplicate-
    // work race the registry exists to prevent. The injected `session_live`
    // probe can only ADD liveness: `true` is a positive same-host
    // verification, `false` is "no signal" and leaves every older clause
    // deciding unchanged.
    // trace:BUG-1765 | ai:claude

    /// AC2: a session-lease claim whose holding session is verified live must
    /// not yield `Reclaim` purely because `ttl_secs` elapsed.
    #[test]
    fn bug_1765_live_session_claim_past_ttl_is_refused_not_reclaimed() {
        // 3600s old against a 1800s TTL — aged out, but verified live.
        let existing = session_claim("2", "imac", 4242, "2026-06-16T11:00:00Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| true,
        );
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    /// AC3 (the adjacent half): with NO affirmative probe signal the TTL
    /// backstop still reclaims the same aged claim — a gone holder cannot
    /// deadlock the scope. Differs from AC2's fixture only in the probe's
    /// answer, so the pair witnesses exactly what the probe changes.
    #[test]
    fn bug_1765_unaffirmed_session_claim_past_ttl_still_reclaims() {
        let existing = session_claim("2", "imac", 4242, "2026-06-16T11:00:00Z");
        let d = decide_claim(
            Some(&existing),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| false,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    /// The probe is gated OFF process-backed claims: a drain/solo lock is
    /// governed by its own exact pid probe and TTL, and even a (buggy) probe
    /// answering `true` must not rescue it.
    #[test]
    fn bug_1765_probe_cannot_rescue_a_process_backed_claim() {
        // Dead pid, fresh heartbeat → reclaim, probe ignored.
        let dead = lock_claim("imac", 4242, "2026-06-16T11:59:55Z");
        let d = decide_claim(
            Some(&dead),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| false,
            |_| true,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
        // Live pid, aged-out heartbeat → reclaim, probe still ignored.
        let aged = lock_claim("imac", 4242, "2026-06-16T11:00:00Z");
        let d = decide_claim(
            Some(&aged),
            &ours("1", "imac"),
            now(),
            "imac",
            |_, _| true,
            |_| true,
        );
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    /// AC4's predicate: `claim_staleness` mirrors `decide_claim`, so every
    /// reporting surface behind `partition_claims` (`aida session leases`,
    /// `status`, `doctor`, `team`) shows a verified-live cross-clone session
    /// as live past its TTL, and an unaffirmed one as stale.
    #[test]
    fn bug_1765_staleness_mirrors_decide_for_a_live_session_past_ttl() {
        let c = lease_claim("imac", 1134977, "2026-10-01T22:05:14+00:00");
        let aged = at("2026-10-02T05:19:29+00:00"); // 26055s past the 1800s TTL
        assert_eq!(
            claim_staleness(&c, aged, "imac", |_, _| false, |_| true),
            None,
            "a verified-live session's claim must be reported live past its TTL"
        );
        assert!(
            claim_staleness(&c, aged, "imac", |_, _| false, |_| false).is_some(),
            "with no probe signal the TTL verdict is unchanged"
        );
    }

    /// Plumbing: the real probe reads the holder clone's OWN local lease
    /// table and affirms a standing (here: Dormant — worktree exists, no live
    /// claude, young) lease for the claim's scope, case-insensitively; it
    /// affirms nothing for a remote host, a different scope, a deleted clone,
    /// a process-backed claim, or a deleted worktree.
    #[test]
    fn bug_1765_probe_reads_the_holder_clones_local_lease_table() {
        let dir = tempfile::tempdir().unwrap();
        let clone_root = dir.path();
        let sessions = clone_root.join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let worktree = clone_root.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let started = Utc::now().to_rfc3339();
        // Serialize the lease through the toml crate so the worktree path is
        // escaped correctly on every platform (a raw format! into a TOML
        // basic string would corrupt Windows backslashes).
        let mut lease = toml::map::Map::new();
        for (k, v) in [
            ("id", "feedbeef0001"),
            ("scope", "BUG-9999"),
            ("slug", "bug-9999"),
            ("owner", "test"),
            ("branch", "bug-9999"),
            ("hostname", "testhost"),
        ] {
            lease.insert(k.to_string(), toml::Value::String(v.to_string()));
        }
        lease.insert(
            "worktree_path".to_string(),
            toml::Value::String(worktree.display().to_string()),
        );
        lease.insert(
            "started_at".to_string(),
            toml::Value::String(started.clone()),
        );
        std::fs::write(
            sessions.join("feedbeef0001.toml"),
            toml::to_string(&toml::Value::Table(lease)).unwrap(),
        )
        .unwrap();
        let mut c = lease_claim("imac", 1, &started);
        c.scope = "bug-9999".to_string(); // matches "BUG-9999" case-insensitively
        c.clone_path = clone_root.display().to_string();
        let now_t = Utc::now();
        assert!(
            probe_session_liveness_with(&c, "imac", &[], now_t),
            "a standing lease for the scope must affirm the claim"
        );
        assert!(
            !probe_session_liveness_with(&c, "otherhost", &[], now_t),
            "a remote host is never affirmed"
        );
        let mut other_scope = c.clone();
        other_scope.scope = "BUG-0000".to_string();
        assert!(
            !probe_session_liveness_with(&other_scope, "imac", &[], now_t),
            "a scope with no lease in the holder clone is not affirmed"
        );
        let mut gone_clone = c.clone();
        gone_clone.clone_path = clone_root.join("nope").display().to_string();
        assert!(
            !probe_session_liveness_with(&gone_clone, "imac", &[], now_t),
            "a deleted clone is not affirmed"
        );
        let mut locked = c.clone();
        locked.process_backed = true;
        assert!(
            !probe_session_liveness_with(&locked, "imac", &[], now_t),
            "the probe never applies to a process-backed claim"
        );
        // Reap the worktree: the lease now classifies Stale → no affirmation,
        // so the TTL backstop reclaims a genuinely gone same-host holder.
        std::fs::remove_dir(&worktree).unwrap();
        assert!(
            !probe_session_liveness_with(&c, "imac", &[], now_t),
            "a deleted worktree must not be affirmed"
        );
    }

    // trace:TASK-1930 | ai:agy
    #[test]
    fn task_1930_release_claim_rechecks_ownership_after_pull_rebase() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let clone_a_store = dir.path().join("clone_a_store");
        let clone_b_store = dir.path().join("clone_b_store");
        let clone_a_root = dir.path().join("clone_a");
        let clone_b_root = dir.path().join("clone_b");
        std::fs::create_dir_all(&clone_a_root).unwrap();
        std::fs::create_dir_all(&clone_b_root).unwrap();

        let status = std::process::Command::new("git")
            .args(["init", "--bare", origin.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());

        aida_core::git_ops::init(&clone_a_store).unwrap();
        aida_core::git_ops::configure_user(&clone_a_store, "Test", "test@example.com").unwrap();
        let status = std::process::Command::new("git")
            .current_dir(&clone_a_store)
            .args(["remote", "add", "origin", origin.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(clone_a_store.join("README.md"), "seed\n").unwrap();
        aida_core::git_ops::add(&clone_a_store, &["README.md"]).unwrap();
        aida_core::git_ops::commit(&clone_a_store, "seed").unwrap();
        let branch = aida_core::git_ops::current_branch(&clone_a_store).unwrap();
        assert!(aida_core::git_ops::push(&clone_a_store, "origin", &branch).unwrap());

        let status = std::process::Command::new("git")
            .args([
                "clone",
                origin.to_str().unwrap(),
                clone_b_store.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        aida_core::git_ops::configure_user(&clone_b_store, "Test", "test@example.com").unwrap();

        // Clone A acquires the claim first.
        let out_a = acquire_claim(
            &clone_a_store,
            "TASK-1930",
            &clone_a_root,
            "claude",
            false,
            false,
        )
        .unwrap();
        assert_eq!(out_a, AcquireOutcome::Acquired);

        // Clone B forces an override and pushes to origin while Clone A's local
        // store still has Clone A's own claim on disk.
        let out_b = acquire_claim(
            &clone_b_store,
            "TASK-1930",
            &clone_b_root,
            "claude",
            false,
            true,
        )
        .unwrap();
        assert!(matches!(
            out_b,
            AcquireOutcome::Acquired | AcquireOutcome::Reclaimed(_)
        ));

        // Clone A now calls release_claim: before pull_rebase its local disk
        // says clone_a_root owns the claim, but after pull_rebase origin's tip
        // says clone_b_root owns it. release_claim must NOT delete Clone B's claim.
        release_claim(&clone_a_store, "TASK-1930", &clone_a_root);

        let surviving = read_claim(&claim_path(&clone_a_store, "TASK-1930"))
            .expect("Clone B's claim must survive Clone A's release_claim after pull_rebase");
        assert_eq!(surviving.clone_path, canonical_clone_path(&clone_b_root));
    }

    // trace:TASK-1930 | ai:agy
    #[test]
    fn task_1930_acquire_claim_and_lock_claim_fail_closed_on_unsafe_store_and_local_write_errors() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let store_root = dir.path().join("store");
        let clone_root = dir.path().join("clone");
        std::fs::create_dir_all(&clone_root).unwrap();

        let status = std::process::Command::new("git")
            .args(["init", "--bare", origin.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());

        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let status = std::process::Command::new("git")
            .current_dir(&store_root)
            .args(["remote", "add", "origin", origin.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(store_root.join("README.md"), "seed\n").unwrap();
        aida_core::git_ops::add(&store_root, &["README.md"]).unwrap();
        aida_core::git_ops::commit(&store_root, "seed").unwrap();
        let branch = aida_core::git_ops::current_branch(&store_root).unwrap();
        assert!(aida_core::git_ops::push(&store_root, "origin", &branch).unwrap());

        // 1. Unsafe store (rebase in progress): ensure_store_write_safe fails inside
        // with_store_write_lock and MUST propagate as Err (fail-closed), never Ok(Unavailable).
        let rebase_dir = store_root.join(".git").join("rebase-merge");
        std::fs::create_dir_all(&rebase_dir).unwrap();
        let unsafe_claim = acquire_claim(
            &store_root,
            "TASK-1930",
            &clone_root,
            "claude",
            false,
            false,
        );
        assert!(
            unsafe_claim.is_err(),
            "acquire_claim must fail closed on an unsafe (mid-rebase) store, got {unsafe_claim:?}"
        );
        let unsafe_lock = acquire_lock_claim(
            &store_root,
            LockKind::Drain,
            &clone_root,
            "aida queue drain",
            60,
            false,
        );
        assert!(
            unsafe_lock.is_err(),
            "acquire_lock_claim must fail closed on an unsafe (mid-rebase) store, got {unsafe_lock:?}"
        );
        std::fs::remove_dir_all(&rebase_dir).unwrap();

        // 2. Local claim-file write failure (after refresh_store succeeds) MUST
        // propagate as Err, never Ok(Unavailable).
        let lease_file_as_dir = claim_path(&store_root, "TASK-1930");
        std::fs::create_dir_all(&lease_file_as_dir).unwrap();
        let claim_write_err = acquire_claim(
            &store_root,
            "TASK-1930",
            &clone_root,
            "claude",
            false,
            false,
        );
        assert!(
            claim_write_err.is_err(),
            "acquire_claim must propagate claim-file write failure as Err, got {claim_write_err:?}"
        );
        std::fs::remove_dir_all(&lease_file_as_dir).unwrap();

        let lock_file_as_dir = lock_claim_path(&store_root, LockKind::Drain);
        std::fs::create_dir_all(&lock_file_as_dir).unwrap();
        let lock_write_err = acquire_lock_claim(
            &store_root,
            LockKind::Drain,
            &clone_root,
            "aida queue drain",
            60,
            false,
        );
        assert!(
            lock_write_err.is_err(),
            "acquire_lock_claim must propagate claim-file write failure as Err, got {lock_write_err:?}"
        );
        std::fs::remove_dir_all(&lock_file_as_dir).unwrap();

        // 3. Local git commit error (after refresh_store and git add succeed, via a
        // ref lock `.git/refs/heads/<branch>.lock`) MUST propagate as Err and must
        // not roll back pre-existing HEAD.
        let head_before = aida_core::git_ops::head_sha(&store_root).unwrap();
        let branch_lock = store_root
            .join(".git")
            .join("refs")
            .join("heads")
            .join(format!("{branch}.lock"));
        std::fs::write(&branch_lock, "locked").unwrap();

        let claim_res = acquire_claim(
            &store_root,
            "TASK-1930",
            &clone_root,
            "claude",
            false,
            false,
        );
        assert!(
            claim_res.is_err(),
            "acquire_claim must propagate git commit failure as Err, got {claim_res:?}"
        );
        let lock_res = acquire_lock_claim(
            &store_root,
            LockKind::Drain,
            &clone_root,
            "aida queue drain",
            60,
            false,
        );
        assert!(
            lock_res.is_err(),
            "acquire_lock_claim must propagate git commit failure as Err, got {lock_res:?}"
        );
        std::fs::remove_file(&branch_lock).unwrap();
        assert_eq!(
            aida_core::git_ops::head_sha(&store_root).unwrap(),
            head_before,
            "failed local claim commit must not roll back pre-existing HEAD"
        );
    }

    // trace:TASK-1930 | ai:agy
    #[test]
    fn task_1930_coordination_claim_and_release_wait_for_store_write_lock_barrier() {
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let store_root = dir.path().join("store");
        let clone_root = dir.path().join("clone");
        std::fs::create_dir_all(&clone_root).unwrap();

        assert!(std::process::Command::new("git")
            .args(["init", "--bare", origin.to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        assert!(std::process::Command::new("git")
            .current_dir(&store_root)
            .args(["remote", "add", "origin", origin.to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        std::fs::write(store_root.join("README.md"), "seed\n").unwrap();
        aida_core::git_ops::add(&store_root, &["README.md"]).unwrap();
        aida_core::git_ops::commit(&store_root, "seed").unwrap();
        let branch = aida_core::git_ops::current_branch(&store_root).unwrap();
        assert!(aida_core::git_ops::push(&store_root, "origin", &branch).unwrap());

        let initial_head = aida_core::git_ops::head_sha(&store_root).unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let store_holder = store_root.clone();
        let holder = std::thread::spawn(move || {
            aida_core::db::with_store_write_lock(&store_holder, || -> Result<()> {
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        locked_rx.recv().unwrap();

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let store_writer = store_root.clone();
        let clone_writer = clone_root.clone();
        let writer = std::thread::spawn(move || {
            let outcome = acquire_claim(
                &store_writer,
                "TASK-1930",
                &clone_writer,
                "claude",
                false,
                false,
            )
            .unwrap();
            done_tx.send(outcome).unwrap();
        });

        let wait_start = Instant::now();
        while aida_core::db::store_write_lock_contended_for(&store_root) == 0 {
            assert!(
                wait_start.elapsed() < Duration::from_secs(10),
                "timed out waiting for acquire_claim to contend on store_write_lock"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            done_rx.try_recv().is_err(),
            "acquire_claim must not complete while store_write_lock is held"
        );
        assert_eq!(
            aida_core::git_ops::head_sha(&store_root).unwrap(),
            initial_head,
            "HEAD must not advance while store_write_lock is held"
        );

        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            AcquireOutcome::Acquired
        );
        writer.join().unwrap();
    }
}

#[cfg(test)]
#[path = "tests/bug_2162_evidence_refusal_tests.rs"]
mod bug_2162_evidence_refusal_tests;

// trace:BUG-2017 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2017_allocation_refusal_tests.rs"]
mod bug_2017_allocation_refusal_tests;

// trace:BUG-2378 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2378_lease_stamp_tests.rs"]
mod bug_2378_lease_stamp_tests;

// trace:BUG-2188 | ai:codex
#[cfg(test)]
mod bug2188_claim_rollback_tests {
    use super::*;
    #[test]
    fn bug2188_acquisition_receipt_adoption_and_conditional_release() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let hub = temp.path().join("hub.git");
        let root = temp.path().join("project");
        let store = root.join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(
            temp.path(),
            &["init", "-q", "--bare", hub.to_str().unwrap()],
        );
        run(&store, &["init", "-q", "-b", "aida-store"]);
        run(&store, &["config", "user.name", "fixture"]);
        run(&store, &["config", "user.email", "fixture@test"]);
        run(&store, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        run(&store, &["remote", "add", "origin", hub.to_str().unwrap()]);
        run(&store, &["push", "-q", "-u", "origin", "aida-store"]);
        let (_, receipt) =
            acquire_claim_receipted(&store, "TASK-1", &root, "implementer", false, false).unwrap();
        let expected = receipt.expect("fresh acquisition has an invocation receipt");
        assert!(expected.claim_id.is_some());
        let (_, adopted) =
            acquire_claim_receipted(&store, "TASK-1", &root, "implementer", false, false).unwrap();
        assert!(adopted.is_none(), "same-clone adoption owns no rollback");
        let mut successor = expected.clone();
        successor.claim_id = Some(uuid::Uuid::new_v4().to_string());
        aida_core::db::with_store_write_lock(&store, || {
            std::fs::write(claim_path(&store, "TASK-1"), toml::to_string(&successor)?)?;
            let rel = format!("coordination/leases/{}.toml", sanitize_scope("TASK-1"));
            aida_core::git_ops::add(&store, &[&rel])?;
            aida_core::git_ops::commit(&store, "successor")?;
            Ok(())
        })
        .unwrap();
        assert!(aida_core::git_ops::push(&store, "origin", "aida-store").unwrap());
        assert!(!release_claim_receipt(&store, &expected).unwrap());
        assert_eq!(
            read_claim(&claim_path(&store, "TASK-1")).unwrap(),
            successor
        );
        let (_, receipt) =
            acquire_claim_receipted(&store, "TASK-2", &root, "implementer", false, false).unwrap();
        assert!(release_claim_receipt(&store, &receipt.unwrap()).unwrap());
        assert!(!claim_path(&store, "TASK-2").exists());
    }

    // Review round 1: a same-clone re-claim must rewrite the claim's
    // freshness/identity metadata (TTL and liveness read it) while keeping
    // the claim_id and granting no rollback authority.
    // trace:BUG-2188.ac41d009 | ai:claude
    #[test]
    fn bug2188_same_clone_reacquire_refreshes_metadata_without_receipt() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let hub = temp.path().join("hub.git");
        let root = temp.path().join("project");
        let store = root.join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(
            temp.path(),
            &["init", "-q", "--bare", hub.to_str().unwrap()],
        );
        run(&store, &["init", "-q", "-b", "aida-store"]);
        run(&store, &["config", "user.name", "fixture"]);
        run(&store, &["config", "user.email", "fixture@test"]);
        run(&store, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        run(&store, &["remote", "add", "origin", hub.to_str().unwrap()]);
        run(&store, &["push", "-q", "-u", "origin", "aida-store"]);
        let (_, receipt) =
            acquire_claim_receipted(&store, "TASK-1", &root, "implementer", false, false).unwrap();
        let first = receipt.expect("fresh acquisition has an invocation receipt");
        // Age the stored claim as an earlier session's would be.
        let mut aged = first.clone();
        aged.started_at = "2026-01-01T00:00:00+00:00".into();
        aged.heartbeat_at = aged.started_at.clone();
        aged.pid = 1;
        aida_core::db::with_store_write_lock(&store, || {
            std::fs::write(claim_path(&store, "TASK-1"), toml::to_string(&aged)?)?;
            let rel = format!("coordination/leases/{}.toml", sanitize_scope("TASK-1"));
            aida_core::git_ops::add(&store, &[&rel])?;
            aida_core::git_ops::commit(&store, "aged")?;
            Ok(())
        })
        .unwrap();
        assert!(aida_core::git_ops::push(&store, "origin", "aida-store").unwrap());

        let (outcome, adopted) =
            acquire_claim_receipted(&store, "TASK-1", &root, "reviewer", true, false).unwrap();
        assert_eq!(outcome, AcquireOutcome::Acquired);
        assert!(adopted.is_none(), "same-clone adoption owns no rollback");
        let refreshed = read_claim(&claim_path(&store, "TASK-1")).unwrap();
        assert_eq!(refreshed.claim_id, first.claim_id, "claim_id is kept");
        assert_ne!(refreshed.started_at, aged.started_at);
        assert_ne!(refreshed.heartbeat_at, aged.heartbeat_at);
        assert_eq!(refreshed.pid, std::process::id());
        assert_eq!(refreshed.agent, "reviewer");
        assert!(refreshed.review_verb);
        // The refresh reached the shared store, not just the local file.
        run(&store, &["fetch", "-q", "origin"]);
        let remote = crate::test_git::git_command()
            .current_dir(&store)
            .args(["rev-parse", "HEAD", "origin/aida-store"])
            .output()
            .unwrap();
        let shas = String::from_utf8_lossy(&remote.stdout);
        let mut lines = shas.lines();
        assert_eq!(lines.next(), lines.next());
        // The refreshed claim now belongs to the re-claiming session: the
        // first invocation's receipt no longer authorizes deleting it.
        assert!(!release_claim_receipt(&store, &first).unwrap());
        assert!(claim_path(&store, "TASK-1").exists());
    }

    /// TASK-2069: a writer that committed on top of our unpushed claim
    /// commit (push runs outside the lock) no longer fails the CAS retry:
    /// our commit is dropped and the peer's commit kept.
    // trace:TASK-1955 | ai:claude (TASK-2069)
    #[test]
    fn unpushed_claim_commit_is_dropped_under_a_newer_local_commit() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path();
        let run = |args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(store)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "aida-store"]);
        run(&["config", "user.name", "fixture"]);
        run(&["config", "user.email", "fixture@test"]);
        run(&["commit", "-q", "--allow-empty", "-m", "seed"]);
        std::fs::create_dir_all(leases_dir(store)).unwrap();
        let claim = build_claim(store, "TASK-1", store, "implementer", false);
        std::fs::write(
            claim_path(store, "TASK-1"),
            toml::to_string(&claim).unwrap(),
        )
        .unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "claim"]);
        let claim_sha = aida_core::git_ops::head_sha(store).unwrap();
        std::fs::write(store.join("peer.txt"), "peer\n").unwrap();
        run(&["add", "peer.txt"]);
        run(&["commit", "-q", "-m", "peer"]);

        drop_unpushed_claim_commit(store, &claim_sha, &claim).unwrap();
        assert!(
            !claim_path(store, "TASK-1").exists(),
            "our claim commit is gone"
        );
        assert!(store.join("peer.txt").exists(), "the peer's commit is kept");
        let log = crate::test_git::git_command()
            .current_dir(store)
            .args(["log", "--format=%s"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout), "peer\nseed\n");
    }

    /// TASK-2101: when the rollback removed nothing (the claim commit was
    /// rewritten, so it is no longer an ancestor of HEAD), the drop reports
    /// failure instead of success, and the claim is left in place.
    // trace:TASK-1955 | ai:claude (TASK-2101)
    #[test]
    fn unpushed_claim_commit_drop_fails_when_nothing_was_removed() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path();
        let run = |args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(store)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "aida-store"]);
        run(&["config", "user.name", "fixture"]);
        run(&["config", "user.email", "fixture@test"]);
        run(&["commit", "-q", "--allow-empty", "-m", "seed"]);
        std::fs::create_dir_all(leases_dir(store)).unwrap();
        let claim = build_claim(store, "TASK-1", store, "implementer", false);
        std::fs::write(
            claim_path(store, "TASK-1"),
            toml::to_string(&claim).unwrap(),
        )
        .unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "claim"]);
        let claim_sha = aida_core::git_ops::head_sha(store).unwrap();
        // A peer's pull --rebase rewrote the claim commit under a new sha.
        run(&["commit", "-q", "--amend", "-m", "claim (rebased)"]);
        std::fs::write(store.join("peer.txt"), "peer\n").unwrap();
        run(&["add", "peer.txt"]);
        run(&["commit", "-q", "-m", "peer"]);

        let err =
            drop_unpushed_claim_commit(store, &claim_sha, &claim).expect_err("nothing was dropped");
        assert!(err.to_string().contains("was not dropped"), "{err}");
        assert!(claim_path(store, "TASK-1").exists(), "the claim stays");
    }

    #[test]
    fn bug2188_successor_claim_survives_exact_rollback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(leases_dir(dir.path())).unwrap();
        let expected = build_claim(dir.path(), "TASK-1", dir.path(), "implementer", false);
        let mut successor = expected.clone();
        successor.claim_id = Some(uuid::Uuid::new_v4().to_string());
        let path = claim_path(dir.path(), "TASK-1");
        let body = toml::to_string(&successor).unwrap();
        std::fs::write(&path, &body).unwrap();
        assert!(!delete_claim_receipt_locked(dir.path(), &expected).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        let mut legacy = expected;
        legacy.claim_id = None;
        std::fs::write(&path, toml::to_string(&legacy).unwrap()).unwrap();
        assert!(!delete_claim_receipt_locked(dir.path(), &legacy).unwrap());
    }
}
