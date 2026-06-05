//! Retry-with-backoff wrapper for the orchestrator's gh/git subprocess calls
//! during `--auto-complete` phases 3-6.
//!
//! A sub-second network blip during `gh pr merge` used to stall a headless
//! drain mid-phase-4. This module classifies a subprocess failure as
//! *transient* (retry) vs *permanent* (give up immediately) by matching the
//! captured stderr against a configurable allow-list, and surfaces every
//! retry attempt to both the user (`StderrSink`) and the drain-state file
//! (`DrainStateSink`).
//!
//! See `docs/plans/2026-05-21-bug-286-network-retry.md`.
//! trace:BUG-286 | ai:claude

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

/// 3 attempts × exponential backoff (1s, 4s) — worst-case ~5s before giving
/// up. The user's BUG-286 incident resolved on the immediate retry; the
/// long tail (TLS renegotiation, DNS flap) settles inside the second wait.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_BASE_DELAY_MS: u64 = 1_000;
const DEFAULT_BACKOFF_FACTOR: u32 = 4;

/// Tunables the orchestrator passes to [`run_with_retry`]. Loaded from
/// `.aida/config.toml [orchestrator]` via [`RetryConfig::load`].
#[derive(Debug, Clone)]
pub(crate) struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub factor: u32,
    pub transient_patterns: Vec<String>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            base_delay: Duration::from_millis(DEFAULT_BASE_DELAY_MS),
            factor: DEFAULT_BACKOFF_FACTOR,
            transient_patterns: default_transient_patterns()
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

impl RetryConfig {
    /// Load `[orchestrator]` overrides from `.aida/config.toml`. Silently
    /// falls back to defaults when the file is missing or malformed — a
    /// broken config must not poison the retry path.
    pub(crate) fn load(project_root: &Path) -> Self {
        let mut cfg = Self::default();
        let path = project_root.join(".aida").join("config.toml");
        let Ok(content) = std::fs::read_to_string(&path) else {
            return cfg;
        };
        let Ok(toml_val) = content.parse::<toml::Value>() else {
            return cfg;
        };
        let Some(orch) = toml_val.get("orchestrator").and_then(|v| v.as_table()) else {
            return cfg;
        };
        if let Some(patterns) = orch
            .get("transient_error_patterns")
            .and_then(|v| v.as_array())
        {
            let pats: Vec<String> = patterns
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .collect();
            if !pats.is_empty() {
                cfg.transient_patterns = pats;
            }
        }
        if let Some(n) = orch.get("retry_max_attempts").and_then(|v| v.as_integer()) {
            cfg.max_attempts = n.clamp(1, 10) as u32;
        }
        if let Some(ms) = orch.get("retry_base_delay_ms").and_then(|v| v.as_integer()) {
            cfg.base_delay = Duration::from_millis(ms.max(0) as u64);
        }
        cfg
    }
}

/// The transient-error allow-list seeded by `RetryConfig::default`. Matched
/// against the captured stderr of the failed attempt; any substring hit
/// classifies the failure as transient. Mirrors the patterns BUG-257 /
/// BUG-266 already use for the phase-1 implementer leg.
pub(crate) fn default_transient_patterns() -> &'static [&'static str] {
    &[
        // gh / git network layer
        "error connecting to",
        "Could not resolve host",
        "could not resolve host",
        "Connection reset by peer",
        "connection reset by peer",
        "Connection timed out",
        "connection timed out",
        "Network is unreachable",
        "network is unreachable",
        "Temporary failure in name resolution",
        "no route to host",
        "No route to host",
        // TLS layer
        "TLS handshake",
        "tls handshake",
        "SSL_ERROR",
        "SSL connect error",
        "handshake failure",
        // HTTP 5xx
        "502 Bad Gateway",
        "503 Service Unavailable",
        "504 Gateway",
        "server returned HTTP error 5",
        // GitHub-specific transient framing
        "GraphQL error: Something went wrong",
        "API rate limit exceeded for", // technically rate-limit — but waits resolve it
    ]
}

