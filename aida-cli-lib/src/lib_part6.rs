#[cfg(test)]
#[path = "tests/queue_work_resume_tests.rs"]
mod queue_work_resume_tests;

#[cfg(test)]
#[path = "tests/drain_reliability_wiring_tests.rs"]
mod drain_reliability_wiring_tests;

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

#[cfg(test)]
#[path = "tests/story_255_discipline_pack_tests.rs"]
mod story_255_discipline_pack_tests;

#[cfg(test)]
#[path = "tests/story_781_project_manifest_tests.rs"]
mod story_781_project_manifest_tests;

/// STORY-127: unit tests for the Scope-B runtime anti-pattern detector
/// predicates. Each predicate is pure over its inputs so the warning
/// conditions are exercised without git/forge/queue I/O.
/// trace:STORY-127 | ai:claude
#[cfg(test)]
#[path = "tests/story_127_antipattern_detectors.rs"]
mod story_127_antipattern_detectors;

// trace:TASK-0414 — opt-in statusline bootstrap.
#[cfg(test)]
#[path = "tests/task_0414_statusline_setup.rs"]
mod task_0414_statusline_setup;

/// trace:STORY-582 | ai:claude — durable processing-record audit trail.
#[cfg(test)]
#[path = "tests/story_582_processing_record_tests.rs"]
mod story_582_processing_record_tests;

// BUG-618: lock the `aida queue list --json` shape — the TUI cockpit panel
// parses {spec_id,title,status,for_role}, so the cache-summary resolution must
// keep emitting exactly those keys in queue order. trace:BUG-618 | ai:claude
#[cfg(test)]
#[path = "tests/queue_json_rows_tests.rs"]
mod queue_json_rows_tests;

#[cfg(test)]
#[path = "tests/phase_heartbeat_tests.rs"]
mod phase_heartbeat_tests;

// trace:STORY-716 | ai:claude
#[cfg(test)]
#[path = "tests/worktree_handler_tests.rs"]
mod worktree_handler_tests;

// trace:STORY-725 | ai:claude — the THOUGHT → spec front door + zen help cleanup.
#[cfg(test)]
#[path = "tests/zen_front_door_tests.rs"]
mod zen_front_door_tests;

// TASK-1063: the three dead-queue-pruning verbs (`queue prune --orphaned`,
// `queue prune --merged`, `queue gc`) must cross-reference one another in
// their --help so an operator can pick the right one without reading all
// three pages — and `queue prune` with no predicate must name every verb.
// trace:TASK-1063 | ai:claude
#[cfg(test)]
#[path = "tests/task_1063_prune_verb_crossref_tests.rs"]
mod task_1063_prune_verb_crossref_tests;

// TASK-1056: the batched git-fanout collapse for `aida status --full`. These
// tests pin the substitution to OUTPUT EQUIVALENCE — each batched helper must
// return exactly the data the per-branch git loop it replaced produced, so the
// rendered status is byte-identical while the process count drops from O(local
// branches) to one `git for-each-ref`. trace:TASK-1056 | ai:claude
#[cfg(test)]
#[path = "tests/task_1056_batched_git_fanout_tests.rs"]
mod task_1056_batched_git_fanout_tests;

// STORY-698: verification-step capture at `aida queue done`. The precedence
// decision is factored into `decide_test_plan_capture` precisely so it can be
// exercised without a TTY. trace:STORY-698 | ai:claude
#[cfg(test)]
#[path = "tests/story_698_test_plan_capture_tests.rs"]
mod story_698_test_plan_capture_tests;

// BUG-800: review worksheets must name runnable focused-test commands instead
// of bare test identifiers, so Codex does not pass the name to the wrong
// binary. trace:BUG-800 | ai:codex
#[cfg(test)]
#[path = "tests/bug_800_review_test_command_prompt_tests.rs"]
mod bug_800_review_test_command_prompt_tests;

// STORY-1350: round-one review compares the recorded implementation intent
// with the resulting diff, using the pickup heading as a stable marker.
#[cfg(test)]
#[path = "tests/story_1350_review_approach_tests.rs"]
mod story_1350_review_approach_tests;

// BUG-1434: the review prompt states how far a PR's head trails the
// default branch, computed locally, never rendering "unknown" as 0.
#[cfg(test)]
#[path = "tests/bug_1434_stale_base_tests.rs"]
mod bug_1434_stale_base_tests;

