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

/// The adoption at plane `commit`, rebuilt by moving the store ref back.
fn load_at(top: &Path, commit: &str) -> Adoption {
    let current = git_out(top, &["rev-parse", STORE_REF]);
    git_out(top, &["update-ref", STORE_REF, commit]);
    let adoption = adoption(top);
    git_out(top, &["update-ref", STORE_REF, &current]);
    adoption
}

#[test]
fn a_projection_feeds_readers_without_touching_the_product_tree() {
    // trace:STORY-1741.acd975e1 | ai:claude
    // trace:STORY-1741.ac6b1be3 | ai:claude
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
            config_kept: None
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
            commit: commit.clone(),
            config_kept: None
        }
    );
    let Delivery::Verified {
        commit: rev, docs, ..
    } = delivered_guidance(&top)
    else {
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
    // trace:STORY-1741.acd975e1 | ai:claude
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
            plane: second.clone(),
            config_kept: None
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

    // A guidance file the projection never wrote.
    let guidance = top.join(GUIDANCE_DIR).join("review-norms.md");
    std::fs::create_dir_all(guidance.parent().unwrap()).unwrap();
    std::fs::write(&guidance, "mine\n").unwrap();
    let err = format!("{:#}", sync(&top).unwrap_err());
    assert!(
        err.contains("not written by the adoption projection")
            && err.contains("adoption/settings.toml")
            && err.contains("~/.aida/config"),
        "{err}"
    );
    assert_eq!(std::fs::read_to_string(&guidance).unwrap(), "mine\n");

    std::fs::remove_file(&guidance).unwrap();
    sync(&top).unwrap();
    std::fs::write(&guidance, "edited\n").unwrap();
    let commit = read_manifest(&top).unwrap().unwrap().commit;
    assert_eq!(
        inspect(&top).unwrap(),
        State::Damaged {
            commit,
            paths: vec![format!("{GUIDANCE_DIR}/review-norms.md")]
        }
    );
    assert!(
        matches!(delivered_guidance(&top), Delivery::Withheld { .. }),
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
    assert_eq!(outcome.config_kept, Some(KeptConfig::Tracked));
    assert_eq!(product_snapshot(&top), before);
    assert!(top.join(GUIDANCE_DIR).join("review-norms.md").is_file());
    assert!(!read_manifest(&top)
        .unwrap()
        .unwrap()
        .files
        .contains_key(CONFIG));
    assert!(matches!(
        inspect(&top).unwrap(),
        State::Current {
            config_kept: Some(KeptConfig::Tracked),
            ..
        }
    ));
    assert!(matches!(
        delivered_guidance(&top),
        Delivery::Verified {
            config_kept: Some(KeptConfig::Tracked),
            ..
        }
    ));
}

#[test]
fn a_config_the_product_branch_starts_tracking_is_forgotten_not_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap();
    let projected = std::fs::read(top.join(CONFIG)).unwrap();

    // A checkout that tracks the config, byte-identical to the owned copy so
    // the ownership hash alone would claim it.
    git_out(&top, &["add", "-f", CONFIG]);
    git_out(&top, &["commit", "-q", "-m", "track config"]);
    let before = product_snapshot(&top);
    let outcome = sync(&top).unwrap().unwrap();
    assert_eq!(outcome.config_kept, Some(KeptConfig::Tracked));
    assert_eq!(std::fs::read(top.join(CONFIG)).unwrap(), projected);
    assert_eq!(product_snapshot(&top), before);
    assert!(!read_manifest(&top)
        .unwrap()
        .unwrap()
        .files
        .contains_key(CONFIG));

    // Different tracked bytes: still neither refused nor touched.
    std::fs::write(top.join(CONFIG), "[agents]\nvendor = \"product\"\n").unwrap();
    git_out(&top, &["commit", "-q", "-am", "product config"]);
    let mut manifest = read_manifest(&top).unwrap().unwrap();
    manifest.files.insert(CONFIG.into(), sha256(&projected));
    write_manifest(&top, &manifest).unwrap();
    let before = product_snapshot(&top);
    assert!(matches!(inspect(&top).unwrap(), State::Current { .. }));
    sync(&top).unwrap();
    assert_eq!(product_snapshot(&top), before);
}

