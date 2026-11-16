//! Vendor-neutral, read-only landing health. Scheduling/routing stays in the
//! substrate; the evaluator consumes owned evidence and never launches a model.
// trace:STORY-2849 | ai:codex
use crate::{forge::ReviewDecision, DoctorFinding};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) const CATEGORY: &str = "landing-health";

#[derive(Clone, Debug)]
pub struct Step {
    pub check: String,
    pub name: String,
    pub workflow: String,
    pub completed: DateTime<Utc>,
    pub seconds: Option<f64>,
    pub failed: bool,
    pub successful: bool,
    pub required: bool,
    pub error: String,
}

#[derive(Debug)]
struct Policy {
    enabled: bool,
    gap_minutes: i64,
    min_prs: usize,
    ratio: f64,
    window_minutes: i64,
}
impl Policy {
    fn read(value: Option<&toml::Value>) -> Result<Self> {
        let cfg = value.and_then(|v| v.get("landing_health"));
        let integer = |key: &str, default: i64| -> Result<i64> {
            let v = cfg.and_then(|v| v.get(key));
            let n = match v {
                None => default,
                Some(v) => v
                    .as_integer()
                    .ok_or_else(|| anyhow::anyhow!("landing_health.{key} must be an integer"))?,
            };
            anyhow::ensure!(n > 0, "landing_health.{key} must be positive");
            Ok(n)
        };
        let ratio = match cfg.and_then(|v| v.get("step_headroom_ratio")) {
            None => 0.8,
            Some(v) => v
                .as_float()
                .or_else(|| v.as_integer().map(|n| n as f64))
                .ok_or_else(|| anyhow::anyhow!("step_headroom_ratio must be numeric"))?,
        };
        anyhow::ensure!(
            ratio > 0.0 && ratio <= 1.0,
            "step_headroom_ratio must be in (0, 1]"
        );
        let enabled = match cfg.and_then(|v| v.get("enabled")) {
            None => true,
            Some(v) => v
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("landing_health.enabled must be boolean"))?,
        };
        Ok(Self {
            enabled,
            gap_minutes: integer("landing_gap_minutes", 90)?,
            min_prs: integer("shared_failure_min_prs", 3)? as usize,
            ratio,
            window_minutes: integer("window_minutes", 1440)?,
        })
    }
}

struct Change {
    id: u64,
    reason: String,
    steps: Vec<Step>,
    timeouts: BTreeMap<(String, String, String), f64>,
}

fn finding(id: &str, summary: String) -> DoctorFinding {
    DoctorFinding { category: CATEGORY.into(), id: format!("{CATEGORY}:{id}"), summary,
        action: "diagnose the shared CI cause first; re-run `aida doctor check landing-health` after repair".into(), safe_heal: false }
}

fn evaluate(
    changes: &[Change],
    tip: DateTime<Utc>,
    now: DateTime<Utc>,
    policy: &Policy,
) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();
    let mut failures: BTreeMap<(&str, &str), (BTreeSet<u64>, &str)> = BTreeMap::new();
    let mut headroom: BTreeMap<(&str, &str, &str), (f64, f64)> = BTreeMap::new();
    for change in changes {
        for step in &change.steps {
            if step.completed > now
                || now - step.completed > Duration::minutes(policy.window_minutes)
            {
                continue;
            }
            if step.failed {
                let entry = failures
                    .entry((&step.check, &step.name))
                    .or_insert_with(|| (BTreeSet::new(), step.error.as_str()));
                entry.0.insert(change.id);
            }
            if step.successful && step.required {
                if let (Some(seconds), Some(timeout)) =
                    (step.seconds, timeout_for(step, &change.timeouts))
                {
                    if seconds > timeout * policy.ratio {
                        let entry = headroom
                            .entry((&step.workflow, &step.check, &step.name))
                            .or_insert((seconds, timeout));
                        if seconds / timeout > entry.0 / entry.1 {
                            *entry = (seconds, timeout);
                        }
                    }
                }
            }
        }
    }
    // Shared cause leads both the evidence and the routed diagnosis prompt.
    for ((check, step), (prs, error)) in failures {
        if prs.len() >= policy.min_prs {
            findings.push(finding(&format!("shared:{check}:{step}"), format!("probable infrastructure or base cause: {} open PRs fail check {check}, step {step}; error: {error}; PRs {prs:?}", prs.len())));
        }
    }
    let age = crate::landing_gap::landing_gap(Some(tip), now).unwrap_or_default();
    if !changes.is_empty() && age > Duration::minutes(policy.gap_minutes) {
        let reasons = changes
            .iter()
            .map(|c| format!("#{}: {}", c.id, c.reason))
            .collect::<Vec<_>>()
            .join("; ");
        findings.push(finding(
            "gap",
            format!(
                "main tip age {} minutes; {} open PRs; {reasons}",
                age.num_minutes(),
                changes.len()
            ),
        ));
    }
    for ((workflow, check, step), (seconds, timeout)) in headroom {
        findings.push(finding(&format!("headroom:{workflow}:{check}:{step}"), format!("required CI step {check} / {step} used {:.0}s of {:.0}s timeout ({:.0}%); workflow {workflow} at base revision", seconds, timeout, 100.0 * seconds / timeout)));
    }
    findings
}

