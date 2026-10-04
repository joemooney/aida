//! Tests for STORY-1476: graded review runs spec-authored acceptance commands
//! only when the machine-global trusted config opts in.
//!
//! Every test is fixture-based: temp checkouts for marker files, the hermetic
//! test HOME (`test_home` + `EnvVarsGuard`) for `~/.aida/config.toml`, and a
//! real-git temp repo for the repo-level opt-in case. Nothing touches the
//! operator's real `~/.aida`.
//
// trace:STORY-1476 | ai:claude

use crate::evaluator::{
    ChoiceResponse, EvaluatorEngine, EvaluatorFuture, MockEvaluator, NoulResponse, ScoreResponse,
};
use crate::graded_review::{
    execute_graded_review, generate_graded_reviewer_prompt, AcceptanceCommandPolicy, CriterionKind,
    CriterionStatus, GradedReviewVerdict, NOT_RUN_OUTPUT, REPO_OPTIN_IGNORED_NOTE,
};
use crate::test_env::EnvVarsGuard;
use crate::{acceptance_command_policy_from_toml, acceptance_command_policy_global};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A `MockEvaluator` that also records every noul instruction it was asked.
struct RecordingEvaluator {
    inner: MockEvaluator,
    instructions: Arc<Mutex<Vec<String>>>,
}

impl RecordingEvaluator {
    fn new(inner: MockEvaluator) -> Self {
        Self {
            inner,
            instructions: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl EvaluatorEngine for RecordingEvaluator {
    fn evaluate_noul<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
    ) -> EvaluatorFuture<'a, NoulResponse> {
        self.instructions
            .lock()
            .unwrap()
            .push(format!("{context}\n{instruction}"));
        self.inner.evaluate_noul(context, instruction)
    }

    fn evaluate_choice<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        options: &'a HashMap<String, String>,
    ) -> EvaluatorFuture<'a, ChoiceResponse> {
        self.inner.evaluate_choice(context, instruction, options)
    }

    fn evaluate_score<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        levels: &'a [String],
    ) -> EvaluatorFuture<'a, ScoreResponse> {
        self.inner.evaluate_score(context, instruction, levels)
    }
}

fn run(
    desc: &str,
    checkout: &Path,
    evaluator: Option<&dyn EvaluatorEngine>,
    policy: &AcceptanceCommandPolicy,
) -> GradedReviewVerdict {
    execute_graded_review(
        "STORY-9001",
        "Trust gate",
        desc,
        "+ fn changed() {}",
        "abc1234",
        checkout,
        evaluator,
        policy,
    )
    .unwrap()
}

fn allow(entries: &[&str]) -> AcceptanceCommandPolicy {
    AcceptanceCommandPolicy {
        enabled: true,
        allow: entries.iter().map(|s| s.to_string()).collect(),
        repo_optin_ignored: false,
    }
}

/// Write `body` as the fake HOME's `~/.aida/config.toml` and return the guard
/// that pins `HOME` there for the test's lifetime.
fn fake_home_with_config(home: &Path, body: Option<&str>) -> EnvVarsGuard {
    let guard = EnvVarsGuard::set(&[("HOME", home.to_str().unwrap())]);
    if let Some(body) = body {
        std::fs::create_dir_all(home.join(".aida")).unwrap();
        std::fs::write(home.join(".aida").join("config.toml"), body).unwrap();
    }
    guard
}

fn not_run_statuses(verdict: &GradedReviewVerdict) -> Vec<&CriterionKind> {
    verdict
        .results
        .iter()
        .filter(|r| r.status == CriterionStatus::NotRun)
        .map(|r| &r.criterion)
        .collect()
}

// --- Default (untrusted) behaviour -----------------------------------------

