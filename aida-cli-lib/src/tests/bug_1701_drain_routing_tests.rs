// BUG-1701: `aida agent new --spec <ID>` must route a drain-groomed spec to the
// one-shot lane instead of spawning an interactive TUI that holds its caller.
// Pure over the execution mode, so both the refusal and the fail-open default are
// asserted without a store fixture or a spawned agent.
// trace:BUG-1701 | ai:claude

use super::{drain_mode_agent_new_refusal, extract_aida_suggestion};
use aida_core::ExecutionMode;

#[test]
fn drain_mode_is_refused_and_points_at_the_one_shot_lane() {
    let msg = drain_mode_agent_new_refusal(Some(ExecutionMode::Drain), "BUG-1701", true)
        .expect("a drain-groomed spec must be refused");
    assert!(msg.contains("BUG-1701"), "names the spec: {msg}");
    assert!(
        msg.contains("aida do BUG-1701"),
        "must point at the one-shot lane: {msg}"
    );
    assert!(
        msg.contains("--auto-complete"),
        "must name what `aida do` resolves to for drain: {msg}"
    );
    assert!(msg.contains("--force"), "must name the escape hatch: {msg}");
}

// The foreground lane blocks its caller; `--bg` does not. The message must claim
// only what is true of the lane actually being refused.
#[test]
fn only_the_blocking_lane_claims_to_hold_the_caller() {
    let fg = drain_mode_agent_new_refusal(Some(ExecutionMode::Drain), "BUG-1701", true).unwrap();
    assert!(
        fg.contains("holds your caller"),
        "foreground must say it blocks: {fg}"
    );
    let bg = drain_mode_agent_new_refusal(Some(ExecutionMode::Drain), "BUG-1701", false).unwrap();
    assert!(
        !bg.contains("holds your caller"),
        "--bg detaches, so it must NOT claim to block: {bg}"
    );
    assert!(
        bg.contains("aida do BUG-1701"),
        "--bg is still the wrong lane and must be redirected: {bg}"
    );
}

// Every other mode legitimately wants a human-attended seat, which is what this
// lane provides. Refusing them would remove the only lane they have.
#[test]
fn every_supervised_mode_is_allowed_through() {
    for mode in [
        ExecutionMode::Drive,
        ExecutionMode::Guided,
        ExecutionMode::Operator,
        ExecutionMode::Decide,
    ] {
        assert!(
            drain_mode_agent_new_refusal(Some(mode), "TASK-1", true).is_none(),
            "{mode} must not be refused by the drain routing guard"
        );
    }
}

// An ungroomed spec must keep working — the guard fails OPEN, so adding it cannot
// break a spec nobody has set a mode on.
#[test]
fn ungroomed_spec_is_allowed_through() {
    assert!(drain_mode_agent_new_refusal(None, "TASK-1", true).is_none());
}

// The refusal is consumed by agents through the agent-mode TOON block, which
// keeps ONLY the first line as `error:` and lifts the first BACKTICK-quoted
// `aida ...` command as `help:`. The first draft of this message recommended
// `aida do` unbackticked, so `help:` advertised the lower-level
// `aida queue work ... --auto-complete` instead and the primary recommendation
// was dropped. Pin both halves of that contract.
// trace:BUG-1701 | ai:claude
#[test]
fn first_line_stands_alone_because_agent_mode_shows_only_that() {
    for holds_caller in [true, false] {
        let msg =
            drain_mode_agent_new_refusal(Some(ExecutionMode::Drain), "TASK-1499", holds_caller)
                .unwrap();
        let first = msg.lines().next().unwrap();
        assert!(
            first.contains("TASK-1499") && first.contains("aida do TASK-1499"),
            "the first line alone must name the spec AND the lane to use \
             (holds_caller={holds_caller}): {first}"
        );
    }
}

// trace:BUG-1701 | ai:claude
#[test]
fn agent_mode_help_line_points_at_aida_do_not_the_lower_level_command() {
    let msg = drain_mode_agent_new_refusal(Some(ExecutionMode::Drain), "TASK-1499", true).unwrap();
    assert_eq!(
        extract_aida_suggestion(&msg).as_deref(),
        Some("aida do TASK-1499"),
        "the FIRST backticked `aida ...` command is what agent mode shows as `help:`; \
         it must be the one-shot lane, not `aida queue work`"
    );
}

