//! BUG-2221: exact-query CLOSED-negative ancestry proof for `aida show`'s
//! git-linkage commit scan.
//!
//! The measured dominant cost of a default `aida show` on a spec with NO
//! referencing commits is the `git log --all -F --date-order --grep <id>`
//! child (~885 ms on the live repo): proving the ABSENCE of a match walks
//! every commit reachable from every ref on every call. This module replaces
//! that repeated full-history walk, for the show (deadline) path only, with a
//! resumable proof:
//!
//! - A commit is NEGATIVE for a query set when Git's own `-F --grep`
//!   prefilter plus the existing message parsers
//!   ([`crate::extract_spec_ids_from_commit`],
//!   [`crate::extract_referenced_spec_ids_from_commit`],
//!   [`crate::extract_trace_line_spec_ids`]) admit no wanted id in its full
//!   message.
//! - A commit is CLOSED when it is negative AND every parent is CLOSED
//!   (parentless commits close first; closure propagates child-ward). A tip
//!   being negative never infers anything about its ancestry.
//! - Every CURRENT root (the tips `git log --all` itself admits, including
//!   linked-worktree detached HEADs and peeled tags) being CLOSED is an exact
//!   proof of "no referencing commit anywhere": the collector may return
//!   complete empty commits without any history walk.
//!
//! Evidence is gathered in bounded `--stdin` batches (an exact parent-framing
//! pass, one grep pass over the SAME immutable OIDs) and persisted
//! content-addressed in a local, disposable, versioned namespace under
//! `<git-common-dir>/aida-cache/show-negative-v1/`. A partially built proof
//! returns truthfully INCOMPLETE linkage and resumes on later ordinary shows
//! — no background warmer, no poller, no child outliving the command.
//!
//! A fully COLD key (no state, or a reset tombstone) on the production path
//! does not batch at all: it runs the ORIGINAL complete `--all` collector
//! (its unchanged bounded opportunity to answer — advisor amendment 4) and,
//! when that oracle completes with zero parsed references under an unchanged
//! before/after S and R, seeds the proof with every current root CLOSED.
//! That is the same CLOSED definition — Git walked every commit reachable
//! from those roots and its prefilter plus the parsers admitted none — so
//! the very next show of the same key is a warm read. Measured reason: the
//! live repository has ~83k reachable commits, so a batch-only cold proof
//! needs dozens of incomplete shows PER QUERY KEY before it ever answers. A parsed hit stops the proof and defers to the original
//! complete `--date-order` collector (ordering, branch, shipped and drift
//! behavior stay Git-owned). Unsupported history semantics (shallow, grafts,
//! replace refs or replace-control overrides) refuse this fast path entirely
//! and keep the original bounded query.
//!
//! Losing or corrupting the cache changes speed/completeness, never canonical
//! spec content; contention or an unwritable repo degrades to the original
//! bounded foreground query. Only digests of effective config/environment are
//! persisted, never raw values.
//!
//! Approved design: BUG-2221 SKETCH v1 (codex2) + advisor signoff with
//! binding amendments (see the spec's comments).
// trace:BUG-2221 | ai:claude