/// The acceptance test the story asks for: with no opt-in, a spec-authored
/// command never runs.
#[test]
fn story_1476_default_policy_does_not_run_spec_command() {
    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `touch pwned`\n";
    let verdict = run(
        desc,
        checkout.path(),
        None,
        &AcceptanceCommandPolicy::default(),
    );

    assert!(
        !checkout.path().join("pwned").exists(),
        "the spec-authored command must not have been spawned"
    );
    assert_eq!(verdict.results.len(), 1);
    assert_eq!(verdict.results[0].status, CriterionStatus::NotRun);
    assert_eq!(verdict.results[0].output.as_deref(), Some(NOT_RUN_OUTPUT));
    assert_eq!(verdict.results[0].exit_code, None);
    assert_eq!(verdict.machine_verified_count, 0);
    assert_eq!(verdict.machine_passed_count, 0);
    assert_eq!(verdict.not_run_count, 1);
    assert_eq!(verdict.overall_verdict, "escalated");
    assert!(verdict.escalated_to_seat);
    assert!(verdict
        .summary
        .contains("1 spec-authored command(s) not run"));
}

/// Every criterion refused, no prose, an evaluator that would say p=1.0:
/// the evaluator fast-pass must not auto-approve.
#[test]
fn story_1476_all_notrun_with_evaluator_never_auto_approves() {
    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `true`\n- [ ] `echo ok`\n";
    let mock = MockEvaluator::new().with_noul_confidence(1.0, 1.0);
    let verdict = run(
        desc,
        checkout.path(),
        Some(&mock),
        &AcceptanceCommandPolicy::default(),
    );

    assert_ne!(verdict.overall_verdict, "approved");
    assert_eq!(verdict.overall_verdict, "escalated");
    assert!(verdict.escalated_to_seat);
    assert_eq!(verdict.not_run_count, 2);
    assert_eq!(verdict.machine_verified_count, 0);
}

/// A1: one permitted passing command plus one refused command and no prose
/// would otherwise hit the Rung-2 auto-approve branch.
#[cfg(unix)]
#[test]
fn story_1476_mixed_passed_and_notrun_escalates() {
    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `true`\n- [ ] `touch pwned`\n";
    let verdict = run(desc, checkout.path(), None, &allow(&["true"]));

    assert!(!checkout.path().join("pwned").exists());
    assert_eq!(verdict.machine_verified_count, 1);
    assert_eq!(verdict.machine_passed_count, 1);
    assert_eq!(verdict.not_run_count, 1);
    assert_eq!(verdict.results[0].status, CriterionStatus::Passed);
    assert_eq!(verdict.results[1].status, CriterionStatus::NotRun);
    assert_eq!(verdict.overall_verdict, "escalated");
    assert!(verdict.escalated_to_seat);
}

/// A1 plus the STORY-1476 review finding: a failed permitted command keeps
/// the fail-closed `rejected` machine verdict, and the refused command still
/// reaches the Phase 3 seat as a manual-verification obligation instead of
/// being dropped with the early return.
#[cfg(unix)]
#[test]
fn story_1476_failed_plus_notrun_rejects_and_still_escalates_manual_checks() {
    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `false`\n- [ ] `touch pwned`\n";
    let verdict = run(desc, checkout.path(), None, &allow(&["false"]));

    assert!(!checkout.path().join("pwned").exists());
    assert_eq!(
        verdict.overall_verdict, "rejected",
        "the failed deterministic check keeps its veto"
    );
    assert_eq!(verdict.verdict_kind, "Rejected");
    assert!(
        verdict.escalated_to_seat,
        "the refused command must still reach the reviewer seat: {}",
        verdict.summary
    );
    assert_eq!(verdict.not_run_count, 1);
    assert!(verdict
        .summary
        .contains("1 spec-authored command(s) not run"));

    // The seat's prompt carries both halves: the settled failure it may not
    // approve over, and the refused command to verify by hand.
    let prompt = generate_graded_reviewer_prompt("STORY-9001", Some(79), &verdict);
    assert!(prompt.contains("[FAILED (exit 1)] `false`"), "{prompt}");
    assert!(prompt.contains("cannot be an approval"), "{prompt}");
    assert!(prompt.contains("Needs manual verification"), "{prompt}");
    assert!(prompt.contains("[NOT RUN] `touch pwned`"), "{prompt}");
    let (_, context) = prompt.split_once("\n\n").unwrap();
    assert!(context.contains("[NOT RUN] `touch pwned`"));
}

