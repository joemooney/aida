use super::*;
use crate::statusline_cmd::{
    derive_session_branch_suffix, sess_anchor_annotation, sess_label_with_suffix,
    wt_divergence_segment,
};

/// TASK-244: matching shell + session role — no warning, plain
/// `role:X` segment (the common case, behavior unchanged).
#[test]
fn role_segment_matching_state_is_unchanged() {
    let (text, mismatch) = role_segment_text("implementer", Some("implementer"), true);
    assert_eq!(text, "role:implementer");
    assert!(!mismatch);
    // Case-insensitive — `Implementer` vs `implementer` is a match.
    let (_, mismatch) = role_segment_text("Implementer", Some("implementer"), true);
    assert!(!mismatch);
}

/// STORY-718: the integrator seat renders in the statusline exactly like the
/// other agent-wired roles — a plain `role:integrator` segment with no
/// mismatch glyph when shell + session agree.
#[test]
fn role_segment_renders_integrator_seat() {
    let (text, mismatch) = role_segment_text("integrator", Some("integrator"), true);
    assert_eq!(text, "role:integrator");
    assert!(!mismatch);
    // A roleless statusline (no session role) still names the seat.
    let (text, mismatch) = role_segment_text("integrator", None, true);
    assert_eq!(text, "role:integrator");
    assert!(!mismatch);
}

/// TASK-244: shell role disagrees with the active session's role —
/// both surfaced with the warning glyph.
#[test]
fn role_segment_mismatch_surfaces_both() {
    let (text, mismatch) = role_segment_text("implementer", Some("reviewer"), true);
    assert!(mismatch);
    assert!(text.contains("role:implementer"), "got: {}", text);
    assert!(text.contains("session:reviewer"), "got: {}", text);
    assert!(
        text.contains(crate::glyph(crate::glyphs::Glyph::Warning)),
        "got: {}",
        text
    );
}

/// TASK-244: no active session → no mismatch, plain segment.
#[test]
fn role_segment_no_active_session() {
    let (text, mismatch) = role_segment_text("implementer", None, true);
    assert_eq!(text, "role:implementer");
    assert!(!mismatch);
}

/// BUG-519: `general-purpose` is the harness's generic fallback agent_type,
/// not a real AIDA role — a deliberately-started general-purpose session
/// must NOT get the warn glyph. The segment reads as the plain `role:X`.
#[test]
fn role_segment_general_purpose_session_is_not_warned() {
    let (text, mismatch) = role_segment_text("implementer", Some("general-purpose"), true);
    assert_eq!(text, "role:implementer");
    assert!(!mismatch);
    // Case-insensitive — `General-Purpose` is still the generic fallback.
    let (_, mismatch) = role_segment_text("implementer", Some("General-Purpose"), true);
    assert!(!mismatch);
}

/// TASK-244: the `[statusline] role_mismatch_warning = false` knob
/// suppresses the warning even when the roles disagree.
#[test]
fn role_segment_warning_disabled_suppresses_mismatch() {
    let (text, mismatch) = role_segment_text("implementer", Some("reviewer"), false);
    assert_eq!(text, "role:implementer");
    assert!(!mismatch);
}

/// TASK-244: `role_mismatch_warning` defaults to on, parses an
/// explicit `false`, and ignores the key outside `[statusline]`.
#[test]
fn statusline_role_mismatch_config_parsing() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task244-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let aida = tmp.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();

    // No config file → default true.
    assert!(statusline_role_mismatch_enabled(&tmp));

    // Explicit false in [statusline] → false.
    std::fs::write(
        aida.join("config.toml"),
        "[statusline]\nrole_mismatch_warning = false  # quiet\n",
    )
    .unwrap();
    assert!(!statusline_role_mismatch_enabled(&tmp));

    // The key under a different section is ignored → default true.
    std::fs::write(
        aida.join("config.toml"),
        "[behavior]\nrole_mismatch_warning = false\n",
    )
    .unwrap();
    assert!(statusline_role_mismatch_enabled(&tmp));

    let _ = std::fs::remove_dir_all(&tmp);
}

// Cache-stored full SHA matches `--short` output. trace:TASK-1-045
#[test]
fn sha_prefix_match_full_vs_short() {
    let full = "4e39de29ddd72417772aa14b552937018d270746";
    let short = "4e39de2";
    assert!(sha_prefix_match(full, short));
    assert!(sha_prefix_match(short, full));
}

#[test]
fn sha_prefix_match_different_shas() {
    assert!(!sha_prefix_match("4e39de2", "deadbee"));
    assert!(!sha_prefix_match("4e39de29ddd724", "4e3a000000"));
}

#[test]
fn sha_prefix_match_case_insensitive() {
    assert!(sha_prefix_match("4E39DE2", "4e39de29ddd724"));
}

#[test]
fn sha_prefix_match_empty_strings() {
    assert!(sha_prefix_match("", ""));
    assert!(!sha_prefix_match("", "abc"));
    assert!(!sha_prefix_match("abc", ""));
}

// ── classify_cache_freshness ──
// STORY-78: pure decision function for cache:fresh|stale|behind|?.
// Tri-state `local_behind_origin`: Some(true)=behind, Some(false)=
// equal-or-ahead, None=can't tell. trace:STORY-78 | ai:claude

/// Cache matches local AND fetch is recent AND we're not behind → Fresh.
#[test]
fn freshness_all_aligned_is_fresh() {
    let r = classify_cache_freshness(Some("abc1234"), "abc1234", Some(false), Some(60), 300);
    assert_eq!(r, CacheFreshness::Fresh);
}

/// Cache SHA != local → Stale. Wins over every other axis because
/// the next read will pay the rebuild cost regardless of remote.
#[test]
fn freshness_cache_mismatch_is_stale_even_when_origin_fresh() {
    let r = classify_cache_freshness(Some("aaaaaaa"), "bbbbbbb", Some(false), Some(60), 300);
    assert_eq!(r, CacheFreshness::Stale);
}

/// recorded_sha=None counts as stale (cache hasn't recorded HEAD yet
/// or schema row is missing).
#[test]
fn freshness_missing_cache_sha_is_stale() {
    let r = classify_cache_freshness(None, "abc1234", Some(false), Some(60), 300);
    assert_eq!(r, CacheFreshness::Stale);
}

/// Cache matches local, fetch is recent, local lags origin → Behind.
#[test]
fn freshness_local_lags_origin_is_behind() {
    let r = classify_cache_freshness(Some("aaaaaaa"), "aaaaaaa", Some(true), Some(60), 300);
    assert_eq!(r, CacheFreshness::Behind);
}

/// Cache matches local, fetch is recent, direction unknown → Unknown
/// (we don't render false-fresh).
#[test]
fn freshness_direction_unknown_is_unknown() {
    let r = classify_cache_freshness(Some("aaaaaaa"), "aaaaaaa", None, Some(60), 300);
    assert_eq!(r, CacheFreshness::Unknown);
}

/// Local strictly ahead of origin → Fresh (no pull needed; cache:behind
/// would be misleading since there's nothing to pull). The user may
/// still need to push, but that's a different signal.
// trace:STORY-78 | ai:claude
#[test]
fn freshness_local_ahead_of_origin_is_fresh() {
    let r = classify_cache_freshness(Some("aaaaaaa"), "aaaaaaa", Some(false), Some(60), 300);
    assert_eq!(r, CacheFreshness::Fresh);
}

/// Cache matches local, fetch timestamp absent → Unknown. This is the
/// dominant state until STORY-79 starts writing last-fetch.toml.
#[test]
fn freshness_no_last_fetch_is_unknown() {
    let r = classify_cache_freshness(Some("aaaaaaa"), "aaaaaaa", Some(false), None, 300);
    assert_eq!(r, CacheFreshness::Unknown);
}

/// Cache matches local, fetch is OLDER than threshold → Unknown.
/// Equal-to-threshold counts as fresh (<= boundary).
#[test]
fn freshness_stale_fetch_is_unknown() {
    let r = classify_cache_freshness(Some("aaaaaaa"), "aaaaaaa", Some(false), Some(301), 300);
    assert_eq!(r, CacheFreshness::Unknown);
}

/// Empty local SHA (no orphan store attached) → NoStore, distinct
/// from Unknown (transiently-unknown origin freshness on an attached
/// store). NoStore renders nothing so a store-less worktree doesn't
// show a misleading `cache:?`. trace:BUG-518 | ai:claude
#[test]
fn freshness_no_local_store_is_no_store() {
    let r = classify_cache_freshness(Some("aaaaaaa"), "", Some(true), Some(60), 300);
    assert_eq!(r, CacheFreshness::NoStore);
}

/// Label mapping: Fresh + NoStore suppressed (render nothing), others
// render the documented strings. trace:BUG-518 | ai:claude
#[test]
fn freshness_label_mapping() {
    assert_eq!(CacheFreshness::Fresh.label(), None);
    assert_eq!(CacheFreshness::Stale.label(), Some("stale"));
    assert_eq!(CacheFreshness::Behind.label(), Some("behind"));
    assert_eq!(CacheFreshness::Unknown.label(), Some("?"));
    assert_eq!(CacheFreshness::NoStore.label(), None);
}

// ── derive_session_branch_suffix ──
// TASK-60: trace:TASK-60 | ai:claude

/// branch == slugified scope → no suffix (the common case where
/// the user picked the obvious branch name).
#[test]
fn sess_suffix_matches_scope_slug_returns_empty() {
    assert_eq!(derive_session_branch_suffix("EPIC-20", "epic-20"), "");
    assert_eq!(derive_session_branch_suffix("PR-6", "pr-6"), "");
}

/// branch starts with `<slug>-` → suffix is `#<rest>` (the common
/// epic-N-batchM pattern).
#[test]
fn sess_suffix_batched_branch() {
    assert_eq!(
        derive_session_branch_suffix("EPIC-20", "epic-20-batch7"),
        "#batch7"
    );
    assert_eq!(
        derive_session_branch_suffix("PR-6", "pr-6-review"),
        "#review"
    );
}

/// Free-form branch (no scope-slug prefix) → suffix is `@<branch>`.
#[test]
fn sess_suffix_freeform_branch() {
    assert_eq!(
        derive_session_branch_suffix("EPIC-22", "feature-cross-project"),
        "@feature-cross-project"
    );
}

/// Empty scope-slug (after slugification) → empty suffix.
/// Defensive: don't crash on pathological scope strings.
#[test]
fn sess_suffix_empty_scope_returns_empty() {
    assert_eq!(derive_session_branch_suffix("", "anything"), "");
}

// ── sess_label_with_suffix ──

/// No suffix → just truncated scope.
#[test]
fn sess_label_no_suffix() {
    assert_eq!(sess_label_with_suffix("EPIC-20", "", 20), "EPIC-20");
}

/// Scope + suffix fit within budget → concatenated.
#[test]
fn sess_label_fits() {
    assert_eq!(
        sess_label_with_suffix("EPIC-20", "#batch7", 20),
        "EPIC-20#batch7"
    );
}

/// Combined length overflow → scope gets truncated, suffix stays.
/// The new signal (batch info) is preserved at the cost of scope detail.
#[test]
fn sess_label_truncates_scope_keeps_suffix() {
    // budget 10, "#batch10" = 8 chars → scope budget 2
    let got = sess_label_with_suffix("EPIC-20-LONG", "#batch10", 10);
    assert!(got.ends_with("#batch10"), "{:?}", got);
}

/// Pathological: suffix alone overflows → just render scope-truncated.
#[test]
fn sess_label_pathological_long_suffix() {
    let got = sess_label_with_suffix("EPIC-20", "@really-long-branch-name-overflow", 10);
    // Suffix is dropped entirely; falls back to scope-only truncation.
    assert!(!got.contains("@really"));
}

// ── sess_anchor_annotation / wt_divergence_segment ──
// TASK-282: trace:TASK-282 | ai:claude

/// No-divergence default: `@<scope>` and the session anchor are the
/// same scope with no batch suffix → the `[sess:]` annotation is
/// suppressed (the redundancy this task removes).
#[test]
fn sess_anchor_hidden_when_redundant() {
    assert_eq!(sess_anchor_annotation("TASK-282", "TASK-282", ""), None);
}

/// Divergence — the role is touching a child spec while the session
/// is anchored elsewhere → the annotation names the anchor.
#[test]
fn sess_anchor_shown_on_scope_divergence() {
    assert_eq!(
        sess_anchor_annotation("TASK-280", "TASK-257", "").as_deref(),
        Some("[sess:TASK-257]")
    );
}

/// Same scope but a batch suffix is present → still shown, so TASK-60's
/// batch disambiguation (epic-20 batch3 vs batch7) isn't lost when the
/// role's @SPEC happens to equal the bare session scope.
#[test]
fn sess_anchor_shown_when_batch_suffix_present() {
    assert_eq!(
        sess_anchor_annotation("EPIC-20", "EPIC-20", "#batch7").as_deref(),
        Some("[sess:EPIC-20#batch7]")
    );
}

/// Auto-named worktree (`<repo>-<slug>`) carries the scope slug → no
/// `wt:` segment. Covers a plain repo name, a dashed repo name, and a
/// bare `<slug>` directory — all of which "match" the scope.
#[test]
fn wt_segment_hidden_for_auto_named_worktree() {
    assert_eq!(
        wt_divergence_segment(
            std::path::Path::new("/home/joe/ai/aida-task-257"),
            "task-257"
        ),
        None
    );
    assert_eq!(
        wt_divergence_segment(
            std::path::Path::new("/home/joe/ai/aida-web-react-task-257"),
            "task-257"
        ),
        None
    );
    assert_eq!(
        wt_divergence_segment(std::path::Path::new("/tmp/task-257"), "task-257"),
        None
    );
}

/// An explicit `--path` worktree whose name doesn't carry the slug →
/// `wt:<name>` renders so the divergence is visible (the compose case
/// from the spec: `@TASK-280 [sess:TASK-257] · wt:hot-fix`).
#[test]
fn wt_segment_shown_on_divergence() {
    assert_eq!(
        wt_divergence_segment(std::path::Path::new("/home/joe/ai/hot-fix"), "task-280").as_deref(),
        Some("wt:hot-fix")
    );
}

// ── classify_lease_state ──
// TASK-55: trace:TASK-55 | ai:claude

/// Worktree missing → Stale regardless of age / claude state.
#[test]
fn lease_missing_worktree_is_stale() {
    assert_eq!(classify_lease_state(false, false, 0), LeaseState::Stale);
    assert_eq!(classify_lease_state(false, true, 0), LeaseState::Stale);
    assert_eq!(classify_lease_state(false, false, 48), LeaseState::Stale);
}

/// Worktree present + live claude → Live (regardless of age).
#[test]
fn lease_with_live_claude_is_live() {
    assert_eq!(classify_lease_state(true, true, 0), LeaseState::Live);
    assert_eq!(classify_lease_state(true, true, 100), LeaseState::Live);
}

/// Worktree present, no live claude, fresh (<24h) → Dormant.
/// Could be a shell-only session or a paused claude.
#[test]
fn lease_fresh_dormant() {
    assert_eq!(classify_lease_state(true, false, 0), LeaseState::Dormant);
    assert_eq!(classify_lease_state(true, false, 23), LeaseState::Dormant);
}

/// Worktree present, no live claude, >=24h old → Stale.
/// The cutoff is inclusive of 24 — exactly 1 day = stale.
#[test]
fn lease_aged_dormant_becomes_stale() {
    assert_eq!(classify_lease_state(true, false, 24), LeaseState::Stale);
    assert_eq!(classify_lease_state(true, false, 25), LeaseState::Stale);
    assert_eq!(classify_lease_state(true, false, 1000), LeaseState::Stale);
}

/// State glyph / label mapping stays in sync with the rendering
/// contract documented in the section header.
#[test]
fn lease_state_renders() {
    assert_eq!(LeaseState::Live.glyph(), "●");
    assert_eq!(
        LeaseState::Dormant.glyph(),
        crate::glyph(crate::glyphs::Glyph::InFlight)
    );
    assert_eq!(
        LeaseState::Stale.glyph(),
        crate::glyph(crate::glyphs::Glyph::Warning)
    );
    assert_eq!(LeaseState::Live.label(), "live");
    assert_eq!(LeaseState::Dormant.label(), "dormant");
    assert_eq!(LeaseState::Stale.label(), "stale");
}

// ── local_lags_origin (fixture-backed) ──
// STORY-78: direction-aware comparison via git rev-list --count.
// trace:STORY-78 | ai:claude

/// Build a two-branch fixture: `main` at the same commit as a tracked
/// "origin/main" ref, then advance one side as the test requires.
fn fixture_local_origin(
    advance_local: u32,
    advance_origin: u32,
) -> (tempfile::TempDir, String, String) {
    use std::process::Command;
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let r = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&r.stderr)
        );
    };
    run(&["init", "--initial-branch=main", "--quiet"]);
    run(&["commit", "--allow-empty", "-m", "base", "--quiet"]);
    // Create a fake "origin/main" tracking ref by branching here.
    run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    for i in 0..advance_local {
        run(&[
            "commit",
            "--allow-empty",
            "-m",
            &format!("local{}", i),
            "--quiet",
        ]);
    }
    let local = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    // Advance the "origin" ref by committing while pointed there.
    if advance_origin > 0 {
        run(&[
            "checkout",
            "-q",
            "-b",
            "tmp-origin",
            "refs/remotes/origin/main",
        ]);
        for i in 0..advance_origin {
            run(&[
                "commit",
                "--allow-empty",
                "-m",
                &format!("origin{}", i),
                "--quiet",
            ]);
        }
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&["checkout", "-q", "main"]);
    }
    let origin = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["rev-parse", "--short", "refs/remotes/origin/main"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    (tmp, local, origin)
}

/// local == origin → Some(false) (caught by SHA prefix-match shortcut).
#[test]
fn lags_equal_shas_returns_false() {
    let (tmp, sha, _) = fixture_local_origin(0, 0);
    let r = local_lags_origin(tmp.path(), &sha, &sha);
    assert_eq!(r, Some(false));
}

/// origin has 2 commits local doesn't → Some(true).
#[test]
fn lags_local_behind_returns_true() {
    let (tmp, local, origin) = fixture_local_origin(0, 2);
    assert_ne!(local, origin);
    let r = local_lags_origin(tmp.path(), &local, &origin);
    assert_eq!(r, Some(true));
}

/// local has 3 commits origin doesn't → Some(false) (strictly ahead).
#[test]
fn lags_local_ahead_returns_false() {
    let (tmp, local, origin) = fixture_local_origin(3, 0);
    assert_ne!(local, origin);
    let r = local_lags_origin(tmp.path(), &local, &origin);
    assert_eq!(r, Some(false));
}

/// Both sides advanced (diverged) → Some(true) because origin has
/// commits we don't, regardless of our own ahead-ness.
#[test]
fn lags_diverged_returns_true() {
    let (tmp, local, origin) = fixture_local_origin(2, 1);
    let r = local_lags_origin(tmp.path(), &local, &origin);
    assert_eq!(r, Some(true));
}

/// Unknown SHA → None (rev-list errors, we collapse to Unknown
/// freshness rather than guessing).
#[test]
fn lags_unknown_sha_returns_none() {
    let (tmp, local, _) = fixture_local_origin(0, 0);
    let r = local_lags_origin(tmp.path(), &local, "deadbee");
    assert_eq!(r, None);
}

// ── should_spawn_bg_fetch ──
// STORY-79: pure decision function for the background fetch spawner.
// trace:STORY-79 | ai:claude

/// No prior fetch and no live lock → spawn.
#[test]
fn bg_spawn_cold_start() {
    assert!(should_spawn_bg_fetch(None, None, 300));
}

/// Recent successful fetch → skip.
#[test]
fn bg_spawn_skips_when_fetch_fresh() {
    assert!(!should_spawn_bg_fetch(Some(60), None, 300));
}

/// Fetch older than interval → spawn (assuming no live lock).
#[test]
fn bg_spawn_after_interval() {
    assert!(should_spawn_bg_fetch(Some(301), None, 300));
}

/// Live lockfile (another shell mid-fetch) → skip even when stale.
#[test]
fn bg_spawn_yields_to_live_lock() {
    assert!(!should_spawn_bg_fetch(Some(301), Some(10), 300));
}

/// Stale lockfile (>3× interval, presumed dead worker) → spawn.
#[test]
fn bg_spawn_overrides_stale_lock() {
    // 3 * 300 = 900; lock age 1000 > 900 → considered dead.
    assert!(should_spawn_bg_fetch(Some(301), Some(1000), 300));
}

/// Equal-to-interval is still "fresh" (strict less-than boundary).
/// Tests the boundary so future refactors don't silently flip it.
#[test]
fn bg_spawn_fresh_boundary_is_strict_less_than() {
    assert!(!should_spawn_bg_fetch(Some(299), None, 300));
    assert!(should_spawn_bg_fetch(Some(300), None, 300));
}

// ── bg_fetch_enabled (env-driven) ──
// STORY-79: explicit disable values must turn the feature off.
// trace:STORY-79 | ai:claude

/// Run a closure with `AIDA_BG_FETCH` set to `val`, restoring the
/// previous value afterwards. Some tests run in parallel inside
/// the same process; we serialize via a static mutex so they
/// don't trample each other's env.
fn with_bg_fetch_env<R>(val: Option<&str>, f: impl FnOnce() -> R) -> R {
    // BUG-697: shared process-global env lock (was a module-local mutex).
    let _guard = crate::test_env::env_lock();
    let prev = std::env::var("AIDA_BG_FETCH").ok();
    match val {
        Some(v) => std::env::set_var("AIDA_BG_FETCH", v),
        None => std::env::remove_var("AIDA_BG_FETCH"),
    }
    let result = f();
    match prev {
        Some(v) => std::env::set_var("AIDA_BG_FETCH", v),
        None => std::env::remove_var("AIDA_BG_FETCH"),
    }
    result
}

#[test]
fn bg_enabled_default_is_on() {
    let r = with_bg_fetch_env(None, bg_fetch_enabled);
    assert!(r);
}

#[test]
fn bg_enabled_false_disables() {
    for v in ["false", "FALSE", "0", "no", "off", "  off  "] {
        let r = with_bg_fetch_env(Some(v), bg_fetch_enabled);
        assert!(!r, "value {:?} should disable", v);
    }
}

#[test]
fn bg_enabled_truthy_keeps_on() {
    for v in ["true", "1", "yes", ""] {
        let r = with_bg_fetch_env(Some(v), bg_fetch_enabled);
        assert!(r, "value {:?} should leave enabled", v);
    }
}

// ── bg_fetch_lock_path ──
// STORY-79: lockfile naming. trace:STORY-79 | ai:claude

/// Two stores under different project roots get different lockfiles
/// (collision would deadlock both projects' fetchers).
#[test]
fn bg_lock_path_is_per_project() {
    let a = tempfile::TempDir::new().unwrap();
    let b = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(a.path().join(".aida-store")).unwrap();
    std::fs::create_dir_all(b.path().join(".aida-store")).unwrap();
    let pa = bg_fetch_lock_path(&a.path().join(".aida-store")).unwrap();
    let pb = bg_fetch_lock_path(&b.path().join(".aida-store")).unwrap();
    assert_ne!(pa, pb);
}

/// Same store yields the same lockfile path across calls — lock
/// coordination depends on this stability.
#[test]
fn bg_lock_path_is_stable() {
    let a = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(a.path().join(".aida-store")).unwrap();
    let p1 = bg_fetch_lock_path(&a.path().join(".aida-store")).unwrap();
    let p2 = bg_fetch_lock_path(&a.path().join(".aida-store")).unwrap();
    assert_eq!(p1, p2);
}

// ── handle_bg_fetch_command (worker, end-to-end) ──
// STORY-79: full fixture round-trip — set up a real git project,
// run the worker, verify last-fetch.toml ends up with the expected
// result string. Skipped under restricted sandboxes that block
// `git fetch` on file:// remotes (rare; CI rolls everything).
// trace:STORY-79 | ai:claude

