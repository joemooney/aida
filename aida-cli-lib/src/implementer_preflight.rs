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
    Failed {
        name: String,
        output: String,
    },
    /// A guard that was NOT RUN because nothing identified it — an unknown or
    /// absent name. Nothing was claimed about the code, and nothing is owed.
    Skipped(String),
    /// A guard that WAS identified and COULD NOT COMPLETE — it timed out or
    /// failed to start.
    ///
    /// Distinct from `Skipped` on purpose. Collapsing them is what made this
    /// gate fail OPEN: a configured guard that never finished was recorded the
    /// same way as one nobody asked for, and publication proceeded as though it
    /// had passed. "We did not look" and "we looked and could not tell" are
    /// different states, and only the first is safe to treat as no objection.
    // trace:TASK-1289 | ai:claude
    Inconclusive {
        name: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreflightDecision {
    Open,
    Refuse { failed: Vec<(String, String)> },
}

// trace:TASK-1289 | ai:codex
pub(crate) fn decide(results: &[GuardResult]) -> PreflightDecision {
    // An INCONCLUSIVE guard refuses alongside a failing one. It has not passed,
    // and a gate that publishes on "we could not tell" is not a gate — a guard
    // made to time out becomes a guard made to succeed.
    // trace:TASK-1289 | ai:claude
    let failed = results
        .iter()
        .filter_map(|result| match result {
            GuardResult::Failed { name, output } => Some((name.clone(), output.clone())),
            GuardResult::Inconclusive { name, reason } => {
                Some((name.clone(), format!("did not complete: {reason}")))
            }
            GuardResult::Passed(_) | GuardResult::Skipped(_) => None,
        })
        .collect::<Vec<_>>();
    if failed.is_empty() {
        PreflightDecision::Open
    } else {
        PreflightDecision::Refuse { failed }
    }
}

/// The comment posted on a change that is being retracted because the
/// publication guards refused it.
///
/// TASK-1289: whoever finds a closed PR needs three things from it — that a
/// machine closed it, why, and what to do next. A closure without them reads
/// as someone else's mistake and gets reopened.
// trace:TASK-1289 | ai:claude
pub(crate) fn retraction_notice(detail: &str) -> String {
    format!(
        "Closed automatically: the publication guards refused this change before it was \
         reviewed.\n\n{detail}\n\nThe branch is untouched — fix the guard failure and reopen, \
         or let the drain retry."
    )
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
/// The GitHub Actions expression for the PR's base branch, which preflight
/// replaces with the repository's default branch.
// trace:BUG-1624 | ai:claude
const BASE_REF_PLACEHOLDER: &str = "${{ github.base_ref }}";

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
    // Built at most once, on the first guard that will ACTUALLY RUN.
    //
    // The build is not keyed on `guard_uses_aida`: a guard whose text does not
    // name `aida` can still reach it through a script, and PATH is prepended
    // for every executing guard, so keying the build on the textual predicate
    // would let a stale installation answer — the BUG-1420 defect this module
    // exists to prevent. What IS safe to skip is the case where no guard
    // executes at all (every one resolved Skipped or Invalid): nothing runs,
    // so nothing can consult PATH, so there is nothing to build.
    // trace:TASK-1289 | ai:claude
    let mut binary: Option<Result<String, String>> = None;
    // The default branch comes from the forge (`gh repo view`) or from
    // `origin/HEAD`, neither of which the operator controls, and the guard
    // text is run by `bash -c`. git ref names may contain `;`, `$()`,
    // backticks and quotes, so a name is substituted into guard text only
    // when it is a valid branch name made of shell-inert characters;
    // otherwise any guard that needs it is reported inconclusive and runs
    // nothing. trace:BUG-1624 | ai:claude
    let raw_default_branch = crate::forge::default_branch_of(project_root);
    let default_branch = crate::git_arg_guard::is_shell_safe_branch_name(&raw_default_branch)
        .then(|| raw_default_branch.clone());
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

    let mut results = Vec::with_capacity(guards.len());
    for resolved in guards {
        let guard = match resolved {
            ResolvedGuard::Found(guard) => guard,
            ResolvedGuard::Invalid(output) => {
                results.push(GuardResult::Failed {
                    name: "CI shell parity".into(),
                    output,
                });
                continue;
            }
            // A skipped guard runs nothing, so it must not be the reason a
            // build happens; report the binary only if one was already built.
            ResolvedGuard::Skipped(note) => {
                let label = match binary.as_ref() {
                    Some(Ok(label)) | Some(Err(label)) => label.as_str(),
                    None => "not built (no guard required it)",
                };
                results.push(GuardResult::Skipped(format!("{note}; binary: {label}")));
                continue;
            }
        };
        // trace:BUG-1624 | ai:claude
        if guard.command.contains(BASE_REF_PLACEHOLDER) && default_branch.is_none() {
            results.push(GuardResult::Inconclusive {
                name: guard.name,
                reason: format!(
                    "the repository's default branch name {raw_default_branch:?} is not a \
                     shell-safe git branch name, so it was not substituted for \
                     `{BASE_REF_PLACEHOLDER}` and the guard did not run"
                ),
            });
            continue;
        }
        // This guard will execute, so the worktree binary is needed now.
        let built = binary.get_or_insert_with(|| build_worktree_binary(project_root, &binary_path));
        let binary_label = match &*built {
            Ok(label) | Err(label) => label.clone(),
        };
        if guard_uses_aida(&guard) {
            if let Err(reason) = &*built {
                // Its own message says "not authoritative" — that is inconclusive,
                // not skipped. Publishing on it is publishing on a guard that was
                // asked for and never answered. trace:TASK-1289 | ai:claude
                let reason = format!("worktree binary unavailable, so this binary-dependent guard could not be authoritative: {reason}");
                results.push(GuardResult::Inconclusive {
                    name: guard.name,
                    reason,
                });
                continue;
            }
        }
        // Only a shell-safe name reaches the command text (checked above).
        // trace:BUG-1624 | ai:claude
        let mut command = guard
            .command
            .replace("${{ github.event_name }}", "pull_request");
        if let Some(branch) = default_branch.as_deref() {
            command = command.replace(BASE_REF_PLACEHOLDER, branch);
        }
        let mut child = github_actions_bash(&command);
        child
            .current_dir(project_root)
            .env("PATH", &path)
            .env("AIDA_PREFLIGHT_BINARY", &binary_path);
        match default_branch.as_deref() {
            Some(branch) => child.env("AIDA_PREFLIGHT_DEFAULT_BRANCH", branch),
            None => child.env_remove("AIDA_PREFLIGHT_DEFAULT_BRANCH"),
        };
        results.push(match run_bounded(&mut child, timeout) {
            Ok(Some(out)) if out.status.success() => GuardResult::Passed(guard.name),
            Ok(Some(out)) => GuardResult::Failed {
                name: guard.name,
                output: format!("binary: {binary_label}\n{}", output_text(&out)),
            },
            // NOT Skipped: this guard was identified and asked to run.
            // trace:TASK-1289 | ai:claude
            Ok(None) => GuardResult::Inconclusive {
                name: guard.name,
                reason: format!(
                    "timed out after {}s; binary: {binary_label}",
                    timeout.as_secs()
                ),
            },
            Err(err) => GuardResult::Inconclusive {
                name: guard.name,
                reason: format!("could not start ({err}); binary: {binary_label}"),
            },
        });
    }
    results
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
    /// Was `guard_timeout_skips_instead_of_refusing`, which asserted that a
    /// guard timing out still published. That is the defect finding 2 named:
    /// a guard made to time out became a guard made to succeed, so any
    /// slow-enough failure published itself. The contract is now inverted —
    /// an identified guard that cannot finish is INCONCLUSIVE and refuses.
    // trace:TASK-1289 | ai:claude
    #[test]
    fn guard_timeout_is_inconclusive_and_refuses() {
        let root = tempfile::tempdir().unwrap();
        let results = execute(
            root.path(),
            vec![ResolvedGuard::Found(Guard {
                name: "Hung guard".into(),
                command: "sleep 5".into(),
            })],
            Duration::from_millis(50),
        );
        let GuardResult::Inconclusive { name, reason } = &results[0] else {
            panic!(
                "a guard that timed out has not passed, got {:?}",
                results[0]
            )
        };
        assert_eq!(name, "Hung guard");
        assert!(reason.contains("timed out"), "reason was: {reason}");
        assert!(reason.contains("binary:"), "reason was: {reason}");
        // the decision, not just the classification — this is what publishes
        let PreflightDecision::Refuse { failed } = decide(&results) else {
            panic!("an inconclusive guard must refuse publication")
        };
        assert_eq!(failed[0].0, "Hung guard");
        assert!(
            failed[0].1.contains("did not complete"),
            "the refusal must say the guard never answered: {}",
            failed[0].1
        );
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

    /// BUG-1624: a default branch name is forge/remote data. One carrying
    /// shell syntax (`;`, `$()`, backticks, a quote breakout) must never be
    /// spliced into the `bash -c` guard text: the guard that needs it is
    /// inconclusive (so publication refuses) and nothing runs. The marker
    /// file each payload would create lives inside the scratch repo.
    // trace:BUG-1624 | ai:claude
    #[test]
    fn bug_1624_hostile_default_branch_runs_nothing() {
        for payload in [
            "main;>injected",
            "main$(>injected)",
            "main`>injected`",
            "x';>injected;'",
            "x\";>injected;\"",
        ] {
            let root = tempfile::tempdir().unwrap();
            assert!(Command::new("git")
                .args(["init", "-q"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success());
            let target = format!("refs/remotes/origin/{payload}");
            assert!(
                Command::new("git")
                    .args(["symbolic-ref", "refs/remotes/origin/HEAD", &target])
                    .current_dir(root.path())
                    .status()
                    .unwrap()
                    .success(),
                "git accepts {payload:?} as a ref name"
            );
            assert_eq!(crate::forge::default_branch_of(root.path()), payload);
            let results = execute(
                root.path(),
                vec![
                    ResolvedGuard::Found(Guard {
                        name: "Quoted base".into(),
                        command: "test '${{ github.base_ref }}' = trunk".into(),
                    }),
                    ResolvedGuard::Found(Guard {
                        name: "Bare base".into(),
                        command: "git diff origin/${{ github.base_ref }} >/dev/null 2>&1 || true"
                            .into(),
                    }),
                ],
                Duration::from_secs(5),
            );
            assert_eq!(results.len(), 2, "{payload:?}: {results:?}");
            for result in &results {
                let GuardResult::Inconclusive { reason, .. } = result else {
                    panic!("{payload:?}: expected inconclusive, got {result:?}");
                };
                assert!(reason.contains("not a shell-safe"), "{reason}");
            }
            assert!(
                matches!(decide(&results), PreflightDecision::Refuse { .. }),
                "{payload:?}: an unsubstituted guard must refuse publication"
            );
            assert!(
                !root.path().join("injected").exists(),
                "{payload:?}: the default branch name was run as shell code"
            );
        }
    }

    /// BUG-1624: a hostile default branch is not exported to guards either,
    /// and a guard that never mentions it still runs.
    // trace:BUG-1624 | ai:claude
    #[test]
    fn bug_1624_hostile_default_branch_is_not_exported() {
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
                "refs/remotes/origin/main;>injected",
            ])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());
        let results = execute(
            root.path(),
            vec![ResolvedGuard::Found(Guard {
                name: "No base".into(),
                command: "test -z \"${AIDA_PREFLIGHT_DEFAULT_BRANCH:-}\"".into(),
            })],
            Duration::from_secs(5),
        );
        assert_eq!(results, vec![GuardResult::Passed("No base".into())]);
        assert!(!root.path().join("injected").exists());
    }

    /// Finding 3: a guard set where nothing executes must not trigger the
    /// worktree build. The Skipped note is the observable proof — it can only
    /// read "not built" while the build memo was never forced, so restoring an
    /// eager build fails this test rather than merely slowing it down.
    // trace:TASK-1289 | ai:claude
    #[test]
    fn clean_guard_set_runs_no_build() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        // a VALID workflow that simply does not define the requested guard
        std::fs::write(
            root.path().join(".github/workflows/ci.yml"),
            "jobs:\n  build:\n    defaults:\n      run:\n        shell: bash --noprofile --norc -e -o pipefail {0}\n    steps:\n      - name: Something else\n        run: true\n",
        )
        .unwrap();

        let results = execute(
            root.path(),
            guards_from_ci(root.path(), &["absent-guard".into()]),
            Duration::from_secs(2),
        );

        assert_eq!(results.len(), 1);
        let GuardResult::Skipped(note) = &results[0] else {
            panic!("an absent guard must skip, got {:?}", results[0])
        };
        assert!(
            note.contains("not built (no guard required it)"),
            "no guard executed, so no build may have run; note was: {note}"
        );
        // and skipping still publishes — the acceptance criterion for unknown guards
        assert_eq!(decide(&results), PreflightDecision::Open);
    }

    /// Finding 5: the criterion has three branches, and the previous test
    /// exercised only the missing-workflow one.
    // trace:TASK-1289 | ai:claude
    #[test]
    fn guard_selection_reads_config_defaults_match_ci_and_unknown_steps_surface_a_note() {
        // branch 1 — [preflight].guards is actually read
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[preflight]\nguards = [\"Alpha\", \"Beta\"]\n",
        )
        .unwrap();
        assert_eq!(
            configured_guard_names(root.path()),
            vec!["Alpha".to_string(), "Beta".to_string()],
            "configured guards must come from [preflight].guards, not the defaults"
        );

        // branch 2 — every default names a real step in THIS repo's CI
        let repo = workspace_root();
        if repo.join(".github/workflows/ci.yml").exists() {
            let resolved = guards_from_ci(&repo, &configured_guard_names(&repo));
            for (name, guard) in DEFAULT_GUARD_NAMES.iter().zip(resolved.iter()) {
                assert!(
                    matches!(guard, ResolvedGuard::Found(_)),
                    "default guard `{name}` does not match a step in ci.yml — \
                     the defaults have drifted from the CI guard set: {guard:?}"
                );
            }
        }

        // branch 3 — an unknown guard in a VALID workflow skips with a note
        // that names it (distinct from the missing-workflow wording)
        let valid = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(valid.path().join(".github/workflows")).unwrap();
        std::fs::write(
            valid.path().join(".github/workflows/ci.yml"),
            "jobs:\n  build:\n    defaults:\n      run:\n        shell: bash --noprofile --norc -e -o pipefail {0}\n    steps:\n      - name: Real step\n        run: true\n",
        )
        .unwrap();
        let resolved = guards_from_ci(valid.path(), &["ghost".into()]);
        assert_eq!(
            resolved,
            vec![ResolvedGuard::Skipped("ghost: no matching CI guard".into())],
            "an unknown guard in a valid workflow must surface why it skipped"
        );
    }

    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("aida-cli-lib always has a workspace parent")
            .to_path_buf()
    }

    /// Finding 4: the previous coverage used a synthetic `echo`/`exit 1` step,
    /// which proves the plumbing and nothing about the guard the criterion
    /// names. This builds a real git repository carrying THIS repo's actual
    /// `scripts/check-portability.sh`, rules, allowlist and `ci.yml`, gives it
    /// a real `origin/main` (the ratchet diffs against a base ref), plants a
    /// genuine violation, and runs the real CI command through the real
    /// preflight — then asserts publication is refused AND that the offending
    /// file survives into the refusal, since a refusal nobody can act on is
    /// only marginally better than no refusal.
    // trace:TASK-1289 | ai:claude
    #[test]
    fn portability_ratchet_violation_refuses_with_the_real_ci_command() {
        const GUARD: &str = "Check Rust test portability ratchet";
        let src = workspace_root();
        // The fixture is only meaningful against the real assets.
        for asset in [
            "scripts/check-portability.sh",
            "scripts/portability-allowlist.txt",
            "scripts/portability-rules.json",
            ".github/workflows/ci.yml",
        ] {
            assert!(
                src.join(asset).exists(),
                "fixture needs the real {asset}; the guard cannot be exercised without it"
            );
        }

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let bare = tmp.path().join("origin.git");
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
        std::fs::create_dir_all(repo.join("tests")).unwrap();

        let mut copied_growth = false;
        for asset in [
            "scripts/check-portability.sh",
            "scripts/check-portability-growth.py",
            "scripts/portability-allowlist.txt",
            "scripts/portability-rules.json",
            ".github/workflows/ci.yml",
        ] {
            let from = src.join(asset);
            if !from.exists() {
                continue;
            }
            if asset.ends_with("growth.py") {
                copied_growth = true;
            }
            std::fs::copy(&from, repo.join(asset)).unwrap();
        }
        let _ = copied_growth;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                repo.join("scripts/check-portability.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let git = |args: &[&str], cwd: &Path| {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                .output()
                .unwrap_or_else(|e| panic!("git {args:?} could not start: {e}"));
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        git(
            &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
            tmp.path(),
        );
        git(&["init", "-b", "main"], &repo);
        git(&["add", "-A"], &repo);
        git(&["commit", "-m", "base"], &repo);
        git(&["remote", "add", "origin", bare.to_str().unwrap()], &repo);
        git(&["push", "-u", "origin", "main"], &repo);

        // The violation is a temp-dir literal in test code that is not in the
        // allowlist. It is assembled from two fragments on purpose: this file
        // is itself Rust test code, so writing the literal inline makes THIS
        // file a violation and the fixture for the ratchet trips the ratchet.
        // (Confirmed the hard way — the first version failed the real guard.)
        let planted = format!(
            "#[test]\nfn t() {{\n    let p = \"{}{}\";\n    assert!(!p.is_empty());\n}}\n",
            "/t", "mp/hardcoded-path"
        );
        std::fs::write(repo.join("tests/planted.rs"), &planted).unwrap();
        git(&["add", "-A"], &repo);
        git(&["commit", "-m", "plant a portability violation"], &repo);

        std::fs::create_dir_all(repo.join(".aida")).unwrap();
        std::fs::write(
            repo.join(".aida/config.toml"),
            format!("[preflight]\nguards = [\"{GUARD}\"]\n"),
        )
        .unwrap();

        let results = run(&repo);
        let PreflightDecision::Refuse { failed } = decide(&results) else {
            panic!("a real portability violation must refuse publication, got {results:?}")
        };
        assert_eq!(
            failed[0].0, GUARD,
            "the real guard must be the one refusing"
        );
        assert!(
            failed[0].1.contains("tests/planted.rs"),
            "the refusal must name the offending file so it can be fixed; got:\n{}",
            failed[0].1
        );
    }

    /// Finding 1: the retraction is only as good as what it tells the person
    /// who finds the closed PR.
    // trace:TASK-1289 | ai:claude
    #[test]
    fn retraction_notice_carries_the_guard_failure_and_the_next_step() {
        let detail = "guard `Check formatting` failed:\nsrc/x.rs needs rustfmt";
        let note = retraction_notice(detail);
        assert!(
            note.contains(detail),
            "the guard output is the whole reason for the closure: {note}"
        );
        assert!(
            note.contains("Closed automatically"),
            "a human must not mistake this for someone closing their PR: {note}"
        );
        assert!(
            note.contains("branch is untouched"),
            "closing a PR looks like losing work unless it says otherwise: {note}"
        );
        assert!(
            note.contains("reopen"),
            "the notice must name the way forward: {note}"
        );
    }
}