/// One retry attempt the sink is asked to log. `attempt` is 1-indexed and is
/// the number of the call that *just failed*; `backoff_ms` is the wait before
/// the next call.
#[derive(Debug, Clone)]
pub struct RetryEvent {
    pub label: String,
    pub attempt: u32,
    pub max: u32,
    pub backoff_ms: u64,
    pub stderr_snippet: String,
}

/// Where retry events go. The orchestrator wires a [`DualSink`] of
/// [`StderrSink`] + drain-state sink so post-hoc analysis can correlate.
/// `pub` (not `pub(crate)`) so it can appear in the `Forge::merge_change`
/// signature — STORY-516 routes the orchestrator merge's DualSink through it.
/// trace:STORY-516 | ai:claude
pub trait RetrySink {
    fn on_retry(&mut self, ev: &RetryEvent);
}

/// Default sink: prints the orchestrator's `↻ <label> — transient error
/// (attempt N/M) — retrying in Xms` line to stderr.
pub(crate) struct StderrSink;

impl RetrySink for StderrSink {
    fn on_retry(&mut self, ev: &RetryEvent) {
        use colored::Colorize;
        eprintln!(
            "  {} {} — transient error (attempt {}/{}) — retrying in {}ms",
            "↻".yellow(),
            ev.label,
            ev.attempt,
            ev.max,
            ev.backoff_ms
        );
        if !ev.stderr_snippet.is_empty() {
            eprintln!("    {}", ev.stderr_snippet.dimmed());
        }
    }
}

/// Discard sink — used by tests and by hypothetical non-orchestrator callers
/// that just want the retry behaviour without the user-facing log line.
#[allow(dead_code)]
pub(crate) struct NoopSink;

impl RetrySink for NoopSink {
    fn on_retry(&mut self, _ev: &RetryEvent) {}
}

/// Fan-out to two sinks. Lets the orchestrator mirror to stderr *and* the
/// drain-state file in one `run_with_retry` call.
pub(crate) struct DualSink<'a, A: RetrySink + ?Sized, B: RetrySink + ?Sized> {
    pub a: &'a mut A,
    pub b: &'a mut B,
}

impl<'a, A: RetrySink + ?Sized, B: RetrySink + ?Sized> RetrySink for DualSink<'a, A, B> {
    fn on_retry(&mut self, ev: &RetryEvent) {
        self.a.on_retry(ev);
        self.b.on_retry(ev);
    }
}

/// Substring-match against the transient-error allow-list. Empty patterns
/// are skipped so an empty config entry can't accidentally classify
/// everything as transient.
pub(crate) fn classify_transient(stderr: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|p| !p.is_empty() && stderr.contains(p.as_str()))
}

fn first_meaningful_stderr_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(180)
        .collect()
}

