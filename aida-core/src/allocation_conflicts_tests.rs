//! Allocation-claim conflict collector: pure rules plus every store sync
//! wrapper on real clones.
// trace:BUG-2017 | ai:claude
use super::*;
use crate::git_ops::{
    fast_forward_store, head_sha, merge_union_auto, pull_rebase, pull_rebase_auto_merge,
    rebase_in_progress,
};
use crate::models::Requirement;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Pure collector
// ---------------------------------------------------------------------------

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn tree(commit: &str) -> Tree {
    Tree {
        commit: commit.to_string(),
        ..Tree::default()
    }
}

fn claim(mut t: Tree, id: &str, owner: Uuid) -> Tree {
    let ordinal = t.ids.values().map(BTreeMap::len).sum();
    t.ids
        .entry(id.to_string())
        .or_default()
        .insert(owner, ordinal);
    t
}

fn block(mut t: Tree, node: &str, prefix: &str, start: u32, end: u32, next: u32) -> Tree {
    t.has_blocks = true;
    let ordinal = t.blocks.len();
    t.blocks.push(BlockRow {
        key: (node.into(), prefix.into(), start, end),
        next,
        ordinal,
    });
    t
}

fn node(mut t: Tree, id: &str, payload: &str) -> Tree {
    let ordinal = t.nodes.len();
    t.nodes
        .insert(id.into(), (digest(payload.as_bytes()), ordinal));
    t
}

fn kinds(conflicts: &[Conflict]) -> Vec<ConflictKind> {
    conflicts.iter().map(|c| c.kind).collect()
}

#[test]
fn bug_2017_same_uuid_spec_and_agreed_alias_is_not_a_conflict() {
    let a = claim(claim(tree("a"), "FR-1", uuid(1)), "FR-A1B2-1", uuid(1));
    let b = claim(tree("b"), "FR-1", uuid(1));
    assert!(analyze(&[a, b], &[]).is_empty());
}

#[test]
fn bug_2017_two_uuids_on_one_display_id_conflict_within_and_across_tips() {
    let within = claim(claim(tree("a"), "FR-1", uuid(1)), "FR-1", uuid(2));
    assert_eq!(kinds(&analyze(&[within], &[])), [ConflictKind::DisplayId]);
    let a = claim(tree("a"), "FR-1", uuid(1));
    let b = claim(tree("b"), "FR-1", uuid(2));
    let found = analyze(&[a, b], &[]);
    assert_eq!(kinds(&found), [ConflictKind::DisplayId]);
    let owners: Vec<_> = found[0].refs.iter().filter_map(|r| r.uuid).collect();
    assert_eq!(owners, [uuid(1), uuid(2)]);
}

#[test]
fn bug_2017_one_side_reassigning_a_baseline_display_id_is_not_a_conflict() {
    let base = claim(tree("base"), "FR-1", uuid(1));
    let a = claim(tree("a"), "FR-1", uuid(1));
    let b = claim(tree("b"), "FR-1", uuid(2));
    assert!(analyze(&[a, b], &[base]).is_empty());
}

#[test]
fn bug_2017_node_baseline_repetition_vs_independent_equal_registration() {
    let base = node(tree("base"), "N1", "row");
    let a = node(tree("a"), "N1", "row");
    let b = node(tree("b"), "N1", "row");
    assert!(analyze(&[a.clone(), b.clone()], &[base]).is_empty());
    // The same payload, registered on both sides with no shared history of
    // it, is not proof of one registration.
    assert_eq!(kinds(&analyze(&[a, b], &[])), [ConflictKind::NodeBinding]);
}

#[test]
fn bug_2017_node_binding_changed_on_one_or_both_sides() {
    let base = node(tree("base"), "N1", "row");
    let edited = node(tree("a"), "N1", "renamed");
    let kept = node(tree("b"), "N1", "row");
    assert!(analyze(&[edited.clone(), kept], &[base.clone()]).is_empty());
    let other = node(tree("b"), "N1", "other host");
    assert_eq!(
        kinds(&analyze(&[edited, other], &[base])),
        [ConflictKind::NodeBinding]
    );
}