/// Read timeout-minutes from the base's workflow, never the PR or working tree.
/// A step's explicit timeout takes precedence over the enclosing job timeout.
fn workflow_timeouts(body: &str, path: &str) -> Result<BTreeMap<(String, String, String), f64>> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(body)?;
    let mut result = BTreeMap::new();
    if let Some(jobs) = yaml["jobs"].as_mapping() {
        for (id, job) in jobs {
            let check = job["name"]
                .as_str()
                .or_else(|| id.as_str())
                .unwrap_or_default();
            let job_timeout = job["timeout-minutes"].as_f64();
            if let Some(steps) = job["steps"].as_sequence() {
                for step in steps {
                    if let (Some(name), Some(timeout)) = (
                        step["name"].as_str(),
                        step["timeout-minutes"].as_f64().or(job_timeout),
                    ) {
                        if timeout.is_finite() && timeout > 0.0 {
                            result.insert((path.into(), check.into(), name.into()), timeout * 60.0);
                        }
                    }
                }
            }
        }
    }
    Ok(result)
}

// Workflow names can contain matrix expressions; bind their literal parts to
// the observed job/step name. Ambiguous names with different timeouts stay unknown.
fn workflow_name_matches(template: &str, observed: &str) -> bool {
    let Some((prefix, rest)) = template.split_once("${{") else {
        return template == observed;
    };
    let Some((_, suffix)) = rest.split_once("}}") else {
        return false;
    };
    let Some(remaining) = observed.strip_prefix(prefix) else {
        return false;
    };
    if suffix.is_empty() {
        return true;
    }
    remaining
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(remaining.len()))
        .any(|i| workflow_name_matches(suffix, &remaining[i..]))
}

// A whole-expression job name is not a matrix wildcard: only its literal
// alternatives can establish observed coverage or a timeout binding.
// trace:STORY-2849 | ai:codex
pub(crate) fn workflow_job_name_matches(template: &str, observed: &str) -> bool {
    if template.starts_with("${{") && template.ends_with("}}") {
        template.contains(&format!("'{observed}'")) || template.contains(&format!("\"{observed}\""))
    } else {
        workflow_name_matches(template, observed)
    }
}

fn timeout_for(step: &Step, values: &BTreeMap<(String, String, String), f64>) -> Option<f64> {
    let mut matches = values
        .iter()
        .filter(|((path, check, name), _)| {
            path == &step.workflow
                && workflow_job_name_matches(check, &step.check)
                && workflow_name_matches(name, &step.name)
        })
        .map(|(_, seconds)| *seconds);
    let first = matches.next()?;
    matches.all(|seconds| seconds == first).then_some(first)
}

// trace:STORY-2849 | ai:codex
pub(crate) fn required_names(json: &str) -> Result<Vec<String>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json)
        .context("required-check coverage unavailable: malformed JSON")?;
    rows.iter()
        .map(|row| {
            row["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .context("required-check coverage unavailable: missing name")
        })
        .collect()
}

