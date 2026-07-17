use super::*;
use std::fs;
use tempfile::TempDir;

/// `.aida/config.toml` only at the repo root → `aida edit` from a
/// nested subdir must still resolve the store.
// trace:BUG-57 | ai:claude
#[test]
fn detect_distributed_store_walks_up_from_subdir() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".aida")).unwrap();
    fs::write(
        root.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".aida-store")).unwrap();

    let nested = root.join("aida-cli/src/foo");
    fs::create_dir_all(&nested).unwrap();

    let resolved = detect_distributed_store_from(&nested).expect("should walk up");
    assert_eq!(resolved, root.join(".aida-store"));
}

/// BUG-428: distributed config present but NO `.aida-store/` worktree
/// (the fresh-clone state) → `detect_distributed_store_from` returns None
/// (no worktree) while `distributed_mode_declared_from` returns Some — the
/// pair the dispatcher uses to bail with "run aida init" instead of
/// silently reading the legacy requirements.yaml.
#[test]
fn distributed_declared_but_worktree_missing_is_detectable() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".aida")).unwrap();
    fs::write(
        root.join(".aida/config.toml"),
        "[deployment]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    // deliberately do NOT create .aida-store/ (fresh clone)
    assert!(
        detect_distributed_store_from(root).is_none(),
        "no worktree → store unresolvable"
    );
    assert_eq!(
        distributed_mode_declared_from(root),
        Some(root.to_path_buf()),
        "distributed mode is still declared in config"
    );
}

/// A legacy project — `.aida/config.toml` without distributed mode (or no
/// config at all) → `distributed_mode_declared_from` is None, so the
/// dispatcher keeps the legacy YAML/SQLite fallback (no false bail).
#[test]
fn legacy_config_does_not_declare_distributed() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // no .aida/config.toml at all
    assert_eq!(distributed_mode_declared_from(root), None);
    // a config.toml that does NOT set distributed mode
    fs::create_dir_all(root.join(".aida")).unwrap();
    fs::write(
        root.join(".aida/config.toml"),
        "[hints]\nworkflow_hints = true\n",
    )
    .unwrap();
    assert_eq!(distributed_mode_declared_from(root), None);
}

/// store_path is interpreted relative to the directory containing
/// config.toml, not relative to the starting cwd.
// trace:BUG-57 | ai:claude
#[test]
fn detect_distributed_store_resolves_relative_to_config_dir() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".aida")).unwrap();
    fs::write(
        root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".aida-store")).unwrap();

    let nested = root.join("a/b/c");
    fs::create_dir_all(&nested).unwrap();

    // If we incorrectly resolved relative to nested, we'd look for
    // `<nested>/.aida-store/` which doesn't exist.
    let resolved = detect_distributed_store_from(&nested).unwrap();
    assert_eq!(resolved, root.join(".aida-store"));
}

/// FR-267: in an `aida init --sibling` workspace the code repo's
/// `.aida/config.toml` points `store_path` OUTSIDE the repo (e.g.
/// `../aida-store`). Store resolution must follow that pointer to the
/// SIBLING directory — this is the same resolution that now backs
/// `aida trace scan` / `aida trace list` (they route through the
/// resolved `store_path` in `handle_git_backend_command`). Resolving
/// from a nested subdir of the code repo must still land on the sibling.
// trace:FR-267 | ai:claude
#[test]
fn detect_distributed_store_resolves_sibling_store_outside_repo() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    // workspace/
    //   code-a/.aida/config.toml  (store_path = "../aida-store")
    //   aida-store/               (the shared sibling store)
    let code_a = ws.join("code-a");
    fs::create_dir_all(code_a.join(".aida")).unwrap();
    fs::write(
            code_a.join(".aida/config.toml"),
            "[deployment]\nmode = \"distributed\"\nstore_path = \"../aida-store\"\nstore_type = \"sibling\"\n",
        )
        .unwrap();
    fs::create_dir_all(ws.join("aida-store")).unwrap();

    // Invoked from a nested source dir inside the code repo.
    let nested = code_a.join("src/foo");
    fs::create_dir_all(&nested).unwrap();

    let resolved = detect_distributed_store_from(&nested).expect("sibling store must resolve");
    // Compare canonicalized paths so the `../` segment is normalized.
    assert_eq!(
        resolved.canonicalize().unwrap(),
        ws.join("aida-store").canonicalize().unwrap(),
        "store must resolve to the SIBLING aida-store, not inside code-a"
    );
}

