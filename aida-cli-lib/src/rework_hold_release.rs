//! Narrow, recorded release of nonsensitive Rework-only holds.
//! Shares today's record-time seat/grant check. It does not authenticate
//! same-user JSON against forgery or provide the pending authorship ledger.
//! Hold writes are atomic. Hold placements and the release's second pass,
//! label drop and marker removal serialize on the per-PR hold lock
//! (TASK-1757); see [`release_with`] for the order and why it differs from
//! the human clear.
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
    // TASK-1757: an approval recorded under another key (the spec instead of
    // the `PR-N` the refusal armed, or the reverse) leaves this one
    // unanswered; say which key the release reads.
    // trace:TASK-1951 trace:TASK-1757 | ai:claude
    let raw = body.get("verdict").and_then(Value::as_str);
    if raw.map(rv::VerdictKind::parse) != Some(rv::VerdictKind::Approved) {
        let key = hold.verdict_ref.as_ref().map_or("?", |r| r.key.as_str());
        bail!(
            "verdict is not APPROVED: the latest verdict recorded under {key} is {}; \
             auto-release reads the key the refusal was recorded under, so record the \
             approval as `aida review record {key}`",
            raw.unwrap_or("missing")
        );
    }
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
///
/// TASK-1757 order and locking. Auto-release records the clearance, posts the
/// audit comment, drops the labels, and only then removes the marker: no
/// human is present to retry, so the marker stays armed until the forge
/// agrees. The human `aida merge-hold clear` records the clearance, removes
/// the marker, then drops the labels, because the operator at the terminal
/// sees a label failure and re-runs it. The second validation pass, the
/// label drop and the marker removal run under the hold lock
/// ([`mh::with_hold_lock`]), so a re-arm cannot land between the check and
/// the removal. A second pass that refuses supersedes the clearance it
/// recorded, so it is never read as a release.
// trace:TASK-1951 trace:TASK-1757 | ai:claude
fn release_with(
    root: &Path,
    hold: &mh::MergeHoldRecord,
    release: &Release,
    also: &[mh::VerdictRef],
    mut validate: impl FnMut() -> Result<String>,
    comment: impl Fn(&str) -> Result<()>,
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
    let also_json = serde_json::to_value(also)?;
    let same = existing.as_ref().is_some_and(|v| {
        v.get("cleared_by").and_then(Value::as_str) == Some(PRINCIPAL)
            && v.get("released_by_verdict") == serde_json::to_value(verdict).ok().as_ref()
            && v.get("also_released_by")
                .unwrap_or(&Value::Array(Vec::new()))
                == &also_json
    });
    if !same {
        let mut approved_hold = hold.clone();
        approved_hold.target_head_sha = verdict.reviewed_sha.clone();
        mh::record_clearance_by(root, &approved_hold, PRINCIPAL, Some(verdict.clone()))?;
        // trace:STORY-1697 trace:BUG-2123 | ai:claude
        if release.mandate.is_some() || !also.is_empty() {
            let mut audit: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            if let Some(id) = &release.mandate {
                audit["released_under_mandate"] = Value::String(id.clone());
            }
            if !also.is_empty() {
                audit["also_released_by"] = also_json.clone();
            }
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
        let answered = if also.is_empty() {
            String::new()
        } else {
            format!(
                " It also answers the absorbed refusals {}.",
                also.iter()
                    .map(mh::VerdictRef::path)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        comment(&format!("Recorded auto-release authorization: {} by {} at {}. Recorded clearance principal `{PRINCIPAL}`; all hold kinds are Rework and {specs}.{answered}",
            verdict.path(), verdict.recorded_by.as_deref().unwrap_or_default(), verdict.reviewed_sha.as_deref().unwrap_or_default()))?;
        let mut audit: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        audit["audit_comment_posted"] = Value::Bool(true);
        aida_core::fs_atomic::write_atomic(&path, &serde_json::to_vec_pretty(&audit)?)?;
    }
    let locked = mh::with_hold_lock(root, hold.pr, || -> Result<()> {
        let second = validate().and_then(|head| {
            ensure!(
                Some(head.as_str()) == verdict.reviewed_sha.as_deref(),
                "PR head moved before release"
            );
            Ok(())
        });
        if let Err(why) = second {
            supersede_clearance(root, hold.pr, &why, &comment);
            return Err(why);
        }
        labels()?;
        mh::clear_hold(root, hold.pr)?;
        Ok(())
    });
    match locked {
        Ok(released) => released,
        // Busy: the second pass never ran, so the clearance just recorded
        // must not stand either.
        Err(busy) => {
            let why = anyhow::Error::new(busy);
            supersede_clearance(root, hold.pr, &why, &comment);
            Err(why)
        }
    }
}

/// TASK-1757: the second validation pass refused after the clearance was
/// recorded. Move the clearance aside (kept for audit, never read as a
/// clearance: a stale one beside a live marker would mask a later
/// hand-deletion from the tamper check) and say so on the PR, best-effort.
// trace:TASK-1951 trace:TASK-1757 | ai:claude
fn supersede_clearance(
    root: &Path,
    pr: u64,
    why: &anyhow::Error,
    comment: &dyn Fn(&str) -> Result<()>,
) {
    let path = mh::clearance_path(root, pr);
    if !path.exists() {
        return;
    }
    let aside = mh::superseded_clearance_path(root, pr, chrono::Utc::now());
    if let Err(e) = std::fs::rename(&path, &aside) {
        eprintln!(
            "  warning: could not supersede PR #{pr}'s auto-release clearance {} ({e}); remove it by hand",
            path.display()
        );
        return;
    }
    let _ = comment(&format!(
        "Auto-release authorization withdrawn: {why:#}. The hold stays armed; the withdrawn clearance is kept at {}.",
        aside.display()
    ));
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
    // Each arming verdict is read from the root it was first found under, so
    // both validation passes read the same file (BUG-2123: one per key).
    let verdict_roots = std::cell::RefCell::new(std::collections::HashMap::<String, &Path>::new());
    let verdict_file = |key: &str| -> Result<std::path::PathBuf> {
        let known = verdict_roots.borrow().get(key).copied();
        let verdict_root = match known {
            Some(path) => path,
            None => {
                let path = [project, root.as_path()]
                    .into_iter()
                    .find(|r| rv::verdict_path(r, key).is_file())
                    .with_context(|| format!("arming verdict {key} is missing"))?;
                verdict_roots.borrow_mut().insert(key.to_string(), path);
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
                let headlines = forge.change_commit_headlines(pr, &mut sink)?;
                let title = forge.change_metadata(pr, &mut sink)?.title;
                let ids = covered_spec_ids(&headlines, &title, &hold);
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
                mh::sync_label(&root, pr, false).map_err(|e| auto_release_label_failure(pr, &e))
            },
        },
    )
}

/// The auto-release label step failed. The marker is still armed (labels run
/// before the marker is removed), so the retry is the auto-release itself,
/// not `aida merge-hold clear`.
// trace:TASK-1951 | ai:claude
fn auto_release_label_failure(pr: u64, err: &mh::LabelSyncError) -> anyhow::Error {
    anyhow::anyhow!(
        "release labels failed: {err}; the hold stays armed — re-run `aida merge-hold auto-release {pr}` when the forge is reachable"
    )
}

/// BUG-2123: the specs an auto-release must resolve and judge: every commit
/// headline's and the PR title's trailing ids, the hold's own spec, and the
/// spec of every hold it absorbed. A spec named in only one of those places
/// is still covered.
// trace:TASK-1951 trace:BUG-2123 | ai:claude
fn covered_spec_ids(headlines: &[String], title: &str, hold: &mh::MergeHoldRecord) -> Vec<String> {
    let mut ids: Vec<String> = headlines
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(title))
        .flat_map(crate::pr_ship::extract_trailing_spec_ids_from_subject)
        .collect();
    ids.extend(
        hold.spec
            .iter()
            .chain(hold.absorbed.iter().filter_map(|a| a.spec.as_ref()))
            .filter(|spec| !spec.starts_with("PR-"))
            .cloned(),
    );
    ids.sort();
    ids.dedup();
    ids
}

/// BUG-2123: every refusal the hold stands on: its own verdict, then each
/// absorbed Rework hold's. Auto-release must find each one answered. An
/// absorbed Rework hold with no verdict reference cannot be answered, so it
/// needs a human clear.
// trace:TASK-1951 trace:BUG-2123 | ai:claude
fn arming_refs(hold: &mh::MergeHoldRecord) -> Result<Vec<mh::VerdictRef>> {
    let mut refs = vec![hold
        .verdict_ref
        .clone()
        .context("hold has no arming verdict reference")?];
    for absorbed in hold
        .absorbed
        .iter()
        .filter(|a| a.reason_kind == mh::HoldReasonKind::Rework)
    {
        let r = absorbed.verdict_ref.clone().with_context(|| {
            format!(
                "an absorbed rework hold ({}) has no verdict reference; human clear required",
                absorbed.detail
            )
        })?;
        if !refs.iter().any(|known| known.key == r.key) {
            refs.push(r);
        }
    }
    Ok(refs)
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
    let refs = arming_refs(hold)?;
    let check = |hold: &mh::MergeHoldRecord,
                 body: &Value,
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
    // BUG-2123: every arming refusal must be answered at this head. Each is
    // judged against the hold as if it alone armed it, so its Release names
    // its own verdict. trace:TASK-1951 trace:BUG-2123 | ai:claude
    let check_all = |ids: &[String],
                     store: &aida_core::RequirementsStore|
     -> Result<Vec<Release>> {
        refs.iter()
            .map(|r| {
                let mut armed = hold.clone();
                armed.verdict_ref = Some(r.clone());
                let judged = (io.read_body)(&r.key)
                    .and_then(|body| check(&armed, &body, &(io.read_history)(&r.key)?, ids, store));
                match judged {
                    Ok(release) => Ok(release),
                    Err(e) if refs.len() > 1 => Err(anyhow::anyhow!(
                        "the refusal on {} at {} is not answered: {e:#}",
                        r.key,
                        r.reviewed_sha
                            .as_deref()
                            .map_or("an unknown head", |s| &s[..s.len().min(8)])
                    )),
                    Err(e) => Err(e),
                }
            })
            .collect()
    };
    let (ids, store) = (io.covered_specs)()?;
    let releases = check_all(&ids, &store)?;
    let release = releases[0].clone();
    let also: Vec<mh::VerdictRef> = releases[1..].iter().map(|r| r.verdict.clone()).collect();
    let released = release_with(
        root,
        hold,
        &release,
        &also,
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
                check_all(&ids, &store)? == releases,
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

#[cfg(test)]
#[path = "tests/bug_2123_composed_rework_release_tests.rs"]
mod bug_2123_tests;
