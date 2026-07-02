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

/// Append an arbitrary structured JSON record (e.g. a TASK-967
/// `drain_summary` event) as one JSONL line. Same opt-out + swallow-errors
/// contract as [`append_event`]; the caller decides whether to write (gating
/// on [`is_enabled`]). The distinct `event` discriminator on the record means
/// [`read_events`]'s `UsageEvent` parse skips it, so per-invocation stats stay
/// uncontaminated while a cost-per-drain reader can select it.
// trace:TASK-967 | ai:claude
pub fn append_value(value: &serde_json::Value) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(json) = serde_json::to_string(value) else {
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
        // BUG-699: collapse any leaked positional id from historical shapes so
        // every report lens sees clean command shapes (`show story-74` → `show`).
        .map(|mut ev| {
            ev.cmd = normalize_shape(&ev.cmd);
            ev
        })
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
    // BUG-699: a real aida subcommand is lowercase ASCII with optional internal
    // hyphens and NEVER contains a digit (verified: no subcommand has one). Every
    // id/positional value a user passes carries a digit or uppercase — `STORY-74`,
    // `story-74`, `12345`, `tsk-1`, `pr-43` — so excluding digits keeps arg values
    // out of the command shape and honors the "no argument values" privacy floor.
    // Cheap (no clap-tree walk) so the hot `statusline` log path stays fast.
    // trace:BUG-699 | ai:claude
    tok.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}

/// BUG-699: re-normalize a stored command-shape string, collapsing any positional
/// arg value that leaked in before the [`is_subcommand_token`] fix (e.g.
/// `"show story-74"` → `"show"`, `"show not-a-real-id"` → `"show"`). Idempotent
/// for already-clean shapes and a no-op for `"<root>"`. Applied at
/// [`read_events`] so every report lens sees clean shapes with no leaked ids.
// trace:BUG-699 | ai:claude
pub fn normalize_shape(cmd: &str) -> String {
    if cmd == "<root>" || cmd.is_empty() {
        return cmd.to_string();
    }
    let kept: Vec<&str> = cmd
        .split_whitespace()
        .take_while(|t| is_subcommand_token(t))
        .take(2)
        .collect();
    if kept.is_empty() {
        // First token isn't subcommand-shaped (shouldn't happen for a logged
        // shape) — return as-is rather than silently dropping it.
        cmd.to_string()
    } else {
        kept.join(" ")
    }
}

/// How a logged command shape relates to the requirement/intent graph.
///
/// The trace-read-rate audit (TASK-872) asks the cheap internal falsifier
/// for P2b: is AIDA's rich intent graph CONSULTED (read) or merely WRITTEN?
/// A high read:write ratio is evidence the typed layer earns its keep; a
/// written-but-near-zero-read shape would falsify P2b cleanly.
///
/// trace:TASK-872 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphAccess {
    /// Consults the intent graph (list/show/search/graph/why/history/…).
    Read,
    /// Mutates the intent graph (add/edit/comment/rel/queue add/defer/…).
    Write,
    /// Neither — machinery, sync, dev tooling, statusline, telemetry, etc.
    /// Excluded from the ratio so plumbing noise doesn't drown the signal.
    Neither,
}

