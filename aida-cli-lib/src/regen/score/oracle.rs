//! Immutable held-out inputs from the same commit as the trusted config.
// trace:TASK-1730 | ai:codex
use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::symlink;

pub(super) struct Entry {
    pub(super) mode: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct OracleBundle {
    pub(super) root: PathBuf,
    pub(super) sha: String,
    pub(super) hash: String,
}

fn relative(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    anyhow::ensure!(
        !value.is_empty()
            && !path.is_absolute()
            && path.components().all(|c| matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            ))
            && path
                .components()
                .any(|c| matches!(c, std::path::Component::Normal(_))),
        "oracle paths must be nonempty repo-relative paths without '..': {value}"
    );
    Ok(path
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect())
}

pub(super) fn hash(entries: &BTreeMap<String, Entry>) -> String {
    let mut digest = Sha256::new();
    for (path, entry) in entries {
        // Length framing prevents path/content ambiguity; mode authenticates
        // executable bits and distinguishes symlinks from ordinary files.
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update(entry.mode.as_bytes());
        digest.update((entry.bytes.len() as u64).to_be_bytes());
        digest.update(&entry.bytes);
    }
    format!("{:x}", digest.finalize())
}

pub(super) fn frozen_entries(
    project: &Path,
    sha: &str,
    paths: &[String],
) -> Result<BTreeMap<String, Entry>> {
    anyhow::ensure!(!paths.is_empty(), "oracle_paths must not be empty");
    let allowed: Vec<_> = paths.iter().map(|p| relative(p)).collect::<Result<_>>()?;
    // Enumerate the pinned tree, not pathspecs (which interpret glob syntax).
    let tree = super::super::git(project, &["ls-tree", "-rz", "--full-tree", sha])?;
    let mut entries = BTreeMap::new();
    let mut matched = vec![false; allowed.len()];
    for row in tree.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let row = std::str::from_utf8(row).context("oracle tree paths must be UTF-8")?;
        let (meta, name) = row.split_once('\t').context("invalid git tree row")?;
        let path = Path::new(name);
        let mut include = false;
        for (i, allow) in allowed.iter().enumerate() {
            if path == allow || path.starts_with(allow) {
                matched[i] = true;
                include = true;
            }
        }
        if !include {
            continue;
        }
        relative(name)?;
        let fields: Vec<_> = meta.split_whitespace().collect();
        anyhow::ensure!(
            fields.len() == 3
                && fields[1] == "blob"
                && matches!(fields[0], "100644" | "100755" | "120000"),
            "unsupported oracle tree entry: {name}"
        );
        // trace:BUG-2160 | ai:claude
        let bytes = super::super::git(project, &["cat-file", "blob", fields[2]])
            .with_context(|| format!("read oracle input {name} at {sha}"))?;
        entries.insert(
            name.into(),
            Entry {
                mode: fields[0].into(),
                bytes,
            },
        );
    }
    anyhow::ensure!(
        matched.iter().all(|m| *m),
        "oracle_paths contains a missing or empty input"
    );
    validate_links(&entries)?;
    Ok(entries)
}

// trace:TASK-1731 | ai:codex
pub(super) fn validate_links(entries: &BTreeMap<String, Entry>) -> Result<()> {
    // Validate links lexically before writing anything. A link must resolve to
    // another included file, never to the host, trial, or an omitted input.
    for (name, entry) in entries {
        if entry.mode != "120000" {
            continue;
        }
        let mut current = PathBuf::from(name);
        let mut seen = HashSet::new();
        loop {
            anyhow::ensure!(seen.insert(current.clone()), "oracle symlink cycle: {name}");
            let target_entry = entries
                .get(current.to_str().context("invalid oracle path")?)
                .context("oracle symlink targets an omitted input")?;
            if target_entry.mode != "120000" {
                break;
            }
            let target = Path::new(
                std::str::from_utf8(&target_entry.bytes)
                    .context("oracle symlink target must be UTF-8")?,
            );
            anyhow::ensure!(
                !target.is_absolute(),
                "oracle symlink escapes bundle: {name}"
            );
            let mut resolved = current.parent().unwrap_or(Path::new("")).to_path_buf();
            for component in target.components() {
                match component {
                    std::path::Component::Normal(c) => resolved.push(c),
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        anyhow::ensure!(resolved.pop(), "oracle symlink escapes bundle: {name}");
                    }
                    _ => anyhow::bail!("oracle symlink escapes bundle: {name}"),
                }
            }
            current = resolved;
        }
    }
    // No file (including a symlink) can serve as another entry's directory.
    for name in entries.keys() {
        for parent in Path::new(name).ancestors().skip(1) {
            anyhow::ensure!(
                !entries.contains_key(parent.to_str().unwrap_or("")),
                "oracle file used as directory"
            );
        }
    }
    Ok(())
}

