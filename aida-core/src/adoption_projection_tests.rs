// trace:STORY-1741 | ai:claude
use super::*;
use crate::adoption_plane::{discover, load, STORE_REF};

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

/// A product repo with one tracked file and one product commit.
fn product(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git_out(dir, &["init", "-q", "-b", "main"]);
    git_out(dir, &["config", "user.email", "t@example.invalid"]);
    git_out(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("README.md"), "product\n").unwrap();
    git_out(dir, &["add", "README.md"]);
    git_out(dir, &["commit", "-q", "-m", "product"]);
    dir.canonicalize().unwrap()
}

/// Publish `settings` as the plane's adoption namespace, from objects only.
fn publish(dir: &Path, settings: &str) -> String {
    let blob = git_in(dir, &["hash-object", "-w", "--stdin"], settings);
    let sub = git_in(
        dir,
        &["mktree"],
        &format!("100644 blob {blob}\tsettings.toml\n"),
    );
    let tree = git_in(dir, &["mktree"], &format!("040000 tree {sub}\tadoption\n"));
    let commit = git_out(dir, &["commit-tree", &tree, "-m", "plane"]);
    git_out(dir, &["update-ref", STORE_REF, &commit]);
    commit
}

const V1: &str = r#"
schema = 1

[settings._top]
use_agreed_blocks = true

[settings.agents]
vendor = "claude"

[settings."store.sync"]
auto_push = false

[guidance]
review-norms = "Review every PR against its spec.\n"
"#;

const V2: &str = r#"
schema = 1

[settings.agents]
vendor = "codex"

[guidance]
style = "Prefer small PRs.\n"
"#;

/// Everything about the product side that adoption must never change.
fn product_snapshot(top: &Path) -> String {
    let tracked = git_out(top, &["ls-files", "-s"]);
    let refs = git_out(
        top,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/main",
        ],
    );
    let remotes = git_out(top, &["remote", "-v"]);
    let status = git_out(top, &["status", "--porcelain"]);
    let bytes: Vec<String> = tracked
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .map(|p| format!("{p}:{:?}", std::fs::read(top.join(p)).ok()))
        .collect();
    format!(
        "{tracked}\n{refs}\n{remotes}\n{status}\n{}",
        bytes.join("\n")
    )
}

fn adoption(top: &Path) -> Adoption {
    load(&discover(top).unwrap().unwrap()).unwrap().unwrap()
}

#[test]
fn a_projection_feeds_readers_without_touching_the_product_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    // A pre-dirty product tree stays exactly as dirty.
    std::fs::write(top.join("README.md"), "local edit\n").unwrap();
    std::fs::write(top.join("scratch.txt"), "untracked\n").unwrap();
    let before = product_snapshot(&top);
    let commit = publish(&top, V1);

    let outcome = sync(&top).unwrap().unwrap();
    assert_eq!(
        outcome,
        Outcome {
            commit: commit.clone(),
            written: true,
            config_tracked: false
        }
    );
    assert_eq!(product_snapshot(&top), before);

    let config: toml::Table =
        toml::from_str(&std::fs::read_to_string(top.join(CONFIG)).unwrap()).unwrap();
    assert_eq!(config["use_agreed_blocks"].as_bool(), Some(true));
    assert_eq!(config["agents"]["vendor"].as_str(), Some("claude"));
    assert_eq!(config["store"]["sync"]["auto_push"].as_bool(), Some(false));
    assert_eq!(
        std::fs::read_to_string(top.join(GUIDANCE_DIR).join("review-norms.md")).unwrap(),
        "Review every PR against its spec.\n"
    );
    assert_eq!(
        inspect(&top).unwrap(),
        State::Current {
            commit: commit.clone()
        }
    );
    let Delivery::Verified { commit: rev, docs } = delivered_guidance(&top) else {
        panic!("guidance not delivered");
    };
    assert_eq!(rev, commit);
    assert_eq!(docs[0].0, "review-norms");
    assert!(is_projected(&top));
    assert!(owns(&top.join(CONFIG)));
    assert!(!owns(&top.join("README.md")));
}

#[test]
fn a_refresh_is_deterministic_and_replaces_only_owned_outputs() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap();
    let first = std::fs::read(top.join(CONFIG)).unwrap();
    let manifest = std::fs::read(top.join(MANIFEST)).unwrap();

    // Same revision: no write at all.
    assert!(!sync(&top).unwrap().unwrap().written);
    assert_eq!(std::fs::read(top.join(MANIFEST)).unwrap(), manifest);

    // A second worktree renders identical bytes from the same revision.
    let other = tmp.path().join("other");
    git_out(
        &top,
        &["worktree", "add", "-q", other.to_str().unwrap(), "-b", "f"],
    );
    sync(&other).unwrap();
    assert_eq!(std::fs::read(other.join(CONFIG)).unwrap(), first);

    // A new revision replaces the config and retires the dropped guidance.
    let before = product_snapshot(&top);
    let second = publish(&top, V2);
    assert_eq!(
        inspect(&top).unwrap(),
        State::Stale {
            projected: read_manifest(&top).unwrap().unwrap().commit,
            plane: second.clone()
        }
    );
    assert!(sync(&top).unwrap().unwrap().written);
    assert!(!top.join(GUIDANCE_DIR).join("review-norms.md").exists());
    assert!(top.join(GUIDANCE_DIR).join("style.md").is_file());
    assert_eq!(read_manifest(&top).unwrap().unwrap().commit, second);
    assert_eq!(product_snapshot(&top), before);
}

