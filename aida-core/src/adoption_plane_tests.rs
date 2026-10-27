// trace:STORY-1741 | ai:claude
use super::*;
use std::os::unix::fs::PermissionsExt;

fn git_out(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn git_in(dir: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut child = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn repo(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git_out(dir, &["init", "-q", "-b", "main"]);
    git_out(dir, &["config", "user.email", "t@example.invalid"]);
    git_out(dir, &["config", "user.name", "t"]);
    dir.canonicalize().unwrap()
}

/// An orphan commit holding `adoption/settings.toml` (when `settings` is
/// set) at `reference`, built from objects only: no checkout, no product
/// commit, no working-tree write.
fn plane_commit(dir: &Path, reference: &str, settings: Option<(&str, &str)>) -> String {
    let readme = git_in(dir, &["hash-object", "-w", "--stdin"], "store\n");
    let mut root = format!("100644 blob {readme}\tREADME\n");
    if let Some((mode, body)) = settings {
        let blob = git_in(dir, &["hash-object", "-w", "--stdin"], body);
        let sub = git_in(
            dir,
            &["mktree"],
            &format!("{mode} blob {blob}\tsettings.toml\n"),
        );
        root.push_str(&format!("040000 tree {sub}\tadoption\n"));
    }
    let tree = git_in(dir, &["mktree"], &root);
    let commit = git_out(dir, &["commit-tree", &tree, "-m", "plane"]);
    git_out(dir, &["update-ref", reference, &commit]);
    commit
}

const GOOD: &str = r#"
schema = 1

[settings.agents]
vendor = "claude"
claude = { model = "opus" }

[settings.notify]
quiet_hours = ["22:00", "07:00"]

[guidance]
review-norms = "Review every PR against its spec."
"#;

fn locator(repo: &Path, body: &str, mode: u32) {
    let common = crate::allocation_authority::common_dir(repo).unwrap();
    let path = common.join(LOCATOR_FILE);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn refusal(result: Result<Option<Plane>>) -> String {
    format!("{:#}", result.expect_err("expected a refusal"))
}

#[test]
fn conventional_plane_is_found_without_any_product_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let commit = plane_commit(&top, STORE_REF, Some(("100644", GOOD)));
    let nested = top.join("src/deep");
    std::fs::create_dir_all(&nested).unwrap();

    let plane = discover(&nested).unwrap().expect("plane");
    assert_eq!(plane.repo, top);
    assert_eq!(
        plane.source,
        Source::Conventional {
            reference: STORE_REF.into()
        }
    );
    let adoption = load(&plane).unwrap().expect("adoption namespace");
    assert_eq!(adoption.revision.commit, commit);
    assert_eq!(adoption.revision.schema, SCHEMA);
    assert_eq!(
        adoption.settings["agents"]["vendor"].as_str(),
        Some("claude")
    );
    assert!(adoption.guidance.contains_key("review-norms"));
    verify(&adoption, &commit).unwrap();

    // A repo with no product commit at all: discovery and reading never
    // manufacture one, and the product tree is untouched.
    assert!(git_out(&top, &["status", "--porcelain", "--ignored"]).is_empty());
    assert!(!top.join(".aida").exists());
    assert!(crate::test_git::git_command()
        .current_dir(&top)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .unwrap()
        .stdout
        .is_empty());
}

#[test]
fn a_clone_without_a_plane_is_not_adopted_and_a_plane_without_adoption_reads_none() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    assert_eq!(discover(&top).unwrap(), None);
    assert_eq!(discover(tmp.path()).unwrap(), None);

    plane_commit(&top, STORE_REF, None);
    let plane = discover(&top).unwrap().unwrap();
    assert_eq!(load(&plane).unwrap(), None);
}

#[test]
fn the_nearest_repository_wins_and_an_ancestor_plane_is_never_adopted() {
    let tmp = tempfile::tempdir().unwrap();
    let outer = repo(&tmp.path().join("outer"));
    plane_commit(&outer, STORE_REF, Some(("100644", GOOD)));
    let inner = repo(&outer.join("vendor/inner"));
    assert_eq!(discover(&inner.join("x")).unwrap(), None);
    assert_eq!(discover(&outer).unwrap().unwrap().repo, outer);
}

#[test]
fn a_temp_root_repository_is_never_adopted() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    plane_commit(&top, STORE_REF, Some(("100644", GOOD)));
    assert_eq!(discover_with_roots(&top, &[top.clone()]).unwrap(), None);
    assert!(discover_with_roots(&top, &[]).unwrap().is_some());
}

