/// BUG-1468: a PR branch's green check is evidence about the guards that
/// existed WHEN IT RAN. Nothing re-evaluates that green when a new required
/// guard lands on main, so a long-lived branch can present a green check
/// that no longer means what a reader assumes (observed on PR #1979 —
/// `aida-core/templates/.aida/discipline/session-discipline.md` grew a
/// content check 15h after the branch's own CI ran, and the branch still
/// showed green).
///
/// TWO TIERS (BUG-1468 follow-up — the single-tier version refused almost
/// every ship in this repo, because nearly every commit on main touches
/// `*/tests/` or `*_tests.rs`, making `--override-stale-check` routine):
/// - `definition_files`: a `.github/workflows/*` file whose `on:` triggers
///   include `pull_request` (a plain substring check on the file's content
///   at `base_ref` — nightly/cron-only, release-only, and dispatch-only
///   workflows don't gate a PR's own check, so they do NOT count), or a
///   `scripts/` file one of THOSE PR-triggered workflows invokes directly —
///   the check's own DEFINITION changed, so the green no longer means what
///   it looks like. `aida pr ship` REFUSES on this tier (override-able); the
///   drain's merge phase only WARNS and proceeds (BUG-1468 follow-up 2).
/// - `test_files`: WARN-only, everywhere. An ordinary test file changed on
///   base since divergence — the common, usually-harmless "base moved" case;
///   still worth surfacing (a reader may want to re-run), never worth
///   refusing.
// trace:BUG-1468 | ai:claude
pub(crate) struct StaleCheckWarning {
    pub(crate) behind_commits: u64,
    pub(crate) definition_files: Vec<String>,
    pub(crate) test_files: Vec<String>,
}

/// Pure classifier for the two tiers above. `workflow_is_pr_triggered`
/// decides whether a changed `.github/workflows/...` file's `on:` triggers
/// include `pull_request` (a non-PR-triggered workflow — cron/nightly,
/// release, manual dispatch — does NOT count as a definition change at
/// all: it lands in neither tier). `script_is_pr_workflow_invoked` decides
/// whether a `scripts/...` path is one a PR-triggered workflow calls
/// directly. Callers resolve both via git on `base_ref` (tests fake them
/// directly, no git needed).
// trace:BUG-1468 | ai:claude
pub(crate) fn classify_changed_files(
    changed_files: &[String],
    workflow_is_pr_triggered: &dyn Fn(&str) -> bool,
    script_is_pr_workflow_invoked: &dyn Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut definition = Vec::new();
    let mut test_only = Vec::new();
    for f in changed_files {
        if is_workflow_path(f) {
            if workflow_is_pr_triggered(f) {
                definition.push(f.clone());
            }
            // else: a nightly/cron/dispatch/release-only workflow — its
            // green never covered this PR's check to begin with, so it
            // doesn't count in either tier.
        } else if is_script_path(f) && script_is_pr_workflow_invoked(f) {
            definition.push(f.clone());
        } else if is_test_path(f) {
            test_only.push(f.clone());
        }
    }
    (definition, test_only)
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_workflow_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/") || lower.contains("/.github/workflows/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_script_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("scripts/") || lower.contains("/scripts/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.sh")
}

/// True when a `.github/workflows/...` file's content, AT `base_ref`,
/// mentions `pull_request` — a plain substring check (no YAML parsing),
/// standing in for "this workflow's `on:` triggers include `pull_request`"
/// (BUG-1468 follow-up 2). A nightly/cron, release, or manual-dispatch-only
/// workflow never gates a PR's own check, so it must not count as a
/// definition change even though it lives under `.github/workflows/`.
/// False on any git error (fail-open — a git hiccup demotes a definition
/// change to "doesn't count" rather than blocking a ship on its own).
// trace:BUG-1468 | ai:claude
pub(crate) fn workflow_is_pr_triggered(
    repo: &std::path::Path,
    base_ref: &str,
    workflow_path: &str,
) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{base_ref}:{workflow_path}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("pull_request"))
        .unwrap_or(false)
}

/// The `.github/workflows/*` files, AT `base_ref`, whose content mentions
/// `pull_request` — the PR-triggered subset a `scripts/` change must be
/// invoked by to count as a definition change. Empty on any git error
/// (fail-open — see [`workflow_is_pr_triggered`]).
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_triggered_workflow_files(repo: &std::path::Path, base_ref: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "grep",
            "-l",
            "-F",
            "pull_request",
            base_ref,
            "--",
            ".github/workflows",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    // `git grep -l <rev> -- <pathspec>` prints "<rev>:<path>" per match.
    let prefix = format!("{base_ref}:");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix(prefix.as_str()).map(|p| p.to_string()))
        .collect()
}

/// True when `script_path` (a `scripts/...` file that changed) is invoked
/// directly by one of the PR-TRIGGERED workflows on `base_ref` — a literal
/// substring match of the path inside those tracked `.github/workflows/*`
/// blobs. A workflow that invokes the same script but isn't itself
/// PR-triggered (nightly, release, dispatch-only) does not count (BUG-1468
/// follow-up 2). False on any git error (fail-open).
// trace:BUG-1468 | ai:claude
pub(crate) fn script_referenced_by_pr_workflows(
    repo: &std::path::Path,
    base_ref: &str,
    script_path: &str,
) -> bool {
    pr_triggered_workflow_files(repo, base_ref)
        .iter()
        .any(|wf| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["show", &format!("{base_ref}:{wf}")])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(script_path))
                .unwrap_or(false)
        })
}

/// Resolve the ref to measure `branch`'s staleness FROM: `origin/<branch>`
/// when that remote-tracking ref exists (what a PR's own CI actually ran
/// against), else the local `branch` ref itself (BUG-1468 follow-up 3 — a
/// local checkout can be ahead or behind what's actually pushed/reviewed).
// trace:BUG-1468 | ai:claude
pub(crate) fn stale_check_branch_ref(repo: &std::path::Path, branch: &str) -> String {
    let remote_ref = format!("origin/{branch}");
    let exists = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &remote_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        remote_ref
    } else {
        branch.to_string()
    }
}

/// git-IO wrapper: how far `branch` is behind `base_ref`, and the two-tier
/// classification (see [`StaleCheckWarning`]) of what changed on `base_ref`
/// since divergence. Staleness is measured from `origin/<branch>` when that
/// ref exists, else the local `branch` ref (see [`stale_check_branch_ref`]).
/// `None` when the branch is not behind base (nothing to warn about) or on
/// a git error — same fail-open convention as [`branch_behind_main`], so a
/// git hiccup never blocks a ship.
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_stale_check_warning(
    repo: &std::path::Path,
    branch: &str,
    base_ref: &str,
) -> Option<StaleCheckWarning> {
    let branch_ref = stale_check_branch_ref(repo, branch);
    let count_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args([
            "rev-list",
            "--count",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{branch_ref}..{base_ref}"),
        ])
        .output()
        .ok()?;
    if !count_out.status.success() {
        return None;
    }
    let behind_commits: u64 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .ok()?;
    if behind_commits == 0 {
        return None;
    }
    let diff_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "--name-only",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{branch_ref}...{base_ref}"),
            "--",
        ])
        .output()
        .ok()?;
    if !diff_out.status.success() {
        return None;
    }
    let changed: Vec<String> = String::from_utf8_lossy(&diff_out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    let (definition_files, test_files) = classify_changed_files(
        &changed,
        &|workflow_path| workflow_is_pr_triggered(repo, base_ref, workflow_path),
        &|script_path| script_referenced_by_pr_workflows(repo, base_ref, script_path),
    );
    Some(StaleCheckWarning {
        behind_commits,
        definition_files,
        test_files,
    })
}

#[cfg(test)]
#[path = "tests/bug_1468_stale_check_tests.rs"]
mod bug_1468_stale_check_tests;

