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
//! A fully COLD key on the production path does not batch at all and does
//! not even snapshot: one cheap `rev-parse` locates the key's state file
//! and, when there is none (every positive key, every never-shown key), the
//! ORIGINAL complete `--all` collector runs at once with its unchanged
//! bounded opportunity (advisor amendment 4) and no serial snapshot cost in
//! front of it. When that oracle completes with zero parsed references the
//! key gets an empty MARKER file (no claim of any kind). The next show of a
//! marked key is a SEEDING show: S and R are captured BEFORE the original
//! collector starts and re-captured after it; only an oracle that ran to
//! completion against that same unchanged snapshot seeds every current root
//! CLOSED (advisor ruling 2026-10-25 (a)). That is the same CLOSED
//! definition — Git walked every commit reachable from those roots and its
//! prefilter plus the parsers admitted none — so the show after it is a
//! warm read. Measured reason: the live repository has ~83k reachable
//! commits, so a batch-only cold proof needs dozens of incomplete shows PER
//! QUERY KEY before it ever answers. A parsed hit stops the proof, drops the
//! key's state (a positive key goes back to the cheap cold path) and defers
//! to the original complete `--date-order` collector (ordering, branch,
//! shipped and drift behavior stay Git-owned). Unsupported history
//! semantics (shallow, grafts, replace refs or replace-control overrides)
//! refuse this fast path entirely and keep the original bounded query.
//!
//! Every `ProvedEmpty` — a pure warm read included — is re-checked against
//! an AFTER S/R capture taken once the frontier walk is done (amendment 1;
//! ruling 2026-10-25 (e)).
//!
//! P1 (SKETCH v2, signed 2026-10-25 23:55): POSITIVE keys are cached too. A
//! complete positive answer leaves a positive marker; the next show seeds a
//! [`PositiveCone`] (the roots Git started from plus its records in their
//! exact `--date-order` order) under the same seed rule. The BEFORE
//! snapshot's roots (R) are read ahead of the original query, so they are
//! pinned before the oracle reads any ref; only its context half (S)
//! overlaps the original. A warm positive show walks only the commits above
//! the cone, orders them by their own ancestry and dates, and answers only
//! when the P1' guards in [`positive_warm`] (advisor ack 2026-10-26 04:15,
//! superseding plain O1) prove that equals Git's whole-graph order; otherwise
//! the original runs and re-seeds. Git renders the answer, so it is
//! byte-for-byte what the original prints.
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
/// configured guard is out of scope).
// trace:BUG-2221 | ai:claude
const SHOW_OBJECTIVE: Duration = Duration::from_millis(1000);

/// Reserve for post-linkage work (relationships/render/queue membership),
/// chosen from the slice-1 phase measurements on the live shape (render +
/// relationships measured well under this on a loaded host). The probe stops
/// batching early enough to leave this much of the objective for the rest of
/// the command.
// trace:BUG-2221 | ai:claude
const RENDER_RESERVE: Duration = Duration::from_millis(150);

/// Below this remaining OUTER time the proof cannot even complete its warm
/// snapshot reads — skip the fast path and keep the original bounded query
/// (never suppress a formerly useful answer just to avoid going over the
/// objective; advisor amendment 4).
// trace:BUG-2221 | ai:claude
const MIN_PROBE_TIME: Duration = Duration::from_millis(50);

/// Maximum OIDs per `--stdin` batch.
// trace:BUG-2221 | ai:claude
const BATCH_MAX: usize = 1024;

/// Cache record schema. Covers this file format AND the behavior of the
/// three commit-message parsers plus the `-F --grep` argument construction:
/// BUMP THIS on any change to
/// [`crate::extract_spec_ids_from_commit`],
/// [`crate::extract_referenced_spec_ids_from_commit`],
/// [`crate::extract_trace_line_spec_ids`], or to how linkage grep queries are
/// formed — a stale proof made under older predicate semantics must never be
/// reused (the `parser_behavior_pin` test is the tripwire).
// trace:BUG-2221 | ai:claude
const SCHEMA_VERSION: u32 = 1;

/// Per-(context, query) state file byte cap — read AND write.
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;

/// Per-(context, query) proved-node cap (closed + open). A DAG that exceeds
/// it records an `oversize` tombstone: the fast path stays off for that
/// context/query instead of churning forever.
// trace:BUG-2221 | ai:claude
const MAX_STATE_NODES: usize = 150_000;

/// P1: most grep-matched records a positive cone caches. A key past it is
/// pinned to the original query (an `oversize` positive marker).
// trace:BUG-2221 | ai:claude
const MAX_POSITIVE_HITS: usize = 4096;

/// P1: most commits a warm positive read walks above its cone. More new
/// history than this → the original query (and a re-seed).
// trace:BUG-2221 | ai:claude
const MAX_NEW_COMMITS: usize = 4096;

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

/// Record process entry (idempotent).
// trace:BUG-2221 | ai:claude
pub(crate) fn note_process_entry() {
    let _ = PROCESS_ENTRY.set(Instant::now());
}

fn process_entry() -> Instant {
    *PROCESS_ENTRY.get_or_init(Instant::now)
}

/// Outcome of the negative probe, as seen by the linkage collector.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum NegativeProbe {
    /// Every current root is CLOSED under an unchanged snapshot: exact proof
    /// of zero referencing commits — return complete empty commits.
    ProvedEmpty,
    /// P1 (SKETCH v2): a warm POSITIVE key answered from its cached cone. The
    /// payload is byte-for-byte what the original `--all --date-order -F
    /// --grep` collector prints (same pretty format, same records, same
    /// order), rendered by Git from the proved, ordered OID list.
    ProvedPositive(String),
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
    /// Production `true`: a key with no state file defers to the original
    /// collector with NO snapshot ([`OracleSeed::Mark`]); a marked or reset
    /// key defers with a before-snapshot ([`OracleSeed::Seed`]) instead of
    /// batching. Batch-proof fixtures pass `false` to exercise cold
    /// batching directly.
    pub(crate) cold_oracle: bool,
}

