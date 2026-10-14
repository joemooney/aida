//! Narrow, recorded release of nonsensitive Rework-only holds.
//! Shares today's record-time seat/grant check. It does not authenticate
//! same-user JSON against forgery or provide the pending authorship ledger.
//! Hold writes are atomic; there is no existing hold lock. Concurrent re-arm
//! serialization is deferred by the signed slice; re-read before release.
// trace:STORY-1687 | ai:codex
use crate::{merge_hold as mh, review_verdict as rv, seat_authority as sa};
use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, FixedOffset};
use serde_json::{Map, Value};
use std::path::Path;

const PRINCIPAL: &str = "auto-released:verdict";

/// The same implementer exclusion used by review record on current main.
pub(crate) fn record_seat_allowed(seat: Option<&str>) -> bool {
    seat != Some("implementer")
}

/// Only the supported record handler stamps the grant, after its authority check.
pub(crate) fn stamp_record_grant(root: &Path, obj: &mut Map<String, Value>) {
    obj.remove("release_grant");
    if let sa::GrantCheck::Valid(grant) = sa::grant_check(root) {
        obj.insert(
            "release_grant".into(),
            serde_json::json!({
                "id": grant.id, "subject": grant.subject, "session_id": grant.session_id,
                "recorded_by": obj.get("recorded_by")
            }),
        );
    }
}

fn sensitive(req: &aida_core::Requirement) -> Option<String> {
    use aida_core::ExecutionMode;
    if matches!(
        req.execution_mode,
        Some(ExecutionMode::Guided | ExecutionMode::Operator)
    ) {
        return Some("guided/operator execution requires a human clear".into());
    }
    if crate::presence::is_keystone_class(
        &req.req_type.to_string(),
        req.tags.iter().map(String::as_str),
    ) {
        return Some("keystone-class spec requires a human clear".into());
    }
    let terms = [
        "sandbox",
        "contained",
        "confinement",
        "security",
        "authority",
        "grant",
        "identity",
        "credential",
        "secret",
    ];
    for text in std::iter::once(&req.title).chain(req.tags.iter()) {
        let text = text.to_ascii_lowercase();
        if let Some(term) = terms.iter().find(|term| text.contains(**term)) {
            return Some(format!(
                "sensitive spec metadata ({term}) requires a human clear"
            ));
        }
    }
    None
}

fn timestamp(value: Option<&Value>, context: &str) -> Result<DateTime<FixedOffset>> {
    let text = value
        .and_then(Value::as_str)
        .context(format!("missing {context} timestamp"))?;
    DateTime::parse_from_rfc3339(text).with_context(|| format!("invalid {context} timestamp"))
}

