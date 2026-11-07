# Claude shared-wrapper launch sketch

<!-- trace:TASK-3219 | ai:codex -->

Status: approved by the advisor's 2026-11-06 16:19 UTC spec comment, with
four separate Refs slices in order: S1 coordinator, S2 interactive PTY,
S3 background supervisor, S4 resume. S4 alone removes the remaining refusal
and carries the completing trailer. S1 uses the existing foreground headless
boundary, receipt-before-release and worker grant binding. S2 adds private-PTY
foreground agent-new on that boundary; background and resume continue to refuse. This document grants no launch or publication authority.

## Contract

With trusted `[contained.vendors] claude = "aida"`, interactive agent-new,
background, coordinator and resume sessions must run inside the existing
receipt-attested shared boundary. With that opt-in unset, retain existing
Claude-native argv, environment, grant and lifecycle behavior. Preserve the
TASK-2708 accepted WAIT_KILLABLE_RECV signal deferral, Linux 5.19+ fail-closed
floor, ancillary-credential refusal, and sendmmsg/named-sendmsg restrictions.
No broker or network-policy relaxation is proposed.

## Current seams and missing support

* `session::claude_contained_flags_for_roots` refuses all compositions outside
  `ClaudeWrapperComposition`. Removing that refusal alone would leave the
  new surfaces executing without the shared wrapper.
* `agent_launch::run_agent_generation` owns interactive spawn, grant rollback,
  registry binding, signals and wait. It currently spawns a plain Command.
* `agent_launch::build_agent_new_bg_command` and its caller dispatch Claude's
  `--bg`; the existing `refuse_bg_under_os_wrap` explains that the vendor
  daemon, rather than this dispatcher, parents the actual session. Wrapping
  this handoff cannot establish session containment.
* `session::build_coordinator_headless_command_inner` builds coordinator argv;
  `coordinator_status_with_handle_guard` finalizes and spawns it. Both need
  wrapper selection and receipt-before-release handling.
* `session::build_contained_resume_command` stages a transcript into the
  host Claude project store, then `exec_or_spawn_agent` directly execs Claude.
  The wrapper uses a fresh private config directory, and `contained::claude_argv`
  explicitly refuses resume/continue/fork flags. Merely allowing those flags
  cannot make the transcript available.
* `PreparedLaunch::prepare_for_owner` admits headless launches only;
  `PreparedLaunch::spawn` forces null stdin. Its write profile uses bwrap
  `--new-session` and a fresh `/dev`. Interactive terminal behavior needs an
  explicit tested design, not just inherited stdout.

## Proposed implementation after signoff

1. Resolve wrapper selection at each launch entry point from trusted policy,
   anchored to the launch project. Scope argv composition only around launches
   whose spawn will use `PreparedLaunch`; bypass legacy os_wrap only there.
   Remove `CLAUDE_WRAPPER_UNWIRED` after all four surfaces are wired and tested.
   Keep widening-flag validation before config/broker setup.
2. Add explicit launch I/O modes to the shared wrapper. Preserve today's
   headless mode exactly; interactive mode uses a private PTY with host-side
   terminal relay, resize and signal forwarding. Keep namespace/session
   isolation and descriptor filtering. Restore terminal state on all exits;
   grant no host `/dev/tty` mount or generic inherited descriptors.
3. Route interactive generations through prepare, attested spawn, child-worker
   grant binding, registry/lease binding, release and supervision. Keep the
   wrapper state owner alive until the vendor exits; revoke untransferred
   grants and reservations on refusal. Track the attested worker separately
   from the helper PID used for host process supervision.
4. For the opt-in background path, propose an AIDA-owned detached supervisor
   running a foreground Claude child inside the wrapper, rather than Claude's
   external daemon. It owns wrapper state, logs, signals, registry and grants
   until child exit. Acknowledge dispatch only after receipt acceptance and
   worker binding. Native `--bg` remains unchanged. This changes the opted-in
   background lifecycle and needs the advisor's explicit approval, including
   whether attach/log compatibility must be provided in this slice.
5. Route coordinator spawning through the same prepare/bind/release/wait
   sequence using the coordinator's explicit child grant, identity, cwd and
   CLI/MCP selection. Preserve quiet/stream output and reservation cleanup.
6. For explicit resume IDs, copy only the validated, bounded, singly linked
   regular transcript from the selected session's project store into the new
   private config's cwd-keyed project directory. Use handle-based no-follow
   reads and exclusive private writes. Never expose the whole host Claude
   config directory. Persist the resumed transcript on exit through an
   explicit host-side, identity-checked publication seam so a later resume
   sees new turns. Keep credentials read-only and expiry checks unchanged.
   Refuse implicit continue/fork forms until their selection and persistence
   semantics are explicitly covered. Resume must supervise rather than exec
   away the wrapper owner. Revalidate resume grant authority; do not borrow
   the launcher's grant accidentally.

## Validation plan

Add one path-level targeted regression for each of interactive, background,
coordinator and resume. Exercise the actual spawn adapter with a fake Claude
worker: require accepted receipt and own seat, cwd/write limits, selected
CLI/MCP surface and failure-before-release behavior. Interactive testing must
exercise PTY input, resize and terminal restoration; background testing must
prove the worker survives dispatcher exit while remaining confined and loses
authority on teardown; resume must prove selected-history visibility,
cross-session isolation and persistence after a second resume.

For every path, compare opt-in-unset behavior against the existing native
command and environment contract. Retain the existing headless suite and
TASK-2708 signal, old-kernel, credential and network regressions. Add new live
tests to the required-live bwrap CI invocation and exact success-count check;
neither a missing bwrap/kernel capability nor an unavailable fake worker may
silently skip when required-live is enabled. Run targeted tests,
`cargo fmt --all -- --check`, and `make check-ci-fast`; report the actual live
count and infrastructure refusals separately from unit-test results.

## Advisor decision

The 2026-11-06 16:19 UTC signoff approves private-PTY interactive transport,
the AIDA-owned background supervisor, and the narrow explicit-ID resume
import/publication lifecycle. Background access is limited to AIDA-level logs
and `aida agent attach`-style access; opted-in `claude attach` must refuse with
an explicit hint. Native background behavior stays unchanged. Continue and
fork remain refused.

The binding implementation order is S1 coordinator, S2 interactive PTY,
S3 background supervisor, S4 resume, with a separate Refs PR and targeted/live
regression for each. Only S4 removes the remaining refusal and completes the
task. This checkout implements S1 and S2; validation results belong in the spec audit
trail and host publication handoff.
