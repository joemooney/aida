//! Descriptor-pinned mounts; required mounts never use bind-try. Denials last.
// trace:TASK-2705 | ai:codex
use anyhow::{Context, Result};
use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug)]
pub(super) enum WriteRoot {
    Workspace,
    Store,
    Cache,
    VendorState,
}
impl WriteRoot {
    fn destination(self) -> &'static str {
        match self {
            Self::Workspace => "/lane/workspace",
            Self::Store => "/lane/store",
            Self::Cache => "/lane/cache",
            Self::VendorState => "/lane/state",
        }
    }
}
struct Mount {
    source: File,
    destination: String,
    writable: bool,
}
pub(super) struct Boundary {
    mounts: Vec<Mount>,
}

impl Boundary {
    /// Only host-created synthetic roots are admitted by this slice. No project
    /// paths, environment root claims, credentials or stored shell are accepted.
    pub(super) fn synthetic(root: &Path) -> Result<Self> {
        let mut boundary = Self { mounts: Vec::new() };
        for path in ["/usr", "/bin", "/lib", "/lib64"] {
            // Resolve distro symlinks once, then pin their final inode. Sources
            // here are fixed system paths; never branch-controlled mount roots.
            if Path::new(path).exists() {
                boundary.add(&std::fs::canonicalize(path)?, path, false)?;
            }
        }
        for kind in [
            WriteRoot::Workspace,
            WriteRoot::Store,
            WriteRoot::Cache,
            WriteRoot::VendorState,
        ] {
            let dest = kind.destination();
            let source = root.join(dest.trim_start_matches("/lane/"));
            boundary.add(&source, dest, true)?;
        }
        boundary.add(&root.join("outside"), "/outside", false)?;
        // Entire protected directories, including missing child names: no
        // writable ancestor of protected metadata is re-admitted afterward.
        for name in [".aida", ".git", ".agents", ".codex", ".claude"] {
            let dest = format!("/lane/workspace/{name}");
            boundary.add(&root.join("workspace").join(name), &dest, false)?;
        }
        for name in ["AGENTS.md", "CLAUDE.md", ".mcp.json"] {
            boundary.add(
                &root.join("workspace").join(name),
                &format!("/lane/workspace/{name}"),
                false,
            )?;
        }
        Ok(boundary)
    }

    fn add(&mut self, source: &Path, destination: &str, writable: bool) -> Result<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(source)
            .with_context(|| format!("pin required mount {}", source.display()))?;
        let md = file.metadata()?;
        anyhow::ensure!(!md.file_type().is_symlink(), "symlink mount refused");
        anyhow::ensure!(md.is_dir() || md.is_file(), "non-regular mount refused");
        if md.is_file() {
            anyhow::ensure!(md.nlink() == 1, "hard-linked mount refused");
        }
        self.mounts.push(Mount {
            source: file,
            destination: destination.into(),
            writable,
        });
        Ok(())
    }

    pub(super) fn append_to(&self, cmd: &mut Command) {
        cmd.args([
            "--unshare-all",
            "--unshare-user",
            "--unshare-cgroup",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
            "--new-session",
            "--clearenv",
            "--tmpfs",
            "/",
            "--dir",
            "/lane",
        ]);
        for mount in &self.mounts {
            cmd.arg(if mount.writable {
                "--bind-fd"
            } else {
                "--ro-bind-fd"
            })
            .arg(mount.source.as_raw_fd().to_string())
            .arg(&mount.destination);
        }
        cmd.args([
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/run",
            "--setenv",
            "HOME",
            "/lane/state",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--chdir",
            "/lane/workspace",
            "--remount-ro",
            "/",
        ]);
    }

    /// Exact typed mount descriptors survive only the exec into bwrap. bwrap
    /// consumes/closes them before starting the lane. The lane checks this.
    pub(super) fn fds(&self) -> Vec<RawFd> {
        self.mounts.iter().map(|m| m.source.as_raw_fd()).collect()
    }
}