/// Pure eligibility: callers inject the revalidated recorded reviewer's grant.
fn eligible(
    hold: &mh::MergeHoldRecord,
    body: &Value,
    head: &str,
    specs: &[Option<&aida_core::Requirement>],
    authority: &sa::GrantCheck,
) -> Result<mh::VerdictRef> {
    ensure!(
        !hold.legacy
            && hold.reason_kind == mh::HoldReasonKind::Rework
            && hold
                .absorbed
                .iter()
                .all(|h| h.reason_kind == mh::HoldReasonKind::Rework),
        "only nonlegacy Rework-only holds can auto-release; composed holds require a human clear"
    );
    ensure!(
        !specs.is_empty(),
        "covered specs are missing; human clear required"
    );
    for req in specs {
        let req = req.context("covered spec is missing or unreadable; human clear required")?;
        if let Some(reason) = sensitive(req) {
            bail!("{reason}");
        }
    }
    ensure!(
        body.get("verdict")
            .and_then(Value::as_str)
            .map(rv::VerdictKind::parse)
            == Some(rv::VerdictKind::Approved),
        "verdict is not APPROVED"
    );
    let sha = body
        .get("reviewed_sha")
        .and_then(Value::as_str)
        .context("approval has no head SHA")?;
    ensure!(
        head.len() == 40 && sha == head && crate::git_arg_guard::is_hex_sha(head),
        "approval is at a stale or unverifiable head"
    );
    let grant = match authority {
        sa::GrantCheck::Valid(grant) => grant,
        sa::GrantCheck::Absent => bail!("approval has no validated reviewer grant"),
        sa::GrantCheck::Rejected { reason, .. } => bail!("reviewer grant rejected: {reason}"),
    };
    ensure!(
        record_seat_allowed(Some(&grant.seat)),
        "recorder is an implementer (author/self-review)"
    );
    let att = body
        .get("release_grant")
        .context("approval lacks record-time grant provenance")?;
    ensure!(
        att.get("id").and_then(Value::as_str) == Some(grant.id.as_str())
            && att.get("subject").and_then(Value::as_str) == Some(grant.subject.as_str())
            && att.get("session_id").and_then(Value::as_str) == Some(grant.session_id.as_str()),
        "recorded reviewer identity does not match its valid grant"
    );
    let reviewer = body
        .get("recorded_by")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .context("approval has no recording identity")?;
    ensure!(
        att.get("recorded_by").and_then(Value::as_str) == Some(reviewer),
        "recorded reviewer identity changed after review record"
    );
    let approved_at = timestamp(body.get("recorded_at"), "approval")?;
    let rounds = body
        .get("rounds")
        .and_then(Value::as_array)
        .context("missing arming refusal rounds")?;
    let mut armed = false;
    for round in rounds {
        let kind = round
            .get("verdict")
            .and_then(Value::as_str)
            .map(rv::VerdictKind::parse)
            .context("ambiguous retained verdict provenance")?;
        ensure!(
            kind != rv::VerdictKind::Unknown,
            "ambiguous retained verdict provenance"
        );
        if kind.blocks_done() {
            let at = timestamp(round.get("recorded_at"), "arming refusal")?;
            ensure!(
                approved_at > at,
                "approval is not strictly newer than the arming refusal"
            );
            armed = true;
        }
    }
    ensure!(
        armed,
        "no earlier request-changes/rejected arming round is recorded"
    );
    let key = hold
        .verdict_ref
        .as_ref()
        .context("hold has no arming verdict reference")?
        .key
        .clone();
    Ok(mh::VerdictRef::new(
        &key,
        Some(hold.pr),
        Some(head.into()),
        Some(reviewer.into()),
    ))
}

/// I/O seam shared by automatic recording and explicit retries. A failed audit
/// or mirror write leaves the active marker in place; repeated successful calls
/// are no-ops. The audit flag prevents duplicate comments on a mirror retry.
fn release_with(
    root: &Path,
    hold: &mh::MergeHoldRecord,
    verdict: &mh::VerdictRef,
    mut validate: impl FnMut() -> Result<String>,
    comment: impl FnOnce(&str) -> Result<()>,
    labels: impl FnOnce() -> Result<()>,
) -> Result<()> {
    ensure!(
        Some(validate()?.as_str()) == verdict.reviewed_sha.as_deref(),
        "PR head moved before release"
    );
    let path = mh::clearance_path(root, hold.pr);
    let existing = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let same = existing.as_ref().is_some_and(|v| {
        v.get("cleared_by").and_then(Value::as_str) == Some(PRINCIPAL)
            && v.get("released_by_verdict") == serde_json::to_value(verdict).ok().as_ref()
    });
    if !same {
        let mut approved_hold = hold.clone();
        approved_hold.target_head_sha = verdict.reviewed_sha.clone();
        mh::record_clearance_by(root, &approved_hold, PRINCIPAL, Some(verdict.clone()))?;
    }
    if !same
        || !existing
            .as_ref()
            .is_some_and(|v| v.get("audit_comment_posted") == Some(&Value::Bool(true)))
    {
        comment(&format!("Recorded auto-release authorization: {} by {} at {}. Recorded clearance principal `{PRINCIPAL}`; all hold kinds are Rework and covered specs require no human signoff.",
            verdict.path(), verdict.recorded_by.as_deref().unwrap_or_default(), verdict.reviewed_sha.as_deref().unwrap_or_default()))?;
        let mut audit: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        audit["audit_comment_posted"] = Value::Bool(true);
        aida_core::fs_atomic::write_atomic(&path, &serde_json::to_vec_pretty(&audit)?)?;
    }
    ensure!(
        Some(validate()?.as_str()) == verdict.reviewed_sha.as_deref(),
        "PR head moved before release"
    );
    labels()?;
    mh::clear_hold(root, hold.pr)?;
    Ok(())
}

