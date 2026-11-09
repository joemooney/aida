// trace:TASK-3426 | ai:codex
use super::*;

/// Parse a `--since`/`--until` style time bound into an absolute UTC instant.
/// Accepted forms, tried in this order:
///
/// - RFC3339 (`2026-05-01T10:00:00Z`, `2026-05-01T10:00:00+02:00`): the zone
///   in the input is honored exactly.
/// - A bare ISO date (`2026-05-01`): local midnight on that date.
/// - A zone-less ISO datetime (`2026-05-01T10:00`, `2026-05-01 10:00`,
///   optionally with seconds): that wall-clock time in the local timezone.
/// - A compact relative duration `<N>{m,h,d,w}` (`45m`, `12h`, `2d`, `2w`),
///   or the phrase form `<N> <unit>(s) ago` with unit minute/hour/day/week
///   (`24 hours ago`, `1 week ago`): that long before now.
///
/// Local-time forms use the offset in effect on the requested date, so a
/// January date and a July date resolve correctly across a DST change. A
/// local time that falls in a DST gap or overlap is rejected rather than
/// guessed.
///
/// Every time-bound flag in the CLI resolves through this grammar (history,
/// queue progress, archive, approvals, review/findings classes, status,
/// digest, doctor, calibration, tail, usage, metrics, mailbox). The list is
/// enforced by `tests/time_bound_flags_tests.rs`, which walks the clap tree.
// trace:TASK-1502 | ai:claude
// trace:TASK-1509 | ai:claude
pub(crate) fn parse_since_arg(raw: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    parse_since_arg_at(raw, chrono::Utc::now(), &chrono::Local)
}

/// A local wall-clock time that has no single meaning in the local timezone
/// (it falls in a DST gap or overlap). Kept as a distinct error type so
/// callers that relabel parse errors can still surface this one verbatim.
// trace:TASK-1502 | ai:claude
#[derive(Debug)]
pub(crate) struct AmbiguousLocalTime(pub String);

impl std::fmt::Display for AmbiguousLocalTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is ambiguous or doesn't exist in the local timezone (a DST \
             transition); give an explicit offset instead, e.g. RFC3339",
            self.0
        )
    }
}

impl std::error::Error for AmbiguousLocalTime {}

/// Zone-less ISO datetime layouts accepted as local wall-clock time.
pub(crate) const NAIVE_DATETIME_FORMATS: &[&str] = &[
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%d %H:%M",
];

/// [`parse_since_arg`], but resolves relative durations against an explicit
/// `now` and local-time forms against an explicit timezone `tz`, instead of
/// the wall clock and system timezone. Production passes `chrono::Local`;
/// tests pass a `FixedOffset` or a DST-observing zone for deterministic
/// output.
// trace:TASK-1502 | ai:claude
pub(crate) fn parse_since_arg_at<Tz: chrono::TimeZone>(
    raw: &str,
    now: chrono::DateTime<chrono::Utc>,
    tz: &Tz,
) -> Result<chrono::DateTime<chrono::Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--since cannot be empty");
    }
    // An explicit zone in the input is always honored exactly.
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Ok(ts.with_timezone(&chrono::Utc));
    }
    // Local-time forms: resolve against the zone's offset on THAT date, so
    // a DST gap or overlap is detected instead of silently misapplied.
    let local = |naive: chrono::NaiveDateTime| -> Result<chrono::DateTime<chrono::Utc>> {
        tz.from_local_datetime(&naive)
            .single()
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .ok_or_else(|| anyhow::Error::new(AmbiguousLocalTime(trimmed.to_string())))
    };
    if let Ok(d) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return local(d.and_time(chrono::NaiveTime::MIN));
    }
    for fmt in NAIVE_DATETIME_FORMATS {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, fmt) {
            return local(naive);
        }
    }
    let invalid = || {
        anyhow::anyhow!(
            "invalid --since value `{}` (try `2d`, `12h`, `2w`, `24 hours ago`, \
             an ISO date or datetime, or RFC3339)",
            raw
        )
    };
    // Phrase form: `<N> <unit>(s) ago`, kept for callers of the old
    // git-approxidate-backed history flags.
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let (num_str, unit) = if let [n, unit, ago] = words.as_slice() {
        if !ago.eq_ignore_ascii_case("ago") {
            return Err(invalid());
        }
        let unit = unit.to_ascii_lowercase();
        let unit = match unit.strip_suffix('s').unwrap_or(&unit) {
            "minute" => "m",
            "hour" => "h",
            "day" => "d",
            "week" => "w",
            _ => return Err(invalid()),
        };
        (*n, unit)
    } else {
        // Compact form: <number><unit>, unit ∈ {m,h,d,w}.
        // BUG-100: peel the last CHAR rather than the last BYTE so
        // multi-byte trailing units don't crash the process.
        split_last_char(trimmed)
    };
    let n: i64 = num_str.parse().map_err(|_| invalid())?;
    // Checked arithmetic: a huge count is refused, never a panic.
    // trace:TASK-1509 | ai:claude
    let delta = match unit {
        "d" => chrono::Duration::try_days(n),
        "h" => chrono::Duration::try_hours(n),
        "m" => chrono::Duration::try_minutes(n),
        "w" => chrono::Duration::try_weeks(n),
        _ => anyhow::bail!(
            "invalid --since unit `{}`: use m/h/d/w, `<N> <unit>s ago`, an ISO \
             date or datetime, or RFC3339",
            unit
        ),
    };
    delta
        .and_then(|d| now.checked_sub_signed(d))
        .ok_or_else(|| anyhow::Error::new(TimeBoundOutOfRange(trimmed.to_string())))
}