use crate::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The unchanged per-call latency objective (`.aida/config.toml` budget is
/// 1000 ms / 10% / 5000 ms — this const mirrors the objective for FOREGROUND
/// SCHEDULING of proof batches; it is not a new budget and changing the
/// configured guard is out of scope). trace:BUG-2221 | ai:claude
const SHOW_OBJECTIVE: Duration = Duration::from_millis(1000);

/// Reserve for post-linkage work (relationships/render/queue membership),
/// chosen from the slice-1 phase measurements on the live shape (render +
/// relationships measured well under this on a loaded host). The probe stops
/// batching early enough to leave this much of the objective for the rest of
/// the command. trace:BUG-2221 | ai:claude
const RENDER_RESERVE: Duration = Duration::from_millis(150);

/// Below this remaining OUTER time the proof cannot even complete its warm
/// snapshot reads — skip the fast path and keep the original bounded query
/// (never suppress a formerly useful answer just to avoid going over the
/// objective; advisor amendment 4). trace:BUG-2221 | ai:claude
const MIN_PROBE_TIME: Duration = Duration::from_millis(50);

/// Maximum OIDs per `--stdin` batch. trace:BUG-2221 | ai:claude
const BATCH_MAX: usize = 1024;

/// Cache record schema. Covers this file format AND the behavior of the
/// three commit-message parsers plus the `-F --grep` argument construction:
/// BUMP THIS on any change to
/// [`crate::extract_spec_ids_from_commit`],
/// [`crate::extract_referenced_spec_ids_from_commit`],
/// [`crate::extract_trace_line_spec_ids`], or to how linkage grep queries are
/// formed — a stale proof made under older predicate semantics must never be
/// reused (the `parser_behavior_pin` test is the tripwire).
/// trace:BUG-2221 | ai:claude
const SCHEMA_VERSION: u32 = 1;

/// Per-(context, query) state file byte cap — read AND write.
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;

/// Per-(context, query) proved-node cap (closed + open). A DAG that exceeds
/// it records an `oversize` tombstone: the fast path stays off for that
/// context/query instead of churning forever. trace:BUG-2221 | ai:claude
const MAX_STATE_NODES: usize = 150_000;

/// Aggregate namespace bounds, enforced deterministically at publish time:
/// oldest (mtime, then name) state files are evicted first. Sized for a
/// fleet: keys are per (worktree context, query), and an oracle-seeded state
/// holds roughly one OID per current root (~3k on the live repo, ~130 KB).
const MAX_CACHE_FILES: usize = 256;
const MAX_CACHE_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Versioned namespace directory under the git COMMON dir (shared across
/// linked worktrees, never inside `aida-store` or tracked config).
const CACHE_NAMESPACE: &str = "aida-cache/show-negative-v1";

/// Process-entry anchor for the foreground scheduling window. Set by
/// `main_entry`; falls back to first use (tests, embedded callers).
static PROCESS_ENTRY: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Record process entry (idempotent). trace:BUG-2221 | ai:claude
pub(crate) fn note_process_entry() {
    let _ = PROCESS_ENTRY.set(Instant::now());
}

fn process_entry() -> Instant {
    *PROCESS_ENTRY.get_or_init(Instant::now)
}

/// Outcome of the negative probe, as seen by the linkage collector.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum NegativeProbe {
    /// Every current root is CLOSED under an unchanged snapshot: exact proof
    /// of zero referencing commits — return complete empty commits.
    ProvedEmpty,
    /// A batch surfaced a parsed hit: run the ORIGINAL `--date-order`
    /// collector (ordering/branch/shipped stay Git-owned).
    PositiveHit,
    /// Fast path unavailable (unsupported history semantics, snapshot or
    /// batch failure, unusable cache, no time to even try): run the original
    /// bounded query — never a degraded answer.
    RunOriginal,
    /// Cold / partially built proof: bounded progress was made and persisted;
    /// report the commit scan truthfully incomplete (shipped unknown) and let
    /// later ordinary shows finish the proof.
    Incomplete,
}

/// Tuning seam: production derives it from the objective; tests pin exact
/// deadlines/batch sizes/batch counts to make progress deterministic (the
/// injected-budget idiom — no wall-clock races in fixtures).
pub(crate) struct ProbeTuning {
    /// Bound for the snapshot captures and the warm read: the outer show
    /// deadline clipped to the per-call commit-log cap.
    pub(crate) deadline: Instant,
    /// When proof BATCHING must yield (production: the 1000 ms objective
    /// from process entry minus [`RENDER_RESERVE`]). A warm read never
    /// abstains because this passed — it is far cheaper than the original.
    pub(crate) batch_deadline: Instant,
    pub(crate) batch_max: usize,
    /// Batches allowed THIS invocation; hitting it behaves exactly like the
    /// deadline expiring (publish progress, report incomplete). Production
    /// passes `usize::MAX` — the wall clock is the real bound.
    pub(crate) max_batches: usize,
    /// A fully cold key defers to the original collector and returns an
    /// [`OracleSeed`] instead of batching (production: `true`). Batch-proof
    /// fixtures pass `false` to exercise cold batching directly.
    pub(crate) cold_oracle: bool,
}

/// Everything needed to seed a cold key from the original collector's
/// complete result: the BEFORE snapshot the oracle's answer must be pinned
/// to. Opaque to the collector. trace:BUG-2221 | ai:claude
pub(crate) struct OracleSeed {
    ctx: GitContext,
    roots: RootsSnapshot,
    queries: QueryKey,
    /// The proof state the seed extends (empty for a cold/reset key).
    state: NegativeState,
}

/// The production probe for `aida show`'s linkage commit scan.
///
/// Snapshot captures and the warm read are bounded by the outer
/// [`crate::SHOW_LINKAGE_DEADLINE`]-based `outer_deadline` clipped to the
/// per-call [`crate::LINKAGE_COMMIT_LOG_TIMEOUT`]. Proof BATCHING yields at
/// the unchanged 1000 ms objective counted from process entry, reserving
/// [`RENDER_RESERVE`] for the rest of the command. With less than
/// [`MIN_PROBE_TIME`] of outer time left the probe abstains entirely.
// trace:BUG-2221 | ai:claude
pub(crate) fn probe_negative_linkage(
    project_root: &Path,
    ids: &[String],
    outer_deadline: Instant,
) -> (NegativeProbe, Option<OracleSeed>) {
    let now = Instant::now();
    let deadline = outer_deadline.min(now + LINKAGE_COMMIT_LOG_TIMEOUT);
    let objective_deadline = process_entry() + SHOW_OBJECTIVE.saturating_sub(RENDER_RESERVE);
    if deadline.saturating_duration_since(now) < MIN_PROBE_TIME {
        return (NegativeProbe::RunOriginal, None);
    }
    probe_inner(
        project_root,
        ids,
        &ProbeTuning {
            deadline,
            batch_deadline: objective_deadline.min(deadline),
            batch_max: BATCH_MAX,
            max_batches: usize::MAX,
            cold_oracle: true,
        },
    )
}

/// [`probe_negative_linkage`] with explicit tuning (the test seam).
// trace:BUG-2221 | ai:claude
pub(crate) fn probe_negative_linkage_tuned(
    project_root: &Path,
    ids: &[String],
    tuning: &ProbeTuning,
) -> NegativeProbe {
    probe_inner(project_root, ids, tuning).0
}

fn probe_inner(
    project_root: &Path,
    ids: &[String],
    tuning: &ProbeTuning,
) -> (NegativeProbe, Option<OracleSeed>) {
    let outcome = |o: NegativeProbe| (o, None);
    // ---- S: bounded context snapshot (or an unsupported-mode refusal) ----
    let (ctx, roots) = capture_snapshot(project_root, tuning.deadline);
    let Some(ctx) = ctx else {
        return outcome(NegativeProbe::RunOriginal);
    };
    let ContextSnapshot::Supported(ctx) = ctx else {
        // Shallow/graft/replace/override semantics: preserve them through the
        // ORIGINAL query under the outer deadline. The refusal itself is part
        // of S, so no proof is ever reused across the mode flipping.
        return outcome(NegativeProbe::RunOriginal);
    };
    // ---- R: the roots Git itself admits for `--all` ----
    let Some(mut roots) = roots else {
        return outcome(NegativeProbe::RunOriginal);
    };
    if roots.replace_refs {
        return outcome(NegativeProbe::RunOriginal);
    }

    let queries = query_key(ids);
    let wanted_uc: HashSet<String> = ids.iter().map(|s| s.to_ascii_uppercase()).collect();
    let mut state = load_state(&ctx, &queries);
    if tuning.cold_oracle && (state.oversize || state.is_empty()) {
        // Fully cold (or reset) key: the original complete collector runs
        // with its unchanged bounded opportunity, and its zero-reference
        // answer seeds this key (see the module docs).
        let state = NegativeState {
            oversize: false,
            ..state
        };
        return (
            NegativeProbe::RunOriginal,
            Some(OracleSeed {
                ctx,
                roots,
                queries,
                state,
            }),
        );
    }
    if state.oversize {
        return outcome(NegativeProbe::RunOriginal);
    }

    let mut batches_ran = 0usize;
    let mut dirty = false;
    let mut retried = false;
    loop {
        let frontier = pending_frontier(&state, &roots.roots);
        let Some(frontier) = frontier else {
            // Frontier bookkeeping itself blew the node bound.
            state.mark_oversize();
            let _ = publish_state(&ctx, &state);
            return outcome(NegativeProbe::RunOriginal);
        };
        if frontier.is_empty() {
            // All current roots CLOSED. A pure warm read (no batches this
            // invocation) is a point-in-time read — no evidence window to
            // revalidate. After batches, re-capture S and R with the same
            // bounded routines; a moved snapshot must not be published as a
            // complete negative (retry once if time remains).
            if batches_ran == 0 {
                return outcome(NegativeProbe::ProvedEmpty);
            }
            let (ctx_after, roots_after) = capture_snapshot(project_root, tuning.deadline);
            let Some(ctx_after) = ctx_after else {
                return outcome(NegativeProbe::Incomplete);
            };
            let s_unchanged = matches!(ctx_after, ContextSnapshot::Supported(ref c) if c.s_digest == ctx.s_digest);
            if !s_unchanged {
                // Mixed-semantics evidence: do not persist, do not certify.
                return outcome(NegativeProbe::Incomplete);
            }
            let Some(roots_after) = roots_after else {
                if dirty {
                    let _ = publish_state(&ctx, &state);
                }
                return outcome(NegativeProbe::Incomplete);
            };
            if roots_after.digest == roots.digest {
                // A failed publication (read-only/contended cache) loses only
                // speed: THIS invocation's proof is sound and complete.
                if dirty {
                    let _ = publish_state(&ctx, &state);
                }
                return outcome(NegativeProbe::ProvedEmpty);
            }
            if retried || expired(tuning.batch_deadline) {
                if dirty {
                    let _ = publish_state(&ctx, &state);
                }
                return outcome(NegativeProbe::Incomplete);
            }
            retried = true;
            roots = roots_after;
            continue;
        }
        if expired(tuning.batch_deadline) || batches_ran >= tuning.max_batches {
            // Incomplete is only ever the answer of an invocation that made
            // (and persisted) progress — that is what makes cold unknown
            // converge. No progress possible (no batch time left, or the
            // progress cannot persist): the original bounded query keeps its
            // full opportunity, never worse than before this fast path, and
            // a zero-reference answer from it may seed the key.
            if dirty && publish_state(&ctx, &state) {
                return outcome(NegativeProbe::Incomplete);
            }
            let seed = tuning.cold_oracle.then(|| OracleSeed {
                ctx,
                roots,
                queries,
                state,
            });
            return (NegativeProbe::RunOriginal, seed);
        }
        match run_batch(
            project_root,
            &queries.strings,
            &wanted_uc,
            &frontier,
            &state,
            tuning.batch_max,
            tuning.batch_deadline,
        ) {
            BatchOutcome::Hit { negatives } => {
                // The batch framed completely; its non-hit members are proved
                // negative and reusable. The hit itself is never cached.
                let changed = state.absorb(negatives);
                if dirty || changed {
                    let _ = publish_state(&ctx, &state);
                }
                return outcome(NegativeProbe::PositiveHit);
            }
            BatchOutcome::Proved { negatives } => {
                batches_ran += 1;
                dirty = true;
                state.absorb(negatives);
                if state_nodes(&state).len() > MAX_STATE_NODES {
                    state.mark_oversize();
                    let _ = publish_state(&ctx, &state);
                    return outcome(NegativeProbe::RunOriginal);
                }
            }
            BatchOutcome::Failed => {
                // Truncated / missing / malformed output certifies nothing.
                if dirty {
                    let _ = publish_state(&ctx, &state);
                }
                return outcome(NegativeProbe::RunOriginal);
            }
        }
    }
}

/// Seed a cold key from the ORIGINAL collector's answer. Call only after the
/// original `git log --all -F --grep` completed successfully and its parsed
/// result held ZERO referencing commits for exactly the seed's queries.
///
/// Re-captures S and R with the same bounded routines; only when both are
/// unchanged since the seed's BEFORE snapshot does it publish every current
/// root as CLOSED (the oracle walked their whole ancestry and admitted no
/// reference — the CLOSED definition, proved by Git itself). A moved
/// snapshot, an expired deadline or a failed publication records nothing;
/// the next show simply stays cold. Returns whether a seed was published.
// trace:BUG-2221 | ai:claude
pub(crate) fn publish_oracle_negative(
    project_root: &Path,
    seed: OracleSeed,
    deadline: Instant,
) -> bool {
    let (ctx_after, roots_after) = capture_snapshot(project_root, deadline);
    let Some(ContextSnapshot::Supported(ctx_after)) = ctx_after else {
        return false;
    };
    if ctx_after.s_digest != seed.ctx.s_digest {
        return false;
    }
    let Some(roots_after) = roots_after else {
        return false;
    };
    if roots_after.digest != seed.roots.digest || roots_after.replace_refs {
        return false;
    }
    let mut state = seed.state;
    // Every current root is negative with an all-negative ancestry: close
    // them, then let closure propagate to any open evidence they unblock.
    for root in seed.roots.roots {
        state.open.remove(&root);
        state.closed.insert(root);
    }
    state.absorb(Vec::new());
    if state_nodes(&state).len() > MAX_STATE_NODES {
        // Too large to keep: publish the roots-only form instead.
        state.open.clear();
        state.closed = roots_after.roots.into_iter().collect();
        if state.closed.len() > MAX_STATE_NODES {
            return false;
        }
    }
    publish_state(&seed.ctx, &state)
}

fn expired(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

// ---------------------------------------------------------------------------
// S: context snapshot
// ---------------------------------------------------------------------------

enum ContextSnapshot {
    Supported(GitContext),
    /// Shallow / grafts / replace refs / replace-control overrides /
    /// unsupported revision or config modes.
    Unsupported,
}

struct GitContext {
    /// Absolute git COMMON dir (cache namespace anchor).
    common_dir: PathBuf,
    /// Digest binding everything a proof's validity depends on.
    s_digest: String,
}

fn sha256_hex(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
        h.update([0u8]);
    }
    format!("{:x}", h.finalize())
}

/// `git --version` once per process (immutable for the process lifetime).
fn git_version() -> Option<String> {
    static V: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let out = std::process::Command::new("git")
            .arg("--version")
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    })
    .clone()
}

