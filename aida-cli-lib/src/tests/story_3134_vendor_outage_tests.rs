//! STORY-3134: a usage limit, read from the run's terminal structured event,
//! is recorded with its reset time and refuses launches until then.
// trace:STORY-3134 | ai:claude
use crate::session::HeadlessVendor::{self, Claude, Codex};
use crate::standalone_implementer::Outcome;
use crate::vendor_outage::{self, Outage};
use crate::vendors::usage_limit::{codex_reset_time, detect_log, note_failed_run};
use chrono::{DateTime, Duration, Local, TimeZone, Timelike, Utc};

const REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1795356000,"rateLimitType":"seven_day"}}"#;
const CLAUDE_FAILED: &str = r#"{"type":"result","is_error":true,"result":"limit"}"#;

/// Codex's real `turn.failed` usage-limit event, resetting at `at`.
fn codex_limit(at: &str) -> String {
    let msg = format!("You’ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at {at}.");
    serde_json::json!({"type": "turn.failed", "error": {"message": msg}}).to_string()
}

/// A reset two days out, as codex spells it, and the instant it names.
fn soon() -> (String, DateTime<Utc>) {
    let at = (Local::now() + Duration::days(2)).with_second(0).unwrap();
    let spelled = at.format("%b %-dth %I:%M %p").to_string();
    (spelled, at.with_nanosecond(0).unwrap().to_utc())
}

/// A local wall-clock time as UTC.
fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap().to_utc()
}

fn write_log(dir: &std::path::Path, lines: &[&str]) -> std::path::PathBuf {
    let log = dir.join("run.jsonl");
    std::fs::write(&log, lines.join("\n") + "\n").unwrap();
    log
}

fn detect(vendor: HeadlessVendor, lines: &[&str]) -> Option<Outage> {
    let tmp = tempfile::tempdir().unwrap();
    detect_log(vendor, &write_log(tmp.path(), lines))
}

fn outage(vendor: &str, hours: i64) -> Outage {
    let until = Utc::now() + Duration::hours(hours);
    Outage::new(vendor, Some(until), "limit".into())
}

#[test]
fn ac1_codex_reset_time_resolves_the_next_occurrence_within_two_weeks() {
    let now = local(2026, 11, 26, 9, 41);
    let parse = |at: &str| codex_reset_time(&format!("try again at {at}."), now);
    let reset = Some(local(2026, 11, 29, 10, 41));
    assert_eq!(parse("Nov 29th 10:41 AM"), reset, "the real string");
    assert_eq!(parse("Nov 29, 2026 10:41 AM"), reset);
    assert_eq!(
        parse("Nov 29th, 2026 10:41 PM"),
        Some(local(2026, 11, 29, 22, 41))
    );
    let utc = Utc.with_ymd_and_hms(2026, 11, 29, 10, 41, 0).unwrap();
    assert_eq!(parse("Nov 29th 10:41 AM UTC"), Some(utc), "the stated zone");
    // Garbage, in the past, or more than 14 days away: the default hold.
    for at in [
        "soon-ish",
        "Nov 29, 2025 10:41 AM",
        "Dec 31st 10:41 AM",
        "Nov 26th 8:00 AM",
    ] {
        assert_eq!(parse(at), None, "{at}");
    }
    let new_year = codex_reset_time("try again at Jan 2nd 10:41 AM", local(2026, 12, 30, 9, 0));
    assert_eq!(new_year, Some(local(2027, 1, 2, 10, 41)));
}

#[test]
fn ac1_codex_usage_limit_is_typed_with_its_reset_time() {
    let (at, reset) = soon();
    let found = detect(Codex, &[r#"{"type":"thread.started"}"#, &codex_limit(&at)]).unwrap();
    assert_eq!((found.vendor.as_str(), found.until), ("codex", reset));
    assert!(found.until_known);
    // No parseable reset time: the default one-hour hold.
    let later = r#"{"type":"error","message":"You've hit your usage limit. Try again later."}"#;
    let found = detect(Codex, &[later]).unwrap();
    assert!(!found.until_known && found.until > Utc::now() + Duration::minutes(59));
}

#[test]
fn ac1_claude_rejection_before_a_failed_result_carries_resets_at() {
    let found = detect(Claude, &[REJECTED, CLAUDE_FAILED]).unwrap();
    assert_eq!(found.until, Utc.timestamp_opt(1795356000, 0).unwrap());
    assert!(detect(Claude, &[REJECTED]).is_some(), "terminal rejection");
    let allowed = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
    assert_eq!(detect(Claude, &[REJECTED, allowed, CLAUDE_FAILED]), None);
    assert_eq!(detect(Claude, &[REJECTED, r#"{"type":"result"}"#]), None);
}

#[test]
fn ac1_free_text_earlier_matches_and_other_failures_are_not_an_outage() {
    let text = r#"{"type":"item.completed","item":{"text":"You've hit your usage limit"}}"#;
    let other = r#"{"type":"turn.failed","error":{"message":"stream disconnected"}}"#;
    let limit = codex_limit(&soon().0);
    let not_json = "not json: usage limit";
    for lines in [&[text][..], &[other], &[not_json], &[limit.as_str(), other]] {
        assert_eq!(detect(Codex, lines), None, "{lines:?}");
    }
    assert_eq!(detect(Claude, &[&limit]), None, "another vendor's shape");
}

#[test]
fn ac2_an_active_outage_refuses_launches_until_its_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    vendor_outage::record(main, &outage("codex", 2)).unwrap();
    // The gate `queue work --no-human` runs before any lease exists.
    let err = crate::lane_host::preflight(main, "codex", false).unwrap_err();
    assert!(format!("{err:#}").contains("`codex` is unavailable until"));
    vendor_outage::refuse_launch(main, "claude").expect("another vendor still launches");
    // Past the reset the same record no longer refuses anything.
    vendor_outage::record(main, &outage("codex", -1)).unwrap();
    vendor_outage::refuse_launch(main, "codex").expect("reset passed");
    std::fs::write(main.join(".aida/vendor-outages/claude.json"), "{").unwrap();
    vendor_outage::refuse_launch(main, "claude").expect("an unreadable record never blocks");
}

#[test]
fn ac2_a_lane_that_fails_on_a_usage_limit_records_it_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let (at, reset) = soon();
    let log = write_log(tmp.path(), &[&codex_limit(&at)]);
    let note = |success, outcome| note_failed_run(tmp.path(), Codex, &log, success, &outcome);
    let held = Outcome::Held { branch: "b".into() };
    let punted = Outcome::Punted { reason: "r".into() };
    for (success, outcome) in [(true, Outcome::NoPr), (false, held), (false, punted)] {
        assert_eq!(note(success, outcome), None, "no outage");
        assert!(vendor_outage::refuse_launch(tmp.path(), "codex").is_ok());
    }
    let message = note(false, Outcome::NoPr).unwrap();
    let until = reset.with_timezone(&Local).format("%Y-%m-%d %H:%M");
    let note = format!("agent usage limit: `codex` is unavailable until {until}");
    assert!(message.starts_with(&note), "{message}");
    assert!(vendor_outage::refuse_launch(tmp.path(), "codex").is_err());
}
