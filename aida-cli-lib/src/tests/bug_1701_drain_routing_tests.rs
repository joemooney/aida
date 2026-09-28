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
