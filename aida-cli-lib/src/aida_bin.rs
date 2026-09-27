//! TASK-1499: Portable AIDA binary resolution across local installations.
//!
//! Precedence:
//! 1. Explicit override: `AIDA_BIN` environment variable or `[agents] aida_bin`
//!    in `~/.aida/agents.toml`. An override may name a binary file or a checkout directory.
//! 2. Running executable: current running executable (with Linux ` (deleted)` stripped).
//! 3. PATH: `aida` looked up on `PATH`.
//! 4. Bare `aida` with `PathUnverified` fallback if not found on PATH.
//!
//! Profile rule:
//! (a) A file path whose last three segments are `target/<debug|release>/aida[.exe]`
//!     reports that profile and checkout root; `newer_alternate` is set when the
//!     other profile exists and is newer (`alternate_build_is_newer`).
//! (b) A directory override resolves `<dir>/target/{release,debug}/aida`:
//!     an explicit profile request (`AIDA_BUILD_PROFILE=debug|release`, or
//!     `[agents] aida_build_profile`) wins and errors if that build is absent;
//!     otherwise release, unless debug is newer by mtime (freshest-wins);
//!     only one present -> that one.
//! (c) Never pick a binary the rule did not name; the diagnostics always say why.

// trace:TASK-1499 | ai:antigravity

use anyhow::{bail, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AidaBinSource {
    OverrideEnv,
    OverrideConfig,
    RunningExe,
    Path,
    PathUnverified,
}

impl AidaBinSource {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::OverrideEnv => "override-env",
            Self::OverrideConfig => "override-config",
            Self::RunningExe => "running-exe",
            Self::Path => "path",
            Self::PathUnverified => "path-unverified",
        }
    }
}

impl std::fmt::Display for AidaBinSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildProfile {
    Debug,
    Release,
}

impl BuildProfile {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    pub(crate) fn from_str_opt(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Self::Debug),
            "release" => Some(Self::Release),
            _ => None,
        }
    }
}

impl std::fmt::Display for BuildProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedAida {
    pub path: PathBuf,
    pub source: AidaBinSource,
    pub profile: Option<BuildProfile>,
    pub checkout_root: Option<PathBuf>,
    pub newer_alternate: Option<(BuildProfile, PathBuf)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolveInputs {
    pub override_path: Option<PathBuf>,
    pub override_source: Option<AidaBinSource>,
    pub profile_request: Option<BuildProfile>,
    pub current_exe: Option<PathBuf>,
    pub path_env: Option<OsString>,
}

#[cfg(unix)]
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = path.metadata() {
        meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
    } else {
        false
    }
}

#[cfg(windows)]
pub(crate) fn is_executable(path: &Path) -> bool {
    if let Ok(meta) = path.metadata() {
        if !meta.is_file() {
            return false;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "exe" {
            return true;
        }
        if let Ok(pathext) = std::env::var("PATHEXT") {
            let extensions: Vec<String> = pathext
                .split(';')
                .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
                .collect();
            extensions.contains(&ext)
        } else {
            false
        }
    } else {
        false
    }
}

pub(crate) fn ensure_executable(path: &Path, source: AidaBinSource) -> Result<()> {
    if !path.exists() {
        bail!("aida executable {} (from {source}) does not exist", path.display());
    }
    if !is_executable(path) {
        bail!(
            "aida executable {} (from {source}) is not executable; set AIDA_BIN to a built binary or run cargo build --release",
            path.display()
        );
    }
    Ok(())
}

pub(crate) fn detect_profile_and_checkout(path: &Path) -> (Option<BuildProfile>, Option<PathBuf>) {
    let components: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if components.len() < 3 {
        return (None, None);
    }
    let filename = &components[components.len() - 1];
    let profile_seg = &components[components.len() - 2];
    let target_seg = &components[components.len() - 3];

    let is_aida = filename == "aida" || filename == "aida.exe";
    let is_target = target_seg == "target";

    if is_aida && is_target {
        let profile = match profile_seg.to_ascii_lowercase().as_str() {
            "debug" => Some(BuildProfile::Debug),
            "release" => Some(BuildProfile::Release),
            _ => None,
        };
        if profile.is_some() {
            if let Some(parent3) = path.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
                let checkout_root = if parent3.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    parent3.to_path_buf()
                };
                return (profile, Some(checkout_root));
            }
        }
    }
    (None, None)
}

