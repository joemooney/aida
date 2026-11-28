//! `aida init --report-allowlists`: detect the enabled agent CLIs and the
//! forge hosts named by the repository's git remotes, then report the
//! per-agent sandbox allowlist and permission entries init would tailor.
//! Read-only: it writes nothing. trace:STORY-2838 | ai:claude

use super::{agent_cli_detected, parse_agent_selection, AgentSelection};
use anyhow::Result;
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
    /// Always true: this report changes nothing on disk.
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
/// directory that is not a git repository, or has no remotes, yields none.
pub(crate) fn read_git_remotes(root: &Path) -> Vec<(String, String)> {
    let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "-v"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut remotes: Vec<(String, String)> = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
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
    let contained_hosts = || SettingEntry {
        file: ".aida/config.toml",
        key: "contained.allowed_hosts",
        entries: hosts.clone(),
    };
    let mut agents = Vec::new();
    if selection.claude {
        agents.push(AgentAllowlist {
            agent: "claude",
            settings: vec![
                SettingEntry {
                    file: ".claude/settings.json",
                    key: "sandbox.network.allowedDomains",
                    entries: hosts.clone(),
                },
                contained_hosts(),
            ],
        });
    }
    if selection.codex {
        agents.push(AgentAllowlist {
            agent: "codex",
            settings: vec![
                contained_hosts(),
                SettingEntry {
                    file: ".aida/config.toml",
                    key: "contained.vendors.codex",
                    entries: vec!["native".to_string()],
                },
            ],
        });
    }
    if selection.antigravity {
        agents.push(AgentAllowlist {
            agent: "antigravity",
            settings: vec![contained_hosts()],
        });
    }
    AllowlistReport {
        forge_hosts,
        agents,
        dry_run: true,
    }
}

/// Human rendering. User-facing: no internal spec IDs.
pub(crate) fn render_allowlist_report(report: &AllowlistReport) -> String {
    let mut out = String::from("Sandbox allowlists aida init would set (nothing was written)\n\n");
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
/// `--agent` list, else the same PATH detection init uses.
fn report_agent_selection(project_root: &Path, agent: Option<&str>) -> Result<AgentSelection> {
    if let Some(agent) = agent {
        return parse_agent_selection(agent);
    }
    let mut selection = AgentSelection {
        claude: agent_cli_detected("claude"),
        codex: agent_cli_detected("codex"),
        antigravity: agent_cli_detected("agy") || agent_cli_detected("antigravity"),
    };
    if aida_core::agents_config::resolve_default_vendor(project_root).as_deref() == Some("codex") {
        selection.codex = true;
    }
    if !(selection.claude || selection.codex || selection.antigravity) {
        selection = AgentSelection::all();
    }
    Ok(selection)
}

pub(crate) fn run_report_allowlists(
    project_root: &Path,
    agent: Option<&str>,
    json: bool,
) -> Result<()> {
    let selection = report_agent_selection(project_root, agent)?;
    let report = build_allowlist_report(
        selection,
        forge_hosts_from_remotes(&read_git_remotes(project_root)),
    );
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_allowlist_report(&report));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/init_allowlist_report_tests.rs"]
mod tests;
