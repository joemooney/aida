//! Tests of the merge-hold core: the record types, the hold read path and
//! the hold decisions (STORY-1889, moved from `merge_hold.rs`).
// trace:STORY-1889 | ai:claude
use super::*;

// STORY-1165: the label mirror must route to the right forge CLI — gh for
// GitHub, glab for GitLab (mr update --label/--unlabel), nothing for pure-git.
fn pin(kind: crate::forge::ForgeKind, origin: &str) -> PinnedRepo {
    PinnedRepo::from_origin(kind, origin).expect("origin must pin")
}

fn recused_record() -> MergeHoldRecord {
    let mut record = typed_hold(
        42,
        HoldReasonKind::Recusal,
        "author recused",
        Some("head-a".into()),
    );
    record.recused_principals = vec![PrincipalIdentity::parse("agent:author")];
    record
}

// TASK-1582 acceptance 1: with a hold armed, the next-action line names
// `aida merge-hold clear` BEFORE `aida pr ship` (no circular hints).
// trace:TASK-1582.ac14557d | ai:antigravity
#[test]
fn task_1582_release_met_next_action_names_clear_before_ship() {
    let line = release_met_next_action(2386, "TASK-1", "abc1234");
    let clear = line
        .find("`aida merge-hold clear 2386`")
        .unwrap_or_else(|| panic!("no clear step: {line}"));
    let ship = line
        .find("`aida pr ship 2386`")
        .unwrap_or_else(|| panic!("no ship step: {line}"));
    assert!(clear < ship, "clear must come first: {line}");
    assert!(line.contains("APPROVED for TASK-1 at abc1234"), "{line}");
    assert!(line.contains("still armed"), "{line}");
    assert!(line.contains("pins the merge to abc1234"), "{line}");
    assert!(!line.contains("may now `aida pr ship"), "{line}");
}

// trace:BUG-1693 | ai:codex
#[test]
fn unrecorded_manual_marker_removal_is_detected_but_recorded_clear_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let record = typed_hold(4321, HoldReasonKind::Supervision, "fixture", None);
    write_typed_hold(root, &record).unwrap();
    let mut placed = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MergeHoldChanged {
            pr: 4321,
            placed: true,
            reason: Some("fixture".into()),
            principal: None,
            human_attested: false,
        },
    );
    std::fs::remove_file(hold_path(root, 4321)).unwrap();
    assert_eq!(unrecorded_marker_removals(root), vec![4321]);
    assert!(read_hold(root, 4321).is_some());

    record_clearance(root, &record, &PrincipalIdentity::human("joe")).unwrap();
    placed.kind = crate::events::EventKind::MergeHoldChanged {
        pr: 4321,
        placed: false,
        reason: Some("fixture cleared".into()),
        principal: None,
        human_attested: false,
    };
    crate::events::emit(root, &placed);
    assert!(unrecorded_marker_removals(root).is_empty());
}

// BUG-1693: `read_hold` gained a SECOND meaning — it now answers `Some`
// for a PR with no marker at all when AIDA recorded placing one. Every
// caller reads that as "held", which is the fail-closed behaviour we want
// at the merge chokepoint (forge.rs refuses the merge). Pin both halves of
// that invariant, and pin that a clearance recorded by the human-gated
// path is what re-opens the PR — otherwise a tampered PR is unmergeable
// through AIDA forever.
// trace:BUG-1693 | ai:claude
#[test]
fn tampered_hold_reads_as_held_until_a_clearance_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let record = typed_hold(8801, HoldReasonKind::Supervision, "supervised spec", None);
    write_typed_hold(root, &record).unwrap();
    std::fs::remove_file(hold_path(root, 8801)).unwrap();

    // Fail closed: the merge chokepoint must still see a hold, and it must
    // say WHY, not repeat the deleted marker's reason as if it were read.
    assert_eq!(
        read_hold(root, 8801).as_deref(),
        Some(MARKER_MISSING_REASON)
    );

    // …but the synthetic value must not masquerade as a hold RECORD:
    // `list_holds` walks the marker directory, so a deleted marker has no
    // entry and cannot be reported as a live hold with a real reason.
    assert!(
        list_holds(root).iter().all(|(pr, _)| *pr != 8801),
        "a missing marker must not appear as a live hold record"
    );
    assert!(read_hold_record(root, 8801).is_none());

    // The human-gated clearance is the door back out. After it, the PR
    // reads unheld and the tampering report stops.
    record_clearance(
        root,
        &typed_hold(8801, HoldReasonKind::Unknown, MARKER_MISSING_REASON, None),
        &PrincipalIdentity::human("joe"),
    )
    .unwrap();
    assert!(unrecorded_marker_removals(root).is_empty());
    assert!(read_hold(root, 8801).is_none());
}

// trace:BUG-1747 | ai:codex
#[test]
fn label_definition_failure_keeps_the_local_hold_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q"])
        .status()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["remote", "add", "origin", "git@github.com:o/r.git"])
        .status()
        .unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    write_typed_hold(
        dir.path(),
        &typed_hold(44, HoldReasonKind::Supervision, "test", None),
    )
    .unwrap();
    let err = sync_label_with(dir.path(), 44, true, |_, _, _| {
        Ok((false, "'aida:merge-hold-cleared' not found".into()))
    })
    .unwrap_err();
    assert!(hold_path(dir.path(), 44).exists());
    assert!(read_hold(dir.path(), 44).is_some());
    assert!(matches!(
        read_label_state(dir.path(), 44),
        LabelState::Unsynced(_)
    ));
    assert!(err.is_definition_missing());
}

// trace:BUG-1693 | ai:codex
#[test]
fn a_second_hold_drops_the_stale_clearance_so_manual_label_removal_cannot_release() {
    use crate::forge::ForgeKind;
    use std::collections::BTreeSet;

    fn apply_label_args(labels: &mut BTreeSet<String>, argv: &[String]) {
        let mut index = 0;
        while index + 1 < argv.len() {
            let add = match argv[index].as_str() {
                "--add-label" | "--label" => true,
                "--remove-label" | "--unlabel" => false,
                _ => {
                    index += 1;
                    continue;
                }
            };
            for label in argv[index + 1].split(',') {
                if add {
                    labels.insert(label.to_string());
                } else {
                    labels.remove(label);
                }
            }
            index += 2;
        }
    }

    fn run_sequence(kind: ForgeKind) {
        let repo_origin = match kind {
            ForgeKind::GitHub => "git@github.com:o/r.git",
            ForgeKind::GitLab => "https://gitlab.com/g/sub/p.git",
            ForgeKind::None => unreachable!(),
        };
        let repo = pin(kind, repo_origin);
        let mut labels = BTreeSet::new();
        for held in [true, false, true] {
            let cleared = if held {
                ClearedLabel::Remove
            } else if labels.contains(HOLD_RECORDED_LABEL) {
                ClearedLabel::Add
            } else {
                ClearedLabel::Remove
            };
            let (_, args) = sync_label_command(&repo, 42, held, cleared).unwrap();
            apply_label_args(&mut labels, &args);
        }
        labels.remove(HOLD_LABEL);

        // tests/test_merge_hold_gate.sh case
        // 'aida:merge-hold-recorded|1|dropping the active label by hand leaves history without clearance'
        // proves the shipped gate fails on this set.
        assert_eq!(labels, BTreeSet::from([HOLD_RECORDED_LABEL.to_string()]));
    }

    run_sequence(ForgeKind::GitHub);
    run_sequence(ForgeKind::GitLab);
}

