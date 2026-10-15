//! Drain merge-approval gate, PR review-verdict stamping and spec-done-after-PR (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Accept a reconciliation-time CI result only when it is terminal green and
/// belongs to the exact head the PR/MR metadata reports. The error deliberately
/// names both heads so stale and incomplete forge evidence is diagnosable.
// trace:BUG-1819 | ai:codex
pub(crate) fn verified_from_pr_ci_head(
    pr: u32,
    expected_head: Option<&str>,
    evidence: &crate::forge::CiProbeEvidence,
) -> Result<String, auto_complete::PhaseFailure> {
    let expected = expected_head
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .unwrap_or("<unknown>");
    let observed = evidence
        .head_sha
        .as_deref()
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .unwrap_or("<unknown>");
    let observed_change = match &evidence.probe {
        crate::forge::CiProbeResult::Green { change } => Some(*change),
        _ => None,
    };
    if observed_change != Some(u64::from(pr)) {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "review of PR-{pr} refused: CI evidence is not terminal green for this change (observed change {}); expected head {expected}, observed head {observed}",
                observed_change
                    .map(|change| change.to_string())
                    .unwrap_or_else(|| "<nonterminal>".to_string())
            ),
        ));
    }
    if expected == "<unknown>"
        || observed == "<unknown>"
        || !expected.eq_ignore_ascii_case(observed)
    {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "review of PR-{pr} refused: CI evidence does not cover the current change; expected head {expected}, observed head {observed}"
            ),
        ));
    }
    Ok(observed.to_string())
}

#[cfg(test)]
mod bug_1819_from_pr_ci_evidence_tests {
    use super::*;

    const HEAD: &str = "a91957a858320c0e17f3a0eca7cfacbff50ea29a";
    const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn evidence(
        probe: crate::forge::CiProbeResult,
        head: Option<&str>,
    ) -> crate::forge::CiProbeEvidence {
        crate::forge::CiProbeEvidence {
            probe,
            head_sha: head.map(str::to_string),
        }
    }

    #[test]
    fn direct_reviewer_reentry_accepts_exact_green_head() {
        let verified = verified_from_pr_ci_head(
            26,
            Some(HEAD),
            &evidence(
                crate::forge::CiProbeResult::Green { change: 26 },
                Some(HEAD),
            ),
        )
        .expect("an exact terminal green head is reviewable");
        assert_eq!(verified, HEAD);

        let tmp = tempfile::TempDir::new().unwrap();
        let mut driver = RealPhaseDriver::new(
            tmp.path().to_path_buf(),
            "BUG-205".into(),
            "tester".into(),
            None,
            false,
            None,
            AutonomyMode::Default,
            "run-token".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::Full,
        );
        driver.seed_resume_state(
            Some("bug-205".into()),
            Some(26),
            None,
            Some(verified),
            Some(true),
        );
        assert_eq!(driver.ci_terminal_sha.as_deref(), Some(HEAD));
        assert_eq!(driver.ci_terminal_green, Some(true));
    }

    #[test]
    fn stale_evidence_names_expected_and_observed_heads() {
        let failure = verified_from_pr_ci_head(
            26,
            Some(HEAD),
            &evidence(crate::forge::CiProbeResult::Green { change: 26 }, Some(OLD)),
        )
        .expect_err("a green result for an old head must be rejected");
        assert!(failure.reason.contains(HEAD), "{}", failure.reason);
        assert!(failure.reason.contains(OLD), "{}", failure.reason);
    }

    #[test]
    fn nonterminal_evidence_names_expected_and_observed_heads() {
        let failure = verified_from_pr_ci_head(
            26,
            Some(HEAD),
            &evidence(
                crate::forge::CiProbeResult::InProgress { change: 26 },
                Some(HEAD),
            ),
        )
        .expect_err("pending CI must not authorize review");
        assert!(failure.reason.contains("not terminal green"));
        assert!(failure.reason.contains(HEAD), "{}", failure.reason);
    }
}

