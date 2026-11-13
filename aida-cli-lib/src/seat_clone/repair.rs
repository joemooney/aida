//! Host-owned evidence for a retained native-seat rework merge.
// trace:BUG-2558 | ai:codex
use super::*;
use fs2::FileExt;

#[derive(serde::Serialize, serde::Deserialize)]
struct Journal {
    version: u32,
    entries: Vec<Repair>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Repair {
    tree: PathBuf,
    identity: (u64, u64),
    branch: String,
    head: String,
    base: String,
    paths: Vec<String>,
    recorded_at: chrono::DateTime<chrono::Utc>,
}

/// The journal lives at the trusted main root, never in the seat. Keep its
/// permanent lock across read/modify/write; unknown versions fail closed.
fn journal<T>(main: &Path, f: impl FnOnce(&mut Journal) -> Result<(T, bool)>) -> Result<T> {
    let path = main.join(".aida/repair-journal.json");
    std::fs::create_dir_all(path.parent().context("repair journal directory")?)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "repair journal is busy"
                );
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => return Err(e.into()),
        }
    }
    let mut state = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Journal>(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal {
            version: 1,
            entries: Vec::new(),
        },
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(state.version == 1, "unsupported repair journal version");
    let (result, changed) = f(&mut state)?;
    if changed {
        aida_core::write_atomic(&path, serde_json::to_vec(&state)?)?;
    }
    Ok(result)
}

pub(super) fn record(
    main: &Path,
    tree: &Path,
    branch: &str,
    identity: (u64, u64),
    head: String,
    base: String,
    paths: Vec<String>,
) -> Result<()> {
    ensure_dir_identity(tree, identity)?;
    anyhow::ensure!(!paths.is_empty(), "cannot record an empty conflict repair");
    journal(main, |state| {
        state.entries.retain(|entry| entry.tree != tree);
        state.entries.push(Repair {
            tree: tree.to_owned(),
            identity,
            branch: branch.into(),
            head,
            base,
            paths,
            recorded_at: chrono::Utc::now(),
        });
        Ok(((), true))
    })
}

/// Bind immutable original paths to both merge parents, the lease branch and
/// the original directory. The mutable index cannot supply this evidence.
pub(super) fn paths(
    main: &Path,
    tree: &Path,
    branch: &str,
    head: &str,
    base: &str,
) -> Result<Vec<String>> {
    journal(main, |state| {
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.tree == tree)
            .context("pending merge has no host-recorded repair journal entry")?;
        anyhow::ensure!(
            entry.identity == dir_identity(tree)?
                && entry.branch == branch
                && entry.head == head
                && entry.base == base,
            "pending merge differs from its host-recorded repair; nothing was staged or committed"
        );
        anyhow::ensure!(!entry.paths.is_empty(), "empty host-recorded repair paths");
        Ok((entry.paths.clone(), false))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real Git conflict, read-only lane edits, staged retry and journal reload.
    // No bubblewrap dependency: the separate clone fixture covers that boundary.
    // trace:BUG-2558 | ai:codex
    #[test]
    fn bug_2558_retained_merge_uses_persisted_original_paths_on_retry() {
        let root = tempfile::tempdir().unwrap();
        let tree = root.path();
        let execute = |args: &[&str]| -> Result<String> {
            let out = crate::test_git::git_command()
                .arg("-C")
                .arg(tree)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.test",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let text = String::from_utf8(out.stdout)?;
            Ok(if args[0] == "diff" {
                text
            } else {
                text.trim().into()
            })
        };
        execute(&["init", "-b", "main"]).unwrap();
        std::fs::write(tree.join("shared"), "base\n").unwrap();
        execute(&["add", "shared"]).unwrap();
        execute(&["commit", "-m", "base"]).unwrap();
        execute(&["checkout", "-b", "rework"]).unwrap();
        std::fs::write(tree.join("shared"), "lane\n").unwrap();
        execute(&["commit", "-am", "lane"]).unwrap();
        let head = execute(&["rev-parse", "HEAD"]).unwrap();
        execute(&["checkout", "main"]).unwrap();
        std::fs::write(tree.join("shared"), "main\n").unwrap();
        execute(&["commit", "-am", "main"]).unwrap();
        let base = execute(&["rev-parse", "HEAD"]).unwrap();
        execute(&["checkout", "rework"]).unwrap();
        let original = prepare_rework_merge_impl(
            &execute,
            true,
            "rework",
            false,
            |_| panic!("conflicted merge must not publish before lane resolution"),
            || Ok((base.clone(), head.clone())),
        )
        .unwrap();
        assert_eq!(original, ["shared"]);
        assert_eq!(execute(&["rev-parse", "MERGE_HEAD"]).unwrap(), base);
        assert!(!execute(&["ls-files", "-u"]).unwrap().is_empty());
        record(
            tree,
            tree,
            "rework",
            dir_identity(tree).unwrap(),
            head.clone(),
            base.clone(),
            original.clone(),
        )
        .unwrap();
        let probe = |program: &str, args: &[&std::ffi::OsStr]| -> Result<Vec<u8>> {
            let out = std::process::Command::new(program)
                .current_dir(tree)
                .args(args)
                .output()?;
            anyhow::ensure!(out.status.success(), "marker probe failed");
            Ok(out.stdout)
        };
        let scan = || -> Result<()> {
            let paths: Vec<std::ffi::OsString> = paths(tree, tree, "rework", &head, &base)?
                .into_iter()
                .map(Into::into)
                .collect();
            refuse_pending_conflict_markers(&probe, Some(&paths))
        };
        assert!(scan().is_err());
        std::fs::write(tree.join("shared"), ">>>>>>> closing only\n").unwrap();
        execute(&["add", "shared"]).unwrap();
        assert!(execute(&["ls-files", "-u"]).unwrap().is_empty());
        assert!(scan().is_err());
        std::fs::write(tree.join("shared"), "lane and main resolved\n").unwrap();
        scan().unwrap();
        assert_eq!(paths(tree, tree, "rework", &head, &base).unwrap(), original);
        assert!(paths(tree, tree, "other", &head, &base).is_err());
        assert!(paths(tree, tree, "rework", &base, &head).is_err());
        execute(&["add", "shared"]).unwrap();
        execute(&["commit", "-m", "resolve"]).unwrap();
        assert_eq!(
            execute(&["show", "-s", "--format=%P", "HEAD"]).unwrap(),
            format!("{head} {base}")
        );
        std::fs::write(
            tree.join(".aida/repair-journal.json"),
            r#"{"version":2,"entries":[]}"#,
        )
        .unwrap();
        assert!(paths(tree, tree, "rework", &head, &base).is_err());
    }
}
