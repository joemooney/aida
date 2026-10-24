//! BUG-2214: a supervised PR whose merge-hold a human already cleared is
//! covered by a mandate only when three independently writable sources agree —
//! the local clearance record, a TTY-attested human release event, and the
//! forge's label state. Any single source alone (a forged clearance file, a
//! stripped label, a fabricated event) stays `HoldFacts::None`, which the
//! cover refuses as `NoHoldMarker`. Plus: the evidence fetch that brings a
//! forge-moved head into the local clone before the exclusion diff reads it.
// trace:BUG-2214 | ai:claude

use crate::mandate::HoldFacts;
use crate::mandate_ship::cleared_by_human_facts;
use crate::merge_hold::{self, ChangeState, HoldClearance, HoldReasonKind, PinnedChange};

const PR: u64 = 44;

fn write_clearance(root: &std::path::Path, pr: u64, cleared_by: &str, mandate: Option<&str>) {
    let clearance = HoldClearance {
        pr,
        reason_kind: HoldReasonKind::Supervision,
        detail: "supervised spec".into(),
        cleared_by: cleared_by.into(),
        target_head_sha: None,
        cleared_at: chrono::Utc::now().to_rfc3339(),
        released_by_verdict: None,
        released_under_mandate: mandate.map(String::from),
    };
    let path = merge_hold::clearance_path(root, pr);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&clearance).unwrap()).unwrap();
}

fn emit_hold_event(
    root: &std::path::Path,
    pr: u64,
    placed: bool,
    principal: Option<&str>,
    human_attested: bool,
) {
    let ev = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MergeHoldChanged {
            pr: pr as u32,
            placed,
            reason: Some("fixture".into()),
            principal: principal.map(String::from),
            human_attested,
        },
    );
    crate::events::emit(root, &ev);
}

fn labels(hold: bool, recorded: bool, cleared: bool) -> PinnedChange {
    PinnedChange {
        merged: false,
        state: ChangeState::Open,
        head_sha: None,
        hold_label: hold,
        recorded_label: recorded,
        cleared_label: cleared,
    }
}

/// A clearance, an attested human release event, and agreeing labels.
fn qualified_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_clearance(tmp.path(), PR, "human:joe", None);
    emit_hold_event(tmp.path(), PR, true, None, false);
    emit_hold_event(tmp.path(), PR, false, Some("human:joe"), true);
    tmp
}

#[test]
fn all_three_sources_agree_and_the_hold_reads_as_cleared_by_human() {
    // trace:BUG-2214.ace5f93a | ai:claude
    let tmp = qualified_root();
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, true)))),
        HoldFacts::ClearedByHuman
    );
    // The BUG-1693 pair rule also passes a PR the forge never saw held.
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, false, false)))),
        HoldFacts::ClearedByHuman
    );
}

/// The advisor-required test: a forged clearance file plus stripped labels
/// with no attested event is refused — before the forge is even asked.
#[test]
fn a_forged_clearance_without_an_attested_event_is_refused() {
    // trace:BUG-2214.acd6c45c | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    write_clearance(tmp.path(), PR, "human:joe", None);
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || unreachable!(
            "no event: the forge must not be read"
        )),
        HoldFacts::None
    );
}

#[test]
fn no_clearance_record_never_reads_the_forge() {
    // trace:BUG-2214.acd6c45c | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    emit_hold_event(tmp.path(), PR, false, Some("human:joe"), true);
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || unreachable!(
            "no clearance: the forge must not be read"
        )),
        HoldFacts::None
    );
}

#[test]
fn a_mandate_clearance_is_not_a_human_clearance() {
    // trace:BUG-2214.ace5f93a | ai:claude
    let tmp = qualified_root();
    write_clearance(tmp.path(), PR, "advisor:joe", Some("mandate-1"));
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, true)))),
        HoldFacts::None
    );
}

#[test]
fn an_agent_clearance_is_not_a_human_clearance() {
    // trace:BUG-2214.ace5f93a | ai:claude
    let tmp = qualified_root();
    write_clearance(tmp.path(), PR, "agent:drain-worker", None);
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, true)))),
        HoldFacts::None
    );
}

#[test]
fn a_hold_placed_after_the_release_cancels_it() {
    // trace:BUG-2214.acd6c45c | ai:claude
    let tmp = qualified_root();
    // A re-arm after a mandate ship failure is a placement like any other.
    emit_hold_event(tmp.path(), PR, true, None, false);
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, true)))),
        HoldFacts::None
    );
}

