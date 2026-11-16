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

pub(crate) fn protection(root: &Path, key: &str, pr: u64, head: &str) -> Result<Protection> {
    let verdict = crate::gate::review_verdict::checked_current_verdict(root, key)
        .context("cannot verify recorded approval; refusing PR head mutation")?;
    anyhow::ensure!(
        verdict.as_ref().is_none_or(|v| !v.kind.approves()
            || v.is_closed()
            || v.reviewed_sha.as_deref().is_some_and(|sha| !sha.is_empty())),
        "approval has no head binding; refusing PR head mutation"
    );
    let approval_head = verdict
        .filter(|v| v.kind.approves() && !v.is_closed())
        .and_then(|v| v.reviewed_sha);
    Ok(Protection {
        approved_at_head: approval_head.as_deref() == Some(head),
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
