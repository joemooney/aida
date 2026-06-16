//! Format-preserving writers for the glyph config surface (EPIC-45 phase 4).
//!
//! `aida config glyph set/unset/reset/theme` mutate `[glyphs]` / `[ui] theme`
//! in a `config.toml`. These writers use `toml_edit` so the rest of the file —
//! comments, key order, other sections — is preserved byte-for-byte.
//!
//! Resolution logic lives in [`crate::glyphs`]; this module is purely the
//! disk-writer half. trace:STORY-633 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Table, Value};

/// Which `config.toml` a glyph command targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Project-level `.aida/config.toml` (default).
    Project,
    /// User-level `~/.aida/config.toml` (`--user`).
    User,
}

/// Resolve the `config.toml` path for `scope`. The project path is derived from
/// [`crate::find_project_root`]; the user path from the home dir.
pub(crate) fn config_path_for(scope: Scope) -> Result<PathBuf> {
    match scope {
        Scope::Project => {
            let root = crate::find_project_root()
                .context("not inside an AIDA project — run `aida init`, or use `--user`")?;
            Ok(root.join(".aida").join("config.toml"))
        }
        Scope::User => {
            let home = home_dir()
                .ok_or_else(|| anyhow::anyhow!("could not resolve home directory for --user"))?;
            Ok(home.join(".aida").join("config.toml"))
        }
    }
}

/// Honors the test override so unit tests don't read the real home.
fn home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("AIDA_TEST_HOME") {
        return Some(PathBuf::from(home).join("home"));
    }
    dirs::home_dir()
}

/// Load a `config.toml` into an editable document, or a fresh empty one if the
/// file is absent. Parse errors surface (we don't want to clobber a malformed
/// file silently).
fn load_doc(path: &Path) -> Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(body) => body
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Write a document back, creating the parent dir if needed.
fn save_doc(path: &Path, doc: &DocumentMut) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    aida_core::write_atomic(path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Ensure `doc[key]` is a table, returning a mutable reference. Preserves an
/// existing table.
fn table_mut<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
    if !doc.contains_table(key) {
        doc.insert(key, Item::Table(Table::new()));
    }
    doc[key]
        .as_table_mut()
        .expect("just-inserted/confirmed table")
}

/// Set `[glyphs] <name> = "<value>"`, preserving the rest of the file.
/// `name` is assumed already validated by the caller. trace:STORY-633
pub(crate) fn set_override(path: &Path, name: &str, value: &str) -> Result<()> {
    let mut doc = load_doc(path)?;
    let glyphs = table_mut(&mut doc, "glyphs");
    glyphs.insert(name, Item::Value(Value::from(value)));
    save_doc(path, &doc)
}

/// Remove `[glyphs] <name>`. Returns whether an entry was actually present.
/// Drops the `[glyphs]` table entirely if it becomes empty. trace:STORY-633
pub(crate) fn unset_override(path: &Path, name: &str) -> Result<bool> {
    let mut doc = load_doc(path)?;
    let mut removed = false;
    if let Some(glyphs) = doc.get_mut("glyphs").and_then(|i| i.as_table_mut()) {
        removed = glyphs.remove(name).is_some();
        if glyphs.is_empty() {
            doc.remove("glyphs");
        }
    }
    if removed {
        save_doc(path, &doc)?;
    }
    Ok(removed)
}

/// Clear the whole `[glyphs]` table. Returns whether anything was removed.
/// trace:STORY-633
pub(crate) fn reset_overrides(path: &Path) -> Result<bool> {
    let mut doc = load_doc(path)?;
    let removed = doc.remove("glyphs").is_some();
    if removed {
        save_doc(path, &doc)?;
    }
    Ok(removed)
}

