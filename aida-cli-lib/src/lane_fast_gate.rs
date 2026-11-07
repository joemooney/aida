//! Host-owned fast-gate receipts and the single repair round.
// trace:STORY-2640 | ai:codex
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Receipt {
    branch: String,
    head: Option<String>,
    marker: String,
    pending: bool,
    repair_used: bool,
    #[serde(default)]
    native_ready: bool,
}

fn path(main: &Path, branch: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    main.join(".aida/fast-gates")
        .join(format!("{:x}.json", Sha256::digest(branch.as_bytes())))
}
fn read(main: &Path, branch: &str) -> Result<Option<Receipt>> {
    match std::fs::read(path(main, branch)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn write(main: &Path, receipt: &Receipt) -> Result<()> {
    let path = path(main, &receipt.branch);
    std::fs::create_dir_all(path.parent().context("fast gate directory")?)?;
    aida_core::write_atomic(&path, serde_json::to_vec(receipt)?)?;
    Ok(())
}

/// Reserve before the vendor can push. Existing retry credit survives relaunches.
pub(crate) fn begin(main: &Path, branch: &str) -> Result<()> {
    let mut receipt = read(main, branch)?.unwrap_or(Receipt {
        branch: branch.into(),
        head: None,
        marker: "skipped(pending)".into(),
        pending: true,
        repair_used: false,
        native_ready: false,
    });
    if !receipt.pending {
        receipt.repair_used = false;
    }
    receipt.pending = true;
    receipt.native_ready = false;
    receipt.head = None;
    receipt.marker = "skipped(pending)".into();
    write(main, &receipt)
}

/// An absent receipt is an ordinary non-lane PR. Corrupt state fails closed.
pub(crate) fn blocks_review(main: &Path, branch: &str, head: &str) -> Result<bool> {
    Ok(read(main, branch)?.is_some_and(|r| r.pending || r.head.as_deref() != Some(head)))
}

pub(crate) fn marker(main: &Path, branch: &str) -> String {
    read(main, branch)
        .ok()
        .flatten()
        .map(|r| r.marker)
        .unwrap_or_else(|| "skipped(no-result)".into())
}

fn veto(main: &Path, spec: &str) -> bool {
    crate::punt::read_signal(&crate::punt::signal_path(main, spec)).is_some()
        || crate::punt::read_hold_signal(&crate::punt::hold_signal_path(main, spec)).is_some()
}

fn commit(main: &Path, tree: &Path, spec: &str, lease: &str, clean: bool) -> Result<bool> {
    match crate::ship_from_seat::post_exit_commit_reported(main, tree, spec, lease, clean) {
        crate::ship_from_seat::PostExitCommit::Failed(why) => anyhow::bail!("{why}"),
        crate::ship_from_seat::PostExitCommit::NotASeat => Ok(false),
        _ => Ok(true),
    }
}

/// Probe clone Git only through its existing sandbox. Linked worktrees use
/// the ordinary sanitized host reader, which refuses private clones.
fn snapshot(main: &Path, tree: &Path, branch: &str) -> Result<String> {
    if crate::seat_clone::host_recorded_clone(main, tree).is_some() {
        match crate::seat_clone::probe(main, tree) {
            crate::seat_clone::Probe::Known(facts) => {
                anyhow::ensure!(
                    facts.dirty.is_empty(),
                    "fast gate needs a clean committed seat"
                );
                facts.head.context("fast gate seat has no head")
            }
            _ => anyhow::bail!("cannot bind fast gate to the seat head"),
        }
    } else {
        let state = crate::headless_background::SeatState::read(tree, branch);
        anyhow::ensure!(
            state.uncommitted == Some(0),
            "fast gate needs a clean committed seat"
        );
        crate::headless_background::seat_head(tree).context("fast gate seat head unavailable")
    }
}

fn attempt(
    main: &Path,
    tree: &Path,
    lease: &str,
    branch: &str,
) -> Result<(String, String, String)> {
    if crate::session::bwrap_availability() != crate::session::BwrapAvailability::Ok {
        record_marker(main, lease, "skipped(no-sandbox)")?;
        eprintln!("fast_gate=skipped(no-sandbox)");
        return Ok((String::new(), "skipped(no-sandbox)".into(), String::new()));
    }
    let head = snapshot(main, tree, branch)?;
    let (marker, output): (String, String) =
        match crate::lane_job::run_fast_gate(main, tree, lease)? {
            None => ("skipped(no-sandbox)".into(), String::new()),
            Some((0, output)) => ("pass".into(), output),
            Some((_, output)) => ("fail".into(), output),
        };
    anyhow::ensure!(
        snapshot(main, tree, branch)? == head,
        "seat changed during fast gate; refusing stale evidence"
    );
    record_marker(main, lease, &marker)?;
    eprintln!("fast_gate={marker} head={head}\n{output}");
    Ok((head, marker, output))
}

/// Host-published lanes commit locally, gate, resume once on red, and gate
/// the repaired commit before any publication. Signals remain publication vetoes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn native(
    main: &Path,
    tree: &Path,
    spec: &str,
    lease: &str,
    branch: &str,
    clean: bool,
    mut repair: impl FnMut(&str) -> Result<bool>,
) -> Result<bool> {
    if veto(main, spec) {
        return Ok(true);
    }
    if crate::session::bwrap_availability() != crate::session::BwrapAvailability::Ok {
        record_marker(main, lease, "skipped(no-sandbox)")?;
        let mut receipt = read(main, branch)?.context("fast gate was not reserved")?;
        receipt.marker = "skipped(no-sandbox)".into();
        write(main, &receipt)?;
        eprintln!(
            "fast_gate=skipped(no-sandbox); native commit/publication still requires its sandbox"
        );
        return Ok(true);
    }
    let clean_exit = std::cell::Cell::new(clean);
    let publishable = std::cell::Cell::new(true);
    native_rounds(
        main,
        branch,
        || {
            if veto(main, spec) {
                return Ok(None);
            }
            if !commit(main, tree, spec, lease, clean_exit.get())? {
                publishable.set(false);
                return Ok(None);
            }
            attempt(main, tree, lease, branch).map(Some)
        },
        |prompt| {
            clean_exit.set(repair(prompt)?);
            Ok(())
        },
    )?;
    Ok(publishable.get())
}

// The native publication driver, with only the sandbox/commit effects injected.
// The durable reservation and one-round limit are identical in production/tests.
fn native_rounds(
    main: &Path,
    branch: &str,
    mut attempt: impl FnMut() -> Result<Option<(String, String, String)>>,
    mut repair: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let mut receipt = read(main, branch)?.context("fast gate was not reserved")?;
    let Some((mut head, mut result, output)) = attempt()? else {
        return Ok(());
    };
    if result == "fail" && !receipt.repair_used {
        receipt.repair_used = true;
        receipt.marker = result.clone();
        write(main, &receipt)?;
        repair(&repair_prompt(&output))?;
        let Some((next_head, next_result, _)) = attempt()? else {
            return Ok(());
        };
        head = next_head;
        result = next_result;
    }
    receipt.head = Some(head);
    receipt.marker = result;
    receipt.pending = true;
    receipt.native_ready = true;
    write(main, &receipt)
}

fn repair_prompt(output: &str) -> String {
    format!("The host's sandboxed make check-ci-fast failed. This is your ONE fast-gate fix round. Fix the failing gates below and finish. Native seats must leave edits uncommitted for host publication; retain all punt/hold instructions.\n{output}")
}

/// A self-published lane queues its one repair before review admission. The
/// retry receipt is branch-scoped, so an unchanged or still-red retry terminates.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pushed(
    main: &Path,
    tree: &Path,
    spec: &str,
    lease: &str,
    branch: &str,
    pr: u64,
    vendor: &str,
) -> Result<bool> {
    if veto(main, spec) {
        return Ok(false);
    }
    let (mut head, result, output) = attempt(main, tree, lease, branch)?;
    if head.is_empty() {
        head = crate::standalone_implementer::lookup_pr(main, branch)
            .filter(|(id, _)| *id == pr)
            .and_then(|(_, head)| head)
            .context("cannot bind skipped fast gate to the published PR head")?;
    }
    pushed_round(
        main,
        branch,
        &head,
        &result,
        || annotate(main, pr, &head, &result),
        || {
            let storage = crate::Storage::new(
                crate::detect_distributed_store_from(main)
                    .context("fast gate rework store unavailable")?,
            );
            let store = storage.load()?;
            let req = store
                .get_requirement_unambiguous(spec)?
                .context("fast gate spec unavailable")?;
            let note = repair_prompt(&output);
            let vendor = crate::brief::validate_brief_agent(vendor)?;
            let dir = main.join(".aida/agent-briefs").join(vendor);
            std::fs::create_dir_all(&dir)?;
            let stamp = chrono::Utc::now()
                .format("%Y-%m-%dT%H%M%S%.9fZ")
                .to_string();
            let brief = dir.join(format!("{}-{stamp}.md", req.display_id()));
            aida_core::write_atomic(
                &brief,
                crate::brief::render_agent_brief(
                    main,
                    vendor,
                    &req.display_id(),
                    req,
                    &store,
                    &stamp,
                    Some(&note),
                    None,
                    None,
                ),
            )?;
            crate::brief::add_pending_brief(main, vendor, &brief)?;
            let mut retryable = false;
            storage.update_atomically(|store| {
                if let Some(live) = store.requirements.iter_mut().find(|r| r.id == req.id) {
                    if !matches!(
                        live.status,
                        aida_core::RequirementStatus::Completed
                            | aida_core::RequirementStatus::Rejected
                            | aida_core::RequirementStatus::Superseded
                    ) {
                        aida_core::conflict::set_status_recorded(
                            live,
                            aida_core::RequirementStatus::InProgress,
                            "fast-gate",
                        );
                        retryable = true;
                    }
                }
            })?;
            anyhow::ensure!(
                retryable,
                "fast gate spec disappeared or became terminal; no repair queued"
            );
            for user in storage.queue_users()? {
                if user != "role:implementer" {
                    storage.queue_remove_for_role(&user, &req.id, Some("implementer"))?;
                }
            }
            storage.queue_add(aida_core::QueueEntry {
                user_id: "role:implementer".into(),
                requirement_id: req.id,
                position: 0,
                added_by: "fast-gate".into(),
                note: Some(note),
                added_at: chrono::Utc::now(),
                for_role: Some("implementer".into()),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })?;
            Ok(())
        },
    )
}

