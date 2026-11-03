//! STORY-2324 PR-A: the canonical-ID resolver (test plan section 8,
//! "Resolver tests").
// trace:STORY-2324 | ai:claude

use super::*;

const NODE: &str = "b6b1278890774ef993d2ebc327862470";

pub(crate) fn req(native: &str, friendly: Option<&str>) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("fixture {native}"), "fixture".into());
    r.spec_id = Some(native.to_string());
    r.agreed_id = friendly.map(str::to_string);
    r
}

fn aliases() -> String {
    format!("[[binding]]\nlabel = \"HOME\"\nnode = \"{NODE}\"\n")
}

fn retired(old: &str, uuid: Uuid) -> String {
    format!("[[retired]]\nold = \"{old}\"\nnew = \"x\"\nuuid = \"{uuid}\"\n")
}

fn hit(snap: &Snapshot, label: &str) -> Option<SpecKey> {
    match snap.resolve_one(label) {
        Resolution::Hit(k) => Some(k),
        _ => None,
    }
}

#[test]
fn native_and_friendly_resolve_case_insensitively_to_the_uuid() {
    let a = req(&format!("BUG-{NODE}-020"), Some("BUG-2403"));
    let snap = Snapshot::from_parts("h", std::slice::from_ref(&a), None, None).unwrap();
    let key = SpecKey(a.id);
    assert_eq!(hit(&snap, &format!("BUG-{NODE}-020")), Some(key));
    // Upper-cased hex: the spelling the ledger index minted.
    assert_eq!(
        hit(&snap, &format!("BUG-{}-020", NODE.to_ascii_uppercase())),
        Some(key)
    );
    assert_eq!(hit(&snap, "bug-2403"), Some(key));
}

#[test]
fn a_node_label_form_resolves_through_the_committed_binding() {
    let a = req(&format!("BUG-{NODE}-176"), None);
    let snap = Snapshot::from_parts("h", std::slice::from_ref(&a), Some(&aliases()), None).unwrap();
    assert_eq!(hit(&snap, "BUG-HOME-176"), Some(SpecKey(a.id)));
    assert_eq!(hit(&snap, "bug-home-176"), Some(SpecKey(a.id)));
    // Exact text only: no sequence re-padding, prefix or substring match.
    assert_eq!(snap.resolve_one("BUG-HOME-0176"), Resolution::Unknown);
    assert_eq!(snap.resolve_one("BUG-HOME-17"), Resolution::Unknown);
    assert_eq!(
        snap.resolve_one(&format!("BUG-{NODE}")),
        Resolution::Unknown
    );
}

#[test]
fn a_retired_id_resolves_only_to_an_existing_uuid() {
    let a = req(&format!("CR-{NODE}-001"), None);
    let snap = Snapshot::from_parts(
        "h",
        std::slice::from_ref(&a),
        None,
        Some(&retired("CR-1-116", a.id)),
    )
    .unwrap();
    assert_eq!(hit(&snap, "CR-1-116"), Some(SpecKey(a.id)));

    let gone = Snapshot::from_parts(
        "h",
        std::slice::from_ref(&a),
        None,
        Some(&retired("CR-1-117", Uuid::new_v4())),
    )
    .unwrap();
    assert_eq!(gone.resolve_one("CR-1-117"), Resolution::Unknown);
}

#[test]
fn an_unknown_label_is_reported_unresolved_and_change_keys_are_skipped() {
    let a = req("STORY-1", None);
    let snap = Snapshot::from_parts("h", std::slice::from_ref(&a), None, None).unwrap();
    let set = snap.resolve(&["story-1", "STORY-1", "TASK-99", "PR-12", " ", "???"]);
    assert_eq!(set.members.len(), 1);
    assert_eq!(set.members[0].key, SpecKey(a.id));
    assert_eq!(set.unresolved, vec!["TASK-99".to_string()]);
    assert!(set.ambiguous.is_empty());
}

#[test]
fn two_requirements_claiming_one_label_is_an_error_not_a_pick() {
    let a = req("STORY-1", Some("STORY-77"));
    let b = req("STORY-2", Some("STORY-77"));
    let snap = Snapshot::from_parts("h", &[a.clone(), b.clone()], None, None).unwrap();
    assert!(matches!(
        snap.resolve_one("STORY-77"),
        Resolution::Ambiguous(_)
    ));
    let set = snap.resolve(&["STORY-77"]);
    assert!(set.members.is_empty());
    assert_eq!(set.ambiguous.len(), 1);
    // The contested label belongs to neither requirement's label set.
    assert!(!snap.spec(SpecKey(a.id)).unwrap().answers_to("STORY-77"));
    assert!(snap.spec(SpecKey(a.id)).unwrap().answers_to("story-1"));
}