// TASK-32 introduced a local AIDA_HOME RAII guard; TASK-521 rewrites
// its callsite to use the shared `crate::test_env::EnvVarGuard`, which
// serialises process-global env-var swaps under a single mutex so
// sibling tests reading `AIDA_HOME` can't race. trace:TASK-521 trace:TASK-32 | ai:claude

/// Worker writes `result = "error: ..."` to last-fetch.toml when the
/// store has no `origin` remote configured. Exercises the failure
/// path without needing network. Uses an isolated AIDA_HOME so the
/// test doesn't clobber the real ~/.aida/cache/last-fetch.toml.
// trace:TASK-32 | ai:claude — Windows-capable now that bg_worker
/// routes home-dir lookups through `aida_home_dir()`.
#[test]
fn bg_worker_records_error_on_missing_remote() {
    use std::process::Command;
    let tmp = tempfile::TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&fake_home).unwrap();
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let store = project.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    let run = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&store)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
    };
    run(&["init", "--initial-branch=aida-store", "--quiet"]);
    run(&["commit", "--allow-empty", "-m", "base", "--quiet"]);
    // No `origin` remote configured → fetch fails.

    // Redirect AIDA_HOME so write_last_fetch_entry lands under our
    // temp dir — works on all platforms (HOME alone can't isolate
    // on Windows because dirs uses SHGetKnownFolderPath there).
    // trace:TASK-32 | ai:claude
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &fake_home);
    let result = crate::session_misc_cmd::handle_bg_fetch_command(&store);
    drop(_home_guard);
    assert!(result.is_ok());

    let toml_path = fake_home.join(".aida/cache/last-fetch.toml");
    assert!(
        toml_path.exists(),
        "expected {} to exist",
        toml_path.display()
    );
    let raw = std::fs::read_to_string(&toml_path).unwrap();
    // Expected: error result keyed by the canonical project path.
    assert!(raw.contains("result"), "{}", raw);
    assert!(raw.contains("error"), "expected error result, got: {}", raw);

    // Lockfile must be cleaned up on worker exit (drop guard).
    let lock_path = bg_fetch_lock_path(&store).unwrap();
    assert!(
        !lock_path.exists(),
        "lockfile {} should be removed",
        lock_path.display()
    );
}

/// STORY-66: the auto-queue hook parses `aida add`'s spec_id out of a
/// line that may be wrapped in colored() output. Drop SGR sequences
/// without depending on a regex / ansi crate.
// trace:STORY-66 | ai:claude
#[test]
fn strip_ansi_color_basic() {
    assert_eq!(strip_ansi_color("plain"), "plain");
    assert_eq!(strip_ansi_color("\x1b[32mSTORY-77\x1b[0m"), "STORY-77");
    assert_eq!(
        strip_ansi_color("\x1b[1;32mbold green\x1b[0m and tail"),
        "bold green and tail"
    );
    // Lone ESC without `[` survives — we only strip well-formed SGR.
    assert_eq!(strip_ansi_color("a\x1bb"), "a\x1bb");
}

/// STORY-60: byte-count formatter — boundary cases at KB/MB/GB
/// crossover. Below 1KB renders as bytes; otherwise one decimal.
// trace:STORY-60 | ai:claude
#[test]
fn humanize_size_buckets() {
    assert_eq!(humanize_size(0), "0 B");
    assert_eq!(humanize_size(512), "512 B");
    assert_eq!(humanize_size(1024), "1.0 KB");
    assert_eq!(humanize_size(1536), "1.5 KB");
    assert_eq!(humanize_size(1024 * 1024), "1.0 MB");
    assert_eq!(humanize_size(1024 * 1024 * 3 / 2), "1.5 MB");
    assert_eq!(humanize_size(1024 * 1024 * 1024), "1.0 GB");
}

// ---- TASK-817: arrow-key role picker option labels ----

fn picker_row(
    marker: &str,
    name: &str,
    global: bool,
    recency: &str,
    purpose: Option<&str>,
) -> super::RolePickerRow {
    super::RolePickerRow {
        marker: marker.to_string(),
        name: name.to_string(),
        global,
        recency: recency.to_string(),
        purpose: purpose.map(|p| p.to_string()),
    }
}

/// TASK-817: a role's option label carries the marker, name, scope tag,
/// recency, and (truncated) purpose, all on a single line so
/// `inquire::Select` renders one row per role.
#[test]
fn role_picker_option_shows_name_scope_recency_and_purpose() {
    use super::format_role_picker_option;
    let row = picker_row(
        crate::glyph(crate::glyphs::Glyph::Arrow),
        "advisor",
        true,
        "3h ago",
        Some("Routes work and gardens the queue."),
    );
    let label = format_role_picker_option(&row, 80);
    assert!(
        label.starts_with(&format!(
            "{} advisor",
            crate::glyph(crate::glyphs::Glyph::Arrow)
        )),
        "label was {label:?}"
    );
    assert!(label.contains("[global]"), "label was {label:?}");
    assert!(label.contains("· 3h ago"), "label was {label:?}");
    assert!(
        label.contains("Routes work and gardens the queue."),
        "label was {label:?}"
    );
    // Single line — no embedded newlines.
    assert!(!label.contains('\n'), "label must be one line: {label:?}");
}

/// TASK-817: a role with no purpose renders just the name/scope/recency
/// header with no trailing separator.
#[test]
fn role_picker_option_no_purpose_has_no_separator() {
    use super::format_role_picker_option;
    let with_none =
        format_role_picker_option(&picker_row("*", "advisor", false, "5m ago", None), 80);
    assert_eq!(with_none, "* advisor · 5m ago");
    // A whitespace-only purpose is treated as absent.
    let blank = format_role_picker_option(
        &picker_row(" ", "implementer", false, "2d ago", Some("   ")),
        80,
    );
    assert_eq!(blank, "  implementer · 2d ago");
}

/// TASK-817: the whole option label stays within the terminal width — a
/// long purpose is truncated (ellipsis) rather than wrapping the row.
#[test]
fn role_picker_option_truncates_to_width() {
    use super::format_role_picker_option;
    let long = "word ".repeat(200);
    let width = 50;
    let label = format_role_picker_option(
        &picker_row(" ", "advisor", false, "now", Some(&long)),
        width,
    );
    assert!(
        label.chars().count() <= width,
        "label exceeds width {width}: {label:?} ({} cols)",
        label.chars().count()
    );
    assert!(
        label.contains('…'),
        "a truncated purpose should end with an ellipsis: {label:?}"
    );
}

/// TASK-817: when the header (name+scope+recency) already nearly fills the
/// width, the purpose is dropped rather than overflowing the row.
#[test]
fn role_picker_option_drops_purpose_when_no_room() {
    use super::format_role_picker_option;
    // Width just past the header so <8 cols remain for the purpose.
    let row = picker_row(" ", "advisor", false, "now", Some("Routes work."));
    let header_only = "  advisor · now";
    let width = header_only.chars().count() + 4;
    let label = format_role_picker_option(&row, width);
    assert_eq!(label, header_only, "purpose should be dropped: {label:?}");
}

/// TASK-645: the read-side role default. Unset/blank → implementer
/// (flagged as default); any explicit value passes through canonicalized
/// BUG-460: advisor authority is granted by advisor role, a TTY, OR a
/// live-orchestrator-corroborated op — but a bare non-advisor headless agent
/// (no TTY, not orchestrated) is still gated.
#[test]
fn advisor_authority_grants_orchestrated_ops_but_gates_bare_agents() {
    use super::advisor_authority_from;
    // bare headless implementer/reviewer: no authority
    assert!(!advisor_authority_from("implementer", false, false));
    assert!(!advisor_authority_from("reviewer", false, false));
    // the three authority sources
    assert!(advisor_authority_from("advisor", false, false)); // advisor role
    assert!(advisor_authority_from("implementer", true, false)); // interactive TTY
    assert!(advisor_authority_from("implementer", false, true)); // under a live orchestrator
    assert!(advisor_authority_from("reviewer", false, true)); // orchestrated reviewer phase
}

/// BUG-498: the advisor-seat hint fires only when the resolved role is
/// advisor (advisor work) AND the persistent seat was never established
/// (`AIDA_SESSION_PROJECT` unset — i.e. advisor came from a one-off
/// `AIDA_SESSION_ROLE=advisor` prefix, not `aida role enter advisor`).
#[test]
fn advisor_seat_hint_fires_only_for_unseated_advisor() {
    use super::advisor_seat_hint_warranted;
    // advisor via prefix, no seat → hint
    assert!(advisor_seat_hint_warranted("advisor", false));
    // advisor seated (role enter exported AIDA_SESSION_PROJECT) → no hint
    assert!(!advisor_seat_hint_warranted("advisor", true));
    // non-advisor roles never warrant the advisor-seat hint
    assert!(!advisor_seat_hint_warranted("implementer", false));
    assert!(!advisor_seat_hint_warranted("reviewer", false));
    assert!(!advisor_seat_hint_warranted("implementer", true));
}

/// TASK-130: a Spike is born human-only (research is human-driven) unless
/// `--no-human-only` opts it back into auto-pickup; every other type
/// defaults to NOT human-only; `--human-only` flips any type on. The flags
/// always win over the per-type default.
#[test]
fn resolve_human_only_spike_defaults_human_only_override_respected() {
    use super::resolve_human_only;
    use aida_core::RequirementType;

    // Spike → human-only by default (no flags).
    assert!(resolve_human_only(&RequirementType::Spike, false, false));
    // Spike + --no-human-only → opted back into auto-pickup.
    assert!(!resolve_human_only(&RequirementType::Spike, false, true));
    // Spike + --human-only → still human-only (flag agrees with default).
    assert!(resolve_human_only(&RequirementType::Spike, true, false));

    // Non-Spike types default to NOT human-only.
    assert!(!resolve_human_only(&RequirementType::Task, false, false));
    assert!(!resolve_human_only(&RequirementType::Bug, false, false));
    assert!(!resolve_human_only(&RequirementType::Story, false, false));
    assert!(!resolve_human_only(
        &RequirementType::Functional,
        false,
        false
    ));

    // --human-only flips any type on regardless of its default.
    assert!(resolve_human_only(&RequirementType::Task, true, false));
    // --no-human-only is a no-op for a type that already defaults off.
    assert!(!resolve_human_only(&RequirementType::Task, false, true));
}

/// and unflagged. `dialog` canonicalizes to `advisor` (TASK-586).
#[test]
fn effective_role_defaults_to_implementer_when_unset() {
    use super::resolve_effective_role;
    assert_eq!(
        resolve_effective_role(None),
        ("implementer".to_string(), true)
    );
    assert_eq!(
        resolve_effective_role(Some("")),
        ("implementer".to_string(), true)
    );
    assert_eq!(
        resolve_effective_role(Some("   ")),
        ("implementer".to_string(), true)
    );
    assert_eq!(
        resolve_effective_role(Some("advisor")),
        ("advisor".to_string(), false)
    );
    assert_eq!(
        resolve_effective_role(Some("  reviewer  ")),
        ("reviewer".to_string(), false)
    );
    // explicit implementer is NOT flagged as the implicit default
    assert_eq!(
        resolve_effective_role(Some("implementer")),
        ("implementer".to_string(), false)
    );
    // dialog → advisor canonicalization still applies
    assert_eq!(
        resolve_effective_role(Some("dialog")),
        ("advisor".to_string(), false)
    );
}

/// TASK-647 (ADR-3): the intake gate's status predicate. Approved and
/// beyond require advisor authority to produce; Draft / Rejected /
/// NeedsAttention don't (they aren't approved pipeline work).
#[test]
fn status_authority_gate_covers_approved_and_beyond() {
    use super::status_requires_advisor_authority;
    use aida_core::models::RequirementStatus as S;
    assert!(status_requires_advisor_authority(&S::Approved));
    assert!(status_requires_advisor_authority(&S::Planned));
    assert!(status_requires_advisor_authority(&S::InProgress));
    assert!(status_requires_advisor_authority(&S::Done));
    assert!(status_requires_advisor_authority(&S::Completed));
    assert!(!status_requires_advisor_authority(&S::Draft));
    assert!(!status_requires_advisor_authority(&S::Rejected));
    assert!(!status_requires_advisor_authority(&S::NeedsAttention));
}

/// BUG-482: `aida edit` gated only a `Draft` source, so a non-advisor could
/// self-re-approve a punted (`NeedsAttention`) spec — bypassing the triage
/// the punt requests. The (source, target) predicate now gates BOTH `Draft`
/// and `NeedsAttention` sources into the approved+ pipeline, while leaving
/// in-pipeline execution flips ungated.
// trace:BUG-482 | ai:claude
#[test]
fn status_advance_authority_gate_covers_draft_and_needs_attention_sources() {
    use super::status_advance_requires_advisor_authority as gate;
    use aida_core::models::RequirementStatus as S;

    // NeedsAttention → Approved is the headline hole: re-approving a punted
    // spec is now an advisor-authority act.
    assert!(gate(&S::NeedsAttention, &S::Approved));
    assert!(gate(&S::NeedsAttention, &S::Planned));
    assert!(gate(&S::NeedsAttention, &S::InProgress));
    // Draft → approved+ stays gated (the original TASK-647 behaviour).
    assert!(gate(&S::Draft, &S::Approved));
    assert!(gate(&S::Draft, &S::InProgress));

    // In-pipeline execution flips (source already past intake) are NOT
    // gated — drains and implementers move freely.
    assert!(!gate(&S::Approved, &S::InProgress));
    assert!(!gate(&S::InProgress, &S::Done));
    assert!(!gate(&S::Planned, &S::InProgress));

    // Triage to a non-pipeline outcome is not an authority act either.
    assert!(!gate(&S::NeedsAttention, &S::Rejected));
    assert!(!gate(&S::Draft, &S::Rejected));
    // NeedsAttention → InProgress would be gated (above); but the
    // design-fork ENTRY (InProgress → NeedsAttention) is not — NeedsAttention
    // is not a pipeline target.
    assert!(!gate(&S::InProgress, &S::NeedsAttention));
}

// trace:TASK-761 | ai:codex
#[test]
fn approval_type_gate_blocks_non_execution_classes() {
    use super::approval_forbidden_for_type as blocked;
    use aida_core::models::RequirementType as T;

    for req_type in [
        T::Vision,
        T::Epic,
        T::Principle,
        T::Constraint,
        T::Decision,
        T::Term,
    ] {
        assert!(blocked(&req_type), "{req_type} should not be approvable");
    }

    for req_type in [
        T::Functional,
        T::NonFunctional,
        T::System,
        T::User,
        T::ChangeRequest,
        T::Bug,
        T::Story,
        T::Task,
        T::Spike,
        T::Sprint,
        T::Folder,
        T::Meta,
        T::Doc,
    ] {
        assert!(!blocked(&req_type), "{req_type} should remain approvable");
    }
}

// A manual epic status edit is rejected (status is a read-only rollup), but
// `--force` recovers and non-epics are never gated by this rule.
// trace:BUG-626 | ai:claude
#[test]
fn manual_epic_status_edit_is_forbidden_without_force() {
    use super::manual_epic_status_edit_forbidden as forbidden;
    use aida_core::models::RequirementType as T;

    // An epic edit is rejected without --force, allowed with it.
    assert!(
        forbidden(&T::Epic, false),
        "epic status edit must be rejected"
    );
    assert!(!forbidden(&T::Epic, true), "--force must recover");

    // Every non-epic type is unaffected by this rule (with or without force).
    for req_type in [
        T::Functional,
        T::NonFunctional,
        T::System,
        T::User,
        T::ChangeRequest,
        T::Bug,
        T::Story,
        T::Task,
        T::Spike,
        T::Sprint,
        T::Folder,
        T::Meta,
        T::Doc,
        T::Vision,
        T::Principle,
        T::Constraint,
        T::Decision,
        T::Term,
    ] {
        assert!(
            !forbidden(&req_type, false),
            "{req_type} must not be gated by the epic-rollup rule"
        );
    }
}

/// TASK-739: exhaustive parity — `status_requires_advisor_authority` (now
/// delegating to `lifecycle::target_requires_advisor_authority`) matches the
// pre-migration hand-coded predicate over every status. trace:TASK-739
#[test]
fn status_requires_advisor_authority_parity_with_oracle() {
    use super::status_requires_advisor_authority as f;
    use aida_core::models::RequirementStatus as S;
    fn oracle(s: &S) -> bool {
        matches!(
            s,
            S::Approved | S::Planned | S::InProgress | S::Done | S::Completed
        )
    }
    let all = [
        S::Draft,
        S::Approved,
        S::Planned,
        S::InProgress,
        S::Done,
        S::Completed,
        S::Rejected,
        S::NeedsAttention,
    ];
    for s in &all {
        assert_eq!(f(s), oracle(s), "parity mismatch at {s}");
    }
}

/// TASK-739: exhaustive parity — `status_advance_requires_advisor_authority`
/// (now delegating to `lifecycle::transition_guard`) matches the
/// pre-migration predicate over every (from, to) pair, including direct
/// edits the model does not declare (e.g. `Draft → InProgress`).
// trace:TASK-739
#[test]
fn status_advance_requires_advisor_authority_parity_with_oracle() {
    use super::status_advance_requires_advisor_authority as gate;
    use aida_core::models::RequirementStatus as S;
    fn oracle(from: &S, to: &S) -> bool {
        matches!(from, S::Draft | S::NeedsAttention)
            && matches!(
                to,
                S::Approved | S::Planned | S::InProgress | S::Done | S::Completed
            )
    }
    let all = [
        S::Draft,
        S::Approved,
        S::Planned,
        S::InProgress,
        S::Done,
        S::Completed,
        S::Rejected,
        S::NeedsAttention,
    ];
    for from in &all {
        for to in &all {
            assert_eq!(
                gate(from, to),
                oracle(from, to),
                "parity mismatch at {from} → {to}"
            );
        }
    }
}

/// BUG-444: the keystone phase-1 decision. When both PR lookups return
/// empty-but-successful, ONLY a branch genuinely absent from origin is a
/// definitive NoPr; an on-origin (or unconfirmable) branch is a retryable
/// eventual-consistency window. Regression guard for the dominant
/// drain-failure cause (false NoPr racing GitHub's index).
#[test]
fn empty_phase1_lookup_only_definitive_when_branch_absent() {
    use super::{empty_phase1_lookup_is_definitive_nopr, BranchOriginProbe};
    assert!(
        empty_phase1_lookup_is_definitive_nopr(&BranchOriginProbe::Absent),
        "absent branch (never pushed) → definitive NoPr"
    );
    assert!(
        !empty_phase1_lookup_is_definitive_nopr(&BranchOriginProbe::Present),
        "on-origin branch → retry (PR likely exists but not indexed yet)"
    );
    assert!(
        !empty_phase1_lookup_is_definitive_nopr(&BranchOriginProbe::LsRemoteFailed),
        "unconfirmable → retry rather than false NoPr"
    );
}

/// BUG-64: terminal-status predicate. Completed and Rejected are
/// terminal; everything else (including the STORY-86 `Done` state) is
// open and accepts new children. trace:BUG-64 STORY-86 | ai:claude
#[test]
fn is_terminal_status_buckets() {
    assert!(is_terminal_status(&RequirementStatus::Completed));
    assert!(is_terminal_status(&RequirementStatus::Rejected));
    assert!(!is_terminal_status(&RequirementStatus::Draft));
    assert!(!is_terminal_status(&RequirementStatus::Approved));
    assert!(!is_terminal_status(&RequirementStatus::Planned));
    assert!(!is_terminal_status(&RequirementStatus::InProgress));
    // STORY-86: `Done` is "finished on a branch", not terminal. It
    // auto-bumps to Completed when the referencing commit merges to
    // the default branch. Children are still allowed.
    assert!(!is_terminal_status(&RequirementStatus::Done));
}

/// STORY-72: position math for `queue move --after`. Three regimes —
/// gapped (typical), adjacent (collision fallback), bottom (no
// successor). trace:STORY-72 | ai:claude
#[test]
fn position_after_picks_midpoint_when_gapped() {
    // Typical case: anchor + successor with the standard 1000 gap.
    assert_eq!(position_after(0, Some(1000)), 500);
    // Wider gap still midpoints.
    assert_eq!(position_after(2000, Some(4000)), 3000);
    // Anchor at bottom — no successor — uses the +1000 step that
    // matches the existing `--bottom` convention.
    assert_eq!(position_after(7000, None), 8000);
    // Adjacent positions: midpoint would land on the anchor (collision).
    // Fall through to +1 even though it risks colliding with the next
    // entry — the situation only arises in pathologically dense queues.
    assert_eq!(position_after(5, Some(6)), 6);
    assert_eq!(position_after(5, Some(5)), 6);
    // Negative anchor (queue items moved to top via `--top` use
    // negative positions) still produces a sortable midpoint.
    assert_eq!(position_after(-1000, Some(0)), -500);
    // Saturating arithmetic: a pre-fix corrupt queue where every
    // entry has `position: i64::MAX` (see git_backend's queue_add
    // sentinel resolution) must not overflow. The result clamps to
    // i64::MAX rather than wrapping. The user-visible result is a
    // no-op move, which is fine — better than a panic.
    assert_eq!(position_after(i64::MAX, None), i64::MAX);
    assert_eq!(position_after(i64::MAX, Some(i64::MAX)), i64::MAX);
    assert_eq!(position_after(i64::MAX - 1, None), i64::MAX);
}

/// TASK-280: `aida queue move X --to N` absolute positioning. The
/// helper drops the moved id, then re-inserts it at the clamped
/// 1-indexed slot; the caller renumbers the returned order. Cover
/// front / middle / back and the out-of-range clamp on both ends.
// trace:TASK-280 | ai:claude
#[test]
fn move_to_absolute_position_places_at_requested_slot() {
    let a = uuid::Uuid::from_u128(1);
    let b = uuid::Uuid::from_u128(2);
    let c = uuid::Uuid::from_u128(3);
    let d = uuid::Uuid::from_u128(4);
    let queue = [a, b, c, d];

    // Move C to the front (--to 1 / --to-front equivalent).
    assert_eq!(
        move_to_absolute_position(&queue, c, 1),
        (vec![c, a, b, d], 1)
    );
    // Move A to the third slot — the moved item is excluded before
    // the index is applied, so slot 3 lands it between C and D.
    assert_eq!(
        move_to_absolute_position(&queue, a, 3),
        (vec![b, c, a, d], 3)
    );
    // Move A to the last slot (--to 4 / --to-back equivalent).
    assert_eq!(
        move_to_absolute_position(&queue, a, 4),
        (vec![b, c, d, a], 4)
    );
    // Out-of-range high (--to 99 on a 4-item queue) clamps to the
    // last slot rather than erroring.
    assert_eq!(
        move_to_absolute_position(&queue, a, 99),
        (vec![b, c, d, a], 4)
    );
    // Out-of-range low (--to 0) clamps to the front.
    assert_eq!(
        move_to_absolute_position(&queue, d, 0),
        (vec![d, a, b, c], 1)
    );
    // Single-item queue: any N is a no-op landing at slot 1.
    assert_eq!(move_to_absolute_position(&[a], a, 5), (vec![a], 1));
}

/// TASK-280: `aida queue move` accepts the absolute-positioning forms
/// — the `--to-front` / `--to-back` aliases and `--to <N>` — alongside
/// the existing relative flags, and `--to` conflicts with them.
// trace:TASK-280 | ai:claude
#[test]
fn queue_move_absolute_flags_parse() {
    // --to-front is a visible alias of --top.
    let cli = Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--to-front"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Queue(QueueCommand::Move { top: true, .. })
    ));
    // --to-back is a visible alias of --bottom.
    let cli = Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--to-back"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Queue(QueueCommand::Move { bottom: true, .. })
    ));
    // --to <N> parses an absolute slot.
    let cli = Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--to", "3"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Queue(QueueCommand::Move { to: Some(3), .. })
    ));
    // Existing relative flags still parse (no breaking change).
    let cli =
        Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--before", "TASK-2"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Queue(QueueCommand::Move {
            before: Some(_),
            ..
        })
    ));
    // --to conflicts with the end / relative flags.
    assert!(
        Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--to", "2", "--to-front",])
            .is_err()
    );
    assert!(Cli::try_parse_from([
        "aida", "queue", "move", "TASK-1", "--to", "2", "--before", "TASK-2",
    ])
    .is_err());
}

