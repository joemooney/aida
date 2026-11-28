//! `aida init --report-allowlists`: detect the enabled agent CLIs and the
//! forge hosts named by the repository's git remotes, then report the
//! per-agent sandbox allowlist and permission entries init would tailor.
//! Read-only unless `--write` (or an interactive yes) applies the reported
//! entries additively. trace:STORY-2838 | ai:claude

use super::{detected_agent_selection, parse_agent_selection, AgentSelection};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// One forge host and the remotes that name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ForgeHost {
    pub host: String,
    pub remotes: Vec<String>,
}

/// One setting init would write for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SettingEntry {
    pub file: &'static str,
    pub key: &'static str,
    pub entries: Vec<String>,
    /// One value (set only when absent) rather than a list.
    #[serde(skip)]
    pub scalar: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentAllowlist {
    pub agent: &'static str,
    pub settings: Vec<SettingEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AllowlistReport {
    pub forge_hosts: Vec<ForgeHost>,
    pub agents: Vec<AgentAllowlist>,
    /// False only when `--write` (or a confirmed prompt) changed a file.
    pub dry_run: bool,
}

/// The companion hosts a known public forge needs beyond its web host
/// (API, release assets, archive downloads).
fn forge_companion_hosts(host: &str) -> &'static [&'static str] {
    match host {
        "github.com" => &[
            "github.com",
            "api.github.com",
            "objects.githubusercontent.com",
            "codeload.github.com",
        ],
        _ => &[],
    }
}

/// Forge hosts named by `(remote name, url)` pairs: lowercased, validated as
/// exact DNS hostnames, de-duplicated, sorted. Local-path and malformed
/// remotes contribute nothing.
pub(crate) fn forge_hosts_from_remotes(remotes: &[(String, String)]) -> Vec<ForgeHost> {
    let mut by_host: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, url) in remotes {
        let Some(host) = crate::forge::forge_host_of(url) else {
            continue;
        };
        let host = host.trim().to_ascii_lowercase();
        if crate::launch::contained::validate_allowed_host(&host).is_err() || !host.contains('.') {
            continue;
        }
        let companions = forge_companion_hosts(&host);
        let hosts: Vec<String> = if companions.is_empty() {
            vec![host]
        } else {
            companions.iter().map(|h| h.to_string()).collect()
        };
        for h in hosts {
            let names = by_host.entry(h).or_default();
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    by_host
        .into_iter()
        .map(|(host, remotes)| ForgeHost { host, remotes })
        .collect()
}

/// Every configured remote's fetch and push URL (`git remote -v`). A
/// directory that is not a git repository's top level (including a DIR
/// nested inside an enclosing repository), or has no remotes, yields none.
pub(crate) fn read_git_remotes(root: &Path) -> Vec<(String, String)> {
    let git = |args: &[&str]| {
        aida_core::git_ops::host_fetch_git_command(root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let toplevel =
        git(&["rev-parse", "--show-toplevel"]).and_then(|t| std::fs::canonicalize(t.trim()).ok());
    if toplevel.is_none() || toplevel != std::fs::canonicalize(root).ok() {
        return Vec::new();
    }
    let Some(stdout) = git(&["remote", "-v"]) else {
        return Vec::new();
    };
    let mut remotes: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url)) = (parts.next(), parts.next()) else {
            continue;
        };
        let pair = (name.to_string(), url.to_string());
        if !remotes.contains(&pair) {
            remotes.push(pair);
        }
    }
    remotes
}

/// The tailored entries per enabled agent. Pure.
pub(crate) fn build_allowlist_report(
    selection: AgentSelection,
    forge_hosts: Vec<ForgeHost>,
) -> AllowlistReport {
    let hosts: Vec<String> = forge_hosts.iter().map(|f| f.host.clone()).collect();
    let agents = selection
        .names()
        .into_iter()
        .map(|agent| AgentAllowlist {
            agent,
            settings: crate::vendors::init_allowlists::allowlist_settings(agent, &hosts)
                .into_iter()
                .map(|(file, key, entries, scalar)| SettingEntry {
                    file,
                    key,
                    entries,
                    scalar,
                })
                .collect(),
        })
        .collect();
    AllowlistReport {
        forge_hosts,
        agents,
        dry_run: true,
    }
}

