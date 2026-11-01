//! BUG-057: a lane relaunched with a merge-only `ADVISOR BRIEF` re-ran the
//! older review verdict's work instead. These pin what the rework pickup
//! prompt leads with, end to end through `assemble_pickup_prompt` (the
//! function `handle_queue_work` calls), and the git conflict forecast.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
use super::*;
use crate::queue_cmd::{QueueWorkMode, QueueWorkPlan};

fn comment(mins_ago: i64, content: &str) -> aida_core::Comment {
    let mut c = aida_core::Comment::new("advisor".to_string(), content.to_string());
    c.created_at = Utc::now() - chrono::Duration::minutes(mins_ago);
    c
}

fn item_plan() -> QueueWorkPlan {
    QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: Vec::new(),
        scope: "BUG-051".into(),
        review_target: None,
        anchor_display: "BUG-051".into(),
        anchor_title: "anchor".into(),
    }
}

const FINDINGS: &str =
    "REVIEW FINDINGS TO ADDRESS (PR #3169):\nVerdict: request-changes\n- run the bwrap tests";

fn assemble(
    plan: &QueueWorkPlan,
    findings: Option<&str>,
    comments: &[aida_core::Comment],
    conflict: Option<&MainConflict>,
) -> String {
    assemble_pickup_prompt(&PickupInputs {
        plan,
        role: "implementer",
        plan_only: false,
        guided: false,
        review_findings: findings,
        spec_comments: comments,
        conflict,
    })
}

fn no_gate() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[("AIDA_AGENT_GATE_NAME", None)])
}

#[test]
fn brief_comments_are_advisor_direction() {
    assert!(is_brief_comment(
        "ADVISOR BRIEF 13:10 (merge-only, retry): PR-3169 CONFLICTS with main"
    ));
    assert!(is_brief_comment("**Rework brief:** merge main"));
    assert!(!is_brief_comment("see the advisor brief above"));
    assert!(is_brief_comment("## ADVISOR BRIEF\nmerge"));
    assert!(!is_brief_comment("Advisor ruling: keep it"));
    assert!(!is_brief_comment("Returned from NeedsAttention"));
}

/// Acceptance (1), end to end: the brief leads, the findings are relabelled
/// superseded context, and the authoritative round is gone.
#[test]
fn newer_brief_leads_and_supersedes_the_older_verdict_end_to_end() {
    let _g = no_gate();
    let comments = vec![
        comment(120, "Advisor ruling: config-only change"),
        comment(90, FINDINGS),
        comment(
            10,
            "ADVISOR BRIEF 13:10 (merge-only, retry): git merge origin/main, resolve, push",
        ),
    ];
    let plan = item_plan();
    let prompt = assemble(&plan, Some(FINDINGS), &comments, None);
    assert!(
        prompt.starts_with("ADVISOR BRIEF ("),
        "first task is the brief: {prompt}"
    );
    assert!(prompt.contains("This is your task"));
    let brief_at = prompt.find("merge-only").unwrap();
    let ctx_at = prompt
        .find("SUPERSEDED CONTEXT")
        .expect("findings labelled superseded");
    assert!(brief_at < ctx_at);
    assert!(prompt[ctx_at..].contains("run the bwrap tests"));
    assert_eq!(
        prompt.matches("run the bwrap tests").count(),
        1,
        "the findings appear once, as superseded context: {prompt}"
    );
    assert!(
        !prompt.contains("AUTHORITATIVE TASK"),
        "the superseded verdict is not the authoritative round: {prompt}"
    );
    assert!(prompt.ends_with("/aida-pickup BUG-051"), "pickup line last");

    // Control: without the brief the same inputs DO produce the
    // authoritative round, so the assertion above is not vacuous.
    let control = assemble(&plan, Some(FINDINGS), &comments[..2], None);
    assert!(control.contains("ITEMS STILL OPEN (AUTHORITATIVE TASK)"));
    assert!(!control.contains("SUPERSEDED CONTEXT"));
}

