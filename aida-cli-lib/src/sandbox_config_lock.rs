//! `aida doctor` check for the empty read-only `config.lock` a sandboxed
//! shell leaves in the shared git directory. While it is there, every git
//! config write fails ("could not lock config file .git/config: File
//! exists"): new worktrees, branch deletion, `push -u`. The predicate lives in
//! [`aida_core::git_ops::is_sandbox_placeholder`]: an empty file with mode
//! exactly 0444, which a real git lock never is.
// trace:BUG-2211 | ai:claude

use std::path::Path;

/// Canonical `aida doctor` category name.
pub(crate) const CATEGORY: &str = "sandbox-config-lock";

/// One finding when the placeholder is present in `project_root`'s git
/// common dir; the id is the lock path.
// trace:BUG-2211 | ai:claude
pub(crate) fn scan(project_root: &Path) -> Vec<crate::DoctorFinding> {
    let Some(lock) = aida_core::git_ops::sandbox_config_lock(project_root) else {
        return Vec::new();
    };
    vec![crate::DoctorFinding {
        category: CATEGORY.to_string(),
        id: lock.display().to_string(),
        summary: format!(
            "{} is an empty read-only lock left by a sandboxed shell; \
             git config writes (new worktrees, branch deletion) fail until it is removed",
            lock.display()
        ),
        action: "run `aida doctor --heal --category sandbox-config-lock` to remove it (only an empty, mode-0444 lock is removed)"
            .to_string(),
        safe_heal: true,
    }]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_git::git_command;
    use std::os::unix::fs::PermissionsExt;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let out = git_command()
            .current_dir(dir.path())
            .args(["init", "-q", "-b", "main"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        dir
    }

    fn plant(dir: &Path, body: &[u8], mode: u32) -> std::path::PathBuf {
        let lock = dir.join(".git/config.lock");
        std::fs::write(&lock, body).unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(mode)).unwrap();
        lock
    }

    #[test]
    fn reports_the_placeholder_as_safe_to_heal() {
        let dir = repo();
        plant(dir.path(), b"", 0o444);
        let findings = scan(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, CATEGORY);
        assert!(findings[0].safe_heal);
        assert!(findings[0].id.ends_with("config.lock"));
        assert!(!findings[0].summary.contains("BUG-"));
    }

    #[test]
    fn ignores_a_real_or_writable_lock() {
        let dir = repo();
        let lock = plant(dir.path(), b"[core]\n", 0o444);
        assert!(scan(dir.path()).is_empty());
        std::fs::remove_file(&lock).unwrap();
        plant(dir.path(), b"", 0o644);
        assert!(scan(dir.path()).is_empty());
    }

    #[test]
    fn heal_removes_only_the_placeholder() {
        let dir = repo();
        let lock = plant(dir.path(), b"", 0o444);
        let finding = scan(dir.path()).remove(0);
        let healed =
            crate::doctor_cmd::heal_doctor_sandbox_config_lock(dir.path(), &finding).unwrap();
        assert_eq!(healed.status, "healed");
        assert!(!lock.exists());

        // A lock that changed to a real one between scan and heal is kept.
        let lock = plant(dir.path(), b"[core]\n", 0o444);
        let healed =
            crate::doctor_cmd::heal_doctor_sandbox_config_lock(dir.path(), &finding).unwrap();
        assert_eq!(healed.status, "skipped");
        assert!(lock.exists());
    }
}
