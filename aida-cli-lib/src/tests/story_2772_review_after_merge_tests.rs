//! STORY-2772: review after merge on the integration track.
// trace:STORY-2772 | ai:claude

use super::*;

use crate::land::{Action, Row, State, Track};
use aida_core::ExecutionMode;

fn low_risk() -> Config {
    Config {
        mode: Mode::LowRisk,
        ..Config::default()
    }
}

fn spec(id: &str, tags: &[&str]) -> (String, Option<SpecFacts>) {
    (
        id.to_string(),
        Some(SpecFacts {
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            mode: None,
        }),
    )
}

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| path.to_string()).collect()
}

fn decide(config: &Config, specs: &[(String, Option<SpecFacts>)], changed: &[&str]) -> Eligibility {
    review_after_merge_eligible(config, specs, &paths(changed), ProtectedPaths::floor_ref())
}

fn refusal(eligibility: Eligibility) -> String {
    match eligibility {
        Eligibility::Ineligible(why) => why,
        Eligibility::Eligible(why) => panic!("expected a refusal, got eligible: {why}"),
    }
}

// ---- config --------------------------------------------------------------

#[test]
fn config_defaults_to_off_with_the_stated_knobs() {
    for body in [None, Some(""), Some("[land]\nwip_max = 3\n")] {
        let config = Config::parse(body).unwrap();
        assert_eq!(config.mode, Mode::Off);
        assert_eq!(config.max_effort, EffortBucket::FourHours);
        assert_eq!(
            config.excluded_tags,
            ["architecture", "security", "gate", "supervised"]
        );
        assert_eq!(config.checks, ["make check-ci-fast"]);
    }
}

#[test]
fn config_reads_every_key() {
    let config = Config::parse(Some(
        "[integration]\nreview_after_merge = \"low-risk\"\nmax_effort = \"1h\"\n\
         excluded_tags = [\"security\"]\nchecks = [\"make check-ci-fast\", \"cargo check\"]\n",
    ))
    .unwrap();
    assert_eq!(config.mode, Mode::LowRisk);
    assert_eq!(config.max_effort, EffortBucket::OneHour);
    assert_eq!(config.excluded_tags, ["security"]);
    assert_eq!(config.checks, ["make check-ci-fast", "cargo check"]);
}

#[test]
fn config_refuses_a_value_it_cannot_read() {
    for (body, names) in [
        (
            "[integration]\nreview_after_merge = \"lowrisk\"\n",
            "review_after_merge",
        ),
        (
            "[integration]\nreview_after_merge = true\n",
            "review_after_merge",
        ),
        ("[integration]\nmax_effort = \"2h\"\n", "max_effort"),
        (
            "[integration]\nexcluded_tags = \"security\"\n",
            "excluded_tags",
        ),
        ("[integration]\nchecks = [1]\n", "checks"),
        ("[integration\n", "config.toml"),
    ] {
        let error = Config::parse(Some(body)).unwrap_err();
        assert!(error.contains(names), "{body:?}: {error}");
    }
}

// ---- eligibility matrix --------------------------------------------------

#[test]
fn eligible_when_every_rule_passes() {
    let specs = [
        spec("BUG-1", &["effort:open:1h"]),
        spec("BUG-2", &["effort:open:4h"]),
    ];
    match decide(&low_risk(), &specs, &["aida-cli-lib/src/now.rs"]) {
        Eligibility::Eligible(why) => {
            assert!(
                why.contains("BUG-1 1h") && why.contains("BUG-2 4h"),
                "{why}"
            )
        }
        Eligibility::Ineligible(why) => panic!("{why}"),
    }
}

#[test]
fn config_off_refuses_everything() {
    let why = refusal(decide(
        &Config::default(),
        &[spec("BUG-1", &["effort:open:15m"])],
        &["README.md"],
    ));
    assert!(why.contains("off"), "{why}");
}

#[test]
fn a_change_naming_no_spec_or_an_unknown_spec_is_refused() {
    let why = refusal(decide(&low_risk(), &[], &["README.md"]));
    assert!(why.contains("names no spec"), "{why}");
    let why = refusal(decide(
        &low_risk(),
        &[("BUG-9".into(), None)],
        &["README.md"],
    ));
    assert!(why.contains("BUG-9 is not a spec"), "{why}");
}

