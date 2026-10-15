//! Seat rotation, first slice (STORY-1464).
//!
//! A long-lived seat (advisor, reviewer, product) that never restarts pays its
//! whole accumulated context on every model call. BUG-1589 measured seats that
//! lived for days and averaged ~580k tokens per call, while a fresh session
//! starts around ~85k. This module holds the two primitives rotation needs:
//!
//! 1. **A context-size signal.** [`last_context_tokens`] reads only the TAIL of
//!    a session transcript and returns the prompt size of the most recent model
//!    call. It understands the same two vendor shapes as the token ledger
//!    (`token_ledger.rs`, TASK-1427): the Claude `message.usage` record and the
//!    Codex `token_count` event. The per-turn `aida awaiting --notice` line uses
//!    it, via [`rotation_notice_line`], to tell a seat that has crossed the
//!    ceiling to hand off and restart.
//! 2. **A handoff primitive.** [`write_handoff`] stores a size-capped handoff
//!    note under `.aida/handoff/<seat>/`, updates `latest.md` (the seed the
//!    next session reads with [`read_latest_handoff`]) and appends one record
//!    to `.aida/handoff/<seat>/log.jsonl` so each rotation leaves a trace.
//!
//! The cap exists because the handoff becomes the next session's baseline;
//! anything bigger belongs in specs, comments or findings.
//!
//! trace:STORY-1464 | ai:claude

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Default context ceiling. Shared with the STORY-1462 watchdog's restart
/// recommendation (300k). `AIDA_SEAT_CONTEXT_CEILING` overrides it; `0`
/// disables the rotation notice.
pub(crate) const DEFAULT_CONTEXT_CEILING: u64 = 300_000;

/// Handoff size cap in bytes (~4k tokens at ~4 bytes per token).
pub(crate) const HANDOFF_MAX_BYTES: usize = 16_000;

/// How much of the transcript tail to scan for the latest usage record. A
/// single assistant record is small; 512 KiB covers even a turn with large
/// tool output while keeping the per-turn hook cheap.
const TAIL_BYTES: u64 = 512 * 1024;

/// The effective ceiling: env override, else the default. `None` = disabled.
pub(crate) fn context_ceiling() -> Option<u64> {
    ceiling_from(std::env::var("AIDA_SEAT_CONTEXT_CEILING").ok().as_deref())
}

fn ceiling_from(raw: Option<&str>) -> Option<u64> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Some(DEFAULT_CONTEXT_CEILING),
        Some(s) => match s.parse::<u64>() {
            Ok(0) => None,
            Ok(n) => Some(n),
            Err(_) => Some(DEFAULT_CONTEXT_CEILING),
        },
    }
}

/// Extract `transcript_path` from a Claude/Codex hook JSON payload.
pub(crate) fn transcript_path_from_payload(payload: &str) -> Option<PathBuf> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let p = v.get("transcript_path")?.as_str()?.trim();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// Prompt-side context of ONE transcript record, if it carries usage.
/// Claude: `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
/// Codex: `payload.info.last_token_usage.input_tokens` (cached input is a
/// subset of `input_tokens` there, so it is not added again).
fn record_context_tokens(v: &Value) -> Option<u64> {
    if let Some(usage) = v.pointer("/message/usage") {
        let field = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
        let total = field("input_tokens")
            + field("cache_creation_input_tokens")
            + field("cache_read_input_tokens");
        return (total > 0).then_some(total);
    }
    if v.pointer("/payload/type").and_then(Value::as_str) == Some("token_count") {
        return v
            .pointer("/payload/info/last_token_usage/input_tokens")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0);
    }
    None
}

/// The context size of the most recent model call recorded in `transcript`,
/// read from the file's tail only. `None` when the file is unreadable or has
/// no usage record in the tail. Never errors: this feeds a fail-open hook.
pub(crate) fn last_context_tokens(transcript: &Path) -> Option<u64> {
    let mut file = std::fs::File::open(transcript).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    // A mid-file start almost always lands inside a record; drop the partial.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    lines.iter().rev().find_map(|line| {
        let v: Value = serde_json::from_str(line.trim()).ok()?;
        record_context_tokens(&v)
    })
}

