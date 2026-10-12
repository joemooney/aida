//! Upgrade and release: mode normalisation, SHA match classification, release/dev-release handlers, workspace version and install method (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Register this binary's identity (version, build SHA, filing agent vendor)
/// with the core filing-provenance capture, so every spec this process files
/// is stamped with the tooling that filed it. Uses the existing version /
/// build-SHA helpers; best-effort — an unknown vendor is simply omitted.
// trace:CR-8 | ai:claude
pub(crate) fn register_filing_identity() {
    let vendor = agent_registry::detect_agent_type();
    aida_core::provenance::register_tool_identity(aida_core::provenance::ToolIdentity {
        version: Some(current_version().to_string()),
        build_sha: build_sha_short(),
        vendor: (vendor != "other").then_some(vendor),
    });
}

/// True when the working directory is inside the aida source repo itself —
/// the one project where a spec's filing code SHA and the filing binary's
/// build SHA describe the same code, so a mismatch is meaningful.
// trace:CR-8 | ai:claude
pub(crate) fn cwd_is_aida_source_repo() -> bool {
    std::env::current_dir()
        .map(|cwd| {
            cwd.ancestors().any(|dir| {
                dir.join("aida-cli-lib").join("build.rs").is_file()
                    && dir.join("aida-core").join("Cargo.toml").is_file()
            })
        })
        .unwrap_or(false)
}

pub(crate) const ASCIINEMA_WRAPPED_ENV: &str = "AIDA_ASCIINEMA_WRAPPED";

// Keep generated cast names readable while preventing pathological command
// lines from becoming filesystem-hostile. Truncated slugs end in "-trunc".
// trace:STORY-423 | ai:codex
pub(crate) const ASCIINEMA_SLUG_MAX_CHARS: usize = 80;

// EPIC-28: default safety cap for `--auto-complete --batch` drains — after
// this many phase failures shelve in one batch the drain stops rather than
// keep parking innocent specs because the environment is broken. The user
// can override per-invocation with `--max-failures N`, and `--max-failures 0`
// turns the cap off (falls back to the historical "first failure stops"
// behaviour). trace:EPIC-28 | ai:claude
pub(crate) const DEFAULT_MAX_FAILURES: usize = 5;

// Requester intake must remain a standalone Draft. Both the CLI and MCP gates
// consume this list so relationship and grooming-field policy cannot drift.
// trace:BUG-1211 | ai:codex
pub(crate) const REQUESTER_INTAKE_FORBIDDEN_FIELDS: &[&str] = &["parent", "feature", "owner"];

// TASK-970: default row cap for a bare `aida list` in AGENT MODE. ~925
// unbounded rows is a token blowout when an agent reads the listing as
// context; cap to the N most-recent (post-sort, post-filter) and emit a
// `count: N of M` header + a widen hint. The human TTY path is unbounded
// (no surprise cap); an explicit `--limit`/`--all` always overrides.
// trace:TASK-970
// STORY-1644: raised 30 → 100 and made configurable (`[list] agent_limit`,
// env `AIDA_LIST_LIMIT`, 0 = unbounded); see `resolve_agent_list_limit`.
// trace:STORY-1644 | ai:claude
pub(crate) const AGENT_LIST_DEFAULT_LIMIT: usize = 100;

/// STORY-1644: the env var that overrides `[list] agent_limit`.
pub(crate) const AGENT_LIST_LIMIT_ENV: &str = "AIDA_LIST_LIMIT";

/// STORY-1644: the built-in `aida list` sort when neither `--sort` nor
/// `[list] default_sort` names one — natural numeric ID order.
pub(crate) const LIST_DEFAULT_SORT: &str = "id";

/// STORY-1644: every `--sort` / `[list] default_sort` value `aida list`
/// understands (`centrality` is a hidden alias of `heft`).
pub(crate) const LIST_SORT_VALUES: &[&str] =
    &["id", "modified", "created", "heft", "weight", "completed"];

// TASK-970: number of queued items the content-first bare `aida` (agent mode)
// lists beneath the status snapshot. trace:TASK-970
pub(crate) const AGENT_BARE_QUEUE_TOPN: usize = 5;

// trace:STORY-1028 | ai:codex
pub(crate) fn note_hidden_alias(old: &str, new: &str) {
    eprintln!("note: {old} is now {new}");
}

// trace:STORY-1028 | ai:codex
pub(crate) fn normalize_release_mode(
    patch: bool,
    minor: bool,
    major: bool,
    check: bool,
    cmd: Option<&ReleaseCommand>,
) -> (bool, bool, bool, bool) {
    match cmd {
        Some(ReleaseCommand::Patch) => (true, false, false, false),
        Some(ReleaseCommand::Minor) => (false, true, false, false),
        Some(ReleaseCommand::Major) => (false, false, true, false),
        Some(ReleaseCommand::Check) => (false, false, false, true),
        None => {
            if patch {
                note_hidden_alias(concat!("aida release ", "--patch"), "aida release patch");
            }
            if minor {
                note_hidden_alias(concat!("aida release ", "--minor"), "aida release minor");
            }
            if major {
                note_hidden_alias(concat!("aida release ", "--major"), "aida release major");
            }
            if check {
                note_hidden_alias(concat!("aida release ", "--check"), "aida release check");
            }
            (patch, minor, major, check)
        }
    }
}

// trace:STORY-1028 | ai:codex
pub(crate) fn normalize_upgrade_mode(
    check: bool,
    diff: bool,
    cmd: Option<&UpgradeCommand>,
) -> (bool, bool) {
    match cmd {
        Some(UpgradeCommand::Check) => (true, false),
        Some(UpgradeCommand::Diff) => (false, true),
        None => {
            if check {
                note_hidden_alias(concat!("aida upgrade ", "--check"), "aida upgrade check");
            }
            if diff {
                note_hidden_alias(concat!("aida upgrade ", "--diff"), "aida upgrade diff");
            }
            (check, diff)
        }
    }
}

// trace:STORY-1028 | ai:codex
#[allow(clippy::type_complexity)]
pub(crate) fn normalize_usage_mode<'a>(
    unused: Option<&'a str>,
    errors: bool,
    auto_complete: bool,
    failures: bool,
    pattern: bool,
    health: bool,
    slowest: bool,
    events: bool,
    action: Option<&'a UsageCommand>,
) -> (
    Option<&'a str>,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
) {
    match action {
        Some(UsageCommand::Rebuild { .. } | UsageCommand::Show { .. }) => {
            (None, false, false, false, false, false, false, false, false)
        }
        Some(UsageCommand::Slowest) => {
            (None, false, false, false, false, false, true, false, false)
        }
        Some(UsageCommand::Unused { duration }) => (
            Some(duration.as_str()),
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        ),
        Some(UsageCommand::Errors) => (None, true, false, false, false, false, false, false, false),
        Some(UsageCommand::Events) => (None, false, false, false, false, false, false, true, false),
        // TASK-1481: `aida usage timeline` — the compact one-line-per-invocation
        // view. Brand new surface (no legacy flag predates it), so it's
        // subcommand-only: no hidden `--timeline` alias to normalize.
        Some(UsageCommand::Timeline) => {
            (None, false, false, false, false, false, false, false, true)
        }
        Some(UsageCommand::Drains { failures, pattern }) => (
            None, false, true, *failures, *pattern, false, false, false, false,
        ),
        Some(UsageCommand::Health) => (None, false, false, false, false, true, false, false, false),
        None => {
            if slowest {
                note_hidden_alias(concat!("aida usage ", "--slowest"), "aida usage slowest");
            }
            if unused.is_some() {
                note_hidden_alias(concat!("aida usage ", "--unused"), "aida usage unused");
            }
            if errors {
                note_hidden_alias(concat!("aida usage ", "--errors"), "aida usage errors");
            }
            if events {
                note_hidden_alias(concat!("aida usage ", "--events"), "aida usage events");
            }
            if auto_complete {
                note_hidden_alias(
                    concat!("aida usage ", "--auto-complete"),
                    "aida usage drains",
                );
            }
            if health {
                note_hidden_alias(concat!("aida usage ", "--health"), "aida usage health");
            }
            (
                unused,
                errors,
                auto_complete,
                failures,
                pattern,
                health,
                slowest,
                events,
                false,
            )
        }
    }
}

