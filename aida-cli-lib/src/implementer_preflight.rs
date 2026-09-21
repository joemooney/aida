//! Local CI-guard parity before an orchestrated implementer branch is published.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const GUARD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CI_BASH_SHELL: &str = "bash --noprofile --norc -e -o pipefail {0}";

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
pub(crate) enum ResolvedGuard {
    Found(Guard),
    Invalid(String),
    Skipped(String),
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
pub(crate) fn guards_from_ci(project_root: &Path, names: &[String]) -> Vec<ResolvedGuard> {
    let path = project_root.join(".github/workflows/ci.yml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return names
            .iter()
            .map(|name| ResolvedGuard::Skipped(format!("{name}: CI workflow missing")))
            .collect();
    };
    let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return names
            .iter()
            .map(|name| ResolvedGuard::Skipped(format!("{name}: CI workflow unreadable")))
            .collect();
    };
    let configured_shell = yaml
        .get("jobs")
        .and_then(|v| v.get("build"))
        .and_then(|v| v.get("defaults"))
        .and_then(|v| v.get("run"))
        .and_then(|v| v.get("shell"))
        .and_then(|v| v.as_str());
    if configured_shell != Some(CI_BASH_SHELL) {
        let actual = configured_shell.unwrap_or("<missing>");
        return names
            .iter()
            .map(|name| {
                ResolvedGuard::Invalid(format!(
                    "{name}: CI Bash contract drifted: expected `{CI_BASH_SHELL}`, found `{actual}`"
                ))
            })
            .collect();
    }
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
                .map(ResolvedGuard::Found)
                .unwrap_or_else(|| ResolvedGuard::Skipped(format!("{name}: no matching CI guard")))
        })
        .collect()
}

fn binary_path(project_root: &Path) -> PathBuf {
    let mut metadata = Command::new("cargo");
    metadata
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(project_root);
    if let Ok(Some(out)) = run_bounded(&mut metadata, Duration::from_secs(10)) {
        if out.status.success() {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(target) = value.get("target_directory").and_then(|v| v.as_str()) {
                    return PathBuf::from(target).join("debug/aida");
                }
            }
        }
    }
    project_root.join("target/debug/aida")
}

fn output_text(out: &Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text.trim().to_string()
}

