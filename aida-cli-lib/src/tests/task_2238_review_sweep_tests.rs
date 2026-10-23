//! TASK-2238: post-merge review notes swept from the 2026-10-23 PRs. The
//! mandate, replay and `search --fields` items are tested beside their code;
//! this file pins the always-loaded docs and the mailbox/hook wiring.
//!
//! Docs are read AT RUNTIME from the workspace root, so editing one back to
//! the old wording fails here.
// trace:TASK-2238 | ai:claude

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("aida-cli-lib always has a workspace parent")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{} must be readable: {err}", path.display()))
}

/// TASK-2225: under the LIGHT gate, `make check-ci-compile` after a main
/// merge is conditional, so the root guide must not list it as unconditional.
#[test]
fn root_guide_lists_check_ci_compile_as_conditional() {
    let guide = read("CLAUDE.md");
    let line = guide
        .lines()
        .find(|l| l.starts_with("make check-ci-compile"))
        .expect("CLAUDE.md lists make check-ci-compile");
    assert!(!line.contains("after merging main into a branch"), "{line}");
    assert!(line.contains(">10 files"), "{line}");
}

/// TASK-2227: a trace tag sits at the end of the rendered step-2 bullet, not
/// in the middle of its prose.
#[test]
fn aida_review_step_two_trace_tag_ends_the_bullet() {
    let skill = read("aida-core/templates/skills/aida-review.md");
    let step = skill
        .lines()
        .find(|l| l.starts_with("2. **Gate on CI, not local cargo**"))
        .expect("aida-review step 2");
    assert!(
        step.trim_end().ends_with("trace:TASK-2209 | ai:claude"),
        "{step}"
    );
    assert_eq!(step.matches("trace:").count(), 1, "{step}");
}

/// TASK-2228: the replay README says the collector runs once and only the
/// diff call is compared under the other rename configs.
#[test]
fn replay_readme_does_not_overstate_the_per_config_runs() {
    let readme = read("tests/fixtures/task-1800-replay/README.md");
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        !flat.contains("under `diff.renames` unset, `false` and `copies`"),
        "{readme}"
    );
    assert!(
        flat.contains("runs the production collector once"),
        "{flat}"
    );
    assert!(flat.contains("`diff_tree_args`"), "{flat}");
}

/// TASK-2236: the operator escape in a refusal comes through
/// `RecipientDirectory::load` from the real roster and `AIDA_USER`, end to
/// end, not from a hand-set field.
#[test]
fn refusal_hint_operator_comes_from_aida_user_and_the_roster() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".aida-store");
    std::fs::create_dir_all(store.join("registry")).unwrap();
    std::fs::write(
        store.join("registry/team.toml"),
        "[members]\nZara = []\nAdvisor = [\"operator\"]\n",
    )
    .unwrap();
    let hint = |aida_user: Option<&str>| {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_USER", aida_user)]);
        let directory = crate::mailbox_resolver::RecipientDirectory::load(tmp.path(), &store)
            .expect("directory loads");
        let error = directory.resolve("missing").unwrap_err().to_string();
        let target = error
            .split("--to ")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .unwrap_or_else(|| panic!("the refusal names an operator: {error}"))
            .to_string();
        assert!(directory.resolve(&target).is_ok(), "{error}");
        target
    };
    assert_eq!(hint(Some(" zara ")), "person:Zara");
    assert_eq!(hint(None), "person:Advisor");
}

/// TASK-2237: Codex's SessionStart hook runs the mail-notice relay, and a
/// Codex SessionStart payload (`hook_event_name` is the constant
/// `SessionStart` in its input schema) is recognised as a session start, so
/// the notice-time registry work runs for Codex as it does for Claude.
#[test]
fn codex_session_start_reaches_the_notice_session_start_path() {
    let hooks: serde_json::Value =
        serde_json::from_str(&read("aida-core/templates/codex-hooks.json")).unwrap();
    let commands: Vec<&str> = hooks["hooks"]["SessionStart"]
        .as_array()
        .expect("codex SessionStart hooks")
        .iter()
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .filter_map(|hook| hook["command"].as_str())
        .collect();
    assert!(
        commands.iter().any(|c| c.ends_with("aida-mail-notice.sh")),
        "{commands:?}"
    );
    let relay = read("aida-core/templates/hooks/aida-mail-notice.sh");
    assert!(relay.contains("export AIDA_HOOK_PAYLOAD"), "{relay}");
    let payload = r#"{"session_id":"019a-codex","transcript_path":null,"cwd":"/w","hook_event_name":"SessionStart","model":"gpt","permission_mode":"default","source":"startup"}"#;
    assert_eq!(
        crate::presence::parse_hook_payload(payload),
        (Some("019a-codex".to_string()), true)
    );
}

/// TASK-1709: no comment in the CLI team module is pushed out to a far
/// column beside code (rustfmt's trailing-comment alignment); the STORY-650
/// delegation note sits above the `#[cfg(test)]` guard.
#[test]
fn team_module_has_no_far_column_trailing_comments() {
    let team = read("aida-cli-lib/src/team.rs");
    for (n, line) in team.lines().enumerate() {
        let indent = line.len() - line.trim_start().len();
        assert!(
            !(indent > 40 && line.trim_start().starts_with("//")),
            "aida-cli-lib/src/team.rs:{}: {line}",
            n + 1
        );
    }
}