/// TASK-1244 / ADR-41: map a merge-lease acquisition error to the drain's
/// typed failure. A `WouldBlock` (another live merger held the lease past the
/// bounded wait) is the shelvable `lease-conflict` cause — NeedsAttention for
/// triage, never retried as a transient. Any other IO fault (lock dir
/// unwritable, …) is a plain merge failure.
// trace:TASK-1244 | ai:claude
pub(crate) fn drain_merge_lease_failure(
    e: &std::io::Error,
    target: &str,
    pr: u32,
) -> auto_complete::PhaseFailure {
    if e.kind() == std::io::ErrorKind::WouldBlock {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::LeaseConflict,
            format!("another merger holds the '{target}' merge-lease while merging PR-{pr}: {e}"),
        )
    } else {
        auto_complete::PhaseFailure::new(format!(
            "could not take the '{target}' merge-lease for PR-{pr}: {e}"
        ))
    }
}

/// BUG-1316: distinguish the substrate's deliberate supervised-hold refusal
/// from a forge CLI failure. The typed error is emitted at the single
/// `Forge::merge_change` chokepoint and its display includes both the persisted
/// hold reason and exact clear command, so carry that evidence into the hint.
// trace:BUG-1316 trace:BUG-1435 trace:BUG-1447 | ai:codex
pub(crate) fn classify_drain_merge_failure(
    forge: crate::forge::ForgeKind,
    pr: u32,
    error: &anyhow::Error,
) -> auto_complete::PhaseFailure {
    let detail = format!("{error:#}");
    if error
        .downcast_ref::<crate::forge::MergeHoldRefusal>()
        .is_some()
    {
        let reason = format!(
            "{} merge refused for {}-{pr}: {detail}",
            forge
                .cli_name()
                .is_empty()
                .then_some("pure-git")
                .unwrap_or(forge.cli_name()),
            forge.change_noun(),
        );
        return auto_complete::PhaseFailure::of(auto_complete::FailureKind::MergeHold, &reason)
            .with_hint_override(format!("A human/advisor merge-hold is open: {detail}"));
    }
    // GitHub's forge-enforced branch protection is the remote half of the
    // same supervised hold. It arrives as CLI text rather than our local typed
    // `MergeHoldRefusal`, so promote its distinctive policy detail to the same
    // non-transient kind before generic tool-exit handling.
    if forge == crate::forge::ForgeKind::GitHub
        && auto_complete::is_branch_policy_merge_refusal(&detail)
    {
        return auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::MergeHold,
            format!("gh merge refused for PR-{pr}: {detail}"),
        )
        .with_hint_override(format!(
            "A human/advisor merge-hold is enforcing branch policy on PR-{pr}. \
             Clear it after review with `aida merge-hold clear {pr}`."
        ));
    }
    let merge_tool = match forge.cli_name() {
        "" => "pure-git",
        cli => cli,
    };
    auto_complete::PhaseFailure::new(format!(
        "{merge_tool} merge failed for {}-{pr}: {detail}",
        forge.change_noun()
    ))
}

#[cfg(test)]
mod bug_1316_merge_hold_failure_tests {
    use super::*;

    #[test]
    fn task_1293_pr_2003_hold_is_typed_and_real_tool_failure_stays_retryable() {
        let production = crate::forge::MergeHoldRefusal::new(2003, "advisor hold: PR interaction");
        assert_eq!(
            production.to_string(),
            "refusing to merge PR-2003: supervised merge-hold — advisor hold: PR interaction. \
             A human/advisor must review, then clear the hold (`aida merge-hold clear 2003`) before merging."
        );

        // Deliberately unlike the production sentence: classification follows
        // the error type and cannot decay when the human-facing prose changes.
        // trace:BUG-1435 | ai:codex
        let held = anyhow::Error::new(crate::forge::MergeHoldRefusal::with_detail(
            2003,
            "advisor hold: PR interaction",
            "aida merge-hold clear 2003",
            "PR-2003 remains gated for advisor hold: PR interaction; release it with \
             `aida merge-hold clear 2003` after review.",
        ));
        let failure = classify_drain_merge_failure(crate::forge::ForgeKind::GitHub, 2003, &held);
        assert_eq!(failure.kind, auto_complete::FailureKind::MergeHold);
        assert!(!auto_complete::is_transient_retry_cause(
            failure.kind.cause_slug()
        ));
        let hint = failure.hint_override.expect("hold-specific recovery hint");
        assert!(hint.contains("advisor hold: PR interaction"), "{hint}");
        assert!(hint.contains("aida merge-hold clear 2003"), "{hint}");

        let tool = anyhow::anyhow!("HTTP 502 from GitHub while merging");
        let failure = classify_drain_merge_failure(crate::forge::ForgeKind::GitHub, 2003, &tool);
        assert_eq!(failure.kind, auto_complete::FailureKind::Failed);
        assert!(auto_complete::is_transient_retry_cause(
            failure.kind.cause_slug()
        ));
    }

    /// BUG-1447: the exact forge refusal observed on PR-2014 is the remote
    /// supervised-hold shape, despite sharing "not mergeable" with conflicts.
    // trace:BUG-1447 | ai:codex
    #[test]
    fn pr_2014_branch_policy_refusal_is_typed_with_clear_hint() {
        let observed = anyhow::anyhow!(
            "gh pr merge failed for #2014: X Pull request joemooney/aida#2014 is not mergeable:\n\
             the base branch policy prohibits the merge."
        );
        let failure =
            classify_drain_merge_failure(crate::forge::ForgeKind::GitHub, 2014, &observed);
        assert_eq!(failure.kind, auto_complete::FailureKind::MergeHold);
        assert!(!auto_complete::is_transient_retry_cause(
            failure.kind.cause_slug()
        ));
        assert!(!auto_complete::is_merge_conflict_failure(&failure.reason));
        let hint = failure.hint_override.expect("hold-specific recovery hint");
        assert!(hint.contains("merge-hold"), "{hint}");
        assert!(hint.contains("aida merge-hold clear 2014"), "{hint}");
    }
}

#[cfg(test)]
mod task_1244_drain_merge_lease_tests {
    use super::*;

    #[test]
    fn would_block_maps_to_shelvable_lease_conflict() {
        let blocked = std::io::Error::new(std::io::ErrorKind::WouldBlock, "held by a live merger");
        let f = drain_merge_lease_failure(&blocked, "main", 1875);
        assert_eq!(f.kind, auto_complete::FailureKind::LeaseConflict);
        assert!(f.kind.is_shelvable());
        assert!(
            f.reason.contains("PR-1875") && f.reason.contains("'main'"),
            "{}",
            f.reason
        );
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "locks dir");
        let f = drain_merge_lease_failure(&io, "main", 1875);
        assert_ne!(f.kind, auto_complete::FailureKind::LeaseConflict);
    }

    /// Source-shape guard: both AIDA merge paths take the lease before they
    /// merge. Needles are split so this file cannot match its own literals.
    #[test]
    fn drain_merge_and_wave_merge_take_the_merge_lease() {
        let src = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs"),
            include_str!("burndown_run.rs")
        );
        let wave_start = src
            .find(concat!("fn merge_wave_pr", "(project_root"))
            .expect("wave fn");
        // Window widened for BUG-1562's typed spec on the wave hold.
        let wave_body = &src[wave_start..wave_start + 5000];
        let acquire = concat!("merge_lock::", "acquire(");
        let merge_call = concat!(".merge_change(", "&change_ref");
        let a = wave_body
            .find(acquire)
            .expect("wave merge acquires the lease");
        let m = wave_body.find(merge_call).expect("wave merge merges");
        assert!(a < m, "the wave lease must be taken BEFORE merge_change");
        let drain_start = src
            .find(concat!("\"aida queue work ", "(drain merge phase)\""))
            .expect("drain merge phase acquires the lease");
        let after = &src[drain_start..drain_start + 4000];
        assert!(
            after.contains(merge_call),
            "the drain merge_change follows the lease"
        );
    }
}

