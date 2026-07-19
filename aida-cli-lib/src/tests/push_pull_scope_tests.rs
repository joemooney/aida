use super::*;
use clap::Parser;

/// Default: neither flag, no env → both legs.
#[test]
fn resolve_scope_default_is_both() {
    assert_eq!(resolve_push_scope(false, false, None), (false, false));
}

/// Explicit flags pass straight through.
#[test]
fn resolve_scope_explicit_flags() {
    assert_eq!(resolve_push_scope(true, false, None), (true, false));
    assert_eq!(resolve_push_scope(false, true, None), (false, true));
}

/// AIDA_PUSH_DEFAULT flips the default when no flag is passed.
#[test]
fn resolve_scope_env_override() {
    assert_eq!(
        resolve_push_scope(false, false, Some("code")),
        (true, false)
    );
    assert_eq!(
        resolve_push_scope(false, false, Some("code-only")),
        (true, false)
    );
    assert_eq!(
        resolve_push_scope(false, false, Some("store")),
        (false, true)
    );
    assert_eq!(
        resolve_push_scope(false, false, Some("STORE-ONLY")),
        (false, true)
    );
    // Unrecognized / explicit "both" → historical default.
    assert_eq!(
        resolve_push_scope(false, false, Some("both")),
        (false, false)
    );
    assert_eq!(
        resolve_push_scope(false, false, Some("xyz")),
        (false, false)
    );
}

/// An explicit flag always wins over the env var.
#[test]
fn resolve_scope_explicit_beats_env() {
    assert_eq!(
        resolve_push_scope(true, false, Some("store")),
        (true, false)
    );
    assert_eq!(resolve_push_scope(false, true, Some("code")), (false, true));
}

/// All four parse combinations for `aida push`.
#[test]
fn push_flag_combinations_parse() {
    // default
    let cli = Cli::try_parse_from(["aida", "push"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Push {
            code_only: false,
            store_only: false,
            ..
        }
    ));
    // --code-only
    let cli = Cli::try_parse_from(["aida", "push", "--code-only"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Push {
            code_only: true,
            store_only: false,
            ..
        }
    ));
    // --store-only
    let cli = Cli::try_parse_from(["aida", "push", "--store-only"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Push {
            code_only: false,
            store_only: true,
            ..
        }
    ));
    // both → clap rejects via conflicts_with
    assert!(Cli::try_parse_from(["aida", "push", "--code-only", "--store-only"]).is_err());
}

/// TASK-863: --no-notice parses and lands on the Push variant.
#[test]
fn push_no_notice_flag_parses() {
    let cli = Cli::try_parse_from(["aida", "push", "--no-notice"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Push {
            no_notice: true,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["aida", "push"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Push {
            no_notice: false,
            ..
        }
    ));
}

/// TASK-863: the uncommitted-change counter returns `Some(n)` on a dirty
/// tree and `None` on a clean one (so the caller stays silent).
#[test]
fn uncommitted_change_count_dirty_vs_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap()
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@e.st"]);
    run(&["config", "user.name", "Test"]);
    std::fs::write(repo.join("a.txt"), "hello").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-q", "-m", "init"]);

    // Clean tree → None (silent).
    assert_eq!(super::uncommitted_change_count(repo), None);

    // One unstaged modification → Some(1).
    std::fs::write(repo.join("a.txt"), "changed").unwrap();
    assert_eq!(super::uncommitted_change_count(repo), Some(1));

    // A second untracked file → Some(2).
    std::fs::write(repo.join("b.txt"), "new").unwrap();
    assert_eq!(super::uncommitted_change_count(repo), Some(2));

    // Non-git path → None (never block on a status failure).
    let nonrepo = tempfile::tempdir().unwrap();
    assert_eq!(super::uncommitted_change_count(nonrepo.path()), None);
}

/// TASK-863: AIDA_PUSH_QUIET suppression honors truthy/falsey values.
/// Serialized via a global env var, so guarded against parallel test
/// interference by setting+clearing within the single test.
#[test]
fn push_notice_env_suppression() {
    // Save + restore the ambient value so we don't clobber a real env.
    let saved = std::env::var("AIDA_PUSH_QUIET").ok();

    std::env::remove_var("AIDA_PUSH_QUIET");
    assert!(!super::push_notice_suppressed_by_env());

    std::env::set_var("AIDA_PUSH_QUIET", "1");
    assert!(super::push_notice_suppressed_by_env());

    std::env::set_var("AIDA_PUSH_QUIET", "true");
    assert!(super::push_notice_suppressed_by_env());

    std::env::set_var("AIDA_PUSH_QUIET", "0");
    assert!(!super::push_notice_suppressed_by_env());

    std::env::set_var("AIDA_PUSH_QUIET", "false");
    assert!(!super::push_notice_suppressed_by_env());

    std::env::set_var("AIDA_PUSH_QUIET", "");
    assert!(!super::push_notice_suppressed_by_env());

    match saved {
        Some(v) => std::env::set_var("AIDA_PUSH_QUIET", v),
        None => std::env::remove_var("AIDA_PUSH_QUIET"),
    }
}

/// Same four combinations for `aida pull`.
#[test]
fn pull_flag_combinations_parse() {
    let cli = Cli::try_parse_from(["aida", "pull"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Pull {
            code_only: false,
            store_only: false,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["aida", "pull", "--code-only"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Pull {
            code_only: true,
            store_only: false,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["aida", "pull", "--store-only"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Pull {
            code_only: false,
            store_only: true,
            ..
        }
    ));
    assert!(Cli::try_parse_from(["aida", "pull", "--code-only", "--store-only"]).is_err());
}

/// TASK-108: --dry-run and --json parse for both verbs and compose
/// with the scope flags.
#[test]
fn dry_run_flags_parse() {
    let cli = Cli::try_parse_from(["aida", "push", "--dry-run"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Push {
            dry_run: true,
            json: false,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["aida", "push", "--json", "--code-only"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Push {
            json: true,
            code_only: true,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["aida", "pull", "--dry-run", "--store-only"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Pull {
            dry_run: true,
            store_only: true,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["aida", "pull", "--json"]).unwrap();
    assert!(matches!(cli.command, Command::Pull { json: true, .. }));
}
