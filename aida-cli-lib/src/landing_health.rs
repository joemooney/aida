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

#[derive(Default)]
pub struct StepEvidence {
    pub steps: Vec<Step>,
    pub unknown: Vec<String>,
    pub(crate) replaced_runs: BTreeMap<u64, u64>,
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
    let age = crate::landing_gaps::since_last(Some(tip), now).unwrap_or_default();
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
        let mut rows = forge.check_rows(&change)?;
        let holds = forge.recorded_hold_labels(root, &change)?;
        let (steps, timeouts) = scan_steps(
            forge.as_ref(),
            &change,
            &default_branch,
            &base,
            &mut unknown,
            &mut rows,
        );
        let reason = blocked_reason(&rows, holds, &status);
        changes.push(Change {
            id: change.id,
            reason,
            steps,
            timeouts,
        });
    }
    Ok((evaluate(&changes, tip, Utc::now(), &policy), unknown))
}

fn scan_steps(
    forge: &dyn crate::forge::Forge,
    change: &crate::forge::ChangeRef,
    default_branch: &str,
    base: &str,
    unknown: &mut Vec<String>,
    rows: &mut Vec<crate::ci_gate::CheckRow>,
) -> (Vec<Step>, BTreeMap<(String, String, String), f64>) {
    let steps = match forge.landing_steps(change) {
        Ok(Some(evidence)) => {
            rows.retain(|row| {
                !crate::forge::is_replaced_run(&evidence.replaced_runs, row.run_id.as_deref())
            });
            unknown.extend(
                evidence
                    .unknown
                    .into_iter()
                    .map(|error| format!("#{} CI step headroom: unknown ({error})", change.id)),
            );
            evidence.steps
        }
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
        Some(base.to_owned())
    } else {
        match forge.landing_tip(&change.base) {
            Ok((sha, _)) => Some(sha),
            Err(error) => {
                unknown.push(format!(
                    "#{} CI step headroom: unknown (base {}: {error:#})",
                    change.id, change.base
                ));
                None
            }
        }
    };
    if let Some(sha) = change_base {
        let paths: BTreeSet<_> = steps
            .iter()
            .map(|s| s.workflow.as_str())
            .filter(|p| {
                p.starts_with(".github/workflows/") && !p.contains("..") && !p.contains(':')
            })
            .collect();
        for path in paths {
            match forge
                .landing_workflow(&sha, path)
                .and_then(|body| workflow_timeouts(&body, path))
            {
                Ok(values) => timeouts.extend(values),
                Err(error) => unknown.push(format!(
                    "#{} CI step headroom: unknown ({path} at base: {error})",
                    change.id
                )),
            }
        }
    }
    for step in &steps {
        if step.successful
            && step.required
            && (step.seconds.is_none() || timeout_for(step, &timeouts).is_none())
        {
            unknown.push(format!(
                "#{} CI step headroom: unknown ({} / {} has no duration or literal base timeout)",
                change.id, step.check, step.name
            ));
        }
    }
    (steps, timeouts)
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

    // trace:BUG-2685 | ai:codex
    #[test]
    fn landing_health_gitlab_stalled_trace_keeps_failed_steps() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        use crate::forge::{ChangeRef, Forge, GitLabForge};
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration as StdDuration, Instant};

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let script = root.join("glab");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$4" in
  projects/:id/pipelines) printf '%s' '[{"id":42}]' ;;
  projects/:id/pipelines/42/jobs) cat jobs.json ;;
  projects/:id/jobs/1/trace) touch stalled-trace-started; exec sleep 30 ;;
  projects/:id/jobs/2/trace) printf '%s\n' 'ERROR: test failed' ;;
  projects/:id/jobs/3/trace) printf '%s\n' 'ERROR: unavailable'; exit 1 ;;
  *) exit 99 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("PATH", "/usr/bin:/bin"),
            ("AIDA_TEST_GLAB_BINARY", script.to_str().unwrap()),
        ]);
        std::fs::write(
            root.join("jobs.json"),
            r#"[
                {"id":1,"name":"build","status":"failed","finished_at":"2026-11-16T02:55:00+01:00"},
                {"id":2,"name":"test","status":"failed","finished_at":"2026-11-16T01:56:00Z"},
                {"id":3,"name":"lint","status":"failed","finished_at":"2026-11-16T01:57:00Z"},
                {"name":"package","status":"failed","finished_at":"2026-11-16T01:58:00Z"},
                {"id":4,"name":"docs","status":"success","finished_at":"2026-11-16T01:59:00Z"}
            ]"#,
        )
        .unwrap();
        let forge = GitLabForge::new(root);
        let change = ChangeRef {
            id: 1,
            branch: "feature".into(),
            base: "main".into(),
            url: String::new(),
            title: None,
        };
        let started = Instant::now();
        let steps = forge.landing_steps(&change).unwrap().unwrap().steps;
        assert!(root.join("stalled-trace-started").is_file());
        assert!(started.elapsed() < StdDuration::from_secs(25));
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].check, "build");
        assert_eq!(steps[0].completed.to_rfc3339(), "2026-11-16T01:55:00+00:00");
        assert_eq!(steps[1].check, "test");
        assert_eq!(steps[1].error, "ERROR: test failed");
        assert_eq!(steps[2].check, "lint");
        assert_eq!(steps[3].check, "package");
        for index in [0, 2, 3] {
            assert_eq!(steps[index].error, "unknown (job trace unavailable)");
        }
        assert!(steps
            .iter()
            .all(|step| step.failed && step.name == "script"));
    }

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
  *'/commits/main') test ! -f fail-tip && cat tip.json ;;
  *'/contents/.github/workflows/new.yml?'*) exit 1 ;;
  *'/contents/'*)
    if test -f fail-workflow-once; then rm fail-workflow-once; exit 1; fi
    cat workflow.yml ;;
  *'/actions/runs/44/jobs?per_page=100') exit 1 ;;
  *'/actions/runs/45/jobs?per_page=100') cat cancelled-jobs.json ;;
  *'/jobs?per_page=100') cat jobs.json ;;
  *--log-failed*)
    if test -f slow-log-once; then rm slow-log-once; exec sleep 60; fi
    exit 1 ;;
  *'/actions/runs/45') printf '%s' '{"id":45,"workflow_id":1,"event":"pull_request","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","head_branch":"feature","head_repository":{"id":1},"conclusion":"cancelled","path":".github/workflows/ci.yml"}' ;;
  *'/actions/runs/46') printf '%s' '{"id":46,"workflow_id":1,"event":"pull_request","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","head_branch":"feature","head_repository":{"id":1},"conclusion":"success","path":".github/workflows/ci.yml"}' ;;
  *'/actions/runs/43') printf '%s' '{"path":".github/workflows/new.yml"}' ;;
  *'/actions/runs/42') printf '%s' '{"path":".github/workflows/ci.yml"}' ;;
  *) exit 99 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The fake gh needs only the fixture directory and fixed system tools.
        // Keep its PATH independent of the host's ambient environment.
        let path = format!("{}:/usr/bin:/bin", root.display());
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
        let steps = forge.landing_steps(&change).unwrap().unwrap().steps;
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
        // trace:STORY-2849 | ai:claude — BUG-2617 against the shipped workflow:
        // a successful Ratchet set inside the non-listed fast-compile dependency
        // at 90% of its real base timeout must warn.
        let shipped_body = include_str!("../../.github/workflows/ci.yml");
        let shipped_timeouts = workflow_timeouts(shipped_body, ".github/workflows/ci.yml").unwrap();
        let ratchet = Step {
            check: "CI (fast compile)".into(),
            name: "Ratchet set".into(),
            workflow: ".github/workflows/ci.yml".into(),
            completed: now - Duration::minutes(5),
            seconds: Some(0.9 * 25.0 * 60.0),
            failed: false,
            successful: true,
            required: shipped
                .iter()
                .any(|name| workflow_job_name_matches(name, "CI (fast compile)")),
            error: String::new(),
        };
        assert_eq!(timeout_for(&ratchet, &shipped_timeouts), Some(25.0 * 60.0));
        let shipped_findings = evaluate(
            &[Change {
                id: 2,
                reason: String::new(),
                steps: vec![ratchet],
                timeouts: shipped_timeouts,
            }],
            now,
            now,
            &Policy::read(None).unwrap(),
        );
        assert_eq!(shipped_findings.len(), 1, "{shipped_findings:?}");
        assert!(shipped_findings[0].summary.contains("Ratchet set"));
        // trace:BUG-2692 | ai:codex
        let aggregate = |conclusion| {
            serde_json::json!([{"jobs":[{
                "id":1,"name":"Build (ubuntu-latest)","conclusion":conclusion,
                "steps":[{"name":"Require every CI lane","conclusion":conclusion,
                          "completed_at":"2026-11-16T01:55:00Z"}]
            }]}])
        };
        std::fs::write(root.join("jobs.json"), aggregate("success").to_string()).unwrap();
        std::fs::write(
            root.join("cancelled-jobs.json"),
            aggregate("failure").to_string(),
        )
        .unwrap();
        let tier = |name, bucket, id| {
            serde_json::json!({
                "name":name,"bucket":bucket,"link":format!("https://github.com/o/r/actions/runs/{id}/job/1")
            })
        };
        let check_rows = |rows: Vec<serde_json::Value>| {
            std::fs::write(
                root.join("checks.json"),
                serde_json::Value::Array(rows).to_string(),
            )
            .unwrap();
        };
        let fast = tier("ci-tier: fast", "fail", 45);
        for replaced in [false, true] {
            let mut rows = vec![fast.clone()];
            if replaced {
                rows.push(tier("ci-tier: full", "pass", 46));
            }
            check_rows(rows);
            let steps = forge.landing_steps(&change).unwrap().unwrap().steps;
            assert_eq!(steps.len(), 1);
            assert_eq!(steps[0].failed, !replaced);
            let changes: Vec<_> = (1..=3)
                .map(|id| Change {
                    id,
                    reason: String::new(),
                    steps: steps.clone(),
                    timeouts: BTreeMap::new(),
                })
                .collect();
            let findings = evaluate(&changes, now, now, &Policy::read(None).unwrap());
            assert_eq!(findings.len(), usize::from(!replaced), "{findings:?}");
        }
        // trace:STORY-2849 | ai:codex — a workflow added by the PR cannot
        // suppress failures from the existing workflow (in either run order).
        let mut failed_job = job.clone();
        failed_job["conclusion"] = "failure".into();
        failed_job["steps"][0]["conclusion"] = "failure".into();
        std::fs::write(
            root.join("jobs.json"),
            serde_json::json!([{"jobs":[failed_job]}]).to_string(),
        )
        .unwrap();
        for runs in [[42, 43], [43, 42]] {
            let rows: Vec<_> = runs
                .iter()
                .map(|run| {
                    serde_json::json!({
                        "name":"CI (fast compile)", "bucket":"fail", "workflow":"CI",
                        "link":format!("https://github.com/o/r/actions/runs/{run}/job/1")
                    })
                })
                .collect();
            std::fs::write(
                root.join("checks.json"),
                serde_json::to_string(&rows).unwrap(),
            )
            .unwrap();
            let steps = forge.landing_steps(&change).unwrap().unwrap().steps;
            assert_eq!(steps.len(), 2);
            assert!(steps.iter().all(|step| step.failed));
            assert!(
                steps
                    .iter()
                    .find(|s| s.workflow.ends_with("ci.yml"))
                    .unwrap()
                    .required
            );
            assert!(
                !steps
                    .iter()
                    .find(|s| s.workflow.ends_with("new.yml"))
                    .unwrap()
                    .required
            );
            assert!(forge
                .landing_workflow(&"a".repeat(40), ".github/workflows/new.yml")
                .is_err());
            let changes: Vec<_> = (1..=3)
                .map(|id| Change {
                    id,
                    reason: String::new(),
                    steps: steps.clone(),
                    timeouts: BTreeMap::new(),
                })
                .collect();
            let findings = evaluate(&changes, now, now, &Policy::read(None).unwrap());
            assert_eq!(findings.len(), 1, "{findings:?}");
            assert!(findings[0]
                .summary
                .contains("3 open PRs fail check CI (fast compile), step Ratchet set"));
        }
        // An unavailable dependency graph stays visible even when timeout parsing succeeds.
        std::fs::write(
            root.join("jobs.json"),
            serde_json::json!([{"jobs":[job]}]).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("checks.json"), r#"[{"name":"CI (fast compile)","bucket":"pass","workflow":"CI","link":"https://github.com/o/r/actions/runs/42/job/1"}]"#).unwrap();
        let sha = "a".repeat(40);
        std::fs::write(root.join("fail-workflow-once"), "").unwrap();
        for expected in [
            "base workflow observation failed",
            "required workflow dependency missing",
        ] {
            if expected.contains("dependency missing") {
                std::fs::write(
                    root.join("workflow.yml"),
                    workflow.replace("needs: [lanes]", "needs: [lanes, missing]"),
                )
                .unwrap();
            }
            let mut unknown = Vec::new();
            let (steps, timeouts) =
                scan_steps(&forge, &change, "main", &sha, &mut unknown, &mut Vec::new());
            assert_eq!(steps.len(), 1);
            assert!(!steps[0].required);
            assert_eq!(steps[0].seconds, Some(648.0));
            assert_eq!(timeout_for(&steps[0], &timeouts), Some(720.0));
            assert_eq!(unknown.len(), 1, "{unknown:?}");
            assert!(
                unknown[0].contains("CI step headroom: unknown"),
                "{unknown:?}"
            );
            assert!(unknown[0].contains(expected), "{unknown:?}");
        }
        std::fs::write(root.join("workflow.yml"), workflow).unwrap();

        // A positive empty answer differs from failed auth or malformed evidence.
        std::fs::write(root.join("required.json"), "[]").unwrap();
        let evidence = forge.landing_steps(&change).unwrap().unwrap();
        assert!(!evidence.steps[0].required);
        assert!(evidence.unknown.is_empty(), "{:?}", evidence.unknown);

        std::fs::write(
            root.join("jobs.json"),
            serde_json::json!([{"jobs":[failed_job]}]).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("checks.json"), r#"[
            {"name":"Other","bucket":"fail","workflow":"CI","link":"https://github.com/o/r/actions/runs/44/job/2"},
            {"name":"CI (fast compile)","bucket":"fail","workflow":"CI","link":"https://github.com/o/r/actions/runs/42/job/1"}
        ]"#).unwrap();
        for (body, exit, expected) in [
            ("[]", "4", "required-check coverage unavailable"),
            ("not json", "0", "required-check coverage unavailable"),
            (
                r#"[{"name":"Build (ubuntu-latest)"}]"#,
                "0",
                "base tip observation failed",
            ),
            ("[]", "0", "run 44 jobs"),
        ] {
            std::fs::write(root.join("required.json"), body).unwrap();
            std::fs::write(root.join("required.exit"), exit).unwrap();
            if expected == "base tip observation failed" {
                std::fs::write(root.join("fail-tip"), "").unwrap();
            }
            if expected == "run 44 jobs" {
                std::fs::write(root.join("slow-log-once"), "").unwrap();
            }
            let mut unknown = Vec::new();
            let mut changes = Vec::new();
            for id in 1..=3 {
                let change = ChangeRef {
                    id,
                    ..change.clone()
                };
                let (steps, timeouts) =
                    scan_steps(&forge, &change, "main", &sha, &mut unknown, &mut Vec::new());
                assert_eq!(steps.len(), 1);
                assert!(steps[0].failed);
                assert_eq!(steps[0].error, "unknown (failure log unavailable)");
                changes.push(Change {
                    id,
                    reason: String::new(),
                    steps,
                    timeouts,
                });
            }
            if expected == "base tip observation failed" {
                std::fs::remove_file(root.join("fail-tip")).unwrap();
            }
            assert!(!root.join("slow-log-once").exists());
            assert!(
                unknown.iter().any(|message| message.contains(expected)),
                "{unknown:?}"
            );
            let findings = evaluate(&changes, now, now, &Policy::read(None).unwrap());
            assert_eq!(findings.len(), 1, "{findings:?}");
            assert!(findings[0]
                .summary
                .contains("3 open PRs fail check CI (fast compile), step Ratchet set"));
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
        assert_eq!(crate::landing_gaps::since_last(None, now), None);
        assert_eq!(
            crate::landing_gaps::since_last(Some(now + Duration::hours(1)), now),
            Some(Duration::zero())
        );
    }
}
