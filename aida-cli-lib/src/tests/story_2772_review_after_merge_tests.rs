//! STORY-2772: review after merge on the integration track.
// trace:STORY-2772 STORY-b6b1278890774ef993d2ebc327862470-695.Tests | ai:claude

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
// trace:STORY-b6b1278890774ef993d2ebc327862470-695.ace3fe97 | ai:claude

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
        // A misspelt key is refused, not dropped in favour of the default.
        (
            "[integration]\nexclude_tags = [\"security\"]\n",
            "exclude_tags",
        ),
        ("[integration]\nmax_efort = \"1h\"\n", "max_efort"),
        ("integration = \"low-risk\"\n", "must be a table"),
    ] {
        let error = Config::parse(Some(body)).unwrap_err();
        assert!(error.contains(names), "{body:?}: {error}");
    }
}

// ---- eligibility matrix --------------------------------------------------

// The one eligibility function and its matrix.
// trace:STORY-b6b1278890774ef993d2ebc327862470-695.acbbdc03 STORY-b6b1278890774ef993d2ebc327862470-695.Tests | ai:claude
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

// trace:STORY-b6b1278890774ef993d2ebc327862470-695.ac77df43 | ai:claude
#[test]
fn an_eligible_unapproved_change_merges_before_review() {
    let got = admit(
        &row(Track::Integration, State::NeedsReview),
        Ok(None),
        eligible,
    );
    assert_eq!(got, Ok(Admission::ReviewAfterMerge("low-risk".into())));
}

