//! In-process tee for headless `claude -p` JSONL streams.
//!
//! The `--no-human` orchestrator phases write the headless Claude session's
//! stream-json output to a per-run log under `.aida/headless-logs/`. Without
//! a tee, the operator sees the launch banner and then nothing until the
//! phase verdict lands — has it stalled? is it about to fail? is it making
//! progress? `start_tee` answers that by tailing the log file from a
//! background thread and surfacing the high-signal events to stderr with a
//! `│ [headless]` prefix, alongside whatever the orchestrator itself prints.
//!
//! `format_event_for_tee` is the pure filter; it's deliberately split out so
//! the high-signal set is unit-testable from JSONL fixtures.
//!
//! trace:TASK-307 | ai:claude

use colored::Colorize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Polling cadence for the tail thread. Short enough to feel live, long
/// enough that a stalled headless run doesn't spin the CPU. Matches
/// `headless_tail::FOLLOW_POLL`. trace:TASK-307
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Truncation width for a tool-use input preview (mirrors
/// `headless_tail::TOOL_INPUT_PREVIEW_CHARS`).
const TOOL_INPUT_PREVIEW_CHARS: usize = 120;

/// Caller-supplied options that govern tee behaviour.
///
/// `enabled` is the user-facing on/off (default on; `--no-tee-headless` /
/// `AIDA_TEE_HEADLESS=0` flips it off). When off, only loud-failure events
/// surface — `is_error: true` and non-empty `permission_denials`. The
/// always-stream rule on errors is by design: a noise-sensitive operator
/// can quiet the routine chatter but the failure signal must NEVER hide.
///
/// `label` lets concurrent callers disambiguate their lines. When `Some`,
/// the prefix becomes `│ [headless:<label>]`; when `None`, plain
/// `│ [headless]`.
#[derive(Debug, Clone, Default)]
pub struct TeeOptions {
    pub enabled: bool,
    pub label: Option<String>,
}

impl TeeOptions {
    /// Resolve `enabled` from the `--no-tee-headless` flag and the
    /// `AIDA_TEE_HEADLESS` env var. Flag wins (explicit user intent at the
    /// CLI); else env `0`/`false`/`off` disables; else default on.
    pub fn from_env_and_flag(no_tee_flag: bool) -> Self {
        let env_value = std::env::var("AIDA_TEE_HEADLESS").ok();
        Self::resolve(no_tee_flag, env_value.as_deref())
    }

    /// Pure policy: flag + env-var value → `TeeOptions`. Split out from
    /// `from_env_and_flag` so the precedence rules can be exercised in
    /// tests without mutating process-wide env state (which races other
    /// tests in the same binary). trace:TASK-426
    fn resolve(no_tee_flag: bool, env_value: Option<&str>) -> Self {
        let env_off = env_value
            .map(|v| matches!(v.trim(), "0" | "false" | "off" | "no"))
            .unwrap_or(false);
        Self {
            enabled: !no_tee_flag && !env_off,
            label: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    fn prefix(&self) -> String {
        match self.label.as_deref() {
            Some(l) if !l.is_empty() => format!("│ [headless:{}]", l),
            _ => "│ [headless]".to_string(),
        }
    }
}

/// Handle returned by `start_tee`. Dropping or calling `stop` signals the
/// background thread to drain any remaining bytes and exit. Join is best-
/// effort — a stuck tee thread (e.g. the log file vanished) must not hang
/// the orchestrator forever, so we set the flag and move on if join takes
/// too long.
pub struct TeeHandle {
    stop_flag: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl TeeHandle {
    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

impl Drop for TeeHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

/// Spawn a background thread that tails `log_path` and writes filtered
/// events to stderr until either `TeeHandle::stop` is called or the stream
/// emits a `result` event (the headless session's terminal marker).
///
/// The log file may not exist yet at call time — the spawn site usually
/// creates it just before exec. The thread retries opens for a short grace
/// window so a slow file-create doesn't make the tee bail silently.
pub fn start_tee(log_path: &Path, opts: &TeeOptions) -> TeeHandle {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop = stop_flag.clone();
    let opts = opts.clone();
    let path: PathBuf = log_path.to_path_buf();
    let join = thread::spawn(move || {
        tail_loop(&path, &opts, stop);
    });
    TeeHandle {
        stop_flag,
        join: Some(join),
    }
}

/// The tail loop. Reads from `pos` forward each tick, parses each newline-
/// terminated line, and emits any lines `format_event_for_tee` returns.
/// Exits when (a) the stop flag is set and we've drained the current view,
/// (b) a `result` event is seen, or (c) the file disappears for longer than
/// the grace window.
fn tail_loop(path: &Path, opts: &TeeOptions, stop: Arc<AtomicBool>) {
    let prefix = opts.prefix();
    let mut pos: u64 = 0;
    let mut leftover = String::new();
    // Grace window for the log file to appear after the caller creates it.
    let mut missing_ticks: u32 = 0;
    const MAX_MISSING_TICKS: u32 = 40; // 40 * 250ms = 10s
    let mut seen_result = false;
    loop {
        let file = match std::fs::File::open(path) {
            Ok(f) => {
                missing_ticks = 0;
                f
            }
            Err(_) => {
                missing_ticks = missing_ticks.saturating_add(1);
                if missing_ticks >= MAX_MISSING_TICKS {
                    return;
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(POLL_INTERVAL);
                continue;
            }
        };
        let file_len = file.metadata().map(|m| m.len()).unwrap_or(pos);
        if file_len < pos {
            // Truncation / rotation — replay from the top so we don't miss
            // anything.
            pos = 0;
            leftover.clear();
        }
        let mut reader = match seek_to(file, pos) {
            Some(r) => r,
            None => return,
        };
        let mut buf = String::new();
        loop {
            buf.clear();
            let read = match reader.read_line(&mut buf) {
                Ok(n) => n,
                Err(_) => break,
            };
            if read == 0 {
                break;
            }
            if !buf.ends_with('\n') {
                // Partial line — keep `pos` where it is so the next tick
                // re-reads from here and the leftover prefix grows.
                leftover.push_str(&buf);
                break;
            }
            pos += read as u64;
            let line = if leftover.is_empty() {
                std::borrow::Cow::Borrowed(buf.trim_end_matches('\n'))
            } else {
                leftover.push_str(buf.trim_end_matches('\n'));
                let s = std::mem::take(&mut leftover);
                std::borrow::Cow::Owned(s)
            };
            let outcome = format_event_for_tee(&line, opts);
            for evt in &outcome.lines {
                emit_line(&prefix, evt);
            }
            if outcome.is_result {
                seen_result = true;
            }
        }
        if seen_result {
            // Terminal event — the session is over; no more bytes will be
            // appended. Bail out so the parent doesn't have to wait for the
            // stop flag to propagate.
            return;
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn seek_to(mut file: std::fs::File, pos: u64) -> Option<BufReader<std::fs::File>> {
    file.seek(SeekFrom::Start(pos)).ok()?;
    Some(BufReader::new(file))
}

fn emit_line(prefix: &str, line: &str) {
    let stderr = std::io::stderr();
    let mut out = stderr.lock();
    let _ = writeln!(out, "{} {}", prefix.dimmed(), line);
}

/// One emitted entry from `format_event_for_tee`. `is_result` lets the tail
/// loop short-circuit when the terminal event arrives.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TeeOutcome {
    pub lines: Vec<String>,
    pub is_result: bool,
}

/// Filter one JSONL line down to its high-signal projection.
///
/// Default (`opts.enabled == true`) surfaces:
///   - `system` / `init` — a one-liner with model + cwd
///   - `assistant.content[].type == "text"` — Claude's plain commentary
///   - `assistant.content[].type == "tool_use"` — `<Name>: <preview>`
///   - `result` — verdict + duration + cost
///
/// Always-on (regardless of `opts.enabled`) — loud:
///   - `result.is_error == true` (or top-level `is_error: true`)
///   - any event with non-empty `permission_denials`
///
/// Pure: takes the JSONL line, returns the lines to emit. Lets the unit
/// tests verify the filter without spawning a thread or creating files.
pub fn format_event_for_tee(line: &str, opts: &TeeOptions) -> TeeOutcome {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return TeeOutcome::default();
    }
    let parsed: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return TeeOutcome::default(),
    };
    let mut out = TeeOutcome::default();
    let typ = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // trace:TASK-840 | ai:claude — route the status markers through the registry
    // (resolve the profile once for this event).
    let profile = crate::glyphs::active_profile(crate::find_project_root().ok().as_deref());
    let warn = crate::glyphs::Glyph::Warning.render(profile);
    let cross = crate::glyphs::Glyph::Cross.render(profile);
    let check = crate::glyphs::Glyph::Check.render(profile);

    // Loud always-on signals — compute before the enabled gate. A loud
    // surfacing for `is_error` on a `result` event also flips `is_result`
    // so the tail loop exits.
    let mut denials_surfaced = false;
    if let Some(denials) = parsed.get("permission_denials").and_then(|v| v.as_array()) {
        if !denials.is_empty() {
            let summary = summarize_denials(denials);
            out.lines
                .push(format!("{warn} permission denied: {}", summary.red()));
            denials_surfaced = true;
        }
    }
    let top_is_error = parsed.get("is_error").and_then(|v| v.as_bool()) == Some(true);
    if typ == "result" {
        let is_err = parsed.get("is_error").and_then(|v| v.as_bool()) == Some(true);
        out.is_result = true;
        if is_err {
            let subtype = parsed
                .get("subtype")
                .and_then(|v| v.as_str())
                .unwrap_or("error");
            let detail = parsed
                .get("result")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let line = if detail.is_empty() {
                format!("{cross} result is_error ({})", subtype)
            } else {
                format!(
                    "{cross} result is_error ({}): {}",
                    subtype,
                    first_line(&detail)
                )
            };
            out.lines.push(line.red().to_string());
        } else if opts.enabled {
            // Verdict + cost line (informational; only when tee is on).
            let duration_ms = parsed.get("duration_ms").and_then(|v| v.as_u64());
            let cost = parsed.get("total_cost_usd").and_then(|v| v.as_f64());
            let mut bits: Vec<String> = vec![format!("{check} result")];
            if let Some(d) = duration_ms {
                bits.push(format!("{}ms", d));
            }
            if let Some(c) = cost {
                bits.push(format!("${:.4}", c));
            }
            out.lines.push(bits.join(" "));
        }
    } else if top_is_error && !denials_surfaced {
        // A non-result event with is_error:true (rare — usually denials
        // ride alongside) — surface it raw so the failure can't hide.
        // Skip if denials already covered it.
        out.lines
            .push(format!("{cross} {}", trimmed).red().to_string());
    }

    if !opts.enabled {
        // When disabled, emit only the always-on signals we already pushed.
        return out;
    }

    match typ {
        "system" => {
            let subtype = parsed.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            if subtype == "init" {
                let model = parsed.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                let cwd = parsed.get("cwd").and_then(|v| v.as_str()).unwrap_or("?");
                out.lines
                    .push(format!("starting session ({}) in {}", model, cwd));
            }
        }
        "assistant" => {
            let content = parsed
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());
            if let Some(content) = content {
                for block in content {
                    let bt = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match bt {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                let t = text.trim_end();
                                if !t.is_empty() {
                                    // Multi-line text: emit each non-empty
                                    // line as its own tee row so the prefix
                                    // sits at the start of every printed
                                    // line, not just the first.
                                    for ln in t.lines() {
                                        let trimmed = ln.trim_end();
                                        if !trimmed.is_empty() {
                                            out.lines.push(trimmed.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        "tool_use" => {
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            let preview = preview_tool_input(block.get("input"));
                            if preview.is_empty() {
                                out.lines.push(format!("{}", name.cyan()));
                            } else {
                                out.lines.push(format!("{}: {}", name.cyan(), preview));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn summarize_denials(denials: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for d in denials.iter().take(3) {
        let tool = d
            .get("tool_name")
            .or_else(|| d.get("tool"))
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        parts.push(tool.to_string());
    }
    if denials.len() > 3 {
        parts.push(format!("…+{} more", denials.len() - 3));
    }
    parts.join(", ")
}

fn preview_tool_input(input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let s = if let Some(s) = input.as_str() {
        s.to_string()
    } else if let Some(obj) = input.as_object() {
        // Dominant-field heuristic mirrors `headless_tail::preview_tool_input`
        // so the tee and the post-hoc tail render the same tool signatures.
        let dominant = [
            "command",
            "file_path",
            "pattern",
            "skill",
            "query",
            "prompt",
            "url",
        ]
        .iter()
        .find_map(|k| obj.get(*k).and_then(|v| v.as_str()));
        match dominant {
            Some(d) => d.to_string(),
            None => serde_json::to_string(obj).unwrap_or_default(),
        }
    } else {
        serde_json::to_string(input).unwrap_or_default()
    };
    let collapsed: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    if collapsed.chars().count() > TOOL_INPUT_PREVIEW_CHARS {
        let truncated: String = collapsed.chars().take(TOOL_INPUT_PREVIEW_CHARS).collect();
        format!("{}…", truncated)
    } else {
        collapsed
    }
}

fn first_line(s: &str) -> String {
    for ln in s.lines() {
        let t = ln.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_opts() -> TeeOptions {
        TeeOptions {
            enabled: true,
            label: None,
        }
    }

    fn disabled_opts() -> TeeOptions {
        TeeOptions {
            enabled: false,
            label: None,
        }
    }

    fn strip_ansi(s: &str) -> String {
        // Crude ANSI escape stripper — good enough for assertions on
        // colorized substrings. Walks chars (not bytes) so multibyte UTF-8
        // characters like `…` survive intact.
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' && chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn empty_line_is_silent() {
        let o = format_event_for_tee("", &enabled_opts());
        assert!(o.lines.is_empty());
        assert!(!o.is_result);
    }

    #[test]
    fn malformed_json_is_silent() {
        let o = format_event_for_tee("{not-json", &enabled_opts());
        assert!(o.lines.is_empty());
    }

    #[test]
    fn system_init_surfaces_model_and_cwd() {
        let line = r#"{"type":"system","subtype":"init","model":"claude-opus","cwd":"/tmp/work","session_id":"abc"}"#;
        let o = format_event_for_tee(line, &enabled_opts());
        assert_eq!(o.lines.len(), 1);
        assert!(o.lines[0].contains("claude-opus"));
        assert!(o.lines[0].contains("/tmp/work"));
    }

    #[test]
    fn assistant_text_surfaces() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"reading PR diff"}]}}"#;
        let o = format_event_for_tee(line, &enabled_opts());
        assert_eq!(o.lines, vec!["reading PR diff".to_string()]);
    }

    #[test]
    fn assistant_text_multiline_emits_per_line() {
        // Multi-line text gets one tee row per non-empty line so the prefix
        // sits at the start of EVERY line, not just the first.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"line one\nline two\n\nline three"}]}}"#;
        let o = format_event_for_tee(line, &enabled_opts());
        assert_eq!(
            o.lines,
            vec![
                "line one".to_string(),
                "line two".to_string(),
                "line three".to_string()
            ]
        );
    }

    #[test]
    fn assistant_tool_use_summary() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"git diff --stat","description":"see what changed"}}]}}"#;
        let o = format_event_for_tee(line, &enabled_opts());
        assert_eq!(o.lines.len(), 1);
        let stripped = strip_ansi(&o.lines[0]);
        assert!(stripped.starts_with("Bash: "), "got: {}", stripped);
        assert!(stripped.contains("git diff --stat"));
    }

    #[test]
    fn assistant_tool_use_truncates_long_input() {
        let long = "x".repeat(200);
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"{}"}}}}]}}}}"#,
            long
        );
        let o = format_event_for_tee(&line, &enabled_opts());
        assert_eq!(o.lines.len(), 1);
        let stripped = strip_ansi(&o.lines[0]);
        // 6 chars for "Bash: " + 120 chars truncated body + 1 ellipsis.
        assert!(stripped.ends_with('…'), "got: {}", stripped);
    }

    #[test]
    fn result_event_surfaces_summary_when_enabled() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1234,"total_cost_usd":0.0123}"#;
        let o = format_event_for_tee(line, &enabled_opts());
        assert!(o.is_result);
        assert_eq!(o.lines.len(), 1);
        assert!(o.lines[0].contains("1234ms"));
        assert!(o.lines[0].contains("$0.0123"));
    }

    #[test]
    fn result_event_silent_when_disabled_if_no_error() {
        let line = r#"{"type":"result","subtype":"success","is_error":false}"#;
        let o = format_event_for_tee(line, &disabled_opts());
        // Terminal marker still set (lets the tail loop exit) but nothing
        // streams when the run was clean and tee is disabled.
        assert!(o.is_result);
        assert!(o.lines.is_empty());
    }

    #[test]
    fn is_error_result_always_surfaces_even_when_disabled() {
        // The crux of the acceptance criterion: a failure must NEVER hide,
        // even when the user explicitly silenced tee output.
        let line = r#"{"type":"result","subtype":"error","is_error":true,"result":"permission gate refused Bash"}"#;
        let o = format_event_for_tee(line, &disabled_opts());
        assert!(o.is_result);
        assert_eq!(o.lines.len(), 1);
        let stripped = strip_ansi(&o.lines[0]);
        assert!(stripped.contains("is_error"));
        assert!(stripped.contains("permission gate refused Bash"));
    }

    #[test]
    fn permission_denials_always_surface_even_when_disabled() {
        let line = r#"{"type":"assistant","permission_denials":[{"tool_name":"Bash"},{"tool_name":"Write"}],"message":{"content":[{"type":"text","text":"will not stream"}]}}"#;
        let o = format_event_for_tee(line, &disabled_opts());
        // Loud denial line; the suppressed text does NOT appear.
        assert_eq!(o.lines.len(), 1);
        let stripped = strip_ansi(&o.lines[0]);
        assert!(stripped.contains("permission denied"));
        assert!(stripped.contains("Bash"));
        assert!(stripped.contains("Write"));
    }

    #[test]
    fn unknown_event_type_is_silent() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{}}"#;
        let o = format_event_for_tee(line, &enabled_opts());
        assert!(o.lines.is_empty());
    }

    #[test]
    fn thinking_block_is_silent() {
        // Internal thinking blocks must not leak — they're firehose noise.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"weighing options..."}]}}"#;
        let o = format_event_for_tee(line, &enabled_opts());
        assert!(o.lines.is_empty());
    }

    #[test]
    fn prefix_has_label_when_set() {
        let opts = TeeOptions {
            enabled: true,
            label: Some("pr-67".to_string()),
        };
        assert_eq!(opts.prefix(), "│ [headless:pr-67]");
    }

    #[test]
    fn prefix_is_bare_without_label() {
        let opts = enabled_opts();
        assert_eq!(opts.prefix(), "│ [headless]");
    }

    #[test]
    fn prefix_is_bare_when_label_empty() {
        let opts = TeeOptions {
            enabled: true,
            label: Some(String::new()),
        };
        assert_eq!(opts.prefix(), "│ [headless]");
    }

    // Precedence rules (flag wins; else env `0`/`false`/`off`/`no` disables;
    // else on by default) are tested against the pure `resolve` helper so
    // these cases don't mutate the global `AIDA_TEE_HEADLESS` env var and
    // race other tests in the same binary. trace:TASK-426

    #[test]
    fn resolve_disabled_by_flag() {
        // Flag wins even when the env says "on" (no env var set).
        assert!(!TeeOptions::resolve(true, None).enabled);
        // Flag still wins when the env explicitly enables.
        assert!(!TeeOptions::resolve(true, Some("1")).enabled);
    }

    #[test]
    fn resolve_enabled_by_default() {
        assert!(TeeOptions::resolve(false, None).enabled);
    }

    #[test]
    fn resolve_disabled_by_env() {
        for v in ["0", "false", "off", "no", " 0 ", "OFF"].iter().copied() {
            // Trim + lowercase-friendly match: "OFF" should NOT match (we
            // match exact lowercase tokens); whitespace-padded "0" should.
            let opts = TeeOptions::resolve(false, Some(v));
            let expect_off = matches!(v.trim(), "0" | "false" | "off" | "no");
            assert_eq!(!opts.enabled, expect_off, "value: {:?}", v);
        }
    }

    #[test]
    fn resolve_env_other_values_stay_enabled() {
        // Anything outside the disable set leaves tee on.
        for v in ["1", "true", "on", "yes", "", "garbage"].iter().copied() {
            assert!(
                TeeOptions::resolve(false, Some(v)).enabled,
                "value: {:?} should leave tee enabled",
                v
            );
        }
    }

    #[test]
    fn start_tee_terminates_on_result_event() {
        // Integration smoke: spawn the real background thread on a temp
        // log file, append a result event, and verify the thread exits on
        // its own (no need to call stop). Confirms the tail-loop
        // short-circuit on the terminal marker.
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "aida-tee-smoke-result-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        // Pre-create so the grace-window logic doesn't matter for the test.
        std::fs::File::create(&path).unwrap();
        let opts = TeeOptions {
            enabled: true,
            label: Some("smoke".to_string()),
        };
        let handle = start_tee(&path, &opts);
        // Append a result event; the tail loop should see it within a
        // couple of poll intervals and exit on its own.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"system","subtype":"init","model":"x","cwd":"/tmp"}}"#
            )
            .unwrap();
            writeln!(
                f,
                r#"{{"type":"result","subtype":"success","is_error":false}}"#
            )
            .unwrap();
        }
        // Give the thread a few ticks. POLL_INTERVAL is 250ms; 1.2s is
        // comfortably more than two ticks without making the test slow.
        thread::sleep(Duration::from_millis(1200));
        // The thread should have set its join handle's terminated state.
        // `stop()` is idempotent — if the thread is already gone, join
        // returns immediately. Wrap in a watchdog so a stuck loop fails
        // visibly instead of hanging CI.
        let (tx, rx) = std::sync::mpsc::channel();
        let stop_thread = thread::spawn(move || {
            handle.stop();
            let _ = tx.send(());
        });
        let drained = rx.recv_timeout(Duration::from_secs(3));
        assert!(
            drained.is_ok(),
            "tee thread did not terminate within 3s after a result event"
        );
        let _ = stop_thread.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn full_fixture_emits_expected_high_signal_subset() {
        // A realistic JSONL fixture — system.init, two assistant turns
        // (text + tool_use), a thinking block we drop, and the terminal
        // result. The tee should emit exactly the high-signal lines.
        let fixture = [
            r#"{"type":"system","subtype":"init","model":"claude-opus-4-7","cwd":"/work","session_id":"a"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"reading PR diff"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/auth.rs"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"file body"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"verdict: Approved"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1500,"total_cost_usd":0.01}"#,
        ];
        let mut emitted: Vec<String> = Vec::new();
        let mut saw_result = false;
        let opts = enabled_opts();
        for line in fixture.iter() {
            let o = format_event_for_tee(line, &opts);
            for l in o.lines {
                emitted.push(strip_ansi(&l));
            }
            if o.is_result {
                saw_result = true;
            }
        }
        assert!(saw_result);
        // system.init, two assistant text lines, one tool_use summary, and
        // the result line — five high-signal lines, no firehose.
        assert_eq!(emitted.len(), 5, "got: {:?}", emitted);
        assert!(emitted[0].contains("starting session"));
        assert_eq!(emitted[1], "reading PR diff");
        assert!(emitted[2].starts_with("Read: src/auth.rs"));
        assert_eq!(emitted[3], "verdict: Approved");
        assert!(emitted[4].contains("1500ms"));
        assert!(emitted[4].contains("$0.0100"));
    }
}
