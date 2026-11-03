//! The sandbox resources a launch declares once. Slice A declares the type
//! and the one fixed decision, sccache off for every lane (advisor ruling O2:
//! a compile cache a seat can write poisons every seat and the host). The
//! per-backend renderers (Claude `--settings`, bwrap, Codex writable roots,
//! the lane-job runner) read it in slice C, which also resolves the cargo
//! slot directory.
// trace:STORY-2326 | ai:claude

use std::path::PathBuf;

/// The compile cache a lane may use. There is one value: off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sccache {
    Off,
}

impl Sccache {
    /// The `RUSTC_WRAPPER` value that enforces it.
    pub(crate) fn rustc_wrapper(self) -> &'static str {
        match self {
            Self::Off => "",
        }
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
    /// The cargo env every renderer applies: the cache is off and builds land
    /// in the seat's own target directory.
    pub(crate) fn cargo_env(&self) -> [(&'static str, std::ffi::OsString); 2] {
        [
            ("RUSTC_WRAPPER", self.cargo.sccache.rustc_wrapper().into()),
            (
                "CARGO_TARGET_DIR",
                self.cargo.target_dir.clone().into_os_string(),
            ),
        ]
    }
}
