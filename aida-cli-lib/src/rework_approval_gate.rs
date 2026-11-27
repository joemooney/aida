//! BUG-2785: a rework round is refused when the spec's open PR already has a
//! FINAL approval at its current head. A push from that round would void an
//! approval that is on its way to landing. The check runs where a round is
//! queued (`aida queue rework`) and again where it starts (`aida queue work`
//! on the PR branch), so a round that went stale while waiting for a slot is
//! refused when it would launch. An approved change that conflicts with its
//! base or fails CI cannot land as it stands, so its rework is admitted.
//!
//! A sweep-pending record is a delivery barrier, not an approval:
//! `disposition_verdict` already reads it as request-changes, so it never
//! counts here.
// trace:BUG-2785 | ai:claude

use crate::gate::review_verdict::{current_verdict, parse_recorded_verdict, RecordedVerdict};
use aida_core::Storage;
use anyhow::Result;
use std::path::Path;

/// First token of the spec comment that records an override.
pub(crate) const OVERRIDE_TAG: &str = "[despite-approval]";

/// The approval that makes a rework round stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeadApproval {
    /// Change key, `PR-<N>` / `MR-<N>`.
    pub(crate) change: String,
    /// The change's current head, as the forge reports it.
    pub(crate) head_sha: String,
    pub(crate) reviewer: String,
    pub(crate) recorded_at: String,
}

/// Two spellings of one commit: equal after trimming and lowercasing, or one
/// an abbreviation (at least 7 characters) of the other.
fn same_commit(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase());
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.len() >= 7 && long.starts_with(&short)
}

/// The current verdicts for the change key and the spec's own labels.
fn candidate_verdicts(root: &Path, change: &str, ids: &[&str]) -> Vec<RecordedVerdict> {
    let mut keys = vec![change.trim().to_ascii_uppercase()];
    for label in crate::gate::review_alias::spec_labels(ids) {
        if !keys.contains(&label) {
            keys.push(label);
        }
    }
    keys.iter()
        .filter_map(|key| current_verdict(root, key))
        .filter_map(|current| parse_recorded_verdict(&current.body))
        .collect()
}

/// The FINAL approval at `head_sha`, when the newest verdict recorded at that
/// head approves. On a timestamp tie a non-approval wins: conflicting evidence
/// never refuses a rework.
pub(crate) fn approval_at_head(
    verdicts: &[RecordedVerdict],
    change: &str,
    head_sha: &str,
) -> Option<HeadApproval> {
    let newest = verdicts
        .iter()
        .filter(|v| {
            v.reviewed_sha
                .as_deref()
                .is_some_and(|sha| same_commit(sha, head_sha))
        })
        .max_by_key(|v| {
            (
                crate::queue_cmd::verdict_recorded_time(v, None),
                !v.kind.approves(),
            )
        })?;
    (newest.kind.approves() && !newest.is_closed()).then(|| HeadApproval {
        change: change.trim().to_ascii_uppercase(),
        head_sha: head_sha.trim().to_ascii_lowercase(),
        reviewer: newest
            .recorded_by
            .clone()
            .unwrap_or_else(|| "unknown reviewer".into()),
        recorded_at: newest
            .recorded_at
            .clone()
            .unwrap_or_else(|| "unknown time".into()),
    })
}

/// The override comment's first line, bound to one change and one head.
pub(crate) fn override_marker(change: &str, head_sha: &str) -> String {
    format!(
        "{OVERRIDE_TAG} {}@{}",
        change.trim().to_ascii_uppercase(),
        head_sha.trim().to_ascii_lowercase()
    )
}

/// Whether an override was recorded on the spec for this approval's change
/// and head. An override for an older head does not carry over.
pub(crate) fn override_recorded(comments: &[aida_core::Comment], approval: &HeadApproval) -> bool {
    let marker = override_marker(&approval.change, &approval.head_sha);
    comments.iter().any(|c| {
        c.content.lines().next().is_some_and(|line| {
            line.trim() == marker || line.trim().starts_with(&format!("{marker} "))
        })
    })
}

