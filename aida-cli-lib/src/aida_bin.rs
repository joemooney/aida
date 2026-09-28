//! Portable resolution of the AIDA executable used by child launches.
//!
//! Precedence is `AIDA_BIN` (file or checkout), running executable, then PATH.
//! A checkout directory selects release by default; if both profiles exist and
//! debug is newer it selects debug. `AIDA_BUILD_PROFILE=debug|release` pins a
//! profile and errors if it is missing. A file under target/{debug,release}
//! keeps that profile. No checkout path is written into generated project files.
// trace:TASK-1499 | ai:codex

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    Debug,
    Release,
}
impl Profile {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub path: PathBuf,
    pub source: &'static str,
    pub profile: Option<Profile>,
    pub stale: bool,
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        path.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    }
}

pub(crate) fn resolve(
    override_path: Option<&Path>,
    requested: Option<Profile>,
    current: Option<&Path>,
    path_env: Option<&std::ffi::OsStr>,
) -> Result<Resolved> {
    if let Some(p) = override_path {
        return resolve_candidate(p, requested, "override-env");
    }
    if let Some(p) = current.filter(|p| executable(p)) {
        return resolve_candidate(p, requested, "running-exe");
    }
    if let Some(paths) = path_env {
        for dir in std::env::split_paths(paths) {
            let p = dir.join(if cfg!(windows) { "aida.exe" } else { "aida" });
            if executable(&p) {
                return resolve_candidate(&p, requested, "path");
            }
        }
    }
    Ok(Resolved {
        path: PathBuf::from("aida"),
        source: "path-unverified",
        profile: None,
        stale: false,
    })
}

fn resolve_candidate(
    candidate: &Path,
    requested: Option<Profile>,
    source: &'static str,
) -> Result<Resolved> {
    if candidate.is_dir() {
        let dbg =
            candidate
                .join("target/debug")
                .join(if cfg!(windows) { "aida.exe" } else { "aida" });
        let rel =
            candidate
                .join("target/release")
                .join(if cfg!(windows) { "aida.exe" } else { "aida" });
        let pick = requested.or_else(|| match (executable(&dbg), executable(&rel)) {
            (true, true) => {
                let dm = std::fs::metadata(&dbg).and_then(|m| m.modified()).ok();
                let rm = std::fs::metadata(&rel).and_then(|m| m.modified()).ok();
                if dm > rm {
                    Some(Profile::Debug)
                } else {
                    Some(Profile::Release)
                }
            }
            (true, false) => Some(Profile::Debug),
            (false, true) => Some(Profile::Release),
            _ => None,
        });
        let Some(profile) = pick else {
            bail!(
                "no executable AIDA build found at {} or {}",
                dbg.display(),
                rel.display()
            );
        };
        let path = if profile == Profile::Debug {
            dbg.clone()
        } else {
            rel.clone()
        };
        if !executable(&path) {
            bail!(
                "requested {} AIDA binary is missing or not executable: {}",
                profile.name(),
                path.display()
            );
        }
        let alternate = if profile == Profile::Debug { rel } else { dbg };
        let stale = executable(&alternate)
            && std::fs::metadata(&alternate)
                .and_then(|m| m.modified())
                .ok()
                > std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        return Ok(Resolved {
            path,
            source,
            profile: Some(profile),
            stale,
        });
    }
    if !executable(candidate) {
        bail!(
            "AIDA executable override is missing or not executable: {}",
            candidate.display()
        );
    }
    let parts: Vec<_> = candidate.components().collect();
    let profile = parts
        .iter()
        .rev()
        .nth(1)
        .and_then(|c| c.as_os_str().to_str())
        .and_then(|s| match s {
            "debug" => Some(Profile::Debug),
            "release" => Some(Profile::Release),
            _ => None,
        });
    if let (Some(_selected), Some(requested)) = (profile, requested) {
        let Some(root) = candidate
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        else {
            bail!(
                "cannot resolve checkout root for AIDA binary {}",
                candidate.display()
            );
        };
        let chosen = root
            .join("target")
            .join(requested.name())
            .join(if cfg!(windows) { "aida.exe" } else { "aida" });
        if !executable(&chosen) {
            bail!(
                "requested {} AIDA binary is missing or not executable: {}",
                requested.name(),
                chosen.display()
            );
        }
        let alternate = root
            .join("target")
            .join(requested_other_name(requested))
            .join(if cfg!(windows) { "aida.exe" } else { "aida" });
        let stale = executable(&alternate)
            && std::fs::metadata(&alternate)
                .and_then(|m| m.modified())
                .ok()
                > std::fs::metadata(&chosen).and_then(|m| m.modified()).ok();
        return Ok(Resolved {
            path: chosen,
            source,
            profile: Some(requested),
            stale,
        });
    }
    let stale = profile.is_some_and(|selected| {
        let Some(root) = candidate
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        else {
            return false;
        };
        let alternate = root
            .join("target")
            .join(requested_other_name(selected))
            .join(if cfg!(windows) { "aida.exe" } else { "aida" });
        executable(&alternate)
            && std::fs::metadata(&alternate)
                .and_then(|m| m.modified())
                .ok()
                > std::fs::metadata(candidate).and_then(|m| m.modified()).ok()
    });
    Ok(Resolved {
        path: candidate.to_path_buf(),
        source,
        profile,
        stale,
    })
}

