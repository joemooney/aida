//! Advisor-directed worktree lock — the on-disk half of STORY-711 slice 1.
//!
//! `aida-core::lock` holds the PURE verifier (`verify_worktree_lock`); this
//! module is the file-IO side the `aida lock` CLI drives: find the session
//! lease covering a worktree, read/write its `authorized_by` field.
//!
//! Deliberately extends the EXISTING session-lease registry
//! (`.aida/sessions/*.toml`, the same files `aida ps` / `aida session leases`
//! / the TUI liveness glyph already read) rather than inventing a parallel
//! `.aida/locks/` directory — the signed-off design fork in
//! `docs/plans/2026-07-12-story-711-advisor-lock.md`. Patches are done via
//! generic `toml::Value` edits (insert/remove the `authorized_by` key) so an
//! existing lease's other ~20 fields are preserved untouched; when no lease
//! covers the target worktree yet, a minimal lock-only entry is created,
//! carrying just enough (`scope`, `started_at`, `worktree_path`,
//! `authorized_by`) for `SessionLeaseLite` to read it, plus an
//! `aida_lock_only` marker so `release` knows it's safe to delete outright
//! instead of leaving an empty synthetic lease behind.
//!
//! trace:STORY-711 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// One worktree currently carrying an `authorized_by` lock, as surfaced by
/// [`list_locks`] / `aida lock status`.
#[derive(Debug, Clone)]
pub(crate) struct LockedWorktree {
    pub worktree_path: String,
    pub authorized_by: String,
    /// The lease's `scope` (informational — names the work the lease itself
    /// covers, distinct from the lock's authorizing advisor).
    pub scope: String,
}

/// Best-effort canonicalization for comparison: a target that no longer
/// exists (or never did) compares on its raw path instead of erroring —
/// matching never silently "succeeds" via two different unresolved forms of
/// the same nonexistent path, but a lookup against a live lease (which DOES
/// exist) still works.
fn canonical_str(p: &Path) -> String {
    p.canonicalize()
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Find the `.aida/sessions/*.toml` file whose `worktree_path` matches
/// `target` (already canonicalized), if any. Tolerant of malformed files —
/// mirrors every other lease reader in this codebase (a bad file is skipped,
/// never fatal).
// trace:STORY-711 | ai:claude
fn find_lease_file_for_worktree(project_root: &Path, target_canonical: &str) -> Option<PathBuf> {
    let dir = aida_core::liveness::leases_dir(project_root);
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&content) else {
            continue;
        };
        let Some(wt) = value.get("worktree_path").and_then(|v| v.as_str()) else {
            continue;
        };
        if wt.is_empty() {
            continue;
        }
        if canonical_str(Path::new(wt)) == target_canonical {
            return Some(p);
        }
    }
    None
}