#[cfg(all(test, unix))]
mod bug_1265_finish_ci_tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: std::path::PathBuf,
        gh: std::path::PathBuf,
        mode: std::path::PathBuf,
        reads: std::path::PathBuf,
        calls: std::path::PathBuf,
    }

    /// Runs the production CI driver through the real drain orchestration
    /// boundary. Only the CI implementation is delegated: the remaining
    /// phases are unreachable because these regressions resume at phase 2 and
    /// stop at `ThroughCi`.
    // trace:BUG-1265 | ai:codex
    struct DrainHarness {
        real: RealPhaseDriver,
        finish_ci_calls: usize,
        shelf_calls: usize,
    }

    impl auto_complete::PhaseDriver for DrainHarness {
        fn run_implementer(
            &mut self,
        ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
            unreachable!("CI-resume harness must not run phase 1")
        }

        fn finish_ci(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            self.finish_ci_calls += 1;
            auto_complete::PhaseDriver::finish_ci(&mut self.real)
        }

        fn run_reviewer(
            &mut self,
        ) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must stop after phase 2")
        }

        fn merge(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must not merge")
        }

        fn pull(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must not pull")
        }

        fn build(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must not build")
        }

        fn hint_context(&self) -> auto_complete::HintContext {
            auto_complete::PhaseDriver::hint_context(&self.real)
        }

        fn transient_retry_budget(&self) -> usize {
            0
        }

        fn shelve_on_failure(
            &mut self,
            _spec: &str,
            phase: auto_complete::Phase,
            failure: &auto_complete::PhaseFailure,
            recovery_hint: &str,
        ) -> anyhow::Result<Option<aida_core::FailureReason>> {
            self.shelf_calls += 1;
            Ok(Some(aida_core::FailureReason {
                phase: phase.slug().to_string(),
                phase_index: phase.index() as u8,
                kind: failure.kind.cause_slug().to_string(),
                detail: failure.reason.clone(),
                recovery_hint: Some(recovery_hint.to_string()),
                shelved_by: None,
                shelved_at: chrono::Utc::now(),
            }))
        }
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("repo");
            std::fs::create_dir_all(&root).unwrap();
            let git = |args: &[&str]| {
                let out = std::process::Command::new("git")
                    .current_dir(&root)
                    .args(args)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "git {args:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            };
            git(&["init", "-q", "-b", "main"]);
            git(&[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/finish-ci-fixture.git",
            ]);

            let mode = temp.path().join("mode");
            let reads = temp.path().join("reads");
            let calls = temp.path().join("calls");
            let gh = temp.path().join("gh");
            let script = format!(
                r###"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
if [ "${{1:-}}" = "--version" ]; then echo 'gh version test'; exit 0; fi
if [ "${{1:-}} ${{2:-}}" = "pr view" ]; then
  printf '%s\n' '{{"state":"OPEN","title":"test","baseRefName":"main","headRefName":"bug-1265","headRefOid":"deadbeefdeadbeefdeadbeef","isCrossRepository":false,"isDraft":false}}'
  exit 0
fi
if [ "${{1:-}} ${{2:-}}" = "pr list" ]; then
  printf '%s\n' '[{{"number":1265,"statusCheckRollup":[{{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"FAILURE"}}]}}]'
  exit 0
fi
if [ "${{1:-}} ${{2:-}}" = "pr checks" ]; then
  mode=$(cat '{}')
  if [ "$mode" = unavailable ]; then
    if printf '%s' "$*" | grep -q -- '--required'; then
      printf '%s\n' '[{{"name":"merge-hold-gate","workflow":"merge-hold-gate","bucket":"fail"}},{{"name":"Build","workflow":"CI","bucket":"pass"}}]'
      exit 1
    fi
    n=0; test ! -f '{}' || n=$(cat '{}'); n=$((n+1)); echo "$n" > '{}'
    echo 'temporary rows failure' >&2
    exit 1
  fi
  if [ "$mode" = real ]; then
    printf '%s\n' '[{{"name":"merge-hold-gate","workflow":"merge-hold-gate","bucket":"fail"}},{{"name":"Build","workflow":"CI","bucket":"fail"}}]'
  else
    printf '%s\n' '[{{"name":"merge-hold-gate","workflow":"merge-hold-gate","bucket":"fail"}},{{"name":"Build","workflow":"CI","bucket":"pass"}}]'
  fi
  exit 1
fi
echo "unexpected gh call: $*" >&2
exit 2
"###,
                calls.display(),
                mode.display(),
                reads.display(),
                reads.display(),
                reads.display()
            );
            crate::test_exec::write_executable(&gh, script);
            merge_hold::write_hold(&root, 1265, "supervised test hold").unwrap();
            Self {
                _temp: temp,
                root,
                gh,
                mode,
                reads,
                calls,
            }
        }

        fn driver(&self) -> RealPhaseDriver {
            let mut driver = RealPhaseDriver::new(
                self.root.clone(),
                "BUG-1265".into(),
                "test".into(),
                None,
                true,
                None,
                AutonomyMode::Default,
                "test-token".into(),
                false,
                false,
                false,
                false,
                auto_complete::LifecycleSkip::none(),
                auto_complete::AutoCompleteVariant::ThroughCi,
            );
            driver.branch = Some("bug-1265".into());
            driver.from_pr = true; // an already-open PR needs no worktree push
            driver
        }

        fn set_mode(&self, mode: &str) {
            std::fs::write(&self.mode, mode).unwrap();
            let _ = std::fs::remove_file(&self.reads);
            let _ = std::fs::remove_file(&self.calls);
        }

        fn run_drain(&self, mode: &str) -> (auto_complete::OrchestrationResult, usize, usize) {
            self.set_mode(mode);
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            let path = std::env::join_paths(
                std::iter::once(self.gh.parent().unwrap().to_path_buf())
                    .chain(std::env::split_paths(&inherited)),
            )
            .unwrap();
            let gh = self.gh.to_string_lossy().into_owned();
            let path = path.to_string_lossy().into_owned();
            let _env = crate::test_env::EnvVarsGuard::set(&[
                ("AIDA_TEST_GH_BINARY", gh.as_str()),
                ("PATH", path.as_str()),
            ]);
            let mut harness = DrainHarness {
                real: self.driver(),
                finish_ci_calls: 0,
                shelf_calls: 0,
            };
            let result = auto_complete::orchestrate_with_resume(
                &mut harness,
                "BUG-1265",
                auto_complete::AutoCompleteVariant::ThroughCi,
                true,
                auto_complete::EscalateMode::Blocks,
                auto_complete::LifecycleSkip::none(),
                true,
                auto_complete::Phase::Ci,
            );
            (result, harness.finish_ci_calls, harness.shelf_calls)
        }
    }

    /// The fake `gh` must tolerate probes with fewer than two arguments and
    /// reach its normal fallback under `set -u` instead of aborting while
    /// expanding an unset positional parameter.
    //
    // `fixture.gh` is a script this test suite just wrote to disk, so the
    // spawn can race a concurrent fork that briefly still holds the file
    // open for writing (ETXTBSY) — the same class fixed at forge.rs and
    // pr_cmd.rs for production `gh` invocations. Production never hits this
    // because it launches an already-installed `gh`, never a freshly
    // written binary, so only the fixture side needs the retry. Route
    // through the same process_retry helper those sites use rather than
    // a bespoke loop.
    // trace:BUG-1544 | ai:claude
    // trace:BUG-1460 | ai:codex
    #[test]
    fn fake_gh_short_argv_reaches_fallback() {
        let fixture = Fixture::new();
        for args in [&[][..], &["pr"][..]] {
            let mut command = std::process::Command::new(&fixture.gh);
            command.args(args);
            let output = crate::test_exec::output(&mut command).unwrap();
            assert_eq!(output.status.code(), Some(2));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("unexpected gh call:"), "stderr: {stderr}");
            assert!(!stderr.contains("parameter not set"), "stderr: {stderr}");
        }
    }

    /// Exercise the real drain driver, not just `ci_gate::classify_red`: this
    /// is the seam that originally bypassed refinement and shelved the coarse
    /// hold-gate verdict as `ci-red`.
    // trace:BUG-1265 | ai:codex
    #[test]
    fn drain_finish_ci_completes_for_hold_gate_only_red() {
        let fixture = Fixture::new();
        assert!(
            merge_hold::read_hold(&fixture.root, 1265).is_some(),
            "the regression requires a real local supervised-hold marker"
        );
        let (result, finish_ci_calls, shelf_calls) = fixture.run_drain("hold");
        assert_eq!(
            finish_ci_calls, 1,
            "drain must execute production finish_ci"
        );
        assert_eq!(shelf_calls, 0, "the supervised hold is not a failure shelf");
        assert!(result.failed_phase.is_none(), "{result:?}");
        assert!(result.shelved_reason.is_none(), "{result:?}");
        assert_eq!(result.process_exit_code(), auto_complete::DRIVE_EXIT_CLEAN);
        assert!(
            merge_hold::read_hold(&fixture.root, 1265).is_some(),
            "finish_ci must not clear the supervised hold marker"
        );
    }

    // trace:BUG-1265 | ai:codex
    #[test]
    fn drain_finish_ci_shelves_real_red_naming_only_the_genuine_check() {
        let fixture = Fixture::new();
        let (result, finish_ci_calls, shelf_calls) = fixture.run_drain("real");
        assert_eq!(
            finish_ci_calls, 1,
            "drain must execute production finish_ci"
        );
        assert_eq!(shelf_calls, 1, "a genuine required red must shelf once");
        let shelf = result
            .shelved_reason
            .as_ref()
            .expect("genuine red must shelve");
        assert_eq!(result.failed_phase, Some(auto_complete::Phase::Ci));
        assert_eq!(
            result.process_exit_code(),
            auto_complete::DRIVE_EXIT_SHELVED
        );
        assert_eq!(shelf.kind, "ci-red");
        assert!(shelf.detail.contains("Build"), "{}", shelf.detail);
        assert!(
            !shelf.detail.contains("merge-hold-gate"),
            "{}",
            shelf.detail
        );
        let calls = std::fs::read_to_string(&fixture.calls).unwrap();
        assert!(
            calls
                .lines()
                .any(|call| call.contains("pr checks") && call.contains("--required")),
            "finish_ci must discover which red checks are genuinely required: {calls}"
        );
    }

    // trace:BUG-1265 | ai:codex
    #[test]
    fn drain_finish_ci_retries_unavailable_rows_then_shelves_ci_unavailable() {
        let fixture = Fixture::new();
        let (result, finish_ci_calls, shelf_calls) = fixture.run_drain("unavailable");
        assert_eq!(finish_ci_calls, 1, "row retries belong inside finish_ci");
        assert_eq!(
            shelf_calls, 1,
            "persistent row unavailability must shelf once"
        );
        let shelf = result
            .shelved_reason
            .as_ref()
            .expect("unavailable rows under a hold must shelve");
        assert_eq!(result.failed_phase, Some(auto_complete::Phase::Ci));
        assert_eq!(
            result.process_exit_code(),
            auto_complete::DRIVE_EXIT_SHELVED
        );
        assert_eq!(shelf.kind, "ci-unavailable");
        assert_ne!(shelf.kind, "ci-red");
        assert_eq!(
            std::fs::read_to_string(&fixture.reads).unwrap().trim(),
            "3",
            "initial row read plus two bounded retries"
        );
        let calls = std::fs::read_to_string(&fixture.calls).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|call| call.contains("pr checks") && !call.contains("--required"))
                .count(),
            3,
            "finish_ci must make the initial row read plus two bounded retries: {calls}"
        );
    }
}

