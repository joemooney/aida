//! Each vendor's typed usage-limit signal in its headless event stream. Only
//! the run's terminal structured event counts; free text never does.
// trace:STORY-3134 | ai:claude
use crate::session::HeadlessVendor;
use crate::vendor_outage::Outage;
use chrono::{Local, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

/// The usage-limit outage `vendor`'s headless `log` ends with, if any.
/// Claude's terminal error `result` counts only when the newest rate-limit
/// event before it is a rejection.
pub(crate) fn detect_log(vendor: HeadlessVendor, log: &std::path::Path) -> Option<Outage> {
    let events = crate::vendor_outage::tail_events(log)?;
    let (mut event, earlier) = events.split_last()?;
    if vendor == HeadlessVendor::Claude && event["type"] == "result" && event["is_error"] == true {
        event = earlier.iter().rfind(|e| e["type"] == "rate_limit_event")?;
    }
    from_event(vendor, event)
}

fn from_event(vendor: HeadlessVendor, event: &Value) -> Option<Outage> {
    let name = vendor.as_str();
    match vendor {
        // `{"type":"turn.failed","error":{"message":"You've hit your usage limit ... try again at <time>."}}`
        HeadlessVendor::Codex => {
            let message = match event["type"].as_str()? {
                "turn.failed" => event["error"]["message"].as_str()?,
                "error" => event["message"].as_str()?,
                _ => return None,
            };
            let hit = message.to_ascii_lowercase().contains("usage limit");
            hit.then(|| Outage::new(name, codex_reset_time(message), message.into()))
        }
        // `{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":<unix>}}`
        HeadlessVendor::Claude => {
            let info = &event["rate_limit_info"];
            if event["type"] != "rate_limit_event" || info["status"] != "rejected" {
                return None;
            }
            let secs = info["resetsAt"].as_i64();
            let until = secs.and_then(|s| Utc.timestamp_opt(s, 0).single());
            let window = info["rateLimitType"].as_str().unwrap_or("unknown window");
            let message = format!("usage limit reached ({window})");
            Some(Outage::new(name, until, message))
        }
        HeadlessVendor::Agy => None,
    }
}

/// Parse codex's `try again at Nov 29th, 2026 10:41 AM.` as a local
/// wall-clock time; an unparseable time is `None` (the default hold).
pub(crate) fn codex_reset_time(message: &str) -> Option<chrono::DateTime<Utc>> {
    let (_, rest) = message.split_once("try again at ")?;
    // `Nov 29th, 2026 10:41 AM.` -> `Nov 29, 2026 10:41 AM`, text after it ignored.
    let words: Vec<&str> = rest.split_whitespace().take(5).collect();
    let mut when = words.join(" ");
    for ordinal in ["st,", "nd,", "rd,", "th,"] {
        when = when.replace(ordinal, ",");
    }
    let when = when.trim_end_matches('.');
    let naive = NaiveDateTime::parse_from_str(when, "%b %d, %Y %I:%M %p").ok()?;
    let local = Local.from_local_datetime(&naive).earliest()?;
    Some(local.with_timezone(&Utc))
}

/// Record the usage-limit outage a failed run's `log` ends with, if any, and
/// return its description for the lane's failure message.
pub(crate) fn note_failed_run(
    main: &std::path::Path,
    vendor: HeadlessVendor,
    log: &std::path::Path,
) -> Option<String> {
    let outage = detect_log(vendor, log)?;
    let recorded = crate::vendor_outage::record(main, &outage);
    let failed = recorded
        .map_err(|e| format!("; could not record it: {e:#}"))
        .err();
    Some(outage.describe() + &failed.unwrap_or_default())
}