/// Run `build()`'s `Command` with capture-output retry-on-transient. Returns
/// the final [`Output`] (success or final failure) — the caller decides
/// whether to surface non-success as a phase failure, since the
/// success-vs-failure classification differs per call site (`gh pr merge`
/// vs `gh pr view`).
///
/// `build` is a closure rather than a `Command` because `Command` is not
/// cloneable; we mint a fresh one per attempt.
pub(crate) fn run_with_retry(
    label: &str,
    config: &RetryConfig,
    sink: &mut dyn RetrySink,
    mut build: impl FnMut() -> Command,
) -> std::io::Result<Output> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let out = build().output()?;
        if out.status.success() || attempt >= config.max_attempts {
            return Ok(out);
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !classify_transient(stderr.as_ref(), &config.transient_patterns) {
            return Ok(out);
        }
        let backoff = config
            .base_delay
            .saturating_mul(config.factor.saturating_pow(attempt - 1));
        sink.on_retry(&RetryEvent {
            label: label.to_string(),
            attempt,
            max: config.max_attempts,
            backoff_ms: backoff.as_millis() as u64,
            stderr_snippet: first_meaningful_stderr_line(stderr.as_ref()),
        });
        std::thread::sleep(backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn pats(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn classifies_transient_stderr() {
        let p = pats(&["error connecting to", "Could not resolve host"]);
        assert!(classify_transient("error connecting to api.github.com", &p));
        assert!(classify_transient(
            "Could not resolve host: api.github.com",
            &p
        ));
    }

    #[test]
    fn classifies_permanent_stderr() {
        let p = pats(&["error connecting to", "Could not resolve host"]);
        assert!(!classify_transient("HTTP 404 Not Found", &p));
        assert!(!classify_transient("authentication failed", &p));
    }

    #[test]
    fn empty_pattern_does_not_match_everything() {
        let p = pats(&[""]);
        assert!(!classify_transient("anything at all", &p));
    }

    #[test]
    fn default_patterns_cover_bug_286_symptom() {
        let cfg = RetryConfig::default();
        assert!(classify_transient(
            "error connecting to api.github.com\ncheck your internet connection",
            &cfg.transient_patterns
        ));
    }

    /// Capturing test sink for assertions.
    #[derive(Default)]
    struct CaptureSink {
        events: RefCell<Vec<RetryEvent>>,
    }

    impl RetrySink for CaptureSink {
        fn on_retry(&mut self, ev: &RetryEvent) {
            self.events.borrow_mut().push(ev.clone());
        }
    }

    /// Fast-test config: 1ms base delay, no factor (1× every wait) so a
    /// 3-attempt scenario takes <5ms wall-clock.
    fn fast_cfg(patterns: Vec<String>) -> RetryConfig {
        RetryConfig {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            factor: 1,
            transient_patterns: patterns,
        }
    }

    /// Subprocess that fails N times (writing a transient-pattern stderr)
    /// then succeeds. Implementation: a shell script using a counter file.
    fn build_flaky_cmd(counter_file: &std::path::Path, fail_n: u32, msg: &str) -> Command {
        // sh -c with the counter file path + threshold + message as args.
        let script = r#"
            COUNT_FILE="$1"
            FAIL_N="$2"
            MSG="$3"
            count=$(cat "$COUNT_FILE" 2>/dev/null || echo 0)
            count=$((count + 1))
            echo "$count" > "$COUNT_FILE"
            if [ "$count" -le "$FAIL_N" ]; then
                printf '%s\n' "$MSG" >&2
                exit 1
            fi
            exit 0
        "#;
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(script)
            .arg("flaky") // $0
            .arg(counter_file)
            .arg(fail_n.to_string())
            .arg(msg);
        c
    }

    #[test]
    fn retries_then_succeeds_on_transient() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count");
        let mut sink = CaptureSink::default();
        let cfg = fast_cfg(pats(&["error connecting to"]));

        let out = run_with_retry("gh pr merge 42", &cfg, &mut sink, || {
            build_flaky_cmd(&counter, 2, "error connecting to api.github.com")
        })
        .expect("subprocess should spawn");
        assert!(out.status.success(), "third attempt should succeed");

        let events = sink.events.borrow();
        assert_eq!(
            events.len(),
            2,
            "two retries before the third-attempt success"
        );
        assert_eq!(events[0].attempt, 1);
        assert_eq!(events[0].max, 3);
        assert_eq!(events[1].attempt, 2);
        assert!(events[0].stderr_snippet.contains("error connecting to"));
    }

    #[test]
    fn permanent_failure_is_not_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count");
        let mut sink = CaptureSink::default();
        let cfg = fast_cfg(pats(&["error connecting to"]));

        let out = run_with_retry("gh pr merge 42", &cfg, &mut sink, || {
            // Fails forever with a permanent-shape stderr.
            build_flaky_cmd(&counter, 99, "HTTP 404 Not Found")
        })
        .expect("subprocess should spawn");
        assert!(!out.status.success());
        assert_eq!(
            sink.events.borrow().len(),
            0,
            "no retries on permanent failure"
        );
    }

    #[test]
    fn transient_failure_exhausts_attempts() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count");
        let mut sink = CaptureSink::default();
        let cfg = fast_cfg(pats(&["error connecting to"]));

        let out = run_with_retry("gh pr merge 42", &cfg, &mut sink, || {
            build_flaky_cmd(&counter, 99, "error connecting to api.github.com")
        })
        .expect("subprocess should spawn");
        assert!(
            !out.status.success(),
            "exhausted retries return the final failure"
        );
        // max_attempts=3 → 2 retry events logged (between attempts 1→2 and 2→3).
        assert_eq!(sink.events.borrow().len(), 2);
    }

    #[test]
    fn success_on_first_attempt_logs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count");
        let mut sink = CaptureSink::default();
        let cfg = fast_cfg(pats(&["error connecting to"]));

        let out = run_with_retry("gh pr merge 42", &cfg, &mut sink, || {
            build_flaky_cmd(&counter, 0, "unused")
        })
        .expect("subprocess should spawn");
        assert!(out.status.success());
        assert_eq!(sink.events.borrow().len(), 0);
    }

    #[test]
    fn dual_sink_fans_out() {
        let mut a = CaptureSink::default();
        let mut b = CaptureSink::default();
        {
            let mut dual = DualSink {
                a: &mut a,
                b: &mut b,
            };
            dual.on_retry(&RetryEvent {
                label: "x".into(),
                attempt: 1,
                max: 3,
                backoff_ms: 10,
                stderr_snippet: "boom".into(),
            });
        }
        assert_eq!(a.events.borrow().len(), 1);
        assert_eq!(b.events.borrow().len(), 1);
    }

    #[test]
    fn config_load_reads_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".aida");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            r#"
