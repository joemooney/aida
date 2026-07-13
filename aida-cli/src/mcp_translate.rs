//! `aida mcp translate` — write the Codex CLI and Gemini CLI equivalents of
//! this project's `.mcp.json` AIDA server registration (TASK-1046).
//!
//! Extraction of the AIDA server entry from `.mcp.json` (the pure, no-I/O
//! part) lives in `aida_core::scaffolding::mcp_translate`, along with the
//! shared "brand new file" renderers both this command and `aida init`'s
//! codex scaffold (TASK-0424) use. This module owns the vendor *file*
//! writers: merging into an already-existing `.codex/config.toml` needs
//! `toml_edit`'s format-preserving document editor (this crate's existing
//! convention for config.toml writers — see `config_edit.rs`,
//! `glyph_config.rs`), which isn't a dependency of `aida-core`; the
//! equivalent JSON merge for `.gemini/settings.json` is the natural
//! counterpart to keep alongside it.

use aida_core::scaffolding::mcp_translate::{
    find_aida_mcp_server, render_codex_config_document, render_gemini_settings_document,
    McpServerSpec,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

/// What happened (or, under `--dry-run`, would happen) to one target file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslateAction {
    /// The file didn't exist, or existed without an aida entry — wrote (or
    /// would write) a new / additively-merged entry. No `--force` needed:
    /// this is purely additive, nothing pre-existing was overwritten.
    Created,
    /// The file existed with a *different* aida entry; `--force` overwrote it.
    Updated,
    /// The file already had exactly the entry `.mcp.json` would produce —
    /// nothing to do.
    UpToDate,
    /// The file existed with a different aida entry and `--force` wasn't given.
    SkippedExisting,
}

impl TranslateAction {
    pub fn wrote(self) -> bool {
        matches!(self, TranslateAction::Created | TranslateAction::Updated)
    }
}

#[derive(Debug, Clone)]
pub struct TargetOutcome {
    pub path: PathBuf,
    pub action: TranslateAction,
}

#[derive(Debug, Clone)]
pub enum TranslateReport {
    /// No `.mcp.json`, or one with no aida server registered.
    NothingToTranslate { mcp_json_path: PathBuf },
    Translated {
        server_name: String,
        codex: TargetOutcome,
        gemini: TargetOutcome,
    },
}

/// Translate `<project_root>/.mcp.json`'s aida server into
/// `.codex/config.toml` and `.gemini/settings.json`. `dry_run` computes the
/// same report without writing anything.
pub fn translate(project_root: &Path, force: bool, dry_run: bool) -> Result<TranslateReport> {
    let mcp_json_path = project_root.join(".mcp.json");
    let Some((name, spec)) = find_aida_mcp_server(&mcp_json_path)? else {
        return Ok(TranslateReport::NothingToTranslate { mcp_json_path });
    };

    let codex_path = project_root.join(".codex").join("config.toml");
    let codex = translate_codex(&codex_path, &name, &spec, force, dry_run)?;

    let gemini_path = project_root.join(".gemini").join("settings.json");
    let gemini = translate_gemini(&gemini_path, &name, &spec, force, dry_run)?;

    Ok(TranslateReport::Translated {
        server_name: name,
        codex,
        gemini,
    })
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(body) => Ok(Some(body)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_file(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    aida_core::write_atomic(path, body).with_context(|| format!("writing {}", path.display()))
}

// ---------------------------------------------------------------------
// Codex: `.codex/config.toml`, `[mcp_servers.<name>]`
// ---------------------------------------------------------------------

fn read_codex_entry(table: &Table) -> Option<McpServerSpec> {
    let command = table.get("command")?.as_str()?.to_string();
    let args = table
        .get("args")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env = table
        .get("env")
        .and_then(|i| i.as_table())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Some(McpServerSpec { command, args, env })
}

fn ensure_subtable<'a>(parent: &'a mut Table, key: &str) -> &'a mut Table {
    if !parent.contains_table(key) {
        parent.insert(key, Item::Table(Table::new()));
    }
    parent[key]
        .as_table_mut()
        .expect("just-inserted/confirmed table")
}

