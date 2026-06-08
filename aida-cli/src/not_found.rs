//! Rich "requirement not found" errors that surface what store was queried
//! and how to point at a different one. Also (BUG-97 / TASK-223) a shared
//! formatter for parse-failure warnings, so the "what do I do next" hint
//! is consistent across `aida show`, `aida queue list`, `aida cache rebuild`,
//! etc.
//!
//! Replaces a chorus of bare `anyhow::anyhow!("Requirement not found: {}", id)`
//! call sites that gave the user no clue why the lookup failed when run from
//! the wrong directory, plus the equally-bare `Failed to parse <path>` lines
//! that pointed at the file without saying what to do.
//!
//! trace:FR-1-011 BUG-97 TASK-223 | ai:claude

use std::path::{Path, PathBuf};

/// What kind of store the path refers to. The git-canonical mode is the
/// current default; the two legacy modes still exist behind the deprecated
/// `aida init --centralized` opt-in path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
    /// Distributed git-canonical: orphan `aida-store` branch + sharded YAML
    /// objects under `<store>/objects/<TYPE>/<NN>/<SPEC-ID>.yaml`.
    GitCanonical,
    /// Legacy SQLite (`requirements.db`).
    LegacySqlite,
    /// Legacy YAML (single file).
    LegacyYaml,
    /// No store could be located.
    None,
}

impl StoreMode {
    pub fn label(self) -> &'static str {
        match self {
            StoreMode::GitCanonical => "distributed git-canonical",
            StoreMode::LegacySqlite => "legacy SQLite",
            StoreMode::LegacyYaml => "legacy YAML",
            StoreMode::None => "none",
        }
    }
}

/// Inspect a path and infer which storage mode it represents. The git-
/// canonical mode is detected by the presence of `objects/` (the sharded
/// YAML tree); SQLite by the `.db` extension; YAML by `.yaml`/`.yml`.
pub fn detect_mode(path: Option<&Path>) -> StoreMode {
    let Some(p) = path else {
        return StoreMode::None;
    };
    if !p.exists() {
        return StoreMode::None;
    }
    if p.is_dir() && p.join("objects").is_dir() {
        return StoreMode::GitCanonical;
    }
    match p.extension().and_then(|e| e.to_str()) {
        Some("db") => StoreMode::LegacySqlite,
        Some("yaml") | Some("yml") => StoreMode::LegacyYaml,
        _ => StoreMode::None,
    }
}

/// Build the multi-line "Requirement not found" error per FR-1-011.
///
/// When `store_path` resolves to a real store, the user gets the path and
/// detected mode; when it doesn't, they get the explicit "no store found"
/// case plus a hint about how to fix it.
pub fn requirement_not_found(id: &str, store_path: Option<&Path>) -> anyhow::Error {
    let mode = detect_mode(store_path);
    let mut msg = format!("Requirement not found: {}\n", id);

    match mode {
        StoreMode::None => {
            // Where did we look? Use the path the dispatcher resolved (even
            // if it doesn't exist) so the user sees what was searched.
            let searched: PathBuf = store_path
                .map(Path::to_path_buf)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            msg.push_str(&format!(
                "  Searched in: (no aida store found at {} or any parent)\n",
                searched.display()
            ));
            msg.push_str(
                "  Hint: cd into the project root (a directory containing .aida/config.toml or requirements.db),\n",
            );
            msg.push_str("        or pass --project <path> if the command supports it.");
        }
        _ => {
            let path_str = store_path
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(unknown)".to_string());
            msg.push_str(&format!("  Searched in: {}\n", path_str));
            msg.push_str(&format!("  Mode: {}\n", mode.label()));
            msg.push_str("  Hint: check the spec ID (try `aida list` or `aida search <terms>`).");
        }
    }

    anyhow::anyhow!(msg)
}

// BUG-97 / TASK-223: the shared parse-failure hint lives in
// aida_core::object_store::parse_failure_hint so both aida-cli and
// aida-core can use it. Import it directly from there.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn detects_git_canonical_by_objects_dir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("objects")).unwrap();
        assert_eq!(detect_mode(Some(tmp.path())), StoreMode::GitCanonical);
    }

    #[test]
    fn detects_sqlite_by_extension() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("requirements.db");
        fs::write(&db, b"").unwrap();
        assert_eq!(detect_mode(Some(&db)), StoreMode::LegacySqlite);
    }

    #[test]
    fn detects_yaml_by_extension() {
        let tmp = TempDir::new().unwrap();
        let yaml = tmp.path().join("requirements.yaml");
        fs::write(&yaml, b"").unwrap();
        assert_eq!(detect_mode(Some(&yaml)), StoreMode::LegacyYaml);
    }

    #[test]
    fn missing_path_is_none() {
        assert_eq!(detect_mode(None), StoreMode::None);
        assert_eq!(
            detect_mode(Some(Path::new("/nonexistent-aida-path-xyz"))),
            StoreMode::None
        );
    }

    #[test]
    fn none_mode_emits_hint_about_cwd() {
        let err = requirement_not_found("EPIC-1-001", None);
        let s = format!("{}", err);
        assert!(s.contains("Requirement not found: EPIC-1-001"));
        assert!(s.contains("no aida store found"));
        assert!(s.contains("cd into the project root"));
    }

    #[test]
    fn git_canonical_mode_emits_path_and_mode() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("objects")).unwrap();
        let err = requirement_not_found("FR-42", Some(tmp.path()));
        let s = format!("{}", err);
        assert!(s.contains("Requirement not found: FR-42"));
        assert!(s.contains(&tmp.path().display().to_string()));
        assert!(s.contains("distributed git-canonical"));
        // Hint should point at search/list, not at cwd-walking.
        assert!(s.contains("aida list") || s.contains("aida search"));
    }

    #[test]
    fn sqlite_mode_emits_legacy_label() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("requirements.db");
        fs::write(&db, b"").unwrap();
        let err = requirement_not_found("BUG-1", Some(&db));
        let s = format!("{}", err);
        assert!(s.contains("legacy SQLite"));
    }

    // BUG-97 / TASK-223: parse_failure_hint tests moved to
    // aida-core/src/object_store.rs alongside the function's new home.
}