/// BUG-249: `aida queue move` exposes `--force` for the bypass path
/// (move a terminal-status entry that lingers in the queue file).
/// Default is `force: false`; the long flag flips it.
// trace:BUG-249 | ai:claude
#[test]
fn queue_move_force_flag_parses() {
    let cli = Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--top"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Queue(QueueCommand::Move { force: false, .. })
    ));
    let cli = Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--top", "--force"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Queue(QueueCommand::Move { force: true, .. })
    ));
}

/// BUG-249: pre-fix, `aida queue move <id>` printed a `Moved` check line even
/// when `<id>` wasn't in the queue at all — queue_reorder's update
/// loop simply didn't match anything and the write completed with
/// no entries changed. The classifier distinguishes that case from
/// "in queue but terminal status" and demands `--force` for the
/// latter.
// trace:BUG-249 | ai:claude
#[test]
fn queue_move_target_not_in_queue_errors() {
    let target = uuid::Uuid::now_v7();
    let err =
        classify_queue_move_target(target, "TASK-999", &RequirementStatus::Approved, &[], false)
            .expect_err("move on absent spec must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not in the queue"),
        "error must name the absent state: {msg}"
    );
    // The display id should appear so the user sees what was looked up.
    assert!(msg.contains("TASK-999"), "error must echo the id: {msg}");
    // Should NOT recommend --force — that's the wrong tool for this case.
    assert!(
        !msg.contains("--force"),
        "absent-spec error must not suggest --force: {msg}"
    );
}

/// BUG-249: a Completed (terminal) entry still appears in the
/// queue YAML file but is hidden from the default `queue list`
/// view. `move` on it errors with a `--force` hint instead of
/// silently succeeding.
// trace:BUG-249 | ai:claude
#[test]
fn queue_move_target_terminal_errors_with_force_hint() {
    let target = uuid::Uuid::now_v7();
    let entry = aida_core::QueueEntry {
        user_id: "tester".into(),
        requirement_id: target,
        position: 1000,
        added_by: "tester".into(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: None,
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    let err = classify_queue_move_target(
        target,
        "TASK-998",
        &RequirementStatus::Completed,
        std::slice::from_ref(&entry),
        false,
    )
    .expect_err("move on Completed spec must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("--force"),
        "terminal-status error must hint --force: {msg}"
    );
    assert!(
        msg.contains("terminal") || msg.contains("Completed"),
        "terminal-status error must name the state: {msg}"
    );
    // With --force the same target passes through.
    classify_queue_move_target(
        target,
        "TASK-998",
        &RequirementStatus::Completed,
        &[entry],
        true,
    )
    .expect("--force must bypass the terminal-status guard");
}

/// BUG-249: sanity check the happy path — an in-queue non-terminal
/// entry passes the classifier so the handler proceeds to the
/// path-specific reorder logic.
// trace:BUG-249 | ai:claude
#[test]
fn queue_move_target_ok_for_in_queue_non_terminal() {
    let target = uuid::Uuid::now_v7();
    let entry = aida_core::QueueEntry {
        user_id: "tester".into(),
        requirement_id: target,
        position: 1000,
        added_by: "tester".into(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: None,
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    classify_queue_move_target(
        target,
        "TASK-997",
        &RequirementStatus::Planned,
        &[entry],
        false,
    )
    .expect("in-queue non-terminal target should pass the guard");
}

/// TASK-491: `--to-top` is an additional visible alias of `--top` so
/// the operator can use the spelling the task spec calls out without
/// having to remember `--to-front`. The existing `--to-front` alias
/// still parses too.
// trace:TASK-491 | ai:claude
#[test]
fn queue_move_to_top_alias_parses() {
    let cli = Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--to-top"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Queue(QueueCommand::Move { top: true, .. })
    ));
    // Conflict with --to <N> still holds for the new alias.
    assert!(
        Cli::try_parse_from(["aida", "queue", "move", "TASK-1", "--to-top", "--to", "2",]).is_err()
    );
}

/// TASK-280/TASK-491: regression for the shared visible ordering in the
/// spec's worked example — add 5 items in order, move the position-5 item
/// to absolute slot 1, queue ends up as `[5, 1, 2, 3, 4]`. This exercises
/// the `--to 1` path; `--top` uses separate position arithmetic that should
/// be tested independently if it needs coverage.
// trace:TASK-491 TASK-501 | ai:claude
#[test]
fn queue_move_to_absolute_slot_promotes_last_item_to_head() {
    let ids: Vec<uuid::Uuid> = (1..=5).map(uuid::Uuid::from_u128).collect();
    let (order, slot) = move_to_absolute_position(&ids, ids[4], 1);
    assert_eq!(slot, 1);
    assert_eq!(order, vec![ids[4], ids[0], ids[1], ids[2], ids[3]]);
}

/// STORY-60: age formatter for the prune candidate list. Resolution
/// drops as we cross day/week/month/year boundaries; sub-day uses
/// hours since `--days 30` makes anything finer than that
/// uninteresting (and 0d would be confusing).
// trace:STORY-60 | ai:claude
#[test]
fn humanize_age_secs_buckets() {
    assert_eq!(humanize_age_secs(3600), "1h");
    assert_eq!(humanize_age_secs(86_399), "23h");
    assert_eq!(humanize_age_secs(86_400), "1d");
    assert_eq!(humanize_age_secs(7 * 86_400 - 1), "6d");
    assert_eq!(humanize_age_secs(7 * 86_400), "1w");
    assert_eq!(humanize_age_secs(30 * 86_400), "1mo");
    assert_eq!(humanize_age_secs(365 * 86_400), "1y");
}

/// STORY-66: parse spec_id out of `aida add` stdout. Cover both backend
/// output shapes — git-canonical (`Added: ID - title`) and legacy
/// (`ID: spec_id`) — plus colored output and trailing `Hint:` noise.
// trace:STORY-66 | ai:claude
#[test]
fn parse_spec_id_from_add_output_handles_known_shapes() {
    // Git-canonical default.
    assert_eq!(
        parse_spec_id_from_add_output(
            "Added: STORY-82 - Test STORY-66 auto-queue helper\nHint: link it via …\n"
        ),
        Some("STORY-82".to_string())
    );
    // Legacy YAML/SQLite path.
    assert_eq!(
        parse_spec_id_from_add_output(
            "Requirement added successfully!\nUUID: 019e1300-…\nID: \x1b[32mSTORY-77\x1b[0m\n"
        ),
        Some("STORY-77".to_string())
    );
    // Color-wrapped git-canonical.
    assert_eq!(
        parse_spec_id_from_add_output("Added: \x1b[1;32mSTORY-99\x1b[0m - hello\n"),
        Some("STORY-99".to_string())
    );
    // Output with no recognizable line.
    assert_eq!(parse_spec_id_from_add_output("something unrelated\n"), None);
}

/// BUG-72: the auto-queue outcome shape must distinguish Filed,
/// AlreadyExists, and the various Skipped reasons so `session end`
/// can render the right glyph and the user knows why the reviewer
/// queue did or didn't grow an entry. Smoke-checks the constructors
/// since the real flow needs a live gh + filesystem to exercise.
// trace:BUG-72 | ai:claude
#[test]
fn auto_queue_outcome_constructors_tag_status_correctly() {
    let filed = AutoQueueOutcome::filed("filed STORY-99 (covers FR-1) → reviewer queue (PR #7)");
    assert!(matches!(filed.status, AutoQueueStatus::Filed));
    assert!(filed.summary.starts_with("filed STORY-99"));

    let dup = AutoQueueOutcome::already_exists(
        "PR #7 already has a `Review PR-7` story queued — skipping",
    );
    assert!(matches!(dup.status, AutoQueueStatus::AlreadyExists));

    // TASK-74: skip reasons now split into ByDesign (nothing to fix —
    // session shape never produces a PR) and NeedsAttention (missing
    // tool, gh failed, queue subprocess errored). Cover each phrasing
    // tagged with the right variant.
    for phrase in [
        "auto-queue: reviewer session on `pr-7` — no PR to file (skip by design)",
        "auto-queue: no open PR for branch `epic-20-batch7` — reviewer queue not filed",
    ] {
        let s = AutoQueueOutcome::skipped_by_design(phrase);
        assert!(matches!(s.status, AutoQueueStatus::SkippedByDesign));
        assert_eq!(s.summary, phrase);
    }
    for phrase in [
            "auto-queue: `gh` CLI not on PATH — would have queued reviewer story for branch `x`. Install gh to enable.",
            "auto-queue: `gh pr list` failed for branch `x` (HTTP 401) — no reviewer story filed",
            "auto-queue: `aida add` failed for PR #42 (see warning above)",
        ] {
            let s = AutoQueueOutcome::skipped_needs_attention(phrase);
            assert!(matches!(s.status, AutoQueueStatus::SkippedNeedsAttention));
            assert_eq!(s.summary, phrase);
        }
}

/// BUG-107: `aida session end` removes the session worktree, then runs
/// the auto-queue, which shells out to `gh`. If the auto-queue uses the
/// removed worktree as its cwd, every spawn fails with a misleading
/// ENOENT. `auto_queue_working_dir` must pick a directory that still
/// exists — preferring the lease's recorded parent project root.
// trace:BUG-107 | ai:claude
#[test]
fn auto_queue_working_dir_skips_removed_worktree() {
    let real = std::env::temp_dir(); // always exists
    let gone = real.join("aida-bug107-removed-worktree-does-not-exist");
    let gone2 = real.join("aida-bug107-parent-also-gone");
    assert!(
        !gone.exists(),
        "test precondition: phantom path must not exist"
    );

    // Worktree (invocation dir) removed → fall back to the parent.
    assert_eq!(
        auto_queue_working_dir(Some(real.as_path()), &gone),
        Some(real.clone())
    );
    // Parent recorded but itself gone → fall through to a live cwd.
    assert_eq!(
        auto_queue_working_dir(Some(gone2.as_path()), &real),
        Some(real.clone())
    );
    // No parent recorded (pre-STORY-58 lease) + live cwd → use the cwd.
    assert_eq!(auto_queue_working_dir(None, &real), Some(real.clone()));
    // Both gone → no safe directory; caller must skip, not spawn.
    assert_eq!(auto_queue_working_dir(Some(gone2.as_path()), &gone), None);
}

/// BUG-107: a spawn failure whose real cause is a removed working
/// directory must not read as "gh is missing" — `gh_spawn_error`
/// names the cwd as the culprit and absolves the binary.
// trace:BUG-107 | ai:claude
#[test]
fn gh_spawn_error_blames_the_missing_cwd_not_the_binary() {
    let gh = std::path::Path::new("/usr/bin/gh");
    let gone = std::env::temp_dir().join("aida-bug107-no-such-cwd");
    let enoent = std::io::Error::from(std::io::ErrorKind::NotFound);

    let removed = gh_spawn_error(gh, &gone, &enoent);
    assert!(removed.contains("no longer exists"));
    assert!(removed.contains("binary is fine"));

    // A live cwd → the plain message, no false "missing cwd" claim.
    let live = gh_spawn_error(gh, &std::env::temp_dir(), &enoent);
    assert!(!live.contains("no longer exists"));
}

/// BUG-108 / BUG-566: a worktree removed by `aida session end` leaves
/// any shell still inside it with a dangling cwd — `getcwd()` returns
/// ENOENT. The warning must fire for that `Err` and stay silent for a
/// healthy `Ok` cwd, so a genuinely-empty queue is never mislabeled.
/// BUG-566 additionally requires the message to surface the REAL
/// io::ErrorKind and stay cause-neutral (not assert the worktree case),
/// so a network-share / permission failure isn't misattributed.
// trace:BUG-108 trace:BUG-566 | ai:claude
#[test]
fn cwd_removed_warning_fires_only_for_a_dangling_cwd() {
    // Healthy cwd → no warning.
    let healthy: std::io::Result<std::path::PathBuf> = Ok(std::env::temp_dir());
    assert!(cwd_removed_warning(&healthy).is_none());

    // ENOENT branch (deleted dir / removed worktree): warning fires,
    // surfaces the real error kind, and lists the worktree cause WITHOUT
    // asserting it as the only one.
    let enoent: std::io::Result<std::path::PathBuf> =
        Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    let msg = cwd_removed_warning(&enoent).expect("warning for a deleted cwd");
    assert!(msg.contains("cannot read the current directory"));
    // The real io::ErrorKind text is surfaced (not discarded).
    assert!(msg.contains(&std::io::Error::from(std::io::ErrorKind::NotFound).to_string()));
    // Cause-neutral: lists worktree-removal as ONE possibility...
    assert!(msg.contains("session end"));
    // ...alongside the network-share / permission alternatives.
    assert!(msg.contains("network share"));
    assert!(msg.contains("permissions"));
    // Recovery hint stays, without hardcoding a `~/ai/<project>` layout.
    assert!(msg.contains("readable directory"));
    assert!(!msg.contains("~/ai/"));

    // Non-ENOENT branch (e.g. a dropped network share → PermissionDenied
    // standing in for ESTALE/EACCES, which aren't stable ErrorKinds):
    // the warning still fires and surfaces THIS kind, not a hardcoded one.
    let perm: std::io::Result<std::path::PathBuf> =
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let msg = cwd_removed_warning(&perm).expect("warning for an unreadable cwd");
    assert!(msg.contains("cannot read the current directory"));
    assert!(msg.contains(&std::io::Error::from(std::io::ErrorKind::PermissionDenied).to_string()));
}

/// TASK-74: branch-shape heuristic for "this is a reviewer session;
/// don't expect a PR from it". Covers the four branch prefixes the
/// `aida session start --owns PR-N` / `MR-N` etc. flows produce, and
/// rejects neighbors that just happen to start with the same letters.
// trace:TASK-74 | ai:claude
#[test]
fn is_review_session_branch_recognizes_review_branches() {
    // Positives
    for b in ["pr-7", "PR-7", "mr-12", "MR-1", "github-99", "gitlab-3"] {
        assert!(is_review_session_branch(b), "{}", b);
    }
    // Negatives — implementer-style branches must not match
    for b in [
        "epic-20-batch7",
        "main",
        "feature/foo",
        "pr-foo",
        "pr-",
        "pr",
        "fix-pr-7-handling",
        "release-1.2.3",
    ] {
        assert!(!is_review_session_branch(b), "{}", b);
    }
}

/// STORY-55: scope-fallback decision table for the `@<…>` segment.
/// Captures the four (latest-activity, active-lease) cases the
/// statusline distinguishes.
// trace:STORY-55 | ai:claude
#[test]
fn scope_fallback_decision_table() {
    use chrono::TimeZone;
    let lease_started = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 12, 0, 0).unwrap();
    let before_lease = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 11, 0, 0).unwrap();
    let after_lease = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 13, 0, 0).unwrap();

    let pick = |latest_at: Option<chrono::DateTime<chrono::Utc>>,
                lease: Option<chrono::DateTime<chrono::Utc>>,
                spec: &str,
                scope: &str|
     -> Option<String> {
        match (latest_at, lease) {
            (Some(at), Some(started_at)) if at >= started_at => Some(spec.to_string()),
            (_, Some(_)) => Some(scope.to_string()),
            (Some(_), None) => Some(spec.to_string()),
            (None, None) => None,
        }
    };

    // In-session activity wins over scope.
    assert_eq!(
        pick(
            Some(after_lease),
            Some(lease_started),
            "STORY-48",
            "EPIC-20"
        ),
        Some("STORY-48".into())
    );
    // Pre-session activity is shadowed by the scope.
    assert_eq!(
        pick(
            Some(before_lease),
            Some(lease_started),
            "STORY-54",
            "EPIC-20"
        ),
        Some("EPIC-20".into())
    );
    // Lease but no activity at all → scope.
    assert_eq!(
        pick(None, Some(lease_started), "", "EPIC-20"),
        Some("EPIC-20".into())
    );
    // No lease, only activity → spec (existing behavior).
    assert_eq!(
        pick(Some(after_lease), None, "STORY-48", ""),
        Some("STORY-48".into())
    );
    // No lease, no activity → nothing.
    assert_eq!(pick(None, None, "", ""), None);
}

/// STORY-55: long scopes (e.g. file-path scopes) are truncated to fit
/// the statusline budget, matching @SPEC's visual width.
// trace:STORY-55 | ai:claude
#[test]
fn scope_label_truncates_to_budget() {
    let long = "very-long-scope-name-that-overflows";
    let short = truncate(long, SCOPE_LABEL_MAX);
    assert!(short.chars().count() <= SCOPE_LABEL_MAX);
    assert!(short.ends_with('…'));

    let exact = "EPIC-20";
    assert_eq!(truncate(exact, SCOPE_LABEL_MAX), "EPIC-20");
}

/// STORY-48: enforcement-mode parsing is forgiving on capitalization
/// and whitespace, and unknown values fall through to `Warn`.
// trace:STORY-48 | ai:claude
#[test]
fn session_enforcement_parsing() {
    assert_eq!(
        SessionEnforcement::from_config_str("off"),
        SessionEnforcement::Off
    );
    assert_eq!(
        SessionEnforcement::from_config_str("OFF"),
        SessionEnforcement::Off
    );
    assert_eq!(
        SessionEnforcement::from_config_str("none"),
        SessionEnforcement::Off
    );
    assert_eq!(
        SessionEnforcement::from_config_str(" warn "),
        SessionEnforcement::Warn
    );
    assert_eq!(
        SessionEnforcement::from_config_str("Warn"),
        SessionEnforcement::Warn
    );
    assert_eq!(
        SessionEnforcement::from_config_str("block"),
        SessionEnforcement::Block
    );
    assert_eq!(
        SessionEnforcement::from_config_str("strict"),
        SessionEnforcement::Block
    );
    // Unknown → Warn (the safe default).
    assert_eq!(
        SessionEnforcement::from_config_str("xyzzy"),
        SessionEnforcement::Warn
    );
}

/// STORY-53: the `sess:<scope>` segment reuses SCOPE_LABEL_MAX, the
/// same budget that bounds @SPEC's width — long path-glob scopes get
/// the trailing ellipsis so the statusline stays scannable, and short
/// scopes pass through verbatim.
// trace:STORY-53 | ai:claude
#[test]
fn sess_segment_label_truncation() {
    let short = "EPIC-20";
    assert_eq!(truncate(short, SCOPE_LABEL_MAX), "EPIC-20");

    let long = "feature:auth-flow-rewrite";
    let out = truncate(long, SCOPE_LABEL_MAX);
    assert!(out.chars().count() <= SCOPE_LABEL_MAX);
    assert!(out.ends_with('…'));
}

/// STORY-56: appending session activity dedupes consecutive same-(role,
/// spec_id, action) writes by ticking the timestamp instead of stacking
/// duplicate entries — same shape as project-level role activity.
// trace:STORY-56 | ai:claude
#[test]
fn session_activity_dedupes_consecutive_repeats() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();

    let id = "test-session-01";
    append_session_activity(root, id, "implementer", "STORY-56", "edit").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    append_session_activity(root, id, "implementer", "STORY-56", "edit").unwrap();
    let log = load_session_activity(root, id);
    assert_eq!(log.entries.len(), 1, "consecutive same-action collapse");

    // A different action breaks the dedupe.
    append_session_activity(root, id, "implementer", "STORY-56", "show").unwrap();
    let log = load_session_activity(root, id);
    assert_eq!(log.entries.len(), 2);
    assert_eq!(log.entries[0].action, "show", "newest first");
}

/// BUG-65: dedupe is LRU-by-(role, spec_id, action), not just
/// consecutive — interleaved actions across specs still collapse
/// duplicates. Without this, a long agent run that revisits the same
/// spec produces an ever-growing log.
// trace:BUG-65 | ai:claude
#[test]
fn session_activity_dedupes_lru_across_interleaving() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    let id = "lru-session-01";

    // Sequence: show A → edit B → show A. The second show A must
    // remove the first one and land at the front (not append a
    // duplicate behind edit B).
    append_session_activity(root, id, "implementer", "STORY-A", "show").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    append_session_activity(root, id, "implementer", "STORY-B", "edit").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    append_session_activity(root, id, "implementer", "STORY-A", "show").unwrap();

    let log = load_session_activity(root, id);
    assert_eq!(log.entries.len(), 2, "duplicate show STORY-A collapsed");
    assert_eq!(log.entries[0].spec_id, "STORY-A", "newest first");
    assert_eq!(log.entries[0].action, "show");
    assert_eq!(log.entries[1].spec_id, "STORY-B");
}

/// STORY-70: convention-check predicate flags STORY/BUG with no
/// acceptance section, accepts STORY/BUG that has one (any of the
/// recognized headings), and ignores other types entirely. Pins the
/// scope of the lint so it doesn't grow over-eagerly.
// trace:STORY-70 | ai:claude
#[test]
fn requirement_missing_acceptance_scope() {
    use aida_core::{Requirement, RequirementType};

    // STORY without acceptance → flagged.
    let mut story = Requirement::new("S".into(), "Just a paragraph.".into());
    story.req_type = RequirementType::Story;
    assert!(requirement_missing_acceptance(&story));

    // STORY with `## Acceptance` → not flagged.
    let mut story_ok = Requirement::new("S".into(), "Intro.\n\n## Acceptance\n\n- alpha\n".into());
    story_ok.req_type = RequirementType::Story;
    assert!(!requirement_missing_acceptance(&story_ok));

    // BUG with `## Verify` (alias) → not flagged.
    let mut bug_ok = Requirement::new("B".into(), "Repro.\n\n## Verify\n\n- behaves\n".into());
    bug_ok.req_type = RequirementType::Bug;
    assert!(!requirement_missing_acceptance(&bug_ok));

    // BUG without acceptance → flagged.
    let mut bug = Requirement::new("B".into(), "Repro only.".into());
    bug.req_type = RequirementType::Bug;
    assert!(requirement_missing_acceptance(&bug));

    // EPIC / TASK / etc. are out of scope — even with no section,
    // they're never flagged.
    let mut epic = Requirement::new("E".into(), "No section.".into());
    epic.req_type = RequirementType::Epic;
    assert!(!requirement_missing_acceptance(&epic));
    let mut task = Requirement::new("T".into(), "No section.".into());
    task.req_type = RequirementType::Task;
    assert!(!requirement_missing_acceptance(&task));
}

