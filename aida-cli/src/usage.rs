//! STORY-122 v1: per-invocation usage telemetry.
//!
//! Local-only, append-only JSONL log at `~/.aida/usage.jsonl`. Each
//! aida CLI invocation writes one line with cmd shape + outcome —
//! never argument values, file paths, or requirement content. The data
//! lets `aida usage` answer "which subcommands actually get used" and
//! "where do we have high error rates" without phoning home.
//!
//! Opt-out via `[telemetry] enabled = false` in `.aida/config.toml`.
//! Default is enabled (still local-only) — telemetry only earns its
//! keep if the data is there when we want to answer questions.
//!
//! Privacy floor:
//!   - cmd: top-level subcommand path (e.g. "queue list", "rel add")
//!   - args_count: number of positional/flag args, NOT their values
//!   - exit_code, duration_ms, ts: machine outcome
//!   - binary_sha: short hash from build banner (release tracking)
//!   - role / scope: derived from env / lease, NOT inferred from args
//!
//! trace:STORY-122 | ai:claude

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub ts: String,
    pub cmd: String,
    pub args_count: usize,
    pub exit_code: i32,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Resolve `~/.aida/usage.jsonl`. Returns `None` when the home dir
/// can't be located (treat as "telemetry off" — never error out).
pub fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida").join("usage.jsonl"))
}

/// Check whether telemetry is enabled. Resolution order:
///   1. `AIDA_TELEMETRY=0` env var (universal kill-switch)
///   2. `[telemetry] enabled = false` in the project's `.aida/config.toml`
///   3. default: enabled
///
/// Errors are swallowed — telemetry never blocks the user.
pub fn is_enabled(project_dir: Option<&std::path::Path>) -> bool {
    if let Ok(v) = std::env::var("AIDA_TELEMETRY") {
        if matches!(v.trim(), "0" | "false" | "no" | "off") {
            return false;
        }
    }
    let Some(dir) = project_dir else {
        return true;
    };
    let path = dir.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return true;
    };
    parse_telemetry_enabled(&content).unwrap_or(true)
}

/// Parse `[telemetry] enabled = false` out of a TOML string. Returns
/// `Some(true|false)` when the key is present, `None` when absent or
/// unparseable. Pulled out for unit testing.
pub fn parse_telemetry_enabled(content: &str) -> Option<bool> {
    let mut in_telemetry = false;
    for raw in content.lines() {
        let line = raw.split('#').next()?.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_telemetry = rest.trim_end_matches(']').trim() == "telemetry";
            continue;
        }
        if !in_telemetry {
            continue;
        }
        if let Some(rest) = line.strip_prefix("enabled") {
            let val = rest.split('=').nth(1)?.trim().trim_matches('"');
            return match val {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        }
    }
    None
}

/// Append a single event as JSONL. Errors are intentionally swallowed
/// (telemetry must never break the foreground command).
pub fn append_event(event: &UsageEvent) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(json) = serde_json::to_string(event) else {
        return;
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{}", json);
    }
}

/// Read every event from the log, newest-first order is the caller's
/// problem (this returns insertion order). Best-effort — malformed
/// lines are skipped silently.
pub fn read_events() -> Vec<UsageEvent> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<UsageEvent>(l).ok())
        .collect()
}

/// Derive the command-shape string from raw argv. We slice off the
/// program name and any value-bearing pieces (positional ids,
/// option-values starting without `--`). Subcommand path is captured
/// up to the first non-subcommand-looking token. The point is "what
/// command shape did the user invoke", NOT "what argument values did
/// they pass."
///
/// Example: `aida queue list --tags batch:foo --status approved`
///   → cmd = "queue list"
///
/// Limits the depth to two subcommand levels — enough for nested
/// command trees (e.g. `db sync`, `queue list`) without leaking
/// positional arg shapes into the cmd string.
pub fn derive_cmd_shape(argv: &[String]) -> String {
    let mut iter = argv.iter().skip(1); // skip program name
    let mut parts: Vec<String> = Vec::new();
    for tok in iter.by_ref() {
        if tok.starts_with('-') {
            // flag — stop capturing the subcommand path
            break;
        }
        // A subcommand looks like a lowercase word with maybe a hyphen.
        // Anything containing `/`, `.`, ID-shaped uppercase, etc. is an
        // arg value, not a subcommand — stop.
        if !is_subcommand_token(tok) {
            break;
        }
        parts.push(tok.clone());
        if parts.len() >= 2 {
            break;
        }
    }
    if parts.is_empty() {
        "<root>".to_string()
    } else {
        parts.join(" ")
    }
}