#[test]
fn a_literal_claim_and_a_retired_mapping_to_another_requirement_is_ambiguous() {
    let a = req("TASK-5", None);
    let b = req("TASK-6", None);
    let snap = Snapshot::from_parts(
        "h",
        &[a.clone(), b.clone()],
        None,
        Some(&retired("TASK-5", b.id)),
    )
    .unwrap();
    assert!(
        matches!(snap.resolve_one("TASK-5"), Resolution::Ambiguous(_)),
        "a label A claims literally but the retired map gives B is an Err"
    );
    assert!(matches!(
        snap.resolve_one("task-5"),
        Resolution::Ambiguous(_)
    ));
    // Neither requirement owns the contested label.
    assert!(!snap.spec(SpecKey(a.id)).unwrap().answers_to("TASK-5"));
    assert!(!snap.spec(SpecKey(b.id)).unwrap().answers_to("TASK-5"));
    let set = snap.resolve(&["TASK-5"]);
    assert!(set.members.is_empty());
    assert_eq!(set.ambiguous.len(), 1);
    // A retired mapping back to the claimant itself is not a second claim.
    let snap =
        Snapshot::from_parts("h", &[a.clone()], None, Some(&retired("TASK-5", a.id))).unwrap();
    assert_eq!(hit(&snap, "TASK-5"), Some(SpecKey(a.id)));
}

#[test]
fn the_label_set_carries_every_spelling_and_the_superseded_edge() {
    let mut a = req(&format!("BUG-{NODE}-176"), Some("BUG-2413"));
    let b = req("STORY-9", None);
    a.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::SupersededBy,
        target_id: b.id,
        created_at: None,
        created_by: None,
    });
    let snap = Snapshot::from_parts(
        "h",
        &[a.clone(), b.clone()],
        Some(&aliases()),
        Some(&retired("BUG-1-9", a.id)),
    )
    .unwrap();
    let spec = snap.spec(SpecKey(a.id)).unwrap();
    assert_eq!(spec.superseded_by, Some(SpecKey(b.id)));
    for l in [
        format!("BUG-{NODE}-176"),
        "BUG-2413".into(),
        "BUG-HOME-176".into(),
        "BUG-1-9".into(),
    ] {
        assert!(spec.answers_to(&l), "{l} should answer: {:?}", spec.labels);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(ok.status.success(), "git {args:?}: {ok:?}");
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[test]
fn the_snapshot_reads_head_and_ignores_a_dirty_working_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path();
    git(store, &["init", "-q", "-b", "main"]);
    let a = req("STORY-1", Some("STORY-100"));
    let yaml = serde_yaml::to_string(&a).unwrap();
    write(store, "objects/STORY/000/STORY-1.yaml", &yaml);
    write(store, NODE_ALIASES_REL, &aliases());
    // Not a requirement object: outside the TYPE/SHARD/FILE layout.
    write(store, "objects/README.yaml", "not: a requirement\n");
    git(store, &["add", "-A"]);
    git(store, &["commit", "-q", "-m", "seed"]);

    // Uncommitted: a re-labelled object and a brand-new one.
    let mut edited = a.clone();
    edited.agreed_id = Some("STORY-200".into());
    write(
        store,
        "objects/STORY/000/STORY-1.yaml",
        &serde_yaml::to_string(&edited).unwrap(),
    );
    write(
        store,
        "objects/STORY/000/STORY-2.yaml",
        &serde_yaml::to_string(&req("STORY-2", None)).unwrap(),
    );

    let snap = Snapshot::of_store(store).unwrap();
    assert_eq!(snap.head.len(), 40);
    assert_eq!(hit(&snap, "STORY-100"), Some(SpecKey(a.id)), "HEAD wins");
    assert_eq!(snap.resolve_one("STORY-200"), Resolution::Unknown);
    assert_eq!(snap.resolve_one("STORY-2"), Resolution::Unknown);
}

#[test]
fn a_snapshot_refuses_a_directory_that_is_not_its_own_repository() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    let nested = tmp.path().join("store");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(Snapshot::of_store(&nested).is_err());
}

#[test]
fn the_batch_parser_refuses_a_truncated_or_mismatched_answer() {
    let entries = vec![("abc1".to_string(), "objects/A/000/A-1.yaml".to_string())];
    assert_eq!(
        parse_batch(b"abc1 blob 3\nxyz\n", &entries).unwrap(),
        vec![b"xyz".as_slice()]
    );
    assert!(parse_batch(b"abc1 blob 9\nxyz\n", &entries).is_err());
    assert!(parse_batch(b"ffff blob 3\nxyz\n", &entries).is_err());
    assert!(parse_batch(b"abc1 missing\n", &entries).is_err());
}