/// TASK-680: the release-time doc-coverage selector reports a spec iff it
/// reached Completed at/after the cutoff AND no Doc references it. Pins:
/// window filtering by completion timestamp, the doc-reference exemption,
/// the modified_at fallback for history-less Completed specs, and that
/// Docs / archived specs are never themselves reported.
// trace:TASK-680 | ai:claude
#[test]
fn doc_coverage_selects_completed_since_tag_without_doc() {
    use aida_core::models::{
        FieldChange, HistoryEntry, Relationship, RelationshipType, RequirementStatus,
        RequirementType,
    };
    use aida_core::Requirement;
    use chrono::{Duration, Utc};

    let cutoff = Utc::now() - Duration::days(7);
    let after = cutoff + Duration::days(1);
    let before = cutoff - Duration::days(1);

    let status_completed = |ts| HistoryEntry {
        id: uuid::Uuid::new_v4(),
        author: "tester".into(),
        timestamp: ts,
        changes: vec![FieldChange {
            field_name: "status".into(),
            old_value: "Done".into(),
            new_value: RequirementStatus::Completed.to_string(),
        }],
    };

    // (1) Completed after the cutoff, no doc → REPORTED.
    let mut gap = Requirement::new("Shipped, undocumented".into(), String::new());
    gap.spec_id = Some("TASK-100".into());
    gap.req_type = RequirementType::Task;
    gap.status = RequirementStatus::Completed;
    gap.history.push(status_completed(after));

    // (2) Completed after the cutoff but documented → exempt.
    let mut documented = Requirement::new("Shipped, documented".into(), String::new());
    documented.spec_id = Some("TASK-101".into());
    documented.req_type = RequirementType::Task;
    documented.status = RequirementStatus::Completed;
    documented.history.push(status_completed(after));

    // (3) Completed BEFORE the cutoff → outside the window, not reported.
    let mut old = Requirement::new("Shipped last release".into(), String::new());
    old.spec_id = Some("TASK-102".into());
    old.req_type = RequirementType::Task;
    old.status = RequirementStatus::Completed;
    old.history.push(status_completed(before));

    // (4) Still in progress → never reported.
    let mut wip = Requirement::new("In flight".into(), String::new());
    wip.spec_id = Some("TASK-103".into());
    wip.req_type = RequirementType::Task;
    wip.status = RequirementStatus::InProgress;

    // (5) Currently Completed but no history row; modified_at after cutoff
    //     → reported via the fallback.
    let mut legacy = Requirement::new("Legacy completed".into(), String::new());
    legacy.spec_id = Some("TASK-104".into());
    legacy.req_type = RequirementType::Task;
    legacy.status = RequirementStatus::Completed;
    legacy.modified_at = after;

    // (6) The Doc itself — never reported even if Completed.
    let mut doc = Requirement::new("Doc about TASK-101".into(), String::new());
    doc.spec_id = Some("DOC-1".into());
    doc.req_type = RequirementType::Doc;
    doc.status = RequirementStatus::Completed;
    doc.modified_at = after;
    doc.relationships.push(Relationship {
        target_id: documented.id,
        rel_type: RelationshipType::References,
        created_at: Some(Utc::now()),
        created_by: None,
    });

    let all = vec![gap.clone(), documented, old, wip, legacy.clone(), doc];

    let reported = find_uncovered_completed_specs(&all, Some(cutoff));
    let ids: Vec<String> = reported.iter().map(|r| r.display_id()).collect();
    assert_eq!(
        ids,
        vec!["TASK-100".to_string(), "TASK-104".to_string()],
        "only the undocumented specs completed since the cutoff are reported"
    );

    // No cutoff → full-history scan also pulls in the pre-cutoff gap.
    let all_history = find_uncovered_completed_specs(&all, None);
    let all_ids: Vec<String> = all_history.iter().map(|r| r.display_id()).collect();
    assert!(all_ids.contains(&"TASK-102".to_string()));
    assert!(all_ids.contains(&"TASK-100".to_string()));
    assert!(all_ids.contains(&"TASK-104".to_string()));
    assert!(!all_ids.contains(&"DOC-1".to_string()));
    assert!(!all_ids.contains(&"TASK-101".to_string()));
}

/// BUG-65 acceptance: shipping 3 specs sequentially via a typical
/// implementer lifecycle (edit → done) leaves the activity log
/// pointing at the 3rd, not the 1st. This is the contract the
/// statusline @SPEC reads off of, so this test pins the regression
/// that motivated the bug.
// trace:BUG-65 | ai:claude
#[test]
fn session_activity_three_specs_lifecycle_points_at_last() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    let id = "lifecycle-01";

    for spec in ["STORY-1", "STORY-2", "STORY-3"] {
        append_session_activity(root, id, "implementer", spec, "edit").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
        append_session_activity(root, id, "implementer", spec, "done").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let log = load_session_activity(root, id);
    // Newest action is `done` on STORY-3.
    assert_eq!(log.entries[0].spec_id, "STORY-3");
    assert_eq!(log.entries[0].action, "done");
    // Each spec contributes 2 entries (edit, done) — total 6, no dups.
    assert_eq!(log.entries.len(), 6);
}

/// STORY-67: extract_acceptance_section finds `## Acceptance`,
/// `## Verify`, etc. (case-insensitive) and returns the body until
/// the next `## ` heading. Missing or empty sections return None
/// so the caller can render a placeholder rather than an empty
// section. trace:STORY-67 | ai:claude
#[test]
fn extract_acceptance_section_basic() {
    let desc =
        "Some intro paragraph.\n\n## Acceptance\n\n- alpha\n- bravo\n\n## Notes\n\nFollow-up.\n";
    let body = extract_acceptance_section(desc).unwrap();
    assert_eq!(body, "- alpha\n- bravo");
}

// trace:STORY-67 | ai:claude
#[test]
fn extract_acceptance_section_aliases() {
    for heading in &[
        "Acceptance",
        "Verify",
        "Test cases",
        "Tests",
        "Verification",
    ] {
        let desc = format!("blah\n\n## {}\n\nbody text\n", heading);
        assert!(
            extract_acceptance_section(&desc).is_some(),
            "missing recognition for `## {}`",
            heading
        );
    }
    // Non-matching headings fall through.
    let desc = "Body.\n\n## Why\n\nBecause.\n";
    assert!(extract_acceptance_section(desc).is_none());
}

// trace:STORY-67 | ai:claude
#[test]
fn extract_acceptance_section_empty_body() {
    let desc = "## Acceptance\n\n## Why\n\nReason.\n";
    // Empty body (just whitespace until the next heading) → None.
    assert!(extract_acceptance_section(desc).is_none());
}

/// TASK-265: card_lead_prose returns everything before the first
/// `## ` heading — the plain summary AIDA descriptions front-load.
// trace:TASK-265 | ai:claude
#[test]
fn card_lead_prose_stops_at_first_heading() {
    let desc = "Lead summary.\n\nSecond line.\n\n## Acceptance\n\n- a\n";
    assert_eq!(card_lead_prose(desc), "Lead summary.\n\nSecond line.");
    // No headings → whole description (trailing whitespace trimmed).
    assert_eq!(card_lead_prose("Just prose.\n"), "Just prose.");
    // A `## ` mid-line is not a heading and must not split.
    assert_eq!(
        card_lead_prose("see ## not a heading"),
        "see ## not a heading"
    );
}

/// TASK-265: card_truncate_paragraphs caps at N paragraphs / ~M chars
// without ever cutting inside a paragraph. trace:TASK-265 | ai:claude
#[test]
fn card_truncate_paragraphs_respects_boundaries() {
    let text = "Para one.\n\nPara two.\n\nPara three.\n\nPara four.";
    let (body, truncated) = card_truncate_paragraphs(text, 3, 500);
    assert_eq!(body, "Para one.\n\nPara two.\n\nPara three.");
    assert!(truncated, "fourth paragraph dropped");

    // Under both limits → nothing dropped.
    let (body, truncated) = card_truncate_paragraphs("Only one.", 3, 500);
    assert_eq!(body, "Only one.");
    assert!(!truncated);

    // Char budget stops accumulation, but never mid-paragraph.
    let long = "aaaa\n\nbbbbbbbbbb\n\ncccc";
    let (body, truncated) = card_truncate_paragraphs(long, 3, 8);
    assert_eq!(body, "aaaa", "second paragraph blows the 8-char budget");
    assert!(truncated);

    // The first paragraph is always kept even if it alone exceeds
    // the budget — the card never shows an empty description.
    let (body, _) = card_truncate_paragraphs("wayy-too-long-first-para", 3, 5);
    assert_eq!(body, "wayy-too-long-first-para");
}

/// TASK-265: card_count_acceptance counts `- [ ]` / `* [ ]` items.
// trace:TASK-265 | ai:claude
#[test]
fn card_count_acceptance_counts_checkboxes() {
    let body = "- [ ] one\n- [x] two\n  * [ ] nested three\nplain line\n- bullet not a box";
    assert_eq!(card_count_acceptance(body), 3);
    assert_eq!(card_count_acceptance(""), 0);
}

/// TASK-265: card_rel_label buckets a `Child` edge under "Parent"
/// (the edge reads "I am a child of the target") and everything else
// under "Related". trace:TASK-265 | ai:claude
#[test]
fn card_rel_label_buckets_relationships() {
    assert_eq!(card_rel_label(&RelationshipType::Child), "Parent");
    assert_eq!(card_rel_label(&RelationshipType::Parent), "Related");
    assert_eq!(card_rel_label(&RelationshipType::References), "Related");
    assert_eq!(
        card_rel_label(&RelationshipType::Custom("blocks".into())),
        "Related"
    );
}

/// STORY-67: spec ID detection inside a `(...)` group at end of
/// commit subject. Matches AIDA-format SPEC-IDs and rejects
/// anything else (e.g., issue refs, version strings).
// trace:STORY-67 | ai:claude
#[test]
fn extract_spec_ids_from_commit_subject() {
    let msg = "[AI:claude] feat(api): add endpoint (FR-1-042)\n\nBody text.\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["FR-1-042".to_string()]
    );
    let msg = "fix(scope): tweak (BUG-23)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-23".to_string()]
    );
    // Commits with no trailer leave nothing.
    let msg = "chore: bump dep version\n";
    assert!(extract_spec_ids_from_commit(msg).is_empty());
    // Version-like parens shouldn't match.
    let msg = "release: v1.2.3 (1.2.3)\n";
    assert!(extract_spec_ids_from_commit(msg).is_empty());

    // BUG-78: space-separated multi-spec parens.
    let msg = "[AI:claude] feat(session): polish (STORY-98 STORY-90 BUG-74) (#10)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec![
            "STORY-98".to_string(),
            "STORY-90".to_string(),
            "BUG-74".to_string()
        ]
    );

    // Comma-separated still works.
    let msg = "fix(api): (BUG-23, BUG-24)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-23".to_string(), "BUG-24".to_string()]
    );

    // Mixed comma + whitespace.
    let msg = "fix: (FR-1, BUG-2 TASK-3)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec![
            "FR-1".to_string(),
            "BUG-2".to_string(),
            "TASK-3".to_string()
        ]
    );

    // Mixed prose + ID still rejects (so release notes don't false-match).
    let msg = "release: stuff (foo BUG-23)\n";
    assert!(extract_spec_ids_from_commit(msg).is_empty());

    // BUG-85: body content that ends in `(SPEC-ID)` must NOT pollute the
    // delivered list. Only the subject's `(REQ-ID)` parens-suffix counts.
    let msg = "[AI:claude] fix(scope): description (BUG-83)\n\n\
            - aida role enter \"Queued for this role\" section (TASK-48)\n\
            - aida role enter \"Last touched\" section (TASK-49)\n\
            - aida session show --plan manifest table (STORY-98)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-83".to_string()]
    );

    // BUG-270: a leading `SPEC-ID:` prefix (web /ultraplan / hand-authored
    // squash shape) is recognized — STORY-439 was stranded at Approved
    // after PR-270 merged with exactly this subject.
    let msg = "STORY-439: three-way complexity calibration substrate (#270)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["STORY-439".to_string()]
    );
    // Node-aware id prefix works too.
    let msg = "BUG-1-099: fix the thing (#42)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-1-099".to_string()]
    );
    // Conventional-commit heads must NOT false-match as a colon-prefix id.
    for msg in [
        "fix: something broke\n",
        "feat(scope): add a thing\n",
        "[AI:claude] feat(api): add endpoint (FR-1-042)\n",
        "chore: bump dep version\n",
    ] {
        let got = extract_spec_ids_from_commit(msg);
        // The [AI:claude] one still delivers its paren id; the others none.
        assert!(
            got.is_empty() || got == vec!["FR-1-042".to_string()],
            "colon-prefix false-match on {:?}: {:?}",
            msg,
            got
        );
    }
    // Prefix + trailing paren naming the SAME id dedupes to one.
    let msg = "TASK-5: tidy (TASK-5)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["TASK-5".to_string()]
    );
}

// BUG-506: a squash subject carrying MULTIPLE separate `(SPEC-ID)`
// groups — `... (BUG-503) (BUG-504) (#794)` — must deliver EVERY id,
// not just the last group before the PR number. Both the `aida pull`
// auto-bump scan and `aida db reconcile-status` share this extractor,
// so the single-group assumption stranded all-but-one spec of every
// two-spec cluster PR at its pre-merge status.
// trace:BUG-506 | ai:claude
#[test]
fn extract_spec_ids_collects_all_paren_groups() {
    // Two separate groups + PR-number suffix → both ids, subject order.
    let msg = "fix: x (BUG-1) (BUG-2) (#99)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-1".to_string(), "BUG-2".to_string()]
    );
    // The real-world shape that stranded BUG-503 (PR #794 squash).
    let msg = "[AI:claude] fix(review): stale-base pre-flight + session \
            lease for aida review <spec> (BUG-503) (BUG-504) (#794)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-503".to_string(), "BUG-504".to_string()]
    );
    // Three groups, no PR suffix.
    let msg = "feat: y (TASK-1) (TASK-2) (TASK-3)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec![
            "TASK-1".to_string(),
            "TASK-2".to_string(),
            "TASK-3".to_string()
        ]
    );
    // Single-spec case still works.
    let msg = "fix(scope): tweak (BUG-23) (#7)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-23".to_string()]
    );
    // No-spec case still yields nothing.
    let msg = "chore: bump dep version (#12)\n";
    assert!(extract_spec_ids_from_commit(msg).is_empty());
    // A conventional-commit `(scope)` group left of the id trailer must
    // NOT be mined — the walk stops at the first non-spec-id group.
    let msg = "feat(api): add endpoint (FR-1-042)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["FR-1-042".to_string()]
    );
    // Prose-y paren left of the trailer doesn't leak either.
    let msg = "release: stuff (foo) (BUG-9)\n";
    assert_eq!(extract_spec_ids_from_commit(msg), vec!["BUG-9".to_string()]);
    // A non-spec-id LAST group still drops the line's contribution.
    let msg = "release: v1.2.3 (BUG-9) (1.2.3)\n";
    assert!(extract_spec_ids_from_commit(msg).is_empty());
}

// BUG-546: the non-standard `(TASK-800 / STORY-610 slice 1a)` trailer that
// broke `aida review` surface-detection, the `aida human` reviews bucket,
// and the `aida pull` Done→Completed auto-bump — all three share the
// `(SPEC-ID)`-paren parser, which previously extracted NEITHER spec from a
// multi-spec-plus-prose paren. After the fix it must harvest BOTH ids while
// the prose / separators are ignored. Single clean trailers and the
// prose-rejecting guard must still behave.
// trace:BUG-546
#[test]
fn extract_spec_ids_robust_multi_spec_prose_trailer() {
    // The real PR #869 shape: two specs + a `/` separator + prose, with a
    // `(#NN)` PR-number suffix. Both ids resolve, in subject order.
    let msg = "[AI:claude] feat(burndown): exclude supervised specs \
            (TASK-800 / STORY-610 slice 1a) (#869)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["TASK-800".to_string(), "STORY-610".to_string()]
    );
    // Prose-led paren still contributes nothing (release-note guard).
    let msg = "release: stuff (foo BUG-23)\n";
    assert!(extract_spec_ids_from_commit(msg).is_empty());
    // A `(scope)` conventional-commit group still stops the walk.
    let msg = "feat(api): add endpoint\n";
    assert!(extract_spec_ids_from_commit(msg).is_empty());
    // Comma + prose tail after a leading id: ids harvested, prose dropped.
    let msg = "fix: thing (BUG-1, BUG-2 follow-up)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-1".to_string(), "BUG-2".to_string()]
    );
}

// BUG-546: a commit whose ONLY spec linkage is `trace:SPEC-ID` lines in the
// body (no usable subject trailer) must still resolve — `collect_git_linkage`
// and the review surfaces use this to link an open PR to its spec when the
// subject paren is unusable. trace:BUG-546
#[test]
fn extract_trace_line_spec_ids_resolves_body_trace_markers() {
    // The PR #869 body carried `trace:TASK-800 trace:STORY-610`.
    let msg = "[AI:claude] feat: something\n\n\
            Body prose.\n\ntrace:TASK-800 trace:STORY-610\n";
    let ids = extract_trace_line_spec_ids(msg);
    assert!(ids.contains(&"TASK-800".to_string()), "{ids:?}");
    assert!(ids.contains(&"STORY-610".to_string()), "{ids:?}");
    // A `| ai:claude` provenance suffix doesn't get mined as a spec.
    let msg = "fix: x\n\ntrace:BUG-546 | ai:claude\n";
    assert_eq!(
        extract_trace_line_spec_ids(msg),
        vec!["BUG-546".to_string()]
    );
    // `trace:relates:STORY-611` qualifier form → take the final segment.
    let msg = "fix: x\n\ntrace:relates:STORY-611\n";
    assert_eq!(
        extract_trace_line_spec_ids(msg),
        vec!["STORY-611".to_string()]
    );
    // Comma-separated run after a single `trace:`.
    let msg = "fix: x\n\ntrace:BUG-1,BUG-2\n";
    assert_eq!(
        extract_trace_line_spec_ids(msg),
        vec!["BUG-1".to_string(), "BUG-2".to_string()]
    );
    // No trace lines → empty.
    assert!(extract_trace_line_spec_ids("chore: bump\n").is_empty());
}

// STORY-498: pure validity-gate core. A resolver maps id → resolution; the
// gate must flag nonexistent + rejected references, pass live ones, and
// honour the no-trailer / plan-commit exemptions. trace:STORY-498 | ai:claude
#[test]
fn trace_gate_flags_nonexistent_and_rejected_references() {
    let commits = vec![
        // live → passes
        (
            "aaaaaaa".to_string(),
            "[AI:claude] feat(api): add endpoint (FR-1-042) (#10)".to_string(),
        ),
        // nonexistent → fails
        (
            "bbbbbbb".to_string(),
            "[AI:claude] fix(scope): hallucinated id (FR-99999)".to_string(),
        ),
        // rejected → fails (dead reference)
        (
            "ccccccc".to_string(),
            "fix(thing): cite a dead spec (BUG-7)".to_string(),
        ),
        // mechanical / release commit with no trailer → exempt
        (
            "ddddddd".to_string(),
            "chore(release): bump to v1.2.3".to_string(),
        ),
        // plan commit naming a not-yet-real id → skipped
        (
            "eeeeeee".to_string(),
            "[AI:claude] docs(plans): plan for unbuilt work (STORY-12345)".to_string(),
        ),
    ];

    let resolve = |id: &str| match id.to_ascii_uppercase().as_str() {
        "FR-1-042" => SpecResolution::Live,
        "BUG-7" => SpecResolution::Rejected,
        _ => SpecResolution::Missing,
    };

    let violations = validate_trailer_references(&commits, resolve);
    assert_eq!(violations.len(), 2, "got: {violations:?}");

    let missing = &violations[0];
    assert_eq!(missing.spec_id, "FR-99999");
    assert_eq!(missing.verdict, TrailerVerdict::Nonexistent);
    assert_eq!(missing.sha, "bbbbbbb");

    let dead = &violations[1];
    assert_eq!(dead.spec_id, "BUG-7");
    assert_eq!(dead.verdict, TrailerVerdict::Rejected);
    assert_eq!(dead.sha, "ccccccc");
}

#[test]
fn trace_gate_passes_when_all_references_live() {
    let commits = vec![
        (
            "1111111".to_string(),
            "[AI:claude] feat(x): a (TASK-1)".to_string(),
        ),
        (
            "2222222".to_string(),
            "STORY-2: leading-colon shape (#42)".to_string(),
        ),
        (
            "3333333".to_string(),
            "fix: multi (BUG-3 BUG-4)".to_string(),
        ),
        ("4444444".to_string(), "chore: no trailer here".to_string()),
    ];
    // Everything that resolves is live.
    let violations = validate_trailer_references(&commits, |_| SpecResolution::Live);
    assert!(
        violations.is_empty(),
        "expected clean gate, got: {violations:?}"
    );
}

#[test]
fn trace_gate_reports_every_id_in_a_multi_spec_trailer() {
    let commits = vec![(
        "abcabc1".to_string(),
        "[AI:claude] feat(s): bulk (BUG-1 BUG-2 BUG-3) (#9)".to_string(),
    )];
    // BUG-2 missing, the rest live.
    let resolve = |id: &str| {
        if id.eq_ignore_ascii_case("BUG-2") {
            SpecResolution::Missing
        } else {
            SpecResolution::Live
        }
    };
    let violations = validate_trailer_references(&commits, resolve);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].spec_id, "BUG-2");
    assert_eq!(violations[0].verdict, TrailerVerdict::Nonexistent);
}

// ── TASK-868: trace-ROT resolution (`aida trace check` core) ──
//
// The rot decision is `resolve_trace_rot(store, id)`. Verify a resolving
// trace is NOT flagged, while each rotted shape (deleted/renumbered,
// rejected, archived) IS — and that the missing-store fallback is permissive.
// trace:TASK-868 | ai:claude
#[test]
fn trace_check_flags_rotted_resolves_clean_ones() {
    use aida_core::{Requirement, RequirementStatus, RequirementsStore};

    let mut store = RequirementsStore::default();

    let mut live = Requirement::new("Live spec".to_string(), "desc".to_string());
    live.spec_id = Some("TASK-868".to_string());
    live.status = RequirementStatus::Approved;
    store.requirements.push(live);

    let mut rejected = Requirement::new("Dead spec".to_string(), "desc".to_string());
    rejected.spec_id = Some("BUG-7".to_string());
    rejected.status = RequirementStatus::Rejected;
    store.requirements.push(rejected);

    let mut archived = Requirement::new("Filed-away spec".to_string(), "desc".to_string());
    archived.spec_id = Some("STORY-9".to_string());
    archived.status = RequirementStatus::Completed;
    archived.archived = true;
    store.requirements.push(archived);

    let s = Some(&store);

    // A resolving trace is NOT rot and NOT flagged.
    assert_eq!(resolve_trace_rot(s, "TASK-868"), TraceRotVerdict::Live);
    assert!(!resolve_trace_rot(s, "TASK-868").is_rot());
    assert!(!resolve_trace_rot(s, "TASK-868").is_flagged());
    // case-insensitive resolution
    assert_eq!(resolve_trace_rot(s, "task-868"), TraceRotVerdict::Live);

    // A known-dangling trace (deleted/renumbered target) IS hard rot.
    assert_eq!(resolve_trace_rot(s, "FR-99999"), TraceRotVerdict::Unknown);
    assert!(resolve_trace_rot(s, "FR-99999").is_rot());
    assert!(resolve_trace_rot(s, "FR-99999").is_flagged());

    // Rejected target is a dead link → hard rot.
    assert_eq!(resolve_trace_rot(s, "BUG-7"), TraceRotVerdict::Rejected);
    assert!(resolve_trace_rot(s, "BUG-7").is_rot());

    // Archived target still resolves: flagged (soft signal) but NOT hard rot
    // — `--block` must not fail on it.
    assert_eq!(resolve_trace_rot(s, "STORY-9"), TraceRotVerdict::Archived);
    assert!(!resolve_trace_rot(s, "STORY-9").is_rot());
    assert!(resolve_trace_rot(s, "STORY-9").is_flagged());

    // No store reachable → permissive (never flag a whole tree).
    assert_eq!(resolve_trace_rot(None, "FR-99999"), TraceRotVerdict::Live);
}

// ── STORY-499: diff-level trace-COVERAGE core (fully-isolated) ──
//
// The whole point of STORY-499 is that the coverage decision (file →
// requires-trace? + hunk → has-trace?) is a PURE fn with every dependency
// injected — no git, no store. These tests exercise it with hand-built
// hunks + an in-test resolver. trace:STORY-499 | ai:claude

fn live_resolver(id: &str) -> SpecResolution {
    // Treat anything starting `DEAD` as missing; everything else live.
    if id.to_ascii_uppercase().starts_with("DEAD") {
        SpecResolution::Missing
    } else {
        SpecResolution::Live
    }
}

// why: test-only helper returning three empty fixture maps; an alias for a test scaffold adds noise without aiding readers.
#[allow(clippy::type_complexity)]
fn empty_maps() -> (
    std::collections::HashMap<String, Vec<String>>,
    std::collections::HashMap<String, String>,
    std::collections::HashMap<String, bool>,
) {
    (
        std::collections::HashMap::new(),
        std::collections::HashMap::new(),
        std::collections::HashMap::new(),
    )
}

// ---- TASK-939: `aida doc suggest` public-surface detector ----