pub(crate) fn auto_release(project: &Path, pr: u64) -> Result<bool> {
    let root = crate::main_worktree_root_from(project);
    let Some(hold) = mh::read_hold_record(&root, pr) else {
        return Ok(false);
    };
    let kind = crate::forge::resolve_open_change_forge_kind(&root);
    let pin = mh::resolve_pinned_repo(&root, kind)
        .map_err(anyhow::Error::msg)?
        .context("no forge repository for auto-release")?;
    let live_head = || -> Result<String> {
        let change = mh::fetch_pinned_change(&root, kind, pr)
            .map_err(anyhow::Error::msg)?
            .context("no forge change for auto-release")?;
        ensure!(change.state == mh::ChangeState::Open, "change is not open");
        change.head_sha.context("current head could not be read")
    };
    let head = live_head()?;
    let forge = crate::forge::forge_for_kind(&root, kind);
    let mut sink = crate::network_retry::StderrSink;
    let mut ids: Vec<String> = forge
        .change_commit_headlines(pr, &mut sink)?
        .iter()
        .flat_map(|s| crate::pr_ship::extract_trailing_spec_ids_from_subject(s))
        .collect();
    if let Some(spec) = &hold.spec {
        if !spec.starts_with("PR-") {
            ids.push(spec.clone());
        }
    }
    ids.sort();
    ids.dedup();
    let store = crate::pr_lookup::load_review_story_inventory(&root)
        .context("covered spec store is unreadable; human clear required")?;
    let specs: Vec<_> = ids
        .iter()
        .map(|id| {
            store.requirements.iter().find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(id))
                    || r.agreed_id
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(id))
            })
        })
        .collect();
    let key = hold
        .verdict_ref
        .as_ref()
        .context("hold has no arming verdict reference")?
        .key
        .clone();
    let verdict_root = [project, root.as_path()]
        .into_iter()
        .find(|r| rv::verdict_path(r, &key).is_file())
        .context("arming verdict is missing")?;
    let read_body = || -> Result<Value> {
        Ok(serde_json::from_slice(&std::fs::read(rv::verdict_path(
            verdict_root,
            &key,
        ))?)?)
    };
    let check = |body: &Value| -> Result<mh::VerdictRef> {
        let att = body
            .get("release_grant")
            .context("approval lacks record-time grant provenance")?;
        let id = att
            .get("id")
            .and_then(Value::as_str)
            .context("approval lacks reviewer grant id")?;
        let subject = att
            .get("subject")
            .and_then(Value::as_str)
            .context("approval lacks reviewer subject")?;
        eligible(
            &hold,
            body,
            &head,
            &specs,
            &sa::grant_check_for(&root, id, subject),
        )
    };
    let verdict = check(&read_body()?)?;
    release_with(
        &root,
        &hold,
        &verdict,
        || {
            let current_head = live_head()?;
            ensure!(
                mh::read_hold_record(&root, pr).as_ref() == Some(&hold),
                "hold changed before release"
            );
            ensure!(
                check(&read_body()?)? == verdict,
                "approval changed before release"
            );
            let bodies =
                mh::corpus_gate_bodies(&[project, &root], pr, &ids).map_err(anyhow::Error::msg)?;
            ensure!(
                matches!(
                    mh::corpus_merge_gate(pr, &bodies, Some(&head)),
                    mh::CorpusMergeGate::Clear
                ),
                "verdict corpus still refuses this head"
            );
            Ok(current_head)
        },
        |body| {
            let (cli, args) = match kind {
                crate::forge::ForgeKind::GitHub => ("gh", vec!["pr", "comment"]),
                crate::forge::ForgeKind::GitLab => ("glab", vec!["mr", "note"]),
                _ => bail!("no forge audit comment support"),
            };
            let output = std::process::Command::new(cli)
                .current_dir(&root)
                .args(args)
                .arg(pr.to_string())
                .arg("-R")
                .arg(pin.repo_arg())
                .arg(if kind == crate::forge::ForgeKind::GitHub {
                    "--body"
                } else {
                    "--message"
                })
                .arg(body)
                .output()?;
            ensure!(
                output.status.success(),
                "audit comment failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(())
        },
        || {
            mh::sync_label(&root, pr, false)
                .map_err(|e| anyhow::anyhow!("release labels failed: {e}"))
        },
    )?;
    Ok(true)
}

#[cfg(test)]
#[path = "tests/story_1687_rework_release_tests.rs"]
mod tests;
