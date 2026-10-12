//! drain phase-1 scope prep, phase-3 verdicts, tuning, and watchdog (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Resolve a spec's scope epic: walk its `parent:<ID>` tag chain (bounded) to
/// the nearest ancestor of type Epic. Returns its display id, or `None` when no
/// epic ancestor is reachable.
// trace:ADR-6 trace:TASK-1037 | ai:claude
pub(crate) fn resolve_spec_scope(req: &Requirement, store: &RequirementsStore) -> Option<String> {
    let mut current = req;
    let mut seen = std::collections::HashSet::new();
    seen.insert(current.id);
    for _ in 0..16 {
        let parent_ref = current
            .tags
            .iter()
            .find_map(|t| t.strip_prefix("parent:").map(|p| p.trim().to_string()))?;
        let parent = store
            .requirements
            .iter()
            .find(|r| spec_matches(r, &parent_ref))?;
        if !seen.insert(parent.id) {
            break;
        }
        if matches!(parent.req_type, RequirementType::Epic) {
            return Some(parent.display_id());
        }
        current = parent;
    }
    None
}

/// Does the scope (epic / focus) have OTHER in-flight work right now? Used for
/// the ADR-6 override-into-collision warning: a spec directly tagged
/// `parent:<scope>` that is In Progress.
// trace:ADR-6 trace:TASK-1037 | ai:claude
pub(crate) fn scope_has_in_flight_work(
    store: &RequirementsStore,
    scope: &str,
    exclude: &Requirement,
) -> bool {
    store.requirements.iter().any(|r| {
        r.id != exclude.id
            && matches!(r.status, RequirementStatus::InProgress)
            && r.tags.iter().any(|t| {
                t.strip_prefix("parent:")
                    .map(|p| p.trim().eq_ignore_ascii_case(scope))
                    .unwrap_or(false)
            })
    })
}

/// Queue `spec` for the implementer role if it isn't already queued for the
/// current user — the preflight that lets `--auto-complete` accept a
/// freshly-added spec. trace:STORY-246 | ai:claude
pub(crate) fn ensure_queued_for_implementer(
    storage: &Storage,
    user_id: &str,
    spec: &str,
) -> Result<()> {
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    let already = storage
        .queue_list(user_id, false)?
        .iter()
        .any(|e| e.requirement_id == req.id);
    if already {
        return Ok(());
    }
    eprintln!(
        "  {} {} is not queued — queueing it for the implementer",
        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
        spec
    );
    let exe = aida_exe_path();
    let status = std::process::Command::new(exe)
        .args(auto_complete_queue_add_args(spec))
        .status_retrying_etxtbsy()
        .context("failed to run `aida queue add`")?;
    if !status.success() {
        anyhow::bail!("`aida queue add {spec} --for implementer --no-scope` failed");
    }
    Ok(())
}

pub(crate) fn auto_complete_phase1_target_status(
    status: &RequirementStatus,
) -> Option<RequirementStatus> {
    match status {
        RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::Planned => {
            Some(RequirementStatus::InProgress)
        }
        _ => None,
    }
}

/// Registration and the status bump share one checked entry point, so an
/// ownership failure cannot leave Approved work InProgress without a lease.
// trace:TASK-1603 | ai:codex
pub(crate) fn prepare_registered_auto_complete_phase1(
    storage: &Storage,
    project_root: &std::path::Path,
    spec: &str,
    run_token: &str,
    zen: bool,
    owns_drain_state: bool,
) -> Result<Option<(String, RequirementStatus)>> {
    drain_state::register_run(project_root, spec, run_token, zen, owns_drain_state)?;
    let result = prepare_auto_complete_phase1_status(storage, spec);
    if result.is_err() {
        drain_state::clear_run(project_root, run_token);
    }
    result
}

/// BUG-369: mark orchestrator-driven phase-1 work as InProgress before the
/// implementer subprocess starts. `aida punt` correctly allows only
/// InProgress → NeedsAttention; without this pre-spawn flip, an early design
/// fork on an Approved/Planned/Draft spec made `/aida-punt` refuse and the
/// orchestrator misclassified the clean exit as NoPR.
pub(crate) fn prepare_auto_complete_phase1_status(
    storage: &Storage,
    spec: &str,
) -> Result<Option<(String, RequirementStatus)>> {
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    let display_id = req.display_id();
    let current = req.status.clone();
    let Some(target) = auto_complete_phase1_target_status(&current) else {
        return Ok(None);
    };
    let now = chrono::Utc::now();
    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    // BUG-1637: re-check the source status inside the write (a concurrent
    // Rejected/Completed must not become In Progress), record the bump under
    // the orchestrator's phase-1 author, and take the restore target from the
    // status read under the lock, not the pre-lock read.
    // trace:BUG-1637 | ai:claude
    let mut prior: Option<RequirementStatus> = None;
    storage.update_spec_atomically(req, |r| {
        let before = r.status.clone();
        if phase1_status_bump(r, &target, now) {
            prior = Some(before);
        }
    })?;
    Ok(prior.map(|p| (display_id, p)))
}

pub(crate) fn resolve_lifecycle_skip(
    storage: &Storage,
    spec: &str,
) -> Result<auto_complete::LifecycleSkip> {
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    Ok(auto_complete::LifecycleSkip::from_tags(
        req.tags.iter().map(String::as_str),
    ))
}

/// TASK-827: classify this spec as keystone/architecture-class for the solo
/// posture. Best-effort — if the spec can't be loaded we treat it as
/// non-keystone so the posture errs toward the cheap error (a safe spec parked
/// or proceeded), never the expensive one (shipping keystone unattended is
/// guarded separately by the `supervised` drain exclusion). Returns the
/// `(req_type, tags)` of a conservative classification via
/// `presence::is_keystone_class`. trace:TASK-827 | ai:claude
pub(crate) fn solo_spec_is_keystone(storage: &Storage, spec: &str) -> bool {
    let Ok(store) = storage.load() else {
        return false;
    };
    let Some(req) = store.requirements.iter().find(|r| spec_matches(r, spec)) else {
        return false;
    };
    presence::is_keystone_class(
        &req.req_type.to_string(),
        req.tags.iter().map(String::as_str),
    )
}

/// Best-effort lookup of the most recent workflow run id for `branch`, used
/// to enrich the CI-failure recovery hint. trace:STORY-246 | ai:claude
pub(crate) fn latest_run_id_for_branch(branch: &str) -> Option<String> {
    let gh = resolve_gh_binary()?;
    let out = std::process::Command::new(gh)
        .args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "databaseId",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    first_run_id_from_gh_json(&String::from_utf8_lossy(&out.stdout))
}

/// Build the red-CI recovery route from the exact rows that caused the gate
/// to fail. In particular, never perform a second branch-level run lookup:
/// another attached workflow may be newer and have colliding job names.
// trace:BUG-1298 | ai:codex
pub(crate) fn ci_red_recovery_hint(
    forge: crate::forge::ForgeKind,
    spec: &str,
    branch: &str,
    failed: &[ci_gate::CheckRow],
) -> String {
    let subjects = failed
        .iter()
        .map(|row| match row.run_id.as_deref() {
            Some(run) => forge
                .ci_view_cmd(run)
                .map(|cmd| format!("{} (run {run}; `{cmd}`)", row.name))
                .unwrap_or_else(|| format!("{} (run {run})", row.name)),
            None => format!("{} (owning run unavailable)", row.name),
        })
        .collect::<Vec<_>>()
        .join("; ");
    let fallback = match forge {
        crate::forge::ForgeKind::GitHub => format!("`gh run list --branch {branch}`"),
        crate::forge::ForgeKind::GitLab => "`glab ci status`".to_string(),
        crate::forge::ForgeKind::None => "your CI dashboard".to_string(),
    };
    let route = if subjects.is_empty() {
        format!("CI failed — inspect {fallback}")
    } else {
        format!("CI failed: {subjects}")
    };
    format!(
        "{route}. Push fixups to the same branch: `aida queue work {spec} --branch {branch} --steal`"
    )
}

