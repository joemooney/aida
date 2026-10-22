//! BUG-2180: commit the store writes a contained child session left for its
//! host.
//!
//! A contained session can write the store's files but not its git history
//! (BUG-1929 r4), so every store write it makes — the reviewer's
//! request-changes shelve, a claim, a comment — is left uncommitted for "the
//! host's next store sync". The process that launched the child and waited
//! for it IS that host, and nothing else was guaranteed to sync before the
//! mandate's fail-closed store snapshot refused every ship. So each launcher
//! that waits on a child session commits what the child wrote, as one
//! store-writer transaction (store write lock + stage + commit of only those
//! paths), the same shape as the BUG-2178 timing-record publish.
//!
//! "What the child wrote" is every store path whose `git status` entry
//! differs from the one captured just before the launch: a path that turned
//! dirty (modified, added, untracked, deleted, renamed or copied, both sides
//! of a rename), or a path already dirty whose state or content then changed.
//! Host writers commit their own changes under the same lock, so a change in
//! that window is a deferred contained write; a path left exactly as it was
//! before the launch is the host's own older dirt and is left alone.
//!
//! [`ChildStoreCommit`] is armed before the launch and commits when the
//! launcher is done with the child, on every exit path: an error after the
//! child ran drops the guard, which commits.
//!
//! trace:BUG-2180 trace:BUG-2185 | ai:claude

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// One store path's `git status --porcelain=v2` entry (path fields
/// excluded) plus a hash of its worktree content, so a further write to an
/// already-dirty file counts as a change.
// trace:BUG-2185 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathState {
    entry: String,
    content: Option<u64>,
}

/// The store's dirty paths just before a child launch.
// trace:BUG-2185 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct StoreBaseline {
    dirty: HashMap<String, PathState>,
}

