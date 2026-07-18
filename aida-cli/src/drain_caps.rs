//! Hard budget caps for autonomous drains (TASK-966).
//!
//! `aida goal` expresses *what* "done" looks like; nothing expresses *"…or stop
//! after N tokens / N iterations / N minutes."* An unattended `drain-loop.sh`
//! can otherwise silently burn a weekly quota on a wedged chunk. This module is
//! the pure heart of three orthogonal stop conditions wired into
//! `aida queue work --auto-complete`:
//!
//! - `--max-tokens <n>` — abort once cumulative reported tokens cross the cap
//!   (checked at each spec/phase boundary, where the headless `claude -p`
//!   `stream-json` output reports usage).
//! - `--max-iterations <n>` — stop *before* the next spec begins.
//! - `--max-runtime <dur>` — wall-clock cap, stop *between* specs past the
//!   deadline.
//!
//! The caps compose with the existing `--max-failures` budget and any `aida
//! goal` condition: whichever fires first stops the drain. A cap stop is a
//! *clean* intentional stop — it reports why and never loses in-flight state.
//!
//! Everything here is pure (no I/O) so the cap-checking logic is unit-testable:
//! given accumulated counters + caps, decide should-stop + reason.
// trace:TASK-966 | ai:claude

use std::time::Duration;

/// The three orthogonal hard caps. Any combination may be active; an unset cap
/// (`None`) never fires.
// trace:TASK-966 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DrainCaps {
    /// Cumulative reported-token ceiling across every phase/spec of the drain.
    pub(crate) max_tokens: Option<u64>,
    /// Maximum number of specs the drain may *act on* before stopping.
    pub(crate) max_iterations: Option<u64>,
    /// Wall-clock ceiling for the whole drain.
    pub(crate) max_runtime: Option<Duration>,
}

impl DrainCaps {
    /// True when at least one cap is set — lets callers skip the (cheap)
    /// token-accounting work entirely when no cap is active.
    pub(crate) fn is_active(&self) -> bool {
        self.max_tokens.is_some() || self.max_iterations.is_some() || self.max_runtime.is_some()
    }

    /// Between-specs check: iteration + runtime caps, evaluated *before* the
    /// next spec is dispatched. The token cap is intentionally NOT checked here
    /// — tokens are only known *after* a phase reports usage, so the loop checks
    /// them via [`DrainCaps::check_tokens`] once the counter is updated.
    pub(crate) fn check_before_iteration(&self, counters: &DrainCounters) -> Option<CapStop> {
        if let Some(cap) = self.max_iterations {
            if counters.iterations >= cap {
                return Some(CapStop::Iterations {
                    completed: counters.iterations,
                    cap,
                });
            }
        }
        if let Some(cap) = self.max_runtime {
            if counters.elapsed >= cap {
                return Some(CapStop::Runtime {
                    elapsed_secs: counters.elapsed.as_secs(),
                    cap_secs: cap.as_secs(),
                });
            }
        }
        None
    }

    /// Token check: evaluated *after* a phase/spec reports cumulative usage. The
    /// drain stops once accumulated tokens cross the cap (the "abort the
    /// in-flight phase" condition at the coarsest granularity the headless
    /// `stream-json` output makes available — per-phase usage events).
    pub(crate) fn check_tokens(&self, counters: &DrainCounters) -> Option<CapStop> {
        if let Some(cap) = self.max_tokens {
            if counters.tokens >= cap {
                return Some(CapStop::Tokens {
                    used: counters.tokens,
                    cap,
                });
            }
        }
        None
    }
}

/// Accumulated drain progress measured against [`DrainCaps`].
// trace:TASK-966 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DrainCounters {
    /// Cumulative reported tokens (input + output + cache) across every headless
    /// phase the drain has run so far.
    pub(crate) tokens: u64,
    /// Number of specs the drain has acted on (shipped + punted + escalated +
    /// shelved) so far.
    pub(crate) iterations: u64,
    /// Wall time elapsed since the drain started.
    pub(crate) elapsed: Duration,
}

