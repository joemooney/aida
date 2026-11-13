// trace:TASK-3426 | ai:codex
use super::*;

/// TASK-836: probe the richer pre-merge facts for ONE ready integration
/// candidate — CI rollup, RequestChanges (local verdict OR forge decision), and
/// mergeability — and normalize them into the pure [`integrate::PrIntegrationState`]
/// the pre-merge gate decides on. All the messy `gh`-string interpretation lives
/// here, at the probe boundary; the decision ([`integrate::classify_integration_action`])
/// stays pure + unit-tested.
///
/// Conservative: anything we cannot tell degrades to the optimistic value
/// (CI `None`, no RequestChanges, `Unknown` mergeable) — the `--from-pr` drive
/// re-gates CI + merge before the irreversible step, so "couldn't tell" never
/// ships something unsafe; it just doesn't pre-empt a park. trace:TASK-836
pub(crate) fn probe_pr_integration_state(
    project_root: &std::path::Path,
    spec_id: &str,
    branch: Option<&str>,
    snapshot: &OpenPrSnapshot,
) -> integrate::PrIntegrationState {
    // The forge row for this PR (keyed by head branch).
    let item = branch.and_then(|b| snapshot.by_branch.get(b));

    // CI: prefer the snapshot rollup ("pass"/"fail"/"pending"/"missing"/
    // "unknown"/"?"), normalized. "missing" (a required check's row never
    // showed up on this head) and "unknown" (the required-check set itself
    // couldn't be read) are BOTH absent-evidence states, not passes — mapping
    // either into the catch-all `None` arm is the exact PR-2009 false-green
    // shape through `aida integrate` (`classify_integration_action` merges on
    // `None`). trace:BUG-1481 | ai:claude
    // trace:BUG-2492 | ai:codex — an intentional hold cannot settle by
    // waiting for CI. Park it, including recorded holds with a removed label.
    let held = item.is_some_and(|i| {
        i.labels.iter().any(|l| l == "aida:merge-hold")
            || (i.labels.iter().any(|l| l == "aida:merge-hold-recorded")
                && !i.labels.iter().any(|l| l == "aida:merge-hold-cleared"))
    });
    let ci = match item.and_then(|i| i.ci_rollup.as_deref()) {
        Some("pending") if held => integrate::CiState::Failing,
        Some("pass") => integrate::CiState::Passing,
        Some("fail") => integrate::CiState::Failing,
        Some("pending") => integrate::CiState::Running,
        Some("missing") => integrate::CiState::RequiredCheckMissing,
        Some("unknown") => integrate::CiState::Indeterminate,
        _ => integrate::CiState::None,
    };

    // Mergeability: the forge's `mergeable` field — MERGEABLE / CONFLICTING /
    // UNKNOWN. "UNKNOWN" means GitHub hasn't computed it yet; treated optimistic.
    // A behind-base branch presents as CONFLICTING only on a real conflict, so
    // we can't distinguish BehindBase from the snapshot alone — the caller's
    // --rebase step covers the behind-base case, so mapping non-conflict to
    // Unknown here is sufficient. trace:TASK-836
    let mergeable = match item.and_then(|i| i.mergeable.as_deref()) {
        Some("MERGEABLE") => integrate::MergeableState::Mergeable,
        Some("CONFLICTING") => integrate::MergeableState::Conflicting,
        _ => integrate::MergeableState::Unknown,
    };

    // RequestChanges from the forge's reviewDecision (CHANGES_REQUESTED).
    let forge_request_changes = item
        .and_then(|i| i.review_decision.as_deref())
        .map(|d| d.eq_ignore_ascii_case("CHANGES_REQUESTED"))
        .unwrap_or(false);

    // RequestChanges from the LOCAL review-verdict file (the orchestrator's own
    // reviewer writes `.aida/review-verdicts/PR-N.json`). Either source is a
    // hard stop — never merge over a pending RequestChanges. trace:TASK-836
    // Share the launcher's merge chokepoint interpretation. In particular,
    // an existing artifact that is conflicting or unreadable is a block, not
    // the optimistic absence of RequestChanges.
    // trace:BUG-1581 | ai:codex
    let (local_request_changes, review_waiting, review_integrity_unproven) = item
        .map(|i| {
            // The merge chokepoint's corpus, gate ledger included (M1).
            // trace:TASK-1801 | ai:claude
            // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
            let bodies: Vec<String> = match crate::gate::merge_hold_core::corpus_gate_bodies(
                &[project_root],
                i.number,
                &[spec_id.to_string()],
            ) {
                Ok(found) => found.into_iter().map(|(_, body)| body).collect(),
                Err(_) => return (false, false, true),
            };
            if bodies.is_empty() {
                return (false, false, false);
            }
            let Some(head) = i.head_sha.as_deref() else {
                return (false, false, true);
            };
            match crate::gate::review_verdict::reconcile_artifacts_for_sha(
                bodies.iter().map(String::as_str),
                head,
            ) {
                Ok(Some(crate::gate::review_verdict::VerdictKind::Approved)) | Ok(None) => {
                    (false, false, false)
                }
                // Waiting blocks integration without requesting implementation rework.
                // trace:TASK-3777 | ai:codex
                Ok(Some(crate::gate::review_verdict::VerdictKind::Waiting)) => (false, true, false),
                Ok(Some(_)) => (true, false, false),
                Err(_) => (false, false, true),
            }
        })
        .unwrap_or((false, false, false));

    integrate::PrIntegrationState {
        ci,
        request_changes_pending: forge_request_changes || local_request_changes,
        review_integrity_unproven,
        review_waiting,
        mergeable,
    }
}

#[cfg(test)]
mod bug_1581_integration_probe_tests {
    use super::*;

    fn snapshot() -> OpenPrSnapshot {
        let mut snapshot = OpenPrSnapshot::default();
        snapshot.by_branch.insert(
            "bug-1581".into(),
            crate::status_cleanup::OpenPrItem {
                number: 2090,
                title: "fix review reconciliation".into(),
                head_branch: "bug-1581".into(),
                ci_rollup: Some("pass".into()),
                mergeable: Some("MERGEABLE".into()),
                review_decision: None,
                head_sha: Some("ac772eaca9d389fa762a232156df996023bfdf7a".into()),
                labels: Vec::new(),
                created_at: None,
                is_draft: false,
            },
        );
        snapshot
    }

    fn verdict_path(root: &std::path::Path) -> std::path::PathBuf {
        root.join(".aida/review-verdicts/PR-2090.json")
    }

    fn write_separate_verdicts(root: &std::path::Path, pr_verdict: &str, spec_verdict: &str) {
        let dir = root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = |verdict: &str, reviewer: &str| {
            format!(
                r#"{{"verdict":"{verdict}","reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a","recorded_by":"{reviewer}"}}"#
            )
        };
        std::fs::write(
            dir.join("PR-2090.json"),
            artifact(pr_verdict, "reviewer-pr"),
        )
        .unwrap();
        std::fs::write(
            dir.join("BUG-1581.json"),
            artifact(spec_verdict, "reviewer-spec"),
        )
        .unwrap();
    }

