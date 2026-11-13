//! STORY-2401 slice 2: the gh-read ratchet. Every forge read is meant to go
//! through `forge.rs` / `forge_observed.rs`, where a raw `gh` exit status
//! becomes an `Observed<T>`. In every non-test source outside those two
//! files, this gate counts the sites that resolve or spawn `gh` directly
//! and references to the legacy train pass-through helpers outside pr_train.rs.
//! COUNTS MAY ONLY GO DOWN.
//!
//! The advisor ruling (2026-11-03) on how these sites are handled:
//! - Decision-making sites migrate in STORY-2401: the train, idle_eligible,
//!   phase_driver/phase_gates, dispatch_flow, reconcile_evidence, pr_lookup,
//!   and the review-watch head fetch.
//! - Display-only sites (status_prs, the TUI, history_timeline) stay frozen
//!   at their counts here.
//!
//! A call that is not a forge read (for example `gh auth status` in a
//! capability probe) can be justified with
//! `// raw-gh-ok(STORY-2401): <why>` in the three lines above it.
//! trace:STORY-2401 | ai:claude

use crate::seat_clone::clone_git_ratchet_tests::{test_module_lines, workspace_sources};
use std::collections::BTreeMap;
use std::path::Path;

/// Files that own the gh boundary; never counted.
const BOUNDARY: &[&str] = &[
    "aida-cli-lib/src/forge.rs",
    "aida-cli-lib/src/forge_observed.rs",
];

/// Raw gh sites per file. Lower a count in the same change that moves a
/// site behind forge.rs; a file that drops to zero leaves the list. Never
/// raise a count or add a file.
const RAW_GH_ALLOWLIST: &[(&str, usize)] = &[
    ("aida-cli-lib/src/agent_launch.rs", 1),
    ("aida-cli-lib/src/ci_failures.rs", 1),
    ("aida-cli-lib/src/ci_probe.rs", 1),
    ("aida-cli-lib/src/disk_emergency.rs", 1),
    ("aida-cli-lib/src/dispatch_flow.rs", 2),
    ("aida-cli-lib/src/docs_sweep.rs", 2),
    ("aida-cli-lib/src/gate/ledger/migrate.rs", 1),
    ("aida-cli-lib/src/gitlab_mirror_link.rs", 1),
    ("aida-cli-lib/src/harvest.rs", 1),
    ("aida-cli-lib/src/history_timeline.rs", 1),
    ("aida-cli-lib/src/kernel.rs", 1),
    ("aida-cli-lib/src/lane_liveness.rs", 1),
    ("aida-cli-lib/src/ci_watch.rs", 1),
    ("aida-cli-lib/src/forge_lookup.rs", 4),
    ("aida-cli-lib/src/orchestrator_leases.rs", 1),
    ("aida-cli-lib/src/ownership/migrate.rs", 1),
    ("aida-cli-lib/src/phase_driver.rs", 1),
    ("aida-cli-lib/src/phase_driver_trait.rs", 1),
    ("aida-cli-lib/src/phase_gates.rs", 1),
    // Present on main before this ratchet's initial landing (STORY-2476).
    ("aida-cli-lib/src/pipeline_stall.rs", 2),
    ("aida-cli-lib/src/plan_cmd.rs", 1),
    // BUG-2515 moved ship title/body reads behind the resolved forge.
    ("aida-cli-lib/src/pr_cmd.rs", 3),
    ("aida-cli-lib/src/pr_list.rs", 1),
    ("aida-cli-lib/src/pr_lookup.rs", 3),
    ("aida-cli-lib/src/pr_ship_batch.rs", 1),
    ("aida-cli-lib/src/project_capabilities.rs", 1),
    ("aida-cli-lib/src/reconcile_evidence.rs", 2),
    ("aida-cli-lib/src/remote_create.rs", 1),
    ("aida-cli-lib/src/review_watch_update.rs", 1),
    ("aida-cli-lib/src/seat_clone.rs", 1),
    // Present on main before this ratchet's initial landing (BUG-2450).
    ("aida-cli-lib/src/standalone_empty_rework.rs", 1),
    ("aida-cli-lib/src/status_awaiting.rs", 1),
    ("aida-cli-lib/src/status_prs.rs", 7),
    ("aida-cli-lib/src/status_sections.rs", 2),
    ("aida-cli-lib/src/worktree_reclaim.rs", 1),
    ("aida-tui/src/board.rs", 1),
    ("aida-tui/src/dashboard.rs", 1),
    ("aida-tui/src/welcome.rs", 1),
];