/// TASK-233: `--watch-ci` — stream live CI progress via `gh run watch`
/// rather than silently polling, then re-probe for the terminal state so
/// the end-session decision tree (green proceeds / red prompts) runs
/// exactly as it does for `--wait-ci`. Falls back to the silent poll
/// loop when `gh` is missing or no run id resolves. trace:TASK-233
// trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_terminal(project_root: Option<&std::path::Path>, branch: &str) -> CiProbe {
    let Some(gh) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return wait_for_ci_terminal(project_root, branch);
    };
    // Resolve the latest workflow run id for this branch.
    let run_id = std::process::Command::new(&gh)
        .args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "databaseId",
        ])
        .output_retrying_etxtbsy()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| first_run_id_from_gh_json(&String::from_utf8_lossy(&o.stdout)));
    let Some(run_id) = run_id else {
        // No resolvable run — degrade to the silent poll loop rather
        // than failing the session-end.
        return wait_for_ci_terminal(project_root, branch);
    };
    eprintln!(
        "  {} streaming `gh run watch {}`",
        crate::glyph(crate::glyphs::Glyph::FlowActive).cyan(),
        run_id.dimmed()
    );
    // Inherit stdio so gh's live-updating display renders straight to
    // the user's terminal. `gh run watch` returns when the run reaches
    // a terminal state.
    let _ = std::process::Command::new(&gh)
        .args(["run", "watch", &run_id])
        .status_retrying_etxtbsy();
    // Re-probe to classify Green/Red for the end-session decision tree.
    // Keep the re-probe on the caller-injected root. Re-deriving from the
    // process cwd can silently route a GitLab branch through GitHub when the
    // driven worktree is not the agent's cwd. Only the `None` arm — where the
    // caller supplied no root — keeps the historical cwd discovery.
    // trace:TASK-1273 | ai:claude
    let probe_root = project_root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| find_project_root().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let forge_kind = crate::forge::resolve_forge_kind(&probe_root);
    ci_probe_with_forge(&probe_root, forge_kind, branch) // STORY-516: forge-routed
}

/// BUG-1288: the wall-clock ceiling a single forge-CLI subprocess (`gh`/`glab`)
/// may run before [`command_output_with_timeout`] gives up on it. Measured
/// cause of this spec's `aida status --full` / `aida awaiting --json` stall:
/// one `gh api .../branches/main/protection` call took 10.5s in this
/// repository while every other `gh` call in the same run finished in under a
/// second — `gh` itself has no request-timeout flag, so an occasional slow
/// endpoint or rate-limit backoff can otherwise consume the whole machine-
/// readable budget on ONE subprocess.
// trace:BUG-1288 | ai:claude
pub(crate) const FORGE_CLI_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Review follow-up (BUG-1288): on unix, kill the CHILD'S WHOLE PROCESS
/// GROUP rather than only the direct child. `command_output_with_timeout`
/// spawns with `process_group(0)` below, which makes the child's own pid its
/// process group id, so any grandchild it forks (a credential-manager
/// helper, a background `git` op) inherits that same group — `killpg` reaps
/// the group in one signal instead of leaving a grandchild alive to hold the
/// stdout/stderr pipe write ends open past the timeout. `pid` must be a pid
/// this process spawned with `process_group(0)` (the only caller), which is
/// what makes it both safe to signal and a valid process-group id.
// trace:BUG-1288 | ai:claude
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    // SAFETY: `pid` is this process's own child (see the doc comment above),
    // and `libc::killpg` is a plain signal-delivery syscall — no pointers,
    // no aliasing concerns.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

// trace:TASK-1535 | ai:codex
pub(crate) enum BoundedCommandOutput {
    Completed(std::process::Output),
    SpawnFailed,
    TimedOut,
}

pub(crate) fn command_output_with_timeout_detail(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> BoundedCommandOutput {
    use std::io::Read;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // BUG-1735: a bare spawn here turns a transient ETXTBSY into
        // `SpawnFailed`, which discards the errno and reads as "could not
        // start". Every bounded-run caller inherits that, so the retry
        // belongs in the wrapper, not at each call site.
        // trace:BUG-1735 | ai:claude
        .spawn_retrying_etxtbsy()
    {
        Ok(child) => child,
        Err(_) => return BoundedCommandOutput::SpawnFailed,
    };
    // Only used to target `killpg` below; on non-unix targets nothing reads
    // it, so it is cfg-gated too rather than left as a dead binding.
    #[cfg(unix)]
    let pid = child.id();
    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = stdout_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = stderr_tx.send(buf);
    });
    let start = std::time::Instant::now();
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    killed = true;
                    #[cfg(unix)]
                    kill_process_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // BUG-1288 fix-up: a `try_wait` error leaves the child un-reaped
            // exactly like the timeout branch above — kill and wait it here
            // too, or it leaks as an orphan/zombie every time this arm is
            // hit instead of only on the timeout path.
            // trace:BUG-1288 | ai:claude
            Err(_) => {
                killed = true;
                #[cfg(unix)]
                kill_process_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // Bounded on the kill path (short — the group is already dead or dying,
    // this is only a safety net for a descendant that escaped it), generous
    // on the normal-exit path (the child already closed its own pipe ends;
    // this bound exists so a hypothetical stuck reader still can't hang the
    // caller forever, not because it is expected to be hit).
    let read_wait = if killed {
        std::time::Duration::from_millis(500)
    } else {
        std::time::Duration::from_secs(30)
    };
    let stdout = stdout_rx.recv_timeout(read_wait).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(read_wait).unwrap_or_default();
    match status {
        Some(status) => BoundedCommandOutput::Completed(std::process::Output {
            status,
            stdout,
            stderr,
        }),
        None => BoundedCommandOutput::TimedOut,
    }
}

/// BUG-1594: the message a forge lookup carries when the scoped
/// [`with_forge_lookup_timeout`] ceiling killed it. Rendered by `aida show`
/// as "timed out" rather than a generic unreachable so the output says
/// exactly why the PR/MR state is unknown (PRIN-5).
// trace:BUG-1594 | ai:claude
pub(crate) const FORGE_LOOKUP_TIMED_OUT: &str = "forge lookup timed out";

thread_local! {
    // trace:BUG-1594 | ai:claude
    pub(crate) static FORGE_LOOKUP_TIMEOUT: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// BUG-1594: run `f` with every open-change forge lookup it makes (`gh pr
/// list` / `glab api`) bounded by `timeout`. Scoped rather than global so
/// interactive read surfaces (`aida show`) get a hard ceiling while the
/// orchestrator's phase probes keep their existing unbounded semantics.
// trace:BUG-1594 | ai:claude
pub(crate) fn with_forge_lookup_timeout<T>(
    timeout: std::time::Duration,
    f: impl FnOnce() -> T,
) -> T {
    let prev = FORGE_LOOKUP_TIMEOUT.with(|c| c.replace(Some(timeout)));
    struct Restore(Option<std::time::Duration>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0;
            FORGE_LOOKUP_TIMEOUT.with(|c| c.set(prev));
        }
    }
    let _restore = Restore(prev);
    f()
}

/// BUG-1594: spawn a forge-CLI lookup honoring the scoped
/// [`with_forge_lookup_timeout`] ceiling. `Ok(None)` means the call was
/// killed at the ceiling; `Err` is a spawn failure; with no scoped ceiling
/// this is exactly `Command::output()`.
// trace:BUG-1594 | ai:claude
pub(crate) fn forge_lookup_output(
    mut cmd: std::process::Command,
) -> std::io::Result<Option<std::process::Output>> {
    match FORGE_LOOKUP_TIMEOUT.with(|c| c.get()) {
        // trace:BUG-1735 | ai:claude
        None => cmd.output_retrying_etxtbsy().map(Some),
        Some(timeout) => {
            let started = std::time::Instant::now();
            match command_output_with_timeout(cmd, timeout) {
                Some(out) => Ok(Some(out)),
                None if started.elapsed() >= timeout => Ok(None),
                None => Err(std::io::Error::other("could not spawn the forge CLI")),
            }
        }
    }
}

/// Resolve the `glab` (GitLab CLI) binary, mirroring `resolve_gh_binary`'s
/// PATH-walk + sanity-spawn (BUG-74/79). Wired into the forge call sites in
/// follow-on STORY-621 slices. trace:STORY-621 | ai:claude
#[allow(dead_code)] // wired into call sites in follow-on STORY-621 slices
pub(crate) fn resolve_glab_binary() -> Option<std::path::PathBuf> {
    resolve_forge_binary("glab", "AIDA_TEST_GLAB_BINARY", "AIDA_DEBUG_GLAB")
}

#[cfg(test)]
#[path = "tests/forge_binary_resolution_tests.rs"]
mod forge_binary_resolution_tests;

