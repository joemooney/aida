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
//! "What the child wrote" is every uncommitted store path modified at or
//! after the child's launch. Host writers commit their own changes under the
//! same lock, so a dirty path in that window is a deferred contained write;
//! anything older is left alone.
//!
//! trace:BUG-2180 | ai:claude

use std::path::Path;
use std::time::SystemTime;

/// Commit the store writes a child session launched at `since` left
/// uncommitted in `project_root`'s store. Best-effort: a failed lock or commit
/// is reported on stderr and never fails the caller, and the changes stay
/// written for the next store sync. Returns the committed store paths.
// trace:BUG-2180 | ai:claude
pub(crate) fn commit_child_store_writes(
    project_root: &Path,
    since: SystemTime,
    child: &str,
) -> Vec<String> {
    let Some(store) = crate::detect_distributed_store_from(project_root) else {
        return Vec::new();
    };
    match commit_store_writes_since(&store, since, child) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("warning: the {child} session's store changes were not committed: {e:#}");
            Vec::new()
        }
    }
}

/// [`commit_child_store_writes`] with the store worktree named explicitly
/// (the testable form).
// trace:BUG-2180 | ai:claude
pub(crate) fn commit_store_writes_since(
    store: &Path,
    since: SystemTime,
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
        let paths = dirty_paths_since(store, since)?;
        if paths.is_empty() {
            return Ok(paths);
        }
        for path in &paths {
            aida_core::git_ops::add_all(store, path)?;
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

/// The store's uncommitted paths (modified, added or untracked; ignored files
/// excluded) whose file was modified at or after `since`. A deleted path has
/// no modification time and is left for the next sync.
// trace:BUG-2180 | ai:claude
fn dirty_paths_since(store: &Path, since: SystemTime) -> anyhow::Result<Vec<String>> {
    let out = aida_core::git_ops::host_git_command(store)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut entries = stdout.split('\0').filter(|e| !e.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = entries.next() {
        let (code, path) = entry.split_at(entry.len().min(3));
        // A staged rename or copy carries its source path as the next entry.
        if code.starts_with('R') || code.starts_with('C') {
            entries.next();
        }
        let recent = std::fs::metadata(store.join(path))
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified >= since);
        if recent {
            paths.push(path.to_string());
        }
    }
    Ok(paths)
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

    /// Only paths written after the launch are committed; an older dirty
    /// path is left exactly as it was.
    // trace:BUG-2180 | ai:claude
    #[test]
    fn bug_2180_commits_only_paths_written_since_the_launch() {
        let (_g, store) = store_fixture();
        std::fs::write(store.join("objects/stale.yaml"), "s: 1\n").unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(store.join("objects/stale.yaml"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let since = SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::write(store.join("objects/old.yaml"), "a: 2\n").unwrap();
        std::fs::write(store.join("objects/new.yaml"), "n: 1\n").unwrap();

        let mut committed = commit_store_writes_since(&store, since, "reviewer").unwrap();
        committed.sort();
        assert_eq!(committed, ["objects/new.yaml", "objects/old.yaml"]);
        assert_eq!(porcelain(&store), "?? objects/stale.yaml\n");

        // Nothing left to commit: no commit, no paths.
        let again = commit_store_writes_since(&store, since, "reviewer").unwrap();
        assert!(again.is_empty());
    }

    // trace:BUG-2180 | ai:claude
    #[test]
    fn bug_2180_clean_or_non_git_store_is_a_no_op() {
        let (_g, store) = store_fixture();
        let since = SystemTime::now();
        assert!(commit_store_writes_since(&store, since, "reviewer")
            .unwrap()
            .is_empty());
        let plain = tempfile::tempdir().unwrap();
        std::fs::write(plain.path().join("x.yaml"), "x").unwrap();
        assert!(commit_store_writes_since(plain.path(), since, "reviewer")
            .unwrap()
            .is_empty());
    }
}