/// Expand branch protection's aggregate jobs through the pinned workflow DAG.
/// Matrix names retain their templates for matching the observed job names.
// trace:STORY-2849 | ai:codex
pub(crate) fn required_workflow_jobs(body: &str, required: &[String]) -> Result<Vec<String>> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(body)?;
    let jobs = yaml["jobs"]
        .as_mapping()
        .context("workflow jobs unavailable")?;
    let name = |id: &serde_yaml::Value, job: &serde_yaml::Value| {
        job["name"]
            .as_str()
            .or_else(|| id.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let mut pending: Vec<String> = jobs
        .iter()
        .filter(|(id, job)| {
            let template = name(id, job);
            required
                .iter()
                .any(|r| workflow_job_name_matches(&template, r))
        })
        .filter_map(|(id, _)| id.as_str().map(str::to_owned))
        .collect();
    let mut seen = BTreeSet::new();
    let mut covered = Vec::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let job = jobs
            .get(&serde_yaml::Value::String(id.clone()))
            .context("required workflow dependency missing")?;
        covered.push(name(&serde_yaml::Value::String(id), job));
        match &job["needs"] {
            serde_yaml::Value::Null => {}
            serde_yaml::Value::String(id) => pending.push(id.clone()),
            serde_yaml::Value::Sequence(ids) => {
                for id in ids {
                    pending.push(
                        id.as_str()
                            .context("invalid required workflow dependency")?
                            .into(),
                    );
                }
            }
            _ => anyhow::bail!("invalid required workflow dependencies"),
        }
    }
    Ok(covered)
}