/// A refused command's text never reaches the evaluator, and neither does
/// the prose when escalation is already forced.
#[test]
fn story_1476_notrun_not_sent_to_evaluator() {
    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `touch pwned`\n- Error messages are clear\n";
    let recorder = RecordingEvaluator::new(MockEvaluator::new().with_noul(0.99));
    let verdict = run(
        desc,
        checkout.path(),
        Some(&recorder),
        &AcceptanceCommandPolicy::default(),
    );

    let calls = recorder.instructions.lock().unwrap();
    assert!(
        calls.iter().all(|c| !c.contains("touch pwned")),
        "refused command text reached the evaluator: {calls:?}"
    );
    assert!(
        calls.is_empty(),
        "no evaluation should run once a refused command forces escalation: {calls:?}"
    );
    assert!(verdict.escalated_to_seat);
    assert_eq!(verdict.overall_verdict, "escalated");
    // The prose is still handed to the seat.
    assert!(verdict
        .results
        .iter()
        .any(|r| r.status == CriterionStatus::Escalated
            && matches!(&r.criterion, CriterionKind::Prose { text } if text.contains("Error messages"))));
}

// --- Opt-in behaviour ------------------------------------------------------

/// Fake HOME config enabled with `touch` allowlisted: the command runs.
#[cfg(unix)]
#[test]
fn story_1476_enabled_allowlisted_command_runs() {
    let home = tempfile::tempdir().unwrap();
    let _env = fake_home_with_config(
        home.path(),
        Some("[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"touch marker\"]\n"),
    );
    let policy = acceptance_command_policy_global();
    assert!(policy.enabled);
    assert_eq!(policy.allow, vec!["touch marker".to_string()]);

    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `touch marker`\n";
    let verdict = run(desc, checkout.path(), None, &policy);

    assert!(checkout.path().join("marker").exists());
    assert_eq!(verdict.results[0].status, CriterionStatus::Passed);
    assert_eq!(verdict.not_run_count, 0);
    assert_eq!(verdict.overall_verdict, "approved");
}

/// A chained command passes the prefix but not the metacharacter refusal.
#[test]
fn story_1476_metachar_chained_command_refused_under_prefix() {
    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `cargo test && touch pwned`\n";
    let verdict = run(desc, checkout.path(), None, &allow(&["cargo test *"]));

    assert!(!checkout.path().join("pwned").exists());
    assert_eq!(verdict.results[0].status, CriterionStatus::NotRun);
    assert_eq!(verdict.not_run_count, 1);
}

/// Whole-word matching: "cargo test" does not match "cargo testx".
#[test]
fn story_1476_word_prefix_not_string_prefix() {
    let policy = allow(&["cargo test"]);
    assert!(policy.permits("cargo test"));
    assert!(!policy.permits("cargo testx"));
    assert!(!policy.permits("cargo testx --release"));
    let star = allow(&["cargo test *"]);
    assert!(star.permits("cargo test --release"));
    assert!(!star.permits("cargo testx --release"));
}

/// A2: exact word-sequence by default; a trailing `*` opts into arguments.
#[test]
fn story_1476_exact_match_refuses_arguments_unless_trailing_star() {
    let exact = allow(&["cargo test"]);
    assert!(exact.permits("cargo test"));
    assert!(exact.permits("cargo   test"));
    assert!(!exact.permits("cargo test --config x=y"));
    assert!(!exact.permits("cargo"));

    let star = allow(&["cargo test *"]);
    assert!(star.permits("cargo test"));
    assert!(star.permits("cargo test --config x=y"));
    assert!(!star.permits("make check-templates"));
}