pub(super) fn disk_entries(
    root: &Path,
    dir: &Path,
    entries: &mut BTreeMap<String, Entry>,
) -> Result<()> {
    for child in std::fs::read_dir(dir)? {
        let child = child?;
        let path = child.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            disk_entries(root, &path, entries)?;
        } else {
            let (mode, bytes) = if metadata.file_type().is_symlink() {
                let target = std::fs::read_link(&path)?;
                (
                    "120000",
                    target
                        .to_str()
                        .context("invalid cached symlink")?
                        .as_bytes()
                        .to_vec(),
                )
            } else {
                anyhow::ensure!(
                    metadata.is_file() && metadata.nlink() == 1,
                    "unsafe cached oracle file"
                );
                (
                    if metadata.permissions().mode() & 0o111 != 0 {
                        "100755"
                    } else {
                        "100644"
                    },
                    std::fs::read(&path)?,
                )
            };
            entries.insert(
                path.strip_prefix(root)?
                    .to_str()
                    .context("invalid cached oracle path")?
                    .into(),
                Entry {
                    mode: mode.into(),
                    bytes,
                },
            );
        }
    }
    Ok(())
}
use std::os::unix::fs::MetadataExt;

// Registered worktrees may be prunable because their directory disappeared.
// Preserve their reserved path in overlap checks without requiring that an
// absent checkout exists. Resolve every existing ancestor; dangling redirects
// and errors other than absence still refuse.
fn registered_worktree_path(path: &Path) -> Result<PathBuf> {
    let mut current = path;
    let mut missing = Vec::new();
    loop {
        match current.canonicalize() {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(current).is_ok_and(|m| m.file_type().is_symlink()) {
                    anyhow::bail!(
                        "registered worktree has a dangling redirect: {}",
                        current.display()
                    );
                }
                missing.push(
                    current
                        .file_name()
                        .context("registered worktree has no existing ancestor")?
                        .to_os_string(),
                );
                current = current
                    .parent()
                    .context("registered worktree has no parent")?;
            }
            Err(error) => {
                return Err(error).context("resolve registered worktree for oracle isolation")
            }
        }
    }
}

impl OracleBundle {
    #[cfg(not(test))]
    pub(super) fn freeze_guards(project: &Path, trial: &Path, tools: &[String]) -> Result<Self> {
        let cache = match std::env::var_os("XDG_CACHE_HOME") {
            Some(path) => PathBuf::from(path),
            None => aida_core::home::home_dir()
                .context("cannot resolve guard cache home")?
                .join(".cache"),
        };
        Self::freeze_guards_in_cache(project, trial, tools, &cache)
    }