#[test]
fn a_stale_projection_is_withheld_when_the_refresh_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    let first = publish(&top, V1);
    sync(&top).unwrap();
    let second = publish(&top, V2);
    // The refresh refuses: the owned config was edited.
    std::fs::write(top.join(CONFIG), "edited = 1\n").unwrap();
    assert!(sync(&top).is_err());
    let Delivery::Withheld { reason } = delivered_guidance(&top) else {
        panic!("revision {first} delivered after a failed refresh");
    };
    assert!(reason.contains(CONFIG), "{reason}");

    // With the config intact again the projection is merely behind: still
    // withheld rather than delivered as the old revision.
    let manifest = read_manifest(&top).unwrap().unwrap();
    let body = render(&load_at(&top, &first), true).unwrap()[CONFIG].clone();
    std::fs::write(top.join(CONFIG), body).unwrap();
    assert_eq!(manifest.commit, first);
    let Delivery::Withheld { reason } = delivered_guidance(&top) else {
        panic!("stale revision delivered");
    };
    assert!(reason.contains(&second), "{reason}");
}

#[test]
fn an_edited_projected_config_withholds_guidance_like_the_preview_says() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap();
    std::fs::write(top.join(CONFIG), "edited = 1\n").unwrap();
    assert!(matches!(inspect(&top).unwrap(), State::Damaged { .. }));
    assert!(matches!(
        delivered_guidance(&top),
        Delivery::Withheld { .. }
    ));
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
    assert!(matches!(
        delivered_guidance(&top),
        Delivery::Withheld { .. }
    ));

    sync(&top).unwrap();
    assert_eq!(
        inspect(&top).unwrap(),
        State::Current {
            commit: second,
            config_kept: None
        }
    );
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
    assert_eq!(
        inspect(&top).unwrap(),
        State::Current {
            commit,
            config_kept: None
        }
    );
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

#[test]
fn a_config_the_product_branch_stops_tracking_reads_as_stale_until_refreshed() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    std::fs::create_dir_all(top.join(".aida")).unwrap();
    std::fs::write(top.join(CONFIG), "[agents]\nvendor = \"legacy\"\n").unwrap();
    git_out(&top, &["add", "-f", CONFIG]);
    git_out(&top, &["commit", "-q", "-m", "legacy scaffolding"]);
    let commit = publish(&top, V1);
    assert_eq!(
        sync(&top).unwrap().unwrap().config_kept,
        Some(KeptConfig::Tracked)
    );

    // A checkout that drops the config: git deletes the file, and the
    // manifest has no settings entry, yet the plane is unchanged.
    git_out(&top, &["rm", "-q", CONFIG]);
    git_out(&top, &["commit", "-q", "-m", "drop scaffolding"]);
    assert!(!top.join(CONFIG).exists());
    assert!(matches!(
        inspect(&top).unwrap(),
        State::Stale {
            config_kept: None,
            ..
        }
    ));
    assert!(matches!(
        delivered_guidance(&top),
        Delivery::Withheld { .. }
    ));

    sync(&top).unwrap();
    assert_eq!(
        inspect(&top).unwrap(),
        State::Current {
            commit,
            config_kept: None
        }
    );
    assert!(top.join(CONFIG).is_file());
}

#[test]
fn a_second_interrupted_refresh_keeps_every_hash_a_file_still_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap();

    // Refresh to V2 interrupted after the config was written.
    publish(&top, V2);
    let outputs = render(&adoption(&top), true).unwrap();
    let mut intent = read_manifest(&top).unwrap().unwrap();
    intent.pending = outputs
        .iter()
        .map(|(rel, body)| (rel.clone(), sha256(body.as_bytes())))
        .collect();
    write_manifest(&top, &intent).unwrap();
    std::fs::write(top.join(CONFIG), &outputs[CONFIG]).unwrap();

    // The plane moves on, and the next refresh fails on the guidance write
    // (written first) before it reaches the config.
    let third = publish(
        &top,
        "schema = 1\n[settings.agents]\nvendor = \"antigravity\"\n[guidance]\nthird = \"Third.\\n\"\n",
    );
    let guidance = top.join(GUIDANCE_DIR);
    let mode =
        |m| std::fs::set_permissions(&guidance, std::os::unix::fs::PermissionsExt::from_mode(m));
    mode(0o555).unwrap();
    let probe = guidance.join(".probe");
    if std::fs::write(&probe, "x").is_ok() {
        // Running as root: permissions cannot force the failure.
        std::fs::remove_file(&probe).unwrap();
        mode(0o755).unwrap();
        return;
    }
    assert!(sync(&top).is_err());
    mode(0o755).unwrap();
    assert_eq!(
        std::fs::read_to_string(top.join(CONFIG)).unwrap(),
        outputs[CONFIG]
    );

    sync(&top).unwrap();
    assert_eq!(
        inspect(&top).unwrap(),
        State::Current {
            commit: third,
            config_kept: None
        }
    );
}