/// One bounded git call, stdout on success.
fn git_capture(project_root: &Path, args: &[&str], deadline: Instant) -> Option<String> {
    let left = remaining(deadline);
    if left.is_zero() {
        return None;
    }
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(project_root).args(args);
    match command_output_with_timeout_detail(cmd, left) {
        BoundedCommandOutput::Completed(o) if o.status.success() => {
            Some(String::from_utf8_lossy(&o.stdout).to_string())
        }
        _ => None,
    }
}

/// Semantic environment digest: every `GIT_*` variable plus the locale
/// variables Git's grep/encoding behavior can key on. Digest only — raw
/// values (which can carry credentials) are never persisted or logged.
fn env_digest() -> String {
    let mut vars: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("GIT_") || k == "LANG" || k == "LC_ALL" || k == "LC_CTYPE")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    vars.sort();
    let joined = vars.join("\u{0}");
    sha256_hex(&[joined.as_bytes()])
}

/// The effective config records that can affect what `git log --all -F
/// --grep` admits. `branch.*` and `remote.*` (upstream tracking, fetch/push
/// specs) are dropped: they never change local history walking or message
/// matching, and a fleet rewrites them on every `worktree add -b` /
/// `push -u`, which would otherwise cold-reset every proof. Everything else
/// is kept verbatim (input is `config --list -z`: `key\nvalue\0` records).
fn semantic_config(config_z: &str) -> String {
    config_z
        .split('\0')
        .filter(|rec| {
            let key = rec.split('\n').next().unwrap_or("").to_ascii_lowercase();
            !(key.starts_with("branch.") || key.starts_with("remote."))
        })
        .collect::<Vec<_>>()
        .join("\0")
}

