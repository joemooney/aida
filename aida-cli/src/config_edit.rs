//! Generic section-preserving TOML writer for `aida config menu` in-place edits
//! (STORY-669). Sets `[section] key = value` while preserving every other key,
//! section, comment, and the file's ordering — the same `toml_edit` machinery
//! `glyph_config` uses for `[glyphs]`, generalized to an arbitrary section/key.
//!
//! The small primitive duplication with `glyph_config` is intentional for this
//! slice; STORY-671 (the central config registry) is where the two writers
//! consolidate behind one source of truth. trace:STORY-669 | ai:claude

use anyhow::{Context, Result};
use std::path::Path;
use toml_edit::{DocumentMut, Item, Table, Value};

/// Set `[section] key = value`, preserving the rest of the file. Creates the
/// file and/or the section if absent.
pub(crate) fn set_kv(path: &Path, section: &str, key: &str, value: Value) -> Result<()> {
    let mut doc = load_doc(path)?;
    let tbl = table_mut(&mut doc, section);
    tbl.insert(key, Item::Value(value));
    save_doc(path, &doc)
}

/// Load a `config.toml` into an editable document, or a fresh empty one if the
/// file is absent. Parse errors surface — never clobber a malformed file.
fn load_doc(path: &Path) -> Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(body) => body
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn save_doc(path: &Path, doc: &DocumentMut) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    aida_core::write_atomic(path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        (d, p)
    }

    #[test]
    fn set_kv_preserves_unrelated_keys_and_comments() {
        let (_d, p) = tmp_path();
        std::fs::write(
            &p,
            "# top comment\n[telemetry]\nenabled = true # inline\n\n[other]\nkeep = 1\n",
        )
        .unwrap();
        set_kv(&p, "telemetry", "enabled", Value::from(false)).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(
            body.contains("# top comment"),
            "top comment preserved: {body}"
        );
        assert!(body.contains("enabled = false"), "value flipped: {body}");
        assert!(
            body.contains("[other]") && body.contains("keep = 1"),
            "unrelated section preserved: {body}"
        );
    }

    #[test]
    fn set_kv_creates_section_when_absent() {
        let (_d, p) = tmp_path();
        std::fs::write(&p, "[existing]\nx = 1\n").unwrap();
        set_kv(&p, "field_study", "enabled", Value::from(true)).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(
            body.contains("[field_study]") && body.contains("enabled = true"),
            "new section written: {body}"
        );
        assert!(body.contains("[existing]"), "existing section kept: {body}");
    }

    #[test]
    fn set_kv_creates_file_when_absent() {
        let (_d, p) = tmp_path();
        set_kv(&p, "hints", "workflow_hints", Value::from(false)).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(
            body.contains("[hints]") && body.contains("workflow_hints = false"),
            "file created with the kv: {body}"
        );
    }

    #[test]
    fn set_kv_round_trips_bool() {
        let (_d, p) = tmp_path();
        set_kv(&p, "telemetry", "enabled", Value::from(false)).unwrap();
        set_kv(&p, "telemetry", "enabled", Value::from(true)).unwrap();
        let doc = std::fs::read_to_string(&p)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(doc["telemetry"]["enabled"].as_bool(), Some(true));
    }
}