    #[test]
    fn integrate_probe_parks_opposite_verdicts_in_separate_artifacts_both_orders() {
        for (pr, spec) in [
            ("Approved", "RequestChanges"),
            ("RequestChanges", "Approved"),
        ] {
            let root = tempfile::tempdir().unwrap();
            write_separate_verdicts(root.path(), pr, spec);
            let state =
                probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snapshot());
            assert!(matches!(
                crate::integrate::classify_integration_action(&state),
                crate::integrate::IntegrationAction::Park(
                    crate::integrate::ParkReason::ReviewIntegrity
                )
            ));
        }
    }

    #[test]
    fn integrate_probe_parks_unattributed_retained_round() {
        let root = tempfile::tempdir().unwrap();
        let path = verdict_path(root.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            r#"{"verdict":"Approved","reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a","recorded_by":"reviewer-a","rounds":[{"verdict":"RequestChanges","reviewed_sha":"ac772eaca9"}]}"#,
        )
        .unwrap();
        let state =
            probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snapshot());
        assert!(matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Park(
                crate::integrate::ParkReason::ReviewIntegrity
            )
        ));
    }

    #[test]
    fn integrate_probe_ignores_stale_opposition_in_separate_artifact() {
        let root = tempfile::tempdir().unwrap();
        write_separate_verdicts(root.path(), "Approved", "RequestChanges");
        let spec = root.path().join(".aida/review-verdicts/BUG-1581.json");
        let stale = std::fs::read_to_string(&spec).unwrap().replace(
            "ac772eaca9d389fa762a232156df996023bfdf7a",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        std::fs::write(spec, stale).unwrap();
        let state =
            probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snapshot());
        assert!(matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Merge
        ));
    }

    #[test]
    fn integrate_probe_parks_malformed_retained_round() {
        let root = tempfile::tempdir().unwrap();
        let path = verdict_path(root.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            r#"{"verdict":"Approved","reviewed_sha":"ac772eaca9d389fa762a232156df996023bfdf7a","recorded_by":"reviewer-a","rounds":["lost evidence"]}"#,
        )
        .unwrap();
        let state =
            probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snapshot());
        assert!(matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Park(
                crate::integrate::ParkReason::ReviewIntegrity
            )
        ));
    }

    // Exercise the integrate probe and decision with the same append order as
    // merge/train gates, even when the retained act has a future timestamp.
    // trace:BUG-1581 trace:TASK-2964 | ai:codex
    #[test]
    fn integrate_probe_follows_same_artifact_append_order() {
        for (current, retained, blocks) in [
            ("Approved", "RequestChanges", false),
            ("Approved", "Rejected", false),
            ("RequestChanges", "Approved", true),
            ("Rejected", "Approved", true),
            ("Waiting", "Approved", false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = verdict_path(root.path());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let body = serde_json::json!({
                "verdict": current,
                "reviewed_sha": "ac772eaca9d389fa762a232156df996023bfdf7a",
                "recorded_by": "reviewer-a",
                "recorded_at": "2026-01-01T12:01:00Z",
                "rounds": [{
                    "verdict": retained,
                    "reviewed_sha": "ac772eaca9",
                    "recorded_by": "reviewer-b",
                    "recorded_at": "2099-01-01T00:00:00Z"
                }]
            });
            std::fs::write(path, body.to_string()).unwrap();
            let state =
                probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snapshot());
            assert!(!state.review_integrity_unproven);
            assert_eq!(state.request_changes_pending, blocks);
            assert_eq!(state.review_waiting, current == "Waiting");
            let expected = if current == "Waiting" {
                crate::integrate::IntegrationAction::Park(
                    crate::integrate::ParkReason::ReviewWaiting,
                )
            } else if blocks {
                crate::integrate::IntegrationAction::Park(
                    crate::integrate::ParkReason::RequestChanges,
                )
            } else {
                crate::integrate::IntegrationAction::Merge
            };
            assert_eq!(
                crate::integrate::classify_integration_action(&state),
                expected,
                "{current} after retained {retained}"
            );
        }
    }

    // An artifact that exists but cannot be interpreted is not evidence of
    // approval. The integration preflight must park it rather than degrade to
    // the optimistic no-local-verdict state.
    // trace:BUG-1581 | ai:codex
    #[test]
    fn integrate_probe_parks_an_unreadable_local_verdict() {
        let root = tempfile::tempdir().unwrap();
        let path = verdict_path(root.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "not json").unwrap();

        let state =
            probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snapshot());
        assert!(matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Park(
                crate::integrate::ParkReason::ReviewIntegrity
            )
        ));
    }

    // trace:BUG-2492 | ai:codex
    #[test]
    fn bug2492_integrate_parks_pending_and_malformed_hold_status() {
        let root = tempfile::tempdir().unwrap();
        for status in ["PENDING", "FAILURE", "ERROR"] {
            let mut snap = snapshot();
            let rows = vec![serde_json::json!({
                "__typename":"StatusContext", "context":"merge-hold-gate", "state":status
            })];
            let item = snap.by_branch.get_mut("bug-1581").unwrap();
            item.ci_rollup = Some(crate::status_prs::summarize_status_check_rollup(&rows));
            item.labels = vec!["aida:merge-hold-recorded".into()];
            let state =
                probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snap);
            assert!(!matches!(
                crate::integrate::classify_integration_action(&state),
                crate::integrate::IntegrationAction::Merge
            ));
            assert_eq!(state.ci, crate::integrate::CiState::Failing);
            assert!(matches!(
                crate::integrate::classify_integration_action(&state),
                crate::integrate::IntegrationAction::Park(_)
            ));
        }
    }

    // BUG-1481: a `ci_rollup` of "missing" (a required check's row never
    // showed up on this head) must never be probed into `CiState::None` and
    // never classify as Merge — that is the exact PR-2009 false-green shape
    // through `aida integrate`.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn integrate_probe_never_merges_on_missing_required_check() {
        let root = tempfile::tempdir().unwrap();
        let mut snap = snapshot();
        snap.by_branch.get_mut("bug-1581").unwrap().ci_rollup = Some("missing".into());
        let state = probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snap);
        assert_eq!(state.ci, crate::integrate::CiState::RequiredCheckMissing);
        assert!(!matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Merge
        ));
        assert!(matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Park(
                crate::integrate::ParkReason::RequiredCheckMissing
            )
        ));
    }

    // BUG-1481: a `ci_rollup` of "unknown" (the required-check set itself
    // couldn't be read — branch protection unreadable) must never be probed
    // into `CiState::None` and never classify as Merge.
    // trace:BUG-1481 | ai:claude
    #[test]
    fn integrate_probe_never_merges_on_unknown_required_checks() {
        let root = tempfile::tempdir().unwrap();
        let mut snap = snapshot();
        snap.by_branch.get_mut("bug-1581").unwrap().ci_rollup = Some("unknown".into());
        let state = probe_pr_integration_state(root.path(), "BUG-1581", Some("bug-1581"), &snap);
        assert_eq!(state.ci, crate::integrate::CiState::Indeterminate);
        assert!(!matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Merge
        ));
        assert!(matches!(
            crate::integrate::classify_integration_action(&state),
            crate::integrate::IntegrationAction::Park(
                crate::integrate::ParkReason::RequiredCheckMissing
            )
        ));
    }
}