#[test]
fn an_edited_or_unowned_output_is_preserved_and_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);

    // A config file the projection never wrote.
    std::fs::create_dir_all(top.join(".aida")).unwrap();
    std::fs::write(top.join(CONFIG), "mine = 1\n").unwrap();
    let err = format!("{:#}", sync(&top).unwrap_err());
    assert!(
        err.contains("not written by the adoption projection"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(top.join(CONFIG)).unwrap(),
        "mine = 1\n"
    );

    std::fs::remove_file(top.join(CONFIG)).unwrap();
    sync(&top).unwrap();
    let guidance = top.join(GUIDANCE_DIR).join("review-norms.md");
    std::fs::write(&guidance, "edited\n").unwrap();
    let commit = read_manifest(&top).unwrap().unwrap().commit;
    assert_eq!(
        inspect(&top).unwrap(),
        State::Damaged {
            commit,
            paths: vec![format!("{GUIDANCE_DIR}/review-norms.md")]
        }
    );
    assert_eq!(
        delivered_guidance(&top),
        Delivery::Unverified,
        "edited guidance is never delivered"
    );
    publish(&top, V2);
    assert!(
        sync(&top).is_err(),
        "a dropped-but-edited output is not removed"
    );
    assert_eq!(std::fs::read_to_string(&guidance).unwrap(), "edited\n");
}

#[test]
fn symlinked_tracked_or_reincluded_outputs_refuse() {
    let tmp = tempfile::tempdir().unwrap();

    let linked = product(&tmp.path().join("linked"));
    publish(&linked, V1);
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, linked.join(".aida")).unwrap();
    assert!(format!("{:#}", sync(&linked).unwrap_err()).contains("not a plain directory"));
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);

    let target = tmp.path().join("target.toml");
    let file_link = product(&tmp.path().join("file-link"));
    publish(&file_link, V1);
    std::fs::create_dir_all(file_link.join(".aida")).unwrap();
    std::os::unix::fs::symlink(&target, file_link.join(CONFIG)).unwrap();
    assert!(format!("{:#}", sync(&file_link).unwrap_err()).contains("not a regular file"));
    assert!(!target.exists());

    let reincluded = product(&tmp.path().join("reincluded"));
    std::fs::write(
        reincluded.join(".gitignore"),
        "!/.aida/adoption/\n!/.aida/adoption/**\n",
    )
    .unwrap();
    git_out(&reincluded, &["add", ".gitignore"]);
    git_out(&reincluded, &["commit", "-q", "-m", "reinclude"]);
    publish(&reincluded, V1);
    assert!(format!("{:#}", sync(&reincluded).unwrap_err()).contains("not ignored"));
    assert!(git_out(&reincluded, &["status", "--porcelain"]).is_empty());
}

#[test]
fn a_tracked_product_config_is_never_projected_over() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    std::fs::create_dir_all(top.join(".aida")).unwrap();
    std::fs::write(top.join(CONFIG), "[agents]\nvendor = \"legacy\"\n").unwrap();
    git_out(&top, &["add", "-f", CONFIG]);
    git_out(&top, &["commit", "-q", "-m", "legacy scaffolding"]);
    let before = product_snapshot(&top);
    publish(&top, V1);

    let outcome = sync(&top).unwrap().unwrap();
    assert!(outcome.config_tracked);
    assert_eq!(product_snapshot(&top), before);
    assert!(top.join(GUIDANCE_DIR).join("review-norms.md").is_file());
    assert!(!read_manifest(&top)
        .unwrap()
        .unwrap()
        .files
        .contains_key(CONFIG));
}

#[test]
fn an_interrupted_refresh_completes_instead_of_reading_as_an_edit() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap();
    let second = publish(&top, V2);
    let next = adoption(&top);

    // Simulate a crash after the pending manifest and one file were written.
    let outputs = render(&next, true).unwrap();
    let mut intent = read_manifest(&top).unwrap().unwrap();
    intent.pending = outputs
        .iter()
        .map(|(rel, body)| (rel.clone(), sha256(body.as_bytes())))
        .collect();
    write_manifest(&top, &intent).unwrap();
    std::fs::write(top.join(CONFIG), &outputs[CONFIG]).unwrap();
    assert!(matches!(inspect(&top).unwrap(), State::Damaged { .. }));
    assert_eq!(delivered_guidance(&top), Delivery::Unverified);

    sync(&top).unwrap();
    assert_eq!(inspect(&top).unwrap(), State::Current { commit: second });
}

#[test]
fn concurrent_refreshes_converge_on_one_projection() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    let commit = publish(&top, V1);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let top = top.clone();
            std::thread::spawn(move || sync(&top).unwrap().unwrap())
        })
        .collect();
    let written = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|o| o.written)
        .count();
    assert_eq!(written, 1);
    assert_eq!(inspect(&top).unwrap(), State::Current { commit });
}

#[test]
fn a_clone_without_an_adoption_namespace_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    assert_eq!(sync(&top).unwrap(), None);
    assert_eq!(inspect(&top).unwrap(), State::NotAdopted);
    assert_eq!(delivered_guidance(&top), Delivery::None);
    assert!(!top.join(".aida").exists());
}

#[test]
fn a_manifest_naming_a_path_outside_aida_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap();
    let mut manifest = read_manifest(&top).unwrap().unwrap();
    manifest.files.insert("README.md".into(), "x".into());
    std::fs::write(top.join(MANIFEST), toml::to_string(&manifest).unwrap()).unwrap();
    assert!(format!("{:#}", sync(&top).unwrap_err()).contains("outside .aida"));
    assert_eq!(
        std::fs::read_to_string(top.join("README.md")).unwrap(),
        "product\n"
    );
}
