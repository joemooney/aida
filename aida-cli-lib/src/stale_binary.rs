//! Informational stale-image inventory and host service binary selection.
// trace:BUG-2654 | ai:codex
use anyhow::{Context, Result};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(crate) fn service_program() -> Result<PathBuf> {
    let live = crate::dev_build::default_live_dir()?.join("aida");
    service_program_at(&live, &std::env::current_exe()?)
}

fn service_program_at(live: &Path, current: &Path) -> Result<PathBuf> {
    // Keep the path, not its canonicalized target: the next restart follows
    // the next promotion too. Never fall back to a deleted kernel-held image.
    let program = if live.try_exists()? { live } else { current };
    let metadata = std::fs::metadata(program).with_context(|| {
        format!(
            "service binary {} is unavailable; restart from the live build",
            program.display()
        )
    })?;
    anyhow::ensure!(
        metadata.is_file() && metadata.mode() & 0o111 != 0,
        "service binary {} is not executable",
        program.display()
    );
    Ok(program.to_path_buf())
}

pub(crate) fn warnings(live: &Path) -> Vec<String> {
    scan(Path::new("/proc"), live)
        .unwrap_or_else(|e| vec![format!("cannot inspect stale AIDA processes: {e}")])
}

fn scan(proc: &Path, live: &Path) -> std::io::Result<Vec<String>> {
    let identity = |path: &Path| std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
    let live_id = identity(live);
    let server_id = identity(&live.with_file_name("aida-server"));
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(proc)? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let exe = entry.path().join("exe");
        let Ok(target) = std::fs::read_link(&exe) else {
            continue;
        };
        let text = target.to_string_lossy();
        let deleted = text.ends_with(" (deleted)");
        let path = Path::new(text.trim_end_matches(" (deleted)"));
        if !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("aida" | "aida-server")
        ) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&exe) else {
            continue;
        };
        let id = (meta.dev(), meta.ino());
        let replaced = std::fs::metadata(path).is_ok_and(|m| (m.dev(), m.ino()) != id);
        let expected = if path.file_name().is_some_and(|n| n == "aida-server") {
            server_id
        } else {
            live_id
        };
        let old_live = path.starts_with(live.parent().unwrap_or(live)) && Some(id) != expected;
        if deleted || replaced || old_live {
            rows.push(format!("stale AIDA process pid {pid}: {}; restart deputy runners after active jobs finish and rotate relays; older writers can fail with ENOENT at the format-2 gate-ledger fence; do not create its target directory", target.display()));
        }
    }
    rows.sort();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bug_2654_services_follow_replacement_and_refuse_missing_images() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("aida");
        let current = dir.path().join("old");
        std::fs::copy("/bin/true", &live).unwrap();
        let selected = service_program_at(&live, &current).unwrap();
        std::fs::rename(&live, &current).unwrap();
        std::fs::copy("/bin/false", &live).unwrap();
        assert!(!std::process::Command::new(selected)
            .status()
            .unwrap()
            .success());
        std::fs::remove_file(&live).unwrap();
        assert_eq!(service_program_at(&live, &current).unwrap(), current);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(service_program_at(&live, &current).is_err());
        std::fs::remove_file(&current).unwrap();
        assert!(service_program_at(&live, &current).is_err());
    }
    #[test]
    fn bug_2654_inventory_reports_old_and_deleted_but_not_current_or_unrelated() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live/aida");
        std::fs::create_dir(live.parent().unwrap()).unwrap();
        std::fs::write(&live, "new").unwrap();
        let previous = dir.path().join("live/prev/aida");
        std::fs::create_dir(previous.parent().unwrap()).unwrap();
        std::fs::write(&previous, "old").unwrap();
        let server = live.with_file_name("aida-server");
        let old_server = previous.with_file_name("aida-server");
        std::fs::write(&server, "new").unwrap();
        std::fs::write(&old_server, "old").unwrap();
        let proc = dir.path().join("proc");
        for (pid, target) in [
            (1, live.clone()),
            (2, previous),
            (3, dir.path().join("aida (deleted)")),
            (4, dir.path().join("other")),
            (5, server),
            (6, old_server),
        ] {
            let entry = proc.join(pid.to_string());
            std::fs::create_dir_all(&entry).unwrap();
            if pid >= 3 {
                std::fs::write(&target, "old").unwrap();
            }
            std::os::unix::fs::symlink(target, entry.join("exe")).unwrap();
        }
        let rows = scan(&proc, &live).unwrap();
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("pid 2:")));
        assert!(rows.iter().any(|r| r.contains("pid 3:")));
        assert!(rows.iter().any(|r| r.contains("pid 6:")));
    }
}