fn run_bounded(command: &mut Command, timeout: Duration) -> Result<Option<Output>, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Put the shell and all descendants (cargo, clippy, scripts) in their
        // own process group so timeout cleanup cannot leave pipe-holding
        // grandchildren behind. trace:TASK-1289 | ai:codex
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }
    let mut child = command.spawn().map_err(|err| err.to_string())?;
    let mut stdout = child.stdout.take().expect("stdout was configured as piped");
    let mut stderr = child.stderr.take().expect("stderr was configured as piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(|err| err.to_string())? {
            Some(_) => {
                let status = child.wait().map_err(|err| err.to_string())?;
                let stdout = stdout_reader
                    .join()
                    .map_err(|_| "stdout reader panicked".to_string())?
                    .map_err(|err| err.to_string())?;
                let stderr = stderr_reader
                    .join()
                    .map_err(|_| "stderr reader panicked".to_string())?
                    .map_err(|err| err.to_string())?;
                return Ok(Some(Output {
                    status,
                    stdout,
                    stderr,
                }));
            }
            None if Instant::now() >= deadline => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                #[cfg(not(unix))]
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Ok(None);
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn describe_binary(path: &Path) -> String {
    let display = path.display();
    match run_bounded(Command::new(path).arg("--version"), Duration::from_secs(10)) {
        Ok(Some(out)) if out.status.success() => format!("{display} ({})", output_text(&out)),
        Ok(Some(out)) => format!("{display} (version unavailable: {})", output_text(&out)),
        Ok(None) => format!("{display} (version probe timed out)"),
        Err(err) => format!("{display} (version probe failed: {err})"),
    }
}

fn guard_uses_aida(guard: &Guard) -> bool {
    guard.name == "CLI-manual drift-guard (every command documented)"
        || guard
            .command
            .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
            .any(|word| word == "aida")
}

/// Build the shell invocation used for CI-derived `run:` steps.
///
/// GitHub Actions invokes Bash on Linux as
/// `bash --noprofile --norc -e -o pipefail {0}`. Keep this argv in parity with
/// that runner contract so the local preflight cannot accept a command that CI
/// rejects. See <https://docs.github.com/actions/writing-workflows/workflow-syntax-for-github-actions#custom-shell>.
// trace:BUG-1420 | ai:codex
fn github_actions_bash(command: &str) -> Command {
    let mut shell = Command::new("bash");
    shell.args([
        "--noprofile",
        "--norc",
        "-e",
        "-o",
        "pipefail",
        "-c",
        command,
    ]);
    shell
}

fn build_worktree_binary(project_root: &Path, path: &Path) -> Result<String, String> {
    let mut build = Command::new("cargo");
    build
        .args(["build", "-p", "aida-cli", "--bin", "aida"])
        .current_dir(project_root);
    match run_bounded(&mut build, BUILD_TIMEOUT) {
        Ok(Some(out)) if out.status.success() => Ok(describe_binary(path)),
        Ok(Some(out)) => Err(format!(
            "{} (build failed: {})",
            path.display(),
            output_text(&out)
        )),
        Ok(None) => Err(format!(
            "{} (build timed out after {}s)",
            path.display(),
            BUILD_TIMEOUT.as_secs()
        )),
        Err(err) => Err(format!("{} (build could not start: {err})", path.display())),
    }
}

fn execute(project_root: &Path, guards: Vec<ResolvedGuard>, timeout: Duration) -> Vec<GuardResult> {
    let binary_path = binary_path(project_root);
    // Build once from this worktree before running any CI-derived command.
    // Scripts can invoke `aida` indirectly, so PATH must never inherit a stale
    // installation even when the YAML command itself does not name it.
    let binary = build_worktree_binary(project_root, &binary_path);
    let binary_label = match &binary {
        Ok(label) | Err(label) => label.clone(),
    };
    let default_branch = crate::forge::default_branch_of(project_root);
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(
            binary_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
        )
        .chain(std::env::split_paths(&inherited_path)),
    )
    .unwrap_or(inherited_path);

    guards.into_iter().map(|resolved| {
        let guard = match resolved {
            ResolvedGuard::Found(guard) => guard,
            ResolvedGuard::Invalid(output) => return GuardResult::Failed {
                name: "CI shell parity".into(),
                output,
            },
            ResolvedGuard::Skipped(note) => return GuardResult::Skipped(format!("{note}; binary: {binary_label}")),
        };
        if guard_uses_aida(&guard) {
            if let Err(reason) = &binary {
                return GuardResult::Skipped(format!("{}: worktree binary unavailable, so binary-dependent guard was not authoritative: {reason}; binary: {reason}", guard.name));
            }
        }
        let command = guard.command
            .replace("${{ github.event_name }}", "pull_request")
            .replace("${{ github.base_ref }}", &default_branch);
        let mut child = github_actions_bash(&command);
        child.current_dir(project_root)
            .env("PATH", &path)
            .env("AIDA_PREFLIGHT_BINARY", &binary_path)
            .env("AIDA_PREFLIGHT_DEFAULT_BRANCH", &default_branch);
        match run_bounded(&mut child, timeout) {
            Ok(Some(out)) if out.status.success() => GuardResult::Passed(guard.name),
            Ok(Some(out)) => GuardResult::Failed {
                name: guard.name,
                output: format!("binary: {binary_label}\n{}", output_text(&out)),
            },
            Ok(None) => GuardResult::Skipped(format!("{}: timed out after {}s; binary: {binary_label}", guard.name, timeout.as_secs())),
            Err(err) => GuardResult::Skipped(format!("{}: could not start ({err}); binary: {binary_label}", guard.name)),
        }
    }).collect()
}

