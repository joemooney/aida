//! Stable, read-only JSON contract consumed by external monitor applications.
// trace:STORY-1352 | ai:codex

use anyhow::Result;
use serde_json::{json, Value};
use std::io::Write;

pub const NAME: &str = "aida-monitor";
pub const VERSION: &str = "1.1.0";

/// Return the deliberately small set of fields promised to monitor consumers.
/// Fields not named here remain implementation details even when they happen to
/// appear in a command's JSON today.
pub fn manifest() -> Value {
    json!({
        "name": NAME,
        "version": VERSION,
        "surfaces": [
            surface("status", "aida status --json", &[("requirements.total", "integer"), ("requirements.by_status", "object")]),
            surface("ps", "aida ps --json", &[("sessions", "array")]),
            surface("integrate", "aida integrate --json", &[("ready", "array"), ("blocked", "array")]),
            surface("statusbar", "aida statusbar --once --plain", &[("text", "string")]),
            surface("usage", "aida usage --json", &[("summary", "object")]),
            surface("awaiting", "aida awaiting --json", &[("items", "array")]),
            surface("queue-progress", "aida queue progress --json", &[("queued", "integer"), ("in_progress", "integer"), ("done", "integer")]),
            surface("findings-list", "aida findings list --json", &[("findings", "array")]),
            surface("drain-status", "aida drain status --json", &[("drain.running", "boolean"), ("in_flight", "array")]),
            surface("tail", "aida tail --json", &[("lines", "array")]),
            surface("schedule", "aida schedule list --json", &[("jobs", "array")]),
            surface("events-follow", "aida watch --all --json", &[("ts", "string"), ("spec", "string|null"), ("run_uuid", "string"), ("seat", "string|null"), ("kind.event", "string")]),
        ],
    })
}

fn surface(name: &str, command: &str, fields: &[(&str, &str)]) -> Value {
    json!({
        "name": name,
        "command": command,
        "fields": fields.iter().map(|(path, kind)| json!({"path": path, "type": kind})).collect::<Vec<_>>(),
    })
}

pub fn print_json() -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer_pretty(&mut out, &manifest())?;
    writeln!(out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-1352 | ai:codex
    #[test]
    fn contract_is_semver_and_covers_eleven_polls_plus_follow() {
        let value = manifest();
        assert_eq!(value["name"], NAME);
        assert_eq!(value["version"], VERSION);
        assert_eq!(value["surfaces"].as_array().unwrap().len(), 12);
        assert!(VERSION.split('.').count() == 3);
        assert_eq!(value["surfaces"][11]["command"], "aida watch --all --json");
    }
}
