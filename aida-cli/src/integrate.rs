//! Pure decision logic for the thin integrator watch-loop (STORY-520, bounded
//! slice).
//!
//! The producer/consumer split (STORY-520) decouples PR *production* (parallel
//! implementers, each leased to its own scope) from PR *integration* (a single
//! serial loop that drives the back-end merge phases). The operator's decision
//! (2026-06-06) was to PROTOTYPE the watch-loop as a thin command first —
//! reusing the TASK-405 `--from-pr` primitive — and promote it to a first-class
//! `integrator` role only if the loop proves out. This module is the loop's
//! pure core: **given already-probed facts about each candidate spec, decide
//! which are ready for integration**, as side-effect-free functions.
//!
//! The probing (loading the store, looking up open PRs / merged state via the
//! forge) and the acting (shelling out to the TASK-405 `--from-pr` path) are
//! SEPARATE and live in `main.rs::handle_queue_integrate`. Keeping the decision
//! pure means it is exhaustively unit-testable with zero risk to the live
//! integration control flow — the same discipline `drain_resume` follows.
//!
//! The handoff protocol is the SUBSTRATE, not a message bus: an implementer
//! finishing work flips the spec to Done and leaves an open PR. The integrator
//! polls for exactly that pair — Done + open PR (and not already merged) — so
//! the substrate state machine *is* the protocol.
//!
//! trace:STORY-520 | ai:claude

/// Already-probed facts about one candidate spec the integrator considers.
/// Built in `main.rs` from the store status + a forge PR lookup; consumed by
/// the pure [`classify_candidate`] below. trace:STORY-520 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntegrationCandidate {
    /// The display SPEC-ID (e.g. `STORY-520`) — for messaging only.
    pub id: String,
    /// True when the spec's status is `Done` (work finished on a branch).
    pub is_done: bool,
    /// True when an OPEN PR was found for this spec via the forge.
    pub has_open_pr: bool,
    /// True when the spec's PR (open or by-branch) is already merged. A merged
    /// PR means integration already happened — only the auto-bump pull is left,
    /// which the integrator does NOT own (TASK-405 refuses an already-merged
    /// drive). trace:STORY-520 | ai:claude
    pub pr_merged: bool,
    /// True when the PR lookup was INCONCLUSIVE (gh missing, auth failure,
    /// transient network error) rather than a clean "no PR". The integrator
    /// must not treat "couldn't tell" as "no PR" — it skips and reports, so a
    /// flaky probe never silently strands a mergeable spec. trace:STORY-520
    pub pr_lookup_inconclusive: bool,
}

/// What the integrator should do with one candidate. trace:STORY-520 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateVerdict {
    /// Done + open, unmerged PR — drive the TASK-405 `--from-pr` phases.
    Integrate,
    /// Done but no open PR yet — the implementer hasn't opened one (or it was
    /// closed without merge). Nothing to integrate; skip quietly.
    SkipNoPr,
    /// Done with an already-merged PR — integration already happened; the
    /// pull/auto-bump (which the integrator doesn't own) will promote it.
    SkipAlreadyMerged,
    /// Status is not Done — not in the ready-for-integration set at all.
    SkipNotDone,
    /// The PR probe was inconclusive (gh missing / auth / network). Skip and
    /// surface, never guess. trace:STORY-520 | ai:claude
    SkipProbeInconclusive,
}

/// The pure heart of the integrator: classify ONE candidate from probed facts.
///
/// The ready-for-integration set is exactly "Done + open PR + not merged". An
/// inconclusive probe is surfaced rather than guessed (a flaky `gh` must not
/// strand a mergeable spec OR — worse — drive a merged one). Order matters:
/// non-Done is filtered first (the cheapest, broadest exclusion), then the
/// inconclusive guard (we can't trust the PR facts), then merged (irreversible
/// already happened), then the open-PR gate. trace:STORY-520 | ai:claude
pub(crate) fn classify_candidate(c: &IntegrationCandidate) -> CandidateVerdict {
    if !c.is_done {
        return CandidateVerdict::SkipNotDone;
    }
    if c.pr_lookup_inconclusive {
        return CandidateVerdict::SkipProbeInconclusive;
    }
    if c.pr_merged {
        return CandidateVerdict::SkipAlreadyMerged;
    }
    if c.has_open_pr {
        return CandidateVerdict::Integrate;
    }
    CandidateVerdict::SkipNoPr
}

