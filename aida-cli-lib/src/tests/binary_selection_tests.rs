//! TASK-221: pin the pure parts of `aida dev activate`'s binary
//! selection. `pick_dev_binary_dir` itself touches the filesystem
//! and spawns subprocesses; this mod covers `parse_embedded_sha` and
//! the `ShaMatch::Exact` shortcut path of `classify_sha_match` which
//! is logic, not git. trace:TASK-221 | ai:claude
use super::*;
// The pure auto-select binary picker moved to `dev_cmd` (SPIKE-78).
use crate::dev_cmd::{
    activate_reexec_target, auto_select_dev_profile, classify_dev_activation,
    BinarySelectionReason, DevActivationState, DevBuildCandidate, DevProfile,
};

// ---- BUG-1305: activation is a fact about the pinned binary, not the env ----

#[test]
fn dev_activation_state_distinguishes_present_missing_and_path_fallthrough() {
    let temp = tempfile::tempdir().unwrap();
    let pinned = temp.path().join("aida");
    let fallback = temp.path().join("fallback-aida");

    assert_eq!(
        classify_dev_activation(true, Some(&pinned), Some(&fallback)),
        DevActivationState::Missing
    );

    std::fs::write(&pinned, b"binary").unwrap();
    assert_eq!(
        classify_dev_activation(true, Some(&pinned), Some(&pinned)),
        DevActivationState::Active
    );
    assert_eq!(
        classify_dev_activation(true, Some(&pinned), Some(&fallback)),
        DevActivationState::PathMismatch
    );
    assert_eq!(
        classify_dev_activation(false, Some(&pinned), Some(&pinned)),
        DevActivationState::Inactive
    );
}

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
        None,
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
        None,
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Debug);

    // Symmetric: release newer among two exacts → release.
    let (pick, _) = auto_select_dev_profile(
        Some(cand(200, ShaMatch::Exact)),
        Some(cand(100, ShaMatch::Exact)),
        None,
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
        None,
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
        None,
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
        None,
    )
    .unwrap();
    assert_eq!(pick, DevProfile::Release);
}

#[test]
fn auto_only_one_build_present() {
    let (pick, reason) =
        auto_select_dev_profile(None, Some(cand(100, ShaMatch::Unknown)), None).unwrap();
    assert_eq!(pick, DevProfile::Debug);
    assert_eq!(reason, BinarySelectionReason::OnlyOne);

    let (pick, reason) =
        auto_select_dev_profile(Some(cand(100, ShaMatch::Unknown)), None, None).unwrap();
    assert_eq!(pick, DevProfile::Release);
    assert_eq!(reason, BinarySelectionReason::OnlyOne);
}

#[test]
fn auto_no_build_present_is_none() {
    assert!(auto_select_dev_profile(None, None, None).is_none());
}

// ---- TASK-1158: bare-activate default is the release profile ----
//
// `resolve_activation_request` maps {CLI request, env pin} to the request
// `pick_dev_binary_dir` sees (`None` = auto freshest-wins). The bare form
// (no request, no pin) must default to release — auto is the explicit,
// sticky opt-in. trace:TASK-1158 | ai:claude

use crate::dev_cmd::resolve_activation_request;

#[test]
fn bare_activate_defaults_to_release_not_auto() {
    // No CLI request, no env pin → release, flagged as the applied default.
    assert_eq!(
        resolve_activation_request(None, None),
        (Some("release"), true)
    );
}

#[test]
fn explicit_auto_opts_into_freshest_wins() {
    // `aida dev activate auto` / `--auto` → auto-select (None), not default.
    assert_eq!(
        resolve_activation_request(Some("auto"), None),
        (None, false)
    );
    // Sticky: an `auto` env pin from a previous activation keeps
    // freshest-wins on subsequent bare activates.
    assert_eq!(
        resolve_activation_request(None, Some("auto")),
        (None, false)
    );
}