#[cfg(test)]
mod bug_1205_ci_phase_fallthrough_tests {
    use super::*;

    #[test]
    fn headless_lease_conflict_is_decided_from_the_substrate() {
        assert_eq!(
            lease_conflict_decision(false, false),
            LeaseConflictDecision::Prompt
        );
        assert_eq!(
            lease_conflict_decision(false, true),
            LeaseConflictDecision::Prompt
        );
        assert_eq!(
            lease_conflict_decision(true, false),
            LeaseConflictDecision::AutoEnd
        );
        assert_eq!(
            lease_conflict_decision(true, true),
            LeaseConflictDecision::Refuse
        );
    }

    /// Source-shape guard: the CI phase's informational-only red must NOT
    /// return early — it falls through to the shared post-probe steps (ending
    /// the implementer session). Needles are split so this file cannot match
    /// its own literals.
    #[test]
    fn informational_red_falls_through_to_the_green_steps() {
        let src = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        // BUG-1265 added drain-level test harnesses that also `impl PhaseDriver`,
        // so searching the file for the first `fn finish_ci` now lands on a mock
        // whose body is a one-line delegation. Anchor on the REAL impl block
        // first, then find finish_ci inside it, so the guard keeps asserting the
        // production path no matter how many harnesses exist.
        // trace:BUG-1205 trace:BUG-1265 | ai:claude
        let real_impl = src
            .find(concat!(
                "impl auto_complete::PhaseDriver ",
                "for RealPhaseDriver {"
            ))
            .expect("real PhaseDriver impl present");
        let start = src[real_impl..]
            .find(concat!(
                "fn finish_ci",
                "(&mut self) -> Result<(), auto_complete::PhaseFailure>"
            ))
            .map(|i| real_impl + i)
            .expect("finish_ci present in the real impl");
        let end = src[start..]
            .find(concat!("    fn ", "run_reviewer("))
            .map(|e| start + e)
            .unwrap_or(src.len());
        let body = &src[start..end];
        let red_arm = body.find(concat!("CiProbe::", "Red {")).expect("Red arm");
        let not_failure = body[red_arm..]
            .find(concat!("is not a ", "failure"))
            .map(|i| red_arm + i)
            .expect("not-a-failure note");
        let window = &body[not_failure..not_failure + 400];
        assert!(
            !window.contains(concat!("return ", "Ok(())")),
            "early return is back"
        );
        assert!(body.contains(concat!("let refined_", "green")));
        assert!(body.contains(concat!("self.end_implementer_", "session()")));
    }
}

#[cfg(test)]
mod bug_1195_review_story_lookup_tests {
    use super::*;

    fn story(title: &str, status: aida_core::RequirementStatus) -> aida_core::Requirement {
        let mut r = aida_core::Requirement::new(title.to_string(), String::new());
        r.req_type = aida_core::RequirementType::Story;
        r.status = status;
        r.spec_id = Some("STORY-1191".to_string());
        r
    }
    fn entry(user: &str, id: uuid::Uuid) -> aida_core::QueueEntry {
        aida_core::QueueEntry {
            user_id: user.to_string(),
            requirement_id: id,
            position: 1000,
            added_by: user.to_string(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".to_string()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        }
    }

    // BUG-1195 / BUG-1193 shape: the review story was filed by a sibling
    // worktree session under `joe`, the drain looks it up as
    // `role:implementer`. The role-fallback listing hands the classifier the
    // foreign entry; it must be Found — not "no story".
    #[test]
    fn review_story_filed_by_another_user_is_found_through_the_role_route() {
        let s = story(
            "Review PR-1901: [AI:codex] fix(tests): normalize review envelope source guard (BUG-1193)",
            aida_core::RequirementStatus::Approved,
        );
        let store = RequirementsStore {
            requirements: vec![s.clone()],
            ..Default::default()
        };
        let foreign = vec![entry("joe", s.id)];
        assert_eq!(
            classify_review_story_lookup(&foreign, &store, ReviewForge::GitHub, 1901),
            ReviewStoryLookup::Found("STORY-1191".to_string())
        );
        assert!(matches!(
            classify_review_story_lookup(&foreign, &store, ReviewForge::GitHub, 1900),
            ReviewStoryLookup::NoTitleMatch { entries: 1 }
        ));
        assert_eq!(
            classify_review_story_lookup(&[], &store, ReviewForge::GitHub, 1901),
            ReviewStoryLookup::NoEntries
        );
        let done = story("Review PR-1901: x", aida_core::RequirementStatus::Done);
        let store = RequirementsStore {
            requirements: vec![done.clone()],
            ..Default::default()
        };
        match classify_review_story_lookup(
            &[entry("joe", done.id)],
            &store,
            ReviewForge::GitHub,
            1901,
        ) {
            ReviewStoryLookup::NotPickable { story, reason } => {
                assert_eq!(story, "STORY-1191");
                assert!(reason.contains("awaiting merge"), "{reason}");
            }
            other => panic!("expected NotPickable, got {other:?}"),
        }
        assert!(ReviewStoryLookup::Failed("boom".into())
            .describe()
            .contains("lookup failed"));
    }

    /// Source-shape guard: the lookup reads the queue through the role
    /// fallback (the plan-builder's view), never the bare per-user list.
    #[test]
    fn review_story_lookup_uses_the_role_fallback_listing() {
        let src = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs"),
            include_str!("pr_lookup.rs")
        );
        let start = src
            .find(concat!("fn queued_review_story_for_pr", "("))
            .unwrap();
        let body = &src[start..start + 1200];
        assert!(body.contains(concat!("queue_list_with_role_", "fallback(")));
        assert!(!body.contains(concat!("storage.queue_", "list(")));
    }
}

#[cfg(test)]
mod bug_1173_detection_hold_tests {
    // BUG-1236 (supersedes the BUG-1173 guard this module held): set_pr_number stamps the
    // marker AND mirrors the aida:merge-hold LABEL at PR-detection. The early
    // label is safe since BUG-1180 / ADR-39 (the CI watch and `aida pr ship`
    // classify a red merge-hold-gate as the hold when the marker exists), and
    // deferring it let three supervised PRs go unlabelled while the required
    // check read pass. A sync failure must be SURFACED (the `--fix` hint), never
    // swallowed. trace:BUG-1173 trace:BUG-1236 | ai:claude
    #[test]
    fn set_pr_number_stamps_marker_and_syncs_label_surfacing_failure() {
        let src = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs"),
            include_str!("phase_driver.rs")
        );
        // build the needle from split pieces so this test's own source cannot self-match
        // (the test module sits before the real function in the file).
        let needle = concat!("fn set_pr_number", "(&mut self, pr: u32) {");
        let start = src.find(needle).expect("set_pr_number present");
        // bound the body at the next 4-space-indented method so we read ONLY set_pr_number.
        let after = &src[start + 38..];
        let end = after
            .find("    fn ")
            .map(|e| start + 38 + e)
            .unwrap_or(src.len());
        let body = &src[start..end];
        assert!(
            body.contains("merge_hold::write_typed_hold")
                && body.contains("HoldReasonKind::Supervision"),
            "set_pr_number must stamp a typed supervision marker at PR-detection"
        );
        assert!(
            body.contains("merge_hold::sync_label"),
            "BUG-1236 regression: set_pr_number must mirror the aida:merge-hold LABEL at \
             PR-detection so the required merge-hold-gate check enforces Layer 2"
        );
        assert!(
            body.contains("merge-hold list --fix"),
            "a label sync failure must be surfaced with the repair hint, never swallowed"
        );
    }
}