#[cfg(all(test, unix))]
mod story1163_forge_dispatch_tests {
    use super::*;
    use tempfile::TempDir;

    fn gitlab_project() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let aida = tmp.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(aida.join("config.toml"), "[forge]\nprovider = \"gitlab\"\n").unwrap();
        tmp
    }

    #[test]
    fn detect_open_pr_for_branch_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 printf '%s\\n' '[{{\"iid\":7,\"web_url\":\"https://gitlab.example.com/g/p/-/merge_requests/7\",\"source_branch\":\"feature/x\",\"target_branch\":\"main\",\"title\":\"GitLab MR\"}}]'\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        let found = match detect_open_pr_for_branch(project.path(), "feature/x") {
            PrLookup::Found(pr) => pr,
            _ => panic!("expected GitLab MR lookup"),
        };

        assert_eq!(found.number, 7);
        assert_eq!(found.head_branch.as_deref(), Some("feature/x"));
        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("api -X GET projects/:id/merge_requests"));
        assert!(logged.contains("-f source_branch=feature/x"));
        assert!(logged.contains("-f state=opened"));
    }

    #[test]
    fn diff_change_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 exit 0\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        crate::forge::forge_for_kind(project.path(), crate::forge::ForgeKind::GitLab)
            .diff_change(7)
            .expect("GitLab diff should dispatch through glab");

        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("mr diff 7"));
    }
}

// ============================================================================
// TASK-96 — plan Followups extraction. When a spec reaches Done (`aida queue
// done`) or Completed (the STORY-86 auto-bump), parse the `## Followups`
// section of any matching docs/plans/ file and offer to file each bullet as
// a child TASK so out-of-scope items don't get forgotten.
// ============================================================================

// BUG-656: BUG-655's per-parent `(parent, title)` dedup was not enough. The
// auto-followups path keys child TASKs to the *completing spec*, so the same
// plan's bullets get re-filed when a DIFFERENT spec completes — either because
// the plan's header lists several specs (each "owns" the plan and re-extracts
// it) or because the same followup text was copied into a sibling plan. On
// EPIC-0428 this re-filed ~14 cross-plan duplicate TASKs (TASK-1019-1032). The
// stable signature is the PLAN PATH, not the completing spec: a plan's followups
// must be extracted exactly once across the whole store. The marker comment
// already records `extracted from <relative plan paths>`; we parse that back out
// of every spec's comments to build the already-extracted set, then skip any
// plan already in it. A re-completion, a sibling-spec completion, or a slice PR
// landing the plan twice all become no-ops at the plan granularity.

/// Best-effort load of the requirements store for read-only spec-id /
/// title / status lookups inside session-manifest rendering. Tries
/// distributed git-canonical mode first (the default), then falls back
/// to the legacy YAML/SQLite path. Returns None on any error — caller
/// renders with "(not found)" placeholders so missing data degrades
/// gracefully.
///
/// Pure with respect to `project_root`: BOTH resolution modes read only stores
/// reachable from that root, so this can never return another project's
/// requirements. No process CWD, no `REQ_DB_NAME`, no registry default — see
/// the legacy arm's comment for what went wrong when it did (BUG-1732).
// trace:STORY-98 trace:BUG-1732 | ai:claude
pub(crate) fn load_store_for_lookup(
    project_root: &std::path::Path,
) -> Option<aida_core::RequirementsStore> {
    // Git-canonical: walk up from project_root looking for an .aida/config.toml.
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        if let Ok(backend) = aida_core::GitBackend::new(&store_path) {
            if let Ok(s) = aida_core::DatabaseBackend::load(&backend) {
                return Some(s);
            }
        }
    }
    // Legacy fallback for projects that haven't migrated. BUG-1732: this used
    // `determine_requirements_path(None)`, which probes for `requirements.db` /
    // `requirements.yaml` relative to the PROCESS CWD and honours an ambient
    // `REQ_DB_NAME` — both of which ignore the `project_root` every one of this
    // function's ~70 callers went to the trouble of resolving. On a host where
    // the cwd happens to hold an unrelated legacy store, a project-scoped
    // lookup silently returned that other project's requirements, which is how
    // `aida awaiting` came to report six In-Progress specs from a different
    // project in its orphaned-in-progress channel. BUG-1574's author already
    // documented this hazard and hand-bypassed the fallback for one call site
    // (see `unattended_git_mutation_refusal`); this makes the function itself
    // pure with respect to its argument so no caller has to remember.
    //
    // `resolve_requirements_path_in` is reused rather than reimplemented so the
    // probe ORDER (`requirements.db` before `requirements.yaml`) stays defined
    // in one place. Passing `project_root` as its cwd and `None` for both the
    // `-p` option and `REQ_DB_NAME` leaves no ambient input: on that arm the
    // resolver returns before it ever reads the registry, so the empty registry
    // path below is inert and is passed to make that unreadability explicit
    // rather than to imply the registry participates.
    // trace:BUG-1732 | ai:claude
    aida_core::resolve_requirements_path_in(project_root, None, None, std::path::Path::new(""))
        .ok()
        .map(Storage::new)
        .and_then(|s| s.load().ok())
}

// ───────────────────────────────────────────────────────────────────
// STORY-79 — background fetch from statusline (auto-freshness)
// ───────────────────────────────────────────────────────────────────

/// Pure source-label resolver for `aida whoami`'s user-id line. Mirrors
/// `current_user_id`'s resolution order (`AIDA_USER` → `USER` → `USERNAME` →
/// default) over already-read env values, so the printed source label can never
/// drift from the value `current_user_id` actually returned. Each arg is the
/// env var's value as `Option`, where an empty string counts as unset (matching
/// the `is_ok_and(|v| !v.is_empty())` checks the resolver uses).
/// trace:TASK-784
#[cfg(test)]
#[path = "tests/statusline_tests.rs"]
mod statusline_tests;

// trace:TASK-1479 | ai:claude
#[cfg(test)]
#[path = "tests/statusline_client_adapters_integration_tests.rs"]
mod statusline_client_adapters_integration_tests;

#[cfg(test)]
#[path = "tests/bug_88_pr_lookup_parse_tests.rs"]
mod bug_88_pr_lookup_parse_tests;

/// BUG-257: `gh_stderr_is_network_error` classifies a `gh` stderr line as a
/// transient network error vs. an auth/parse/other failure. The split powers
/// the `PrLookup::GhUnreachable` variant so the orchestrator can report
/// *Inconclusive* (drain pauses) instead of conflating "no PR" with "can't
/// reach the API". trace:BUG-257 | ai:claude
#[cfg(test)]
#[path = "tests/bug_257_gh_stderr_network_classifier_tests.rs"]
mod bug_257_gh_stderr_network_classifier_tests;

/// BUG-266: `claude_log_indicates_api_outage` classifies the headless
/// implementer's JSONL log as evidence the upstream Anthropic API took the
/// session out (a transient outage), rather than the implementer attempting
/// the work and failing. The orchestrator flips that phase from *failed* to
/// *Inconclusive* (the BUG-257 path), so a 529 / 5xx / stream-timeout no
/// longer marks a spec failed-by-implementer. trace:BUG-266 | ai:claude
#[cfg(test)]
#[path = "tests/bug_266_anthropic_api_outage_classifier_tests.rs"]
mod bug_266_anthropic_api_outage_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_354_text_question_classifier_tests.rs"]
mod bug_354_text_question_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_87_queue_filter_tests.rs"]
mod bug_87_queue_filter_tests;

/// BUG-231: `aida findings promote` flipped status to Approved and printed
/// "joins the work queue" but never called `queue add` — the finding ended
/// up Approved and in no queue (a silent half-success). These cover the
/// fixed `queue_promoted_finding` helper: it routes to a real queue, honors
/// `--for`, and surfaces a queue-add failure as an error.
#[cfg(test)]
#[path = "tests/bug_231_findings_promote_tests.rs"]
mod bug_231_findings_promote_tests;
#[cfg(test)]
#[path = "tests/cr_8_filing_provenance_tests.rs"]
mod cr_8_filing_provenance_tests;
#[cfg(test)]
#[path = "tests/story_1428_gate_promote_tests.rs"]
mod story_1428_gate_promote_tests;