/// A row under review was derived before CI, the approval and the review
/// rounds were read, so it is refused without asking eligibility.
#[test]
fn a_change_under_review_is_refused_without_asking_eligibility() {
    let why = admit(&row(Track::Integration, State::Reviewing), Ok(None), || {
        panic!("eligibility must not be read for a change under review")
    })
    .unwrap_err();
    assert!(why.contains("under review"), "{why}");
    assert!(why.contains("the row's reason"), "{why}");
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-695.ac77df43 | ai:claude
#[test]
fn only_a_merge_before_review_marks_its_known_unmarked_specs() {
    let specs = vec![
        ("BUG-1".to_string(), Some((1u8, vec!["bug".to_string()]))),
        (
            "BUG-2".to_string(),
            Some((2u8, vec![MERGED_BEFORE_REVIEW_TAG.to_string()])),
        ),
        ("BUG-3".to_string(), None),
    ];
    let before_review = Admission::ReviewAfterMerge("low-risk".into());
    assert_eq!(specs_to_mark(&before_review, &specs), [("BUG-1", &1u8)]);
    let approved = Admission::Approved("approved at the head".into());
    assert!(specs_to_mark(&approved, &specs).is_empty());
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
// trace:STORY-b6b1278890774ef993d2ebc327862470-695.acf93ff9 STORY-b6b1278890774ef993d2ebc327862470-695.Tests | ai:claude

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
        authority: Ok(()),
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

// trace:STORY-b6b1278890774ef993d2ebc327862470-695.acf93ff9 | ai:claude
#[test]
fn a_promotion_is_refused_naming_the_pending_constituent() {
    let verdict = |pr: u64, _: &str| if pr == 11 { approved_at(HEAD_A) } else { None };
    let (approved, findings) = promotion_check(&history(), &on_main, &verdict);
    assert_eq!(approved.iter().map(|c| c.pr).collect::<Vec<_>>(), [11]);
    assert_eq!(findings.len(), 1);
    let text = findings[0].describe();
    assert!(text.contains("#12"), "{text}");
    assert!(text.contains("git revert -m 1 m3"), "{text}");
    assert!(text.contains("no review verdict"), "{text}");
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-695.Tests | ai:claude
#[test]
fn a_promotion_is_allowed_once_every_constituent_is_approved_at_its_merged_head() {
    let verdict = |pr: u64, _: &str| match pr {
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
    let verdict = |pr: u64, _: &str| match pr {
        11 => approved_at("cccccccccccc"),
        _ => Some(Verdict {
            approves: false,
            label: "CHANGES REQUESTED".into(),
            sha: Some(HEAD_B.into()),
            authority: Ok(()),
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
        let verdict = |_: u64, _: &str| {
            Some(Verdict {
                approves: true,
                label: "APPROVED".into(),
                sha: sha.clone(),
                authority: Ok(()),
            })
        };
        let (approved, _) = promotion_check(&history(), &on_main, &verdict);
        assert!(approved.is_empty(), "{sha:?}");
    }
}

/// The ship gate's authority check decides: an approval at the merged head
/// that its author recorded, or that no attested recorder stands behind, is
/// not a review.
#[test]
fn an_approval_without_the_ship_gates_authority_is_refused() {
    use crate::gate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
    use crate::gate::ship::{AuthorSession, ShipApprovalPolicy};
    const AT: &str = "2026-11-14T05:30:00Z";
    let approval = |attestation: Option<RecorderAttestation>| RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(HEAD_B.into()),
        recorded_at: Some(AT.into()),
        recorded_by: Some("aida review record (operator)".into()),
        attestation,
        ..Default::default()
    };
    let attested = |identity: &str| RecorderAttestation {
        sha: HEAD_B.into(),
        human_at_tty: true,
        receipt_id: Some("rcpt-1".into()),
        identity: vec![identity.to_string()],
        ..Default::default()
    };
    let policy = |candidates: &[RecordedVerdict]| {
        let mut policy = ShipApprovalPolicy {
            author_sessions: vec![AuthorSession {
                label: "the lane that wrote the change".into(),
                tokens: vec!["session:author".into()],
            }],
            reviewed_committed_at: Some("2026-11-14T05:00:00Z".parse().unwrap()),
            ..Default::default()
        };
        for verdict in candidates {
            if let Some(attestation) = verdict.attestation.as_ref() {
                if let Some(key) = attestation.authority_key(verdict.recorded_at.as_deref()) {
                    policy
                        .verified_authority
                        .insert(key, attestation.identity.clone());
                }
            }
        }
        policy
    };
    // Recorded by the authoring session itself.
    let own = [approval(Some(attested("session:author")))];
    let why =
        crate::gate::approval::approval_authority_of(12, &own, &policy(&own), true).unwrap_err();
    assert!(why.contains("cannot approve its own work"), "{why}");
    // No attested recorder at all, as a legacy verdict file has it.
    let bare = [approval(None)];
    let why =
        crate::gate::approval::approval_authority_of(12, &bare, &policy(&bare), true).unwrap_err();
    assert!(why.contains("PR-12"), "{why}");
    // Another session's attested approval passes; unreadable does not.
    let other = [approval(Some(attested("session:reviewer")))];
    assert_eq!(
        crate::gate::approval::approval_authority_of(12, &other, &policy(&other), true),
        Ok(())
    );
    let why = crate::gate::approval::approval_authority_of(12, &other, &policy(&other), false)
        .unwrap_err();
    assert!(why.contains("could not be checked"), "{why}");

    // And the check refuses the promotion on it, naming the change.
    let verdict = |pr: u64, _: &str| match pr {
        11 => approved_at(HEAD_A),
        _ => Some(Verdict {
            authority: Err("PR-12: the author cannot approve its own work".into()),
            ..approved_at(HEAD_B).unwrap()
        }),
    };
    let (approved, findings) = promotion_check(&history(), &on_main, &verdict);
    assert_eq!(approved.iter().map(|c| c.pr).collect::<Vec<_>>(), [11]);
    let text = findings[0].describe();
    assert!(
        text.contains("#12") && text.contains("its own work"),
        "{text}"
    );
}

/// A spec keeps its mark while any change naming it is still pending.
#[test]
fn a_spec_is_unmarked_only_when_no_pending_change_names_it() {
    let constituent = |pr: u64, subject: &str| Constituent {
        pr,
        merge: format!("m{pr}"),
        head: HEAD_A.into(),
        subject: subject.into(),
    };
    let approved = [
        constituent(11, "chore(integration): merge PR #11 a (BUG-1 BUG-2)"),
        constituent(13, "chore(integration): merge PR #13 c (BUG-3)"),
    ];
    let findings = [
        Finding::Pending {
            constituent: constituent(12, "chore(integration): merge PR #12 b (bug-2)"),
            why: "no review verdict is recorded".into(),
        },
        Finding::Unreviewed {
            sha: "d1".into(),
            subject: "fix: a direct commit (BUG-3)".into(),
        },
    ];
    assert_eq!(specs_to_unmark(&approved, &findings), ["BUG-1"]);
    assert_eq!(specs_to_unmark(&approved, &[]), ["BUG-1", "BUG-2", "BUG-3"]);
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
    let (approved, findings) = promotion_check(&commits, &on_main, &|_, _| approved_at(HEAD_A));
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
fn a_named_boundary_cuts_the_check_and_an_unknown_one_is_refused() {
    let mut commits = history();
    for (commit, sha) in commits.iter_mut().zip([HEAD_B, MAIN_1, HEAD_A]) {
        commit.sha = sha.to_string();
    }
    let shas = |left: Option<&[FirstParent]>| {
        left.map(|left| left.iter().map(|c| c.sha.clone()).collect::<Vec<_>>())
    };
    assert_eq!(
        shas(after(&commits, MAIN_1)),
        Some(vec![HEAD_B.to_string()])
    );
    assert_eq!(
        shas(after(&commits, &HEAD_A[..7])).map(|left| left.len()),
        Some(2)
    );
    assert_eq!(shas(after(&commits, HEAD_B)), Some(vec![]));
    // Off the line, or too short to name one commit: never "check nothing".
    assert_eq!(after(&commits, "cccccccccccc"), None);
    assert_eq!(after(&commits, "aaaa"), None);
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
    let out = crate::test_git::git_command()
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

/// A protected file moved out of its path is still listed at the protected
/// path, so the move cannot make the change eligible.
// trace:STORY-b6b1278890774ef993d2ebc327862470-695.ace3fe97 | ai:claude
#[test]
fn a_rename_out_of_a_protected_path_still_lists_it_and_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    sh_git(root, &["init", "-q", "-b", "integration"]);
    sh_git(root, &["config", "user.name", "Test"]);
    sh_git(root, &["config", "user.email", "test@example.invalid"]);
    sh_git(root, &["config", "commit.gpgsign", "false"]);
    let protected = "aida-cli-lib/src/gate/approval.rs";
    std::fs::create_dir_all(root.join("aida-cli-lib/src/gate")).unwrap();
    let body: String = (0..40).map(|n| format!("fn rule_{n}() {{}}\n")).collect();
    std::fs::write(root.join(protected), body).unwrap();
    sh_git(root, &["add", "."]);
    sh_git(root, &["commit", "-q", "-m", "base"]);
    sh_git(root, &["checkout", "-q", "-b", "side"]);
    sh_git(root, &["mv", protected, "aida-cli-lib/src/approval.rs"]);
    sh_git(root, &["commit", "-q", "-m", "move the gate file"]);

    let changed = crate::land::changed_paths(root, "integration", "side").unwrap();
    assert!(changed.iter().any(|path| path == protected), "{changed:?}");
    let changed: Vec<&str> = changed.iter().map(String::as_str).collect();
    let why = refusal(decide(
        &low_risk(),
        &[spec("BUG-1", &["effort:open:1h"])],
        &changed,
    ));
    assert!(why.contains(protected), "{why}");
}

/// Git quotes a name with a tab, a newline or non-ASCII bytes in plain
/// `--name-only` output; read that way, a quoted name under a protected
/// directory starts with `"` and slips past the prefix match. The `-z`
/// read keeps each name as stored, so built-in and project protected
/// directories both refuse it.
// trace:STORY-2772 | ai:claude
#[test]
fn quoted_names_under_protected_paths_are_listed_as_stored_and_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    sh_git(root, &["init", "-q", "-b", "integration"]);
    sh_git(root, &["config", "user.name", "Test"]);
    sh_git(root, &["config", "user.email", "test@example.invalid"]);
    sh_git(root, &["config", "commit.gpgsign", "false"]);
    sh_git(root, &["config", "core.quotePath", "true"]);
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    sh_git(root, &["add", "."]);
    sh_git(root, &["commit", "-q", "-m", "base"]);
    sh_git(root, &["checkout", "-q", "-b", "side"]);
    let names = [
        "aida-cli-lib/src/gate/tab\there.rs",
        "aida-cli-lib/src/gate/new\nline.rs",
        "aida-cli-lib/src/gate/caf\u{e9}.rs",
        "project-guarded/caf\u{e9}\tx.txt",
    ];
    for name in names {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x\n").unwrap();
    }
    sh_git(root, &["add", "."]);
    sh_git(root, &["commit", "-q", "-m", "odd names"]);

    let changed = crate::land::changed_paths(root, "integration", "side").unwrap();
    let mut sorted = changed.clone();
    sorted.sort();
    let mut want: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    want.sort();
    assert_eq!(sorted, want);
    assert!(
        changed.iter().all(|path| !path.starts_with('"')),
        "{changed:?}"
    );

    let mut protected = ProtectedPaths::floor();
    protected.extend(
        crate::gate::mandate_gates::PathSource::Committed,
        ["project-guarded/".to_string()],
    );
    for name in names {
        let why = refusal(review_after_merge_eligible(
            &low_risk(),
            &[spec("BUG-1", &["effort:open:1h"])],
            &[name.to_string()],
            &protected,
        ));
        assert!(why.contains(name), "{name:?}: {why}");
    }
}

/// A name that is not UTF-8 is refused, not guessed at.
// trace:STORY-2772 | ai:claude
#[test]
fn a_changed_path_that_is_not_utf8_is_refused() {
    let out = b"ok.rs\0aida-cli-lib/src/gate/\xff.rs\0";
    let error = crate::land::nul_separated_paths(out).unwrap_err();
    assert!(error.contains("not UTF-8"), "{error}");
    assert_eq!(
        crate::land::nul_separated_paths(b"a.rs\0b c.rs\0").unwrap(),
        ["a.rs", "b c.rs"]
    );
}

// ---- land status ---------------------------------------------------------
// trace:STORY-b6b1278890774ef993d2ebc327862470-695.acbbdc03 | ai:claude

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

// ---- review round 3 ------------------------------------------------------

fn direct(sha: &str, subject: &str) -> FirstParent {
    FirstParent {
        sha: sha.to_string(),
        parents: vec!["0000000".into()],
        tree: format!("tree-{sha}"),
        subject: subject.to_string(),
    }
}

/// A pending change and the commit that exactly reverts it both leave the
/// findings; a revert that is not exact, or that precedes the merge, clears
/// nothing; one revert accounts for one change.
// trace:STORY-b6b1278890774ef993d2ebc327862470-695.acf93ff9 | ai:claude
#[test]
fn a_pending_change_that_was_reverted_out_no_longer_refuses_the_promotion() {
    let mut commits = history();
    commits.insert(0, direct("r3", "Revert \"merge PR #12 second\""));
    let verdict = |pr: u64, _: &str| if pr == 11 { approved_at(HEAD_A) } else { None };
    let (_, findings) = promotion_check(&commits, &on_main, &verdict);
    assert_eq!(findings.len(), 2, "{findings:?}");

    let exact = |c: &Constituent, commit: &FirstParent| c.merge == "m3" && commit.sha == "r3";
    assert!(drop_reverted(findings.clone(), &commits, &exact).is_empty());

    // A revert that also changes other content is not exact: both stay.
    let never = |_: &Constituent, _: &FirstParent| false;
    assert_eq!(drop_reverted(findings.clone(), &commits, &never), findings);

    // A direct commit older than the merge cannot have reverted it.
    let mut older = history();
    older.push(direct("r0", "earlier"));
    let (_, findings) = promotion_check(&older, &on_main, &verdict);
    let any = |_: &Constituent, _: &FirstParent| true;
    assert_eq!(drop_reverted(findings.clone(), &older, &any), findings);

    // Two pending changes, one revert: one change is still refused.
    let mut two = history();
    two.insert(0, direct("r3", "Revert"));
    let (_, findings) = promotion_check(&two, &on_main, &|_, _| None);
    let left = drop_reverted(findings, &two, &any);
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(matches!(&left[0], Finding::Pending { constituent, .. } if constituent.pr == 11));

    // The remedy the refusal prints is the one that clears it.
    let (_, findings) = promotion_check(&history(), &on_main, &verdict);
    assert!(findings[0].describe().contains("`git revert -m 1 m3`"));
}

/// The live probe against real git: `git revert -m 1 <merge>` is an exact
/// revert; the same revert with another change on top of it is not.
// trace:STORY-2772 | ai:claude
#[test]
fn git_revert_of_the_merge_is_exact_and_a_revert_with_extra_content_is_not() {
    let (dir, _, head) = repo();
    let root = dir.path();
    sh_git(
        root,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge PR #7 change (BUG-1)",
            &head,
        ],
    );
    let merge = sh_git(root, &["rev-parse", "HEAD"]);
    // An unrelated later commit, so the revert does not sit on the merge.
    std::fs::write(root.join("later.txt"), "later\n").unwrap();
    sh_git(root, &["add", "."]);
    sh_git(root, &["commit", "-q", "-m", "later"]);
    let commit_at = |rev: &str| FirstParent {
        sha: sh_git(root, &["rev-parse", rev]),
        parents: sh_git(root, &["log", "-1", "--format=%P", rev])
            .split_whitespace()
            .map(String::from)
            .collect(),
        tree: sh_git(root, &["rev-parse", &format!("{rev}^{{tree}}")]),
        subject: String::new(),
    };
    assert!(!super::super::is_exact_revert(
        root,
        &merge,
        &commit_at("HEAD")
    ));
    sh_git(root, &["revert", "--no-edit", "-m", "1", &merge]);
    assert!(!root.join("change.txt").exists());
    assert!(super::super::is_exact_revert(
        root,
        &merge,
        &commit_at("HEAD")
    ));
    // The merge itself (two parents) is never a revert.
    assert!(!super::super::is_exact_revert(
        root,
        &merge,
        &commit_at(&merge)
    ));

    std::fs::write(root.join("extra.txt"), "extra\n").unwrap();
    sh_git(root, &["add", "."]);
    sh_git(root, &["commit", "-q", "--amend", "--no-edit"]);
    assert!(!super::super::is_exact_revert(
        root,
        &merge,
        &commit_at("HEAD")
    ));
}

/// A gate config that cannot be read leaves the project's protected paths
/// unknown: eligibility is an error (the caller refuses), never a check
/// against the built-in paths alone.
// trace:STORY-b6b1278890774ef993d2ebc327862470-695.ace3fe97 | ai:claude
#[test]
fn an_unreadable_gate_config_makes_eligibility_unknown_not_floor_only() {
    use crate::gate::mandate_gates::{HumanGates, PathSource};
    let unreadable: Result<HumanGates, String> = Err("bad toml".into());
    let error = protected_or_unknown(unreadable).unwrap_err();
    assert!(error.contains("protected paths are unknown"), "{error}");
    assert!(error.contains("bad toml"), "{error}");

    let mut gates = HumanGates {
        protected: ProtectedPaths::floor(),
        ..Default::default()
    };
    gates
        .protected
        .extend(PathSource::Committed, ["billing/".to_string()]);
    let protected = protected_or_unknown(Ok::<_, String>(gates)).unwrap();
    let has = |pattern: &str| protected.entries().iter().any(|e| e.pattern == pattern);
    assert!(has("billing/"));
    for built_in in ProtectedPaths::floor().entries() {
        assert!(has(&built_in.pattern), "{}", built_in.pattern);
    }
    // And the project path decides: a change touching it is not eligible.
    let refused = review_after_merge_eligible(
        &low_risk(),
        &[spec("BUG-1", &["effort:open:1h"])],
        &paths(&["billing/invoice.rs"]),
        &protected,
    );
    let why = refusal(refused);
    assert!(why.contains("billing/"), "{why}");
}

#[test]
fn an_excluded_tag_matches_whatever_its_case() {
    let why = refusal(decide(
        &low_risk(),
        &[spec("BUG-1", &["Security", "effort:open:1h"])],
        &["src/a.rs"],
    ));
    assert!(why.contains("Security"), "{why}");
}
