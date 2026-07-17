//! TASK-221: pin the pure parts of `aida dev activate`'s binary
//! selection. `pick_dev_binary_dir` itself touches the filesystem
//! and spawns subprocesses; this mod covers `parse_embedded_sha` and
//! the `ShaMatch::Exact` shortcut path of `classify_sha_match` which
//! is logic, not git. trace:TASK-221 | ai:claude
use super::*;
// The pure auto-select binary picker moved to `dev_cmd` (SPIKE-78).
use crate::dev_cmd::{
    auto_select_dev_profile, BinarySelectionReason, DevBuildCandidate, DevProfile,
};

#[test]
fn parse_embedded_sha_short_form() {
    let banner = "aida 0.5.2 (built 2026-05-13 23:00:00 PDT, sha 866b050)";
    assert_eq!(parse_embedded_sha(banner).as_deref(), Some("866b050"));
}

#[test]
fn parse_embedded_sha_with_dirty_marker() {
    let banner = "aida 0.5.2 (built 2026-05-13 23:00:00 PDT, sha 866b050+dirty)";
    // We only want the hex part — "+dirty" terminates extraction.
    assert_eq!(parse_embedded_sha(banner).as_deref(), Some("866b050"));
}

#[test]
fn parse_embedded_sha_full_form() {
    let banner = "aida 0.5.2 (built 2026-05-13, sha 866b050aabbccddeeff1122334455)";
    match parse_embedded_sha(banner) {
        Some(s) => assert!(s.starts_with("866b050"), "got: {s}"),
        None => panic!("expected Some"),
    }
}

#[test]
fn parse_embedded_sha_unknown() {
    let banner = "aida 0.5.2 (built 2026-05-13, sha unknown)";
    // 'unknown' starts with 'u' which isn't hex; the parser should
    // return None rather than a non-hex string. trace:TASK-221
    assert!(parse_embedded_sha(banner).is_none());
}

#[test]
fn parse_embedded_sha_missing_marker() {
    let banner = "aida 0.5.2 (built 2026-05-13)";
    assert!(parse_embedded_sha(banner).is_none());
}

#[test]
fn parse_embedded_sha_too_short() {
    // 6 hex chars is below the 7-char threshold (matches git's default
    // --short prefix length).
    let banner = "aida 0.5.2 (built 2026-05-13, sha abc123)";
    assert!(parse_embedded_sha(banner).is_none());
}

#[test]
fn classify_from_merge_base_exit_maps_purged_sha_to_unknown() {
    // BUG-702: a purged/unresolvable binary_sha makes `git merge-base
    // --is-ancestor` exit 128 (bad object). It must classify as Unknown —
    // no stale-binary nudge, and the raw git fatal is suppressed upstream —
    // NOT a misleading Unrelated (which implies "resolved, different
    // branch"). Exit 1 stays the clean "not an ancestor".
    assert_eq!(classify_from_merge_base_exit(Some(0)), ShaMatch::Ancestor);
    assert_eq!(classify_from_merge_base_exit(Some(1)), ShaMatch::Unrelated);
    assert_eq!(classify_from_merge_base_exit(Some(128)), ShaMatch::Unknown);
    assert_eq!(classify_from_merge_base_exit(None), ShaMatch::Unknown);
}

#[test]
fn classify_sha_exact_when_prefix() {
    // Binary stamped a 7-char prefix; HEAD is the full 40. Exact match.
    let m = classify_sha_match(
        std::path::Path::new("/nonexistent-repo-path"),
        "866b050",
        "866b050aabbccddeeff1122334455667788990011",
    );
    assert_eq!(m, ShaMatch::Exact);
}

#[test]
fn classify_sha_exact_case_insensitive() {
    let m = classify_sha_match(
        std::path::Path::new("/nonexistent-repo-path"),
        "866B050",
        "866b050aabbccddeeff1122334455667788990011",
    );
    assert_eq!(m, ShaMatch::Exact);
}

#[test]
fn classify_sha_unknown_when_git_unavailable() {
    // With a /nonexistent path, the SHAs don't prefix-match, and git
    // exec for `merge-base --is-ancestor` will fail (Err) → Unknown
    // (matches the documented graceful-fallback contract).
    let m = classify_sha_match(
        std::path::Path::new("/nonexistent-repo-path-xyz"),
        "deadbeef",
        "cafebabe1122334455667788990011",
    );
    // The classify implementation returns Unknown when git's process
    // fails to spawn, and Unrelated when git ran but returned non-zero.
    // On hosts where /nonexistent fails-to-spawn, we'd get Unknown;
    // on hosts where git starts but bails on the bad cwd, Unrelated.
    // Both are acceptable here — the point is "not Exact, not
    // Ancestor."
    assert!(
        matches!(m, ShaMatch::Unknown | ShaMatch::Unrelated),
        "got: {:?}",
        m
    );
}

// ---- BUG-665: stale-binary-after-pull warning predicate ----
//
// `pull_binary_is_stale` decides whether `aida pull` should nudge the user
// to `cargo build`. The contract: warn ONLY when dev-activated AND the
// built binary's SHA is a strict ancestor of HEAD (genuinely behind). No
// false alarm when it matches HEAD; no warning for a released binary.
// trace:BUG-665 | ai:claude