/// Which cap stopped the drain, with the numbers needed to explain it. The
/// `reason()` rendering is what the drain summary prints.
// trace:TASK-966 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CapStop {
    /// `--max-tokens` reached: cumulative reported tokens crossed the cap.
    Tokens { used: u64, cap: u64 },
    /// `--max-iterations` reached: enough specs acted on, stop before the next.
    Iterations { completed: u64, cap: u64 },
    /// `--max-runtime` reached: wall-clock deadline passed, stop between specs.
    Runtime { elapsed_secs: u64, cap_secs: u64 },
}

impl CapStop {
    /// The flag name that fired — for machine-readable summaries / JSON.
    pub(crate) fn flag(&self) -> &'static str {
        match self {
            CapStop::Tokens { .. } => "max-tokens",
            CapStop::Iterations { .. } => "max-iterations",
            CapStop::Runtime { .. } => "max-runtime",
        }
    }

    /// One-line human explanation of why the drain stopped cleanly.
    pub(crate) fn reason(&self) -> String {
        match self {
            CapStop::Tokens { used, cap } => format!(
                "reached the --max-tokens budget ({used} reported tokens >= {cap}) — \
                 stopping the drain cleanly"
            ),
            CapStop::Iterations { completed, cap } => format!(
                "reached the --max-iterations budget ({completed} specs >= {cap}) — \
                 stopping before the next spec"
            ),
            CapStop::Runtime {
                elapsed_secs,
                cap_secs,
            } => format!(
                "reached the --max-runtime budget ({} elapsed >= {}) — \
                 stopping between specs",
                fmt_secs(*elapsed_secs),
                fmt_secs(*cap_secs)
            ),
        }
    }
}

/// Render a whole-second duration compactly (`90s` → `1m30s`, `3600s` → `1h`).
fn fmt_secs(total: u64) -> String {
    if total == 0 {
        return "0s".to_string();
    }
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 {
        out.push_str(&format!("{s}s"));
    }
    out
}

/// Parse a `--max-runtime` value: a bare integer = minutes, or a suffixed /
/// compound form (`90s`, `45m`, `2h`, `1h30m`). Whitespace and a trailing
/// `min`/`hr` are tolerated. Returns `None` on anything unparseable so the CLI
/// can reject it with a clear message rather than silently using a wrong cap.
// trace:TASK-966 | ai:claude
pub(crate) fn parse_duration(input: &str) -> Option<Duration> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    // Bare integer → minutes (the unattended-drain natural unit).
    if let Ok(mins) = s.parse::<u64>() {
        return Some(Duration::from_secs(mins.saturating_mul(60)));
    }
    let mut total: u64 = 0;
    let mut num = String::new();
    let mut saw_unit = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        if num.is_empty() {
            return None; // a unit with no preceding number
        }
        let value: u64 = num.parse().ok()?;
        num.clear();
        // Allow `min`/`hr`/`sec` longhand by consuming trailing letters of the
        // same unit token.
        let unit = c;
        while matches!(chars.peek(), Some(p) if p.is_ascii_alphabetic()) {
            chars.next();
        }
        let secs = match unit {
            's' => value,
            'm' => value.saturating_mul(60),
            'h' => value.saturating_mul(3600),
            _ => return None,
        };
        total = total.saturating_add(secs);
        saw_unit = true;
    }
    // A trailing bare number after a unit (e.g. "1h30") is ambiguous — reject.
    if !num.is_empty() {
        return None;
    }
    if !saw_unit {
        return None;
    }
    Some(Duration::from_secs(total))
}