/// Capture S. `None` = capture failed (degrade to the original query);
/// `Unsupported` = semantics this proof refuses to reinterpret.
fn capture_context(project_root: &Path, deadline: Instant) -> Option<ContextSnapshot> {
    // Replace-control overrides change which objects Git serves; refuse
    // rather than second-guess (never pass --no-replace-objects ourselves).
    if std::env::var_os("GIT_NO_REPLACE_OBJECTS").is_some()
        || std::env::var_os("GIT_REPLACE_REF_BASE").is_some()
        || std::env::var_os("GIT_GRAFT_FILE").is_some()
    {
        return Some(ContextSnapshot::Unsupported);
    }
    let (version, config, ids) = std::thread::scope(|s| {
        let version = s.spawn(git_version);
        let config = s.spawn(|| git_capture(project_root, &["config", "--list", "-z"], deadline));
        let ids = git_capture(
            project_root,
            &[
                "rev-parse",
                "--git-common-dir",
                "--absolute-git-dir",
                "--show-object-format",
            ],
            deadline,
        );
        (
            version.join().ok().flatten(),
            config.join().ok().flatten(),
            ids,
        )
    });
    let (version, config, ids) = (version?, config?, ids?);
    let mut lines = ids.lines();
    let common_raw = lines.next()?.trim().to_string();
    let git_dir = lines.next()?.trim().to_string();
    let object_format = lines.next()?.trim().to_string();
    let common_dir = {
        let p = PathBuf::from(&common_raw);
        if p.is_absolute() {
            p
        } else {
            project_root.join(p)
        }
    };
    let common_dir = std::fs::canonicalize(&common_dir).unwrap_or(common_dir);

    // Shallow / graft files make ancestry lie to this proof's closure rule.
    let nonempty = |p: PathBuf| std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false);
    if nonempty(common_dir.join("shallow")) || nonempty(common_dir.join("info/grafts")) {
        return Some(ContextSnapshot::Unsupported);
    }

    let config_digest = sha256_hex(&[semantic_config(&config).as_bytes()]);
    let alternates =
        std::fs::read_to_string(common_dir.join("objects/info/alternates")).unwrap_or_default();
    let s_digest = sha256_hex(&[
        b"show-negative-v1",
        &SCHEMA_VERSION.to_le_bytes(),
        version.as_bytes(),
        common_dir.as_os_str().as_encoded_bytes(),
        git_dir.as_bytes(),
        object_format.as_bytes(),
        config_digest.as_bytes(),
        env_digest().as_bytes(),
        alternates.as_bytes(),
    ]);
    Some(ContextSnapshot::Supported(GitContext {
        common_dir,
        s_digest,
    }))
}

// ---------------------------------------------------------------------------
// R: roots snapshot
// ---------------------------------------------------------------------------