    pub(super) fn freeze_guards_in_cache(
        project: &Path,
        trial: &Path,
        tools: &[String],
        cache: &Path,
    ) -> Result<Self> {
        let mut entries = BTreeMap::new();
        for tool in tools {
            // These names come from validated trusted config. The generated
            // script never searches PATH; absolute system delegation is flagged.
            let executable = ["/usr/local/bin", "/usr/bin", "/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(tool))
                .find(|path| {
                    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
                });
            let action = match executable {
                Some(path) => format!(
                    "exec '{}' \"$@\"",
                    path.to_string_lossy().replace('\'', "'\"'\"'")
                ),
                None => "exit 127".into(),
            };
            let script = format!("#!/bin/sh\nprintf '%s\\n' '{tool}' > \"$AIDA_REGEN_GUARD_PIPE\" || exit 98\n{action}\n");
            entries.insert(
                format!("bin/{tool}"),
                Entry {
                    mode: "100755".into(),
                    bytes: script.into_bytes(),
                },
            );
        }
        // Empty tool sets still need an unambiguous, nonempty strict allowlist
        // and a content-addressed directory that can be verified normally.
        entries.insert(
            ".aida-guard-format".into(),
            Entry {
                mode: "100644".into(),
                bytes: b"guard-fifo-v1\n".to_vec(),
            },
        );
        let hash = hash(&entries);
        let bundle = Self::materialize(project, "guards".into(), hash, entries, cache, trial)?;
        std::fs::create_dir_all(bundle.root.join("bin"))?;
        bundle.verify()?;
        Ok(bundle)
    }

    pub(super) fn freeze(
        project: &Path,
        sha: String,
        paths: &[String],
        trial: &Path,
    ) -> Result<Self> {
        let cache = match std::env::var_os("XDG_CACHE_HOME") {
            Some(path) => PathBuf::from(path),
            None => aida_core::home::home_dir()
                .context("cannot resolve oracle cache home")?
                .join(".cache"),
        };
        Self::freeze_in_cache(project, sha, paths, trial, &cache)
    }

    pub(super) fn freeze_in_cache(
        project: &Path,
        sha: String,
        paths: &[String],
        trial: &Path,
        cache: &Path,
    ) -> Result<Self> {
        let entries = frozen_entries(project, &sha, paths)?;
        let hash = hash(&entries);
        Self::materialize(project, sha, hash, entries, cache, trial)
    }

    fn materialize(
        project: &Path,
        sha: String,
        hash: String,
        entries: BTreeMap<String, Entry>,
        cache: &Path,
        trial: &Path,
    ) -> Result<Self> {
        anyhow::ensure!(cache.is_absolute(), "oracle cache must be absolute");
        std::fs::create_dir_all(cache)?;
        let cache = cache.canonicalize()?;
        let trial = trial.canonicalize()?;
        anyhow::ensure!(
            !["/tmp", "/dev", "/proc"]
                .iter()
                .any(|p| cache.starts_with(p)),
            "oracle cache must be outside /tmp, /dev and /proc (hidden by confinement)"
        );
        let root = cache.join("aida/regen/oracle").join(&hash);
        // Check every registered checkout, including this project's source.
        let worktrees = super::super::git(project, &["worktree", "list", "--porcelain", "-z"])?;
        for field in worktrees.split(|b| *b == 0) {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                let path = registered_worktree_path(Path::new(std::str::from_utf8(path)?))?;
                anyhow::ensure!(
                    !root.starts_with(&path) && !path.starts_with(&root),
                    "oracle bundle overlaps a project worktree"
                );
            }
        }
        anyhow::ensure!(
            !root.starts_with(&trial) && !trial.starts_with(&root),
            "oracle bundle overlaps trial root"
        );
        std::fs::create_dir_all(root.parent().unwrap())?;
        // Also reject a cache inside an unrelated repository's checkout;
        // this project's worktree inventory cannot enumerate other projects.
        for ancestor in root.parent().unwrap().ancestors() {
            match std::fs::symlink_metadata(ancestor.join(".git")) {
                Ok(_) => anyhow::bail!("oracle cache must be outside every worktree"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("cannot verify external oracle cache"),
            }
        }
        // Ancestor symlinks must not redirect the supposedly external cache.
        anyhow::ensure!(
            root.parent().unwrap().canonicalize()? == root.parent().unwrap(),
            "oracle cache has redirected ancestors"
        );
        if !root.try_exists()? {
            let staging = root
                .parent()
                .unwrap()
                .join(format!(".staging-{}", Uuid::new_v4()));
            std::fs::DirBuilder::new().mode(0o700).create(&staging)?;
            let result = (|| -> Result<()> {
                write_entries(&staging, &entries)?;
                match std::fs::rename(&staging, &root) {
                    Ok(()) => Ok(()),
                    Err(_) if root.try_exists()? => Ok(()), // concurrent publisher; verify below
                    Err(e) => Err(e.into()),
                }
            })();
            let _ = std::fs::remove_dir_all(&staging);
            result?;
        }
        let bundle = Self { root, sha, hash };
        bundle.verify()?;
        Ok(bundle)
    }

    pub(super) fn verify(&self) -> Result<()> {
        anyhow::ensure!(
            !std::fs::symlink_metadata(&self.root)?
                .file_type()
                .is_symlink(),
            "oracle bundle root is a symlink"
        );
        anyhow::ensure!(
            self.root.canonicalize()? == self.root,
            "oracle bundle redirected"
        );
        let mut entries = BTreeMap::new();
        disk_entries(&self.root, &self.root, &mut entries)?;
        anyhow::ensure!(
            hash(&entries) == self.hash,
            "oracle bundle hash mismatch; scoring refused"
        );
        Ok(())
    }

    pub(super) fn command(&self, argv: &[String]) -> Result<Vec<String>> {
        self.verify()?;
        let executable = self.root.join(relative(&argv[0])?);
        let resolved = executable
            .canonicalize()
            .context("oracle executable missing from bundle")?;
        anyhow::ensure!(
            resolved.starts_with(&self.root) && resolved.is_file(),
            "oracle executable escapes bundle"
        );
        // env is a fixed system executable, never selected through trial PATH.
        anyhow::ensure!(
            Path::new("/usr/bin/env").is_file(),
            "coreutils /usr/bin/env required for oracle cwd"
        );
        let mut command = vec![
            "/usr/bin/env".into(),
            format!("--chdir={}", self.root.display()),
            resolved.to_str().context("invalid oracle path")?.into(),
        ];
        command.extend_from_slice(&argv[1..]);
        Ok(command)
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Fixture {
        repo: TempDir,
        trial: TempDir,
        cache: TempDir,
        sha: String,
    }
    impl Fixture {
        fn new(script: &str) -> Self {
            let repo = TempDir::new().unwrap();
            let git = |args: &[&str]| super::super::super::git(repo.path(), args).unwrap();
            git(&["init", "-b", "main"]);
            git(&["config", "user.name", "Test"]);
            git(&["config", "user.email", "test@example.com"]);
            std::fs::create_dir(repo.path().join("tests")).unwrap();
            let oracle = repo.path().join("tests/oracle.sh");
            std::fs::write(&oracle, script).unwrap();
            std::fs::set_permissions(oracle, std::fs::Permissions::from_mode(0o755)).unwrap();
            git(&["add", "."]);
            git(&["commit", "-m", "trusted oracle"]);
            let sha = String::from_utf8(git(&["rev-parse", "HEAD"]))
                .unwrap()
                .trim()
                .to_owned();
            // Real confinement hides /tmp RO binds: use an external cache.
            Self {
                repo,
                trial: TempDir::new().unwrap(),
                cache: tempfile::tempdir_in("/var/tmp").unwrap(),
                sha,
            }
        }
        fn bundle(&self, paths: &[&str]) -> Result<OracleBundle> {
            let paths = paths.iter().map(|s| (*s).into()).collect::<Vec<_>>();
            let entries = frozen_entries(self.repo.path(), &self.sha, &paths)?;
            OracleBundle::materialize(
                self.repo.path(),
                self.sha.clone(),
                hash(&entries),
                entries,
                self.cache.path(),
                self.trial.path(),
            )
        }
    }

    #[test]
    fn trusted_fail_cannot_be_shadowed_by_trial_or_dirty_source() {
        let f = Fixture::new("#!/bin/sh\ntest \"$PWD\" != \"$AIDA_REGEN_TRIAL_ROOT\" || exit 5\nprintf 'FAIL - held-out behavior\\n'\nexit 1\n");
        std::fs::create_dir(f.trial.path().join("tests")).unwrap();
        let fake = "#!/bin/sh\nprintf 'ok   - held-out behavior\\n'\n";
        std::fs::write(f.trial.path().join("tests/oracle.sh"), fake).unwrap();
        std::fs::set_permissions(
            f.trial.path().join("tests/oracle.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let substituted = Command::new(f.trial.path().join("tests/oracle.sh"))
            .output()
            .unwrap();
        assert!(substituted.status.success());
        assert!(parse_checks(std::str::from_utf8(&substituted.stdout).unwrap()).unwrap()[0].passed);
        std::fs::write(f.repo.path().join("tests/oracle.sh"), fake).unwrap();
        let bundle = f.bundle(&["tests/"]).unwrap();
        let cfg = RegenConfig {
            oracle_command: vec!["./tests/oracle.sh".into()],
            oracle_paths: vec!["tests/".into()],
            reference_repeats: 3,
            install_command: None,
            required_tools: vec![],
        };
        let confined = matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        );
        let env = ScoreEnv::new(&cfg, !confined).unwrap();
        let record = run_trial_in_env(
            &cfg,
            f.trial.path(),
            super::super::tests::manifest(),
            "m",
            "v",
            "h",
            !confined,
            env,
            &bundle,
            None,
        )
        .unwrap();
        assert_eq!(record.status, "scored", "{:?}", record.error);
        assert_eq!(record.aggregate.unwrap().passed, 0);
        assert_eq!(record.oracle_exit, Some(1));
        assert_eq!(record.oracle_sha, f.sha);
        assert_eq!(record.oracle_bundle_sha256, bundle.hash);
    }

    #[test]
    fn tampered_cached_bundle_refuses_before_execution_or_reuse() {
        let f = Fixture::new("#!/bin/sh\necho 'FAIL - real'\n");
        let bundle = f.bundle(&["tests"]).unwrap();
        std::fs::write(
            bundle.root.join("tests/oracle.sh"),
            "#!/bin/sh\necho 'ok   - fake'\n",
        )
        .unwrap();
        assert!(bundle
            .command(&["tests/oracle.sh".into()])
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
        assert!(f.bundle(&["tests"]).is_err());
    }

    #[test]
    fn missing_registered_worktree_remains_in_overlap_checks() {
        let f = Fixture::new("#!/bin/sh\nexit 0\n");
        let parent = TempDir::new().unwrap();
        let stale = parent.path().join("stale");
        super::super::super::git(
            f.repo.path(),
            &["worktree", "add", "-b", "stale", stale.to_str().unwrap()],
        )
        .unwrap();
        std::fs::remove_dir_all(&stale).unwrap();
        assert_eq!(registered_worktree_path(&stale).unwrap(), stale);
        assert!(
            f.bundle(&["tests"]).is_ok(),
            "a missing unrelated checkout must not prevent an external bundle"
        );
        // Recreating its path as a cache is still an overlap with that reserved
        // checkout, even though no .git file was recreated there.
        let nested_cache = stale.join("cache");
        assert!(OracleBundle::freeze_in_cache(
            f.repo.path(),
            f.sha.clone(),
            &["tests".into()],
            f.trial.path(),
            &nested_cache
        )
        .is_err());
        let dangling = parent.path().join("dangling");
        symlink(parent.path().join("absent"), &dangling).unwrap();
        assert!(registered_worktree_path(&dangling.join("child")).is_err());
    }

    #[test]
    fn path_and_symlink_escapes_and_omitted_targets_refuse() {
        let mut f = Fixture::new("#!/bin/sh\nexit 0\n");
        for path in ["../tests", "tests/../tests", "/tests", "", "."] {
            assert!(f.bundle(&[path]).is_err(), "accepted {path}");
        }
        assert!(f.bundle(&[]).is_err());
        assert!(f.bundle(&["missing"]).is_err());
        assert!(f.bundle(&["tests*"]).is_err(), "paths are not globs");
        for target in ["../../outside", "/bin/sh", "omitted"] {
            let link = f.repo.path().join("tests/link");
            let _ = std::fs::remove_file(&link);
            symlink(target, &link).unwrap();
            let git = |args: &[&str]| super::super::super::git(f.repo.path(), args).unwrap();
            git(&["add", "."]);
            git(&["commit", "-m", "symlink"]);
            f.sha = String::from_utf8(git(&["rev-parse", "HEAD"]))
                .unwrap()
                .trim()
                .into();
            assert!(f.bundle(&["tests"]).is_err(), "accepted {target}");
        }
    }

    #[test]
    fn internal_file_symlink_is_frozen_and_verified() {
        let mut f = Fixture::new("#!/bin/sh\nexit 0\n");
        symlink("oracle.sh", f.repo.path().join("tests/link")).unwrap();
        super::super::super::git(f.repo.path(), &["add", "."]).unwrap();
        super::super::super::git(f.repo.path(), &["commit", "-m", "internal link"]).unwrap();
        f.sha = String::from_utf8(
            super::super::super::git(f.repo.path(), &["rev-parse", "HEAD"]).unwrap(),
        )
        .unwrap()
        .trim()
        .into();
        let bundle = f.bundle(&["tests"]).unwrap();
        assert!(bundle.command(&["tests/link".into()]).unwrap()[2].ends_with("tests/oracle.sh"));
        symlink("/bin/sh", bundle.root.join("extra")).unwrap();
        assert!(bundle.verify().is_err());
    }

    #[test]
    fn bundle_and_trial_cannot_contain_each_other_and_command_never_uses_path() {
        let f = Fixture::new("#!/bin/sh\nexit 0\n");
        let bundle = f.bundle(&["tests"]).unwrap();
        for executable in ["/bin/sh", "../tests/oracle.sh", "sh"] {
            assert!(bundle.command(&[executable.into()]).is_err());
        }
        let entries = frozen_entries(f.repo.path(), &f.sha, &["tests".into()]).unwrap();
        assert!(OracleBundle::materialize(
            f.repo.path(),
            f.sha.clone(),
            hash(&entries),
            entries,
            f.cache.path(),
            f.cache.path()
        )
        .unwrap_err()
        .to_string()
        .contains("overlaps trial"));
        let nested_trial = bundle.root.join("nested-trial");
        std::fs::create_dir(&nested_trial).unwrap();
        let entries = frozen_entries(f.repo.path(), &f.sha, &["tests".into()]).unwrap();
        assert!(OracleBundle::materialize(
            f.repo.path(),
            f.sha.clone(),
            hash(&entries),
            entries,
            f.cache.path(),
            &nested_trial
        )
        .unwrap_err()
        .to_string()
        .contains("overlaps trial"));
    }

    #[test]
    fn cache_inside_unrelated_worktree_is_refused() {
        let f = Fixture::new("#!/bin/sh\nexit 0\n");
        super::super::super::git(f.cache.path(), &["init", "-b", "main"]).unwrap();
        assert!(f
            .bundle(&["tests"])
            .unwrap_err()
            .to_string()
            .contains("outside every worktree"));
    }

    #[test]
    fn trial_cannot_write_read_only_oracle_bundle() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let f = Fixture::new("#!/bin/sh\nexit 0\n");
        let bundle = f.bundle(&["tests"]).unwrap();
        let cfg = RegenConfig {
            oracle_command: vec!["tests/oracle.sh".into()],
            oracle_paths: vec!["tests".into()],
            reference_repeats: 3,
            install_command: None,
            required_tools: vec![],
        };
        let env = ScoreEnv::new(&cfg, false).unwrap();
        let result = env
            .run_with_bundle(
                f.trial.path(),
                &[
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf tampered > \"$1/tests/oracle.sh\"".into(),
                    "write-probe".into(),
                    bundle.root.to_str().unwrap().into(),
                ],
                Some(&bundle),
            )
            .unwrap();
        assert!(!result.status.success(), "trial wrote held-out oracle");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("Read-only file system"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        bundle.verify().unwrap();
    }
}

// trace:TASK-1731 | ai:codex
pub(super) fn write_entries(root: &Path, entries: &BTreeMap<String, Entry>) -> Result<()> {
    for (name, entry) in entries {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        if entry.mode == "120000" {
            symlink(std::str::from_utf8(&entry.bytes)?, &path)?;
        } else {
            std::fs::write(&path, &entry.bytes)?;
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if entry.mode == "100755" { 0o755 } else { 0o644 }),
            )?;
        }
    }
    Ok(())
}
