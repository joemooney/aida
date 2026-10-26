//! BUG-2221 SKETCH v3: hermetic regressions for the incremental spec-id ->
//! commit index behind `aida show`'s positive linkage scan. Real temporary
//! git repos; every answer is compared with the ORIGINAL collector
//! (`collect_git_linkage_opts`, which never consults the index).
// trace:BUG-2221 | ai:claude
use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Fixed committer/author second, so same-second ties are deliberate.
const T: i64 = 1_700_000_000;

fn git_at(repo: &Path, when: Option<i64>, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .arg("-C")
        .arg(repo)
        .args(args);
    if let Some(t) = when {
        let date = format!("@{t} +0000");
        cmd.env("GIT_COMMITTER_DATE", &date)
            .env("GIT_AUTHOR_DATE", &date);
    }
    let out = cmd.output().expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(repo: &Path, args: &[&str]) -> String {
    git_at(repo, None, args)
}

fn init_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main", "--quiet"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    commit_at(&repo, T - 100, "base: scaffolding");
    repo
}

fn commit_at(repo: &Path, when: i64, msg: &str) -> String {
    git_at(
        repo,
        Some(when),
        &["commit", "-q", "--allow-empty", "-m", msg],
    );
    git(repo, &["rev-parse", "HEAD"])
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(600)
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn index(repo: &Path, wanted: &[&str]) -> IndexAnswer {
    query_inner(repo, &ids(wanted), far(), far())
}

fn answered(repo: &Path, wanted: &[&str]) -> Vec<(String, String, String)> {
    match index(repo, wanted) {
        IndexAnswer::Answered(c) => c,
        other => panic!("expected an index answer, got {other:?}"),
    }
}

fn oracle(repo: &Path, wanted: &[&str]) -> Vec<(String, String, String)> {
    crate::collect_git_linkage_opts(repo, &ids(wanted), false).commits
}

fn sorted(mut v: Vec<(String, String, String)>) -> Vec<(String, String, String)> {
    v.sort();
    v
}

fn ns(repo: &Path) -> PathBuf {
    let common = git(repo, &["rev-parse", "--git-common-dir"]);
    let common = repo.join(common);
    std::fs::canonicalize(common).unwrap().join(INDEX_NAMESPACE)
}

fn generation_dir(repo: &Path) -> PathBuf {
    let meta = read_meta(&ns(repo)).ok().flatten().expect("index built");
    ns(repo).join(meta.generation)
}

fn frontier_on_disk(repo: &Path) -> Frontier {
    read_frontier(&generation_dir(repo)).expect("frontier parses")
}

fn shard_file(repo: &Path, id: &str) -> PathBuf {
    generation_dir(repo)
        .join("shards")
        .join(shard_name(shard_of(id)))
}

/// The index answers the original query's commit set; a single batch (cold
/// build) keeps Git's `--date-order` exactly, and the O2 tie-break puts a
/// later batch's same-second commit ahead of earlier batches' — never a
/// parent ahead of its same-second child.
#[test]
fn index_matches_original_including_same_second_ties() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    let a = commit_at(&repo, T, "fix: first (TASK-7)");
    let b = commit_at(&repo, T, "feat: child, same second (TASK-7)");
    commit_at(&repo, T, "update TASK-7"); // bare mention: parser rejects
    commit_at(&repo, T + 1, "fix: lowercase (task-7)"); // Git's -F --grep rejects
    let c = commit_at(&repo, T + 2, "chore: body ref\n\ntrace:TASK-7 | ai:claude");
    commit_at(&repo, T + 3, "docs: TASK-71 only (TASK-71)");

    let cold = answered(&repo, &["TASK-7"]);
    assert_eq!(cold, oracle(&repo, &["TASK-7"]), "one batch: Git's order");
    let shas: Vec<&str> = cold.iter().map(|(f, _, _)| f.as_str()).collect();
    assert_eq!(shas, vec![c.as_str(), b.as_str(), a.as_str()]);
    assert_eq!(frontier_on_disk(&repo).batch, 0);

    // A new branch with a commit in the SAME second as `a`/`b`: batch 1.
    git(&repo, &["checkout", "-q", "-b", "side", &a]);
    let d = commit_at(&repo, T, "fix: side work (TASK-7)");
    git(&repo, &["checkout", "-q", "main"]);
    let warm = answered(&repo, &["TASK-7"]);
    assert_eq!(sorted(warm.clone()), sorted(oracle(&repo, &["TASK-7"])));
    let shas: Vec<&str> = warm.iter().map(|(f, _, _)| f.as_str()).collect();
    assert_eq!(
        shas,
        vec![c.as_str(), d.as_str(), b.as_str(), a.as_str()],
        "O2: ct desc, then batch desc, then walk position asc"
    );
    assert_eq!(frontier_on_disk(&repo).batch, 1);

    // Multi-id queries and the TASK-71 key agree too.
    assert_eq!(
        sorted(answered(&repo, &["TASK-7", "TASK-71"])),
        sorted(oracle(&repo, &["TASK-7", "TASK-71"]))
    );
    assert_eq!(answered(&repo, &["TASK-71"]), oracle(&repo, &["TASK-71"]));
    assert!(answered(&repo, &["TASK-9"]).is_empty());
}