#[test]
fn explicit_profile_requests_and_pins_still_win() {
    assert_eq!(
        resolve_activation_request(Some("debug"), None),
        (Some("debug"), false)
    );
    assert_eq!(
        resolve_activation_request(None, Some("debug")),
        (Some("debug"), false)
    );
    // An explicit CLI request beats the env pin.
    assert_eq!(
        resolve_activation_request(Some("release"), Some("debug")),
        (Some("release"), false)
    );
    assert_eq!(
        resolve_activation_request(Some("auto"), Some("debug")),
        (None, false)
    );
}

// ---- TASK-1157: prompt staleness token ----

#[test]
fn ps1_token_empty_when_active_matches_head() {
    assert_eq!(
        crate::dev_cmd::ps1_staleness_token(
            "866b050",
            Some("866b050aabbccddeeff1122334455667788990011"),
            Some("deadbee"),
        ),
        ""
    );
}

#[test]
fn ps1_token_flip_when_other_build_matches_head() {
    assert_eq!(
        crate::dev_cmd::ps1_staleness_token(
            "deadbee",
            Some("866b050aabbccddeeff1122334455667788990011"),
            Some("866b050"),
        ),
        "⇄"
    );
}

// ---- TASK-1180: activate explains the marker it just installed ----

#[test]
fn activate_says_nothing_when_the_build_is_current() {
    // The common case. Activate is otherwise a quiet command and must not grow
    // noise for a build that has nothing wrong with it.
    assert_eq!(
        crate::dev_cmd::activate_marker_hint("", Some("debug")),
        None
    );
}

#[test]
fn activate_hint_for_flip_names_the_other_profile_and_the_command() {
    let h = crate::dev_cmd::activate_marker_hint("⇄", Some("debug")).expect("should hint");
    assert!(h.contains('⇄'), "must name the marker actually shown: {h}");
    assert!(
        h.contains("debug build matches"),
        "must name the other profile: {h}"
    );
    assert!(
        h.contains("`aida dev activate debug`"),
        "must give the command: {h}"
    );
    assert!(
        h.contains("no rebuild"),
        "the point of ⇄ is that a rebuild is unnecessary: {h}"
    );
}

#[test]
fn activate_hint_for_flip_names_release_when_that_is_the_other_profile() {
    // The alternate profile must never be hardcoded.
    let h = crate::dev_cmd::activate_marker_hint("⇄", Some("release")).unwrap();
    assert!(h.contains("release build matches"), "{h}");
    assert!(h.contains("`aida dev activate release`"), "{h}");
    assert!(
        !h.contains("debug"),
        "must not mention the wrong profile: {h}"
    );
}

#[test]
fn activate_hint_for_rebuild_gives_the_build_command() {
    let h = crate::dev_cmd::activate_marker_hint("↻", Some("debug")).expect("should hint");
    assert!(h.contains('↻'), "{h}");
    assert!(h.contains("no build matches HEAD"), "{h}");
    assert!(h.contains("aida dev build"), "must give the remedy: {h}");
    // Nothing to flip to, so it must not offer a switch.
    assert!(
        !h.contains("activate debug"),
        "↻ means neither build is usable: {h}"
    );
}

#[test]
fn a_flip_hint_without_a_known_other_profile_still_reads() {
    // Not reachable today (⇄ implies the other build matched), but the wording
    // must not degrade to "activate None" if that ever changes.
    let h = crate::dev_cmd::activate_marker_hint("⇄", None).unwrap();
    assert!(!h.contains("None"), "{h}");
    assert!(h.contains("`aida dev activate`"), "{h}");
}

#[test]
fn an_unknown_marker_is_silent_rather_than_guessing() {
    assert_eq!(
        crate::dev_cmd::activate_marker_hint("?", Some("debug")),
        None
    );
}

#[test]
fn ps1_token_rebuild_when_no_build_matches_head() {
    assert_eq!(
        crate::dev_cmd::ps1_staleness_token(
            "deadbee",
            Some("866b050aabbccddeeff1122334455667788990011"),
            Some("cafebabe"),
        ),
        "↻"
    );
}