#[test]
fn surface_detects_new_clap_long_flag() {
    assert_eq!(
        classify_surface_line("aida-cli/src/cli.rs", "        #[clap(long)]"),
        Some(PublicSurfaceKind::CliFlag)
    );
    assert_eq!(
        classify_surface_line("aida-cli/src/cli.rs", "    #[arg(long = \"range\")]"),
        Some(PublicSurfaceKind::CliFlag)
    );
    assert_eq!(
        classify_surface_line(
            "aida-cli/src/cli.rs",
            "    #[clap(long, value_delimiter = ',')]"
        ),
        Some(PublicSurfaceKind::CliFlag)
    );
}

#[test]
fn surface_ignores_long_help_and_non_flag_attrs() {
    // `long_help` is a word-boundary miss — not a new flag.
    assert_eq!(
        classify_surface_line("aida-cli/src/cli.rs", "    #[clap(long_help = \"x\")]"),
        None
    );
    // A plain short attr with no `long`.
    assert_eq!(
        classify_surface_line("aida-cli/src/cli.rs", "    #[clap(short = 'x')]"),
        None
    );
    // `#[clap(subcommand)]` marks a field, not a new flag/subcommand.
    assert_eq!(
        classify_surface_line("aida-cli/src/cli.rs", "    #[clap(subcommand)]"),
        None
    );
    // Ordinary code is never surface.
    assert_eq!(
        classify_surface_line("aida-cli/src/main.rs", "    let x = long_variable + 1;"),
        None
    );
}

#[test]
fn surface_detects_named_subcommand() {
    assert_eq!(
        classify_surface_line("aida-cli/src/cli.rs", "    #[command(name = \"suggest\")]"),
        Some(PublicSurfaceKind::CliSubcommand)
    );
    assert_eq!(
        classify_surface_line("aida-cli/src/cli.rs", "    #[clap(name = \"foo\")]"),
        Some(PublicSurfaceKind::CliSubcommand)
    );
}

#[test]
fn surface_detects_mcp_tool_only_in_mcp_file() {
    // A snake_case tool name in mcp.rs is a new MCP tool.
    assert_eq!(
        classify_surface_line(
            "aida-cli/src/mcp.rs",
            "            \"name\": \"doc_suggest\","
        ),
        Some(PublicSurfaceKind::McpTool)
    );
    // Title-case resource titles are NOT tools.
    assert_eq!(
        classify_surface_line(
            "aida-cli/src/mcp.rs",
            "        \"name\": \"Project Summary\","
        ),
        None
    );
    // The server-name line is excluded.
    assert_eq!(
        classify_surface_line("aida-cli/src/mcp.rs", "            \"name\": \"aida\","),
        None
    );
    // The same shape in a non-mcp file is not a tool.
    assert_eq!(
        classify_surface_line("aida-cli/src/other.rs", "    \"name\": \"doc_suggest\","),
        None
    );
}

#[test]
fn detect_new_public_surface_over_hunks() {
    let hunks = vec![
        ParsedHunk {
            file: "aida-cli/src/cli.rs".to_string(),
            new_start: 2320,
            added: vec![
                "    /// A doc comment (not surface)".to_string(),
                "    #[clap(long)]".to_string(),
                "    range: Option<String>,".to_string(),
            ],
            removed: vec![],
        },
        ParsedHunk {
            file: "aida-cli/src/mcp.rs".to_string(),
            new_start: 6100,
            added: vec!["            \"name\": \"doc_suggest\",".to_string()],
            removed: vec![],
        },
    ];
    let hits = detect_new_public_surface(&hunks);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].kind, PublicSurfaceKind::CliFlag);
    assert_eq!(hits[0].new_start, 2320);
    assert_eq!(hits[1].kind, PublicSurfaceKind::McpTool);
    assert_eq!(hits[1].file, "aida-cli/src/mcp.rs");
}

#[test]
fn detect_new_public_surface_empty_for_plain_code() {
    let hunks = vec![ParsedHunk {
        file: "aida-cli/src/main.rs".to_string(),
        new_start: 10,
        added: vec![
            "    let total = a + b;".to_string(),
            "    println!(\"{}\", total);".to_string(),
        ],
        removed: vec![],
    }];
    assert!(detect_new_public_surface(&hunks).is_empty());
}

#[test]
fn mcp_tool_name_parses_snake_case_only() {
    assert_eq!(
        mcp_tool_name_in_line("\"name\": \"list_requirements\","),
        Some("list_requirements")
    );
    assert_eq!(
        mcp_tool_name_in_line("\"name\": \"Queue — in-flight\","),
        None
    );
    assert_eq!(mcp_tool_name_in_line("let name = 3;"), None);
}

#[test]
fn coverage_file_classifier_exemptions() {
    // F1 tests
    assert_eq!(
        classify_coverage_file("aida-cli/tests/foo.rs", None),
        Some(CoverageExemption::TestFile)
    );
    assert_eq!(
        classify_coverage_file("src/bar_test.rs", None),
        Some(CoverageExemption::TestFile)
    );
    assert_eq!(
        classify_coverage_file("web/comp.test.ts", None),
        Some(CoverageExemption::TestFile)
    );
    // F2 generated — path + marker
    assert_eq!(
        classify_coverage_file("src/generated/types.rs", None),
        Some(CoverageExemption::Generated)
    );
    assert_eq!(
        classify_coverage_file("src/types.gen.rs", None),
        Some(CoverageExemption::Generated)
    );
    assert_eq!(
        classify_coverage_file("src/types.rs", Some("// @generated by tool\nfn x() {}")),
        Some(CoverageExemption::Generated)
    );
    // F3 docs / non-source
    assert_eq!(
        classify_coverage_file("README.md", None),
        Some(CoverageExemption::DocOrProse)
    );
    assert_eq!(
        classify_coverage_file("docs/plans/x.rs", None),
        Some(CoverageExemption::DocOrProse)
    );
    // F4 config
    assert_eq!(
        classify_coverage_file("Cargo.toml", None),
        Some(CoverageExemption::ConfigData)
    );
    assert_eq!(
        classify_coverage_file("Cargo.lock", None),
        Some(CoverageExemption::ConfigData)
    );
    // F5 vendored
    assert_eq!(
        classify_coverage_file("vendor/lib/x.rs", None),
        Some(CoverageExemption::Vendored)
    );
    // Coverable source file → None
    assert_eq!(classify_coverage_file("aida-cli/src/main.rs", None), None);
    // Unknown extension → not coverable (prose)
    assert_eq!(
        classify_coverage_file("Makefile", None),
        Some(CoverageExemption::DocOrProse)
    );
}

#[test]
fn coverage_hunk_classifier_exemptions() {
    let mk = |added: &[&str], removed: &[&str]| ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 10,
        added: added.iter().map(|s| s.to_string()).collect(),
        removed: removed.iter().map(|s| s.to_string()).collect(),
    };
    // H1 pure deletion
    assert_eq!(
        classify_coverage_hunk(&mk(&[], &["old();", "more();"]), 1),
        Some(CoverageExemption::PureDeletion)
    );
    // H2 whitespace-only reflow (same content, different spacing)
    assert_eq!(
        classify_coverage_hunk(&mk(&["let  x =  1;"], &["let x = 1;"]), 1),
        Some(CoverageExemption::CommentOrBlankOnly)
    );
    // H4 comment-only addition
    assert_eq!(
        classify_coverage_hunk(&mk(&["// a comment", "/// doc"], &[]), 1),
        Some(CoverageExemption::CommentOrBlankOnly)
    );
    // H4 trivial one-liner (<=1 effective added line)
    assert_eq!(
        classify_coverage_hunk(&mk(&["let x = 1;"], &[]), 1),
        Some(CoverageExemption::Trivial)
    );
    // Real multi-line code addition → coverable (None)
    assert_eq!(
        classify_coverage_hunk(
            &mk(&["fn f() {", "    do_a();", "    do_b();", "}"], &[]),
            1
        ),
        None
    );
}

#[test]
fn coverage_attribution_in_hunk_anchor() {
    // A real coverable code hunk with an inline `// trace:` is covered.
    let hunks = vec![ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 10,
        added: vec![
            "fn feature() {".to_string(),
            "    // trace:STORY-499 | ai:claude".to_string(),
            "    do_work();".to_string(),
            "    do_more();".to_string(),
            "}".to_string(),
        ],
        removed: vec![],
    }];
    let (files, gen, floor) = empty_maps();
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert_eq!(cov.coverable(), 1);
    assert_eq!(cov.covered(), 1);
    assert_eq!(cov.hunks[0].covered, Some(CoverageSource::InHunkAnchor));
    assert!(cov.uncovered().is_empty());
    assert_eq!(cov.ratio(), 1.0);
}

#[test]
fn coverage_attribution_proximity_anchor_above_only() {
    // The trace anchor sits ABOVE the hunk in the post-change file (an
    // unchanged `/// trace:` over a changed body). new_start=4 (1-based),
    // so file lines [0..3) are searched above it; the anchor at index 2
    // (line 3) is within N=5.
    let post = vec![
        "use std::x;".to_string(),         // line 1
        "".to_string(),                    // line 2
        "/// trace:STORY-499".to_string(), // line 3  (above, within 5)
        "fn body() {".to_string(),         // line 4  <- hunk start
        "    changed();".to_string(),      // line 5
        "    changed2();".to_string(),     // line 6
        "}".to_string(),
    ];
    let hunks = vec![ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 4,
        added: vec!["    changed();".to_string(), "    changed2();".to_string()],
        removed: vec!["    old();".to_string()],
    }];
    let mut files = std::collections::HashMap::new();
    files.insert("src/x.rs".to_string(), post);
    let (_e, gen, floor) = empty_maps();
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert_eq!(cov.hunks[0].covered, Some(CoverageSource::ProximityAnchor));

    // A trace anchor BELOW the hunk must NOT count (it annotates the next
    // item, per SPIKE-47 risk #2).
    let post_below = vec![
        "fn body() {".to_string(),         // line 1  <- hunk start
        "    changed();".to_string(),      // line 2
        "    changed2();".to_string(),     // line 3
        "}".to_string(),                   // line 4
        "/// trace:STORY-499".to_string(), // line 5 (below — must not count)
    ];
    let mut files2 = std::collections::HashMap::new();
    files2.insert("src/x.rs".to_string(), post_below);
    let hunks2 = vec![ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 1,
        added: vec!["    changed();".to_string(), "    changed2();".to_string()],
        removed: vec![],
    }];
    let cov2 = compute_diff_coverage(&hunks2, &files2, &gen, &floor, 5, 1, live_resolver);
    // No anchor above, no trailer → uncovered.
    assert!(cov2.hunks[0].is_uncovered_coverable());
}

#[test]
fn coverage_attribution_file_level_module_anchor() {
    let post = vec![
        "//! trace:STORY-499".to_string(),
        "fn a() {".to_string(),
        "    x();".to_string(),
        "    y();".to_string(),
        "}".to_string(),
    ];
    let hunks = vec![ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 50, // far from the module anchor → only file-level can cover
        added: vec!["    x();".to_string(), "    y();".to_string()],
        removed: vec![],
    }];
    let mut files = std::collections::HashMap::new();
    files.insert("src/x.rs".to_string(), post);
    let (_e, gen, floor) = empty_maps();
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert_eq!(cov.hunks[0].covered, Some(CoverageSource::FileAnchor));
}

#[test]
fn coverage_attribution_commit_trailer_floor() {
    // No inline anchor anywhere, but the file's commit carries a live
    // trailer → covered by the §4.2.4 floor. This is why the default world
    // (every feat/fix commit has a trailer) keeps the gate silent.
    let hunks = vec![ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 10,
        added: vec![
            "fn f() {".to_string(),
            "    a();".to_string(),
            "    b();".to_string(),
            "}".to_string(),
        ],
        removed: vec![],
    }];
    let (files, gen, _f) = empty_maps();
    let mut floor = std::collections::HashMap::new();
    floor.insert("src/x.rs".to_string(), true);
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert_eq!(cov.hunks[0].covered, Some(CoverageSource::CommitTrailer));
}

#[test]
fn coverage_uncovered_coverable_hunk_is_reported() {
    // Real code, no anchor, no trailer → the one thing the gate flags.
    let hunks = vec![ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 10,
        added: vec![
            "fn slipped_in() {".to_string(),
            "    untraced();".to_string(),
            "    more_untraced();".to_string(),
            "}".to_string(),
        ],
        removed: vec![],
    }];
    let (files, gen, floor) = empty_maps();
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert_eq!(cov.coverable(), 1);
    assert_eq!(cov.covered(), 0);
    assert_eq!(cov.uncovered().len(), 1);
    assert_eq!(cov.ratio(), 0.0);
}

#[test]
fn coverage_dead_anchor_does_not_count() {
    // A `// trace:DEAD-1` that resolves Missing is NOT coverage (parity with
    // STORY-498's live-resolution requirement).
    let hunks = vec![ParsedHunk {
        file: "src/x.rs".to_string(),
        new_start: 10,
        added: vec![
            "fn f() {".to_string(),
            "    // trace:DEAD-1".to_string(),
            "    a();".to_string(),
            "    b();".to_string(),
            "}".to_string(),
        ],
        removed: vec![],
    }];
    let (files, gen, floor) = empty_maps();
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert!(cov.hunks[0].is_uncovered_coverable());
}

#[test]
fn coverage_exempt_files_drop_from_denominator() {
    let hunks = vec![
        // exempt: test file
        ParsedHunk {
            file: "tests/it.rs".to_string(),
            new_start: 1,
            added: vec!["assert!(true);".to_string(), "assert!(false);".to_string()],
            removed: vec![],
        },
        // exempt: docs
        ParsedHunk {
            file: "README.md".to_string(),
            new_start: 1,
            added: vec!["a".to_string(), "b".to_string()],
            removed: vec![],
        },
        // coverable + uncovered
        ParsedHunk {
            file: "src/x.rs".to_string(),
            new_start: 10,
            added: vec![
                "fn f() {".to_string(),
                "    a();".to_string(),
                "    b();".to_string(),
                "}".to_string(),
            ],
            removed: vec![],
        },
    ];
    let (files, gen, floor) = empty_maps();
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert_eq!(cov.exempt(), 2);
    assert_eq!(cov.coverable(), 1);
    assert_eq!(cov.uncovered().len(), 1);
}

#[test]
fn coverage_all_exempt_diff_passes_vacuously() {
    let hunks = vec![ParsedHunk {
        file: "docs/plan.md".to_string(),
        new_start: 1,
        added: vec!["prose".to_string()],
        removed: vec![],
    }];
    let (files, gen, floor) = empty_maps();
    let cov = compute_diff_coverage(&hunks, &files, &gen, &floor, 5, 1, live_resolver);
    assert_eq!(cov.coverable(), 0);
    assert!(cov.uncovered().is_empty());
    assert_eq!(cov.ratio(), 1.0);
}

#[test]
fn coverage_diff_parser_splits_hunks_and_files() {
    let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 unchanged
+added line one
+added line two
@@ -10,0 +12,1 @@
+another add
diff --git a/old.rs b/new.rs
--- a/old.rs
+++ b/new.rs
@@ -5,1 +5,1 @@
-removed
+replaced
diff --git a/gone.rs b/gone.rs
--- a/gone.rs
+++ /dev/null
@@ -1,1 +0,0 @@
-deleted file body
";
    let hunks = parse_unified_diff_hunks(diff);
    // 3 hunks attributed to real files; the /dev/null target is skipped.
    assert_eq!(hunks.len(), 3, "got: {hunks:?}");
    assert_eq!(hunks[0].file, "src/a.rs");
    assert_eq!(hunks[0].new_start, 1);
    assert_eq!(hunks[0].added, vec!["added line one", "added line two"]);
    assert_eq!(hunks[1].file, "src/a.rs");
    assert_eq!(hunks[1].new_start, 12);
    assert_eq!(hunks[2].file, "new.rs");
    assert_eq!(hunks[2].added, vec!["replaced"]);
    assert_eq!(hunks[2].removed, vec!["removed"]);
}

// ── STORY-469 Guard 1: client-side trailer-guard refusal formatting ──

#[test]
fn client_trailer_guard_refusal_names_each_dead_reference() {
    // Reuse the shared validator to produce violations, then check the
    // client-side refusal message surfaces every offending id + the
    // recovery guidance + the --force escape.
    let commits = vec![
        (
            "aaa1111".to_string(),
            "[AI:claude] feat(x): hallucinated id (STORY-99999)".to_string(),
        ),
        (
            "bbb2222".to_string(),
            "fix(y): rejected ref (BUG-7)".to_string(),
        ),
    ];
    let resolve = |id: &str| match id.to_ascii_uppercase().as_str() {
        "BUG-7" => SpecResolution::Rejected,
        _ => SpecResolution::Missing,
    };
    let violations = validate_trailer_references(&commits, resolve);
    assert_eq!(violations.len(), 2, "got: {violations:?}");

    let lines = format_trailer_guard_refusal("pr ship", &violations);
    let joined = lines.join("\n");
    assert!(joined.contains("pr ship"), "names the surface: {joined}");
    assert!(joined.contains("STORY-99999"), "names missing id: {joined}");
    assert!(joined.contains("BUG-7"), "names rejected id: {joined}");
    assert!(
        joined.contains("does not exist in the requirement graph"),
        "explains missing verdict: {joined}"
    );
    assert!(
        joined.contains("resolves to a rejected spec"),
        "explains rejected verdict: {joined}"
    );
    assert!(
        joined.contains("--force"),
        "offers the intentional bypass: {joined}"
    );
}

/// TASK-579: the origin-ID-in-git-log detector used by `aida findings
/// promote` to spot a fix that already merged against the id the finding
/// carried before becoming real work.
#[test]
fn commit_subject_references_id_matches_origin_id() {
    let ids = vec!["TASK-1-097".to_string(), "TASK-124".to_string()];

    // The real trigger from the spec: the fix shipped referencing the
    // origin-ID in a trailing paren.
    assert!(commit_subject_references_id(
        "fix(init): push onboarding task onto implementer queue (TASK-1-097)",
        &ids
    ));

    // Case-insensitive match.
    assert!(commit_subject_references_id(
        "fix(init): tweak (task-1-097)",
        &ids
    ));

    // Leading SPEC-ID: prefix shape also counts.
    assert!(commit_subject_references_id(
        "TASK-124: do the thing (#42)",
        &ids
    ));

    // An unrelated spec id does NOT match.
    assert!(!commit_subject_references_id(
        "fix(scope): something else (BUG-999)",
        &ids
    ));

    // A plan commit naming the id is NOT a completion signal.
    assert!(!commit_subject_references_id(
        "docs(plans): plan for (TASK-1-097)",
        &ids
    ));

    // No reference at all → no match.
    assert!(!commit_subject_references_id(
        "chore: bump dep version",
        &ids
    ));

    // Empty id list never matches.
    assert!(!commit_subject_references_id(
        "fix(init): tweak (TASK-1-097)",
        &[]
    ));
}

/// TASK-93: `locate_symbol_line` finds a definition's 1-based line and,
/// crucially, is NOT thrown off by a preceding blank line (the `^\s*`
/// vs `^[ \t]*` off-by-one).
#[test]
fn plan_verify_locate_symbol_line() {
    let src = "// header\n\nfn first() {}\n\n\npub fn target(x: u32) -> u32 { x }\n";
    // `fn first` is on line 3, `pub fn target` on line 6 — the two
    // blank lines before `target` must not shift the result.
    assert_eq!(locate_symbol_line(src, "first"), Some(3));
    assert_eq!(locate_symbol_line(src, "target"), Some(6));
    // Struct / enum / trait kinds + a `::`-qualified name.
    let src2 = "struct Foo;\n\nenum Bar { A }\n";
    assert_eq!(locate_symbol_line(src2, "Foo"), Some(1));
    assert_eq!(locate_symbol_line(src2, "Bar"), Some(3));
    assert_eq!(locate_symbol_line(src2, "mod_path::Foo"), Some(1));
    // A symbol that isn't defined resolves to None.
    assert_eq!(locate_symbol_line(src2, "Nonexistent"), None);
    // A mention that is not a definition (a call site) must not match.
    let src3 = "fn caller() {\n    target();\n}\n";
    assert_eq!(locate_symbol_line(src3, "target"), None);
}

/// TASK-93: placeholder paths from the template (and globs) are not
/// treated as real files; source extensions are recognised.
#[test]
fn plan_verify_path_heuristics() {
    assert!(is_plan_placeholder_path("path/to/file.rs"));
    assert!(is_plan_placeholder_path("aida-core/templates/skills/*.md"));
    assert!(is_plan_placeholder_path("<STORY-N>.md"));
    assert!(is_plan_placeholder_path("main.rs:NNN"));
    assert!(!is_plan_placeholder_path("aida-cli/src/main.rs"));

    assert!(has_plan_source_ext("aida-cli/src/main.rs"));
    assert!(has_plan_source_ext("constants.ts"));
    assert!(has_plan_source_ext("Cargo.lock"));
    assert!(!has_plan_source_ext("aida pull"));
    assert!(!has_plan_source_ext("no_extension"));
}

// trace:TASK-772 | ai:claude
/// TASK-772: the `(new)` / `(to create)` annotation after a backticked
/// path marks a to-be-created file — missing is OK, already-exists warns,
/// and unmarked missing paths still error.
#[test]
fn plan_verify_marked_new_annotation() {
    // The marker matcher itself: case-insensitive, optional whitespace,
    // must be the next thing after the backtick.
    assert!(plan_path_marked_new(" (new) — to be created"));
    assert!(plan_path_marked_new("(NEW)"));
    assert!(plan_path_marked_new("  (To Create) later"));
    assert!(!plan_path_marked_new(" — purpose (new)"));
    assert!(!plan_path_marked_new(" (newish)"));

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/exists.rs"), "// here\n").unwrap();
    std::fs::write(root.join("src/stale.rs"), "// here\n").unwrap();

    let plan = "\
# Plan: test

## Files (in build-order)

- `src/exists.rs` — existing file, unmarked
- `src/created_later.rs` (new) — to be created by this work
- `src/also_later.rs` (to create) — alternate marker form
- `src/missing.rs` — unmarked missing ref
- `src/stale.rs` (new) — marked new but already exists
";
    let report = compute_plan_report(plan, root);
    let find = |needle: &str| {
        report
            .files
            .iter()
            .find(|f| f.msg.contains(needle))
            .unwrap_or_else(|| panic!("no file finding for {needle}"))
    };

    // Existing unmarked file: plain OK, unchanged behavior.
    assert_eq!(find("src/exists.rs").level, PlanFindingLevel::Ok);
    // Marked-new missing files pass instead of erroring.
    assert_eq!(find("src/created_later.rs").level, PlanFindingLevel::Ok);
    assert!(find("src/created_later.rs").msg.contains("marked new"));
    assert_eq!(find("src/also_later.rs").level, PlanFindingLevel::Ok);
    // Unmarked missing file still errors.
    assert_eq!(find("src/missing.rs").level, PlanFindingLevel::Error);
    assert!(find("src/missing.rs").msg.contains("file not found"));
    // Marked-new file that already exists warns (stale plan signal).
    assert_eq!(find("src/stale.rs").level, PlanFindingLevel::Warn);
    assert!(find("src/stale.rs").msg.contains("already exists"));
}

/// TASK-93: symbol extraction strips item keywords and rejects prose,
/// paths, and commands so only real identifiers are verified.
#[test]
fn plan_verify_symbols_on_line() {
    let syms = plan_symbols_on_line("- `fn handle_pull_command`: snapshot the SHA");
    assert!(syms.contains(&"handle_pull_command".to_string()));
    let syms = plan_symbols_on_line("call `Storage::update_atomically` here");
    assert!(syms.contains(&"Storage::update_atomically".to_string()));
    // A backtick path or shell command is not a symbol.
    let syms = plan_symbols_on_line("run `aida pull` against `aida-cli/src/main.rs`");
    assert!(syms.is_empty());
    // Un-backticked `fn name` is still caught.
    let syms = plan_symbols_on_line("the fn verify_plan entry point");
    assert!(syms.contains(&"verify_plan".to_string()));
}