fn write_codex_entry(doc: &mut DocumentMut, name: &str, spec: &McpServerSpec) {
    let root = doc.as_table_mut();
    let mcp_servers = ensure_subtable(root, "mcp_servers");
    let server = ensure_subtable(mcp_servers, name);

    server.insert("command", Item::Value(Value::from(spec.command.clone())));
    let mut arr = Array::new();
    for a in &spec.args {
        arr.push(a.as_str());
    }
    server.insert("args", Item::Value(Value::Array(arr)));

    if spec.env.is_empty() {
        server.remove("env");
    } else {
        // Rebuild the env subtable so removed keys don't linger.
        server.remove("env");
        let env_tbl = ensure_subtable(server, "env");
        for (k, v) in &spec.env {
            env_tbl.insert(k, Item::Value(Value::from(v.clone())));
        }
    }
}

fn translate_codex(
    path: &Path,
    name: &str,
    spec: &McpServerSpec,
    force: bool,
    dry_run: bool,
) -> Result<TargetOutcome> {
    let existing_body = read_optional(path)?;
    let mut doc: DocumentMut = match &existing_body {
        Some(body) => body
            .parse()
            .with_context(|| format!("parsing {}", path.display()))?,
        None => DocumentMut::new(),
    };

    let current = doc
        .get("mcp_servers")
        .and_then(|i| i.as_table())
        .and_then(|t| t.get(name))
        .and_then(|i| i.as_table())
        .and_then(read_codex_entry);

    let action = if current.as_ref() == Some(spec) {
        TranslateAction::UpToDate
    } else if current.is_none() {
        // Nothing pre-existing to clobber — purely additive.
        TranslateAction::Created
    } else if force {
        TranslateAction::Updated
    } else {
        TranslateAction::SkippedExisting
    };

    if dry_run || !action.wrote() {
        return Ok(TargetOutcome {
            path: path.to_path_buf(),
            action,
        });
    }

    if existing_body.is_none() {
        // Brand new file: use the shared full-document renderer (header
        // comments + trust line) instead of a bare toml_edit table, so a
        // fresh translate-created file looks like the init-scaffolded one.
        write_file(path, &render_codex_config_document(name, spec))?;
    } else {
        write_codex_entry(&mut doc, name, spec);
        write_file(path, &doc.to_string())?;
    }

    Ok(TargetOutcome {
        path: path.to_path_buf(),
        action,
    })
}

// ---------------------------------------------------------------------
// Gemini: `.gemini/settings.json`, `mcpServers.<name>`
// ---------------------------------------------------------------------