/// Finding 1: verdict T0 -> brief T1 -> ruling T2. The newer ruling must not
/// hide the post-verdict brief; both are shown and the brief still leads.
#[test]
fn ruling_after_a_post_verdict_brief_does_not_hide_it() {
    let _g = no_gate();
    let comments = vec![
        comment(90, FINDINGS),
        comment(30, "ADVISOR BRIEF (merge-only): merge main and push"),
        comment(5, "Advisor ruling: keep the config-only scope"),
    ];
    let direction = resolve_direction(&comments, Some(FINDINGS));
    assert!(direction.supersedes_verdict());
    assert!(direction.ruling.is_some());

    let prompt = assemble(&item_plan(), Some(FINDINGS), &comments, None);
    assert!(prompt.starts_with("ADVISOR BRIEF ("), "{prompt}");
    let brief_at = prompt.find("merge main and push").unwrap();
    let ruling_at = prompt.find("ADVISOR RULING (").expect("ruling shown");
    assert!(brief_at < ruling_at);
    assert!(prompt.contains("keep the config-only scope"));
    assert!(prompt.contains("SUPERSEDED CONTEXT"));
    assert!(!prompt.contains("AUTHORITATIVE TASK"));
}

/// Finding 2: a brief OLDER than the last verdict must not override it. It
/// is shown as context that defers to the findings, which stay the task.
#[test]
fn brief_older_than_the_verdict_defers_to_the_findings() {
    let _g = no_gate();
    let comments = vec![
        comment(60, "ADVISOR BRIEF (merge-only): just merge main"),
        comment(5, FINDINGS),
    ];
    let direction = resolve_direction(&comments, Some(FINDINGS));
    assert!(!direction.supersedes_verdict());
    assert_eq!(
        direction.brief.map(|(_, s)| s),
        Some(BriefStanding::PredatesVerdict)
    );

    let prompt = assemble(&item_plan(), Some(FINDINGS), &comments, None);
    assert!(prompt.contains("ITEMS STILL OPEN (AUTHORITATIVE TASK)"));
    assert!(!prompt.contains("SUPERSEDED CONTEXT"));
    assert!(!prompt.contains("This is your task"));
    assert!(
        !prompt.contains("the brief wins") && !prompt.contains("the ruling wins"),
        "a stale brief carries no binding wording: {prompt}"
    );
    let brief_line = prompt
        .lines()
        .find(|l| l.starts_with("ADVISOR BRIEF ("))
        .expect("brief shown as context");
    assert!(brief_line.contains("OLDER CONTEXT"));
    assert!(brief_line.contains("the findings win"));
    let task_at = prompt
        .find("ITEMS STILL OPEN (AUTHORITATIVE TASK)")
        .unwrap();
    let older_at = prompt.find("OLDER CONTEXT").unwrap();
    assert!(
        task_at < older_at,
        "the findings come first; the stale brief is background below them: {prompt}"
    );
    assert!(prompt.ends_with("/aida-pickup BUG-051"));
}

/// Brief item 3(d), end to end: findings with no verdict time at all stay
/// the authoritative task, and any brief is older context below them.
#[test]
fn untimed_findings_stay_authoritative_over_a_brief_end_to_end() {
    let _g = no_gate();
    let comments = vec![comment(1, "ADVISOR BRIEF (merge-only): just merge main")];
    let prompt = assemble(&item_plan(), Some(FINDINGS), &comments, None);
    let task_at = prompt
        .find("ITEMS STILL OPEN (AUTHORITATIVE TASK)")
        .unwrap_or_else(|| panic!("findings stay the task: {prompt}"));
    assert!(!prompt.contains("SUPERSEDED CONTEXT"), "{prompt}");
    assert!(!prompt.contains("This is your task"), "{prompt}");
    let older_at = prompt
        .find("OLDER CONTEXT")
        .unwrap_or_else(|| panic!("brief labelled older context: {prompt}"));
    assert!(
        task_at < older_at,
        "brief sits below the findings: {prompt}"
    );
    assert!(prompt.ends_with("/aida-pickup BUG-051"));
}

#[test]
fn verdict_time_comes_from_the_recorded_line_or_comments() {
    let brief = vec![comment(60, "ADVISOR BRIEF: merge main")];
    // The live block's own `Recorded:` time counts even with no comment.
    let recorded = format!(
        "REVIEW FINDINGS TO ADDRESS (PR #1):\nRecorded: {}",
        Utc::now().to_rfc3339()
    );
    assert!(!resolve_direction(&brief, Some(recorded.as_str())).supersedes_verdict());
    // No verdict time at all: the findings stay authoritative.
    let untimed = "REVIEW FINDINGS TO ADDRESS (PR #1):\nVerdict: request-changes";
    let d = resolve_direction(&brief, Some(untimed));
    assert_eq!(
        d.brief.map(|(_, s)| s),
        Some(BriefStanding::PredatesVerdict)
    );
    // A ruling never supersedes a verdict; only a brief does.
    let ruling = vec![comment(90, untimed), comment(1, "Advisor ruling: widen")];
    assert!(!resolve_direction(&ruling, Some(untimed)).supersedes_verdict());
    // No open findings: the brief is binding direction.
    let d = resolve_direction(&brief, None);
    assert_eq!(d.brief.map(|(_, s)| s), Some(BriefStanding::Binding));
}

