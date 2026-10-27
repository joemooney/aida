//! `aida db audit-ids`: parsing, policy and the read-only audit over real
//! store commits.
// trace:BUG-2017 | ai:claude
use super::*;
use aida_core::models::Requirement;

const LEGACY_NAME: &str = "legacy-seat-name";
const LEGACY_EMAIL: &str = "leaky.person@example.invalid";
const LEGACY_HOST: &str = "legacy-host.example.invalid";
const LEGACY_PATH: &str = "/home/legacy-user/private-clone";
const LEGACY_USER: &str = "legacy-user";

fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(repo)
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

fn write(repo: &Path, rel: &str, text: &str) {
    let path = repo.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn claim(repo: &Path, spec: &str) {
    let mut r = Requirement::new("title".into(), "body".into());
    r.spec_id = Some(spec.into());
    // The serialized object at its store path, as the store writer lays it
    // out; the writer's allocation admission is not under test here.
    let path = aida_core::object_store::object_path(&repo.join("objects"), spec).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_yaml::to_string(&r).unwrap()).unwrap();
}

fn commit(repo: &Path, msg: &str) {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", msg]);
}

/// A store on `aida-store` holding one FR block, a legacy node row full of
/// identifying fields, and FR-1; branch `other` forks from it.
fn store() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("store");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "aida-store"]);
    git(&repo, &["config", "user.name", "Generic"]);
    git(&repo, &["config", "user.email", "generic@aida.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    write(
        &repo,
        "registry/nodes.toml",
        &format!(
            "[[nodes]]\nid = \"N1\"\nuser_id = 1\nhostname = \"{LEGACY_HOST}\"\nemail = \"{LEGACY_EMAIL}\"\nclone_path = \"{LEGACY_PATH}\"\nname = \"{LEGACY_NAME}\"\nuser = \"{LEGACY_USER}\"\nregistered = \"2026-01-01T00:00:00Z\"\n"
        ),
    );
    write(
        &repo,
        "registry/blocks.yaml",
        &format!(
            "blocks:\n- node_id: N1\n  owner: {LEGACY_EMAIL}\n  hostname: {LEGACY_HOST}\n  type_prefix: 'FR'\n  range_start: 1\n  range_end: 100\n  next: 2\n  allocated_at: 2026-01-01T00:00:00Z\n"
        ),
    );
    claim(&repo, "FR-1");
    commit(&repo, "seed");
    git(&repo, &["branch", "other"]);
    (tmp, repo)
}

fn refs_and_tree(repo: &Path) -> (String, String, String) {
    (
        git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        git(repo, &["status", "--porcelain", "--ignored"]),
        git(repo, &["rev-parse", "HEAD"]),
    )
}

fn assert_sanitized(text: &str) {
    for leak in [
        LEGACY_NAME,
        LEGACY_EMAIL,
        LEGACY_HOST,
        LEGACY_PATH,
        LEGACY_USER,
        "FR-2",
    ] {
        assert!(
            !text.contains(leak),
            "audit output leaked {leak:?}:\n{text}"
        );
    }
}

#[test]
fn bug_2017_audit_ids_parses_as_a_read_only_db_verb() {
    let cli = crate::command_registry::try_parse_from([
        "aida",
        "db",
        "audit-ids",
        "--json",
        "--against",
        "origin/aida-store",
        "--against",
        "other",
    ])
    .unwrap();
    let crate::cli::Command::Registered(invocation) = &cli.command;
    let policy = invocation.policy();
    assert_eq!(
        policy.stakeholder,
        crate::stakeholder_role::StakeholderAction::Read
    );
    assert!(!policy.per_write_auto_push);
    assert!(!policy.team_identity_write);
}

#[test]
fn bug_2017_audit_ids_clean_store_reports_clean_with_no_plan() {
    let (_tmp, repo) = store();
    git(&repo, &["checkout", "-q", "other"]);
    claim(&repo, "FR-2");
    commit(&repo, "other side");
    git(&repo, &["checkout", "-q", "aida-store"]);
    let report = audit(&repo, &["other".to_string()]);
    assert!(report.is_clean(), "{report}");
    assert!(repair_plan(&report).is_empty());
    let human = render_human(&report);
    assert!(human.starts_with("Allocation claims audit: clean (0 conflict(s), 0 gap(s))"));
    let json: serde_json::Value = serde_json::from_str(&render_json(&report).unwrap()).unwrap();
    assert_eq!(json["status"], "clean");
    assert_eq!(json["counts"]["conflicts"]["display_id"], 0);
    assert_eq!(json["repair_plan"].as_array().unwrap().len(), 0);
}

