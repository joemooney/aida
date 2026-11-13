//! Declared launch resources. Agent seats keep sccache off; lane test jobs
//! may declare a separate cache and Unix socket. PR-B adds fail-closed
//! slot, target and generated-settings validation; backend renderers in the
//! following slice consume these checks. No helper creates or widens a slot.
// trace:STORY-2326 | ai:claude
// trace:STORY-2538 | ai:codex

use aida_core::exec_context::ExecContext;
use std::path::PathBuf;

/// The compile cache declaration; lane caches never select the operator server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sccache {
    Off,
    // trace:STORY-2641 | ai:codex
    Lane {
        wrapper: String,
        cache: PathBuf,
        socket: PathBuf,
    },
}

impl Sccache {
    /// The `RUSTC_WRAPPER` value that enforces it.
    pub(crate) fn rustc_wrapper(&self) -> &str {
        match self {
            Self::Off => "",
            Self::Lane { wrapper, .. } => wrapper,
        }
    }

    // trace:STORY-2641 | ai:codex
    pub(crate) fn env(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        let mut env = vec![("RUSTC_WRAPPER", self.rustc_wrapper().into())];
        if let Self::Lane { cache, socket, .. } = self {
            env.extend([
                ("SCCACHE_DIR", cache.as_os_str().to_owned()),
                // Ignore operator remote backends; lane jobs use only their disk cache.
                ("SCCACHE_CONF", "/dev/null".into()),
                ("SCCACHE_SERVER_UDS", socket.as_os_str().to_owned()),
                // Unsupported UDS clients must never fall back to the operator's TCP server.
                ("SCCACHE_SERVER_PORT", "0".into()),
                ("SCCACHE_IDLE_TIMEOUT", "60".into()),
            ]);
        }
        env
    }
}

/// Cargo resources a lane may write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CargoResources {
    /// The seat-private target directory.
    pub(crate) target_dir: PathBuf,
    /// The cargo slot lock directory; `None` when it does not resolve, and the
    /// lane then reports that cargo slots are unavailable.
    pub(crate) slot_dir: Option<PathBuf>,
    pub(crate) sccache: Sccache,
}

/// Everything a launch may write or reach, declared once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SandboxResources {
    pub(crate) write_roots: Vec<PathBuf>,
    pub(crate) store_paths: Vec<PathBuf>,
    pub(crate) deny: Vec<PathBuf>,
    pub(crate) cargo: CargoResources,
}

impl SandboxResources {
    /// Resolve the seat declaration from host evidence, never forwarded build env.
    // trace:STORY-2641 | ai:codex
    pub(crate) fn for_seat(
        exec: &ExecContext,
        host: Option<&std::path::Path>,
        seat: &std::path::Path,
    ) -> anyhow::Result<Option<Self>> {
        let Some(host) = host else {
            return Ok(None);
        };
        let Some(clone) = crate::seat_clone::host_recorded_clone(host, seat) else {
            return Ok(None);
        };
        // Fresh/non-Rust clones and targets removed by host cargo clean have
        // no build resources to override. Their ordinary launches still work.
        let Some(target) = clone.cargo_target_dir.as_deref() else {
            return Ok(None);
        };
        Ok(Some(Self {
            write_roots: crate::seat_clone::seat_extra_write_paths(&clone),
            store_paths: vec![],
            deny: vec![],
            cargo: CargoResources::resolve(exec, host, seat, target)?,
        }))
    }

    /// Project declared cargo overrides through the same complete environment
    /// policy as vendor launches, rather than ambient inheritance.
    // trace:STORY-2326 | ai:codex
    pub(crate) fn child_env(
        &self,
        policy: &aida_core::launch_policy::EnvPolicy,
        parent: &aida_core::exec_context::Environment,
    ) -> Result<aida_core::launch_policy::ChildEnv, aida_core::launch_policy::EnvPolicyRefusal>
    {
        policy.project(
            parent,
            self.cargo_env()
                .into_iter()
                .map(|(key, value)| (key.into(), Some(value))),
        )
    }

