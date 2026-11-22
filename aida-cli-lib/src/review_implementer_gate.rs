//! Review admission waits for live implementation work, without reserving a head.
// trace:STORY-2705 | ai:codex
use aida_core::liveness::LeaseState;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::path::Path;

/// Minutes a head stays deferred behind a lane on another host whose state
/// can no longer be read, unless `[review] lane_unknown_defer_mins` says
/// otherwise.
// trace:BUG-2706 | ai:claude
pub(crate) const DEFAULT_UNKNOWN_DEFER_MINS: i64 = 20;
/// The setting is a bound, so a value past a day is not honoured.
const MAX_UNKNOWN_DEFER_MINS: i64 = 24 * 60;
/// How far ahead of this host's clock another host's heartbeat may be and
/// still count. Past this the timestamp is not evidence of anything.
#[cfg(test)]
use crate::lane_hosts::CLOCK_SKEW_SECS;
/// What is known about a lane that holds the change's spec or branch.
// trace:BUG-2706 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
enum Standing {
    /// Positively live: a local process, or a fresh heartbeat from its host.
    Live,
    Unreadable(String),
    /// On another host, claim unreleased, heartbeat past its TTL: the lane's
    /// state cannot be read. Deferred until `until`.
    Unknown {
        silent_mins: i64,
        until: DateTime<Utc>,
    },
    /// As `Unknown`, but the bound has passed: the head is reviewed.
    Lapsed {
        silent_mins: i64,
    },
}

#[derive(Debug)]
struct Holder {
    scope: String,
    branch: Option<String>,
    host: String,
    lease: String,
    standing: Standing,
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
            standing: Standing::Live,
        })
        .collect())
}

/// The outcome for one change: the deferral, if any, and otherwise why a lane
/// whose state could not be read no longer holds the head back.
// trace:BUG-2706 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
struct Decision {
    unreadable: Option<String>,
    reason: Option<String>,
    /// One `(lane, explanation)` per lane whose bound lapsed.
    lapsed: Vec<(String, String)>,
}

fn decision(holders: &[Holder], branch: &str, specs: &[String], grace_mins: i64) -> Decision {
    let on_change: Vec<&Holder> = holders
        .iter()
        .filter(|h| {
            (!branch.is_empty() && h.branch.as_deref() == Some(branch))
                || specs.iter().any(|s| s.eq_ignore_ascii_case(&h.scope))
        })
        .collect();
    // A positively live lane outranks one whose state is unknown; among the
    // unknown, the one that holds the head longest is named.
    if let Some(h) = on_change.iter().find(|h| h.standing == Standing::Live) {
        return Decision {
            reason: Some(format!(
                "deferred: implementer lane live ({}, {})",
                h.host, h.lease
            )),
            ..Decision::default()
        };
    }
    let unknown = on_change
        .iter()
        .filter_map(|h| match h.standing {
            Standing::Unknown { silent_mins, until } => Some((until, silent_mins, h)),
            _ => None,
        })
        .max_by_key(|(until, _, _)| *until);
    if let Some((until, silent_mins, h)) = unknown {
        return Decision {
            reason: Some(format!(
                "deferred: implementer lane state unknown ({}, {}): no heartbeat for {silent_mins} min; reviewed after {} UTC unless the lane reports or ends",
                h.host,
                h.lease,
                until.format("%H:%M")
            )),
            ..Decision::default()
        };
    }
    Decision {
        reason: None,
        unreadable: on_change.iter().find_map(|h| match &h.standing { Standing::Unreadable(why) => Some(why.clone()), _ => None }),
        lapsed: on_change
            .iter()
            .filter_map(|h| match h.standing {
                Standing::Lapsed { silent_mins } => Some((
                    format!("{}/{}", h.host, h.lease),
                    format!(
                        "implementer lane state on {} ({}) could not be read: no heartbeat for {silent_mins} min, past the {grace_mins}-minute bound; reviewing",
                        h.host, h.lease
                    ),
                )),
                _ => None,
            })
            .collect(),
    }
}