/// TASK-1449: like [`pr_head_sha_best_effort`], but the PR's head branch
/// name — used by the rework no-op guard to check whether a PR OTHER than
/// the one it armed against (the BUG-1527 shape) is actually attributed to
/// the spec before treating its existence as anything at all. `None` on any
/// forge fault (e.g. pure-git, which has no PR metadata to read).
// trace:TASK-1449 | ai:claude
pub(crate) fn pr_head_ref_best_effort(driver: &RealPhaseDriver, pr: u32) -> Option<String> {
    let mut sink = crate::network_retry::NoopSink;
    driver
        .lifecycle_forge()
        .change_metadata(pr as u64, &mut sink)
        .ok()
        .map(|m| m.head_ref)
        .filter(|s| !s.trim().is_empty())
}

/// TASK-1449 (BUG-1522 AC5/AC6; hardened on re-review): the DISPATCHED
/// branch's head as ORIGIN reports it, read locally rather than via the
/// forge. Fetches `origin/<branch>` first (best effort) so a stale
/// remote-tracking ref is never read as truth — this fn is the ONLY reader
/// for both the arm-time baseline and the post-round comparison, so a ref
/// that was stale at arm time gets refreshed at arm time too, instead of
/// only on the later read (which would manufacture a false "content
/// changed": stale local W at arm time, freshly-fetched real X after —
/// X looks new but was already the state on origin before this round ran).
/// Deliberately does NOT fall back to a same-named local branch: a
/// dispatched round's open PR lives on origin by definition, and a
/// same-named local branch could be unrelated leftover state. `None` —
/// UNKNOWN, fail-closed — when `origin/<branch>` cannot be read at all (no
/// origin, branch never pushed, a git error).
// trace:TASK-1449 | ai:claude
pub(crate) fn dispatched_branch_head_sha(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    let _ = fetch_branch(project_root, branch, true);
    git_rev_parse_quiet(project_root, &format!("origin/{branch}"))
}

/// TASK-1448: the drain merge phase's approval-covers-head gate. `Err` is a
/// shelvable, never-retried `StaleApproval` failure carrying the same
/// refusal text `aida pr ship` prints — the drain parks the spec and moves
/// on rather than exiting. Pure over the verdicts + head so every branch is
/// testable without a forge.
// trace:TASK-1448 | ai:claude
pub(crate) fn drain_merge_approval_gate(
    candidates: &[review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
    pr: u64,
) -> Result<(), auto_complete::PhaseFailure> {
    match pr_ship::approval_head_refusal(candidates, head_sha) {
        None => Ok(()),
        Some(refusal) => Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::StaleApproval,
            pr_ship::approval_head_refusal_message(pr, &refusal),
        )),
    }
}

/// TASK-1458: the `MergeOptions.match_head` pin for the drain's phase-4
/// merge — the approved commit once `drain_merge_approval_gate` has passed.
/// Kept beside the gate so the wiring test drives the same pair of calls the
/// real `merge()` makes.
// trace:TASK-1458 | ai:claude
pub(crate) fn drain_merge_match_head(
    candidates: &[review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
) -> Option<String> {
    pr_ship::approved_match_head(candidates, head_sha)
}

/// Add the orchestrator-owned review context to a reviewer-written PR verdict.
///
/// The reviewer owns the verdict, summary, findings, and any future fields;
/// `record_verdict` deliberately preserves those while adding the commit and
/// audit metadata only the drain can reliably know.
// trace:TASK-168 | ai:codex
pub(crate) fn stamp_pr_review_verdict(
    project_root: &std::path::Path,
    pr: u32,
    reviewed_sha: Option<&str>,
    reviewed_branch: Option<&str>,
    ci_terminal_sha: Option<&str>,
    ci_terminal_green: Option<bool>,
) -> std::io::Result<std::path::PathBuf> {
    let path = review_verdict::record_verdict(
        project_root,
        &format!("PR-{pr}"),
        None,
        reviewed_sha,
        reviewed_branch,
        None,
        &[],
        "aida drain reviewer",
    )?;
    if ci_terminal_sha.is_some() || ci_terminal_green.is_some() {
        let body = std::fs::read_to_string(&path)?;
        let mut value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let object = value.as_object_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "review verdict is not an object",
            )
        })?;
        if let Some(sha) = ci_terminal_sha {
            object.insert(
                "ci_terminal_sha".into(),
                serde_json::Value::String(sha.into()),
            );
        }
        if let Some(green) = ci_terminal_green {
            object.insert("ci_terminal_green".into(), serde_json::Value::Bool(green));
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    }
    Ok(path)
}