#[cfg(test)]
mod bug_1298_tests {
    use super::*;

    #[test]
    fn ci_red_hint_uses_failing_required_checks_own_run_not_newer_colliding_workflow() {
        let rows = ci_gate::parse_check_rows(
            r#"[
                {"name":"Build (ubuntu-latest)","workflow":"CI","bucket":"fail","link":"https://github.com/acme/repo/actions/runs/35471997271/job/1"},
                {"name":"Build (windows-latest)","workflow":"Cross-platform (nightly)","bucket":"pending","link":"https://github.com/acme/repo/actions/runs/35471997274/job/2"}
            ]"#,
        )
        .unwrap();
        let refined = ci_gate::classify_red(
            &rows,
            &["Build (ubuntu-latest)".to_string()],
            &ci_gate::CiGateConfig::default(),
            false,
            false,
        );
        let hint = ci_red_recovery_hint(
            crate::forge::ForgeKind::GitHub,
            "BUG-1231",
            "bug-1231-work",
            &refined.real,
        );

        assert!(hint.contains("Build (ubuntu-latest)"), "{hint}");
        assert!(hint.contains("gh run view 35471997271"), "{hint}");
        assert!(!hint.contains("35471997274"), "{hint}");
        assert!(!hint.contains("Build (windows-latest)"), "{hint}");
    }
}

/// Read + parse a `.aida/review-verdicts/PR-N.json` verdict file written by
/// the `/aida-review` skill.
///
/// Returns [`auto_complete::ReviewerOutcome::EscalatedToHuman`] when the file
/// carries `merge: escalated-to-human` — the reviewer wrote a verdict but
/// escalated the *merge* decision to a human rather than auto-deciding it
/// (STORY-306) — and [`auto_complete::ReviewerOutcome::Verdict`] otherwise.
/// The `merge` field is dominant: an escalation is honoured whatever the
/// `verdict` field says, so the phase-3 handshake artifact always parses.
/// trace:STORY-246, STORY-306 | ai:claude
/// BUG-806: phase-3 fallback — when the PR-keyed handshake file is absent,
/// accept the SPEC-keyed recorded verdict (`aida review record <SPEC>`) if it
/// was written DURING this reviewer session (file mtime >= session start).
///
/// Five consecutive phase-3 shelves proved the narrow polling wrong: on the
/// fifth, the reviewer ran the record verb with a correct verdict and the
/// orchestrator still shelved because only `PR-N.json` was consulted. The
/// orchestrator drives a known spec; refusing first-class verdict state it can
/// already see is the substrate losing information it holds. The mtime gate
/// means a verdict recorded by an EARLIER review can never advance a later
/// diff, and the PR-keyed file remains the primary contract — this runs only
/// on a miss.
// trace:BUG-806 | ai:claude
pub(crate) fn spec_verdict_fallback_for_phase3(
    project_root: &std::path::Path,
    spec: &str,
    reviewer_started_at: std::time::SystemTime,
    current_head: Option<&str>,
) -> Result<Option<auto_complete::ReviewerOutcome>, auto_complete::PhaseFailure> {
    let path = review_verdict::verdict_path(project_root, spec);
    let Some(mtime) = std::fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
    else {
        return Ok(None);
    };
    if mtime < reviewer_started_at {
        return Ok(None); // stale: recorded by some earlier review, not this one
    }
    // TASK-1460: a later round at another commit may have replaced the spec
    // record; the archived round for THIS head (fresh this session) still
    // answers. The current file is consulted first and its error stands when
    // no such archive exists.
    // trace:TASK-1460 | ai:claude
    let outcome = match read_verdict_file_for_head(&path, current_head) {
        Ok(outcome) => outcome,
        Err(e) => {
            phase3_head_archive_fallback(&path, current_head, reviewer_started_at).ok_or(e)?
        }
    };
    eprintln!(
        "  {} no PR-keyed verdict file, but the reviewer recorded {} for {} during this session — accepting it",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        match &outcome {
            auto_complete::ReviewerOutcome::Verdict(verdict) => verdict.label(),
            auto_complete::ReviewerOutcome::EscalatedToHuman { .. } => "ESCALATED TO HUMAN",
        },
        spec
    );
    Ok(Some(outcome))
}

/// BUG-809: last-ditch verdict discovery when both the PR-keyed file and the
/// spec-keyed record are missing from the drive root — sweep SIBLING
/// directories of the drive root for a verdict this reviewer wrote into its
/// own checkout. The env anchor (`AIDA_DRIVE_ROOT` + PATH, BUG-802/BUG-806)
/// does not reliably survive a vendor's tool-call sandbox, so a reviewer that
/// checked the PR out next to the repo and ran `aida review record` from
/// inside the checkout lands both files there. The PR number is unique and
/// the mtime gate (>= reviewer session start) makes stale pickup impossible,
/// so accepting the freshest fresh candidate is safe regardless of what the
/// reviewer named its checkout. A found PR-keyed file is copied back into the
/// drive root's verdict dir so the calibration tag-along and the audit trail
/// see the canonical location.
// trace:BUG-809 | ai:claude
pub(crate) fn sibling_verdict_sweep_for_phase3(
    project_root: &std::path::Path,
    pr: u32,
    spec: &str,
    reviewer_started_at: std::time::SystemTime,
    current_head: Option<&str>,
) -> Result<Option<auto_complete::ReviewerOutcome>, auto_complete::PhaseFailure> {
    let Some(root_canon) = project_root.canonicalize().ok() else {
        return Ok(None);
    };
    let Some(parent) = root_canon.parent() else {
        return Ok(None);
    };
    // Collect fresh candidates: (mtime, path, is_pr_keyed), freshest first.
    let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf, bool)> = Vec::new();
    let Some(entries) = std::fs::read_dir(parent).ok() else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if dir.canonicalize().ok().as_deref() == Some(root_canon.as_path()) {
            continue; // the drive root itself — already consulted
        }
        for (name, is_pr) in [
            (format!("PR-{pr}.json"), true),
            (format!("{spec}.json"), false),
        ] {
            let cand = dir.join(".aida").join("review-verdicts").join(&name);
            let Some(mtime) = std::fs::metadata(&cand)
                .ok()
                .and_then(|m| m.modified().ok())
            else {
                continue;
            };
            if mtime >= reviewer_started_at {
                candidates.push((mtime, cand, is_pr));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let Some((_, freshest, _is_pr)) = candidates.first() else {
        return Ok(None);
    };
    // Every fresh artifact is evidence. Reconcile the complete set rather
    // than allowing whichever file has the newest mtime to hide an opposing
    // same-head review.
    // trace:BUG-1581 | ai:codex
    let mut bodies = candidates
        .iter()
        .map(|(_, path, _)| {
            std::fs::read_to_string(path).map_err(|e| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    format!(
                        "could not read sibling verdict evidence at {}: {e}",
                        path.display()
                    ),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let reconcile = |evidence: &[String], current_head: &str| {
        review_verdict::reconcile_artifacts_for_sha(
            evidence.iter().map(String::as_str),
            current_head,
        )
        .map_err(|reason| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!("{reason} — sibling review evidence cannot be proven safe"),
            )
        })
    };
    let mut outcome = if let Some(current_head) = current_head {
        match reconcile(&bodies, current_head)? {
            Some(review_verdict::VerdictKind::Approved) => {
                auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::Approved)
            }
            Some(review_verdict::VerdictKind::RequestChanges)
            | Some(review_verdict::VerdictKind::Rejected) => {
                auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::RequestChanges)
            }
            Some(review_verdict::VerdictKind::Unknown) | None => {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    "fresh sibling verdict evidence does not contain a verdict for the current PR head",
                ));
            }
        }
    } else {
        // Preserve the legacy fail-safe use case: a refusal does not need a
        // forge head to block. Approvals still fail in
        // `read_verdict_file_for_head`, and every fresh file is consulted.
        let mut blocking = None;
        for (_, path, _) in &candidates {
            let candidate = read_verdict_file_for_head(path, None)?;
            match candidate {
                auto_complete::ReviewerOutcome::Verdict(
                    auto_complete::Verdict::RequestChanges | auto_complete::Verdict::Rejected,
                ) => blocking = Some(candidate),
                auto_complete::ReviewerOutcome::EscalatedToHuman { .. } if blocking.is_none() => {
                    blocking = Some(candidate);
                }
                _ => {}
            }
        }
        blocking.expect("fresh candidates are non-empty")
    };

    // Publish without overwriting a verdict that appeared at the canonical
    // path after the caller's initial probe. A hard link is an atomic,
    // complete-file, no-clobber publication boundary. On collision, include
    // that evidence in the same reconciliation and leave its bytes untouched.
    let dest_dir = project_root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&dest_dir).map_err(|e| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("could not create canonical verdict directory: {e}"),
        )
    })?;
    let dest = dest_dir.join(freshest.file_name().expect("candidate has a file name"));
    let temp_name = format!(
        ".{}.publish-{}-{}",
        dest.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("verdict"),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp = dest_dir.join(temp_name);
    let staged = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bodies[0].as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&temp);
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("could not stage canonical verdict evidence: {e}"),
        ));
    }
    let publication = std::fs::hard_link(&temp, &dest);
    let _ = std::fs::remove_file(&temp);
    match publication {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let canonical = std::fs::read_to_string(&dest).map_err(|read_error| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    format!(
                        "canonical verdict evidence appeared concurrently but could not be read: {read_error}"
                    ),
                )
            })?;
            bodies.push(canonical);
            let Some(current_head) = current_head else {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    "canonical verdict evidence appeared concurrently and cannot be reconciled because the current PR head is unavailable",
                ));
            };
            outcome = match reconcile(&bodies, current_head)? {
                Some(review_verdict::VerdictKind::Approved) => {
                    auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::Approved)
                }
                Some(review_verdict::VerdictKind::RequestChanges)
                | Some(review_verdict::VerdictKind::Rejected) => {
                    auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::RequestChanges)
                }
                Some(review_verdict::VerdictKind::Unknown) | None => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::NoVerdict,
                        "canonical and sibling verdict evidence do not establish a verdict for the current PR head",
                    ));
                }
            };
        }
        Err(e) => {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!("could not publish canonical verdict evidence: {e}"),
            ));
        }
    }
    eprintln!(
        "  {} no verdict at the drive root, but the reviewer wrote one in a sibling checkout ({}) during this session — accepting it",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        freshest.display()
    );
    Ok(Some(outcome))
}