/// A deleted unmerged branch's commit stays in the index but is no longer
/// reachable from T0, so it is dropped; restoring the ref brings it back
/// (re-walked rows dedupe on read).
#[test]
fn deleted_unmerged_branch_and_restored_ref() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: on main (TASK-7)");
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    let lost = commit_at(&repo, T + 1, "feat: unmerged (TASK-7)");
    git(&repo, &["checkout", "-q", "main"]);
    assert_eq!(answered(&repo, &["TASK-7"]).len(), 2);

    git(&repo, &["branch", "-q", "-D", "feature"]);
    let after_delete = answered(&repo, &["TASK-7"]);
    assert_eq!(after_delete, oracle(&repo, &["TASK-7"]));
    assert!(after_delete.iter().all(|(f, _, _)| f != &lost));

    git(&repo, &["branch", "-q", "feature", &lost]);
    let restored = answered(&repo, &["TASK-7"]);
    assert_eq!(restored, oracle(&repo, &["TASK-7"]));
    assert_eq!(
        restored.first().map(|(f, _, _)| f.as_str()),
        Some(lost.as_str())
    );
}

/// A torn append (a final line without its newline) is ignored on read and
/// cut back before the next append, so the shard stays parseable.
#[test]
fn torn_append_is_ignored_and_repaired() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: one (TASK-7)");
    answered(&repo, &["TASK-7"]);
    let shard = shard_file(&repo, "TASK-7");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&shard)
        .unwrap();
    f.write_all(b"0123456789abcdef0123456789abcdef01234567 17")
        .unwrap();
    drop(f);
    assert_eq!(answered(&repo, &["TASK-7"]), oracle(&repo, &["TASK-7"]));

    commit_at(&repo, T + 1, "fix: two (TASK-7)");
    assert_eq!(answered(&repo, &["TASK-7"]), oracle(&repo, &["TASK-7"]));
    let text = std::fs::read_to_string(&shard).unwrap();
    assert!(text.ends_with('\n'));
    assert!(text.lines().all(|l| Row::parse(l).is_some()), "{text}");
    assert_eq!(text.lines().count(), 2);

    // A committed prefix that no longer verifies (a crash lost or damaged
    // unsynced data) is never answered from: the original query runs and
    // the index is rebuilt.
    let damaged = text.replacen(&text[..40], &"0".repeat(40), 1);
    std::fs::write(&shard, damaged).unwrap();
    assert_eq!(index(&repo, &["TASK-7"]), IndexAnswer::RunOriginal);
    assert!(!ns(&repo).join("meta").exists());
    assert_eq!(answered(&repo, &["TASK-7"]), oracle(&repo, &["TASK-7"]));
}

