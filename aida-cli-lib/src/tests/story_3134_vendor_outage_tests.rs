//! STORY-3134: a vendor usage limit is a typed outage. It is detected from the
//! vendor's structured stream event, recorded with its reset time, refused at
//! launch until then, and the stopped lane's work goes to a side branch
//! before its lease is ended.
// trace:STORY-3134 | ai:claude
#![cfg(test)]
use super::vendor_unavailable_exit;
use crate::session::HeadlessVendor;
use crate::vendor_outage::{self, Outage};
use chrono::{Duration, Local, TimeZone, Utc};
use std::path::Path;

const CODEX_LIMIT: &str = r#"{"type":"turn.failed","error":{"message":"You’ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at Nov 29th, 2026 10:41 AM."}}"#;

fn log_with(dir: &Path, lines: &[&str]) -> std::path::PathBuf {
    let path = dir.join("run.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

fn outage(vendor: &str, until: chrono::DateTime<Utc>) -> Outage {
    Outage {
        vendor: vendor.into(),
        until,
        until_known: true,
        detected_at: Utc::now(),
        message: "usage limit".into(),
    }
}

#[test]
fn ac1_codex_turn_failed_usage_limit_is_typed_with_its_reset_time() {
    let tmp = tempfile::tempdir().unwrap();
    let log = log_with(
        tmp.path(),
        &[r#"{"type":"thread.started","thread_id":"t"}"#, CODEX_LIMIT],
    );
    let found =
        crate::vendors::usage_limit::detect_log(HeadlessVendor::Codex, &log, Utc::now()).unwrap();
    let reset = Local.with_ymd_and_hms(2026, 11, 29, 10, 41, 0).unwrap();
    assert_eq!(found.vendor, HeadlessVendor::Codex.as_str());
    assert_eq!(found.until, reset.with_timezone(&Utc));
    assert!(found.until_known);
}

#[test]
fn ac1_an_unparseable_codex_reset_time_falls_back_to_the_default_hold() {
    assert_eq!(
        crate::vendors::usage_limit::codex_reset_time("try again later."),
        None
    );
    let tmp = tempfile::tempdir().unwrap();
    let line = r#"{"type":"error","message":"You've hit your usage limit. Try again later."}"#;
    let log = log_with(tmp.path(), &[line]);
    let now = Utc::now();
    let found = crate::vendors::usage_limit::detect_log(HeadlessVendor::Codex, &log, now).unwrap();
    assert!(!found.until_known);
    assert_eq!(found.until, now + Duration::minutes(60));
}

#[test]
fn ac1_free_text_and_other_failures_are_not_an_outage() {
    let tmp = tempfile::tempdir().unwrap();
    let log = log_with(
        tmp.path(),
        &[
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"You've hit your usage limit"}}"#,
            r#"{"type":"turn.failed","error":{"message":"stream disconnected"}}"#,
            "not json: usage limit",
        ],
    );
    assert_eq!(
        crate::vendors::usage_limit::detect_log(HeadlessVendor::Codex, &log, Utc::now()),
        None
    );
    // Another vendor's event shape does not count either.
    let log = log_with(tmp.path(), &[CODEX_LIMIT]);
    assert_eq!(
        crate::vendors::usage_limit::detect_log(HeadlessVendor::Claude, &log, Utc::now()),
        None
    );
}

#[test]
fn ac1_claude_rejected_rate_limit_event_carries_resets_at() {
    let tmp = tempfile::tempdir().unwrap();
    let log = log_with(
        tmp.path(),
        &[
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1795356000}}"#,
        ],
    );
    assert_eq!(
        crate::vendors::usage_limit::detect_log(HeadlessVendor::Claude, &log, Utc::now()),
        None
    );
    let log = log_with(
        tmp.path(),
        &[
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1795356000,"rateLimitType":"seven_day"}}"#,
        ],
    );
    let found =
        crate::vendors::usage_limit::detect_log(HeadlessVendor::Claude, &log, Utc::now()).unwrap();
    assert_eq!(found.until, Utc.timestamp_opt(1795356000, 0).unwrap());
}