pub(crate) fn github_steps(
    job: &serde_json::Value,
    path: &str,
    required: bool,
) -> Result<Vec<Step>> {
    let rows = job["steps"]
        .as_array()
        .context("workflow step evidence unavailable")?;
    let mut steps = Vec::new();
    for row in rows {
        let Some(end) = row["completed_at"].as_str() else {
            continue;
        };
        let completed = DateTime::parse_from_rfc3339(end)?.with_timezone(&Utc);
        let start = row["started_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok());
        steps.push(Step {
            check: job["name"]
                .as_str()
                .context("missing CI check name")?
                .into(),
            name: row["name"].as_str().context("missing CI step name")?.into(),
            workflow: path.into(),
            completed,
            seconds: start.map(|s| {
                (completed - s.with_timezone(&Utc))
                    .num_milliseconds()
                    .max(0) as f64
                    / 1000.0
            }),
            failed: matches!(row["conclusion"].as_str(), Some("failure" | "timed_out"))
                || (job["conclusion"].as_str() == Some("timed_out")
                    && row["conclusion"].as_str() == Some("cancelled")),
            successful: row["conclusion"].as_str() == Some("success"),
            required,
            error: "unknown (failure log unavailable)".into(),
        });
    }
    Ok(steps)
}

pub(crate) fn path_segment(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub(crate) fn parse_tip(
    value: &serde_json::Value,
    gitlab: bool,
) -> Result<(String, DateTime<Utc>)> {
    let sha = value[if gitlab { "id" } else { "sha" }]
        .as_str()
        .context("base tip SHA missing")?;
    anyhow::ensure!(
        sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid base tip SHA"
    );
    let date = if gitlab {
        &value["committed_date"]
    } else {
        &value["commit"]["committer"]["date"]
    };
    Ok((
        sha.into(),
        DateTime::parse_from_rfc3339(date.as_str().context("base tip timestamp missing")?)?
            .with_timezone(&Utc),
    ))
}

pub(crate) fn error_line(log: &str, step: &str) -> String {
    log.lines()
        .find(|line| {
            line.contains(step)
                && (line.to_ascii_lowercase().contains("error")
                    || line.to_ascii_lowercase().contains("timed out"))
        })
        .map(|line| line.chars().take(500).collect())
        .unwrap_or_else(|| "unknown (no step error line reported)".into())
}

/// Unknown evidence is structured coverage, never an actionable finding.
pub(crate) fn scan(root: &Path) -> Result<(Vec<DoctorFinding>, Vec<String>)> {
    let cfg = crate::read_project_config_value(root);
    let policy = Policy::read(cfg.as_ref())?;
    if !policy.enabled {
        return Ok((Vec::new(), Vec::new()));
    }
    let forge = crate::forge::forge_for_open_change(root);
    let refs = forge.landing_open_changes()?;
    let default_branch = crate::forge::default_branch_of(root);
    let (base, tip) = forge.landing_tip(&default_branch)?;
    let mut changes = Vec::new();
    let mut unknown = Vec::new();
    for change in refs {
        let status = forge.change_status(&change)?;
        let rows = forge.check_rows(&change)?;
        let holds = forge.recorded_hold_labels(root, &change)?;
        let reason = blocked_reason(&rows, holds, &status);
        let steps = match forge.landing_steps(&change) {
            Ok(Some(steps)) => steps,
            Ok(None) => {
                unknown.push(format!(
                    "#{} CI step headroom: unknown (forge cannot report step timings)",
                    change.id
                ));
                Vec::new()
            }
            Err(error) => {
                unknown.push(format!(
                    "#{} CI step headroom: unknown ({error})",
                    change.id
                ));
                Vec::new()
            }
        };
        if steps.is_empty() && forge.kind() != crate::forge::ForgeKind::GitLab {
            unknown.push(format!(
                "#{} CI step headroom: unknown (no completed per-step evidence reported)",
                change.id
            ));
        }
        if forge.kind() == crate::forge::ForgeKind::GitLab {
            unknown.push(format!(
                "#{} CI step headroom: unknown (GitLab reports jobs, not script-step timings)",
                change.id
            ));
        }
        let mut timeouts = BTreeMap::new();
        let change_base = if change.base == default_branch {
            base.clone()
        } else {
            forge.landing_tip(&change.base)?.0
        };
        let paths: BTreeSet<_> = steps
            .iter()
            .map(|s| s.workflow.as_str())
            .filter(|p| {
                p.starts_with(".github/workflows/") && !p.contains("..") && !p.contains(':')
            })
            .collect();
        for path in paths {
            match forge
                .landing_workflow(&change_base, path)
                .and_then(|body| workflow_timeouts(&body, path))
            {
                Ok(values) => timeouts.extend(values),
                Err(error) => unknown.push(format!(
                    "#{} CI step headroom: unknown ({path} at base: {error})",
                    change.id
                )),
            }
        }
        for step in &steps {
            if step.successful
                && step.required
                && (step.seconds.is_none() || timeout_for(step, &timeouts).is_none())
            {
                unknown.push(format!("#{} CI step headroom: unknown ({} / {} has no duration or literal base timeout)", change.id, step.check, step.name));
            }
        }
        changes.push(Change {
            id: change.id,
            reason,
            steps,
            timeouts,
        });
    }
    Ok((evaluate(&changes, tip, Utc::now(), &policy), unknown))
}

fn blocked_reason(
    rows: &[crate::ci_gate::CheckRow],
    holds: (bool, bool, bool),
    status: &crate::forge::ChangeStatus,
) -> String {
    if holds.0 || (holds.1 && !holds.2) {
        return "merge hold".into();
    }
    if let Some(row) = rows
        .iter()
        .find(|r| matches!(r.bucket.as_str(), "fail" | "cancel"))
    {
        return format!("CI red: {}", row.name);
    }
    if matches!(
        status.mergeable,
        crate::forge_observed::Observed::Known(false)
    ) {
        return "conflict".into();
    }
    if status.review != ReviewDecision::Approved {
        return "awaiting review".into();
    }
    if rows.iter().any(|r| r.bucket == "pending") {
        return "CI pending".into();
    }
    if !matches!(
        status.mergeable,
        crate::forge_observed::Observed::Known(true)
    ) {
        return "mergeability unknown".into();
    }
    "ready; awaiting landing".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:STORY-b6b1278890774ef993d2ebc327862470-772.acfec592 | ai:codex
    #[test]
    fn landing_health_github_adapter_required_dependencies() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        use crate::forge::{ChangeRef, Forge, GitHubForge};
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let script = root.join("gh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$*" in
  *--required*) cat required.json; exit "$(cat required.exit)" ;;
  'pr checks '*) cat checks.json ;;
  *'/commits/main') cat tip.json ;;
  *'/contents/'*) cat workflow.yml ;;
  *'/jobs?per_page=100') cat jobs.json ;;
  *'/actions/runs/42') printf '%s' '{"path":".github/workflows/ci.yml"}' ;;
  *) exit 99 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", root.display(), std::env::var("PATH").unwrap());
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("PATH", &path),
            ("AIDA_TEST_GH_BINARY", script.to_str().unwrap()),
        ]);
        let workflow = r#"jobs:
  build-result:
    name: "${{ github.event.action == 'labeled' && 'label event (Build not rerun)' || 'Build (ubuntu-latest)' }}"
    needs: [lanes]
  lanes:
    name: CI (${{ matrix.shard }})
    needs: tier
    steps:
      - name: Ratchet set
        timeout-minutes: 12
  tier:
    steps: []
  unrelated:
    name: Optional
    steps: []