#[test]
fn a_linked_worktree_shares_its_clones_plane() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    git_out(&top, &["commit", "-q", "--allow-empty", "-m", "product"]);
    let commit = plane_commit(&top, STORE_REF, Some(("100644", GOOD)));
    let linked = tmp.path().join("linked");
    git_out(
        &top,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );

    let plane = discover(&linked).unwrap().unwrap();
    assert_eq!(plane.commit, commit);
    assert_eq!(plane.repo, linked.canonicalize().unwrap());
}

#[test]
fn remote_copies_must_agree_when_there_is_no_local_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let first = plane_commit(
        &top,
        "refs/remotes/origin/aida-store",
        Some(("100644", GOOD)),
    );
    git_out(&top, &["update-ref", "refs/remotes/hub/aida-store", &first]);
    let plane = discover(&top).unwrap().unwrap();
    assert_eq!(plane.commit, first);

    plane_commit(&top, "refs/remotes/hub/aida-store", None);
    assert!(refusal(discover(&top)).contains("Remotes disagree"));

    // A local branch is the explicit choice and settles it.
    git_out(&top, &["update-ref", STORE_REF, &first]);
    assert_eq!(discover(&top).unwrap().unwrap().commit, first);
}

#[test]
fn a_locator_ref_selects_a_nonstandard_plane_and_never_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    plane_commit(&top, STORE_REF, Some(("100644", GOOD)));
    let other = plane_commit(&top, "refs/aida/team-plane", Some(("100644", GOOD)));

    locator(&top, "version = 1\nref = \"refs/aida/team-plane\"\n", 0o600);
    let plane = discover(&top).unwrap().unwrap();
    assert_eq!(plane.commit, other);
    assert_eq!(
        plane.source,
        Source::LocatorRef {
            reference: "refs/aida/team-plane".into()
        }
    );

    locator(&top, "version = 1\nref = \"refs/aida/missing\"\n", 0o600);
    assert!(refusal(discover(&top)).contains("does not resolve"));
    locator(&top, "version = 1\nref = \"HEAD\"\n", 0o600);
    assert!(refusal(discover(&top)).contains("full ref name"));
    locator(&top, "version = 1\nref = \"refs/heads/x^{tree}\"\n", 0o600);
    assert!(refusal(discover(&top)).contains("full ref name"));
}

#[test]
fn an_ambiguous_malformed_or_unsafe_locator_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    plane_commit(&top, STORE_REF, Some(("100644", GOOD)));
    for body in [
        "version = 1\n",
        "version = 1\nref = \"refs/heads/aida-store\"\nstore = \"/srv/store\"\n",
    ] {
        locator(&top, body, 0o600);
        assert!(refusal(discover(&top)).contains("exactly one"), "{body}");
    }
    for body in [
        "version = 2\nref = \"refs/heads/aida-store\"\n",
        "version = 1\nref = \"refs/heads/aida-store\"\ntoken = \"x\"\n",
        "not toml",
    ] {
        locator(&top, body, 0o600);
        assert!(refusal(discover(&top)).contains("locator"), "{body}");
    }
    locator(
        &top,
        "version = 1\nref = \"refs/heads/aida-store\"\n",
        0o644,
    );
    assert!(
        discover(&top).is_err(),
        "a group/other-readable locator is not private"
    );
}