/// What the original collector's answer may settle for this key, handed back
/// by the probe whenever it defers to the original query. Opaque to the
/// collector, which passes it to [`settle_oracle`] with the oracle's answer.
// trace:BUG-2221 | ai:claude
pub(crate) enum OracleSeed {
    /// No state file at all: no snapshot was taken, so the answer can only
    /// MARK the key (zero references) — it proves nothing about any snapshot.
    Mark { loc: Location, queries: QueryKey },
    /// A seeding show: the BEFORE snapshot the oracle's answer is pinned to,
    /// captured before the original collector started.
    Seed {
        ctx: GitContext,
        roots: RootsSnapshot,
        queries: QueryKey,
        /// The proof state the seed extends (empty for a marked/reset key).
        state: NegativeState,
    },
    /// P1: a seeding show for a key whose last complete answer was POSITIVE.
    /// The BEFORE R (roots) capture finishes BEFORE the probe returns, so it
    /// is pinned ahead of the oracle's own ref read: a ref move after R
    /// shows up in the AFTER capture and seeds nothing (advisor 2026-10-26
    /// 02:50). Only the S (context) capture runs on a thread overlapping the
    /// original query (ruling 2026-10-25: no needless serial snapshot).
    Concurrent {
        queries: QueryKey,
        pending: PendingSnapshot,
    },
}

/// A BEFORE snapshot whose R half is already captured and whose S half is
/// being captured on its own thread. Dropping it joins the thread (its git
/// children are bounded by the same deadline), so no child outlives the
/// command.
// trace:BUG-2221 | ai:claude
pub(crate) struct PendingSnapshot {
    handle: Option<std::thread::JoinHandle<Option<ContextSnapshot>>>,
    ctx: Option<ContextSnapshot>,
    roots: Option<RootsSnapshot>,
}

type Snapshot = (Option<ContextSnapshot>, Option<RootsSnapshot>);

impl PendingSnapshot {
    /// Start S on a thread, then capture R on the caller's thread and return
    /// only once R is complete — the caller starts the oracle after this, so
    /// every ref R read precedes every ref the oracle reads.
    fn start(project_root: &Path, loc: Location, deadline: Instant) -> Self {
        let root = project_root.to_path_buf();
        let handle = std::thread::spawn(move || capture_context(&root, Some(loc), deadline));
        let roots = capture_roots(project_root, deadline);
        PendingSnapshot {
            handle: Some(handle),
            ctx: None,
            roots,
        }
    }

    /// Wait for the S capture to finish (idempotent).
    fn finish(&mut self) {
        if let Some(h) = self.handle.take() {
            self.ctx = h.join().ok().flatten();
        }
    }

    fn join(mut self) -> Snapshot {
        self.finish();
        (self.ctx.take(), self.roots.take())
    }
}

impl Drop for PendingSnapshot {
    fn drop(&mut self) {
        self.finish();
    }
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
    let queries = query_key(ids);

    // ---- Cold check: no state file → the original query, with no snapshot
    // in front of it (ruling 2026-10-25: positive and cold shows must not pay
    // a serial snapshot that cannot help them). ----
    let mut located = None;
    if tuning.cold_oracle {
        let Some(loc) = locate(project_root, tuning.deadline) else {
            return outcome(NegativeProbe::RunOriginal);
        };
        let Some(on_disk) = key_file_state(&loc, &queries) else {
            return (
                NegativeProbe::RunOriginal,
                Some(OracleSeed::Mark { loc, queries }),
            );
        };
        if on_disk.positive && on_disk.cone.is_none() {
            // P1: the last complete answer was positive. Too many hits to
            // cache → the original query alone, as before P1. Otherwise a
            // seeding show: BEFORE's R is read here, ahead of the original;
            // its S overlaps the original.
            // trace:BUG-2221 | ai:claude
            if on_disk.oversize {
                return outcome(NegativeProbe::RunOriginal);
            }
            let pending = PendingSnapshot::start(project_root, loc, tuning.deadline);
            return (
                NegativeProbe::RunOriginal,
                Some(OracleSeed::Concurrent { queries, pending }),
            );
        }
        located = Some(loc);
    }

    // ---- S: bounded context snapshot (or an unsupported-mode refusal) ----
    let (ctx, roots) = capture_snapshot(project_root, located, tuning.deadline);
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

