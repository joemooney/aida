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

/// True when no one but the owner (and root) can write a directory. World-write
/// always fails. Group-write passes only for a directory this user owns whose
/// group is this user's own primary group: the user-private-group default
/// (umask 002, dirs 0775 user:user) grants nobody else write. Refusing that
/// layout blocked every store write on such hosts.
// trace:BUG-1929 | ai:claude
#[cfg(unix)]
pub(crate) fn only_owner_may_write(owner: u32, group: u32, mode: u32, uid: u32, gid: u32) -> bool {
    mode & 0o002 == 0 && (mode & 0o020 == 0 || (owner == uid && group == gid))
}

/// Whether `/proc/self/uid_map` content is the initial (identity) user
/// namespace: exactly one mapping `0 0 4294967295`, whitespace-insensitive.
/// Unreadable or malformed content counts as identity, so it never relaxes
/// the ancestor-owner check.
// trace:BUG-1929 | ai:claude
pub(crate) fn uid_map_is_identity(map: &str) -> bool {
    let mut lines = Vec::new();
    for line in map.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let parsed: Option<Vec<u32>> = fields.iter().map(|f| f.parse::<u32>().ok()).collect();
        match parsed {
            Some(v) if v.len() == 3 => lines.push(v),
            _ => return true,
        }
    }
    lines.is_empty() || lines == [vec![0, 0, u32::MAX]]
}

/// The kernel overflow uid when this process runs in a non-identity user
/// namespace, else `None`. Read from `/proc/sys/kernel/overflowuid`,
/// falling back to 65534.
// trace:BUG-1929 | ai:claude
#[cfg(unix)]
fn namespace_overflow_uid() -> Option<u32> {
    let map = std::fs::read_to_string("/proc/self/uid_map").ok()?;
    if uid_map_is_identity(&map) {
        return None;
    }
    Some(
        std::fs::read_to_string("/proc/sys/kernel/overflowuid")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(65534),
    )
}

/// Owner and mode rule for one ancestor of the git common directory. The
/// user's own directories must be writable only by the user. Root-owned
/// ones, and overflow-uid ones only when `namespace_overflow` is `Some`
/// (a non-identity user namespace), must have no group/other write unless
/// sticky. Any other owner refuses. `Err` names the failed check.
// trace:BUG-1929 | ai:claude
#[cfg(unix)]
pub(crate) fn check_ancestor(
    owner: u32,
    group: u32,
    mode: u32,
    uid: u32,
    gid: u32,
    namespace_overflow: Option<u32>,
) -> std::result::Result<(), String> {
    let sticky = mode & 0o1000 != 0;
    if owner == uid {
        if only_owner_may_write(owner, group, mode, uid, gid) || (owner == 0 && sticky) {
            return Ok(());
        }
    } else if owner == 0 || namespace_overflow == Some(owner) {
        if only_owner_may_write(owner, group, mode, uid, gid) || sticky {
            return Ok(());
        }
    } else {
        return Err(format!("has untrusted owner uid {owner}"));
    }
    Err(format!(
        "(owner uid {owner}) is group- or world-writable (mode {:o})",
        mode & 0o7777
    ))
}

