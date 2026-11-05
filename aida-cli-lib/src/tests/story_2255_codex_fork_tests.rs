//! STORY-2255: Codex fork-from-live argv — `codex exec fork <thread> <prompt>`
//! keeps the fresh launch's posture, contained or not.
// trace:STORY-2255 | ai:claude
use crate::session::{self, HeadlessVendor};
use std::path::Path;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|a| a.to_string()).collect()
}

// AC1: only the subcommand and the thread positional change.
#[test]
fn exec_fork_rewrite_inserts_subcommand_and_thread() {
    let fresh = session::codex_headless_args_with_model_and_effort("PROMPT", Some("m"), None);
    let forked = session::codex_exec_fork_args(fresh.clone(), "T-1").unwrap();
    let mut expected = fresh.clone();
    expected.insert(1, "fork".to_string());
    expected.insert(expected.len() - 1, "T-1".to_string());
    assert_eq!(forked, expected);
    assert_eq!(&forked[..2], &s(&["exec", "fork"])[..]);
    assert_eq!(&forked[forked.len() - 2..], &s(&["T-1", "PROMPT"])[..]);

    // Posture flags before `exec` stay where they were.
    let contained = s(&["--sandbox", "workspace-write", "exec", "--json", "P"]);
    assert_eq!(
        session::codex_exec_fork_args(contained, "T").unwrap(),
        s(&[
            "--sandbox",
            "workspace-write",
            "exec",
            "fork",
            "--json",
            "T",
            "P"
        ])
    );

    assert!(session::codex_exec_fork_args(s(&["exec", "P"]), " ").is_err());
    assert!(session::codex_exec_fork_args(s(&["-p", "P"]), "T").is_err());
}

// A fork thread on a non-Codex vendor is refused, never silently ignored.
#[test]
fn compose_forking_refuses_non_codex_vendor() {
    let err = session::compose_headless_command_forking(
        crate::seat_clone::launcher_main().as_deref(),
        HeadlessVendor::Claude,
        "p",
        "sid",
        false,
        None,
        None,
        Some("T"),
    )
    .unwrap_err();
    assert!(err.to_string().contains("only a codex thread"), "{err}");
}

fn git(root: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
}

fn native_codex_policy(root: &Path) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[agents]\nvendor = \"codex\"\n[contained]\nenable = true\n[contained.vendors]\ncodex = \"native\"\n",
    )
    .unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["add", ".aida/config.toml"]);
    git(root, &["commit", "-q", "-m", "trusted policy"]);
    git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
}

// AC4: under containment the fork argv carries the native-sandbox flags and
// no bypass flag, exactly like the fresh contained launch.
#[test]
fn contained_fork_uses_the_native_posture_without_bypass() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", None),
        ("AIDA_OS_WRAP", Some("0")),
        ("AIDA_AGENT_CMD", Some("codex")),
    ]);
    let _cwd = session::contained_cwd_seam::pin(root.path());
    native_codex_policy(root.path());

    let (_, fresh) = session::compose_headless_command(
        None,
        HeadlessVendor::Codex,
        "P",
        "sid",
        true,
        None,
        None,
    )
    .unwrap();
    let (_, forked) = session::compose_headless_command_forking(
        crate::seat_clone::launcher_main().as_deref(),
        HeadlessVendor::Codex,
        "P",
        "sid",
        true,
        None,
        None,
        Some("T"),
    )
    .unwrap();
    assert!(
        !forked.iter().any(|a| a.contains("dangerously-bypass")),
        "{forked:?}"
    );
    let flags = crate::tool_contained_flags(None, "codex").unwrap();
    assert_eq!(&forked[..flags.len()], &flags[..]);
    assert_eq!(forked, session::codex_exec_fork_args(fresh, "T").unwrap());

    // The punt-tier advisor fork keeps the contained posture the same way.
    let tier = |is_fork| {
        session::advisor_tier_program_and_args_with_posture(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            is_fork,
            "P",
            "T",
            None,
            None,
            Some(root.path()),
        )
        .unwrap()
        .1
    };
    let (fresh_tier, forked_tier) = (tier(false), tier(true));
    assert!(
        !forked_tier.iter().any(|a| a.contains("dangerously-bypass")),
        "{forked_tier:?}"
    );
    assert_eq!(&forked_tier[..flags.len()], &flags[..]);
    assert_eq!(
        forked_tier,
        session::codex_exec_fork_args(fresh_tier, "T").unwrap()
    );

    // Uncontained keeps the historical bypass posture, forked or not.
    let (_, open) = session::compose_headless_command_forking(
        crate::seat_clone::launcher_main().as_deref(),
        HeadlessVendor::Codex,
        "P",
        "sid",
        false,
        None,
        None,
        Some("T"),
    )
    .unwrap();
    assert!(open
        .iter()
        .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
}