/// Render a token count compactly: `312k`, `1.2M`, `950`.
fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// PURE: the per-turn rotation recommendation, or `None` below the ceiling.
pub(crate) fn rotation_notice_line(
    tokens: u64,
    ceiling: Option<u64>,
    seat: &str,
) -> Option<String> {
    let ceiling = ceiling?;
    if tokens < ceiling {
        return None;
    }
    Some(format!(
        "Seat rotation: context at {} tokens (ceiling {}). Hand off and restart: \
         at the next turn boundary, write a short handoff with \
         `aida session handoff --seat {seat} --write -`, then exit and start a fresh session \
         (it reads `aida session handoff --seat {seat} --show`).",
        human_tokens(tokens),
        human_tokens(ceiling),
    ))
}

/// Resolve the notice line for the current hook payload. Fail-open.
pub(crate) fn notice_from_hook_payload(payload: &str, seat: &str) -> Option<String> {
    let ceiling = context_ceiling()?;
    let transcript = transcript_path_from_payload(payload)?;
    let tokens = last_context_tokens(&transcript)?;
    rotation_notice_line(tokens, Some(ceiling), seat)
}

/// Seat names become directory names; keep them to a safe charset.
pub(crate) fn sanitize_seat(raw: &str) -> Result<String> {
    let seat = raw.trim().to_ascii_lowercase();
    if seat.is_empty()
        || seat.len() > 64
        || !seat
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid seat name '{raw}': use letters, digits, '-' or '_'");
    }
    Ok(seat)
}

fn seat_dir(project_root: &Path, seat: &str) -> PathBuf {
    project_root.join(".aida").join("handoff").join(seat)
}

/// Result of a successful [`write_handoff`].
#[derive(Debug)]
pub(crate) struct HandoffWritten {
    pub path: PathBuf,
    pub latest: PathBuf,
    pub bytes: usize,
}

/// Write a seat handoff note. Refuses an empty note and one over
/// [`HANDOFF_MAX_BYTES`] (the overflow belongs in specs/comments/findings).
pub(crate) fn write_handoff(
    project_root: &Path,
    seat: &str,
    body: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<HandoffWritten> {
    let seat = sanitize_seat(seat)?;
    let body = body.trim();
    if body.is_empty() {
        bail!("handoff note is empty");
    }
    if body.len() > HANDOFF_MAX_BYTES {
        bail!(
            "handoff note is {} bytes; the cap is {} (~4k tokens) because it becomes the next \
             session's baseline. Move detail into specs, comments or findings and keep the \
             handoff to pointers and next actions.",
            body.len(),
            HANDOFF_MAX_BYTES
        );
    }
    let dir = seat_dir(project_root, &seat);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let content = format!(
        "<!-- seat handoff: seat={seat} written={} -->\n{body}\n",
        now.to_rfc3339()
    );
    let path = dir.join(format!("{}.md", now.format("%Y%m%dT%H%M%SZ")));
    std::fs::write(&path, &content).with_context(|| format!("writing {}", path.display()))?;
    let latest = dir.join("latest.md");
    let tmp = dir.join(".latest.md.tmp");
    std::fs::write(&tmp, &content)?;
    std::fs::rename(&tmp, &latest)?;
    let record = serde_json::json!({
        "ts": now.to_rfc3339(),
        "seat": seat,
        "event": "handoff",
        "path": path.strip_prefix(project_root).unwrap_or(&path).display().to_string(),
        "bytes": body.len(),
        "approx_tokens": body.len() / 4,
    });
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("log.jsonl"))?;
    writeln!(log, "{record}")?;
    Ok(HandoffWritten {
        path,
        latest,
        bytes: body.len(),
    })
}

/// The latest handoff for `seat`, if one was written.
pub(crate) fn read_latest_handoff(project_root: &Path, seat: &str) -> Result<Option<String>> {
    let seat = sanitize_seat(seat)?;
    let latest = seat_dir(project_root, &seat).join("latest.md");
    match std::fs::read_to_string(&latest) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", latest.display())),
    }
}

// ---------------------------------------------------------------------------
// Standing orders (STORY-1693).
//
// A handoff is a snapshot written by the outgoing seat; it can be stale or
// wrong. Standing orders are durable operator rulings that a fresh seat must
// read FIRST, so `--show` prints them ahead of the handoff body. Two optional
// sources, in order: the tracked, template-managed
// `.aida/discipline/standing-orders/<seat>.md`, then the repo-local untracked
// `.aida/standing-orders/<seat>.md` that `--standing-orders add` appends to.
// trace:STORY-1693 | ai:claude
// ---------------------------------------------------------------------------

