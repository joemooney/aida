//! Parse a project's `.mcp.json` (Claude Code's MCP server registration) and
//! extract AIDA's server entry so it can be translated into other vendors'
//! MCP config shapes — the Codex CLI and Gemini CLI targets (TASK-1046).
//!
//! Scope note (re-scoped 2026-07-07 during grooming, see TASK-1046
//! comments): `aida init` already scaffolds a project-local
//! `.codex/config.toml` with an `[mcp_servers.aida]` block at init time
//! (TASK-0424, `codex_md::generate_codex_config`). This module is NOT a
//! second codex registration path — it derives the same vendor shape from
//! whatever is *actually* registered in `.mcp.json` today (so a hand-edited
//! command/args/env, or a renamed server, survives translation), and adds
//! the Gemini CLI target the earlier task didn't cover. The file-writing
//! side (which needs `toml_edit` for a format-preserving merge into an
//! existing `.codex/config.toml` / `.gemini/settings.json`) lives in
//! `aida-cli::mcp_translate`, driven by the pure data this module extracts.
//!
//! Gemini CLI's MCP config format — verified against the official docs
//! before writing any translator, per TASK-1046's research-first
//! requirement — is a `mcpServers` object in `settings.json` (project-local
//! `.gemini/settings.json` or user `~/.gemini/settings.json`), one entry per
//! server with `command` / `args` / `env` (plus optional `cwd`, `timeout`,
//! `trust`, …, none of which AIDA's stdio registration needs):
//! <https://geminicli.com/docs/tools/mcp-server/> and
//! <https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md>
//! (both checked 2026-07-12). Structurally the same `command`/`args`/`env`
//! shape `.mcp.json` and Codex's `[mcp_servers.<name>]` already use, so the
//! same `McpServerSpec` extracted here drives both vendor renderers.
// trace:TASK-1046 | ai:claude

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// A vendor-agnostic MCP server registration — everything the Codex and
/// Gemini translations need to reproduce Claude's `.mcp.json` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Does this `.mcp.json` server entry look like AIDA's own MCP server? Used
/// both to prefer the literal `"aida"` key and, if that's absent or
/// renamed, to find it by shape (`aida ... mcp-serve`).
fn looks_like_aida_server(entry: &serde_json::Value) -> bool {
    let command = entry.get("command").and_then(|c| c.as_str()).unwrap_or("");
    let command_is_aida = command == "aida"
        || command.ends_with("/aida")
        || command.ends_with("\\aida.exe")
        || command.ends_with("/aida.exe");
    let args_run_mcp_serve = entry
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().any(|x| x.as_str() == Some("mcp-serve")))
        .unwrap_or(false);
    command_is_aida && args_run_mcp_serve
}