// BUG-1469 verification shape: N labeled PRs (some also marker-backed)
// plus M marker-only holds list as N+M rows — the label-only ones are
// exactly the labeled PRs with no marker.
// trace:BUG-1469 | ai:claude
#[test]
fn label_only_holds_are_the_labeled_prs_without_a_marker() {
    let labeled = [1972, 1974, 1978, 1979, 2014];
    let markers = [1972, 1979, 2050];
    let label_only = label_only_holds(&labeled, &markers);
    assert_eq!(label_only, vec![1974, 1978, 2014]);
    let rows = markers.len() + label_only.len();
    let marker_only = markers.iter().filter(|m| !labeled.contains(m)).count();
    assert_eq!(
        rows,
        labeled.len() + marker_only,
        "N labeled + M marker-only"
    );
    assert_eq!(HoldSource::LabelOnly.as_str(), "label");
}

// BUG-1499: a label-only hold is cleared by the same human floor and the
// clearance is RECORDED like a marker's, naming the missing half. The
// handler reads the forge label before recording (never guesses).
// trace:BUG-1499 | ai:claude
#[test]
fn label_only_clear_is_recorded_as_the_human_principal() {
    let dir = tempfile::tempdir().unwrap();
    let record = typed_hold(2019, HoldReasonKind::Unknown, LABEL_ONLY_REASON, None);
    let actor = human_clear_actor(&record, "joe", None).unwrap();
    record_clearance(dir.path(), &record, &actor).unwrap();
    let clearance: HoldClearance =
        serde_json::from_str(&std::fs::read_to_string(clearance_path(dir.path(), 2019)).unwrap())
            .unwrap();
    assert_eq!(clearance.cleared_by, "human:joe");
    assert_eq!(clearance.detail, LABEL_ONLY_REASON);

    let lib_source = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("../../lib.rs"),
        include_str!("../../lib_part1.rs"),
        include_str!("../../lib_part2.rs"),
        include_str!("../../lib_part3.rs"),
        include_str!("../../lib_part4.rs"),
        include_str!("../../lib_part5.rs"),
        include_str!("../../lib_part6.rs"),
        include_str!("../../merge_hold_cmd.rs")
    );
    let clear = lib_source
        .split("crate::cli::MergeHoldAction::Clear { pr, stale } =>")
        .nth(1)
        .and_then(|body| body.split("(None, true) =>").next())
        .expect("clear handler must remain inspectable");
    let floor = clear.find("has_integrity_floor_authority()").unwrap();
    let read = clear.find("merge_hold::fetch_pinned_change(").unwrap();
    let recorded = clear.rfind("merge_hold_core::record_clearance(").unwrap();
    assert!(
        floor < read && read < recorded,
        "floor, then forge read, then record"
    );
    assert!(clear.contains("merge_hold_core::LABEL_ONLY_REASON"));
    // BUG-1693: a marker deleted by hand leaves AIDA fail-closed on that
    // PR forever unless `merge-hold clear` can RECORD the release. Pin the
    // door itself, not merely a mention of it: whitespace-normalised so
    // `cargo fmt` cannot break the assertion, and ordered before the
    // "nothing to clear" early return a missing marker would otherwise
    // take. trace:BUG-1693 | ai:claude
    let squashed = clear.split_whitespace().collect::<Vec<_>>().join(" ");
    let door = squashed
        .find(
            "if let Some((record, actor)) = &tampering_record { \
             crate::gate::merge_hold_core::record_clearance( &root, record, actor, )?; }",
        )
        .expect("clear must RECORD a clearance for a hold whose marker went missing");
    // TASK-2152: the no-hold return now lives in `finish_clear`, which
    // runs after every clearance is recorded. trace:TASK-1951 | ai:claude
    let no_hold = squashed
        .find("for line in finish_clear(")
        .expect("the no-hold early return must remain inspectable");
    assert!(
        door < no_hold,
        "the tampering clearance must be recorded before the no-hold early return"
    );
}

// BUG-1693 AC1: `merge-hold list` must SURFACE a tampered PR, and the
// "nothing to see here" early return must not swallow it. The render is a
// thin loop over `unrecorded_marker_removals` (tested above), so the one
// place a regression could still mislead an operator is the empty-state
// conjunct: drop it and the command prints TAMPERING and then claims "No
// active merge-holds." in the same breath. Pin both, whitespace-normalised
// so `cargo fmt` cannot break the assertion.
// trace:BUG-1693 | ai:claude
#[test]
fn list_surfaces_a_tampered_pr_and_its_empty_state_cannot_swallow_it() {
    let lib_source = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("../../lib.rs"),
        include_str!("../../lib_part1.rs"),
        include_str!("../../lib_part2.rs"),
        include_str!("../../lib_part3.rs"),
        include_str!("../../lib_part4.rs"),
        include_str!("../../lib_part5.rs"),
        include_str!("../../lib_part6.rs"),
        include_str!("../../merge_hold_cmd.rs")
    );
    let list = lib_source
        .split("crate::cli::MergeHoldAction::List { json, fix } =>")
        .nth(1)
        .and_then(|body| body.split("crate::cli::MergeHoldAction::Add {").next())
        .expect("list handler must remain inspectable");
    let squashed = list.split_whitespace().collect::<Vec<_>>().join(" ");

    let render = squashed
        .find("for pr in &unrecorded_removals {")
        .expect("list must RENDER the unrecorded marker removals it computed");
    assert!(
        squashed[render..].contains("TAMPERING"),
        "the tampering render must name the condition in the operator's words"
    );

    let empty = squashed
        .find(
            "if live.is_empty() && stale.is_empty() && label_only.is_empty() \
               && unrecorded_removals.is_empty() { println!(\"No active merge-holds.\");",
        )
        .expect(
            "the empty-state early return must count unrecorded marker removals, \
             or list prints TAMPERING and then claims there are no holds",
        );
    assert!(
        render < empty,
        "the tampering line must be rendered before the empty-state early return"
    );
}

