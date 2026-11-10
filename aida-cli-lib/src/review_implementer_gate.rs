//! Review admission waits for live implementation work, without reserving a head.
// trace:STORY-2705 | ai:codex
use aida_core::liveness::LeaseState;
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Debug)]
struct Holder {
    scope: String,
    branch: Option<String>,
    host: String,
    lease: String,
}

fn local_holders(root: &Path) -> Result<Vec<Holder>> {
    let leases = crate::leases::list_leases_strict_all(root).map_err(anyhow::Error::msg)?;
    Ok(crate::leases::classify_session_leases(root, &leases)
        .into_iter()
        .filter(|(l, state, _)| {
            *state == LeaseState::Live
                && !l.review_verb
                && l.role.as_deref().is_some_and(|r| {
                    crate::canonical_role_name(r).eq_ignore_ascii_case("implementer")
                })
        })
        .map(|(l, _, _)| Holder {
            scope: l.scope,
            branch: Some(l.branch),
            host: l.hostname,
            lease: l.id,
        })
        .collect())
}

fn reason(holders: &[Holder], branch: &str, specs: &[String]) -> Option<String> {
    holders
        .iter()
        .find(|h| {
            (!branch.is_empty() && h.branch.as_deref() == Some(branch))
                || specs.iter().any(|s| s.eq_ignore_ascii_case(&h.scope))
        })
        .map(|h| format!("deferred: implementer lane live ({}, {})", h.host, h.lease))
}

/// Cross-host claims use the existing heartbeat TTL; local records require
/// positive process liveness, so a dormant or abandoned lease cannot park review.
/// The caller retries admission each pass; no verdict or attempt is written here.
// trace:STORY-2705 | ai:codex
pub(crate) fn deferred(root: &Path, number: u64, gitlab: bool) -> Result<Option<String>> {
    deferred_with_metadata(root, || {
        let kind = if gitlab {
            crate::forge::ForgeKind::GitLab
        } else {
            crate::forge::ForgeKind::GitHub
        };
        let meta = crate::forge::forge_for_kind(root, kind)
            .change_metadata(number, &mut crate::network_retry::NoopSink)
            .context("cannot establish implementer review deferral: change metadata unavailable")?;
        Ok((meta.title, meta.head_ref))
    })
}

fn deferred_with_metadata(
    root: &Path,
    metadata: impl FnOnce() -> Result<(String, String)>,
) -> Result<Option<String>> {
    let mut holders = local_holders(root)?;
    let host = crate::coordination::hostname();
    let now = chrono::Utc::now();
    for claim in crate::coordination::list_claims(&root.join(".aida-store")) {
        if claim.review_verb
            || !crate::canonical_role_name(&claim.agent).eq_ignore_ascii_case("implementer")
        {
            continue;
        }
        if claim.host == host {
            // Same-host clones can be inspected rather than trusting a leftover
            // claim until its TTL. The primary clone is already covered above.
            let clone = Path::new(&claim.clone_path);
            if !claim.clone_path.is_empty() && clone != root && clone.is_dir() {
                holders.extend(local_holders(clone)?);
            }
        } else if remote_live(&claim, now, &host) {
            holders.push(Holder {
                scope: claim.scope,
                branch: None,
                host: claim.host,
                lease: claim.claim_id.unwrap_or(claim.node_id),
            });
        }
    }
    if holders.is_empty() {
        return Ok(None);
    }
    let (title, branch) = metadata()?;
    let mut specs = crate::pr_ship::extract_authorship_spec_ids_from_subject(&title);
    specs.extend(crate::pr_ship::extract_spec_ids_from_text(&branch));
    // Friendly and origin IDs remain interchangeable after store remapping.
    if let Some(store) = crate::load_store_for_lookup(root) {
        for id in specs.clone() {
            if let Some(req) = store.get_requirement_by_spec_id(&id) {
                specs.extend(req.spec_id.iter().chain(req.agreed_id.iter()).cloned());
            }
        }
    }
    Ok(reason(&holders, &branch, &specs))
}