/// TASK-1460: the phase-3 handshake's per-commit fallback. The verdict file
/// at `path` is keyed by PR or spec, so a later round at a different commit
/// replaces it; BUG-1539 archived every round at `<key>/<sha>.json`. This
/// reads the archived round for `head` — only when it was written during this
/// review session (mtime >= `started_at`, the same freshness gate BUG-806
/// applies), and only through [`read_verdict_file_for_head`], so an archived
/// approval still has to prove it covers `head`. `None` = no such round; the
/// caller's original failure then stands.
// trace:TASK-1460 | ai:claude
pub(crate) fn phase3_head_archive_fallback(
    path: &std::path::Path,
    head: Option<&str>,
    started_at: std::time::SystemTime,
) -> Option<auto_complete::ReviewerOutcome> {
    let head = head?;
    // Fail closed: the archive may only answer when the CURRENT file is a
    // well-formed, unconflicted verdict that was recorded at a DIFFERENT
    // commit. Malformed JSON, a missing verdict, a reconcile conflict, or a
    // current file already at `head` keep their original error (PRIN-5,
    // TASK-1169) — an older archived approval must never paper over them.
    // trace:TASK-1460 | ai:claude
    read_verdict_file(path).ok()?;
    let current_sha = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| review_verdict::parse_recorded_verdict(&b))
        .and_then(|v| v.reviewed_sha)?;
    if review_verdict::same_reviewed_sha(&current_sha, head) {
        return None;
    }
    let archived = review_verdict::archived_verdict_file_for_sha(path, head)?;
    let mtime = std::fs::metadata(&archived).ok()?.modified().ok()?;
    if mtime < started_at {
        return None;
    }
    read_verdict_file_for_head(&archived, Some(head)).ok()
}

#[cfg(test)]
mod task_1460_phase3_archive_tests {
    use super::*;

    const R1: &str = "3acf3671fd7a1111111111111111111111111111";
    const R2: &str = "cd21a1dc0a9e2222222222222222222222222222";

    fn record(root: &std::path::Path, path: &std::path::Path, verdict: &str, sha: &str) {
        review_verdict::record_verdict_at_path(
            root,
            path,
            Some(verdict),
            Some(sha),
            None,
            None,
            &[],
            "reviewer",
        )
        .unwrap();
    }

    // trace:TASK-1460 | ai:claude
    #[test]
    fn a_fresh_archived_round_at_head_answers_when_the_current_file_moved_on() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let path = review_verdict::verdict_path(root, "PR-50");
        let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        record(root, &path, "approved", R1);
        record(root, &path, "approved", R2);
        // The current file is at R2, so it cannot prove an approval of R1…
        assert!(read_verdict_file_for_head(&path, Some(R1)).is_err());
        // …but the archived round for R1 can.
        assert!(matches!(
            phase3_head_archive_fallback(&path, Some(R1), started),
            Some(auto_complete::ReviewerOutcome::Verdict(
                auto_complete::Verdict::Approved
            ))
        ));
        // No head, an unreviewed head, or a round older than this session: none.
        assert!(phase3_head_archive_fallback(&path, None, started).is_none());
        assert!(phase3_head_archive_fallback(&path, Some("deadbeefdeadbeef"), started).is_none());
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(phase3_head_archive_fallback(&path, Some(R1), later).is_none());
    }

    // trace:TASK-1460 | ai:claude
    #[test]
    fn a_malformed_current_file_never_falls_back_to_an_archived_approval() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let path = review_verdict::verdict_path(root, "PR-51");
        let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        record(root, &path, "approved", R1);
        // The current file is then corrupted, or carries no reviewed commit.
        for bad in [
            "{not json",
            r#"{"summary":"no verdict"}"#,
            r#"{"verdict":"approved"}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(read_verdict_file_for_head(&path, Some(R1)).is_err());
            assert!(
                phase3_head_archive_fallback(&path, Some(R1), started).is_none(),
                "must fail closed for {bad}"
            );
            assert!(read_verdict_file_for_head(&path, Some(R1))
                .or_else(|e| phase3_head_archive_fallback(&path, Some(R1), started).ok_or(e))
                .is_err());
        }
        // A spec-keyed malformed record is an error from the spec fallback too.
        let spec_path = review_verdict::verdict_path(root, "TASK-61");
        record(root, &spec_path, "approved", R1);
        std::fs::write(&spec_path, "{not json").unwrap();
        assert!(spec_verdict_fallback_for_phase3(root, "TASK-61", started, Some(R1)).is_err());
    }

    // trace:TASK-1460 | ai:claude
    #[test]
    fn the_spec_fallback_reads_the_archived_round_for_head() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let path = review_verdict::verdict_path(root, "TASK-60");
        let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        record(root, &path, "request-changes", R1);
        record(root, &path, "approved", R2);
        let got = spec_verdict_fallback_for_phase3(root, "TASK-60", started, Some(R1))
            .unwrap_or_else(|_| panic!("the archived refusal at R1 must answer"));
        assert!(matches!(
            got,
            Some(auto_complete::ReviewerOutcome::Verdict(
                auto_complete::Verdict::RequestChanges
            ))
        ));
    }
}