#[test]
fn conflicting_pr_adds_resolve_conflicts_first() {
    let _g = no_gate();
    let conflict = MainConflict {
        pr: 3169,
        base: "main".to_string(),
        files: vec!["aida-cli-lib/src/session_end.rs".to_string()],
    };
    let comments = vec![comment(1, "ADVISOR BRIEF: merge main")];
    let prompt = assemble(&item_plan(), None, &comments, Some(&conflict));
    assert!(prompt.starts_with("FIRST TASK — resolve conflicts with main first"));
    assert!(prompt.contains("PR #3169"));
    assert!(prompt.contains("session_end.rs"));
    assert!(prompt.contains("git merge origin/main"));
    assert!(prompt.find("ADVISOR BRIEF").unwrap() > prompt.find("FIRST TASK").unwrap());
    assert!(prompt.ends_with("/aida-pickup BUG-051"));

    // No conflict, no direction: the prompt is untouched.
    assert_eq!(
        assemble(&item_plan(), None, &[], None),
        "/aida-pickup BUG-051"
    );
}

#[test]
fn agent_gates_get_the_prompt_unchanged() {
    let _g = crate::test_env::EnvVarsGuard::apply(&[("AIDA_AGENT_GATE_NAME", Some("security"))]);
    let conflict = MainConflict {
        pr: 1,
        base: "main".to_string(),
        files: vec![],
    };
    assert_eq!(
        lead_rework_prompt(
            "p".to_string(),
            &AdvisorDirection::default(),
            None,
            Some(&conflict)
        ),
        "p"
    );
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.test",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn rev_parse(dir: &std::path::Path, rev: &str) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", rev])
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(dir: &std::path::Path, content: &str, msg: &str) {
    std::fs::write(dir.join("f.txt"), content).unwrap();
    git(dir, &["add", "f.txt"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

/// Acceptance (2), through git: a clean PR branch forecasts `None`; after
/// origin's main gains a conflicting commit (seen only via the fetch), the
/// forecast names the conflicting file.
#[test]
fn detect_main_conflict_forecasts_through_fetch_and_merge_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    commit_file(&origin, "base\n", "base");
    git(&origin, &["checkout", "-q", "-b", "pr-branch"]);
    commit_file(&origin, "pr change\n", "pr");
    git(&origin, &["checkout", "-q", "main"]);

    let clone = tmp.path().join("clone");
    let out = crate::test_git::git_command()
        .arg("clone")
        .arg("-q")
        .arg(&origin)
        .arg(&clone)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Each step asserts on its own, so a failure names the step: a refused
    // or failed git run otherwise surfaces only as a `None` forecast.
    assert_eq!(
        crate::worktree_gc::detect_default_branch_ref(&clone).as_deref(),
        Some("origin/main"),
        "default branch of the clone"
    );
    assert!(
        matches!(
            crate::worktree_gc::forecast_rebase_onto(&clone, "origin/main", "origin/pr-branch"),
            crate::integrate::RebaseForecast::Clean
        ),
        "merge-tree forecast before main moves"
    );
    assert_eq!(detect_main_conflict(&clone, 7, "pr-branch"), None);

    // main moves on origin only; the clone learns of it through the fetch.
    commit_file(&origin, "main change\n", "main");
    let conflict = detect_main_conflict(&clone, 7, "pr-branch");
    assert_eq!(
        rev_parse(&clone, "origin/main"),
        rev_parse(&origin, "main"),
        "the forecast's fetch updated origin/main in the clone"
    );
    let conflict = conflict.expect("conflict forecast");
    assert_eq!(conflict.pr, 7);
    assert_eq!(conflict.base, "main");
    assert_eq!(conflict.files, vec!["f.txt".to_string()]);
}