    /// Every renderer applies the declared cache and seat-private target.
    // trace:STORY-2641 | ai:codex
    pub(crate) fn cargo_env(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        let mut env = self.cargo.sccache.env();
        env.push((
            "CARGO_TARGET_DIR",
            self.cargo.target_dir.clone().into_os_string(),
        ));
        env
    }
}

/// Resolve only an existing, private slot directory. Never create it or
/// broaden its permissions. Symlinks are refused rather than followed.
// trace:STORY-2326 | ai:codex
pub(crate) fn cargo_slot_dir(exec: &ExecContext) -> Option<PathBuf> {
    let runtime = exec.environment().get("XDG_RUNTIME_DIR")?;
    let uid = unsafe { libc::geteuid() };
    checked_slot_dir(
        PathBuf::from(runtime).join(format!("cargo-slots-{uid}")),
        uid,
    )
}

fn checked_slot_dir(path: PathBuf, uid: u32) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    (metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o7777 == 0o700)
        .then_some(path)
}

impl CargoResources {
    /// Accept the host-recorded clone target or this worktree's own target.
    /// Canonicalization catches aliases into the host or another seat.
    /// The caller supplies an existing target (no mkdir here).
    // trace:STORY-2326 | ai:codex
    pub(crate) fn resolve(
        exec: &ExecContext,
        authority_root: &std::path::Path,
        seat_root: &std::path::Path,
        target_dir: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let authority = authority_root.canonicalize()?;
        let seat = seat_root.canonicalize()?;
        let target = target_dir.canonicalize()?;
        let host_target = authority.join("target");
        let host_target = host_target.canonicalize().unwrap_or(host_target);
        let recorded_target = crate::seat_clone::host_recorded_clone(&authority, &seat)
            .and_then(|clone| clone.cargo_target_dir)
            .and_then(|path| path.canonicalize().ok());
        let own_target = seat.join("target").canonicalize().ok();
        let local_target = own_target.as_ref() == Some(&target)
            && target.starts_with(&seat)
            && !target.starts_with(&host_target);
        let recorded_target = recorded_target.as_ref() == Some(&target) && target != host_target;
        anyhow::ensure!(
            seat != authority && target.is_dir() && (local_target || recorded_target),
            "cargo target must be private to the launching seat"
        );
        Ok(Self {
            target_dir: target,
            slot_dir: cargo_slot_dir(exec),
            sccache: Sccache::Off,
        })
    }
}

/// Validate a generated host settings file by handle before a renderer uses
/// it. Its canonical location must be outside every sandbox write root;
/// mode 0600 alone cannot isolate processes sharing the host uid.
/// Renderers are introduced in the following slice.
// trace:STORY-2326 | ai:codex
pub(crate) fn open_host_settings(
    path: &std::path::Path,
    write_roots: &[PathBuf],
) -> anyhow::Result<std::fs::File> {
    let canonical = path.canonicalize()?;
    let expected = std::fs::metadata(&canonical)?;
    for root in write_roots {
        anyhow::ensure!(
            !canonical.starts_with(root.canonicalize()?),
            "generated launch settings must be outside sandbox write roots"
        );
    }
    open_checked_settings(path, &expected)
}

fn open_checked_settings(
    path: &std::path::Path,
    expected: &std::fs::Metadata,
) -> anyhow::Result<std::fs::File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    // Bind the location check to the opened inode, including parent symlinks.
    // trace:STORY-2326 | ai:codex
    anyhow::ensure!(
        metadata.dev() == expected.dev() && metadata.ino() == expected.ino(),
        "generated launch settings changed during validation"
    );
    anyhow::ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7777 == 0o600,
        "generated launch settings must be a host-owned private regular file"
    );
    Ok(file)
}