    let wanted_uc: HashSet<String> = ids.iter().map(|s| s.to_ascii_uppercase()).collect();
    let mut state = load_state(&ctx, &queries);
    if let Some(cone) = state.cone.take() {
        // P1: a warm POSITIVE key. Exact answer from the cone plus the
        // commits added since, or — on any guard failure — the original
        // query, re-seeding against THIS (before) snapshot.
        // trace:BUG-2221 | ai:claude
        return match positive_warm(project_root, &queries, &ctx, &roots, &cone, tuning.deadline) {
            Some(log) => outcome(NegativeProbe::ProvedPositive(log)),
            None => (
                NegativeProbe::RunOriginal,
                Some(OracleSeed::Seed {
                    ctx,
                    roots,
                    queries,
                    state: NegativeState {
                        oversize: false,
                        positive: false,
                        closed: BTreeSet::new(),
                        open: BTreeMap::new(),
                        ..state
                    },
                }),
            ),
        };
    }
    if tuning.cold_oracle && (state.oversize || state.is_empty()) {
        // Marked (or reset, or made under another S) key: a SEEDING show. The
        // original complete collector runs with its unchanged bounded
        // opportunity, pinned to the snapshot captured just above.
        let state = NegativeState {
            oversize: false,
            ..state
        };
        return (
            NegativeProbe::RunOriginal,
            Some(OracleSeed::Seed {
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
    // No complete negative this invocation. Incomplete is only ever the
    // answer of an invocation that made (and persisted) progress — that is
    // what makes cold unknown converge. Otherwise the original bounded query
    // keeps its full opportunity (never worse than before this fast path),
    // and its answer may seed the key against the BEFORE snapshot.
    let yield_unproved = |ctx: GitContext,
                          roots: RootsSnapshot,
                          queries: QueryKey,
                          state: NegativeState,
                          dirty: bool| {
        if dirty && publish_state(&ctx.loc, &state) {
            return outcome(NegativeProbe::Incomplete);
        }
        let seed = tuning.cold_oracle.then_some(OracleSeed::Seed {
            ctx,
            roots,
            queries,
            state,
        });
        (NegativeProbe::RunOriginal, seed)
    };
    loop {
        let frontier = pending_frontier(&state, &roots.roots);
        let Some(frontier) = frontier else {
            // Frontier bookkeeping itself blew the node bound.
            state.mark_oversize();
            let _ = publish_state(&ctx.loc, &state);
            return outcome(NegativeProbe::RunOriginal);
        };
        if frontier.is_empty() {
            // All current roots CLOSED. Re-capture S and R with the same
            // bounded routines AFTER the walk — mandatory on every
            // ProvedEmpty, a pure warm read (no batches) included; a moved
            // snapshot is never answered or published as a complete negative
            // (retry once if time remains).
            let (ctx_after, roots_after) = capture_snapshot(project_root, None, tuning.deadline);
            let s_unchanged = matches!(ctx_after, Some(ContextSnapshot::Supported(ref c)) if c.s_digest == ctx.s_digest);
            if !s_unchanged {
                // Failed capture or mixed-semantics evidence: do not persist,
                // do not certify; the original query answers.
                return outcome(NegativeProbe::RunOriginal);
            }
            let Some(roots_after) = roots_after else {
                return yield_unproved(ctx, roots, queries, state, dirty);
            };
            if roots_after.digest == roots.digest {
                // A failed publication (read-only/contended cache) loses only
                // speed: THIS invocation's proof is sound and complete.
                if dirty {
                    let _ = publish_state(&ctx.loc, &state);
                } else {
                    // A warm hit is a USE: keep it off the eviction front.
                    touch_state(&ctx.loc, &queries);
                }
                return outcome(NegativeProbe::ProvedEmpty);
            }
            if retried || expired(tuning.batch_deadline) {
                return yield_unproved(ctx, roots, queries, state, dirty);
            }
            retried = true;
            roots = roots_after;
            continue;
        }
        if expired(tuning.batch_deadline) || batches_ran >= tuning.max_batches {
            return yield_unproved(ctx, roots, queries, state, dirty);
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
            BatchOutcome::Hit => {
                // The key is POSITIVE: every later show runs the original
                // collector anyway, so its negative evidence buys nothing and
                // keeping it would make every positive show re-snapshot and
                // re-batch. Drop it — the key returns to the cheap cold path.
                forget_key(&ctx.loc, &queries);
                return outcome(NegativeProbe::PositiveHit);
            }
            BatchOutcome::Proved { negatives } => {
                batches_ran += 1;
                dirty = true;
                state.absorb(negatives);
                if state_nodes(&state).len() > MAX_STATE_NODES {
                    state.mark_oversize();
                    let _ = publish_state(&ctx.loc, &state);
                    return outcome(NegativeProbe::RunOriginal);
                }
            }
            BatchOutcome::Failed => {
                // Truncated / missing / malformed output (or the batch
                // deadline expiring mid-batch) certifies nothing. Progress
                // already proved is kept; the original query answers, and
                // settles the key against the BEFORE snapshot (a positive
                // answer must still drop the key's state).
                if dirty {
                    let _ = publish_state(&ctx.loc, &state);
                }
                let seed = tuning.cold_oracle.then_some(OracleSeed::Seed {
                    ctx,
                    roots,
                    queries,
                    state,
                });
                return (NegativeProbe::RunOriginal, seed);
            }
        }
    }
}

/// The original collector's answer, as [`settle_oracle`] needs it.
// trace:BUG-2221 | ai:claude
pub(crate) struct OracleAnswer {
    /// The original `git log --all -F --grep` exited successfully inside its
    /// bound and every output record framed. A timed-out, failed or partial
    /// run is never evidence (ruling 2026-10-25 (a)).
    pub(crate) complete: bool,
    /// Parsed referencing commits it returned.
    pub(crate) references: usize,
    /// P1: the full OID of EVERY record it printed (grep-matched, before the
    /// parsers), in its exact `--date-order` output order.
    pub(crate) records: Vec<String>,
}

/// Settle a deferral with the ORIGINAL collector's answer. Returns whether
/// anything was published.
///
/// - Incomplete oracle → nothing, ever (ruling 2026-10-25 (a)).
/// - Parsed references → the key is positive: its state is dropped.
/// - Zero references on an unsnapshotted ([`OracleSeed::Mark`]) key → an
///   empty marker only, so the next show captures a snapshot first.
/// - Zero references on a seeding show ([`OracleSeed::Seed`]) → re-capture
///   S and R with the same bounded routines; only when both are unchanged
///   since the BEFORE snapshot (captured before the oracle started) is every
///   current root published CLOSED (the oracle walked their whole ancestry
///   and admitted no reference — the CLOSED definition, proved by Git
///   itself). A moved snapshot, an expired deadline or a failed publication
///   records nothing; the next show simply seeds again.
// trace:BUG-2221 | ai:claude
pub(crate) fn settle_oracle(
    project_root: &Path,
    seed: OracleSeed,
    answer: OracleAnswer,
    deadline: Instant,
) -> bool {
    if !answer.complete {
        return false;
    }
    if answer.references > 0 {
        // trace:BUG-2221 | ai:claude
        let usable = answer.records.len() >= answer.references
            && answer.records.iter().all(|o| git_arg_guard::is_hex_sha(o));
        return match seed {
            // P1: a positive MARKER (no claim) — the next show seeds.
            OracleSeed::Mark { loc, queries } if usable => publish_state(
                &loc,
                &NegativeState::positive_marker(&queries, answer.records.len()),
            ),
            OracleSeed::Mark { .. } => false,
            OracleSeed::Seed {
                ctx,
                roots,
                queries,
                ..
            } => {
                if !usable {
                    forget_key(&ctx.loc, &queries);
                    return false;
                }
                publish_positive_seed(project_root, ctx, roots, &queries, answer.records, deadline)
            }
            OracleSeed::Concurrent { queries, pending } => {
                let (ctx, roots) = pending.join();
                let (Some(ContextSnapshot::Supported(ctx)), Some(roots)) = (ctx, roots) else {
                    return false;
                };
                if !usable || roots.replace_refs {
                    forget_key(&ctx.loc, &queries);
                    return false;
                }
                publish_positive_seed(project_root, ctx, roots, &queries, answer.records, deadline)
            }
        };
    }
    match seed {
        OracleSeed::Mark { loc, queries } => publish_state(&loc, &NegativeState::marker(&queries)),
        OracleSeed::Seed {
            ctx,
            roots,
            queries: _,
            state,
        } => publish_seed(project_root, ctx, roots, state, deadline),
        // A key that turned negative: start over from a plain marker.
        OracleSeed::Concurrent { queries, pending } => {
            let (ctx, _) = pending.join();
            match ctx {
                Some(ContextSnapshot::Supported(ctx)) => {
                    publish_state(&ctx.loc, &NegativeState::marker(&queries))
                }
                _ => false,
            }
        }
    }
}

/// P1: seed a POSITIVE key's cone from a COMPLETE original answer pinned to
/// the BEFORE snapshot `ctx`/`roots` (same rule as [`publish_seed`]: S and R
/// re-captured after the oracle must be unchanged, else nothing is
/// published). The cone stores the roots Git started from (in its own
/// `--all` order, with committer timestamps) and the oracle's records in
/// their exact output order.
// trace:BUG-2221 | ai:claude
fn publish_positive_seed(
    project_root: &Path,
    ctx: GitContext,
    roots: RootsSnapshot,
    queries: &QueryKey,
    records: Vec<String>,
    deadline: Instant,
) -> bool {
    if records.len() > MAX_POSITIVE_HITS {
        // Too many to cache: the original query alone from now on.
        let mut tomb = NegativeState::positive_marker(queries, records.len());
        tomb.oversize = true;
        return publish_state(&ctx.loc, &tomb);
    }
    let (ctx_after, roots_after) = capture_snapshot(project_root, None, deadline);
    let Some(ContextSnapshot::Supported(ctx_after)) = ctx_after else {
        return false;
    };
    if ctx_after.s_digest != ctx.s_digest {
        return false;
    }
    let Some(roots_after) = roots_after else {
        return false;
    };
    if roots_after.digest != roots.digest || roots_after.replace_refs {
        return false;
    }
    let state = NegativeState {
        s_digest: ctx.s_digest.clone(),
        positive: true,
        cone: Some(PositiveCone::new(&roots, records)),
        ..NegativeState::marker(queries)
    };
    publish_state(&ctx.loc, &state)
}

fn publish_seed(
    project_root: &Path,
    ctx: GitContext,
    roots: RootsSnapshot,
    mut state: NegativeState,
    deadline: Instant,
) -> bool {
    let (ctx_after, roots_after) = capture_snapshot(project_root, None, deadline);
    let Some(ContextSnapshot::Supported(ctx_after)) = ctx_after else {
        return false;
    };
    if ctx_after.s_digest != ctx.s_digest {
        return false;
    }
    let Some(roots_after) = roots_after else {
        return false;
    };
    if roots_after.digest != roots.digest || roots_after.replace_refs {
        return false;
    }
    // Every current root is negative with an all-negative ancestry: close
    // them, then let closure propagate to any open evidence they unblock.
    for root in roots.roots {
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
    publish_state(&ctx.loc, &state)
}

/// The original collector's pretty format (`trace.rs`), so a warm positive
/// answer renders byte-for-byte what that query prints for the same records.
pub(crate) const COLLECTOR_PRETTY: &str = "--pretty=format:%H\u{1f}%h\u{1f}%s\u{1f}%B%x1e";

/// P1: answer a warm POSITIVE key exactly, or `None` (the caller runs the
/// original query and re-seeds against the same BEFORE snapshot).
///
/// `git log --all --date-order` is Kahn's algorithm over the WHOLE reachable
/// graph (the newest ready commit first, ties by insertion order), and
/// `--grep` filters that sequence. Let OLD be everything reachable from the
/// cone's roots (Git walked it at seed time; `hits` is its filtered order)
/// and NEW be what the current roots reach beyond it. The answer is
/// `kahn(NEW) filtered ++ hits` exactly when:
///
/// 1. every cone root is still reachable — a current root, or a parent of a
///    NEW commit (so OLD is unchanged and still inside `--all`; a deleted
///    ref otherwise falls back);
/// 2. every NEW commit is strictly newer than every cone root (only a cone
///    root, or an OLD commit unblocked by popping one, can compete with a
///    NEW commit, so Git prints all of NEW first);
/// 3. a cone root whose readiness NEW changed (a parent of a NEW commit, or
///    a root no longer listed) has a timestamp no other cone root shares,
///    and cone roots that do share one keep their relative `--all` order (so
///    insertion-order tie breaks inside OLD replay as at seed time);
/// 4. Kahn over NEW never has two ready commits tied on the newest
///    timestamp (the ancestry/date order of NEW — O1 — is then unique).
///
/// Every guard failure, a failed or truncated Git read, a moved snapshot in
/// the mandatory AFTER re-check, or a hit count past the bound returns
/// `None`. A guard never reorders or drops anything itself.
// trace:BUG-2221 | ai:claude
fn positive_warm(
    project_root: &Path,
    queries: &QueryKey,
    ctx: &GitContext,
    roots: &RootsSnapshot,
    cone: &PositiveCone,
    deadline: Instant,
) -> Option<String> {
    let ordered: Vec<String> = if roots.digest == cone.roots_digest {
        // The same ref evidence Git started from at seed time: same answer.
        cone.hits.clone()
    } else {
        let mut fresh = positive_new_hits(project_root, queries, roots, cone, deadline)?;
        fresh.extend(cone.hits.iter().cloned());
        fresh
    };
    if ordered.len() > MAX_POSITIVE_HITS {
        return None;
    }

    // Render through Git in the given order, and re-capture S/R once the
    // walk is done (amendment 1: mandatory on every cached answer). The two
    // reads are independent, so they overlap.
    let (rendered, (ctx_after, roots_after)) = std::thread::scope(|s| {
        let after = s.spawn(|| capture_snapshot(project_root, None, deadline));
        let rendered = git_stdin(
            project_root,
            &["log", "--no-walk=unsorted", COLLECTOR_PRETTY, "--stdin"],
            Some(&ordered),
            deadline,
        );
        (rendered, after.join().unwrap_or((None, None)))
    });
    let unchanged = matches!(ctx_after, Some(ContextSnapshot::Supported(ref c)) if c.s_digest == ctx.s_digest)
        && roots_after.is_some_and(|r| r.digest == roots.digest);
    if !unchanged {
        return None;
    }
    let rendered = rendered?;
    // Exact framing: one record per OID, in order.
    let framed: Vec<&str> = rendered
        .split('\u{1e}')
        .map(|r| r.trim_start_matches(['\n', '\r']))
        .filter(|r| !r.is_empty())
        .map(|r| r.split('\u{1f}').next().unwrap_or("").trim())
        .collect();
    if framed.len() != ordered.len() || framed.iter().zip(&ordered).any(|(a, b)| a != b) {
        return None;
    }

    if roots.digest == cone.roots_digest {
        // A warm hit is a USE: keep it off the eviction front.
        touch_state(&ctx.loc, queries);
    } else {
        let state = NegativeState {
            s_digest: ctx.s_digest.clone(),
            positive: true,
            cone: Some(PositiveCone::new(roots, ordered)),
            ..NegativeState::marker(queries)
        };
        // A failed publication costs speed only; this answer is proved.
        let _ = publish_state(&ctx.loc, &state);
    }
    Some(rendered)
}

/// P1: the grep-matched NEW commits (above the cone), in Git's
/// `--date-order`, or `None` when guards 1-4 of [`positive_warm`] cannot be
/// proved.
// trace:BUG-2221 | ai:claude
fn positive_new_hits(
    project_root: &Path,
    queries: &QueryKey,
    roots: &RootsSnapshot,
    cone: &PositiveCone,
    deadline: Instant,
) -> Option<Vec<String>> {
    let cone_roots: HashSet<&str> = cone.roots.iter().map(String::as_str).collect();
    let current: HashSet<&str> = roots.roots.iter().map(String::as_str).collect();
    let new_tips: Vec<&String> = roots
        .roots
        .iter()
        .filter(|r| !cone_roots.contains(r.as_str()))
        .collect();

    // NEW, framed with parents and committer timestamps.
    let mut new: HashMap<String, (Vec<String>, i64)> = HashMap::new();
    if !new_tips.is_empty() {
        let mut lines: Vec<String> = new_tips.iter().map(|s| s.to_string()).collect();
        lines.extend(cone.roots.iter().map(|r| format!("^{r}")));
        let limit = format!("-n{}", MAX_NEW_COMMITS + 1);
        let out = git_stdin(
            project_root,
            &["log", &limit, "--pretty=format:%H %ct %P", "--stdin"],
            Some(&lines),
            deadline,
        )?;
        for line in out.lines() {
            let mut toks = line.split_whitespace();
            let oid = toks.next()?;
            let ct: i64 = toks.next()?.parse().ok()?;
            let parents: Vec<String> = toks.map(str::to_string).collect();
            if !git_arg_guard::is_hex_sha(oid)
                || parents.iter().any(|p| !git_arg_guard::is_hex_sha(p))
            {
                return None;
            }
            if new.insert(oid.to_string(), (parents, ct)).is_some() {
                return None;
            }
        }
        if new.len() > MAX_NEW_COMMITS {
            return None;
        }
    }

    // Guard 1: no cone root lost.
    let new_parents: HashSet<&str> = new
        .values()
        .flat_map(|(parents, _)| parents.iter().map(String::as_str))
        .collect();
    if cone
        .roots
        .iter()
        .any(|r| !current.contains(r.as_str()) && !new_parents.contains(r.as_str()))
    {
        return None;
    }

    // Guard 2: NEW strictly newer than every cone root.
    let max_root_ct = cone.root_ct.iter().copied().max().unwrap_or(i64::MIN);
    if new.values().any(|(_, ct)| *ct <= max_root_ct) {
        return None;
    }

    // Guard 3: tie-safety of the cone roots' replay.
    let mut ct_count: HashMap<i64, usize> = HashMap::new();
    for ct in &cone.root_ct {
        *ct_count.entry(*ct).or_default() += 1;
    }
    let affected = |r: &str| !current.contains(r) || new_parents.contains(r);
    for (r, ct) in cone.roots.iter().zip(&cone.root_ct) {
        if affected(r) && ct_count[ct] > 1 {
            return None;
        }
    }
    let tied = |r: &str, ct: i64| !affected(r) && ct_count[&ct] > 1;
    let cone_tied: Vec<&str> = cone
        .roots
        .iter()
        .zip(&cone.root_ct)
        .filter(|(r, ct)| tied(r, **ct))
        .map(|(r, _)| r.as_str())
        .collect();
    let cone_tied_set: HashSet<&str> = cone_tied.iter().copied().collect();
    let current_tied: Vec<&str> = roots
        .roots
        .iter()
        .map(String::as_str)
        .filter(|r| cone_tied_set.contains(r))
        .collect();
    if current_tied != cone_tied {
        return None;
    }
    if new.is_empty() {
        return Some(Vec::new());
    }

    // Guard 4 + O1: Kahn over NEW by committer date, refusing any tie at the
    // top of the ready set.
    let mut pending_children: HashMap<&str, usize> = new.keys().map(|k| (k.as_str(), 0)).collect();
    for (parents, _) in new.values() {
        for p in parents {
            if let Some(n) = pending_children.get_mut(p.as_str()) {
                *n += 1;
            }
        }
    }
    let mut ready: std::collections::BinaryHeap<(i64, &str)> = pending_children
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(oid, _)| (new[*oid].1, *oid))
        .collect();
    let mut order: Vec<&str> = Vec::with_capacity(new.len());
    while let Some((ct, oid)) = ready.pop() {
        if ready.peek().is_some_and(|(next, _)| *next == ct) {
            return None;
        }
        order.push(oid);
        for p in &new[oid].0 {
            if let Some(n) = pending_children.get_mut(p.as_str()) {
                *n -= 1;
                if *n == 0 {
                    ready.push((new[p.as_str()].1, p.as_str()));
                }
            }
        }
    }
    if order.len() != new.len() {
        return None;
    }

    // Git's own prefilter over exactly NEW.
    let mut grep_args: Vec<String> = vec!["log".into(), "--no-walk=unsorted".into(), "-F".into()];
    for q in &queries.strings {
        grep_args.push(format!("--grep={q}"));
    }
    grep_args.push("--pretty=format:%H".into());
    grep_args.push("--stdin".into());
    let grep_args: Vec<&str> = grep_args.iter().map(String::as_str).collect();
    let new_oids: Vec<String> = order.iter().map(|s| s.to_string()).collect();
    let matched_raw = git_stdin(project_root, &grep_args, Some(&new_oids), deadline)?;
    let mut matched: HashSet<&str> = HashSet::new();
    for line in matched_raw.lines() {
        let oid = line.trim();
        if oid.is_empty() {
            continue;
        }
        if !new.contains_key(oid) {
            return None;
        }
        matched.insert(oid);
    }
    Some(
        order
            .into_iter()
            .filter(|o| matched.contains(o))
            .map(str::to_string)
            .collect(),
    )
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

pub(crate) struct GitContext {
    /// Where this context's state lives.
    loc: Location,
    /// Digest binding everything a proof's validity depends on.
    s_digest: String,
}

/// The git dirs a key's state file is addressed by, from ONE cheap
/// `rev-parse` — all a cold show pays before the original query.
pub(crate) struct Location {
    /// Absolute git COMMON dir (cache namespace anchor).
    common_dir: PathBuf,
    /// Absolute per-worktree git dir.
    git_dir: String,
    object_format: String,
}

/// Locate the key's git dirs. `None` = not a usable repo (degrade to the
/// original query).
// trace:BUG-2221 | ai:claude
fn locate(project_root: &Path, deadline: Instant) -> Option<Location> {
    let ids = git_capture(
        project_root,
        &[
            "rev-parse",
            "--git-common-dir",
            "--absolute-git-dir",
            "--show-object-format",
        ],
        deadline,
    )?;
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
    Some(Location {
        common_dir,
        git_dir,
        object_format,
    })
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
        let out = aida_core::git_ops::host_git_command(std::env::temp_dir())
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
    git_stdin(project_root, args, None, deadline)
}

/// One bounded git call through the clone-safe builder (host git refuses a
/// seat clone; the TASK-2143 ratchet), with optional `--stdin` lines. Stdout
/// on success. OIDs reach Git only via stdin, never argv interpolation.
// trace:BUG-2221 | ai:claude
fn git_stdin(
    project_root: &Path,
    args: &[&str],
    stdin_lines: Option<&[String]>,
    deadline: Instant,
) -> Option<String> {
    let left = remaining(deadline);
    if left.is_zero() {
        return None;
    }
    let mut cmd = aida_core::git_ops::host_git_command(project_root);
    cmd.args(args);
    // Keep the stdin file alive until the child has exited.
    let mut _stdin_file = None;
    if let Some(lines) = stdin_lines {
        let mut file = tempfile::NamedTempFile::new().ok()?;
        for line in lines {
            writeln!(file, "{line}").ok()?;
        }
        file.flush().ok()?;
        cmd.stdin(std::process::Stdio::from(file.reopen().ok()?));
        _stdin_file = Some(file);
    }
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

/// Capture S. `None` = capture failed (degrade to the original query);
/// `Unsupported` = semantics this proof refuses to reinterpret. A `located`
/// key (the probe's cold check already ran `rev-parse`) skips that read.
///
/// The config digest covers the WHOLE effective `config --list -z`,
/// `branch.*` and `remote.*` included: remote/branch configuration changes
/// ref resolution (advisor ruling 2026-10-25 (d)). The churn that costs —
/// a `push -u` rewriting upstream records resets every proof under this
/// common dir — is speed only.
fn capture_context(
    project_root: &Path,
    located: Option<Location>,
    deadline: Instant,
) -> Option<ContextSnapshot> {
    // Replace-control overrides change which objects Git serves; refuse
    // rather than second-guess (never pass --no-replace-objects ourselves).
    if std::env::var_os("GIT_NO_REPLACE_OBJECTS").is_some()
        || std::env::var_os("GIT_REPLACE_REF_BASE").is_some()
        || std::env::var_os("GIT_GRAFT_FILE").is_some()
    {
        return Some(ContextSnapshot::Unsupported);
    }
    let (version, config, loc) = std::thread::scope(|s| {
        let version = s.spawn(git_version);
        let config = s.spawn(|| git_capture(project_root, &["config", "--list", "-z"], deadline));
        let loc = located.or_else(|| locate(project_root, deadline));
        (
            version.join().ok().flatten(),
            config.join().ok().flatten(),
            loc,
        )
    });
    let (version, config, loc) = (version?, config?, loc?);

    // Shallow / graft files make ancestry lie to this proof's closure rule.
    let nonempty = |p: PathBuf| std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false);
    if nonempty(loc.common_dir.join("shallow")) || nonempty(loc.common_dir.join("info/grafts")) {
        return Some(ContextSnapshot::Unsupported);
    }

    let config_digest = sha256_hex(&[config.as_bytes()]);
    let alternates =
        std::fs::read_to_string(loc.common_dir.join("objects/info/alternates")).unwrap_or_default();
    let s_digest = sha256_hex(&[
        b"show-negative-v1",
        &SCHEMA_VERSION.to_le_bytes(),
        version.as_bytes(),
        loc.common_dir.as_os_str().as_encoded_bytes(),
        loc.git_dir.as_bytes(),
        loc.object_format.as_bytes(),
        config_digest.as_bytes(),
        env_digest().as_bytes(),
        alternates.as_bytes(),
    ]);
    Some(ContextSnapshot::Supported(GitContext { loc, s_digest }))
}

// ---------------------------------------------------------------------------
// R: roots snapshot
// ---------------------------------------------------------------------------

pub(crate) struct RootsSnapshot {
    /// The tip OIDs Git itself admits for `log --all` (deduped), INCLUDING
    /// the current and other linked worktrees' detached HEADs and peeled
    /// annotated tags — obtained from Git's own revision machinery, never
    /// reconstructed from `rev-parse --all` (which omits detached HEADs).
    roots: Vec<String>,
    /// Committer timestamp of each root, parallel to `roots` (P1 ordering
    /// guards).
    root_ct: Vec<i64>,
    /// Digest over the root listing plus companion ref/HEAD evidence
    /// (`for-each-ref` names/OIDs/symrefs and the worktree HEAD list).
    digest: String,
    /// Any `refs/replace/` present → unsupported.
    replace_refs: bool,
}

/// S and R captured concurrently (each is a handful of independent, bounded
/// git reads; serially they were most of a warm read's cost on a loaded
/// host). Both are point-in-time reads either way; callers compare a BEFORE
/// pair with an AFTER pair exactly as before.
// trace:BUG-2221 | ai:claude
fn capture_snapshot(
    project_root: &Path,
    located: Option<Location>,
    deadline: Instant,
) -> (Option<ContextSnapshot>, Option<RootsSnapshot>) {
    std::thread::scope(|s| {
        let roots = s.spawn(|| capture_roots(project_root, deadline));
        let ctx = capture_context(project_root, located, deadline);
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
            &[
                "log",
                "--all",
                "--no-walk=unsorted",
                "--pretty=format:%H %ct",
            ],
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
    let mut root_ct: Vec<i64> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for line in roots_raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (oid, ct) = line.split_once(' ')?;
        let ct: i64 = ct.trim().parse().ok()?;
        if !git_arg_guard::is_hex_sha(oid) {
            return None;
        }
        if seen.insert(oid) {
            roots.push(oid.to_string());
            root_ct.push(ct);
        }
    }
    let replace_refs = refs.lines().any(|l| l.starts_with("refs/replace/"));
    let digest = sha256_hex(&[roots_raw.as_bytes(), refs.as_bytes(), worktrees.as_bytes()]);
    Some(RootsSnapshot {
        roots,
        root_ct,
        digest,
        replace_refs,
    })
}

// ---------------------------------------------------------------------------
// Query identity
// ---------------------------------------------------------------------------

pub(crate) struct QueryKey {
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
pub(crate) struct NegativeState {
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
    /// P1: the last complete original answer for this key was POSITIVE. With
    /// no `cone` this is a marker only (proves nothing); with `oversize` it
    /// pins the key to the original query (too many hits to cache).
    #[serde(default)]
    positive: bool,
    /// P1: the proved positive answer for one snapshot of roots.
    #[serde(default)]
    cone: Option<PositiveCone>,
}

/// P1 (SKETCH v2, signed 2026-10-25 23:55; guards per the P1' amendment in
/// `docs/plans/2026-10-25-bug-2221-positive-path-sketch.md`): Git's own
/// complete answer for one root set. Every commit reachable from `roots` was
/// walked by Git; `hits` is every record it printed, in its exact order.
// trace:BUG-2221 | ai:claude
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PositiveCone {
    /// The roots Git started from, in its own `--all` order.
    roots: Vec<String>,
    /// Committer timestamp of each root, parallel to `roots`.
    root_ct: Vec<i64>,
    /// The R digest those roots were captured under.
    roots_digest: String,
    /// Grep-matched OIDs (before the parsers), in Git's output order.
    hits: Vec<String>,
}

impl PositiveCone {
    fn new(roots: &RootsSnapshot, hits: Vec<String>) -> Self {
        PositiveCone {
            roots: roots.roots.clone(),
            root_ct: roots.root_ct.clone(),
            roots_digest: roots.digest.clone(),
            hits,
        }
    }

    fn well_formed(&self) -> bool {
        self.roots.len() == self.root_ct.len()
            && self.hits.len() <= MAX_POSITIVE_HITS
            && self.roots.len() <= MAX_STATE_NODES
            && self
                .roots
                .iter()
                .chain(&self.hits)
                .all(|o| git_arg_guard::is_hex_sha(o))
    }
}

impl NegativeState {
    /// A positive MARKER: the last complete answer had `hits` records.
    fn positive_marker(queries: &QueryKey, hits: usize) -> Self {
        NegativeState {
            positive: true,
            oversize: hits > MAX_POSITIVE_HITS,
            ..NegativeState::marker(queries)
        }
    }

    /// An empty state for `queries` under no S: the "last complete oracle
    /// answer was zero references" MARKER — and the base every fresh state
    /// is built from. It proves nothing and is never read as a proof.
    fn marker(queries: &QueryKey) -> Self {
        NegativeState {
            schema: SCHEMA_VERSION,
            query_digest: queries.digest.clone(),
            queries: queries.strings.clone(),
            ..Default::default()
        }
    }

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

fn cache_dir(loc: &Location) -> PathBuf {
    loc.common_dir.join(CACHE_NAMESPACE)
}

/// `<16 hex of the worktree's git dirs>-<16 hex of the query>.json`: one file
/// per (worktree, query) key, found WITHOUT computing S (the cold check runs
/// before any snapshot). The S the state was proved under lives inside the
/// file and is validated on load.
fn state_file_name(loc: &Location, queries: &QueryKey) -> String {
    let where_ = sha256_hex(&[
        loc.common_dir.as_os_str().as_encoded_bytes(),
        loc.git_dir.as_bytes(),
    ]);
    format!("{}-{}.json", &where_[..16], &queries.digest[..16])
}

fn state_path(loc: &Location, queries: &QueryKey) -> PathBuf {
    cache_dir(loc).join(state_file_name(loc, queries))
}

/// Whether the key has ANY usable state file (a marker, a proof under any
/// S, a tombstone) — the cold check. Owned no-follow bounded read; a file
/// for another query or schema counts as absent.
fn key_file_state(loc: &Location, queries: &QueryKey) -> Option<NegativeState> {
    read_owned_regular_file(&state_path(loc, queries), MAX_STATE_BYTES)
        .and_then(|bytes| serde_json::from_slice::<NegativeState>(&bytes).ok())
        .filter(|st| {
            st.schema == SCHEMA_VERSION
                && st.query_digest == queries.digest
                && st.queries == queries.strings
        })
}

/// Load state. Any failure — absent, unreadable, over-size, symlinked,
/// unowned, malformed, mismatched identity (a marker included) — degrades
/// to empty (bounded uncached work), never an error or a lock wait.
fn load_state(ctx: &GitContext, queries: &QueryKey) -> NegativeState {
    let fresh = || NegativeState {
        s_digest: ctx.s_digest.clone(),
        ..NegativeState::marker(queries)
    };
    let Some(bytes) = read_owned_regular_file(&state_path(&ctx.loc, queries), MAX_STATE_BYTES)
    else {
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
    if state.cone.as_ref().is_some_and(|c| !c.well_formed()) {
        return fresh();
    }
    state
}

/// Open `path` for reading without following a final symlink; only an
/// owned regular file is returned.
fn open_owned_regular_file(path: &Path) -> Option<(std::fs::File, std::fs::Metadata)> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(path).ok()?;
    let md = file.metadata().ok()?;
    (md.is_file() && owned(&md)).then_some((file, md))
}

/// The metadata's owner is this process's effective uid (always true off
/// unix, where no uid exists to compare).
fn owned(md: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        md.uid() == unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        true
    }
}

/// Bounded, no-follow, owned, regular-file read.
fn read_owned_regular_file(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let (file, md) = open_owned_regular_file(path)?;
    if md.len() > max_bytes {
        return None;
    }
    let mut buf = Vec::with_capacity(md.len() as usize);
    let mut reader = file.take(max_bytes.saturating_add(1));
    reader.read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > max_bytes {
        return None;
    }
    Some(buf)
}

/// Mark a warm read as a use, so eviction is least-recently-USED, not
/// least-recently-written. Best effort: a failure costs only eviction order.
fn touch_state(loc: &Location, queries: &QueryKey) {
    if let Some((file, _)) = open_owned_regular_file(&state_path(loc, queries)) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

/// Drop a key's state (it turned positive). Owned regular files only, inside
/// a verified namespace directory; never follows a link.
fn forget_key(loc: &Location, queries: &QueryKey) {
    let Some(dir) = owned_namespace_dir(loc, false) else {
        return;
    };
    let path = dir.join(state_file_name(loc, queries));
    if std::fs::symlink_metadata(&path).is_ok_and(|md| md.is_file() && owned(&md)) {
        let _ = std::fs::remove_file(&path);
    }
}

/// The namespace directory, every component below the git common dir
/// (`aida-cache`, `show-negative-v1`) verified a REAL directory owned by
/// this uid — never a symlink, which would redirect writes and evictions
/// outside the git dir (amendment 2). `create` makes missing components.
// trace:BUG-2221 | ai:claude
fn owned_namespace_dir(loc: &Location, create: bool) -> Option<PathBuf> {
    let mut dir = loc.common_dir.clone();
    for component in CACHE_NAMESPACE.split('/') {
        dir.push(component);
        if create {
            match std::fs::create_dir(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return None,
            }
        }
        let md = std::fs::symlink_metadata(&dir).ok()?;
        if !md.is_dir() || !owned(&md) {
            return None;
        }
    }
    Some(dir)
}

/// Atomic publish under a NONBLOCKING local cache lock, then deterministic
/// aggregate eviction. Returns `false` when nothing was persisted (contended
/// lock, unwritable repo, symlinked/unowned namespace, serialization
/// failure) — callers degrade, never wait.
// trace:BUG-2221 | ai:claude
fn publish_state(loc: &Location, state: &NegativeState) -> bool {
    let Some(dir) = owned_namespace_dir(loc, true) else {
        return false;
    };
    let lock_path = dir.join(".lock");
    let mut lock_opts = std::fs::OpenOptions::new();
    lock_opts.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_opts.custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(lock) = lock_opts.open(&lock_path) else {
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
            loc,
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
/// owned regular files via symlink metadata — never traverses
/// subdirectories, follows links or unlinks another uid's file), least
/// recently used (mtime — warm reads touch it — then name) first.
// trace:BUG-2221 | ai:claude
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
        if !md.is_file() || !owned(&md) {
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
    /// A parsed hit exists among the batch (the key is positive).
    Hit,
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
        let mut cmd = aida_core::git_ops::host_git_command(project_root);
        cmd.args(args);
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
    let mut hit = false;
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
            hit = true;
        }
    }
    if hit {
        // The caller routes to the original positive collector.
        return BatchOutcome::Hit;
    }
    let mut negatives: Vec<(String, Vec<String>)> = parents_of.into_iter().collect();
    // Inclusion-closed boundary commits close immediately: recorded with no
    // pending parents because their entire ancestry is inside a CLOSED cone.
    negatives.extend(boundary.into_iter().map(|oid| (oid, Vec::new())));
    BatchOutcome::Proved { negatives }
}

#[cfg(test)]
#[path = "tests/bug_2221_negative_proof_tests.rs"]
mod bug_2221_negative_proof_tests;