/// A meta written under another parser version (or format, or encoding)
/// fails closed to the original query and drops `meta`; the next show
/// rebuilds cold.
#[test]
fn parser_version_bump_rebuilds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: one (TASK-7)");
    answered(&repo, &["TASK-7"]);
    let meta = ns(&repo).join("meta");
    let text = std::fs::read_to_string(&meta).unwrap();
    let stale = text.replace(
        &format!("parser={PARSER_VERSION}\n"),
        &format!("parser={}\n", PARSER_VERSION + 1),
    );
    assert_ne!(text, stale);
    std::fs::write(&meta, stale).unwrap();
    assert_eq!(index(&repo, &["TASK-7"]), IndexAnswer::RunOriginal);
    assert!(!meta.exists(), "a mismatched meta is dropped");
    assert_eq!(answered(&repo, &["TASK-7"]), oracle(&repo, &["TASK-7"]));
    assert!(meta.exists(), "the next show rebuilds");
}

/// A git error is UNKNOWN, never an empty answer: a frontier tip whose
/// object is gone (pruned by gc) fails the incremental walk, the original
/// query answers, and the index is rebuilt later.
#[test]
fn git_failure_is_unknown_and_rebuilds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: one (TASK-7)");
    answered(&repo, &["TASK-7"]);
    let gen_dir = generation_dir(&repo);
    let mut frontier = frontier_on_disk(&repo);
    frontier
        .tips
        .push("0123456789abcdef0123456789abcdef01234567".into());
    write_frontier(&gen_dir, &frontier).unwrap();
    commit_at(&repo, T + 1, "fix: two (TASK-7)"); // forces a walk
    assert_eq!(index(&repo, &["TASK-7"]), IndexAnswer::RunOriginal);
    assert!(!ns(&repo).join("meta").exists());
    assert_eq!(answered(&repo, &["TASK-7"]), oracle(&repo, &["TASK-7"]));
}

/// Shallow repositories and replace refs change what ancestry means: the
/// index is disabled and writes nothing.
#[test]
fn shallow_and_replace_refs_disable_the_index() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: one (TASK-7)");
    let two = commit_at(&repo, T + 1, "fix: two (TASK-7)");

    let shallow = tmp.path().join("shallow");
    let url = format!("file://{}", repo.display());
    let out = std::process::Command::new("git")
        .args(["clone", "-q", "--depth", "1", &url])
        .arg(&shallow)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(index(&shallow, &["TASK-7"]), IndexAnswer::RunOriginal);
    assert!(!ns(&shallow).exists());

    git(&repo, &["replace", &two, "HEAD~1"]);
    assert_eq!(index(&repo, &["TASK-7"]), IndexAnswer::RunOriginal);
    assert!(!ns(&repo).exists());
}

/// While another writer holds the lock, a reader answers from its own walk
/// in memory and writes nothing: no build when cold, no append or frontier
/// move when warm.
#[test]
fn held_lock_answers_in_memory_and_writes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: one (TASK-7)");
    let dir = namespace_dir(&ns(&repo).ancestors().nth(3).unwrap().to_path_buf(), true)
        .unwrap()
        .unwrap();
    let held = try_lock(&dir).unwrap().expect("test takes the lock");
    assert_eq!(answered(&repo, &["TASK-7"]), oracle(&repo, &["TASK-7"]));
    assert!(!dir.join("meta").exists(), "cold reader built nothing");
    drop(held);

    answered(&repo, &["TASK-7"]);
    let before = frontier_on_disk(&repo);
    let shard_before = std::fs::read(shard_file(&repo, "TASK-7")).unwrap();
    commit_at(&repo, T + 1, "fix: two (TASK-7)");
    let held = try_lock(&dir).unwrap().expect("test takes the lock");
    let warm = answered(&repo, &["TASK-7"]);
    assert_eq!(warm, oracle(&repo, &["TASK-7"]));
    assert_eq!(warm.len(), 2, "the in-memory batch answers");
    assert_eq!(frontier_on_disk(&repo), before, "frontier unmoved");
    assert_eq!(
        std::fs::read(shard_file(&repo, "TASK-7")).unwrap(),
        shard_before
    );
    drop(held);
    answered(&repo, &["TASK-7"]);
    assert_eq!(frontier_on_disk(&repo).batch, before.batch + 1);
}