#[cfg(test)]
mod task_1448_drain_merge_approval_gate_tests {
    // trace:TASK-1448 | ai:claude
    use super::*;

    const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn approval(sha: Option<&str>) -> review_verdict::RecordedVerdict {
        review_verdict::RecordedVerdict {
            kind: review_verdict::VerdictKind::Approved,
            raw: "approved".into(),
            reviewed_sha: sha.map(str::to_string),
            ..Default::default()
        }
    }

    fn assert_shelves(result: Result<(), auto_complete::PhaseFailure>) -> String {
        let failure = result.expect_err("the merge phase must refuse");
        assert_eq!(failure.kind, auto_complete::FailureKind::StaleApproval);
        // Shelve-and-continue, never stop the drain; never a transient retry.
        assert!(failure.kind.is_shelvable());
        assert!(!auto_complete::should_retry_transient_failure(
            failure.kind,
            0,
            3
        ));
        assert!(
            failure.reason.contains("Re-review the current head"),
            "{}",
            failure.reason
        );
        failure.reason
    }

    #[test]
    fn approval_at_head_lets_the_drain_merge() {
        assert!(drain_merge_approval_gate(&[approval(Some(HEAD))], Some(HEAD), 5).is_ok());
    }

    #[test]
    fn approval_behind_head_shelves() {
        let reason = assert_shelves(drain_merge_approval_gate(
            &[approval(Some(OLD))],
            Some(HEAD),
            5,
        ));
        assert!(
            reason.contains(&OLD[..12]) && reason.contains(&HEAD[..12]),
            "{reason}"
        );
    }

    #[test]
    fn approval_without_sha_shelves() {
        assert_shelves(drain_merge_approval_gate(&[approval(None)], Some(HEAD), 5));
    }

    #[test]
    fn unreadable_head_shelves() {
        assert_shelves(drain_merge_approval_gate(&[approval(Some(HEAD))], None, 5));
    }

    #[test]
    fn stale_approval_recovery_hint_names_the_re_review() {
        let ctx = auto_complete::HintContext {
            spec: "TASK-1448".into(),
            pr_number: Some(5),
            ..Default::default()
        };
        let hint = auto_complete::recovery_hint(
            auto_complete::Phase::Merge,
            auto_complete::FailureKind::StaleApproval,
            &ctx,
        );
        assert!(hint.contains("Re-review the"), "{hint}");
        assert!(hint.contains("PR-5"), "{hint}");
        assert_eq!(
            auto_complete::FailureKind::StaleApproval.cause_slug(),
            "stale-approval"
        );
    }
}

#[cfg(test)]
mod task_168_pr_verdict_metadata_tests {
    use super::*;

    #[test]
    fn stamps_pr_verdict_metadata_without_losing_reviewer_fields() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = review_verdict::verdict_path(tmp.path(), "PR-1965");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"verdict":"Approved","summary":"clean","findings":["note"],"mode":"deep"}"#,
        )
        .unwrap();

        stamp_pr_review_verdict(
            tmp.path(),
            1965,
            Some("deadbeefdeadbeefdeadbeef"),
            Some("task-168-work"),
            Some("deadbeefdeadbeefdeadbeef"),
            Some(true),
        )
        .unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        // BUG-1505: the drain stamp canonicalizes the reviewer's spelling.
        assert_eq!(value["verdict"], "approved");
        assert_eq!(value["summary"], "clean");
        assert_eq!(value["findings"][0], "note");
        assert_eq!(value["mode"], "deep");
        assert_eq!(value["reviewed_sha"], "deadbeefdeadbeefdeadbeef");
        assert_eq!(value["reviewed_branch"], "task-168-work");
        assert_eq!(value["ci_terminal_sha"], "deadbeefdeadbeefdeadbeef");
        assert_eq!(value["ci_terminal_green"], true);
        assert_eq!(value["recorded_by"], "aida drain reviewer");
        assert!(value["recorded_at"].as_str().is_some());
    }
}