pub(crate) fn read_verdict_file(
    path: &std::path::Path,
) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
    let body = std::fs::read_to_string(path).map_err(|_| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            "the reviewer session produced no verdict file — the review did not complete",
        )
    })?;
    if let Some(current_sha) =
        review_verdict::parse_recorded_verdict(&body).and_then(|recorded| recorded.reviewed_sha)
    {
        review_verdict::reconcile_artifacts_for_sha([body.as_str()], &current_sha).map_err(
            |reason| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    format!("{reason} — review evidence cannot be proven safe"),
                )
            },
        )?;
    }
    if let Some(conflict) = review_verdict::verdict_conflict_for_current_sha(&body) {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("{conflict} — refusing to select a winner; reconcile the independent reviews"),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("the verdict file is not valid JSON: {e}"),
        )
    })?;
    // STORY-306: `merge: escalated-to-human` ⇒ the reviewer would not
    // auto-decide the merge. A first-class non-failure outcome — the
    // orchestrator stops clean and leaves the PR for a human. The `summary`
    // field carries the "why".
    if value.get("merge").and_then(|m| m.as_str()).map(str::trim)
        == Some(reviewer_summary::MERGE_ESCALATED_TO_HUMAN)
    {
        let reason = value
            .get("summary")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("the reviewer escalated the merge decision to a human")
            .to_string();
        return Ok(auto_complete::ReviewerOutcome::EscalatedToHuman { reason });
    }
    let raw = value
        .get("verdict")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                "the verdict file has no `verdict` field",
            )
        })?;
    auto_complete::Verdict::parse(raw)
        .map(auto_complete::ReviewerOutcome::Verdict)
        .ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!("unrecognised verdict `{raw}` in the verdict file"),
            )
        })
}

/// Read a live phase-3 handshake and prove an approval covers the PR head the
/// orchestrator is about to advance. Refusals and escalations remain usable
/// without this check because they fail closed already.
// trace:BUG-1466 | ai:codex
// trace:BUG-1538 | ai:codex
pub(crate) fn read_verdict_file_for_head(
    path: &std::path::Path,
    current_head: Option<&str>,
) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
    let outcome = read_verdict_file(path)?;
    if !matches!(
        outcome,
        auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::Approved)
    ) {
        return Ok(outcome);
    }
    let body = std::fs::read_to_string(path).map_err(|e| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("could not re-read approval provenance: {e}"),
        )
    })?;
    let recorded = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("reviewed_sha")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let same_commit = match (recorded.as_deref(), current_head) {
        (Some(a), Some(b)) => {
            let a = a.trim();
            let b = b.trim();
            a.len().min(b.len()) >= 7
                && a.bytes().all(|c| c.is_ascii_hexdigit())
                && b.bytes().all(|c| c.is_ascii_hexdigit())
                && (a.eq_ignore_ascii_case(b)
                    || (a.len() < b.len() && b[..a.len()].eq_ignore_ascii_case(a))
                    || (b.len() < a.len() && a[..b.len()].eq_ignore_ascii_case(b)))
        }
        _ => false,
    };
    if same_commit {
        Ok(outcome)
    } else {
        Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            match (recorded.as_deref(), current_head) {
                (None, _) => "the APPROVED verdict is UNPROVEN because it records no reviewed_sha"
                    .to_string(),
                (_, None) => "the APPROVED verdict is UNPROVEN because the current PR head could not be resolved"
                    .to_string(),
                (Some(reviewed), Some(current)) => format!(
                    "the APPROVED verdict is stale: it reviewed {reviewed}, but the current PR head is {current}"
                ),
            },
        ))
    }
}