/// BUG-574: reliability papercuts — spurious non-zero exits on the idempotent
/// re-entry paths of `aida session start` (re-running for a scope this clone
/// already leases) and `aida pr ship` (re-running on an already-merged PR).
/// Both are benign no-ops that completed, so they must exit 0 — reserving
/// non-zero for real failures — exactly like the BUG-573 read-only contract.
/// These tests pin the pure decision points that drive the exit-0 behavior.
/// trace:BUG-574 | ai:claude
#[cfg(test)]
#[path = "tests/bug_574_spurious_exit_tests.rs"]
mod bug_574_spurious_exit_tests;

#[cfg(test)]
#[path = "tests/lease_enforcement_tests.rs"]
mod lease_enforcement_tests;

#[cfg(test)]
#[path = "tests/scope_fallback_tests.rs"]
mod scope_fallback_tests;

#[cfg(test)]
#[path = "tests/store_walkup_tests.rs"]
mod store_walkup_tests;

#[cfg(test)]
#[path = "tests/session_end_resolution_tests.rs"]
mod session_end_resolution_tests;

#[cfg(test)]
#[path = "tests/terminal_status_guard_tests.rs"]
mod terminal_status_guard_tests;

/// TASK-292 — `aida queue work --auto-complete` with no positional SPEC id
/// inherits the no-arg "pick the queue head" semantics. These cover the pure
/// pickup-order resolver: drivable-status classification, skipping in-flight
/// heads, and the empty-vs-all-in-flight error split.
#[cfg(test)]
#[path = "tests/auto_complete_head_tests.rs"]
mod auto_complete_head_tests;

/// TASK-293 — the `next` / `nextN` keyword parser. Covers every named form
/// in the acceptance criteria (`next`, `next1`, `next 1`, `next3`, `next 3`)
/// plus the malformed-input edge cases. The drain machinery itself
/// (empty-queue, N > queue-length) is the already-tested `drain_batch` /
/// `pick_auto_complete_head` — this module only covers the pure parse.
/// trace:TASK-293 | ai:claude
#[cfg(test)]
#[path = "tests/next_keyword_tests.rs"]
mod next_keyword_tests;

/// STORY-86 — env-flag opt-out + colorize_status coverage for the new
/// `Done` variant. Integration coverage (set up git+store, exercise the
/// helper end-to-end) lives in the verification script in the plan; the
/// unit slice here protects the bits that are pure functions.
/// trace:STORY-86 | ai:claude
#[cfg(test)]
#[path = "tests/auto_bump_done_tests.rs"]
mod auto_bump_done_tests;

/// BUG-95: regression coverage for the claim that `aida pull --code-only`
/// skips the Done → Completed auto-bump. The bug-as-filed asserted the
/// conditional was nested in the wrong branch; the current structure has
/// the auto-bump correctly inside the code-pull block (gated by
/// `!store_only`), so `--code-only` (code_only=true, store_only=false)
/// reaches it. This test exercises that path end-to-end via
/// `handle_pull_command` and verifies the bump fires.
/// trace:BUG-95 | ai:claude
#[cfg(test)]
#[path = "tests/handle_pull_command_tests.rs"]
mod handle_pull_command_tests;

/// TASK-106: coverage for the `aida push` / `aida pull` scope flags —
/// all four combinations (default / --code-only / --store-only / both)
/// at the clap-parse layer, the `AIDA_PUSH_DEFAULT` env-var override,
/// and the mutual-exclusion guard. trace:TASK-106 | ai:claude
#[cfg(test)]
#[path = "tests/push_pull_scope_tests.rs"]
mod push_pull_scope_tests;

/// TASK-234 / TASK-241: integration coverage for the git-state
/// classifiers behind `aida queue list`'s "Done — awaiting merge"
/// section and `aida show`'s git-linkage section. Both extracted
/// helpers (`classify_in_flight_specs`, `collect_git_linkage`) are
/// gh-free — they only run `git` — so a temp repo with hand-built
/// commits exercises every branch without a GitHub round-trip.
/// trace:TASK-234 TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/in_flight_linkage_integration_tests.rs"]
mod in_flight_linkage_integration_tests;

/// TASK-241: coverage for the squash-merge PR-number parser that
/// `aida show`'s git-linkage section uses to point shipped specs at
/// their merged PR. trace:TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/git_linkage_tests.rs"]
mod git_linkage_tests;

/// STORY-511: coverage for the forge-aware change-request (PR/MR) linkage
/// formatter that `aida show`'s git-linkage section renders. Fully
/// isolated — no git repo, no `gh`/`glab` on PATH, no I/O — the rendering
/// is a pure function of (ForgeKind, ChangeLinkageState). EPIC-35 slice 5.
/// trace:STORY-511 | ai:claude
#[cfg(test)]
#[path = "tests/change_linkage_format_tests.rs"]
mod change_linkage_format_tests;

/// TASK-1475 (CR-8 acceptance 6 follow-up): coverage for `filing_drift_hint`
/// — the filing-provenance staleness signal `aida show` / `aida why`
/// render. Pure git, no `gh`/`glab` on PATH.
// trace:TASK-1475 | ai:claude
#[cfg(test)]
#[path = "tests/task_1475_filing_drift_tests.rs"]
mod task_1475_filing_drift_tests;

/// TASK-238: coverage for the queue-list tag surfacing — the inline
/// chip formatter and the `--by-batch` group key. trace:TASK-238
#[cfg(test)]
#[path = "tests/queue_tag_tests.rs"]
mod queue_tag_tests;

/// TASK-107: regression coverage for `aida fetch`. The behaviors we
/// pin: (1) code-leg fetch updates origin/<branch> without touching the
/// worktree, (2) cache-invalidation hook fires after the store leg
/// (best-effort, off the real `~/.aida` since `AIDA_HOME` overrides it
/// for tests), (3) `--code-only` / `--store-only` route correctly,
/// (4) the auto-bump does NOT fire on fetch — that's a pull-only
/// behavior. trace:TASK-107 | ai:claude
#[cfg(test)]
#[path = "tests/handle_fetch_command_tests.rs"]
mod handle_fetch_command_tests;

#[cfg(test)]
#[path = "tests/derive_parent_epic_label_tests.rs"]
mod derive_parent_epic_label_tests;

#[cfg(test)]
#[path = "tests/format_review_story_display_tests.rs"]
mod format_review_story_display_tests;

#[cfg(test)]
#[path = "tests/auto_branch_tests.rs"]
mod auto_branch_tests;

#[cfg(test)]
#[path = "tests/worktree_dirty_entries_tests.rs"]
mod worktree_dirty_entries_tests;

#[cfg(test)]
#[path = "tests/add_aida_gitignore_entries_tests.rs"]
mod add_aida_gitignore_entries_tests;

#[cfg(test)]
#[path = "tests/recent_files_for_branch_tests.rs"]
mod recent_files_for_branch_tests;

#[cfg(test)]
#[path = "tests/branch_behind_main_tests.rs"]
mod branch_behind_main_tests;

