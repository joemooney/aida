// trace:TASK-3428 | ai:codex

// trace:FR-0259 | ai:claude:high
/// BUG-298: find `aida-*` entries under `.claude/{skills,commands,hooks}` that
/// no longer correspond to a template the current binary ships — left behind
/// when a template was renamed/consolidated/retired. Compared by base name, so
/// a flat `aida-x.md` and a folder-form `aida-x/` both resolve to `aida-x`.
/// Symlinks are skipped — the in-repo dogfood `.claude/` is per-file symlinks
/// into the master templates and must never be pruned. trace:BUG-298 | ai:claude
pub(crate) fn detect_obe_aida_scaffold_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    // trace:TASK-1519 | ai:codex
    use std::collections::{HashMap, HashSet};
    const DIRS: [&str; 3] = ["skills", "commands", "hooks"];

    // Expected base names per scaffold dir, from the embedded template set.
    let mut expected: HashMap<&str, HashSet<String>> =
        DIRS.iter().map(|d| (*d, HashSet::new())).collect();
    for key in aida_core::templates::EMBEDDED_TEMPLATES.keys() {
        for dir in DIRS {
            if let Some(rest) = key.strip_prefix(&format!("{dir}/")) {
                let first = rest.split('/').next().unwrap_or(rest);
                let base = std::path::Path::new(first)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(first)
                    .to_string();
                expected.get_mut(dir).unwrap().insert(base);
            }
        }
    }

    let mut obe = Vec::new();
    for dir in DIRS {
        for parent in if dir == "skills" {
            vec![".claude", ".agents"]
        } else {
            vec![".claude"]
        } {
            let d = root.join(parent).join(dir);
            if parent == ".agents"
                && (!root
                    .join(".agents")
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_dir())
                    || !d.symlink_metadata().is_ok_and(|m| m.file_type().is_dir()))
            {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            let exp = &expected[dir];
            for entry in entries.flatten() {
                let path = entry.path();
                // Never touch symlinks (the dogfood per-file symlink layout).
                if path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.starts_with("aida-") {
                    continue;
                }
                let base = std::path::Path::new(name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name)
                    .to_string();
                if !exp.contains(&base) {
                    // Shared portable skills belong to the operator unless we can
                    // prove AIDA generated this exact, unedited regular file.
                    if parent == ".agents" {
                        if !path.is_dir() {
                            continue;
                        }
                        let skill = path.join("SKILL.md");
                        if skill.symlink_metadata().is_err()
                            || skill
                                .symlink_metadata()
                                .is_ok_and(|m| !m.file_type().is_file())
                        {
                            continue;
                        }
                        let Ok(content) = std::fs::read_to_string(&skill) else {
                            continue;
                        };
                        if aida_core::scaffolding::refresh::refresh_disposition(&content)
                            != aida_core::scaffolding::refresh::RefreshDisposition::Pristine
                        {
                            continue;
                        }
                    }
                    obe.push(path);
                }
            }
        }
    }
    obe.sort();
    obe
}