// The guard must FAIL OPEN on every path that is not "this spec is drain". An
// independent review of PR #2249 found a `?` on the store lookup, which turned an
// unreadable store into an aborted launch — the guard becoming a new way for the
// whole fleet to fail. These assert the surrounding guard, not just the pure
// decision, because that is where the defect was.
// trace:BUG-1701 | ai:claude
#[test]
fn guard_passes_through_when_the_project_has_no_store() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        super::drain_mode_agent_new_guard(dir.path(), "TASK-1", false, true).is_ok(),
        "no store must not block a launch"
    );
}

// A directory holding a malformed `.aida-store` also passes through.
//
// HONEST SCOPE: this does NOT cover the store-LOOKUP error path. I wrote a
// version of this test that claimed to, then checked it by restoring the `?` —
// it still passed, so it was vacuous. This fixture returns early at
// `detect_distributed_store_from`, never reaching the lookup, and I could not
// construct a fixture where the store resolves and the backend opens but the
// lookup then errors. The `let Ok(Some(..))` in the guard is therefore a
// DEFENSIVE match on a path no unit test here reaches; it is kept because the
// guard's contract is to redirect a drain spec, never to become a new way for a
// launch to fail, and `?` made that contract false. Do not add an assertion here
// implying the error path is covered without first proving the test fails with
// `?` restored.
// trace:BUG-1701 | ai:claude
#[test]
fn guard_passes_through_when_the_store_directory_is_malformed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida-store/objects")).unwrap();
    std::fs::write(
        dir.path().join(".aida-store/objects/junk.yaml"),
        "not: [valid",
    )
    .unwrap();
    assert!(
        super::drain_mode_agent_new_guard(dir.path(), "TASK-1", false, true).is_ok(),
        "a malformed store must fail OPEN, not abort the launch"
    );
}

// `--force` short-circuits before any store work at all.
// trace:BUG-1701 | ai:claude
#[test]
fn force_bypasses_the_guard_entirely() {
    let dir = tempfile::tempdir().unwrap();
    assert!(super::drain_mode_agent_new_guard(dir.path(), "TASK-1", true, true).is_ok());
}

// AC4 coverage, added because an independent review of PR #2249 asked for
// "an assertion against the actual help output". AC4 is satisfied by PROSE, and
// before this nothing pinned it — so an innocent reword could silently drop the
// disclosure and AC4 would regress with every gate still green. That risk is not
// hypothetical: this very session edited that paragraph.
// trace:BUG-1701 | ai:claude
fn agent_new_long_help() -> String {
    use clap::CommandFactory;
    let mut cli = crate::cli::Cli::command();
    let agent = cli
        .get_subcommands_mut()
        .find(|c| c.get_name() == "agent")
        .expect("agent subcommand exists");
    let new = agent
        .get_subcommands_mut()
        .find(|c| c.get_name() == "new")
        .expect("agent new subcommand exists");
    new.render_long_help().to_string()
}

// trace:BUG-1701 | ai:claude
#[test]
fn agent_new_help_discloses_that_the_lane_blocks_until_the_tui_exits() {
    let help = agent_new_long_help();
    assert!(
        help.contains("BLOCKS"),
        "AC4: the help must say the lane blocks:\n{help}"
    );
    assert!(
        help.contains("does not exit when its turn ends"),
        "AC4: the help must say WHY it keeps blocking — a TUI does not exit when its \
         turn ends. Without the reason the operator reads the block as a hang:\n{help}"
    );
}

// The help must not send the operator to the `status` column for the
// finished-vs-working question: `classify_status` reports `busy` whenever any live
// lease covers the worktree (BUG-1704), so that advice actively misleads. It used
// to say exactly that. Pin the correction.
// trace:BUG-1701 | ai:claude
#[test]
fn agent_new_help_points_at_cpu_not_status_for_finished_vs_working() {
    let help = agent_new_long_help();
    assert!(
        help.contains("CPU column"),
        "the help must name the signal that actually answers it:\n{help}"
    );
    assert!(
        !help.contains("shows `idle` for a seat"),
        "the help must NOT claim `aida agent status` reports idle for a waiting seat \
         — BUG-1704 shows it reports busy:\n{help}"
    );
}