// BUG-1532 criteria 4 + 3 + 11, red-first against REAL verdict files: a
// refusal hold is released only by an APPROVED verdict recorded at the
// current head. A verdict at an older head, a refusal at the head, a
// newer refusal, a verdict with no sha, or an unknown head all keep it —
// and the held message names the verdict + sha (or the missing sha).
// trace:BUG-1532 | ai:claude
#[test]
fn refusal_hold_releases_only_on_a_fresh_verdict_at_the_current_head() {
    use crate::gate::review_verdict::{
        read_recorded_verdict, read_verdict_for_sha, record_verdict,
    };
    const OLD: &str = "3acf3671fd7a0000000000000000000000000000";
    const HEAD: &str = "cd21a1dc0a9e1111111111111111111111111111";
    const OTHER: &str = "af49b12b83fc2222222222222222222222222222";
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut hold = typed_hold(
        2049,
        HoldReasonKind::Rework,
        "CHANGES REQUESTED for STORY-1422 at 3acf3671fd7a",
        Some(OLD.into()),
    );
    hold.verdict_ref = Some(VerdictRef::new(
        "story-1422",
        Some(2049),
        Some(OLD.into()),
        Some("claude-reviewer-1".into()),
    ));
    write_typed_hold(root, &hold).unwrap();
    let before = std::fs::read(hold_path(root, 2049)).unwrap();
    let release = |head: Option<&str>| {
        refusal_release(
            &hold,
            head,
            |k| read_recorded_verdict(root, k),
            |k, sha| read_verdict_for_sha(root, k, sha),
        )
    };

    // No verdict at all → held, naming where it looked.
    match release(Some(HEAD)) {
        RefusalRelease::Held(why) => {
            assert!(why.contains("STORY-1422 or PR-2049"), "{why}")
        }
        other => panic!("no verdict must hold: {other:?}"),
    }
    // The refusal at the OLD head → held, naming the verdict and its sha.
    record_verdict(
        root,
        "STORY-1422",
        Some("request-changes"),
        Some(OLD),
        None,
        Some("tests assert source order"),
        &[],
        "claude-reviewer-1",
    )
    .unwrap();
    match release(Some(HEAD)) {
        RefusalRelease::Held(why) => {
            assert!(
                why.contains("CHANGES REQUESTED for STORY-1422 at 3acf3671fd7a"),
                "{why}"
            );
            assert!(why.contains("claude-reviewer-1"), "{why}");
            assert!(
                why.contains(".aida/review-verdicts/STORY-1422.json"),
                "{why}"
            );
            assert!(why.contains("cd21a1dc0a9e"), "{why}");
        }
        other => panic!("a stale refusal must hold: {other:?}"),
    }
    // Unknown head → held (fail closed).
    assert!(matches!(release(None), RefusalRelease::Held(_)));
    // An approval at a DIFFERENT sha does not cover the head.
    record_verdict(
        root,
        "STORY-1422",
        Some("approved"),
        Some(OTHER),
        None,
        None,
        &[],
        "claude-reviewer-1",
    )
    .unwrap();
    assert!(matches!(release(Some(HEAD)), RefusalRelease::Held(_)));
    // A fresh APPROVED verdict at exactly the current head releases it,
    // referencing the verdict record it rests on.
    record_verdict(
        root,
        "STORY-1422",
        Some("approved"),
        Some(HEAD),
        None,
        None,
        &[],
        "claude-reviewer-1",
    )
    .unwrap();
    match release(Some(HEAD)) {
        RefusalRelease::Released(r) => {
            assert_eq!(r.key, "STORY-1422");
            assert_eq!(r.reviewed_sha.as_deref(), Some(HEAD));
            assert_eq!(r.recorded_by.as_deref(), Some("claude-reviewer-1"));
        }
        other => panic!("a fresh approval at head must release: {other:?}"),
    }
    // A refusal re-recorded AT the head after that approval → held again.
    record_verdict(
        root,
        "STORY-1422",
        Some("request-changes"),
        Some(HEAD),
        None,
        None,
        &[],
        "claude-reviewer-1",
    )
    .unwrap();
    assert!(matches!(release(Some(HEAD)), RefusalRelease::Held(_)));
    // Deciding the condition never touches the marker.
    assert_eq!(std::fs::read(hold_path(root, 2049)).unwrap(), before);
}

// TASK-1801: a current verdict file the anchored reader refuses (here an
// extra hard link) is not evidence, and the per-sha archive must not
// answer in its place, neither in `read_verdict_for_sha` nor in the
// refusal release that `aida pr ship` and `merge-hold list` consult.
// trace:TASK-1801 | ai:claude
#[test]
fn a_refused_current_verdict_does_not_fall_back_to_its_archive() {
    use crate::gate::review_verdict::{
        archived_verdict_file_for_sha, read_recorded_verdict, read_verdict_for_sha, record_verdict,
        verdict_file_for_sha, verdict_path,
    };
    const HEAD: &str = "cd21a1dc0a9e1111111111111111111111111111";
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut hold = typed_hold(
        2712,
        HoldReasonKind::Rework,
        "CHANGES REQUESTED for TASK-1801",
        None,
    );
    hold.verdict_ref = Some(VerdictRef::new(
        "task-1801",
        Some(2712),
        None,
        Some("claude-reviewer-1".into()),
    ));
    record_verdict(
        root,
        "TASK-1801",
        Some("approved"),
        Some(HEAD),
        None,
        None,
        &[],
        "claude-reviewer-1",
    )
    .unwrap();
    let current = verdict_path(root, "TASK-1801");
    assert!(
        archived_verdict_file_for_sha(&current, HEAD).is_some(),
        "the fixture needs an archive that could answer for the head"
    );
    let release = || {
        refusal_release(
            &hold,
            Some(HEAD),
            |k| read_recorded_verdict(root, k),
            |k, sha| read_verdict_for_sha(root, k, sha),
        )
    };
    assert!(matches!(release(), RefusalRelease::Released(_)));
    assert!(matches!(
        crate::pr_cmd::ship_refusal_release(&[root], &hold, Some(HEAD)),
        RefusalRelease::Released(_)
    ));

    // Link only the current file: the archive stays singly linked.
    std::fs::hard_link(&current, root.join("current-link")).unwrap();
    assert!(verdict_file_for_sha(root, "TASK-1801", HEAD).is_none());
    assert!(read_verdict_for_sha(root, "TASK-1801", HEAD).is_none());
    assert!(matches!(release(), RefusalRelease::Held(_)));
    assert!(matches!(
        crate::pr_cmd::ship_refusal_release(&[root], &hold, Some(HEAD)),
        RefusalRelease::Held(_)
    ));
}