fn remote_live(
    claim: &crate::coordination::Claim,
    now: chrono::DateTime<chrono::Utc>,
    host: &str,
) -> bool {
    // Invalid/future timestamps are not affirmative liveness evidence.
    let Ok(heartbeat) = chrono::DateTime::parse_from_rfc3339(&claim.heartbeat_at) else {
        return false;
    };
    !claim.host.is_empty()
        && heartbeat <= now
        && crate::coordination::claim_staleness(claim, now, host, |_, _| false, |_| false).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder() -> Holder {
        Holder {
            scope: "STORY-1".into(),
            branch: Some("fix-one".into()),
            host: "host-two".into(),
            lease: "lease-one".into(),
        }
    }

    #[test]
    fn story2705_real_local_lease_only_blocks_while_process_is_live() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let pid = std::process::id();
        let mut lease: crate::leases::SessionLease = serde_json::from_value(serde_json::json!({
            "id":"abc123def456", "scope":"STORY-1", "slug":"story-1", "owner":"fixture",
            "worktree_path": root, "branch":"fix-one", "started_at": chrono::Utc::now(),
            "hostname":crate::coordination::hostname(), "role":"implementer", "active_pid":pid,
            "active_pid_start_time":crate::process_probe::process_start_identity(pid),
            "pid_namespace":crate::agent_registry::current_pid_namespace(),
            "holder_record":serde_json::to_string(&aida_core::proc_identity::HolderRecord::capture(pid)).unwrap()
        })).unwrap();
        let path = crate::leases::lease_path(root, &lease.id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let write = |l: &crate::leases::SessionLease| {
            std::fs::write(&path, toml::to_string(l).unwrap()).unwrap()
        };
        write(&lease);
        // Refs slices also belong to a spec, regardless of completion credit.
        let metadata = || Ok(("fix: partial (Refs STORY-1)".into(), "other-branch".into()));
        assert!(deferred_with_metadata(root, metadata).unwrap().is_some());
        lease.role = Some("reviewer".into());
        write(&lease);
        assert!(
            deferred_with_metadata(root, || panic!("no implementer needs metadata"))
                .unwrap()
                .is_none()
        );
        lease.role = Some("implementer".into());
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        lease.active_pid = Some(child.id());
        lease.active_pid_start_time = crate::process_probe::process_start_identity(child.id());
        lease.holder_record = Some(
            serde_json::to_string(&aida_core::proc_identity::HolderRecord::capture(child.id()))
                .unwrap(),
        );
        child.kill().unwrap();
        child.wait().unwrap();
        write(&lease);
        assert!(
            deferred_with_metadata(root, || panic!("dead lease must not block"))
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(path).unwrap();
        assert!(
            deferred_with_metadata(root, || panic!("ended lease must not block"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn story2705_live_spec_defers_even_on_another_branch() {
        assert_eq!(
            reason(&[holder()], "rework", &["STORY-1".into()]).as_deref(),
            Some("deferred: implementer lane live (host-two, lease-one)")
        );
        assert!(reason(&[holder()], "unrelated", &["STORY-2".into()]).is_none());
    }

    #[test]
    fn story2705_lease_end_allows_same_head_without_merge_only_permanent_block() {
        // Merge-only and ordinary heads obey the same gate, including after a
        // changes-requested round: no live lease means no deferral.
        assert!(reason(&[holder()], "fix-one", &[]).is_some());
        assert!(reason(&[], "fix-one", &["STORY-1".into()]).is_none());
    }

    #[test]
    fn story2705_cross_host_registry_claim_blocks_only_implementation_until_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let now = chrono::Utc::now();
        let mut claim: crate::coordination::Claim = serde_json::from_value(serde_json::json!({
            "scope":"STORY-1", "node_id":"node-two", "claim_id":"lease-two",
            "clone_path":"/remote", "host":"remote-fixture-host",
            "pid":1, "agent":"implementer", "started_at":now.to_rfc3339(),
            "heartbeat_at":now.to_rfc3339(), "ttl_secs":60
        }))
        .unwrap();
        let path = crate::coordination::claim_path(&root.join(".aida-store"), &claim.scope);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let write = |c: &crate::coordination::Claim| {
            std::fs::write(&path, toml::to_string(c).unwrap()).unwrap()
        };
        let metadata = || Ok(("fix: partial (Refs STORY-1)".into(), "other-branch".into()));
        write(&claim);
        assert_eq!(
            deferred_with_metadata(root, metadata).unwrap().as_deref(),
            Some("deferred: implementer lane live (remote-fixture-host, lease-two)")
        );
        claim.agent = "advisor".into();
        write(&claim);
        assert!(
            deferred_with_metadata(root, || panic!("advisor must not defer"))
                .unwrap()
                .is_none()
        );
        claim.agent = "implementer".into();
        claim.heartbeat_at = (now - chrono::Duration::seconds(61)).to_rfc3339();
        write(&claim);
        assert!(
            deferred_with_metadata(root, || panic!("expired claim must not defer"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn story2705_remote_claim_expires_and_invalid_heartbeat_never_blocks() {
        let now = chrono::Utc::now();
        let mut claim: crate::coordination::Claim = serde_json::from_value(serde_json::json!({
            "scope":"STORY-1", "node_id":"node-two", "clone_path":"/remote", "host":"host-two",
            "pid":1, "agent":"implementer", "started_at":now.to_rfc3339(), "heartbeat_at":now.to_rfc3339(), "ttl_secs":60
        })).unwrap();
        assert!(remote_live(&claim, now, "host-one"));
        assert!(!remote_live(
            &claim,
            now + chrono::Duration::seconds(61),
            "host-one"
        ));
        claim.heartbeat_at = "invalid".into();
        assert!(!remote_live(&claim, now, "host-one"));
    }
}