#[cfg(test)]
mod story_1028_mode_alias_tests {
    use super::*;

    // trace:STORY-1028 | ai:codex
    #[test]
    fn release_hidden_flags_normalize_like_subcommands() {
        assert_eq!(
            normalize_release_mode(true, false, false, false, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Patch))
        );
        assert_eq!(
            normalize_release_mode(false, true, false, false, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Minor))
        );
        assert_eq!(
            normalize_release_mode(false, false, true, false, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Major))
        );
        assert_eq!(
            normalize_release_mode(false, false, false, true, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Check))
        );
    }

    // trace:STORY-1028 | ai:codex
    #[test]
    fn upgrade_hidden_flags_normalize_like_subcommands() {
        assert_eq!(
            normalize_upgrade_mode(true, false, None),
            normalize_upgrade_mode(false, false, Some(&UpgradeCommand::Check))
        );
        assert_eq!(
            normalize_upgrade_mode(false, true, None),
            normalize_upgrade_mode(false, false, Some(&UpgradeCommand::Diff))
        );
    }

    // trace:STORY-1028 | ai:codex
    #[test]
    fn usage_hidden_flags_normalize_like_subcommands() {
        assert_eq!(
            normalize_usage_mode(None, false, false, false, false, false, true, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Slowest)
            )
        );
        assert_eq!(
            normalize_usage_mode(
                Some("30d"),
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                None
            ),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Unused {
                    duration: "30d".to_string()
                })
            )
        );
        assert_eq!(
            normalize_usage_mode(None, true, false, false, false, false, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Errors)
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, false, false, false, false, false, true, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Events)
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, true, true, false, false, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Drains {
                    failures: true,
                    pattern: false
                })
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, true, false, true, false, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Drains {
                    failures: false,
                    pattern: true
                })
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, false, false, false, true, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Health)
            )
        );
        // TASK-1481: `timeline` is subcommand-only (no legacy flag predates
        // it) — assert it flips only the new 9th (timeline) slot.
        assert_eq!(
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Timeline)
            ),
            (None, false, false, false, false, false, false, false, true)
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShaMatch {
    Exact,
    Ancestor,
    Unrelated,
    Unknown,
}

