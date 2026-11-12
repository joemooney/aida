//! Retire terminal batch waits without completing their owning spec.
// trace:BUG-2525 | ai:codex
use crate::gate::review_verdict as rv;
use std::path::Path;

fn change_from_review_url(root: &Path, url: &str) -> Option<crate::gate::ledger::ChangeKey> {
    let origin = crate::forge::origin_url(root)?;
    let host = crate::forge::forge_host_of(&origin)?;
    let project = crate::forge::project_path_of(&origin)?;
    let prefix = format!("https://{host}/{project}/");
    let tail = url.strip_prefix(&prefix)?;
    let (forge, number) = if let Some(n) = tail.strip_prefix("pull/") {
        (crate::gate::ledger::ChangeForge::Pr, n)
    } else {
        (
            crate::gate::ledger::ChangeForge::Mr,
            tail.strip_prefix("-/merge_requests/")?,
        )
    };
    let number = number.split(['/', '#', '?']).next()?.parse::<u64>().ok()?;
    (number > 0).then_some(crate::gate::ledger::ChangeKey { forge, number })
}

/// Close waits belonging to terminal batches independently of spec completion.
/// Unknown forge evidence and open changes retain the wait. Explicit change
/// identity survives a moved PR head; index-only matches require head evidence.
/// This annotates the old review; it grants no approval or spec completion.
// trace:BUG-2525 | ai:codex
pub(crate) fn reconcile_waiting_verdicts(project_root: &Path, keys: Option<&[String]>) {
    use crate::forge_observed::Observed;
    let forge = crate::forge::forge_for(project_root);
    let kind = forge.kind();
    reconcile_waiting_verdicts_with(project_root, keys, |change| {
        use crate::gate::ledger::ChangeForge;
        if !matches!(
            (kind, change.forge),
            (crate::forge::ForgeKind::GitHub, ChangeForge::Pr)
                | (crate::forge::ForgeKind::GitLab, ChangeForge::Mr)
        ) {
            return None;
        }
        match forge.observe_change_metadata(change.number, &mut crate::network_retry::NoopSink) {
            Observed::Known(Ok(meta)) => Some((meta.state, meta.head_sha)),
            _ => None,
        }
    });
}

// Injectable observation keeps regression tests independent of PATH and live APIs.
// trace:BUG-2525 | ai:codex
pub(crate) fn reconcile_waiting_verdicts_with(
    project_root: &Path,
    keys: Option<&[String]>,
    mut observe: impl FnMut(
        crate::gate::ledger::ChangeKey,
    ) -> Option<(crate::forge::ChangeState, String)>,
) {
    use crate::forge::ChangeState;
    use crate::gate::ledger::{self, ChangeKey};
    let mut observations = std::collections::HashMap::new();
    let candidates = keys
        .map(<[String]>::to_vec)
        .unwrap_or_else(|| rv::verdict_keys(project_root));
    for key in candidates {
        let Some(source) = rv::current_verdict(project_root, &key) else {
            continue;
        };
        let Some(verdict) = rv::parse_recorded_verdict(&source.body) else {
            continue;
        };
        if verdict.kind != rv::VerdictKind::Waiting || verdict.is_closed() {
            continue;
        }
        let Some(head) = verdict
            .reviewed_sha
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        else {
            continue;
        };
        let mut changes = Vec::new();
        let mut explicitly_named = ChangeKey::parse(&key).is_some();
        if let Some(change) = ChangeKey::parse(&key) {
            changes.push(change);
        } else {
            // Verified publication projections carry their repository-bound
            // change in the receipt identity, even without a local index.
            let signed_source = crate::gate::review_publication::current(project_root, &[&key])
                .is_some_and(|signed| signed.body == source.body && signed.source == source.source);
            if let Some(receipt) = verdict
                .attestation
                .as_ref()
                .and_then(|a| a.receipt_id.as_deref())
                .filter(|_| signed_source)
            {
                if let Some(change) = receipt
                    .strip_prefix("publication:")
                    .and_then(|r| r.split(':').next())
                    .and_then(ChangeKey::parse)
                {
                    explicitly_named = true;
                    changes.push(change);
                }
            }
            if changes.is_empty() {
                if let Some(url) = &verdict.comment_url {
                    // A foreign URL is not evidence for a same-number local PR.
                    // Nor may an invalid explicit URL fall back to the spec index.
                    let Some(change) = change_from_review_url(project_root, url) else {
                        continue;
                    };
                    explicitly_named = true;
                    changes.push(change);
                }
            }
            // An explicit review URL identifies the batch. Only fall back to
            // the spec index when the round does not name its own change.
            if changes.is_empty() {
                if let Ok(root) = ledger::store_root(project_root) {
                    changes.extend(ledger::project::changes_for_spec(&root, &key));
                }
            }
        }
        for change in changes {
            let observation = observations
                .entry(change)
                .or_insert_with(|| observe(change));
            let Some((state, terminal_head)) = observation else {
                continue;
            };
            if matches!(state, ChangeState::Merged | ChangeState::Closed)
                && (explicitly_named || rv::same_reviewed_sha(head, terminal_head))
            {
                if let Err(error) = rv::close_terminal_wait(project_root, &key, change, &source) {
                    eprintln!("warning: could not close terminal batch review for {key}: {error}");
                }
                break;
            }
        }
    }
}

// trace:BUG-2525 | ai:codex
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_2525_review_urls_require_exact_repository() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(tmp.path())
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .current_dir(tmp.path())
            .args(["remote", "add", "origin", "git@github.com:owner/repo.git"])
            .status()
            .unwrap()
            .success());
        assert_eq!(
            change_from_review_url(
                tmp.path(),
                "https://github.com/owner/repo/pull/7#issuecomment-1"
            ),
            Some(crate::gate::ledger::ChangeKey::pr(7))
        );
        for url in [
            "https://github.com/other/repo/pull/7",
            "https://evil.example/owner/repo/pull/7",
            "https://github.com/owner/repo-extra/pull/7",
            "https://github.com/owner/repo/pull/7junk",
            "https://github.com/owner/repo/pull/0",
        ] {
            assert!(change_from_review_url(tmp.path(), url).is_none(), "{url}");
        }
        let dir = tmp.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        let forged = crate::gate::review_verdict::RecorderAttestation {
            receipt_id: Some("publication:PR-7:forged:00000000-0000-0000-0000-000000000000".into()),
            ..Default::default()
        };
        std::fs::write(
            dir.join("TASK-3448.json"),
            serde_json::json!({
                "verdict":"waiting", "reviewed_sha":"1111111111111111111111111111111111111111",
                "summary":"external wait", "comment_url":"https://github.com/other/repo/pull/7",
                "recorder_attestation":forged
            })
            .to_string(),
        )
        .unwrap();
        reconcile_waiting_verdicts_with(tmp.path(), Some(&["TASK-3448".into()]), |_| {
            panic!("a self-asserted publication identity cannot bypass repository validation")
        });
        assert!(!rv::read_recorded_verdict(tmp.path(), "TASK-3448")
            .unwrap()
            .is_closed());
    }
}
