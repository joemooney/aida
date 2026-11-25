//! Report stalled review heads using ledger and CI timestamps.
// trace:STORY-3095 | ai:codex
use crate::doctor_root::DoctorFinding;
use crate::gate::ledger::{project, ChangeKey};
use crate::gate::review_verdict::{parse_ledger_record, VerdictKind};
use crate::ownership::{dual::spec_key_in_store, reader::LegacyRule, OwnerVerdict};
use crate::pr_ship::{extract_authorship_spec_ids_from_subject, extract_branch_spec_ids};
use crate::ship_from_seat::advisor_request_on_thread as notify_advisor;
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
    let now = exec.clock().wall_now().into();
    let root = exec.project_root();
    scan_rows(root, now, threshold, open_prs(root)?, false)
}

fn open_prs(root: &Path) -> Result<Vec<serde_json::Value>> {
    let repo = crate::review_watch::forge::Repository::load(root)?;
    let slug = repo
        .github_slug()
        .context("stalled-reviews requires a GitHub repository")?;
    if let Some((report, true)) = crate::forge::github_core_budget_uncached(root) {
        anyhow::bail!("{report} Skipping stalled-review scan");
    }
    crate::forge::github_open_changes_detailed(root, &slug, &|cmd| repo.pin_command(cmd))
}

// trace:TASK-3878 | ai:codex
pub(crate) fn queue_repairs(root: &Path) -> Result<()> {
    let prs = open_prs(root)?;
    let directory = crate::repair_journal::RepairGuard::directory(root)?;
    fs2::FileExt::try_lock_exclusive(&directory)
        .context("repair journal or conflict sweep active")?;
    for finding in scan_rows(root, Utc::now(), TimeDelta::zero(), prs, true)? {
        println!("{}", finding.summary);
    }
    Ok(())
}

