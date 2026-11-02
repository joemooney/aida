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

    let (_, fresh) =
        session::compose_headless_command(HeadlessVendor::Codex, "P", "sid", true, None, None)
            .unwrap();
    let (_, forked) = session::compose_headless_command_forking(
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
    let flags = crate::tool_contained_flags("codex").unwrap();
    assert_eq!(&forked[..flags.len()], &flags[..]);
    assert_eq!(forked, session::codex_exec_fork_args(fresh, "T").unwrap());

    // Uncontained keeps the historical bypass posture, forked or not.
    let (_, open) = session::compose_headless_command_forking(
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

// `aida advisor register` records a Codex thread only when there is no
// explicit id and no Claude session id; Claude keeps precedence.
#[test]
fn register_identity_prefers_claude_then_codex_thread() {
    use crate::advisor_cmd_root::resolve_register_identity as id;
    assert_eq!(
        id(Some("x"), Some("c".into()), Some("t".into())),
        Some(("x".to_string(), None))
    );
    assert_eq!(
        id(None, Some("c".into()), Some("t".into())),
        Some(("c".to_string(), None))
    );
    assert_eq!(
        id(None, None, Some("t".into())),
        Some(("t".to_string(), Some("codex".to_string())))
    );
    assert_eq!(id(None, None, None), None);
}
