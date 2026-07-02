# Plan: SPIKE-76 dispatch resilience

Date: 2026-07-02
Specs: SPIKE-76
Status: Draft
Complexity: ~0 prod LOC for this plan; recommended first slice ~150-300 prod LOC, ~150-250 test LOC, 1-2 commits, risk medium

## Approach

Treat the observed failures as a dispatch-discipline problem before treating them as an infrastructure problem. AIDA already has the useful primitives: CLI/TOON agent output, `aida agent new claude|codex|antigravity`, per-agent registry and liveness, worktree/session leases, `aida brief`, local/canonical mailbox, drain locks, retry helpers, idle watchdogs, and git as the durable handoff boundary. The smallest valuable design is therefore CLI-first fan-out with early durable git checkpoints, explicit resume instructions, and liveness timeouts; only add daemon-grade supervision after field evidence shows this lighter pattern still leaves real halts.

### Diagram

```
operator / advisor
  |
  | assign: brief + lease/worktree + vendor policy
  v
dispatch ledger via CLI/TOON, not MCP as the acting channel
  |
  +-- claude implementer/reviewer where native background/resume is useful
  +-- codex implementer/reviewer for bounded code + sandboxed cross-checks
  +-- antigravity draft-for-review / cross-validate / mechanical work
  |
  v
agent worktree: commit early -> push early -> PR/ship when ready
  |
  +-- progress observed: commits, dirty diff, branch push, registry heartbeat
  +-- stalled/crashed: resume same vendor or rebrief fallback vendor
  v
human/advisor integrates with existing drain lock + git two-leg discipline
```

## Decisions

- **Decision: dispatch through the AIDA CLI/TOON surface first, with MCP as a coordination/read fallback rather than the primary acting path.** **Rationale**: this session observed repeated MCP disconnect/reconnect stalls, and prior benchmarking says MCP costs about 2x. The CLI path already has token-efficient `AIDA_AGENT_OUTPUT`/TOON output, survives ordinary process restarts, and was the path used by the cross-vendor portability experiment where Codex completed a pickup with zero integration.
- **Decision: make commit-early/push-early the first recovery boundary.** **Rationale**: two isolated implementers died before committing; their work survived only as uncommitted worktree diffs. A trailered commit on the branch turns crash recovery into `git fetch`/`git log`/resume rather than hand salvage. Push-early makes the boundary survive machine/worktree loss too.
- **Decision: resume by durable state and fresh launch, not by assuming every vendor can replay a suspended tool call.** **Rationale**: Claude has `--resume`/`defer`; Codex and Antigravity do not have equivalent documented semantics. The portable pattern is to record state in the spec/brief/mailbox/branch and start a fresh agent on the same worktree or branch.
- **Decision: use existing liveness mechanisms before a daemon.** **Rationale**: AIDA already has agent registry PID liveness, pause/resume markers, stale lease reaping, drain locks with stale reclaim, CI idle timeout, event idle backstops, and phase watchdogs. A new daemon is not justified until these are composed and measured.
- **Decision: fan out across vendors by role and risk, not by equal round-robin.** **Rationale**: vendor capabilities differ. Claude is still strongest for existing headless drain/resume paths; Codex has a useful sandbox posture and a verified CLI pickup path; Antigravity should be used for draft-for-review, cross-validation, and mechanical/bounded work unless its current session proves stronger.

### Failure-Mode Catalog