/// Human rendering. User-facing: no internal spec IDs.
pub(crate) fn render_allowlist_report(report: &AllowlistReport) -> String {
    let mut out = String::from("Sandbox allowlists aida init would set\n\n");
    if report.forge_hosts.is_empty() {
        out.push_str("Forge hosts: none detected (no git remote names a forge host)\n");
    } else {
        out.push_str("Forge hosts (from git remotes):\n");
        for f in &report.forge_hosts {
            out.push_str(&format!("  {}  ({})\n", f.host, f.remotes.join(", ")));
        }
    }
    if report.agents.is_empty() {
        out.push_str("\nNo agents enabled.\n");
    }
    for agent in &report.agents {
        out.push_str(&format!("\n{}:\n", agent.agent));
        for s in &agent.settings {
            let entries = if s.entries.is_empty() {
                "(no forge hosts to add)".to_string()
            } else {
                s.entries.join(", ")
            };
            out.push_str(&format!("  {} {}: {}\n", s.file, s.key, entries));
        }
    }
    out.push_str(
        "\nThe default hosts already in the contained template (model APIs, package registries) stay as they are.\n",
    );
    out
}

/// Resolve the agents to report on without prompting: an explicit
/// `--agent` list, else the same PATH detection init uses (all agents when
/// none is detected), plus Codex for a Codex-default project.
fn report_agent_selection(project_root: &Path, agent: Option<&str>) -> Result<AgentSelection> {
    if let Some(agent) = agent {
        return parse_agent_selection(agent);
    }
    let mut selection = detected_agent_selection();
    if crate::vendors::init_allowlists::project_defaults_to_codex(project_root) {
        selection.codex = true;
    }
    Ok(selection)
}

/// One edit `--write` makes: the entries missing under `key` in `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedEdit {
    pub file: &'static str,
    pub key: &'static str,
    pub add: Vec<String>,
    pub scalar: bool,
}

/// The existing value at dotted `key` in `file` (JSON when the file ends in
/// `.json`, else TOML) as strings; `None` when the file or key is absent.
/// An unparseable file is an error, so nothing is written over it.
fn existing_entries(root: &Path, file: &str, key: &str) -> Result<Option<Vec<String>>> {
    let path = root.join(file);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };
    let ctx = || format!("{} is not valid; nothing was written", path.display());
    let value: serde_json::Value = if file.ends_with(".json") {
        serde_json::from_str(&body).with_context(ctx)?
    } else {
        serde_json::to_value(toml::from_str::<toml::Value>(&body).with_context(ctx)?)?
    };
    let pointer = format!("/{}", key.replace('.', "/"));
    Ok(value.pointer(&pointer).map(|v| match v {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect(),
        serde_json::Value::String(s) => vec![s.clone()],
        other => vec![other.to_string()],
    }))
}

/// The edits that would bring each reported setting up to date, merged
/// across agents that share a setting. Never removes an entry; a scalar the
/// user already set is kept. Hosts compare case-insensitively.
pub(crate) fn plan_allowlist_writes(
    root: &Path,
    report: &AllowlistReport,
) -> Result<Vec<PlannedEdit>> {
    let mut wanted: Vec<PlannedEdit> = Vec::new();
    for s in report.agents.iter().flat_map(|a| &a.settings) {
        let slot = match wanted
            .iter_mut()
            .find(|w| w.file == s.file && w.key == s.key)
        {
            Some(slot) => slot,
            None => {
                wanted.push(PlannedEdit {
                    file: s.file,
                    key: s.key,
                    add: Vec::new(),
                    scalar: s.scalar,
                });
                wanted.last_mut().expect("just pushed")
            }
        };
        for e in &s.entries {
            if !slot.add.iter().any(|a| a.eq_ignore_ascii_case(e)) {
                slot.add.push(e.clone());
            }
        }
    }
    let mut edits = Vec::new();
    for mut w in wanted {
        let existing = existing_entries(root, w.file, w.key)?;
        if w.scalar && existing.is_some() {
            continue;
        }
        let have = existing.unwrap_or_default();
        w.add
            .retain(|e| !have.iter().any(|h| h.eq_ignore_ascii_case(e)));
        if !w.add.is_empty() {
            edits.push(w);
        }
    }
    Ok(edits)
}

/// The diff shown before writing: one `+` line per added entry.
pub(crate) fn render_planned_edits(edits: &[PlannedEdit]) -> String {
    if edits.is_empty() {
        return "\nEvery reported entry is already present; nothing to write.\n".to_string();
    }
    let mut out = String::from("\nChanges to write (existing entries are kept):\n");
    for e in edits {
        out.push_str(&format!("  {} {}:\n", e.file, e.key));
        for a in &e.add {
            out.push_str(&format!("    + {a}\n"));
        }
    }
    out
}

