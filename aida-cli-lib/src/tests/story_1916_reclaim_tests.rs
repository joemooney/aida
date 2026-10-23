//! STORY-1916 AC2 — `aida worktree reclaim` sees the BUG-1783 cache layout,
//! orphaned caches and idle duplicates, and proves merges with git alone
//! when `gh` is unreachable. Tempdir fixtures only; the git fixture is built
//! with `test_git::git_command`.
// trace:STORY-1916 | ai:claude

use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const HOUR: Duration = Duration::from_secs(3600);

/// Set the mtime of `path` and everything under it to `at`.
fn age_tree(path: &Path, at: SystemTime) {
    if std::fs::symlink_metadata(path).unwrap().is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            age_tree(&entry.path(), at);
        }
    }
    std::fs::File::open(path).unwrap().set_modified(at).unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    main: PathBuf,
    cache_root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let main = root.join("main");
    std::fs::create_dir_all(main.join("target/agent")).unwrap();
    std::fs::create_dir_all(main.join("target/worktrees")).unwrap();
    let cache_root = main.join("target/worktrees");
    Fixture {
        _tmp: tmp,
        root,
        main,
        cache_root,
    }
}

impl Fixture {
    /// A worktree source dir (no own target/) and its BUG-1783 cache,
    /// written `ago` before now.
    fn worktree(&self, name: &str, branch: Option<&str>, ago: Duration) -> WorktreeEntry {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        self.cache(name, ago);
        WorktreeEntry {
            path,
            branch: branch.map(str::to_string),
        }
    }

    fn cache(&self, name: &str, ago: Duration) -> PathBuf {
        let dir = self.cache_root.join(name);
        std::fs::create_dir_all(dir.join("debug/deps")).unwrap();
        std::fs::write(dir.join("debug/deps/lib.rlib"), vec![0u8; 1000]).unwrap();
        age_tree(&dir, SystemTime::now() - ago);
        dir
    }

    fn survey(&self, worktrees: Vec<WorktreeEntry>) -> Survey {
        let mut all = vec![WorktreeEntry {
            path: self.main.clone(),
            branch: Some("main".into()),
        }];
        all.extend(worktrees);
        Survey {
            main_root: self.main.clone(),
            worktrees: all,
            open_pr_branches: Some(Vec::new()),
            leases: Vec::new(),
            min_age_mins: 60,
            include_live: false,
            include_open_prs: false,
            now: SystemTime::now(),
            cache_root: Some(self.cache_root.clone()),
            open_prs: Vec::new(),
            merged_offline: Default::default(),
        }
    }
}