/// Classify a logged command shape (e.g. `"queue list"`, `"rel add"`) as a
/// graph READ, a graph WRITE, or NEITHER.
///
/// Matching is on the command shape string `usage::derive_cmd_shape`
/// produces — the first one or two subcommand tokens, never arg values.
/// We classify on the two-token shape first (so `queue list` ≠ `queue add`),
/// then fall back to the single leading token.
///
/// READ = the command's job is to consult the graph: read a spec, walk
/// relationships, search, or render history/status derived from specs.
/// WRITE = the command's job is to mutate the graph: file/edit a spec,
/// comment, add a relationship, queue/defer/archive a spec.
/// NEITHER = git sync, dev shell, statusline, telemetry, init, etc. — these
/// touch the *store plumbing* but aren't graph consultation or authorship,
/// so counting them would muddy the read:write signal.
pub fn classify_access(cmd: &str) -> GraphAccess {
    let cmd = cmd.trim();
    // Two-token shapes that need disambiguation from their sibling verbs.
    match cmd {
        // queue: `list`/`next`/`progress` read the routed graph; `add` writes
        // a routing edge; `done`/`move`/`remove`/`rework`/`work` mutate state.
        "queue list" | "queue next" | "queue progress" => return GraphAccess::Read,
        "queue add" | "queue done" | "queue move" | "queue remove" | "queue rework"
        | "queue work" => return GraphAccess::Write,
        // findings: `list`/`calibration` read; `add`/`triage` write.
        "findings list" | "findings calibration" => return GraphAccess::Read,
        "findings add" | "findings triage" => return GraphAccess::Write,
        // relationships: `rel add`/`rel rm` mutate graph edges; `rel list`/
        // `rel show` consult them (a graph READ — walking typed edges is the
        // canonical "is the graph consulted" signal).
        "rel add" | "rel rm" | "rel remove" => return GraphAccess::Write,
        "rel list" | "rel show" => return GraphAccess::Read,
        // comments: `comment add` authors graph content.
        "comment add" => return GraphAccess::Write,
        // doc: `doc add` authors a Doc node + edge; `doc list`/`doc show` read.
        "doc add" => return GraphAccess::Write,
        "doc list" | "doc show" => return GraphAccess::Read,
        // db: sync/merge-gate/reconcile are plumbing, not graph consultation.
        _ => {}
    }
    // Single leading-token shapes.
    let head = cmd.split_whitespace().next().unwrap_or("");
    match head {
        // READ — consult the graph.
        "list" | "show" | "search" | "graph" | "why" | "history" | "tree" | "find" | "intent"
        | "status" | "lint" => GraphAccess::Read,
        // WRITE — mutate the graph. (`rel`/`comment`/`queue`/`findings`/`doc`
        // are deliberately absent: their read-vs-write split is decided by the
        // two-token match above, so a bare leading token alone is ambiguous
        // and classed NEITHER rather than guessed.)
        "add" | "edit" | "defer" | "undefer" | "archive" | "unarchive" | "decompose" | "import" => {
            GraphAccess::Write
        }
        // NEITHER — plumbing / machinery / dev tooling.
        _ => GraphAccess::Neither,
    }
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

    // BUG-699: the leak the operator caught — a LOWERCASE id slipped the old
    // `is_ascii_digit()` allowance and got captured into the shape.
    #[test]
    fn derive_cmd_shape_stops_at_lowercase_id() {
        for id in ["story-74", "task-931", "12345", "tsk-1", "pr-43"] {
            let argv = vec!["aida".to_string(), "show".to_string(), id.to_string()];
            assert_eq!(derive_cmd_shape(&argv), "show", "id `{id}` must not leak");
        }
        // A real hyphenated subcommand (no digit) is still captured.
        let argv = vec![
            "aida".to_string(),
            "doctor".to_string(),
            "verify-relationships".to_string(),
        ];
        assert_eq!(derive_cmd_shape(&argv), "doctor verify-relationships");
    }

    #[test]
    fn normalize_shape_collapses_leaked_ids() {
        assert_eq!(normalize_shape("show story-74"), "show");
        assert_eq!(normalize_shape("show 12345"), "show");
        assert_eq!(normalize_shape("queue list"), "queue list"); // already clean
        assert_eq!(normalize_shape("<root>"), "<root>");
        assert_eq!(normalize_shape("rel add"), "rel add");
    }

    #[test]
    fn count_args_excludes_program_name() {
        let argv = vec!["aida".to_string(), "queue".to_string(), "list".to_string()];
        assert_eq!(count_args(&argv), 2);
    }

    #[test]
    fn classify_access_reads() {
        // trace:TASK-872 | ai:claude
        for cmd in [
            "list",
            "show",
            "search",
            "graph",
            "why",
            "history",
            "queue list",
            "queue next",
            "queue progress",
            "findings list",
            "doc show",
            "rel list",
            "rel show",
        ] {
            assert_eq!(
                classify_access(cmd),
                GraphAccess::Read,
                "{cmd} should be READ"
            );
        }
    }

    #[test]
    fn classify_access_writes() {
        // trace:TASK-872 | ai:claude
        for cmd in [
            "add",
            "edit",
            "comment add",
            "rel add",
            "rel rm",
            "defer",
            "archive",
            "queue add",
            "queue done",
            "findings add",
            "doc add",
        ] {
            assert_eq!(
                classify_access(cmd),
                GraphAccess::Write,
                "{cmd} should be WRITE"
            );
        }
    }

    #[test]
    fn classify_access_neither() {
        // Plumbing / machinery / dev tooling must NOT count toward the
        // read:write ratio. trace:TASK-872 | ai:claude
        for cmd in [
            "pull",
            "push",
            "fetch",
            "db sync",
            "db merge-gate",
            "cache status",
            "statusline",
            "usage",
            "init",
            "dev activate",
            "session start",
            "<root>",
        ] {
            assert_eq!(
                classify_access(cmd),
                GraphAccess::Neither,
                "{cmd} should be NEITHER"
            );
        }
    }

    #[test]
    fn classify_access_queue_verbs_split_read_vs_write() {
        // The two-token disambiguation is the load-bearing case: `queue list`
        // is a graph read, `queue add` is a graph write. trace:TASK-872
        assert_eq!(classify_access("queue list"), GraphAccess::Read);
        assert_eq!(classify_access("queue add"), GraphAccess::Write);
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