/// TASK-221: shell out to `git rev-parse HEAD` inside the repo. Returns
/// the full 40-char SHA, or None if git is unavailable or HEAD missing.
pub(crate) fn current_branch_head_sha(repo: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// TASK-221: extract the SHA embedded by build.rs from a binary's
/// --version output. The banner shape is:
///   "aida X.Y.Z (built TIMESTAMP, sha SHA[+dirty])"
/// Returns None if the binary is missing, refuses to run, or the banner
/// shape changed.
pub(crate) fn binary_embedded_sha(binary: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output_retrying_etxtbsy()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    parse_embedded_sha(&s)
}

/// Pure parser for the --version banner SHA. Split out so the unit tests
/// don't need a real binary. trace:TASK-221 | ai:claude
pub(crate) fn parse_embedded_sha(banner: &str) -> Option<String> {
    // Match "sha <HEX>" — accept 7+ hex chars; trim trailing "+dirty" or
    // ")" or whitespace.
    let lower = banner.to_ascii_lowercase();
    let idx = lower.find("sha ")?;
    let after = &banner[idx + 4..];
    let mut end = 0;
    for (i, c) in after.char_indices() {
        if c.is_ascii_hexdigit() {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if end < 7 {
        return None;
    }
    Some(after[..end].to_string())
}

/// TASK-221: classify a binary's embedded SHA against current HEAD.
/// - Exact when the SHAs match (treating the binary's SHA as a prefix of
///   HEAD, since build.rs may stamp a short SHA).
/// - Ancestor when `git merge-base --is-ancestor <binary-sha> HEAD` says
///   so — current branch is ahead of the build, but they're related.
/// - Unrelated otherwise (likely a different branch's build).
/// - Unknown when git isn't available (graceful fallback).
pub(crate) fn classify_sha_match(
    repo: &std::path::Path,
    binary_sha: &str,
    head_sha: &str,
) -> ShaMatch {
    let bin_lower = binary_sha.to_ascii_lowercase();
    let head_lower = head_sha.to_ascii_lowercase();
    if head_lower.starts_with(&bin_lower) || bin_lower.starts_with(&head_lower) {
        return ShaMatch::Exact;
    }
    // git merge-base --is-ancestor returns exit 0 when the first commit
    // is an ancestor of the second.
    //
    // BUG-702: use `.output()` (not `.status()`) so git's stderr is CAPTURED,
    // not inherited — a `binary_sha` that a history rewrite purged (force-push
    // + gc) makes git print `fatal: Not a valid object name <sha>`, which would
    // otherwise leak into `aida pull` output. trace:BUG-702 | ai:claude
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", binary_sha, head_sha])
        .output();
    match output {
        Ok(o) => classify_from_merge_base_exit(o.status.code()),
        Err(_) => ShaMatch::Unknown,
    }
}

/// BUG-702: map `git merge-base --is-ancestor` exit code → [`ShaMatch`]. Exit
/// `0` = ancestor; `1` = a clean "not an ancestor" (both commits resolved);
/// anything else — notably `128` = bad/missing object from a purged SHA, or git
/// unavailable — is Unknown: the SHA can't be placed, so no stale-binary nudge
/// and (with the captured stderr above) no leaked git fatal. A purged SHA is
/// genuinely Unknown, NOT a misleading Unrelated. Pure so the exit-code
/// contract is unit-testable.
// trace:BUG-702 | ai:claude
pub(crate) fn classify_from_merge_base_exit(code: Option<i32>) -> ShaMatch {
    match code {
        Some(0) => ShaMatch::Ancestor,
        Some(1) => ShaMatch::Unrelated,
        _ => ShaMatch::Unknown,
    }
}

/// BUG-665: pure predicate — should `aida pull` warn that the built binary is
/// now stale after the code leg advanced? True only when the in-repo build is
/// dev-activated AND its embedded SHA is a STRICT ancestor of HEAD (genuinely
/// behind the freshly-pulled changes). A non-dev-activated (released) binary on
/// PATH is expected to differ, so no warning; a binary that already matches
/// HEAD ([`ShaMatch::Exact`]) or has diverged ([`ShaMatch::Unrelated`] /
/// [`ShaMatch::Unknown`]) does NOT get this "run cargo build" nudge. Reuses the
/// same SHA classification `aida dev status` shows.
// trace:BUG-665 | ai:claude
pub(crate) fn pull_binary_is_stale(dev_activated: bool, verdict: ShaMatch) -> bool {
    dev_activated && matches!(verdict, ShaMatch::Ancestor)
}

/// BUG-665: after a successful `aida pull` code leg, warn when a dev-activated
/// in-repo binary is now behind the freshly-pulled HEAD — so `aida tui` /
/// `aida integrate` don't silently keep running old code until the next
/// `cargo build`. No-op when not dev-activated, when the binary already matches
/// HEAD, or when the pulled repo isn't the one the active binary was built from.
/// Best-effort: any missing signal (env unset, git/binary unavailable) → silent.
// trace:BUG-665 | ai:claude
pub(crate) fn warn_if_pulled_binary_stale(project_root: &std::path::Path) {
    // Dev-activation: an in-repo build is on PATH. Same signals `aida dev
    // status` reads (AIDA_DEV_ACTIVE + AIDA_DEV_BIN + AIDA_DEV_REPO).
    let dev_activated = std::env::var("AIDA_DEV_ACTIVE").is_ok();
    if !dev_activated {
        return;
    }
    let (Ok(bin_dir), Ok(repo)) = (
        std::env::var("AIDA_DEV_BIN"),
        std::env::var("AIDA_DEV_REPO"),
    ) else {
        return;
    };
    let repo_path = std::path::PathBuf::from(&repo);
    // Only reason about the binary when the repo we just pulled IS the one the
    // active binary was built from — otherwise the SHAs aren't comparable and a
    // warning would be a false alarm.
    let same_repo = match (project_root.canonicalize(), repo_path.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same_repo {
        return;
    }
    let aida_path = std::path::PathBuf::from(&bin_dir).join("aida");
    let head = current_branch_head_sha(&repo_path);
    let bin_sha = binary_embedded_sha(&aida_path);
    if let (Some(h), Some(b)) = (head.as_deref(), bin_sha.as_deref()) {
        let verdict = classify_sha_match(&repo_path, b, h);
        // trace:TASK-1719 | ai:antigravity
        if pull_binary_is_stale(dev_activated, verdict) {
            eprintln!();
            eprintln!(
                "  {} your aida binary is now behind HEAD — run `aida dev build` to pick up the pulled changes.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
            );
        }
    }
}

/// TASK-667: render a runtime hint for one of the shell-modifying subcommands
/// (`role enter <role>`, `role end`, `role add <name>`, `session start`,
/// `session end`, `dev activate`, `dev deactivate`) in the form the caller's
/// shell will actually honor.
///
/// The `aida()` wrapper installed by `aida dev shell-init` auto-evals these
/// subcommands, so under the wrapper the BARE form (`aida role enter <role>`)
/// is correct — the function evals the binary's stdout. Printing
/// `eval "$(aida role enter <role>)"` there double-evals (the inner eval runs
/// in a subshell) and the shell change is silently lost — the long-standing
/// double-eval footgun.
///
/// Without the wrapper (raw binary on PATH), the bare form would just print
/// shell code that never executes, so the `eval "$(...)"` form is required.
///
/// The wrapper signals its presence via `AIDA_SHELL_WRAPPER` (exported from
/// SHELL_HELPERS), which is how we branch.
pub(crate) fn eval_subcommand_hint(subcommand: &str) -> String {
    if std::env::var_os("AIDA_SHELL_WRAPPER").is_some() {
        format!("aida {subcommand}")
    } else {
        format!("eval \"$(aida {subcommand})\"")
    }
}

/// STORY-472: resolve the bump kind from the mutually-exclusive
/// `--patch`/`--minor`/`--major` flags. None set → "patch" (smallest, safe
/// default); more than one set → a clean error. trace:STORY-472 | ai:claude
pub(crate) fn resolve_release_bump(
    patch: bool,
    minor: bool,
    major: bool,
) -> Result<&'static str, String> {
    match (patch, minor, major) {
        (_, false, false) => Ok("patch"), // explicit --patch or nothing
        (false, true, false) => Ok("minor"),
        (false, false, true) => Ok("major"),
        _ => Err("specify only one of --patch / --minor / --major".to_string()),
    }
}

/// STORY-472: compute the next semver from `current` for a patch/minor/major
/// bump (lower components reset to 0), for the `--check` preview. Returns None
/// if `current` isn't a plain MAJOR.MINOR.PATCH. trace:STORY-472 | ai:claude
pub(crate) fn preview_next_version(current: &str, bump: &str) -> Option<String> {
    let core = current.split(['-', '+']).next().unwrap_or(current);
    let mut it = core.split('.');
    let maj: u64 = it.next()?.parse().ok()?;
    let min: u64 = it.next()?.parse().ok()?;
    let pat: u64 = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(match bump {
        "minor" => format!("{maj}.{}.0", min + 1),
        "major" => format!("{}.0.0", maj + 1),
        _ => format!("{maj}.{min}.{}", pat + 1),
    })
}

pub(crate) fn handle_release(
    patch: bool,
    minor: bool,
    major: bool,
    check: bool,
    after_pr: Option<u64>,
    skip_xplat_check: bool,
) -> Result<()> {
    let bump = resolve_release_bump(patch, minor, major).map_err(|e| anyhow::anyhow!(e))?;

    if check {
        let cur = current_version();
        let target = preview_next_version(cur, bump).unwrap_or_else(|| "?".to_string());
        let repo = std::env::current_dir().ok().and_then(|cwd| {
            find_aida_repo_above(&cwd).or_else(|| {
                std::env::var("AIDA_DEV_REPO")
                    .ok()
                    .map(std::path::PathBuf::from)
                    .filter(|p| is_aida_repo(p))
            })
        });
        println!(
            "{} would run a {} release: {} → {}",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
            bump.bold(),
            cur,
            target.green()
        );
        match &repo {
            Some(r) => {
                let branch = current_branch_at(r).unwrap_or_else(|| "?".to_string());
                let dirty = std::process::Command::new("git")
                    .arg("-C")
                    .arg(r)
                    .args(["status", "--porcelain"])
                    .output()
                    .ok()
                    .map(|o| !o.stdout.is_empty())
                    .unwrap_or(false);
                println!("  repo:    {}", r.display());
                println!("  branch:  {branch}");
                println!(
                    "  tree:    {}",
                    if dirty {
                        "DIRTY — release.sh requires a clean tree"
                            .yellow()
                            .to_string()
                    } else {
                        "clean".to_string()
                    }
                );
            }
            None => println!(
                "  {} not in an aida repo (cd into the checkout or set AIDA_DEV_REPO)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            ),
        }
        if let Some(n) = after_pr {
            println!(
                "  after-pr: would wait for PR #{n}'s checks, merge it (--squash --delete-branch), and sync main FIRST"
            );
        }
        // ADR-72: no cross-platform pre-release gate; --skip-xplat-check is a
        // no-op kept for compatibility. trace:TASK-1723 | ai:claude
        println!("  steps:   1) aida-store pull  2) scripts/release.sh {bump} (tag + push)  3) wait for tarballs  4) upgrade sibling installs");
        println!("  cross-platform gate: none (Linux-only releases)");
        println!("\n  Re-run without --check to execute.");
        return Ok(());
    }

    // TASK-693: land an in-flight PR before releasing — wait for its checks,
    // merge it, and sync local main so scripts/release.sh tags a main that
    // actually includes the merged work. Refuse if the checks don't pass.
    // trace:STORY-472 | ai:claude
    if let Some(n) = after_pr {
        let repo = std::env::current_dir().ok().and_then(|cwd| {
            find_aida_repo_above(&cwd).or_else(|| {
                std::env::var("AIDA_DEV_REPO")
                    .ok()
                    .map(std::path::PathBuf::from)
                    .filter(|p| is_aida_repo(p))
            })
        });
        let merge_root = repo
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("could not resolve repository root for --after-pr merge")
            })?;
        let forge = crate::forge::forge_for(&merge_root);
        let mut metadata_sink = crate::network_retry::NoopSink;
        let metadata = forge.change_metadata(n, &mut metadata_sink).ok();
        let change_ref = crate::forge::ChangeRef {
            id: n,
            url: String::new(),
            branch: metadata
                .as_ref()
                .map(|m| m.head_ref.clone())
                .unwrap_or_default(),
            base: metadata
                .as_ref()
                .map(|m| m.base_ref.clone())
                .unwrap_or_default(),
            title: metadata.as_ref().map(|m| m.title.clone()),
        };
        let change_noun = crate::forge::resolve_forge_kind(&merge_root).change_noun();

        println!(
            "{} waiting for {change_noun} #{n}'s checks…",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
        );
        match forge.watch_ci(&change_ref) {
            Ok(crate::forge::CiState::Success | crate::forge::CiState::None) => println!(
                "  {} {change_noun} #{n} checks passed",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            ),
            Ok(state) => anyhow::bail!(
                "{change_noun} #{n}'s checks did not pass ({state:?}) — not merging or releasing."
            ),
            Err(e) => anyhow::bail!(
                "could not watch {change_noun} #{n}'s checks via forge provider: {e:#}"
            ),
        }

        println!(
            "{} merging {change_noun} #{n}…",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
        );
        let merge_opts = crate::forge::MergeOptions {
            method: crate::forge::MergeMethod::Squash,
            squash_subject: None,
            squash_body: None, // trace:TASK-1330 | ai:claude
            delete_branch: true,
            match_head: None,
        };
        let mut sink = crate::network_retry::StderrSink;
        if let Err(e) =
            crate::forge::forge_for(&merge_root).merge_change(&change_ref, &merge_opts, &mut sink)
        {
            let hint = crate::forge::resolve_forge_kind(&merge_root)
                .merge_cmd(&n.to_string())
                .unwrap_or_else(|| format!("merge change {n}"));
            anyhow::bail!(
                "`{hint}` failed ({e:#}) — merge it manually, then re-run `aida release` without --after-pr."
            );
        }
        println!(
            "  {} {change_noun} #{n} merged",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );

        // Sync local main so release.sh tags the merged commit.
        if let Some(r) = &repo {
            println!(
                "{} syncing local main…",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
            );
            let pull = std::process::Command::new("git")
                .arg("-C")
                .arg(r)
                .args(["pull", "--ff-only"])
                .status();
            if !matches!(pull, Ok(s) if s.success()) {
                anyhow::bail!(
                    "`git pull --ff-only` failed after merging PR #{n} — sync main manually, then re-run `aida release` without --after-pr."
                );
            }
        }
    }

    if skip_xplat_check {
        // scripts/release.sh / pre-release-check.sh honor this env. trace:STORY-472
        std::env::set_var("AIDA_SKIP_XPLAT_CHECK", "1");
    }
    handle_dev_release(bump)
}

#[cfg(test)]
#[path = "tests/release_verb_tests.rs"]
mod release_verb_tests;

/// Stream a child's stdout/stderr to the parent's stderr with a prefix.
/// `aida dev release [bump]` — the one-command release flow:
/// 1. run scripts/release.sh (bumps version, tags, pushes, interactive)
/// 2. wait for the GitHub Actions workflow to publish the binary tarballs
///    (HEAD-poll the asset URL with timeout)
/// 3. upgrade sibling installs to the new version (auto-yes)
///
/// `aida dev patch` is a thin alias that calls this with bump = "patch".
/// trace:EPIC-1-001 | ai:claude
// trace:FR-2-004 | ai:claude
pub(crate) fn handle_dev_release(bump: &str) -> Result<()> {
    // Locate the aida repo. Prefer PWD walk, then $AIDA_DEV_REPO.
    let cwd = std::env::current_dir()?;
    let repo = find_aida_repo_above(&cwd)
        .or_else(|| {
            std::env::var("AIDA_DEV_REPO")
                .ok()
                .map(std::path::PathBuf::from)
                .filter(|p| is_aida_repo(p))
        })
        .ok_or_else(|| {
            // trace:TASK-667 — wrapper-correct activate form.
            anyhow::anyhow!(
                "Not in an aida repo. cd into the aida checkout, set AIDA_DEV_REPO, \
                 or run `{}` first.",
                eval_subcommand_hint("dev activate")
            )
        })?;

    let script = repo.join("scripts/release.sh");
    if !script.is_file() {
        anyhow::bail!(
            "scripts/release.sh not found at {} — is this checkout up to date?",
            script.display()
        );
    }

    // Detect if this repo has a git-canonical aida store to sync.
    let store_path = detect_store_path(&repo);

    // ── Step 1/5: sync store (pull) ──────────────────────────────────────
    if let Some(ref sp) = store_path {
        println!("{}", "─── Step 1/5: syncing aida-store (pull) ───".bold());
        let branch =
            aida_core::git_ops::current_branch(sp).unwrap_or_else(|_| "aida-store".to_string());
        // Use rebase: bare `git pull` fails on divergent branches when
        // the user has no pull.rebase/pull.ff config; the orphan-store
        // model wants linear history. trace:BUG-1-051 | ai:claude
        match aida_core::git_ops::pull_rebase(sp, "origin", &branch) {
            Ok(()) => println!("  Store pull complete."),
            Err(e) => {
                anyhow::bail!(
                    "aida-store pull failed: {}\n\
                     Resolve store conflicts first: aida db sync --pull\n\
                     Then re-run `aida dev release {}`.",
                    e,
                    bump
                );
            }
        }
        println!();
    }

    // STORY-127 detector (3): warn (don't block) when there are open PRs the
    // release tag will not include. A single batched `gh pr list` snapshot.
    // trace:STORY-127 | ai:claude
    {
        let open_numbers: Vec<u64> = collect_open_prs(&repo)
            .by_branch
            .values()
            .map(|p| p.number)
            .collect();
        if let Some(msg) = release_unmerged_pr_warning(&open_numbers) {
            eprintln!("  {} {}", "Warning:".yellow().bold(), msg);
            println!();
        }
    }

    // ── Step 2/5: run release.sh ─────────────────────────────────────────
    println!(
        "{}",
        format!("─── Step 2/5: ./scripts/release.sh {} ───", bump).bold()
    );
    println!("Working in {}", repo.display());
    println!();

    // Run release.sh interactively — it prints the version-bump diff and
    // asks for confirmation. Inheriting stdio means the user sees and
    // responds to the prompts directly.
    let status = std::process::Command::new(&script)
        .arg(bump)
        .current_dir(&repo)
        .status_retrying_etxtbsy()
        .with_context(|| format!("Failed to invoke {}", script.display()))?;
    if !status.success() {
        anyhow::bail!(
            "release.sh exited non-zero (likely cancelled at the confirmation \
             prompt). Aborting upgrade phase — your tree may have a pending \
             version bump; resolve manually if needed."
        );
    }

    // After release.sh completes, the new tag is the latest one in the repo.
    let new_tag = git_describe_latest_tag(&repo).ok_or_else(|| {
        anyhow::anyhow!("release.sh succeeded but no tag is reachable — confused state, please check `git tag --list` manually.")
    })?;

    // ── Step 3/5: wait for GitHub release artifacts ───────────────────────
    println!();
    println!(
        "{}",
        format!(
            "─── Step 3/5: waiting for {} release artifacts ───",
            new_tag
        )
        .bold()
    );
    let target = release_target().ok_or_else(|| {
        anyhow::anyhow!(
            "Unsupported platform — can't auto-poll for tarballs. Wait for the \
             release.yml workflow to finish and run `aida upgrade` manually."
        )
    })?;
    let asset_url = format!(
        "https://github.com/joemooney/aida/releases/download/{}/aida-{}.tar.gz",
        new_tag, target
    );
    println!("Polling {} ...", asset_url);
    poll_until_published(&asset_url, std::time::Duration::from_secs(600))?;

    // ── Step 4/5: upgrade sibling installs ───────────────────────────────
    println!();
    println!(
        "{}",
        format!(
            "─── Step 4/5: upgrading sibling installs to {} ───",
            new_tag
        )
        .bold()
    );
    let bare_version = strip_v(&new_tag).to_string();
    upgrade_cmd::upgrade_dev_mode_sibling_scan(false, Some(&bare_version), true, false)?;

    // ── Step 5/5: sync store (push) ───────────────────────────────────────
    if let Some(ref sp) = store_path {
        println!();
        println!(
            "{}",
            format!(
                "─── Step 5/5: syncing aida-store (push) for {} ───",
                new_tag
            )
            .bold()
        );
        let branch =
            aida_core::git_ops::current_branch(sp).unwrap_or_else(|_| "aida-store".to_string());

        // Commit any pending store changes (e.g., block pointer updates)
        if aida_core::git_ops::has_changes(sp).unwrap_or(false) {
            let msg = format!("chore: sync store for release {}", new_tag);
            let _ = aida_core::git_ops::add_all(sp, "objects");
            let _ = aida_core::git_ops::add_all(sp, "registry");
            if sp.join("metadata.yaml").exists() {
                let _ = aida_core::git_ops::add(sp, &["metadata.yaml"]);
            }
            if let Err(e) = aida_core::git_ops::commit(sp, &msg) {
                eprintln!("  Warning: could not commit store changes: {}", e);
            } else {
                println!("  Committed store changes: {}", msg);
            }
        }

        match aida_core::git_ops::push(sp, "origin", &branch) {
            Ok(true) => println!("  Store push complete."),
            Ok(false) => {
                println!("  Push rejected. Pulling and retrying...");
                if let Err(e) = aida_core::git_ops::pull_rebase(sp, "origin", &branch) {
                    eprintln!("  Warning: store pull-rebase failed: {}", e);
                } else {
                    match aida_core::git_ops::push(sp, "origin", &branch) {
                        Ok(_) => println!("  Store push complete after rebase."),
                        Err(e) => eprintln!("  Warning: store push failed after rebase: {}", e),
                    }
                }
            }
            Err(e) => eprintln!("  Warning: store push failed: {}", e),
        }
    }

    println!();
    println!(
        "{}: shipped {} and refreshed sibling installs.",
        "DONE".green().bold(),
        new_tag
    );
    Ok(())
}

pub(crate) fn read_workspace_version(root: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_workspace_package = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }
        if in_workspace_package {
            if line.starts_with('[') {
                break;
            }
            if let Some(rest) = line.strip_prefix("version") {
                let v = rest
                    .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
                    .trim_matches('"');
                return Some(v.to_string());
            }
        }
    }
    None
}