struct RootsSnapshot {
    /// The tip OIDs Git itself admits for `log --all` (deduped), INCLUDING
    /// the current and other linked worktrees' detached HEADs and peeled
    /// annotated tags — obtained from Git's own revision machinery, never
    /// reconstructed from `rev-parse --all` (which omits detached HEADs).
    roots: Vec<String>,
    /// Digest over the root listing plus companion ref/HEAD evidence
    /// (`for-each-ref` names/OIDs/symrefs and the worktree HEAD list).
    digest: String,
    /// Any `refs/replace/` present → unsupported.
    replace_refs: bool,
}

/// S and R captured concurrently (each is a handful of independent, bounded
/// git reads; serially they were most of a warm read's cost on a loaded
/// host). Both are point-in-time reads either way; callers compare a BEFORE
/// pair with an AFTER pair exactly as before. trace:BUG-2221 | ai:claude
fn capture_snapshot(
    project_root: &Path,
    deadline: Instant,
) -> (Option<ContextSnapshot>, Option<RootsSnapshot>) {
    std::thread::scope(|s| {
        let roots = s.spawn(|| capture_roots(project_root, deadline));
        let ctx = capture_context(project_root, deadline);
        (ctx, roots.join().ok().flatten())
    })
}

fn capture_roots(project_root: &Path, deadline: Instant) -> Option<RootsSnapshot> {
    let (roots_raw, refs, worktrees) = std::thread::scope(|s| {
        let refs = s.spawn(|| {
            git_capture(
                project_root,
                &[
                    "for-each-ref",
                    "--format=%(refname)%00%(objectname)%00%(symref)",
                ],
                deadline,
            )
        });
        let worktrees =
            s.spawn(|| git_capture(project_root, &["worktree", "list", "--porcelain"], deadline));
        let roots_raw = git_capture(
            project_root,
            &["log", "--all", "--no-walk=unsorted", "--pretty=format:%H"],
            deadline,
        );
        (
            roots_raw,
            refs.join().ok().flatten(),
            worktrees.join().ok().flatten(),
        )
    });
    let (roots_raw, refs, worktrees) = (roots_raw?, refs?, worktrees?);
    let mut roots: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for line in roots_raw.lines() {
        let oid = line.trim();
        if oid.is_empty() {
            continue;
        }
        if !git_arg_guard::is_hex_sha(oid) {
            return None;
        }
        if seen.insert(oid) {
            roots.push(oid.to_string());
        }
    }
    let replace_refs = refs.lines().any(|l| l.starts_with("refs/replace/"));
    let digest = sha256_hex(&[roots_raw.as_bytes(), refs.as_bytes(), worktrees.as_bytes()]);
    Some(RootsSnapshot {
        roots,
        digest,
        replace_refs,
    })
}

// ---------------------------------------------------------------------------
// Query identity
// ---------------------------------------------------------------------------

struct QueryKey {
    /// The exact byte strings passed as `--grep` arguments (sorted, deduped)
    /// — never a substring/case approximation of Git's own prefilter.
    strings: Vec<String>,
    digest: String,
}

fn query_key(ids: &[String]) -> QueryKey {
    let mut strings: Vec<String> = ids.to_vec();
    strings.sort();
    strings.dedup();
    let joined = strings.join("\u{0}");
    let digest = sha256_hex(&[&SCHEMA_VERSION.to_le_bytes(), joined.as_bytes()]);
    QueryKey { strings, digest }
}

// ---------------------------------------------------------------------------
// Persistent state
// ---------------------------------------------------------------------------

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct NegativeState {
    schema: u32,
    s_digest: String,
    query_digest: String,
    queries: Vec<String>,
    /// The DAG for this context/query exceeded the node bound: fast path
    /// permanently off for this key (deterministic reset, no churn).
    #[serde(default)]
    oversize: bool,
    /// CLOSED-negative OIDs (self negative, every parent CLOSED).
    #[serde(default)]
    closed: BTreeSet<String>,
    /// Proved-negative OIDs whose parents are not all CLOSED yet, with their
    /// parent OIDs (the resumable frontier evidence).
    #[serde(default)]
    open: BTreeMap<String, Vec<String>>,
}

impl NegativeState {
    fn is_empty(&self) -> bool {
        self.closed.is_empty() && self.open.is_empty()
    }

    fn mark_oversize(&mut self) {
        self.oversize = true;
        self.closed.clear();
        self.open.clear();
    }

    /// Insert newly proved negatives and propagate closure: a node closes
    /// when parentless or when every parent is already closed; closing a
    /// node re-checks its known children (worklist, O(nodes + edges)).
    /// Returns whether anything actually changed (a warm re-proof of
    /// already-known negatives must not re-publish the file every show).
    fn absorb(&mut self, negatives: Vec<(String, Vec<String>)>) -> bool {
        let mut changed = false;
        for (oid, parents) in negatives {
            if !self.closed.contains(&oid) {
                let prev = self.open.insert(oid, parents);
                changed |= prev.is_none();
            }
        }
        // children[parent] = open children waiting on it.
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for (oid, parents) in &self.open {
            for p in parents {
                children.entry(p.clone()).or_default().push(oid.clone());
            }
        }
        let mut ready: Vec<String> = self
            .open
            .iter()
            .filter(|(_, parents)| parents.iter().all(|p| self.closed.contains(p)))
            .map(|(oid, _)| oid.clone())
            .collect();
        while let Some(oid) = ready.pop() {
            let Some(parents) = self.open.remove(&oid) else {
                continue;
            };
            debug_assert!(parents.iter().all(|p| self.closed.contains(p)));
            self.closed.insert(oid.clone());
            changed = true;
            if let Some(kids) = children.get(&oid) {
                for kid in kids {
                    if let Some(kparents) = self.open.get(kid) {
                        if kparents.iter().all(|p| self.closed.contains(p)) {
                            ready.push(kid.clone());
                        }
                    }
                }
            }
        }
        changed
    }
}

fn state_nodes(state: &NegativeState) -> BTreeSet<&String> {
    state.closed.iter().chain(state.open.keys()).collect()
}