#[test]
fn an_excluded_tag_on_any_spec_refuses() {
    for tag in ["architecture", "security", "gate", "supervised"] {
        let specs = [
            spec("BUG-1", &["effort:open:1h"]),
            spec("BUG-2", &["effort:open:1h", tag]),
        ];
        let why = refusal(decide(&low_risk(), &specs, &["README.md"]));
        assert!(why.contains("BUG-2") && why.contains(tag), "{why}");
    }
}

#[test]
fn a_supervised_execution_mode_refuses() {
    for (mode, eligible) in [
        (ExecutionMode::Drain, true),
        (ExecutionMode::Drive, true),
        (ExecutionMode::Guided, false),
        (ExecutionMode::Operator, false),
        (ExecutionMode::Decide, false),
    ] {
        let mut one = spec("BUG-1", &["effort:open:1h"]);
        one.1.as_mut().unwrap().mode = Some(mode);
        let got = decide(&low_risk(), &[one], &["README.md"]);
        assert_eq!(
            matches!(got, Eligibility::Eligible(_)),
            eligible,
            "{mode:?}: {got:?}"
        );
    }
}

#[test]
fn effort_missing_or_above_the_limit_refuses() {
    let why = refusal(decide(
        &low_risk(),
        &[spec("BUG-1", &["landing"])],
        &["README.md"],
    ));
    assert!(why.contains("no effort estimate"), "{why}");
    // Only the opening estimate counts.
    let why = refusal(decide(
        &low_risk(),
        &[spec("BUG-1", &["effort:impl:15m"])],
        &["README.md"],
    ));
    assert!(why.contains("no effort estimate"), "{why}");
    let why = refusal(decide(
        &low_risk(),
        &[spec("BUG-1", &["effort:open:1d"])],
        &["README.md"],
    ));
    assert!(why.contains("1d") && why.contains("4h"), "{why}");
    let tight = Config {
        max_effort: EffortBucket::OneHour,
        ..low_risk()
    };
    assert!(matches!(
        decide(
            &tight,
            &[spec("BUG-1", &["effort:open:4h"])],
            &["README.md"]
        ),
        Eligibility::Ineligible(_)
    ));
}

#[test]
fn a_protected_path_refuses_and_cannot_be_configured_away() {
    let permissive = Config {
        excluded_tags: Vec::new(),
        max_effort: EffortBucket::OneWeek,
        ..low_risk()
    };
    for path in [
        ".aida/config.toml",
        "aida-cli-lib/src/gate/approval.rs",
        "aida-cli-lib/src/tests/story_1888_gate_paths_tests.rs",
    ] {
        let why = refusal(decide(
            &permissive,
            &[spec("BUG-1", &["effort:open:15m"])],
            &["README.md", path],
        ));
        assert!(why.contains(path), "{why}");
    }
}

// ---- merge admission -----------------------------------------------------

fn row(track: Track, state: State) -> Row {
    Row {
        pr: 7,
        track,
        state,
        action: Action::Wait,
        since: None,
        reason: "the row's reason".into(),
        actor: None,
        ci: None,
    }
}

fn eligible() -> Result<Eligibility, String> {
    Ok(Eligibility::Eligible("low-risk".into()))
}

fn ineligible() -> Result<Eligibility, String> {
    Ok(Eligibility::Ineligible("BUG-1 is tagged `security`".into()))
}

#[test]
fn an_approved_change_merges_without_asking_eligibility() {
    let got = admit(&row(Track::Integration, State::Ready), Ok(None), || {
        panic!("eligibility must not be read for an approved change")
    });
    assert_eq!(got, Ok(Admission::Approved("the row's reason".into())));
}

#[test]
fn an_eligible_unapproved_change_merges_before_review() {
    for state in [State::NeedsReview, State::Reviewing] {
        let got = admit(&row(Track::Integration, state), Ok(None), eligible);
        assert_eq!(got, Ok(Admission::ReviewAfterMerge("low-risk".into())));
    }
}