pub(crate) fn ci_state_from_terminal_probe(probe: &CiProbe) -> integrate::CiState {
    match probe {
        CiProbe::Green { .. } => integrate::CiState::Passing,
        CiProbe::Red { .. } => integrate::CiState::Failing,
        CiProbe::PrNoChecks { .. } | CiProbe::NoSignal(_) => integrate::CiState::None,
        CiProbe::InProgress { .. } => integrate::CiState::Running,
    }
}

pub(crate) fn wait_for_integrate_ci(
    project_root: &std::path::Path,
    spec_id: &str,
    branch: Option<&str>,
    mut state: integrate::PrIntegrationState,
) -> integrate::PrIntegrationState {
    let Some(branch) = branch else {
        println!(
            "  {} {} — CI still running but no PR branch resolved; skipping this pass",
            crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
            spec_id
        );
        return state;
    };
    println!(
        "  {} {} — CI still running on `{branch}`; waiting for terminal verdict…",
        crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
        spec_id
    );
    // trace:BUG-1052 | ai:codex
    state.ci = ci_state_from_terminal_probe(&wait_for_ci_terminal(Some(project_root), branch));
    state
}

/// STORY-520: `aida queue integrate` — the thin integrator watch-loop.
///
/// The consumer half of a producer/consumer split: parallel implementers
/// produce PRs + flip specs to Done; this single serial loop consumes that
/// signal (Done + open PR) and drives the back-end merge phases on each in turn.
///
/// A FRONT-END over existing primitives, not new mechanism: the ready-set query
/// reuses the store load + the forge PR/merged probes (`probe_resume_facts`),
/// the membership decision is the pure [`integrate::classify_candidate`], and
/// each ready spec is driven by shelling out to the SAME TASK-405 PR-only path
/// (`aida queue work <id> --auto-complete --from-pr`) the resume/recover flows
/// use. The serial-merge invariant (one merge authority over `main`) is
/// preserved by driving each spec to completion before the next.
/// trace:STORY-520 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
/// TASK-1036: build the focus subtree as a set of display SPEC-IDs — the focus
/// root (`focus_ref`, a spec/agreed/uuid form) plus its transitive descendants
/// (the cache's `descendant_ids` closure, TASK-955, which INCLUDES the root),
/// mapped to the same display-id form the integrator keys candidates on. Used to
/// scope the integrator `--watch` candidate scan and the event-wake filter.
///
/// Best-effort: an unresolvable store / focus spec / cache error yields `None`,
/// which the caller treats as "no scoping" (whole-project scan) rather than
/// blocking — a focus that no longer resolves must never wedge the integrator.
// trace:TASK-1036 | ai:claude
pub(crate) fn build_focus_display_subtree(
    project_root: &std::path::Path,
    focus_ref: &str,
    store: &aida_core::RequirementsStore,
) -> Option<std::collections::HashSet<String>> {
    let store_path = detect_distributed_store_from(project_root)?;
    let backend = advance_backend(&store_path).ok()?;
    let focus_req = backend
        .get_requirement_by_spec_id(focus_ref)
        .ok()
        .flatten()?;
    let uuid_subtree = backend.descendant_ids(&focus_req.id).ok()?;
    let display: std::collections::HashSet<String> = store
        .requirements
        .iter()
        .filter(|r| uuid_subtree.contains(&r.id))
        .map(|r| r.display_id())
        .collect();
    Some(display)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_queue_integrate(
    storage: &Storage,
    _user_id: &str,
    dry_run: bool,
    watch: bool,
    interval: u64,
    max: usize,
    wait_ci: bool,
    rebase: bool,
    strategy: Option<integrate::IntegrateStrategy>,
    // TASK-1036: scope the candidate scan to a focus subtree. `--focus <id>`
    // overrides; else the per-worktree `aida focus` marker / `AIDA_FOCUS`.
    focus_override: Option<String>,
    // TASK-1036: idle backstop for the event-driven `--watch` loop, in minutes.
    // `None` falls back to `--interval` seconds (the old blind-timer cadence).
    idle_minutes: Option<u64>,
) -> Result<()> {
    let project_root = find_project_root()?;

    // TASK-812: take the global drain lock so an integrate run (especially
    // `--watch`, which a solo operator leaves running) can't double-drive the
    // tree against a `burndown run` / `queue work --auto-complete` / a second
    // `integrate` — they ALL merge into the shared default branch, so there can
    // be only one merge authority at a time. Held for the whole run, including
    // the `--watch` loop; a crashed watcher's lock is stale-reclaimed by the
    // next launch (BUG-538). `--dry-run` drives nothing, so it skips the lock.
    // The guard frees on return (this fn returns Result, never `process::exit`).
    // trace:TASK-812 | ai:claude
    let _drain_guard = if dry_run {
        None
    } else {
        // Linked worktrees share the main checkout's merge authority.
        // trace:BUG-1683 | ai:codex
        Some(drain_lock::acquire_drain_lock(
            &find_main_worktree_root()?,
            "queue integrate",
        )?)
    };

    // TASK-691: resolve the accumulation strategy — the --strategy flag wins,
    // else the project default (`[integrate] strategy` in .aida/config.toml),
    // else `per-item`. trace:TASK-691 | ai:claude
    let strategy = strategy
        .or_else(|| {
            std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
                .ok()
                .and_then(|c| integrate::integrate_strategy_from_config(&c))
        })
        .unwrap_or(integrate::IntegrateStrategy::PerItem);

    // STORY-335: only `per-item` is built; refuse `one-branch`/`stacked` cleanly
    // (with a pointer) before doing any probing or acting. trace:STORY-335
    if let Some(msg) = integrate::strategy_unsupported_message(strategy) {
        anyhow::bail!(msg);
    }

    let aida = crate::aida_exe_path();

    // TASK-1036: resolve the active focus once — `--focus <id>` wins, else the
    // per-worktree `aida focus` marker / `AIDA_FOCUS` (STORY-706). The per-pass
    // subtree is re-resolved inside the loop (cache-backed, cheap) so newly-filed
    // descendants are picked up. trace:TASK-1036 | ai:claude
    let active_focus = focus_override.or_else(|| crate::focus::resolve_focus(&project_root));

    // TASK-1036: in `--watch`, wake on real drain events instead of a blind
    // timer. Seed the event cursor at the stream's CURRENT end so a stale backlog
    // from a prior drain never re-fires an old wake (the advisor_watch precedent).
    // The idle backstop is the documented timer fallback: `--idle-minutes` if
    // given, else the legacy `--interval` seconds. trace:TASK-1036 | ai:claude
    let events_path = crate::events::events_path(&project_root);
    let mut event_offset: u64 = std::fs::metadata(&events_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let idle_backstop_secs = idle_minutes
        .map(|m| m.saturating_mul(60))
        .unwrap_or(interval);

    // TASK-1050 / BUG-650 (slice 2b): give `aida integrate` its OWN checkout so
    // it never contends the advisor's harness-worktree lease — the gate that
    // unblocks UNATTENDED `--watch`. (1) acquire/reuse a dedicated warm-pool
    // worktree pinned to the default branch under a distinct integrator lease
    // scope; (2) refuse to drive from — or relocate the drives out of — a shared
    // checkout a live non-integrator session holds (option-c). The merge drives
    // below run IN this dedicated checkout. `--dry-run` drives nothing, so it
    // skips both. The global drain lock taken above is untouched: it stays the
    // one merge authority repo-wide. trace:TASK-1050 trace:BUG-650 | ai:claude
    let integrator_checkout: Option<std::path::PathBuf> = if dry_run {
        None
    } else {
        let dedicated = integrate_checkout::ensure_integrator_checkout(&project_root)?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
        integrate_checkout::guard_not_shared_checkout(&project_root, &cwd, &dedicated)?;
        Some(dedicated)
    };
    // The cwd for the spawned child drives: the dedicated integrator checkout
    // when we have one, else the project root (dry-run never spawns a real
    // drive). trace:TASK-1050 | ai:claude
    let drive_cwd: &std::path::Path = integrator_checkout
        .as_deref()
        .unwrap_or(project_root.as_path());
    let integrate_headless = std::env::var("AIDA_HEADLESS")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false);

    let mut integrated_total: usize = 0;
    let mut pass: usize = 0;
    // TASK-836: track whether any member was parked (shelvable scenario) or made
    // to wait (CI running) across the whole run, so the exit code mirrors the
    // resilient-drain contract: exit 2 when anything was parked/skipped so a
    // wrapping script triages instead of treating the run as a clean success.
    // trace:TASK-836 | ai:claude
    let mut any_parked_or_waited = false;

    loop {
        pass += 1;

        // --- Probe: build the candidate set from store status + forge facts. ---
        // Reuse `probe_resume_facts` (the same PR/merged probe `--from-pr` and
        // `--resume-drain` use) so the integrator sees exactly the reality the
        // drive path will. trace:STORY-520 | ai:claude
        let store = storage.load()?;

        // TASK-1036: re-resolve the focus subtree this pass (cache-backed, cheap)
        // — the display-id set of the focus root + its transitive descendants —
        // so an out-of-scope Done spec is never probed or driven. `None` = no
        // focus set → whole-project scan, exactly as before. trace:TASK-1036
        let focus_subtree: Option<std::collections::HashSet<String>> = active_focus
            .as_deref()
            .and_then(|focus_ref| build_focus_display_subtree(&project_root, focus_ref, &store));
        if watch {
            if let (Some(focus_ref), Some(subtree)) = (active_focus.as_deref(), &focus_subtree) {
                println!(
                    "  {} focus {} — scanning {} spec(s) in subtree",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    focus_ref,
                    subtree.len()
                );
            }
        }

        let mut candidates: Vec<integrate::IntegrationCandidate> = Vec::new();
        // STORY-335: keep each candidate's PR branch (for the dry-run forecast)
        // and PR number (for the --rebase step) per ready member. trace:STORY-335
        let mut branches: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let mut pr_numbers: std::collections::HashMap<String, Option<u32>> =
            std::collections::HashMap::new();
        for req in &store.requirements {
            if req.status != RequirementStatus::Done {
                continue;
            }
            let id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("?")
                .to_string();
            // TASK-1036: focus-scope the scan BEFORE the (forge) probe, so an
            // out-of-scope Done spec is never probed, reported, or driven. Pure
            // `integrate::in_focus_scope` over the display-id subtree set.
            // trace:TASK-1036 | ai:claude
            if let Some(subtree) = &focus_subtree {
                if !integrate::in_focus_scope(&id, subtree) {
                    continue;
                }
            }
            // Classify the PR lookup as conclusive-or-not BEFORE probing the
            // richer facts, so a flaky gh never gets read as "no PR".
            let lookup = detect_open_pr_for_spec_via_forge(&project_root, &id);
            let inconclusive = matches!(
                lookup,
                PrLookup::GhMissing | PrLookup::GhFailed(_) | PrLookup::GhUnreachable(_)
            );
            let (facts, branch, pr) = probe_resume_facts(&project_root, storage, &id, None);
            branches.insert(id.clone(), branch);
            pr_numbers.insert(id.clone(), pr);
            candidates.push(integrate::IntegrationCandidate {
                id,
                is_done: true,
                has_open_pr: pr.is_some(),
                pr_merged: facts.pr_merged,
                pr_lookup_inconclusive: inconclusive,
                // TASK-813: keystone work the integrator must NOT auto-merge.
                held_for_human: req.tags.iter().any(|t| {
                    let t = t.trim();
                    t.eq_ignore_ascii_case("supervised")
                        || t.eq_ignore_ascii_case("review:draft-only")
                }),
            });
        }

        let ready = integrate::ready_for_integration(&candidates);

        // TASK-836: one forge snapshot per pass (CI rollup + mergeable +
        // reviewDecision keyed by branch) feeds the pre-merge scenario gate
        // below. Empty when gh is missing/failing — the gate then degrades to
        // the optimistic "Merge" verdict, and the --from-pr drive re-gates the
        // irreversible step. trace:TASK-836 | ai:claude
        let mut pr_snapshot = if dry_run || !ready.is_empty() {
            collect_open_prs(&project_root)
        } else {
            OpenPrSnapshot::default()
        };

        // --- Report the pass. ---
        if watch {
            println!(
                "{} integrator pass {} — {} Done spec(s), {} ready for integration",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                pass,
                candidates.len(),
                ready.len()
            );
        } else {
            println!(
                "{} {} Done spec(s); {} ready for integration",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                candidates.len(),
                ready.len()
            );
        }
        for c in &candidates {
            match integrate::classify_candidate(c) {
                integrate::CandidateVerdict::Integrate => {
                    println!("  {} {} — open PR, ready to merge", "→".green(), c.id);
                }
                integrate::CandidateVerdict::SkipNoPr => {
                    println!("  {} {} — Done but no open PR (skip)", "·".dimmed(), c.id);
                }
                integrate::CandidateVerdict::SkipAlreadyMerged => {
                    println!(
                        "  {} {} — PR already merged; `aida pull` will promote it (skip)",
                        "·".dimmed(),
                        c.id
                    );
                }
                integrate::CandidateVerdict::SkipProbeInconclusive => {
                    println!(
                        "  {} {} — PR probe inconclusive (gh missing/auth/network); skipping, not guessing",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        c.id
                    );
                }
                integrate::CandidateVerdict::SkipHeldForHuman => {
                    println!(
                        "  {} {} — keystone (supervised / review:draft-only): parked for your review, not auto-merged",
                        "⏸".yellow(),
                        c.id
                    );
                }
                // Non-Done specs were filtered out before classification.
                integrate::CandidateVerdict::SkipNotDone => {}
            }
        }

        // --- Act: drive each ready spec, serially, through the PR-only path. ---
        let mut ready_ids: Vec<String> = ready.iter().map(|c| c.id.clone()).collect();

        // TASK-841: `--strategy stacked` — order + gate the ready set by the
        // recorded stack graph. A member stacked behind another still-open branch
        // is DEFERRED this pass (merging it now would drag the parent's
        // un-squashed commits in under the wrong PR); only the mergeable
        // bottom-of-stack layer is driven. Completion stays per-commit (each
        // member auto-bumps when its own commit lands), so no special barrier is
        // needed — just this ordering. The drive's `aida pull` then cascade-
        // rebases the next layer's worktree (STORY-248); a later `--watch` pass
        // (or re-run) picks it up. trace:TASK-841 | ai:claude
        if matches!(strategy, integrate::IntegrateStrategy::Stacked) && !ready_ids.is_empty() {
            let mut graph = stacks::load(&project_root);
            let default_short = detect_default_branch_ref(&project_root)
                .as_deref()
                .map(|s| s.strip_prefix("origin/").unwrap_or(s).to_string())
                .unwrap_or_else(|| "main".to_string());
            let plan =
                integrate::plan_stacked_integration(&ready_ids, &branches, &graph, &default_short);
            // TASK-1080: stack-aware promotion. A deferred member whose parent
            // branch is GONE on origin (squash-merged + deleted) is promoted in
            // place: its ORIGIN PR branch is rebased with the 3-arg
            // `git rebase --onto <default> <recorded fork SHA>` + force-pushed
            // with lease — composing the one `pr rebase` machinery (temp
            // worktree, BUG-640 patch-id guard, lease-anchored push) via
            // `--onto-parent` — then merged by the normal drive THIS pass. The
            // STORY-248 cascade only rebases the child's local worktree; this
            // closes the origin-PR half so a 2-deep stack drains without a
            // manual `/aida-rebase`. The decision per member is the pure
            // `integrate::classify_stacked_promotion`. trace:TASK-1080 | ai:claude
            let mut promoted: Vec<String> = Vec::new();
            let mut still_deferred = 0usize;
            for d in &plan.deferred {
                if let Some(parent_spec) = &d.blocked_on_spec {
                    // Parent is in THIS pass's ready set — it merges below; the
                    // next pass (or --watch wake) promotes this child.
                    println!(
                        "  {} {} — stacked behind {} (branch `{}`); deferring until it lands",
                        "⏸".yellow(),
                        d.id,
                        parent_spec,
                        d.blocked_on_branch
                    );
                    still_deferred += 1;
                    continue;
                }
                let child_branch = branches.get(&d.id).and_then(|b| b.clone());
                let recorded_sha = child_branch
                    .as_deref()
                    .and_then(|b| graph.get(b))
                    .map(|e| e.parent_branch_sha.clone());
                let parent_gone = remote_branch_gone(&project_root, &d.blocked_on_branch);
                let pr_num = pr_numbers.get(&d.id).and_then(|p| *p);

                // BUG-1574: refuse the rebase/force-push before it happens —
                // not after — when the child is keyboard-only (tag or
                // execution_mode) or outside the currently active batch's
                // declared member set. Fail-closed on a store-load failure
                // or malformed drain-state.json too.
                // trace:BUG-1574 | ai:claude
                if let Some(reason) = crate::unattended_git_mutation_refusal(
                    &project_root,
                    &d.id,
                    "rebase/force-push",
                    child_branch.as_deref(),
                ) {
                    println!("  {} {} — {}", "⏸".yellow(), d.id, reason);
                    if !dry_run {
                        if let Err(e) = shelve_spec_on_failure(
                            &project_root,
                            &d.id,
                            "integrate",
                            0,
                            "stacked-rebase-refused",
                            &reason,
                            "resolve by hand: `aida pr rebase <PR> --onto-parent <SHA>` from an \
                             interactive/guided session, or add the spec to the active batch, \
                             then re-run `aida queue integrate --strategy stacked`",
                        ) {
                            eprintln!("  {} could not park {}: {e}", "Note:".dimmed(), d.id);
                        }
                    }
                    still_deferred += 1;
                    continue;
                }
                match integrate::classify_stacked_promotion(
                    recorded_sha.as_deref(),
                    parent_gone,
                    pr_num.is_some(),
                ) {
                    integrate::StackedPromotion::Promote { parent_sha } => {
                        let pr = pr_num.expect("Promote implies a resolved PR number");
                        if dry_run {
                            println!(
                                "  {} {} — parent `{}` merged+deleted; [dry-run] would rebase PR #{} onto {} (stack-aware) and force-push-with-lease",
                                "↻".cyan(),
                                d.id,
                                d.blocked_on_branch,
                                pr,
                                default_short
                            );
                            promoted.push(d.id.clone());
                            continue;
                        }
                        println!(
                            "  {} {} — parent `{}` merged+deleted; promoting: stack-aware rebase of PR #{} onto {}…",
                            "↻".cyan(),
                            d.id,
                            d.blocked_on_branch,
                            pr,
                            default_short
                        );
                        let status = std::process::Command::new(&aida)
                            .current_dir(drive_cwd)
                            .args(integrate::promotion_rebase_args(pr, &parent_sha))
                            .status_retrying_etxtbsy();
                        match status {
                            Ok(s) if s.success() => {
                                // The child now forks straight from the default
                                // branch — drop its stack entry so no later pass
                                // or cascade replays the consumed fork record.
                                // Dependents (grandchildren) keep their own
                                // records: their recorded fork SHA is still an
                                // ancestor of THEIR untouched PR head, so their
                                // eventual promotion stays correct.
                                if let Some(b) = child_branch.as_deref() {
                                    stacks::remove(&mut graph, b);
                                    if let Err(e) = stacks::save(&project_root, &graph) {
                                        eprintln!(
                                            "  {} could not update .aida/stacks.json: {e}",
                                            "Note:".dimmed()
                                        );
                                    }
                                }
                                println!(
                                    "  {} {} promoted — PR #{} rebased + force-pushed; merging this pass",
                                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                                    d.id,
                                    pr
                                );
                                promoted.push(d.id.clone());
                            }
                            Ok(_) => {
                                // Shelvable failure — park + continue (resilient-
                                // drain contract). Never corrupts: `pr rebase`
                                // aborts a conflicted rebase and cleans its temp
                                // worktree; nothing was pushed.
                                println!(
                                    "  {} {} — stack-aware rebase failed (conflict?); parking and continuing",
                                    "⏸".yellow(),
                                    d.id
                                );
                                if let Err(e) = shelve_spec_on_failure(
                                    &project_root,
                                    &d.id,
                                    "integrate",
                                    0,
                                    "stacked-rebase",
                                    &format!(
                                        "stack-aware rebase of PR #{pr} onto {default_short} failed (likely a conflict replaying the stacked commits)"
                                    ),
                                    &format!(
                                        "resolve with `aida pr rebase {pr} --onto-parent {parent_sha}` (or add `--interactive`), then re-run `aida queue integrate --strategy stacked`"
                                    ),
                                ) {
                                    eprintln!(
                                        "  {} could not park {}: {e}",
                                        "Note:".dimmed(),
                                        d.id
                                    );
                                }
                                any_parked_or_waited = true; // trace:TASK-836
                            }
                            Err(e) => {
                                println!(
                                    "  {} {} — could not launch the stack-aware rebase ({e}); deferring",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                    d.id
                                );
                                still_deferred += 1;
                            }
                        }
                    }
                    integrate::StackedPromotion::ParentStillOpen => {
                        println!(
                            "  {} {} — stacked behind `{}` (parent not merged yet); deferring until it lands",
                            "⏸".yellow(),
                            d.id,
                            d.blocked_on_branch
                        );
                        still_deferred += 1;
                    }
                    integrate::StackedPromotion::ProbeInconclusive => {
                        println!(
                            "  {} {} — stacked behind `{}`; couldn't verify the parent branch on origin (offline?); deferring, not guessing",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            d.id,
                            d.blocked_on_branch
                        );
                        still_deferred += 1;
                    }
                    integrate::StackedPromotion::ChurnedNoEntry => {
                        // Churn guard: the cascade removed the stack entry after
                        // this pass's plan was computed — defer and let the next
                        // re-plan classify it (it will read as mergeable).
                        println!(
                            "  {} {} — its stack record vanished mid-pass; deferring (next pass re-plans)",
                            "·".dimmed(),
                            d.id
                        );
                        still_deferred += 1;
                    }
                    integrate::StackedPromotion::NoPrNumber => {
                        println!(
                            "  {} {} — parent `{}` merged but no PR number resolved; run `/aida-rebase` in its worktree, then re-run",
                            "⏸".yellow(),
                            d.id,
                            d.blocked_on_branch
                        );
                        still_deferred += 1;
                    }
                }
            }
            if still_deferred > 0 {
                // Mirror the resilient-drain contract: something was held back, so
                // the run exits 2 and a wrapping loop (or `--watch`) re-checks
                // after the bottom layer merges. trace:TASK-836 | ai:claude
                any_parked_or_waited = true;
            }
            ready_ids = plan.mergeable;
            ready_ids.extend(promoted.iter().cloned());
            // A promoted child's snapshot row predates the force-push — refresh
            // so the TASK-836 pre-merge gate judges the NEW head, not the stale
            // stacked one. trace:TASK-1080 | ai:claude
            if !promoted.is_empty() && !dry_run {
                pr_snapshot = collect_open_prs(&project_root);
            }
        }

        // STORY-335: dry-run rebase-conflict forecast. Each ready member's PR
        // branch is checked (read-only, via `git merge-tree`) against current
        // main — surfacing which WILL conflict before any merge is attempted.
        // Read-only first slice: the act path below is unchanged (no rebase step
        // yet). trace:STORY-335 | ai:claude
        if dry_run && !ready_ids.is_empty() {
            let base = detect_default_branch_ref(&project_root);
            let rows: Vec<integrate::ForecastRow> = ready_ids
                .iter()
                .map(|id| {
                    let forecast = match (&base, branches.get(id).and_then(|b| b.clone())) {
                        (Some(base_ref), Some(branch)) => {
                            forecast_rebase_onto(&project_root, base_ref, &branch)
                        }
                        (None, _) => integrate::RebaseForecast::Unknown(
                            "no default branch (origin/main) detected".to_string(),
                        ),
                        (_, None) => integrate::RebaseForecast::Unknown(
                            "no PR branch resolved for spec".to_string(),
                        ),
                    };
                    integrate::ForecastRow {
                        id: id.clone(),
                        forecast,
                    }
                })
                .collect();
            let base_label = base.as_deref().unwrap_or("main");
            println!(
                "\n{} Rebase forecast (each PR branch onto {}, in order):",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
                base_label
            );
            for r in &rows {
                match &r.forecast {
                    integrate::RebaseForecast::Clean => {
                        println!(
                            "    {} {} — clean",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            r.id
                        );
                    }
                    integrate::RebaseForecast::Conflict(files) => {
                        let detail = if files.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", files.join(", "))
                        };
                        println!(
                            "    {} {} — conflict{}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            r.id,
                            detail
                        );
                    }
                    integrate::RebaseForecast::Unknown(why) => {
                        println!("    {} {} — unknown ({})", "?".dimmed(), r.id, why);
                    }
                }
            }
            let s = integrate::summarize_forecast(&rows);
            if s.conflict > 0 {
                println!(
                    "  {} {} of {} will conflict — resolve {} first.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    s.conflict,
                    rows.len(),
                    s.conflicting_ids.join(", ")
                );
            } else if s.unknown > 0 {
                println!(
                    "  {} {} clean, {} indeterminate — re-check those before integrating.",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    s.clean,
                    s.unknown
                );
            } else {
                println!(
                    "  {} all {} forecast clean.",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                    s.clean
                );
            }
            println!(
                "  {} forecast checks each branch against current {} independently; a member that\n    only conflicts with an earlier un-landed batch member won't show here yet (follow-up).",
                "·".dimmed(),
                base_label
            );
        }

        // TASK-842: recognize the full set of specs each ready PR will complete,
        // and dedupe by PR number across the ready set so a multi-spec cluster PR
        // — surfaced once per member spec by the Done-spec scan — is driven ONCE,
        // not N times. The merge-trailer → auto-bump path still completes every
        // trailered spec; this is recognition + dedupe + reporting only. The
        // dedupe itself is the pure `integrate::dedupe_pr_completions` over
        // (pr_number, trailered-spec-ids) rows, so it's unit-tested independently.
        // trace:TASK-842 | ai:claude
        let completion_rows: Vec<(u64, Vec<String>)> = ready_ids
            .iter()
            .filter_map(|id| {
                let pr_num = pr_numbers.get(id).and_then(|p| *p)? as u64;
                // The PR's trailered spec set, from the snapshot title (AIDA PR
                // titles carry the `(SPEC-ID)` trailers); fall back to the spec
                // the scan keyed on when the title can't be read.
                let spec_ids = branches
                    .get(id)
                    .and_then(|b| b.as_deref())
                    .and_then(|b| pr_snapshot.by_branch.get(b))
                    .map(|it| extract_spec_ids_from_commit(&it.title))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| vec![id.clone()]);
                Some((pr_num, spec_ids))
            })
            .collect();
        let pr_completions = integrate::dedupe_pr_completions(&completion_rows);
        let multi_spec_pr: std::collections::HashMap<u64, integrate::PrCompletion> = pr_completions
            .into_iter()
            .filter(|c| c.spec_ids.len() > 1)
            .map(|c| (c.number, c))
            .collect();
        // Track which PR numbers we've already driven this pass — so a multi-spec
        // PR reached again via a sibling spec isn't re-driven. trace:TASK-842
        let mut integrated_pr_numbers: std::collections::HashSet<u32> =
            std::collections::HashSet::new();

        for id in &ready_ids {
            if max != 0 && integrated_total >= max {
                println!(
                    "{} reached --max {} this run; stopping.",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    max
                );
                return Ok(());
            }
            // TASK-836: pre-merge scenario gate. Probe the richer PR facts (CI
            // state, RequestChanges, mergeability) and decide handle-vs-park
            // BEFORE driving the merge. Reuses the resilient-drain
            // park-and-continue contract: a shelvable scenario (CI red,
            // RequestChanges, conflict) parks the spec with one legible line and
            // the loop continues; CI-running waits (re-decided next --watch
            // pass). trace:TASK-836 | ai:claude
            let branch = branches.get(id).and_then(|b| b.clone());
            let mut pr_state =
                probe_pr_integration_state(&project_root, id, branch.as_deref(), &pr_snapshot);
            // BUG-1052: `integrate --run` is itself the retry loop; when a
            // ready PR's CI is already in-flight, wait once under the standard
            // bounded CI watcher and re-classify instead of skipping the entire
            // pass. `--no-wait-ci` preserves the old skip behavior.
            if wait_ci
                && matches!(
                    integrate::classify_integration_action(&pr_state),
                    integrate::IntegrationAction::WaitCi
                )
            {
                pr_state = wait_for_integrate_ci(&project_root, id, branch.as_deref(), pr_state);
            }
            match integrate::classify_integration_action(&pr_state) {
                integrate::IntegrationAction::Park(reason) => {
                    println!("  {} {} — {}", "⏸".yellow(), id, reason.message());
                    any_parked_or_waited = true;
                    continue;
                }
                integrate::IntegrationAction::WaitCi => {
                    println!(
                        "  {} {} — CI still running; skipping this pass (will re-check)",
                        crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
                        id
                    );
                    any_parked_or_waited = true;
                    continue;
                }
                integrate::IntegrationAction::Merge => {}
            }

            // TASK-843: a spec may have MORE than one open PR (a reopened /
            // duplicate). Before driving, list every open PR referencing the spec
            // and apply the pure newest-canonical policy: pick the newest
            // mergeable PR, report the rest as ignored-this-pass, or PARK when
            // none are clean. When only one PR is found this is a no-op (the
            // common case). The forge list is best-effort; unknown enumeration
            // falls back to the existing single-PR drive path. trace:TASK-843 | ai:claude
            // Preserve the existing best-effort policy: an unknown enumeration
            // leaves the already-probed primary path in charge. Never describe
            // this as proof that there are no duplicates. trace:BUG-2400 | ai:codex
            let multi_prs = match all_open_prs_for_spec_via_forge(&project_root, id) {
                Ok(prs) => prs,
                Err(_) => {
                    println!(
                        "  {} {} — open PR enumeration incomplete; retaining primary PR path",
                        "·".dimmed(),
                        id
                    );
                    Vec::new()
                }
            };
            if multi_prs.len() > 1 {
                // Mergeability per candidate from the pass snapshot (CONFLICTING
                // → not mergeable; anything else admissible — the --from-pr drive
                // re-gates the irreversible merge).
                let candidates: Vec<integrate::PrCandidate> = multi_prs
                    .iter()
                    .map(|(number, branch)| {
                        let mergeable = pr_snapshot
                            .by_branch
                            .get(branch)
                            .and_then(|it| it.mergeable.as_deref())
                            .map(|m| !m.eq_ignore_ascii_case("CONFLICTING"))
                            .unwrap_or(true);
                        integrate::PrCandidate {
                            number: *number,
                            mergeable,
                        }
                    })
                    .collect();
                match integrate::select_canonical_pr(&candidates) {
                    integrate::CanonicalPrDecision::Park { candidates } => {
                        let list = candidates
                            .iter()
                            .map(|n| format!("#{n}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!(
                            "  {} {} — {} open PRs but none mergeable ({}); parked for triage",
                            "⏸".yellow(),
                            id,
                            candidates.len(),
                            list
                        );
                        any_parked_or_waited = true;
                        continue;
                    }
                    integrate::CanonicalPrDecision::Integrate { chosen, ignored } => {
                        if !ignored.is_empty() {
                            let list = ignored
                                .iter()
                                .map(|n| format!("#{n}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            println!(
                                "  {} {} — {} open PRs; integrating newest (#{}), ignoring this pass: {}",
                                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                                id,
                                ignored.len() + 1,
                                chosen,
                                list
                            );
                        }
                    }
                    // 0 or 1 candidate is handled by the len()>1 guard above.
                    integrate::CanonicalPrDecision::NoPr => {}
                }
            }

            // TASK-842: dedupe drive + emit the multi-spec completion line. The
            // recognition + dedupe were computed (purely) before the loop; here
            // we (a) skip re-driving a PR already driven this pass for a sibling
            // spec, and (b) emit one legible line naming every spec the merge
            // completes. trace:TASK-842 | ai:claude
            let Some(pr_num) = pr_numbers.get(id).and_then(|p| *p) else {
                println!(
                    "  {} `{}` — no PR number resolved; skipping",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    id
                );
                any_parked_or_waited = true;
                continue;
            };
            {
                if !integrated_pr_numbers.insert(pr_num) {
                    // Already driven this pass for a sibling spec — the
                    // merge-trailer → auto-bump completes this spec too; don't
                    // re-drive the same PR.
                    println!(
                        "  {} {} — same PR (#{}) already integrating this pass for a sibling spec (skip re-drive)",
                        "·".dimmed(),
                        id,
                        pr_num
                    );
                    continue;
                }
                if let Some(completion) = multi_spec_pr.get(&(pr_num as u64)) {
                    println!(
                        "  {} {}",
                        "↩".cyan(),
                        integrate::describe_pr_completion(completion)
                    );
                }
            }

            if dry_run {
                let rebase_note = if rebase { " (would rebase first)" } else { "" };
                println!(
                    "  {} [dry-run] would drive `{}` via `aida queue work PR-{} --auto-complete --from-pr`{}",
                    "→".dimmed(),
                    id,
                    pr_num,
                    rebase_note
                );
                continue;
            }

            // STORY-335: rebase this member's PR branch onto current main before
            // merging it. A deferred batch cuts every branch from the same stale
            // main, so without this they would merge un-rebased. Composes the
            // existing `aida pr rebase <N> --no-smoke` primitive (temp-worktree
            // rebase + force-push-with-lease). A rebase conflict/failure skips
            // this member and continues — punt-and-continue, like the resilient
            // drain; the slice-1 --dry-run forecast previews these. The local
            // smoke is skipped because the --from-pr drive below runs CI.
            // trace:STORY-335 | ai:claude
            if rebase {
                match pr_numbers.get(id).and_then(|p| *p) {
                    Some(pr) => {
                        println!(
                            "{} rebasing `{}` (PR #{}) onto current main…",
                            "↻".cyan(),
                            id,
                            pr
                        );
                        // TASK-1050/BUG-650: rebase in the integrator's own
                        // checkout, not the (possibly advisor-leased) launch
                        // worktree. trace:TASK-1050 | ai:claude
                        let rb = std::process::Command::new(&aida)
                            .current_dir(drive_cwd)
                            .args(build_integrate_rebase_args(pr))
                            .status_retrying_etxtbsy();
                        match rb {
                            Ok(s) if s.success() => {
                                println!(
                                    "  {} `{}` rebased onto main",
                                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                                    id
                                );
                            }
                            Ok(_) => {
                                println!(
                                    "  {} `{}` rebase failed (conflict?) — skipping; resolve with `aida pr rebase {}` then re-run",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                    id,
                                    pr
                                );
                                any_parked_or_waited = true; // trace:TASK-836
                                continue;
                            }
                            Err(e) => {
                                println!(
                                    "  {} `{}` rebase could not run ({e}) — skipping",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                    id
                                );
                                any_parked_or_waited = true; // trace:TASK-836
                                continue;
                            }
                        }
                    }
                    None => {
                        // Defensive: ready members have an open PR, so a number
                        // should resolve. If it somehow doesn't, integrate as-is
                        // rather than skip — the --from-pr drive still gates on CI.
                        println!(
                            "  {} `{}` — --rebase set but no PR number resolved; integrating without rebase",
                            "·".dimmed(),
                            id
                        );
                    }
                }
            }

            println!(
                "{} integrating `{}` (reviewer → CI → merge → pull → build)…",
                "↩".cyan().bold(),
                id
            );
            // TASK-1050/BUG-650: drive in the integrator's OWN checkout so the
            // merge work never sits in (or lease-contends) a live session's
            // worktree. Because the drive runs in a linked worktree, its
            // `find_main_worktree_root()` drain lock targets MAIN — the very lock
            // THIS integrate run already holds. Authorize the delegated sub-drive
            // past that self-conflict with the force escape (env-scoped to the
            // child only); a genuinely independent drain never inherits it and is
            // still refused, so the repo-wide one-authority invariant holds.
            // trace:TASK-1050 trace:BUG-650 | ai:claude
            // ONE per-spec orchestration engine (ADR-7): integrate hands the
            // spec to the SAME `--auto-complete` engine, entering at the
            // reviewer phase via `--from-pr`. The routing argv is a pure helper
            // so the `orchestration_routing` guardrail can assert it.
            // trace:ADR-7 trace:ADR-9 | ai:claude
            let status = std::process::Command::new(&aida)
                .current_dir(drive_cwd)
                .env("AIDA_DRAIN_FORCE", "1")
                // BUG-748: this is an internal child drive launched under the
                // integrator's existing drain lock. Borrow that parent lock so
                // the child does not overwrite and release `.aida/drain.lock`
                // before the parent loop finishes. trace:BUG-748 | ai:codex
                .env("AIDA_DRAIN_BORROW", "1")
                .args(integrate::drive_args(pr_num, integrate_headless))
                .status_retrying_etxtbsy();
            match status {
                Ok(s) if s.success() => {
                    integrated_total += 1;
                    println!(
                        "  {} `{}` integrated",
                        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                        id
                    );
                }
                Ok(s) => {
                    // A non-zero exit means the drive shelved/refused this spec
                    // (CI red, RequestChanges, already-merged race, …). Leave it
                    // visible and move on — the serial loop must not stall on one
                    // spec. trace:STORY-520 | ai:claude
                    eprintln!(
                        "  {} `{}` did not integrate cleanly (exit {}); leaving for triage and continuing",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        id,
                        s.code().unwrap_or(-1)
                    );
                    any_parked_or_waited = true; // trace:TASK-836
                }
                Err(e) => {
                    eprintln!(
                        "  {} failed to launch the drive for `{}`: {}",
                        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                        id,
                        e
                    );
                    any_parked_or_waited = true; // trace:TASK-836
                }
            }
        }

        if !watch {
            break;
        }
        if max != 0 && integrated_total >= max {
            break;
        }
        // TASK-1036: event-driven wake. Instead of a blind `interval` sleep, BLOCK
        // until a focus-relevant actionable drain event lands in
        // `.aida/events.jsonl` (PhaseDonePr / CiTerminal / PrMerged / SpecShelved
        // — the same `events::is_actionable` taxonomy `aida watch` and the advisor
        // loop use), the idle backstop elapses (the old blind timer, now the
        // documented fallback — exactly like `advisor_watch::plan_watch_tick`'s
        // cadence path), or a live drain we were following stops. The store reload
        // at the top of the loop then picks up whatever shipped. trace:TASK-1036
        match event_wait::wait_for_actionable(
            &project_root,
            &mut event_offset,
            idle_backstop_secs,
            focus_subtree.as_ref(),
        ) {
            event_wait::WakeReason::Event(kind) => {
                let label = match &kind {
                    crate::events::EventKind::PhaseDonePr { .. } => "a PR is ready",
                    crate::events::EventKind::CiTerminal { .. } => "CI reached a verdict",
                    crate::events::EventKind::PrMerged { .. } => "a PR merged",
                    crate::events::EventKind::SpecShelved { .. } => "a spec shelved",
                    _ => "a drain event",
                };
                println!(
                    "  {} woke on {} — rescanning",
                    crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                    label
                );
            }
            event_wait::WakeReason::DrainCrashed => {
                println!(
                    "  {} the drain we were following stopped streaming events — rescanning",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            event_wait::WakeReason::IdleBackstop => {
                // Quiet idle re-scan — same cadence the old blind timer gave.
            }
        }
    }

    // TASK-836: mirror the resilient-drain exit-code contract — exit 2 when any
    // member was parked (CI red / RequestChanges / conflict / rebase-skip /
    // drive-refused) or made to wait (CI running), so a wrapping script (the
    // solo loop) treats the run as "did its job but triage pending" rather than
    // a clean success. Dry-run never mutates, so it always exits 0. The drain
    // guard is RAII (Drop), and process::exit skips destructors — so drop it
    // explicitly first to release the lock cleanly. trace:TASK-836 | ai:claude
    if !dry_run && any_parked_or_waited {
        println!(
            "{} integration left member(s) parked/waiting — triage with `aida findings list`, then re-run.",
            crate::glyph(crate::glyphs::Glyph::Arrow).yellow().bold()
        );
        drop(_drain_guard);
        std::process::exit(2);
    }

    Ok(())
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        QueueCommand::Integrate {
            dry_run,
            once: _,
            watch,
            interval,
            max,
            wait_ci: _,
            no_wait_ci,
            rebase,
            strategy,
            focus,
            idle_minutes,
            force,
            user,
        } => {
            // STORY-647: team RBAC guardrail — integrating ready PRs into the
            // default branch is an advisor-gated op by default (tunable via
            // `[team.permissions] integrate`). Checked before any work, including
            // dry-run, so the gate is what the harness probes. `--force` /
            // advisor authority bypass. trace:STORY-647 | ai:claude
            enforce_team_gate(permissions::GatedOp::Integrate, *force)?;
            let user_id = current_user_id(user.as_deref());
            handle_queue_integrate(
                storage,
                &user_id,
                *dry_run,
                *watch,
                *interval,
                *max,
                !*no_wait_ci,
                *rebase,
                *strategy,
                focus.clone(),
                *idle_minutes,
            )?;
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