#[test]
fn a_sibling_store_must_be_a_separate_repository_with_an_aida_store_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let store = repo(&tmp.path().join("team-store"));
    let commit = plane_commit(&store, STORE_REF, Some(("100644", GOOD)));

    locator(
        &top,
        &format!("version = 1\nstore = \"{}\"\n", store.display()),
        0o600,
    );
    let plane = discover(&top).unwrap().unwrap();
    assert_eq!(plane.repo, store);
    assert_eq!(plane.commit, commit);
    assert_eq!(
        plane.source,
        Source::Sibling {
            store: store.clone()
        }
    );

    locator(
        &top,
        &format!("version = 1\nstore = \"{}\"\n", top.display()),
        0o600,
    );
    assert!(refusal(discover(&top)).contains("names this clone"));
    locator(&top, "version = 1\nstore = \"team-store\"\n", 0o600);
    assert!(refusal(discover(&top)).contains("absolute"));
    let bare_dir = tmp.path().join("not-a-repo");
    std::fs::create_dir_all(&bare_dir).unwrap();
    locator(
        &top,
        &format!("version = 1\nstore = \"{}\"\n", bare_dir.display()),
        0o600,
    );
    assert!(refusal(discover(&top)).contains("not a Git repository"));
    let empty = repo(&tmp.path().join("empty-store"));
    locator(
        &top,
        &format!("version = 1\nstore = \"{}\"\n", empty.display()),
        0o600,
    );
    assert!(refusal(discover(&top)).contains("no aida-store branch"));
}

#[test]
fn a_read_stays_pinned_when_the_ref_moves_and_verify_names_the_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let first = plane_commit(&top, STORE_REF, Some(("100644", GOOD)));
    let plane = discover(&top).unwrap().unwrap();
    let second = plane_commit(
        &top,
        STORE_REF,
        Some((
            "100644",
            "schema = 1\n[settings.forge]\nprovider = \"glab\"\n",
        )),
    );

    let pinned = load(&plane).unwrap().unwrap();
    assert_eq!(pinned.revision.commit, first);
    assert!(pinned.settings.contains_key("agents"));
    let err = format!("{:#}", verify(&pinned, &second).unwrap_err());
    assert!(err.contains(&first) && err.contains(&second), "{err}");

    let fresh = load(&discover(&top).unwrap().unwrap()).unwrap().unwrap();
    verify(&fresh, &second).unwrap();
    assert_eq!(fresh.settings["forge"]["provider"].as_str(), Some("glab"));
}

fn load_body(body: &str, mode: &str) -> Result<Option<Adoption>> {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    plane_commit(&top, STORE_REF, Some((mode, body)));
    load(&discover(&top).unwrap().unwrap())
}

#[test]
fn only_inert_policy_keys_are_admitted() {
    let refused = |body: &str| format!("{:#}", load_body(body, "100644").unwrap_err());
    // A command that only the trusted default-branch copy may carry.
    assert!(
        refused("schema = 1\n[settings.notify]\ncommand = \"sh -c x\"\n")
            .contains("notify.command")
    );
    // A frozen credential-shaped breach and a data location.
    assert!(
        refused("schema = 1\n[settings.terminal]\nsend_token = \"t\"\n")
            .contains("not a policy key")
    );
    assert!(
        refused("schema = 1\n[settings.deployment]\nstore_path = \"../s\"\n")
            .contains("not a policy key")
    );
    // Unknown sections and nested tables under a named key.
    assert!(
        refused("schema = 1\n[settings.hooks]\npre_commit = \"x\"\n").contains("not a policy key")
    );
    assert!(
        refused("schema = 1\n[settings.forge]\nprovider = { run = \"x\" }\n")
            .contains("not a policy key")
    );
    // Unknown top-level fields and schemas.
    assert!(refused("schema = 1\n[hooks]\nx = 1\n").contains("invalid"));
    assert!(refused("schema = 2\n").contains("schema 2"));
    assert!(refused("[settings.forge]\nprovider = \"gh\"\n").contains("invalid"));
    assert!(refused("schema = 1\n[guidance]\n\"../x\" = \"t\"\n").contains("slug"));
}

#[test]
fn the_settings_entry_must_be_a_regular_file() {
    for mode in ["100755", "120000"] {
        let err = format!("{:#}", load_body(GOOD, mode).unwrap_err());
        assert!(err.contains("not a regular file"), "{mode}: {err}");
    }
}

#[test]
fn every_policy_row_in_the_trust_manifest_is_admissible() {
    let policy = policy_keys();
    assert!(policy["agents"].iter().any(|k| k == "vendor"));
    assert!(!policy.contains_key("deployment"));
    assert!(!policy["terminal"].iter().any(|k| k == "send_token"));
    assert!(!policy
        .values()
        .flatten()
        .any(|k| k == "command" || k.ends_with("_command")));
}