/// TASK-96: the Followups parser picks up column-0 bullets in the
/// `## Followups` section, strips a trailing period, and stops at the
/// next `##` header.
#[test]
fn plan_followups_parse_basic() {
    let plan = "\
# Plan: STORY-1

## Risks + gotchas

- This bullet is in Risks, not Followups.

## Followups

- Reverted-commit handling.
- Statusline color for the new state
  - a nested detail bullet that is not its own followup
- Wire up the metrics dashboard.

## Related

- See also: nothing.
";
    let followups = parse_plan_followups(plan);
    assert_eq!(
        followups,
        vec![
            "Reverted-commit handling".to_string(),
            "Statusline color for the new state".to_string(),
            "Wire up the metrics dashboard".to_string(),
        ]
    );
}

/// TASK-96: a plan with no Followups section yields nothing, and a
/// fenced code block inside the section is not mined for bullets.
#[test]
fn plan_followups_parse_edges() {
    // No Followups header at all.
    let no_section = "# Plan: X\n\n## Approach\n\n- not a followup\n";
    assert!(parse_plan_followups(no_section).is_empty());

    // Fenced block inside the section contributes no bullets.
    let fenced = "\
## Followups

- real followup

```bash
- this is shell output, not a followup
```

## Related
";
    assert_eq!(
        parse_plan_followups(fenced),
        vec!["real followup".to_string()]
    );
}

/// TASK-431: "no followups" sentinel bullets (None / N/A / Nothing /
/// (none), with or without an em-dash/colon/dash explanation) are dropped,
/// while a real bullet that merely STARTS with one of those words is kept.
#[test]
fn plan_followups_drops_sentinel_bullets() {
    let plan = "\
## Followups

- None — the fix is self-contained.
- N/A
- (none)
- Nothing
- Nothing to do: shipped complete
- None of these handlers validate the input
- Note: add a retry here

## Related
";
    assert_eq!(
        parse_plan_followups(plan),
        vec![
            "None of these handlers validate the input".to_string(),
            "Note: add a retry here".to_string(),
        ]
    );
    // Direct predicate coverage.
    // The caller strips a trailing '.' before calling, so pass dotless forms.
    for s in [
        "None",
        "none",
        "N/A",
        "na",
        "Nothing",
        "(none)",
        "Nothing — done",
    ] {
        assert!(followup_is_sentinel(s), "should be sentinel: {s:?}");
    }
    for s in ["None of these", "Nothing works yet", "Notify the user"] {
        assert!(!followup_is_sentinel(s), "should be real: {s:?}");
    }
}

/// BUG-104: a real followup that merely *contains* `<` (generics, a `<`
/// comparison) is kept; only a literal `<placeholder>` bullet is dropped.
#[test]
fn plan_followups_keeps_angle_brackets_drops_placeholder() {
    let plan = "\
## Followups

- Support `Vec<T>` in the parser
- Fix the `a < b` guard
- <describe the followup here>

## Related
";
    assert_eq!(
        parse_plan_followups(plan),
        vec![
            "Support `Vec<T>` in the parser".to_string(),
            "Fix the `a < b` guard".to_string(),
        ]
    );
}

/// BUG-655: a bullet whose title already exists as a child of the SAME
/// parent is recognised as already-filed (so a second commit can't double
/// file it); a genuinely-new bullet is not. Match is trim + case
/// insensitive, and scoped to the parent.
#[test]
fn followup_already_filed_matches_existing_child_title() {
    let existing = vec![
        (
            "STORY-712".to_string(),
            "Wire the zero-token path".to_string(),
        ),
        (
            "STORY-712".to_string(),
            "Add the supervision metric".to_string(),
        ),
    ];

    // Exact match under the same parent → already filed.
    assert!(followup_already_filed(
        &existing,
        "STORY-712",
        "Wire the zero-token path"
    ));
    // Trim + case-insensitive match → still already filed.
    assert!(followup_already_filed(
        &existing,
        "STORY-712",
        "  wire the zero-token path  "
    ));
    // A genuinely-new bullet under the same parent → not filed yet.
    assert!(!followup_already_filed(
        &existing,
        "STORY-712",
        "Document the new flag"
    ));
    // Same title text but a DIFFERENT parent → not a dup (parent-scoped).
    assert!(!followup_already_filed(
        &existing,
        "STORY-999",
        "Wire the zero-token path"
    ));
    // Empty existing set → nothing is ever already-filed.
    assert!(!followup_already_filed(&[], "STORY-712", "anything"));
}

/// BUG-655: the idempotency the bug breaks — re-running the dedup decision
/// over the same plan's bullets after the first run filed them is a no-op
/// (every bullet is now recognised as already-filed), so a plan committed
/// by multiple slice PRs cannot double-file. Models the filing loop's
/// growing `existing_children` set.
#[test]
fn followup_dedup_makes_refiling_a_noop() {
    let parent = "STORY-712";
    let bullets = vec![
        "Wire the zero-token path".to_string(),
        "Add the supervision metric".to_string(),
        "Document the new flag".to_string(),
    ];

    // First run: nothing exists, so every bullet is filed and recorded.
    let mut existing: Vec<(String, String)> = Vec::new();
    let mut filed_first = Vec::new();
    for b in &bullets {
        if !followup_already_filed(&existing, parent, b) {
            existing.push((parent.to_string(), b.clone()));
            filed_first.push(b.clone());
        }
    }
    assert_eq!(filed_first, bullets);

    // Second run (a sibling commit carrying the same plan): the children
    // now exist in the store, so the same loop files NOTHING.
    let mut filed_second = Vec::new();
    for b in &bullets {
        if !followup_already_filed(&existing, parent, b) {
            existing.push((parent.to_string(), b.clone()));
            filed_second.push(b.clone());
        }
    }
    assert!(
        filed_second.is_empty(),
        "re-running the filing over the same plan must be a no-op, got {filed_second:?}"
    );
}

/// BUG-655: a plan that lists the SAME bullet twice in one `## Followups`
/// section files it only once within a single run (the loop grows the
/// dedup set as it files).
#[test]
fn followup_dedup_handles_duplicate_bullets_in_one_plan() {
    let parent = "STORY-712";
    let bullets = vec![
        "Tighten the retry budget".to_string(),
        "Tighten the retry budget".to_string(), // duplicate bullet
    ];
    let mut existing: Vec<(String, String)> = Vec::new();
    let mut filed = Vec::new();
    for b in &bullets {
        if !followup_already_filed(&existing, parent, b) {
            existing.push((parent.to_string(), b.clone()));
            filed.push(b.clone());
        }
    }
    assert_eq!(filed, vec!["Tighten the retry budget".to_string()]);
}

/// BUG-656: the marker comment records `extracted from <relative paths>`;
/// the parser pulls that comma-separated list back out for the cross-store
/// plan-path dedup. A non-marker comment, or a marker without an
/// `extracted from` clause, yields nothing.
#[test]
fn parse_extracted_plans_from_marker_pulls_the_plan_paths() {
    // The exact shape `extract_plan_followups` writes.
    let marker = format!(
        "{} extracted from docs/plans/a.md, docs/plans/b.md\nfiled 2 task(s): TASK-1, TASK-2",
        FOLLOWUPS_MARKER
    );
    assert_eq!(
        parse_extracted_plans_from_marker(&marker),
        vec!["docs/plans/a.md".to_string(), "docs/plans/b.md".to_string()]
    );
    // A single plan, filed-0 variant — still records the plan path.
    let single = format!(
        "{} extracted from docs/plans/solo.md\nfiled 0 task(s)",
        FOLLOWUPS_MARKER
    );
    assert_eq!(
        parse_extracted_plans_from_marker(&single),
        vec!["docs/plans/solo.md".to_string()]
    );
    // An unrelated comment contributes no plan paths.
    assert!(parse_extracted_plans_from_marker("just a normal comment").is_empty());
    // A marker missing the `extracted from` clause contributes nothing.
    assert!(parse_extracted_plans_from_marker(FOLLOWUPS_MARKER).is_empty());
}

/// BUG-656: running the followup filing twice over the SAME plan files the
/// children only on the first completion — the second run is a no-op. The
/// stable signature is the plan PATH, not the completing spec, so this holds
/// even when the re-completion is a different spec that also owns the plan
/// (a plan header listing several specs, as on EPIC-0428).
#[test]
fn followup_extraction_is_idempotent_per_plan_across_completions() {
    let plan = "docs/plans/2026-06-30-shared.md".to_string();

    // First completion: nothing extracted yet, so the plan is pending.
    let already_extracted: std::collections::HashSet<String> = std::collections::HashSet::new();
    let pending_first = plans_pending_extraction(&[plan.clone()], &already_extracted);
    assert_eq!(
        pending_first,
        vec![plan.clone()],
        "first completion must process the plan"
    );

    // The first run wrote `[aida:followups] extracted from <plan>`; rebuild
    // the already-extracted set from that marker, exactly as the real path
    // does by scanning every spec's comments.
    let marker = format!(
        "{} extracted from {}\nfiled 2 task(s)",
        FOLLOWUPS_MARKER, plan
    );
    let already_extracted: std::collections::HashSet<String> =
        parse_extracted_plans_from_marker(&marker)
            .into_iter()
            .collect();

    // Second completion (re-completion, OR a sibling spec that also owns the
    // plan): the plan is no longer pending, so nothing is re-filed.
    let pending_second = plans_pending_extraction(&[plan.clone()], &already_extracted);
    assert!(
        pending_second.is_empty(),
        "re-completing a plan already extracted must be a no-op, got {pending_second:?}"
    );
}

/// BUG-656: a cross-plan completion does NOT re-file another plan's
/// followups. Spec A owns plan-A; spec B's header also references plan-A (so
/// B "owns" it) plus its own plan-B. After A extracts plan-A, completing B
/// processes only plan-B — plan-A's followups are not re-filed under B.
#[test]
fn cross_plan_completion_does_not_refile_another_plans_followups() {
    let plan_a = "docs/plans/a.md".to_string();
    let plan_b = "docs/plans/b.md".to_string();

    // Spec A completes first and extracts plan-A.
    let after_a: std::collections::HashSet<String> = parse_extracted_plans_from_marker(&format!(
        "{} extracted from {}\nfiled 1 task(s)",
        FOLLOWUPS_MARKER, plan_a
    ))
    .into_iter()
    .collect();

    // Spec B owns BOTH plan-A (cross-reference) and plan-B. Only plan-B is
    // pending — plan-A was already extracted by A.
    let b_owned = vec![plan_a.clone(), plan_b.clone()];
    let pending = plans_pending_extraction(&b_owned, &after_a);
    assert_eq!(
        pending,
        vec![plan_b.clone()],
        "B must only extract its own plan, not re-file plan-A's followups"
    );
}

/// BUG-656: the secondary global-title guard catches the same followup text
/// copied into a DISTINCT sibling plan (a different path the plan-path
/// signature won't match). Once `alpha` is filed anywhere, a sibling plan's
/// identical bullet is recognised as already-filed regardless of parent;
/// trim + case-insensitive.
#[test]
fn followup_filed_anywhere_dedups_copied_bullet_text() {
    let filed = vec!["alpha".to_string(), "wire the path".to_string()];
    assert!(followup_filed_anywhere(&filed, "alpha"));
    assert!(followup_filed_anywhere(&filed, "  ALPHA  "));
    assert!(!followup_filed_anywhere(&filed, "beta"));
    assert!(!followup_filed_anywhere(&[], "anything"));
}

/// BUG-680: the shipped-followup guard skips a bullet already filed from the
/// SAME source plan when that spec has reached a terminal status (Completed
/// or Rejected), and links to it. An open prior filing (not terminal), a
/// different plan, or a non-matching title do NOT count — those cases are the
/// existing BUG-655/656 guards' job.
#[test]
fn followup_shipped_from_plan_skips_terminal_prior_filing() {
    let plan = "docs/plans/2026-06-30-spike-70.md".to_string();
    let other = "docs/plans/2026-06-30-other.md".to_string();
    let owned: std::collections::HashSet<String> = [plan.clone()].into_iter().collect();

    let filed_from_plan = vec![
        // Completed followup from THIS plan → the re-file it must block.
        (
            plan.clone(),
            "Wire the metrics dashboard".to_string(),
            "TASK-1003".to_string(),
            true,
        ),
        // Rejected followup from THIS plan → terminal, also blocks.
        (
            plan.clone(),
            "Add the retry budget".to_string(),
            "TASK-1005".to_string(),
            true,
        ),
        // Still-open followup from THIS plan → NOT terminal, does not block.
        (
            plan.clone(),
            "Open item".to_string(),
            "TASK-1100".to_string(),
            false,
        ),
        // Completed followup from a DIFFERENT plan → wrong plan, does not block.
        (
            other.clone(),
            "Elsewhere item".to_string(),
            "TASK-2000".to_string(),
            true,
        ),
    ];

    // Terminal + same plan + matching title (trim/case-insensitive) → linked.
    assert_eq!(
        followup_shipped_from_plan(&filed_from_plan, &owned, "  wire the metrics dashboard "),
        Some("TASK-1003")
    );
    assert_eq!(
        followup_shipped_from_plan(&filed_from_plan, &owned, "Add the retry budget"),
        Some("TASK-1005")
    );
    // Open prior filing → not blocked here (BUG-655/656 handle the open case).
    assert_eq!(
        followup_shipped_from_plan(&filed_from_plan, &owned, "Open item"),
        None
    );
    // A completed followup filed from a plan this spec does not own → skipped.
    assert_eq!(
        followup_shipped_from_plan(&filed_from_plan, &owned, "Elsewhere item"),
        None
    );
    // Genuinely-new bullet → nothing to link.
    assert_eq!(
        followup_shipped_from_plan(&filed_from_plan, &owned, "Brand new work"),
        None
    );
    // Empty history → never blocks.
    assert_eq!(followup_shipped_from_plan(&[], &owned, "anything"), None);
}

/// BUG-680 (acceptance): filing the same plan followup twice yields ONE spec,
/// not two — once the first filing ships (Completed), a re-extraction of the
/// same plan recognises the followup as already-shipped straight from the
/// child's recorded `followup-src:` provenance and does not open a duplicate.
/// Models the store-backed `filed_from_plan` set the real path builds from
/// tags. This is the guarantee the parent's marker comment cannot make (it
/// can be lost/unsynced); the child's tag is durable.
#[test]
fn followup_refiled_after_ship_yields_one_spec() {
    let plan = "docs/plans/2026-06-30-spike-70.md".to_string();
    let owned: std::collections::HashSet<String> = [plan.clone()].into_iter().collect();
    let bullet = "Wire the metrics dashboard";

    // First extraction: no followup has been filed from this plan yet, so the
    // shipped guard does not fire and the bullet is filed as a new TASK.
    let filed_from_plan: Vec<(String, String, String, bool)> = Vec::new();
    assert_eq!(
        followup_shipped_from_plan(&filed_from_plan, &owned, bullet),
        None,
        "first run must file the followup"
    );

    // That TASK later ships: it carries `followup-src:<plan>` and is now
    // Completed. Rebuild the store-backed set exactly as the real path does.
    let filed_from_plan = vec![(
        plan.clone(),
        bullet.to_string(),
        "TASK-1003".to_string(),
        true, // Completed
    )];

    // Second extraction of the same plan (parent marker lost/unsynced): the
    // shipped guard now links to the existing spec instead of opening a
    // second one — one spec total, not two.
    assert_eq!(
        followup_shipped_from_plan(&filed_from_plan, &owned, bullet),
        Some("TASK-1003"),
        "re-filing after the followup shipped must be a no-op"
    );
}

/// BUG-105: when multiple plan files own the same spec, discover_plan_context
/// merges them all (union of critical-files + followups, all paths listed)
/// instead of silently using only the first.
#[test]
fn discover_plan_context_merges_multiple_owning_plans() {
    let dir = tempfile::tempdir().unwrap();
    let plans = dir.path().join("docs/plans");
    std::fs::create_dir_all(&plans).unwrap();
    std::fs::write(
        plans.join("a.md"),
        "# Plan A (STORY-1)\n\n## Critical Files\n- `a.rs`\n\n## Followups\n- followup A\n",
    )
    .unwrap();
    std::fs::write(
        plans.join("b.md"),
        "# Plan B (STORY-1)\n\n## Critical Files\n- `b.rs`\n\n## Followups\n- followup B\n",
    )
    .unwrap();

    let ctx = discover_plan_context(dir.path(), "STORY-1").expect("should find a merged context");
    assert!(
        ctx.plan_file.contains("a.md") && ctx.plan_file.contains("b.md"),
        "both plan paths listed: {}",
        ctx.plan_file
    );
    assert!(
        ctx.critical_files.contains(&"a.rs".to_string())
            && ctx.critical_files.contains(&"b.rs".to_string()),
        "critical files unioned: {:?}",
        ctx.critical_files
    );
    assert!(
        ctx.followups.contains(&"followup A".to_string())
            && ctx.followups.contains(&"followup B".to_string()),
        "followups unioned: {:?}",
        ctx.followups
    );
}

/// TASK-95: the Critical Files parser collects backtick paths from the
/// `## Critical Files` section's bullets and nowhere else.
#[test]
fn plan_critical_files_parse() {
    let plan = "\
# Plan: STORY-1

## Files

### `not-collected/here.rs` — this is the Files section

## Critical Files

- `aida-cli/src/main.rs`
- `aida-core/src/models.rs` — with a trailing note
- a bullet with no backtick path

## Verification

- `also-not-collected.rs`
";
    assert_eq!(
        parse_plan_critical_files(plan),
        vec![
            "aida-cli/src/main.rs".to_string(),
            "aida-core/src/models.rs".to_string(),
        ]
    );
}

/// TASK-95: the Verification parser returns the first fenced block of
/// the `## Verification` section, and None when there isn't one.
#[test]
fn plan_verification_parse() {
    let plan = "\
## Verification

Some prose first.

```bash
cargo build --workspace
cargo test -p aida-cli
```

## Followups
";
    assert_eq!(
        parse_plan_verification(plan).as_deref(),
        Some("cargo build --workspace\ncargo test -p aida-cli")
    );
    // No fenced block → None.
    let no_fence = "## Verification\n\nJust prose, no script.\n\n## Related\n";
    assert!(parse_plan_verification(no_fence).is_none());
    // No Verification section at all → None.
    assert!(parse_plan_verification("## Approach\n\ntext\n").is_none());
}

/// TASK-94: the trace-graph scanner walks source files, captures the
/// full node-aware spec id, finds the symbol on or just below the
/// trace line, and skips build/vendor trees.
#[test]
fn plan_helpers_scan_trace_graph() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sub");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.rs"),
        "// trace:STORY-86 | ai:claude\nfn auto_bump() {}\n\n// trace:FR-1-042\nstruct Thing;\n",
    )
    .unwrap();
    std::fs::write(src.join("b.rs"), "pub fn inline() {} // trace:TASK-50\n").unwrap();
    // A file under target/ must be skipped.
    let skip = dir.path().join("target");
    std::fs::create_dir_all(&skip).unwrap();
    std::fs::write(skip.join("c.rs"), "// trace:STORY-86\nfn ignored() {}\n").unwrap();

    let wanted: HashSet<String> = ["STORY-86", "FR-1-042", "TASK-50"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let hits = scan_trace_graph(dir.path(), &wanted);

    // STORY-86 → auto_bump in a.rs only (the target/ copy is skipped).
    let s86 = hits.get("STORY-86").expect("STORY-86 hit");
    assert_eq!(s86.len(), 1);
    assert_eq!(s86[0].symbol.as_deref(), Some("auto_bump"));
    // Node-aware id captured in full — never bucketed under `FR-1`.
    assert!(!hits.contains_key("FR-1"));
    assert_eq!(
        hits.get("FR-1-042").expect("FR-1-042 hit")[0]
            .symbol
            .as_deref(),
        Some("Thing")
    );
    // An inline trailing trace comment resolves the symbol on its line.
    assert_eq!(
        hits.get("TASK-50").expect("TASK-50 hit")[0]
            .symbol
            .as_deref(),
        Some("inline")
    );
}

/// TASK-113: the ultraplan prompt assembler handles a spec with no
/// acceptance criteria (edge case 1) and no trace-graph helpers (edge
/// case 3) — placeholder text in the prompt, warnings surfaced — and
/// the happy path with both present.
#[test]
fn ultraplan_prompt_assembly() {
    use aida_core::{Requirement, RequirementsStore};

    // No `## Acceptance`, no helpers section.
    let mut store = RequirementsStore::new();
    let mut bare = Requirement::new("do the thing".into(), "Some context.".into());
    bare.spec_id = Some("TASK-1".into());
    store.requirements.push(bare);
    let (prompt, warnings) =
        assemble_ultraplan_prompt(&store, &store.requirements[0], None, true, &[]);
    assert!(prompt.contains("Plan the implementation of TASK-1: do the thing."));
    assert!(prompt.contains("## Plan structure"));
    assert!(prompt.contains("(none specified"));
    assert!(warnings.iter().any(|w| w.contains("`## Acceptance`")));
    assert!(warnings.iter().any(|w| w.contains("reusable helpers")));

    // With acceptance + a helpers section → no warnings.
    let mut store2 = RequirementsStore::new();
    let mut ok = Requirement::new(
        "t2".into(),
        "Intro.\n\n## Acceptance\n\n- [ ] alpha\n- [ ] beta\n".into(),
    );
    ok.spec_id = Some("TASK-2".into());
    store2.requirements.push(ok);
    let (prompt2, warnings2) = assemble_ultraplan_prompt(
        &store2,
        &store2.requirements[0],
        Some("## Reusable helpers\n\n- x\n"),
        true,
        &[],
    );
    assert!(prompt2.contains("- [ ] alpha"));
    assert!(prompt2.contains("## Reusable helpers"));
    assert!(warnings2.is_empty());
}

/// TASK-113 edge case 2: a spec with many siblings — the prompt caps
/// the list and notes how many were omitted.
#[test]
fn ultraplan_prompt_sibling_truncation() {
    use aida_core::models::Relationship;
    use aida_core::{RelationshipType, Requirement, RequirementsStore};

    let child_rel = |parent: uuid::Uuid| Relationship {
        rel_type: RelationshipType::Child,
        target_id: parent,
        created_at: None,
        created_by: None,
    };
    let mut store = RequirementsStore::new();
    let mut parent = Requirement::new("epic".into(), String::new());
    parent.spec_id = Some("EPIC-1".into());
    let parent_id = parent.id;
    store.requirements.push(parent);

    let mut target = Requirement::new("target".into(), "## Acceptance\n\n- x\n".into());
    target.spec_id = Some("TASK-1".into());
    target.relationships.push(child_rel(parent_id));
    store.requirements.push(target);

    for i in 0..14 {
        let mut sib = Requirement::new(format!("sib {i}"), String::new());
        sib.spec_id = Some(format!("TASK-{}", 100 + i));
        sib.relationships.push(child_rel(parent_id));
        store.requirements.push(sib);
    }

    let target = store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some("TASK-1"))
        .unwrap();
    let (prompt, _) = assemble_ultraplan_prompt(&store, target, None, true, &[]);
    assert!(prompt.contains("Siblings (share a parent — 14 total)"));
    assert!(prompt.contains("more siblings omitted"));
    assert!(prompt.contains("Parent: EPIC-1"));
}

/// TASK-247: the ultraplan prompt pulls the spec's enrichment
/// comments into a `## Comments` section (most recent first), and
/// `--no-comments` (`include_comments = false`) omits it.
// trace:TASK-247 | ai:claude
#[test]
fn ultraplan_prompt_includes_comments() {
    use aida_core::models::Comment;
    use aida_core::{Requirement, RequirementsStore};

    let mut store = RequirementsStore::new();
    let mut req = Requirement::new("build it".into(), "## Acceptance\n\n- x\n".into());
    req.spec_id = Some("EPIC-9".into());
    let mut old = Comment::new("joe".into(), "first thought".into());
    old.created_at = chrono::Utc::now() - chrono::Duration::hours(2);
    let recent = Comment::new("joe".into(), "design fork: pick approach B".into());
    req.comments.push(old);
    req.comments.push(recent);
    store.requirements.push(req);
    let target = &store.requirements[0];

    // Default: comments included, most recent first.
    let (with, _) = assemble_ultraplan_prompt(&store, target, None, true, &[]);
    assert!(with.contains("## Comments"));
    assert!(with.contains("design fork: pick approach B"));
    assert!(with.contains("first thought"));
    let recent_at = with.find("design fork").unwrap();
    let old_at = with.find("first thought").unwrap();
    assert!(recent_at < old_at, "comments render most-recent-first");

    // `--no-comments` → the section is omitted entirely.
    let (without, _) = assemble_ultraplan_prompt(&store, target, None, false, &[]);
    assert!(!without.contains("## Comments"));
    assert!(!without.contains("design fork"));
}