// STORY-790 review findings: drift brief then-vs-now + mail/briefs since exit.
// trace:STORY-790 | ai:claude
#[cfg(test)]
#[path = "tests/story_790_drift_brief_tests.rs"]
mod story_790_drift_brief_tests;

// STORY-780: init-from-nothing bootstrap — the ordered sequence, its
// refusals, idempotent resume, and the forge repo-create argv.
// trace:STORY-780 | ai:claude
#[cfg(test)]
#[path = "tests/story_780_init_bootstrap_tests.rs"]
mod story_780_init_bootstrap_tests;

// trace:STORY-827 | ai:codex
#[cfg(test)]
#[path = "tests/story_827_init_readme_registry_tests.rs"]
mod story_827_init_readme_registry_tests;

// BUG-777: recovering a lease left behind by an already-exited session — the
// verifiably-gone + clean-worktree reclaim, the never-reap-a-live-lease floor,
// and the clean/dirty reporting that makes the refusal evaluable.
// trace:BUG-777 | ai:claude
#[cfg(test)]
#[path = "tests/bug_777_stale_lease_recovery_tests.rs"]
mod bug_777_stale_lease_recovery_tests;

// TASK-1175: the burndown ready set orders on QUEUE-INSERTION time. These tests
// cover the impure halves — the per-user queue-file `added_at` join and the
// env → project → global → default resolution ladder for `[burndown] order`.
// trace:TASK-1175 | ai:claude
#[cfg(test)]
#[path = "tests/task_1175_queue_insertion_order_tests.rs"]
mod task_1175_queue_insertion_order_tests;

// trace:TASK-1265 | ai:codex
#[cfg(test)]
#[path = "tests/task_1265_rework_no_op_tests.rs"]
mod task_1265_rework_no_op_tests;

// BUG-1295: the orchestrator's phase-3 auto-rebase failure classification
// must ride on the `aida pr rebase` subprocess's EXIT CODE, never on
// substring-matching its message text — the drift guard.
// trace:BUG-1295 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1295_rebase_exit_code_tests.rs"]
mod bug_1295_rebase_exit_code_tests;

// trace:ADR-55 | ai:antigravity
#[cfg(test)]
#[path = "tests/adr_55_evaluator_tests.rs"]
mod adr_55_evaluator_tests;

// trace:STORY-1426 | ai:antigravity
#[cfg(test)]
#[path = "tests/story_1426_contradictions_tests.rs"]
mod story_1426_contradictions_tests;

// trace:STORY-1424 | ai:antigravity
#[cfg(test)]
#[path = "tests/story_1424_graded_review_tests.rs"]
mod story_1424_graded_review_tests;

// trace:BUG-1668 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1668_trivial_criteria_tests.rs"]
mod bug_1668_trivial_criteria_tests;

// trace:BUG-1418 | ai:codex
#[cfg(test)]
#[path = "tests/bug_1418_drain_token_measurement_tests.rs"]
mod bug_1418_drain_token_measurement_tests;

// trace:BUG-1510 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1510_lease_brief_dispatch_tests.rs"]
mod bug_1510_lease_brief_dispatch_tests;
// trace:TASK-1442 | ai:claude
#[cfg(test)]
#[path = "tests/task_1442_pr_open_spec_guard_tests.rs"]
mod task_1442_pr_open_spec_guard_tests;

// trace:TASK-1444 | ai:claude
#[cfg(test)]
#[path = "tests/task_1444_shelve_attribution_tests.rs"]
mod task_1444_shelve_attribution_tests;

// trace:TASK-1445 | ai:claude
#[cfg(test)]
#[path = "tests/task_1445_pr_attribution_disagreement_tests.rs"]
mod task_1445_pr_attribution_disagreement_tests;

// trace:BUG-1486 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1486_stakeholder_db_verb_tests.rs"]
mod bug_1486_stakeholder_db_verb_tests;

// trace:EPIC-72 trace:TASK-1435 trace:TASK-1436 trace:TASK-1438 trace:TASK-1439 | ai:antigravity
#[cfg(test)]
#[path = "tests/epic_72_exposition_tests.rs"]
mod epic_72_exposition_tests;

