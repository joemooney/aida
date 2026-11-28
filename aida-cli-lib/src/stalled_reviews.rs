//! Report stalled review heads using ledger and CI timestamps.
// trace:STORY-3095 | ai:codex
use crate::doctor_root::DoctorFinding;
use crate::gate::ledger::{project, ChangeKey};
use crate::gate::review_verdict::{parse_ledger_record, VerdictKind};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::HashSet;
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
    if !overdue || !reviewed.is_some_and(|sha| sha.eq_ignore_ascii_case(head)) {
        return None;
    }
    match kind {
        VerdictKind::RequestChanges if !reworking => Some("request-changes without rework"),
        VerdictKind::Approved if green && !on_train => Some("approved but not landing"),
        _ => None,
    }
}

pub(crate) fn scan(exec: &aida_core::exec_context::ExecContext) -> Result<Vec<DoctorFinding>> {
    let minutes = exec
        .environment()
        .get("AIDA_STALLED_REVIEW_MINUTES")
        .map(|s| -> Result<i64> { Ok(s.to_str().context("non-Unicode stall threshold")?.parse()?) })
        .transpose()?
        .unwrap_or(60);
    let threshold = TimeDelta::try_minutes(minutes)
        .filter(|t| *t > TimeDelta::zero())
        .context("AIDA_STALLED_REVIEW_MINUTES must be a positive duration")?;
    scan_open(exec, threshold, open_prs)
}

/// Rework lanes: queued non-reviewer work and retained implementer leases.
/// Shared with landing-drought's conflict cause. trace:STORY-3131 | ai:claude
pub(crate) struct ReworkLanes {
    queued: HashSet<uuid::Uuid>,
    active: Vec<crate::leases::SessionLease>,
}

impl ReworkLanes {
    pub(crate) fn load(
        root: &Path,
        storage: &aida_core::GitBackend,
        store: &aida_core::RequirementsStore,
    ) -> Result<Self> {
        use aida_core::DatabaseBackend;
        let ownership = crate::ownership::reader::LeaseReader::with_store(root, store);
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
        let active = crate::list_leases(root)
            .into_iter()
            .filter(|l| {
                l.role
                    .as_deref()
                    .is_some_and(|r| r.eq_ignore_ascii_case("implementer"))
            })
            .filter(|lease| ownership.retained(lease))
            .collect();
        Ok(Self { queued, active })
    }

    pub(crate) fn covers(
        &self,
        store: &aida_core::RequirementsStore,
        spec: &str,
        branch: &str,
    ) -> bool {
        store
            .get_requirement_by_spec_id(spec)
            .is_some_and(|r| self.queued.contains(&r.id))
            || self
                .active
                .iter()
                .any(|l| aida_core::id_identity::same_id(&l.scope, spec) || l.branch == branch)
    }
}

pub(crate) fn open_prs(root: &Path) -> Result<Vec<serde_json::Value>> {
    let repo = crate::review_watch::forge::Repository::load(root)?;
    let slug = repo
        .github_slug()
        .context("stalled-reviews requires a GitHub repository")?;
    if let Some((report, true)) = crate::forge::github_core_budget_uncached(root) {
        anyhow::bail!("{report} Skipping stalled-review scan");
    }
    crate::forge::github_open_changes_detailed(root, &slug, &|cmd| repo.pin_command(cmd))
}

fn scan_open(
    exec: &aida_core::exec_context::ExecContext,
    threshold: TimeDelta,
    list: impl FnOnce(&Path) -> Result<Vec<serde_json::Value>>,
) -> Result<Vec<DoctorFinding>> {
    let root = exec.project_root();
    let prs = list(root)?;
    ensure!(
        prs.len() < 1000,
        "open PR listing reached the 1000-row limit"
    );
    use aida_core::DatabaseBackend;
    let storage = aida_core::GitBackend::open_existing(&root.join(".aida-store"))?;
    let store = storage.load_for_read()?;
    let lanes = ReworkLanes::load(root, &storage, &store)?;
    let now = DateTime::<Utc>::from(exec.clock().wall_now());
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
        let green_at = approved
            .then(|| crate::land::green_rollup_at(&pr["statusCheckRollup"]))
            .flatten();
        let green = green_at.is_some();
        let since = green_at.map_or(at, |ci| at.max(ci));
        let on_train = green && !crate::pr_train::pr_head_pins(root, number)?.is_empty();
        for spec in specs {
            let reworking = lanes.covers(&store, &spec, branch);
            let Some(reason) = stall(
                verdict.kind,
                verdict.reviewed_sha.as_deref(),
                head,
                reworking,
                green,
                on_train,
                now.signed_duration_since(since) > threshold,
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
            findings.push(DoctorFinding {
                category: "stalled-reviews".into(),
                id: format!("PR-{number}/{spec}"),
                summary,
                action: "Inspect the review and landing status".into(),
                safe_heal: false,
            });
        }
    }
    Ok(findings)
}

#[cfg(test)]
#[path = "tests/stalled_reviews_tests.rs"]
mod tests;