#[test]
fn bug_2017_audit_ids_reports_a_disputed_display_id_sanitized_and_changes_nothing() {
    let (_tmp, repo) = store();
    git(&repo, &["checkout", "-q", "other"]);
    claim(&repo, "FR-2");
    commit(&repo, "other side");
    git(&repo, &["checkout", "-q", "aida-store"]);
    claim(&repo, "FR-2");
    commit(&repo, "local side");
    let before = refs_and_tree(&repo);

    let report = audit(&repo, &["other".to_string()]);
    assert_eq!(refs_and_tree(&repo), before, "the audit must write nothing");
    assert!(report.is_complete());
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].kind, ConflictKind::DisplayId);

    let human = render_human(&report);
    assert!(human.contains("Allocation claims audit: conflicts (1 conflict(s), 0 gap(s))"));
    assert!(human.contains("different objects claim one display ID"));
    assert!(human.contains("aida db block claim"));
    assert!(human.contains("Keep every object UUID"));
    assert_sanitized(&human);

    let json_text = render_json(&report).unwrap();
    assert_sanitized(&json_text);
    let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
    assert_eq!(json["status"], "conflicts");
    assert_eq!(json["counts"]["conflicts"]["display_id"], 1);
    assert_eq!(json["counts"]["conflicts"]["global_range"], 0);
    assert_eq!(json["conflicts"][0]["kind"], "display_id");
    assert_eq!(json["conflicts"][0]["refs"].as_array().unwrap().len(), 2);
    assert!(json["repair_plan"].as_array().unwrap().len() >= 3);
}

#[test]
fn bug_2017_audit_ids_reports_node_and_range_conflicts_together() {
    let (_tmp, repo) = store();
    git(&repo, &["checkout", "-q", "other"]);
    let nodes = std::fs::read_to_string(repo.join("registry/nodes.toml")).unwrap();
    write(
        &repo,
        "registry/nodes.toml",
        &nodes.replace(LEGACY_HOST, "second-host.example.invalid"),
    );
    let blocks = std::fs::read_to_string(repo.join("registry/blocks.yaml")).unwrap();
    write(
        &repo,
        "registry/blocks.yaml",
        &format!(
            "{blocks}- node_id: N2\n  owner: {LEGACY_EMAIL}\n  hostname: {LEGACY_HOST}\n  type_prefix: 'FR'\n  range_start: 50\n  range_end: 150\n  next: 50\n  allocated_at: 2026-01-01T00:00:00Z\n"
        ),
    );
    commit(&repo, "other side");
    git(&repo, &["checkout", "-q", "aida-store"]);
    write(
        &repo,
        "registry/nodes.toml",
        &nodes.replace(LEGACY_HOST, "third-host.example.invalid"),
    );
    commit(&repo, "local side");

    let report = audit(&repo, &["other".to_string()]);
    let kinds: Vec<_> = report.conflicts.iter().map(|c| c.kind).collect();
    assert!(kinds.contains(&ConflictKind::NodeBinding), "{kinds:?}");
    assert!(kinds.contains(&ConflictKind::GlobalRange), "{kinds:?}");
    let human = render_human(&report);
    assert!(human.contains("decide which registration it keeps"));
    assert!(human.contains("Retire the overlapping range rows"));
    for text in [human, render_json(&report).unwrap()] {
        assert_sanitized(&text);
        assert!(!text.contains("second-host") && !text.contains("third-host"));
    }
}

#[test]
fn bug_2017_audit_ids_unknown_ref_is_incomplete_never_clean() {
    let (_tmp, repo) = store();
    let report = audit(&repo, &["refs/remotes/origin/no-such-ref".to_string()]);
    assert!(!report.is_complete());
    assert!(!report.is_clean());
    let human = render_human(&report);
    assert!(human.starts_with("Allocation claims audit: incomplete"));
    assert!(human.contains("a git read failed or was refused"));
    assert!(human.contains("Make every claim readable"));
    assert!(!human.contains("no-such-ref"));
    let json: serde_json::Value = serde_json::from_str(&render_json(&report).unwrap()).unwrap();
    assert_eq!(json["status"], "incomplete");
    assert_eq!(json["counts"]["gaps"]["git_read"], 1);
}