#[test]
fn bug_2017_independently_claimed_identical_block_conflicts_unless_baseline() {
    let a = block(tree("a"), "N1", "FR", 1, 100, 1);
    let b = block(tree("b"), "N1", "FR", 1, 100, 7);
    let base = block(tree("base"), "N1", "FR", 1, 100, 1);
    assert!(analyze(&[a.clone(), b.clone()], &[base]).is_empty());
    assert_eq!(kinds(&analyze(&[a, b], &[])), [ConflictKind::GlobalRange]);
}

#[test]
fn bug_2017_overlapping_ranges_with_different_owners_conflict() {
    let a = block(tree("a"), "N1", "FR", 1, 100, 1);
    let b = block(tree("b"), "N2", "FR", 50, 150, 50);
    assert_eq!(
        kinds(&analyze(&[a.clone(), b.clone()], &[])),
        [ConflictKind::GlobalRange]
    );
    // An overlap both sides already shared is baseline, not new.
    let base = block(
        block(tree("base"), "N1", "FR", 1, 100, 1),
        "N2",
        "FR",
        50,
        150,
        50,
    );
    assert!(analyze(&[a, b], &[base]).is_empty());
    // Different types never collide.
    let c = block(tree("c"), "N1", "FR", 1, 100, 1);
    let d = block(tree("d"), "N2", "BUG", 1, 100, 1);
    assert!(analyze(&[c, d], &[]).is_empty());
}

#[test]
fn bug_2017_star_range_conflicts_only_where_it_has_dispensed() {
    let star = block(tree("base"), "N0", "*", 1, 1000, 1);
    let dormant = block(star.clone(), "N1", "TASK", 200, 299, 200);
    assert!(analyze(&[dormant.clone(), star.clone()], &[star.clone()]).is_empty());
    let dispensed = block(tree("b"), "N0", "*", 1, 1000, 250);
    assert_eq!(
        kinds(&analyze(&[dormant, dispensed], &[star])),
        [ConflictKind::GlobalRange]
    );
}

#[test]
fn bug_2017_global_id_above_every_range_is_unissued_unless_baseline() {
    let registry = block(tree("a"), "N1", "FR", 1, 100, 1);
    let mut counters = registry.clone();
    counters.counters.insert("FR".into(), 400);
    let a = claim(registry.clone(), "FR-500", uuid(1));
    assert_eq!(kinds(&analyze(&[a.clone()], &[])), [ConflictKind::Unissued]);
    assert!(analyze(&[claim(counters, "FR-400", uuid(1))], &[]).is_empty());
    let base = claim(registry, "FR-500", uuid(1));
    assert!(analyze(&[a], &[base]).is_empty());
    // Node-local IDs are never global claims; stores without a block
    // registry issue no global ranges.
    let local = claim(
        block(tree("a"), "N1", "FR", 1, 100, 1),
        "FR-0123456789ABCDEF0123456789ABCDEF-900",
        uuid(2),
    );
    assert!(analyze(&[local], &[]).is_empty());
    assert!(analyze(&[claim(tree("a"), "FR-500", uuid(1))], &[]).is_empty());
}

