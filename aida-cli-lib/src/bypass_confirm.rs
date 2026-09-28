//! Fail-closed confirmation for supervised launches that disable tool prompts.
// trace:TASK-1500 | ai:codex

use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConfirmBypass {
    pub on: bool,
    pub source: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BypassSource {
    Configured,
    Explicit,
    Background,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    Proceed,
    Prompt,
    DowngradeNative,
    Refuse,
}

pub(crate) fn load(project_root: &Path) -> ConfirmBypass {
    let user_path = crate::aida_home_dir().map(|h| h.join(".aida/config.toml"));
    let user_value = user_path.as_deref().and_then(read_value);
    let project_text = crate::trusted_config::read_trusted_config_toml(project_root);
    let project_malformed = project_text
        .as_deref()
        .is_some_and(|body| body.parse::<toml::Value>().is_err());
    let project_value = project_text.as_deref().and_then(parse_value);
    // A project may require consent, but cannot revoke the human's choice.
    // The trusted-config fallback to a local default branch is acceptable: this
    // project scope only tightens the consent requirement. trace:TASK-1500
    resolve(user_value, project_value, project_malformed)
}

fn resolve(
    user_value: Option<bool>,
    project_value: Option<bool>,
    project_malformed: bool,
) -> ConfirmBypass {
    let on = project_malformed || user_value != Some(false) || project_value == Some(true);
    let source = if project_value == Some(true) {
        "trusted project require".to_string()
    } else if user_value == Some(false) {
        "user config".to_string()
    } else if user_value == Some(true) {
        "user config".to_string()
    } else if project_value == Some(false) {
        "project false ignored".to_string()
    } else {
        "default".to_string()
    };
    ConfirmBypass { on, source }
}

fn read_value(path: &Path) -> Option<bool> {
    let body = std::fs::read_to_string(path).ok()?;
    parse_value(&body)
}

fn parse_value(body: &str) -> Option<bool> {
    let doc: toml::Value = body.parse().ok()?;
    doc.get("agents")?.get("confirm_bypass")?.as_bool()
}

pub(crate) fn has_user_setting(body: &str) -> bool {
    body.parse::<toml::Value>()
        .ok()
        .and_then(|doc| {
            doc.get("agents")
                .and_then(|a| a.get("confirm_bypass"))
                .cloned()
        })
        .is_some()
}

pub(crate) fn decide(bypass: bool, source: BypassSource, confirm: bool, tty: bool) -> Decision {
    if !bypass || !confirm {
        return Decision::Proceed;
    }
    if tty {
        Decision::Prompt
    } else if matches!(source, BypassSource::Explicit | BypassSource::Background) {
        Decision::Refuse
    } else {
        Decision::DowngradeNative
    }
}

pub(crate) fn prompt(agent: &str) -> bool {
    use std::io::Write as _;
    eprint!(
        "  {agent} will launch with permission prompts turned off. Continue with bypass? [y/N] "
    );
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

pub(crate) fn strip_bypass_flags(args: &mut Vec<String>) {
    // trace:TASK-206 | ai:codex
    let mut clean = Vec::with_capacity(args.len());
    let mut iter = args.drain(..).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--permission-mode" {
            if iter
                .peek()
                .is_some_and(|value| value == "bypassPermissions")
            {
                iter.next();
            } else {
                clean.push(arg);
            }
            continue;
        }
        if matches!(
            arg.as_str(),
            "--dangerously-bypass-approvals-and-sandbox" | "--dangerously-skip-permissions"
        ) {
            continue;
        }
        if arg == "--permission-mode=bypassPermissions" {
            continue;
        }
        clean.push(arg);
    }
    drop(iter);
    *args = clean;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_bypass_decision_matrix() {
        assert_eq!(
            decide(false, BypassSource::Configured, true, false),
            Decision::Proceed
        );
        assert_eq!(
            decide(true, BypassSource::Configured, false, false),
            Decision::Proceed
        );
        assert_eq!(
            decide(true, BypassSource::Configured, true, true),
            Decision::Prompt
        );
        assert_eq!(
            decide(true, BypassSource::Configured, true, false),
            Decision::DowngradeNative
        );
        assert_eq!(
            decide(true, BypassSource::Explicit, true, false),
            Decision::Refuse
        );
        assert_eq!(
            decide(true, BypassSource::Background, true, false),
            Decision::Refuse
        );
    }

    #[test]
    fn project_false_does_not_disable_confirmation() {
        assert!(resolve(None, Some(false), false).on);
        assert_eq!(
            resolve(None, Some(false), false).source,
            "project false ignored"
        );
        assert!(resolve(Some(false), Some(true), false).on);
        assert!(!resolve(Some(false), Some(false), false).on);
        assert!(!resolve(Some(false), None, false).on);
    }

    #[test]
    fn absent_and_unreadable_config_fail_closed() {
        assert!(resolve(None, None, false).on);
        assert!(resolve(Some(false), None, true).on);
    }

    #[test]
    fn strips_only_bypass_permission_mode_pair() {
        // trace:TASK-206 | ai:codex
        let mut args = vec![
            "before".into(),
            "--permission-mode".into(),
            "bypassPermissions".into(),
            "after".into(),
        ];
        strip_bypass_flags(&mut args);
        assert_eq!(args, ["before", "after"]);
    }

    #[test]
    fn preserves_non_bypass_permission_mode_values() {
        // trace:TASK-206 | ai:codex
        let mut args = vec![
            "--permission-mode".into(),
            "acceptEdits".into(),
            "next".into(),
        ];
        strip_bypass_flags(&mut args);
        assert_eq!(args, ["--permission-mode", "acceptEdits", "next"]);
    }

    #[test]
    fn strips_equals_bypass_and_preserves_equals_non_bypass() {
        // trace:TASK-206 | ai:codex
        let mut args = vec![
            "--permission-mode=bypassPermissions".into(),
            "--permission-mode=acceptEdits".into(),
        ];
        strip_bypass_flags(&mut args);
        assert_eq!(args, ["--permission-mode=acceptEdits"]);
    }

    #[test]
    fn strips_dangerous_flags_wherever_they_appear() {
        // trace:TASK-206 | ai:codex
        let mut args = vec![
            "--dangerously-skip-permissions".into(),
            "one".into(),
            "--dangerously-bypass-approvals-and-sandbox".into(),
            "two".into(),
        ];
        strip_bypass_flags(&mut args);
        assert_eq!(args, ["one", "two"]);
    }

    #[test]
    fn preserves_unpaired_bypass_tokens_and_trailing_permission_flag() {
        // trace:TASK-206 | ai:codex
        let mut args = vec![
            "bypassPermissions".into(),
            "--permission-mode".into(),
            "bypassPermissionsExtra".into(),
            "tail".into(),
            "--permission-mode".into(),
        ];
        strip_bypass_flags(&mut args);
        assert_eq!(
            args,
            [
                "bypassPermissions",
                "--permission-mode",
                "bypassPermissionsExtra",
                "tail",
                "--permission-mode"
            ]
        );
    }
}