// BUG-1532 criterion 11: a refusal whose verdict has NO sha is released
// only by a fresh verdict that records one; the message names the missing
// provenance. Untyped legacy markers are refusals (criterion 5) and are
// answered under the PR-keyed record — never a key parsed from prose.
// trace:BUG-1532 | ai:claude
#[test]
fn refusal_with_an_unstamped_verdict_names_the_missing_provenance() {
    use crate::gate::review_verdict::{read_recorded_verdict, read_verdict_for_sha, verdict_path};
    const HEAD: &str = "35de6d3abb00000000000000000000000000000a";
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_hold(root, 2040, "REVIEWER HOLD: BUG-1 needs work").unwrap();
    let legacy = read_hold_record(root, 2040).unwrap();
    assert!(legacy.is_refusal());
    assert_eq!(refusal_verdict_keys(&legacy), vec!["PR-2040".to_string()]);
    let path = verdict_path(root, "PR-2040");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"verdict":"request-changes"}"#).unwrap();
    // Even an unstamped approval is not a verdict AT the head.
    let bug1 = verdict_path(root, "BUG-1");
    std::fs::write(
        &bug1,
        format!(r#"{{"verdict":"approved","reviewed_sha":"{HEAD}"}}"#),
    )
    .unwrap();
    match refusal_release(
        &legacy,
        Some(HEAD),
        |k| read_recorded_verdict(root, k),
        |k, sha| read_verdict_for_sha(root, k, sha),
    ) {
        RefusalRelease::Held(why) => {
            assert!(why.contains("NO reviewed sha"), "{why}");
            assert!(why.contains("missing"), "{why}");
        }
        other => panic!("unstamped refusal must hold: {other:?}"),
    }
    // Supervision is not a refusal; recusal and malformed are not either
    // (they have their own release paths).
    assert!(!typed_hold(1, HoldReasonKind::Supervision, "x", None).is_refusal());
    assert!(typed_hold(1, HoldReasonKind::Rework, "x", None).is_refusal());
}

// BUG-1532 criterion 10 + BUG-1562: a typed marker round-trips its
// verdict reference, release condition and spec; an older marker without
// them still reads (every new field is optional).
// trace:BUG-1532 trace:BUG-1562 | ai:claude
#[test]
fn marker_references_its_verdict_and_carries_a_release_condition() {
    let dir = tempfile::tempdir().unwrap();
    let mut record = typed_hold(7, HoldReasonKind::Rework, "summary", Some("abc1234".into()));
    record.verdict_ref = Some(VerdictRef::new(
        "bug-1460",
        Some(7),
        Some("abc1234".into()),
        Some("claude-reviewer-1".into()),
    ));
    record.release_condition = Some("1. approved at head 2. squash names BUG-1460".into());
    record.spec = Some("BUG-1460".into());
    write_typed_hold(dir.path(), &record).unwrap();
    let body = std::fs::read_to_string(hold_path(dir.path(), 7)).unwrap();
    assert!(body.contains("\"verdict_ref\""), "{body}");
    let back = read_hold_record(dir.path(), 7).unwrap();
    assert_eq!(back.verdict_ref.as_ref().unwrap().key, "BUG-1460");
    assert_eq!(
        back.verdict_ref.as_ref().unwrap().path(),
        ".aida/review-verdicts/BUG-1460.json"
    );
    assert_eq!(back.spec.as_deref(), Some("BUG-1460"));
    assert_eq!(
        back.release_condition_or_default(),
        (
            "1. approved at head 2. squash names BUG-1460".to_string(),
            false
        )
    );
    // A pre-BUG-1532 typed marker (no new fields) still parses.
    let old = r#"{"schema_version":2,"pr":8,"reason_kind":"supervision","detail":"d","routing_state":"pending"}"#;
    std::fs::write(hold_path(dir.path(), 8), old).unwrap();
    let back = read_hold_record(dir.path(), 8).unwrap();
    assert_eq!(back.reason_kind, HoldReasonKind::Supervision);
    assert!(back.verdict_ref.is_none() && back.release_condition.is_none());
    let (cond, is_default) = back.release_condition_or_default();
    assert!(is_default && cond.contains("aida pr ship 8"), "{cond}");
}

// BUG-1562, red-first: a marker carrying a distinctive release condition
// SURVIVES `merge-hold add` on the same PR; only `--replace` replaces it.
// trace:BUG-1562 | ai:claude
#[test]
fn hand_add_never_silently_overwrites_an_existing_marker() {
    let dir = tempfile::tempdir().unwrap();
    let line = "RELEASE: squash subject must not name BUG-1288";
    write_hold(dir.path(), 1999, line).unwrap();
    let fresh = typed_hold(1999, HoldReasonKind::Supervision, "one line", None);
    let err = place_hand_hold(dir.path(), &fresh, false).unwrap_err();
    assert!(err.contains(line) && err.contains("--replace"), "{err}");
    let body = std::fs::read_to_string(hold_path(dir.path(), 1999)).unwrap();
    assert!(
        body.contains(line),
        "the existing body must survive: {body}"
    );
    place_hand_hold(dir.path(), &fresh, true).unwrap();
    assert_eq!(
        read_hold_record(dir.path(), 1999).unwrap().detail,
        "one line"
    );
    // No marker → placed.
    place_hand_hold(
        dir.path(),
        &typed_hold(2000, HoldReasonKind::Decision, "d", None),
        false,
    )
    .unwrap();
    assert!(read_hold_record(dir.path(), 2000).is_some());
}

#[test]
fn write_read_clear_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // No hold initially.
    assert!(read_hold(root, 42).is_none());
    // Write a hold → read returns the reason.
    write_hold(
        root,
        42,
        "STORY-1155 is marked drive — merge requires review",
    )
    .unwrap();
    let reason = read_hold(root, 42).expect("hold must be present");
    assert!(reason.contains("drive"), "{reason}");
    // Clear → gone.
    clear_hold(root, 42).unwrap();
    assert!(read_hold(root, 42).is_none());
    // Clearing an absent hold is a no-op success.
    clear_hold(root, 42).unwrap();
}

#[test]
fn empty_reason_still_holds_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_hold(root, 7, "   ").unwrap();
    // An empty/whitespace reason must STILL register as a hold — the marker's
    // presence is the signal, never its content. A blank marker that merged
    // would be the fail-open bug this fix exists to prevent.
    assert!(
        read_hold(root, 7).is_some(),
        "a marker with a blank reason must still hold"
    );
}

#[test]
fn present_but_unreadable_marker_holds_fail_closed() {
    // TASK-1238: a marker that EXISTS but can't be read as a file must HOLD,
    // not merge. Simulate it by putting a directory where the marker file
    // would be — `read_to_string` then errors with something other than
    // NotFound. The old `.ok()?` returned None here (fail-open → merge); the
    // fix must return Some.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(hold_path(root, 99)).unwrap();
    assert!(
        read_hold(root, 99).is_some(),
        "a present-but-unreadable marker must HOLD (fail closed), never merge"
    );
}

#[test]
fn list_holds_enumerates_ascending() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_hold(root, 30, "b").unwrap();
    write_hold(root, 5, "a").unwrap();
    let holds = list_holds(root);
    assert_eq!(
        holds.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        vec![5, 30]
    );
}

// trace:BUG-1236 | ai:claude
#[test]
fn sync_failure_is_returned_and_recorded_on_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // A git repo with a GitHub origin so the forge resolves to GitHub.
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
    write_hold(root, 7, "drive").unwrap();
    let calls = std::cell::Cell::new(0);
    let err = sync_label_with(root, 7, true, |_, _, _| {
        calls.set(calls.get() + 1);
        Ok((false, "gh: HTTP 502 bad gateway\n".to_string()))
    })
    .unwrap_err();
    assert_eq!(calls.get(), 2, "one retry");
    assert!(format!("{err}").contains("502"), "{err}");
    assert_eq!(
        read_label_state(root, 7),
        LabelState::Unsynced("gh: HTTP 502 bad gateway".to_string())
    );
    assert_eq!(
        read_hold(root, 7).as_deref(),
        Some("drive"),
        "reason line untouched"
    );
    // A later successful sync flips the state.
    sync_label_with(root, 7, true, |_, _, _| Ok((true, String::new()))).unwrap();
    assert_eq!(read_label_state(root, 7), LabelState::Synced);
}