#[test]
fn bug_2017_every_category_is_reported_together_in_a_stable_order() {
    let build = || {
        let a = block(
            node(claim(tree("a"), "FR-1", uuid(1)), "N1", "x"),
            "N1",
            "FR",
            1,
            100,
            1,
        );
        let b = block(
            node(
                claim(claim(tree("b"), "FR-1", uuid(2)), "FR-999", uuid(3)),
                "N1",
                "y",
            ),
            "N2",
            "FR",
            90,
            190,
            90,
        );
        (a, b)
    };
    let (a, b) = build();
    let mut first = ConflictReport {
        conflicts: analyze(&[a, b], &[]),
        gaps: Vec::new(),
    };
    first.normalize();
    let (a, b) = build();
    let mut second = ConflictReport {
        conflicts: analyze(&[b, a], &[]),
        gaps: Vec::new(),
    };
    second.normalize();
    assert_eq!(first, second);
    assert_eq!(
        kinds(&first.conflicts),
        [
            ConflictKind::GlobalRange,
            ConflictKind::NodeBinding,
            ConflictKind::DisplayId,
            ConflictKind::Unissued,
        ]
    );
}

#[test]
fn bug_2017_line_scan_matches_strict_parse_or_defers_to_it() {
    let row = Requirement::new("title".into(), "spec_id: FR-9\nid: nope".into());
    let mut row = row;
    row.spec_id = Some("FR-1".into());
    row.agreed_id = Some("FR-2".into());
    let text = serde_yaml::to_string(&row).unwrap();
    let scanned = scan_claim(&text).unwrap();
    assert_eq!(Some(scanned.clone()), strict_claim(&text));
    assert_eq!(scanned, (row.id, Some("FR-1".into()), Some("FR-2".into())));
    // Not plainly shaped: the scan declines and the strict parse decides.
    assert!(scan_claim("---\nid: x\n").is_none());
    assert!(scan_claim("id: a\nid: b\n").is_none());
}

// ---------------------------------------------------------------------------
// Store sync wrappers on real clones
// ---------------------------------------------------------------------------

const SECRET_NODE: &str = "secret-host-joe";
const SECRET_EMAIL: &str = "leaky@example.invalid";