// Git option injection through user-supplied refs, ranges and branches.
// trace:BUG-1622 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1622_git_option_injection_tests.rs"]
mod bug_1622_git_option_injection_tests;

// trace:BUG-1624 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1624_shell_and_git_guard_tests.rs"]
mod bug_1624_shell_and_git_guard_tests;

// trace:BUG-1627 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1627_session_env_hardening_tests.rs"]
mod bug_1627_session_env_hardening_tests;

// trace:BUG-1650 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1650_store_resolver_tests.rs"]
mod bug_1650_store_resolver_tests;

// Mutating callers re-validate against the current store, not a stale cache.
// trace:BUG-1670 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1670_stale_cache_callers_tests.rs"]
mod bug_1670_stale_cache_callers_tests;
#[cfg(test)]
#[path = "tests/bug_1799_carry_forward_tests.rs"]
mod bug_1799_carry_forward_tests;

pub mod worktree_dismiss;

#[cfg(test)]
#[path = "tests/bug_1800_worktree_undecidable_cache_tests.rs"]
mod bug_1800_worktree_undecidable_cache_tests;

// trace:BUG-1807 | ai:codex
#[cfg(test)]
mod bug_1807_handoff_tests {
    use super::*;

    #[test]
    fn only_confirmed_queue_ownership_allows_completion() {
        assert!(require_review_handoff(&AutoQueueOutcome::filed("queued MR-35")).is_ok());
        assert!(require_review_handoff(&AutoQueueOutcome::already_exists("queued MR-35")).is_ok());
        for outcome in [
            AutoQueueOutcome::skipped_needs_attention("MR-35 queue insertion failed"),
            AutoQueueOutcome::skipped_by_design("no open MR"),
        ] {
            assert!(require_review_handoff(&outcome).is_err());
        }
    }

    #[test]
    fn reviewer_pickup_commands_parse_for_both_forges() {
        for label in ["MR-35", "PR-35"] {
            assert!(
                Cli::try_parse_from(["aida", "queue", "work", label, "--role", "reviewer"]).is_ok()
            );
        }
    }

    #[test]
    fn configured_missing_canonical_store_never_uses_legacy_inventory() {
        let root = tempfile::tempdir().unwrap();
        Storage::new(root.path().join("requirements.yaml"))
            .save(&RequirementsStore::default())
            .unwrap();
        std::fs::create_dir(root.path().join(".aida")).unwrap();
        for config in [
            "mode = \"distributed\"\nstore_path = \".aida-store\"\n",
            "store_path = \".aida-store\"\n",
        ] {
            std::fs::write(root.path().join(".aida/config.toml"), config).unwrap();
            assert!(load_review_story_inventory(root.path()).is_err());
            assert!(!root.path().join(".aida-store").exists());
        }
        // A partial store must not become an empty inventory or get repaired
        // as a side effect of this read gate.
        std::fs::create_dir(root.path().join(".aida-store")).unwrap();
        assert!(load_review_story_inventory(root.path()).is_err());
        assert!(!root.path().join(".aida-store/objects").exists());
    }

    #[test]
    fn existing_mr35_story_is_recognized_without_github_collision() {
        let mut store = RequirementsStore::default();
        for (id, title) in [
            ("STORY-1", "Review PR-35: unrelated"),
            ("STORY-2", "Review MR-35: fixture"),
        ] {
            let mut req = aida_core::Requirement::new(title.into(), String::new());
            req.spec_id = Some(id.into());
            req.status = RequirementStatus::Approved;
            store.requirements.push(req);
        }
        // Legacy lookup must read this project's existing story, too.
        let root = tempfile::tempdir().unwrap();
        Storage::new(root.path().join("requirements.yaml"))
            .save(&store)
            .unwrap();
        let store = load_review_story_inventory(root.path()).expect("legacy store");
        assert_eq!(
            canonical_review_story(&store, ReviewForge::GitLab, 35, None, None)
                .unwrap()
                .display_id(),
            "STORY-2"
        );
        assert_eq!(
            canonical_review_story(&store, ReviewForge::GitHub, 35, None, None)
                .unwrap()
                .display_id(),
            "STORY-1"
        );
        assert!(canonical_review_story(&store, ReviewForge::GitLab, 3, None, None).is_none());
    }
}
