//! Shared refusal before AIDA moves an approved or reserved PR head.
// trace:BUG-2618 | ai:codex
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Debug)]
pub(crate) struct Protection {
    pub approval_head: Option<String>,
    pub approved_at_head: bool,
    pub train_pins: Vec<String>,
}

pub(crate) fn enabled(root: &Path) -> Result<bool> {
    let path = crate::config_path_for_project(root);
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).context("cannot read PR head protection policy"),
    };
    let doc: toml::Value = text.parse().context("invalid PR head protection policy")?;
    let Some(value) = doc.get("pr-head").and_then(|s| s.get("protect")) else {
        return Ok(false);
    };
    value.as_bool().context("pr-head.protect must be a boolean")
}

/// The open approval's reviewed head, refusing an approval with no head.
fn open_approval_head(
    verdict: Option<crate::gate::review_verdict::RecordedVerdict>,
) -> Result<Option<String>> {
    let Some(v) = verdict.filter(|v| v.kind.approves() && !v.is_closed()) else {
        return Ok(None);
    };
    match v.reviewed_sha.filter(|sha| !sha.trim().is_empty()) {
        Some(sha) => Ok(Some(sha)),
        None => anyhow::bail!("approval has no head binding; refusing PR head mutation"),
    }
}

/// Every approval head the canonical sources hold, each read strictly.
/// Lossy projections turn an unreadable ledger into "no events" and a
/// refused signed publication into a non-approval; either would let a
/// protected head move, so each source is read here with its errors kept.
// trace:BUG-2618 | ai:claude
fn approval_heads(root: &Path, key: &str, head: &str) -> Result<Vec<String>> {
    use crate::gate::review_verdict as rv;
    const REFUSE: &str = "refusing PR head mutation";
    let mut heads = Vec::new();
    // Signed publication at the live head being protected, never at this
    // node's possibly unfetched branch tip. A verification error refuses
    // rather than projecting request-changes.
    let published =
        crate::gate::review_publication::checked_current_at_head(root, &[key], head.trim())
            .with_context(|| {
                format!("cannot verify signed review publication for {key}; {REFUSE}")
            })?;
    if let Some(current) = published {
        let parsed = rv::parse_recorded_verdict(&current.body)
            .with_context(|| format!("invalid signed review publication for {key}; {REFUSE}"))?;
        heads.extend(open_approval_head(Some(parsed))?);
    }
    // Gate ledger: the strict reader errors on an unresolvable store or an
    // unreadable, linked or over-budget change ledger.
    if let Some(change) = crate::gate::ledger::ChangeKey::parse(key) {
        let records = crate::gate::ledger::project::change_verdict_records_checked(root, change)
            .map_err(|e| anyhow::anyhow!("cannot read approval ledger for {key}: {e}; {REFUSE}"))?;
        // The newest act of review; lifecycle annotations are not reviews.
        // A ledger approval at the head protects even where an older-format
        // legacy file would otherwise outrank it: refusal is the safe side.
        if let Some(record) = records
            .into_iter()
            .rfind(|r| !r.contains_key("terminal_wait_source"))
        {
            let parsed = rv::parse_ledger_record(&record)
                .with_context(|| format!("invalid ledger verdict for {key}; {REFUSE}"))?;
            heads.extend(open_approval_head(Some(parsed))?);
        }
    }
    // Legacy file plus the effective projection (overlays, closures).
    let verdict = rv::checked_current_verdict(root, key)
        .with_context(|| format!("cannot verify recorded approval; {REFUSE}"))?;
    heads.extend(open_approval_head(verdict)?);
    // The projection above lets an unpinned publication (selected at this
    // node's possibly stale branch tip) outrank the legacy file, so the
    // file, already validated there, is weighed on its own as well.
    // Read through the storage owner's strict reader: unreadable, invalid or
    // dangling-link evidence refuses; only true absence is no approval.
    // trace:BUG-2618 | ai:claude
    let legacy = rv::checked_legacy_verdict(root, key)
        .with_context(|| format!("cannot read recorded approval for {key}; {REFUSE}"))?;
    heads.extend(open_approval_head(legacy)?);
    heads.sort_unstable_by_key(|h| h.to_ascii_lowercase());
    heads.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(heads)
}

pub(crate) fn protection(root: &Path, key: &str, pr: u64, head: &str) -> Result<Protection> {
    // An empty live head would match no approval and silently allow.
    anyhow::ensure!(
        !head.trim().is_empty(),
        "{key} has no live head to check; refusing PR head mutation"
    );
    let heads = approval_heads(root, key, head)?;
    let approved_at_head = heads.iter().any(|h| h.eq_ignore_ascii_case(head.trim()));
    let approval_head = heads
        .iter()
        .find(|h| h.eq_ignore_ascii_case(head.trim()))
        .or(heads.first())
        .cloned();
    Ok(Protection {
        approved_at_head,
        approval_head,
        train_pins: if key.starts_with("PR-") {
            crate::pr_train::pr_head_pins(root, pr)?
        } else {
            Vec::new()
        },
    })
}