/// Never repair a suspicious existing profile. Versioned directories let a new
/// binary install its own template masters without overwriting another build.
#[cfg(unix)]
pub(crate) fn establish(common: &Path, store: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let result = (|| -> Result<PathBuf> {
        // SAFETY: geteuid/getegid have no arguments or memory-safety preconditions.
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        // A writable ancestor could replace even a read-only hooks directory.
        // Root-owned sticky temporary roots protect each user-owned child.
        // Inside a non-identity user namespace (bwrap) root-owned ancestors
        // such as / and /home read back as the kernel overflow uid.
        // trace:BUG-1929 | ai:claude
        let overflow = namespace_overflow_uid();
        for ancestor in common.ancestors() {
            let meta = std::fs::symlink_metadata(ancestor).with_context(|| {
                format!(
                    "cannot inspect store hooks dir ancestor {}",
                    ancestor.display()
                )
            })?;
            anyhow::ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "store hooks dir ancestor {} is not a real directory",
                ancestor.display()
            );
            if let Err(reason) =
                check_ancestor(meta.uid(), meta.gid(), meta.mode(), uid, gid, overflow)
            {
                anyhow::bail!("store hooks dir ancestor {} {reason}", ancestor.display());
            }
        }
        let common_meta = std::fs::symlink_metadata(common)?;
        anyhow::ensure!(
            common_meta.is_dir()
                && common_meta.uid() == uid
                && only_owner_may_write(
                    common_meta.uid(),
                    common_meta.gid(),
                    common_meta.mode(),
                    uid,
                    gid
                ),
            "git common directory {} is not owned exclusively by this user",
            common.display()
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
            "unsafe hooks directory {} (not a directory, symlink, wrong owner or group/world-writable)",
            dir.display()
        );
        let entries = std::fs::read_dir(&dir)?.collect::<std::io::Result<Vec<_>>>()?;
        anyhow::ensure!(
            entries.len() == HOOKS.len(),
            "unexpected hooks directory contents in {}",
            dir.display()
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
    // Name the failed check in the top-level message: callers and the
    // refusal shim show only this line, never the error chain.
    // trace:BUG-1929 | ai:claude
    result.map_err(|e| {
        anyhow::anyhow!(
            "refusing AIDA store write: cannot establish verified host-owned AIDA hooks in {}: {e:#}. \
             Check ownership, permissions, symlinks and contents; project hooks cannot run on store commits",
            common.display()
        )
    })
}

#[cfg(not(unix))]
pub(crate) fn establish(_common: &Path, _store: &Path) -> Result<PathBuf> {
    anyhow::bail!("refusing AIDA store write: host-owned hooks require Linux")
}

/// First git release that honours `GIT_ATTR_SOURCE`.
pub(crate) const MIN_ATTR_SOURCE_GIT: (u32, u32) = (2, 40);

/// `(major, minor)` from `git --version` output, with or without its
/// `git version ` prefix (`2.43.0.windows.1`, `2.40.1 (Apple Git-143)`).
/// `None` when the text is not a version.
// trace:BUG-2131 | ai:claude
pub(crate) fn parse_git_version(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let version = text.strip_prefix("git version ").unwrap_or(text);
    let mut parts = version.split_whitespace().next()?.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor))
}

/// The version gate: only a parsed version at or above 2.40 is safe.
/// Unparseable output is unsafe.
// trace:BUG-2131 | ai:claude
pub(crate) fn attr_source_supported_by(version_output: &str) -> bool {
    parse_git_version(version_output).is_some_and(|v| v >= MIN_ATTR_SOURCE_GIT)
}

// Test seam: the `git --version` text the gate sees on this thread, so a
// test can fake an old git without touching PATH. Set only through
// `git_ops::with_host_git_version`; no environment variable or file selects it.
// trace:BUG-2131 | ai:claude
thread_local! {
    pub(crate) static FAKE_GIT_VERSION: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Host `git --version` output, run once per process. Empty when git
/// cannot be run or fails (which the gate treats as unsafe).
// trace:BUG-2131 | ai:claude
fn host_git_version(store: &Path) -> String {
    if let Some(fake) = FAKE_GIT_VERSION.with(|f| f.borrow().clone()) {
        return fake;
    }
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| {
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
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_default()
        })
        .clone()
}

/// Whether host git honours `GIT_ATTR_SOURCE` (git 2.40+).
pub(crate) fn supports_attr_source(store: &Path) -> bool {
    attr_source_supported_by(&host_git_version(store))
}

/// Human description of the host git version for refusal and warning text.
fn describe_git_version(output: &str) -> String {
    if output.is_empty() {
        "could not be determined (`git --version` failed)".to_string()
    } else if parse_git_version(output).is_none() {
        format!("could not be determined (`git --version` printed {output:?})")
    } else {
        format!(
            "is {}",
            output.strip_prefix("git version ").unwrap_or(output)
        )
    }
}

/// Refuse a contained launch (or enabling contained mode) when host git
/// ignores `GIT_ATTR_SOURCE`: a contained session writes the store, and
/// below 2.40 host store git cannot keep its `.gitattributes` from selecting
/// a host filter/textconv driver. Unparseable versions count as old.
// trace:BUG-2131 | ai:claude
pub(crate) fn ensure_git_supports_contained() -> Result<()> {
    let output = host_git_version(Path::new("/"));
    if attr_source_supported_by(&output) {
        return Ok(());
    }
    let (major, minor) = MIN_ATTR_SOURCE_GIT;
    anyhow::bail!(
        "refusing contained mode: host git {}, and git older than {major}.{minor} ignores \
         GIT_ATTR_SOURCE, so a contained session could plant a store .gitattributes that \
         selects a host filter/textconv driver. Upgrade git to {major}.{minor} or newer",
        describe_git_version(&output)
    )
}

/// The host-owned marker a contained launch leaves in the store's git common
/// directory, which the sandbox cannot write.
pub(crate) const CONTAINED_MARKER: &str = "aida-contained-store";

/// Absolute git common directory of `store`, or `None`.
pub(crate) fn git_common_dir(store: &Path) -> Option<PathBuf> {
    // Read-only lookup the hardened builder depends on (BUG-2131).
    // raw-git-ok(BUG-1929): read-only rev-parse with sanitized PATH.
    let out = std::process::Command::new("git")
        .env(
            "PATH",
            safe_path(store, &std::env::var_os("PATH").unwrap_or_default()),
        )
        .current_dir(store)
        .args(["-c", "core.fsmonitor=false"])
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    let dir = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}