/// Extract the cumulative token count from one parsed `claude -p`
/// `--output-format stream-json` JSON value: input + output + cache-creation +
/// cache-read tokens from a `usage` object found at the top level (the terminal
/// `result` event) or under `message.usage` (an `assistant` event). Returns
/// `None` when the value carries no usage.
// trace:TASK-966 | ai:claude
fn usage_tokens(v: &serde_json::Value) -> Option<u64> {
    let usage = v
        .get("usage")
        .or_else(|| v.get("message").and_then(|m| m.get("usage")))?;
    let sum: u64 = [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .iter()
    .filter_map(|k| usage.get(*k).and_then(serde_json::Value::as_u64))
    .sum();
    Some(sum)
}

/// Parse a single line of `claude -p --output-format stream-json` output and
/// return its cumulative token total, or `None` if the line is not JSON / has
/// no usage.
// trace:TASK-966 | ai:claude
#[allow(dead_code)] // per-line seam over usage_tokens; exercised by tests, production sums via tokens_from_log
pub(crate) fn parse_usage_tokens(line: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    usage_tokens(&v)
}

/// Sum the tokens a single headless phase reported, from the full text of its
/// `stream-json` log. Claude's terminal `result` event carries the session's
/// *cumulative* usage, so we prefer it; absent a result event (a killed /
/// truncated log) we fall back to the largest per-line usage seen, which is the
/// best lower bound available. Returns `0` for an empty / non-JSON log (an
/// interactive phase writes no such log, so it contributes nothing).
// trace:TASK-966 | ai:claude
pub(crate) fn tokens_from_log(contents: &str) -> u64 {
    let mut result_tokens: Option<u64> = None;
    let mut max_tokens: u64 = 0;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(t) = usage_tokens(&v) {
            max_tokens = max_tokens.max(t);
            if v.get("type").and_then(serde_json::Value::as_str) == Some("result") {
                result_tokens = Some(t);
            }
        }
    }
    result_tokens.unwrap_or(max_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(tokens: u64, iterations: u64, elapsed_secs: u64) -> DrainCounters {
        DrainCounters {
            tokens,
            iterations,
            elapsed: Duration::from_secs(elapsed_secs),
        }
    }

    #[test]
    fn no_caps_never_stops() {
        let caps = DrainCaps::default();
        assert!(!caps.is_active());
        let c = counters(1_000_000, 999, 999_999);
        assert_eq!(caps.check_before_iteration(&c), None);
        assert_eq!(caps.check_tokens(&c), None);
    }

    #[test]
    fn max_iterations_stops_before_next_spec() {
        let caps = DrainCaps {
            max_iterations: Some(3),
            ..Default::default()
        };
        // Two acted-on specs: keep going.
        assert_eq!(caps.check_before_iteration(&counters(0, 2, 0)), None);
        // Third boundary reached: stop before the next.
        assert_eq!(
            caps.check_before_iteration(&counters(0, 3, 0)),
            Some(CapStop::Iterations {
                completed: 3,
                cap: 3
            })
        );
        // Over (defensive): still stops.
        assert!(caps.check_before_iteration(&counters(0, 5, 0)).is_some());
    }

    #[test]
    fn max_runtime_stops_between_specs_past_deadline() {
        let caps = DrainCaps {
            max_runtime: Some(Duration::from_secs(600)),
            ..Default::default()
        };
        // Under the deadline: keep going.
        assert_eq!(caps.check_before_iteration(&counters(0, 0, 599)), None);
        // At/over the deadline: stop.
        match caps.check_before_iteration(&counters(0, 0, 600)) {
            Some(CapStop::Runtime {
                elapsed_secs,
                cap_secs,
            }) => {
                assert_eq!(elapsed_secs, 600);
                assert_eq!(cap_secs, 600);
            }
            other => panic!("expected runtime stop, got {other:?}"),
        }
    }

    #[test]
    fn max_tokens_aborts_once_accumulated_exceeds_cap() {
        let caps = DrainCaps {
            max_tokens: Some(50_000),
            ..Default::default()
        };
        // Under cap: keep going.
        assert_eq!(caps.check_tokens(&counters(49_999, 0, 0)), None);
        // At cap: stop.
        assert_eq!(
            caps.check_tokens(&counters(50_000, 0, 0)),
            Some(CapStop::Tokens {
                used: 50_000,
                cap: 50_000
            })
        );
        // Over cap: stop.
        assert!(caps.check_tokens(&counters(60_000, 0, 0)).is_some());
        // Token cap is NOT consulted by the between-iteration check.
        assert_eq!(caps.check_before_iteration(&counters(60_000, 0, 0)), None);
    }

    #[test]
    fn iteration_cap_takes_precedence_over_runtime_when_both_fire() {
        let caps = DrainCaps {
            max_iterations: Some(2),
            max_runtime: Some(Duration::from_secs(1)),
            ..Default::default()
        };
        // Both would fire; iteration is checked first and wins the report.
        assert_eq!(
            caps.check_before_iteration(&counters(0, 5, 100)),
            Some(CapStop::Iterations {
                completed: 5,
                cap: 2
            })
        );
    }

    #[test]
    fn cap_stop_reports_flag_and_reason() {
        assert_eq!(CapStop::Tokens { used: 10, cap: 5 }.flag(), "max-tokens");
        assert_eq!(
            CapStop::Iterations {
                completed: 3,
                cap: 3
            }
            .flag(),
            "max-iterations"
        );
        assert_eq!(
            CapStop::Runtime {
                elapsed_secs: 60,
                cap_secs: 60
            }
            .flag(),
            "max-runtime"
        );
        assert!(CapStop::Tokens { used: 10, cap: 5 }
            .reason()
            .contains("--max-tokens"));
        assert!(CapStop::Runtime {
            elapsed_secs: 90,
            cap_secs: 60
        }
        .reason()
        .contains("1m30s"));
    }

    #[test]
    fn parse_duration_bare_integer_is_minutes() {
        assert_eq!(parse_duration("10"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration(" 0 "), Some(Duration::from_secs(0)));
    }

    #[test]
    fn parse_duration_suffixed_and_compound() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("45m"), Some(Duration::from_secs(2700)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("1H30M"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("90min"), Some(Duration::from_secs(5400)));
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("h"), None);
        assert_eq!(parse_duration("1x"), None);
        assert_eq!(parse_duration("1h30"), None);
    }

    #[test]
    fn parse_usage_tokens_from_result_event() {
        let line = r#"{"type":"result","subtype":"success","usage":{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":5,"cache_read_input_tokens":100}}"#;
        assert_eq!(parse_usage_tokens(line), Some(135));
    }

    #[test]
    fn parse_usage_tokens_from_assistant_message() {
        let line =
            r#"{"type":"assistant","message":{"usage":{"input_tokens":4,"output_tokens":8}}}"#;
        assert_eq!(parse_usage_tokens(line), Some(12));
    }

    #[test]
    fn parse_usage_tokens_none_when_no_usage() {
        assert_eq!(parse_usage_tokens(r#"{"type":"system"}"#), None);
        assert_eq!(parse_usage_tokens("not json"), None);
        assert_eq!(parse_usage_tokens(""), None);
    }

    #[test]
    fn tokens_from_log_prefers_result_event() {
        let log = [
            r#"{"type":"assistant","message":{"usage":{"input_tokens":4,"output_tokens":8}}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":40}}}"#,
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":12,"output_tokens":50,"cache_read_input_tokens":1000}}"#,
            "",
        ]
        .join("\n");
        // Result event is cumulative: 12 + 50 + 1000.
        assert_eq!(tokens_from_log(&log), 1062);
    }

    #[test]
    fn tokens_from_log_falls_back_to_max_without_result_event() {
        let log = [
            r#"{"type":"assistant","message":{"usage":{"input_tokens":4,"output_tokens":8}}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":40}}}"#,
        ]
        .join("\n");
        assert_eq!(tokens_from_log(&log), 50);
    }

    #[test]
    fn tokens_from_log_zero_for_empty_or_garbage() {
        assert_eq!(tokens_from_log(""), 0);
        assert_eq!(tokens_from_log("not json\nstill not json"), 0);
    }
}
