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
//! - **Same host** (`host == ours`): probe the recorded `pid` — a dead pid is
//!   reclaimed immediately (fast, exact; slice-1's primary same-host case).
//! - **Any host:** `now - heartbeat_at > ttl_secs` is stale and reclaimable
//!   (portable backstop). Periodic heartbeat refresh is slice 2; slice 1 sets
//!   `heartbeat_at = started_at` so the TTL backstop still works.
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

use anyhow::Result;
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
    /// session leases the holder is the worktree (cross-clone-invisible), so
    /// only the TTL backstop governs reclaim. Defaults to `false` so an old
    /// reader / older binary treats every claim as TTL-governed (never wrongly
    /// reclaims via a dead ephemeral pid). trace:STORY-637 | ai:claude
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
/// otherwise REFUSE. (Session leases are NOT `process_backed` — only the TTL
/// backstop governs them; see [`Claim::process_backed`].)
///
/// `our_host` is this machine's hostname (used to gate the pid fast path —
/// probing a pid only makes sense for a same-host holder). `is_alive` reports
/// whether a pid is currently running on THIS machine.
pub(crate) fn decide_claim(
    existing: Option<&Claim>,
    ours: &Claim,
    now: DateTime<Utc>,
    our_host: &str,
    is_alive: impl Fn(u32) -> bool,
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
    if holder.process_backed && same_host && !is_alive(holder.pid) {
        return ClaimDecision::Reclaim {
            stale_reason: format!(
                "holder pid {} on {} is not running",
                holder.pid, holder.host
            ),
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

/// Resolve this clone's node id. Two clones sharing one store inherit the same
/// `node_id` in the store worktree's `.aida/node.toml` (it rides the branch),
/// so that is NOT a per-clone discriminator — the authoritative per-clone id is
/// the `registry/nodes.toml` entry keyed by `clone_path`. Look that up first,
/// falling back to the store node.toml, then `"1"`. Used only for a human-
/// readable label; clone-identity comparisons key off `clone_path`.
/// trace:STORY-637 | ai:claude
fn node_id_for_clone(store_root: &Path, clone_path: &Path) -> String {
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
        scope: scope.to_string(),
        node_id: node_id_for_clone(store_root, clone_path),
        clone_path: canonical_clone_path(clone_path),
        host: hostname(),
        pid: std::process::id(),
        agent: agent.to_string(),
        started_at: now.clone(),
        // Slice 1: heartbeat == started_at. Slice 2 refreshes periodically.
        heartbeat_at: now,
        ttl_secs: DEFAULT_TTL_SECS,
        // A session lease is worktree-backed, not process-backed: the pid above
        // is the ephemeral `aida session start` shell that exits immediately, so
        // it must NOT drive same-host reclaim. Only the TTL backstop (or an
        // explicit `aida session end` release, or `--force`) reclaims it.
        // Slice 2's drain/solo claims set this `true`. trace:STORY-637
        process_backed: false,
        review_verb,
        // trace:STORY-711 | ai:claude — not wired yet; see the field doc.
        authorized_by: None,
    }
}

/// Outcome of [`acquire_claim`], so the call site can print the right note.
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
pub(crate) fn acquire_claim(
    store_root: &Path,
    scope: &str,
    clone_path: &Path,
    agent: &str,
    review_verb: bool,
    force: bool,
) -> Result<AcquireOutcome> {
    if !store_root.exists() {
        return Ok(AcquireOutcome::Unavailable(
            "no .aida-store worktree attached".to_string(),
        ));
    }
    // Cross-clone coordination only matters when clones SHARE a remote store.
    // A solo clone (no origin) has nothing to coordinate against — proceed.
    if !aida_core::git_ops::has_remote(store_root, "origin") {
        return Ok(AcquireOutcome::Unavailable(
            "store has no origin remote (solo clone)".to_string(),
        ));
    }
    let branch =
        aida_core::git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let ours = build_claim(store_root, scope, clone_path, agent, review_verb);
    let our_host = ours.host.clone();
    let path = claim_path(store_root, scope);

    for attempt in 0..MAX_CAS_RETRIES {
        // Step 1: refresh the store so we see other clones' claims. A failed
        // pull (offline) is NOT fatal — fall through to local-only behavior.
        if let Err(e) = aida_core::git_ops::pull_rebase(store_root, "origin", &branch) {
            return Ok(AcquireOutcome::Unavailable(format!(
                "store unreachable ({e})"
            )));
        }

        // Step 2: decide against the freshly-pulled claim.
        let existing = read_claim(&path);
        let outcome_note = if force {
            // --force: skip the decision, but still report what we overrode.
            existing
                .as_ref()
                .filter(|h| h.node_id != ours.node_id)
                .map(|h| format!("--force overriding {}'s claim ({})", h.host, h.clone_path))
        } else {
            match decide_claim(
                existing.as_ref(),
                &ours,
                Utc::now(),
                &our_host,
                crate::process_probe::pid_is_alive,
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
        let toml = toml::to_string_pretty(&ours)
            .map_err(|e| anyhow::anyhow!("could not serialize lease claim: {e}"))?;
        std::fs::write(&path, toml)
            .map_err(|e| anyhow::anyhow!("could not write lease claim {}: {e}", path.display()))?;

        let rel = format!("{LEASES_SUBDIR}/{}.toml", sanitize_scope(scope));
        aida_core::git_ops::add(store_root, &[&rel])?;
        let msg = format!(
            "chore(coordination): claim lease {scope} (node {})",
            ours.node_id
        );
        // `commit` returns Ok(false) for "nothing to commit" — treat as a
        // successful no-op (our claim already matched what was on disk).
        let committed = aida_core::git_ops::commit(store_root, &msg).unwrap_or(true);
        if !committed {
            return Ok(match outcome_note {
                Some(reason) => AcquireOutcome::Reclaimed(reason),
                None => AcquireOutcome::Acquired,
            });
        }

        match aida_core::git_ops::push(store_root, "origin", &branch) {
            Ok(true) => {
                return Ok(match outcome_note {
                    Some(reason) => AcquireOutcome::Reclaimed(reason),
                    None => AcquireOutcome::Acquired,
                });
            }
            Ok(false) => {
                // Push rejected — another clone wrote first. Discard our local
                // commit + working-tree change so the next iteration's
                // pull --rebase has a clean tree, then re-decide. Mirrors
                // register_node_full's BUG-1-069 reset.
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
                let _ = attempt; // loop continues
                continue;
            }
            Err(e) => {
                // Push errored for a non-contention reason (network mid-flight).
                // Roll back our commit and proceed local-only rather than wedge.
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
                return Ok(AcquireOutcome::Unavailable(format!(
                    "could not push lease claim ({e})"
                )));
            }
        }
    }

    anyhow::bail!(
        "could not claim lease for `{scope}` after {MAX_CAS_RETRIES} attempts — \
         too much contention on the shared lease registry"
    )
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
    // Best-effort pull so the delete applies on top of the latest store.
    let _ = aida_core::git_ops::pull_rebase(store_root, "origin", &branch);
    if std::fs::remove_file(&path).is_err() && path.exists() {
        return;
    }
    let rel = format!("{LEASES_SUBDIR}/{}.toml", sanitize_scope(scope));
    if aida_core::git_ops::add(store_root, &[&rel]).is_err() {
        return;
    }
    let msg = format!("chore(coordination): release lease {scope}");
    // Push only when there was actually something to commit; "nothing to
    // commit" (claim already gone) or a commit error is a quiet no-op.
    if let Ok(true) = aida_core::git_ops::commit(store_root, &msg) {
        let _ = aida_core::git_ops::push(store_root, "origin", &branch);
    }
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
        scope: kind.label().to_string(),
        node_id: node_id_for_clone(store_root, clone_path),
        clone_path: canonical_clone_path(clone_path),
        host: hostname(),
        pid: std::process::id(),
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
        if let Err(e) = aida_core::git_ops::pull_rebase(store_root, "origin", &branch) {
            return Ok(LockAcquireOutcome::Unavailable(format!(
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
                crate::process_probe::pid_is_alive,
            ) {
                ClaimDecision::Refuse { holder } => {
                    anyhow::bail!("{}", lock_refusal_message(kind, &holder, &path));
                }
                ClaimDecision::Acquire => None,
                ClaimDecision::Reclaim { stale_reason } => Some(stale_reason),
            }
        };

        std::fs::create_dir_all(store_root.join(LOCKS_SUBDIR)).ok();
        let toml = toml::to_string_pretty(&ours)
            .map_err(|e| anyhow::anyhow!("could not serialize {} claim: {e}", kind.label()))?;
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
        let committed = aida_core::git_ops::commit(store_root, &msg).unwrap_or(true);
        if !committed {
            return Ok(match outcome_note {
                Some(reason) => LockAcquireOutcome::Reclaimed(reason),
                None => LockAcquireOutcome::Acquired,
            });
        }

        match aida_core::git_ops::push(store_root, "origin", &branch) {
            Ok(true) => {
                return Ok(match outcome_note {
                    Some(reason) => LockAcquireOutcome::Reclaimed(reason),
                    None => LockAcquireOutcome::Acquired,
                });
            }
            Ok(false) => {
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
                continue;
            }
            Err(e) => {
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
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
    let _ = aida_core::git_ops::pull_rebase(store_root, "origin", &branch);
    let path = lock_claim_path(store_root, kind);
    let our_clone = canonical_clone_path(clone_path);
    let Some(mut claim) = read_claim(&path) else {
        return;
    };
    if !claim.clone_path.is_empty() && claim.clone_path != our_clone {
        // A successor reclaimed it — we don't own it anymore; don't stomp.
        return;
    }
    claim.heartbeat_at = Utc::now().to_rfc3339();
    let Ok(toml) = toml::to_string_pretty(&claim) else {
        return;
    };
    if std::fs::write(&path, toml).is_err() {
        return;
    }
    let rel = format!("{LOCKS_SUBDIR}/{}", kind.file_name());
    if aida_core::git_ops::add(store_root, &[&rel]).is_err() {
        return;
    }
    let msg = format!("chore(coordination): heartbeat {} lock", kind.label());
    if let Ok(true) = aida_core::git_ops::commit(store_root, &msg) {
        let _ = aida_core::git_ops::push(store_root, "origin", &branch);
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
    let _ = aida_core::git_ops::pull_rebase(store_root, "origin", &branch);
    if std::fs::remove_file(&path).is_err() && path.exists() {
        return;
    }
    let rel = format!("{LOCKS_SUBDIR}/{}", kind.file_name());
    if aida_core::git_ops::add(store_root, &[&rel]).is_err() {
        return;
    }
    let msg = format!("chore(coordination): release {} lock", kind.label());
    if let Ok(true) = aida_core::git_ops::commit(store_root, &msg) {
        let _ = aida_core::git_ops::push(store_root, "origin", &branch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PROCESS-BACKED foreign claim (slice-2 drain/solo shape): its pid
    /// faithfully tracks liveness, so the same-host pid probe is exact.
    fn claim(node_id: &str, host: &str, pid: u32, heartbeat_at: &str) -> Claim {
        Claim {
            scope: "FR-1".to_string(),
            node_id: node_id.to_string(),
            clone_path: "/home/joe/ai/aida-b".to_string(),
            host: host.to_string(),
            pid,
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
            scope: "FR-1".to_string(),
            node_id: node_id.to_string(),
            clone_path: "/home/joe/ai/aida-a".to_string(),
            host: host.to_string(),
            pid: 1234,
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
        let d = decide_claim(None, &ours("1", "imac"), now(), "imac", |_| true);
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
        let d = decide_claim(Some(&existing), &ours("2", "imac"), now(), "imac", |_| true);
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
        let d = decide_claim(Some(&existing), &mine, now(), "imac", |_| true);
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn dead_pid_same_host_reclaims() {
        // PROCESS-BACKED foreign clone, same host, pid dead → reclaim
        // immediately (fast path), even though heartbeat is fresh.
        let existing = claim("2", "imac", 4242, "2026-06-16T11:59:50Z");
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn session_lease_dead_pid_same_host_does_not_reclaim() {
        // THE MU-504 BUG: a session lease is NOT process-backed — its pid is
        // the ephemeral `aida session start` shell that has already exited. A
        // dead pid must therefore NOT reclaim a fresh-heartbeat session lease;
        // it stays REFUSED until its TTL or an explicit release. trace:STORY-637
        let existing = session_claim("2", "imac", 4242, "2026-06-16T11:59:50Z");
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn session_lease_stale_ttl_still_reclaims() {
        // A session lease is still reclaimable once its heartbeat ages past the
        // TTL — the crash backstop that prevents a permanent deadlock.
        let existing = session_claim("2", "imac", 4242, "2026-06-16T11:00:00Z"); // 3600s
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn live_heartbeat_refuses() {
        // foreign clone, pid alive (or cross-host), heartbeat within TTL → refuse.
        let existing = claim("2", "imac", 4242, "2026-06-16T11:59:00Z");
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| true);
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
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| true);
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn cross_host_live_pid_does_not_take_fast_path() {
        // foreign clone on a DIFFERENT host: pid liveness is meaningless (the
        // pid is on another machine), so a dead-pid probe must NOT reclaim a
        // claim whose heartbeat is still fresh → refuse.
        let existing = claim("2", "otherhost", 4242, "2026-06-16T11:59:00Z");
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn cross_host_stale_heartbeat_reclaims() {
        // foreign clone on another host whose heartbeat aged out → reclaim
        // via the universal TTL backstop (no pid probe needed).
        let existing = claim("2", "otherhost", 4242, "2026-06-16T11:00:00Z");
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn unparseable_heartbeat_falls_back_to_pid_liveness_same_host() {
        // age unknown → same-host pid decides. Alive → refuse; dead → reclaim.
        let existing = claim("2", "imac", 4242, "not-a-timestamp");
        let alive = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| true);
        assert!(
            matches!(alive, ClaimDecision::Refuse { .. }),
            "got {alive:?}"
        );
        let dead = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
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
            scope: "drain".to_string(),
            node_id: "2".to_string(),
            clone_path: "/home/joe/ai/aida-b".to_string(),
            host: host.to_string(),
            pid,
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
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn drain_live_pid_same_host_refuses() {
        // A live drain in another clone, same host → refuse.
        let existing = lock_claim("imac", 4242, "2026-06-16T11:59:30Z");
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| true);
        assert!(matches!(d, ClaimDecision::Refuse { .. }), "got {d:?}");
    }

    #[test]
    fn drain_stale_ttl_reclaims_even_when_pid_alive() {
        // Heartbeat aged past the TTL (pid-recycle / cross-host backstop) →
        // reclaim even with the pid reported alive.
        let existing = lock_claim("imac", 4242, "2026-06-16T11:00:00Z"); // 3600s
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| true);
        assert!(matches!(d, ClaimDecision::Reclaim { .. }), "got {d:?}");
    }

    #[test]
    fn drain_cross_host_live_within_ttl_refuses() {
        // A drain holder on a DIFFERENT host within TTL: pid probe is meaningless
        // → refuse (no fast-path reclaim of a remote live drain).
        let existing = lock_claim("otherhost", 4242, "2026-06-16T11:59:00Z");
        let d = decide_claim(Some(&existing), &ours("1", "imac"), now(), "imac", |_| {
            false
        });
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
}