/// `[review] lane_unknown_defer_mins`: a whole number of minutes from 0 to a
/// day. Unset, unreadable or out of range keeps the default, so a mistyped
/// value can neither park review nor remove the bound.
// trace:BUG-2706 | ai:claude
fn unknown_defer_mins(root: &Path) -> i64 {
    crate::review_helpers::project_config_text(root)
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|config| {
            config
                .get("review")?
                .get("lane_unknown_defer_mins")?
                .as_integer()
        })
        .filter(|mins| (0..=MAX_UNKNOWN_DEFER_MINS).contains(mins))
        .unwrap_or(DEFAULT_UNKNOWN_DEFER_MINS)
}

/// Local records require positive process liveness, so a dormant or abandoned
/// lease cannot park review. A lane on another host is judged by the heartbeat
/// its launcher keeps on the shared claim: fresh is live; expired with the
/// claim still unreleased is unknown and defers for a bounded time only.
/// The caller retries admission each pass; no verdict or attempt is written here.
// trace:STORY-2705 | ai:codex
// trace:BUG-2706 | ai:claude
pub(crate) fn deferred(root: &Path, number: u64, gitlab: bool) -> Result<Option<String>> {
    deferred_change(
        root,
        &format!("{}-{number}", if gitlab { "MR" } else { "PR" }),
        || {
            let kind = if gitlab {
                crate::forge::ForgeKind::GitLab
            } else {
                crate::forge::ForgeKind::GitHub
            };
            let meta = crate::forge::forge_for_kind(root, kind)
                .change_metadata(number, &mut crate::network_retry::NoopSink)
                .context(
                    "cannot establish implementer review deferral: change metadata unavailable",
                )?;
            Ok((meta.title, meta.head_ref))
        },
    )
}

#[cfg(test)]
pub(crate) fn deferred_with_metadata(
    root: &Path,
    metadata: impl FnOnce() -> Result<(String, String)>,
) -> Result<Option<String>> {
    deferred_change(root, "fixture", metadata)
}

#[derive(serde::Serialize, serde::Deserialize, Default, PartialEq)]
struct Episode {
    held: bool,
    unreadable_since: Option<DateTime<Utc>>,
}

fn deferred_change(
    root: &Path,
    key: &str,
    metadata: impl FnOnce() -> Result<(String, String)>,
) -> Result<Option<String>> {
    let grace = unknown_defer_mins(root);
    let now = Utc::now();
    let episodes = crate::review_watch::Directory::lane_episodes(root)?;
    let previously_held = episodes
        .read(key)?
        .as_deref()
        .map(serde_json::from_slice::<Episode>)
        .transpose()?
        .is_some_and(|episode| episode.held);
    let decision = observe_change(
        root,
        &crate::coordination::hostname(),
        now,
        grace,
        previously_held,
        metadata,
    )?;
    settle_episode(root, key, decision, now, grace)
}

fn settle_episode(
    root: &Path,
    key: &str,
    decision: Decision,
    now: DateTime<Utc>,
    grace: i64,
) -> Result<Option<String>> {
    let dir = crate::review_watch::Directory::lane_episodes(root)?;
    let _lock = dir.lock("episodes.lock")?;
    let bytes = dir.read(key)?;
    let mut episode: Episode = bytes
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?
        .unwrap_or_default();
    let mut reason = decision.reason;
    if let Some(detail) = decision.unreadable.as_deref().filter(|_| reason.is_none()) {
        let since = *episode.unreadable_since.get_or_insert(now);
        if now.signed_duration_since(since) < Duration::minutes(grace) {
            reason = Some(format!(
                "deferred: implementer lane state unknown: {detail}"
            ));
        }
    } else {
        episode.unreadable_since = None;
    }
    let released = episode.held && reason.is_none();
    episode.held = reason.is_some();
    if episode != Episode::default() {
        let new = serde_json::to_vec(&episode)?;
        if bytes.as_deref() != Some(new.as_slice()) {
            dir.write(key, &new)?;
        }
    } else if bytes.is_some() {
        dir.remove(key)?;
    }
    if released && decision.unreadable.is_some() {
        eprintln!("{key}: implementer lane evidence unreadable past the {grace}-minute bound; reviewing ({})", decision.unreadable.as_deref().unwrap_or_default());
    } else if released {
        let detail = decision
            .lapsed
            .first()
            .map(|(_, detail)| detail.as_str())
            .unwrap_or("implementer lane released; reviewing");
        eprintln!("{key}: {detail}");
    }
    Ok(reason)
}