#[test]
fn an_ineligible_unapproved_change_is_refused_with_the_reason() {
    let why = admit(
        &row(Track::Integration, State::NeedsReview),
        Ok(None),
        ineligible,
    )
    .unwrap_err();
    assert!(why.contains("tagged `security`"), "{why}");
    let why = admit(
        &row(Track::Integration, State::NeedsReview),
        Ok(None),
        || Err("the store could not be read".into()),
    )
    .unwrap_err();
    assert!(why.contains("could not be read"), "{why}");
}

#[test]
fn an_armed_or_unreadable_hold_refuses_approved_and_eligible_alike() {
    for state in [State::Ready, State::NeedsReview] {
        let why = admit(
            &row(Track::Integration, state.clone()),
            Ok(Some("rework hold: findings".into())),
            eligible,
        )
        .unwrap_err();
        assert!(why.contains("merge hold is armed"), "{why}");
        let why = admit(
            &row(Track::Integration, state),
            Err("the forge did not answer".into()),
            eligible,
        )
        .unwrap_err();
        assert!(why.contains("could not be read"), "{why}");
    }
}

#[test]
fn every_other_state_and_the_main_track_refuse() {
    for state in [
        State::Landing,
        State::Conflict,
        State::CiFailed,
        State::ReadyPendingCover,
        State::ChangesRequested,
        State::Blocked,
        State::Unknown,
    ] {
        let label = state.label();
        let why = admit(&row(Track::Integration, state), Ok(None), eligible).unwrap_err();
        assert!(why.contains(label), "{why}");
    }
    let why = admit(&row(Track::Main, State::Ready), Ok(None), eligible).unwrap_err();
    assert!(why.contains("does not target"), "{why}");
}

// ---- promotion check -----------------------------------------------------

const HEAD_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MAIN_1: &str = "1111111111111111111111111111111111111111";

fn merge(sha: &str, second: &str, subject: &str) -> FirstParent {
    FirstParent {
        sha: sha.to_string(),
        parents: vec!["0000000".into(), second.to_string()],
        tree: format!("tree-{sha}"),
        subject: subject.to_string(),
    }
}

fn approved_at(sha: &str) -> Option<Verdict> {
    Some(Verdict {
        approves: true,
        label: "APPROVED".into(),
        sha: Some(sha.to_string()),
    })
}

fn on_main(sha: &str) -> bool {
    sha == MAIN_1
}

fn history() -> Vec<FirstParent> {
    vec![
        merge(
            "m3",
            HEAD_B,
            "chore(integration): merge PR #12 second (BUG-2)",
        ),
        merge(
            "m2",
            MAIN_1,
            "[AI:claude] chore(integration): merge main 1111111 (STORY-2737)",
        ),
        merge(
            "m1",
            HEAD_A,
            "[AI:claude] chore(integration): merge PR #11 first (BUG-1)",
        ),
    ]
}

#[test]
fn a_promotion_is_refused_naming_the_pending_constituent() {
    let verdict = |pr: u64| if pr == 11 { approved_at(HEAD_A) } else { None };
    let (approved, findings) = promotion_check(&history(), &on_main, &verdict);
    assert_eq!(approved.iter().map(|c| c.pr).collect::<Vec<_>>(), [11]);
    assert_eq!(findings.len(), 1);
    let text = findings[0].describe();
    assert!(text.contains("#12"), "{text}");
    assert!(text.contains("git revert -m 1 m3"), "{text}");
    assert!(text.contains("no review verdict"), "{text}");
}

#[test]
fn a_promotion_is_allowed_once_every_constituent_is_approved_at_its_merged_head() {
    let verdict = |pr: u64| match pr {
        11 => approved_at(HEAD_A),
        // A recorded short sha matches the head it abbreviates.
        12 => approved_at(&HEAD_B[..12]),
        _ => None,
    };
    let (approved, findings) = promotion_check(&history(), &on_main, &verdict);
    assert_eq!(findings, []);
    assert_eq!(approved.iter().map(|c| c.pr).collect::<Vec<_>>(), [12, 11]);
    assert_eq!(approved[0].head, HEAD_B);
}