/// The frontier: OIDs reachable from the current roots through
/// proved-negative-but-open chains that have no verdict yet. `None` when the
/// walk itself exceeds the node bound. An empty vec = all roots CLOSED.
fn pending_frontier(state: &NegativeState, roots: &[String]) -> Option<Vec<String>> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut pending: Vec<String> = Vec::new();
    let mut work: Vec<&str> = roots.iter().map(String::as_str).collect();
    while let Some(oid) = work.pop() {
        if !seen.insert(oid) {
            continue;
        }
        if seen.len() > MAX_STATE_NODES.saturating_mul(2) {
            return None;
        }
        if state.closed.contains(oid) {
            continue;
        }
        if let Some(parents) = state.open.get(oid) {
            work.extend(parents.iter().map(String::as_str));
        } else {
            pending.push(oid.to_string());
        }
    }
    Some(pending)
}

fn cache_dir(ctx: &GitContext) -> PathBuf {
    ctx.common_dir.join(CACHE_NAMESPACE)
}

fn state_file_name(ctx: &GitContext, queries: &QueryKey) -> String {
    format!("{}-{}.json", &ctx.s_digest[..16], &queries.digest[..16])
}

/// Load state. Any failure — absent, unreadable, over-size, symlinked,
/// unowned, malformed, mismatched identity — degrades to empty (bounded
/// uncached work), never an error or a lock wait.
fn load_state(ctx: &GitContext, queries: &QueryKey) -> NegativeState {
    let fresh = || NegativeState {
        schema: SCHEMA_VERSION,
        s_digest: ctx.s_digest.clone(),
        query_digest: queries.digest.clone(),
        queries: queries.strings.clone(),
        ..Default::default()
    };
    let path = cache_dir(ctx).join(state_file_name(ctx, queries));
    let Some(bytes) = read_owned_regular_file(&path, MAX_STATE_BYTES) else {
        return fresh();
    };
    let Ok(state) = serde_json::from_slice::<NegativeState>(&bytes) else {
        return fresh();
    };
    if state.schema != SCHEMA_VERSION
        || state.s_digest != ctx.s_digest
        || state.query_digest != queries.digest
        || state.queries != queries.strings
    {
        return fresh();
    }
    // Structural sanity: every stored OID must be hex (the cache is derived
    // state, not an authority — a forged entry can only cost a re-proof, but
    // a non-OID string must never reach a git argv/stdin).
    let hex_ok = state
        .closed
        .iter()
        .chain(state.open.keys())
        .chain(state.open.values().flatten())
        .all(|oid| git_arg_guard::is_hex_sha(oid));
    if !hex_ok || state_nodes(&state).len() > MAX_STATE_NODES {
        return fresh();
    }
    state
}

/// Bounded, no-follow, owned, regular-file read.
fn read_owned_regular_file(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(path).ok()?;
    let md = file.metadata().ok()?;
    if !md.is_file() || md.len() > max_bytes {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if md.uid() != unsafe { libc::geteuid() } {
            return None;
        }
    }
    let mut buf = Vec::with_capacity(md.len() as usize);
    let mut reader = file.take(max_bytes.saturating_add(1));
    reader.read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > max_bytes {
        return None;
    }
    Some(buf)
}

/// Atomic publish under a NONBLOCKING local cache lock, then deterministic
/// aggregate eviction. Returns `false` when nothing was persisted (contended
/// lock, unwritable repo, serialization failure) — callers degrade, never
/// wait. trace:BUG-2221 | ai:claude
fn publish_state(ctx: &GitContext, state: &NegativeState) -> bool {
    let dir = cache_dir(ctx);
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let lock_path = dir.join(".lock");
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    else {
        return false;
    };
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        // Contention skips cache publication; the proof result stands.
        return false;
    }
    let published = (|| {
        let mut bytes = serde_json::to_vec(state).ok()?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            // Over the byte bound: deterministic reset to a tombstone.
            let mut tomb = NegativeState {
                schema: state.schema,
                s_digest: state.s_digest.clone(),
                query_digest: state.query_digest.clone(),
                queries: state.queries.clone(),
                ..Default::default()
            };
            tomb.oversize = true;
            bytes = serde_json::to_vec(&tomb).ok()?;
        }
        let mut tmp = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempfile_in(&dir)
            .ok()?;
        tmp.write_all(&bytes).ok()?;
        tmp.persist(dir.join(state_file_name(
            ctx,
            &QueryKey {
                strings: state.queries.clone(),
                digest: state.query_digest.clone(),
            },
        )))
        .ok()?;
        Some(())
    })()
    .is_some();
    if published {
        enforce_aggregate_bounds(&dir);
    }
    let _ = fs2::FileExt::unlock(&lock);
    published
}

/// Keep the namespace within [`MAX_CACHE_FILES`] / [`MAX_CACHE_TOTAL_BYTES`]:
/// evict our own state files only (strict `<16hex>-<16hex>.json` names,
/// regular files via symlink metadata — never traverses subdirectories or
/// follows links), oldest (mtime, name) first. trace:BUG-2221 | ai:claude
fn enforce_aggregate_bounds(dir: &Path) {
    enforce_aggregate_bounds_with(dir, MAX_CACHE_FILES, MAX_CACHE_TOTAL_BYTES);
}

/// [`enforce_aggregate_bounds`] with explicit limits (unit-test seam).
fn enforce_aggregate_bounds_with(dir: &Path, max_files: usize, max_total_bytes: u64) {
    let is_state_name = |name: &str| -> bool {
        let Some(stem) = name.strip_suffix(".json") else {
            return false;
        };
        let bytes = stem.as_bytes();
        bytes.len() == 33
            && bytes[16] == b'-'
            && stem
                .chars()
                .enumerate()
                .all(|(i, c)| i == 16 || c.is_ascii_hexdigit())
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, String, u64)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_state_name(&name) {
            continue;
        }
        let Ok(md) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !md.is_file() {
            continue;
        }
        let mtime = md.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        files.push((mtime, name, md.len()));
    }
    files.sort();
    let mut count = files.len();
    let mut total: u64 = files.iter().map(|(_, _, len)| len).sum();
    for (_, name, len) in &files {
        if count <= max_files && total <= max_total_bytes {
            break;
        }
        if std::fs::remove_file(dir.join(name)).is_ok() {
            count -= 1;
            total = total.saturating_sub(*len);
        }
    }
}

