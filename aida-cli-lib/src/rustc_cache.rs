//! Diagnose Cargo's cached rustc probes without invoking a broken compiler.
// trace:TASK-2324 | ai:codex
use anyhow::{Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const CATEGORY: &str = "rustc-cache";
const LIMIT: u64 = 1024 * 1024;

fn path(root: &Path) -> Result<PathBuf> {
    let target = match std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        // Cargo resolves an explicit relative target against the invocation
        // directory, including when invoked below the repository root.
        Some(target) if target.is_relative() => std::env::current_dir()?.join(target),
        Some(target) => target,
        None => root.join("target"),
    };
    Ok(target.join(".rustc_info.json"))
}

fn failed_wrapper(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    value
        .get("outputs")
        .and_then(|v| v.as_object())
        .is_some_and(|outputs| {
            outputs.values().any(|output| {
                if output.get("success").and_then(|v| v.as_bool()) != Some(false) {
                    return false;
                }
                let text = format!(
                    "{}\n{}",
                    output.get("stdout").and_then(|v| v.as_str()).unwrap_or(""),
                    output.get("stderr").and_then(|v| v.as_str()).unwrap_or("")
                )
                .to_lowercase();
                [
                    "sccache",
                    "ccache",
                    "cachepot",
                    "rustc_wrapper",
                    "rustc wrapper",
                ]
                .iter()
                .any(|s| text.contains(s))
                    && ["error", "failed", "eperm", "permission denied"]
                        .iter()
                        .any(|s| text.contains(s))
            })
        })
}

fn read_failed(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading compiler cache {}", path.display()))
        }
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > LIMIT {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= LIMIT && failed_wrapper(&bytes)).then_some(bytes))
}

pub(crate) fn scan(root: &Path) -> Vec<crate::DoctorFinding> {
    let Ok(path) = path(root) else {
        return Vec::new();
    };
    if !matches!(read_failed(&path), Ok(Some(_))) {
        return Vec::new();
    }
    vec![crate::DoctorFinding {
        category: CATEGORY.into(),
        id: path.display().to_string(),
        summary: format!(
            "{} caches a failed compiler-wrapper probe; Cargo can replay the error",
            path.display()
        ),
        action: "run `aida doctor rustc-cache --fix` to remove the stale probe cache".into(),
        safe_heal: false,
    }]
}

fn inspect(path: &Path, fix: bool) -> Result<bool> {
    let Some(snapshot) = read_failed(path)? else {
        return Ok(false);
    };
    if fix {
        // Recheck immediately before deletion: a refreshed or healthy cache stays.
        anyhow::ensure!(
            read_failed(path)?.as_ref() == Some(&snapshot),
            "compiler cache {} changed; rerun the check",
            path.display()
        );
        std::fs::remove_file(path)
            .with_context(|| format!("removing compiler cache {}", path.display()))?;
    }
    Ok(true)
}

pub(crate) fn run(root: &Path, fix: bool) -> Result<()> {
    let path = path(root)?;
    if inspect(&path, fix)? {
        if fix {
            println!("removed stale compiler cache {}", path.display());
        } else {
            println!(
                "{} caches a failed compiler-wrapper probe; run `aida doctor rustc-cache --fix`",
                path.display()
            );
        }
    } else {
        println!("no cached compiler-wrapper failure at {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(success: bool, text: &str) -> Vec<u8> {
        serde_json::to_vec(
            &serde_json::json!({"outputs":{"123":{"success":success,"stdout":"","stderr":text}}}),
        )
        .unwrap()
    }
    #[test]
    fn task_2324_reports_and_removes_only_with_fix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".rustc_info.json");
        let bytes = fixture(
            false,
            "sccache: error: failed to execute compile: Operation not permitted (os error 1)",
        );
        std::fs::write(&path, &bytes).unwrap();
        assert!(inspect(&path, false).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(inspect(&path, true).unwrap());
        assert!(!path.exists());
        assert!(!inspect(&path, true).unwrap());
    }
    #[test]
    fn task_2324_keeps_healthy_malformed_and_unrelated_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".rustc_info.json");
        for bytes in [
            fixture(true, "sccache: error: old unrelated text"),
            fixture(false, "rustc: error: unknown target"),
            b"bad json".to_vec(),
            b"{}".to_vec(),
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(!inspect(&path, true).unwrap());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn task_2324_relative_target_uses_invocation_directory() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&nested).unwrap();
        let target_env = crate::test_env::EnvVarGuard::set("CARGO_TARGET_DIR", "relative-target");
        let selected = nested.join("relative-target/.rustc_info.json");
        let unrelated = dir.path().join("relative-target/.rustc_info.json");
        let bytes = fixture(false, "sccache: error: cached EPERM");
        for path in [&selected, &unrelated] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &bytes).unwrap();
        }
        assert_eq!(path(dir.path()).unwrap(), selected);
        assert_eq!(scan(dir.path()).len(), 1);
        run(dir.path(), false).unwrap();
        assert!(selected.exists());
        run(dir.path(), true).unwrap();
        assert!(!selected.exists());
        assert_eq!(std::fs::read(&unrelated).unwrap(), bytes);
        drop(target_env);
        // With no override, keep the supplied worktree's default target.
        assert_eq!(
            path(dir.path()).unwrap(),
            dir.path().join("target/.rustc_info.json")
        );
        std::env::set_current_dir(original).unwrap();
    }

    #[test]
    fn task_2324_cli_requires_explicit_fix() {
        let parsed =
            crate::command_registry::try_parse_from(["aida", "doctor", "rustc-cache"]).unwrap();
        let args = parsed.command.registered::<crate::doctor_cmd::DoctorArgs>();
        assert!(matches!(
            args.cmd,
            Some(crate::cli::DoctorCommand::RustcCache { fix: false })
        ));
    }
}