fn observed_claims(root: &Path) -> Result<crate::lane_hosts::Inventory> {
    let store = root.join(".aida-store");
    if !store.try_exists()? {
        anyhow::ensure!(!store.is_symlink(), "shared claim store is unavailable");
        return Ok(crate::lane_hosts::Inventory::default());
    }
    if store.is_file() {
        return Ok(crate::lane_hosts::Inventory::default());
    }
    if store.join(".git").try_exists()? && aida_core::git_ops::probe_remote(&store, "origin")? {
        crate::lane_hosts::published_claims(&store)
    } else {
        crate::coordination::strict_claims(&store)
    }
}

#[cfg(test)]
fn decide(
    root: &Path,
    host: &str,
    now: DateTime<Utc>,
    grace_mins: i64,
    metadata: impl FnOnce() -> Result<(String, String)>,
) -> Result<Decision> {
    observe_change(root, host, now, grace_mins, true, metadata)
}

fn observe_change(
    root: &Path,
    host: &str,
    now: DateTime<Utc>,
    grace_mins: i64,
    previously_held: bool,
    metadata: impl FnOnce() -> Result<(String, String)>,
) -> Result<Decision> {
    let mut holders = local_holders(root)?;
    let inventory = observed_claims(root).unwrap_or_else(|error| crate::lane_hosts::Inventory {
        claims: Vec::new(),
        incomplete: Some(error.to_string()),
    });
    for claim in inventory.claims {
        // Legacy spec claims omitted the scope-derived role. Keep those
        // conservative until their existing liveness evidence expires.
        // trace:STORY-2705 | ai:codex
        let implementation_role = crate::canonical_role_name(&claim.agent)
            .eq_ignore_ascii_case("implementer")
            || (claim.agent.trim().is_empty()
                && aida_core::object_store::valid_spec_id_format(&claim.scope));
        if claim.review_verb || !implementation_role {
            continue;
        }
        if claim.host == host {
            // Same-host clones can be inspected rather than trusting a leftover
            // claim until its TTL. The primary clone is already covered above.
            let clone = Path::new(&claim.clone_path);
            if !claim.clone_path.is_empty() && clone != root && clone.is_dir() {
                holders.extend(local_holders(clone)?);
            }
        } else {
            let (standing, lease) = match crate::lane_hosts::judge(&claim, now, host) {
                crate::lane_hosts::Judged::NotForeign => continue,
                crate::lane_hosts::Judged::Invalid(why) => (
                    Standing::Unreadable(why),
                    claim.lease_id.or(claim.claim_id).unwrap_or(claim.node_id),
                ),
                crate::lane_hosts::Judged::Lane(lane) => (
                    lane_standing(&lane, now, grace_mins),
                    lane.lease().to_owned(),
                ),
            };
            if !previously_held && matches!(standing, Standing::Lapsed { .. }) {
                continue;
            }
            holders.push(Holder {
                scope: claim.scope,
                branch: None,
                host: claim.host,
                lease,
                standing,
            });
        }
    }
    if holders.is_empty() {
        return Ok(Decision {
            unreadable: inventory.incomplete,
            ..Decision::default()
        });
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
    let mut result = decision(&holders, &branch, &specs, grace_mins);
    result.unreadable = result.unreadable.or(inventory.incomplete);
    Ok(result)
}

#[cfg(test)]
fn remote_standing(
    claim: &crate::coordination::Claim,
    now: DateTime<Utc>,
    host: &str,
    grace_mins: i64,
) -> Option<Standing> {
    match crate::lane_hosts::judge(claim, now, host) {
        crate::lane_hosts::Judged::Lane(lane) => Some(lane_standing(&lane, now, grace_mins)),
        _ => None,
    }
}

fn lane_standing(lane: &crate::lane_hosts::Lane, now: DateTime<Utc>, grace_mins: i64) -> Standing {
    if lane.state == crate::lane_hosts::State::Reporting {
        return Standing::Live;
    }
    let Some(until) = lane
        .expires
        .checked_add_signed(Duration::minutes(grace_mins))
    else {
        return Standing::Unreadable("invalid claim expiry".into());
    };
    let silent_mins = lane.age_mins(now);
    if now < until {
        Standing::Unknown { silent_mins, until }
    } else {
        Standing::Lapsed { silent_mins }
    }
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
            standing: Standing::Live,
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

    fn reason(holders: &[Holder], branch: &str, specs: &[String]) -> Option<String> {
        decision(holders, branch, specs, DEFAULT_UNKNOWN_DEFER_MINS).reason
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
        for role in ["reviewer", "advisor"] {
            claim.agent = role.into();
            write(&claim);
            assert!(
                deferred_with_metadata(root, || panic!("explicit role must not defer"))
                    .unwrap()
                    .is_none()
            );
        }
        claim.agent.clear();
        write(&claim);
        assert!(deferred_with_metadata(root, metadata).unwrap().is_some());
        claim.scope = "workspace".into();
        write(&claim);
        assert!(
            deferred_with_metadata(root, || panic!("non-spec must not defer"))
                .unwrap()
                .is_none()
        );
        // Expired and unreleased: the lane's state is unknown, so the head
        // waits out the bound (BUG-2706) and is then reviewed.
        claim.scope = "STORY-1".into();
        claim.heartbeat_at = (now - chrono::Duration::seconds(61)).to_rfc3339();
        write(&claim);
        assert!(deferred_with_metadata(root, metadata)
            .unwrap()
            .unwrap()
            .starts_with(
                "deferred: implementer lane state unknown (remote-fixture-host, lease-two)"
            ));
        claim.heartbeat_at = (now
            - chrono::Duration::seconds(61)
            - chrono::Duration::minutes(DEFAULT_UNKNOWN_DEFER_MINS))
        .to_rfc3339();
        write(&claim);
        assert!(deferred_with_metadata(root, metadata).unwrap().is_none());
        // Long lapsed: an ordinary leftover that costs no forge lookup.
        claim.heartbeat_at = (now - chrono::Duration::hours(3)).to_rfc3339();
        write(&claim);
        assert!(
            deferred_with_metadata(root, || panic!("old leftover claim must not defer"))
                .unwrap()
                .is_none()
        );
    }

    fn remote_claim(now: DateTime<Utc>) -> crate::coordination::Claim {
        serde_json::from_value(serde_json::json!({
            "scope":"STORY-1", "node_id":"node-two", "claim_id":"lease-two", "clone_path":"/remote",
            "host":"lane-host-fixture", "pid":1, "agent":"implementer",
            "started_at":now.to_rfc3339(), "heartbeat_at":now.to_rfc3339(), "ttl_secs":1800
        }))
        .unwrap()
    }

    #[test]
    fn story2705_remote_claim_expires_and_invalid_heartbeat_never_blocks() {
        let now = Utc::now();
        let mut claim = remote_claim(now);
        let standing = |claim: &crate::coordination::Claim, at| {
            remote_standing(claim, at, "host-one", DEFAULT_UNKNOWN_DEFER_MINS)
        };
        assert_eq!(standing(&claim, now), Some(Standing::Live));
        assert_eq!(
            standing(&claim, now + Duration::seconds(1800)),
            Some(Standing::Live)
        );
        // Past the TTL the lane is no longer known to be live, and past the
        // bound it no longer defers at all.
        assert!(matches!(
            standing(&claim, now + Duration::seconds(1801)),
            Some(Standing::Unknown { silent_mins: 30, until }) if until == now + Duration::minutes(50)
        ));
        assert_eq!(
            standing(&claim, now + Duration::minutes(50)),
            Some(Standing::Lapsed { silent_mins: 50 })
        );
        assert_eq!(
            standing(&claim, now + Duration::minutes(81)),
            Some(Standing::Lapsed { silent_mins: 81 })
        );
        // Another host's clock may run a little ahead; the far future and an
        // unreadable or hostless record are no evidence.
        assert_eq!(
            standing(&claim, now - Duration::seconds(CLOCK_SKEW_SECS)),
            Some(Standing::Live)
        );
        assert_eq!(
            standing(&claim, now - Duration::seconds(CLOCK_SKEW_SECS + 1)),
            None
        );
        claim.heartbeat_at = "invalid".into();
        assert_eq!(standing(&claim, now), None);
        claim = remote_claim(now);
        claim.host.clear();
        assert_eq!(standing(&claim, now), None);
        claim = remote_claim(now);
        claim.ttl_secs = u64::MAX;
        claim.heartbeat_at = (now - Duration::days(400_000)).to_rfc3339();
        let _ = standing(&claim, now);
    }

    /// A project root whose shared store carries `claim`, as this host sees
    /// a lane that runs on another one.
    fn observer(claim: Option<&crate::coordination::Claim>) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".aida-store/coordination/leases");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(claim) = claim {
            let path =
                crate::coordination::claim_path(&temp.path().join(".aida-store"), &claim.scope);
            std::fs::write(path, toml::to_string(claim).unwrap()).unwrap();
        }
        temp
    }

    fn metadata() -> Result<(String, String)> {
        Ok(("fix: rework (STORY-1)".into(), "story-1-rework".into()))
    }

    // AC1 + AC2: a lane on another host defers the head for as long as its
    // launcher keeps the claim's heartbeat fresh, however long it has run and
    // whatever the head is; once it ends, the head is admitted.
    // trace:BUG-2706 | ai:claude
    #[test]
    fn bug2706_foreign_lane_defers_past_claim_ttl_until_it_exits() {
        let now = Utc::now();
        let mut claim = remote_claim(now - Duration::minutes(45));
        // The reported failure: 45 minutes in, heartbeat never refreshed.
        // The merge-only head is still held, as state unknown.
        let root = observer(Some(&claim));
        let held = decide(root.path(), "watch-host", now, 20, metadata).unwrap();
        assert!(
            held.reason.as_deref().is_some_and(|r| r.starts_with(
                "deferred: implementer lane state unknown (lane-host-fixture, lease-two): no heartbeat for 45 min"
            )),
            "{held:?}"
        );
        // With the launcher's heartbeat the same lane is simply live, at 45
        // minutes and at four hours.
        for (age, beat) in [(45, 1), (240, 9)] {
            claim.started_at = (now - Duration::minutes(age)).to_rfc3339();
            claim.heartbeat_at = (now - Duration::minutes(beat)).to_rfc3339();
            let root = observer(Some(&claim));
            assert_eq!(
                decide(root.path(), "watch-host", now, 20, metadata).unwrap(),
                Decision {
                    reason: Some(
                        "deferred: implementer lane live (lane-host-fixture, lease-two)".into()
                    ),
                    ..Decision::default()
                }
            );
            // A change for another spec is not held by this lane.
            let other = || Ok(("fix: other (STORY-2)".into(), "story-2".into()));
            assert_eq!(
                decide(root.path(), "watch-host", now, 20, other).unwrap(),
                Decision::default()
            );
        }
        // The lane exits and releases its claim: nothing defers, and no
        // forge lookup is needed to know it.
        let root = observer(None);
        assert_eq!(
            decide(root.path(), "watch-host", now, 20, || panic!(
                "no lane, no lookup"
            ))
            .unwrap(),
            Decision::default()
        );
    }

    #[test]
    fn bug2706_unreadable_interval_survives_restart_and_recovers() {
        let root = observer(None);
        let now = Utc::now();
        let path = crate::coordination::claim_path(&root.path().join(".aida-store"), "broken");
        let pass = |mins| {
            let at = now + Duration::minutes(mins);
            let decision = decide(root.path(), "observer", at, 20, metadata).unwrap();
            settle_episode(root.path(), "PR-1", decision, at, 20).unwrap()
        };
        for start in [0, 100] {
            std::fs::write(&path, "[").unwrap();
            assert!(pass(start).is_some());
            assert!(pass(start + 19).is_some());
            assert!(pass(start + 60).is_none());
            assert!(pass(start + 61).is_none());
            std::fs::remove_file(&path).unwrap();
            assert!(pass(start + 62).is_none());
        }
    }

    // AC3: an unreadable lane defers for the configured bound and no longer,
    // and the lapse says why the head is reviewed.
    // trace:BUG-2706 | ai:claude
    #[test]
    fn bug2706_unreadable_foreign_lane_defers_for_the_bound_then_reviews_with_a_reason() {
        let now = Utc::now();
        // Heartbeat 45 minutes old on a 30-minute TTL: 15 minutes unreadable.
        let claim = remote_claim(now - Duration::minutes(45));
        let root = observer(Some(&claim));
        let at = |grace| decide(root.path(), "watch-host", now, grace, metadata).unwrap();
        assert!(at(20).reason.is_some());
        assert!(at(16).reason.is_some());
        for grace in [15, 5, 0] {
            let lapsed = at(grace);
            assert_eq!(lapsed.reason, None);
            assert_eq!(lapsed.lapsed.len(), 1, "{lapsed:?}");
            assert_eq!(lapsed.lapsed[0].0, "lane-host-fixture/lease-two");
            assert_eq!(
                lapsed.lapsed[0].1,
                format!("implementer lane state on lane-host-fixture (lease-two) could not be read: no heartbeat for 45 min, past the {grace}-minute bound; reviewing")
            );
        }
        // A live lane on the same spec outranks an unreadable one.
        let mut live = remote_claim(now);
        live.scope = "story-1".into();
        live.claim_id = Some("lease-live".into());
        std::fs::write(
            crate::coordination::claim_path(&root.path().join(".aida-store"), "story-1"),
            toml::to_string(&live).unwrap(),
        )
        .unwrap();
        assert_eq!(
            at(0).reason.as_deref(),
            Some("deferred: implementer lane live (lane-host-fixture, lease-live)")
        );
    }

    // trace:BUG-2706 | ai:claude
    #[test]
    fn bug2706_unknown_defer_setting_defaults_to_twenty_minutes_and_bounds_the_wait() {
        let now = Utc::now();
        // 45 minutes silent: inside a 20-minute bound, outside a 10-minute one.
        let root = observer(Some(&remote_claim(now - Duration::minutes(45))));
        let config = root.path().join(".aida/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        assert_eq!(unknown_defer_mins(root.path()), 20);
        assert!(deferred_with_metadata(root.path(), metadata)
            .unwrap()
            .is_some());
        for (text, mins) in [
            ("[review]\nlane_unknown_defer_mins = 10\n", 10),
            ("[review]\nlane_unknown_defer_mins = 0\n", 0),
            ("[review]\nlane_unknown_defer_mins = 1440\n", 1440),
            // Out of range, mistyped or unparseable keeps the default.
            ("[review]\nlane_unknown_defer_mins = 1441\n", 20),
            ("[review]\nlane_unknown_defer_mins = -1\n", 20),
            ("[review]\nlane_unknown_defer_mins = \"soon\"\n", 20),
            ("[review\n", 20),
            ("[drain]\nlane_unknown_defer_mins = 1\n", 20),
        ] {
            std::fs::write(&config, text).unwrap();
            assert_eq!(unknown_defer_mins(root.path()), mins, "{text}");
        }
        std::fs::write(&config, "[review]\nlane_unknown_defer_mins = 10\n").unwrap();
        assert!(deferred_with_metadata(root.path(), metadata)
            .unwrap()
            .is_none());
    }
}