// trace:BUG-1236 | ai:claude
#[test]
fn markers_without_a_state_line_read_as_unknown() {
    let dir = tempfile::tempdir().unwrap();
    write_hold(dir.path(), 9, "guided").unwrap();
    assert_eq!(read_label_state(dir.path(), 9), LabelState::Unknown);
    assert_eq!(read_label_state(dir.path(), 10), LabelState::Unknown);
}

// trace:STORY-1397 | ai:codex
#[test]
fn typed_recusal_round_trip_and_label_update_preserve_routing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida/agents")).unwrap();
    std::fs::write(dir.path().join(".aida/agents/reader.toml"), format!("id = \"cold-reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
    let record = MergeHoldRecord {
        schema_version: 2,
        pr: 2023,
        reason_kind: HoldReasonKind::Recusal,
        detail: "author cannot merge".into(),
        recused_principals: vec!["agent:author".into()],
        routed_to: vec!["agent:cold-reader".into()],
        routing_state: HoldRoutingState::Routed,
        target_head_sha: Some("abcdef0123456789".into()),
        label_state: None,
        legacy: false,
        verdict_ref: None,
        release_condition: None,
        spec: None,
        placed_by: None,
        absorbed: Vec::new(),
    };
    write_typed_hold(dir.path(), &record).unwrap();
    record_label_state(dir.path(), 2023, &LabelState::Synced).unwrap();
    let reread = read_hold_record(dir.path(), 2023).unwrap();
    assert_eq!(reread.reason_kind, HoldReasonKind::Recusal);
    assert_eq!(reread.recused_principals, vec!["agent:author"]);
    assert_eq!(reread.routed_to, vec!["agent:cold-reader"]);
    assert_eq!(reread.target_head_sha.as_deref(), Some("abcdef0123456789"));
    assert_eq!(read_label_state(dir.path(), 2023), LabelState::Synced);
    assert!(principal_is_recused(
        &reread,
        &PrincipalIdentity::parse("agent:author")
    ));
    assert!(!principal_is_recused(
        &reread,
        &PrincipalIdentity::parse("agent:cold-reader")
    ));
    assert!(dir
        .path()
        .join(".aida/agent-briefs/codex/PR-2023-abcdef0123456789.md")
        .exists());
}

#[test]
fn recusal_requires_named_principal_and_malformed_json_stays_held() {
    let dir = tempfile::tempdir().unwrap();
    let record = MergeHoldRecord {
        schema_version: 2,
        pr: 7,
        reason_kind: HoldReasonKind::Recusal,
        detail: "recused".into(),
        recused_principals: Vec::new(),
        routed_to: Vec::new(),
        routing_state: HoldRoutingState::NoIndependentReader,
        target_head_sha: None,
        label_state: None,
        legacy: false,
        verdict_ref: None,
        release_condition: None,
        spec: None,
        placed_by: None,
        absorbed: Vec::new(),
    };
    assert!(write_typed_hold(dir.path(), &record).is_err());
    std::fs::create_dir_all(holds_dir(dir.path())).unwrap();
    std::fs::write(hold_path(dir.path(), 7), "{not-json").unwrap();
    let held = read_hold_record(dir.path(), 7).unwrap();
    assert_eq!(held.reason_kind, HoldReasonKind::Unknown);
    assert!(read_hold(dir.path(), 7).is_some());
}

#[test]
fn legacy_marker_is_supervision_without_guessing_recusal_from_prose() {
    let dir = tempfile::tempdir().unwrap();
    write_hold(dir.path(), 8, "I wrote this; another reader is needed").unwrap();
    let held = read_hold_record(dir.path(), 8).unwrap();
    assert!(held.legacy);
    assert_eq!(held.reason_kind, HoldReasonKind::Supervision);
    assert!(held.recused_principals.is_empty());
}

// trace:BUG-1691 | ai:codex
#[test]
fn legacy_detail_is_preserved_when_absorbed_by_rework() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_detail = "I wrote this; another reader is needed";
    write_hold(dir.path(), 8, legacy_detail).unwrap();
    let rework = typed_hold(8, HoldReasonKind::Rework, "changes requested", None);
    write_typed_hold(dir.path(), &rework).unwrap();
    let got = read_hold_record(dir.path(), 8).unwrap();
    assert_eq!(got.reason_kind, HoldReasonKind::Rework);
    assert_eq!(got.absorbed[0].reason_kind, HoldReasonKind::Supervision);
    assert_eq!(got.absorbed[0].detail, legacy_detail);
}

// trace:STORY-1397 | ai:claude
#[test]
fn human_at_terminal_clears_recusal_without_agent_id() {
    let record = recused_record();
    let actor = human_clear_actor(&record, "joe", None).expect("human clears");
    assert_eq!(actor.key(), "human:joe");
    // An unrelated declared agent id neither helps nor hinders the human.
    let other = PrincipalIdentity::parse("agent:someone-else");
    assert_eq!(
        human_clear_actor(&record, "joe", Some(&other))
            .unwrap()
            .key(),
        "human:joe"
    );
}

#[test]
fn human_clear_refuses_a_recused_declared_agent_and_a_recused_human() {
    let record = recused_record();
    let author = PrincipalIdentity::parse("agent:author");
    assert!(human_clear_actor(&record, "joe", Some(&author)).is_err());

    let mut human_recused = recused_record();
    human_recused
        .recused_principals
        .push(PrincipalIdentity::parse("human:joe"));
    assert!(human_clear_actor(&human_recused, "joe", None).is_err());
    assert!(human_clear_actor(&human_recused, "ann", None).is_ok());

    let mut operator_recused = recused_record();
    operator_recused
        .recused_principals
        .push(PrincipalIdentity::parse("operator:joe"));
    assert!(human_clear_actor(&operator_recused, "joe", None).is_err());

    assert!(human_clear_actor(&record, "  ", None).is_err());
}

#[test]
fn clearance_is_recorded_as_the_human_principal() {
    let dir = tempfile::tempdir().unwrap();
    let record = recused_record();
    let actor = human_clear_actor(&record, "joe", None).unwrap();
    record_clearance(dir.path(), &record, &actor).unwrap();
    let body = std::fs::read_to_string(clearance_path(dir.path(), 42)).unwrap();
    let clearance: HoldClearance = serde_json::from_str(&body).unwrap();
    assert_eq!(clearance.cleared_by, "human:joe");
    assert_eq!(clearance.reason_kind, HoldReasonKind::Recusal);
    assert_eq!(clearance.target_head_sha.as_deref(), Some("head-a"));
}

