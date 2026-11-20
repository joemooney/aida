//! BUG-050: THE headless `claude -p` posture, shared by every headless claude
//! launch in the workspace. It lives in `aida-core` so the AI client
//! ([`crate::ai::client`]) and the CLI's launchers (`aida-cli-lib`'s
//! `session.rs` re-exports it) take the same flags. A ratchet test in
//! `aida-cli-lib/src/session.rs` scans both crates for `claude -p` builders
//! that skip it.

// trace:STORY-2539 | ai:codex
use anyhow::{Context, Result};

/// The tools a headless `claude -p` session never gets. `AskUserQuestion`
/// has no human to answer it (BUG-327). `Monitor` arms a watch that reports
/// back on a *later* turn, and `-p` has no later turn: a lane that armed one
/// ended its only turn and hung, its work uncommitted.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub const HEADLESS_DISALLOWED_TOOLS: &[&str] = &["AskUserQuestion", "Monitor"];

/// The Claude Code variable that turns off background tasks: Bash's
/// `run_in_background`, auto-backgrounding of a long command, and background
/// subagents. Claude Code 2.1.316 reads it (probed in the binary). It is set
/// both in the `--settings` document's `env` (so it rides the argv) and on
/// the child process ([`DISABLE_BACKGROUND_TASKS_ENV`] via `Command::env`),
/// so the guarantee does not hang on Claude Code's settings-env timing.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub const DISABLE_BACKGROUND_TASKS_ENV: &str = "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS";

/// THE headless `claude -p` posture. `contained_flags` is the contained
/// `--settings` pair, or empty for an uncontained launch. Returns those flags
/// with the no-background posture applied ([`headless_no_background_settings`]),
/// then `--disallowed-tools` with [`HEADLESS_DISALLOWED_TOOLS`].
///
/// The tool list is variadic on claude's side: the caller must follow these
/// flags with another `--flag` (or end the argv), never with the prompt
/// positional, which the list would swallow.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub fn headless_claude_posture(contained_flags: Vec<String>) -> Result<Vec<String>> {
    let mut flags = headless_no_background_settings(contained_flags)?;
    flags.push("--disallowed-tools".to_string());
    flags.extend(HEADLESS_DISALLOWED_TOOLS.iter().map(|t| t.to_string()));
    Ok(flags)
}

/// `flags` (the contained `--settings` flags, or none) with the headless
/// no-background posture added: the `--settings` document gains
/// `env.CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` and a `Monitor` deny rule.
/// Without contained flags, a `--settings` document carrying only those two
/// is appended. Headless only: the interactive contained launch builds the
/// same settings and keeps background tasks, since a human turn follows.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
fn headless_no_background_settings(mut flags: Vec<String>) -> Result<Vec<String>> {
    let at = match flags.iter().position(|f| f == "--settings") {
        Some(at) => at,
        None => {
            flags.extend(["--settings".to_string(), "{}".to_string()]);
            flags.len() - 2
        }
    };
    let value = flags
        .get_mut(at + 1)
        .ok_or_else(|| anyhow::anyhow!("`--settings` without a value"))?;
    let mut settings: serde_json::Value =
        serde_json::from_str(value).context("the `--settings` document is not JSON")?;
    let obj = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("the `--settings` document is not an object"))?;
    // dontAsk can refuse Bash before sandbox auto-approval. Explicit tool
    // permission is safe only beside the mandatory sandbox and an existing
    // write grant; never turn a failed/empty boundary into a shell grant.
    // The sandbox owns command confinement, while explicit deny rules still win.
    // trace:BUG-2652 | ai:codex
    let sandboxed = obj.get("sandbox").is_some_and(|sandbox| {
        sandbox["enabled"] == true
            && sandbox["failIfUnavailable"] == true
            && sandbox["allowUnsandboxedCommands"] == false
    });
    if sandboxed {
        if let Some(allow) = obj
            .get_mut("permissions")
            .and_then(|p| p.get_mut("allow"))
            .and_then(|a| a.as_array_mut())
        {
            let writable = allow.iter().any(|rule| {
                rule.as_str()
                    .is_some_and(|r| r.starts_with("Edit(//") && r.ends_with("/**)"))
            });
            if writable && !allow.iter().any(|rule| rule == "Bash") {
                allow.push("Bash".into());
            }
        }
    }
    obj.entry("env")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("the `--settings` env is not an object"))?
        .insert(DISABLE_BACKGROUND_TASKS_ENV.to_string(), "1".into());
    let deny = obj
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("the `--settings` permissions is not an object"))?
        .entry("deny")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("the `--settings` permissions.deny is not a list"))?;
    if !deny.iter().any(|r| r == "Monitor") {
        deny.push("Monitor".into());
    }
    *value = settings.to_string();
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose(settings: serde_json::Value) -> serde_json::Value {
        let args =
            headless_claude_posture(vec!["--settings".into(), settings.to_string()]).unwrap();
        serde_json::from_str(&args[1]).unwrap()
    }

    fn boundary() -> serde_json::Value {
        serde_json::json!({
            "sandbox": {"enabled": true, "failIfUnavailable": true,
                        "allowUnsandboxedCommands": false},
            "permissions": {"allow": ["Edit(//workspace/seat/**)", "Bash(aida *)"],
                            "deny": ["Bash(git reset --hard *)"]}
        })
    }

    // trace:BUG-2652 | ai:codex
    #[test]
    fn headless_bash_requires_mandatory_sandbox_and_write_grant() {
        let result = compose(boundary());
        assert!(result["permissions"]["allow"]
            .as_array()
            .unwrap()
            .contains(&"Bash".into()));
        assert!(result["permissions"]["deny"]
            .as_array()
            .unwrap()
            .contains(&"Bash(git reset --hard *)".into()));
        assert!(result["permissions"]["deny"]
            .as_array()
            .unwrap()
            .contains(&"Monitor".into()));
        for key in ["enabled", "failIfUnavailable", "allowUnsandboxedCommands"] {
            let mut settings = boundary();
            settings["sandbox"].as_object_mut().unwrap().remove(key);
            let result = compose(settings);
            assert!(
                !result["permissions"]["allow"]
                    .as_array()
                    .unwrap()
                    .contains(&"Bash".into()),
                "{key}"
            );
        }
        let mut settings = boundary();
        settings["permissions"]["allow"] = serde_json::json!([]);
        assert_eq!(
            compose(settings)["permissions"]["allow"],
            serde_json::json!([])
        );
        assert!(compose(serde_json::json!({}))["permissions"]
            .get("allow")
            .is_none());
    }

    // trace:BUG-2652 | ai:codex
    #[test]
    fn headless_bash_grant_is_idempotent_and_rejects_wrong_sandbox_values() {
        let once = compose(boundary());
        assert_eq!(compose(once.clone()), once);
        for (key, value) in [
            ("enabled", false),
            ("failIfUnavailable", false),
            ("allowUnsandboxedCommands", true),
        ] {
            let mut settings = boundary();
            settings["sandbox"][key] = value.into();
            assert!(!compose(settings)["permissions"]["allow"]
                .as_array()
                .unwrap()
                .contains(&"Bash".into()));
        }
    }
}