/// The refusal, naming the approval and the override.
pub(crate) fn refusal(display: &str, approval: &HeadApproval) -> String {
    let head: String = approval.head_sha.chars().take(12).collect();
    format!(
        "rework-refused reason=approved-at-head: {display}'s open change {} is already approved \
         at its current head {head} (reviewer {}, recorded {}); a rework push would void that \
         approval. To rework anyway, run `aida queue rework {display} --despite-approval \
         \"<reason>\"`; the reason is recorded on the spec.",
        approval.change, approval.reviewer, approval.recorded_at
    )
}

/// Outcome of the gate before any override is recorded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// No FINAL approval at the current head (or the head is unknown).
    Clear,
    /// Approved at the head, and an override is already on the spec.
    Overridden(HeadApproval),
    /// Approved at the head with no override.
    Approved(HeadApproval),
}

/// The change's head, and why the approval cannot land as it stands. The
/// landing reconciler reworks an approved change that conflicts with its base
/// or fails CI; that round is not stale, so it is admitted.
#[derive(Debug, Clone, Default)]
pub(crate) struct Head {
    pub(crate) sha: String,
    pub(crate) blocked: Option<String>,
}

/// Evaluate the gate. `head` is called only when some candidate approves, so a
/// spec without an approval never costs a forge call. An unknown head admits:
/// it cannot prove the approval is current.
pub(crate) fn evaluate(
    root: &Path,
    change: &str,
    ids: &[&str],
    comments: &[aida_core::Comment],
    head: impl FnOnce() -> Option<Head>,
) -> Gate {
    let verdicts = candidate_verdicts(root, change, ids);
    if !verdicts.iter().any(|v| v.kind.approves() && !v.is_closed()) {
        return Gate::Clear;
    }
    let Some(head) = head() else {
        eprintln!(
            "warning: {change} has an approval but its current head could not be read; \
             not checking whether this rework would void it"
        );
        return Gate::Clear;
    };
    match approval_at_head(&verdicts, change, &head.sha) {
        None => Gate::Clear,
        Some(_) if head.blocked.is_some() => {
            let why = head.blocked.unwrap_or_default();
            eprintln!("  {change} is approved at its head but {why}; the rework may proceed");
            Gate::Clear
        }
        Some(a) if override_recorded(comments, &a) => Gate::Overridden(a),
        Some(a) => Gate::Approved(a),
    }
}

/// The change's current head and landing blockers as the forge reports them,
/// `None` when the head cannot be read.
pub(crate) fn forge_head(root: &Path, change: &str) -> Option<Head> {
    let key = crate::gate::ledger::ChangeKey::parse(change)?;
    let forge = crate::forge::forge_for_open_change(root);
    let status = forge
        .change_status(&crate::forge::ChangeRef {
            id: key.number,
            url: String::new(),
            branch: String::new(),
            base: String::new(),
            title: None,
        })
        .ok()?;
    let sha = status.head_sha.trim().to_string();
    if sha.is_empty() {
        return None;
    }
    let blocked = if status.mergeable == crate::forge_observed::Observed::Known(false) {
        Some("conflicts with its base".to_string())
    } else {
        let ci = forge.ci_status(crate::forge::CiTarget::Commit(sha.clone()));
        ci.ok()
            .filter(|ci| ci.state == crate::gate::dispatch_types::CiState::Failed)
            .map(|_| "its CI failed".to_string())
    };
    Some(Head { sha, blocked })
}