/// STORY-439: pick the calibration review-slot fields out of a verdict
/// file and upsert the per-spec capture record. The verdict file is
/// already loaded by `read_verdict_file` for the orchestrator's PASS /
/// FAIL decision; this is a tag-along read that records advisory
/// metadata (it never changes the decision). Best-effort — a missing
/// file, missing fields, or write error all silently no-op. Called for
/// each spec the PR credits; one PR populates N records.
/// trace:STORY-439 | ai:claude
pub(crate) fn capture_review_calibration_for_spec(
    project_root: &std::path::Path,
    verdict_path: &std::path::Path,
    spec: &str,
) {
    let Ok(body) = std::fs::read_to_string(verdict_path) else {
        return;
    };
    let Some(verdict) = reviewer_summary::parse_verdict_file(&body) else {
        return;
    };
    if let Some(level_raw) = verdict
        .implementation_complexity
        .as_deref()
        .map(str::trim)
        .filter(|s: &&str| !s.is_empty())
    {
        if let Some(level) = complexity_calibration::ComplexityLevel::parse_str(level_raw) {
            let agreement = verdict
                .complexity_agreement
                .as_deref()
                .and_then(complexity_calibration::ComplexityAgreement::parse_str);
            if let Err(e) =
                complexity_calibration::upsert_review(project_root, spec, level, agreement)
            {
                eprintln!(
                    "  {} could not record review calibration for {spec}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
    }
    let effort = verdict
        .implementation_effort
        .as_deref()
        .and_then(effort_calibration::EffortBucket::parse_str);
    if let Err(e) =
        effort_calibration::upsert_review(project_root, spec, effort, Some("reviewer".to_string()))
    {
        eprintln!(
            "  {} could not record review effort for {spec}: {e}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
}

/// When phase 3 ends with [`auto_complete::FailureKind::NoVerdict`] under a
/// headless `--no-human` drain, scan the reviewer's headless log for the
/// BUG-280 signature: an `AskUserQuestion` event. The harness denies
/// AskUserQuestion under `--no-human=both`, the reviewer Claude bails ~10s
/// later, and the verdict file never lands — so an AskUserQuestion event
/// alongside a missing verdict file is a near-certain diagnosis.
///
/// Returns the original failure when no recent log can be located, when the
/// log cannot be read, or when no AskUserQuestion event is found. On a hit,
/// returns a NoVerdict failure whose `reason` names AskUserQuestion as the
/// likely cause and points at the offending log file. trace:BUG-280 | ai:claude
pub(crate) fn enrich_no_verdict_with_headless_diagnostic(
    failure: auto_complete::PhaseFailure,
    project_root: &std::path::Path,
    started_at: std::time::SystemTime,
) -> auto_complete::PhaseFailure {
    if failure.kind != auto_complete::FailureKind::NoVerdict {
        return failure;
    }
    let logs_dir = project_root.join(".aida").join("headless-logs");
    let Ok(entries) = std::fs::read_dir(&logs_dir) else {
        return failure;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < started_at {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        // The harness emits AskUserQuestion as a tool_use with name field —
        // the substring catches both the structured `"name":"AskUserQuestion"`
        // form and any narrative mention in assistant text.
        if body.contains("AskUserQuestion") {
            return auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!(
                    "{} — the reviewer's headless log ({}) contains an \
                     AskUserQuestion call, which is forbidden under \
                     `--no-human` and likely caused the session to bail \
                     before writing the verdict file (BUG-280). The \
                     `/aida-review` skill must auto-resolve every \
                     confirmation under AIDA_HEADLESS=1 instead of \
                     calling AskUserQuestion.",
                    failure.reason,
                    entry.path().display(),
                ),
            );
        }
    }
    enrich_headless_wait_failure(failure, project_root, started_at)
}

// trace:BUG-1063 | ai:codex
pub(crate) fn enrich_headless_wait_failure(
    failure: auto_complete::PhaseFailure,
    project_root: &std::path::Path,
    started_at: std::time::SystemTime,
) -> auto_complete::PhaseFailure {
    if !matches!(
        failure.kind,
        auto_complete::FailureKind::NoPr | auto_complete::FailureKind::NoVerdict
    ) {
        return failure;
    }
    let logs_dir = project_root.join(".aida").join("headless-logs");
    let Ok(entries) = std::fs::read_dir(&logs_dir) else {
        return failure;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < started_at {
            continue;
        }
        let path = entry.path();
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(last_text) = last_headless_assistant_text(&body) else {
            continue;
        };
        if headless_wait_text(&last_text) {
            return auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::HeadlessWait,
                format!(
                    "{} — the headless log ({}) ended with assistant text that \
                     waits for a future notification/monitor. Under AIDA_HEADLESS=1 \
                     there is no next turn; poll in <=60s bounded steps and write \
                     the phase result with the current wait state instead (BUG-1063).",
                    failure.reason,
                    path.display(),
                ),
            );
        }
    }
    failure
}

// trace:BUG-1063 | ai:codex
pub(crate) fn last_headless_assistant_text(content: &str) -> Option<String> {
    content
        .lines()
        .rev()
        .filter_map(headless_line_assistant_text)
        .find(|s| !s.trim().is_empty())
}

// trace:BUG-1063 | ai:codex
pub(crate) fn headless_line_assistant_text(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("type").and_then(|v| v.as_str()) == Some("result") {
        return value
            .get("result")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }
    if value.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        parts.push(text.to_string());
    }
    let content = value
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| value.get("content"));
    if let Some(blocks) = content.and_then(|v| v.as_array()) {
        for block in blocks {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

// trace:BUG-1063 | ai:codex
pub(crate) fn headless_wait_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "waiting on",
        "wait for the notification",
        "waiting for the notification",
        "waiting on the watcher notification",
        "armed a monitor",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
#[path = "tests/enrich_headless_diagnostic_tests.rs"]
mod enrich_headless_diagnostic_tests;

#[cfg(test)]
#[path = "tests/read_verdict_file_tests.rs"]
mod read_verdict_file_tests;

/// TASK-262: cover the `RealPhaseDriver` subprocess-wiring seams that drive the
/// orchestrator's real phases — the argv each phase hands to its `aida`
/// subprocess, plus the lease-discovery multiplicity (0/1/N) that decides
/// whether phase 1 succeeds or emits a candidate-listing failure. STORY-246
/// shipped the orchestrator with full `MockPhaseDriver` coverage but left the
/// real driver's command-building seams untested; these close that gap without
/// ever spawning `claude`/`aida`/`git`. Every test is fully isolated (its own
/// `tempfile::tempdir`, no shared CWD/HOME, no real git ops) to avoid the
/// BUG-463-class flakiness.
// trace:TASK-262 | ai:claude
#[cfg(test)]
#[path = "tests/real_phase_driver_wiring_tests.rs"]
mod real_phase_driver_wiring_tests;

/// BUG-1716: the never-started launch classification — missing-log evidence,
/// the commits corroboration, and the no-transient-retry contract.
// trace:BUG-1716 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1716_never_started_launch_tests.rs"]
mod bug_1716_never_started_launch_tests;

/// Read a spec's current status straight from the git-canonical store —
/// ground truth for the BUG-241 reconcile step. A spec the implementer (or a
/// human) marked Completed needed no further work, even when the phase
/// produced no PR (instance B: resolved-by-supersession). `None` when the
/// store can't be opened or the spec isn't found. trace:BUG-241 | ai:claude
pub(crate) fn spec_status(project_root: &std::path::Path, spec: &str) -> Option<RequirementStatus> {
    use aida_core::DatabaseBackend;
    let store_path = project_root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_path).ok()?;
    let req = backend.get_requirement_by_spec_id(spec).ok()??;
    Some(req.status)
}

// trace:BUG-1112 | ai:codex
pub(crate) fn latest_reopen_transition_at(
    req: &aida_core::Requirement,
) -> Option<chrono::DateTime<chrono::Utc>> {
    req.history
        .iter()
        .filter(|entry| {
            entry.changes.iter().any(|change| {
                change.field_name == "status"
                    && aida_core::lifecycle::State::from_status_str(&change.old_value)
                        .is_some_and(|s| s.is_terminal())
                    && aida_core::lifecycle::State::from_status_str(&change.new_value)
                        .is_some_and(|s| !s.is_terminal())
            })
        })
        .map(|entry| entry.timestamp)
        .max()
}

// trace:BUG-1112 | ai:codex
pub(crate) fn spec_latest_reopen_transition_at(
    project_root: &std::path::Path,
    spec: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    use aida_core::DatabaseBackend;
    let store_path = project_root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_path).ok()?;
    let req = backend.get_requirement_by_spec_id(spec).ok()??;
    latest_reopen_transition_at(&req)
}

// trace:BUG-1112 | ai:codex
pub(crate) fn merged_at_satisfies_latest_reopen(
    merged_at: Option<chrono::DateTime<chrono::Utc>>,
    latest_reopen_at: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    match latest_reopen_at {
        None => true,
        Some(reopen_at) => merged_at.is_some_and(|merged_at| merged_at > reopen_at),
    }
}

/// Pure decision for the BUG-241 reconcile (`RealPhaseDriver::reconcile_failure`):
/// given the two ground-truth signals — a verified merged PR number, if one
/// was found, and whether the spec reached Completed — decide whether a phase
/// failure is genuine or an out-of-band success. A merged PR reaches this
/// helper only after the caller verifies that the PR credits the dispatched
/// spec; that guard prevents BUG-357's wrong-spec reconcile false positive.
/// Either verified signal alone is proof the spec shipped; needing both would
/// miss instance A (the status auto-bump lags a human's out-of-band merge) and
/// instance B (a no-work spec never gets a PR). Split out so the rule is
/// unit-testable without `gh` or a store.
/// trace:BUG-241 | ai:claude
pub(crate) fn reconcile_verdict(
    verified_merged_pr: Option<u32>,
    spec_completed: bool,
    spec: &str,
) -> auto_complete::PhaseReconcile {
    use auto_complete::PhaseReconcile;
    match (verified_merged_pr, spec_completed) {
        (Some(pr), true) => PhaseReconcile::ShippedOutOfBand {
            reason: format!("PR-{pr} is merged and {spec} is Completed"),
        },
        (Some(pr), false) => PhaseReconcile::ShippedOutOfBand {
            reason: format!("PR-{pr} is already merged"),
        },
        (None, true) => PhaseReconcile::ShippedOutOfBand {
            reason: format!("{spec} is Completed — the spec needed no further work"),
        },
        // Regression guard: no merged PR and the spec is still open — reality
        // confirms nothing shipped, so the phase failure stands. trace:BUG-241
        (None, false) => PhaseReconcile::GenuineFailure,
    }
}

#[cfg(test)]
#[path = "tests/reconcile_verdict_tests.rs"]
mod reconcile_verdict_tests;

/// TASK-136 / BUG-420: drain-reliability tunables for the orchestrator's phase
/// loop. Resolved once per `RealPhaseDriver` from the `[drain]` section of
/// `.aida/config.toml`, then overridden by env vars (which the `queue work`
/// dispatch sets from the `--no-progress-minutes` / `--phase-ceiling-minutes`
/// flags so the values compose across single-spec / batch / nextN drains
/// without threading through every signature — the same env-var pattern
/// `AIDA_CALIBRATE` uses). A `0`-minute value disables that watchdog check.
/// trace:TASK-136 BUG-420 | ai:claude
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrainTuning {
    /// TASK-136: how many times to retry the phase-1 GH PR-verify on a
    /// transient GH-API outage before declaring the run inconclusive. The
    /// backoff cadence is [`auto_complete::gh_verify_backoff_schedule`]
    /// (30s / 1m / 5m / then 15m). Default 3.
    pub(crate) gh_verify_retries: usize,
    /// BUG-420: no-progress window — a headless phase that makes no commit /
    /// file-change for this long is killed and shelved. Default 10m; `0`
    /// disables.
    pub(crate) no_progress: std::time::Duration,
    /// BUG-420: hard wall-clock ceiling per headless phase, a backstop in case
    /// progress-detection misses. Default 45m; `0` disables.
    pub(crate) ceiling: std::time::Duration,
    /// STORY-998: low-information output must repeat for this long before the
    /// drain shelves it as a spinning session. Default 90s; `0` disables.
    pub(crate) spinning_after: std::time::Duration,
    /// STORY-998: high line rate trips runaway before the window is pruned.
    pub(crate) runaway_rate: f64,
    /// STORY-998: maximum Shannon entropy for the repeated-template window.
    pub(crate) low_entropy_bits: f64,
    /// STORY-998: no information-bearing line for this long is Quiet.
    pub(crate) progress_activity: std::time::Duration,
    /// TASK-975: CI auto-fix budget — how many in-drain headless fix cycles
    /// phase 2 may attempt on a red CI run before the failure proceeds to
    /// shelve. Also arms the phase-4 merge-conflict rebase when > 0.
    /// Default 0 (off — red CI shelves immediately, the pre-TASK-975
    /// behaviour).
    pub(crate) ci_auto_fix: usize,
    /// STORY-975: whole-phase retry budget for transient shelve causes.
    /// Default 1 retry, clamped to 3.
    // trace:STORY-975 | ai:codex
    pub(crate) retry_transient: usize,
    /// STORY-1033: retry attempt 2 may move to the next configured model tier.
    /// Defaults true; `[drain] retry_escalate_model = false` disables it.
    pub(crate) retry_escalate_model: bool,
    /// STORY-1041: in-flight member window for pipelined drains. Clamped 1..3.
    // trace:STORY-1041 trace:ADR-27 | ai:codex
    pub(crate) pipeline_depth: usize,
}

impl DrainTuning {
    /// Resolve from `[drain]` config then env overrides. Env wins so the
    /// per-invocation flags (mapped to env at dispatch) override the project
    /// default. trace:TASK-136 BUG-420 | ai:claude
    pub(crate) fn resolve(project_root: &std::path::Path) -> Self {
        let cfg = read_drain_config(project_root);
        let env_usize = |k: &str| -> Option<usize> {
            std::env::var(k).ok().and_then(|s| s.trim().parse().ok())
        };
        let env_u64 =
            |k: &str| -> Option<u64> { std::env::var(k).ok().and_then(|s| s.trim().parse().ok()) };
        let env_f64 =
            |k: &str| -> Option<f64> { std::env::var(k).ok().and_then(|s| s.trim().parse().ok()) };
        let gh_verify_retries = env_usize("AIDA_GH_VERIFY_RETRIES")
            .or(cfg.gh_verify_retries)
            .unwrap_or(3);
        let no_progress_min = env_u64("AIDA_NO_PROGRESS_MINUTES")
            .or(cfg.no_progress_minutes)
            .unwrap_or(10);
        let ceiling_min = env_u64("AIDA_PHASE_CEILING_MINUTES")
            .or(cfg.phase_ceiling_minutes)
            .unwrap_or(45);
        let idle_defaults = aida_core::idle::IdleConfig::default();
        let spinning_after_secs = env_u64("AIDA_SPINNING_AFTER_SECS")
            .or(cfg.spinning_after)
            .unwrap_or(idle_defaults.spinning_after.as_secs());
        let runaway_rate = env_f64("AIDA_RUNAWAY_RATE")
            .or(cfg.runaway_rate)
            .unwrap_or(idle_defaults.runaway_rate);
        let low_entropy_bits = env_f64("AIDA_LOW_ENTROPY_BITS")
            .or(cfg.low_entropy_bits)
            .unwrap_or(idle_defaults.low_entropy_bits);
        let progress_activity_secs = env_u64("AIDA_PROGRESS_ACTIVITY_SECS")
            .or(cfg.progress_activity)
            .unwrap_or(idle_defaults.progress_activity.as_secs());
        // trace:TASK-975 | ai:claude
        let ci_auto_fix = env_usize("AIDA_CI_AUTO_FIX")
            .or(cfg.ci_auto_fix)
            .unwrap_or(0);
        let retry_transient = auto_complete::clamp_transient_retry_budget(
            env_usize("AIDA_RETRY_TRANSIENT")
                .or(cfg.retry_transient)
                .unwrap_or(1),
        );
        let retry_escalate_model = std::env::var("AIDA_RETRY_ESCALATE_MODEL")
            .ok()
            .and_then(|s| parse_boolish(&s))
            .or(cfg.retry_escalate_model)
            .unwrap_or(true);
        let pipeline_depth = drain_state::clamp_pipeline_depth(
            env_usize("AIDA_DRAIN_PIPELINE_DEPTH")
                .or(cfg.pipeline_depth)
                .unwrap_or_else(drain_state::default_pipeline_depth),
        );
        Self {
            gh_verify_retries,
            no_progress: std::time::Duration::from_secs(no_progress_min.saturating_mul(60)),
            ceiling: std::time::Duration::from_secs(ceiling_min.saturating_mul(60)),
            spinning_after: std::time::Duration::from_secs(spinning_after_secs),
            runaway_rate,
            low_entropy_bits,
            progress_activity: std::time::Duration::from_secs(progress_activity_secs),
            ci_auto_fix,
            retry_transient,
            retry_escalate_model,
            pipeline_depth,
        }
    }

    // trace:STORY-998 | ai:codex
    pub(crate) fn idle_config(&self) -> aida_core::idle::IdleConfig {
        aida_core::idle::IdleConfig {
            spinning_after: self.spinning_after,
            runaway_rate: self.runaway_rate,
            low_entropy_bits: self.low_entropy_bits,
            progress_activity: self.progress_activity,
            ..aida_core::idle::IdleConfig::default()
        }
    }

    // trace:STORY-1041 trace:ADR-27 | ai:codex
    pub(crate) fn pipeline_depth(&self) -> usize {
        self.pipeline_depth
    }
}

/// Format the periodic liveness heartbeat an otherwise-silent headless phase
/// emits every ~30s — actor + spec + elapsed + time since the last observed
/// activity. The watchdog already computes "time since last progress" to decide
/// when to KILL a degenerate session; this turns that same signal into
/// reassurance so the operator can tell "working hard" from "hung". Pure so the
/// elapsed / since-progress formatting is pinned by a unit test (no real drive).
// trace:STORY-726 | ai:claude
pub(crate) fn phase_heartbeat_line(
    actor: &str,
    spec: &str,
    elapsed: std::time::Duration,
    since_progress: std::time::Duration,
) -> String {
    let e = elapsed.as_secs();
    format!(
        "{actor} working on {spec} ({}m {}s elapsed, last activity {}s ago)",
        e / 60,
        e % 60,
        since_progress.as_secs(),
    )
}

/// BUG-875: progress signals are phase-shaped. Implementers can prove progress
/// through the worktree or the stream log; reviewers are read-then-verdict, so
/// their no-progress watchdog must key on stream output activity only.
// trace:BUG-875 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchdogProgressSignal {
    WorktreeAndOutput,
    OutputOnly,
}

// trace:BUG-875 | ai:codex
pub(crate) fn watchdog_progress_signal_for_phase(
    phase: auto_complete::Phase,
) -> WatchdogProgressSignal {
    match phase {
        auto_complete::Phase::Reviewer => WatchdogProgressSignal::OutputOnly,
        _ => WatchdogProgressSignal::WorktreeAndOutput,
    }
}

/// BUG-420: a phase-scoped no-progress + wall-clock-ceiling watchdog for a
/// *headless* orchestrator phase. While the `claude -p` child runs, the
/// orchestrator's [`exit_signal::spawn_and_wait_watched`] poll loop calls
/// [`PhaseWatchdog::check`]; it (rate-limited) probes the phase worktree for
/// the phase-appropriate progress signal and trips when it stalls for
/// `no_progress` or when wall-clock exceeds `ceiling`. The pure verdict is
/// [`auto_complete::watchdog_verdict`]; this struct is just the I/O shell that
/// gathers the facts and remembers the last-progress timestamp.
///
/// The worktree is created by the phase child, so it is not known at spawn
/// time — `check` resolves it lazily from the session lease and, until it
/// appears, treats the phase as "making progress" (startup grace) so a slow
/// session launch never trips the watchdog. trace:BUG-420 | ai:claude
pub(crate) struct PhaseWatchdog {
    pub(crate) project_root: std::path::PathBuf,
    pub(crate) session_id: String,
    pub(crate) vendor: session::HeadlessVendor,
    pub(crate) root_pid: Option<u32>,
    pub(crate) phase_start: std::time::Instant,
    pub(crate) last_progress: std::time::Instant,
    pub(crate) last_sig: Option<String>,
    pub(crate) last_poll: std::time::Instant,
    pub(crate) poll_every: std::time::Duration,
    pub(crate) no_progress: std::time::Duration,
    pub(crate) ceiling: std::time::Duration,
    pub(crate) worktree: Option<std::path::PathBuf>,
    pub(crate) pr_ship_wait_seen: bool,
    pub(crate) progress_signal: WatchdogProgressSignal,
    pub(crate) idle_detector: aida_core::idle::IdleDetector,
    pub(crate) idle_log_pos: u64,
    /// STORY-726: when `Some((actor, spec))`, the watchdog also emits a
    /// liveness heartbeat to stderr on each poll tick where it does NOT trip —
    /// reassurance that an otherwise-silent headless phase is alive. Set only
    /// when the phase would run silent (headless + teeing off); a teed or
    /// interactive phase already streams its own progress.
    // trace:STORY-726 | ai:claude
    pub(crate) heartbeat: Option<(String, String)>,
    /// BUG-1299: the recovery hint [`Self::trip_reason`] resolved alongside
    /// the just-returned trip detail, in the same `watchdog_trip_report`
    /// call — the caller `take`s it right after `check()` reports a trip and
    /// attaches it to the `PhaseFailure` as `hint_override`, so the hint the
    /// operator sees is guaranteed to describe the SAME trip as the detail.
    // trace:BUG-1299 | ai:claude
    pub(crate) last_trip_hint: Option<String>,
}

impl PhaseWatchdog {
    /// How often to actually shell out to git — far coarser than the
    /// `spawn_and_wait` poll cadence (which is ~100ms) so the watchdog never
    /// spins git. trace:BUG-420 | ai:claude
    const GIT_POLL: std::time::Duration = std::time::Duration::from_secs(30);

    pub(crate) fn new(
        project_root: std::path::PathBuf,
        session_id: String,
        vendor: session::HeadlessVendor,
        no_progress: std::time::Duration,
        ceiling: std::time::Duration,
    ) -> Self {
        let now = std::time::Instant::now();
        let idle_defaults = aida_core::idle::IdleConfig::default();
        Self {
            project_root,
            session_id,
            vendor,
            root_pid: None,
            phase_start: now,
            last_progress: now,
            last_sig: None,
            last_poll: now,
            poll_every: Self::GIT_POLL,
            no_progress,
            ceiling,
            worktree: None,
            pr_ship_wait_seen: false,
            progress_signal: WatchdogProgressSignal::WorktreeAndOutput,
            idle_detector: aida_core::idle::IdleDetector::new(idle_defaults),
            idle_log_pos: 0,
            heartbeat: None,
            last_trip_hint: None,
        }
    }

    // trace:STORY-998 | ai:codex
    pub(crate) fn with_idle_config(mut self, cfg: aida_core::idle::IdleConfig) -> Self {
        self.idle_detector = aida_core::idle::IdleDetector::new(cfg);
        self
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn new_for_phase(
        project_root: std::path::PathBuf,
        session_id: String,
        vendor: session::HeadlessVendor,
        no_progress: std::time::Duration,
        ceiling: std::time::Duration,
        idle_config: aida_core::idle::IdleConfig,
        phase: auto_complete::Phase,
    ) -> Self {
        Self::new(project_root, session_id, vendor, no_progress, ceiling)
            .with_progress_signal(watchdog_progress_signal_for_phase(phase))
            .with_idle_config(idle_config)
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn with_progress_signal(mut self, progress_signal: WatchdogProgressSignal) -> Self {
        self.progress_signal = progress_signal;
        self
    }

    /// STORY-726: arm the periodic liveness heartbeat. `actor` is the phase
    /// noun ("implementer"), `spec` the SPEC-ID being worked. Call only when the
    /// phase would otherwise be silent so the heartbeat never double-prints over
    /// a teed/interactive child.
    // trace:STORY-726 | ai:claude
    pub(crate) fn with_heartbeat(
        mut self,
        actor: impl Into<String>,
        spec: impl Into<String>,
    ) -> Self {
        self.heartbeat = Some((actor.into(), spec.into()));
        self
    }

    /// Emit one liveness heartbeat (when armed). Mirrors the CI-wait phase's
    /// `CI still running …` reassurance so an unattended drive is never dark.
    // trace:STORY-726 | ai:claude
    pub(crate) fn emit_heartbeat(
        &self,
        since_progress: std::time::Duration,
        total: std::time::Duration,
    ) {
        if let Some((actor, spec)) = &self.heartbeat {
            eprintln!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                phase_heartbeat_line(actor, spec, total, since_progress),
            );
        }
    }

    /// A progress signature for the worktree: HEAD sha + a hash of the
    /// porcelain status + the newest mtime among changed files. A degenerate
    /// echo/sleep spin moves none of these; a real session commits, stages, or
    /// re-edits a file (advancing its mtime). `None` when git can't be read.
    /// trace:BUG-420 | ai:claude
    pub(crate) fn progress_signature(worktree: &std::path::Path) -> Option<String> {
        let sha = git_capture(worktree, &["rev-parse", "HEAD"]).unwrap_or_default();
        let porcelain = git_capture(worktree, &["status", "--porcelain"]).unwrap_or_default();
        // Newest mtime among the changed paths — catches repeated edits to an
        // already-dirty file (porcelain text alone would not change).
        let mut newest: u128 = 0;
        for line in porcelain.lines() {
            // Porcelain lines are `XY <path>`; take everything after the status
            // columns. Rename lines (`R  old -> new`) end with the new path.
            let path = line
                .get(3..)
                .unwrap_or("")
                .rsplit(" -> ")
                .next()
                .unwrap_or("");
            let path = path.trim().trim_matches('"');
            if path.is_empty() {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(worktree.join(path)) {
                if let Ok(m) = meta.modified() {
                    if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                        newest = newest.max(d.as_nanos());
                    }
                }
            }
        }
        if sha.is_empty() && porcelain.is_empty() {
            return None;
        }
        Some(format!("{sha}|{}|{newest}", porcelain.len()))
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn select_progress_signature(
        progress_signal: WatchdogProgressSignal,
        worktree_sig: Option<String>,
        output_sig: Option<String>,
    ) -> Option<String> {
        match progress_signal {
            WatchdogProgressSignal::OutputOnly => output_sig,
            WatchdogProgressSignal::WorktreeAndOutput => match (worktree_sig, output_sig) {
                (Some(w), Some(o)) => Some(format!("{w}|{o}")),
                (Some(w), None) => Some(w),
                (None, Some(o)) => Some(o),
                (None, None) => None,
            },
        }
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn observed_progress_signature(&self, worktree: &std::path::Path) -> Option<String> {
        let ctx = vendor_activity::VendorActivityContext::new(&self.project_root, &self.session_id);
        let output_sig = vendor_activity::snapshot(self.vendor, &ctx).signature();
        let worktree_sig = match self.progress_signal {
            WatchdogProgressSignal::WorktreeAndOutput => Self::progress_signature(worktree),
            WatchdogProgressSignal::OutputOnly => None,
        };
        Self::select_progress_signature(self.progress_signal, worktree_sig, output_sig)
    }

    // trace:STORY-998 | ai:codex
    pub(crate) fn idle_verdict_from_log(
        &mut self,
        now: std::time::Instant,
    ) -> aida_core::idle::IdleVerdict {
        let Some(path) = headless_log_path_for_session(&self.project_root, &self.session_id) else {
            return self.idle_detector.verdict(now);
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return self.idle_detector.verdict(now);
        };
        let len = meta.len();
        if len < self.idle_log_pos {
            self.idle_log_pos = 0;
        }
        if len > self.idle_log_pos {
            if let Ok(mut file) = std::fs::File::open(&path) {
                use std::io::{BufRead, Seek};
                let _ = file.seek(std::io::SeekFrom::Start(self.idle_log_pos));
                let mut reader = std::io::BufReader::new(file);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let Ok(n) = reader.read_until(b'\n', &mut line) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    self.idle_detector.feed_bytes(&line, now);
                }
                if let Ok(pos) = reader.stream_position() {
                    self.idle_log_pos = pos;
                } else {
                    self.idle_log_pos = len;
                }
            }
        }
        self.idle_detector.verdict(now)
    }

    /// Probe (rate-limited) and return `Some(reason)` if the watchdog should
    /// trip — the caller then reaps the child. trace:BUG-420 | ai:claude
    pub(crate) fn check(&mut self) -> Option<String> {
        // Both kill-checks disabled AND no heartbeat to emit → nothing to do.
        // STORY-726: with the heartbeat armed we still poll (on the same coarse
        // cadence) so a silent headless phase stays visibly alive even if a user
        // disabled the no-progress / ceiling kills.
        if self.no_progress.is_zero() && self.ceiling.is_zero() && self.heartbeat.is_none() {
            return None;
        }
        let now = std::time::Instant::now();
        if now.duration_since(self.last_poll) < self.poll_every {
            return None;
        }
        self.last_poll = now;

        // Lazily resolve the worktree from the session lease. Until it appears
        // (the child is still starting up), keep resetting last_progress so a
        // slow launch can't trip the no-progress check.
        if self.worktree.is_none() {
            if let Some((_, _, wt, creator_pid)) =
                find_orchestrated_lease(&self.project_root, &self.session_id)
            {
                self.worktree = Some(wt);
                self.root_pid = creator_pid;
            }
            if self.worktree.is_none() {
                self.last_progress = now;
                // Ceiling still applies even before the worktree resolves.
                return auto_complete::watchdog_verdict(
                    now.duration_since(self.last_progress),
                    now.duration_since(self.phase_start),
                    self.no_progress,
                    self.ceiling,
                )
                .map(|t| self.trip_reason(t));
            }
        }
        let worktree = self.worktree.clone().unwrap();

        // BUG-453 / BUG-875: implementers use worktree change OR output
        // activity; reviewers use output activity only because their normal
        // work is read-then-verdict with no intermediate file edits.
        let combined = self.observed_progress_signature(&worktree);
        if let Some(sig) = combined {
            if self.last_sig.as_deref() != Some(sig.as_str()) {
                self.last_sig = Some(sig);
                self.last_progress = now;
            }
        }
        // BUG-749: once the implementer has opened a PR and is blocked inside
        // `aida pr ship` watching CI, worktree/log movement can legitimately
        // stop. A live pr-ship descendant is therefore progress for the
        // no-progress watchdog; the phase ceiling still applies.
        // trace:BUG-749 | ai:codex
        let pr_ship_waiting = self.root_pid.is_some_and(live_aida_pr_ship_descendant);
        if pr_ship_waiting {
            self.pr_ship_wait_seen = true;
            self.last_progress = now;
        }

        // STORY-998: a session can produce output forever without adding
        // information (e.g. status/redraw loops). Classify the stream itself,
        // but preserve the BUG-749 CI wait exemption.
        if !pr_ship_waiting {
            if let aida_core::idle::IdleVerdict::Spinning { template, count } =
                self.idle_verdict_from_log(now)
            {
                return Some(
                    self.trip_reason(auto_complete::WatchdogTrip::Spinning { template, count }),
                );
            }
        }

        // TASK-298: a `--no-human` headless run that hit a permission gate (or
        // reported an `is_error` envelope) is silently stuck — `claude -p`
        // exits 0 even when it bailed (SPIKE-7), so neither the natural-exit
        // path nor the no-progress timer would catch it promptly. The log
        // keeps the offending event, so scan it and trip the watchdog
        // *immediately* with the specific reason rather than waiting out the
        // no-progress / ceiling window. trace:TASK-298 | ai:claude
        if let Some(reason) = read_headless_log_for_session(&self.project_root, &self.session_id)
            .as_deref()
            .and_then(headless_log_permission_stall)
        {
            return Some(reason);
        }

        let since_progress = now.duration_since(self.last_progress);
        let total = now.duration_since(self.phase_start);
        let verdict = auto_complete::watchdog_verdict_with_ci_wait(
            since_progress,
            total,
            self.no_progress,
            self.ceiling,
            pr_ship_waiting,
        );
        // STORY-726: not tripping → the phase is alive; emit the reassurance
        // heartbeat (when armed) using the very signal the watchdog would
        // otherwise only weaponize to kill.
        if verdict.is_none() {
            self.emit_heartbeat(since_progress, total);
        }
        verdict.map(|t| self.trip_reason(t))
    }

    /// BUG-1299: resolve BOTH the failure detail (returned) and the recovery
    /// hint (stashed on `self.last_trip_hint`, fetched by the caller via
    /// [`Self::take_trip_hint`]) from `auto_complete::watchdog_trip_report` —
    /// one function, one match on `trip`, so the two strings the operator
    /// eventually sees cannot name different trips.
    // trace:BUG-1299 | ai:claude
    pub(crate) fn trip_reason(&mut self, trip: auto_complete::WatchdogTrip) -> String {
        let output_only = matches!(self.progress_signal, WatchdogProgressSignal::OutputOnly);
        let (detail, hint) = auto_complete::watchdog_trip_report(
            &trip,
            self.no_progress,
            self.ceiling,
            self.pr_ship_wait_seen,
            output_only,
        );
        self.last_trip_hint = Some(hint);
        detail
    }

    /// Take the recovery hint resolved alongside the most recent trip detail
    /// [`Self::check`] returned. `None` once taken, or if the watchdog never
    /// tripped.
    // trace:BUG-1299 | ai:claude
    pub(crate) fn take_trip_hint(&mut self) -> Option<String> {
        self.last_trip_hint.take()
    }
}

/// BUG-749: recognize commands equivalent to `aida pr ship`, including
/// explicit binary paths (`target/debug/aida pr ship`) and shell wrappers
/// (`bash -lc 'aida pr ship 123'`). Kept pure so the watchdog regression does
/// not need to spawn or block real processes.
// trace:BUG-749 | ai:codex
pub(crate) fn command_line_runs_aida_pr_ship(parts: &[String]) -> bool {
    if parts.len() < 3 {
        return false;
    }
    let mut words = Vec::new();
    for part in parts {
        words.extend(part.split_whitespace().map(str::to_string));
    }
    words.windows(3).any(|w| {
        std::path::Path::new(&w[0])
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "aida")
            && w[1] == "pr"
            && w[2] == "ship"
    })
}

/// Best-effort local process-table probe for a live `aida pr ship` descendant
/// under the orchestrated implementer session. Failure to inspect the table is
/// conservative: no CI-wait signal, so the existing watchdog behavior remains.
// trace:BUG-749 | ai:codex
pub(crate) fn live_aida_pr_ship_descendant(root_pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

    let root = Pid::from_u32(root_pid);
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());

    let mut tree = vec![root];
    let mut idx = 0;
    while idx < tree.len() {
        let parent = tree[idx];
        idx += 1;
        for (pid, proc_) in sys.processes() {
            if proc_.parent() == Some(parent) && !tree.contains(pid) {
                tree.push(*pid);
                let parts: Vec<String> = proc_.cmd().iter().map(|p| p.to_string()).collect();
                if command_line_runs_aida_pr_ship(&parts) {
                    return true;
                }
            }
        }
    }
    false
}