fn read_gemini_entry(value: &serde_json::Value, name: &str) -> Option<McpServerSpec> {
    let entry = value.get("mcpServers")?.get(name)?;
    let command = entry.get("command")?.as_str()?.to_string();
    let args = entry
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env = entry
        .get("env")
        .and_then(|e| e.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Some(McpServerSpec { command, args, env })
}

fn write_gemini_entry(root: &mut serde_json::Value, name: &str, spec: &McpServerSpec) {
    if !root.is_object() {
        *root = serde_json::json!({});
    }
    let root_obj = root.as_object_mut().expect("just-ensured object");
    let servers = root_obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        *servers = serde_json::json!({});
    }
    let servers_obj = servers.as_object_mut().expect("just-ensured object");

    let mut entry = serde_json::Map::new();
    entry.insert(
        "command".to_string(),
        serde_json::Value::String(spec.command.clone()),
    );
    entry.insert(
        "args".to_string(),
        serde_json::Value::Array(
            spec.args
                .iter()
                .map(|a| serde_json::Value::String(a.clone()))
                .collect(),
        ),
    );
    if !spec.env.is_empty() {
        let mut env = serde_json::Map::new();
        for (k, v) in &spec.env {
            env.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        entry.insert("env".to_string(), serde_json::Value::Object(env));
    }
    servers_obj.insert(name.to_string(), serde_json::Value::Object(entry));
}

fn translate_gemini(
    path: &Path,
    name: &str,
    spec: &McpServerSpec,
    force: bool,
    dry_run: bool,
) -> Result<TargetOutcome> {
    let existing_body = read_optional(path)?;
    let mut root: serde_json::Value = match &existing_body {
        Some(body) => serde_json::from_str(body)
            .with_context(|| format!("parsing {} as JSON", path.display()))?,
        None => serde_json::json!({}),
    };

    let current = read_gemini_entry(&root, name);

    let action = if current.as_ref() == Some(spec) {
        TranslateAction::UpToDate
    } else if current.is_none() {
        TranslateAction::Created
    } else if force {
        TranslateAction::Updated
    } else {
        TranslateAction::SkippedExisting
    };

    if dry_run || !action.wrote() {
        return Ok(TargetOutcome {
            path: path.to_path_buf(),
            action,
        });
    }

    if existing_body.is_none() {
        write_file(path, &render_gemini_settings_document(name, spec)?)?;
    } else {
        write_gemini_entry(&mut root, name, spec);
        write_file(path, &serde_json::to_string_pretty(&root)?)?;
    }

    Ok(TargetOutcome {
        path: path.to_path_buf(),
        action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn spec() -> McpServerSpec {
        McpServerSpec {
            command: "aida".to_string(),
            args: vec!["mcp-serve".to_string()],
            env: BTreeMap::new(),
        }
    }

    fn spec_with_env() -> McpServerSpec {
        let mut env = BTreeMap::new();
        env.insert("AIDA_AGENT_OUTPUT".to_string(), "toon".to_string());
        McpServerSpec {
            command: "aida".to_string(),
            args: vec!["mcp-serve".to_string()],
            env,
        }
    }

    fn write_mcp_json(dir: &Path, name: &str, spec: &McpServerSpec) {
        let mut entry = serde_json::json!({
            "command": spec.command,
            "args": spec.args,
        });
        if !spec.env.is_empty() {
            entry["env"] = serde_json::to_value(&spec.env).unwrap();
        }
        let body = serde_json::json!({ "mcpServers": { name: entry } });
        std::fs::write(
            dir.join(".mcp.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn no_mcp_json_reports_nothing_to_translate() {
        let tmp = tempfile::tempdir().unwrap();
        let report = translate(tmp.path(), false, false).unwrap();
        assert!(matches!(report, TranslateReport::NothingToTranslate { .. }));
    }

    #[test]
    fn mcp_json_without_aida_server_reports_nothing_to_translate() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".mcp.json"),
            r#"{"mcpServers": {"other": {"command": "npx", "args": ["-y", "some-tool"]}}}"#,
        )
        .unwrap();
        let report = translate(tmp.path(), false, false).unwrap();
        assert!(matches!(report, TranslateReport::NothingToTranslate { .. }));
    }

    #[test]
    fn fresh_project_writes_both_valid_configs() {
        let tmp = tempfile::tempdir().unwrap();
        write_mcp_json(tmp.path(), "aida", &spec_with_env());

        let report = translate(tmp.path(), false, false).unwrap();
        let TranslateReport::Translated { codex, gemini, .. } = report else {
            panic!("expected Translated");
        };
        assert_eq!(codex.action, TranslateAction::Created);
        assert_eq!(gemini.action, TranslateAction::Created);

        let codex_body = std::fs::read_to_string(&codex.path).unwrap();
        let codex_toml: toml::Value = toml::from_str(&codex_body).unwrap();
        assert_eq!(
            codex_toml["mcp_servers"]["aida"]["command"].as_str(),
            Some("aida")
        );
        assert_eq!(
            codex_toml["mcp_servers"]["aida"]["env"]["AIDA_AGENT_OUTPUT"].as_str(),
            Some("toon")
        );

        let gemini_body = std::fs::read_to_string(&gemini.path).unwrap();
        let gemini_json: serde_json::Value = serde_json::from_str(&gemini_body).unwrap();
        assert_eq!(
            gemini_json["mcpServers"]["aida"]["command"].as_str(),
            Some("aida")
        );
        assert_eq!(
            gemini_json["mcpServers"]["aida"]["env"]["AIDA_AGENT_OUTPUT"].as_str(),
            Some("toon")
        );
    }

    #[test]
    fn rerun_is_idempotent_up_to_date() {
        let tmp = tempfile::tempdir().unwrap();
        write_mcp_json(tmp.path(), "aida", &spec());
        translate(tmp.path(), false, false).unwrap();

        let second = translate(tmp.path(), false, false).unwrap();
        let TranslateReport::Translated { codex, gemini, .. } = second else {
            panic!("expected Translated");
        };
        assert_eq!(codex.action, TranslateAction::UpToDate);
        assert_eq!(gemini.action, TranslateAction::UpToDate);
    }

    #[test]
    fn differing_existing_entry_is_skipped_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        write_mcp_json(tmp.path(), "aida", &spec());
        translate(tmp.path(), false, false).unwrap();

        // .mcp.json's command changes (e.g. hand-edited to an absolute path).
        let mut changed = spec();
        changed.command = "/usr/local/bin/aida".to_string();
        write_mcp_json(tmp.path(), "aida", &changed);

        let report = translate(tmp.path(), false, false).unwrap();
        let TranslateReport::Translated { codex, gemini, .. } = report else {
            panic!("expected Translated");
        };
        assert_eq!(codex.action, TranslateAction::SkippedExisting);
        assert_eq!(gemini.action, TranslateAction::SkippedExisting);

        // Files were NOT touched.
        let codex_body = std::fs::read_to_string(&codex.path).unwrap();
        assert!(codex_body.contains("command = \"aida\""));
        let gemini_body = std::fs::read_to_string(&gemini.path).unwrap();
        assert!(gemini_body.contains("\"command\": \"aida\""));
    }

    #[test]
    fn force_overwrites_a_differing_existing_entry() {
        let tmp = tempfile::tempdir().unwrap();
        write_mcp_json(tmp.path(), "aida", &spec());
        translate(tmp.path(), false, false).unwrap();

        let mut changed = spec();
        changed.command = "/usr/local/bin/aida".to_string();
        write_mcp_json(tmp.path(), "aida", &changed);

        let report = translate(tmp.path(), true, false).unwrap();
        let TranslateReport::Translated { codex, gemini, .. } = report else {
            panic!("expected Translated");
        };
        assert_eq!(codex.action, TranslateAction::Updated);
        assert_eq!(gemini.action, TranslateAction::Updated);

        let codex_body = std::fs::read_to_string(&codex.path).unwrap();
        assert!(codex_body.contains("command = \"/usr/local/bin/aida\""));
        let gemini_body = std::fs::read_to_string(&gemini.path).unwrap();
        assert!(gemini_body.contains("\"command\": \"/usr/local/bin/aida\""));
    }

    #[test]
    fn dry_run_never_writes_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_mcp_json(tmp.path(), "aida", &spec());

        let report = translate(tmp.path(), false, true).unwrap();
        let TranslateReport::Translated { codex, gemini, .. } = report else {
            panic!("expected Translated");
        };
        assert_eq!(codex.action, TranslateAction::Created);
        assert_eq!(gemini.action, TranslateAction::Created);
        assert!(!codex.path.exists());
        assert!(!gemini.path.exists());
    }

    #[test]
    fn merge_preserves_unrelated_codex_config_content() {
        let tmp = tempfile::tempdir().unwrap();
        write_mcp_json(tmp.path(), "aida", &spec());
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        std::fs::write(
            tmp.path().join(".codex/config.toml"),
            "model = \"some-model\"\n\n[mcp_servers.other_tool]\ncommand = \"other\"\nargs = []\n",
        )
        .unwrap();

        let report = translate(tmp.path(), false, false).unwrap();
        let TranslateReport::Translated { codex, .. } = report else {
            panic!("expected Translated");
        };
        assert_eq!(codex.action, TranslateAction::Created);
        let body = std::fs::read_to_string(&codex.path).unwrap();
        assert!(body.contains("model = \"some-model\""));
        assert!(body.contains("[mcp_servers.other_tool]"));
        assert!(body.contains("[mcp_servers.aida]"));
    }

    #[test]
    fn merge_preserves_unrelated_gemini_settings_content() {
        let tmp = tempfile::tempdir().unwrap();
        write_mcp_json(tmp.path(), "aida", &spec());
        std::fs::create_dir_all(tmp.path().join(".gemini")).unwrap();
        std::fs::write(
            tmp.path().join(".gemini/settings.json"),
            r#"{"theme": "dark", "mcpServers": {"other": {"command": "other", "args": []}}}"#,
        )
        .unwrap();

        let report = translate(tmp.path(), false, false).unwrap();
        let TranslateReport::Translated { gemini, .. } = report else {
            panic!("expected Translated");
        };
        assert_eq!(gemini.action, TranslateAction::Created);
        let body = std::fs::read_to_string(&gemini.path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["theme"].as_str(), Some("dark"));
        assert_eq!(
            parsed["mcpServers"]["other"]["command"].as_str(),
            Some("other")
        );
        assert_eq!(
            parsed["mcpServers"]["aida"]["command"].as_str(),
            Some("aida")
        );
    }
}