/// `[agents] contained` / `[contained] enable` for a project, with
/// `load_agents_contained`'s precedence (global agents.toml, then project
/// agents.toml, then project config.toml; last set wins). A file that exists
/// but cannot be read or parsed counts as contained: this decides a refusal.
// trace:BUG-2131 | ai:claude
pub(crate) fn contained_configured_from(
    global_agents: Option<&Path>,
    project_agents: &Path,
    project_config: &Path,
) -> bool {
    // Some(Some(v)) = set, Some(None) = unset, None = unreadable.
    let read = |path: &Path, section: &str, key: &str| -> Option<Option<bool>> {
        match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(None),
            Err(_) => None,
            Ok(body) => {
                let value: toml::Value = toml::from_str(&body).ok()?;
                Some(
                    value
                        .get(section)
                        .and_then(|t| t.get(key))
                        .and_then(toml::Value::as_bool),
                )
            }
        }
    };
    let mut contained = false;
    let sources = [
        global_agents.map(|p| (p, "agents", "contained")),
        Some((project_agents, "agents", "contained")),
        Some((project_config, "contained", "enable")),
    ];
    for (path, section, key) in sources.into_iter().flatten() {
        match read(path, section, key) {
            None => return true,
            Some(Some(v)) => contained = v,
            Some(None) => {}
        }
    }
    contained
}

/// Whether a contained (untrusted) writer may exist for this store:
/// `AIDA_CONTAINED` in this process, a contained-launch marker in the git
/// common directory, or contained mode configured for the owning project.
// trace:BUG-2131 | ai:claude
pub(crate) fn store_contained_enabled(store: &Path) -> bool {
    if std::env::var("AIDA_CONTAINED").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes")) {
        return true;
    }
    if git_common_dir(store).is_some_and(|c| c.join(CONTAINED_MARKER).exists()) {
        return true;
    }
    let global = crate::home::home_dir().map(|h| h.join(".aida").join("agents.toml"));
    let abs = std::path::absolute(store).unwrap_or_else(|_| store.to_path_buf());
    let canon = std::fs::canonicalize(store).ok();
    [Some(abs), canon]
        .into_iter()
        .flatten()
        .filter(|p| p.file_name().is_some_and(|n| n == ".aida-store"))
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .any(|root| {
            contained_configured_from(
                global.as_deref(),
                &root.join(".aida").join("agents.toml"),
                &root.join(".aida").join("config.toml"),
            )
        })
}

/// What store git may do on this host's git.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AttrSourceVerdict {
    /// git honours GIT_ATTR_SOURCE.
    Supported,
    /// Old git, but no contained writer: keep working, warn once.
    WarnOnly(String),
    /// Old git with contained mode in play: refuse.
    Refuse(String),
}

/// Classify store git for `store` (BUG-2131). Supported git costs nothing
/// beyond the cached version; only old git looks for a contained writer.
// trace:BUG-2131 | ai:claude
pub(crate) fn attr_source_verdict(store: &Path) -> AttrSourceVerdict {
    let output = host_git_version(store);
    if attr_source_supported_by(&output) {
        return AttrSourceVerdict::Supported;
    }
    let (major, minor) = MIN_ATTR_SOURCE_GIT;
    let found = describe_git_version(&output);
    if store_contained_enabled(store) {
        AttrSourceVerdict::Refuse(format!(
            "refusing AIDA store git: contained sessions are enabled for this store and host \
             git {found}; git older than {major}.{minor} ignores GIT_ATTR_SOURCE, so the \
             store's .gitattributes could select a host filter/textconv driver that runs \
             outside any sandbox. Upgrade git to {major}.{minor} or newer"
        ))
    } else {
        AttrSourceVerdict::WarnOnly(format!(
            "warning: host git {found}; contained sessions are unavailable until git is \
             {major}.{minor} or newer"
        ))
    }
}

/// Refuse host git over a store tree when git ignores `GIT_ATTR_SOURCE` and a
/// contained writer may exist; otherwise warn once per process and continue.
// trace:BUG-2131 | ai:claude
pub(crate) fn ensure_attr_source(store: &Path) -> Result<()> {
    match attr_source_verdict(store) {
        AttrSourceVerdict::Supported => Ok(()),
        AttrSourceVerdict::WarnOnly(warning) => {
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("{warning}");
            }
            Ok(())
        }
        AttrSourceVerdict::Refuse(message) => Err(anyhow::anyhow!(message)),
    }
}
