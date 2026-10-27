//! BUG-2221: `aida show` runs the filing-drift walk concurrently with the
//! shipped check and the branch search. These pin that the concurrent
//! collector answers exactly what the serial collect-then-drift did, on a
//! large trace footprint, and that the one show deadline still bounds it.
// trace:BUG-2221 | ai:claude
use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo filed at its first commit, then ~200 files tracing STORY-9 and a
/// trail of referencing commits on `main` and on an unmerged `story-9-work`
/// branch, so the drift walk, the shipped check and the branch search all
/// have real work. Returns the repo and the filing sha.
fn large_footprint_repo(tmp: &Path) -> (PathBuf, String) {
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main", "--quiet"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "init\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "chore: init"]);
    let filed = git(&repo, &["rev-parse", "HEAD"]);
    for n in 0..200 {
        let path = repo.join(format!("src/m{}/f{n}.rs", n % 10));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("// trace:STORY-9 | ai:claude\nfn f{n}() {{}}\n"),
        )
        .unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "feat: trace (STORY-9)"]);
    for n in 0..5 {
        std::fs::write(
            repo.join(format!("src/m{n}/f{n}.rs")),
            format!("// trace:STORY-9\n// v{n}\n"),
        )
        .unwrap();
        git(
            &repo,
            &["commit", "-qam", &format!("fix: touch {n} (STORY-9)")],
        );
    }
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    git(&repo, &["checkout", "-q", "-b", "story-9-work"]);
    for n in 0..3 {
        std::fs::write(
            repo.join(format!("src/m9/f{}.rs", 9 + 10 * n)),
            "// trace:STORY-9\n",
        )
        .unwrap();
        git(&repo, &["commit", "-qam", &format!("wip {n} (STORY-9)")]);
    }
    (repo, filed)
}

fn provenance(sha: &str) -> aida_core::FilingProvenance {
    aida_core::FilingProvenance {
        code_sha: Some(sha.to_string()),
        ..Default::default()
    }
}

#[test]
fn concurrent_drift_equals_serial_on_large_footprint() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (repo, filed) = large_footprint_repo(tmp.path());
    let ids = vec!["STORY-9".to_string()];
    let p = provenance(&filed);

    let deadline = Instant::now() + SHOW_LINKAGE_DEADLINE;
    let serial = collect_git_linkage_until(&repo, &ids, deadline);
    let serial_drift =
        filing_drift_hint_until(&repo, Some(&p), &serial.files, &serial.commits, deadline);

    let (concurrent, concurrent_drift) = collect_git_linkage_with_drift_until(
        &repo,
        &ids,
        Some(&p),
        Instant::now() + SHOW_LINKAGE_DEADLINE,
    );

    assert_eq!(serial.files.len(), 200, "{serial:?}");
    assert_eq!(serial.commits.len(), 9, "{serial:?}");
    assert_eq!(serial.branch.as_deref(), Some("story-9-work"), "{serial:?}");
    assert_eq!(format!("{serial:?}"), format!("{concurrent:?}"));
    assert_eq!(
        serial_drift.as_deref(),
        Some("9 commits since filing touched 200 traced files")
    );
    assert_eq!(serial_drift, concurrent_drift);

    // No provenance: no drift line, exactly as the serial hint answered.
    let (_, none) = collect_git_linkage_with_drift_until(
        &repo,
        &ids,
        None,
        Instant::now() + SHOW_LINKAGE_DEADLINE,
    );
    assert_eq!(none, None);
}

/// A drift walk slower than its budget still ends at the one show deadline
/// and reads "unknown", never a guessed zero.
#[cfg(unix)]
#[test]
fn show_deadline_still_bounds_the_concurrent_drift_walk() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let (repo, filed) = large_footprint_repo(tmp.path());
    let ids = vec!["STORY-9".to_string()];
    let p = provenance(&filed);
    let dir = tmp.path().join("shim");
    std::fs::create_dir_all(&dir).unwrap();
    // Only the drift walk's `<code_sha>..HEAD` log stalls.
    crate::test_exec::write_executable(
        &dir.join("git"),
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  case \"$arg\" in\n    {filed}..HEAD) sleep 5; break ;;\n  esac\ndone\nexec '{}' \"$@\"\n",
            real_git().display()
        ),
    );
    let mut paths = vec![dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let _path = crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap());

    let started = Instant::now();
    let (linkage, drift) = collect_git_linkage_with_drift_until(
        &repo,
        &ids,
        Some(&p),
        Instant::now() + Duration::from_millis(1500),
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(2500),
        "the drift walk must stop at its bound, not wait out the stalled git: {elapsed:?}"
    );
    assert_eq!(linkage.commits.len(), 9, "{linkage:?}");
    assert_eq!(
        drift.as_deref(),
        Some("commits since filing: unknown (scan didn't finish in budget)")
    );
}

/// The real `git` binary on PATH, before the shim is prepended.
#[cfg(unix)]
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("git"))
        .find(|c| c.is_file())
        .expect("git on PATH")
}