/// `"*"` is explicit full trust: metacharacters are allowed through.
#[cfg(unix)]
#[test]
fn story_1476_star_allows_metachars() {
    let checkout = tempfile::tempdir().unwrap();
    let desc = "## Acceptance\n- [ ] `true && touch ok`\n";
    let verdict = run(desc, checkout.path(), None, &allow(&["*"]));

    assert!(checkout.path().join("ok").exists());
    assert_eq!(verdict.results[0].status, CriterionStatus::Passed);
    // First word `true` classifies the whole command as trivial, so the
    // passed check is not approval evidence. trace:BUG-1668 | ai:claude
    assert_eq!(verdict.overall_verdict, "escalated");
}

/// A3: quotes and control characters are refused too (the checker's split
/// would disagree with bash's).
#[test]
fn story_1476_quotes_and_control_chars_refused() {
    let policy = allow(&["echo *"]);
    assert!(policy.permits("echo hi"));
    assert!(policy.permits("echo\thi"));
    assert!(!policy.permits("echo 'hi'"));
    assert!(!policy.permits("echo \"hi\""));
    assert!(!policy.permits("echo hi\r"));
    assert!(!policy.permits("echo hi\x07"));
    assert!(!policy.permits("echo $(touch pwned)"));
    assert!(!policy.permits("echo hi > out"));
}

/// Disabled or empty allowlists deny everything, including with `*`.
#[test]
fn story_1476_disabled_or_empty_policy_denies() {
    let disabled = AcceptanceCommandPolicy {
        enabled: false,
        allow: vec!["*".into()],
        repo_optin_ignored: false,
    };
    assert!(!disabled.permits("true"));
    let empty = allow(&[]);
    assert!(!empty.permits("true"));
    assert!(!AcceptanceCommandPolicy::default().permits("true"));
}

// --- Trust sources -----------------------------------------------------------

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo-level opt-in (committed to main AND pushed to origin/main) is
/// ignored: no marker, and the summary names the ignored opt-in.
#[test]
fn story_1476_repo_config_optin_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    let out = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&origin)
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&work)
        .output()
        .unwrap();
    assert!(out.status.success());
    git(&work, &["config", "user.email", "t@example.invalid"]);
    git(&work, &["config", "user.name", "t"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    std::fs::create_dir_all(work.join(".aida")).unwrap();
    std::fs::write(
        work.join(".aida").join("config.toml"),
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"*\"]\n",
    )
    .unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "hostile opt-in"]);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&work, &["push", "-q", "-u", "origin", "main"]);

    // Fake HOME with no config at all.
    let home = tempfile::tempdir().unwrap();
    let _env = fake_home_with_config(home.path(), None);
    let mut policy = acceptance_command_policy_global();
    assert_eq!(policy, AcceptanceCommandPolicy::default());
    policy.repo_optin_ignored = crate::repo_review_optin_present(&work);
    assert!(policy.repo_optin_ignored);

    let desc = "## Acceptance\n- [ ] `touch pwned`\n";
    let verdict = run(desc, &work, None, &policy);
    assert!(!work.join("pwned").exists());
    assert_eq!(verdict.results[0].status, CriterionStatus::NotRun);
    assert!(verdict.summary.contains(REPO_OPTIN_IGNORED_NOTE));
}

/// A repo with no `[review]` opt-in produces no notice.
#[test]
fn story_1476_repo_without_optin_has_no_notice() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    std::fs::write(
        root.path().join(".aida").join("config.toml"),
        "[review]\nmass_change_mode = \"off\"\n",
    )
    .unwrap();
    assert!(!crate::repo_review_optin_present(root.path()));
    let verdict = run(
        "## Acceptance\n- [ ] `true`\n",
        root.path(),
        None,
        &AcceptanceCommandPolicy::default(),
    );
    assert!(!verdict.summary.contains(REPO_OPTIN_IGNORED_NOTE));
}