pub(crate) fn check(root: &Path, key: &str, pr: u64, head: &str) -> Result<()> {
    let root = crate::main_worktree_root_from(root);
    if !enabled(&root)? {
        return Ok(());
    }
    let p = protection(&root, key, pr, head)?;
    refuse(&p, key)
}

fn refuse(p: &Protection, key: &str) -> Result<()> {
    anyhow::ensure!(p.train_pins.is_empty(),
        "{key} head mutation refused: pinned by train {}. Retire the train through the human gate before moving this head.", p.train_pins.join(", "));
    anyhow::ensure!(!p.approved_at_head,
        "{key} head mutation refused: recorded approval at {}. Land through the train, or have a human revoke approval before moving this head.", p.approval_head.as_deref().unwrap_or("unknown"));
    Ok(())
}

/// Resolve live forge evidence rather than using an informational display cache.
pub(crate) fn check_branch(root: &Path, branch: &str) -> Result<()> {
    let main = crate::main_worktree_root_from(root);
    if !enabled(&main)? {
        return Ok(());
    }
    let kind = crate::forge::resolve_forge_kind(root);
    if kind == crate::forge::ForgeKind::None {
        return Ok(());
    }
    // trace:BUG-2618 | ai:codex — one branch may back PRs to multiple bases.
    // Refuse ambiguity before the single-change Forge lookup can hide one.
    if kind == crate::forge::ForgeKind::GitHub {
        match crate::pr_lookup::gh_pr_unique_head(root, branch) {
            crate::PrLookup::NoOpenPr => return Ok(()),
            crate::PrLookup::Found(change) => {
                let meta = crate::fetch_change_info_via_forge(root, change.number)?;
                return check(
                    &main,
                    &format!("PR-{}", change.number),
                    change.number,
                    &meta.head_oid,
                );
            }
            crate::PrLookup::GhFailed(message) | crate::PrLookup::GhUnreachable(message) => {
                anyhow::bail!(
                    "cannot verify PR head protection for `{branch}`: {message}; nothing pushed"
                )
            }
            crate::PrLookup::GhMissing => anyhow::bail!(
                "cannot verify PR head protection for `{branch}`: gh missing; nothing pushed"
            ),
        }
    }
    let forge = crate::forge::forge_for_kind(root, kind);
    match forge.change_for_branch(branch)? {
        crate::forge::ChangeLookup::NoChange => Ok(()),
        crate::forge::ChangeLookup::Found(change) => {
            let meta = crate::fetch_change_info_via_forge(root, change.id)?;
            let key = format!("{}-{}", kind.change_noun(), change.id);
            check(&main, &key, change.id, &meta.head_oid)
        }
        other => anyhow::bail!(
            "cannot verify PR head protection for `{branch}`: {other:?}; nothing pushed"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug2618_guard_refuses_current_approval_and_any_train_pin() {
        let mut p = Protection {
            approval_head: Some("old".into()),
            approved_at_head: false,
            train_pins: vec![],
        };
        assert!(refuse(&p, "PR-7").is_ok());
        p.approved_at_head = true;
        assert!(refuse(&p, "PR-7")
            .unwrap_err()
            .to_string()
            .contains("approval at old"));
        p.approved_at_head = false;
        p.train_pins.push("train/example (PR #8)".into());
        assert!(refuse(&p, "PR-7")
            .unwrap_err()
            .to_string()
            .contains("train/example (PR #8)"));
    }

    #[test]
    fn bug2618_guard_reads_recorded_head_and_refuses_malformed_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".aida/review-verdicts")).unwrap();
        let path = root.join(".aida/review-verdicts/PR-7.json");
        std::fs::write(
            &path,
            r#"{"verdict":"APPROVED","reviewed_sha":"abc","recorded_by":"reviewer-a"}"#,
        )
        .unwrap();
        let p = protection(root, "PR-7", 7, "abc").unwrap();
        assert!(p.approved_at_head);
        assert!(refuse(&p, "PR-7").is_err());
        assert!(!protection(root, "PR-7", 7, "def").unwrap().approved_at_head);
        std::fs::write(&path, "broken").unwrap();
        assert!(protection(root, "PR-7", 7, "abc").is_err());
    }

    /// Round-3 finding 2: with no legacy JSON, a ledger-only approval
    /// protects the head, and an unreadable ledger refuses instead of
    /// reading as "no approval".
    // trace:BUG-2618 | ai:claude
    #[test]
    fn bug2618_ledger_only_approval_protects_and_unreadable_ledger_refuses() {
        use crate::gate::ledger::{store, ChangeKey, EventBody, GateEvent};
        use std::os::unix::fs::PermissionsExt;
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let change = ChangeKey::pr(7);
        let mut record = serde_json::Map::new();
        for (k, v) in [
            ("verdict", "approved"),
            ("reviewed_sha", "abc"),
            ("recorded_by", "reviewer-a"),
            ("recorded_at", &chrono::Utc::now().to_rfc3339()),
        ] {
            record.insert(k.into(), serde_json::Value::String(v.into()));
        }
        let mut event = GateEvent::new(change, EventBody::Verdict { record });
        event.head = Some("abc".into());
        store::append_one(&root, event).unwrap();
        assert!(!root.join(".aida/review-verdicts/PR-7.json").exists());

        let p = protection(&root, "PR-7", 7, "abc").unwrap();
        assert!(p.approved_at_head, "ledger-only approval must protect");
        assert!(refuse(&p, "PR-7").is_err());
        assert!(
            !protection(&root, "PR-7", 7, "def")
                .unwrap()
                .approved_at_head
        );

        let ledger = store::ledger_file(&root, change);
        std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Only root (CAP_DAC_OVERRIDE) can still open a 0o000 file.
        let readable = std::fs::File::open(&ledger).is_ok();
        // SAFETY: geteuid has no preconditions.
        assert!(!readable || unsafe { libc::geteuid() } == 0);
        if !readable {
            for head in ["abc", "def"] {
                let err = protection(&root, "PR-7", 7, head).unwrap_err();
                assert!(
                    format!("{err:#}").contains("refusing PR head mutation"),
                    "{err:#}"
                );
            }
        }
        std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn bug2618_policy_is_explicit_and_malformed_values_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = crate::config_path_for_project(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        assert!(!enabled(dir.path()).unwrap());
        std::fs::write(&path, "[pr-head]\nprotect = true\n").unwrap();
        assert!(enabled(dir.path()).unwrap());
        std::fs::write(&path, "[pr-head]\nprotect = 'yes'\n").unwrap();
        assert!(enabled(dir.path()).is_err());
    }

    #[test]
    fn bug2618_train_pin_names_owner_and_ignores_retired_members() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let journal = root.join(".aida/train");
        std::fs::create_dir_all(&journal).unwrap();
        let path = journal.join("example.json");
        let mut entry = serde_json::json!({
            "version": 1, "branch": "train/example", "change_id": 8,
            "step": "Ci", "completed_members": [], "rejected": [], "call": "",
            "train": { "base": "base", "tip": "tip", "cars": [{
                "member": { "pr": 7, "head": "old", "subject": "subject", "body": "" },
                "commit": "commit", "patch_id": "patch"
            }], "dropped": [] }
        });
        std::fs::write(&path, entry.to_string()).unwrap();
        let p = protection(root, "PR-7", 7, "new").unwrap();
        assert_eq!(p.train_pins, ["train/example (PR #8)"]);
        assert!(refuse(&p, "PR-7").is_err());
        // GitLab's MR number cannot borrow a GitHub train reservation.
        assert!(protection(root, "MR-7", 7, "new")
            .unwrap()
            .train_pins
            .is_empty());
        entry["completed_members"] = serde_json::json!([7]);
        std::fs::write(&path, entry.to_string()).unwrap();
        assert!(protection(root, "PR-7", 7, "new")
            .unwrap()
            .train_pins
            .is_empty());
        entry["completed_members"] = serde_json::json!([]);
        entry["step"] = serde_json::json!("Done");
        std::fs::write(&path, entry.to_string()).unwrap();
        assert!(protection(root, "PR-7", 7, "new")
            .unwrap()
            .train_pins
            .is_empty());
        std::fs::write(&path, "broken").unwrap();
        assert!(protection(root, "PR-7", 7, "new").is_err());
    }

    #[test]
    fn bug2618_linked_review_evidence_refuses() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".aida/review-verdicts")).unwrap();
        let target = root.join("outside.json");
        let path = root.join(".aida/review-verdicts/PR-7.json");
        std::fs::write(
            &target,
            r#"{"verdict":"APPROVED","reviewed_sha":"abc","recorded_by":"reviewer-a"}"#,
        )
        .unwrap();
        symlink(&target, &path).unwrap();
        assert!(protection(root, "PR-7", 7, "abc").is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&target, &path).unwrap();
        assert!(protection(root, "PR-7", 7, "abc").is_err());
    }
}
