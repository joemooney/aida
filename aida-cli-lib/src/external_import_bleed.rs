//! TASK-696: detect an ancestor `CLAUDE.md` / `CLAUDE.local.md` / `AGENTS.md`
//! whose `@`-imports resolve OUTSIDE the project — the bleed that makes every
//! child project (a) inherit the ancestor's AI instructions and (b) trip Claude
//! Code's "Allow external CLAUDE.md file imports?" prompt on launch.
//!
//! Empirical origin (2026-06-07): `~/ai/` (parent of ~80 projects, not a git
//! repo) held a stray AIDA scaffold from an accidental init; launching any child
//! worktree prompted for the external `~/ai/docs/aida/discipline/README.md`
//! import. TASK-686 prevents NEW such scaffolds at `aida init`; this is the
//! detect-half (an `aida doctor` finding). Companion to that prevention.
//!
//! This module is the PURE core: parsing `@`-imports and deciding whether one
//! escapes the project root. The filesystem walk (find ancestor instruction
//! files, read them) lives in `main.rs::collect_doctor_findings`, which calls
//! these. Keeping the decision pure makes it unit-testable with no fs.
//! trace:TASK-696 | ai:claude

use std::path::{Component, Path, PathBuf};

/// The instruction-file names Claude Code (and Codex) load from ancestors.
pub(crate) const ANCESTOR_INSTRUCTION_FILES: &[&str] =
    &["CLAUDE.md", "CLAUDE.local.md", "AGENTS.md"];

/// Extract the `@`-import targets from an instruction file's content. Claude
/// Code's import syntax is a line whose first non-space token begins with `@`
/// (e.g. `@docs/aida/discipline/README.md`). Conservative: we take the token
/// after `@` up to the first whitespace, and ignore `@` mid-sentence (only a
/// leading `@` on the trimmed line counts). trace:TASK-696
pub(crate) fn parse_at_imports(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let rest = trimmed.strip_prefix('@')?;
            let token = rest.split_whitespace().next().unwrap_or("");
            if token.is_empty() {
                None
            } else {
                Some(token.to_string())
            }
        })
        .collect()
}

/// Lexically normalize a path (resolve `.` and `..` without touching the
/// filesystem) so `starts_with` is meaningful. Does not follow symlinks — a
/// best-effort prefix check, which is all the bleed heuristic needs.
/// trace:TASK-696
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                // Pop the last normal component; keep `..` if nothing to pop
                // (e.g. a relative path that climbs above its anchor).
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Does `import` — an `@`-import found in an instruction file located in
/// `file_dir` — resolve to a path OUTSIDE `project_root`? Relative imports
/// resolve against the file's own directory (how Claude Code resolves them);
/// absolute imports are taken as-is. An import that resolves within
/// `project_root` is fine (not a bleed). trace:TASK-696
pub(crate) fn import_escapes_project(file_dir: &Path, import: &str, project_root: &Path) -> bool {
    let raw = Path::new(import);
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        file_dir.join(raw)
    };
    let resolved = lexical_normalize(&resolved);
    let root = lexical_normalize(project_root);
    !resolved.starts_with(&root)
}

/// TASK-699 (heal half): given a stray ancestor instruction file's directory,
/// its current on-disk content, and the project root, decide whether the file
/// STILL has at least one `@`-import escaping the project — i.e. whether it
/// should be removed by the opt-in `external-import-bleed` heal. Pure (no fs):
/// the caller reads the file fresh right before deletion and passes the content
/// here, mirroring `heal_doctor_dead_agent`'s re-check-before-acting discipline
/// so a file edited between scan and heal (no longer escaping) is left alone.
// trace:TASK-699 | ai:claude
pub(crate) fn file_still_escapes(file_dir: &Path, content: &str, project_root: &Path) -> bool {
    parse_at_imports(content)
        .iter()
        .any(|imp| import_escapes_project(file_dir, imp, project_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_at_imports_takes_leading_at_tokens_only() {
        let content = "\
# CLAUDE.md
@.claude/AIDA.md
some prose mentioning an @handle inline — not an import
   @docs/aida/discipline/README.md   trailing note
@
plain line
";
        assert_eq!(
            parse_at_imports(content),
            vec![
                ".claude/AIDA.md".to_string(),
                "docs/aida/discipline/README.md".to_string(),
            ]
        );
    }

    #[test]
    fn relative_import_in_ancestor_escapes_project() {
        // ~/ai/CLAUDE.md @docs/... resolves to ~/ai/docs/... — OUTSIDE ~/ai/aida.
        let ancestor_dir = Path::new("/home/joe/ai");
        let project_root = Path::new("/home/joe/ai/aida");
        assert!(import_escapes_project(
            ancestor_dir,
            "docs/aida/discipline/README.md",
            project_root
        ));
    }

    #[test]
    fn import_resolving_into_project_is_not_a_bleed() {
        // An import that resolves to a path under project_root is fine.
        let file_dir = Path::new("/home/joe/ai/aida");
        let project_root = Path::new("/home/joe/ai/aida");
        assert!(!import_escapes_project(
            file_dir,
            "docs/aida/discipline/README.md",
            project_root
        ));
        // Even climbing then back in stays inside.
        assert!(!import_escapes_project(
            Path::new("/home/joe/ai/aida/sub"),
            "../docs/x.md",
            project_root
        ));
    }

    #[test]
    fn file_still_escapes_decides_removal() {
        let ancestor_dir = Path::new("/home/joe/ai");
        let project_root = Path::new("/home/joe/ai/aida");
        // A stray scaffold whose @-import escapes → would be removed.
        let stray = "# CLAUDE.md\n@docs/aida/discipline/README.md\n";
        assert!(file_still_escapes(ancestor_dir, stray, project_root));
        // A file whose only @-import now resolves inside the project (e.g. edited
        // between scan and heal) → NOT removed.
        let fixed = "# CLAUDE.md\n@aida/CLAUDE.md\n";
        assert!(!file_still_escapes(ancestor_dir, fixed, project_root));
        // A file with no @-imports at all → NOT removed.
        let no_imports = "# CLAUDE.md\njust prose, no imports\n";
        assert!(!file_still_escapes(ancestor_dir, no_imports, project_root));
        // Empty file → NOT removed.
        assert!(!file_still_escapes(ancestor_dir, "", project_root));
    }

    #[test]
    fn absolute_import_outside_project_escapes() {
        assert!(import_escapes_project(
            Path::new("/home/joe/ai"),
            "/etc/something.md",
            Path::new("/home/joe/ai/aida")
        ));
        // Absolute import that points back inside the project is fine.
        assert!(!import_escapes_project(
            Path::new("/home/joe/ai"),
            "/home/joe/ai/aida/CLAUDE.md",
            Path::new("/home/joe/ai/aida")
        ));
    }
}