fn candidate_path(checkout_root: &Path, profile: BuildProfile) -> PathBuf {
    let p_str = profile.as_str();
    #[cfg(windows)]
    {
        let exe = checkout_root.join("target").join(p_str).join("aida.exe");
        if exe.exists() {
            return exe;
        }
    }
    checkout_root.join("target").join(p_str).join("aida")
}

fn check_newer_alternate(
    checkout_root: &Path,
    active_profile: BuildProfile,
    active_path: &Path,
) -> Option<(BuildProfile, PathBuf)> {
    let alt_profile = match active_profile {
        BuildProfile::Debug => BuildProfile::Release,
        BuildProfile::Release => BuildProfile::Debug,
    };
    let alt_path = candidate_path(checkout_root, alt_profile);
    if !alt_path.exists() {
        return None;
    }
    let active_mtime = active_path.metadata().and_then(|m| m.modified()).ok()?;
    let alt_mtime = alt_path.metadata().and_then(|m| m.modified()).ok()?;
    if alt_mtime > active_mtime {
        Some((alt_profile, alt_path))
    } else {
        None
    }
}

fn resolve_from_directory(
    dir: &Path,
    source: AidaBinSource,
    profile_request: Option<BuildProfile>,
) -> Result<ResolvedAida> {
    let rel_cand = candidate_path(dir, BuildProfile::Release);
    let dbg_cand = candidate_path(dir, BuildProfile::Debug);
    let rel_exists = rel_cand.exists();
    let dbg_exists = dbg_cand.exists();

    if let Some(req) = profile_request {
        match req {
            BuildProfile::Debug => {
                if !dbg_exists {
                    bail!(
                        "no debug build at {}.\nRun `cargo build` first.",
                        dbg_cand.display()
                    );
                }
                ensure_executable(&dbg_cand, source)?;
                let newer_alternate = check_newer_alternate(dir, BuildProfile::Debug, &dbg_cand);
                return Ok(ResolvedAida {
                    path: dbg_cand,
                    source,
                    profile: Some(BuildProfile::Debug),
                    checkout_root: Some(dir.to_path_buf()),
                    newer_alternate,
                });
            }
            BuildProfile::Release => {
                if !rel_exists {
                    bail!(
                        "no release build at {}.\nRun `cargo build --release` first.",
                        rel_cand.display()
                    );
                }
                ensure_executable(&rel_cand, source)?;
                let newer_alternate = check_newer_alternate(dir, BuildProfile::Release, &rel_cand);
                return Ok(ResolvedAida {
                    path: rel_cand,
                    source,
                    profile: Some(BuildProfile::Release),
                    checkout_root: Some(dir.to_path_buf()),
                    newer_alternate,
                });
            }
        }
    }

    if !rel_exists && !dbg_exists {
        bail!(
            "no aida binary found at {} or {}",
            rel_cand.display(),
            dbg_cand.display()
        );
    }

    if rel_exists && !dbg_exists {
        ensure_executable(&rel_cand, source)?;
        return Ok(ResolvedAida {
            path: rel_cand,
            source,
            profile: Some(BuildProfile::Release),
            checkout_root: Some(dir.to_path_buf()),
            newer_alternate: None,
        });
    }

    if !rel_exists && dbg_exists {
        ensure_executable(&dbg_cand, source)?;
        return Ok(ResolvedAida {
            path: dbg_cand,
            source,
            profile: Some(BuildProfile::Debug),
            checkout_root: Some(dir.to_path_buf()),
            newer_alternate: None,
        });
    }

    let rel_mtime = rel_cand.metadata().and_then(|m| m.modified()).ok();
    let dbg_mtime = dbg_cand.metadata().and_then(|m| m.modified()).ok();

    let pick_debug = matches!((rel_mtime, dbg_mtime), (Some(r), Some(d)) if d > r);
    if pick_debug {
        ensure_executable(&dbg_cand, source)?;
        let newer_alternate = check_newer_alternate(dir, BuildProfile::Debug, &dbg_cand);
        Ok(ResolvedAida {
            path: dbg_cand,
            source,
            profile: Some(BuildProfile::Debug),
            checkout_root: Some(dir.to_path_buf()),
            newer_alternate,
        })
    } else {
        ensure_executable(&rel_cand, source)?;
        let newer_alternate = check_newer_alternate(dir, BuildProfile::Release, &rel_cand);
        Ok(ResolvedAida {
            path: rel_cand,
            source,
            profile: Some(BuildProfile::Release),
            checkout_root: Some(dir.to_path_buf()),
            newer_alternate,
        })
    }
}

