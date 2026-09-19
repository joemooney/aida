//! Local CI-guard parity before an orchestrated implementer branch is published.

use std::path::Path;
use std::process::Command;

const DEFAULT_GUARD_NAMES: &[&str] = &[
    "Check formatting",
    "Run clippy",
    "Check Rust test portability ratchet",
    "CLI-manual drift-guard (every command documented)",
    "Doc-intent gate (surface changes mark doc-impact)",
];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Guard {
    pub(crate) name: String,
    pub(crate) command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardResult {
    Passed(String),
    Failed { name: String, output: String },
    Skipped(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreflightDecision {
    Open,
    Refuse { failed: Vec<(String, String)> },
}

// trace:TASK-1289 | ai:codex
pub(crate) fn decide(results: &[GuardResult]) -> PreflightDecision {
    let failed = results
        .iter()
        .filter_map(|result| match result {
            GuardResult::Failed { name, output } => Some((name.clone(), output.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    if failed.is_empty() {
        PreflightDecision::Open
    } else {
        PreflightDecision::Refuse { failed }
    }
}

pub(crate) fn configured_guard_names(project_root: &Path) -> Vec<String> {
    let path = project_root.join(".aida/config.toml");
    let defaults = || {
        DEFAULT_GUARD_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return defaults();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return defaults();
    };
    let configured = value
        .get("preflight")
        .and_then(|v| v.get("guards"))
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if configured.is_empty() {
        defaults()
    } else {
        configured
    }
}

/// Resolve configured step names against CI's own YAML. Commands deliberately
/// remain owned by the workflow; AIDA never carries a second hand-written copy.
pub(crate) fn guards_from_ci(project_root: &Path, names: &[String]) -> Vec<GuardResult> {
    let path = project_root.join(".github/workflows/ci.yml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return names
            .iter()
            .map(|name| GuardResult::Skipped(format!("{name}: CI workflow missing")))
            .collect();
    };
    let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return names
            .iter()
            .map(|name| GuardResult::Skipped(format!("{name}: CI workflow unreadable")))
            .collect();
    };
    let steps = yaml
        .get("jobs")
        .and_then(|v| v.as_mapping())
        .into_iter()
        .flat_map(|jobs| jobs.values())
        .filter_map(|job| job.get("steps").and_then(|v| v.as_sequence()))
        .flatten()
        .filter_map(|step| {
            Some(Guard {
                name: step.get("name")?.as_str()?.to_string(),
                command: step.get("run")?.as_str()?.to_string(),
            })
        })
        .collect::<Vec<_>>();
    names
        .iter()
        .map(|name| {
            steps
                .iter()
                .find(|guard| guard.name == *name)
                .cloned()
                .map(|guard| GuardResult::Passed(serde_json::to_string(&guard).unwrap()))
                .unwrap_or_else(|| GuardResult::Skipped(format!("{name}: no matching CI guard")))
        })
        .collect()
}

fn decoded_guard(result: &GuardResult) -> Option<Guard> {
    let GuardResult::Passed(encoded) = result else {
        return None;
    };
    serde_json::from_str(encoded).ok()
}

pub(crate) fn run(project_root: &Path) -> Vec<GuardResult> {
    let names = configured_guard_names(project_root);
    guards_from_ci(project_root, &names)
        .into_iter()
        .map(|resolved| {
            let Some(guard) = decoded_guard(&resolved) else {
                return resolved;
            };
            // GitHub expressions used by PR-only guards are resolved to the
            // local equivalent while the command body itself stays CI-owned.
            let command = guard
                .command
                .replace("${{ github.event_name }}", "pull_request")
                .replace("${{ github.base_ref }}", "main");
            match Command::new("bash")
                .args(["-lc", &command])
                .current_dir(project_root)
                .output()
            {
                Ok(out) if out.status.success() => GuardResult::Passed(guard.name),
                Ok(out) => {
                    let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
                    output.push_str(&String::from_utf8_lossy(&out.stderr));
                    GuardResult::Failed {
                        name: guard.name,
                        output: output.trim().to_string(),
                    }
                }
                Err(err) => {
                    GuardResult::Skipped(format!("{}: could not start ({err})", guard.name))
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_decision_records_runs_and_refuses_on_any_failure() {
        let results = vec![
            GuardResult::Passed("fmt".into()),
            GuardResult::Skipped("unknown".into()),
            GuardResult::Failed {
                name: "portability".into(),
                output: "bad.rs:7; use TempDir".into(),
            },
        ];
        assert_eq!(
            decide(&results),
            PreflightDecision::Refuse {
                failed: vec![("portability".into(), "bad.rs:7; use TempDir".into())]
            }
        );
        assert_eq!(decide(&results[..2]), PreflightDecision::Open);
    }

    #[test]
    fn missing_config_uses_ci_step_name_defaults_and_unknown_steps_skip() {
        let root = tempfile::tempdir().unwrap();
        let names = configured_guard_names(root.path());
        assert_eq!(names.len(), 5);
        let resolved = guards_from_ci(root.path(), &["not-a-step".into()]);
        assert_eq!(
            resolved,
            vec![GuardResult::Skipped(
                "not-a-step: CI workflow missing".into()
            )]
        );
    }

    #[test]
    fn failing_ci_command_preserves_offending_line_and_fix() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[preflight]\nguards = [\"Portability\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join(".github/workflows/ci.yml"),
            "jobs:\n  build:\n    steps:\n      - name: Portability\n        run: |\n          echo 'tests/x.rs:7 non-portable path; fix: use tempfile' >&2\n          exit 1\n",
        )
        .unwrap();

        let results = run(root.path());
        assert_eq!(
            decide(&results),
            PreflightDecision::Refuse {
                failed: vec![(
                    "Portability".into(),
                    "tests/x.rs:7 non-portable path; fix: use tempfile".into()
                )]
            }
        );
    }
}