/// Write `[ui] theme = "<name>"`, preserving the rest of the file. Clears any
/// expanded `[glyphs]` table the user previously materialized? No — leave it;
/// per-symbol overrides intentionally win over the theme (precedence tier).
/// trace:STORY-633
pub(crate) fn set_theme(path: &Path, name: &str) -> Result<()> {
    let mut doc = load_doc(path)?;
    let ui = table_mut(&mut doc, "ui");
    ui.insert("theme", Item::Value(Value::from(name)));
    save_doc(path, &doc)
}

/// Materialize a theme's bundle into `[glyphs]` and set `[ui] glyphs` to the
/// theme's base profile, instead of writing a named `[ui] theme` reference
/// (`--expand`). Removes any existing `[ui] theme` so the expanded form is the
/// single source. trace:STORY-633
pub(crate) fn expand_theme(path: &Path, base_profile: &str, bundle: &[(&str, &str)]) -> Result<()> {
    let mut doc = load_doc(path)?;
    {
        let ui = table_mut(&mut doc, "ui");
        ui.insert("glyphs", Item::Value(Value::from(base_profile)));
        ui.remove("theme");
    }
    {
        let glyphs = table_mut(&mut doc, "glyphs");
        for (k, v) in bundle {
            glyphs.insert(k, Item::Value(Value::from(*v)));
        }
    }
    save_doc(path, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn set_override_preserves_unrelated_keys_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(
            &path,
            "# top comment\n[node]\nid = \"alice\"  # inline\n\n[ui]\nglyphs = \"unicode\"\n",
        );

        set_override(&path, "check", "OK").unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        // Unrelated content preserved verbatim.
        assert!(out.contains("# top comment"));
        assert!(out.contains("id = \"alice\"  # inline"));
        assert!(out.contains("glyphs = \"unicode\""));
        // New override landed under [glyphs].
        assert!(out.contains("[glyphs]"));
        assert!(out.contains("check = \"OK\""));
    }

    #[test]
    fn set_override_creates_file_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        set_override(&path, "cross", "NO").unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[glyphs]"));
        assert!(out.contains("cross = \"NO\""));
    }

    #[test]
    fn unset_removes_one_and_keeps_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(&path, "[glyphs]\ncheck = \"A\"\ncross = \"B\"\n");

        assert!(unset_override(&path, "check").unwrap());
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("check ="));
        assert!(out.contains("cross = \"B\""));
        // Removing a non-existent key reports false and doesn't error.
        assert!(!unset_override(&path, "nope").unwrap());
    }

    #[test]
    fn unset_last_override_drops_glyphs_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(
            &path,
            "[ui]\nglyphs = \"unicode\"\n\n[glyphs]\ncheck = \"A\"\n",
        );
        assert!(unset_override(&path, "check").unwrap());
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("[glyphs]"));
        // [ui] survives.
        assert!(out.contains("glyphs = \"unicode\""));
    }

    #[test]
    fn reset_clears_all_overrides_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(
            &path,
            "[ui]\ntheme = \"nerd-font\"\n\n[glyphs]\ncheck = \"A\"\ncross = \"B\"\n",
        );
        assert!(reset_overrides(&path).unwrap());
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("[glyphs]"));
        // Theme reference + [ui] untouched.
        assert!(out.contains("theme = \"nerd-font\""));
        // Idempotent: second reset reports nothing removed.
        assert!(!reset_overrides(&path).unwrap());
    }

    #[test]
    fn set_theme_writes_reference_and_preserves_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(&path, "[node]\nid = \"bob\"\n");
        set_theme(&path, "minimal").unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("id = \"bob\""));
        assert!(out.contains("[ui]"));
        assert!(out.contains("theme = \"minimal\""));
    }

    #[test]
    fn expand_theme_materializes_bundle_and_drops_reference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(&path, "[ui]\ntheme = \"old\"\n");
        expand_theme(&path, "unicode", &[("check", "✔"), ("done", "●")]).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("theme ="));
        assert!(out.contains("glyphs = \"unicode\""));
        assert!(out.contains("[glyphs]"));
        assert!(out.contains("check = \"✔\""));
        assert!(out.contains("done = \"●\""));
    }
}