fn scan_rows(
    root: &Path,
    now: DateTime<Utc>,
    threshold: TimeDelta,
    prs: Vec<serde_json::Value>,
    repair: bool,
) -> Result<Vec<DoctorFinding>> {
    ensure!(
        prs.len() < 1000,
        "open PR listing reached the 1000-row limit"
    );
    use aida_core::DatabaseBackend;
    let storage = aida_core::GitBackend::open_existing(&root.join(".aida-store"))?;
    let store = storage.load_for_read()?;
    let ownership = crate::ownership::reader::LeaseReader::with_store(root, &store);
    let mut queued = HashSet::new();
    let mut queue_notes = HashSet::new();
    for user in storage.queue_users()? {
        for entry in storage.queue_list(&user, false)? {
            if entry
                .for_role
                .as_deref()
                .is_none_or(|r| !r.eq_ignore_ascii_case("reviewer"))
            {
                if user == "role:implementer" {
                    queue_notes.insert((entry.requirement_id, entry.note));
                }
                queued.insert(entry.requirement_id);
            }
        }
    }
    let reservations = crate::repair_journal::RepairGuard::retained_prs(root)?;
    let leases = crate::leases::list_leases_strict_all(root).map_err(anyhow::Error::msg)?;
    let implementer = |lease: &crate::SessionLease| {
        lease
            .role
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case("implementer"))
    };
    let active: Vec<_> = leases
        .iter()
        .filter(|l| implementer(l))
        .filter(|lease| ownership.retained(lease))
        .collect();
    let mut findings = Vec::new();
    let mut conflicts = Vec::new();
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
        let branch = pr["headRefName"].as_str().context("PR branch missing")?;
        let title = pr["title"].as_str().context("PR title missing")?;
        let conflicting = pr["mergeable"] == "CONFLICTING";
        let mut specs = if conflicting {
            let mut ids = extract_branch_spec_ids(branch);
            ids.extend(extract_authorship_spec_ids_from_subject(title));
            ids
        } else {
            crate::pr_ship::extract_pr_spec_ids(branch, title)
        };
        let mut seen = HashSet::new();
        specs.retain(|s| {
            store
                .get_requirement_by_spec_id(s)
                .is_some_and(|r| seen.insert(r.id))
        });
        if specs.is_empty() {
            specs.push("unknown spec".into());
        }
        // Conflict evidence is independent of review age and verdict availability.
        // trace:TASK-3878 | ai:codex
        if conflicting {
            let base = pr["baseRefOid"]
                .as_str()
                .filter(|v| crate::forge::is_full_commit_sha(v))
                .context("invalid conflicting PR base tip")?;
            let marker = format!("conflict-repair-{number}-{head}-{base}");
            let matches_spec =
                |id: &str| spec_key_in_store(&store, id).is_some_and(|key| seen.contains(&key.0));
            let mut blocked = reservations.iter().any(|(n, b)| {
                *n == number
                    || b == branch
                    || extract_branch_spec_ids(b).iter().any(|id| matches_spec(id))
            });
            let mut running = false;
            for lease in &leases {
                if lease.branch == branch || matches_spec(&lease.scope) {
                    match ownership.pickup_owner(lease, LegacyRule::Adoption) {
                        OwnerVerdict::Alive { .. } => running |= implementer(lease),
                        _ => blocked = true,
                    }
                }
            }
            let queued_here = !seen.is_disjoint(&queued);
            let verdict = latest.as_ref().and_then(|(_, r)| parse_ledger_record(r));
            let approved = pr["reviewDecision"] == "APPROVED"
                || verdict.is_some_and(|v| {
                    !v.is_closed()
                        && v.kind == VerdictKind::Approved
                        && v.reviewed_sha.as_deref() == Some(head)
                });
            let spec = specs.join(", ");
            let reason = format!(
                "{marker}: merge the base into PR-{number}, resolve conflicts, no scope change"
            );
            let requested_at = specs
                .iter()
                .filter_map(|s| store.get_requirement_by_spec_id(s))
                .flat_map(|r| &r.comments)
                .filter(|c| c.content == reason)
                .map(|c| c.created_at)
                .min();
            let age = requested_at.map_or_else(
                || "unknown".into(),
                |at| format!("at least {}m", (now - at).num_minutes().max(0)),
            );
            if blocked || (!running && !queued_here) {
                let (status, action) = if blocked {
                    (
                        "blocked: reservation repair needed",
                        "Inspect repair reservations",
                    )
                } else {
                    ("conflicting", "aida doctor heal stalled-reviews")
                };
                conflicts.push((
                    !approved,
                    DoctorFinding {
                        category: "stalled-reviews".into(),
                        id: format!("PR-{number}/{spec}"),
                        summary: format!("PR-{number} {spec} {head}: {status}, conflict age {age}"),
                        action: action.into(),
                        safe_heal: false,
                    },
                ));
            }
            if !repair || blocked {
                continue;
            }
            let target_spec = &specs[0];
            if let Some(target) = store.get_requirement_by_spec_id(target_spec) {
                let ours = queue_notes.contains(&(target.id, Some(reason.clone())));
                if ours || (!running && !queued_here) {
                    let delivered = crate::repair_journal::mailbox_has_delivery(root, &marker)?;
                    if should_queue(ours, delivered, requested_at.is_some())? {
                        queue_rework(root, target_spec, &reason)?;
                        let rows = storage.queue_list("role:implementer", false)?;
                        let accepted = rows.iter().any(|r| {
                            r.requirement_id == target.id && r.note.as_ref() == Some(&reason)
                        });
                        ensure!(accepted, "missing queue receipt for {target_spec}");
                        queued.insert(target.id);
                    }
                    if !delivered {
                        let subject = "Conflict repair queued";
                        notify_advisor(root, subject, &reason, Some(&marker))?;
                    }
                }
            }
            continue;
        }
        if repair {
            continue;
        }
        let Some((at, record)) = latest else { continue };
        let verdict = parse_ledger_record(&record).context("invalid ledger verdict")?;
        if verdict.is_closed() {
            continue;
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
    conflicts.sort_by_key(|(rank, _)| *rank);
    let ranked = conflicts.into_iter().map(|(_, f)| f);
    Ok(ranked.chain(findings).collect())
}

fn queue_rework(root: &Path, spec: &str, reason: &str) -> Result<()> {
    let route = ["queue", "rework", spec, "--for", "implementer"];
    let mut cmd = crate::launch::control::control_child()?;
    cmd.current_dir(root).args(route).args(["--reason", reason]);
    let status = cmd.status()?;
    ensure!(status.success(), "conflict-repair rework failed for {spec}");
    Ok(())
}

fn should_queue(queued: bool, delivered: bool, reason_recorded: bool) -> Result<bool> {
    if queued || delivered {
        return Ok(false);
    }
    ensure!(
        !reason_recorded,
        "prior rework has no receipt; inspect before retrying"
    );
    Ok(true)
}

#[cfg(test)]
#[path = "tests/stalled_reviews_tests.rs"]
mod tests;