#[test]
fn direct_head_reader_resolves_loose_ref_without_git() {
    let tmp = tempfile::tempdir().unwrap();
    let git = tmp.path().join(".git");
    std::fs::create_dir_all(git.join("refs/heads")).unwrap();
    std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(
        git.join("refs/heads/main"),
        "866b050aabbccddeeff1122334455667788990011\n",
    )
    .unwrap();

    assert_eq!(
        crate::dev_cmd::current_branch_head_sha_direct(tmp.path()).as_deref(),
        Some("866b050aabbccddeeff1122334455667788990011")
    );
}

#[test]
fn direct_head_reader_resolves_linked_worktree_gitfile() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("worktree");
    let git = tmp.path().join("main.git");
    let worktree_git = git.join("worktrees/wt");
    std::fs::create_dir_all(git.join("refs/heads")).unwrap();
    std::fs::create_dir_all(&worktree_git).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join(".git"),
        format!("gitdir: {}\n", worktree_git.display()),
    )
    .unwrap();
    std::fs::write(worktree_git.join("commondir"), "../..\n").unwrap();
    std::fs::write(worktree_git.join("HEAD"), "ref: refs/heads/task-1157\n").unwrap();
    std::fs::write(
        git.join("refs/heads/task-1157"),
        "866b050aabbccddeeff1122334455667788990011\n",
    )
    .unwrap();

    assert_eq!(
        crate::dev_cmd::current_branch_head_sha_direct(&repo).as_deref(),
        Some("866b050aabbccddeeff1122334455667788990011")
    );
}

// ── BUG-760: re-exec resolution for `dev activate` ──────────────────────
// The first activate of a fresh shell runs the INSTALLED aida (PATH not
// yet prepended); the fix delegates to the repo's own freshest built
// binary. These pin the pure decision core. trace:BUG-760 | ai:claude

fn reexec_time(secs: u64) -> std::time::SystemTime {
    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)
}

#[test]
fn reexec_targets_freshest_repo_binary_for_foreign_exe() {
    let target = std::path::Path::new("/repo/target");
    let release = Some((
        std::path::PathBuf::from("/repo/target/release/aida"),
        reexec_time(100),
    ));
    let debug = Some((
        std::path::PathBuf::from("/repo/target/debug/aida"),
        reexec_time(200),
    ));
    // Installed binary elsewhere on PATH; debug build is fresher → its
    // activate semantics drive.
    let got = activate_reexec_target(
        target,
        Some(std::path::Path::new("/usr/local/bin/aida")),
        false,
        release,
        debug,
    );
    assert_eq!(
        got.as_deref(),
        Some(std::path::Path::new("/repo/target/debug/aida"))
    );
}

#[test]
fn reexec_mtime_tie_falls_to_release() {
    let target = std::path::Path::new("/repo/target");
    let release = Some((
        std::path::PathBuf::from("/repo/target/release/aida"),
        reexec_time(100),
    ));
    let debug = Some((
        std::path::PathBuf::from("/repo/target/debug/aida"),
        reexec_time(100),
    ));
    let got = activate_reexec_target(
        target,
        Some(std::path::Path::new("/usr/local/bin/aida")),
        false,
        release,
        debug,
    );
    assert_eq!(
        got.as_deref(),
        Some(std::path::Path::new("/repo/target/release/aida"))
    );
}

#[test]
fn reexec_skips_when_exe_already_in_repo_target() {
    // Running binary IS an in-repo build (either profile) — its semantics
    // are the repo's; no delegation, no extra process.
    let target = std::path::Path::new("/repo/target");
    let release = Some((
        std::path::PathBuf::from("/repo/target/release/aida"),
        reexec_time(100),
    ));
    let debug = Some((
        std::path::PathBuf::from("/repo/target/debug/aida"),
        reexec_time(200),
    ));
    let got = activate_reexec_target(
        target,
        Some(std::path::Path::new("/repo/target/release/aida")),
        false,
        release,
        debug,
    );
    assert_eq!(got, None);
}