| Failure mode | Evidence / source | Blast radius | First response |
|---|---|---|---|
| MCP server disconnect/reconnect stalls dispatch | Observed in this SPIKE-76 session; MCP path also has prior ~2x cost note | Any dispatch that relies on live MCP calls can stall even though local CLI still works | Route acting dispatch through CLI/TOON; keep MCP for read/coordination when healthy |
| Agent crashes before commit | Two worktree-isolated implementers died mid-flight with only dirty diffs | Work is recoverable only by hand from a local worktree; machine loss can lose it entirely | Require early WIP commit after first coherent change; push branch once tests or compile boundary is reached |
| Agent waits forever on missing background-test notification | Observed paused agent whose child process was reaped | One spec/agent can halt indefinitely; fan-out appears active but makes no progress | Add liveness timeout/resume prompt based on no progress in commits, dirty diff, logs, and child PID |
| Single-vendor fan-out | All fan-out used Claude subagents while Codex/AGY idled | Claude outage, rate limit, MCP issue, or model regression can halt the whole fleet | Dispatch at least one independent vendor on every batch: Codex for implementation/cross-check, AGY for draft/review/mechanical work |
| Vendor-specific resume semantics leak into generic orchestration | `session-communication.md` and parity inventory show Claude-only `defer`/`--resume` gaps | Non-Claude recovery either cannot resume or pretends to resume and loses context | Use branch/spec/brief state as the portable resume contract |
| Brief delivery is passive unless notified or polled | `aida brief` docs: no `--notify` means it waits until poll | Idle agents do not wake; work looks assigned but unstarted | Use `--notify` for urgent fallback briefs and require polling cadence in launch context |
| Mailbox local layer is not durable across clones until sync | `aida mailbox` docs and `mailbox_store.rs` local/canonical split | Cross-clone handoffs can be invisible after clone/machine switch | For crash/fallback handoff, prefer spec comments/branch commits/briefs; sync mailbox when it carries recovery state |
| Brief target ambiguity / paused target | Agent registry supports exact names, type-role dirs, and pause warnings | Work may be routed to an unavailable or ambiguous agent class | Dispatch by stable agent name when possible; skip paused/rate-limited agents unless explicitly overriding |
| Drain concurrency races | `drain_lock.rs` documents one drain per repo with stale reclaim | Multiple integrators can race git, worktrees, target build dir, and merges | Keep global drain lock; do not solve fan-out by starting a second drain |
| Cross-clone coordination unavailable | `drain_lock.rs` warns and proceeds local-only when store claim unavailable | Safety degrades to local lock; multiple clones may unknowingly drive work | Accept for now; surface status warning and rely on git branch/PR collision as second boundary |
| Queue work and headless drain remain Claude-bound | Parity inventory: `aida queue work` and auto-complete phase spawns hardcode Claude | A “no Claude” event still blocks autonomous drain even if manual Codex launch works | Do not promise cross-vendor autonomous drain in slice 1; use `aida agent new codex --spec`/brief path first |
| Antigravity lacks direct headless adapter in `compete` | `compete.rs` treats Antigravity as `HumanBriefed` | Cannot assume fully unattended AGY execution | Route AGY via briefs and human-reviewable drafts until headless behavior is proven |
| Dirty dormant worktree is intentionally not auto-reaped | Lease logic leaves dirty dormant work alone to avoid data loss | Crashed work can accumulate and block clean session end | Add recovery report that names dirty worktree, branch, last commit, and next resume command |

### Accept vs Engineer

Engineer now:

- **MCP acting-channel stalls**: use CLI/TOON for dispatch and status snapshots because this is already supported and directly addresses the observed stall.
- **Pre-commit crash loss**: add commit-early/push-early discipline because it is cheap, uses git, and turns hand salvage into normal recovery.
- **Indefinite waits**: add liveness/timeout/resume behavior using existing registry, lease, worktree, and watchdog signals.
- **Single-vendor halt risk**: require mixed-vendor fan-out for batches and fallback routing for unavailable vendors.
- **Passive brief pickup for urgent recovery**: use `brief --notify`, stable agent names, and explicit polling guidance.

Accept for now:

- **Full non-Claude autonomous drain parity**: expensive and already identified as a separate uncovered gap. Slice 1 can materially reduce halts without porting the whole drain.
- **Perfect cross-clone drain exclusion during store/network outage**: current drain lock intentionally warns and proceeds local-only. Keep that behavior unless real duplicate drains recur.
- **True process supervision daemon**: no evidence yet that commit/push/resume/liveness discipline is insufficient.
- **Antigravity unattended execution**: treat AGY as human-briefed/draft/cross-validation until its headless surface is verified.
- **Mailbox as the sole crash-recovery record**: mailbox is useful for conversation, but branch commits, spec comments, briefs, and PRs are stronger recovery anchors.

### Ranked Smallest-Valuable-Slice

1. **CLI-first resilient dispatch discipline**: document and wire a dispatch path that creates/uses a brief, starts `aida agent new <vendor> --spec <SPEC>`, sets `AIDA_AGENT_OUTPUT=1` for machine-readable status calls, requires an early WIP commit once a coherent edit exists, pushes the branch at the first verified boundary, and emits a resume command if liveness stalls. This is the recommended first build.
2. **Fallback rebriefing**: when a vendor is paused, dead, or idle past threshold, create a `--notify` brief for the next allowed vendor with branch/worktree/last-commit context and mark the original agent paused/rate-limited rather than waiting.
3. **Dispatch health report**: one CLI report that reads agent registry, leases, branch ahead counts, dirty worktrees, pending briefs, and paused agents to say which specs are moving, stalled, or salvageable.
4. **Cross-vendor batch policy**: add a small routing policy file or config section mapping roles to vendors and fallback order, using current launcher adapters and AGY human-briefed behavior.
5. **Headless Codex drain adapter**: only after the above is measured, port selected headless phases from Claude-only spawn helpers to a vendor adapter reused from `compete.rs`.
6. **Daemon-grade supervisor**: only if field data shows repeated halts remain after slices 1-4, and only with evidence naming the failure class it solves.

### Cross-Vendor Fan-Out + Fallback