#[test]
fn human_principal_round_trips() {
    let human = PrincipalIdentity::parse("human:joe");
    assert_eq!(human.principal_kind, PrincipalKind::Human);
    assert_eq!(human.key(), "human:joe");
    let json = serde_json::to_string(&human).unwrap();
    assert_eq!(
        serde_json::from_str::<PrincipalIdentity>(&json).unwrap(),
        human
    );
}

// trace:BUG-1691 | ai:codex
#[test]
fn a_rework_hold_never_displaces_a_recusal_hold() {
    let dir = tempfile::tempdir().unwrap();
    let recusal = recused_record();
    write_typed_hold(dir.path(), &recusal).unwrap();
    let mut rework = typed_hold(
        42,
        HoldReasonKind::Rework,
        "CHANGES REQUESTED for BUG-1 at abc1234",
        Some("abc1234-full".into()),
    );
    rework.spec = Some("BUG-1".into());
    write_typed_hold(dir.path(), &rework).unwrap();
    let got = read_hold_record(dir.path(), 42).unwrap();
    assert_eq!(got.reason_kind, HoldReasonKind::Recusal);
    assert_eq!(got.recused_principals, recusal.recused_principals);
    assert_eq!(got.target_head_sha.as_deref(), Some("head-a"));
    assert_eq!(got.absorbed.len(), 1);
    assert_eq!(got.absorbed[0].reason_kind, HoldReasonKind::Rework);
    assert_eq!(got.absorbed[0].detail, rework.detail);
}

// trace:BUG-1691 | ai:codex
#[test]
fn recusal_written_after_rework_is_primary_and_absorbs_rework() {
    let dir = tempfile::tempdir().unwrap();
    write_typed_hold(
        dir.path(),
        &typed_hold(
            42,
            HoldReasonKind::Rework,
            "changes",
            Some("new-head".into()),
        ),
    )
    .unwrap();
    let recusal = recused_record();
    write_typed_hold(dir.path(), &recusal).unwrap();
    let got = read_hold_record(dir.path(), 42).unwrap();
    assert_eq!(got.reason_kind, HoldReasonKind::Recusal);
    assert_eq!(got.recused_principals, recusal.recused_principals);
    assert_eq!(got.target_head_sha.as_deref(), Some("head-a"));
    assert_eq!(
        got.absorbed
            .iter()
            .map(|a| a.reason_kind)
            .collect::<Vec<_>>(),
        vec![HoldReasonKind::Rework]
    );
}

// trace:BUG-1691 | ai:codex
#[test]
fn same_kind_refresh_unions_existing_absorbed_holds() {
    let dir = tempfile::tempdir().unwrap();
    let recusal = recused_record();
    write_typed_hold(dir.path(), &recusal).unwrap();
    write_typed_hold(
        dir.path(),
        &typed_hold(42, HoldReasonKind::Rework, "changes", None),
    )
    .unwrap();
    let mut refreshed = recused_record();
    refreshed.label_state = Some("synced".into());
    write_typed_hold(dir.path(), &refreshed).unwrap();
    let got = read_hold_record(dir.path(), 42).unwrap();
    assert_eq!(got.reason_kind, HoldReasonKind::Recusal);
    assert_eq!(got.label_state.as_deref(), Some("synced"));
    assert_eq!(got.absorbed.len(), 1);
    assert_eq!(got.absorbed[0].reason_kind, HoldReasonKind::Rework);
}

// trace:BUG-1691 | ai:codex
#[test]
fn composition_is_flat_bounded_and_unique_by_kind() {
    let mut record = typed_hold(42, HoldReasonKind::Supervision, "supervision", None);
    for (kind, detail) in [
        (HoldReasonKind::Rework, "rework"),
        (HoldReasonKind::Decision, "decision"),
        (HoldReasonKind::Rework, "rework refreshed"),
    ] {
        record = match compose_holds(&record, &typed_hold(42, kind, detail, None)) {
            Composed::Placed(r) | Composed::Preserved(r) => r,
        };
    }
    assert_eq!(record.reason_kind, HoldReasonKind::Decision);
    assert!(record.absorbed.len() <= 3);
    let mut kinds = record
        .absorbed
        .iter()
        .map(|a| a.reason_kind)
        .collect::<Vec<_>>();
    kinds.sort_by_key(|k| k.strictness());
    kinds.dedup();
    assert_eq!(kinds.len(), record.absorbed.len());
}

// trace:BUG-1691 | ai:codex
#[test]
fn composition_rejects_invalid_recusal_and_accepts_valid_existing_recusal() {
    let dir = tempfile::tempdir().unwrap();
    let mut invalid = recused_record();
    invalid.recused_principals.clear();
    assert!(write_typed_hold(dir.path(), &invalid).is_err());
    write_typed_hold(dir.path(), &recused_record()).unwrap();
    assert!(write_typed_hold(
        dir.path(),
        &typed_hold(42, HoldReasonKind::Rework, "changes", None)
    )
    .is_ok());
}

// trace:BUG-1691 | ai:codex
#[test]
fn hand_replace_discards_recusal_but_default_hand_add_refuses() {
    let dir = tempfile::tempdir().unwrap();
    write_typed_hold(dir.path(), &recused_record()).unwrap();
    let rework = typed_hold(42, HoldReasonKind::Rework, "manual rework", None);
    assert!(place_hand_hold(dir.path(), &rework, false).is_err());
    place_hand_hold(dir.path(), &rework, true).unwrap();
    let got = read_hold_record(dir.path(), 42).unwrap();
    assert_eq!(got.reason_kind, HoldReasonKind::Rework);
    assert!(got.absorbed.is_empty());
}

// trace:BUG-1691 | ai:codex
#[test]
fn old_typed_markers_without_absorbed_still_parse() {
    let dir = tempfile::tempdir().unwrap();
    let record = typed_hold(42, HoldReasonKind::Supervision, "old marker", None);
    let mut json = serde_json::to_value(record).unwrap();
    json.as_object_mut().unwrap().remove("absorbed");
    std::fs::create_dir_all(holds_dir(dir.path())).unwrap();
    std::fs::write(
        hold_path(dir.path(), 42),
        serde_json::to_vec(&json).unwrap(),
    )
    .unwrap();
    let got = read_hold_record(dir.path(), 42).unwrap();
    assert!(got.absorbed.is_empty());
}

// trace:BUG-1691 | ai:codex
#[test]
fn preserving_rework_write_does_not_reconcile_existing_recusal_route() {
    let dir = tempfile::tempdir().unwrap();
    let mut recusal = recused_record();
    recusal.routed_to = vec![PrincipalIdentity::parse("agent:cold-reader")];
    recusal.routing_state = HoldRoutingState::Routed;
    write_marker(dir.path(), &recusal, false, false, None).unwrap();
    let expected_route = recusal.routed_to.clone();
    write_typed_hold(
        dir.path(),
        &typed_hold(42, HoldReasonKind::Rework, "changes requested", None),
    )
    .unwrap();
    let got = read_hold_record(dir.path(), 42).unwrap();
    assert_eq!(got.routed_to, expected_route);
    assert_eq!(got.routing_state, HoldRoutingState::Routed);
    assert_ne!(got.routing_state, HoldRoutingState::NoIndependentReader);
}