#[test]
fn ac2_an_active_outage_refuses_launches_until_its_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    vendor_outage::record(main, &outage("codex", Utc::now() + Duration::hours(2))).unwrap();
    let err = vendor_outage::refuse_launch(main, "codex").unwrap_err();
    assert!(
        format!("{err:#}").contains("`codex` is unavailable until"),
        "{err:#}"
    );
    vendor_outage::refuse_launch(main, "claude").expect("another vendor still launches");
    assert_eq!(
        vendor_outage::scan(main).len(),
        1,
        "doctor shows the outage"
    );
    // Past the reset the same record no longer refuses anything.
    vendor_outage::record(main, &outage("codex", Utc::now() - Duration::minutes(1))).unwrap();
    vendor_outage::refuse_launch(main, "codex").expect("reset passed");
    assert!(vendor_outage::scan(main).is_empty());
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A seat with a commit, a staged edit and an untracked file, and its origin.
fn dirty_seat(root: &Path) -> std::path::PathBuf {
    let origin = root.join("origin.git");
    let seat = root.join("seat");
    std::fs::create_dir_all(&seat).unwrap();
    git(root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(&seat, &["init", "-q"]);
    git(
        &seat,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    std::fs::write(seat.join("a.txt"), "one\n").unwrap();
    git(&seat, &["add", "a.txt"]);
    git(
        &seat,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "c1",
        ],
    );
    std::fs::write(seat.join("a.txt"), "two\n").unwrap();
    std::fs::write(seat.join("new.txt"), "untracked\n").unwrap();
    seat
}

fn fake_aida(root: &Path) -> (std::path::PathBuf, crate::kernel::test_aida_exe::Guard) {
    let log = root.join("aida.log");
    let aida = root.join("aida");
    crate::test_exec::write_executable(
        &aida,
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display()),
    );
    (log, crate::kernel::test_aida_exe::set(aida))
}

#[test]
fn ac1_a_stopped_lane_is_pushed_to_a_side_branch_then_its_lease_ends() {
    let tmp = tempfile::tempdir().unwrap();
    let seat = dirty_seat(tmp.path());
    let head = git(&seat, &["rev-parse", "HEAD"]);
    let (log, _aida) = fake_aida(tmp.path());
    let limit = outage("codex", Utc::now() + Duration::hours(1));
    let detail = vendor_unavailable_exit(
        (tmp.path(), "0123456789ab", "lane-x"),
        tmp.path(),
        &seat,
        &limit,
    );
    assert!(
        detail.contains("work preserved on `aida-outage/lane-x`"),
        "{detail}"
    );
    assert!(detail.contains("lease 0123456789ab ended"), "{detail}");
    let origin = tmp.path().join("origin.git");
    let side = "refs/heads/aida-outage/lane-x";
    assert_eq!(git(&origin, &["show", &format!("{side}:a.txt")]), "two");
    assert_eq!(
        git(&origin, &["show", &format!("{side}:new.txt")]),
        "untracked"
    );
    assert_eq!(git(&origin, &["rev-parse", &format!("{side}^")]), head);
    // The seat itself is untouched: same HEAD, the edit still unstaged.
    assert_eq!(git(&seat, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&seat, &["status", "--porcelain"]),
        "M a.txt\n?? new.txt".to_string()
    );
    let call = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        call.trim(),
        "session end 0123456789ab --yes --skip-ci --force --discard-unpushed"
    );
    assert!(
        vendor_outage::refuse_launch(tmp.path(), "codex").is_err(),
        "outage recorded"
    );
}

#[test]
fn ac1_a_failed_side_branch_push_keeps_the_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let seat = dirty_seat(tmp.path());
    git(
        &seat,
        &["remote", "set-url", "origin", "/nonexistent/origin.git"],
    );
    let (log, _aida) = fake_aida(tmp.path());
    let limit = outage("codex", Utc::now() + Duration::hours(1));
    let detail = vendor_unavailable_exit(
        (tmp.path(), "0123456789ab", "lane-x"),
        tmp.path(),
        &seat,
        &limit,
    );
    assert!(
        detail.contains("lease 0123456789ab and its tree"),
        "{detail}"
    );
    assert!(!log.exists(), "no session end without the side branch");
}