#[cfg(all(test, unix))]
mod resolve_gh_binary_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // These tests run in exact single-test children. A shared writer mutex
    // alone cannot protect other tests' unlocked executable lookups.
    /// Prepend a directory only inside the isolated test child.
    // trace:BUG-1994 | ai:codex
    // trace:TASK-1532 | ai:agy
    fn scoped_prepend_path(dir: &std::path::Path) -> impl Drop {
        crate::test_env::EnvVarGuard::prepend_path(dir)
    }

    fn make_executable(path: &std::path::Path) {
        crate::test_exec::write_executable(path, "#!/bin/sh\necho gh fake\n");
    }

    fn make_non_executable(path: &std::path::Path) {
        std::fs::write(path, "not executable\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// PATH walk picks up gh even when the system gh isn't on PATH.
    /// Critical regression guard for BUG-74. trace:BUG-74 | ai:claude
    #[test]
    fn finds_gh_via_path() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary().expect("expected to find fake gh on PATH");
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&gh).unwrap()
        );
    }

    /// PATH walk ignores non-executable files named gh. trace:BUG-74
    #[test]
    fn rejects_non_executable() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_non_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // We may still pick up a real system gh via the absolute-path
        // fallback — but we should NOT have picked up our broken fake.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&gh).unwrap()
            );
        }
    }

    #[test]
    fn is_executable_checks_perm_bit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("x");
        make_executable(&path);
        assert!(is_executable(&path));
        make_non_executable(&path);
        assert!(!is_executable(&path));
    }

    #[test]
    fn is_executable_rejects_dirs() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(tmp.path()));
    }

    #[test]
    fn is_executable_rejects_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(&tmp.path().join("nope")));
    }

    /// BUG-79: a file that passes is_executable but whose spawn fails
    /// (broken interpreter line) is not returned — we fall back to the
    /// next candidate or return None. trace:BUG-79 | ai:claude
    #[test]
    fn rejects_is_executable_but_spawn_fails() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("gh");
        // Shebang points at a non-existent interpreter, so spawn errors
        // with ENOENT even though is_executable returns true.
        crate::test_exec::write_executable(
            &bad,
            "#!/this/interpreter/does/not/exist\necho should never run\n",
        );
        assert!(is_executable(&bad));

        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // Either None (no other gh) or some other path — never the broken one.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&bad).unwrap()
            );
        }
    }

    /// BUG-79: when PATH has a broken candidate followed by a working one,
    /// the working one wins. trace:BUG-79 | ai:claude
    #[test]
    fn falls_back_past_broken_to_working() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let broken_dir = TempDir::new().unwrap();
        let good_dir = TempDir::new().unwrap();

        let broken = broken_dir.path().join("gh");
        crate::test_exec::write_executable(&broken, "#!/this/does/not/exist\n");

        let good = good_dir.path().join("gh");
        make_executable(&good);

        // Prepend BOTH dirs so the broken one is hit first, then the good
        // one. Order matters: broken_dir wins on lookup order.
        let combined = std::path::PathBuf::from(format!(
            "{}:{}",
            broken_dir.path().display(),
            good_dir.path().display()
        ));
        let _g = scoped_prepend_path(&combined);
        let resolved = resolve_gh_binary().expect("should fall back to the working gh");
        drop(_g);
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&good).unwrap()
        );
    }
}

#[cfg(test)]
#[path = "tests/session_root_resolution_tests.rs"]
mod session_root_resolution_tests;

/// Apply the user's `--color=auto|always|never` choice to the colored
/// crate's global override. `auto` is the colored crate's default
/// behavior — it uses `isatty(stdout)` and respects `NO_COLOR`.
/// trace:FR-1-041 | ai:claude
pub(crate) fn apply_color_mode(mode: &str) {
    match mode {
        "always" => colored::control::set_override(true),
        "never" => colored::control::set_override(false),
        _ => {} // "auto" — let colored crate decide via tty detection
    }
}