/// Record a `--despite-approval` override on the spec.
pub(crate) fn record_override(
    storage: &Storage,
    req: &aida_core::Requirement,
    approval: &HeadApproval,
    reason: &str,
) -> Result<()> {
    let reason = reason.trim();
    anyhow::ensure!(!reason.is_empty(), "--despite-approval needs a reason");
    let text = format!(
        "{} {reason}\n\nRework requested although {} is approved at its current head \
         (reviewer {}, recorded {}).",
        override_marker(&approval.change, &approval.head_sha),
        approval.change,
        approval.reviewer,
        approval.recorded_at
    );
    let comment = aida_core::Comment::new(crate::get_default_author(), text);
    storage.update_spec_atomically(req, |r| r.add_comment(comment))?;
    eprintln!(
        "  override recorded: reworking {} despite its approval at {}",
        approval.change,
        approval.head_sha.chars().take(12).collect::<String>()
    );
    Ok(())
}

/// Apply the gate to a resolved spec: refuse, admit an existing override, or
/// record a new one from `despite`.
pub(crate) fn apply(
    storage: &Storage,
    req: &aida_core::Requirement,
    display: &str,
    gate: Gate,
    despite: Option<&str>,
) -> Result<()> {
    match gate {
        Gate::Clear | Gate::Overridden(_) => Ok(()),
        Gate::Approved(approval) => match despite {
            Some(reason) => record_override(storage, req, &approval, reason),
            None => anyhow::bail!(refusal(display, &approval)),
        },
    }
}

/// `aida queue rework <ID>` / `aida rework <ID>`: the queue-time check. Runs
/// before any other rework write. A round routed to the reviewer pushes
/// nothing and is not checked.
pub(crate) fn check_rework(
    storage: &Storage,
    id: &str,
    for_role: Option<&str>,
    despite: Option<&str>,
) -> Result<()> {
    if for_role.is_some_and(|r| r.eq_ignore_ascii_case("reviewer")) {
        return Ok(());
    }
    let Ok(root) = crate::find_project_root() else {
        return Ok(());
    };
    let store = storage.load()?;
    let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_unambiguous(id)?
    };
    let Some(req) = req else {
        // The handler reports the unknown ID.
        return Ok(());
    };
    let spec_id = req.spec_id.clone().unwrap_or_default();
    let display = req.agreed_id.clone().unwrap_or_else(|| spec_id.clone());
    let ids = [display.as_str(), spec_id.as_str()];
    let change =
        crate::forge_lookup::with_forge_lookup_deadline(std::time::Duration::from_secs(15), || {
            crate::queue_cmd::spec_open_change_key(&root, &ids)
        });
    let Some(change) = change else {
        return Ok(());
    };
    let gate = evaluate(&root, &change, &ids, &req.comments, || {
        forge_head(&root, &change)
    });
    apply(storage, req, &display, gate, despite)
}

