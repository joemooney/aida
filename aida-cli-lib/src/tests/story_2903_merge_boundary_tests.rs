//! STORY-2903 S1c: one production merge boundary. No production code calls
//! `Forge::merge_change` except `landing_merge::merge`, so a new merge path
//! cannot skip the landing checks.
// trace:STORY-2903 | ai:claude

/// Production sources with a `.merge_change(` call, outside test files and
/// after-`#[cfg(test)]` modules.
fn merge_change_callers() -> Vec<String> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "tests") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Inline test modules sit at the end of a file by convention.
            let production = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in production.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains(".merge_change(") && !code.contains("concat!") {
                    found.push(format!(
                        "{}:{}",
                        path.strip_prefix(&src).unwrap().display(),
                        n + 1
                    ));
                }
            }
        }
    }
    found.sort();
    found
}

#[test]
fn only_the_landing_boundary_calls_merge_change() {
    let callers = merge_change_callers();
    assert_eq!(callers.len(), 1, "{callers:?}");
    assert!(
        callers[0].starts_with("landing_merge.rs:"),
        "every merge path goes through landing_merge::merge: {callers:?}"
    );
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repo whose local origin's `main` now carries `[landing] merger =
/// "advisor"` while the caller started under the defaults.
fn repo_whose_policy_changed() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["branch", "feat"]);
    let origin = root.join("origin.git");
    git(&root, &["clone", "-q", "--bare", "repo", origin.to_str().unwrap()]);
    git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    let other = root.join("other");
    git(&root, &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
    git(&other, &["config", "user.email", "t@example.com"]);
    git(&other, &["config", "user.name", "T"]);
    std::fs::create_dir_all(other.join(".aida")).unwrap();
    std::fs::write(
        other.join(".aida/config.toml"),
        "[landing]\nmerger = \"advisor\"\n",
    )
    .unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "policy"]);
    git(&other, &["push", "-q", "origin", "main"]);
    (temp, repo)
}

fn change() -> crate::forge::ChangeRef {
    crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: "feat".into(),
        base: "main".into(),
        title: None,
    }
}

fn opts() -> crate::forge::MergeOptions {
    crate::forge::MergeOptions {
        method: crate::forge::MergeMethod::Squash,
        squash_subject: None,
        squash_body: None,
        delete_branch: false,
        match_head: None,
    }
}

/// Every non-train caller is refused at the boundary when the policy
/// changed under it, before the forge is asked to merge (the recording
/// forge errors if it is). The refusal is a review refusal (exit 23).
#[test]
fn every_caller_is_refused_at_the_boundary_before_the_forge_merges() {
    use super::{merge, Caller, Entry, MergeError};
    let (_temp, repo) = repo_whose_policy_changed();
    let forge = crate::forge::fake::RecordingForge::new();
    let entry = Entry {
        policy: crate::landing_policy::LandingPolicy::default(),
        floor: false,
    };
    for caller in [
        Caller::Ship,
        Caller::Drain,
        Caller::Burndown,
        Caller::Release,
    ] {
        let got = merge(
            &forge,
            &repo,
            caller,
            &entry,
            &change(),
            &[],
            &opts(),
            &mut crate::network_retry::NoopSink,
        );
        match got {
            Err(MergeError::Refused(kind, why)) => {
                assert_eq!(kind, crate::gate::ship::ShipRefusal::Review, "{caller:?}");
                assert!(why.contains("landing policy changed"), "{caller:?}: {why}");
            }
            other => panic!("{caller:?} must be refused before the forge: {other:?}"),
        }
    }
    // The train is checked per car by S1t; it reaches the forge.
    let got = merge(
        &forge,
        &repo,
        Caller::Train,
        &entry,
        &change(),
        &[],
        &opts(),
        &mut crate::network_retry::NoopSink,
    );
    assert!(matches!(got, Err(MergeError::Forge(_))), "{got:?}");
}