/// Count queue entries in the user's queue file that pass the role filter.
/// When `role` is `Some`, returns the count of entries with `for_role`
/// matching exactly. When `role` is `None`, returns the total entry
/// count (no role filter).
///
/// Reads `<project>/.aida-store/registry/queues/<user>.yaml` directly —
/// keeps statusline off the heavier Storage::load() path so the sub-50ms
/// budget holds even when the orphan store has hundreds of objects.
/// Returns `None` if the file is missing or unreadable.
/// trace:FR-1-041 | ai:claude
/// TASK-648 (ADR-3): non-archived, non-deferred draft count from the cache,
/// read-only and fast (same SQLite-without-Cache::open pattern as the
/// statusline freshness probe — no migration, no write lock on the prompt hot
/// path). The advisor's statusline shows this as the active draft backlog to
/// triage. Returns 0 when the cache is absent or unreadable rather than
/// erroring the prompt.
// trace:BUG-1171 | ai:codex
pub(crate) fn read_draft_backlog_depth(cache_path: &std::path::Path) -> usize {
    if !cache_path.exists() {
        return 0;
    }
    let conn = match rusqlite::Connection::open_with_flags(
        cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let has_deferred_column = conn
        .prepare("SELECT deferred FROM requirements_cache LIMIT 0")
        .is_ok();
    let sql = if has_deferred_column {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft' AND deferred = 0"
    } else {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft'"
    };
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

/// Count URGENT unread mailbox messages for the current shell user, for the
/// statusline nag. Returns `None` when there is no mailbox to count (stay
/// silent), `Some(0)` when caught up.
///
/// This MUST agree with `aida mailbox inbox` — the statusline count and the read
/// command are two surfaces of one "unread" state, so reading the inbox has to
/// drive the count to 0. The divergence that left a high count stuck forever was
/// that this read only the LOCAL `.aida/mailbox/` layer while `aida mailbox
/// inbox` reads the MERGED (local + canonical orphan-store) set and advances each
/// identity's watermark to the MERGED per-identity newest. When the two message
/// sets differed, the watermark the inbox-read advanced could not clear a count
/// computed over a different set. The fix reads the SAME merged set and the same
/// identity union, so the surfaces compute "unread" over identical inputs and a
/// read clears the count. Reading the canonical layer is a plain directory read
/// of the already-checked-out `.aida-store/mailbox/` — pure file I/O, no git
/// spawn — so the statusline's no-git contract still holds.
// trace:STORY-539 trace:BUG-625 | ai:claude
pub(crate) fn read_urgent_unread_count(project_root: &std::path::Path) -> Option<usize> {
    let local = mailbox_store::read_local_messages(project_root).ok()?;
    // Merge in the canonical (orphan-store) layer so this sees exactly what
    // `aida mailbox inbox` reads (and advances watermarks against). Best-effort:
    // a missing/unreadable canonical dir degrades to the local layer rather than
    // silencing the nag. trace:BUG-625
    let store_root = project_root.join(".aida-store");
    let canonical = mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
    let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
    if merged.is_empty() {
        return None;
    }
    // Scope to the same identity union the agent-facing notice and the inbox-read
    // use (shell user + session role + agent type), so a role-/type-addressed
    // urgent message isn't invisible here while the notice surfaces it.
    // build_notice dedups a broadcast across identities and counts urgent across
    // the unread set. trace:STORY-585 trace:BUG-625
    let identities = inbox_identities();
    let watermarks = mailbox_store::read_all_watermarks(project_root).ok()?;
    let summary = aida_core::mailbox::build_notice(
        identities.iter().map(String::as_str),
        &merged,
        &watermarks,
        aida_core::mailbox::NOTICE_DEFAULT_CAP,
    );
    Some(summary.urgent)
}

// ─── STORY-127: Scope-B runtime anti-pattern detectors ──────────────────
//
// Each surface the user TYPES (`aida pull`, `git pull` (via the code leg),
// `aida dev release`, `aida session end`) detects when its precondition is
// not met and emits a one-line WARNING (never a block — the user/agent
// always proceeds). The detection is a single git/queue/lease query.
//
// The detection LOGIC is a set of pure, unit-tested predicates so the
// warning conditions are testable without process/git/forge I/O. The call
// sites do the cheap query and feed the result to the predicate.
// trace:STORY-127 | ai:claude

/// Detector (1) + (2): `aida pull` / `git pull` (code leg) no-op.
///
/// `pull_was_noop` — the code-leg `git pull --ff-only` advanced nothing
/// (local branch already at origin). When that coincides with the user
/// holding an active **reviewer** lease on an **unmerged** PR, the no-op is
/// almost certainly the PR-27-style mistake: running catch-up commands
/// BEFORE the merge has happened. Warn so the user waits for the merge.
///
/// When there is no such reviewer lease the no-op is unremarkable — a plain
/// one-line note still helps ("Already up to date — nothing to pull") but is
/// not the same alarm. The two callers distinguish via the second arg.
/// trace:STORY-127 | ai:claude
pub(crate) fn pull_noop_warning(
    pull_was_noop: bool,
    reviewer_unmerged_pr: Option<&str>,
) -> Option<String> {
    if !pull_was_noop {
        return None;
    }
    match reviewer_unmerged_pr {
        Some(pr) => Some(format!(
            "No new commits on origin. You hold a reviewer lease on {pr}, which is \
             not merged yet — did you mean to wait for {pr} to merge first? \
             (catch-up before the merge is a no-op)"
        )),
        None => Some("Already up to date — origin had no new commits.".to_string()),
    }
}

/// Detector (2): cheap "local main already at origin/main" check, expressed
/// as a pure predicate over the two SHAs so it is testable. `None` for
/// either SHA (couldn't resolve a ref) means "can't tell" → no warning.
/// trace:STORY-127 | ai:claude
pub(crate) fn local_main_already_at_origin(
    local_sha: Option<&str>,
    origin_sha: Option<&str>,
) -> bool {
    match (local_sha, origin_sha) {
        (Some(a), Some(b)) => !a.is_empty() && a == b,
        _ => false,
    }
}

/// Detector (3): `aida dev release` (→ `scripts/release.sh`) with unmerged
/// PRs. Returns a one-line warning naming every open PR — a release tag cut
/// now will NOT include their changes. Pure over the already-collected
/// open-PR numbers. trace:STORY-127 | ai:claude
pub(crate) fn release_unmerged_pr_warning(open_pr_numbers: &[u64]) -> Option<String> {
    if open_pr_numbers.is_empty() {
        return None;
    }
    let mut nums: Vec<u64> = open_pr_numbers.to_vec();
    nums.sort_unstable();
    nums.dedup();
    let list = nums
        .iter()
        .map(|n| format!("PR-{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let (verb, subj) = if nums.len() == 1 {
        ("is", "PR")
    } else {
        ("are", "PRs")
    };
    Some(format!(
        "{} open {} ({}) — the release {} not merged, so the tag will NOT include \
         their changes. Merge first if they belong in this release.",
        nums.len(),
        subj,
        list,
        verb,
    ))
}

/// Detector (4): `aida session end` while a DIFFERENT role has queued work
/// waiting. `ending_role` is the role of the lease being torn down;
/// `role_counts` is the per-role queue depth for the current user. Returns
/// the `(role, count)` pairs for every OTHER role with at least one waiting
/// item, sorted by role name for stable output. Pure over the inputs.
/// trace:STORY-127 | ai:claude
pub(crate) fn cross_role_queue_waiting(
    ending_role: Option<&str>,
    role_counts: &std::collections::HashMap<String, usize>,
) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = role_counts
        .iter()
        .filter(|(role, &count)| {
            count > 0 && ending_role.map(|er| er != role.as_str()).unwrap_or(true)
        })
        .map(|(role, &count)| (role.clone(), count))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Render the detector-(4) warning body from the cross-role pairs. `None`
/// when nothing is waiting elsewhere. trace:STORY-127 | ai:claude
pub(crate) fn session_end_cross_role_warning(waiting: &[(String, usize)]) -> Option<String> {
    if waiting.is_empty() {
        return None;
    }
    let list = waiting
        .iter()
        .map(|(role, count)| {
            format!(
                "{} has {} item{}",
                role,
                count,
                if *count == 1 { "" } else { "s" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "About to end this session, but another role's queue has work waiting \
         ({list}). Switch role first (`aida role enter <role>`) if you meant to \
         pick that up."
    ))
}

/// STORY-127: per-role queue depth for the current user. Reads the same
/// queue YAML `aida queue list` / `read_queue_depth` consult (keyed off
/// `current_user_id`). Returns an empty map on any read/parse failure —
/// the detectors degrade to silent. trace:STORY-127 | ai:claude
pub(crate) fn read_queue_role_counts(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let user = current_user_id(None);
    let queue_path = project_root
        .join(".aida-store/registry/queues")
        .join(format!("{}.yaml", user));
    let Ok(content) = std::fs::read_to_string(&queue_path) else {
        return counts;
    };
    let Ok(entries) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
        return counts;
    };
    for e in &entries {
        let role = e
            .get("for_role")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("implementer");
        *counts.entry(role.to_string()).or_insert(0) += 1;
    }
    counts
}

/// STORY-127: does the current user hold an active **reviewer** lease whose
/// scope is a PR (`PR-N` / `MR-N`) that is still open (unmerged)? Returns
/// the PR label for the warning, else `None`. Cheap: walks the local lease
/// set, then a single forge open-PR snapshot keyed by number. The PR is
/// "unmerged" iff its number appears in the open-PR snapshot.
/// trace:STORY-127 | ai:claude
pub(crate) fn active_reviewer_unmerged_pr(project_root: &std::path::Path) -> Option<String> {
    let leases = list_leases(project_root);
    // Reviewer leases scope to a PR/MR; the scope string is e.g. "PR-27".
    let reviewer_pr_scopes: Vec<String> = leases
        .iter()
        .filter(|l| {
            l.role
                .as_deref()
                .map(|r| canonical_role_name(r) == "reviewer")
                .unwrap_or(false)
        })
        .filter(|l| pr_number_from_scope(&l.scope).is_some())
        .map(|l| l.scope.clone())
        .collect();
    if reviewer_pr_scopes.is_empty() {
        return None;
    }
    let open_numbers: std::collections::HashSet<u64> = collect_open_prs(project_root)
        .by_branch
        .values()
        .map(|p| p.number)
        .collect();
    for scope in reviewer_pr_scopes {
        if let Some(n) = pr_number_from_scope(&scope) {
            if open_numbers.contains(&n) {
                return Some(format!("PR-{n}"));
            }
        }
    }
    None
}

/// Parse a PR/MR number out of a lease scope like `PR-27` / `MR-3` / `pr-27`.
/// trace:STORY-127 | ai:claude
pub(crate) fn pr_number_from_scope(scope: &str) -> Option<u64> {
    let s = scope.trim();
    let rest = s
        .strip_prefix("PR-")
        .or_else(|| s.strip_prefix("pr-"))
        .or_else(|| s.strip_prefix("MR-"))
        .or_else(|| s.strip_prefix("mr-"))?;
    rest.parse::<u64>().ok()
}

pub(crate) fn read_queue_depth(
    project_root: &std::path::Path,
    role: Option<&str>,
) -> Option<usize> {
    // BUG-89 + BUG-675: resolve identity through the SAME two-step path
    // `aida queue list` uses — `current_user_id` first, then the queue-file
    // case-fold (`resolve_queue_user`, TASK-951/TASK-845) — so a shell reporting
    // `Joe` counts the queue keyed under `joe.yaml` instead of reading zero.
    // Reading `<current_user_id>.yaml` directly skipped that fold, so the depth
    // diverged from the queue-list count on a case-only identity mismatch; the
    // BUG-670 band-aid then suppressed the divergent number instead of fixing the
    // resolution. trace:BUG-675 trace:BUG-89 | ai:claude
    let store_path = project_root.join(".aida-store");
    let user = aida_core::db::resolve_queue_user(&store_path, &current_user_id(None));

    let queue_path = store_path
        .join("registry/queues")
        .join(format!("{}.yaml", user));

    // An absent own-queue file is zero own items, NOT "unknown" — a peer may
    // still have routed work to this role, and bailing here would hide it.
    // A file that exists but won't parse stays `None` (a real read failure).
    // trace:BUG-774 | ai:claude
    let entries: Vec<serde_yaml::Value> = match std::fs::read_to_string(&queue_path) {
        Ok(content) if content.trim().is_empty() => Vec::new(),
        Ok(content) => serde_yaml::from_str(&content).ok()?,
        Err(_) => Vec::new(),
    };

    let count = match role {
        Some(want) => entries
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == want)
                    .unwrap_or(false)
            })
            .count(),
        None => entries.len(),
    };
    // A `--for <role>` routing written by a PEER lands in that peer's queue
    // file, so a depth read scoped to our own file under-counted the work
    // actually routed to us — the meter disagreed with `aida queue list`.
    // Add the sibling files' entries routed to this role. Read-only; the
    // stored keys are untouched and identity folds through the shared
    // canonical helper. trace:BUG-774 | ai:claude
    let cross_user = match role {
        Some(want) => cross_user_role_routed_depth(&store_path, &user, want),
        None => 0,
    };
    Some(count + cross_user)
}

/// Count entries routed to `role` that live in OTHER users' queue files.
/// Best-effort + allocation-light (the same raw-YAML read the own-file depth
/// uses); an unreadable sibling file contributes zero rather than failing the
/// meter.
// trace:BUG-774 | ai:claude
pub(crate) fn cross_user_role_routed_depth(
    store_path: &std::path::Path,
    self_user: &str,
    role: &str,
) -> usize {
    let dir = store_path.join("registry/queues");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let me = aida_core::node::canonical_user_id(self_user);
    let mut total = 0usize;
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if aida_core::node::canonical_user_id(stem) == me {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rows) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
            continue;
        };
        total += rows
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == role)
                    .unwrap_or(false)
            })
            .count();
    }
    total
}

