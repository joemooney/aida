//! Detect an unchanged, still blocked PR before a standalone lane reports success.
// trace:BUG-2452 | ai:codex
use crate::launch;
use aida_core::{RequirementStatus, Storage};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub(super) const EXIT_CODE: i32 = 35;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrState {
    state: String,
    head_ref_oid: String,
    mergeable: String,
}

fn output(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new("gh");
    cmd.current_dir(root).args(args);
    crate::command_output_with_timeout(cmd, Duration::from_secs(10))
        .ok_or_else(|| anyhow::anyhow!("empty-rework forge probe timed out"))
}

fn state(root: &Path, pr: u64) -> Result<PrState> {
    let out = output(
        root,
        &[
            "pr",
            "view",
            &pr.to_string(),
            "--json",
            "state,headRefOid,mergeable",
        ],
    )?;
    anyhow::ensure!(
        out.status.success(),
        "cannot read rework PR: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn reasons(
    state: &PrState,
    checks: &[crate::ci_gate::CheckRow],
    verdicts: &[crate::gate::review_verdict::RecordedVerdict],
) -> Vec<String> {
    let mut reasons = Vec::new();
    if state.mergeable == "CONFLICTING" {
        reasons.push("mergeable=CONFLICTING: resolve the merge conflicts".into());
    }
    for check in checks
        .iter()
        .filter(|c| matches!(c.bucket.as_str(), "fail" | "cancel"))
    {
        reasons.push(format!("required check failed: {}", check.name));
    }
    for verdict in verdicts.iter().filter(|v| {
        v.kind.requires_changes()
            && v.closed_by_merge.is_none()
            && v.reviewed_sha.as_deref().is_some_and(|sha| {
                crate::gate::review_verdict::same_reviewed_sha(sha, &state.head_ref_oid)
            })
    }) {
        reasons.push(format!(
            "blocking review: {}\n{}",
            verdict
                .summary
                .as_deref()
                .unwrap_or(verdict.kind.canonical().unwrap_or("unknown")),
            verdict.findings.join("\n")
        ));
    }
    reasons
}

/// Signals take precedence; a fresh PR or a pushed head never enters this guard.
/// Unknown local push evidence skips classification: the forge can lag a push.
/// Two forge head reads bind the checks to the same head, rather than diagnosing
/// a head that changed while evidence was collected. Unknown forge reads error.
pub(super) fn inspect(
    root: &Path,
    spec: &str,
    launch: &super::LaunchSeat<'_>,
    outcome: &super::Outcome,
    seat: &super::SeatState,
) -> Result<Option<(u64, String, String)>> {
    if matches!(
        outcome,
        super::Outcome::Punted { .. } | super::Outcome::Held { .. }
    ) {
        return Ok(None);
    }
    let Some((pr, before)) = launch.pr else {
        return Ok(None);
    };
    if seat.pushed_since(launch.head) != Some(false) {
        return Ok(None);
    }
    // These PR-specific states and required-check semantics are GitHub's.
    if crate::forge::resolve_open_change_forge_kind(root) != crate::forge::ForgeKind::GitHub {
        return Ok(None);
    }
    let before = before
        .as_ref()
        .context("rework launch PR head was unknown")?;
    let after = state(root, *pr)?;
    if after.state != "OPEN" || after.head_ref_oid != *before {
        return Ok(None);
    }
    let out = output(
        root,
        &[
            "pr",
            "checks",
            &pr.to_string(),
            "--required",
            "--json",
            "name,bucket,workflow,link",
        ],
    )?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut checks = if stdout.trim().starts_with('[') {
        crate::ci_gate::parse_check_rows(&stdout).context("invalid required-check rows")?
    } else if crate::pr_ship::gh_pr_checks_output_is_unregistered(&stdout, &stderr) {
        Vec::new()
    } else {
        crate::forge::required_checks_from_output(out.status.success(), &stdout, &stderr)?;
        Vec::new()
    };
    // Local readiness policy may require checks even without forge branch
    // protection (this repository's aggregate Build check is one example).
    let configured = crate::review_watch::required_checks(root)?;
    if !configured.is_empty() {
        let all = output(
            root,
            &[
                "pr",
                "checks",
                &pr.to_string(),
                "--json",
                "name,bucket,workflow,link",
            ],
        )?;
        let stdout = String::from_utf8_lossy(&all.stdout);
        let rows = if crate::pr_ship::gh_pr_checks_output_is_unregistered(
            &stdout,
            &String::from_utf8_lossy(&all.stderr),
        ) {
            Vec::new()
        } else {
            crate::ci_gate::parse_check_rows(&stdout)
                .context("invalid configured-check evidence")?
        };
        checks.extend(rows.into_iter().filter(|row| {
            configured
                .iter()
                .any(|name| crate::ci_gate::check_name_eq(name, &row.name))
        }));
    }
    anyhow::ensure!(
        checks.iter().all(|c| !c.name.trim().is_empty()
            && matches!(
                c.bucket.as_str(),
                "pass" | "fail" | "cancel" | "pending" | "skipping"
            )),
        "invalid required-check evidence"
    );
    let verdicts = [spec.to_string(), format!("PR-{pr}")]
        .iter()
        .filter_map(|key| crate::gate::review_verdict::read_recorded_verdict(root, key))
        .collect::<Vec<_>>();
    let reasons = reasons(&after, &checks, &verdicts);
    if reasons.is_empty() {
        return Ok(None);
    }
    let confirmed = state(root, *pr)?;
    if confirmed.state != "OPEN" || confirmed.head_ref_oid != *before {
        return Ok(None);
    }
    Ok(Some((*pr, before.clone(), reasons.join("\n"))))
}

/// The retry receipt is keyed by immutable requirement identity, PR and head.
/// A permanent lock serializes concurrent exits. A changed head starts a new
/// allowance; repeated exits at the same head never grow an unbounded retry loop.
pub(super) fn route(
    root: &Path,
    spec: &str,
    vendor: &str,
    pr: u64,
    head: &str,
    reason: &str,
) -> Result<bool> {
    use fs2::FileExt;
    let storage = Storage::new(
        crate::detect_distributed_store_from(root)
            .ok_or_else(|| anyhow::anyhow!("rework store unavailable"))?,
    );
    let store = storage.load()?;
    let req = store
        .get_requirement_unambiguous(spec)?
        .ok_or_else(|| anyhow::anyhow!("rework spec unavailable"))?;
    let dir = root.join(".aida/empty-rework");
    std::fs::create_dir_all(&dir)?;
    let key = format!("{}-{pr}", req.id);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(format!("{key}.lock")))?;
    lock.try_lock_exclusive()
        .context("another empty-rework exit is being routed; retry the pickup")?;
    let receipt = dir.join(format!("{key}.json"));
    let previous: Option<String> = match std::fs::read(&receipt) {
        Ok(body) => Some(serde_json::from_slice(&body)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let second = previous.as_deref() == Some(head);
    let note = format!("Empty rework of PR-{pr} at {head}: no new pushed commit.\n{reason}\nAddress these blockers, commit and push. If disputing a finding, use `aida punt` and name that finding.");
    let mut retryable = false;
    let store = storage.update_atomically(|store| {
        if let Some(live) = store.requirements.iter_mut().find(|r| r.id == req.id) {
            if !matches!(
                live.status,
                RequirementStatus::Completed
                    | RequirementStatus::Rejected
                    | RequirementStatus::Superseded
            ) {
                retryable = true;
                aida_core::conflict::set_status_recorded(
                    live,
                    if second {
                        RequirementStatus::NeedsAttention
                    } else {
                        RequirementStatus::InProgress
                    },
                    "empty-rework",
                );
            }
        }
    })?;
    anyhow::ensure!(
        retryable,
        "empty-rework spec disappeared or became terminal; no automatic retry queued"
    );
    if second {
        let message = advisor_message(spec, req.id, pr, head, &note);
        let path = crate::mailbox_store::mailbox_dir(root).join(format!("{}.json", message.id));
        if !path.try_exists()? {
            crate::mailbox_store::write_message(root, &message)?;
        }
        // Leave no automatic retry behind after escalation, including a
        // producer's personal queue row routed to the implementer role.
        for user in storage.queue_users()? {
            storage.queue_remove_for_role(&user, &req.id, Some("implementer"))?;
        }
    } else {
        let vendor = crate::brief::validate_brief_agent(vendor)?;
        let dir = root.join(".aida/agent-briefs").join(vendor);
        std::fs::create_dir_all(&dir)?;
        let stamp = chrono::Utc::now()
            .format("%Y-%m-%dT%H%M%S%.9fZ")
            .to_string();
        let spec_id = req.spec_id.as_deref().unwrap_or(spec);
        let path = dir.join(format!("{spec_id}-{stamp}.md"));
        let body = crate::brief::render_agent_brief(
            root,
            vendor,
            spec_id,
            req,
            &store,
            &stamp,
            Some(&note),
            None,
            None,
        );
        aida_core::write_atomic(&path, body)?;
        crate::brief::add_pending_brief(root, vendor, &path)?;
        // Consolidate producer-owned implementer rows into the shared retry.
        for user in storage.queue_users()? {
            if user != "role:implementer" {
                storage.queue_remove_for_role(&user, &req.id, Some("implementer"))?;
            }
        }
        // Reserve before publishing the queue row: a crash must not grant
        // another automatic retry. Publication failures remain non-success.
        aida_core::write_atomic(&receipt, serde_json::to_vec(head)?)?;
        storage.queue_add(aida_core::QueueEntry {
            user_id: "role:implementer".into(),
            requirement_id: req.id,
            position: 0,
            added_by: "empty-rework".into(),
            note: Some(note),
            added_at: chrono::Utc::now(),
            for_role: Some("implementer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })?;
    }
    Ok(second)
}

fn advisor_message(
    spec: &str,
    requirement: uuid::Uuid,
    pr: u64,
    head: &str,
    note: &str,
) -> aida_core::mailbox::Message {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    let id = format!("empty-rework-{requirement}-{pr}-{head}");
    Message {
        id: id.clone(), thread_id: id, from: "empty-rework".into(), to: Recipient::Agent("advisor".into()),
        timestamp: chrono::Utc::now().timestamp_millis(), in_reply_to: None,
        subject: Some(format!("{spec}: second empty rework of PR-{pr}")),
        body: format!("The automatic retry also exited without moving the blocked head. Advisor decision requested.\n{note}"),
        urgent: false, intent: Intent::Request, retracted: false, deleted: false, archived: false,
        from_source: SenderSource::Explicit, from_role: None, relayed_from: None,
    }
}

#[cfg(test)]
#[path = "tests/bug_2452_empty_rework_tests.rs"]
mod tests;
