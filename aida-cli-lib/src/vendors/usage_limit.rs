//! Each vendor's typed usage-limit signal in its headless event stream.
//! Only a structured failure event counts; text elsewhere in the log never does.
// trace:STORY-3134 | ai:claude
use crate::session::HeadlessVendor;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

/// A usage-limit event: the vendor's message and, when it gave one, the time
/// it said the limit resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageLimit {
    pub(crate) until: Option<DateTime<Utc>>,
    pub(crate) message: String,
}

/// The usage-limit outage `vendor`'s headless `log` ends with, if any.
pub(crate) fn detect_log(
    vendor: HeadlessVendor,
    log: &std::path::Path,
    now: DateTime<Utc>,
) -> Option<crate::vendor_outage::Outage> {
    crate::vendor_outage::detect_log(
        vendor.as_str(),
        |t, e| from_terminal(vendor, t, e),
        log,
        now,
    )
}

/// The usage-limit signal a run's `terminal` event carries, if any. Claude's
/// terminal error `result` counts only when the newest rate-limit event
/// before it (`earlier`) is a rejection.
pub(crate) fn from_terminal(
    vendor: HeadlessVendor,
    terminal: &serde_json::Value,
    earlier: &[serde_json::Value],
) -> Option<UsageLimit> {
    if vendor == HeadlessVendor::Claude
        && terminal["type"] == "result"
        && terminal["is_error"] == true
    {
        let last_limit = earlier
            .iter()
            .rev()
            .find(|e| e["type"] == "rate_limit_event")?;
        return from_event(vendor, last_limit);
    }
    from_event(vendor, terminal)
}

/// The usage-limit signal one headless event carries, if any.
fn from_event(vendor: HeadlessVendor, event: &serde_json::Value) -> Option<UsageLimit> {
    match vendor {
        // `{"type":"turn.failed","error":{"message":"You've hit your usage limit ... try again at <time>."}}`
        HeadlessVendor::Codex => {
            let message = match event["type"].as_str()? {
                "turn.failed" => event["error"]["message"].as_str()?,
                "error" => event["message"].as_str()?,
                _ => return None,
            };
            message
                .to_ascii_lowercase()
                .contains("usage limit")
                .then(|| UsageLimit {
                    until: codex_reset_time(message),
                    message: message.to_string(),
                })
        }
        // `{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":<unix>}}`
        HeadlessVendor::Claude => {
            let info = &event["rate_limit_info"];
            (event["type"] == "rate_limit_event" && info["status"] == "rejected").then(|| {
                UsageLimit {
                    until: info["resetsAt"]
                        .as_i64()
                        .and_then(|secs| Utc.timestamp_opt(secs, 0).single()),
                    message: format!(
                        "usage limit reached ({})",
                        info["rateLimitType"].as_str().unwrap_or("unknown window")
                    ),
                }
            })
        }
        HeadlessVendor::Agy => None,
    }
}

/// Parse the reset time from codex's `try again at Nov 29th, 2026 10:41 AM.`,
/// a local wall-clock time. An unparseable time is `None` (the default hold).
pub(crate) fn codex_reset_time(message: &str) -> Option<DateTime<Utc>> {
    let (_, rest) = message.split_once("try again at ")?;
    let words: Vec<String> = rest
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        .split_whitespace()
        .take(5)
        .map(|word| {
            let (digits, tail) = word.split_at(
                word.find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(word.len()),
            );
            let tail = tail.trim_start_matches(['s', 't', 'n', 'd', 'r', 'h']);
            if digits.is_empty() {
                word.to_string()
            } else {
                format!("{digits}{tail}")
            }
        })
        .collect();
    let naive = NaiveDateTime::parse_from_str(&words.join(" "), "%b %d, %Y %I:%M %p").ok()?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
}