/// BUG-1186: `Some(message)` when the PR head moved between the pre- and
/// post-review probes — the reviewer seat wrote to the branch under review.
/// Pure so the decision is unit-testable; SHA comparison is whitespace- and
/// case-insensitive (forges differ in casing).
// trace:BUG-1186 | ai:claude
pub(crate) fn reviewer_wrote_message(pr: u32, pre: &str, post: &str) -> Option<String> {
    let pre = pre.trim().to_ascii_lowercase();
    let post = post.trim().to_ascii_lowercase();
    if pre.is_empty() || post.is_empty() || pre == post {
        return None;
    }
    let short = |s: &str| s.chars().take(9).collect::<String>();
    Some(format!(
        "the reviewer pushed to PR-{pr} during the review (head {} → {}) — a reviewer must \
         verdict, not implement; the CI the drain watched no longer covers this head",
        short(&pre),
        short(&post),
    ))
}

/// BUG-1186 / ADR-40: substrate-as-bouncer for the Done transition. The
/// implementer opened (or was handed) PR-{pr}; if it exited without `aida
/// queue done` the spec is still In Progress, and the review phase's `queue
/// work PR-N` would then see an in-flight spec carrying a rework reason — the
/// seat swap BUG-1186 reproduced. The orchestrator asserts Done itself rather
/// than relying on the agent remembering. Best-effort: a store fault prints a
/// note and the drive continues (the review envelope is the second guard).
///
/// BUG-1527: this write is itself a swap-acceptance hazard — `pr` may have
/// been found on a branch the implementer swapped to mid-phase (BUG-1485's
/// failure mode), which can be a DIFFERENT spec's PR entirely. Gate the flip
/// on the same TASK-1442 trailer-attribution check the PR-open recovery
/// paths already use (`ensure_pr_open_spec_attribution`) so a PR that
/// credits some other spec never marks THIS spec Done. The check runs
/// BEFORE the write, not after, so it can never contradict the BUG-245
/// `shipped_spec_id` mismatch report that follows phase 1 (that report is
/// what surfaces the swap as an anomaly; this function just goes quiet on a
/// mismatch instead of writing a false Done).
// trace:BUG-1186 trace:BUG-1527 | ai:claude
pub(crate) fn ensure_spec_done_after_pr(
    project_root: &std::path::Path,
    repo: &std::path::Path,
    branch: &str,
    spec: &str,
    pr: u32,
    json: bool,
) {
    // TASK-1457: this gate is intentionally coarse — a PR whose commits name
    // a different spec AND a PR whose commits carry no spec-ID trailer at
    // all both skip the Done write here (see the doc comment on
    // `ensure_pr_open_spec_attribution` for why this caller keeps the two
    // collapsed rather than adopting the finer three-way split the
    // branch-swap seam uses).
    if let Err(e) = ensure_pr_open_spec_attribution(repo, branch, spec) {
        if !json {
            eprintln!(
                "  {} PR-{} on `{}` does not credit {} — not marking it Done ({e})",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                pr,
                branch,
                spec,
            );
        }
        return;
    }
    let flipped = (|| -> anyhow::Result<bool> {
        let Some(store_path) = detect_distributed_store_from(project_root) else {
            return Ok(false);
        };
        let dispenser = load_dispenser(&store_path)?;
        let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
        let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;
        // trace:TASK-1468 | ai:claude
        let Some(req) = backend.get_requirement_unambiguous(spec)? else {
            return Ok(false);
        };
        // BUG-1638: the race-test seam between the read and the write.
        // trace:BUG-1638 | ai:claude
        status_write_race_seam(&store_path);
        // BUG-1637: the status check runs on the copy read under the store
        // lock; a refused flip writes nothing. trace:BUG-1637 | ai:claude
        let mut flipped = false;
        backend.update_spec_atomically(&req, |r| flipped = pr_open_done_flip(r))?;
        Ok(flipped)
    })();
    match flipped {
        Ok(true) => {
            if !json {
                eprintln!(
                    "  {} {} was still In Progress after PR-{} opened — marked Done (the \
                     implementer skipped `aida queue done`)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    spec,
                    pr,
                );
            }
        }
        Ok(false) => {}
        Err(e) => {
            if !json {
                eprintln!(
                    "  {} could not assert {} Done after PR-{} ({e}) — the review envelope \
                     still forces the reviewer seat",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    spec,
                    pr,
                );
            }
        }
    }
}