// Keep review blocked through annotation and queue insertion, including errors.
fn pushed_round(
    main: &Path,
    branch: &str,
    head: &str,
    result: &str,
    annotate: impl FnOnce() -> Result<()>,
    queue_repair: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    let mut receipt = read(main, branch)?.context("fast gate was not reserved")?;
    receipt.head = Some(head.into());
    receipt.marker = result.into();
    let queue = result == "fail" && !receipt.repair_used;
    receipt.pending = true;
    write(main, &receipt)?;
    annotate()?;
    if queue {
        receipt.repair_used = true;
        write(main, &receipt)?;
        queue_repair()?;
    } else {
        receipt.pending = false;
        write(main, &receipt)?;
    }
    Ok(queue)
}

// trace:STORY-2640 | ai:codex
pub(crate) use crate::forge::annotate_fast_gate as annotate;

// trace:STORY-2640 | ai:codex
pub(crate) fn head(main: &Path, branch: &str) -> Option<String> {
    read(main, branch).ok().flatten().and_then(|r| r.head)
}

/// Publication must still name the commit which the sandbox checked.
// trace:STORY-2640 | ai:codex
pub(crate) fn publication_ready(main: &Path, tree: &Path, branch: &str) -> Result<()> {
    let r = read(main, branch)?.context("host publication has no fast-gate receipt")?;
    anyhow::ensure!(
        r.native_ready && r.head.as_deref() == Some(snapshot(main, tree, branch)?.as_str()),
        "host publication fast-gate evidence is pending or stale"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane_job::{self, JobRequest};
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn native_fail_then_fixed_round_binds_repaired_head() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        begin(root, "lane").unwrap();
        let mut attempts = 0;
        let mut repairs = 0;
        native_rounds(
            root,
            "lane",
            || {
                attempts += 1;
                assert!(blocks_review(root, "lane", "old").unwrap());
                Ok(Some(if attempts == 1 {
                    (
                        "old".into(),
                        "fail".into(),
                        "portability gate FAILED".into(),
                    )
                } else {
                    ("fixed".into(), "pass".into(), String::new())
                }))
            },
            |prompt| {
                repairs += 1;
                assert!(prompt.contains("portability gate FAILED"));
                assert!(read(root, "lane")?.unwrap().repair_used);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!((attempts, repairs), (2, 1));
        assert_eq!(marker(root, "lane"), "pass");
        assert!(blocks_review(root, "lane", "fixed").unwrap());
        published(root, "lane").unwrap();
        assert!(!blocks_review(root, "lane", "fixed").unwrap());
        assert!(blocks_review(root, "lane", "old").unwrap());
    }

    #[test]
    fn native_persistent_failure_releases_for_publication_without_third_round() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        begin(root, "lane").unwrap();
        let mut repairs = 0;
        for _ in 0..2 {
            native_rounds(
                root,
                "lane",
                || Ok(Some(("same".into(), "fail".into(), "gate red".into()))),
                |_| {
                    repairs += 1;
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(marker(root, "lane"), "fail");
            assert!(read(root, "lane").unwrap().unwrap().native_ready);
        }
        assert_eq!(repairs, 1);
    }

    #[test]
    fn native_repair_veto_keeps_publication_and_review_pending() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        let mut attempts = 0;
        native_rounds(
            tmp.path(),
            "lane",
            || {
                attempts += 1;
                Ok((attempts == 1).then(|| ("old".into(), "fail".into(), "failure".into())))
            },
            |_| Ok(()),
        )
        .unwrap();
        assert!(blocks_review(tmp.path(), "lane", "old").unwrap());
    }

    #[test]
    fn exact_make_job_has_fixed_timeout_and_never_counts_as_cargo_tests() {
        let mut job = JobRequest {
            argv: vec!["make".into(), "check-ci-fast".into()],
            env: BTreeMap::new(),
        };
        assert!(lane_job::validate(&job).is_ok());
        assert_eq!(lane_job::timeout_for(&job, true), Duration::from_secs(300));
        assert!(!lane_job::runs_tests(&job));
        let tally = lane_job::Tally::default();
        tally.record_exit(&job, "test result: ok. 1 passed; 0 failed;".as_bytes());
        assert!(tally.untested());
        job.env.insert("AIDA_CARGO_TIMEOUT".into(), "3600".into());
        assert!(lane_job::validate(&job).is_err());
        job.env.clear();
        job.argv.push("SHELL=evil".into());
        assert!(lane_job::validate(&job).is_err());
    }

    #[test]
    fn host_receipt_is_required_only_for_lane_branches_and_corruption_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!blocks_review(tmp.path(), "ordinary", "head").unwrap());
        begin(tmp.path(), "lane").unwrap();
        assert!(blocks_review(tmp.path(), "lane", "head").unwrap());
        std::fs::write(path(tmp.path(), "lane"), b"invalid").unwrap();
        assert!(blocks_review(tmp.path(), "lane", "head").is_err());
    }
    #[test]
    fn native_publication_refuses_pending_and_changed_heads() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("seat");
        let main = tmp.path().join("host");
        std::fs::create_dir_all(&tree).unwrap();
        let first = crate::test_git::commit_fixture(&tree);
        begin(&main, "main").unwrap();
        assert!(publication_ready(&main, &tree, "main").is_err());
        native_rounds(
            &main,
            "main",
            || {
                Ok(Some((
                    snapshot(&main, &tree, "main")?,
                    "pass".into(),
                    String::new(),
                )))
            },
            |_| panic!("green gates need no repair"),
        )
        .unwrap();
        publication_ready(&main, &tree, "main").unwrap();
        check_import(&main, "main", &first).unwrap();
        assert!(check_import(&main, "main", "different-import").is_err());
        let next = crate::test_git::commit_fixture(&tree);
        assert_ne!(first, next);
        assert!(publication_ready(&main, &tree, "main").is_err());
    }

    #[test]
    fn self_pushed_failure_queues_before_review_and_retry_can_stay_red() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        begin(root, "lane").unwrap();
        let mut queued = 0;
        let first = pushed_round(
            root,
            "lane",
            "first",
            "fail",
            || {
                assert!(blocks_review(root, "lane", "first")?);
                Ok(())
            },
            || {
                queued += 1;
                assert!(blocks_review(root, "lane", "first")?);
                Ok(())
            },
        )
        .unwrap();
        assert!(first);
        assert!(blocks_review(root, "lane", "first").unwrap());
        begin(root, "lane").unwrap();
        assert!(!pushed_round(
            root,
            "lane",
            "second",
            "fail",
            || Ok(()),
            || {
                queued += 1;
                Ok(())
            }
        )
        .unwrap());
        assert_eq!(queued, 1);
        assert_eq!(marker(root, "lane"), "fail");
        assert!(!blocks_review(root, "lane", "second").unwrap());
    }

    #[test]
    fn annotation_and_queue_failures_keep_review_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        for annotate_fails in [true, false] {
            let branch = format!("lane-{annotate_fails}");
            begin(tmp.path(), &branch).unwrap();
            let result = pushed_round(
                tmp.path(),
                &branch,
                "head",
                "fail",
                || {
                    anyhow::ensure!(!annotate_fails, "forge unavailable");
                    Ok(())
                },
                || anyhow::bail!("queue unavailable"),
            );
            assert!(result.is_err());
            assert!(blocks_review(tmp.path(), &branch, "head").unwrap());
        }
    }

    #[test]
    fn skipped_gate_needs_no_repair_and_does_not_claim_a_pass() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        native_rounds(
            tmp.path(),
            "lane",
            || {
                Ok(Some((
                    "head".into(),
                    "skipped(no-sandbox)".into(),
                    String::new(),
                )))
            },
            |_| panic!("no sandbox must never run a repair or an unsandboxed fallback"),
        )
        .unwrap();
        assert_eq!(marker(tmp.path(), "lane"), "skipped(no-sandbox)");
    }
}

/// Release review admission only after the host published and annotated its PR.
// trace:STORY-2640 | ai:codex
pub(crate) fn published(main: &Path, branch: &str) -> Result<()> {
    let mut receipt = read(main, branch)?.context("fast gate receipt disappeared")?;
    receipt.pending = false;
    receipt.native_ready = false;
    write(main, &receipt)
}

// trace:STORY-2640 | ai:codex
fn record_marker(main: &Path, lease: &str, marker: &str) -> Result<()> {
    let dir = crate::lane_job::results_dir(main, lease);
    std::fs::create_dir_all(&dir)?;
    aida_core::write_atomic(&dir.join("fast-gate-marker"), marker.as_bytes())?;
    Ok(())
}

/// The final imported bundle must still be the commit the host checked,
/// including when publication reconciled a concurrently advanced remote head.
// trace:STORY-2640 | ai:codex
pub(crate) fn check_import(main: &Path, branch: &str, tip: &str) -> Result<()> {
    if let Some(r) = read(main, branch)? {
        anyhow::ensure!(
            r.native_ready && r.head.as_deref() == Some(tip),
            "imported lane head differs from the host fast gate; rerun the lane before publication"
        );
    }
    Ok(())
}
