//! STORY-3134: a usage limit, read from the run's terminal structured event,
//! is recorded with its reset time and refuses launches until then.
// trace:STORY-3134 | ai:claude
use crate::session::HeadlessVendor::{self, Claude, Codex};
use crate::vendor_outage::{self, Outage};
use chrono::{Duration, Local, TimeZone, Utc};

const CODEX_LIMIT: &str = r#"{"type":"turn.failed","error":{"message":"You’ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at Nov 29th, 2026 10:41 AM."}}"#;
const REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1795356000,"rateLimitType":"seven_day"}}"#;
const CLAUDE_FAILED: &str = r#"{"type":"result","is_error":true,"result":"limit"}"#;

fn write_log(dir: &std::path::Path, lines: &[&str]) -> std::path::PathBuf {
    let log = dir.join("run.jsonl");
    std::fs::write(&log, lines.join("\n") + "\n").unwrap();
    log
}

fn detect(vendor: HeadlessVendor, lines: &[&str]) -> Option<Outage> {
    let tmp = tempfile::tempdir().unwrap();
    crate::vendors::usage_limit::detect_log(vendor, &write_log(tmp.path(), lines))
}

fn outage(vendor: &str, hours: i64) -> Outage {
    let until = Some(Utc::now() + Duration::hours(hours));
    Outage::new(vendor, until, "limit".into())
}

#[test]
fn ac1_codex_usage_limit_is_typed_with_its_reset_time() {
    let found = detect(Codex, &[r#"{"type":"thread.started"}"#, CODEX_LIMIT]).unwrap();
    let reset = Local.with_ymd_and_hms(2026, 11, 29, 10, 41, 0).unwrap();
    assert_eq!(found.vendor, "codex");
    assert_eq!(found.until, reset.with_timezone(&Utc));
    assert!(found.until_known);
    // No parseable reset time: the default one-hour hold.
    let later = r#"{"type":"error","message":"You've hit your usage limit. Try again later."}"#;
    let found = detect(Codex, &[later]).unwrap();
    assert!(!found.until_known);
    assert!(found.until > Utc::now() + Duration::minutes(59));
}

#[test]
fn ac1_claude_rejection_before_a_failed_result_carries_resets_at() {
    let found = detect(Claude, &[REJECTED, CLAUDE_FAILED]).unwrap();
    assert_eq!(found.until, Utc.timestamp_opt(1795356000, 0).unwrap());
    assert!(
        detect(Claude, &[REJECTED]).is_some(),
        "a terminal rejection counts"
    );
    let allowed = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
    assert_eq!(detect(Claude, &[REJECTED, allowed, CLAUDE_FAILED]), None);
    assert_eq!(detect(Claude, &[REJECTED, r#"{"type":"result"}"#]), None);
}

#[test]
fn ac1_free_text_earlier_matches_and_other_failures_are_not_an_outage() {
    let text = r#"{"type":"item.completed","item":{"text":"You've hit your usage limit"}}"#;
    let other = r#"{"type":"turn.failed","error":{"message":"stream disconnected"}}"#;
    assert_eq!(detect(Codex, &[text]), None);
    assert_eq!(detect(Codex, &[other]), None);
    assert_eq!(detect(Codex, &["not json: usage limit"]), None);
    assert_eq!(
        detect(Codex, &[CODEX_LIMIT, other]),
        None,
        "only the terminal event"
    );
    assert_eq!(
        detect(Claude, &[CODEX_LIMIT]),
        None,
        "another vendor's shape"
    );
}

#[test]
fn ac2_an_active_outage_refuses_launches_until_its_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    vendor_outage::record(main, &outage("codex", 2)).unwrap();
    // The gate `queue work --no-human` runs before any lease exists.
    let err = crate::lane_host::preflight(main, "codex", false).unwrap_err();
    assert!(
        format!("{err:#}").contains("`codex` is unavailable until"),
        "{err:#}"
    );
    vendor_outage::refuse_launch(main, "claude").expect("another vendor still launches");
    // Past the reset the same record no longer refuses anything.
    vendor_outage::record(main, &outage("codex", -1)).unwrap();
    vendor_outage::refuse_launch(main, "codex").expect("reset passed");
    std::fs::write(main.join(".aida/vendor-outages/claude.json"), "{").unwrap();
    vendor_outage::refuse_launch(main, "claude").expect("an unreadable record never blocks");
}

#[test]
fn ac2_a_failed_run_records_its_outage() {
    let tmp = tempfile::tempdir().unwrap();
    let log = write_log(tmp.path(), &[&CODEX_LIMIT.replace("2026", "2099")]);
    let note = crate::vendors::usage_limit::note_failed_run(tmp.path(), Codex, &log).unwrap();
    assert!(
        note.contains("`codex` is unavailable until 2099-11-29"),
        "{note}"
    );
    assert!(vendor_outage::refuse_launch(tmp.path(), "codex").is_err());
}