/// Raw gh sites in one source: lines outside `#[cfg(test)]` modules and
/// comments that resolve the gh binary (`resolve_gh_binary*()`,
/// `resolve_forge_cli(ForgeKind::GitHub)`) or build `Command::new("gh")`,
/// plus legacy train-helper references unless scanning pr_train.rs, minus the
/// resolver definitions and any site with a `raw-gh-ok(STORY-2401):`
/// justification in the three lines above it.
fn raw_gh_sites(src: &str, train_helpers_allowed: bool) -> usize {
    static RAW: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"resolve_gh_binary(_for_probe)?\(|resolve_forge_cli\(\s*(crate::forge::)?ForgeKind::GitHub|\b\w+::new\("gh"\)"#,
        )
        .unwrap()
    });
    // References (including imports/aliases) count, not just direct calls.
    // The legacy helpers are reserved for pr_train.rs until its typed migration.
    // trace:STORY-2401 | ai:codex
    static TRAIN_HELPER: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\btrain_gh_(stdout|api_read)\b").unwrap());
    static DEF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\bfn\s+resolve_(gh_binary|forge_cli)").unwrap()
    });
    let lines: Vec<&str> = src.lines().collect();
    let tests = test_module_lines(&lines);
    (0..lines.len())
        .filter(|&i| !tests.contains(&i) && !lines[i].trim_start().starts_with("//"))
        .filter(|&i| {
            (RAW.is_match(lines[i]) && !DEF.is_match(lines[i]))
                || (!train_helpers_allowed && TRAIN_HELPER.is_match(lines[i]))
        })
        .filter(|&i| {
            !lines[i.saturating_sub(3)..i]
                .iter()
                .any(|l| l.contains("raw-gh-ok(STORY-2401):"))
        })
        .count()
}

fn ratchet_violations(
    measured: &BTreeMap<String, usize>,
    allowed: &[(&str, usize)],
) -> Vec<String> {
    let allowed: BTreeMap<&str, usize> = allowed.iter().copied().collect();
    let mut violations = Vec::new();
    for (file, &count) in measured {
        match allowed.get(file.as_str()) {
            None => violations.push(format!(
                "{file}: {count} new raw gh site(s); read the forge through \
                 crate::forge (Forge::observe_* / forge_observed::observe_gh)"
            )),
            Some(&max) if count > max => violations.push(format!(
                "{file}: {count} raw gh sites, allowlist says {max}; route the new \
                 read through crate::forge"
            )),
            Some(&max) if count < max => violations.push(format!(
                "{file}: {count} raw gh sites, allowlist says {max}; lower the \
                 allowlist to {count} (the ratchet only tightens)"
            )),
            _ => {}
        }
    }
    for (file, max) in allowed {
        if !measured.contains_key(file) {
            violations.push(format!(
                "{file}: allowlisted for {max} but has no raw gh site left; \
                 remove it from the allowlist"
            ));
        }
    }
    violations
}