pub(crate) fn git_describe_latest_tag(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub(crate) fn git_commits_since_tag(root: &std::path::Path, tag: &str) -> Option<usize> {
    if tag == "(none)" {
        return None;
    }
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("{}..HEAD", tag)])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

pub(crate) fn check_template_symlinks(root: &std::path::Path) -> String {
    let claude_skills = root.join(".claude/skills");
    if !claude_skills.is_dir() {
        return "no .claude/skills/ directory".to_string();
    }
    let mut total = 0;
    let mut broken = 0;
    if let Ok(entries) = std::fs::read_dir(&claude_skills) {
        for entry in entries.flatten() {
            let path = entry.path();
            // Only count the actual symlinks (skip non-symlink files).
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.file_type().is_symlink() {
                    total += 1;
                    if !path.exists() {
                        broken += 1;
                    }
                }
            }
        }
    }
    if total == 0 {
        return "no symlinks (templates copied, not symlinked)".to_string();
    }
    if broken == 0 {
        format!("{}/{} OK", total, total).green().to_string()
    } else {
        format!("{} broken / {}", broken, total).red().to_string()
    }
}

/// Returns (ahead, behind) of `aida-store` branch vs `origin/aida-store`.
pub(crate) fn orphan_branch_sync_state(store_path: &std::path::Path) -> Option<(usize, usize)> {
    // Run inside the store worktree so we see the right branch.
    let ahead = std::process::Command::new("git")
        .args(["rev-list", "--count", "origin/aida-store..HEAD"])
        .current_dir(store_path)
        .output()
        .ok()?;
    let behind = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD..origin/aida-store"])
        .current_dir(store_path)
        .output()
        .ok()?;
    if !ahead.status.success() || !behind.status.success() {
        return None;
    }
    let a: usize = String::from_utf8_lossy(&ahead.stdout).trim().parse().ok()?;
    let b: usize = String::from_utf8_lossy(&behind.stdout)
        .trim()
        .parse()
        .ok()?;
    Some((a, b))
}