/// TASK-517: `/ultraplan` includes project-reserved namespaces so plans
/// avoid colliding with generated or convention-owned paths.
#[test]
fn ultraplan_prompt_includes_reserved_namespaces() {
    use aida_core::{Requirement, RequirementsStore};

    let mut store = RequirementsStore::new();
    let mut req = Requirement::new(
        "avoid docs collision".into(),
        "Plan proposes docs/aida/new-guide.md.\n\n## Acceptance\n\n- x\n".into(),
    );
    req.spec_id = Some("TASK-517".into());
    store.requirements.push(req);
    let reservations = vec![ReservedPath {
        path: "docs/aida/".into(),
        reason: "reserved by `aida docs build` for requirement-layer projection".into(),
    }];

    let (prompt, _) =
        assemble_ultraplan_prompt(&store, &store.requirements[0], None, true, &reservations);

    assert!(prompt.contains("## Reserved namespaces and conventions"));
    assert!(prompt.contains("`docs/aida/`"));
    assert!(prompt.contains("reserved by `aida docs build`"));
}

#[test]
fn reserved_paths_file_parses_reservations_array() {
    let parsed: ReservedPathsFile = toml::from_str(
        r#"
[[reservations]]
path = "docs/aida/"
reason = "reserved by docs build"
"#,
    )
    .unwrap();

    assert_eq!(
        parsed.reservations,
        vec![ReservedPath {
            path: "docs/aida/".into(),
            reason: "reserved by docs build".into()
        }]
    );
}

/// TASK-247: an empty comment list produces no `## Comments` section
// (and no stray heading). trace:TASK-247 | ai:claude
#[test]
fn ultraplan_comments_section_empty_is_none() {
    assert!(ultraplan_comments_section(&[]).is_none());
}

/// TASK-514: test --copy flag is accepted and conflicts with --stdout and --json
// trace:TASK-514 | ai:antigravity
#[test]
fn test_ultraplan_copy_flag() {
    use crate::cli::{Cli, Command};
    use clap::Parser;

    // No copy flag
    let cli = Cli::try_parse_from(["aida", "ultraplan", "TASK-1"]).unwrap();
    if let Command::Ultraplan {
        spec,
        stdout,
        json,
        copy,
        ..
    } = cli.command
    {
        assert_eq!(spec, "TASK-1");
        assert!(!stdout);
        assert!(!json);
        assert!(!copy);
    } else {
        panic!("expected Ultraplan command");
    }

    // With copy flag
    let cli = Cli::try_parse_from(["aida", "ultraplan", "TASK-1", "--copy"]).unwrap();
    if let Command::Ultraplan {
        spec,
        stdout,
        json,
        copy,
        ..
    } = cli.command
    {
        assert_eq!(spec, "TASK-1");
        assert!(!stdout);
        assert!(!json);
        assert!(copy);
    } else {
        panic!("expected Ultraplan command");
    }

    // Copy and stdout conflicts
    assert!(Cli::try_parse_from(["aida", "ultraplan", "TASK-1", "--copy", "--stdout"]).is_err());

    // Copy and json conflicts
    assert!(Cli::try_parse_from(["aida", "ultraplan", "TASK-1", "--copy", "--json"]).is_err());
}

/// TASK-516: the `aida queue work` warn-decision fires exactly when the
/// spec carries `plan-review:pending`, and stays silent otherwise.
/// Fully isolated — feeds tag sets straight into the pure decision
// function, no store / FS / process. trace:TASK-516 | ai:claude
#[test]
fn plan_review_warning_fires_only_on_pending_tag() {
    use std::collections::HashSet;

    // Pending tag present → warns, and the message names the spec + the
    // remediation (clear the tag).
    let mut pending: HashSet<String> = HashSet::new();
    pending.insert("plan-review:pending".to_string());
    pending.insert("batch:foo".to_string());
    let warn = plan_review_warning(&pending, "TASK-516");
    assert!(warn.is_some(), "pending tag must warn");
    let msg = warn.unwrap();
    assert!(msg.contains("TASK-516"));
    assert!(msg.contains("plan-review:pending"));
    assert!(msg.contains("--remove-tag"));

    // No pending tag (even with other tags) → silent.
    let mut other: HashSet<String> = HashSet::new();
    other.insert("batch:foo".to_string());
    other.insert("from-advisor:observation".to_string());
    assert!(
        plan_review_warning(&other, "TASK-516").is_none(),
        "absent pending tag must not warn"
    );

    // Empty tag set → silent.
    assert!(plan_review_warning(&HashSet::new(), "TASK-516").is_none());

    // A near-miss tag that merely contains the prefix as a substring of
    // a DIFFERENT tag must NOT trip the warning (exact-membership test).
    let mut near: HashSet<String> = HashSet::new();
    near.insert("plan-review:pending-later".to_string());
    assert!(
        plan_review_warning(&near, "TASK-516").is_none(),
        "substring near-miss must not warn"
    );
}

/// TASK-516: the CLI exposes `import-plan` with the `--request-review`
// flag and an optional `--spec`. trace:TASK-516 | ai:claude
#[test]
fn import_plan_flag_parses() {
    use crate::cli::{Cli, Command};
    use clap::Parser;

    let cli = Cli::try_parse_from(["aida", "import-plan", "plan.md"]).unwrap();
    if let Command::ImportPlan {
        file,
        spec,
        request_review,
    } = cli.command
    {
        assert_eq!(file, "plan.md");
        assert_eq!(spec, None);
        assert!(!request_review);
    } else {
        panic!("expected ImportPlan command");
    }

    let cli = Cli::try_parse_from([
        "aida",
        "import-plan",
        "plan.md",
        "--spec",
        "TASK-516",
        "--request-review",
    ])
    .unwrap();
    if let Command::ImportPlan {
        file,
        spec,
        request_review,
    } = cli.command
    {
        assert_eq!(file, "plan.md");
        assert_eq!(spec.as_deref(), Some("TASK-516"));
        assert!(request_review);
    } else {
        panic!("expected ImportPlan command");
    }
}

/// STORY-306: the punt payload carries the spec's identity + the fork
/// question from the `AttentionReason`, and embeds the ultraplan-grade
/// context brief so a fresh advisor has the full spec context.
#[test]
fn assemble_punt_payload_includes_spec_and_fork() {
    use aida_core::{AttentionReason, PuntCategory, Requirement, RequirementsStore};

    let mut store = RequirementsStore::new();
    let mut spec = Requirement::new(
        "Add a --json flag".into(),
        "Intro.\n\n## Acceptance\n\n- [ ] emits JSON\n".into(),
    );
    spec.spec_id = Some("TASK-9".into());
    store.requirements.push(spec);
    let attention = AttentionReason {
        category: PuntCategory::DesignFork,
        detail: "flag name --json vs --format json — no recorded convention".into(),
        lean: Some("--json bool".into()),
        raised_by: Some("implementer".into()),
        raised_at: chrono::Utc::now(),
    };
    let dir = tempfile::tempdir().unwrap();
    let payload = assemble_punt_payload(&store, &store.requirements[0], dir.path(), &attention);
    assert_eq!(payload.spec, "TASK-9");
    assert_eq!(payload.category, PuntCategory::DesignFork);
    assert!(
        payload.question.contains("--json vs --format"),
        "{}",
        payload.question
    );
    assert_eq!(payload.lean.as_deref(), Some("--json bool"));
    // The context brief embeds the ultraplan-grade spec context.
    assert!(
        payload.context_markdown.contains("TASK-9"),
        "{}",
        payload.context_markdown
    );
    assert!(payload.context_markdown.contains("emits JSON"));
}

/// BUG-102: subject scanner picks up the trailing `(#N)` squash-merge
/// suffix so the auto-bump pass can match review stories filed against
// that PR. trace:BUG-102 | ai:claude
#[test]
fn extract_pr_number_from_commit_subject_shapes() {
    // Plain squash-merge subject from `gh pr merge --squash`.
    let s = "EPIC-23 batch 6: observability — queue progress, status (#26)";
    assert_eq!(extract_pr_number_from_commit_subject(s), Some(26));
    // Conventional + spec ids + `(#N)` (the canonical AIDA shape).
    let s = "[AI:claude] feat(queue): rework verb (TASK-218) (#24)";
    assert_eq!(extract_pr_number_from_commit_subject(s), Some(24));
    // Multi-spec + (#N).
    let s = "[AI:claude] feat(session): polish (STORY-98 STORY-90 BUG-74) (#10)";
    assert_eq!(extract_pr_number_from_commit_subject(s), Some(10));
    // No (#N) trailer — non-PR commit or pushed-to-main commit.
    let s = "[AI:claude] feat(api): endpoint (FR-1-042)";
    assert_eq!(extract_pr_number_from_commit_subject(s), None);
    // `(#N)` only valid when it's the literal trailing group.
    let s = "release (#23) v1.2.3";
    assert_eq!(extract_pr_number_from_commit_subject(s), None);
    // Not a digit-only group.
    let s = "chore: (#foo)";
    assert_eq!(extract_pr_number_from_commit_subject(s), None);
    // Empty parens.
    let s = "chore: ()";
    assert_eq!(extract_pr_number_from_commit_subject(s), None);
    // Multi-line input — only subject (first non-blank line) is scanned.
    let s = "subject (#7)\n\nbody mentions (#9)\n";
    assert_eq!(extract_pr_number_from_commit_subject(s), Some(7));
}

/// BUG-245: `pick_credited_spec` precedence. Given the PR's commit
/// subjects (one per line) and the dispatched id, returns:
///   - the dispatched id when it appears among the credits
///   - the first non-dispatched id otherwise
///   - `None` when no commit names any spec
//     trace:BUG-245 | ai:claude
#[test]
fn pick_credited_spec_returns_dispatched_when_credited() {
    // Single commit crediting the dispatched id — no mismatch.
    let subjects = "[AI:claude] feat(scope): description (STORY-276)";
    assert_eq!(
        pick_credited_spec(subjects, "STORY-276"),
        Some("STORY-276".to_string())
    );

    // Multi-commit PR carrying both the dispatched id and another —
    // dispatched wins, no mismatch fires.
    let subjects = "[AI:claude] fix(scope): unrelated (BUG-244)\n\
            [AI:claude] feat(scope): description (STORY-276)";
    assert_eq!(
        pick_credited_spec(subjects, "STORY-276"),
        Some("STORY-276".to_string())
    );
}

/// BUG-245: the observed case — PR-108 crediting BUG-244 while STORY-276
/// was dispatched. The dispatched id is absent from the credits, so the
// first non-dispatched id is returned. trace:BUG-245 | ai:claude
#[test]
fn pick_credited_spec_returns_other_when_dispatched_absent() {
    let subjects = "[AI:claude] fix(release): v0.8.0 blocker (BUG-244)";
    assert_eq!(
        pick_credited_spec(subjects, "STORY-276"),
        Some("BUG-244".to_string())
    );

    // First non-dispatched wins when the PR carries multiple unrelated
    // ids.
    let subjects = "[AI:claude] fix: a (TASK-77)\n[AI:claude] feat: b (BUG-244)";
    assert_eq!(
        pick_credited_spec(subjects, "STORY-276"),
        Some("TASK-77".to_string())
    );
}

/// BUG-245: `None` when the PR's commits credit no spec at all — the
/// orchestrator treats this as "cannot determine", which preserves the
// pre-BUG-245 dispatched-id credit. trace:BUG-245 | ai:claude
#[test]
fn pick_credited_spec_returns_none_when_no_spec_credited() {
    // No `(SPEC-ID)` trailers anywhere.
    let subjects = "chore: bump dep version\nrefactor: rename helper";
    assert_eq!(pick_credited_spec(subjects, "STORY-276"), None);
    // Empty PR — defensive, shouldn't happen in practice.
    assert_eq!(pick_credited_spec("", "STORY-276"), None);
}

/// BUG-357: a reconcile rescue may only trust a merged PR when the PR
/// metadata credits the dispatched spec. An unrelated merged PR on a
/// misattributed branch must not turn a phase-1 failure into success.
#[test]
fn classify_pr_credit_rejects_unrelated_merged_pr_metadata() {
    assert_eq!(
        classify_pr_credit(
            "[AI:codex] feat(brief): agent briefs (TASK-492)",
            None,
            "TASK-488"
        ),
        PrCreditMatch::Other("TASK-492".to_string())
    );
}

#[test]
fn classify_pr_credit_accepts_dispatched_commit_or_title_trailer() {
    assert_eq!(
        classify_pr_credit(
            "[AI:codex] fix(orchestrator): guard reconcile (BUG-357)",
            None,
            "BUG-357"
        ),
        PrCreditMatch::Dispatched
    );
    assert_eq!(
        classify_pr_credit(
            "chore: branch commit without trailer",
            Some("[AI:codex] fix(orchestrator): guard reconcile (BUG-357)"),
            "BUG-357",
        ),
        PrCreditMatch::Dispatched
    );
    assert_eq!(
        classify_pr_credit(
            "[AI:codex] fix(orchestrator): guard reconcile (BUG-357)",
            Some("[AI:codex] docs: unrelated title (TASK-492)"),
            "BUG-357",
        ),
        PrCreditMatch::Dispatched
    );
}

/// BUG-102: title-to-PR parser used to match Done review stories
// against just-merged PR numbers. trace:BUG-102 | ai:claude
#[test]
fn parse_review_story_pr_number_shapes() {
    let t = "Review PR-26: EPIC-23 batch 6: observability — 6 specs";
    assert_eq!(parse_review_story_pr_number(t), Some(26));
    let t = "Review PR-1: small";
    assert_eq!(parse_review_story_pr_number(t), Some(1));
    // Non-review-story titles return None.
    assert_eq!(parse_review_story_pr_number("Add OAuth provider"), None);
    // Prefix-but-no-colon must NOT match (avoids prose like "Review PR-26 mention").
    assert_eq!(
        parse_review_story_pr_number("Review PR-26 mention in docs"),
        None
    );
    // Non-numeric.
    assert_eq!(parse_review_story_pr_number("Review PR-abc: x"), None);
}

/// BUG-85: end-to-end shape of the PR-14 over-counting incident. The
/// auto-queue extractor walked a multi-commit range and incorrectly
/// added body-referenced spec IDs to the "covers" list. After this fix,
/// delivered = subject parens-suffix only; referenced = body content,
// disjoint. trace:BUG-85 | ai:claude
#[test]
fn bug_85_delivered_vs_referenced_disjoint() {
    // Simulated 6-commit PR + 1 commit with body refs to non-delivered specs.
    let messages = [
        "[AI:claude] fix(queue,session,role): prefer agreed_id (BUG-83)\n\n\
                Several render paths that share the same data weren't part of that sweep:\n\
                - aida role enter \"Queued for this role\" section (TASK-48)\n\
                - aida role enter \"Last touched\" section (TASK-49)\n\
                - aida session show --plan manifest table (STORY-98)\n",
        "[AI:claude] feat(db): aida db check --collisions audit (TASK-80)\n",
        "[AI:claude] fix(hooks): commit-msg accepts comma-separated scopes (BUG-84)\n",
        "[AI:claude] fix(release): non-interactive --yes support (TASK-79)\n",
        "[AI:claude] feat(scaffold): pre-allow aida-family bash (TASK-82)\n",
        "[AI:claude] docs(session,queue): surface 'auto' permission mode (TASK-83)\n",
    ];
    let mut delivered: Vec<String> = Vec::new();
    let mut referenced: Vec<String> = Vec::new();
    for msg in &messages {
        for id in extract_spec_ids_from_commit(msg) {
            if !delivered.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                delivered.push(id);
            }
        }
        for id in extract_referenced_spec_ids_from_commit(msg) {
            if !referenced.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                referenced.push(id);
            }
        }
    }
    referenced.retain(|r| !delivered.iter().any(|d| d.eq_ignore_ascii_case(r)));

    assert_eq!(
        delivered,
        vec!["BUG-83", "TASK-80", "BUG-84", "TASK-79", "TASK-82", "TASK-83"]
    );
    assert_eq!(referenced, vec!["TASK-48", "TASK-49", "STORY-98"]);
}

/// BUG-85: referenced extractor excludes IDs already delivered in the
// same commit's subject. trace:BUG-85 | ai:claude
#[test]
fn referenced_specs_disjoint_from_delivered_within_one_commit() {
    // BUG-83 appears in BOTH the subject paren and the body; it's
    // delivered, so it must NOT appear in referenced.
    let msg = "fix(scope): description (BUG-83)\n\n\
            - cleanup pass on (BUG-83)\n\
            - touches the same area as (TASK-48)\n";
    assert_eq!(
        extract_spec_ids_from_commit(msg),
        vec!["BUG-83".to_string()]
    );
    assert_eq!(
        extract_referenced_spec_ids_from_commit(msg),
        vec!["TASK-48".to_string()]
    );
}

/// BUG-412: a `(PREFIX-NNN)` literal inside a pasted code snippet in the
/// body must NOT be mined as a referenced spec, while a genuine prose/bullet
// reference still is. trace:BUG-412 | ai:claude
#[test]
fn referenced_specs_skip_code_like_body_lines() {
    let msg = "feat(x): real work (TASK-1)\n\n\
            - genuinely references the same area as (TASK-48)\n\
            enum Status { Ok = (STATUS-200) }\n\
            let x = compute(CODE-42);\n";
    let refs = extract_referenced_spec_ids_from_commit(msg);
    assert!(
        refs.contains(&"TASK-48".to_string()),
        "prose ref kept: {refs:?}"
    );
    assert!(
        !refs
            .iter()
            .any(|r| r.starts_with("STATUS-") || r == "CODE-42"),
        "code-snippet literals must be skipped: {refs:?}"
    );
}

/// BUG-536: a squash-merge of an umbrella PR names only ONE spec in its
/// subject trailer, but GitHub's default squash body concatenates every
/// constituent commit's message — each with its own `(SPEC-ID)` completion
/// trailer. The auto-bump / reconcile candidate scan must read that body
/// (gated on a `(#N)` squash/merge subject) so the folded child specs
/// complete instead of stranding. Mirrors the exact composition the scan
/// now performs: subject trailers ∪ (squash-gated) body trailers.
// trace:BUG-536 | ai:claude
#[test]
fn squash_body_yields_folded_child_completion_trailers() {
    // Realistic GitHub squash body (the #832 shape): subject trailers
    // STORY-585, body bullets carry STORY-585 again AND the folded
    // BUG-525 fix that single-trailer squash-merges strand.
    let msg = "[AI:claude] feat(mailbox): surface unread mail (STORY-585) (#832)\n\n\
            * [AI:claude] feat(mailbox): surface unread mail (STORY-585)\n\n\
            The read/notice half of the inter-agent mailbox.\n\n\
            * [AI:claude] fix(show): char-boundary-safe prefix check (BUG-525)\n\n\
            format_review_story_display byte-sliced the title.\n";

    // Reproduce the scan's candidate collection: subject trailers always,
    // plus body trailers when the subject is a `(#N)` squash/merge.
    let subject = msg.lines().find(|l| !l.trim().is_empty()).unwrap().trim();
    let mut ids: Vec<String> = extract_spec_ids_from_commit(subject);
    assert!(
        extract_pr_number_from_commit_subject(subject).is_some(),
        "subject must be recognised as a squash/merge (#N) commit"
    );
    for id in extract_referenced_spec_ids_from_commit(msg) {
        if !ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
            ids.push(id);
        }
    }

    assert!(
        ids.iter().any(|x| x == "STORY-585"),
        "lead spec from subject trailer present: {ids:?}"
    );
    assert!(
        ids.iter().any(|x| x == "BUG-525"),
        "folded child from squash body MUST be a completion candidate: {ids:?}"
    );

    // Gate check: the SAME body on a non-squash commit (no `(#N)` suffix)
    // must NOT contribute body trailers as completion candidates — only the
    // squash/merge body is a trusted concatenation of ship signals.
    let non_squash = "[AI:claude] feat(mailbox): surface unread mail (STORY-585)";
    assert!(
        extract_pr_number_from_commit_subject(non_squash).is_none(),
        "non-squash subject must not parse a (#N) PR number"
    );
}

/// BUG-606: the completed-without-commit corroboration scan must read FULL
/// messages, not subjects. A squash-merge keeps only the PR title as its
/// subject (no spec id) and folds each child's `(SPEC-ID)` trailer into the
/// body — a subject-only scan false-flagged every squash-merged Completed
// spec (~1464 false positives on this repo). trace:BUG-606 | ai:claude
#[test]
fn corroboration_scan_finds_squash_body_trailer() {
    // The exact shape that broke: subject is the PR title with NO spec id;
    // the corroborating `(TASK-216)` lives only in the concatenated body.
    let squash = "[AI:claude] feat(review): own-PR handling (#1042)\n\n\
            * [AI:claude] fix(aida-review): detect own-PR before request-changes API call (TASK-216)\n\n\
            mirroring the existing skip.\n";
    let refs = referenced_spec_ids_from_messages([squash]);
    assert!(
        refs.contains("TASK-216"),
        "body trailer must corroborate (no false 'no commit' flag): {refs:?}"
    );
}

#[test]
fn corroboration_scan_still_finds_subject_trailer() {
    let refs = referenced_spec_ids_from_messages(["[AI:claude] fix(z): thing (BUG-89)"]);
    assert!(
        refs.contains("BUG-89"),
        "subject trailer still works: {refs:?}"
    );
}

/// BUG-606: prose / punctuated / mid-line paren mentions must also
/// corroborate — `(BUG-109).` (trailing period) and `(ADR-3 / TASK-647),`
/// (mid-line, prose after) are real references a trailing-trailer-only
// extractor missed. trace:BUG-606 | ai:claude
#[test]
fn corroboration_scan_finds_prose_and_punctuated_mentions() {
    let msg = "feat(tui): shell (#144)\n\n\
            The empty shell previously showed only keybindings (BUG-109).\n\
            advisor-authority-gated (ADR-3 / TASK-647), so it cannot drain.\n";
    let refs = referenced_spec_ids_from_messages([msg]);
    for id in ["BUG-109", "ADR-3", "TASK-647"] {
        assert!(
            refs.contains(id),
            "{id} must corroborate from prose: {refs:?}"
        );
    }
}

/// Plan commits name PLANNED, not shipped, specs (BUG-426) — they must not
// corroborate a completion. trace:BUG-606 | ai:claude
#[test]
fn corroboration_scan_skips_plan_commits() {
    let refs = referenced_spec_ids_from_messages(["docs(plans): plan for the thing (STORY-999)"]);
    assert!(
        !refs.contains("STORY-999"),
        "plan-commit references are not completion evidence: {refs:?}"
    );
}

/// A bare `update SPEC-ID` subject is `aida edit` store bookkeeping — it
/// exists for every edited spec and must not corroborate (STORY-462),
/// else the liberal token match would mask every violation.
// trace:BUG-606 | ai:claude
#[test]
fn corroboration_scan_skips_store_bookkeeping_commits() {
    assert!(
        !referenced_spec_ids_from_messages(["update TASK-400"]).contains("TASK-400"),
        "bare 'update SPEC-ID' is store bookkeeping, not corroboration"
    );
    // But a real commit that merely starts with 'update' AND carries a
    // trailer still corroborates.
    assert!(
        referenced_spec_ids_from_messages(["update the parser (TASK-401)"]).contains("TASK-401"),
        "a real commit with a trailer must still corroborate"
    );
}