impl StoreBaseline {
    /// Capture `store`'s dirty paths. An unreadable status (not a repo, git
    /// failure) is an empty baseline: every dirty path after the run then
    /// counts as the child's, the conservative side for a dirty-store refusal.
    // trace:BUG-2185 | ai:claude
    pub(crate) fn capture(store: &Path) -> Self {
        StoreBaseline {
            dirty: store_status(store)
                .map(|entries| {
                    entries
                        .into_iter()
                        .flat_map(|e| {
                            e.paths()
                                .map(|p| (p.to_string(), e.state(store, p)))
                                .collect::<Vec<_>>()
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// Arm before launching a child session; commits what the child wrote to the
/// store when [`ChildStoreCommit::commit`] is called or, on any other exit
/// path (an `Err` returned after the child ran, a panic), when dropped.
// trace:BUG-2185 | ai:claude
#[must_use = "dropping the guard commits immediately"]
pub(crate) struct ChildStoreCommit {
    project_root: PathBuf,
    child: &'static str,
    baseline: Option<StoreBaseline>,
}

impl ChildStoreCommit {
    // trace:BUG-2185 | ai:claude
    pub(crate) fn arm(project_root: &Path, child: &'static str) -> Self {
        let baseline = crate::detect_distributed_store_from(project_root)
            .map(|store| StoreBaseline::capture(&store));
        ChildStoreCommit {
            project_root: project_root.to_path_buf(),
            child,
            baseline,
        }
    }

    /// Commit now (the launcher's normal point, before it reads the store).
    /// Returns the committed store paths.
    // trace:BUG-2185 | ai:claude
    pub(crate) fn commit(mut self) -> Vec<String> {
        self.commit_once()
    }

    fn commit_once(&mut self) -> Vec<String> {
        match self.baseline.take() {
            Some(baseline) => commit_child_store_writes(&self.project_root, &baseline, self.child),
            None => Vec::new(),
        }
    }
}

impl Drop for ChildStoreCommit {
    fn drop(&mut self) {
        self.commit_once();
    }
}

/// Commit the store writes a child session left uncommitted in
/// `project_root`'s store since `baseline`. Best-effort: a failed lock or
/// commit is reported on stderr and never fails the caller, and the changes
/// stay written for the next store sync. Returns the committed store paths.
// trace:BUG-2180 trace:BUG-2185 | ai:claude
pub(crate) fn commit_child_store_writes(
    project_root: &Path,
    baseline: &StoreBaseline,
    child: &str,
) -> Vec<String> {
    let Some(store) = crate::detect_distributed_store_from(project_root) else {
        return Vec::new();
    };
    match commit_store_writes_since(&store, baseline, child) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("warning: the {child} session's store changes were not committed: {e:#}");
            Vec::new()
        }
    }
}

/// [`commit_child_store_writes`] with the store worktree named explicitly
/// (the testable form).
// trace:BUG-2180 trace:BUG-2185 | ai:claude
pub(crate) fn commit_store_writes_since(
    store: &Path,
    baseline: &StoreBaseline,
    child: &str,
) -> anyhow::Result<Vec<String>> {
    if !aida_core::git_ops::is_git_repo(store) {
        return Ok(Vec::new());
    }
    // A launcher that is itself contained cannot write the history either;
    // its own host commits both.
    if aida_core::git_ops::store_commit_deferred_to_host(store) {
        aida_core::git_ops::notice_store_commit_deferred(store);
        return Ok(Vec::new());
    }
    aida_core::db::with_store_write_lock(store, || {
        let paths = changed_paths_since(store, baseline)?;
        if paths.is_empty() {
            return Ok(paths);
        }
        for path in &paths {
            if store.join(path).symlink_metadata().is_ok() {
                aida_core::git_ops::add_all(store, path)?;
            } else {
                // Gone from the worktree: stage the deletion if the index
                // still has it (a rename's source already staged is not).
                let rm = aida_core::git_ops::host_git_command(store)
                    .args(["rm", "--cached", "--ignore-unmatch", "-q", "--", path])
                    .output()?;
                anyhow::ensure!(
                    rm.status.success(),
                    "git rm --cached {path} failed: {}",
                    String::from_utf8_lossy(&rm.stderr).trim()
                );
            }
        }
        // TASK-2009: a path staged-added before the launch and then deleted
        // by the child is now unknown to both the index and HEAD, and one
        // unknown pathspec aborts the whole `git commit -- <paths>`. It has
        // nothing to commit, so it is dropped.
        // trace:TASK-1952 | trace:TASK-2009 | trace:TASK-1652 | ai:claude
        let known = known_to_git(store, &paths)?;
        let paths: Vec<String> = paths.into_iter().filter(|p| known.contains(p)).collect();
        if paths.is_empty() {
            return Ok(paths);
        }
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let unchanged = aida_core::git_ops::host_git_command(store)
            .args(["diff", "--cached", "--quiet", "--"])
            .args(&refs)
            .status()?;
        if unchanged.success() {
            return Ok(Vec::new());
        }
        aida_core::git_ops::commit_paths(
            store,
            &format!(
                "{child}: commit {} store change{} from the contained session",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            ),
            &refs,
        )?;
        Ok(paths)
    })
}

/// Which of `paths` the store's index or HEAD knows (a pathspec `git commit
/// -- <paths>` accepts). A store with no commit yet (an unborn HEAD) has no
/// HEAD paths; any other git failure is an error.
// trace:TASK-1952 | trace:TASK-2009 | ai:claude
fn known_to_git(
    store: &Path,
    paths: &[String],
) -> anyhow::Result<std::collections::HashSet<String>> {
    let list = |args: &[&str]| -> anyhow::Result<Vec<String>> {
        let out = aida_core::git_ops::host_git_command(store)
            .arg("--literal-pathspecs")
            .args(args)
            .arg("--")
            .args(paths)
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect())
    };
    let mut known: std::collections::HashSet<String> =
        list(&["ls-files", "-z"])?.into_iter().collect();
    // Only an unborn HEAD has no paths; any other ls-tree failure is an
    // error, never an empty HEAD.
    let born = aida_core::git_ops::host_git_command(store)
        .args(["rev-parse", "-q", "--verify", "HEAD^{commit}"])
        .output()?
        .status
        .success();
    if born {
        known.extend(list(&["ls-tree", "-r", "-z", "--name-only", "HEAD"])?);
    }
    Ok(known)
}

/// One `git status --porcelain=v2 -z` entry.
// trace:BUG-2185 | ai:claude
struct StatusEntry {
    /// The entry without its path fields.
    fields: String,
    path: String,
    /// A rename or copy's source path.
    orig: Option<String>,
}

impl StatusEntry {
    /// Every path the entry touches: both sides of a rename or copy, so the
    /// source-side deletion is committed with the destination.
    fn paths(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.path.as_str()).chain(self.orig.as_deref())
    }

    fn state(&self, store: &Path, path: &str) -> PathState {
        use std::hash::{Hash, Hasher};
        let content = std::fs::read(store.join(path)).ok().map(|bytes| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut h);
            h.finish()
        });
        PathState {
            entry: self.fields.clone(),
            content,
        }
    }
}

/// The store's uncommitted entries (modified, added, deleted, renamed,
/// copied, unmerged or untracked; ignored files excluded).
// trace:BUG-2185 | ai:claude
fn store_status(store: &Path) -> anyhow::Result<Vec<StatusEntry>> {
    let out = aida_core::git_ops::host_git_command(store)
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_porcelain_v2(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `git status --porcelain=v2 -z`. Field counts before the path: `1`
/// ordinary (8), `2` rename/copy (9, then the source path as the next
/// NUL-separated record), `u` unmerged (10), `?` untracked (1). Header (`#`)
/// and ignored (`!`) records are skipped.
// trace:BUG-2185 | ai:claude
fn parse_porcelain_v2(stdout: &str) -> Vec<StatusEntry> {
    let mut records = stdout.split('\0').filter(|r| !r.is_empty());
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        let leading = match record.as_bytes().first() {
            Some(b'1') => 8,
            Some(b'2') => 9,
            Some(b'u') => 10,
            Some(b'?') => 1,
            _ => continue,
        };
        let mut split = record.splitn(leading + 1, ' ');
        let fields: Vec<&str> = split.by_ref().take(leading).collect();
        let Some(path) = split.next() else {
            continue;
        };
        let orig = (leading == 9).then(|| records.next()).flatten();
        entries.push(StatusEntry {
            fields: fields.join(" "),
            path: path.to_string(),
            orig: orig.map(str::to_string),
        });
    }
    entries
}

/// Store paths whose status entry or content differs from `baseline`,
/// sorted. A deleted path and a rename's source are included.
// trace:BUG-2185 | ai:claude
fn changed_paths_since(store: &Path, baseline: &StoreBaseline) -> anyhow::Result<Vec<String>> {
    let mut changed = BTreeSet::new();
    for entry in store_status(store)? {
        let touched = entry
            .paths()
            .any(|p| baseline.dirty.get(p) != Some(&entry.state(store, p)));
        if touched {
            changed.extend(entry.paths().map(str::to_string));
        }
    }
    Ok(changed.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        aida_core::git_ops::init(&store).unwrap();
        aida_core::git_ops::configure_user(&store, "fixture", "fixture@localhost").unwrap();
        std::fs::write(store.join("objects/old.yaml"), "a: 1\n").unwrap();
        aida_core::git_ops::add(&store, &["objects/old.yaml"]).unwrap();
        aida_core::git_ops::commit(&store, "fixture").unwrap();
        (tmp, store)
    }

    fn porcelain(store: &Path) -> String {
        let out = aida_core::git_ops::host_git_command(store)
            .args(["status", "--porcelain=v1", "--untracked-files=all"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    fn git(store: &Path, args: &[&str]) {
        let st = aida_core::git_ops::host_git_command(store)
            .args(args)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    /// Only paths changed after the baseline are committed: a new file, a
    /// modified tracked file, and a further write to a file that was already
    /// dirty. Dirt left exactly as it was before the launch stays.
    // trace:BUG-2180 trace:BUG-2185 | ai:claude
    #[test]
    fn bug_2185_commits_only_paths_changed_since_the_baseline() {
        let (_g, store) = store_fixture();
        std::fs::write(store.join("objects/stale.yaml"), "s: 1\n").unwrap();
        std::fs::write(store.join("objects/touched.yaml"), "t: 1\n").unwrap();
        let baseline = StoreBaseline::capture(&store);
        std::fs::write(store.join("objects/old.yaml"), "a: 2\n").unwrap();
        std::fs::write(store.join("objects/new.yaml"), "n: 1\n").unwrap();
        std::fs::write(store.join("objects/touched.yaml"), "t: 2\n").unwrap();

        let committed = commit_store_writes_since(&store, &baseline, "reviewer").unwrap();
        assert_eq!(
            committed,
            [
                "objects/new.yaml",
                "objects/old.yaml",
                "objects/touched.yaml"
            ]
        );
        assert_eq!(porcelain(&store), "?? objects/stale.yaml\n");

        // Nothing left to commit: no commit, no paths.
        let baseline = StoreBaseline::capture(&store);
        let again = commit_store_writes_since(&store, &baseline, "reviewer").unwrap();
        assert!(again.is_empty());
    }

    /// A deletion (a released lease, say) has no modification time; it is
    /// committed like any other write.
    // trace:BUG-2185 | ai:claude
    #[test]
    fn bug_2185_a_deleted_path_is_committed() {
        let (_g, store) = store_fixture();
        let baseline = StoreBaseline::capture(&store);
        std::fs::remove_file(store.join("objects/old.yaml")).unwrap();

        let committed = commit_store_writes_since(&store, &baseline, "reviewer").unwrap();
        assert_eq!(committed, ["objects/old.yaml"]);
        assert_eq!(porcelain(&store), "", "the deletion is committed");
    }

    /// A staged rename commits both sides: the source-side deletion is not
    /// left staged.
    // trace:BUG-2185 | ai:claude
    #[test]
    fn bug_2185_a_staged_rename_commits_its_source_side() {
        let (_g, store) = store_fixture();
        let baseline = StoreBaseline::capture(&store);
        git(&store, &["mv", "objects/old.yaml", "objects/moved.yaml"]);

        let committed = commit_store_writes_since(&store, &baseline, "reviewer").unwrap();
        assert_eq!(committed, ["objects/moved.yaml", "objects/old.yaml"]);
        assert_eq!(
            porcelain(&store),
            "",
            "both sides of the rename are committed"
        );
    }

    /// A path staged-added before the launch and deleted by the child is
    /// known to neither the index nor HEAD; it is dropped, and the child's
    /// other writes still commit.
    // trace:TASK-1952 | trace:TASK-2009 | trace:TASK-1652 | ai:claude
    #[test]
    fn task_2009_a_host_staged_path_the_child_deleted_does_not_abort_the_commit() {
        let (_g, store) = store_fixture();
        std::fs::write(store.join("objects/staged.yaml"), "s: 1\n").unwrap();
        git(&store, &["add", "objects/staged.yaml"]);
        git(&store, &["mv", "objects/old.yaml", "objects/renamed.yaml"]);
        let baseline = StoreBaseline::capture(&store);
        std::fs::remove_file(store.join("objects/staged.yaml")).unwrap();
        std::fs::remove_file(store.join("objects/renamed.yaml")).unwrap();
        std::fs::write(store.join("objects/new.yaml"), "n: 1\n").unwrap();

        let committed = commit_store_writes_since(&store, &baseline, "reviewer").unwrap();
        assert!(
            committed.contains(&"objects/new.yaml".to_string()),
            "{committed:?}"
        );
        assert!(
            !committed.contains(&"objects/staged.yaml".to_string()),
            "{committed:?}"
        );
        assert!(
            !porcelain(&store).contains("new.yaml"),
            "the child's other write is committed: {}",
            porcelain(&store)
        );
        let head = aida_core::git_ops::host_git_command(&store)
            .args(["ls-tree", "-r", "--name-only", "HEAD"])
            .output()
            .unwrap();
        let head = String::from_utf8(head.stdout).unwrap();
        assert!(head.contains("objects/new.yaml"), "{head}");
        assert!(
            !head.contains("objects/old.yaml"),
            "the rename's source deletion is committed: {head}"
        );
        assert!(!head.contains("objects/staged.yaml"), "{head}");
    }

    /// An unborn store HEAD has no paths; the index alone decides.
    // trace:TASK-1952 | trace:TASK-2126 | ai:claude
    #[test]
    fn task_2126_known_to_git_on_an_unborn_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        aida_core::git_ops::init(&store).unwrap();
        std::fs::write(store.join("objects/a.yaml"), "a: 1\n").unwrap();
        git(&store, &["add", "objects/a.yaml"]);
        let paths = vec![
            "objects/a.yaml".to_string(),
            "objects/gone.yaml".to_string(),
        ];
        let known = known_to_git(&store, &paths).expect("unborn HEAD is not an error");
        assert!(known.contains("objects/a.yaml"));
        assert!(!known.contains("objects/gone.yaml"));
    }

    // trace:BUG-2180 trace:BUG-2185 | ai:claude
    #[test]
    fn bug_2180_clean_or_non_git_store_is_a_no_op() {
        let (_g, store) = store_fixture();
        let baseline = StoreBaseline::capture(&store);
        assert!(commit_store_writes_since(&store, &baseline, "reviewer")
            .unwrap()
            .is_empty());
        let plain = tempfile::tempdir().unwrap();
        std::fs::write(plain.path().join("x.yaml"), "x").unwrap();
        let baseline = StoreBaseline::capture(plain.path());
        assert!(
            commit_store_writes_since(plain.path(), &baseline, "reviewer")
                .unwrap()
                .is_empty()
        );
    }

    /// The guard commits when a launcher returns an error after its child
    /// ran, the path a `?` used to skip.
    // trace:BUG-2185 | ai:claude
    #[test]
    fn bug_2185_an_error_exit_after_the_child_still_commits() {
        let (tmp, store) = store_fixture();
        let project = tmp.path();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        std::fs::write(
            project.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        fn launcher(project: &Path, store: &Path) -> anyhow::Result<()> {
            let _commit = ChildStoreCommit::arm(project, "reviewer");
            std::fs::remove_file(store.join("objects/old.yaml"))?;
            anyhow::bail!("waiting on the child failed")
        }
        assert!(launcher(project, &store).is_err());
        assert_eq!(
            porcelain(&store),
            "",
            "the error exit committed the child's write"
        );
    }
}