fn reclaimed(p: &Plan) -> Vec<(String, ReclaimWhy)> {
    let mut out: Vec<(String, ReclaimWhy)> = p
        .reclaim
        .iter()
        .map(|r| {
            (
                r.target.file_name().unwrap().to_string_lossy().into(),
                r.why,
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn story_1916_reclaim_finds_the_target_worktrees_cache_layout() {
    let f = fixture();
    let cold = f.worktree("aida-cold", Some("cold"), 3 * HOUR);
    let warm = f.worktree("aida-warm", Some("warm"), Duration::ZERO);
    let p = plan(&f.survey(vec![cold, warm]));
    assert_eq!(reclaimed(&p), vec![("aida-cold".into(), ReclaimWhy::Stale)]);
    assert!(p.reclaim[0].target.starts_with(&f.cache_root));
    assert!(
        p.skipped
            .iter()
            .any(|(path, s)| path.ends_with("aida-warm")
                && matches!(s, Skip::RecentlyTouched { .. })),
        "{:?}",
        p.skipped
    );
}

#[test]
fn story_1916_rail6_reads_the_newest_write_not_the_root_mtime() {
    let f = fixture();
    let wt = f.worktree("aida-building", Some("b"), 5 * HOUR);
    let cache = f.cache_root.join("aida-building");
    // Cargo writes deep inside; the cache root's own mtime stays 5h old.
    std::fs::write(cache.join("debug/deps/new.o"), b"x").unwrap();
    std::fs::File::open(cache.join("debug/deps"))
        .unwrap()
        .set_modified(SystemTime::now() - 5 * HOUR)
        .unwrap();
    std::fs::File::open(&cache)
        .unwrap()
        .set_modified(SystemTime::now() - 5 * HOUR)
        .unwrap();
    assert!(plan(&f.survey(vec![wt])).reclaim.is_empty());
}

#[test]
fn story_1916_orphan_cache_is_reclaimable_unless_a_live_lease_names_it() {
    let f = fixture();
    f.cache("gone-wt", 3 * HOUR);
    f.cache("leased-elsewhere", 3 * HOUR);
    f.cache("fresh-orphan", Duration::ZERO);
    let mut s = f.survey(Vec::new());
    s.leases = vec![LeaseHold {
        worktree_path: PathBuf::from("/some/clone/leased-elsewhere"),
        alive: true,
        pid: Some(7),
    }];
    let p = plan(&s);
    assert_eq!(reclaimed(&p), vec![("gone-wt".into(), ReclaimWhy::Orphan)]);
}

#[test]
fn story_1916_gh_down_reclaims_proven_merges_and_holds_only_the_rest() {
    let f = fixture();
    let merged = f.worktree("aida-merged", Some("landed"), 3 * HOUR);
    let open = f.worktree("aida-unknown", Some("wip"), 3 * HOUR);
    let mut s = f.survey(vec![merged, open]);
    s.open_pr_branches = None;
    s.merged_offline = ["landed".to_string()].into_iter().collect();
    let p = plan(&s);
    assert_eq!(
        reclaimed(&p),
        vec![("aida-merged".into(), ReclaimWhy::MergedOffline)]
    );
    assert!(
        p.skipped
            .iter()
            .any(|(path, s)| path.ends_with("aida-unknown") && *s == Skip::PrStateUnknown),
        "{:?}",
        p.skipped
    );
}

#[test]
fn story_1916_idle_duplicate_on_one_open_pr_keeps_the_freshest() {
    let f = fixture();
    // gh down: two detached review worktrees of PR 2830.
    let older = f.worktree("take-2830", None, 6 * HOUR);
    let newer = f.worktree("pr-2830", None, 2 * HOUR);
    let solo = f.worktree("take-2900", None, 6 * HOUR);
    let mut s = f.survey(vec![older, newer, solo]);
    s.open_pr_branches = None;
    let p = plan(&s);
    assert_eq!(
        reclaimed(&p),
        vec![("take-2830".into(), ReclaimWhy::IdleDuplicate)]
    );
    for kept in ["pr-2830", "take-2900"] {
        assert!(
            p.skipped
                .iter()
                .any(|(path, s)| path.ends_with(kept) && *s == Skip::PrStateUnknown),
            "{kept} must stay held: {:?}",
            p.skipped
        );
    }
}

#[test]
fn story_1916_idle_duplicate_with_gh_up_matches_take_dir_to_the_pr_branch() {
    let f = fixture();
    let branch_wt = f.worktree("aida-feat", Some("feat"), 3 * HOUR);
    let dup = f.worktree("aida-feat-2", Some("feat"), 5 * HOUR);
    let mut s = f.survey(vec![branch_wt, dup]);
    s.open_pr_branches = Some(vec!["feat".into()]);
    s.open_prs = vec![(2845, "feat".into())];
    let p = plan(&s);
    assert_eq!(
        reclaimed(&p),
        vec![("aida-feat-2".into(), ReclaimWhy::IdleDuplicate)]
    );
}

#[test]
fn story_1916_live_group_member_is_the_keeper() {
    let f = fixture();
    let live = f.worktree("aida-live", Some("feat"), 9 * HOUR);
    let dup = f.worktree("aida-dup", Some("feat"), 4 * HOUR);
    let mut s = f.survey(vec![live.clone(), dup]);
    s.open_pr_branches = Some(vec!["feat".into()]);
    s.leases = vec![LeaseHold {
        worktree_path: live.path,
        alive: true,
        pid: Some(1),
    }];
    let p = plan(&s);
    assert_eq!(
        reclaimed(&p),
        vec![("aida-dup".into(), ReclaimWhy::IdleDuplicate)]
    );
}

#[test]
fn story_1916_a_lone_open_pr_worktree_stays_held() {
    let f = fixture();
    let wt = f.worktree("aida-only", Some("feat"), 9 * HOUR);
    let mut s = f.survey(vec![wt]);
    s.open_pr_branches = Some(vec!["feat".into()]);
    assert!(plan(&s).reclaim.is_empty());
}

#[test]
fn story_1916_apply_refuses_target_agent_and_symlinks() {
    let f = fixture();
    let forged = |target: PathBuf| Plan {
        main_root: f.main.clone(),
        reclaim: vec![Reclaimable {
            target,
            branch: None,
            bytes: 1,
            why: ReclaimWhy::Orphan,
        }],
        skipped: Vec::new(),
        cache_root: Some(f.cache_root.clone()),
    };
    let rm = |p: &Path| std::fs::remove_dir_all(p);
    let err = apply(&forged(f.main.join("target/agent")), None, |_| None, rm).unwrap_err();
    assert!(err.to_string().contains("refusing to delete"), "{err}");
    assert!(f.main.join("target/agent").is_dir());

    let outside = f.root.join("precious");
    std::fs::create_dir_all(&outside).unwrap();
    let link = f.root.join("aida-link").join("target");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let err = apply(&forged(link), None, |_| None, rm).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
    assert!(outside.is_dir());

    let odd = f.root.join("somewhere/else");
    std::fs::create_dir_all(&odd).unwrap();
    assert!(apply(&forged(odd), None, |_| None, rm).is_err());
}

#[test]
fn story_1916_pr_number_and_gh_line_parsing() {
    assert_eq!(pr_number_from_dir("take-2830"), Some(2830));
    assert_eq!(pr_number_from_dir("pr-12"), Some(12));
    assert_eq!(pr_number_from_dir("aida-pr-77-rebase"), Some(77));
    assert_eq!(pr_number_from_dir("impl-story-1916"), None);
    assert_eq!(pr_number_from_dir("take-"), None);
    assert_eq!(
        parse_number_branch_lines("12 feat/x\n\nbad line\n13 y\n"),
        vec![(12, "feat/x".to_string()), (13, "y".to_string())]
    );
}

// ---------------------------------------------------------------------------
// Offline merge proof against a real repository
// ---------------------------------------------------------------------------

fn git(repo: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_file(repo: &Path, file: &str, body: &str) {
    std::fs::write(repo.join(file), body).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", file]);
}

fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    git(r, &["init", "-q", "-b", "main"]);
    commit_file(r, "base.txt", "base\n");
    // Fast-forward merged.
    git(r, &["checkout", "-q", "-b", "ff"]);
    commit_file(r, "ff.txt", "ff\n");
    git(r, &["checkout", "-q", "main"]);
    git(r, &["merge", "-q", "--ff-only", "ff"]);
    // Squash merged: two commits on the branch, one on main.
    git(r, &["checkout", "-q", "-b", "squashed"]);
    commit_file(r, "s1.txt", "one\n");
    commit_file(r, "s2.txt", "two\n");
    git(r, &["checkout", "-q", "main"]);
    git(r, &["merge", "-q", "--squash", "squashed"]);
    git(r, &["commit", "-q", "-m", "squash"]);
    // Never merged.
    git(r, &["checkout", "-q", "-b", "open"]);
    commit_file(r, "open.txt", "wip\n");
    git(r, &["checkout", "-q", "main"]);
    tmp
}

#[test]
fn story_1916_offline_merge_proof_ancestor_squash_and_unmerged() {
    let tmp = repo();
    let r = tmp.path();
    let d = SQUASH_SCAN_DEADLINE;
    assert_eq!(
        branch_merged_offline(r, "ff", "main", d),
        Some(MergeProof::Ancestor)
    );
    assert_eq!(
        branch_merged_offline(r, "squashed", "main", d),
        Some(MergeProof::Squash)
    );
    assert_eq!(branch_merged_offline(r, "open", "main", d), None);
    assert_eq!(branch_merged_offline(r, "no-such-branch", "main", d), None);
}

#[test]
fn story_1916_squash_scan_past_its_deadline_is_unknown_never_merged() {
    let tmp = repo();
    // C3: a timed-out scan holds the branch; the ancestry check alone still
    // runs, and a squash needs the scan.
    assert_eq!(
        branch_merged_offline(tmp.path(), "squashed", "main", Duration::ZERO),
        None
    );
    assert_eq!(
        branch_merged_offline(tmp.path(), "ff", "main", Duration::ZERO),
        Some(MergeProof::Ancestor)
    );
}