#[test]
fn pull_stale_warns_when_binary_behind_head() {
    // Dev-activated + binary SHA is an ancestor of HEAD → behind → warn.
    assert!(pull_binary_is_stale(true, ShaMatch::Ancestor));
}

#[test]
fn pull_stale_silent_when_binary_matches_head() {
    // Dev-activated but the binary already matches HEAD → no warning.
    assert!(!pull_binary_is_stale(true, ShaMatch::Exact));
}

#[test]
fn pull_stale_silent_when_not_dev_activated() {
    // A released binary on PATH is expected to differ from HEAD — even an
    // ancestor verdict must NOT warn when dev-activation is off.
    assert!(!pull_binary_is_stale(false, ShaMatch::Ancestor));
}

#[test]
fn pull_stale_silent_for_diverged_or_unknown() {
    // Diverged (different branch's build) / unknown (git unavailable) do
    // not get the "run cargo build to pick up the pull" nudge.
    assert!(!pull_binary_is_stale(true, ShaMatch::Unrelated));
    assert!(!pull_binary_is_stale(true, ShaMatch::Unknown));
}

// ---- BUG-643: pure auto-mode build selection ----
//
// `auto_select_dev_profile` is the freshest-wins picker the `auto` pin
// uses on every `aida dev activate`. These pin it: given two builds with
// {mtime, sha-verdict}, which profile is selected (and the reason chip).
// trace:BUG-643 | ai:claude

use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn cand(secs: u64, sha: ShaMatch) -> DevBuildCandidate {
    DevBuildCandidate {
        mtime: UNIX_EPOCH + Duration::from_secs(secs),
        sha,
    }
}

#[test]
fn auto_flips_to_newer_debug_exact_over_older_release_ancestor() {
    // The reported BUG-643 scenario: release is an ancestor of HEAD,
    // debug is freshly rebuilt at HEAD (exact) and newer. Auto must pick
    // debug — NOT stay sticky on the stale release.
    let (pick, reason) = auto_select_dev_profile(
        Some(cand(100, ShaMatch::Ancestor)),
        Some(cand(200, ShaMatch::Exact)),
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Debug);
    assert_eq!(reason, BinarySelectionReason::ShaExactMatch);
}

#[test]
fn auto_breaks_same_sha_class_tie_by_freshest_not_release() {
    // Both builds are exact matches; debug is newer. The pre-fix logic
    // always tie-broke to release (sticky). Post-fix: freshest wins.
    let (pick, _) = auto_select_dev_profile(
        Some(cand(100, ShaMatch::Exact)),
        Some(cand(200, ShaMatch::Exact)),
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Debug);

    // Symmetric: release newer among two exacts → release.
    let (pick, _) = auto_select_dev_profile(
        Some(cand(200, ShaMatch::Exact)),
        Some(cand(100, ShaMatch::Exact)),
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Release);
}

#[test]
fn auto_prefers_exact_even_when_alternate_is_newer_but_weaker() {
    // Active release is exact; debug is NEWER by mtime but only an
    // ancestor. The stronger SHA match wins — this is the case where the
    // old "Re-run to flip" advice was false, so auto must keep release.
    let (pick, reason) = auto_select_dev_profile(
        Some(cand(100, ShaMatch::Exact)),
        Some(cand(999, ShaMatch::Ancestor)),
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Release);
    assert_eq!(reason, BinarySelectionReason::ShaExactMatch);
}

#[test]
fn auto_recency_fallback_when_neither_matches() {
    // Neither build's SHA is recognizable on HEAD → freshest mtime wins,
    // reason is the recency fallback (preserves pre-TASK-221 behavior).
    let (pick, reason) = auto_select_dev_profile(
        Some(cand(100, ShaMatch::Unrelated)),
        Some(cand(200, ShaMatch::Unknown)),
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Debug);
    assert_eq!(reason, BinarySelectionReason::RecencyFallback);
}

#[test]
fn auto_exact_mtime_tie_falls_to_release() {
    // Identical mtime + identical SHA class → stable release default.
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (pick, _) = auto_select_dev_profile(
        Some(cand(t, ShaMatch::Exact)),
        Some(cand(t, ShaMatch::Exact)),
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Release);
}

#[test]
fn auto_only_one_build_present() {
    let (pick, reason) = auto_select_dev_profile(None, Some(cand(100, ShaMatch::Unknown))).unwrap();
    assert_eq!(pick, DevProfile::Debug);
    assert_eq!(reason, BinarySelectionReason::OnlyOne);

    let (pick, reason) = auto_select_dev_profile(Some(cand(100, ShaMatch::Unknown)), None).unwrap();
    assert_eq!(pick, DevProfile::Release);
    assert_eq!(reason, BinarySelectionReason::OnlyOne);
}

#[test]
fn auto_no_build_present_is_none() {
    assert!(auto_select_dev_profile(None, None).is_none());
}