// trace:BUG-1691 | ai:codex
#[test]
fn preserving_path_still_rejects_invalid_recusal_refresh() {
    let dir = tempfile::tempdir().unwrap();
    write_marker(dir.path(), &recused_record(), false, false, None).unwrap();
    let mut invalid = recused_record();
    invalid.recused_principals.clear();
    assert!(write_typed_hold(dir.path(), &invalid).is_err());
}

#[test]
fn explicit_reconciliation_refreshes_routes_and_render_stays_read_only() {
    // Normalise CRLF (Windows autocrlf checkout) so the column-0
    // closing-brace split below still finds the body end.
    // trace:BUG-1556 | ai:claude
    let lib_source = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("../../lib.rs"),
        include_str!("../../lib_part1.rs"),
        include_str!("../../lib_part2.rs"),
        include_str!("../../lib_part3.rs"),
        include_str!("../../lib_part4.rs"),
        include_str!("../../lib_part5.rs"),
        include_str!("../../lib_part6.rs"),
        include_str!("../../merge_hold_cmd.rs"),
        include_str!("../../status_awaiting.rs")
    )
    .replace("\r\n", "\n");
    let awaiting = lib_source
        .split("fn collect_awaiting_report_inner(")
        .nth(1)
        // The collector body ends at its closing brace in column 0.
        .and_then(|body| body.split("\n}\n").next())
        .expect("awaiting/status collector must remain inspectable");
    // STORY-1397 review: the awaiting render (incl. the per-turn notice)
    // is read-only and cheap. Reconciliation writes markers and briefs and
    // probes liveness, so it lives on the explicit `merge-hold list --fix`.
    for forbidden in [
        "reconcile_recusal_hold",
        "write_typed_hold",
        "merge_hold::write_hold",
        "sysinfo::",
    ] {
        assert!(
            !awaiting.contains(forbidden),
            "awaiting render must not call `{forbidden}`"
        );
    }
    assert!(
        awaiting.contains("awaiting_you::project_held_pr"),
        "every non-recusal held PR must be projected, not dropped"
    );
    // BUG-1773: a refusal recorded outside `aida review record` arms no
    // marker, so the corpus arm is the only thing that surfaces it. It is
    // part of the same read-only contract as the forbidden list above —
    // it derives the hold and writes neither marker nor label.
    // trace:BUG-1773 | ai:claude
    assert!(
        awaiting.contains("awaiting_you::corpus_held_prs"),
        "a refusal with no hold marker must still reach `held_prs`"
    );
    let handler = lib_source
        .split("fn handle_merge_hold(")
        .nth(1)
        .and_then(|body| body.split("MergeHoldAction::Add").next())
        .expect("merge-hold list handler must remain inspectable");
    assert!(
        handler.contains("merge_hold_core::reconcile_recusal_hold"),
        "`aida merge-hold list --fix` must reconcile recusal routing"
    );
    // TASK-189: the label column is a live (pinned) forge read; the
    // marker's recorded claim is shown only as `label_recorded`.
    assert!(handler.contains("merge_hold::fetch_pinned_change"));
    assert!(handler.contains("\"label\": forge_label.as_str()"));
    assert!(handler.contains("\"label_recorded\": recorded_of(pr)"));
    assert!(handler.contains("\"label_diverged\""));
    let own_source = include_str!("../../merge_hold.rs");
    let route = own_source
        .split("fn reconcile_recusal_route(")
        .nth(1)
        .and_then(|body| body.split("fn retire_stale_route_briefs(").next())
        .unwrap();
    assert!(
        !route.contains(concat!("new", "_all")),
        "routing must probe one pid, never scan the whole process table"
    );

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida/agents")).unwrap();
    let mut record = typed_hold(44, HoldReasonKind::Recusal, "recused", Some("abc".into()));
    record.recused_principals = vec![PrincipalIdentity::parse("agent:author")];
    write_typed_hold(dir.path(), &record).unwrap();

    std::fs::write(dir.path().join(".aida/agents/reader.toml"), format!("id = \"reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
    let routed = reconcile_recusal_hold(dir.path(), 44, Some("abc")).unwrap();
    assert_eq!(routed.routing_state, HoldRoutingState::Routed);
    assert!(dir
        .path()
        .join(".aida/agent-briefs/codex/PR-44-abc.md")
        .exists());

    std::fs::remove_file(dir.path().join(".aida/agents/reader.toml")).unwrap();
    let retired = reconcile_recusal_hold(dir.path(), 44, Some("abc")).unwrap();
    assert_eq!(retired.routing_state, HoldRoutingState::NoIndependentReader);
    assert!(dir
        .path()
        .join(".aida/agent-briefs/codex/PR-44-abc.md.stale")
        .exists());

    std::fs::write(dir.path().join(".aida/agents/new-reader.toml"), format!("id = \"new-reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
    let moved = reconcile_recusal_hold(dir.path(), 44, Some("def")).unwrap();
    assert_eq!(moved.target_head_sha.as_deref(), Some("def"));
    assert_eq!(moved.routing_state, HoldRoutingState::Routed);
    assert!(dir
        .path()
        .join(".aida/agent-briefs/codex/PR-44-def.md")
        .exists());
}

// trace:STORY-1889 | ai:agy
#[test]
fn label_state_persistence_lives_in_protected_core_for_typed_and_legacy_markers() {
    let core_src = include_str!("../merge_hold_core.rs");
    let outer_src = include_str!("../../merge_hold.rs");
    assert!(
        core_src.contains("pub(crate) fn record_label_state(")
            && core_src.contains("pub(crate) fn read_label_state("),
        "record_label_state and read_label_state must live inside protected gate/merge_hold_core.rs"
    );
    assert!(
        !outer_src.contains("fn record_label_state(")
            && !outer_src.contains("fn read_label_state("),
        "unprotected merge_hold.rs must not define label-state marker writers"
    );

    let dir = tempfile::tempdir().unwrap();
    // Missing marker is a no-op.
    record_label_state(dir.path(), 100, &LabelState::Synced).unwrap();
    assert!(!hold_path(dir.path(), 100).exists());
    assert_eq!(read_label_state(dir.path(), 100), LabelState::Unknown);

    // Legacy plaintext marker round-trips Synced, Unsynced, and Unknown while preserving reason.
    write_hold(dir.path(), 101, "guided").unwrap();
    record_label_state(
        dir.path(),
        101,
        &LabelState::Unsynced("gh: 503\ndetail".into()),
    )
    .unwrap();
    assert_eq!(
        read_label_state(dir.path(), 101),
        LabelState::Unsynced("gh: 503".into())
    );
    assert_eq!(read_hold(dir.path(), 101).as_deref(), Some("guided"));
    record_label_state(dir.path(), 101, &LabelState::Synced).unwrap();
    assert_eq!(read_label_state(dir.path(), 101), LabelState::Synced);
    record_label_state(dir.path(), 101, &LabelState::Unknown).unwrap();
    assert_eq!(read_label_state(dir.path(), 101), LabelState::Unknown);
    assert_eq!(read_hold(dir.path(), 101).as_deref(), Some("guided"));

    // Typed JSON marker round-trips Synced, Unsynced, and Unknown while preserving record fields.
    let record = typed_hold(
        102,
        HoldReasonKind::Supervision,
        "manual check",
        Some("abc1234".into()),
    );
    write_typed_hold(dir.path(), &record).unwrap();
    record_label_state(
        dir.path(),
        102,
        &LabelState::Unsynced("glab: 502\nextra".into()),
    )
    .unwrap();
    assert_eq!(
        read_label_state(dir.path(), 102),
        LabelState::Unsynced("glab: 502".into())
    );
    record_label_state(dir.path(), 102, &LabelState::Synced).unwrap();
    assert_eq!(read_label_state(dir.path(), 102), LabelState::Synced);
    record_label_state(dir.path(), 102, &LabelState::Unknown).unwrap();
    assert_eq!(read_label_state(dir.path(), 102), LabelState::Unknown);
    let reread = read_hold_record(dir.path(), 102).unwrap();
    assert_eq!(reread.reason_kind, HoldReasonKind::Supervision);
    assert_eq!(reread.detail, "manual check");
    assert_eq!(reread.target_head_sha.as_deref(), Some("abc1234"));
}

// trace:STORY-1889 | ai:agy
#[test]
fn routing_proposal_boundary_cannot_mutate_non_routing_fields_at_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let vref = VerdictRef::new(
        "PR-2099",
        Some(2099),
        Some("abc1234567890".into()),
        Some("reviewer".into()),
    );
    let absorbed_rework = AbsorbedHold {
        reason_kind: HoldReasonKind::Rework,
        detail: "needs rework".into(),
        recused_principals: Vec::new(),
        target_head_sha: Some("abc1234567890".into()),
        verdict_ref: Some(vref.clone()),
        spec: Some("STORY-1889".into()),
        placed_by: Some("reviewer".into()),
    };
    let original = MergeHoldRecord {
        schema_version: 2,
        pr: 2099,
        reason_kind: HoldReasonKind::Recusal,
        detail: "author recused".into(),
        recused_principals: vec![PrincipalIdentity::parse("agent:author")],
        routed_to: Vec::new(),
        routing_state: HoldRoutingState::Pending,
        target_head_sha: Some("abc1234567890".into()),
        label_state: Some("synced".into()),
        legacy: false,
        verdict_ref: Some(vref),
        release_condition: Some("independent review".into()),
        spec: Some("STORY-1889".into()),
        placed_by: Some("advisor".into()),
        absorbed: vec![absorbed_rework.clone()],
    };

    // Adversarial route callback tries to tamper with every authority-bearing field.
    let tamper_route = |proposal: &mut MergeHoldRecord| -> std::io::Result<()> {
        proposal.schema_version = 99;
        proposal.pr = 9999;
        proposal.reason_kind = HoldReasonKind::Rework;
        proposal.detail = "tampered detail".into();
        proposal.recused_principals.clear();
        proposal.target_head_sha = Some("deadbeef".into());
        proposal.label_state = Some("unsynced: tampered".into());
        proposal.legacy = true;
        proposal.verdict_ref = None;
        proposal.release_condition = None;
        proposal.spec = Some("TAMPERED-1".into());
        proposal.placed_by = Some("attacker".into());
        proposal.absorbed.clear();
        proposal.routed_to = vec![PrincipalIdentity::parse("agent:reader-1")];
        proposal.routing_state = HoldRoutingState::Routed;
        Ok(())
    };

    // 1. write_marker_with_route persists ONLY routed_to and routing_state from the proposal.
    write_marker_with_route(dir.path(), &original, true, false, None, tamper_route).unwrap();
    let persisted = read_hold_record(dir.path(), 2099).unwrap();
    assert_eq!(
        persisted.routed_to,
        vec![PrincipalIdentity::parse("agent:reader-1")]
    );
    assert_eq!(persisted.routing_state, HoldRoutingState::Routed);
    let mut expected_after_write = original.clone();
    expected_after_write.routed_to = vec![PrincipalIdentity::parse("agent:reader-1")];
    expected_after_write.routing_state = HoldRoutingState::Routed;
    assert_eq!(persisted, expected_after_write);

    // 2. reconcile_recusal_hold_with persists ONLY routed_to and routing_state (plus live_head projection).
    let refreshed =
        reconcile_recusal_hold_with(dir.path(), 2099, Some("fedcba987654"), |proposal| {
            proposal.reason_kind = HoldReasonKind::Supervision;
            proposal.detail = "tampered on refresh".into();
            proposal.recused_principals = vec![PrincipalIdentity::parse("agent:someone-else")];
            proposal.target_head_sha = Some("000000000000".into());
            proposal.label_state = None;
            proposal.legacy = true;
            proposal.verdict_ref = None;
            proposal.release_condition = None;
            proposal.spec = None;
            proposal.placed_by = None;
            proposal.absorbed.clear();
            proposal.routed_to = vec![PrincipalIdentity::parse("agent:reader-2")];
            proposal.routing_state = HoldRoutingState::Routed;
            Ok(())
        })
        .unwrap();
    let reread = read_hold_record(dir.path(), 2099).unwrap();
    let mut expected_after_refresh = expected_after_write;
    expected_after_refresh.target_head_sha = Some("fedcba987654".into());
    expected_after_refresh.routed_to = vec![PrincipalIdentity::parse("agent:reader-2")];
    expected_after_refresh.routing_state = HoldRoutingState::Routed;
    assert_eq!(refreshed, expected_after_refresh);
    assert_eq!(reread, expected_after_refresh);
    assert_eq!(reread.absorbed, vec![absorbed_rework.clone()]);

    // 3. project_live_head lives in protected gate/merge_hold_core.rs (not unprotected merge_hold.rs)
    // and mutates only target_head_sha, routing_state, and routed_to when a recusal head moves.
    let core_src = include_str!("../merge_hold_core.rs");
    let wrapper_src = include_str!("../../merge_hold.rs");
    assert!(
        core_src.contains("pub(crate) fn project_live_head("),
        "project_live_head must live in protected gate/merge_hold_core.rs"
    );
    assert!(
        !wrapper_src.contains("fn project_live_head("),
        "unprotected merge_hold.rs must not define project_live_head"
    );
    let projected = project_live_head(&expected_after_refresh, Some("112233445566"));
    let mut expected_projected = expected_after_refresh;
    expected_projected.target_head_sha = Some("112233445566".into());
    expected_projected.routing_state = HoldRoutingState::StaleHead;
    expected_projected.routed_to.clear();
    assert_eq!(projected, expected_projected);
    assert_eq!(projected.absorbed, vec![absorbed_rework]);
}