/// How aida was installed on this machine. Determines the upgrade strategy.
pub(crate) enum InstallMethod {
    /// Found under `~/.cargo/bin/` — installed via `cargo install`.
    /// Upgrade by re-running `cargo install --git`.
    Cargo(std::path::PathBuf),
    /// Found in a system bin dir (`/usr/local/bin`, `/opt/...`, etc.) —
    /// installed via release tarball. Upgrade by downloading the matching
    /// release artifact and replacing the binary in place.
    Binary(std::path::PathBuf),
    /// Found inside a `target/debug` or `target/release` directory — the
    /// running binary is a developer build. Refuse to upgrade.
    DeveloperBuild(std::path::PathBuf),
}

pub(crate) fn detect_install_method() -> Result<InstallMethod> {
    let exe = aida_exe_path();
    let exe_str = exe.to_string_lossy();

    if exe_str.contains("/target/debug/") || exe_str.contains("/target/release/") {
        return Ok(InstallMethod::DeveloperBuild(exe));
    }

    // Cargo install puts binaries in $CARGO_HOME/bin (default ~/.cargo/bin).
    let cargo_home = std::env::var("CARGO_HOME").ok();
    let cargo_bin = cargo_home
        .map(|h| std::path::PathBuf::from(h).join("bin"))
        .or_else(|| crate::home_dir().map(|h| h.join(".cargo/bin")));
    if let Some(bin) = cargo_bin {
        if exe.starts_with(&bin) {
            return Ok(InstallMethod::Cargo(exe));
        }
    }

    Ok(InstallMethod::Binary(exe))
}

pub(crate) fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Build git SHA stamped at compile time (or "unknown" if git wasn't
/// available in the build env). Set by build.rs.
pub(crate) fn build_git_sha() -> &'static str {
    env!("AIDA_BUILD_GIT_SHA")
}

/// Whether the working tree had uncommitted changes at build time.
pub(crate) fn build_git_dirty() -> bool {
    env!("AIDA_BUILD_GIT_DIRTY") == "1"
}

/// Build time formatted in the user's local timezone — the banner is
/// human-facing so we render it where the user reads it, not in UTC.
/// On-disk fields (oplog, YAML created_at, etc.) stay UTC.
/// trace:feedback_local_time | ai:claude
pub(crate) fn build_time_iso() -> String {
    let secs: i64 = env!("AIDA_BUILD_UNIX_TIME").parse().unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .unwrap_or_else(|| "(unknown)".to_string())
}

/// One-line build banner for use in --version and status output:
///   "0.4.0 (built 2026-05-03 01:23:45 PDT, sha 866b050[+dirty])"
pub(crate) fn build_banner() -> String {
    format!(
        "{} (built {}, sha {}{})",
        current_version(),
        build_time_iso(),
        build_git_sha(),
        if build_git_dirty() { "+dirty" } else { "" }
    )
}

/// Compile-time-resolved release-artifact target name (matches the matrix in
/// `.github/workflows/release.yml`). Returns None on unsupported platforms.
pub(crate) fn release_target() -> Option<&'static str> {
    use std::env::consts::{ARCH, OS};
    match (OS, ARCH) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        _ => None,
    }
}

/// Strip a leading `v` from a tag string (`v0.4.0` -> `0.4.0`).
pub(crate) fn strip_v(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s)
}

// ?-exempt: bare helper; each consequential CALLER carries its own card or exemption. trace:STORY-809
pub(crate) fn confirm(prompt: &str) -> bool {
    print!("{}", prompt);
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Common locations where users typically have aida installed. Order matters
/// for display; we use it as-is for the scan-and-report output.
/// Walk up from `start` (a binary path or directory) looking for the aida
/// repo root. Used to discover the dev binary's source repo so we can ask
/// "is this build ahead of the latest release tag".
pub(crate) fn find_aida_repo_above(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut probe = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    for _ in 0..6 {
        if is_aida_repo(&probe) {
            return Some(probe);
        }
        probe = match probe.parent() {
            Some(p) => p.to_path_buf(),
            None => return None,
        };
    }
    None
}

/// Bulk-import helper for git-canonical stores (FR-1-002): drains an iterator
/// of new requirements through a single `GitBackend::bulk_writer()` session
/// so one git commit covers the whole batch. Falls back to the legacy
/// `update_atomically` path for non-git backends (SQLite / YAML), since the
/// bulk-writer's optimization only applies to the git path.
/// trace:FR-1-002 | ai:claude
pub(crate) fn bulk_import_via_writer<I>(
    storage: &Storage,
    commit_subject: &str,
    reqs: I,
) -> Result<usize>
where
    I: IntoIterator<Item = Requirement>,
{
    let path = storage.path();
    if path.is_dir() {
        // Attach the same dispenser `storage` was built with, when it has
        // one — otherwise a fresh `GitBackend` defaults to node_id "0" for
        // every oplog entry this writer commits. trace:TASK-1487 | ai:claude
        let mut backend = aida_core::GitBackend::new(path)?;
        if let Some(dispenser) = storage.dispenser() {
            backend = backend.with_dispenser(dispenser.clone());
        }
        let mut writer = backend.bulk_writer()?;
        for req in reqs {
            writer.add(req)?;
        }
        let n = writer.finish(commit_subject)?;
        return Ok(n);
    }

    // Non-git backend: legacy path (single update_atomically commit).
    let mut count = 0;
    let reqs: Vec<Requirement> = reqs.into_iter().collect();
    storage.update_atomically(|store| {
        for req in reqs {
            let type_prefix = store.get_type_prefix(&req.req_type);
            store.add_requirement_with_id(req, None, type_prefix.as_deref());
            count += 1;
        }
    })?;
    Ok(count)
}