fn requested_other_name(profile: Profile) -> &'static str {
    match profile {
        Profile::Debug => "release",
        Profile::Release => "debug",
    }
}

pub(crate) fn process() -> Result<Resolved> {
    let override_path = std::env::var_os("AIDA_BIN").map(PathBuf::from);
    let requested = std::env::var("AIDA_BUILD_PROFILE")
        .ok()
        .map(|s| match s.as_str() {
            "debug" => Ok(Profile::Debug),
            "release" => Ok(Profile::Release),
            _ => bail!("AIDA_BUILD_PROFILE must be debug or release"),
        })
        .transpose()?;
    let current = std::env::current_exe().ok();
    resolve(
        override_path.as_deref(),
        requested,
        current.as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn binary(root: &Path, profile: &str) -> PathBuf {
        let p = root.join("target").join(profile).join("aida");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn override_running_and_path_precedence_and_layouts() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let over = binary(a.path(), "release");
        let override_debug = binary(a.path(), "debug");
        let running = binary(b.path(), "debug");
        let pathdir = tempfile::tempdir().unwrap();
        std::fs::write(pathdir.path().join("aida"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            pathdir.path().join("aida"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let path = std::env::join_paths([pathdir.path()]).unwrap();
        assert_eq!(
            resolve(Some(&over), None, Some(&running), Some(&path))
                .unwrap()
                .path,
            over
        );
        assert_eq!(
            resolve(None, None, Some(&running), Some(&path))
                .unwrap()
                .path,
            running
        );
        assert_eq!(
            resolve(None, None, None, Some(&path)).unwrap().source,
            "path"
        );
        assert_eq!(
            resolve(Some(a.path()), None, None, None).unwrap().profile,
            Some(Profile::Debug)
        );
        assert_eq!(
            resolve(Some(&over), Some(Profile::Debug), None, None)
                .unwrap()
                .path,
            override_debug
        );
        assert_ne!(a.path(), b.path());
    }

    #[test]
    fn rejects_non_executable_override_and_selects_fresh_profile() {
        let d = tempfile::tempdir().unwrap();
        let dbg = binary(d.path(), "debug");
        let rel = binary(d.path(), "release");
        let now = std::time::SystemTime::now();
        std::fs::File::open(&dbg)
            .unwrap()
            .set_modified(now)
            .unwrap();
        std::fs::File::open(&rel)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(10))
            .unwrap();
        let selected = resolve(Some(d.path()), Some(Profile::Release), None, None).unwrap();
        assert_eq!(selected.profile, Some(Profile::Release));
        assert!(selected.stale);
        std::fs::set_permissions(&rel, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolve(Some(&rel), None, None, None)
            .unwrap_err()
            .to_string()
            .contains("not executable"));
    }

    #[test]
    fn scaffolded_files_do_not_embed_checkout_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut scaffold = aida_core::scaffolding::Scaffolder::new(
            root.path().to_path_buf(),
            aida_core::scaffolding::ScaffoldConfig::default(),
        );
        let preview = scaffold.preview(&aida_core::RequirementsStore::default());
        let absolute = root.path().to_string_lossy();
        for artifact in preview.artifacts {
            assert!(
                !artifact.content.contains(absolute.as_ref()),
                "{} embeds checkout path {}",
                artifact.path.display(),
                absolute
            );
        }
    }
}