#[test]
fn an_unattested_or_non_human_release_event_is_refused() {
    // trace:BUG-2214.acd6c45c | ai:claude
    for (principal, attested) in [
        (Some("human:joe"), false),   // no integrity-floor attestation
        (Some("advisor:joe"), true),  // an advisor seat is not a human
        (Some("agent:worker"), true), // an agent is not a human
        (None, true),                 // no principal recorded
    ] {
        let tmp = tempfile::tempdir().unwrap();
        write_clearance(tmp.path(), PR, "human:joe", None);
        emit_hold_event(tmp.path(), PR, true, None, false);
        emit_hold_event(tmp.path(), PR, false, principal, attested);
        assert_eq!(
            cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, true)))),
            HoldFacts::None,
            "principal {principal:?}, attested {attested}"
        );
    }
}

#[test]
fn another_prs_release_does_not_qualify() {
    // trace:BUG-2214.acd6c45c | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    write_clearance(tmp.path(), PR, "human:joe", None);
    emit_hold_event(tmp.path(), PR + 1, false, Some("human:joe"), true);
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, true)))),
        HoldFacts::None
    );
}

/// A clearance file copied from another PR's record does not qualify.
#[test]
fn a_copied_clearance_record_does_not_qualify() {
    // trace:BUG-2214.acd6c45c | ai:claude
    let tmp = qualified_root();
    write_clearance(tmp.path(), PR + 1, "human:joe", None);
    let copied = merge_hold::clearance_path(tmp.path(), PR + 1);
    std::fs::copy(&copied, merge_hold::clearance_path(tmp.path(), PR)).unwrap();
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, true)))),
        HoldFacts::None
    );
}

#[test]
fn forge_label_state_must_agree() {
    // trace:BUG-2214.acd6c45c | ai:claude
    let tmp = qualified_root();
    // A live hold label: the forge still holds the PR.
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(true, true, true)))),
        HoldFacts::None
    );
    // BUG-1693 pair rule: recorded without cleared (a label stripped by hand),
    // and cleared without recorded (a fabricated clearance label).
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, true, false)))),
        HoldFacts::None
    );
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(Some(labels(false, false, true)))),
        HoldFacts::None
    );
    // Unreadable labels and a change the forge cannot find fail closed.
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Err("api down".into())),
        HoldFacts::None
    );
    assert_eq!(
        cleared_by_human_facts(tmp.path(), PR, || Ok(None)),
        HoldFacts::None
    );
}

/// Events written before BUG-2214 carry neither field and must parse (as
/// unattested), and an event with the defaults serializes in the old shape.
#[test]
fn hold_event_serde_is_backward_compatible() {
    // trace:BUG-2214.ace5f93a | ai:claude
    let ev = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MergeHoldChanged {
            pr: 5,
            placed: false,
            reason: None,
            principal: None,
            human_attested: false,
        },
    );
    let json = serde_json::to_string(&ev).unwrap();
    assert!(!json.contains("principal"), "{json}");
    assert!(!json.contains("human_attested"), "{json}");
    let old: crate::events::Event = serde_json::from_str(&json).unwrap();
    assert_eq!(old.kind, ev.kind);
}

// ── The evidence fetch (acceptance: ship/train fetch the PR head before
//    reading mandate evidence) ───────────────────────────────────────────────

fn git(repo: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=AIDA Test",
            "-c",
            "user.email=aida@example.test",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
        ])
        .args(args)
        .status();
    assert!(ok.map(|s| s.success()).unwrap_or(false), "git {args:?}");
}

fn commit(repo: &std::path::Path, file: &str, message: &str) -> String {
    std::fs::write(repo.join(file), message).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-qm", message]);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn the_evidence_fetch_brings_a_forge_moved_head() {
    // trace:BUG-2214.ac2210da | ai:claude
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    commit(&origin, "seed.txt", "seed");
    let clone = dir.path().join("clone");
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            &origin.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    );
    // The forge moves the head after the clone — `gh pr update-branch`.
    git(&origin, &["checkout", "-qb", "pr"]);
    let moved = commit(&origin, "pr.txt", "moved head");
    git(&origin, &["checkout", "-q", "main"]);
    let have = |sha: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["cat-file", "-e", sha])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    assert!(!have(&moved), "the clone must start without the moved head");
    crate::pr_cmd::fetch_ship_refs(&clone, "main", "pr").expect("fetch");
    assert!(have(&moved), "the fetch must bring the moved head");
}

/// TASK-2323: every `mandate_ship_cover` call site passes the PR's head
/// branch, so the evidence fetch runs on the rework-release path too — not
/// only on ship's and the train's.
// trace:BUG-2214 | ai:claude
#[test]
fn every_mandate_cover_call_site_passes_the_head_branch() {
    // trace:BUG-2214.ac2210da | ai:claude
    let rework = include_str!("../rework_hold_release.rs")
        .split("mandate: &||")
        .nth(1)
        .and_then(|s| s.split("comment:").next())
        .expect("the rework-release mandate closure");
    assert!(
        rework.contains("head_branch.as_deref()"),
        "the rework-release cover must pass the PR's head branch"
    );
    assert!(
        rework.contains("change_metadata"),
        "the head branch comes from the forge's change metadata"
    );
}