#[test]
fn a_symlinked_lock_refuses_without_creating_its_target() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    std::fs::create_dir_all(top.join(".aida")).unwrap();
    let target = tmp.path().join("outside.lock");
    std::os::unix::fs::symlink(&target, top.join(LOCK)).unwrap();
    assert!(format!("{:#}", sync(&top).unwrap_err()).contains(LOCK));
    assert!(!target.exists());
}

/// A local untracked config the projection never wrote (an `aida config
/// set` before the first launch) is kept and reported, guidance still
/// flows, and deleting it lets the plane's settings in.
#[test]
fn a_local_unowned_config_is_kept_and_guidance_still_delivered() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    std::fs::create_dir_all(top.join(".aida")).unwrap();
    std::fs::write(top.join(CONFIG), "mine = 1\n").unwrap();

    let outcome = sync(&top).unwrap().unwrap();
    assert_eq!(outcome.config_kept, Some(KeptConfig::Local));
    assert_eq!(
        std::fs::read_to_string(top.join(CONFIG)).unwrap(),
        "mine = 1\n"
    );
    assert!(matches!(
        inspect(&top).unwrap(),
        State::Current {
            config_kept: Some(KeptConfig::Local),
            ..
        }
    ));
    assert!(matches!(
        delivered_guidance(&top),
        Delivery::Verified {
            config_kept: Some(KeptConfig::Local),
            ..
        }
    ));
    let note = KeptConfig::Local.note();
    assert!(
        note.contains("adoption/settings.toml") && note.contains("~/.aida/config"),
        "{note}"
    );
    assert!(
        !sync(&top).unwrap().unwrap().written,
        "a kept config is stable"
    );

    std::fs::remove_file(top.join(CONFIG)).unwrap();
    assert!(matches!(inspect(&top).unwrap(), State::Stale { .. }));
    assert_eq!(sync(&top).unwrap().unwrap().config_kept, None);
    assert!(owns(&top.join(CONFIG)));
}

/// The ownership guard fails closed: a manifest that exists but cannot be
/// read refuses the write rather than reading as "not owned".
#[test]
fn an_unreadable_manifest_refuses_every_config_write() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap();
    let before = std::fs::read(top.join(CONFIG)).unwrap();
    for broken in [
        "not toml = = =",
        "version = 9\ncommit = \"x\"\nschema = 1\ngenerator = \"g\"\n",
    ] {
        std::fs::write(top.join(MANIFEST), broken).unwrap();
        assert!(owns(&top.join(CONFIG)), "{broken}");
        let err = crate::write_config_atomic(&top.join(CONFIG), "x = 1\n").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
        assert_eq!(std::fs::read(top.join(CONFIG)).unwrap(), before);
    }
    // Without any manifest the writer is a plain atomic write.
    std::fs::remove_file(top.join(MANIFEST)).unwrap();
    crate::write_config_atomic(&top.join(CONFIG), "x = 1\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(top.join(CONFIG)).unwrap(),
        "x = 1\n"
    );
}

/// An adopted clone counts as adopted before its first projection, so the
/// scaffold guard refuses there too; a clone without a namespace does not.
#[test]
fn adoption_is_detected_before_the_first_projection() {
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    assert!(!is_adopted(&top));
    publish(&top, V1);
    assert!(!is_projected(&top));
    assert!(is_adopted(&top));
    std::fs::create_dir_all(top.join("src")).unwrap();
    assert!(is_adopted(&top.join("src")));
    assert!(!top.join(".aida").exists(), "detection writes nothing");
}

