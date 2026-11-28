//! Each vendor's typed usage-limit signal in its headless event stream. Only
//! the run's terminal structured event counts; free text never does.
// trace:STORY-3134 | ai:claude
use crate::session::HeadlessVendor;
use crate::vendor_outage::Outage;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDateTime, TimeZone, Utc};
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
            hit.then(|| Outage::new(name, codex_reset_time(message, Utc::now()), message.into()))
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

/// Parse codex's `try again at Nov 29th[,] [2026] 10:41 AM[ UTC].` as the next
/// such time in the stated zone (UTC/GMT, else local). A time in the past or
/// more than 14 days out is unparsed (the default hold) and logged.
pub(crate) fn codex_reset_time(message: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let rest = message.split_once("try again at ")?.1;
    let resolved = resolve_reset(rest, now).filter(|t| *t > now && *t <= now + Duration::days(14));
    if resolved.is_none() {
        eprintln!("warning: unresolved usage-limit reset time {rest:?}; holding for the default");
    }
    resolved
}

fn resolve_reset(rest: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let words = rest.replace([',', '.'], " ");
    let mut words = words.split_whitespace();
    let month = words.next()?;
    let day = words.next()?.trim_end_matches(char::is_alphabetic);
    let mut next = words.next()?;
    let year = next.parse::<i32>().ok().filter(|_| next.len() == 4);
    if year.is_some() {
        next = words.next()?;
    }
    let (time, meridiem) = (next, words.next()?);
    let utc = matches!(words.next(), Some("UTC" | "GMT"));
    let at = |y: i32| {
        let when = format!("{month} {day} {y} {time} {meridiem}");
        let naive = NaiveDateTime::parse_from_str(&when, "%b %d %Y %I:%M %p").ok()?;
        let local = || Local.from_local_datetime(&naive).earliest();
        Some(naive.and_utc())
            .filter(|_| utc)
            .or_else(|| Some(local()?.to_utc()))
    };
    let y = now.year();
    let years = year.map_or(y - 1..=y + 1, |y| y..=y);
    years.filter_map(at).find(|t| *t > now)
}

/// Record the usage-limit outage of a run that failed without a PR, so later
/// launches on its vendor are refused until the reset, and describe it for
/// the lane's failure message. A success, punt or hold records none.
pub(crate) fn note_failed_run(
    main: &std::path::Path,
    vendor: HeadlessVendor,
    log: &std::path::Path,
    lane: &str,
    success: bool,
    outcome: &crate::standalone_implementer::Outcome,
) -> Option<String> {
    if success || !matches!(outcome, crate::standalone_implementer::Outcome::NoPr) {
        return None;
    }
    let mut outage = detect_log(vendor, log)?;
    outage.lane = Some(lane.to_owned());
    let failed = match crate::vendor_outage::record(main, &outage) {
        Ok(()) => String::new(),
        Err(e) => format!("; could not record it: {e:#}"),
    };
    Some(format!("{}{failed}", outage.exit_note()))
}