/// `aida queue work` on an open rework PR: the launch-time check. Only an
/// override already recorded on the spec admits an approved head.
pub(crate) fn check_launch(
    storage: &Storage,
    root: &Path,
    display: &str,
    spec_id: &str,
    change: &str,
) -> Result<()> {
    let store = storage.load()?;
    let Some(req) = store.get_requirement_unambiguous(display).ok().flatten() else {
        return Ok(());
    };
    let ids = [display, spec_id];
    let gate = evaluate(root, change, &ids, &req.comments, || {
        forge_head(root, change)
    });
    apply(storage, req, display, gate, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::DatabaseBackend;

    const HEAD: &str = "a32d61ac0000000000000000000000000000beef";
    const OLD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn write(root: &Path, key: &str, body: serde_json::Value) {
        let path = crate::gate::review_verdict::verdict_path(root, key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body.to_string()).unwrap();
    }

    fn approval(sha: &str) -> serde_json::Value {
        serde_json::json!({"verdict": "approved", "reviewed_sha": sha,
            "recorded_by": "reviewer-7", "recorded_at": "2026-11-26T22:28:00Z"})
    }

    fn gate(root: &Path, comments: &[aida_core::Comment]) -> Gate {
        evaluate(root, "PR-4718", &["BUG-2781"], comments, || {
            Some(Head {
                sha: HEAD.into(),
                blocked: None,
            })
        })
    }

    #[test]
    fn approved_at_head_is_refused_naming_the_approval_and_override() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "PR-4718", approval(HEAD));
        let Gate::Approved(a) = gate(tmp.path(), &[]) else {
            panic!("approval at head must refuse");
        };
        let msg = refusal("BUG-2781", &a);
        for part in [
            "PR-4718",
            "a32d61ac0000",
            "reviewer-7",
            "2026-11-26T22:28:00Z",
            "--despite-approval",
        ] {
            assert!(msg.contains(part), "{part} missing from: {msg}");
        }
        // A spec-keyed approval with an abbreviated sha counts too.
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "BUG-2781", approval("A32D61AC"));
        assert!(matches!(gate(tmp.path(), &[]), Gate::Approved(_)));
    }

    #[test]
    fn older_head_sweep_pending_and_newer_request_changes_admit() {
        let older = tempfile::tempdir().unwrap();
        write(older.path(), "PR-4718", approval(OLD));
        assert_eq!(gate(older.path(), &[]), Gate::Clear);

        let pending = tempfile::tempdir().unwrap();
        let mut record = approval(HEAD);
        record["sweep_pending"] = true.into();
        write(pending.path(), "PR-4718", record);
        assert_eq!(gate(pending.path(), &[]), Gate::Clear);

        let newer = tempfile::tempdir().unwrap();
        write(newer.path(), "PR-4718", approval(HEAD));
        let mut rc = approval(HEAD);
        rc["verdict"] = "request-changes".into();
        rc["findings"] = serde_json::json!(["still broken"]);
        rc["recorded_at"] = "2026-11-26T23:00:00Z".into();
        write(newer.path(), "BUG-2781", rc);
        assert_eq!(gate(newer.path(), &[]), Gate::Clear);

        // A conflicting or CI-red approved change is the reconciler's rework.
        let blocked = tempfile::tempdir().unwrap();
        write(blocked.path(), "PR-4718", approval(HEAD));
        let g = evaluate(blocked.path(), "PR-4718", &["BUG-2781"], &[], || {
            Some(Head {
                sha: HEAD.into(),
                blocked: Some("conflicts with its base".into()),
            })
        });
        assert_eq!(g, Gate::Clear);

        let empty = tempfile::tempdir().unwrap();
        let g = evaluate(empty.path(), "PR-4718", &["BUG-2781"], &[], || {
            panic!("no approval: the forge must not be asked")
        });
        assert_eq!(g, Gate::Clear);
    }

    #[test]
    fn despite_approval_admits_records_and_binds_to_the_head() {
        let tmp = tempfile::tempdir().unwrap();
        let store_root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        let backend = crate::test_env::home_fixture_backend(&store_root).unwrap();
        let storage = Storage::new(&store_root);
        let mut store = aida_core::RequirementsStore::default();
        let mut req = aida_core::Requirement::new("stale".into(), String::new());
        req.spec_id = Some("BUG-2781".into());
        store.requirements.push(req);
        backend.save(&store).unwrap();
        let req = storage.load().unwrap().requirements[0].clone();
        write(tmp.path(), "PR-4718", approval(HEAD));
        let Gate::Approved(a) = gate(tmp.path(), &req.comments) else {
            panic!("approval at head must refuse");
        };
        let err = apply(&storage, &req, "BUG-2781", Gate::Approved(a.clone()), None).unwrap_err();
        assert!(err.to_string().contains("approved-at-head"), "{err}");

        let why = "reviewer missed a regression";
        apply(
            &storage,
            &req,
            "BUG-2781",
            Gate::Approved(a.clone()),
            Some(why),
        )
        .unwrap();
        let comments = storage.load().unwrap().requirements[0].comments.clone();
        assert!(comments.iter().any(|c| c.content.contains(why)));
        // The launch-time check now admits this head, and only this head.
        assert_eq!(gate(tmp.path(), &comments), Gate::Overridden(a.clone()));
        let moved = HeadApproval {
            head_sha: OLD.into(),
            ..a
        };
        assert!(!override_recorded(&comments, &moved));
    }
}