/// Directories are 0700 and files 0600 with explicit modes; a shared-write
/// or symlinked namespace is refused (the original query answers) and
/// never written through.
#[cfg(unix)]
#[test]
fn storage_modes_and_refusals() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: one (TASK-7)");
    answered(&repo, &["TASK-7"]);
    let dir = ns(&repo);
    let gen_dir = generation_dir(&repo);
    for d in [
        dir.parent().unwrap().parent().unwrap().to_path_buf(),
        dir.parent().unwrap().to_path_buf(),
        dir.clone(),
        gen_dir.clone(),
        gen_dir.join("shards"),
    ] {
        assert_eq!(mode(&d), 0o700, "{}", d.display());
    }
    for f in [
        dir.join("meta"),
        dir.join("lock"),
        gen_dir.join("frontier"),
        shard_file(&repo, "TASK-7"),
    ] {
        assert_eq!(mode(&f), 0o600, "{}", f.display());
    }

    // Group/other write on the namespace: refused.
    let write_bits = |add: bool| {
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        let bits = if add {
            perms.mode() | 0o022
        } else {
            perms.mode() & !0o022
        };
        perms.set_mode(bits);
        std::fs::set_permissions(&dir, perms).unwrap();
    };
    write_bits(true);
    commit_at(&repo, T + 1, "fix: two (TASK-7)");
    assert_eq!(index(&repo, &["TASK-7"]), IndexAnswer::RunOriginal);
    assert_eq!(frontier_on_disk(&repo).batch, 0, "nothing written");
    write_bits(false);

    // A symlinked namespace component is never followed.
    let other = tempfile::TempDir::new().unwrap();
    let repo2 = init_repo(other.path());
    commit_at(&repo2, T, "fix: one (TASK-7)");
    let target = other.path().join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    let aida_dir = ns(&repo2).parent().unwrap().parent().unwrap().to_path_buf();
    std::os::unix::fs::symlink(&target, &aida_dir).unwrap();
    assert_eq!(index(&repo2, &["TASK-7"]), IndexAnswer::RunOriginal);
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
}

/// The show path consults the index and returns the original's commits.
#[test]
fn show_path_uses_the_index() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = init_repo(tmp.path());
    commit_at(&repo, T, "fix: one (TASK-7)");
    commit_at(&repo, T + 1, "fix: two (TASK-7)");
    let shown = crate::collect_git_linkage_until(
        &repo,
        &ids(&["TASK-7"]),
        Instant::now() + Duration::from_secs(30),
    );
    assert_eq!(shown.commits, oracle(&repo, &["TASK-7"]));
    assert!(!shown.commits_scan_incomplete);
    assert!(ns(&repo).join("meta").exists(), "the show built the index");
}

/// The resolver source the index's rows depend on. Changing ANY of these
/// functions requires bumping [`PARSER_VERSION`] and re-pinning the hash
/// below in the same change.
#[test]
fn parser_version_ratchet() {
    let pr_lookup = include_str!("../pr_lookup.rs");
    let zen = include_str!("../zen_drive_root.rs");
    let body = |src: &str, name: &str| -> String {
        let start = src
            .find(&format!("pub(crate) fn {name}("))
            .unwrap_or_else(|| panic!("{name} not found"));
        let end = src[start..].find("\n}\n").expect("fn end") + start + 3;
        src[start..end].to_string()
    };
    let mut joined = String::new();
    for (src, name) in [
        (zen, "extract_spec_ids_from_commit"),
        (pr_lookup, "extract_referenced_spec_ids_from_commit"),
        (pr_lookup, "extract_trace_line_spec_ids"),
        (pr_lookup, "push_paren_spec_ids_from_line"),
        (pr_lookup, "body_line_is_code_like"),
        (pr_lookup, "looks_like_spec_id"),
    ] {
        joined.push_str(&body(src, name));
    }
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(joined.as_bytes()));
    assert_eq!(
        (PARSER_VERSION, hash.as_str()),
        (
            1,
            "726831f8b27f9d65f4a7435a6af0e7187815e684284b9506f8f18a96f6dc77c8"
        ),
        "the linkage resolver changed: bump PARSER_VERSION and re-pin this hash"
    );
}