/// The integration-ready subset of a candidate batch, in input order — the
/// specs the watch-loop will drive this pass. Pure projection over
/// [`classify_candidate`] so the "which specs merge this pass, in what order"
/// decision is testable without any forge/store I/O. The serial-merge invariant
/// (one merge at a time over the shared `main`) is enforced by the CALLER
/// driving these in turn; this only decides the membership + order.
/// trace:STORY-520 | ai:claude
pub(crate) fn ready_for_integration(
    candidates: &[IntegrationCandidate],
) -> Vec<&IntegrationCandidate> {
    candidates
        .iter()
        .filter(|c| classify_candidate(c) == CandidateVerdict::Integrate)
        .collect()
}

// ── STORY-335: rebase-conflict forecast (read-only first slice) ──────────────
//
// A deferred batch (`--auto-complete=through-ci`) cuts every branch from the
// same stale main; integration must rebase each onto the advancing main. Before
// landing anything, forecast which members WILL conflict — turning "hope the
// rebase isn't bad" into a checkable preview. This slice is read-only: it uses
// `git merge-tree` (no worktree mutation) and never touches the merge path.
//
// Scope note: each branch is forecast against *current* main independently, so
// it catches conflicts with already-landed code. It does NOT yet model the
// sequential accumulation (a member that only conflicts with an earlier,
// not-yet-landed batch member won't show here) — that sequence-aware forecast
// is a follow-up. trace:STORY-335 | ai:claude

/// Read-only forecast of whether a PR branch rebases cleanly onto current main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RebaseForecast {
    /// No conflict — the branch integrates onto current main cleanly.
    Clean,
    /// Conflicts in these files (best-effort list; may be empty if unparsed).
    Conflict(Vec<String>),
    /// Couldn't tell (git too old, branch missing, probe error) — never guessed.
    Unknown(String),
}

/// One member's forecast row, in batch order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForecastRow {
    pub id: String,
    pub forecast: RebaseForecast,
}

/// Aggregate counts over a batch forecast. Pure projection for testability.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ForecastSummary {
    pub clean: usize,
    pub conflict: usize,
    pub unknown: usize,
    /// IDs that will conflict, in batch order — the "resolve these first" list.
    pub conflicting_ids: Vec<String>,
}

/// Parse the conflicted file paths from `git merge-tree --write-tree
/// --name-only` output. On conflict (git exits 1) the first line is the written
/// tree OID and the conflicted paths follow until the first blank line; the
/// informational "Auto-merging/CONFLICT" messages come after that blank.
/// trace:STORY-335 | ai:claude
pub(crate) fn parse_merge_tree_conflict_files(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .skip(1) // first line is the written tree OID
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Summarize a batch's per-member forecast rows.
pub(crate) fn summarize_forecast(rows: &[ForecastRow]) -> ForecastSummary {
    let mut s = ForecastSummary::default();
    for r in rows {
        match &r.forecast {
            RebaseForecast::Clean => s.clean += 1,
            RebaseForecast::Conflict(_) => {
                s.conflict += 1;
                s.conflicting_ids.push(r.id.clone());
            }
            RebaseForecast::Unknown(_) => s.unknown += 1,
        }
    }
    s
}

// ── STORY-335: accumulation-strategy selector ───────────────────────────────
//
// The accumulation shape — how a deferred batch's items are grouped before
// integration — is orthogonal to the phase-range. Only `per-item` is built; the
// other two are accepted on the CLI from day one and error cleanly until
// implemented (the established `--no-human=both` "accept-the-value,
// error-until-implemented" pattern). trace:STORY-335 | ai:claude

/// How a deferred batch's items are accumulated before integration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum IntegrateStrategy {
    /// Branch + PR per item; integrate rebases + merges them in order. The MVP
    /// and the only implemented strategy today.
    PerItem,
    /// All items as commits on one branch; one rebase, one PR. Not built yet.
    OneBranch,
    /// Each item builds on the previous; integrate rebases the stack as a unit.
    /// Not built yet (gated on stacked-branch awareness).
    Stacked,
}