Default routing should be mixed by batch, not all-Claude:

- **Claude**: use for existing headless drain paths, complex implementation where current Claude session context matters, and review/advisor flows that rely on known `defer`/`--resume` behavior.
- **Codex**: use for bounded implementation, independent cross-checks, and fallback implementation from a pushed branch. Honor Codex sandbox posture; prefer `aida agent new codex --spec <SPEC>` or a CLI brief pickup over MCP-only dispatch.
- **Antigravity / AGY**: use as draft-for-review, cross-validation, and mechanical/bounded work. Because `compete.rs` models AGY as human-briefed, dispatch via `aida brief antigravity <SPEC> --notify` and expect reviewable output rather than unattended merge authority.

Fallback order should be explicit per task:

- If **MCP is down**: use CLI `aida brief`, `aida agent new`, `aida status`, `aida mailbox`, `git`, and `AIDA_AGENT_OUTPUT=1` output. Do not block waiting for MCP reconnect.
- If **Claude is unavailable**: stop assigning all implementation to Claude. Rebrief Codex for bounded implementation or review; rebrief AGY for draft/cross-validation; keep integration under the existing drain lock.
- If **Codex is unavailable or sandbox blocks needed work**: rebrief Claude or AGY, preserving branch and worktree context. Do not weaken Codex sandbox by default.
- If **AGY is unavailable**: skip AGY for unattended work; use Codex or Claude and preserve AGY as optional review capacity.
- If **an agent crashes**: inspect branch/dirty worktree. If commits exist, resume from branch. If only dirty diffs exist, first salvage by committing in that worktree, then rebrief. If the machine/worktree is gone and no push exists, record the loss as a finding because the discipline failed.
- If **an agent is alive but idle**: compare registry liveness, lease worktree, dirty diff, HEAD movement, log growth, and child PID. Past threshold, send an explicit resume message or rebrief a fallback vendor; do not wait indefinitely.

## Files (in build-order)

### `docs/plans/2026-07-02-spike-76-dispatch-resilience.md` (new) — design plan

- Capture the failure catalog, accept-vs-engineer split, smallest slice, and cross-vendor fallback policy for SPIKE-76.

### Future slice files, if approved

### `aida-cli/src/main.rs` — dispatch health/report and launcher composition

- Reuse `run_tracked_agent`, `agent_new_codex`, `agent_new_antigravity`, `agent_new_claude`, launch context rendering, and existing status helpers.
- Add only a thin status/resume/report surface if the first slice is approved.

### `aida-cli/src/agent_registry.rs` — availability and liveness inputs

- Reuse `list_agent_views`, `pause_agent`, `resume_agent`, `resolve_brief_directories`, and paused target warnings.

### `aida-cli/src/session.rs` / `aida-cli/src/compete.rs` — future vendor adapter extraction

- Reuse `resolve_agent_program` and `compete::vendor_adapter` prior art only if moving beyond the discipline slice into headless adapter work.

### `docs/agents/*.md` — operating guidance

- Update Codex/Antigravity/Claude pickup docs only after implementation is approved, keeping the CLI-first recovery instructions consistent across vendors.

## Critical Files

- `docs/plans/2026-07-02-spike-76-dispatch-resilience.md`
- `docs/agents/session-communication.md`
- `docs/agents/claude-surfaces-codex-parity.md`
- `docs/agents/cross-agent-onboarding.md`
- `docs/agents/codex-brief-pickup.md`
- `docs/agents/antigravity-brief-pickup.md`
- `docs/git-verb-surface.md`
- `aida-cli/src/main.rs`
- `aida-cli/src/agent_registry.rs`
- `aida-cli/src/mailbox_store.rs`
- `aida-cli/src/drain_lock.rs`
- `aida-cli/src/compete.rs`
- `aida-cli/src/network_retry.rs`
- `aida-cli/src/ci_idle_timeout.rs`
- `aida-cli/src/event_wait.rs`
- `aida-cli/src/exit_signal.rs`

## Reusable helpers (do not reimplement)

- `run_tracked_agent` (`aida-cli/src/main.rs`) — foreground launch with cwd/env, registry entry, signal forwarding, and cleanup.
- `render_agent_launch_context` / `render_launch_mailbox_section` (`aida-cli/src/main.rs`) — vendor-neutral startup snapshot and mailbox polling guidance.
- `agent_registry::list_agent_views`, `pause_agent`, `resume_agent`, `paused_warning_for_target`, `resolve_brief_directories` — liveness, availability, and routing primitives.
- `mailbox_store::read_local_messages`, `read_canonical_messages`, `digest_local_to_canonical` — hybrid mailbox layers.
- `drain_lock::acquire_drain_lock` — one-integrator-at-a-time guard with stale reclaim and cross-clone claim behavior.
- `network_retry::run_with_retry` / `classify_transient` — transient network retry policy for git/gh-like subprocesses.
- `ci_idle_timeout::ci_wait_verdict` and `event_wait::wait_for_actionable` — existing idle/absolute timeout patterns.
- `exit_signal::spawn_and_wait_watched` and the phase watchdog pattern in `main.rs` — no-progress and process reaping behavior for long child runs.
- `compete::vendor_adapter` / `headless_argv` — prior art for Claude/Codex headless adapters and AGY human-briefed routing.
- Git two-leg verbs from `docs/git-verb-surface.md` — preserve code/store leg discipline; do not create a new sync convention.