/// TASK-647 (ADR-3): a status whose *production* requires advisor authority —
/// anything that places a spec into the active execution pipeline (Approved
/// and beyond). Draft / Rejected / NeedsAttention don't gate (they are not
/// "approved work" a non-advisor could smuggle past triage).
pub(crate) fn status_requires_advisor_authority(status: &RequirementStatus) -> bool {
    // TASK-739: single-sourced in the lifecycle model — the target half of the
    // advisor-authority predicate. trace:TASK-739 | ai:claude
    aida_core::lifecycle::target_requires_advisor_authority(
        aida_core::lifecycle::State::from_status(status),
    )
}

/// BUG-482: whether advancing a spec from `from` to `to` (via `aida edit
/// --status`) is an advisor-authority act keyed on the (source, target) pair.
///
/// A non-advisor may not lift an **un-triaged** (`Draft`) *or* a **punted**
/// (`NeedsAttention`) spec into the approved+ pipeline — both sources sit
/// awaiting the advisor's (or interactive human's) triage decision.
/// `forbidden_attention_transition` *permits* `NeedsAttention → Approved` as a
/// triage outcome, but *who* may make that call is still advisor authority;
/// before BUG-482 only `Draft` was a gated source, so a non-advisor could
/// self-re-approve a spec it (or the orchestrator) had just punted, bypassing
/// the triage the punt exists to request. Execution flips from a source
/// already in the pipeline (`Approved → InProgress → Done`) are NOT gated, so
/// drains are unaffected. trace:BUG-482 | ai:claude
pub(crate) fn status_advance_requires_advisor_authority(
    from: &RequirementStatus,
    to: &RequirementStatus,
) -> bool {
    // TASK-739: delegate to the lifecycle model's transition guard — the single
    // source for which (from → to) edits are advisor-authority acts. Defined
    // over the full domain (direct edits too, not only declared edges).
    // trace:TASK-739 | ai:claude
    use aida_core::lifecycle::{transition_guard, GuardKind, State};
    transition_guard(State::from_status(from), State::from_status(to))
        == GuardKind::RequiresAdvisorAuthority
}

/// BUG-1611: why `aida queue done` refuses a spec on lifecycle grounds.
/// `queue done` is an execution flip: its legal predecessors are the states a
/// spec reaches only after passing the approval gate (`Approved`, `Planned`,
/// `InProgress`, and an idempotent `Done`). Anything else is refused through
/// the existing lifecycle guard rather than a new rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueDoneLifecycleRefusal {
    /// `Draft` / `NeedsAttention`: never approved, or punted back for triage.
    /// The lifecycle guard marks `→ Done` from these as an advisor-authority
    /// act, and the caller does not hold that authority.
    NotApproved,
    /// `Rejected` / `Completed` / `Superseded`: closed. `queue done` is not a
    /// reopen verb, so it refuses whatever the caller's authority.
    Closed,
}

/// BUG-1611: the lifecycle/authority decision for `aida queue done`, shared by
/// the CLI handler and the MCP `queue_done` tool so the two surfaces cannot
/// drift. `has_advisor_authority` is the caller's resolved authority (CLI:
/// [`has_advisor_authority`]; MCP: the server's role). `None` = proceed.
/// `--force` deliberately has no say here.
// trace:BUG-1611 | ai:claude
pub(crate) fn queue_done_lifecycle_refusal(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> Option<QueueDoneLifecycleRefusal> {
    if is_terminal_status(from) {
        return Some(QueueDoneLifecycleRefusal::Closed);
    }
    if status_advance_requires_advisor_authority(from, &RequirementStatus::Done)
        && !has_advisor_authority
    {
        return Some(QueueDoneLifecycleRefusal::NotApproved);
    }
    None
}

/// BUG-1611: the operator-facing refusal line for
/// [`queue_done_lifecycle_refusal`]. Neutral guidance only: it names the
/// legitimate route (approval, or a reopen that is itself authority-gated) and
/// never suggests an environment-variable role override.
// trace:BUG-1611 | ai:claude
pub(crate) fn queue_done_lifecycle_refusal_message(
    display_id: &str,
    from: &RequirementStatus,
    refusal: QueueDoneLifecycleRefusal,
) -> String {
    match refusal {
        QueueDoneLifecycleRefusal::NotApproved => format!(
            "queue done refused: {display_id} is {from} and has not passed the approval \
             gate, so marking it done would skip triage. Ask an advisor to approve it \
             (`aida edit {display_id} --status approved`), then work it and mark it done."
        ),
        QueueDoneLifecycleRefusal::Closed => format!(
            "queue done refused: {display_id} is already {from} (closed), and marking it \
             done would reopen it. To redo the work, have an advisor reopen it \
             (`aida edit {display_id} --status approved --force`) or file a new spec."
        ),
    }
}

/// BUG-1611: whether `aida zen`'s autopilot approve-gate may flip this spec to
/// `Approved`. `[autopilot] approve = "auto"` is a policy knob in an
/// agent-writable config file, so it cannot stand in for approval authority:
/// the auto-approve runs the same (source → Approved) lifecycle guard every
/// other approval path runs.
// trace:BUG-1611 | ai:claude
pub(crate) fn zen_auto_approve_authorized(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> bool {
    !status_advance_requires_advisor_authority(from, &RequirementStatus::Approved)
        || has_advisor_authority
}

/// A manual `aida edit <epic> --status <X>` is forbidden — an epic's status is a
/// read-only rollup of its children. This generalizes the earlier "epics cannot
/// be promoted to Approved" rule to EVERY transition; `--force` is the recovery
/// escape.
// trace:BUG-626 trace:TASK-761 | ai:claude
pub(crate) fn manual_epic_status_edit_forbidden(req_type: &RequirementType, force: bool) -> bool {
    *req_type == RequirementType::Epic && !force
}

pub(crate) fn approval_forbidden_for_type(req_type: &RequirementType) -> bool {
    // trace:TASK-761 | ai:codex
    //
    // BUG-751: `Decision` is deliberately NOT in this set. A decision spec
    // (ADR) is stateful — its documented lifecycle is proposed / accepted /
    // superseded / deprecated — and `Approved` is the sanctioned "accepted"
    // state (Draft == proposed). Gating it left an ADR with no way out of
    // Draft, while existing ADRs already sat at Approved by convention.
    // trace:BUG-751 | ai:claude
    matches!(
        req_type,
        RequirementType::Vision
            | RequirementType::Epic
            | RequirementType::Principle
            | RequirementType::Constraint
            | RequirementType::Term
    )
}

// BUG-751: type-aware wrapper over `validate_status_input`. The ADR lifecycle
// verb `accepted` is not a status in the enum; for decision-class specs it is
// an input alias for `Approved` (approved == accepted for ADRs), so a recorded
// acceptance can be applied with either verb. For every other type — and every
// other input — this delegates to the canonical validator, except that a
// non-decision given `accepted` gets a refusal naming the correct verb.
// trace:BUG-751 | ai:claude
pub(crate) fn validate_status_input_for_type(
    raw: &str,
    req_type: &RequirementType,
) -> Result<&'static str, String> {
    if raw.trim().eq_ignore_ascii_case("accepted") {
        if *req_type == RequirementType::Decision {
            return Ok("Approved");
        }
        return Err(format!(
            "invalid status `{}` — `accepted` applies to decision specs (where it \
             records as `approved`); expected one of: {}",
            raw, VALID_STATUS_INPUTS
        ));
    }
    validate_status_input(raw)
}

/// TASK-130: resolve the `human_only` marker for a freshly-added spec from its
/// type plus the explicit `--human-only` / `--no-human-only` flags.
///
/// A Spike is, by definition, time-boxed research driven by operator judgment
/// (candidate selection, experimental design, rubric evaluation, report
/// writing) — not heads-down implementation. So a Spike defaults to
/// `human_only = true`, which the orchestrator's pre-pickup gate honors by
/// skipping it rather than handing it to an implementer agent that would have
/// to bail or fabricate work. Every other type defaults to `false`.
///
/// The flags always win, and they are mutually exclusive at the CLI layer
/// (`conflicts_with`), so at most one is set:
/// - `--human-only`     → `true`  (opt any type in)
/// - `--no-human-only`  → `false` (opt a Spike back out into auto-pickup)
/// - neither            → the per-type default above
///
/// trace:TASK-130 | ai:claude
pub(crate) fn resolve_human_only(
    req_type: &RequirementType,
    human_only: bool,
    no_human_only: bool,
) -> bool {
    if human_only {
        return true;
    }
    if no_human_only {
        return false;
    }
    matches!(req_type, RequirementType::Spike)
}

/// TASK-754: why `aida add --queue` would refuse to enqueue the freshly-filed
/// spec. Pure decision over the new spec's *final* status (post-intake-gate)
/// plus whether the intake gate downgraded the requested Approved to Draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueAtFilingRefusal {
    /// The advisor-authority gate downgraded Approved → Draft (AC5): a
    /// non-advisor / non-TTY session filed but cannot queue — same hard gate
    /// `aida queue add` enforces.
    Downgraded,
    /// The spec's status is simply not enqueueable (AC2): only Approved is
    /// groomable, matching `aida backlog groom`'s policy.
    NotApproved,
}