/// Acquire (or re-authorize) the lock on `worktree` for `advisor`.
///
/// Finds the session lease covering `worktree` and patches its
/// `authorized_by` field in place, preserving every other field. When no
/// lease covers the worktree yet, creates a minimal lock-only lease entry.
/// Returns the path written.
///
/// `worktree` must exist (locks bind to a real directory — canonicalization
/// is how we recognize the "same worktree" across the argument and whatever
/// lease already covers it).
// trace:STORY-711 | ai:claude
pub(crate) fn acquire(project_root: &Path, worktree: &Path, advisor: &str) -> Result<PathBuf> {
    let target = worktree
        .canonicalize()
        .with_context(|| format!("worktree path does not exist: {}", worktree.display()))?;
    let target_str = target.to_string_lossy().into_owned();

    if let Some(path) = find_lease_file_for_worktree(project_root, &target_str) {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read lease {}", path.display()))?;
        let mut value: toml::Value = toml::from_str(&content)
            .with_context(|| format!("could not parse lease {}", path.display()))?;
        let table = value.as_table_mut().context("lease TOML is not a table")?;
        table.insert(
            "authorized_by".to_string(),
            toml::Value::String(advisor.to_string()),
        );
        let out = toml::to_string_pretty(&value)?;
        aida_core::fs_atomic::write_atomic(&path, out)
            .with_context(|| format!("could not write lease {}", path.display()))?;
        return Ok(path);
    }

    // No lease covers this worktree yet — create a minimal lock-only entry.
    let dir = aida_core::liveness::leases_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let slug = crate::coordination::sanitize_scope(&target_str);
    let path = dir.join(format!("lock-{slug}.toml"));

    let mut table = toml::map::Map::new();
    table.insert(
        "scope".to_string(),
        toml::Value::String(format!("aida:lock:{slug}")),
    );
    table.insert("worktree_path".to_string(), toml::Value::String(target_str));
    table.insert(
        "started_at".to_string(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    table.insert(
        "authorized_by".to_string(),
        toml::Value::String(advisor.to_string()),
    );
    // Marks this as an entry `aida lock` itself minted (vs. a real session
    // lease we merely patched) — `release` deletes the whole file for these
    // instead of leaving an empty husk behind.
    table.insert("aida_lock_only".to_string(), toml::Value::Boolean(true));

    let out = toml::to_string_pretty(&toml::Value::Table(table))?;
    aida_core::fs_atomic::write_atomic(&path, out)
        .with_context(|| format!("could not write lease {}", path.display()))?;
    Ok(path)
}

/// Read the `authorized_by` value for the lease covering `worktree`, if any
/// lease covers it and carries one. `worktree` need not exist (best-effort
/// canonicalization for comparison — a since-removed worktree simply won't
/// match anything).
// trace:STORY-711 | ai:claude
pub(crate) fn read_authorized_by(project_root: &Path, worktree: &Path) -> Option<String> {
    let target_str = canonical_str(worktree);
    let path = find_lease_file_for_worktree(project_root, &target_str)?;
    let content = std::fs::read_to_string(&path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    value
        .get("authorized_by")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Clear the lock on `worktree` (no-op, returns `Ok(false)`, if it carries
/// none). A lock-only entry `acquire` created is deleted outright; a lock
/// patched onto a real session lease just loses the `authorized_by` key,
/// leaving the rest of the lease intact.
// trace:STORY-711 | ai:claude
pub(crate) fn release(project_root: &Path, worktree: &Path) -> Result<bool> {
    let target_str = canonical_str(worktree);
    let Some(path) = find_lease_file_for_worktree(project_root, &target_str) else {
        return Ok(false);
    };
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read lease {}", path.display()))?;
    let mut value: toml::Value = toml::from_str(&content)
        .with_context(|| format!("could not parse lease {}", path.display()))?;
    let table = value.as_table_mut().context("lease TOML is not a table")?;

    let lock_only = matches!(
        table.get("aida_lock_only"),
        Some(toml::Value::Boolean(true))
    );
    let had_lock = table.remove("authorized_by").is_some();
    if !had_lock {
        return Ok(false);
    }

    if lock_only {
        std::fs::remove_file(&path)
            .with_context(|| format!("could not remove lease {}", path.display()))?;
    } else {
        let out = toml::to_string_pretty(&value)?;
        aida_core::fs_atomic::write_atomic(&path, out)
            .with_context(|| format!("could not write lease {}", path.display()))?;
    }
    Ok(true)
}

/// List every worktree currently carrying an `authorized_by` lock. Tolerant
/// of a missing/empty `.aida/sessions/` dir (empty result) and malformed
/// files (skipped).
// trace:STORY-711 | ai:claude
pub(crate) fn list_locks(project_root: &Path) -> Vec<LockedWorktree> {
    let dir = aida_core::liveness::leases_dir(project_root);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&content) else {
            continue;
        };
        let Some(authorized_by) = value
            .get("authorized_by")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let worktree_path = value
            .get("worktree_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let scope = value
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(LockedWorktree {
            worktree_path,
            authorized_by: authorized_by.to_string(),
            scope,
        });
    }
    out.sort_by(|a, b| a.worktree_path.cmp(&b.worktree_path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_session_lease(dir: &Path, filename: &str, worktree: &Path, extra: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let content = format!(
            "id = \"abc123\"\nscope = \"FR-1\"\nslug = \"fr-1\"\nowner = \"joe\"\n\
             worktree_path = {:?}\nbranch = \"fr-1-work\"\n\
             started_at = \"2026-07-12T00:00:00Z\"\nhostname = \"host\"\n{extra}",
            worktree.to_string_lossy()
        );
        std::fs::write(dir.join(filename), content).unwrap();
    }

    #[test]
    fn acquire_creates_a_lock_only_entry_when_no_lease_covers_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("project");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();

        let path = acquire(&project_root, &worktree, "advisor-a").unwrap();
        assert!(path.exists());

        let got = read_authorized_by(&project_root, &worktree);
        assert_eq!(got.as_deref(), Some("advisor-a"));

        let locks = list_locks(&project_root);
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].authorized_by, "advisor-a");
    }

    #[test]
    fn acquire_patches_an_existing_lease_without_losing_other_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("project");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let sessions_dir = aida_core::liveness::leases_dir(&project_root);
        write_session_lease(&sessions_dir, "real.toml", &worktree, "");

        acquire(&project_root, &worktree, "advisor-b").unwrap();

        let content = std::fs::read_to_string(sessions_dir.join("real.toml")).unwrap();
        // The lock landed...
        assert!(content.contains("authorized_by"));
        assert!(content.contains("advisor-b"));
        // ...and every pre-existing field survived the patch.
        assert!(content.contains("\"abc123\""));
        assert!(content.contains("fr-1-work"));
        assert!(content.contains("\"joe\""));

        // No synthetic second file was created for an already-leased worktree.
        assert_eq!(std::fs::read_dir(&sessions_dir).unwrap().count(), 1);
    }

    #[test]
    fn release_clears_the_lock_on_a_real_lease_but_keeps_the_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("project");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let sessions_dir = aida_core::liveness::leases_dir(&project_root);
        write_session_lease(&sessions_dir, "real.toml", &worktree, "");
        acquire(&project_root, &worktree, "advisor-c").unwrap();

        let released = release(&project_root, &worktree).unwrap();
        assert!(released);
        assert_eq!(read_authorized_by(&project_root, &worktree), None);
        // The underlying lease file is still there.
        assert!(sessions_dir.join("real.toml").exists());

        // Releasing again is a no-op.
        assert!(!release(&project_root, &worktree).unwrap());
    }

    #[test]
    fn release_deletes_a_lock_only_entry_entirely() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("project");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        acquire(&project_root, &worktree, "advisor-d").unwrap();
        assert_eq!(list_locks(&project_root).len(), 1);

        assert!(release(&project_root, &worktree).unwrap());
        assert!(list_locks(&project_root).is_empty());
        assert_eq!(
            std::fs::read_dir(aida_core::liveness::leases_dir(&project_root))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn re_acquire_overwrites_the_prior_authorization() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("project");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        acquire(&project_root, &worktree, "advisor-e").unwrap();
        acquire(&project_root, &worktree, "advisor-f").unwrap();

        assert_eq!(
            read_authorized_by(&project_root, &worktree).as_deref(),
            Some("advisor-f")
        );
        // Still exactly one entry — re-acquire didn't fork a second file.
        assert_eq!(list_locks(&project_root).len(), 1);
    }

    #[test]
    fn read_authorized_by_is_none_when_nothing_locked() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path().join("project");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        assert_eq!(read_authorized_by(&project_root, &worktree), None);
    }

    #[test]
    fn list_locks_empty_when_no_sessions_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_locks(tmp.path()).is_empty());
    }
}