/// FR-267: the resolution-path selector that backs trace commands must
/// also declare distributed mode for a sibling-store config — so a
/// sibling workspace never falls through to the legacy YAML/SQLite
// fallback. trace:FR-267 | ai:claude
#[test]
fn sibling_store_config_declares_distributed_mode() {
    let tmp = TempDir::new().unwrap();
    let code_a = tmp.path().join("code-a");
    fs::create_dir_all(code_a.join(".aida")).unwrap();
    fs::write(
            code_a.join(".aida/config.toml"),
            "[deployment]\nmode = \"distributed\"\nstore_path = \"../aida-store\"\nstore_type = \"sibling\"\n",
        )
        .unwrap();
    assert_eq!(
        distributed_mode_declared_from(&code_a),
        Some(code_a.clone()),
        "sibling config must declare distributed mode"
    );
}

/// BUG-568: single-repo store (no `.aida-workspace`) → detector returns
/// None → no multi-repo warning fires (ZERO behavior change for the common
// case). trace:BUG-568 | ai:claude
#[test]
fn detect_multi_repo_returns_none_for_single_repo_store() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("solo");
    std::fs::create_dir_all(&repo).unwrap();
    // No `.aida-workspace` manifest anywhere up the tree.
    assert!(
        detect_multi_repo_shared_store(&repo).is_none(),
        "single-repo store must not trigger the multi-repo warning"
    );
}

/// BUG-568: a `.aida-workspace` manifest listing ≥2 repos → detector returns
/// Some(other_repos) → the loud cross-repo-miss warning fires. The reported
// list excludes the repo we're standing in. trace:BUG-568 | ai:claude
#[test]
fn detect_multi_repo_returns_others_for_workspace_with_two_repos() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    let repo_a = ws.join("repo-a");
    let repo_b = ws.join("repo-b");
    std::fs::create_dir_all(&repo_a).unwrap();
    std::fs::create_dir_all(&repo_b).unwrap();

    let mut manifest = aida_core::workspace::WorkspaceManifest {
        name: "ws".into(),
        ..Default::default()
    };
    manifest.add_repo("repo-a", "RepoA");
    manifest.add_repo("repo-b", "RepoB");
    manifest.save(ws).unwrap();

    // Standing in repo-a: detector fires and names the OTHER repo (RepoB).
    let others = detect_multi_repo_shared_store(&repo_a)
        .expect("workspace with 2 repos must trigger the warning");
    assert_eq!(
        others,
        vec!["RepoB".to_string()],
        "warning should name the sibling repo, not the one we're inside"
    );

    // A single-repo manifest does NOT trigger it (boundary at <2 repos).
    let tmp2 = TempDir::new().unwrap();
    let ws2 = tmp2.path();
    let solo = ws2.join("only");
    std::fs::create_dir_all(&solo).unwrap();
    let mut one = aida_core::workspace::WorkspaceManifest {
        name: "one".into(),
        ..Default::default()
    };
    one.add_repo("only", "Only");
    one.save(ws2).unwrap();
    assert!(
        detect_multi_repo_shared_store(&solo).is_none(),
        "a workspace with a single repo is not multi-repo"
    );
}

/// No `.aida/config.toml` anywhere up the tree → returns None (caller
/// falls through to legacy / registry resolution).
// trace:BUG-57 | ai:claude
#[test]
fn detect_distributed_store_returns_none_when_absent() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(detect_distributed_store_from(&nested).is_none());
}

/// SPIKE-48: a directory that exists and holds `objects/` is accepted as a
/// sandbox store override; the returned path resolves to it.
// trace:SPIKE-48 | ai:claude
#[test]
fn aida_store_override_accepts_dir_with_objects() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("sandbox");
    std::fs::create_dir_all(store.join("objects")).unwrap();
    let resolved = aida_store_override_from(&store).expect("should accept store");
    // Canonicalized, so compare against the canonical form of the input.
    assert_eq!(resolved, store.canonicalize().unwrap());
}

/// SPIKE-48: validation is strict-but-quiet — a missing path, a regular
/// file, or a dir without `objects/` all fall through (return None) rather
/// than erroring, so a stale/typo'd export never silently misdirects writes.
// trace:SPIKE-48 | ai:claude
#[test]
fn aida_store_override_rejects_non_store_paths() {
    let tmp = TempDir::new().unwrap();

    // Missing path.
    assert!(aida_store_override_from(&tmp.path().join("nope")).is_none());

    // Exists, but no objects/ subdir.
    let bare = tmp.path().join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(aida_store_override_from(&bare).is_none());

    // A regular file, not a directory.
    let file = tmp.path().join("afile");
    std::fs::write(&file, "x").unwrap();
    assert!(aida_store_override_from(&file).is_none());

    // objects/ present but as a FILE, not a dir → still rejected.
    let weird = tmp.path().join("weird");
    std::fs::create_dir_all(&weird).unwrap();
    std::fs::write(weird.join("objects"), "not a dir").unwrap();
    assert!(aida_store_override_from(&weird).is_none());
}