"#;
        std::fs::write(root.join("workflow.yml"), workflow).unwrap();
        std::fs::write(
            root.join("required.json"),
            r#"[{"name":"Build (ubuntu-latest)"},{"name":"merge-hold-gate"}]"#,
        )
        .unwrap();
        std::fs::write(root.join("required.exit"), "0").unwrap();
        std::fs::write(root.join("checks.json"), r#"[{"name":"CI (fast compile)","bucket":"pass","workflow":"CI","link":"https://github.com/o/r/actions/runs/42/job/1"}]"#).unwrap();
        std::fs::write(root.join("tip.json"), serde_json::json!({"sha": "a".repeat(40), "commit":{"committer":{"date":"2026-11-16T02:00:00Z"}}}).to_string()).unwrap();
        let job = serde_json::json!({"id":1,"name":"CI (fast compile)","conclusion":"success","steps":[{"name":"Ratchet set","started_at":"2026-11-16T01:40:00Z","completed_at":"2026-11-16T01:50:48Z","conclusion":"success"}]});
        std::fs::write(
            root.join("jobs.json"),
            serde_json::json!([{"jobs":[job]}]).to_string(),
        )
        .unwrap();
        let forge = GitHubForge::new(root);
        let change = ChangeRef {
            id: 1,
            branch: "feature".into(),
            base: "main".into(),
            url: String::new(),
            title: None,
        };
        let steps = forge.landing_steps(&change).unwrap().unwrap();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].required);
        let now = DateTime::parse_from_rfc3339("2026-11-16T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let changes = [Change {
            id: 1,
            reason: String::new(),
            steps,
            timeouts: workflow_timeouts(workflow, ".github/workflows/ci.yml").unwrap(),
        }];
        let findings = evaluate(&changes, now, now, &Policy::read(None).unwrap());
        assert_eq!(findings.len(), 1);
        assert!(findings[0].summary.contains("90%"));
        let covered = required_workflow_jobs(workflow, &["Build (ubuntu-latest)".into()]).unwrap();
        assert!(covered.contains(&"tier".into()));
        assert!(!covered
            .iter()
            .any(|name| workflow_job_name_matches(name, "Optional")));
        let shipped = required_workflow_jobs(
            include_str!("../../.github/workflows/ci.yml"),
            &["Build (ubuntu-latest)".into()],
        )
        .unwrap();
        assert!(shipped
            .iter()
            .any(|name| workflow_job_name_matches(name, "CI (fast compile)")));
        assert!(shipped
            .iter()
            .any(|name| workflow_job_name_matches(name, "CI (unit-1)")));
        assert!(!shipped
            .iter()
            .any(|name| workflow_job_name_matches(name, "Release (ubuntu-latest)")));
        // A positive empty answer differs from failed transport/auth or malformed evidence.
        std::fs::write(root.join("required.json"), "[]").unwrap();
        assert!(!forge.landing_steps(&change).unwrap().unwrap()[0].required);
        for (body, exit) in [("[]", "4"), ("not json", "0"), ("[{}]", "0")] {
            std::fs::write(root.join("required.json"), body).unwrap();
            std::fs::write(root.join("required.exit"), exit).unwrap();
            assert!(forge.landing_steps(&change).is_err(), "{body} / {exit}");
        }
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-772.acfec592 | ai:codex
    #[test]
    fn landing_health_fixture_matrix() {
        let now = DateTime::parse_from_rfc3339("2026-11-16T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let policy = Policy::read(None).unwrap();
        let mut changes = Vec::new();
        for id in 1..=3 {
            changes.push(Change { id, reason: "CI red: Build".into(), steps: vec![Step {
                check: "Build".into(), name: "Ratchet set".into(), workflow: ".github/workflows/ci.yml".into(), completed: now - Duration::minutes(5), seconds: Some(648.0), failed: true, successful: false, required: true, error: "timed out after 12 minutes".into(),
            }], timeouts: workflow_timeouts("jobs:\n  build:\n    name: Build\n    steps:\n      - name: Ratchet set\n        timeout-minutes: 12\n", ".github/workflows/ci.yml").unwrap() });
        }
        let successful_step = Step {
            failed: false,
            successful: true,
            ..changes[0].steps[0].clone()
        };
        changes[0].steps.push(successful_step);
        let findings = evaluate(&changes, now - Duration::minutes(120), now, &policy);
        assert_eq!(findings.len(), 3);
        assert!(findings[0].summary.contains("probable infrastructure"));
        assert!(findings[0].summary.contains("timed out after 12 minutes"));
        assert!(findings[1].summary.contains("3 open PRs"));
        assert!(findings[2].summary.contains("90%"));
        assert!(evaluate(&[], now - Duration::hours(5), now, &policy).is_empty());
        // One PR with retries never supplies the three distinct PRs threshold.
        let repeated = vec![Change {
            id: 1,
            reason: "CI red".into(),
            steps: vec![changes[0].steps[0].clone(); 4],
            timeouts: BTreeMap::new(),
        }];
        assert!(evaluate(&repeated, now, now, &policy).is_empty());
        let job = serde_json::json!({"name": "Build (ubuntu-latest)", "conclusion": "timed_out", "steps": [{"name": "Ratchet set", "started_at": "2026-11-16T01:00:00Z", "completed_at": "2026-11-16T01:10:48Z", "conclusion": "cancelled"}]});
        let parsed = github_steps(&job, ".github/workflows/ci.yml", true).unwrap();
        assert!(parsed[0].failed);
        assert_eq!(parsed[0].seconds, Some(648.0));
        let timeouts = workflow_timeouts("jobs:\n  build:\n    name: Build (${{ matrix.os }})\n    timeout-minutes: 20\n    steps:\n      - name: Ratchet set\n        timeout-minutes: 12\n", ".github/workflows/ci.yml").unwrap();
        assert_eq!(timeout_for(&parsed[0], &timeouts), Some(720.0));
        assert_eq!(
            error_line(
                "Build\tRatchet set\tError: timed out after 12 minutes",
                "Ratchet set"
            ),
            "Build\tRatchet set\tError: timed out after 12 minutes"
        );
        let bad: toml::Value =
            toml::from_str("[landing_health]\nstep_headroom_ratio = 1.1").unwrap();
        assert!(Policy::read(Some(&bad)).is_err());
        let disabled: toml::Value = toml::from_str("[landing_health]\nenabled = false").unwrap();
        assert!(!Policy::read(Some(&disabled)).unwrap().enabled);
        changes[0].steps[1].required = false;
        assert_eq!(evaluate(&changes, now, now, &policy).len(), 1); // shared failure only
        changes[0].steps[1].required = true;
        for c in &mut changes {
            for step in &mut c.steps {
                step.completed = now - Duration::days(2);
            }
        }
        assert!(evaluate(&changes, now, now, &policy).is_empty());
        for c in &mut changes {
            c.steps.clear();
        }
        assert!(evaluate(&changes, now, now, &policy).is_empty());
        assert_eq!(crate::landing_gap::landing_gap(None, now), None);
        assert_eq!(
            crate::landing_gap::landing_gap(Some(now + Duration::hours(1)), now),
            Some(Duration::zero())
        );
    }
}
