//! Fresh scoring inputs, independent of caller directories and prior installs.
// trace:TASK-1731 | ai:codex
use super::*;
use std::collections::BTreeSet;

pub(super) struct FreshRoot {
    root: tempfile::TempDir,
    hash: String,
    directories: BTreeSet<PathBuf>,
}

fn directories(root: &Path, dir: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut result = BTreeSet::new();
    for child in std::fs::read_dir(dir)? {
        let child = child?;
        if child.file_type()?.is_dir() {
            result.insert(child.path().strip_prefix(root)?.to_owned());
            result.extend(directories(root, &child.path())?);
        }
    }
    Ok(result)
}

impl FreshRoot {
    fn from_entries(
        entries: BTreeMap<String, oracle::Entry>,
        dirs: BTreeSet<PathBuf>,
    ) -> Result<Self> {
        oracle::validate_links(&entries)?;
        let root = tempfile::Builder::new()
            .prefix("aida-regen-root-")
            .tempdir_in("/tmp")?;
        anyhow::ensure!(
            std::fs::read_dir(root.path())?.next().is_none(),
            "new scoring root is not empty"
        );
        for dir in dirs {
            std::fs::create_dir_all(root.path().join(dir))?;
        }
        oracle::write_entries(root.path(), &entries)?;
        let value = Self {
            hash: oracle::hash(&entries),
            directories: directories(root.path(), root.path())?,
            root,
        };
        value.assert_pristine()?;
        Ok(value)
    }

    pub(super) fn empty() -> Result<Self> {
        Self::from_entries(BTreeMap::new(), BTreeSet::new())
    }

    pub(super) fn trial(source: &Path) -> Result<Self> {
        let source = source.canonicalize()?;
        let root = Self::empty()?;
        roots::copy_trial_tree(&source, root.path())?;
        let mut entries = BTreeMap::new();
        oracle::disk_entries(root.path(), root.path(), &mut entries)?;
        oracle::validate_links(&entries)?;
        let value = Self {
            hash: oracle::hash(&entries),
            directories: directories(root.path(), root.path())?,
            root: root.root,
        };
        value.assert_pristine()?;
        Ok(value)
    }

    pub(super) fn reference(project: &Path, sha: &str) -> Result<Self> {
        // Enumerate top-level names without pathspec interpretation, then reuse
        // the same pinned-blob exporter and link validation as held-out inputs.
        let names = super::super::git(project, &["ls-tree", "--name-only", "-z", sha])?;
        let paths = names
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| Ok(std::str::from_utf8(b)?.to_owned()))
            .collect::<Result<Vec<String>>>()?;
        Self::from_entries(
            oracle::frozen_entries(project, sha, &paths)?,
            BTreeSet::new(),
        )
    }

    pub(super) fn path(&self) -> &Path {
        self.root.path()
    }

    pub(super) fn assert_pristine(&self) -> Result<()> {
        let mut entries = BTreeMap::new();
        oracle::disk_entries(self.path(), self.path(), &mut entries)?;
        anyhow::ensure!(
            oracle::hash(&entries) == self.hash
                && directories(self.path(), self.path())? == self.directories,
            "fresh scoring root contains unexpected or changed inputs"
        );
        Ok(())
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    #[test]
    fn fresh_trials_preserve_inputs_and_never_mutate_or_reuse_the_source() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("empty")).unwrap();
        std::fs::write(source.path().join("tool"), "input").unwrap();
        std::fs::set_permissions(
            source.path().join("tool"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("tool", source.path().join("link")).unwrap();
        std::fs::write(source.path().join(".git"), "gitdir: /original/source").unwrap();
        let first = FreshRoot::trial(source.path()).unwrap();
        assert!(first.path().join("empty").is_dir());
        assert!(!first.path().join(".git").exists());
        assert_eq!(std::fs::read(first.path().join("link")).unwrap(), b"input");
        std::fs::write(first.path().join("tool"), "installer mutation").unwrap();
        assert!(first.assert_pristine().is_err());
        let second = FreshRoot::trial(source.path()).unwrap();
        assert_ne!(first.path(), second.path());
        assert_eq!(std::fs::read(second.path().join("tool")).unwrap(), b"input");
        assert_eq!(std::fs::read(source.path().join("tool")).unwrap(), b"input");
        second.assert_pristine().unwrap();
        std::fs::create_dir(second.path().join("unexpected-empty")).unwrap();
        assert!(
            second.assert_pristine().is_err(),
            "unexpected empty dirs must be detected too"
        );
    }
    #[test]
    fn escaping_omitted_and_directory_symlinks_refuse() {
        for target in ["/etc/passwd", "../outside", ".git", "directory"] {
            let source = tempfile::tempdir().unwrap();
            std::fs::create_dir(source.path().join("directory")).unwrap();
            std::os::unix::fs::symlink(target, source.path().join("link")).unwrap();
            assert!(
                FreshRoot::trial(source.path()).is_err(),
                "accepted {target}"
            );
        }
    }
    #[test]
    fn reference_export_uses_pinned_tree_and_fresh_independent_roots() {
        let source = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| super::super::super::git(source.path(), args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::write(source.path().join("system"), "trusted").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "reference"]);
        let sha = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        std::fs::write(source.path().join("system"), "dirty substitution").unwrap();
        std::fs::write(source.path().join("untracked"), "not trusted").unwrap();
        let first = FreshRoot::reference(source.path(), sha.trim()).unwrap();
        let second = FreshRoot::reference(source.path(), sha.trim()).unwrap();
        assert_ne!(first.path(), second.path());
        assert_eq!(
            std::fs::read(first.path().join("system")).unwrap(),
            b"trusted"
        );
        assert!(!first.path().join("untracked").exists());
        assert!(!first.path().join(".git").exists());
        std::fs::write(first.path().join("system"), "changed").unwrap();
        second.assert_pristine().unwrap();
    }
}
