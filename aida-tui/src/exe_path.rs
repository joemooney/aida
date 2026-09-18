use std::path::PathBuf;
use std::sync::OnceLock;

/// Resolve the CLI binary once so a long-running TUI survives a rebuild that
/// replaces its on-disk executable. Linux may report the old executable with
/// a ` (deleted)` suffix; use the replacement at the cleaned path when it
/// exists, otherwise let command spawning search `PATH`.
// trace:TASK-1262 | ai:codex
pub(crate) fn aida_exe_path() -> PathBuf {
    static AIDA_EXE: OnceLock<PathBuf> = OnceLock::new();
    AIDA_EXE
        .get_or_init(|| resolve_aida_exe_from(std::env::current_exe().ok()))
        .clone()
}

fn resolve_aida_exe_from(current: Option<PathBuf>) -> PathBuf {
    if let Some(path) = current {
        let lossy = path.to_string_lossy();
        let cleaned = lossy
            .strip_suffix(" (deleted)")
            .map(PathBuf::from)
            .unwrap_or(path);
        if cleaned.exists() {
            return cleaned;
        }
    }
    PathBuf::from("aida")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_executable_path_uses_replacement_at_clean_path() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("aida");
        std::fs::write(&live, b"replacement").unwrap();
        let deleted = PathBuf::from(format!("{} (deleted)", live.display()));
        assert_eq!(resolve_aida_exe_from(Some(deleted)), live);
    }

    #[test]
    fn no_other_tui_source_uses_raw_current_executable_lookup() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        fn visit(dir: &std::path::Path, files: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, files);
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                    files.push(path);
                }
            }
        }
        let mut files = Vec::new();
        visit(&src_dir, &mut files);
        for path in files {
            if path == src_dir.join("exe_path.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains(concat!("current_", "exe()")),
                "raw executable lookup in {}",
                path.display()
            );
        }
    }
}