// `aida advisor register`: an explicit `--uuid` registers as `--vendor`
// (Claude by default). With one env id set, that session registers. With both
// set, one was inherited, so the vendor nearest in the process tree wins; an
// inconclusive tree keeps the Claude session (Claude must not regress).
// trace:TASK-2754 | ai:claude
#[test]
fn register_identity_prefers_explicit_then_the_innermost_vendor() {
    use crate::advisor_cmd_root::{resolve_register_identity as id, InnermostVendor};
    let codex = || Some("codex".to_string());
    let both = || (Some("c".to_string()), Some("t".to_string()));
    assert_eq!(
        id(Some("x"), None, Some("c".into()), Some("t".into()), None),
        Some(("x".to_string(), None))
    );
    assert_eq!(
        id(Some("x"), Some("claude"), None, None, None),
        Some(("x".to_string(), None))
    );
    assert_eq!(
        id(Some("x"), Some("codex"), Some("c".into()), None, None),
        Some(("x".to_string(), codex()))
    );
    // A Codex session started from a Claude seat: Codex is innermost.
    let (c, t) = both();
    assert_eq!(
        id(None, None, c, t, Some(InnermostVendor::Codex)),
        Some(("t".to_string(), codex()))
    );
    // A Claude session started from a Codex shell inherits CODEX_THREAD_ID.
    let (c, t) = both();
    assert_eq!(
        id(None, None, c, t, Some(InnermostVendor::Claude)),
        Some(("c".to_string(), None))
    );
    let (c, t) = both();
    assert_eq!(id(None, None, c, t, None), Some(("c".to_string(), None)));
    assert_eq!(
        id(
            None,
            None,
            Some("c".into()),
            None,
            Some(InnermostVendor::Codex)
        ),
        Some(("c".to_string(), None))
    );
    assert_eq!(
        id(
            None,
            None,
            None,
            Some("t".into()),
            Some(InnermostVendor::Claude)
        ),
        Some(("t".to_string(), codex()))
    );
    assert_eq!(id(None, None, None, None, None), None);
}

// The rollout search walks the date tree newest-first and stops at the first
// match; a planted duplicate in an older day proves the order.
// trace:TASK-2755 | ai:claude
#[test]
fn rollout_search_is_newest_first() {
    use crate::advisor::find_rollout_newest_first as find;
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    let plant = |day: &str, name: &str| {
        let dir = sessions.join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "{}").unwrap();
        path
    };
    plant("2025/12/31", "rollout-2025-12-31T09-00-00-T1.jsonl");
    plant("2026/01/09", "rollout-2026-01-09T09-00-00-other.jsonl");
    let newest = plant("2026/01/10", "rollout-2026-01-10T09-00-00-T1.jsonl");
    plant("2026/01/02", "rollout-2026-01-02T09-00-00-T1.jsonl");
    assert_eq!(find(&sessions, "T1"), Some(newest));
    assert_eq!(find(&sessions, "missing"), None);
    assert_eq!(find(&tmp.path().join("absent"), "T1"), None);
}
