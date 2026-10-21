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

// trace:STORY-1698 | ai:claude — shared with the mandate's excluded classes.
pub(crate) fn sensitive(req: &aida_core::Requirement) -> Option<String> {
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

/// What an eligible approval releases: the verdict reference written into the
/// hold ledger, and the mandate that covered a sensitive spec, when one did.
// trace:STORY-1697 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Release {
    pub verdict: mh::VerdictRef,
    pub mandate: Option<String>,
}

/// The outcome of one auto-release attempt that did not refuse.
// trace:STORY-1697 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    NoHold,
    Released { mandate: Option<String> },
}

impl Outcome {
    pub(crate) fn line(&self, pr: u64) -> String {
        match self {
            Self::NoHold => format!("PR #{pr}: no active local hold"),
            Self::Released { mandate: None } => {
                format!("PR #{pr}: merge-hold auto-released by recorded verdict")
            }
            Self::Released { mandate: Some(id) } => format!(
                "PR #{pr}: merge-hold auto-released by recorded verdict ({})",
                crate::mandate::release_note(id)
            ),
        }
    }
}

/// The round history the arming refusal is read from beyond the verdict's
/// own inline `rounds`: the recordings the writer keeps in the verdict's
/// per-commit archive ([`rv::archived_recordings`]), and where that archive
/// is, for the fail-closed refusal.
// trace:BUG-2186 | ai:claude
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RetainedHistory {
    pub archived: Vec<Value>,
    pub location: String,
}

impl RetainedHistory {
    /// Read the archive beside the verdict file at `path`.
    // trace:BUG-2186 | ai:claude
    pub(crate) fn beside(path: &Path) -> Result<Self> {
        let location = rv::verdict_archive_dir(path).map_or_else(
            || format!("no per-commit archive for {}", path.display()),
            |dir| dir.display().to_string(),
        );
        let archived = rv::archived_recordings(path)
            .map_err(|why| anyhow::anyhow!("retained verdict history is unreadable: {why}"))?;
        Ok(Self { archived, location })
    }
}

/// Where this build of the writer keeps round history, named in refusals.
// trace:BUG-2186 | ai:claude
fn history_note(history: &RetainedHistory) -> String {
    format!(
        "looked in the verdict's `rounds` and its per-commit archive {} (where aida {} ({}) records each round)",
        history.location,
        env!("CARGO_PKG_VERSION"),
        rv::short_sha(env!("AIDA_BUILD_GIT_SHA"))
    )
}

fn timestamp(value: Option<&Value>, context: &str) -> Result<DateTime<FixedOffset>> {
    let text = value
        .and_then(Value::as_str)
        .context(format!("missing {context} timestamp"))?;
    DateTime::parse_from_rfc3339(text).with_context(|| format!("invalid {context} timestamp"))
}

