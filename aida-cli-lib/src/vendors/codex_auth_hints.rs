//! Data and diagnostic rendering for non-renewing ChatGPT launch credentials.
// trace:BUG-2486 | ai:codex
pub(crate) const HOME_ENV: &str = "CODEX_HOME";
pub(crate) const HOME_DIR: &str = ".codex";
pub(crate) const REPORT_FIELD: &str = "codex_token_lifetime";
pub(crate) const MIN_GRANT_KEY: &str = "codex_min_grant_secs";
pub(crate) const MIN_GRANT_INVALID: &str =
    "contained.codex_min_grant_secs must be a positive integer";

pub(crate) fn floor_refusal(exp: i64, left: i64, floor: i64) -> String {
    format!("ChatGPT token expiry {exp} (Unix seconds), remaining {left}s: usable grant below {floor}s floor after 300s margin; run `codex login` on the host, then relaunch")
}
pub(crate) fn launch_lifetime(requested: i64, effective: i64, deadline: i64, exp: i64) -> String {
    format!("contained Codex ChatGPT grant: requested {requested}s, effective {effective}s, deadline {deadline} (Unix seconds), token expiry {exp} (300s margin)")
}
pub(crate) fn token_lifetime(exp: i64, left: i64) -> String {
    format!("ChatGPT token: {left}s remaining; expiry {exp} (Unix seconds); run `codex login` on the host to renew")
}
pub(crate) fn token_unavailable(error: &dyn std::fmt::Display) -> String {
    format!("ChatGPT token: unavailable ({error}); run `codex login` on the host")
}
