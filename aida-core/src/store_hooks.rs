//! Host-owned hooks for git commands over sandbox-writable store trees.
// trace:BUG-1929 | ai:codex
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const HOOKS: &[(&str, &str)] = &[
    ("pre-commit", "hooks/aida-pre-commit.sh"),
    ("commit-msg", "hooks/aida-commit-msg"),
    ("prepare-commit-msg", "hooks/aida-store-pair.sh"),
    ("post-commit", "hooks/aida-post-commit.sh"),
    ("pre-push", "hooks/aida-store-pre-push.sh"),
];

/// Strip cwd-dependent executable lookup, including absolute paths into the store.
pub(crate) fn safe_path(store: &Path, path: &std::ffi::OsStr) -> std::ffi::OsString {
    let store = std::fs::canonicalize(store).unwrap_or_else(|_| store.to_path_buf());
    let safe = std::env::join_paths(std::env::split_paths(path).filter(|p| {
        p.is_absolute() && std::fs::canonicalize(p).is_ok_and(|real| !real.starts_with(&store))
    }))
    .unwrap_or_default();
    // Empty PATH is itself a cwd search on Unix. Never hand it to git or its
    // shell hooks when all caller entries were discarded. Linux-only system
    // directories are outside the writable store and provide a fixed fallback.
    if safe.is_empty() {
        std::ffi::OsString::from("/usr/bin:/bin")
    } else {
        safe
    }
}

/// Never repair a suspicious existing profile. Versioned directories let a new
/// binary install its own template masters without overwriting another build.
#[cfg(unix)]
pub(crate) fn establish(common: &Path, store: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let fail = || {
        format!(
        "refusing AIDA store write: cannot establish verified host-owned AIDA hooks in {}. \
         Check ownership, permissions, symlinks and contents; project hooks cannot run on store commits",
        common.display()
    )
    };
    let result = (|| -> Result<PathBuf> {
        // SAFETY: geteuid has no arguments or memory-safety preconditions.
        let uid = unsafe { libc::geteuid() };
        // A writable ancestor could replace even a read-only hooks directory.
        // Root-owned sticky temporary roots protect each user-owned child.
        for ancestor in common.ancestors() {
            let meta = std::fs::symlink_metadata(ancestor)?;
            anyhow::ensure!(
                meta.is_dir()
                    && !meta.file_type().is_symlink()
                    && (meta.uid() == uid || meta.uid() == 0)
                    && (meta.mode() & 0o022 == 0 || (meta.uid() == 0 && meta.mode() & 0o1000 != 0)),
                "unsafe hooks authority ancestor {}",
                ancestor.display()
            );
        }
        let common_meta = std::fs::symlink_metadata(common)?;
        anyhow::ensure!(
            common_meta.is_dir() && common_meta.uid() == uid && common_meta.mode() & 0o022 == 0,
            "git common directory is not owned exclusively by this user"
        );
        let store_real = std::fs::canonicalize(store)?;
        let common_real = std::fs::canonicalize(common)?;
        // Own-repository stores are allowed only in their denied .git subtree.
        anyhow::ensure!(
            !common_real.starts_with(&store_real) || common_real == store_real.join(".git"),
            "hooks authority is inside the store's writable files"
        );
        let mut hash = Sha256::new();
        for (name, template) in HOOKS {
            hash.update(name.as_bytes());
            hash.update(crate::templates::EMBEDDED_TEMPLATES[*template].as_bytes());
        }
        let dir = common.join(format!("aida-store-hooks-{:x}", hash.finalize()));
        match std::fs::symlink_metadata(&dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let scratch =
                    common.join(format!("aida-store-hooks-install-{}", uuid::Uuid::new_v4()));
                std::fs::DirBuilder::new().mode(0o700).create(&scratch)?;
                let installed = (|| -> Result<()> {
                    for (name, template) in HOOKS {
                        let path = scratch.join(name);
                        std::fs::write(&path, crate::templates::EMBEDDED_TEMPLATES[*template])?;
                        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500))?;
                    }
                    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
                    // Concurrent installers publish a complete directory; a loser
                    // verifies the winner rather than changing an installed profile.
                    if let Err(e) = std::fs::rename(&scratch, &dir) {
                        if !dir.exists() {
                            return Err(e.into());
                        }
                    }
                    Ok(())
                })();
                if scratch.exists() {
                    let _ =
                        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700));
                    let _ = std::fs::remove_dir_all(&scratch);
                }
                installed?;
            }
            Err(e) => return Err(e.into()),
        }
        let meta = std::fs::symlink_metadata(&dir)?;
        anyhow::ensure!(
            meta.is_dir()
                && !meta.file_type().is_symlink()
                && meta.uid() == uid
                && meta.mode() & 0o022 == 0,
            "unsafe hooks directory"
        );
        let entries = std::fs::read_dir(&dir)?.collect::<std::io::Result<Vec<_>>>()?;
        anyhow::ensure!(
            entries.len() == HOOKS.len(),
            "unexpected hooks directory contents"
        );
        for (name, template) in HOOKS {
            let path = dir.join(name);
            let meta = std::fs::symlink_metadata(&path)?;
            anyhow::ensure!(
                meta.is_file()
                    && !meta.file_type().is_symlink()
                    && meta.uid() == uid
                    && meta.mode() & 0o022 == 0
                    && meta.mode() & 0o100 != 0
                    && meta.nlink() == 1,
                "unsafe hook {}",
                path.display()
            );
            anyhow::ensure!(
                std::fs::read(&path)? == crate::templates::EMBEDDED_TEMPLATES[*template].as_bytes(),
                "wrong hook contents in {}",
                path.display()
            );
        }
        Ok(dir)
    })();
    result.with_context(fail)
}

#[cfg(not(unix))]
pub(crate) fn establish(_common: &Path, _store: &Path) -> Result<PathBuf> {
    anyhow::bail!("refusing AIDA store write: host-owned hooks require Linux")
}

/// Git added GIT_ATTR_SOURCE in 2.40. No repository/config lookup is needed.
pub(crate) fn supports_attr_source(store: &Path) -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        // raw-git-ok(BUG-1929): --version reads no repository and uses sanitized PATH.
        std::process::Command::new("git")
            .env(
                "PATH",
                safe_path(store, &std::env::var_os("PATH").unwrap_or_default()),
            )
            .arg("--version")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|s| s.strip_prefix("git version ").map(str::to_owned))
            .and_then(|v| {
                let mut parts = v.split('.');
                Some((
                    parts.next()?.parse::<u32>().ok()?,
                    parts.next()?.parse::<u32>().ok()?,
                ))
            })
            .is_some_and(|(major, minor)| (major, minor) >= (2, 40))
    })
}