#[test]
fn an_approval_at_another_head_or_a_later_refusal_does_not_count() {
    let verdict = |pr: u64| match pr {
        11 => approved_at("cccccccccccc"),
        _ => Some(Verdict {
            approves: false,
            label: "CHANGES REQUESTED".into(),
            sha: Some(HEAD_B.into()),
        }),
    };
    let (approved, findings) = promotion_check(&history(), &on_main, &verdict);
    assert!(approved.is_empty());
    let text: Vec<String> = findings.iter().map(Finding::describe).collect();
    assert!(
        text[0].contains("#12") && text[0].contains("CHANGES REQUESTED"),
        "{text:?}"
    );
    assert!(
        text[1].contains("#11") && text[1].contains("not at the head"),
        "{text:?}"
    );
    // An approval that names no commit, or too short a one, approves nothing.
    for sha in [None, Some("aaaa".to_string())] {
        let verdict = |_: u64| {
            Some(Verdict {
                approves: true,
                label: "APPROVED".into(),
                sha: sha.clone(),
            })
        };
        let (approved, _) = promotion_check(&history(), &on_main, &verdict);
        assert!(approved.is_empty(), "{sha:?}");
    }
}

#[test]
fn a_direct_commit_or_an_unnamed_merge_is_refused() {
    let commits = vec![
        FirstParent {
            sha: "d1".into(),
            parents: vec!["0000000".into()],
            tree: "t".into(),
            subject: "fix(ci): straight onto integration (BUG-3)".into(),
        },
        merge("m9", HEAD_A, "Merge branch 'side' into integration"),
        // A direct commit that only talks about a merge is still direct.
        FirstParent {
            sha: "d2".into(),
            parents: vec!["0000000".into()],
            tree: "t2".into(),
            subject: "chore(integration): merge PR #11 by hand".into(),
        },
    ];
    let (approved, findings) = promotion_check(&commits, &on_main, &|_| approved_at(HEAD_A));
    assert!(approved.is_empty());
    assert_eq!(findings.len(), 3);
    assert!(findings
        .iter()
        .all(|finding| matches!(finding, Finding::Unreviewed { .. })));
}

#[test]
fn commits_at_or_before_the_last_promoted_head_are_not_checked() {
    let commits = history();
    // main holds a squash with m2's tree: m2 and m1 are promoted.
    let main_trees: HashSet<String> = ["tree-m2".to_string()].into();
    let left = unpromoted(&commits, &main_trees);
    assert_eq!(
        left.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(),
        ["m3"]
    );
    assert_eq!(unpromoted(&commits, &HashSet::new()).len(), 3);
}

#[test]
fn merge_subjects_carry_the_change_and_its_trailer() {
    assert_eq!(
        merge_subject(
            4000,
            "[AI:codex] feat(deputy): supervise script runners (STORY-2775)"
        ),
        "chore(integration): merge PR #4000 supervise script runners (STORY-2775)"
    );
    assert_eq!(
        merge_subject(5, "fix(now): one line (BUG-1)"),
        "chore(integration): merge PR #5 one line (BUG-1)"
    );
    assert_eq!(
        merge_subject(6, "No conventional lead (BUG-2)"),
        "chore(integration): merge PR #6 No conventional lead (BUG-2)"
    );
    for subject in [
        merge_subject(4000, "[AI:codex] feat(deputy): x (STORY-2775)"),
        "[AI:claude] chore(integration): merge PR #4000 x".to_string(),
    ] {
        assert_eq!(merged_pr(&subject), Some(4000), "{subject}");
    }
    assert_eq!(merged_pr("chore(integration): merge main 1801b34"), None);
}

// ---- the merge itself ----------------------------------------------------

fn sh_git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository on `integration` with a side branch one commit ahead.
fn repo() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    sh_git(root, &["init", "-q", "-b", "integration"]);
    sh_git(root, &["config", "user.name", "Test"]);
    sh_git(root, &["config", "user.email", "test@example.invalid"]);
    sh_git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("base.txt"), "base\n").unwrap();
    sh_git(root, &["add", "."]);
    sh_git(root, &["commit", "-q", "-m", "base"]);
    let before = sh_git(root, &["rev-parse", "HEAD"]);
    sh_git(root, &["checkout", "-q", "-b", "side"]);
    std::fs::write(root.join("change.txt"), "change\n").unwrap();
    sh_git(root, &["add", "."]);
    sh_git(root, &["commit", "-q", "-m", "change"]);
    let head = sh_git(root, &["rev-parse", "HEAD"]);
    sh_git(root, &["checkout", "-q", "integration"]);
    (dir, before, head)
}