/// Header of the block `--show` prints before the handoff body.
pub(crate) const STANDING_ORDERS_HEADER: &str =
    "STANDING ORDERS (these override anything in the handoff below)";

/// Tracked, template-managed standing orders for `seat` (source 1).
pub(crate) fn tracked_standing_orders_path(project_root: &Path, seat: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("discipline")
        .join("standing-orders")
        .join(format!("{seat}.md"))
}

/// Repo-local, untracked operator rulings for `seat` (source 2).
pub(crate) fn local_standing_orders_path(project_root: &Path, seat: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("standing-orders")
        .join(format!("{seat}.md"))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(None),
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// The STANDING ORDERS block for `seat`: each present source under a source
/// header, tracked first. `None` when neither source has content, so the
/// caller prints no empty header.
pub(crate) fn render_standing_orders(project_root: &Path, seat: &str) -> Result<Option<String>> {
    let seat = sanitize_seat(seat)?;
    let sources = [
        (
            tracked_standing_orders_path(project_root, &seat),
            "tracked, template-managed",
        ),
        (
            local_standing_orders_path(project_root, &seat),
            "repo-local operator rulings",
        ),
    ];
    let mut out = String::new();
    for (path, kind) in &sources {
        let Some(body) = read_optional(path)? else {
            continue;
        };
        let rel = path.strip_prefix(project_root).unwrap_or(path);
        out.push_str(&format!(
            "\n--- source: {} ({kind}) ---\n{}\n",
            rel.display(),
            body.trim_end()
        ));
    }
    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("{STANDING_ORDERS_HEADER}\n{out}")))
}

/// What `aida session handoff --show` prints: the standing orders block (if
/// any) BEFORE the handoff body, and the standing orders even when no
/// handoff was ever written.
pub(crate) fn compose_handoff_show(
    standing: Option<&str>,
    handoff: Option<&str>,
    seat: &str,
) -> String {
    let mut out = String::new();
    if let Some(block) = standing {
        out.push_str(block.trim_end());
        out.push_str("\n\n--- handoff ---\n");
    }
    match handoff {
        Some(text) => out.push_str(text),
        None => out.push_str(&format!("No handoff recorded for seat '{seat}'.\n")),
    }
    out
}

/// Append a dated entry to the repo-local standing orders for `seat`,
/// creating the file (with a title) on first use. Returns the file path.
pub(crate) fn add_standing_order(
    project_root: &Path,
    seat: &str,
    text: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<PathBuf> {
    let seat = sanitize_seat(seat)?;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        bail!("standing order text is empty");
    }
    let path = local_standing_orders_path(project_root, &seat);
    let dir = path.parent().expect("standing-orders path has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let fresh = !path.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    if fresh {
        writeln!(
            f,
            "# Standing orders: {seat} (repo-local operator rulings)\n"
        )?;
    }
    writeln!(f, "- [{}] {text}", now.format("%Y-%m-%d %H:%MZ"))?;
    Ok(path)
}

