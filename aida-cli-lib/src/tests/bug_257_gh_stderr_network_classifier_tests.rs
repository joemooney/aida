use super::*;

/// Real-world stderr from the BUG-257 origin incident — exactly what the
// orchestrator saw. Must classify as network. trace:BUG-257 | ai:claude
#[test]
fn observed_origin_incident_stderr_is_network() {
    let stderr = "error connecting to api.github.com\n\
                      check your internet connection or https://githubstatus.com";
    assert!(gh_stderr_is_network_error(stderr));
}

/// gh's own diagnostic suffix is the most stable signal — its presence
/// alone is sufficient even when the surrounding message changes.
// trace:BUG-257 | ai:claude
#[test]
fn githubstatus_pointer_alone_is_network() {
    assert!(gh_stderr_is_network_error(
        "something went wrong — see https://githubstatus.com for status"
    ));
}

/// Go `net` and `crypto/tls` error families that gh wraps verbatim.
// trace:BUG-257 | ai:claude
#[test]
fn dial_dns_tls_error_families_are_network() {
    for s in [
        "dial tcp 140.82.112.5:443: i/o timeout",
        "no such host",
        "could not resolve host: api.github.com",
        "Temporary failure in name resolution",
        "connection refused",
        "connection reset by peer",
        "connection timed out",
        "network is unreachable",
        "no route to host",
        "tls handshake timeout",
        "tls: handshake failure",
        "request canceled while waiting for connection",
    ] {
        assert!(
            gh_stderr_is_network_error(s),
            "expected network classification for: {s:?}"
        );
    }
}

/// Case-insensitivity — a gh upgrade that capitalizes a phrase must
/// not silently re-classify the error as auth/parse.
// trace:BUG-257 | ai:claude
#[test]
fn classification_is_case_insensitive() {
    assert!(gh_stderr_is_network_error(
        "Error Connecting To Api.GitHub.Com — check status"
    ));
    assert!(gh_stderr_is_network_error("DIAL TCP: I/O TIMEOUT"));
}

/// Auth, parse, and miscellaneous gh failures stay `GhFailed` — they
/// are not transient and a different recovery hint applies.
// trace:BUG-257 | ai:claude
#[test]
fn non_network_failures_are_not_network() {
    for s in [
        "",
        "gh exited 1",
        "HTTP 401: Bad credentials",
        "HTTP 403: API rate limit exceeded",
        "HTTP 404: Not Found",
        "could not find any commits between ...",
        "no remotes configured for this repository",
        "could not parse gh output: \"garbled\"",
        "permission denied: missing scopes",
    ] {
        assert!(
            !gh_stderr_is_network_error(s),
            "expected NON-network classification for: {s:?}"
        );
    }
}