fn resolve_from_file(
    file: &Path,
    source: AidaBinSource,
    _profile_request: Option<BuildProfile>,
) -> Result<ResolvedAida> {
    ensure_executable(file, source)?;
    let (profile, checkout_root) = detect_profile_and_checkout(file);
    let newer_alternate = if let (Some(prof), Some(root)) = (profile, &checkout_root) {
        check_newer_alternate(root, prof, file)
    } else {
        None
    };
    Ok(ResolvedAida {
        path: file.to_path_buf(),
        source,
        profile,
        checkout_root,
        newer_alternate,
    })
}

pub(crate) fn resolve_in_path(path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env) {
        #[cfg(windows)]
        {
            let exe = dir.join("aida.exe");
            if is_executable(&exe) {
                return Some(exe);
            }
        }
        let candidate = dir.join("aida");
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn resolve_aida_executable(inputs: &ResolveInputs) -> Result<ResolvedAida> {
    // 1. Explicit override
    if let Some(ref override_path) = inputs.override_path {
        let source = inputs.override_source.unwrap_or(AidaBinSource::OverrideEnv);
        if !override_path.exists() {
            bail!(
                "aida executable {} (from {source}) does not exist",
                override_path.display()
            );
        }
        if override_path.is_dir() {
            return resolve_from_directory(override_path, source, inputs.profile_request);
        } else {
            return resolve_from_file(override_path, source, inputs.profile_request);
        }
    }

    // 2. Running executable
    if let Some(ref current) = inputs.current_exe {
        let lossy = current.to_string_lossy();
        let cleaned = lossy
            .strip_suffix(" (deleted)")
            .map(PathBuf::from)
            .unwrap_or_else(|| current.clone());
        if cleaned.exists() {
            let (profile, checkout_root) = detect_profile_and_checkout(&cleaned);
            let newer_alternate = if let (Some(prof), Some(root)) = (profile, &checkout_root) {
                check_newer_alternate(root, prof, &cleaned)
            } else {
                None
            };
            return Ok(ResolvedAida {
                path: cleaned,
                source: AidaBinSource::RunningExe,
                profile,
                checkout_root,
                newer_alternate,
            });
        }
    }

    // 3. PATH search
    let path_bin = if let Some(ref path_env) = inputs.path_env {
        resolve_in_path(path_env)
    } else if let Some(path_env) = std::env::var_os("PATH") {
        resolve_in_path(&path_env)
    } else {
        None
    };

    if let Some(p) = path_bin {
        let (profile, checkout_root) = detect_profile_and_checkout(&p);
        let newer_alternate = if let (Some(prof), Some(root)) = (profile, &checkout_root) {
            check_newer_alternate(root, prof, &p)
        } else {
            None
        };
        return Ok(ResolvedAida {
            path: p,
            source: AidaBinSource::Path,
            profile,
            checkout_root,
            newer_alternate,
        });
    }

    // 4. Fallback bare "aida" with PathUnverified
    Ok(ResolvedAida {
        path: PathBuf::from("aida"),
        source: AidaBinSource::PathUnverified,
        profile: None,
        checkout_root: None,
        newer_alternate: None,
    })
}