// trace:TASK-754 trace:BUG-631 | ai:claude
/// `None` ⇒ enqueue is allowed. `Some(reason)` ⇒ refuse with the matching
/// message. AC2 + AC5 collapse to one rule: the new spec must end up Approved,
/// and a downgrade (the authority gate having fired) is reported distinctly so
/// the operator knows to re-run as the advisor rather than just fix the status.
///
/// The `route` (the `--for` target, before defaulting) carves out the
/// request-for-review case. Routing a freshly-filed draft `--for advisor`
/// (or `--for human`/`--for reviewer`) is a REQUEST for review/triage, open to
/// any role — so it is never refused here regardless of status, mirroring the
/// standalone `aida queue add --for advisor`, which enqueues any non-terminal
/// status. Only execution-dispatch routes carry the Approved-only / authority
/// gate.
pub(crate) fn queue_at_filing_refusal(
    final_status: &RequirementStatus,
    intake_downgraded: bool,
    route: Option<&str>,
) -> Option<QueueAtFilingRefusal> {
    // BUG-631: a request-for-review route bypasses the dispatch gate entirely.
    if !for_target_requires_dispatch_authority(route) {
        return None;
    }
    if matches!(final_status, RequirementStatus::Approved) {
        return None;
    }
    if intake_downgraded {
        Some(QueueAtFilingRefusal::Downgraded)
    } else {
        Some(QueueAtFilingRefusal::NotApproved)
    }
}

impl OrchestratorBadge {
    pub(crate) fn build(phase: Option<u8>, no_human_mode: Option<&str>) -> Self {
        let phase = match phase {
            Some(n) => match auto_complete::Phase::from_index(n as i32) {
                Some(p) => format!("auto:{n}/6 {}", p.slug()),
                None => format!("auto:{n}/6"),
            },
            None => "auto:?/6".to_string(),
        };
        OrchestratorBadge {
            phase,
            no_human: no_human_mode.map(|m| format!("no-human:{m}")),
            pause: "pause-here",
        }
    }
}

// ============================================================================
// `aida plan verify` — lint a docs/plans/ file against the structured
// template (TASK-92). Reports drifted line refs, missing files, and absent
// required sections; exits non-zero on any hard failure so it can run as a
// pre-commit hook. trace:TASK-93 | ai:claude
// ============================================================================

// `aida skill lint` — the forward-guard for skills that reference an
// implementation plan. A skill under `.claude/skills/` that points at a
// `docs/plans/*.md` file can rot when that plan drifts or is removed; this
// scans each skill for plan references, runs the same checks `aida plan
// verify` runs on each referenced plan (drifted refs / missing files / absent
// sections), and raw-glyph-checks the skill body. Read-only; never rewrites a
// plan or a skill. trace:TASK-927 | ai:claude

// ============================================================================
// TASK-94 — `aida plan helpers <spec>`. Derive a "Reusable helpers" section
// by walking the requirement graph (sibling / same-feature / same-tag specs)
// and harvesting those specs' `// trace:` comments from the codebase, so the
// implementer reuses existing helpers instead of re-inventing them.
// trace:TASK-94 | ai:claude
// ============================================================================

/// TASK-241: surface git linkage for a spec inside `aida show` — the
/// commits that reference it (AIDA commit format puts `(SPEC-ID)` in
/// the message), the files carrying its `trace:` comments, the branch +
/// worktree the work lives on, and PR state. Every sub-probe is
/// best-effort: a missing `gh`, an absent `origin/main`, or zero
/// matches degrades to a dim note rather than failing the show.
/// `--no-git` skips the whole section; `--verbose` un-caps the commit
/// list and adds per-commit diff stats. trace:TASK-241 | ai:claude
/// Resolve a refname for the default branch that is **known to exist**
/// in `project_root`, so callers can pass it to `git merge-base
/// --is-ancestor` without risking a `fatal: Not a valid object name <X>`
/// stderr splice when the repo's default branch is not `main`.
///
/// Resolution order:
///   1. `origin/HEAD` symbolic ref (the actual default branch on origin)
///   2. `init.defaultBranch` config — `origin/<name>` then bare `<name>`
///   3. The `"main"` / `"master"` literals — `origin/<name>` then bare
///
/// Returns `None` when no candidate ref exists; callers should then
/// skip the ancestor check rather than guess (the prior code passed
/// `"main"` as a literal, which leaked stderr on master-default repos).
/// trace:BUG-380 | ai:claude
pub(crate) fn resolve_default_branch_ref(project_root: &std::path::Path) -> Option<String> {
    use std::process::Command as PCmd;
    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
            .filter(|s| !s.is_empty())
    };

    if let Some(short) = git(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]) {
        return Some(short);
    }

    let mut candidates: Vec<String> = Vec::new();
    if let Some(cfg) = git(&["config", "--get", "init.defaultBranch"]) {
        candidates.push(cfg);
    }
    for lit in ["main", "master"] {
        if !candidates.iter().any(|c| c == lit) {
            candidates.push(lit.to_string());
        }
    }

    for name in candidates {
        let origin = format!("origin/{}", name);
        if git(&["rev-parse", "--verify", "--quiet", &origin]).is_some() {
            return Some(origin);
        }
        if git(&["rev-parse", "--verify", "--quiet", &name]).is_some() {
            return Some(name);
        }
    }
    None
}

// ============================================================================
// TASK-113 — `aida ultraplan <SPEC>`. Assemble a rich, structured planning
// prompt from a spec's full context (description, acceptance, related specs,
// the AIDA plan template, trace-graph reusable helpers) so planners anchor on
// concrete requirements instead of a terse user prompt.
// trace:TASK-113 | ai:claude
// ============================================================================

/// The AIDA plan-template section list, inlined into the planning prompt so
/// the returned plan already matches `docs/plans/_TEMPLATE.md` (TASK-92) and
/// `aida plan verify` (TASK-93) passes on the saved plan.
pub(crate) const ULTRAPLAN_STRUCTURE: &str = "\
Produce the plan in AIDA's structured format — a header line with Date / \
Specs / Status / Complexity, then these sections in order:

1. **Approach** — one-paragraph executive summary, plus an ASCII diagram \
where a state machine or control flow would compress prose.
2. **Decisions** — each significant choice resolved, with rationale (not an \
open-questions list).
3. **Files (in build-order)** — symbol-anchored edits per file, ordered so \
each commit builds clean.
4. **Critical Files** — flat list of every must-touch path.
5. **Reusable helpers** — existing helpers to call rather than re-invent \
(seeded below).
6. **Risks + gotchas** — numbered, each with a mitigation.
7. **Tests** — named test functions, not \"add tests\".
8. **Verification** — an executable bash smoke test (positive + negative).
9. **Followups** — out-of-scope items to file as TASKs at completion time.
10. **Related** — builds-on / blocks / see-also links.";

/// Does the project's working tree have uncommitted changes? Conservative: any
/// `git status --porcelain` output (excluding the gitignored store/cache noise
/// git already omits) counts as dirty. trace:STORY-659
pub(crate) fn working_tree_is_dirty(project_root: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_root)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}