#[test]
fn a_merge_whose_checks_pass_records_the_head_and_the_reason() {
    let (dir, _, head) = repo();
    let tip = super::super::merge_checked(
        dir.path(),
        &head,
        "chore(integration): merge PR #7 change (BUG-1)",
        "Review after merge: low-risk.",
        &["test -f change.txt".to_string()],
    )
    .unwrap();
    assert_eq!(sh_git(dir.path(), &["rev-parse", "HEAD"]), tip);
    assert_eq!(sh_git(dir.path(), &["rev-parse", "HEAD^2"]), head);
    let message = sh_git(dir.path(), &["log", "-1", "--format=%B"]);
    assert!(message.starts_with("chore(integration): merge PR #7 change (BUG-1)"));
    assert!(
        message.contains("Review after merge: low-risk."),
        "{message}"
    );
}

#[test]
fn a_failing_check_leaves_integration_where_it_was() {
    let (dir, before, head) = repo();
    let error = super::super::merge_checked(
        dir.path(),
        &head,
        "chore(integration): merge PR #7 change (BUG-1)",
        "body",
        &["true".to_string(), "false".to_string()],
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("`false` failed"), "{error:#}");
    assert_eq!(sh_git(dir.path(), &["rev-parse", "HEAD"]), before);
    assert!(!dir.path().join("change.txt").exists());
    assert_eq!(sh_git(dir.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_conflicting_merge_leaves_integration_where_it_was() {
    let (dir, _, head) = repo();
    std::fs::write(dir.path().join("change.txt"), "other\n").unwrap();
    sh_git(dir.path(), &["add", "."]);
    sh_git(dir.path(), &["commit", "-q", "-m", "collides"]);
    let before = sh_git(dir.path(), &["rev-parse", "HEAD"]);
    let error = super::super::merge_checked(dir.path(), &head, "subject", "body", &[]).unwrap_err();
    assert!(
        format!("{error:#}").contains("did not complete"),
        "{error:#}"
    );
    assert_eq!(sh_git(dir.path(), &["rev-parse", "HEAD"]), before);
    assert_eq!(sh_git(dir.path(), &["status", "--porcelain"]), "");
}

// ---- land status ---------------------------------------------------------

struct Unapproved(Option<&'static str>);

impl crate::land::Evidence for Unapproved {
    fn conflict(&self, _: &crate::land::Pr) -> Result<bool, String> {
        Ok(false)
    }
    fn reviewing(&self, _: &crate::land::Pr) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn ci(&self, _: &crate::land::Pr) -> Result<crate::land::Ci, String> {
        Ok(crate::land::Ci::Green)
    }
    fn approval(&self, _: &crate::land::Pr) -> Result<crate::land::Approval, String> {
        Ok(crate::land::Approval::Refused(
            "no approval covers the head".into(),
        ))
    }
    fn hold(&self, _: &crate::land::Pr) -> Result<Option<crate::land::Hold>, String> {
        Ok(None)
    }
    fn rounds(&self, _: &crate::land::Pr) -> Result<crate::land::Rounds, String> {
        Ok(Default::default())
    }
    fn lane(&self, _: &crate::land::Pr) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn review_after_merge(&self, _: &crate::land::Pr) -> Option<String> {
        self.0.map(String::from)
    }
}

fn change(base: &str) -> crate::land::Pr {
    crate::land::Pr {
        id: 7,
        head: HEAD_A.into(),
        base: base.into(),
        branch: Some("bug-1".into()),
        title: "fix(now): one line (BUG-1)".into(),
        updated_at: None,
        hold_label: None,
        ci: None,
    }
}

#[test]
fn land_status_shows_the_eligibility_reason_on_integration_rows_only() {
    let note = "review before merge: BUG-1 is tagged `security`";
    let row = crate::land::derive(&change("integration"), &Unapproved(Some(note)));
    assert_eq!(row.state, State::NeedsReview);
    assert_eq!(row.reason, format!("no approval covers the head; {note}"));
    let row = crate::land::derive(&change("main"), &Unapproved(Some(note)));
    assert_eq!(row.reason, "no approval covers the head");
    let row = crate::land::derive(&change("integration"), &Unapproved(None));
    assert_eq!(row.reason, "no approval covers the head");
}