/// BUG-567 Finding 1: a set-but-unusable AIDA_STORE classifies as
/// `Unusable` with a SPECIFIC reason (so the env wrapper can name WHY it
/// fell through), and a valid store classifies as `Usable`. The wrapper
/// stays a pure fall-through — it never errors — preserving the SPIKE-48
/// never-break-a-forgotten-export intent; this only makes the reason
// surfaceable. trace:BUG-567 | ai:claude
#[test]
fn aida_store_override_reports_reason_for_unusable() {
    let tmp = TempDir::new().unwrap();

    // Not a directory (also covers a dropped/typo'd path).
    match aida_store_override_from(&tmp.path().join("nope")) {
        StoreOverride::Unusable { reason } => assert!(reason.contains("not a directory")),
        StoreOverride::Usable(_) => panic!("a missing path must be Unusable"),
    }

    // Exists but lacks objects/ → distinct, namable reason.
    let bare = tmp.path().join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    match aida_store_override_from(&bare) {
        StoreOverride::Unusable { reason } => assert!(reason.contains("objects/")),
        StoreOverride::Usable(_) => panic!("a dir without objects/ must be Unusable"),
    }

    // A valid store → Usable.
    let good = tmp.path().join("good");
    std::fs::create_dir_all(good.join("objects")).unwrap();
    assert!(aida_store_override_from(&good).is_some());
}

/// BUG-567: the notice is suppressible. `aida_quiet()` is true only for a
/// real opt-in value, false for unset / "0" / "false" / empty, so scripts
// can mute the informational store fall-through. trace:BUG-567 | ai:claude
#[test]
fn aida_quiet_honors_only_real_optin_values() {
    // This mutates process env, so keep it self-contained and restore.
    let prev = std::env::var("AIDA_QUIET").ok();
    let restore = |prev: &Option<String>| match prev {
        Some(v) => std::env::set_var("AIDA_QUIET", v),
        None => std::env::remove_var("AIDA_QUIET"),
    };

    std::env::remove_var("AIDA_QUIET");
    assert!(!aida_quiet(), "unset → not quiet");

    for off in ["", "0", "false", "FALSE"] {
        std::env::set_var("AIDA_QUIET", off);
        assert!(!aida_quiet(), "{off:?} → not quiet");
    }
    for on in ["1", "true", "yes", "anything"] {
        std::env::set_var("AIDA_QUIET", on);
        assert!(aida_quiet(), "{on:?} → quiet");
    }

    restore(&prev);
}

/// BUG-331: from a sibling git worktree, detection resolves the canonical
/// store at the MAIN worktree (via git-common-dir) instead of failing and
/// falling back to centralized mode. The sibling has the tracked
/// `.aida/config.toml` but no local `.aida-store/`.
// trace:BUG-331 | ai:claude
#[test]
fn detect_distributed_store_resolves_from_sibling_worktree() {
    fn git(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git on PATH");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let tmp = TempDir::new().unwrap();
    let main_wt = tmp.path().join("main");
    std::fs::create_dir_all(&main_wt).unwrap();
    git(&main_wt, &["init", "--initial-branch=main", "--quiet"]);
    git(&main_wt, &["config", "user.email", "t@example.com"]);
    git(&main_wt, &["config", "user.name", "T"]);

    // Tracked config.toml (a sibling worktree inherits it from the branch).
    std::fs::create_dir_all(main_wt.join(".aida")).unwrap();
    std::fs::write(
        main_wt.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    git(&main_wt, &["add", ".aida/config.toml"]);
    git(&main_wt, &["commit", "-m", "chore: aida config", "--quiet"]);

    // The orphan-branch worktree lives ONLY in the main worktree.
    std::fs::create_dir_all(main_wt.join(".aida-store")).unwrap();

    // Add a sibling worktree — it has config.toml, but no `.aida-store/`.
    let sibling = tmp.path().join("sibling");
    git(
        &main_wt,
        &[
            "worktree",
            "add",
            "--quiet",
            sibling.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    assert!(sibling.join(".aida/config.toml").exists());
    assert!(!sibling.join(".aida-store").exists());

    let resolved =
        detect_distributed_store_from(&sibling).expect("should resolve via main worktree");
    // Canonicalize both sides — worktree paths can differ by symlinks
    // (e.g. /tmp vs /private/tmp) before normalization.
    assert_eq!(
        resolved.canonicalize().unwrap(),
        main_wt.join(".aida-store").canonicalize().unwrap(),
        "sibling worktree must resolve the main worktree's canonical store"
    );

    // Cleanup the registered worktree so the tempdir drops cleanly.
    git(
        &main_wt,
        &["worktree", "remove", "--force", sibling.to_str().unwrap()],
    );
}