#[test]
fn reexec_skips_under_guard() {
    // The delegated child carries the guard env — it must never chain.
    let target = std::path::Path::new("/repo/target");
    let release = Some((
        std::path::PathBuf::from("/repo/target/release/aida"),
        reexec_time(100),
    ));
    let got = activate_reexec_target(
        target,
        Some(std::path::Path::new("/usr/local/bin/aida")),
        true,
        release,
        None,
    );
    assert_eq!(got, None);
}

#[test]
fn reexec_skips_when_no_build_exists() {
    // Nothing built → fall through so the normal path errors helpfully.
    let target = std::path::Path::new("/repo/target");
    let got = activate_reexec_target(
        target,
        Some(std::path::Path::new("/usr/local/bin/aida")),
        false,
        None,
        None,
    );
    assert_eq!(got, None);
}

#[test]
fn reexec_skips_when_current_exe_unknown() {
    // Can't prove we're not the in-repo binary → conservative no-op
    // (worst case is the pre-fix behavior, never a loop).
    let target = std::path::Path::new("/repo/target");
    let release = Some((
        std::path::PathBuf::from("/repo/target/release/aida"),
        reexec_time(100),
    ));
    let got = activate_reexec_target(target, None, false, release, None);
    assert_eq!(got, None);
}

#[test]
fn reexec_single_profile_builds_delegate_to_that_profile() {
    let target = std::path::Path::new("/repo/target");
    let exe = std::path::Path::new("/usr/local/bin/aida");
    let release = Some((
        std::path::PathBuf::from("/repo/target/release/aida"),
        reexec_time(100),
    ));
    let debug = Some((
        std::path::PathBuf::from("/repo/target/debug/aida"),
        reexec_time(50),
    ));
    assert_eq!(
        activate_reexec_target(target, Some(exe), false, release, None).as_deref(),
        Some(std::path::Path::new("/repo/target/release/aida"))
    );
    assert_eq!(
        activate_reexec_target(target, Some(exe), false, None, debug).as_deref(),
        Some(std::path::Path::new("/repo/target/debug/aida"))
    );
}

// ---- BUG-2210: activate in a worktree uses the worktree's own build ----