#[test]
fn body_line_is_code_like_classifies() {
    assert!(body_line_is_code_like("enum Status { Ok = (STATUS-200) }"));
    assert!(body_line_is_code_like("let x = foo();"));
    assert!(body_line_is_code_like("fn handle() {"));
    assert!(body_line_is_code_like("    pub struct Foo;"));
    assert!(body_line_is_code_like("a::b::c"));
    // Genuine reference lines are NOT code-like.
    assert!(!body_line_is_code_like(
        "- references (TASK-48) for context"
    ));
    assert!(!body_line_is_code_like("trace:STORY-7"));
    assert!(!body_line_is_code_like("This touches the (BUG-83) area."));
}

/// STORY-67: looks_like_spec_id validates the alpha-DASH-digits
/// shape used throughout AIDA.
// trace:STORY-67 | ai:claude
#[test]
fn spec_id_shape_recognition() {
    assert!(looks_like_spec_id("FR-42"));
    assert!(looks_like_spec_id("BUG-1-038"));
    assert!(looks_like_spec_id("EPIC-2"));
    assert!(looks_like_spec_id("STORY-100"));
    // Rejects.
    assert!(!looks_like_spec_id("v1.2.3"));
    assert!(!looks_like_spec_id("1.2"));
    assert!(!looks_like_spec_id("X-"));
    assert!(!looks_like_spec_id("X"));
    assert!(!looks_like_spec_id(""));
    // Lowercase prefix is permitted at this layer; commit subjects
    // typically uppercase, but `(fr-1)` shouldn't blow up if it
    // appears.
    assert!(looks_like_spec_id("fr-1"));
}

/// STORY-61: PR-N / MR-N scope parsing — case-insensitive, requires
/// the trailing number, and rejects everything else (so the normal
/// scope flow is preserved for non-review scopes like EPIC-20).
// trace:STORY-61 | ai:claude
#[test]
fn review_scope_parsing() {
    assert_eq!(parse_review_scope("PR-1"), Some((ReviewForge::GitHub, 1)));
    assert_eq!(parse_review_scope("pr-42"), Some((ReviewForge::GitHub, 42)));
    assert_eq!(parse_review_scope("MR-7"), Some((ReviewForge::GitLab, 7)));
    assert_eq!(
        parse_review_scope("mr-2024"),
        Some((ReviewForge::GitLab, 2024))
    );
    // Non-PR scopes pass through unchanged.
    assert_eq!(parse_review_scope("EPIC-20"), None);
    assert_eq!(parse_review_scope("FR-42"), None);
    assert_eq!(parse_review_scope("feature:auth"), None);
    // Missing number rejects.
    assert_eq!(parse_review_scope("PR-"), None);
    assert_eq!(parse_review_scope("MR-abc"), None);
}

/// STORY-61: refspec format — same-repo and fork PRs both work
/// because `pull/N/head` (GitHub) and `merge-requests/N/head`
/// (GitLab) are populated on origin in both cases.
// trace:STORY-61 | ai:claude
#[test]
fn review_forge_refspec() {
    assert_eq!(ReviewForge::GitHub.pr_head_ref(1), "pull/1/head");
    assert_eq!(ReviewForge::GitHub.pr_head_ref(123), "pull/123/head");
    assert_eq!(ReviewForge::GitLab.pr_head_ref(7), "merge-requests/7/head");
    assert_eq!(ReviewForge::GitHub.local_branch_for(1), "pr-1");
    assert_eq!(ReviewForge::GitLab.local_branch_for(7), "mr-7");
}

/// STORY-61: --forge string parsing accepts both the long form
/// (`github`/`gitlab`) and the CLI-tool short form (`gh`/`glab`)
/// since users usually have one or the other muscle-memory'd.
// trace:STORY-61 | ai:claude
#[test]
fn review_forge_override_parsing() {
    assert_eq!(ReviewForge::parse("github"), Some(ReviewForge::GitHub));
    assert_eq!(ReviewForge::parse("GitHub"), Some(ReviewForge::GitHub));
    assert_eq!(ReviewForge::parse("gh"), Some(ReviewForge::GitHub));
    assert_eq!(ReviewForge::parse("gitlab"), Some(ReviewForge::GitLab));
    assert_eq!(ReviewForge::parse("glab"), Some(ReviewForge::GitLab));
    assert_eq!(ReviewForge::parse("bitbucket"), None);
    assert_eq!(ReviewForge::parse(""), None);
}

/// STORY-57: routing-filter decision table for the consumer side.
/// Entries with `for_scope` only route to sessions whose lease scope
/// matches; entries with `for_session` only route to that exact lease
/// (8+ char prefix). No lease + scope-tagged entry → filtered. The
/// `--all` bypass (the boolean param) lets users see everything.
// trace:STORY-57 | ai:claude
#[test]
fn entry_scope_session_match_decision_table() {
    use aida_core::QueueEntry;
    let now = chrono::Utc::now();
    let mk = |scope: Option<&str>, sess: Option<&str>| QueueEntry {
        user_id: "u".into(),
        requirement_id: uuid::Uuid::nil(),
        position: 0,
        added_by: "u".into(),
        note: None,
        added_at: now,
        for_role: Some("implementer".into()),
        for_scope: scope.map(|s| s.to_string()),
        for_session: sess.map(|s| s.to_string()),
        added_by_machine: None,
    };
    let lease = SessionLease {
        id: "abcdef123456".into(),
        scope: "EPIC-20".into(),
        slug: "epic-20".into(),
        owner: "u".into(),
        worktree_path: std::path::PathBuf::from("/tmp/wt"),
        branch: "br".into(),
        started_at: now,
        hostname: "h".into(),
        role: None,
        creator_pid: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    };

    // No routing tags = visible everywhere.
    assert!(entry_scope_session_match(
        &mk(None, None),
        Some(&lease),
        false
    ));
    assert!(entry_scope_session_match(&mk(None, None), None, false));

    // Scope match passes; mismatch filters out.
    assert!(entry_scope_session_match(
        &mk(Some("EPIC-20"), None),
        Some(&lease),
        false
    ));
    assert!(!entry_scope_session_match(
        &mk(Some("OTHER"), None),
        Some(&lease),
        false
    ));
    // BUG-89: Scope-tagged entry without a lease → VISIBLE. The viewer
    // is asking from outside any session (no lease), so the implicit
    // scope filter does not apply — all routed items are shown. Inside
    // a session, scope filtering still kicks in (covered above).
    // trace:BUG-89 | ai:claude
    assert!(entry_scope_session_match(
        &mk(Some("EPIC-20"), None),
        None,
        false
    ));

    // Session prefix matches case-insensitively.
    assert!(entry_scope_session_match(
        &mk(None, Some("abcdef12")),
        Some(&lease),
        false
    ));
    assert!(entry_scope_session_match(
        &mk(None, Some("ABCDEF12")),
        Some(&lease),
        false
    ));
    // Wrong session prefix is filtered (only when there IS a lease to
    // compare against). Without a lease, see the BUG-89 case below.
    assert!(!entry_scope_session_match(
        &mk(None, Some("99999999")),
        Some(&lease),
        false
    ));
    // BUG-89: Session-tagged entry without a lease → VISIBLE. Same
    // rationale as the scope case above. trace:BUG-89 | ai:claude
    assert!(entry_scope_session_match(
        &mk(None, Some("abcdef12")),
        None,
        false
    ));

    // Bypass shows everything regardless.
    assert!(entry_scope_session_match(
        &mk(Some("OTHER"), None),
        Some(&lease),
        true
    ));
    assert!(entry_scope_session_match(
        &mk(Some("EPIC-20"), Some("99999999")),
        None,
        true
    ));
}

/// STORY-56: aggregating a session log into the project role keeps
/// only the newest entry per spec_id, merges in front of the role's
/// existing activity, and respects ACTIVITY_MAX. The session's
/// per-spec winners survive even when the project role already had
/// older entries for the same specs (the session entry is fresher,
/// so it wins).
// trace:STORY-56 | ai:claude
#[test]
fn session_aggregation_dedupes_and_promotes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    std::fs::create_dir_all(root.join(".aida/roles")).unwrap();

    // Seed a project role with one stale entry for STORY-X.
    let role_path = root.join(".aida/roles/implementer.toml");
    let stale = chrono::Utc::now() - chrono::Duration::hours(1);
    let role = RoleState {
        name: "implementer".into(),
        purpose: None,
        created_at: stale,
        last_active_at: stale,
        working_directory: None,
        notes: None,
        global: false,
        activity: vec![RoleActivity {
            spec_id: "STORY-X".into(),
            action: "edit".into(),
            at: stale,
        }],
        scope_tags: vec![],
        scope_status: None,
        system_prompt: None,
    };
    std::fs::write(&role_path, toml::to_string_pretty(&role).unwrap()).unwrap();

    // Session log: STORY-X (newer than the seed) and STORY-Y.
    let id = "agg-session-01";
    append_session_activity(root, id, "implementer", "STORY-X", "show").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    append_session_activity(root, id, "implementer", "STORY-Y", "edit").unwrap();

    aggregate_session_activity_into_roles(root, id);

    let merged: RoleState = toml::from_str(&std::fs::read_to_string(&role_path).unwrap()).unwrap();
    let ids: Vec<&str> = merged.activity.iter().map(|a| a.spec_id.as_str()).collect();
    // STORY-Y (newest in session) first, then STORY-X (the session
    // entry wins over the seed; the seed's stale STORY-X is dropped).
    assert_eq!(ids, vec!["STORY-Y", "STORY-X"]);
    assert!(
        merged.activity[1].at > stale,
        "session-promoted STORY-X must carry the session timestamp, not the stale seed"
    );
}

/// STORY-52: detect_cargo_target_dir returns Some when target/ exists
/// and None otherwise — the latter case is the "not a Rust project /
/// never built" path that should silently skip env-shim generation.
// trace:STORY-52 | ai:claude
#[test]
fn detect_cargo_target_dir_only_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert_eq!(detect_cargo_target_dir(root), None);

    std::fs::create_dir_all(root.join("target")).unwrap();
    let got = detect_cargo_target_dir(root).expect("target/ exists");
    // Result is canonicalized, so just assert it points at the dir we made.
    assert_eq!(
        got.canonicalize().unwrap(),
        root.join("target").canonicalize().unwrap()
    );

    // A regular file named `target` doesn't count.
    let tmp2 = tempfile::tempdir().unwrap();
    std::fs::write(tmp2.path().join("target"), b"not a dir").unwrap();
    assert_eq!(detect_cargo_target_dir(tmp2.path()), None);
}

/// STORY-52: render_session_env_file emits a sourceable export with
/// the path POSIX-quoted so spaces or apostrophes in the parent path
/// don't break the shell.
// trace:STORY-52 | ai:claude
#[test]
fn render_session_env_file_quotes_path() {
    let body = render_session_env_file(std::path::Path::new("/tmp/aida/target"), None);
    assert!(body.contains("export CARGO_TARGET_DIR='/tmp/aida/target'"));
    assert!(body.starts_with("# Generated by `aida session start`"));

    // Apostrophe in the path → close-reopen escape.
    let body = render_session_env_file(std::path::Path::new("/tmp/joe's repo/target"), None);
    assert!(body.contains("export CARGO_TARGET_DIR='/tmp/joe'\\''s repo/target'"));

    let body = render_session_env_file(std::path::Path::new("/tmp/aida/target"), Some("codex"));
    assert!(body.contains("export AIDA_AGENT_TYPE='codex'"));
}

/// STORY-52: write_session_env_file writes `.aida/session-env.sh` under
/// the worktree, creating `.aida/` if it doesn't exist yet (the symlink
/// pass in session_start sometimes runs before this, sometimes the dir
/// is fresh — either way it should land in place).
// trace:STORY-52 | ai:claude
#[test]
fn write_session_env_file_creates_aida_dir_if_needed() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path();
    write_session_env_file(worktree, std::path::Path::new("/tmp/parent/target")).unwrap();
    let written = std::fs::read_to_string(worktree.join(".aida").join("session-env.sh")).unwrap();
    assert!(written.contains("CARGO_TARGET_DIR='/tmp/parent/target'"));
}

/// TASK-63: parse_session_env handles the shape we write today.
/// Cover the round-trip with render_session_env_file (the source of
/// truth for what we produce) so a future change to the shim format
// has to update both sides. trace:TASK-63 | ai:claude
#[test]
fn parse_session_env_roundtrips_with_render() {
    let body = render_session_env_file(std::path::Path::new("/tmp/parent/target"), None);
    let pairs = parse_session_env(&body);
    assert_eq!(pairs.len(), 1, "{:?}", pairs);
    assert_eq!(pairs[0].0, "CARGO_TARGET_DIR");
    assert_eq!(pairs[0].1, "/tmp/parent/target");
}

/// TASK-63: apostrophe in the target path → close-reopen escape on
/// the way out, must unescape cleanly on the way back in.
// trace:TASK-63 | ai:claude
#[test]
fn parse_session_env_unquotes_close_reopen_escape() {
    let body = render_session_env_file(std::path::Path::new("/tmp/joe's repo/target"), None);
    let pairs = parse_session_env(&body);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].1, "/tmp/joe's repo/target");
}

/// TASK-63: ignore comments, blanks, non-export lines, malformed
/// names. The shim is gitignored runtime state but a user might
/// hand-edit it (debug session) — don't poison the env if they do.
// trace:TASK-63 | ai:claude
#[test]
fn parse_session_env_skips_noise() {
    let body = "\
# header comment\n\
\n\
export CARGO_TARGET_DIR='/x'\n\
not an export line\n\
export 1BAD=value\n\
export GOOD_VAR='hello'\n\
export NO_EQUALS\n\
   export INDENTED='trimmed'\n\
";
    let pairs = parse_session_env(body);
    let names: Vec<&str> = pairs.iter().map(|p| p.0.as_str()).collect();
    assert_eq!(names, vec!["CARGO_TARGET_DIR", "GOOD_VAR", "INDENTED"]);
    assert_eq!(pairs[2].1, "trimmed");
}

/// TASK-63: bare (unquoted) values pass through unchanged — the
/// parser is forgiving so a hand-written `export FOO=bar` works.
// trace:TASK-63 | ai:claude
#[test]
fn parse_session_env_handles_unquoted_value() {
    let pairs = parse_session_env("export FOO=bar\n");
    assert_eq!(pairs, vec![("FOO".to_string(), "bar".to_string())]);
}

/// TASK-63: apply_session_env_to_process really mutates the process
/// env, and returns the names it set. Use a name unique to this test
/// so parallel test runs don't trample each other.
// trace:TASK-63 | ai:claude
#[test]
fn apply_session_env_to_process_sets_env() {
    const VAR: &str = "AIDA_TEST_TASK_63_APPLIED";
    // SAFETY: scoped to this test; not racing with anything that
    // reads VAR.
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var(VAR);
    }
    let body = format!("export {}='hello world'\n", VAR);
    let applied = apply_session_env_to_process(&body);
    assert_eq!(applied, vec![VAR.to_string()]);
    assert_eq!(std::env::var(VAR).unwrap(), "hello world");
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var(VAR);
    }
}

/// STORY-52: leases predating the cargo_target_dir field must still
/// deserialize cleanly so an old session can be ended after upgrading
/// aida. `#[serde(default)]` handles this; this test pins the contract.
// trace:STORY-52 | ai:claude
#[test]
fn lease_without_cargo_target_dir_deserializes() {
    let toml_text = r#"
id = "abcdef123456"
scope = "EPIC-20"
slug = "epic-20"
owner = "u"
worktree_path = "/tmp/wt"
branch = "br"
started_at = "2026-05-04T00:00:00Z"
hostname = "h"
"#;
    let lease: SessionLease = toml::from_str(toml_text).unwrap();
    assert_eq!(lease.id, "abcdef123456");
    assert!(lease.cargo_target_dir.is_none());
    // STORY-58 field carries forward the same back-compat contract.
    assert!(lease.parent_project_root.is_none());
}

/// STORY-58: from inside a worktree covered by a lease that records a
/// parent, the helper returns that parent. Models the on-disk layout
/// session_start produces (lease lives at <root>/.aida/sessions/) so
/// we exercise the actual lookup path.
// trace:STORY-58 | ai:claude
#[test]
fn parent_project_root_for_session_returns_recorded_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let parent_dir = root.join("aida");
    let worktree = root.join("aida-epic-20");
    std::fs::create_dir_all(&parent_dir).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let leases = worktree.join(".aida").join("sessions");
    std::fs::create_dir_all(&leases).unwrap();

    let lease = SessionLease {
        id: "abcdef123456".into(),
        scope: "EPIC-20".into(),
        slug: "epic-20".into(),
        owner: "u".into(),
        worktree_path: worktree.canonicalize().unwrap(),
        branch: "epic-20".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: None,
        creator_pid: None,
        cargo_target_dir: None,
        parent_project_root: Some(parent_dir.canonicalize().unwrap()),
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    };
    std::fs::write(
        leases.join("abcdef123456.toml"),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();

    let got = parent_project_root_for_session(&worktree).expect("active lease w/ parent");
    assert_eq!(got, parent_dir.canonicalize().unwrap());
}

/// STORY-58: pre-STORY-58 leases (no parent recorded) return None
/// even when the cwd is squarely inside the lease's worktree, so the
/// list path falls back to the classic single-group output.
// trace:STORY-58 | ai:claude
#[test]
fn parent_project_root_for_session_none_for_legacy_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let leases = worktree.join(".aida").join("sessions");
    std::fs::create_dir_all(&leases).unwrap();

    // Old-format lease: no parent_project_root field.
    let toml_text = format!(
        r#"
id = "legacylease01"
scope = "EPIC-20"
slug = "epic-20"
owner = "u"
worktree_path = "{}"
branch = "br"
started_at = "2026-05-04T00:00:00Z"
hostname = "h"
"#,
        worktree.canonicalize().unwrap().display()
    );
    std::fs::write(leases.join("legacylease01.toml"), toml_text).unwrap();

    assert!(parent_project_root_for_session(&worktree).is_none());
}

/// STORY-58: outside a session worktree (no lease covers cwd), the
/// helper returns None — list stays single-group as today.
// trace:STORY-58 | ai:claude
#[test]
fn parent_project_root_for_session_none_when_no_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("not-a-session");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".aida").join("sessions")).unwrap();
    assert!(parent_project_root_for_session(&dir).is_none());
}

/// STORY-71: parsing the JSON shape `gh pr view --json
/// headRefOid,baseRefOid,baseRefName` returns. Pinned so a future
/// refactor of the field list can't silently break the lease enrichment.
// trace:STORY-71 | ai:claude
#[test]
fn parse_pr_metadata_json_github_shape() {
    let body = serde_json::json!({
        "headRefOid": "deadbeefcafe1234567890abcdef1234567890ab",
        "baseRefOid": "0011223344556677889900112233445566778899",
        "baseRefName": "main",
    });
    let m = parse_pr_metadata_json(ReviewForge::GitHub, &body);
    assert_eq!(
        m.head_sha.as_deref(),
        Some("deadbeefcafe1234567890abcdef1234567890ab")
    );
    assert_eq!(
        m.base_sha.as_deref(),
        Some("0011223344556677889900112233445566778899")
    );
    assert_eq!(m.base_ref.as_deref(), Some("main"));
}

/// STORY-71: glab's `mr view --output json` mirrors the GitLab REST
/// API — head SHA in `sha`, base SHA in `diff_refs.base_sha`, base
/// ref in `target_branch`. Test pins all three lookups.
// trace:STORY-71 | ai:claude
#[test]
fn parse_pr_metadata_json_gitlab_shape() {
    let body = serde_json::json!({
        "sha": "1111222233334444555566667777888899990000",
        "diff_refs": {
            "base_sha": "aaaabbbbccccddddeeeeffff0000111122223333",
            "head_sha": "1111222233334444555566667777888899990000",
        },
        "target_branch": "develop",
    });
    let m = parse_pr_metadata_json(ReviewForge::GitLab, &body);
    assert_eq!(
        m.head_sha.as_deref(),
        Some("1111222233334444555566667777888899990000")
    );
    assert_eq!(
        m.base_sha.as_deref(),
        Some("aaaabbbbccccddddeeeeffff0000111122223333")
    );
    assert_eq!(m.base_ref.as_deref(), Some("develop"));
}

/// STORY-71: missing or empty fields drop through as None rather than
/// pinning empty strings into the lease. Forwards-compat for forge
/// CLIs that omit some keys (auth scope, schema drift, etc.).
// trace:STORY-71 | ai:claude
#[test]
fn parse_pr_metadata_json_missing_fields_yield_none() {
    let body = serde_json::json!({ "headRefOid": "" });
    let m = parse_pr_metadata_json(ReviewForge::GitHub, &body);
    assert!(m.head_sha.is_none());
    assert!(m.base_sha.is_none());
    assert!(m.base_ref.is_none());
}

/// STORY-71: leases written before the new PR fields existed must
/// still deserialize cleanly (so an in-flight session survives an
/// aida upgrade). Test pins the back-compat contract.
// trace:STORY-71 | ai:claude
#[test]
fn lease_without_pr_fields_deserializes() {
    let toml_text = r#"
id = "abcdef123456"
scope = "PR-3"
slug = "pr-3"
owner = "u"
worktree_path = "/tmp/wt"
branch = "pr-3"
started_at = "2026-05-04T00:00:00Z"
hostname = "h"
"#;
    let lease: SessionLease = toml::from_str(toml_text).unwrap();
    assert!(lease.pr_head_sha.is_none());
    assert!(lease.pr_base_sha.is_none());
    assert!(lease.pr_base_ref.is_none());
}

// --- `aida skill lint` plan-ref extraction (TASK-927). ---

/// A real plan path — bare, in a markdown link, or in backticks — is
/// extracted; duplicates collapse; order is first-seen.
// trace:TASK-927 | ai:claude
#[test]
fn skill_lint_extracts_real_plan_refs() {
    let body = "\
See the plan at docs/plans/2026-06-01-foo.md for details.
Also [the design](docs/plans/2026-06-01-foo.md) and `docs/plans/_TEMPLATE.md`.
";
    let refs = extract_plan_refs(body);
    assert_eq!(
        refs,
        vec![
            "docs/plans/2026-06-01-foo.md".to_string(),
            "docs/plans/_TEMPLATE.md".to_string(),
        ]
    );
}

/// Illustrative placeholders — glob, ellipsis, angle-bracket template
/// markers — must NOT be treated as real plan refs.
// trace:TASK-927 | ai:claude
#[test]
fn skill_lint_skips_placeholder_refs() {
    let body = "\
- **Plan docs**: docs/plans/*.md (skip these)
  Plan:    <docs/plans/...md>   |  none
historical: docs/plans/...md
";
    assert!(extract_plan_refs(body).is_empty());
}

/// A trailing sentence period is trimmed, and the bare `docs/plans/`
/// prefix with nothing after it is not a ref.
// trace:TASK-927 | ai:claude
#[test]
fn skill_lint_trims_period_and_ignores_bare_prefix() {
    let body = "Implemented per docs/plans/2026-06-01-foo.md. The dir is docs/plans/.";
    assert_eq!(
        extract_plan_refs(body),
        vec!["docs/plans/2026-06-01-foo.md".to_string()]
    );
}

/// The glyph counter is derived from the registry, so a body with no
/// registry glyphs scores 0 and one with several counts them all.
// trace:TASK-927 | ai:claude
#[test]
fn skill_lint_counts_registry_glyphs() {
    assert_eq!(count_registry_glyphs("plain ascii text, no glyphs"), 0);
    // Build the glyph string from the registry itself so this test file
    // carries no raw glyph literal (which would trip the glyph-lint gate).
    let mut body = String::from("status markers: ");
    body.push_str(crate::glyphs::Glyph::Check.unicode());
    body.push_str(crate::glyphs::Glyph::Cross.unicode());
    body.push_str(crate::glyphs::Glyph::Check.unicode());
    assert_eq!(count_registry_glyphs(&body), 3);
}
