//! Report stalled review heads and wake the advisor once per observed episode.
// trace:STORY-3095 | ai:codex
use crate::doctor_root::DoctorFinding;
use crate::gate::ledger::{project, ChangeKey};
use crate::gate::review_verdict::{parse_ledger_record, VerdictKind};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

fn stall(
    kind: VerdictKind,
    reviewed: Option<&str>,
    head: &str,
    reworking: bool,
    green: bool,
    on_train: bool,
    overdue: bool,
) -> Option<&'static str> {
    if !reviewed.is_some_and(|sha| sha.eq_ignore_ascii_case(head)) {
        return None;
    }
    match kind {
        VerdictKind::RequestChanges if !reworking => Some("request-changes without rework"),
        VerdictKind::Approved if green && !on_train && overdue => Some("approved but not landing"),
        _ => None,
    }
}

pub(crate) fn scan(
    root: &Path,
    store: &aida_core::RequirementsStore,
) -> Result<Vec<DoctorFinding>> {
    let minutes = std::env::var("AIDA_STALLED_REVIEW_MINUTES")
        .ok()
        .map(|s| s.parse::<i64>())
        .transpose()?
        .unwrap_or(60);
    let threshold = TimeDelta::try_minutes(minutes)
        .filter(|t| *t > TimeDelta::zero())
        .context("AIDA_STALLED_REVIEW_MINUTES must be a positive duration")?;
    let dir = root.join(".aida");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("stalled-reviews.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("another stalled-review scan is running")?;
    let path = dir.join("stalled-reviews.json");
    let previous: BTreeMap<String, DateTime<Utc>> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e.into()),
    };
    let repo = crate::review_watch::forge::Repository::load(root)?;
    let slug = repo
        .github_slug()
        .context("stalled-reviews requires a GitHub repository")?;
    if let Some((report, true)) = crate::forge::github_core_budget(root) {
        anyhow::bail!("{report} Skipping stalled-review scan");
    }
    let prs =
        crate::forge::github_open_changes_detailed(root, &slug, &|cmd| repo.pin_command(cmd))?;
    ensure!(
        prs.len() < 1000,
        "open PR listing reached the 1000-row limit"
    );
    let storage = crate::Storage::new(root.join(".aida-store"));
    let mut queued = HashSet::new();
    for user in storage.queue_users()? {
        for entry in storage.queue_list(&user, false)? {
            if entry
                .for_role
                .as_deref()
                .is_none_or(|r| !r.eq_ignore_ascii_case("reviewer"))
            {
                queued.insert(entry.requirement_id);
            }
        }
    }
    let now = Utc::now();
    let ownership = crate::ownership::reader::LeaseReader::new(root);
    let active: Vec<_> = crate::list_leases(root)
        .into_iter()
        .filter(|l| {
            l.role
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case("implementer"))
        })
        .filter(|lease| ownership.retained(lease))
        .collect();
    let mut episodes = BTreeMap::new();
    let mut findings = Vec::new();
    for pr in prs {
        let number = pr["number"].as_u64().context("PR number missing")?;
        let head = pr["headRefOid"]
            .as_str()
            .filter(|h| crate::forge::is_full_commit_sha(h))
            .context("invalid PR head")?;
        let records = project::change_verdicts_checked(root, ChangeKey::pr(number))
            .map_err(anyhow::Error::msg)?;
        let latest = records
            .into_iter()
            .map(|(at, record)| Ok((DateTime::parse_from_rfc3339(&at)?, record)))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max_by_key(|(at, _)| *at);
        let Some((at, record)) = latest else { continue };
        let verdict = parse_ledger_record(&record).context("invalid ledger verdict")?;
        if verdict.is_closed() {
            continue;
        }
        let branch = pr["headRefName"].as_str().context("PR branch missing")?;
        let title = pr["title"].as_str().context("PR title missing")?;
        let mut specs = crate::pr_ship::extract_pr_spec_ids(branch, title);
        specs.retain(|s| store.get_requirement_by_spec_id(s).is_some());
        if specs.is_empty() {
            specs.push("unknown spec".into());
        }
        let age = now.signed_duration_since(at);
        let approved = verdict.kind == VerdictKind::Approved;
        let green = approved
            && matches!(
                crate::land::ci_of_rollup(&pr["statusCheckRollup"]),
                crate::land::Ci::Green
            );
        let on_train = green && !crate::pr_train::pr_head_pins(root, number)?.is_empty();
        for spec in specs {
            let reworking = store
                .get_requirement_by_spec_id(&spec)
                .is_some_and(|r| queued.contains(&r.id))
                || active.iter().any(|l| {
                    aida_core::id_identity::same_id(&l.scope, &spec) || l.branch == branch
                });
            let Some(reason) = stall(
                verdict.kind,
                verdict.reviewed_sha.as_deref(),
                head,
                reworking,
                green,
                on_train,
                age > threshold,
            ) else {
                continue;
            };
            let blocker = if approved && pr["isDraft"] == true {
                " (draft)"
            } else if approved
                && pr["labels"].as_array().is_some_and(|ls| {
                    ls.iter()
                        .any(|l| l["name"] == crate::merge_hold::HOLD_LABEL)
                })
            {
                " (merge hold)"
            } else {
                ""
            };
            let summary = format!(
                "PR-{number} {spec}: {reason}{blocker}, verdict age {}m, {} findings",
                age.num_minutes(),
                verdict.findings.len()
            );
            let key = format!(
                "stalled-PR-{number}-{head}-{}-{spec}",
                at.timestamp_millis()
            );
            let since = *previous.get(&key).unwrap_or(&now);
            episodes.insert(key.clone(), since);
            wake_if_overdue(root, &key, reason, &summary, since, now, threshold)?;
            findings.push(DoctorFinding {
                category: "stalled-reviews".into(),
                id: format!("PR-{number}/{spec}"),
                summary,
                action: "Inspect the review and landing status".into(),
                safe_heal: false,
            });
        }
    }
    aida_core::fs_atomic::write_atomic(&path, serde_json::to_vec(&episodes)?)?;
    Ok(findings)
}

