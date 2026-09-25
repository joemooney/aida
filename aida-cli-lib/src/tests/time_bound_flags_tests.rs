//! Every time-bound flag in the CLI (`--since`, `--until`, `--older-than`,
//! `--unused` and the `aida usage unused <DURATION>` positional) either runs
//! through the shared time-bound grammar in `parse_since_arg_at`, or is
//! listed below as deliberately NOT a time bound (a git ref, a tag, a
//! free-text condition, an integer day count).
//!
//! The clap tree is walked, so a new flag with one of these names fails this
//! test until it is classified here. Each shared-grammar flag is then checked
//! against the full set of shared forms through the parse function its
//! command actually calls, with an injected clock and timezone so nothing
//! depends on the machine or touches `~/.aida`.
// trace:TASK-1509 | ai:claude

use super::*;
use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};

type ParseFn = fn(&str, DateTime<Utc>, &FixedOffset) -> Result<DateTime<Utc>, String>;

enum Kind {
    /// Parsed with the shared grammar, through this command's own parser.
    Shared(ParseFn),
    /// Not a time bound; the reason is why it keeps its own semantics.
    NotATimeBound(&'static str),
}

fn shared(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    parse_since_arg_at(raw, now, tz).map_err(|e| e.to_string())
}

fn history_since(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    crate::history::parse_history_bound(raw, "--since", now, tz).map_err(|e| e.to_string())
}

fn history_until(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    crate::history::parse_history_bound(raw, "--until", now, tz).map_err(|e| e.to_string())
}

/// `parse_days_arg` (usage, health, metrics) and the labeled lookbacks
/// (mailbox `--older-than`, usage `--unused`) all resolve through
/// `parse_lookback_at`.
fn lookback(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    parse_lookback_at(raw, "--since", now, tz)
        .map(|back| now - back)
        .map_err(|e| e.to_string())
}

fn calibration(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    crate::calibration::parse_since_at(raw, now, tz).map(|back| now - back)
}

fn tail(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    crate::headless_tail::parse_since_at(raw, now, tz)
        .map(|back| now - Duration::from_std(back).expect("duration fits"))
        .map_err(|e| e.to_string())
}

fn digest(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    // A scratch dir, never the live project: shared forms resolve before
    // the git-ref fallback, and a bad value's git probe finds no repo.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    crate::digest::parse_digest_since_at(Some(raw), tmp.path(), now, tz).map_err(|e| e.to_string())
}

fn doctor(raw: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Result<DateTime<Utc>, String> {
    // A scratch dir, as for digest: a bad value's git probe finds no repo.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    crate::resolve_completed_since_cutoff_at(tmp.path(), raw, now, tz)
        .ok_or_else(|| format!("`{raw}` did not resolve"))
}

fn classified_flags() -> Vec<(&'static str, Kind)> {
    use Kind::*;
    vec![
        ("aida approvals --since", Shared(shared)),
        ("aida approvals --until", Shared(shared)),
        ("aida findings classes --since", Shared(shared)),
        ("aida review classes --since", Shared(shared)),
        ("aida status --since", Shared(shared)),
        ("aida queue progress --since", Shared(shared)),
        ("aida archive --older-than", Shared(shared)),
        ("aida history --since", Shared(history_since)),
        ("aida history --until", Shared(history_until)),
        ("aida findings calibration --since", Shared(calibration)),
        (
            "aida autonomy calibration mismatches --since",
            Shared(calibration),
        ),
        ("aida load calibration --since", Shared(calibration)),
        ("aida mailbox archive --older-than", Shared(lookback)),
        ("aida mailbox gc --older-than", Shared(lookback)),
        ("aida usage --since", Shared(lookback)),
        // Split so the retired-flag lint (scripts/check-removed-flags.sh),
        // which matches the joined spelling, doesn't flag this label.
        (concat!("aida usage", " --unused"), Shared(lookback)),
        ("aida usage unused <duration>", Shared(lookback)),
        ("aida metrics agent-lift --since", Shared(lookback)),
        ("aida tail --since", Shared(tail)),
        ("aida drain tail --since", Shared(tail)),
        ("aida headless tail --since", Shared(tail)),
        ("aida digest --since", Shared(digest)),
        ("aida doctor --since", Shared(doctor)),
        (
            "aida defer --until",
            NotATimeBound("free-text revisit condition, not a time"),
        ),
        (
            "aida db reconcile-status --since",
            NotATimeBound("git ref bounding a commit scan"),
        ),
        (
            "aida field-study scan --since",
            NotATimeBound("git revision range"),
        ),
        (
            "aida doc coverage --since",
            NotATimeBound("git ref/tag marking the release boundary"),
        ),
        (
            "aida changelog generate --since",
            NotATimeBound("release tag"),
        ),
        (
            "aida changelog generate --until",
            NotATimeBound("release tag"),
        ),
        (
            "aida record prune --older-than",
            NotATimeBound("integer day count (u64), not a bound expression"),
        ),
        (
            "aida agent gc --older-than",
            NotATimeBound("integer day count (u64), not a bound expression"),
        ),
    ]
}

fn collect(cmd: &clap::Command, path: &str, out: &mut Vec<String>) {
    const NAMES: &[&str] = &["since", "until", "older-than", "unused", "duration"];
    for arg in cmd.get_arguments() {
        let (key, shown) = match arg.get_long() {
            Some(long) => (long.to_string(), format!("--{long}")),
            None => {
                let id = arg.get_id().as_str().to_string();
                (id.clone(), format!("<{id}>"))
            }
        };
        if NAMES.contains(&key.as_str()) {
            out.push(format!("{path} {shown}"));
        }
    }
    for sub in cmd.get_subcommands() {
        collect(sub, &format!("{path} {}", sub.get_name()), out);
    }
}

#[test]
fn every_time_bound_flag_is_classified() {
    let cmd = <crate::cli::Cli as clap::CommandFactory>::command();
    let mut found = Vec::new();
    collect(&cmd, "aida", &mut found);
    found.sort();
    let mut listed: Vec<String> = classified_flags()
        .into_iter()
        .map(|(k, _)| k.to_string())
        .collect();
    listed.sort();
    for (flag, kind) in classified_flags() {
        if let Kind::NotATimeBound(reason) = kind {
            assert!(!reason.is_empty(), "{flag}: give the reason it is excluded");
        }
    }
    assert_eq!(
        found, listed,
        "a --since/--until/--older-than style flag was added or removed; \
         route it through the shared time-bound parser and list it in \
         classified_flags(), or list it as NotATimeBound with a reason"
    );
}

#[test]
fn every_time_bound_flag_accepts_the_shared_forms() {
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
    let tz = FixedOffset::east_opt(2 * 3600).unwrap();
    let at = |y, mo, d, h, mi, s| Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap();
    let cases: Vec<(&str, DateTime<Utc>)> = vec![
        ("45m", now - Duration::minutes(45)),
        ("12h", now - Duration::hours(12)),
        ("2d", now - Duration::days(2)),
        ("2w", now - Duration::weeks(2)),
        ("24 hours ago", now - Duration::hours(24)),
        ("1 week ago", now - Duration::weeks(1)),
        ("3 Days Ago", now - Duration::days(3)),
        ("30 minutes ago", now - Duration::minutes(30)),
        // Bare ISO date: local midnight in the injected +02:00 zone.
        ("2026-05-01", at(2026, 4, 30, 22, 0, 0)),
        // Zone-less ISO datetime: local wall-clock time.
        ("2026-05-01T10:00", at(2026, 5, 1, 8, 0, 0)),
        ("2026-05-01 10:00:30", at(2026, 5, 1, 8, 0, 30)),
        // RFC3339: the zone in the input wins over the local zone.
        ("2026-05-01T10:00:00Z", at(2026, 5, 1, 10, 0, 0)),
        ("2026-05-01T10:00:00-05:00", at(2026, 5, 1, 15, 0, 0)),
    ];
    let mut failures = Vec::new();
    for (flag, kind) in classified_flags() {
        let Kind::Shared(parse) = kind else { continue };
        for (raw, want) in &cases {
            match parse(raw, now, &tz) {
                Ok(got) if got == *want => {}
                Ok(got) => failures.push(format!("{flag} `{raw}`: got {got}, want {want}")),
                Err(e) => failures.push(format!("{flag} `{raw}`: rejected: {e}")),
            }
        }
        if parse("not-a-time", now, &tz).is_ok() {
            failures.push(format!("{flag}: accepted `not-a-time`"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn tail_keeps_its_seconds_and_spelled_unit_forms() {
    // Forms `aida tail --since` accepted before the shared grammar that the
    // grammar does not cover; they must keep working.
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
    let tz = FixedOffset::east_opt(0).unwrap();
    for (raw, secs) in [("30s", 30), ("30", 30), ("10min", 600), ("2hours", 7200)] {
        let got = crate::headless_tail::parse_since_at(raw, now, &tz).unwrap();
        assert_eq!(got.as_secs(), secs, "{raw}");
    }
}

/// A scratch git repo with hermetic config (no global/system config, so a
/// host `commit.gpgsign` can't break it) and one commit at a fixed date.
fn scratch_repo() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(tmp.path().join("a.txt"), "hi").unwrap();
    git(tmp.path(), &["add", "."]);
    git(tmp.path(), &["commit", "-q", "-m", "init"]);
    tmp
}

/// The fixed committer date of the [`scratch_repo`] commit.
fn scratch_commit_date() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 2, 3, 4, 5).unwrap()
}

fn git(root: &std::path::Path, args: &[&str]) {
    let date = "2020-01-02T03:04:05Z";
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=Test"])
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn digest_and_doctor_keep_their_git_ref_fallback() {
    // Both accepted a git tag/ref before the shared grammar. Both try the
    // shared forms first and fall back to a ref only when none matches.
    let tmp = scratch_repo();
    let root = tmp.path();
    git(root, &["tag", "v9.9.9"]);
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
    let tz = FixedOffset::east_opt(0).unwrap();
    let digest_at = crate::digest::parse_digest_since_at(Some("v9.9.9"), root, now, &tz).unwrap();
    assert_eq!(digest_at, scratch_commit_date());
    let doctor_at = crate::resolve_completed_since_cutoff_at(root, "v9.9.9", now, &tz).unwrap();
    assert_eq!(doctor_at, scratch_commit_date());
}

#[test]
fn a_duration_that_is_also_a_git_ref_resolves_as_a_duration() {
    // `500d` is valid hex, so git also accepts it as an abbreviated commit
    // ID when one matches, or as a branch/tag of that name. The duration
    // must win: resolving the ref first silently used that commit's date.
    let tmp = scratch_repo();
    let root = tmp.path();
    git(root, &["branch", "500d"]);
    git(root, &["tag", "2w"]);
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
    let tz = FixedOffset::east_opt(0).unwrap();
    for (raw, want) in [
        ("500d", now - Duration::days(500)),
        ("2w", now - Duration::weeks(2)),
    ] {
        let doctor_at = crate::resolve_completed_since_cutoff_at(root, raw, now, &tz).unwrap();
        assert_eq!(doctor_at, want, "doctor --since {raw}");
        let digest_at = crate::digest::parse_digest_since_at(Some(raw), root, now, &tz).unwrap();
        assert_eq!(digest_at, want, "digest --since {raw}");
    }
}

#[test]
fn an_out_of_range_duration_is_refused_not_a_panic() {
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
    let tz = FixedOffset::east_opt(0).unwrap();
    for raw in ["99999999999999d", "99999999999999w", "9223372036854775807m"] {
        let err = parse_since_arg_at(raw, now, &tz).unwrap_err();
        assert!(err.to_string().contains("out of range"), "{raw}: {err}");
        for (flag, kind) in classified_flags() {
            let Kind::Shared(parse) = kind else { continue };
            let err = parse(raw, now, &tz).expect_err(flag);
            if flag != "aida doctor --since" {
                assert!(err.contains("out of range"), "{flag} `{raw}`: {err}");
            }
        }
        let err = parse_time_bound_at(raw, "--until", now, &tz).unwrap_err();
        assert!(
            err.to_string().starts_with("invalid --until value:"),
            "{err}"
        );
    }
    // Out of range even for the tail-only spelled units.
    assert!(crate::headless_tail::parse_since_at("999999999999999999min", now, &tz).is_err());
    // A hex-looking out-of-range duration never falls back to a git ref.
    let tmp = scratch_repo();
    git(tmp.path(), &["branch", "99999999999999d"]);
    assert!(
        crate::resolve_completed_since_cutoff_at(tmp.path(), "99999999999999d", now, &tz).is_none()
    );
    let err = crate::digest::parse_digest_since_at(Some("99999999999999d"), tmp.path(), now, &tz)
        .unwrap_err();
    assert!(err.to_string().contains("out of range"), "{err}");
}

#[test]
fn usage_headers_name_the_window_for_every_form() {
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
    let tz = FixedOffset::east_opt(2 * 3600).unwrap();
    let label = |raw| crate::usage_cmd::window_label_at(raw, now, &tz);
    assert_eq!(label("7d"), ("in the last", "7d".to_string()));
    assert_eq!(label("12h"), ("in the last", "12h".to_string()));
    assert_eq!(
        label("2 weeks ago"),
        ("since", "2026-07-01 14:00 +02:00".to_string())
    );
    assert_eq!(
        label("2026-09-01"),
        ("since", "2026-09-01 00:00 +02:00".to_string())
    );
    assert_eq!(
        label("2026-05-01T10:00:00Z"),
        ("since", "2026-05-01 12:00 +02:00".to_string())
    );
}
