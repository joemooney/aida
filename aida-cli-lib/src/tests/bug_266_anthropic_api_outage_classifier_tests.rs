use super::*;

/// Verbatim 529 incident text from the BUG-266 origin (TASK-358 drain on
/// 2026-05-20). Must classify as outage, and the returned reason must
/// surface the matched phrasing so the orchestrator's epilogue echoes it.
// trace:BUG-266 | ai:claude
#[test]
fn verbatim_529_overloaded_is_outage() {
    let log = "{\"type\":\"assistant\",\"text\":\"API Error: 529 Overloaded. \
                   This is a server-side issue, usually temporary — try again in \
                   a moment. If it persists, check status.claude.com.\"}\n";
    let reason = claude_log_indicates_api_outage(log).expect("529 must classify");
    assert!(
        reason.to_ascii_lowercase().contains("api error: 5")
            || reason.to_ascii_lowercase().contains("overloaded"),
        "reason must echo the matched diagnostic, got: {reason}",
    );
}

/// The 5xx family beyond 529 — 500, 502, 503, 504 are all transient
// upstream classes. trace:BUG-266 | ai:claude
#[test]
fn api_error_5xx_family_is_outage() {
    for code in [500, 502, 503, 504, 520, 599] {
        let log = format!(
            "{{\"type\":\"assistant\",\"text\":\"API Error: {code} Service Unavailable\"}}"
        );
        assert!(
            claude_log_indicates_api_outage(&log).is_some(),
            "API Error: {code} must classify as outage",
        );
    }
}

/// The capitalized `Overloaded` keyword alone is sufficient — Anthropic's
// shorthand for capacity-shed responses. trace:BUG-266 | ai:claude
#[test]
fn overloaded_keyword_alone_is_outage() {
    let log = "{\"type\":\"system\",\"subtype\":\"error\",\"message\":\"Overloaded\"}";
    assert!(claude_log_indicates_api_outage(log).is_some());
}

/// Proxy / load-balancer connectivity errors from the model edge —
/// Envoy's `upstream connect error` is the canonical phrasing.
// trace:BUG-266 | ai:claude
#[test]
fn upstream_connect_error_is_outage() {
    let log = "{\"type\":\"error\",\"text\":\"upstream connect error or disconnect/\
                   reset before headers. reset reason: connection failure\"}";
    assert!(claude_log_indicates_api_outage(log).is_some());
}

/// SSE-stream disconnects — the connection dropped while the model was
/// mid-response. Indistinguishable from an outage from the client side.
// trace:BUG-266 | ai:claude
#[test]
fn stream_timeout_is_outage() {
    for line in [
        "{\"type\":\"error\",\"text\":\"stream timeout after 600s\"}",
        "{\"type\":\"error\",\"text\":\"stream disconnected unexpectedly\"}",
    ] {
        assert!(
            claude_log_indicates_api_outage(line).is_some(),
            "stream-error variant must classify: {line}",
        );
    }
}

/// Case-insensitivity — an Anthropic rephrasing that uppercases or
/// lowercases a phrase must not silently re-classify the error.
// trace:BUG-266 | ai:claude
#[test]
fn classification_is_case_insensitive() {
    assert!(claude_log_indicates_api_outage("API ERROR: 503 OVERLOADED").is_some());
    assert!(claude_log_indicates_api_outage("api error: 529 overloaded").is_some());
    assert!(claude_log_indicates_api_outage("Upstream Connect Error: ...").is_some());
}

/// A clean session log (no errors, just normal tool/assistant events)
/// must NOT classify as an outage. Empty log returns `None`.
// trace:BUG-266 | ai:claude
#[test]
fn clean_session_log_is_not_outage() {
    assert!(claude_log_indicates_api_outage("").is_none());
    let clean = "{\"type\":\"system\",\"subtype\":\"init\"}\n\
                     {\"type\":\"assistant\",\"text\":\"Reading the file.\"}\n\
                     {\"type\":\"tool_use\",\"name\":\"Read\"}\n\
                     {\"type\":\"result\",\"subtype\":\"success\"}\n";
    assert!(claude_log_indicates_api_outage(clean).is_none());
}

/// Non-outage failure modes — permission errors, parse errors, in-session
/// aborts — stay outside the outage classifier. The orchestrator should
// keep treating these as phase-1 failures. trace:BUG-266 | ai:claude
#[test]
fn non_outage_failures_are_not_outage() {
    for s in [
            "{\"type\":\"error\",\"text\":\"permission denied: bash blocked\"}",
            "{\"type\":\"error\",\"text\":\"HTTP 401 Unauthorized\"}",
            "{\"type\":\"error\",\"text\":\"HTTP 403 Forbidden\"}",
            "{\"type\":\"error\",\"text\":\"HTTP 429 rate limited\"}",
            "{\"type\":\"error\",\"text\":\"API Error: 400 Bad Request\"}",
            "{\"type\":\"error\",\"text\":\"invalid JSON in response\"}",
            "{\"type\":\"assistant\",\"text\":\"Note: status.claude.com listed a past 5xx incident yesterday.\"}",
        ] {
            // The trailing assistant note is the only marginal one — it
            // mentions `5xx` but not `API Error: 5` or `Overloaded`, so it
            // must not match. The 400 mentions `API Error:` but with `4`,
            // not `5`, so the 5xx anchor must reject it.
            assert!(
                claude_log_indicates_api_outage(s).is_none(),
                "expected NON-outage classification for: {s:?}",
            );
        }
}

/// The returned reason must stay short — the orchestrator's epilogue is
/// one line and a multi-KB assistant turn would smear the terminal.
// trace:BUG-266 | ai:claude
#[test]
fn reason_excerpt_is_bounded() {
    let mut long = String::from("preamble ");
    long.push_str(&"x".repeat(2000));
    long.push_str(" API Error: 503 Overloaded ");
    long.push_str(&"y".repeat(2000));
    let reason = claude_log_indicates_api_outage(&long).expect("must classify");
    assert!(
        reason.len() <= 200,
        "excerpt must be bounded (got {} chars)",
        reason.len(),
    );
    assert!(
        reason.to_ascii_lowercase().contains("api error: 5"),
        "excerpt must include the matched phrase, got: {reason}",
    );
}