/// A relative duration too large to resolve to a representable instant
/// (e.g. `99999999999999d`). Kept as a distinct error type, like
/// [`AmbiguousLocalTime`], so callers surface it verbatim instead of
/// retrying the value as something else.
// trace:TASK-1509 | ai:claude
#[derive(Debug)]
pub(crate) struct TimeBoundOutOfRange(pub String);

impl std::fmt::Display for TimeBoundOutOfRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is out of range: the duration reaches too far back to \
             resolve to a date",
            self.0
        )
    }
}

impl std::error::Error for TimeBoundOutOfRange {}

/// True when a [`parse_since_arg_at`] error is a definitive answer about a
/// value that does match the grammar (a DST gap/overlap, or an out-of-range
/// duration), rather than "not this grammar". Callers with a fallback (git
/// ref, tail-only units) must report these instead of retrying the value.
// trace:TASK-1509 | ai:claude
pub(crate) fn is_definitive_time_bound_error(e: &anyhow::Error) -> bool {
    e.is::<AmbiguousLocalTime>() || e.is::<TimeBoundOutOfRange>()
}

/// [`parse_since_arg_at`] with an error labeled for the flag that produced
/// it (`parse_since_arg_at`'s own message always says `--since`, which would
/// misname a bad `--until` or `--older-than`). A DST gap/overlap error is kept
/// verbatim so the caller learns why the value was refused; so is an
/// out-of-range duration.
// trace:TASK-1509 | ai:claude
pub(crate) fn parse_time_bound_at<Tz: chrono::TimeZone>(
    raw: &str,
    flag: &str,
    now: chrono::DateTime<chrono::Utc>,
    tz: &Tz,
) -> Result<chrono::DateTime<chrono::Utc>> {
    parse_since_arg_at(raw, now, tz).map_err(|e| {
        if is_definitive_time_bound_error(&e) {
            return anyhow::anyhow!("invalid {flag} value: {e}");
        }
        anyhow::anyhow!(
            "invalid {flag} value `{raw}` — expected a relative duration \
             (e.g. `5h`, `7d`, `30m`, `2w`, `24 hours ago`), an ISO date \
             (`2026-05-01`, local midnight), a zone-less ISO datetime \
             (`2026-05-01T10:00`, local time), or RFC3339"
        )
    })
}

/// A time bound expressed as a lookback: how far before `now` the bound
/// resolved by [`parse_time_bound_at`] lies. For the commands whose filters
/// take a window length rather than an instant (usage, metrics, calibration,
/// mailbox archive). A bound in the future yields a negative duration.
// trace:TASK-1509 | ai:claude
pub(crate) fn parse_lookback_at<Tz: chrono::TimeZone>(
    raw: &str,
    flag: &str,
    now: chrono::DateTime<chrono::Utc>,
    tz: &Tz,
) -> Result<chrono::Duration> {
    let at = parse_time_bound_at(raw, flag, now, tz)?;
    Ok(now.signed_duration_since(at))
}

/// [`parse_lookback_at`] against the wall clock and the system timezone.
// trace:TASK-1509 | ai:claude
pub fn parse_lookback(raw: &str, flag: &str) -> Result<chrono::Duration> {
    parse_lookback_at(raw, flag, chrono::Utc::now(), &chrono::Local)
}