/// A clean "not built yet" message for strategies accepted on the CLI but not
/// yet implemented, or `None` for the supported `per-item` strategy. Lets the
/// flag accept all three values from day one while refusing the unbuilt ones
/// with a pointer rather than a silent no-op. trace:STORY-335 | ai:claude
pub(crate) fn strategy_unsupported_message(strategy: IntegrateStrategy) -> Option<String> {
    match strategy {
        IntegrateStrategy::PerItem => None,
        IntegrateStrategy::OneBranch => Some(
            "the `one-branch` accumulation strategy isn't built yet — all batch items on one \
             branch with a single rebase + PR is a follow-up. Use `--strategy per-item` (default)."
                .to_string(),
        ),
        IntegrateStrategy::Stacked => Some(
            "the `stacked` accumulation strategy isn't built yet — rebasing a stacked branch chain \
             as a unit is a follow-up (gated on stacked-branch awareness). Use `--strategy \
             per-item` (default)."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        id: &str,
        is_done: bool,
        has_open_pr: bool,
        pr_merged: bool,
        inconclusive: bool,
    ) -> IntegrationCandidate {
        IntegrationCandidate {
            id: id.to_string(),
            is_done,
            has_open_pr,
            pr_merged,
            pr_lookup_inconclusive: inconclusive,
        }
    }

    #[test]
    fn done_with_open_unmerged_pr_integrates() {
        let c = candidate("STORY-1", true, true, false, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::Integrate);
    }

    #[test]
    fn not_done_is_excluded_even_with_open_pr() {
        // An InProgress spec with an open PR is the producer still working —
        // never the integrator's to drive.
        let c = candidate("STORY-2", false, true, false, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipNotDone);
    }

    #[test]
    fn done_without_pr_is_skipped() {
        let c = candidate("STORY-3", true, false, false, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipNoPr);
    }

    #[test]
    fn done_with_merged_pr_is_skipped_not_redriven() {
        // The irreversible merge already happened; driving --from-pr would be
        // refused (TASK-405 RefuseAlreadyMerged). We must not even attempt it.
        let c = candidate("STORY-4", true, true, true, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipAlreadyMerged);
    }

    #[test]
    fn merged_takes_precedence_over_open_flag() {
        // Defensive: even if both flags are set, merged wins — never re-drive.
        let c = candidate("STORY-5", true, true, true, false);
        assert_ne!(classify_candidate(&c), CandidateVerdict::Integrate);
    }

    #[test]
    fn inconclusive_probe_is_surfaced_not_guessed() {
        // gh missing / auth / network: we cannot tell whether a PR exists —
        // skip and report, never treat as no-PR and never drive blind.
        let c = candidate("STORY-6", true, false, false, true);
        assert_eq!(
            classify_candidate(&c),
            CandidateVerdict::SkipProbeInconclusive
        );
    }

    #[test]
    fn inconclusive_guard_runs_before_merged_and_open() {
        // Even with has_open_pr set, an inconclusive probe means the facts are
        // untrustworthy — surface rather than integrate.
        let c = candidate("STORY-7", true, true, false, true);
        assert_eq!(
            classify_candidate(&c),
            CandidateVerdict::SkipProbeInconclusive
        );
    }

    #[test]
    fn not_done_short_circuits_before_inconclusive() {
        // A non-Done spec is excluded regardless of probe quality (we wouldn't
        // even have probed it in practice).
        let c = candidate("STORY-8", false, false, false, true);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipNotDone);
    }

    #[test]
    fn ready_set_filters_and_preserves_order() {
        let candidates = vec![
            candidate("A", true, true, false, false),  // integrate
            candidate("B", false, true, false, false), // not done
            candidate("C", true, false, false, false), // no pr
            candidate("D", true, true, true, false),   // merged
            candidate("E", true, true, false, false),  // integrate
            candidate("F", true, false, false, true),  // inconclusive
        ];
        let ready = ready_for_integration(&candidates);
        let ids: Vec<&str> = ready.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "E"]);
    }

    #[test]
    fn empty_batch_yields_empty_ready_set() {
        assert!(ready_for_integration(&[]).is_empty());
    }

    #[test]
    fn all_skipped_yields_empty_ready_set() {
        let candidates = vec![
            candidate("A", false, true, false, false),
            candidate("B", true, false, false, false),
            candidate("C", true, true, true, false),
        ];
        assert!(ready_for_integration(&candidates).is_empty());
    }

    // ── STORY-335 forecast helpers ──────────────────────────────────────────

    #[test]
    fn parse_merge_tree_conflict_files_extracts_paths_before_blank() {
        // Real `git merge-tree --write-tree --name-only` conflict output:
        // OID line, conflicted paths, blank line, then info messages.
        let out = "663d4da1f330\nsrc/main.rs\nsrc/lib.rs\n\nAuto-merging src/main.rs\nCONFLICT (content): Merge conflict in src/main.rs\n";
        assert_eq!(
            parse_merge_tree_conflict_files(out),
            vec!["src/main.rs".to_string(), "src/lib.rs".to_string()]
        );
    }

    #[test]
    fn parse_merge_tree_conflict_files_handles_single_file_no_trailing() {
        let out = "abc123\nf.txt\n\nCONFLICT (content): Merge conflict in f.txt";
        assert_eq!(parse_merge_tree_conflict_files(out), vec!["f.txt"]);
    }

    #[test]
    fn parse_merge_tree_conflict_files_empty_when_only_oid() {
        // Defensive: malformed/short output yields no files rather than panicking.
        assert!(parse_merge_tree_conflict_files("justoid\n").is_empty());
        assert!(parse_merge_tree_conflict_files("").is_empty());
    }

    fn row(id: &str, f: RebaseForecast) -> ForecastRow {
        ForecastRow {
            id: id.to_string(),
            forecast: f,
        }
    }

    #[test]
    fn summarize_forecast_counts_and_orders_conflicts() {
        let rows = vec![
            row("A", RebaseForecast::Clean),
            row(
                "B",
                RebaseForecast::Conflict(vec!["src/main.rs".to_string()]),
            ),
            row("C", RebaseForecast::Clean),
            row("D", RebaseForecast::Conflict(vec![])),
            row("E", RebaseForecast::Unknown("gh".to_string())),
        ];
        let s = summarize_forecast(&rows);
        assert_eq!(s.clean, 2);
        assert_eq!(s.conflict, 2);
        assert_eq!(s.unknown, 1);
        // Conflicting IDs preserved in batch order — the "resolve first" list.
        assert_eq!(s.conflicting_ids, vec!["B".to_string(), "D".to_string()]);
    }

    #[test]
    fn summarize_forecast_empty_is_all_zero() {
        let s = summarize_forecast(&[]);
        assert_eq!(s, ForecastSummary::default());
        assert!(s.conflicting_ids.is_empty());
    }

    // ── STORY-335 strategy selector ─────────────────────────────────────────

    #[test]
    fn per_item_strategy_is_supported() {
        assert!(strategy_unsupported_message(IntegrateStrategy::PerItem).is_none());
    }

    #[test]
    fn one_branch_and_stacked_error_cleanly_pointing_at_followup() {
        // Accepted on the CLI but not built — must refuse with a pointer to the
        // supported strategy, never silently no-op.
        let one = strategy_unsupported_message(IntegrateStrategy::OneBranch)
            .expect("one-branch is unsupported");
        assert!(one.contains("one-branch"));
        assert!(one.contains("per-item"));
        let stacked = strategy_unsupported_message(IntegrateStrategy::Stacked)
            .expect("stacked is unsupported");
        assert!(stacked.contains("stacked"));
        assert!(stacked.contains("per-item"));
    }
}