/// Read `.mcp.json` at `mcp_json_path` and return the AIDA server's
/// `(name, spec)`, if any. `Ok(None)` covers both "no `.mcp.json`" and
/// "`.mcp.json` exists but registers no AIDA server" — the caller reports
/// "nothing to translate" for both (TASK-1046 acceptance), not an error.
/// A malformed `.mcp.json` (present but not valid JSON, or `mcpServers` not
/// an object) *is* an error — that's a real problem worth surfacing, not a
/// silent no-op.
pub fn find_aida_mcp_server(mcp_json_path: &Path) -> Result<Option<(String, McpServerSpec)>> {
    if !mcp_json_path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(mcp_json_path)
        .with_context(|| format!("reading {}", mcp_json_path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("parsing {} as JSON", mcp_json_path.display()))?;

    let Some(servers) = value.get("mcpServers") else {
        return Ok(None);
    };
    let servers = servers.as_object().with_context(|| {
        format!(
            "{}: \"mcpServers\" must be a JSON object",
            mcp_json_path.display()
        )
    })?;

    // Prefer the literal "aida" key (and confirm it's shaped like AIDA's
    // server rather than an unrelated tool someone else named "aida");
    // otherwise scan by shape so a renamed entry is still found.
    let chosen = servers
        .get_key_value("aida")
        .filter(|(_, v)| looks_like_aida_server(v))
        .or_else(|| servers.iter().find(|(_, v)| looks_like_aida_server(v)));

    let Some((name, entry)) = chosen else {
        return Ok(None);
    };

    let command = entry
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or("aida")
        .to_string();
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

    Ok(Some((name.clone(), McpServerSpec { command, args, env })))
}

/// Render a full, fresh `.codex/config.toml` document registering `name`
/// with `spec` — the shared renderer behind both `aida init`'s hardcoded
/// AIDA registration (`codex_md::generate_codex_config`, TASK-0424) and
/// `aida mcp translate`'s data-driven one (TASK-1046), so the two codex
/// paths render byte-for-byte the same shape instead of drifting into two
/// codex registration formats.
pub fn render_codex_config_document(name: &str, spec: &McpServerSpec) -> String {
    let mut out = String::new();
    out.push_str("# AIDA Codex CLI configuration.\n#\n");
    out.push_str("# This file registers AIDA's MCP server with Codex CLI so a Codex session\n");
    out.push_str("# started from this project root discovers AIDA's spec graph + coordination\n");
    out.push_str("# tools out of the box — the Codex-side parallel to the project's `.mcp.json`\n");
    out.push_str("# (which makes the same project MCP-ready for Claude Code).\n#\n");
    out.push_str("# Codex merges this project-local config over your `~/.codex/config.toml`.\n");
    out.push_str("# Personal preferences (model, footer, etc.) belong in the user-level file;\n");
    out.push_str("# keep this one limited to the project's AIDA integration.\n\n");
    out.push_str("# Trust this project's local MCP server without a per-session prompt. This is\n");
    out.push_str(
        "# the Codex analog of the `enabledMcpjsonServers: [\"aida\"]` pre-approval AIDA\n",
    );
    out.push_str("# writes for Claude Code. Remove it if your team prefers an explicit prompt.\n");
    out.push_str("project_trust_level = \"trusted\"\n\n");
    out.push_str("# AIDA MCP server: spec graph + cross-agent coordination surface.\n");
    if spec.command == "aida" {
        out.push_str(
            "# If `aida` is not on PATH, replace `command` with the absolute binary path.\n",
        );
    }
    out.push_str(&format!("[mcp_servers.{name}]\n"));
    out.push_str(&format!("command = {:?}\n", spec.command));
    let args_toml = spec
        .args
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("args = [{args_toml}]\n"));
    if !spec.env.is_empty() {
        out.push_str(&format!("\n[mcp_servers.{name}.env]\n"));
        for (k, v) in &spec.env {
            out.push_str(&format!("{k} = {v:?}\n"));
        }
    }
    out
}