#[cfg(test)]
mod story_2326_resources_tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn fixture(runtime: Option<&std::path::Path>) -> ExecContext {
        use aida_core::exec_context::{Environment, SystemClock, ToolPaths};
        ExecContext::new(
            Environment::from_pairs(
                runtime.map(|path| ("XDG_RUNTIME_DIR".into(), path.as_os_str().to_owned())),
            ),
            None,
            "/seat".into(),
            "/host".into(),
            ToolPaths::default(),
            std::sync::Arc::new(SystemClock::default()),
        )
    }

    #[test]
    fn runtime_slot_uses_only_explicit_context() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join(format!("cargo-slots-{}", unsafe { libc::geteuid() }));
        let exec = fixture(Some(tmp.path()));
        assert_eq!(cargo_slot_dir(&exec), None);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(cargo_slot_dir(&exec), Some(path));
        assert_eq!(cargo_slot_dir(&fixture(None)), None);
    }

    #[test]
    fn slot_directory_requires_existing_owned_mode_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("slots");
        let uid = unsafe { libc::geteuid() };
        assert!(checked_slot_dir(path.clone(), uid).is_none());
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(checked_slot_dir(path.clone(), uid).is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(checked_slot_dir(path.clone(), uid), Some(path.clone()));
        assert!(checked_slot_dir(path.clone(), uid.wrapping_add(1)).is_none());
        let alias = tmp.path().join("alias");
        symlink(&path, &alias).unwrap();
        assert!(checked_slot_dir(alias, uid).is_none());
    }

    #[test]
    fn target_refuses_host_other_seat_and_symlink_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let host = tmp.path().join("host");
        let seat = tmp.path().join("seat");
        let other = tmp.path().join("other");
        for root in [&host, &seat, &other] {
            std::fs::create_dir_all(root.join("target")).unwrap();
        }
        assert!(
            CargoResources::resolve(&fixture(None), &host, &seat, &seat.join("target")).is_ok()
        );
        for target in [host.join("target"), other.join("target"), seat.clone()] {
            assert!(CargoResources::resolve(&fixture(None), &host, &seat, &target).is_err());
        }
        symlink(other.join("target"), seat.join("alias")).unwrap();
        assert!(
            CargoResources::resolve(&fixture(None), &host, &seat, &seat.join("alias")).is_err()
        );
    }

    #[test]
    fn settings_parent_symlink_swap_refuses_different_opened_inode() {
        let tmp = tempfile::tempdir().unwrap();
        let host = tmp.path().join("host");
        let seat = tmp.path().join("seat");
        for dir in [&host, &seat] {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(dir.join("settings"), "{}").unwrap();
            std::fs::set_permissions(dir.join("settings"), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let alias = tmp.path().join("parent");
        symlink(&host, &alias).unwrap();
        let path = alias.join("settings");
        let canonical = path.canonicalize().unwrap();
        let expected = std::fs::metadata(&canonical).unwrap();
        assert!(!canonical.starts_with(&seat));
        std::fs::remove_file(&alias).unwrap();
        symlink(&seat, &alias).unwrap();
        assert!(open_checked_settings(&path, &expected)
            .unwrap_err()
            .to_string()
            .contains("changed during validation"));
    }

    #[test]
    fn lane_cannot_write_its_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        let seat = tmp.path().join("seat");
        std::fs::create_dir(&seat).unwrap();
        let settings = tmp.path().join("settings.json");
        std::fs::write(&settings, "{}").unwrap();
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(open_host_settings(&settings, &[seat.clone()]).is_ok());
        assert!(open_host_settings(&settings, &[tmp.path().to_path_buf()]).is_err());
        let inside = seat.join("settings.json");
        std::fs::write(&inside, "{}").unwrap();
        std::fs::set_permissions(&inside, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(open_host_settings(&inside, &[seat.clone()]).is_err());
        let alias = tmp.path().join("alias");
        symlink(&settings, &alias).unwrap();
        assert!(open_host_settings(&alias, &[seat.clone()]).is_err());
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(open_host_settings(&settings, &[seat]).is_err());
    }
}