#[test]
fn story_2401_raw_gh_scanner_counts_spawns_and_skips_tests_comments_and_escapes() {
    let src = "fn a() {\n    let gh = crate::resolve_gh_binary()?;\n}\n\
               fn b() { let _ = std::process::Command::new(\"gh\"); }\n\
               fn c() { let _ = resolve_forge_cli(crate::forge::ForgeKind::GitHub); }\n\
               fn d() { let _ = resolve_forge_cli(ForgeKind::GitLab); }\n\
               pub(crate) fn resolve_gh_binary() -> Option<PathBuf> { None }\n\
               // Command::new(\"gh\") in a comment\n\
               fn e() {\n    // raw-gh-ok(STORY-2401): auth probe, not a read.\n    \
               let _ = Command::new(\"gh\");\n}\n\
               #[cfg(test)]\nmod t {\n    fn f() { Command::new(\"gh\"); }\n}\n";
    assert_eq!(raw_gh_sites(src, false), 3);
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_ratchet_catches_new_train_helper_callers_and_aliases() {
    for src in [
        r#"fn f() { crate::forge::train_gh_stdout(root, &["pr", "view"])?; }"#,
        r#"fn f() { crate::forge::train_gh_api_read(root, endpoint)?; }"#,
        "use crate::forge::train_gh_stdout as bypass;",
        "use crate::forge::train_gh_api_read as bypass;",
    ] {
        let count = raw_gh_sites(src, false);
        assert_eq!(count, 1, "new caller must be counted: {src}");
        let measured = BTreeMap::from([("new_caller.rs".to_string(), count)]);
        let violations = ratchet_violations(&measured, &[]);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].starts_with("new_caller.rs: 1 new raw gh site"));
        assert_eq!(raw_gh_sites(src, true), 0, "legacy train allowance");
    }
    assert_eq!(
        raw_gh_sites(r#"fn f() { Command::new("gh"); }"#, true),
        1,
        "the train allowance must not exempt direct spawns"
    );
}

#[test]
fn story_2401_ratchet_fails_up_down_new_and_stale() {
    let measured: BTreeMap<String, usize> = [("a.rs", 2), ("b.rs", 1), ("c.rs", 1)]
        .into_iter()
        .map(|(f, n)| (f.to_string(), n))
        .collect();
    let violations = ratchet_violations(&measured, &[("a.rs", 1), ("b.rs", 2), ("e.rs", 3)]);
    assert_eq!(violations.len(), 4, "{violations:#?}");
    assert!(violations[0].starts_with("a.rs: 2 raw gh sites, allowlist says 1"));
    assert!(violations[1].contains("lower the allowlist to 1"));
    assert!(violations[2].starts_with("c.rs: 1 new raw gh site"));
    assert!(violations[3].starts_with("e.rs: allowlisted for 3"));
    assert!(ratchet_violations(&measured, &[("a.rs", 2), ("b.rs", 1), ("c.rs", 1)]).is_empty());
}

/// The gate itself, over the whole workspace.
#[test]
fn story_2401_no_new_raw_gh_reads_outside_forge() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let sources = workspace_sources(workspace);
    for required in BOUNDARY
        .iter()
        .chain(["aida-cli-lib/src/pr_train.rs"].iter())
    {
        assert!(
            sources.contains_key(*required),
            "the gate must scan {required}"
        );
    }
    let measured: BTreeMap<String, usize> = sources
        .iter()
        .filter(|(file, _)| !BOUNDARY.contains(&file.as_str()))
        .map(|(file, src)| {
            (
                file.clone(),
                raw_gh_sites(src, file == "aida-cli-lib/src/pr_train.rs"),
            )
        })
        .filter(|(_, n)| *n > 0)
        .collect();
    // Slice 2 moved the train's and idle_eligible's gh reads behind forge.rs.
    for moved in [
        "aida-cli-lib/src/pr_train.rs",
        "aida-cli-lib/src/idle_eligible.rs",
    ] {
        assert!(
            !measured.contains_key(moved),
            "{moved} must read the forge through crate::forge"
        );
    }
    let violations = ratchet_violations(&measured, RAW_GH_ALLOWLIST);
    assert!(
        violations.is_empty(),
        "STORY-2401 gh-read ratchet:\n  {}\nmeasured: {measured:#?}",
        violations.join("\n  ")
    );
}