// ---------------------------------------------------------------------------
// Batches
// ---------------------------------------------------------------------------

enum BatchOutcome {
    /// A parsed hit exists among the batch; the other members' negative
    /// evidence is still completely framed and valid.
    Hit {
        negatives: Vec<(String, Vec<String>)>,
    },
    /// Every member proved negative, with parent framing.
    Proved {
        negatives: Vec<(String, Vec<String>)>,
    },
    /// Failed / truncated / missing / malformed: certifies nothing.
    Failed,
}

/// Run one bounded batch over the pending frontier:
///
/// (A1) EXACT framing — `git log --no-walk=unsorted --stdin` over the first
/// `batch_max` pending OIDs (the seeds); every seed must come back framed
/// with its parent OIDs, exactly once, or the batch certifies nothing. This
/// is what guarantees progress on every batch regardless of commit dates.
///
/// (A2) a bounded DISCOVERY WALK — `git log -n <rest> --stdin` from the
/// seeds' parents that have no verdict yet, with their already-CLOSED parents
/// passed as `^<oid>` so the walk stops at the proved boundary instead of
/// re-framing closed history. Exact framing alone would advance one parent
/// generation per call and never finish a long linear history.
///
/// (B) Git's own `-F --grep` prefilter, via `--no-walk=unsorted --stdin`,
/// over EXACTLY the OIDs A1+A2 framed, then the existing message parsers on
/// the full `%B` — never a substring/case approximation of Git's matching.
///
/// OIDs travel via `--stdin` (hex-validated, never shell/argv interpolation).
/// Any failed, truncated, missing or malformed output certifies nothing.
// trace:BUG-2221 | ai:claude
fn run_batch(
    project_root: &Path,
    queries: &[String],
    wanted_uc: &HashSet<String>,
    pending: &[String],
    known: &NegativeState,
    batch_max: usize,
    deadline: Instant,
) -> BatchOutcome {
    if pending.is_empty() || pending.iter().any(|oid| !git_arg_guard::is_hex_sha(oid)) {
        return BatchOutcome::Failed;
    }
    let batch_max = batch_max.max(1);
    let write_stdin = |lines: &[String]| -> Option<PathBuf> {
        let mut file = tempfile::NamedTempFile::new().ok()?;
        for line in lines {
            writeln!(file, "{line}").ok()?;
        }
        file.flush().ok()?;
        // Keep the file alive for the whole batch.
        let (_, path) = file.keep().ok()?;
        Some(path)
    };
    let run = |args: &[String], stdin_path: &Path| -> Option<String> {
        let left = remaining(deadline);
        if left.is_zero() {
            return None;
        }
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(project_root).args(args);
        let file = std::fs::File::open(stdin_path).ok()?;
        cmd.stdin(std::process::Stdio::from(file));
        match command_output_with_timeout_detail(cmd, left) {
            BoundedCommandOutput::Completed(o) if o.status.success() => {
                Some(String::from_utf8_lossy(&o.stdout).to_string())
            }
            _ => None,
        }
    };
    // `%H %P` framing lines → (oid, parents); None on any malformed line.
    let parse_frames = |frames: &str| -> Option<Vec<(String, Vec<String>)>> {
        let mut out = Vec::new();
        for line in frames.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut toks = line.split_whitespace();
            let oid = toks.next()?;
            let parents: Vec<String> = toks.map(str::to_string).collect();
            if !git_arg_guard::is_hex_sha(oid)
                || parents.iter().any(|p| !git_arg_guard::is_hex_sha(p))
            {
                return None;
            }
            out.push((oid.to_string(), parents));
        }
        Some(out)
    };
    struct CleanupFiles(Vec<PathBuf>);
    impl Drop for CleanupFiles {
        fn drop(&mut self) {
            for p in &self.0 {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let mut cleanup = CleanupFiles(Vec::new());

    // A1: exact framing of the seeds, with exact accounting.
    let mut seeds: Vec<String> = pending.iter().take(batch_max).cloned().collect();
    seeds.sort();
    seeds.dedup();
    let Some(seeds_path) = write_stdin(&seeds) else {
        return BatchOutcome::Failed;
    };
    cleanup.0.push(seeds_path.clone());
    let exact_args: Vec<String> = vec![
        "log".into(),
        "--no-walk=unsorted".into(),
        "--pretty=format:%H %P".into(),
        "--stdin".into(),
    ];
    let Some(seed_frames) = run(&exact_args, &seeds_path).and_then(|f| parse_frames(&f)) else {
        return BatchOutcome::Failed;
    };
    let mut parents_of: HashMap<String, Vec<String>> = HashMap::new();
    for (oid, parents) in seed_frames {
        if parents_of.insert(oid, parents).is_some() {
            return BatchOutcome::Failed;
        }
    }
    let framed_seeds: BTreeSet<&str> = parents_of.keys().map(String::as_str).collect();
    if framed_seeds != seeds.iter().map(String::as_str).collect::<BTreeSet<&str>>() {
        // A seed missing (unreadable/absent object) or an extra record.
        return BatchOutcome::Failed;
    }

    // A2: bounded discovery walk below the seeds, fenced at CLOSED parents.
    let mut walk_complete_with_fence = false;
    let rest = batch_max.saturating_sub(parents_of.len());
    if rest > 0 {
        let mut walk_from: BTreeSet<String> = BTreeSet::new();
        let mut fence: BTreeSet<String> = BTreeSet::new();
        for parents in parents_of.values() {
            for p in parents {
                if known.closed.contains(p) {
                    fence.insert(p.clone());
                } else if !known.open.contains_key(p) && !parents_of.contains_key(p) {
                    walk_from.insert(p.clone());
                }
            }
        }
        if !walk_from.is_empty() {
            let mut lines: Vec<String> = walk_from.iter().cloned().collect();
            lines.extend(fence.iter().map(|p| format!("^{p}")));
            let Some(walk_path) = write_stdin(&lines) else {
                return BatchOutcome::Failed;
            };
            cleanup.0.push(walk_path.clone());
            let walk_args: Vec<String> = vec![
                "log".into(),
                format!("-n{rest}"),
                "--pretty=format:%H %P".into(),
                "--stdin".into(),
            ];
            let Some(walked) = run(&walk_args, &walk_path).and_then(|f| parse_frames(&f)) else {
                return BatchOutcome::Failed;
            };
            walk_complete_with_fence = walked.len() < rest && !fence.is_empty();
            for (oid, parents) in walked {
                parents_of.entry(oid).or_insert(parents);
            }
        }
    }

    // Closure by inclusion. CLOSED means negative with an all-CLOSED (hence
    // all-negative) ancestry, so every ancestor of a CLOSED commit is CLOSED
    // by definition and needs no grep:
    // (i) framed commits reachable (via framed parent links) from a framed
    //     commit that is already CLOSED — the unfenced walk ran into proved
    //     history;
    // (ii) when the fenced walk was NOT truncated by `-n`, a parent of a
    //     framed commit that the walk did not output was excluded by the
    //     `^fence` — reachable from a CLOSED commit.
    let mut inferred: HashSet<String> = HashSet::new();
    let mut work: Vec<String> = parents_of
        .iter()
        .filter(|(oid, _)| known.closed.contains(*oid))
        .flat_map(|(_, parents)| parents.iter().cloned())
        .collect();
    while let Some(oid) = work.pop() {
        if known.closed.contains(&oid) || !inferred.insert(oid.clone()) {
            continue;
        }
        if let Some(parents) = parents_of.get(&oid) {
            work.extend(parents.iter().cloned());
        }
    }
    parents_of.retain(|oid, _| !known.closed.contains(oid) && !inferred.contains(oid));
    // Only the inferred commits that BOUND the retained evidence are kept
    // (state grows with new history, not with re-walked old history).
    let mut boundary: BTreeSet<String> = BTreeSet::new();
    for parents in parents_of.values() {
        for p in parents {
            if parents_of.contains_key(p) || known.closed.contains(p) || known.open.contains_key(p)
            {
                continue;
            }
            if inferred.contains(p) || walk_complete_with_fence {
                boundary.insert(p.clone());
            }
        }
    }
    if parents_of.is_empty() {
        // Everything framed was already proved: record the boundary only.
        return BatchOutcome::Proved {
            negatives: boundary.into_iter().map(|oid| (oid, Vec::new())).collect(),
        };
    }
    let requested: HashSet<&str> = parents_of.keys().map(String::as_str).collect();

    // B: Git's prefilter over EXACTLY the OIDs A framed, then the real
    // parsers on the full message.
    let framed_oids: Vec<String> = parents_of.keys().cloned().collect();
    let Some(framed_path) = write_stdin(&framed_oids) else {
        return BatchOutcome::Failed;
    };
    cleanup.0.push(framed_path.clone());
    let mut grep_args: Vec<String> = vec!["log".into(), "--no-walk=unsorted".into(), "-F".into()];
    for q in queries {
        grep_args.push(format!("--grep={q}"));
    }
    grep_args.push("--pretty=format:%H\u{1f}%B\u{1e}".into());
    grep_args.push("--stdin".into());
    let Some(grepped) = run(&grep_args, &framed_path) else {
        return BatchOutcome::Failed;
    };
    let mut hit_oids: HashSet<String> = HashSet::new();
    for record in grepped.split('\u{1e}') {
        let record = record.trim_start_matches(['\n', '\r']);
        if record.trim().is_empty() {
            continue;
        }
        let mut p = record.splitn(2, '\u{1f}');
        let Some(oid) = p.next() else {
            return BatchOutcome::Failed;
        };
        let oid = oid.trim();
        let body = p.next().unwrap_or("");
        if !requested.contains(oid) {
            return BatchOutcome::Failed;
        }
        // Same resolution the collector applies (BUG-546): trailer parens,
        // referenced ids, and trace: lines over the FULL message.
        let mut resolved = extract_spec_ids_from_commit(body);
        resolved.extend(extract_referenced_spec_ids_from_commit(body));
        resolved.extend(extract_trace_line_spec_ids(body));
        if resolved
            .iter()
            .any(|r| wanted_uc.contains(&r.to_ascii_uppercase()))
        {
            hit_oids.insert(oid.to_string());
        }
    }
    // A hit commit is POSITIVE — it must never enter the negative evidence.
    let mut negatives: Vec<(String, Vec<String>)> = parents_of
        .into_iter()
        .filter(|(oid, _)| !hit_oids.contains(oid))
        .collect();
    // Inclusion-closed boundary commits close immediately: recorded with no
    // pending parents because their entire ancestry is inside a CLOSED cone.
    negatives.extend(boundary.into_iter().map(|oid| (oid, Vec::new())));
    if hit_oids.is_empty() {
        BatchOutcome::Proved { negatives }
    } else {
        // The non-hit members stay proved negative (completely framed); the
        // caller routes to the original positive collector.
        BatchOutcome::Hit { negatives }
    }
}

#[cfg(test)]
#[path = "tests/bug_2221_negative_proof_tests.rs"]
mod bug_2221_negative_proof_tests;
