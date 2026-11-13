//! Pure hook layout helpers for the closed HookBackend registry.
//! Filesystem installation and compatibility entry points live in scaffolding.
// trace:STORY-2539 | ai:codex
use super::{HookBackend, HookLayout};

/// Where the wiring lives, relative to the project root.
// trace:STORY-2539 | ai:codex
pub const HOOKS_JSON_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
    .hooks
    .wiring;
/// Where the handler scripts live, relative to the project root.
pub const HOOKS_DIR_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
    .hooks
    .handlers;

/// Where the Antigravity wiring lives, relative to the project root.
// trace:TASK-1744 | ai:antigravity
// trace:STORY-2539 | ai:codex
pub const AGY_HOOKS_JSON_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Agy)
    .hooks
    .wiring;
/// Where the Antigravity handler scripts live, relative to the project root.
pub const AGY_HOOKS_DIR_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Agy)
    .hooks
    .handlers;
/// The named hook key AIDA manages inside `.agents/hooks.json`.
pub const AGY_EXPIRY_NOTICE_KEY: &str = "aida-grant-expiry-notice";

pub const CODEX_LAYOUT: HookLayout = super::profile(super::LaunchVendor::Codex).hooks;
pub const AGY_LAYOUT: HookLayout = super::profile(super::LaunchVendor::Agy).hooks;

/// Extract shared handlers using the descriptor's directory, for either backend.
pub fn layout_handler_names(layout: HookLayout) -> Vec<String> {
    let text = crate::templates::EMBEDDED_TEMPLATES[layout.template];
    let prefix = format!("/{}/", layout.handlers);
    let mut names = Vec::new();
    for line in text.lines() {
        for (offset, _) in line.match_indices(&prefix) {
            let rest = &line[offset + prefix.len()..];
            let name = rest
                .split(|c: char| c == '"' || c == '\\' || c.is_whitespace())
                .next()
                .unwrap_or("");
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    names
}

/// Validate the descriptor's event contract before an adapter writes anything.
pub fn hook_template(layout: HookLayout) -> Result<&'static str, String> {
    let raw = crate::templates::EMBEDDED_TEMPLATES[layout.template];
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let events: std::collections::BTreeSet<&str> = match layout.backend {
        HookBackend::Settings | HookBackend::Standalone => value["hooks"]
            .as_object()
            .ok_or("missing hooks object")?
            .keys()
            .map(String::as_str)
            .collect(),
        HookBackend::NamedEntries => value
            .as_object()
            .ok_or("missing named hooks object")?
            .values()
            .filter_map(serde_json::Value::as_object)
            .flat_map(|entry| entry.keys().map(String::as_str))
            .collect(),
    };
    if events != layout.events.iter().copied().collect() {
        return Err(format!(
            "hook event contract differs from {}",
            layout.template
        ));
    }
    Ok(raw)
}
