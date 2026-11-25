//! Report stalled review heads using ledger and CI timestamps.
// trace:STORY-3095 | ai:codex
use crate::doctor_root::DoctorFinding;
use crate::gate::ledger::{project, ChangeKey};
use crate::gate::review_verdict::{parse_ledger_record, VerdictKind};
use crate::pr_ship::{extract_authorship_spec_ids_from_subject, extract_branch_spec_ids};
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
    let prs = open_prs(root)?;
    let tips = if prs.iter().any(|pr| pr["mergeable"] == "CONFLICTING") {
        use aida_core::git_ops::RemoteCredentialsExt as _;
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(root)
            .args(["ls-remote", "--heads", "origin"])
            .env("GIT_TERMINAL_PROMPT", "0");
        crate::review_watch::forge::clear_routing(&mut cmd);
        cmd.remote_credentials(root, "origin")
            .stdin(std::process::Stdio::null());
        crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(30))
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .unwrap_or_default()
    } else {
        String::new()
    };
    scan_rows(root, now, threshold, prs, &tips)
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
fn scan_rows(
    root: &Path,
    now: DateTime<Utc>,
    threshold: TimeDelta,
    prs: Vec<serde_json::Value>,
    tips: &str,
) -> Result<Vec<DoctorFinding>> {
    ensure!(
        prs.len() < 1000,
        "open PR listing reached the 1000-row limit"
    );
    use aida_core::DatabaseBackend;
    let storage = aida_core::GitBackend::open_existing(&root.join(".aida-store"))?;
    let store = storage.load_for_read()?;
    let ownership = crate::ownership::reader::LeaseReader::with_store(root, &store);
    let ordinary = std::cell::LazyCell::new(|| -> Result<_> {
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
        let active: Vec<_> = crate::list_leases(root)
            .into_iter()
            .filter(|l| {
                l.role
                    .as_deref()
                    .is_some_and(|r| r.eq_ignore_ascii_case("implementer"))
            })
            .filter(|l| ownership.retained(l))
            .collect();
        Ok((queued, active))
    });
    let mut findings = Vec::new();
    let mut conflicts = Vec::new();
    for pr in prs {
        let mut row = || -> Result<()> {
            let number = pr["number"].as_u64().context("PR number missing")?;
            let head = pr["headRefOid"]
                .as_str()
                .filter(|h| crate::forge::is_full_commit_sha(h))
                .context("invalid PR head")?;
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
                let name = pr["baseRefName"].as_str().unwrap_or("?");
                let base =
                    crate::pr_rebase::parse_ls_remote_tip(tips, &format!("refs/heads/{name}"))
                        .filter(|sha| crate::forge::is_full_commit_sha(sha))
                        .unwrap_or_else(|| "unknown".into());
                let spec = specs.join(", ");
                let reason =
                    format!("Merge the base into PR-{number}, resolve conflicts, no scope change");
                // Earlier repairs recorded this exact (PR, head, base) observation.
                let observation = format!("conflict-repair-{number}-{head}-{base}: merge the base into PR-{number}, resolve conflicts, no scope change");
                let age = specs
                    .iter()
                    .filter_map(|s| store.get_requirement_by_spec_id(s))
                    .flat_map(|r| &r.comments)
                    .filter(|c| c.content == observation && c.created_at <= now)
                    .map(|c| c.created_at)
                    .min()
                    .map_or_else(
                        || "unknown".into(),
                        |at| format!("at least {}m", (now - at).num_minutes()),
                    );
                let action = if seen.is_empty() {
                    "Identify the backing spec before queuing rework".into()
                } else {
                    specs
                        .iter()
                        .map(|s| {
                            format!("aida queue rework {s} --for implementer --reason '{reason}'")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                let finding = DoctorFinding {
                    category: "stalled-reviews".into(),
                    id: format!("PR-{number}/{spec}"),
                    summary: format!("PR-{number} {spec} {head}: conflicting with {name} {base}, conflict age {age}, owner unknown"),
                    action,
                    safe_heal: false,
                };
                conflicts.push((pr["reviewDecision"] != "APPROVED", finding));
                return Ok(());
            }
            let records = project::change_verdicts_checked(root, ChangeKey::pr(number))
                .map_err(anyhow::Error::msg)?;
            let latest = records
                .into_iter()
                .map(|(at, record)| Ok((DateTime::parse_from_rfc3339(&at)?, record)))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .max_by_key(|(at, _)| *at);
            let Some((at, record)) = latest else {
                return Ok(());
            };
            let verdict = parse_ledger_record(&record).context("invalid ledger verdict")?;
            if verdict.is_closed() {
                return Ok(());
            }
            let age = now.signed_duration_since(at);
            let approved = verdict.kind == VerdictKind::Approved;
            let green_at = approved
                .then(|| crate::land::green_rollup_at(&pr["statusCheckRollup"]))
                .flatten();
            let green = green_at.is_some();
            let since = green_at.map_or(at, |ci| at.max(ci));
            let on_train = green && !crate::pr_train::pr_head_pins(root, number)?.is_empty();
            let (queued, active) = ordinary.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
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
            Ok(())
        };
        if let Err(err) = row() {
            findings.push(DoctorFinding {
                category: "stalled-reviews".into(),
                id: format!("PR-{}", pr["number"]),
                summary: format!("PR-{}: review evidence unavailable: {err}", pr["number"]),
                action: "Inspect the PR evidence".into(),
                safe_heal: false,
            });
        }
    }
    conflicts.sort_by_key(|(rank, _)| *rank);
    let ranked = conflicts.into_iter().map(|(_, f)| f);
    Ok(ranked.chain(findings).collect())
}

#[cfg(test)]
#[path = "tests/stalled_reviews_tests.rs"]
mod tests;