/// Pure eligibility: callers inject the revalidated recorded reviewer's grant
/// and the mandate lookup. `mandate` is consulted only after every other
/// check passed and only when a covered spec is sensitive; `Ok(id)` lets an
/// active `--include-supervised` mandate cover that spec (STORY-1697).
fn eligible(
    hold: &mh::MergeHoldRecord,
    body: &Value,
    history: &RetainedHistory,
    head: &str,
    specs: &[Option<&aida_core::Requirement>],
    authority: &sa::GrantCheck,
    mandate: &dyn Fn() -> Result<String>,
) -> Result<Release> {
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
    let mut sensitive_reason = None;
    for req in specs {
        let req = req.context("covered spec is missing or unreadable; human clear required")?;
        // Operator-only and decide-mode specs never release without a human,
        // whatever a mandate says. trace:STORY-1697 | ai:claude
        if let Some(class) = crate::mandate::always_human_class(req) {
            let human = format!("{class} spec always requires a human clear");
            match sensitive(req) {
                Some(reason) => bail!("{reason}; {human}"),
                None => bail!("{human}"),
            }
        }
        if let Some(reason) = sensitive(req) {
            sensitive_reason.get_or_insert(reason);
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
    // BUG-2186: the arming refusal is read from wherever the writer keeps
    // it — inline `rounds` when the current file survived between rounds,
    // else the per-commit archive the reviewer launchers never clear.
    // trace:BUG-2186 | ai:claude
    let inline = match body.get("rounds") {
        None => &[][..],
        Some(rounds) => rounds
            .as_array()
            .context("retained `rounds` is not an array")?
            .as_slice(),
    };
    let rounds: Vec<&Value> = inline.iter().chain(&history.archived).collect();
    ensure!(
        !rounds.is_empty(),
        "missing arming refusal rounds: {}",
        history_note(history)
    );
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
        "no earlier request-changes/rejected arming round is recorded: {}",
        history_note(history)
    );
    let key = hold
        .verdict_ref
        .as_ref()
        .context("hold has no arming verdict reference")?
        .key
        .clone();
    // STORY-1697: a sensitive spec still needs a human clear unless an active
    // mandate minted with --include-supervised covers this release.
    // trace:STORY-1697 | ai:claude
    let mandate = match sensitive_reason {
        None => None,
        Some(reason) => Some(mandate().map_err(|why| anyhow::anyhow!("{reason} ({why:#})"))?),
    };
    Ok(Release {
        verdict: mh::VerdictRef::new(
            &key,
            Some(hold.pr),
            Some(head.into()),
            Some(reviewer.into()),
        ),
        mandate,
    })
}

/// BUG-1918's approval gate at the PR's current head, as the drain merge
/// applies it: the newest open approval carries a re-validated recorder
/// attestation from a session independent of the PR's authors, and it covers
/// `head`. Release refuses on either.
// trace:STORY-1697 | ai:claude
pub(crate) fn approval_attested(
    candidates: &[rv::RecordedVerdict],
    policy: &crate::pr_ship::ShipApprovalPolicy,
    head: &str,
    pr: u64,
) -> Result<()> {
    crate::pr_merge_gate::drain_merge_approval_gate(candidates, policy, Some(head), pr)
        .map_err(|failure| anyhow::anyhow!("{}", failure.reason))
}

/// I/O seam shared by automatic recording and explicit retries. A failed audit
/// or mirror write leaves the active marker in place; repeated successful calls
/// are no-ops. The audit flag prevents duplicate comments on a mirror retry.
fn release_with(
    root: &Path,
    hold: &mh::MergeHoldRecord,
    release: &Release,
    mut validate: impl FnMut() -> Result<String>,
    comment: impl FnOnce(&str) -> Result<()>,
    labels: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let verdict = &release.verdict;
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
        // trace:STORY-1697 | ai:claude
        if let Some(id) = &release.mandate {
            let mut audit: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            audit["released_under_mandate"] = Value::String(id.clone());
            aida_core::fs_atomic::write_atomic(&path, &serde_json::to_vec_pretty(&audit)?)?;
        }
    }
    if !same
        || !existing
            .as_ref()
            .is_some_and(|v| v.get("audit_comment_posted") == Some(&Value::Bool(true)))
    {
        let specs = match &release.mandate {
            None => "covered specs require no human signoff".to_string(),
            Some(id) => format!(
                "a sensitive covered spec is {}",
                crate::mandate::release_note(id)
            ),
        };
        comment(&format!("Recorded auto-release authorization: {} by {} at {}. Recorded clearance principal `{PRINCIPAL}`; all hold kinds are Rework and {specs}.",
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

// Reads and forge effects are injected; eligibility and both release-time rechecks
// stay in the same path used by production. No test-only authority bypass.
// trace:STORY-1687 | ai:codex
struct AutoReleaseIo<'a> {
    live_head: &'a dyn Fn() -> Result<Option<mh::PinnedChange>>,
    read_hold: &'a dyn Fn() -> Option<mh::MergeHoldRecord>,
    covered_specs: &'a dyn Fn() -> Result<(Vec<String>, aida_core::RequirementsStore)>,
    read_body: &'a dyn Fn(&str) -> Result<Value>,
    /// The archived round history beside the arming verdict.
    // trace:BUG-2186 | ai:claude
    read_history: &'a dyn Fn(&str) -> Result<RetainedHistory>,
    read_corpus: &'a dyn Fn(&[String]) -> Result<Vec<(String, String)>>,
    grant_check: &'a dyn Fn(&str, &str) -> sa::GrantCheck,
    /// BUG-1918 attestation + head coverage ([`approval_attested`]).
    // trace:STORY-1697 | ai:claude
    approval_authority: &'a dyn Fn(&str) -> Result<()>,
    /// The mandate covering a sensitive release, re-read on every call.
    mandate: &'a dyn Fn() -> Result<String>,
    comment: &'a dyn Fn(&str) -> Result<()>,
    labels: &'a dyn Fn() -> Result<()>,
}

pub(crate) fn auto_release(project: &Path, pr: u64) -> Result<Outcome> {
    let root = crate::main_worktree_root_from(project);
    let Some(hold) = mh::read_hold_record(&root, pr) else {
        return Ok(Outcome::NoHold);
    };
    let kind = crate::forge::resolve_open_change_forge_kind(&root);
    let pin = mh::resolve_pinned_repo(&root, kind)
        .map_err(anyhow::Error::msg)?
        .context("no forge repository for auto-release")?;
    let live_head = || mh::fetch_pinned_change(&root, kind, pr).map_err(anyhow::Error::msg);
    let known_ids: Vec<String> = hold
        .spec
        .iter()
        .filter(|spec| !spec.starts_with("PR-"))
        .cloned()
        .collect();
    let verdict_root = std::cell::Cell::new(None::<&Path>);
    let verdict_file = |key: &str| -> Result<std::path::PathBuf> {
        let verdict_root = match verdict_root.get() {
            Some(path) => path,
            None => {
                let path = [project, root.as_path()]
                    .into_iter()
                    .find(|r| rv::verdict_path(r, key).is_file())
                    .context("arming verdict is missing")?;
                verdict_root.set(Some(path));
                path
            }
        };
        Ok(rv::verdict_path(verdict_root, key))
    };
    auto_release_with(
        &root,
        &hold,
        AutoReleaseIo {
            live_head: &live_head,
            read_hold: &|| mh::read_hold_record(&root, pr),
            covered_specs: &|| {
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
                Ok((ids, store))
            },
            // trace:TASK-1801 | ai:claude
            read_body: &|key| {
                Ok(serde_json::from_str(&rv::read_verdict_body(
                    verdict_file(key)?,
                )?)?)
            },
            // trace:BUG-2186 | ai:claude
            read_history: &|key| RetainedHistory::beside(&verdict_file(key)?),
            read_corpus: &|ids| {
                mh::corpus_gate_bodies(&[project, &root], pr, ids).map_err(anyhow::Error::msg)
            },
            grant_check: &|id, subject| sa::grant_check_for(&root, id, subject),
            // trace:STORY-1697 | ai:claude
            approval_authority: &|head| {
                let (candidates, policy) = crate::pr_cmd::merge_approval_inputs(
                    &[project, root.as_path()],
                    &root,
                    pr,
                    &known_ids,
                    None,
                    Some(head),
                );
                approval_attested(&candidates, &policy, head, pr)
            },
            mandate: &|| {
                crate::pr_cmd::mandate_ship_cover(
                    project,
                    &root,
                    &root,
                    pr,
                    &known_ids,
                    None,
                    || open_change_head(live_head().ok().flatten()).ok(),
                    false,
                    crate::mandate::rework_release_decision,
                    // It ignores the carry-over. trace:TASK-1968 | ai:claude
                    false,
                )
                .map(|cover| cover.id)
                .map_err(|refusal| anyhow::anyhow!("{}", refusal.message(pr)))
            },
            comment: &|body| {
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
            labels: &|| {
                mh::sync_label(&root, pr, false)
                    .map_err(|e| anyhow::anyhow!("release labels failed: {e}"))
            },
        },
    )
}

// Keep forge-response validation inside the shared production/test path too.
// trace:STORY-1687 | ai:codex
fn open_change_head(change: Option<mh::PinnedChange>) -> Result<String> {
    let change = change.context("no forge change for auto-release")?;
    ensure!(change.state == mh::ChangeState::Open, "change is not open");
    change.head_sha.context("current head could not be read")
}

fn auto_release_with(
    root: &Path,
    hold: &mh::MergeHoldRecord,
    io: AutoReleaseIo<'_>,
) -> Result<Outcome> {
    let head = open_change_head((io.live_head)()?)?;
    let key = &hold
        .verdict_ref
        .as_ref()
        .context("hold has no arming verdict reference")?
        .key;
    let check = |body: &Value,
                 history: &RetainedHistory,
                 ids: &[String],
                 store: &aida_core::RequirementsStore|
     -> Result<Release> {
        // Display/origin IDs can collide across distinct canonical requirements.
        // Refuse ambiguity at eligibility and at both release-time rechecks.
        // trace:STORY-1687 | ai:codex
        let specs: Vec<_> = ids
            .iter()
            .map(|id| {
                let matches: Vec<_> = store.requirements.iter().filter(|r| {
                    r.spec_id
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(id))
                        || r.agreed_id
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(id))
                }).collect();
                ensure!(
                    matches.len() == 1,
                    "covered spec is missing or ambiguous: {id} resolves to {} requirements; exactly one required; human clear required",
                    matches.len()
                );
                Ok(Some(matches[0]))
            })
            .collect::<Result<_>>()?;
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
        // The approval's own authority is judged before `eligible` can
        // consult the mandate: the mandate lookup reads the mandate records
        // and writes the exclusion ledger, so nothing an unattested approval
        // triggers reaches it. trace:STORY-1697 | ai:claude
        (io.approval_authority)(&head)?;
        eligible(
            hold,
            body,
            history,
            &head,
            &specs,
            &(io.grant_check)(id, subject),
            io.mandate,
        )
    };
    let (ids, store) = (io.covered_specs)()?;
    let release = check(
        &(io.read_body)(key)?,
        &(io.read_history)(key)?,
        &ids,
        &store,
    )?;
    let released = release_with(
        root,
        hold,
        &release,
        || {
            let current_head = open_change_head((io.live_head)()?)?;
            ensure!(
                (io.read_hold)().as_ref() == Some(hold),
                "hold changed before release"
            );
            // Metadata and execution mode can tighten after initial eligibility.
            // Re-resolve every covered spec on both release-time validation passes.
            // trace:STORY-1687 | ai:codex
            let (ids, store) = (io.covered_specs)()?;
            ensure!(
                check(
                    &(io.read_body)(key)?,
                    &(io.read_history)(key)?,
                    &ids,
                    &store
                )? == release,
                "approval changed before release (or its covering mandate did)"
            );
            let bodies = (io.read_corpus)(&ids)?;
            ensure!(
                matches!(
                    mh::corpus_merge_gate(hold.pr, &bodies, Some(&head)),
                    mh::CorpusMergeGate::Clear
                ),
                "verdict corpus still refuses this head"
            );
            Ok(current_head)
        },
        io.comment,
        io.labels,
    );
    // A release a mandate covered is a mandate-driven action: ledger it,
    // released or refused. trace:TASK-1915 | ai:claude
    if let Some(id) = &release.mandate {
        let (outcome, why) = match &released {
            Ok(()) => (
                crate::mandate_drive::Outcome::Done,
                format!(
                    "rework hold released on the approval {}",
                    release.verdict.path()
                ),
            ),
            Err(e) => (
                crate::mandate_drive::Outcome::Refused,
                format!("rework hold release failed: {e:#}"),
            ),
        };
        crate::mandate_drive::emit_action(
            root,
            id,
            "session",
            &crate::mandate_drive::ActionRow::new(
                &format!("pr:{}", hold.pr),
                &format!("PR #{}", hold.pr),
                "release-rework-hold",
                outcome,
                Some(why),
            ),
        );
    }
    released?;
    Ok(Outcome::Released {
        mandate: release.mandate,
    })
}

#[cfg(test)]
#[path = "tests/story_1687_rework_release_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/story_1697_attested_release_tests.rs"]
mod story_1697_tests;

#[cfg(test)]
#[path = "tests/bug_2186_archived_history_release_tests.rs"]
mod bug_2186_tests;