/// Truncated TOML in the fake HOME fails closed to the default policy.
#[test]
fn story_1476_malformed_global_config_fails_closed() {
    let home = tempfile::tempdir().unwrap();
    let _env = fake_home_with_config(
        home.path(),
        Some("[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"*\"\n"),
    );
    let policy = acceptance_command_policy_global();
    assert_eq!(policy, AcceptanceCommandPolicy::default());
    assert!(!policy.permits("true"));
}

/// Enabled without an allowlist denies.
#[test]
fn story_1476_enabled_but_empty_allow_denies() {
    let home = tempfile::tempdir().unwrap();
    let _env = fake_home_with_config(
        home.path(),
        Some("[review]\nrun_acceptance_commands = true\n"),
    );
    let policy = acceptance_command_policy_global();
    assert_eq!(policy, AcceptanceCommandPolicy::default());

    let checkout = tempfile::tempdir().unwrap();
    let verdict = run(
        "## Acceptance\n- [ ] `touch pwned`\n",
        checkout.path(),
        None,
        &policy,
    );
    assert!(!checkout.path().join("pwned").exists());
    assert_eq!(verdict.results[0].status, CriterionStatus::NotRun);
}

/// No file in the fake HOME: denied (the common default install).
#[test]
fn story_1476_missing_global_config_denies() {
    let home = tempfile::tempdir().unwrap();
    let _env = fake_home_with_config(home.path(), None);
    assert_eq!(
        acceptance_command_policy_global(),
        AcceptanceCommandPolicy::default()
    );
}

/// Table-driven pure parse cases.
#[test]
fn story_1476_policy_from_toml_pure() {
    let parse = |body: &str| acceptance_command_policy_from_toml(&toml::from_str(body).unwrap());

    // Well-formed opt-ins.
    let ok = parse(
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"cargo test\", \"make check-templates *\"]\n",
    )
    .unwrap();
    assert!(ok.enabled);
    assert_eq!(ok.allow, vec!["cargo test", "make check-templates *"]);
    assert!(!ok.repo_optin_ignored);
    let star =
        parse("[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"*\"]\n")
            .unwrap();
    assert!(star.permits("anything; goes"));

    // Denied cases: every one is Err, never a partially honoured policy.
    let denied = [
        "",
        "[review]\n",
        "[review]\nrun_acceptance_commands = false\nacceptance_command_allow = [\"*\"]\n",
        "[review]\nrun_acceptance_commands = \"yes\"\nacceptance_command_allow = [\"*\"]\n",
        "[review]\nrun_acceptance_commands = true\n",
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = []\n",
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = \"cargo test\"\n",
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"cargo test\", 1]\n",
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"cargo test\", \"\"]\n",
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"cargo test; rm -rf /\"]\n",
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"cargo * test\"]\n",
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"echo 'hi'\"]\n",
        "review = 1\n",
        "[other]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"*\"]\n",
    ];
    for body in denied {
        assert!(parse(body).is_err(), "expected denied policy for:\n{body}");
    }
}

/// TASK-1545 acceptance 1: a refused character is checked on the RAW entry.
/// Trimming first would normalise a malformed entry such as `*\r` to `*` and
/// accept it as full trust.
#[test]
fn story_1476_allow_entry_refused_char_checked_before_trim() {
    let parse = |body: &str| acceptance_command_policy_from_toml(&toml::from_str(body).unwrap());

    for entry in [
        "*\\r",
        "*\\n",
        "cargo test\\r",
        "\\rcargo test",
        "cargo test\\u0007",
        "cargo test \\r *",
    ] {
        let body = format!(
            "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"{entry}\"]\n"
        );
        assert!(
            parse(&body).is_err(),
            "entry {entry:?} must deny the whole policy"
        );
    }

    // Plain surrounding whitespace is still trimmed, and such an entry stays
    // usable: only refused characters deny.
    let ok = parse(
        "[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"  cargo test  \", \"\\tmake docs\\t\"]\n",
    )
    .unwrap();
    assert_eq!(ok.allow, vec!["cargo test", "make docs"]);
    assert!(ok.permits("cargo test"));

    // End to end: a stray carriage return in the global config denies the
    // whole policy, so the spec-authored command is still refused.
    let home = tempfile::tempdir().unwrap();
    let _env = fake_home_with_config(
        home.path(),
        Some("[review]\nrun_acceptance_commands = true\nacceptance_command_allow = [\"*\\r\"]\n"),
    );
    let policy = acceptance_command_policy_global();
    assert_eq!(policy, AcceptanceCommandPolicy::default());
    let checkout = tempfile::tempdir().unwrap();
    let verdict = run(
        "## Acceptance\n- [ ] `touch pwned`\n",
        checkout.path(),
        None,
        &policy,
    );
    assert!(!checkout.path().join("pwned").exists());
    assert_eq!(verdict.results[0].status, CriterionStatus::NotRun);
}