fn run(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
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

struct Clones {
    _tmp: tempfile::TempDir,
    hub: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

fn blocks_yaml(rows: &[(&str, &str, u32, u32)]) -> String {
    let mut out = String::from("blocks:\n");
    for (node, prefix, start, end) in rows {
        out.push_str(&format!(
            "- node_id: {node}\n  owner: {SECRET_EMAIL}\n  hostname: {node}\n  type_prefix: '{prefix}'\n  range_start: {start}\n  range_end: {end}\n  next: {start}\n  allocated_at: 2026-01-01T00:00:00Z\n"
        ));
    }
    out
}

fn nodes_toml(host: &str) -> String {
    format!(
        "[[nodes]]\nid = \"{SECRET_NODE}\"\nuser_id = 1\nhostname = \"{host}\"\nemail = \"{SECRET_EMAIL}\"\nregistered = \"2026-01-01T00:00:00Z\"\n"
    )
}

fn row(spec: &str) -> Requirement {
    let mut r = Requirement::new("title".into(), "body".into());
    r.spec_id = Some(spec.into());
    r
}

fn write_row(repo: &Path, r: &Requirement) {
    crate::object_store::write_object(&repo.join("objects"), r).unwrap();
}

fn write_file(repo: &Path, rel: &str, text: &str) {
    let path = repo.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit(repo: &Path, msg: &str) {
    run(repo, &["add", "-A"]);
    run(repo, &["commit", "-q", "-m", msg]);
}

fn publish(repo: &Path, msg: &str) {
    commit(repo, msg);
    run(repo, &["push", "-q", "origin", "aida-store"]);
}

/// A hub seeded with an FR block 1..100, one node registration and FR-1, and
/// two store clones of it.
fn clones() -> Clones {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let hub = root.join("hub.git");
    run(&root, &["init", "-q", "--bare", hub.to_str().unwrap()]);
    run(&hub, &["symbolic-ref", "HEAD", "refs/heads/aida-store"]);
    let a = root.join("a");
    run(
        &root,
        &["init", "-q", "-b", "aida-store", a.to_str().unwrap()],
    );
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        run(&a, &["config", k, v]);
    }
    crate::allocation_authority::home_fixture_backend(&a).unwrap();
    write_file(&a, "metadata.yaml", "name: test\n");
    write_file(
        &a,
        "registry/blocks.yaml",
        &blocks_yaml(&[(SECRET_NODE, "FR", 1, 100)]),
    );
    write_file(&a, "registry/nodes.toml", &nodes_toml("seed-host"));
    write_row(&a, &row("FR-1"));
    commit(&a, "seed");
    run(&a, &["remote", "add", "origin", hub.to_str().unwrap()]);
    run(&a, &["push", "-q", "-u", "origin", "aida-store"]);
    let b = root.join("b");
    run(
        &root,
        &["clone", "-q", hub.to_str().unwrap(), b.to_str().unwrap()],
    );
    for (k, v) in [("user.email", "t@example.invalid"), ("user.name", "T")] {
        run(&b, &["config", k, v]);
    }
    crate::allocation_authority::home_fixture_backend(&b).unwrap();
    Clones {
        _tmp: tmp,
        hub,
        a,
        b,
    }
}

type Sync = fn(&Path) -> anyhow::Result<()>;

fn via_rebase(repo: &Path) -> anyhow::Result<()> {
    pull_rebase(repo, "origin", "aida-store")
}

fn via_auto_merge(repo: &Path) -> anyhow::Result<()> {
    pull_rebase_auto_merge(repo, "origin", "aida-store").map(|_| ())
}

fn via_union(repo: &Path) -> anyhow::Result<()> {
    run(repo, &["fetch", "-q", "origin"]);
    merge_union_auto(repo, "origin/aida-store", "union").map(|_| ())
}

fn via_fast_forward(repo: &Path) -> anyhow::Result<()> {
    run(repo, &["fetch", "-q", "origin"]);
    fast_forward_store(repo, "origin/aida-store")
}

const SYNCS: [(&str, Sync); 4] = [
    ("rebase", via_rebase),
    ("auto-merge", via_auto_merge),
    ("union", via_union),
    ("fast-forward", via_fast_forward),
];

struct Snapshot {
    head: String,
    hub: String,
    status: String,
}

fn snapshot(c: &Clones, repo: &Path) -> Snapshot {
    Snapshot {
        head: head_sha(repo).unwrap(),
        hub: run(&c.hub, &["rev-parse", "aida-store"]),
        status: run(repo, &["status", "--porcelain"]),
    }
}

/// The refusal is typed, sanitized, and left HEAD, worktree, counters and
/// the hub exactly as they were.
fn assert_refused(
    name: &str,
    c: &Clones,
    repo: &Path,
    before: &Snapshot,
    err: anyhow::Error,
) -> ConflictReport {
    assert!(is_sync_refusal(&err), "{name}: {err:#}");
    let report = err
        .chain()
        .find_map(|e| e.downcast_ref::<AllocationRefusal>())
        .unwrap_or_else(|| panic!("{name}: not an allocation refusal: {err:#}"))
        .0
        .clone();
    let after = snapshot(c, repo);
    assert_eq!(after.head, before.head, "{name}: HEAD moved");
    assert_eq!(after.hub, before.hub, "{name}: hub moved");
    assert_eq!(after.status, before.status, "{name}: worktree changed");
    assert!(!rebase_in_progress(repo), "{name}: rebase left in progress");
    let text = format!("{err:#}");
    for leak in [
        SECRET_NODE,
        SECRET_EMAIL,
        "seed-host",
        "other-host",
        "FR-",
        "t@example.invalid",
        c.hub.parent().unwrap().to_str().unwrap(),
    ] {
        assert!(
            !text.contains(leak),
            "{name}: report leaks {leak:?}: {text}"
        );
    }
    report
}

#[test]
fn bug_2017_every_wrapper_refuses_a_display_id_claimed_by_two_uuids() {
    for (name, sync) in SYNCS {
        let c = clones();
        write_row(&c.a, &row("FR-50"));
        publish(&c.a, "a issues FR-50");
        // The fast-forward wrapper only sees its own tip; the duplicate is
        // carried into the incoming tip as an agreed alias of another UUID.
        if name == "fast-forward" {
            let mut twin = row("FR-0123456789ABCDEF0123456789ABCDEF-1");
            twin.agreed_id = Some("FR-50".into());
            write_row(&c.a, &twin);
            publish(&c.a, "a aliases FR-50 twice");
        } else {
            write_row(&c.b, &row("FR-50"));
            commit(&c.b, "b issues FR-50");
        }
        let before = snapshot(&c, &c.b);
        let err = sync(&c.b).unwrap_err();
        let report = assert_refused(name, &c, &c.b, &before, err);
        assert!(report.is_complete(), "{name}: {report}");
        assert_eq!(
            kinds(&report.conflicts),
            [ConflictKind::DisplayId],
            "{name}"
        );
    }
}

#[test]
fn bug_2017_clean_fast_forward_and_noop_syncs_pass() {
    for (name, sync) in SYNCS {
        let c = clones();
        write_row(&c.a, &row("FR-2"));
        publish(&c.a, "a issues FR-2");
        sync(&c.b).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let hub = run(&c.hub, &["rev-parse", "aida-store"]);
        run(&c.b, &["merge-base", "--is-ancestor", &hub, "HEAD"]);
        // Nothing new: the no-op exit passes the same check.
        sync(&c.b).unwrap_or_else(|e| panic!("{name} no-op: {e:#}"));
    }
}

#[test]
fn bug_2017_noop_exit_still_refuses_a_local_unissued_claim() {
    for (name, sync) in SYNCS {
        let c = clones();
        write_row(&c.b, &row("FR-500"));
        commit(&c.b, "b claims beyond every range");
        if name == "fast-forward" {
            // A fast-forward from an ahead local refuses in Git anyway; the
            // claims check runs first.
            run(&c.b, &["fetch", "-q", "origin"]);
        }
        let before = snapshot(&c, &c.b);
        let err = sync(&c.b).unwrap_err();
        let report = assert_refused(name, &c, &c.b, &before, err);
        assert_eq!(kinds(&report.conflicts), [ConflictKind::Unissued], "{name}");
    }
}

#[test]
fn bug_2017_node_and_range_conflicts_are_reported_together() {
    for (name, sync) in SYNCS.into_iter().filter(|(n, _)| *n != "fast-forward") {
        let c = clones();
        write_file(&c.a, "registry/nodes.toml", &nodes_toml("other-host"));
        write_file(
            &c.a,
            "registry/blocks.yaml",
            &blocks_yaml(&[(SECRET_NODE, "FR", 1, 100), ("N2", "BUG", 1, 50)]),
        );
        publish(&c.a, "a rebinds and claims");
        write_file(&c.b, "registry/nodes.toml", &nodes_toml("third-host"));
        write_file(
            &c.b,
            "registry/blocks.yaml",
            &blocks_yaml(&[(SECRET_NODE, "FR", 1, 100), ("N3", "BUG", 40, 90)]),
        );
        commit(&c.b, "b rebinds and claims");
        let before = snapshot(&c, &c.b);
        let err = sync(&c.b).unwrap_err();
        let report = assert_refused(name, &c, &c.b, &before, err);
        assert_eq!(
            kinds(&report.conflicts),
            [ConflictKind::GlobalRange, ConflictKind::NodeBinding],
            "{name}: {report}"
        );
    }
}

#[test]
fn bug_2017_malformed_or_oversize_claims_are_incomplete_and_refuse() {
    for (name, sync) in SYNCS {
        let c = clones();
        write_file(
            &c.a,
            "registry/nodes.toml",
            "[[nodes]]\nid = 1\nhostname = [\n",
        );
        publish(&c.a, "a breaks the node registry");
        let before = snapshot(&c, &c.b);
        let err = sync(&c.b).unwrap_err();
        let report = assert_refused(name, &c, &c.b, &before, err);
        assert!(!report.is_complete(), "{name}");
        assert_eq!(report.gaps[0].kind, GapKind::Malformed, "{name}");
        assert_eq!(report.gaps[0].class, Some(PathClass::Nodes), "{name}");
    }
    let c = clones();
    let mut big = row("FR-3");
    big.description = "x".repeat(MAX_BLOB_BYTES as usize + 1);
    write_row(&c.a, &big);
    publish(&c.a, "a writes an oversize object");
    let before = snapshot(&c, &c.b);
    let err = via_rebase(&c.b).unwrap_err();
    let report = assert_refused("oversize", &c, &c.b, &before, err);
    assert!(
        report.gaps.iter().any(|g| g.kind == GapKind::Bound),
        "{report}"
    );
}

#[test]
fn bug_2017_unreadable_commits_never_read_as_clean() {
    let c = clones();
    let head = head_sha(&c.b).unwrap();
    let missing = "0123456789abcdef0123456789abcdef01234567".to_string();
    let report = collect(&c.b, Some(&head), &[missing]);
    assert!(!report.is_clean());
    assert_eq!(report.gaps[0].kind, GapKind::GitRead);
    // A path that is no repository at all is a read failure, not empty.
    let report = collect(&c.hub.join("absent"), Some(&head), &[]);
    assert!(!report.is_clean());
}

#[test]
fn bug_2017_legacy_duplicates_already_shared_never_block_a_sync() {
    for (name, sync) in SYNCS {
        let c = clones();
        let mut twin = row("FR-0123456789ABCDEF0123456789ABCDEF-1");
        twin.agreed_id = Some("FR-1".into());
        write_row(&c.a, &twin);
        publish(&c.a, "legacy duplicate");
        run(&c.b, &["pull", "-q", "--ff-only", "origin", "aida-store"]);
        // Local equal to incoming: one distinct tip is its own baseline.
        sync(&c.b).unwrap_or_else(|e| panic!("{name} equal tips: {e:#}"));
        write_row(&c.a, &row("FR-2"));
        publish(&c.a, "a issues FR-2");
        sync(&c.b).unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}

/// BUG-2378: reconcile passes every hub tip, and one hub can still sit at a
/// commit before a registration change (a node marked retired) while the
/// other hub already has it. A tip that is an ancestor of another tip claims
/// nothing new, so joining it is clean; a real divergence still refuses.
// trace:BUG-2378 | ai:claude
#[test]
fn bug_2378_a_hub_behind_another_tip_is_not_a_second_claimant() {
    let c = clones();
    let stale = head_sha(&c.a).unwrap();
    write_file(&c.a, "registry/nodes.toml", &nodes_toml("retired-host"));
    commit(&c.a, "a changes the node registration");
    let updated = head_sha(&c.a).unwrap();
    write_row(&c.a, &row("FR-2"));
    commit(&c.a, "a keeps working");
    let report = collect(&c.a, Some("HEAD"), &[updated.clone(), stale.clone()]);
    assert!(report.is_clean(), "{report}");
    crate::allocation_conflicts::refuse_conflicts(&c.a, Some("HEAD"), &[updated, stale.clone()])
        .unwrap();

    // The same stale tip with an independent change of its own still refuses.
    write_file(&c.b, "registry/nodes.toml", &nodes_toml("other-host"));
    commit(&c.b, "b rebinds");
    let diverged = head_sha(&c.b).unwrap();
    run(&c.a, &["fetch", "-q", c.b.to_str().unwrap(), "aida-store"]);
    let report = collect(&c.a, Some("HEAD"), &[diverged, stale]);
    assert_eq!(
        kinds(&report.conflicts),
        [ConflictKind::NodeBinding],
        "{report}"
    );
}