/// A plane that cannot be discovered fails closed: the state is unknown,
/// never "not adopted", and the scaffold guard refuses naming the error.
#[test]
fn an_undiscoverable_plane_counts_as_adopted() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let unknown = |top: &Path, needle: &str| {
        let AdoptionState::Unknown(error) = adoption_state(top) else {
            panic!("{needle}: {:?}", adoption_state(top));
        };
        assert!(error.contains(needle), "{error}");
        assert!(is_adopted(top));
        let refusal = crate::scaffolding::refuse_in_adopted_worktree(top)
            .unwrap_err()
            .to_string();
        assert!(refusal.contains(needle), "{refusal}");
        assert!(git_out(top, &["status", "--porcelain"]).is_empty());
    };

    // A locator others can read, and one naming a ref that does not resolve.
    let top = product(&tmp.path().join("locator"));
    let path = crate::allocation_authority::common_dir(&top)
        .unwrap()
        .join(crate::adoption_plane::LOCATOR_FILE);
    std::fs::write(&path, "version = 1\nref = \"refs/heads/plane\"\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    unknown(&top, "chmod 600");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    unknown(&top, "does not resolve");

    // Remotes that disagree and no local aida-store branch.
    let top = product(&tmp.path().join("remotes"));
    let first = publish(&top, V1);
    let second = publish(&top, "schema = 1\n");
    git_out(&top, &["update-ref", "refs/remotes/a/aida-store", &first]);
    git_out(&top, &["update-ref", "refs/remotes/b/aida-store", &second]);
    git_out(&top, &["update-ref", "-d", STORE_REF]);
    unknown(&top, "Remotes disagree");
    assert!(!top.join(".aida").exists(), "detection writes nothing");
}

/// Racing creators of the projection directories both succeed: losing the
/// `create_dir` race re-checks instead of failing.
#[test]
fn ensure_real_dir_tolerates_a_concurrent_creator() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            let root = root.to_path_buf();
            std::thread::spawn(move || {
                barrier.wait();
                ensure_real_dir(&root, ".aida")
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    assert!(root.join(".aida").is_dir());
}

/// Fail closed on adoption evidence, not on any discovery refusal: a
/// non-orphan store or disagreeing remotes with no `adoption/` namespace
/// anywhere is a plain clone, while the same refusal with a namespace on a
/// candidate stays unknown.
#[test]
fn a_refusal_without_adoption_evidence_is_not_adopted() {
    let tmp = tempfile::tempdir().unwrap();
    // `git branch aida-store`: a store grown out of product history.
    let top = product(&tmp.path().join("branch"));
    git_out(&top, &["branch", "aida-store"]);
    assert!(crate::adoption_plane::discover(&top).is_err());
    assert_eq!(adoption_state(&top), AdoptionState::NotAdopted);
    crate::scaffolding::refuse_in_adopted_worktree(&top).unwrap();

    // Disagreeing remotes, neither carrying a namespace.
    let top = product(&tmp.path().join("remotes"));
    let tree = git_in(&top, &["mktree"], "");
    let a = git_out(&top, &["commit-tree", &tree, "-m", "a"]);
    let b = git_out(&top, &["commit-tree", &tree, "-m", "b"]);
    git_out(&top, &["update-ref", "refs/remotes/a/aida-store", &a]);
    git_out(&top, &["update-ref", "refs/remotes/b/aida-store", &b]);
    assert!(crate::adoption_plane::discover(&top).is_err());
    assert_eq!(adoption_state(&top), AdoptionState::NotAdopted);

    // A non-orphan store that does carry a namespace stays unknown.
    let top = product(&tmp.path().join("evidence"));
    let blob = git_in(&top, &["hash-object", "-w", "--stdin"], V1);
    git_out(
        &top,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{blob},adoption/settings.toml"),
        ],
    );
    let tree = git_out(&top, &["write-tree"]);
    git_out(&top, &["read-tree", "HEAD"]);
    let commit = git_out(&top, &["commit-tree", &tree, "-p", "HEAD", "-m", "plane"]);
    git_out(&top, &["update-ref", STORE_REF, &commit]);
    let AdoptionState::Unknown(error) = adoption_state(&top) else {
        panic!("{:?}", adoption_state(&top));
    };
    assert!(error.contains("shares history"), "{error}");
    assert!(crate::scaffolding::refuse_in_adopted_worktree(&top).is_err());
}