/// Apply `edits`, keeping every other key, entry and comment. Checked before
/// anything is written: a symlinked file (a link to a shared master) is
/// refused rather than written through, and a missing project config means
/// the project is not set up yet.
pub(crate) fn apply_allowlist_writes(root: &Path, edits: &[PlannedEdit]) -> Result<()> {
    for e in edits {
        let path = root.join(e.file);
        let meta = std::fs::symlink_metadata(&path);
        if meta.as_ref().is_ok_and(|m| m.file_type().is_symlink()) {
            anyhow::bail!(
                "{} is a symlink; edit its target by hand (nothing was written)",
                path.display()
            );
        }
        if meta.is_err() && !e.file.ends_with(".json") {
            anyhow::bail!(
                "{} does not exist; run `aida init` first (nothing was written)",
                path.display()
            );
        }
    }
    for e in edits {
        let path = root.join(e.file);
        let (tables, key) = e.key.rsplit_once('.').context("setting key has no table")?;
        let tables: Vec<&str> = tables.split('.').collect();
        if e.file.ends_with(".json") {
            let mut doc: serde_json::Value = match std::fs::read_to_string(&path) {
                Ok(body) => serde_json::from_str(&body)?,
                Err(_) => serde_json::json!({}),
            };
            let mut node = &mut doc;
            for t in &tables {
                node = node
                    .as_object_mut()
                    .with_context(|| format!("{} `{t}` is not an object", path.display()))?
                    .entry(*t)
                    .or_insert_with(|| serde_json::json!({}));
            }
            let list = node
                .as_object_mut()
                .with_context(|| format!("{} `{}` is not an object", path.display(), e.key))?
                .entry(key)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .with_context(|| format!("{} `{}` is not a list", path.display(), e.key))?;
            list.extend(e.add.iter().cloned().map(serde_json::Value::from));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, serde_json::to_string_pretty(&doc)? + "\n")?;
        } else if e.scalar {
            crate::config_edit::set_nested(&path, &tables, key, e.add[0].as_str().into())?;
        } else {
            let mut doc = crate::config_edit::load_doc(&path)?;
            let mut item = doc.as_item_mut();
            for t in &tables {
                item = item
                    .as_table_like_mut()
                    .with_context(|| format!("{} `{t}` is not a table", path.display()))?
                    .entry(t)
                    .or_insert(toml_edit::table());
            }
            let list = item
                .as_table_like_mut()
                .with_context(|| format!("{} `{}` is not a table", path.display(), e.key))?
                .entry(key)
                .or_insert(toml_edit::value(toml_edit::Array::new()))
                .as_array_mut()
                .with_context(|| format!("{} `{}` is not a list", path.display(), e.key))?;
            for a in &e.add {
                list.push(a.as_str());
            }
            crate::config_edit::save_doc(&path, &doc)?;
        }
    }
    Ok(())
}

/// Whether to apply the edits: `--write`, else a yes at an interactive
/// prompt (never in JSON output or without a terminal, so a headless or
/// piped run only reports).
fn confirm_write(write: bool, json: bool) -> Result<bool> {
    use std::io::IsTerminal;
    if write {
        return Ok(true);
    }
    if json || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(false);
    }
    crate::prompt_yes_no("Write these entries? [y/N] ", false)
}

pub(crate) fn run_report_allowlists(
    project_root: &Path,
    agent: Option<&str>,
    json: bool,
    write: bool,
) -> Result<()> {
    let selection = report_agent_selection(project_root, agent)?;
    let mut report = build_allowlist_report(
        selection,
        forge_hosts_from_remotes(&read_git_remotes(project_root)),
    );
    let edits = plan_allowlist_writes(project_root, &report)?;
    if !json {
        print!("{}", render_allowlist_report(&report));
        print!("{}", render_planned_edits(&edits));
    }
    if !edits.is_empty() && confirm_write(write, json)? {
        apply_allowlist_writes(project_root, &edits)?;
        report.dry_run = false;
        if !json {
            println!("Wrote the entries above.");
        }
    } else if !edits.is_empty() && !json {
        println!("Nothing was written; add --write to apply these changes.");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/init_allowlist_report_tests.rs"]
mod tests;
