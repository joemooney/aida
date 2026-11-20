//! STORY-2905 S1t-b: a train's merge authority is judged before each forge
//! merge attempt: the landing policy and floor at its proof, the merger
//! rule, each car's author rule, and the stacked proof's base.
// trace:STORY-2905 | ai:claude

use super::{train_recheck, Entry};
use crate::gate::ship::ShipRefusal;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn commit(dir: &Path, file: &str, body: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// A repo with `config` on main and a `train/x` branch on main's tip,
/// served from a local origin and fetched. Returns it and main's tip.
fn repo(config: &str) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    commit(&repo, ".aida/config.toml", config);
    git(&repo, &["branch", "train/x"]);
    let origin = root.join("origin.git");
    git(
        &root,
        &["clone", "-q", "--bare", "repo", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["fetch", "-q", "origin"]);
    let tip = String::from_utf8(
        crate::test_git::git_command()
            .current_dir(&repo)
            .args(["rev-parse", "main"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    (temp, repo, tip)
}

fn train() -> crate::forge::ChangeRef {
    crate::forge::ChangeRef {
        id: 90,
        url: String::new(),
        branch: "train/x".into(),
        base: "main".into(),
        title: None,
    }
}

fn entry(repo: &Path) -> Entry {
    Entry {
        policy: crate::landing_policy::load(repo),
        floor: false,
    }
}

#[test]
fn a_ci_gated_train_on_its_proved_base_passes() {
    let (_t, repo, tip) = repo("[land]\nwip_max = 4\n");
    let cars = vec![(7, vec!["TASK-1".to_string()])];
    assert_eq!(
        train_recheck(&repo, &train(), &cars, &entry(&repo), "main", &tip),
        Ok(())
    );
}

/// The stacked proof is bound to the base it was built on: a base that
/// moved since refuses the merge.
#[test]
fn a_base_that_moved_since_the_proof_refuses_the_train() {
    let (_t, repo, tip) = repo("[land]\nwip_max = 4\n");
    let snapshot = entry(&repo);
    let origin = repo.parent().unwrap().join("origin.git");
    commit(&repo, "notes/later.txt", "later");
    git(&repo, &["push", "-q", origin.to_str().unwrap(), "main"]);
    let (kind, why) = train_recheck(&repo, &train(), &[], &snapshot, "main", &tip).unwrap_err();
    assert_eq!(kind, ShipRefusal::BehindBase, "{why}");
    assert!(why.contains("moved since its stacked proof"), "{why}");
}

/// A landing policy that changed since the proof refuses the train.
#[test]
fn a_policy_change_since_the_proof_refuses_the_train() {
    let (_t, repo, tip) = repo("[land]\nwip_max = 4\n");
    let proved_under = entry(&repo);
    let origin = repo.parent().unwrap().join("origin.git");
    commit(
        &repo,
        ".aida/config.toml",
        "[landing]\nmode = \"review-gated\"\n",
    );
    git(&repo, &["push", "-q", origin.to_str().unwrap(), "main"]);
    let (kind, why) = train_recheck(&repo, &train(), &[], &proved_under, "main", &tip).unwrap_err();
    assert_eq!(kind, ShipRefusal::Review, "{why}");
    assert!(why.contains("landing policy changed"), "{why}");
}

/// In review-gated mode the advisor (or the operator) merges: a session
/// with no advisor seat (as after an advisor grant lapses mid-train) is
/// refused before the train merges.
#[test]
fn a_train_without_advisor_authority_is_refused_in_review_gated_mode() {
    let (_t, repo, tip) = repo("[landing]\nmode = \"review-gated\"\n");
    let (kind, why) = train_recheck(
        &repo,
        &train(),
        &[(7, vec!["TASK-1".to_string()])],
        &entry(&repo),
        "main",
        &tip,
    )
    .unwrap_err();
    assert_eq!(kind, ShipRefusal::Review, "{why}");
    assert!(why.contains("has the advisor perform"), "{why}");
}

/// Round 2 finding 2: a landing config that does not parse is never taken
/// as the ci-gated default; the train is refused.
#[test]
fn a_malformed_landing_config_refuses_the_train() {
    let (_t, repo, tip) = repo("landing = 3\n");
    let (kind, why) = train_recheck(
        &repo,
        &train(),
        &[(7, vec!["TASK-1".to_string()])],
        &entry(&repo),
        "main",
        &tip,
    )
    .unwrap_err();
    assert_eq!(kind, ShipRefusal::Review, "{why}");
    assert!(
        why.contains("landing policy cannot be established"),
        "{why}"
    );
}

/// Promotion 23 review F2: the non-train author rule (C2), shared by ship,
/// drain, burndown, release and land, fails closed on an authorship ledger
/// it cannot trust. The session holds a valid advisor grant; the change's
/// author worked under a lease since released, so only the ledger could
/// name them, and a missing or torn ledger must refuse, never read as "no
/// authors".
// trace:STORY-2903 trace:STORY-2905 | ai:claude
fn c2_with_ledger(ledger: Option<&str>) -> Result<(), String> {
    let (_t, repo, _tip) = repo("[landing]\nmerger = \"advisor\"\n");
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat(&repo, "advisor", &[]);
    assert_eq!(
        crate::gate::seat_authority::current_seat(&repo).as_deref(),
        Some("advisor")
    );
    if let Some(body) = ledger {
        std::fs::write(repo.join(".aida/review-authorship.jsonl"), body).unwrap();
    }
    let change = crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: "train/x".into(),
        base: "main".into(),
        title: None,
    };
    super::caller_may_merge(
        &repo,
        &change,
        &["TASK-1".to_string()],
        &crate::landing_policy::load(&repo),
        &|| false,
    )
}

const OTHER_AUTHOR: &str = r#"{"scope":"TASK-1","lease_id":"released","branch":"train/x","tokens":["session:some-implementer"],"recorded_at":"2026-11-19T00:00:00Z"}"#;

#[test]
fn a_missing_authorship_ledger_refuses_a_non_train_merge() {
    let why = c2_with_ledger(None).unwrap_err();
    assert!(why.contains("authors cannot be established"), "{why}");
}

#[test]
fn a_malformed_authorship_row_refuses_a_non_train_merge() {
    let why = c2_with_ledger(Some(&format!("{OTHER_AUTHOR}\n{{torn\n"))).unwrap_err();
    assert!(why.contains("authors cannot be established"), "{why}");
}

/// Control: a well-formed ledger naming another author lets the
/// independent advisor merge.
#[test]
fn a_trusted_ledger_naming_another_author_lets_the_advisor_merge() {
    c2_with_ledger(Some(&format!("{OTHER_AUTHOR}\n"))).expect("independent advisor");
}