#[test]
fn uninstall_removes_owned_outputs_keeps_edits_and_leaves_the_product_alone() {
    // trace:STORY-1741.ac7e5256 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    std::fs::write(top.join("README.md"), "pre-dirty\n").unwrap();
    publish(&top, V1);
    sync(&top).unwrap().unwrap();
    std::fs::write(top.join(".aida/notes.txt"), "mine\n").unwrap();
    let edited = format!("{GUIDANCE_DIR}/review-norms.md");
    std::fs::write(top.join(&edited), "my edit\n").unwrap();
    let before = product_snapshot(&top);
    let store_ref = git_out(&top, &["rev-parse", STORE_REF]);

    let planned = uninstall(&top, true).unwrap();
    assert!(top.join(MANIFEST).exists(), "a dry run removes nothing");
    let removal = uninstall(&top, false).unwrap();
    assert_eq!(removal, planned);
    assert_eq!(removal.removed, [CONFIG, MANIFEST, LOCK]);
    assert_eq!(removal.kept.len(), 1, "{removal:?}");
    assert_eq!(removal.kept[0].0, edited);

    assert!(!top.join(CONFIG).exists() && !top.join(MANIFEST).exists());
    assert!(!top.join(LOCK).exists());
    assert_eq!(
        std::fs::read_to_string(top.join(&edited)).unwrap(),
        "my edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(top.join(".aida/notes.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(product_snapshot(&top), before);
    assert_eq!(git_out(&top, &["rev-parse", STORE_REF]), store_ref);
    assert!(!is_projected(&top));
    // Idempotent: nothing left to remove.
    assert_eq!(uninstall(&top, false).unwrap(), Removal::default());
}

#[test]
fn uninstall_after_an_interrupted_refresh_removes_pending_outputs() {
    // trace:STORY-1741.ac7e5256 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    publish(&top, V1);
    sync(&top).unwrap().unwrap();
    // Simulate a refresh to V2 that stopped after writing one file.
    let next = load_at(&top, &publish(&top, V2));
    let mut manifest = read_manifest(&top).unwrap().unwrap();
    let outputs = render(&next, true).unwrap();
    manifest.pending = outputs
        .iter()
        .map(|(rel, body)| (rel.clone(), sha256(body.as_bytes())))
        .collect();
    write_manifest(&top, &manifest).unwrap();
    std::fs::write(top.join(CONFIG), &outputs[CONFIG]).unwrap();

    let removal = uninstall(&top, false).unwrap();
    assert!(removal.kept.is_empty(), "{removal:?}");
    assert!(!top.join(CONFIG).exists());
    assert!(!top.join(".aida/adoption").exists());
}

#[test]
fn bootstrap_projects_only_the_selected_revision() {
    // trace:STORY-1741.ac591cd7 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    let old = publish(&top, V1);
    let current = publish(&top, V2);
    let before = product_snapshot(&top);

    let err = bootstrap(&top, &old).unwrap_err().to_string();
    assert!(err.contains("not the selected revision"), "{err}");
    assert!(!top.join(MANIFEST).exists(), "a mismatch writes nothing");
    let err = bootstrap(&top, &current[..12]).unwrap_err().to_string();
    assert!(err.contains("full lowercase commit id"), "{err}");

    let outcome = bootstrap(&top, &current).unwrap();
    assert_eq!(outcome.commit, current);
    assert_eq!(
        inspect(&top).unwrap(),
        State::Current {
            commit: current.clone(),
            config_kept: None
        }
    );
    assert_eq!(product_snapshot(&top), before);
    // Repeating it is a verified no-op.
    assert!(!bootstrap(&top, &current).unwrap().written);
}

#[test]
fn bootstrap_refuses_when_a_local_config_keeps_the_settings_out() {
    // trace:STORY-1741.ac591cd7 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    let commit = publish(&top, V1);
    std::fs::create_dir_all(top.join(".aida")).unwrap();
    std::fs::write(top.join(CONFIG), "[agents]\nvendor = \"codex\"\n").unwrap();
    let err = bootstrap(&top, &commit).unwrap_err().to_string();
    assert!(err.contains("not applied"), "{err}");
}

#[test]
fn bootstrap_refuses_a_clone_with_no_plane() {
    // trace:STORY-1741.ac591cd7 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = product(&tmp.path().join("p"));
    let err = bootstrap(&top, &"a".repeat(40)).unwrap_err().to_string();
    assert!(err.contains("No adoption plane"), "{err}");
}