[orchestrator]
retry_max_attempts = 5
retry_base_delay_ms = 250
transient_error_patterns = ["custom-blip", "another"]
"#,
        )
        .unwrap();
        let cfg = RetryConfig::load(tmp.path());
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.base_delay, Duration::from_millis(250));
        assert_eq!(
            cfg.transient_patterns,
            vec!["custom-blip".to_string(), "another".to_string()]
        );
    }

    #[test]
    fn config_load_uses_defaults_for_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = RetryConfig::load(tmp.path());
        let default = RetryConfig::default();
        assert_eq!(cfg.max_attempts, default.max_attempts);
        assert_eq!(cfg.base_delay, default.base_delay);
        assert_eq!(cfg.transient_patterns, default.transient_patterns);
    }

    #[test]
    fn config_load_uses_defaults_for_malformed_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".aida");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "not valid = = toml ===").unwrap();
        let cfg = RetryConfig::load(tmp.path());
        assert_eq!(cfg.max_attempts, RetryConfig::default().max_attempts);
    }

    #[test]
    fn max_attempts_clamped_to_safe_range() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".aida");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[orchestrator]\nretry_max_attempts = 9999\n",
        )
        .unwrap();
        let cfg = RetryConfig::load(tmp.path());
        assert!(cfg.max_attempts <= 10);
    }

    /// Integration-style test: fake `gh` script that prints a transient
    /// pattern N times then succeeds. Asserts the orchestrator-level retry
    /// loop consumes the blip and surfaces the right RetryEvents.
    #[test]
    fn integration_fake_gh_with_transient_blip() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("gh-count");
        let mut sink = CaptureSink::default();
        let cfg = fast_cfg(pats(default_transient_patterns()));

        // Spawn count
        let spawned = AtomicU32::new(0);
        let out = run_with_retry(
            "gh pr merge 157 --squash --delete-branch",
            &cfg,
            &mut sink,
            || {
                spawned.fetch_add(1, Ordering::SeqCst);
                // 1 transient failure then succeed — typical BUG-286 symptom.
                build_flaky_cmd(&counter, 1, "error connecting to api.github.com")
            },
        )
        .expect("subprocess should spawn");
        assert!(out.status.success(), "phase 4 retry resolves the blip");
        assert_eq!(
            spawned.load(Ordering::SeqCst),
            2,
            "1 fail + 1 success = 2 spawns"
        );
        assert_eq!(sink.events.borrow().len(), 1);
        assert_eq!(
            sink.events.borrow()[0].label,
            "gh pr merge 157 --squash --delete-branch"
        );
    }
}