/// Render a fresh `.gemini/settings.json` document registering `name` with
/// `spec` under the top-level `mcpServers` object — Gemini CLI's MCP config
/// shape (see module docs for the verified schema source). Used for the
/// "no existing `.gemini/settings.json`" case; a pre-existing file is
/// merged key-by-key by the caller instead, to avoid clobbering unrelated
/// Gemini settings.
pub fn render_gemini_settings_document(name: &str, spec: &McpServerSpec) -> Result<String> {
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
    let mut servers = serde_json::Map::new();
    servers.insert(name.to_string(), serde_json::Value::Object(entry));
    let mut root = serde_json::Map::new();
    root.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
    Ok(serde_json::to_string_pretty(&serde_json::Value::Object(
        root,
    ))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_mcp_json(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join(".mcp.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn missing_file_is_nothing_to_translate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".mcp.json");
        assert!(find_aida_mcp_server(&path).unwrap().is_none());
    }

    #[test]
    fn finds_the_aida_server_and_preserves_command_args_env() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_mcp_json(
            tmp.path(),
            r#"{
              "mcpServers": {
                "aida": {
                  "command": "aida",
                  "args": ["mcp-serve"],
                  "env": { "AIDA_AGENT_OUTPUT": "toon" },
                  "description": "AIDA — spec graph + coordination surface."
                }
              }
            }"#,
        );
        let (name, spec) = find_aida_mcp_server(&path).unwrap().unwrap();
        assert_eq!(name, "aida");
        assert_eq!(spec.command, "aida");
        assert_eq!(spec.args, vec!["mcp-serve".to_string()]);
        assert_eq!(
            spec.env.get("AIDA_AGENT_OUTPUT").map(String::as_str),
            Some("toon")
        );
    }

    #[test]
    fn file_present_but_no_mcp_servers_is_nothing_to_translate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_mcp_json(tmp.path(), r#"{}"#);
        assert!(find_aida_mcp_server(&path).unwrap().is_none());
    }

    #[test]
    fn mcp_servers_present_but_no_aida_entry_is_nothing_to_translate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_mcp_json(
            tmp.path(),
            r#"{
              "mcpServers": {
                "github": { "command": "docker", "args": ["run", "ghcr.io/github/github-mcp-server"] }
              }
            }"#,
        );
        assert!(find_aida_mcp_server(&path).unwrap().is_none());
    }

    #[test]
    fn finds_a_renamed_aida_entry_by_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_mcp_json(
            tmp.path(),
            r#"{
              "mcpServers": {
                "spec-graph": { "command": "aida", "args": ["mcp-serve"] }
              }
            }"#,
        );
        let (name, spec) = find_aida_mcp_server(&path).unwrap().unwrap();
        assert_eq!(name, "spec-graph");
        assert_eq!(spec.command, "aida");
    }

    #[test]
    fn absolute_binary_path_still_recognized() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_mcp_json(
            tmp.path(),
            r#"{
              "mcpServers": {
                "aida": { "command": "/usr/local/bin/aida", "args": ["mcp-serve"] }
              }
            }"#,
        );
        let (_, spec) = find_aida_mcp_server(&path).unwrap().unwrap();
        assert_eq!(spec.command, "/usr/local/bin/aida");
    }

    #[test]
    fn malformed_json_is_an_error_not_a_silent_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_mcp_json(tmp.path(), "not json");
        assert!(find_aida_mcp_server(&path).is_err());
    }

    fn sample_spec() -> McpServerSpec {
        let mut env = BTreeMap::new();
        env.insert("AIDA_AGENT_OUTPUT".to_string(), "toon".to_string());
        McpServerSpec {
            command: "aida".to_string(),
            args: vec!["mcp-serve".to_string()],
            env,
        }
    }

    #[test]
    fn codex_document_is_valid_toml_and_preserves_command_args_env() {
        let toml_text = render_codex_config_document("aida", &sample_spec());
        assert!(toml_text.contains("[mcp_servers.aida]"));
        assert!(toml_text.contains("command = \"aida\""));
        assert!(toml_text.contains("args = [\"mcp-serve\"]"));
        assert!(toml_text.contains("[mcp_servers.aida.env]"));
        assert!(toml_text.contains("AIDA_AGENT_OUTPUT = \"toon\""));
        let parsed: toml::Value = toml::from_str(&toml_text).expect("must be valid TOML");
        assert_eq!(
            parsed["mcp_servers"]["aida"]["command"].as_str(),
            Some("aida")
        );
        assert_eq!(
            parsed["mcp_servers"]["aida"]["env"]["AIDA_AGENT_OUTPUT"].as_str(),
            Some("toon")
        );
    }

    #[test]
    fn gemini_document_is_valid_json_with_mcp_servers_shape() {
        let json_text = render_gemini_settings_document("aida", &sample_spec()).expect("renders");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_text).expect("must be valid JSON");
        assert_eq!(
            parsed["mcpServers"]["aida"]["command"].as_str(),
            Some("aida")
        );
        assert_eq!(
            parsed["mcpServers"]["aida"]["args"][0].as_str(),
            Some("mcp-serve")
        );
        assert_eq!(
            parsed["mcpServers"]["aida"]["env"]["AIDA_AGENT_OUTPUT"].as_str(),
            Some("toon")
        );
    }
}
