//! Vendor credential and sandbox configuration diagnostics. No admission authority.
// trace:STORY-2642 | ai:codex
use crate::lane_host::{check, Check};
use aida_core::exec_context::Environment;
use std::path::Path;

pub(crate) fn codex_config_check(body: Option<&str>) -> Check {
    let doc = body.and_then(|s| s.parse::<toml::Value>().ok());
    let pass = doc.as_ref().is_some_and(|d| {
        d.get("sandbox_mode").and_then(|v| v.as_str()) == Some("workspace-write")
            && d.get("approval_policy").and_then(|v| v.as_str()) == Some("never")
            && d.get("sandbox_workspace_write").is_some_and(|s| {
                s.get("network_access").and_then(|v| v.as_bool()) == Some(true)
                    && s.get("writable_roots")
                        .and_then(|v| v.as_array())
                        .is_some_and(Vec::is_empty)
            })
    });
    check(
        "codex-config",
        pass,
        true,
        "user Codex workspace-write preset (network on, no extra writable roots)",
        "aida doctor --lane-host --fix; project overrides are checked by ordinary launch admission",
    )
}

pub(crate) fn auth_check(vendor: &str, home: Option<&Path>, env: &Environment) -> Check {
    // Metadata only. Presence does not establish validity, refreshability or admission.
    let (key, file, login) = match vendor {
        "codex" => ("OPENAI_API_KEY", ".codex/auth.json", "codex login"),
        "claude" => (
            "ANTHROPIC_API_KEY",
            ".claude/.credentials.json",
            "claude auth login",
        ),
        _ => (
            "",
            "",
            "log in with the vendor CLI and verify its supported auth transport",
        ),
    };
    let override_home = (vendor == "codex").then(|| env.get("CODEX_HOME")).flatten();
    let credential = override_home
        .map(|p| std::path::PathBuf::from(p).join("auth.json"))
        .or_else(|| home.map(|h| h.join(file)));
    let present = (!key.is_empty() && env.get(key).is_some_and(|v| !v.is_empty()))
        || (!file.is_empty()
            && credential
                .is_some_and(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0)));
    check(
        &format!("auth-{vendor}"),
        present,
        false,
        if present {
            "credential source present; validity and refresh are not verified"
        } else {
            "no recognized credential source; auth readiness unverified"
        },
        login,
    )
}

pub(crate) fn user_preset_matches(home: &Path) -> bool {
    let agents = std::fs::read_to_string(home.join(".aida/agents.toml"))
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok());
    let codex = std::fs::read_to_string(home.join(".codex/config.toml")).ok();
    agents
        .as_ref()
        .and_then(|d| d.get("agents"))
        .is_some_and(|a| {
            a.get("contained").and_then(|v| v.as_bool()) == Some(true)
                && a.get("bypass").and_then(|v| v.as_bool()) == Some(false)
                && a.get("codex")
                    .and_then(|v| v.get("default_flags"))
                    .and_then(|v| v.as_array())
                    .is_some_and(|flags| {
                        flags.iter().filter_map(|v| v.as_str()).eq([
                            "--sandbox",
                            "workspace-write",
                            "--ask-for-approval",
                            "never",
                        ]) && flags.len() == 4
                    })
        })
        && codex_config_check(codex.as_deref()).pass
}

pub(crate) fn workspace_check(vendor: &str) -> Option<Check> {
    if vendor != "codex" {
        return None;
    }
    let body = crate::aida_home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".codex/config.toml")).ok());
    Some(codex_config_check(body.as_deref()))
}