// --- Reviewer prompt ---------------------------------------------------------

/// The Phase 3 prompt lists refused commands under the manual-verification
/// section, tells the seat not to run them, and does so even with zero
/// passes (A4).
#[test]
fn story_1476_prompt_lists_notrun_as_manual() {
    let checkout = tempfile::tempdir().unwrap();

    // Zero passes: the block must still be emitted.
    let verdict = run(
        "## Acceptance\n- [ ] `touch pwned`\n- Docs updated\n",
        checkout.path(),
        None,
        &AcceptanceCommandPolicy::default(),
    );
    assert_eq!(verdict.machine_passed_count, 0);
    assert_eq!(not_run_statuses(&verdict).len(), 1);
    let prompt = generate_graded_reviewer_prompt("STORY-9001", Some(77), &verdict);
    assert!(prompt.starts_with("/aida-review --pr 77\n\n"));
    assert!(prompt.contains("Needs manual verification"));
    assert!(prompt.contains("Do NOT run them yourself"));
    assert!(prompt.contains("[NOT RUN] `touch pwned`"));
    assert!(prompt.contains("Docs updated"));
    assert!(!prompt.contains("MACHINE-VERIFIED"));
    // The orchestrator forwards everything after the first blank line.
    let (_, context) = prompt.split_once("\n\n").unwrap();
    assert!(context.contains("[NOT RUN] `touch pwned`"));

    // With a pass as well: both sections present.
    #[cfg(unix)]
    {
        let verdict = run(
            "## Acceptance\n- [ ] `true`\n- [ ] `touch pwned`\n",
            checkout.path(),
            None,
            &allow(&["true"]),
        );
        let prompt = generate_graded_reviewer_prompt("STORY-9001", Some(78), &verdict);
        assert!(prompt.contains("[PASSED (exit 0)] `true`"));
        assert!(prompt.contains("[NOT RUN] `touch pwned`"));
        assert_eq!(not_run_statuses(&verdict).len(), 1);
    }
}

/// Records stay readable across the additive schema change.
#[test]
fn story_1476_record_without_not_run_count_deserializes() {
    let legacy = serde_json::json!({
        "spec_id": "TASK-1",
        "reviewed_sha": "abc",
        "overall_verdict": "approved",
        "verdict_kind": "Approved",
        "machine_verified_count": 1,
        "machine_passed_count": 1,
        "prose_count": 0,
        "residual_prose_count": 0,
        "escalated_to_seat": false,
        "results": [],
        "summary": "ok"
    });
    let verdict: GradedReviewVerdict = serde_json::from_value(legacy).unwrap();
    assert_eq!(verdict.not_run_count, 0);

    let checkout = tempfile::tempdir().unwrap();
    let fresh = run(
        "## Acceptance\n- [ ] `touch pwned`\n",
        checkout.path(),
        None,
        &AcceptanceCommandPolicy::default(),
    );
    let json = serde_json::to_string(&fresh).unwrap();
    assert!(json.contains("\"not_run_count\":1"));
    assert!(json.contains("\"NotRun\""));
    let back: GradedReviewVerdict = serde_json::from_str(&json).unwrap();
    assert_eq!(back, fresh);
}