/// A fake aida checkout at `dir` with an agent build stamped `age_secs` old.
fn fake_checkout_with_agent_build(dir: &std::path::Path, age_secs: u64) {
    std::fs::create_dir_all(dir.join("target/agent")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\n[workspace.package]\nrepository = \"https://github.com/joemooney/aida\"\n",
    )
    .unwrap();
    let bin = dir.join("target/agent/aida");
    std::fs::write(&bin, b"binary").unwrap();
    let mtime = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
    std::fs::File::options()
        .write(true)
        .open(&bin)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn activate_auto_in_a_worktree_prefers_its_own_older_build_over_mains_newer_one() {
    use crate::dev_cmd::ActivateRepoSource;
    use crate::dev_cmd::{activate_choice_note, choose_activate_repo, pick_dev_binary_dir};
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("aida");
    let worktree = temp.path().join("aida-worktrees/take-2830");
    fake_checkout_with_agent_build(&main, 10); // newer
    fake_checkout_with_agent_build(&worktree, 3600); // older
    let main = main.canonicalize().unwrap();
    let worktree = worktree.canonicalize().unwrap();
    std::fs::create_dir_all(worktree.join("aida-cli-lib/src")).unwrap();
    let main_str = main.display().to_string();

    // Run from inside the worktree while $AIDA_DEV_REPO names main.
    let (repo, source) = choose_activate_repo(
        None,
        Some(&main_str),
        None,
        &worktree.join("aida-cli-lib/src"),
    );
    assert_eq!(repo, worktree);
    assert_eq!(source, ActivateRepoSource::Enclosing);

    // `auto` (request None) picks the worktree's agent build.
    let (bin_dir, profile, reason) = pick_dev_binary_dir(&repo, None).unwrap();
    assert_eq!(bin_dir, worktree.join("target/agent"));
    assert_eq!(profile, "agent");

    let note = activate_choice_note(
        profile,
        &bin_dir,
        &reason,
        false,
        &repo,
        source,
        Some(&main_str),
    );
    assert!(
        note.contains(&worktree.join("target/agent").display().to_string()),
        "{note}"
    );
    assert!(
        note.contains("the checkout you are in, not $AIDA_DEV_REPO"),
        "{note}"
    );
    assert!(note.contains(&main_str), "{note}");

    // Primary-checkout behaviour is unchanged: from main, main's build.
    let (repo, source) = choose_activate_repo(None, Some(&main_str), None, &main);
    assert_eq!(
        (repo.as_path(), source),
        (main.as_path(), ActivateRepoSource::Enclosing)
    );
    let note = activate_choice_note(
        "agent",
        &main.join("target/agent"),
        &reason,
        false,
        &main,
        source,
        Some(&main_str),
    );
    assert!(note.ends_with("(the checkout you are in)"), "{note}");

    // Outside any checkout, $AIDA_DEV_REPO still applies; --repo beats all.
    let outside = temp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    assert_eq!(
        choose_activate_repo(None, Some(&main_str), None, &outside),
        (main.clone(), ActivateRepoSource::EnvRepo)
    );
    let wt_str = worktree.display().to_string();
    assert_eq!(
        choose_activate_repo(Some(&wt_str), Some(&main_str), None, &main),
        (worktree.clone(), ActivateRepoSource::Flag)
    );
}

// trace:TASK-2251 | ai:claude
#[test]
fn activate_falls_back_to_the_primary_checkout_once_the_worktree_in_aida_dev_repo_is_removed() {
    use crate::dev_cmd::{activate_choice_note, choose_activate_repo, primary_checkout_of};
    use crate::dev_cmd::{ActivateRepoSource, BinarySelectionReason};
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("aida");
    std::fs::create_dir_all(&main).unwrap();
    let main = main.canonicalize().unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&main, &["init", "-q", "-b", "main"]);
    git(
        &main,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let worktree = temp.path().join("aida-worktrees/take-2250");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "take",
            worktree.to_str().unwrap(),
        ],
    );

    // Activation records the primary checkout from either side.
    assert_eq!(primary_checkout_of(&worktree), main);
    assert_eq!(primary_checkout_of(&main), main);

    // The worktree is removed; $AIDA_DEV_REPO still names it.
    let wt_str = worktree.display().to_string();
    git(&main, &["worktree", "remove", "--force", &wt_str]);
    assert!(!worktree.exists());
    let main_str = main.display().to_string();
    let outside = temp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();

    let (repo, source) = choose_activate_repo(None, Some(&wt_str), Some(&main_str), &outside);
    assert_eq!(
        (repo.as_path(), source),
        (main.as_path(), ActivateRepoSource::HomeRepo)
    );
    let note = activate_choice_note(
        "agent",
        &main.join("target/agent"),
        &BinarySelectionReason::OnlyOne,
        false,
        &repo,
        source,
        Some(&wt_str),
    );
    assert!(
        note.contains(&format!("$AIDA_DEV_REPO ({wt_str}) no longer exists")),
        "{note}"
    );

    // A live $AIDA_DEV_REPO still wins over the recorded primary.
    assert_eq!(
        choose_activate_repo(None, Some(&main_str), Some(&wt_str), &outside),
        (main.clone(), ActivateRepoSource::EnvRepo)
    );
    // Nothing to fall back to: the stale path is kept so the error names it.
    assert_eq!(
        choose_activate_repo(None, Some(&wt_str), None, &outside),
        (worktree.clone(), ActivateRepoSource::EnvRepo)
    );
}