## Risks + gotchas

1. **Risk**: WIP commits could lower code quality or clutter history. **Mitigation**: use clearly marked local branch commits and squash before merge; the recovery value outweighs local branch neatness.
2. **Risk**: push-early may expose incomplete work. **Mitigation**: push to feature branches only, keep PR draft/withheld until ready, and use trailered subjects so recovery stays attributable.
3. **Risk**: CLI-first dispatch could underuse MCP's neutral tool surface. **Mitigation**: keep MCP for healthy read/coordination paths, but never make live MCP connectivity the only acting channel.
4. **Risk**: timeout thresholds can kill slow but valid work. **Mitigation**: reset on concrete progress signals: HEAD movement, dirty status changes, log growth, PR/check progress, and mailbox/brief acknowledgement.
5. **Risk**: fallback rebriefing can duplicate work. **Mitigation**: route with leases, branch names, and explicit “continue from this commit/worktree” context; pause the original agent before fallback when possible.
6. **Risk**: vendor policy may become stale as Codex/AGY capabilities change. **Mitigation**: keep policy data-driven and refresh docs when a vendor gains headless resume or stronger sandbox controls.
7. **Risk**: store-less or disconnected worktrees reduce spec visibility. **Mitigation**: for this slice, rely on branch commits, launch context, and CLI status; do not introduce parallel requirement files.
8. **Risk**: daemon pressure returns after the next incident. **Mitigation**: require a failure record showing commit/push/resume/liveness did not cover the incident before approving daemon-grade infrastructure.

## Tests (named, not "add tests")

- `dispatch_report_marks_dead_agent_dirty_worktree_salvageable` — dead PID plus dirty worktree yields salvage/resume guidance, not auto-delete.
- `dispatch_report_marks_pushed_branch_resumable` — branch ahead of main with pushed commits produces a fresh-launch resume command.
- `dispatch_report_skips_paused_vendor_for_fallback` — paused/rate-limited vendor is not selected unless forced.
- `dispatch_policy_routes_agy_as_human_briefed` — AGY fallback emits a notify brief rather than a headless spawn.
- `dispatch_liveness_resets_on_dirty_diff_or_commit` — no-progress timer does not fire when worktree state changes.
- `dispatch_liveness_fires_on_dead_child_no_progress` — reaped child plus unchanged worktree/log reports stalled.
- `dispatch_cli_output_uses_toon_in_agent_mode` — machine status path honors `AIDA_AGENT_OUTPUT=1`.

## Verification

Future implementation smoke should avoid real drains and real store writes where possible:

```bash
TMP=$(mktemp -d)
cd "$TMP"
git init
aida init --no-skills --no-hooks --no-agent-config
aida add --title "resilience smoke" --type task --status approved
aida brief codex TASK-1 --notify --note "bounded smoke"
AIDA_AGENT_OUTPUT=1 aida brief list --for-agent codex
aida agent new codex --spec TASK-1 --show-context
# Expected: dry preview only; no session started, no worktree/lease created.
```

For liveness/report tests, use temp repos and fake registry entries rather than launching real Claude/Codex/AGY processes. For git durability tests, create local commits and bare remotes in `/tmp`; do not touch the real AIDA store. Do not run `aida queue work --auto-complete`, drain, or zen commands as part of this SPIKE-76 plan validation.

## Followups

- Implement CLI-first dispatch health report with resume/fallback hints.
- Add a documented commit-early/push-early policy to vendor pickup docs.
- Add cross-vendor routing config for role-to-vendor fallback order.
- Port selected headless phases to a Codex adapter only after discipline telemetry shows remaining halt risk.
- Repeat the cross-vendor portability experiment for AGY and one additional vendor.

## Related

- Builds on: `docs/agents/session-communication.md`
- Builds on: `docs/agents/claude-surfaces-codex-parity.md`
- Builds on: `docs/agents/cross-agent-onboarding.md`
- Builds on: `docs/git-verb-surface.md`
- See also: `docs/research/ablations/2026-06-19-cross-vendor-portability.md`
- See also: `docs/research/ablations/README.md`
- See also: `docs/research/2026-06-16-layered-evaluation-framework.md`