/// The dated entries in the repo-local standing orders for `seat`, in order.
pub(crate) fn list_standing_orders(project_root: &Path, seat: &str) -> Result<Vec<String>> {
    let seat = sanitize_seat(seat)?;
    let path = local_standing_orders_path(project_root, &seat);
    Ok(read_optional(&path)?
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("- ["))
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_line(input: u64, write: u64, read: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {"usage": {
                "input_tokens": input,
                "cache_creation_input_tokens": write,
                "cache_read_input_tokens": read,
                "output_tokens": 10
            }}
        })
        .to_string()
    }

    #[test]
    fn ceiling_defaults_overrides_and_disables() {
        assert_eq!(ceiling_from(None), Some(DEFAULT_CONTEXT_CEILING));
        assert_eq!(ceiling_from(Some("")), Some(DEFAULT_CONTEXT_CEILING));
        assert_eq!(ceiling_from(Some("150000")), Some(150_000));
        assert_eq!(ceiling_from(Some("0")), None);
        assert_eq!(ceiling_from(Some("junk")), Some(DEFAULT_CONTEXT_CEILING));
    }

    #[test]
    fn reads_latest_claude_usage_from_tail() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("s.jsonl");
        let body = [
            claude_line(1, 2, 3),
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            claude_line(5, 1_000, 311_000),
            r#"{"type":"user","message":{"content":"next"}}"#.to_string(),
        ]
        .join("\n");
        std::fs::write(&t, body).unwrap();
        assert_eq!(last_context_tokens(&t), Some(312_005));
    }

    #[test]
    fn reads_codex_token_count_event() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("rollout.jsonl");
        let line = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "last_token_usage": {"input_tokens": 420_000, "cached_input_tokens": 400_000},
                "total_token_usage": {"input_tokens": 9_000_000}
            }}
        });
        std::fs::write(&t, format!("{line}\n")).unwrap();
        assert_eq!(last_context_tokens(&t), Some(420_000));
    }

    #[test]
    fn tail_read_skips_partial_first_line_of_large_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("big.jsonl");
        let mut f = std::fs::File::create(&t).unwrap();
        writeln!(f, "{}", claude_line(1, 1, 1)).unwrap();
        let filler = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}",
            "x".repeat(1024)
        );
        for _ in 0..600 {
            writeln!(f, "{filler}").unwrap();
        }
        writeln!(f, "{}", claude_line(0, 0, 500_000)).unwrap();
        drop(f);
        assert!(std::fs::metadata(&t).unwrap().len() > TAIL_BYTES);
        assert_eq!(last_context_tokens(&t), Some(500_000));
    }

    #[test]
    fn missing_or_usage_free_transcript_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(last_context_tokens(&dir.path().join("nope.jsonl")), None);
        let t = dir.path().join("empty.jsonl");
        std::fs::write(&t, "{\"type\":\"user\"}\n").unwrap();
        assert_eq!(last_context_tokens(&t), None);
    }

    #[test]
    fn notice_only_at_or_above_ceiling() {
        assert_eq!(
            rotation_notice_line(299_999, Some(300_000), "advisor"),
            None
        );
        assert_eq!(rotation_notice_line(900_000, None, "advisor"), None);
        let line = rotation_notice_line(312_000, Some(300_000), "advisor").unwrap();
        assert!(
            line.contains("context at 312k tokens (ceiling 300k)"),
            "{line}"
        );
        assert!(line.contains("hand off and restart") || line.contains("Hand off and restart"));
        assert!(line.contains("--seat advisor --write -"), "{line}");
    }

    #[test]
    fn payload_transcript_path_is_extracted() {
        assert_eq!(
            transcript_path_from_payload(r#"{"session_id":"s","transcript_path":"/t/x.jsonl"}"#),
            Some(PathBuf::from("/t/x.jsonl"))
        );
        assert_eq!(transcript_path_from_payload(r#"{"session_id":"s"}"#), None);
        assert_eq!(transcript_path_from_payload("not json"), None);
    }

    #[test]
    fn handoff_write_show_and_log() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-23T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let w = write_handoff(dir.path(), "Advisor", "next: review PR 12", now).unwrap();
        assert!(w
            .path
            .ends_with(".aida/handoff/advisor/20260923T100000Z.md"));
        assert_eq!(w.bytes, "next: review PR 12".len());
        let latest = read_latest_handoff(dir.path(), "advisor").unwrap().unwrap();
        assert!(latest.contains("next: review PR 12"));
        let log =
            std::fs::read_to_string(dir.path().join(".aida/handoff/advisor/log.jsonl")).unwrap();
        let rec: Value = serde_json::from_str(log.trim()).unwrap();
        assert_eq!(rec["seat"], "advisor");
        assert_eq!(rec["event"], "handoff");
        assert!(read_latest_handoff(dir.path(), "product")
            .unwrap()
            .is_none());
    }

    #[test]
    fn handoff_refuses_oversize_empty_and_bad_seat() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let big = "x".repeat(HANDOFF_MAX_BYTES + 1);
        let err = write_handoff(dir.path(), "advisor", &big, now).unwrap_err();
        assert!(err.to_string().contains("cap"), "{err}");
        assert!(write_handoff(dir.path(), "advisor", "   ", now).is_err());
        assert!(write_handoff(dir.path(), "../etc", "x", now).is_err());
        assert!(!dir.path().join(".aida/handoff/advisor/latest.md").exists());
    }

    // trace:STORY-1693 | ai:claude
    fn at(ts: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn standing_orders_absent_renders_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(render_standing_orders(dir.path(), "advisor")
            .unwrap()
            .is_none());
        // An empty (whitespace-only) source is also absent: no empty header.
        let p = tracked_standing_orders_path(dir.path(), "advisor");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "  \n").unwrap();
        assert!(render_standing_orders(dir.path(), "advisor")
            .unwrap()
            .is_none());
        assert!(list_standing_orders(dir.path(), "advisor")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn standing_orders_both_sources_in_order_under_one_header() {
        let dir = tempfile::tempdir().unwrap();
        let tracked = tracked_standing_orders_path(dir.path(), "advisor");
        std::fs::create_dir_all(tracked.parent().unwrap()).unwrap();
        std::fs::write(&tracked, "1. ship approved green PRs\n").unwrap();
        add_standing_order(
            dir.path(),
            "advisor",
            "never page the operator",
            at("2026-10-15T05:00:00Z"),
        )
        .unwrap();
        let block = render_standing_orders(dir.path(), "Advisor")
            .unwrap()
            .unwrap();
        assert!(block.starts_with(STANDING_ORDERS_HEADER), "{block}");
        assert_eq!(block.matches(STANDING_ORDERS_HEADER).count(), 1);
        let t = block
            .find("source: .aida/discipline/standing-orders/advisor.md")
            .unwrap();
        let l = block
            .find("source: .aida/standing-orders/advisor.md")
            .unwrap();
        assert!(t < l, "tracked source must come first: {block}");
        assert!(block.find("ship approved green PRs").unwrap() < l);
        assert!(block.find("never page the operator").unwrap() > l);
    }

    #[test]
    fn standing_orders_one_source_only() {
        let dir = tempfile::tempdir().unwrap();
        add_standing_order(dir.path(), "advisor", "local only", chrono::Utc::now()).unwrap();
        let block = render_standing_orders(dir.path(), "advisor")
            .unwrap()
            .unwrap();
        assert!(block.contains("local only"));
        assert!(!block.contains("discipline/standing-orders"), "{block}");
    }

    #[test]
    fn standing_orders_add_list_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        add_standing_order(
            dir.path(),
            "advisor",
            "first ruling",
            at("2026-10-15T05:00:00Z"),
        )
        .unwrap();
        let p = add_standing_order(
            dir.path(),
            "advisor",
            "second\n  ruling",
            at("2026-10-15T06:30:00Z"),
        )
        .unwrap();
        assert!(p.ends_with(".aida/standing-orders/advisor.md"));
        assert_eq!(
            list_standing_orders(dir.path(), "advisor").unwrap(),
            vec![
                "- [2026-10-15 05:00Z] first ruling".to_string(),
                "- [2026-10-15 06:30Z] second ruling".to_string(),
            ]
        );
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.matches("# Standing orders: advisor").count(), 1);
        assert!(add_standing_order(dir.path(), "advisor", "  ", chrono::Utc::now()).is_err());
        assert!(add_standing_order(dir.path(), "../x", "y", chrono::Utc::now()).is_err());
    }

    #[test]
    fn show_puts_standing_orders_before_the_handoff_and_survives_no_handoff() {
        let both = compose_handoff_show(
            Some("STANDING ORDERS (x)\nobey"),
            Some("handoff body\n"),
            "advisor",
        );
        assert!(both.find("obey").unwrap() < both.find("handoff body").unwrap());
        let no_handoff = compose_handoff_show(Some("STANDING ORDERS (x)\nobey"), None, "advisor");
        assert!(no_handoff.starts_with("STANDING ORDERS"));
        assert!(no_handoff.contains("No handoff recorded for seat 'advisor'."));
        // No standing orders: output is exactly the old handoff output.
        assert_eq!(
            compose_handoff_show(None, Some("handoff body\n"), "advisor"),
            "handoff body\n"
        );
        assert_eq!(
            compose_handoff_show(None, None, "advisor"),
            "No handoff recorded for seat 'advisor'.\n"
        );
    }
}