// trace:TASK-2251 | ai:claude
#[test]
fn home_repo_fallback_fires_only_for_a_set_but_missing_aida_dev_repo() {
    use crate::dev_cmd::{choose_activate_repo, choose_env_dev_repo, ActivateRepoSource};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("aida");
    std::fs::create_dir_all(&home).unwrap();
    let home_str = home.display().to_string();
    let gone = temp.path().join("aida-worktrees/removed");
    let gone_str = gone.display().to_string();
    let outside = temp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();

    // Unset (or empty) $AIDA_DEV_REPO: no dev checkout, so no fallback and
    // no "(?) no longer exists" note.
    fn os(s: &str) -> &std::ffi::OsStr {
        std::ffi::OsStr::new(s)
    }
    assert_eq!(choose_env_dev_repo(None, Some(os(&home_str))), None);
    assert_eq!(choose_env_dev_repo(Some(os("")), Some(os(&home_str))), None);
    assert_eq!(
        choose_activate_repo(None, None, Some(&home_str), &outside),
        (outside.clone(), ActivateRepoSource::Cwd)
    );
    // Set but missing: the recorded primary checkout.
    assert_eq!(
        choose_env_dev_repo(Some(os(&gone_str)), Some(os(&home_str))),
        Some((home.clone(), ActivateRepoSource::HomeRepo))
    );
    // Set and present: used as is, even with a home recorded.
    assert_eq!(
        choose_env_dev_repo(Some(os(&home_str)), Some(os(&gone_str))),
        Some((home.clone(), ActivateRepoSource::EnvRepo))
    );
}

// trace:TASK-2251 | ai:claude
#[test]
fn dev_build_and_release_fall_back_to_the_home_repo_for_a_removed_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("aida");
    std::fs::create_dir_all(&home).unwrap();
    let home_str = home.display().to_string();
    let gone_str = temp.path().join("removed").display().to_string();

    // `aida dev build` / `aida dev release` read the env through this.
    let env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_DEV_REPO", Some(&gone_str)),
        ("AIDA_DEV_HOME_REPO", Some(&home_str)),
    ]);
    assert_eq!(crate::dev_cmd::env_dev_repo(), Some(home.clone()));
    drop(env);

    // No fallback recorded: the stale path is dropped, not returned.
    let env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_DEV_REPO", Some(&gone_str)),
        ("AIDA_DEV_HOME_REPO", None),
    ]);
    assert_eq!(crate::dev_cmd::env_dev_repo(), None);
    drop(env);

    // Unset $AIDA_DEV_REPO: nothing, even with a home recorded.
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_DEV_REPO", None),
        ("AIDA_DEV_HOME_REPO", Some(&home_str)),
    ]);
    assert_eq!(crate::dev_cmd::env_dev_repo(), None);
}

// TASK-2272 (TASK-2278): the real `aida dev build` / `aida dev release`
// resolvers, run from outside any checkout with `$AIDA_DEV_REPO` naming a
// removed worktree, land on the recorded primary checkout.
// trace:TASK-2272 | ai:claude
#[test]
fn build_and_release_resolvers_take_the_home_repo_for_a_removed_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("aida");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("Cargo.toml"),
        "[workspace]\n[workspace.package]\nrepository = \"https://github.com/joemooney/aida\"\n",
    )
    .unwrap();
    let outside = temp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let home_str = home.display().to_string();
    let gone_str = temp
        .path()
        .join("aida-worktrees/removed")
        .display()
        .to_string();

    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_DEV_REPO", Some(&gone_str)),
        ("AIDA_DEV_HOME_REPO", Some(&home_str)),
    ]);
    let canonical = home.canonicalize().unwrap();
    assert_eq!(
        crate::dev_build::resolve_build_repo_from(&outside).unwrap(),
        canonical
    );
    assert_eq!(
        crate::upgrade_release::release_repo_from(&outside),
        Some(home.clone())
    );
}
