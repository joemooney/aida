//! STORY-3181 S1: the automatic train refuses every supervised PR unless the
//! default branch's committed `[landing] auto_train_supervised_under_mandate`
//! allows it, and in this slice it still refuses when it is on.
// trace:STORY-3181 | ai:claude
use super::*;

struct Automatic(bool);
impl Automatic {
    fn enter() -> Self {
        Self(ADVISOR_LOOP.with(|v| v.replace(true)))
    }
}
impl Drop for Automatic {
    fn drop(&mut self) {
        ADVISOR_LOOP.with(|v| v.set(self.0));
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A main clone whose committed config is `committed`, with `worktree`
/// written over it uncommitted.
fn clone_with(committed: &str, worktree: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    git(r, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(r.join(".aida")).unwrap();
    std::fs::write(r.join(".aida/config.toml"), committed).unwrap();
    git(r, &["add", "."]);
    git(
        r,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "base",
        ],
    );
    if let Some(body) = worktree {
        std::fs::write(r.join(".aida/config.toml"), body).unwrap();
    }
    dir
}

fn cover(root: &Path, read_only: bool) -> Option<MemberGateError> {
    let forge = crate::forge::fake::RecordingForge::new();
    let mut checks = LiveChecks {
        project_root: root,
        main_worktree: root,
        forge: &forge,
        base: "main",
        named: false,
        read_only,
        actor_probe: None,
        request_full: false,
        released_member: None,
    };
    checks
        .mandate_cover(7, &["BUG-1".into()], "b", &"a".repeat(40))
        .err()
        .expect("the automatic pass never covers a supervised PR in S1")
}

fn refusal(error: Option<MemberGateError>) -> String {
    match error {
        Some(MemberGateError::Refused(why)) => why,
        other => panic!("expected a settled refusal naming the knob, got {other:?}"),
    }
}

#[test]
fn story3181_knob_off_by_default_refuses_the_automatic_pass_and_prescan() {
    let _auto = Automatic::enter();
    let clone = clone_with("[landing]\nmode = \"ci-gated\"\n", None);
    for read_only in [false, true] {
        let why = refusal(cover(clone.path(), read_only));
        assert!(
            why.contains("`[landing] auto_train_supervised_under_mandate` is off"),
            "read_only={read_only}: {why}"
        );
    }
    // No trusted config at all is the default: off.
    let bare = tempfile::tempdir().unwrap();
    assert!(refusal(cover(bare.path(), false)).contains("is off"));
}

#[test]
fn story3181_a_local_only_edit_does_not_turn_the_knob_on() {
    let _auto = Automatic::enter();
    let clone = clone_with(
        "[landing]\n",
        Some("[landing]\nauto_train_supervised_under_mandate = true\n"),
    );
    assert!(refusal(cover(clone.path(), false)).contains("is off"));
}

#[test]
fn story3181_knob_on_is_not_yet_a_landing_path() {
    let _auto = Automatic::enter();
    let clone = clone_with(
        "[landing]\nauto_train_supervised_under_mandate = true\n",
        None,
    );
    for read_only in [false, true] {
        let why = refusal(cover(clone.path(), read_only));
        assert!(
            why.contains("auto_train_supervised_under_mandate` is on, but"),
            "read_only={read_only}: {why}"
        );
    }
}

/// The refusal reaches the member gate as a settled spec-policy refusal
/// that names the knob, so the member is ejected, not retained.
#[test]
fn story3181_the_knob_refusal_is_the_supervised_gate_reason() {
    let _auto = Automatic::enter();
    let clone = clone_with("[landing]\n", None);
    let why = refusal(Some(
        refusal_through_policy(cover(clone.path(), false)).expect("refused"),
    ));
    assert!(
        why.contains("The mandate does not cover it") && why.contains("is off"),
        "{why}"
    );
}

fn refusal_through_policy(error: Option<MemberGateError>) -> Option<MemberGateError> {
    // Feed the cover's refusal through the supervised gate's composition
    // with one supervised record.
    let mode: crate::gate::unset_mode::EffectiveMode = Some(aida_core::ExecutionMode::Drive).into();
    member_spec_policy_refusal(vec![("BUG-1".to_string(), Vec::new(), mode)], false, || {
        Err(error)
    })
}