pub(crate) fn run(project_root: &Path) -> Vec<GuardResult> {
    let names = configured_guard_names(project_root);
    execute(
        project_root,
        guards_from_ci(project_root, &names),
        GUARD_TIMEOUT,
    )
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
            vec![ResolvedGuard::Skipped(
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
            "jobs:\n  build:\n    defaults:\n      run:\n        shell: bash --noprofile --norc -e -o pipefail {0}\n    steps:\n      - name: Portability\n        run: |\n          echo 'tests/x.rs:7 non-portable path; fix: use tempfile' >&2\n          exit 1\n",
        )
        .unwrap();

        let results = run(root.path());
        let PreflightDecision::Refuse { failed } = decide(&results) else {
            panic!("failing guard must refuse publication")
        };
        assert_eq!(failed[0].0, "Portability");
        assert!(failed[0].1.contains("binary:"));
        assert!(failed[0]
            .1
            .contains("tests/x.rs:7 non-portable path; fix: use tempfile"));
    }

    #[test]
    fn ci_shell_contract_drift_refuses_preflight() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        std::fs::write(
            root.path().join(".github/workflows/ci.yml"),
            "jobs:\n  build:\n    defaults:\n      run:\n        shell: bash {0}\n    steps:\n      - name: Guard\n        run: true\n",
        )
        .unwrap();

        let results = execute(
            root.path(),
            guards_from_ci(root.path(), &["Guard".into()]),
            Duration::from_secs(2),
        );

        let PreflightDecision::Refuse { failed } = decide(&results) else {
            panic!("CI shell drift must refuse publication")
        };
        assert_eq!(failed[0].0, "CI shell parity");
        assert!(failed[0]
            .1
            .contains("expected `bash --noprofile --norc -e -o pipefail {0}`"));
        assert!(failed[0].1.contains("found `bash {0}`"));
    }

    #[test]
    fn guard_timeout_skips_instead_of_refusing() {
        let root = tempfile::tempdir().unwrap();
        let results = execute(
            root.path(),
            vec![ResolvedGuard::Found(Guard {
                name: "Hung guard".into(),
                command: "sleep 5".into(),
            })],
            Duration::from_millis(50),
        );
        assert!(
            matches!(&results[0], GuardResult::Skipped(note) if note.contains("timed out") && note.contains("binary:"))
        );
        assert_eq!(decide(&results), PreflightDecision::Open);
    }

    #[test]
    fn guard_shell_does_not_source_bash_profile() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".bash_profile"),
            "export PROFILE_WAS_SOURCED=yes\n",
        )
        .unwrap();
        let results = execute(
            root.path(),
            vec![ResolvedGuard::Found(Guard {
                name: "No profile".into(),
                command: "test -z \"${PROFILE_WAS_SOURCED:-}\"".into(),
            })],
            Duration::from_secs(2),
        );
        assert_eq!(results, vec![GuardResult::Passed("No profile".into())]);
    }

    #[test]
    fn guard_shell_fails_when_an_intermediate_command_fails() {
        let out = run_bounded(
            &mut github_actions_bash("false\nprintf 'last command succeeded\\n'"),
            Duration::from_secs(2),
        )
        .unwrap()
        .unwrap();

        assert!(
            !out.status.success(),
            "preflight must match CI's -e behavior even when the last command would succeed"
        );
    }

    #[test]
    fn guard_shell_fails_when_a_pipeline_stage_fails() {
        let out = run_bounded(
            &mut github_actions_bash("false | true"),
            Duration::from_secs(2),
        )
        .unwrap()
        .unwrap();

        assert!(
            !out.status.success(),
            "preflight must match CI's pipefail behavior"
        );
    }

    #[test]
    fn github_base_ref_uses_repository_default_branch() {
        let root = tempfile::tempdir().unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());
        let results = execute(
            root.path(),
            vec![ResolvedGuard::Found(Guard {
                name: "Base branch".into(),
                command: "test '${{ github.base_ref }}' = trunk".into(),
            })],
            Duration::from_secs(2),
        );
        assert_eq!(results, vec![GuardResult::Passed("Base branch".into())]);
    }
}