fn is_subcommand_token(tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    let bytes = tok.as_bytes();
    // Subcommands are lowercase ASCII with optional internal hyphens.
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || *b == b'-' || b.is_ascii_digit())
}

/// Count args (excluding the program name) for the event's
/// `args_count`. Counts every argv element after argv[0], so flags
/// and positional values both contribute. This is a coarse signal —
/// "did the user pass anything beyond the subcommand path" — and
/// deliberately doesn't try to distinguish flag-vs-positional.
pub fn count_args(argv: &[String]) -> usize {
    argv.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_telemetry_enabled_finds_false() {
        let toml = r#"
[telemetry]
enabled = false
"#;
        assert_eq!(parse_telemetry_enabled(toml), Some(false));
    }

    #[test]
    fn parse_telemetry_enabled_finds_true() {
        let toml = r#"
[telemetry]
enabled = true
"#;
        assert_eq!(parse_telemetry_enabled(toml), Some(true));
    }

    #[test]
    fn parse_telemetry_enabled_other_section_ignored() {
        let toml = r#"
[behavior]
enabled = false

[telemetry]
enabled = true
"#;
        assert_eq!(parse_telemetry_enabled(toml), Some(true));
    }

    #[test]
    fn parse_telemetry_enabled_absent() {
        let toml = "[behavior]\npermission_mode = \"auto\"\n";
        assert_eq!(parse_telemetry_enabled(toml), None);
    }

    #[test]
    fn parse_telemetry_strips_inline_comment() {
        let toml = "[telemetry]\nenabled = false  # opted out\n";
        assert_eq!(parse_telemetry_enabled(toml), Some(false));
    }

    #[test]
    fn derive_cmd_shape_captures_two_levels() {
        let argv = vec![
            "aida".to_string(),
            "queue".to_string(),
            "list".to_string(),
            "--tags".to_string(),
            "batch:foo".to_string(),
        ];
        assert_eq!(derive_cmd_shape(&argv), "queue list");
    }

    #[test]
    fn derive_cmd_shape_stops_at_first_flag() {
        let argv = vec![
            "aida".to_string(),
            "list".to_string(),
            "--status".to_string(),
            "approved".to_string(),
        ];
        assert_eq!(derive_cmd_shape(&argv), "list");
    }

    #[test]
    fn derive_cmd_shape_no_subcommand() {
        let argv = vec!["aida".to_string()];
        assert_eq!(derive_cmd_shape(&argv), "<root>");
    }

    #[test]
    fn derive_cmd_shape_stops_at_arg_value() {
        // `aida show STORY-42` — STORY-42 is a positional value, not a
        // subcommand. The cmd shape should be just "show".
        let argv = vec![
            "aida".to_string(),
            "show".to_string(),
            "STORY-42".to_string(),
        ];
        assert_eq!(derive_cmd_shape(&argv), "show");
    }

    #[test]
    fn count_args_excludes_program_name() {
        let argv = vec!["aida".to_string(), "queue".to_string(), "list".to_string()];
        assert_eq!(count_args(&argv), 2);
    }

    #[test]
    fn is_enabled_respects_env_kill_switch() {
        // TASK-521: serialise AIDA_TELEMETRY swaps under the shared
        // ENV_LOCK so a parallel test that emits telemetry doesn't
        // observe the kill-switch we set here. The guard restores the
        // prior value on drop. trace:TASK-521 | ai:claude
        let mut guard = crate::test_env::EnvVarGuard::set("AIDA_TELEMETRY", "0");
        assert!(!is_enabled(None));
        guard.reset("off");
        assert!(!is_enabled(None));
        guard.reset_unset();
        // No env, no project dir → default enabled.
        assert!(is_enabled(None));
    }
}