fn wake_if_overdue(
    root: &Path,
    key: &str,
    reason: &str,
    summary: &str,
    since: DateTime<Utc>,
    now: DateTime<Utc>,
    threshold: TimeDelta,
) -> Result<()> {
    if now - since > threshold {
        crate::main_watch::Live::advisor_mail(
            root,
            "stalled-reviews",
            &format!("{key}-{}", since.timestamp_millis()),
            reason,
            summary,
        )?;
    }
    Ok(())
}

#[test]
fn story_3095_stalled_review_cases() {
    use VerdictKind::{Approved, RequestChanges};
    let classify = |kind, head, queued, green, train, late| {
        stall(kind, Some("abc0123456"), head, queued, green, train, late)
    };
    assert!(classify(RequestChanges, "abc0123456", false, false, false, false).is_some());
    assert!(classify(RequestChanges, "abc0123456", true, false, false, true).is_none());
    assert!(classify(RequestChanges, "abc0123456ff", false, false, false, true).is_none());
    assert!(classify(RequestChanges, "def0123456", false, false, false, true).is_none());
    assert!(classify(Approved, "abc0123456", false, true, false, true).is_some());
    assert!(classify(Approved, "abc0123456", false, true, true, true).is_none());
    assert!(classify(Approved, "abc0123456", false, false, false, true).is_none());
    assert!(classify(Approved, "abc0123456", false, true, false, false).is_none());
    let root = tempfile::tempdir().unwrap();
    let since = Utc::now();
    let threshold = TimeDelta::minutes(60);
    for (minutes, expected) in [(60, 0), (61, 1), (62, 1)] {
        wake_if_overdue(
            root.path(),
            "pr-1",
            "stalled review",
            "PR-1",
            since,
            since + TimeDelta::minutes(minutes),
            threshold,
        )
        .unwrap();
        let mail = aida_core::mailbox::read_local_messages(root.path()).unwrap();
        assert_eq!(mail.len(), expected);
        assert!(mail
            .iter()
            .all(|m| m.urgent && m.to == aida_core::mailbox::Recipient::Agent("advisor".into())));
    }
}
