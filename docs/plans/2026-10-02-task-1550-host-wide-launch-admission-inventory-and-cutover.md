# TASK-1550: Host-wide AIDA launch admission inventory and cutover manifest

**Status:** Read-only inventory and design record. **No protection is deployed.** Nothing
in this document changes a live launcher, credential, scheduler or service, and nothing
in it authorizes installation, credential migration, deployment or merge.<br>
**Date:** 2026-10-02 (host observed and source inspected this day)<br>
**Owning spec:** TASK-1550 (blocked-by TASK-1549, Completed)<br>
**Source HEAD inspected:** `ad7ecdb79b` (120 commits after the signed sketch's anchor
`854b7e26f7`; per ADR-61, symbols are authoritative and every line anchor here was
refreshed against this HEAD)<br>
**Host observed:** Joe's workstation (`Linux 7.0.0-30-generic`, 6 cores, 70 GiB RAM,
`/dev/kvm` present), 2026-10-02 17:20–17:35 -07:00

**Design authority this document operates under (links required by acceptance):**

* **ADR-61** — P-enrolled-host protected launch boundary (`aida show ADR-61 --full`;
  companion `docs/plans/2026-09-27-adr-61-p-enrolled-host-protected-launch-boundary.md`).
* Independent architecture signoff `01a0e11e-12db-7cd0-a3d1-39df79961a63`, APPROVED as
  amended with **binding amendments M1–M5**, architecture only.
* Revised signed sketch `01a0e116-d863-7bd3-9a94-faa8e2c3888c` (P-enrolled-host).
* Operator decisions `01a0e0a3` (literal same-user anti-forgery), `01a0e103` (host-wide
  scope, unmanaged clients excluded), `01a0e10c` (P-enrolled-host accepted IN PRINCIPLE,
  design authorization only), `01a0e125` (implementation hold on BUG-1669).
* TASK-1549 — the ADR record; SPIKE-91 — sibling deliverable (backend, principals,
  M3 request encoding, M4 failure model, disposable proof plan).

Nothing below claims any gate beyond gate 1 is closed. Gates 2–5 of ADR-61 §6 remain
OPEN. Every "protected admission" cell in the tables below describes the **target**
disposition under the not-yet-built boundary, never present behaviour. Present behaviour
on every full-access route is same-user-forgeable corroboration — that is the open
defect BUG-1669 tracks.

---

## 1. Launch entry-point inventory

The unit of coverage is the **launch route** (ADR-61 §2). Routes split into: production
Rust CLI code paths (1.1, groups A–G refreshed from the BUG-1669 trail), other shipped
launch surfaces (1.2), and this host's live configured routes (1.3). Each route maps to
its **effective resolved vendor posture** and its **target disposition**: `broker`
(becomes a thin proposal/status/attach client of a typed protected operation), `refuse`
(typed nonzero refusal, no fallback), or `non-launch` (confers no authority; must be
tested to confirm it cannot later supply any).

### 1.1 Production Rust CLI routes (verified at HEAD `ad7ecdb79b`)

Paths relative to `aida-cli-lib/src`. "Posture" is the effective resolved posture today.

#### A. Compete / zen bake-off — target: broker (compete root operation)

| Route | Site | Posture today | Target |
|---|---|---|---|
| Compete arm (Claude/Codex) | `compete_cmd.rs::run_compete_arm` :821 (spawn :895), argv from `compete.rs::vendor_adapter` :55 / `headless_argv` :79 | hard-coded bypass (`--permission-mode bypassPermissions` / `--dangerously-bypass-approvals-and-sandbox`) :61–69 | broker: one approved root covers the named arm+judge set; reservation precedes the destructive worktree/branch removal that today runs before spawn |
| Compete judge / zen judge | `run_compete_judge` :319, `spawn_judge` :795 (spawn :803), argv `compete.rs::judge_command` :521 (bypass :533–538) | hard-coded bypass; `AIDA_COMPETE_JUDGE` :800 swaps executable, not authority | broker: judge sees only admitted arm output manifests; a denied arm stops the judge (refusal propagates) |
| Zen compete | `handle_zen_compete` :415 | as above | broker, same root as its arms |
| Antigravity compete adapter | emits a human-run brief, no process | n/a | non-launch (brief text is not authority) |

#### B. Intake — target: broker (intake-propose / intake-apply operations)

| Route | Site | Posture today | Target |
|---|---|---|---|
| `aida intake assess` (also groom/advisor-assess routing), propose/apply, `--then-drain` | `lib.rs::handle_intake_command` :58017; `mode = permission_mode.unwrap_or("bypassPermissions")` :58162, spawn :58204 | bypass by default | broker: propose and apply are distinct typed operations; intake+then-drain is a bundle with frozen members; an apply denial must not fall through into a drain |

#### C. Burndown — target: refuse until the broker-mediated wave controller exists

| Route | Site | Posture today | Target |
|---|---|---|---|
| Quiet run | `lib.rs::handle_burndown_run` :58497 (bypass default :58657) | bypass | refuse (not grandfathered, ADR-61 decision 10) until replaced by the mediated wave controller |
| Verbose run | `run_burndown_verbose` :60175 (spawn :60217), argv `burndown_verbose_claude_args` :59601 | bypass | same |
| Continuation loop | `MAX_RESUME_ROUNDS` :58774, spawn :58801 | bypass, up to 3 resumes | same; continuations share the root's fixed membership/deadline/counters |
| Present guard | `has_dispatch_authority` :52710 + companion/lock checks; `--force` skips the role guard | role/env-derived | retained as workflow constraints; grants nothing (AC2) |

#### D. Shared headless sinks and their callers — target: broker (per-caller typed operations)

Sinks (`session.rs`): `spawn_claude_headless` :1060 → `spawn_vendor_headless` :1174 →
`spawn_vendor_headless_with_seat` :1194 (spawn :1236), composition `compose_headless_command`
:1134; `exec_vendor_headless` :2776 (spawn :2822); `spawn_claude_headless_resume` :3038
(spawn :3065); argv `headless_vendor_args` :1854 — the Codex and Agy builders still emit
bypass regardless of the `contained` boolean, which is why admission must gate the
**resolved** posture, never a flag.

| Caller | Site | Target operation |
|---|---|---|
| Research | `lib.rs::handle_research_command` :8639 (call :8730) | broker: research root |
| Remedy | `questions_remedy` :9101 (:9137; temporarily assumes advisor role) | broker: remedy root; role assumption grants nothing |
| Dry-run AI pass | `run_dryrun_ai_pass` :62796 (:62835) | broker; keep its narrower TTY/headless workflow restriction — a grant is not permission to remove it |
| Intent generation | `generate_intent` :63336 (:63388) | broker; same note |
| Harvest (live-agent branch) | `harvest.rs::run_harvest_agent` :789 (:810), `handle_harvest_command` :1058 (:1207) | broker: harvest root; `--yes-all` candidate consent is not launch authority |
| Reconstitution | `reconstitute.rs::run_headless` :323 (:343), `handle_reconstitute_command` :376 | broker: reconstitution root |
| Queue implementer (fresh/resumed) | `queue_cmd.rs::handle_queue_work` :10487 (calls :12175–:12319; program resolved :11276) | broker: per-spec run under a drain/wave delegation or its own root |
| Standalone reviewer | `run_standalone_reviewer` :12343 (calls :12399–:12527) | broker: reviewer phase attempt |
| Advisor watch | `advisor_watch.rs::run_advisor_watch` :121, `fork_and_run` :215 (:235), `cold_boot_and_run` :257 (:268) | broker: independent watch root (finite starts, authenticated away lease); revalidate before transcript copy, log creation and each spawn |

#### E. Orchestrator / controller and direct phase-driver sinks — target: protected controller only

| Route | Site | Target |
|---|---|---|
| Advisor session (cold/fork) | `lib.rs::RealPhaseDriver::spawn_advisor_session` :107583 (spawn :107698) | controller permit, not ambient child env |
| Resume implementer | `resume_implementer` :112417 (:112467) | same |
| CI-fix leg | `attempt_ci_fix` :112871 (spawn :112940) | same; bounded CI-fix → reviewer edge |
| tmux implementer hosting | `run_implementer` → `pane_host::host_implementer_in_tmux` (call :109791) | transport only, attaches to protected handles |
| Controller entries | `handle_auto_complete` :96858, `run_auto_complete` :97721, `handle_auto_complete_batch` :99046, `handle_auto_complete_single_branch` :99544, `handle_auto_complete_batches` :99719, `handle_auto_complete_next_n` :101348, resume/from-PR/pipeline | authorization precedes token minting, `DrainState::write`/`set_run`, `ensure_queued_for_implementer` and the `--with-plan` prelude; no self-minted authority |
| Phase child env | `orchestrator_phase_child_env` :108174 | diagnostic only; no environment-carried bearer token |
| Legacy corroboration | `orchestrator.rs::detect` :230 / `classify` :191 / `run_is_live` :213 | demoted to diagnostic classifier (ADR-61 decision 8) |

#### F. Agent-new and interactive session surfaces — target: broker for any full-access posture; typed native-posture launches otherwise

| Route | Site | Posture today | Target |
|---|---|---|---|
| `aida agent new` Claude | `lib.rs::agent_new_claude` :28632 (bypass default for `--bg` :28690) | config/flag-resolved | full-access → broker; native posture stays a typed, non-escalatable launch |
| `aida agent new` Codex / Antigravity | `agent_new_codex` :28750, `agent_new_antigravity` :28813 | explicit bypass-sandbox flags | same |
| Config-merged launches | `agent_new_with_config` :28875 (call :29097), `agent_new_bg_dispatch` :29124 (spawn :29307), `run_tracked_agent` :32148 (:32169), `tool_bypass_flags` :28063, `apply_agent_default_flags` :29488 | `~/.aida/agents.toml` + project config can inject bypass | config is input, never authority; resolved full-access → broker |
| Agent resume | `agent_resume_ended` :27745 → `run_tracked_agent` :27831 (resume argv :29463; `AIDA_OS_WRAP` can still apply) | vendor resume adapter, no bypass added | typed resume of an admitted attempt; os-wrap overrides refuse unless enrolled |
| `aida session` | `handle_session_command` :26631 (:26687 → `run_claude_session`), posture via `resolve_interactive_launch_mode`; session resume `session.rs::resume` :638 → `exec_claude_resume` :4173 | flag/config-resolved | full-access → broker; native posture typed |
| Session/queue/reviewer sinks | `session.rs::exec_claude` :932, `run_claude_session` :985, `spawn_claude_resume` :1269, `exec_codex_session` :1314, `spawn_reviewer_launch_plan` :1414 | optional bypass args from callers | same policy at every sink |
| Dead API | `spawn_claude_session` :1028, `spawn_codex_session` :1336 — **no callers at this HEAD** (rg-verified; `lib.rs` is outside codegraph) | n/a | remove or keep `cfg(test)`-only; they must not remain reachable unprotected API |
| Consent gate (new since anchor) | `bypass_confirm.rs` + `gate_agent_bypass` `lib.rs` :29581, called from `handle_session_command` :26672/:26725, `agent_new_with_config` :28972, `agent_new_bg_dispatch` :29214, `queue_cmd.rs::handle_queue_work` :10855 (interactive, non-orchestrated only — **orchestrated children skip it**) | IsTerminal-based prompt | UX improvement only. Under `01a0e0a3`/AC2 an IsTerminal prompt **grants nothing**; it must not be presented as admission, and the orchestrated skip illustrates why ambient orchestration state cannot be the gate |

#### G. Wrappers, schedulers, transports, shipped scripts

| Route | Site | Today | Target |
|---|---|---|---|
| Solo loop | `solo_cmd.rs::solo_cycle` :16, `run_solo_loop` :134 (launches intake --apply and burndown run) | inherits B/C | approved intake+wave bundle; a refusal stops the whole chain including later integration — no next-tick retry-around |
| Shift / night-shift wave | `shift.rs::build_wave_argv` :805, `wave_command` :841 (spawn :857), `spawn_wave` :1302/:1650, `launch_wave_isolated` :1433 | builds `queue work --batch … --auto-complete --no-human=both`, detached or systemd | wake hint only; wave members/budget frozen at approval; refusal propagates |
| Maintenance schedule | `maintenance_schedule.rs::run_aida_command` :2991 | re-invokes `aida` | wake hint only |
| Schedule driver / systemd units | `schedule_driver.rs::run_bounded` :234, `build_systemd_units` :416, `real_tick_invocation` :1333, `build_wave_unit_argv` :1417 | generates/user-systemd launches | timers are unauthenticated wake hints (ADR-61 decision 11); dedupe by protected job generation + slot + attempt |
| tmux pane host | `pane_host.rs::host_implementer_in_tmux` :154, `tmux_new_window_argv` :101 | transports a supplied command | transport attaches to protected handles; may not execute vendor commands on the host |
| Demo script | `scripts/aida-demo.sh` — bypass probe **removed** (BUG-1688, `dc3b03058e`); `demo_refuse_vendor_launch` :45 prints `AIDA_VENDOR_LAUNCH_REFUSED` | refusal | keep refusing; pinned by `tests/test_bug_1688_shipped_launch_refusals.sh` |
| Ablation scripts | `scripts/ablations/gate-vs-rule{,-i2,-i3,-i4}.sh` exit 78 unconditionally (:37/:73/:93/:106); dead bypass argv remains below (:255/:350/:404/:527) | refusal | keep refusing; same pinning test |

#### Indirect self-reexec routes (posture decided downstream in D/E)

`drain_cmd.rs::handle_drain_start` :501 (spawn :578) and `handle_shelved_resume` :472;
`autoprogress.rs::run_aida` :125 (invoked :73 with `--auto-complete --no-human=both
--force-claim`); `lib.rs::run_zen_drive` :102944; `run_plan_prelude` :97685/:97697;
`plan_cmd.rs::plan_fan_out` :848 (`--plan-only`); `run_do_drive` :102256 Drive/Drain
modes (:102363). Each spawns `aida queue work …` and inherits that route's admission.
Wrapper routing must preserve refusal and cannot manufacture authority.

#### Non-bypass launch routes (inventoried for completeness; not full-access today)

| Route | Site | Posture |
|---|---|---|
| AI review probe | `lib.rs::ask_ai_review_once` :91378 (spawn :91414), argv `compete.rs::ask_ai_argv` :94 | read-only hard-coded (Claude `--permission-mode plan`, Codex `--sandbox read-only`); **Agy/other vendors fall through to `agents.toml` `args_template` :114 — config-decided, must be typed or refuse** |
| Clarify | `questions_clarify` :8993 (spawn :9073) | interactive, native permissions |
| Guided drive | `run_do_drive` guided mode :102413 via `session::guided_session_launch` | interactive, native permissions |
| Reviewer launch | `handle_review_spec` → `review_spec_launch_reviewer` :91942 (`spawn_reviewer_launch_plan`, `permission_mode=None`) | native permissions |
| Core AI client | `aida-core/src/ai/client.rs::send_cli_request` :245 (spawn :247) | `claude --print`, no permission flag |
| TUI | `aida-tui/src/dispatch.rs::plan` :56 / `run_child` :90 (resume, allow-listed launch/shell); `app.rs::spawn_tab` :1150 / `queue_work_argv` :1358 (PTY-hosts `aida queue work`) | native / inherits D |

These confer no bypass today, but they are official AIDA-managed launches: under
P-enrolled-host they enter protected execution as typed native-posture operations or
refuse. None may be left as an escalation path (a config edit flipping them to bypass
must hit admission, not succeed silently).

#### Audited non-launch surfaces (must stay non-launch, with tests)

Version/MCP probes (`claude_agents.rs` :56, `doctor_cmd.rs` :1144, `init_cmd.rs`,
`implementer_preflight.rs` :428, `session.rs::preflight_vendor_binary` :1728); posture
renderers and resolvers (`config_cmd.rs::display_tool_bypass_flags` :1653 /
`display_tool_headless_contained_flags` :1676, `resolved_agent_permission_summary`
:32110, `resolve_interactive_launch_mode_for_root` :30516,
`resolve_queue_work_permission_mode` :113196); launch hints
(`queue_cmd.rs::deferred_interactive_launch_hint` :10413); template comments
(`aida-core/templates/agents.toml`). No launch hits in `.claude/`,
`aida-core/templates/hooks`, `aida-server`, plugins, helper, ci, or docker trees.

### 1.2 Shipped launch surfaces outside the Rust CLI

| Route | Site | Today | Disposition |
|---|---|---|---|
| **Benchmark harness — NEWLY FOUND, not in the A–G trail** | `bench/agent-surface/run_bench.py::build_claude_argv` :80–88 and `grade` :290–303 | `subprocess.run` of `claude -p … --dangerously-skip-permissions`, hard-coded, for both bench arms and the LLM judge (predates the sketch anchor; SPIKE-73/TASK-1130; BUG-1688 did not touch it) | Official shipped route; **may not be reclassified as unmanaged**. Same class as the BUG-1688 scripts: no legitimate unattended caller → candidate for an immediate additive refusal spec (drafted as BUG-1784, below), separate from the boundary build |
| MCP server | repo `.mcp.json` → `aida mcp-serve`; Antigravity MCP registration markers in `~/.aida/antigravity-mcp-aida.{offered,registered}` | MCP verbs route into the same lib handlers | any MCP verb reaching a launch handler inherits that route's disposition; the MCP surface must be enumerated verb-by-verb at implementation and is listed in the fixture matrix |
| Claude Code skills / slash commands | `.claude/skills`, `.claude/commands` (generated from `aida-core/templates/`) | instruct an agent to run `aida …` | thin clients by construction; prompts and hooks are not enforcement (M-series); they inherit CLI admission |

### 1.3 This host's live configured routes (observed 2026-10-02)

| # | Route | Observed fact | Cutover disposition |
|---|---|---|---|
| H1 | Login-shell binary resolution | `~/.bashrc` line 177 runs `aida dev activate`; PATH begins `/home/joe/ai/aida/target/release` → live `aida` is **agent-built** `target/release` (0.15.0, sha `d5b1957658+dirty`, built 2026-10-02) | ADR-61 §2: an agent-built binary is never trusted deployment code. Post-cutover the installed broker client is administrator-owned; dev-activate becomes a dev-loop concern (feasibility row F-1) |
| H2 | Stale installed clients | `~/.local/bin/aida` (0.15.0 `0bd634c`, built 2026-09-11) and `~/.cargo/bin/aida` (0.15.0 `0bd634c5+dirty`, 2026-09-13) | legacy clients: may propose if protocol-supported, can never launch; both are **negative-test fixtures** for the stale-client matrix row, then removed at cutover |
| H3 | Shell wrapper | `~/.aida/shell-init.sh` sourced by `~/.bashrc` (the `aida()` auto-eval wrapper) | wrapped route; thin client, no authority |
| H4 | systemd user timer (this repo) | `aida-tick-2b20e8a3.timer` every 10 min → `aida schedule tick` in `/home/joe/ai/aida`, absolute path to `target/release`, `AIDA_SCHEDULE_INVOKER=systemd`, **`KillMode=process`** (launched waves outlive the tick) | timer = wake hint. `KillMode=process` means cutover must sweep for surviving wave processes, not just stop the unit |
| H5 | cron ticks | two crontab lines, every 15 min: `aida schedule tick` in `/home/joe/ai/aida-relay` and `/home/joe/ai/aida-proxy`, `AIDA_SCHEDULE_INVOKER=cron`, absolute `target/release` path | wake hints; replaced by broker-client ticks or disabled at cutover |
| H6 | Schedule jobs that launch agents | main repo: `night-shift` (`aida shift tick`, every 10 min) plus advisor/product seat-route jobs; relay and proxy: `product-wave-relaunch` (every 5 min, conditional on free drain lock), `mailbox-triage`, `shelf-triage` | these are the unattended launch consumers that stop at cutover (ADR-61 gate 4 names this consequence); each becomes an independent finite scoped grant |
| H7 | codegraph timer | `codegraph-sync-aida.timer` (15 min) | non-launch (index sync); out of scope |
| H8 | Global agent config | `~/.aida/agents.toml`: `bypass = false`, `contained = true`; Antigravity `default_flags = ["--dangerously-skip-permissions"]`; Codex `--sandbox workspace-write --ask-for-approval never`; four `.bak`/`.old` copies beside it | config is input, never authority. **Exposure fact E-1 below: contained Codex can write this file** |
| H9 | Codex sandbox config | `~/.codex/config.toml`: `workspace-write`, `network_access = true`, `writable_roots = ["~/.cargo", "~/.rustup", "~/.aida"]` | **`~/.aida` writable from the "contained" posture** — the machine-global launch-posture knob (H8), the schedule-tick log, and shell-init live there. Recorded as exposure E-1; post-boundary the host registry is protected service state precisely so this class of write is inert |
| H10 | Project universe | 120+ directories with `.aida` under `~/ai`: primary repos (aida, quizdom, market-watcher, port_manager, mind, yt, paradox, aida-chat, aida-hub, aida-monitor, aida-proxy, aida-relay, aida-tutor, aida-walk, …), sibling worktrees (`aida-task*`, `aida-bug*`, `wt-*`, `*-epic-*`), pool clones (`aida-pool-*`), review clones (`aida-review-*`), demo copies (`aida-demo-2026…`), mirrors/clones (`aida-gitlab-{mirror,clone,test}`), a literal duplicate (`aida_dup`), toy/bare projects (`foo`, `gld`, `dummy*`, …) | enrollment classes in §2.1; unknown/unenrolled → refusal, not exemption |
| H11 | Current managed workers | `aida ps`: 15 running sessions (1 live harness subagent fan-out, 12 dormant stalled leases incl. this session, 0 live full-access drains at observation time), 10 orphaned stale/flag-only entries, 4 stale hidden | migration plan §3; a quiet window like this one is the cheap time to cut over |
| H12 | Local model proxy | `aida-proxy` reverse proxy on loopback :9090 (ports registry) | credential-free/custom route class (§2.5): enrolled typed adapter or refuse |
| H13 | Unmanaged vendor clients on this host | Claude desktop app running (`app-com.anthropic.Claude-*.scope`); interactive `claude`/`codex` in terminals; playwright MCP configured in `~/ai/dsvm`, `~/ai/wordgame` | **outside the promise** (`01a0e103`), stated here so the exclusion is bounded and honest — and never used to relabel a surviving official route (§2.6) |
| H14 | Host virtualization facts | `/dev/kvm` present (root:kvm + ACL), vmx on all 6 cores, 70 GiB RAM, kernel 7.0.0-30-generic | SPIKE-91 backend qualification inputs; no claim this host qualifies |

---

## 2. Identity, enrollment, and route-class treatment

### 2.1 Per-repo identity and enrollment generations

Canonical repo identity is **protected registry state** (broker-held), never derived
from path, directory name, `.git` indirection, git remote, or a hash the client reports
(ADR-61 §2). Proposed identity classes for the H10 universe:

1. **Enrolled primary** — one canonical checkout per project Joe enrolls (the concrete
   per-repo mapping is a NOT-YET-GRANTED approval; §5). Identity = registry record
   {repo ID, enrollment generation, canonical root, approved forge identity + allowed
   refs, import/export roots}.
2. **Sibling worktree of an enrolled primary** (`aida-task1550`, `wt-*`,
   `aida-worktrees/*`, epic worktrees) — imports into the primary's identity **only
   through validated snapshot manifests**; never a second identity, never implicit.
3. **Ephemeral pool clone** (`aida-pool-*`) — as (2), but created/destroyed by the
   (future) protected controller; a pool clone that exists outside a live grant is
   just an unknown directory.
4. **Review clone** (`aida-review-*`) — read/review only; launch proposals from it
   refuse (its work product returns via forge, not local launch).
5. **Copy/mirror/duplicate** (`aida-demo-2026*`, `aida-gitlab-{mirror,clone,test}`,
   `aida_dup`, copied `.aida-store`) — **refuse**. Same path shape, same spec numbers,
   cloned remote or copied store cannot transfer authority (ADR-61 §3).
6. **Unknown / unenrolled / bare `.aida`** (`foo`, `gld`, `dummy*`, abandoned demo
   dirs) — refuse; absence of enrollment is refusal, not exemption.

**Enrollment generation** increments on every re-enrollment, registry repair, key or
policy rotation for that repo. Every grant, artifact and adapter call binds
{repo ID, generation, base artifact digest, spec/plan content digests, operation/phase}.
A stale generation refuses even with an otherwise-valid-looking grant handle. New
repositories inherit nothing — no watch, wave or schedule consent (ADR-61 decision 5).

### 2.2 Cross-repo grant and observation isolation

Coverage is not a grant. Repo-A authority conveys nothing in repo B: no cross-repo
parent grant in v1, no implicit sharing of artifacts, transcripts, model context or
credentials, and — per **M2** — no cross-repo **observation**: requests for another
repo's transcripts, prompts, logs, artifacts, guest consoles or interactive input
refuse. Public proposal/status IPC exposes only deliberately public, non-sensitive
metadata; opaque IDs unlock nothing. The host-wide scheduler (H4–H6) may display many
jobs, but each job holds independent authority and counters. Repo-A-requests-repo-B is
a required fixture row (§4.2 rows X4–X6).

### 2.3 No legacy-client fallback

The stale H2 binaries, any future stale build, and any copied/modified client are
**proposal-capable at most**: a supported protocol schema may propose/status/attach;
an unsupported schema refuses before preparation. No legacy client ever launches
locally as a fallback, and a denial never degrades into "run it the old way". The
broker trusts no client version string or executable path (ADR-61 §2); support is a
protocol property, not a binary identity claim. Protected binary identity/admission
(BUG-1679, specified inside SPIKE-91) governs which coordinating build a *launch plan*
may name; it is necessary but is not itself a grant.

### 2.4 Missing / unsupported project refusal

A proposal from a directory with no registry identity (classes 5–6, or any path the
registry cannot bind) gets a typed nonzero refusal naming enrollment as the remedy,
with no launch-associated mutation. Unsupported platform, vendor, adapter, schema or
posture refuses identically (ADR-61 §4). Read-only surfaces (previews, argv renderers,
`schedule due`/`status`, diagnostics) stay useful everywhere, including unenrolled
directories — they confer no authority.

### 2.5 Credential-free, custom and local-model routes (M1)

Credential revocation cannot prove these routes blocked; their **execution behaviour**
must be recorded and tested (M1):

* **Local proxy / local models** — `aida-proxy` :9090 (H12) and any ollama-class
  endpoint: no upstream credential to revoke, so admission must gate the launch
  itself; allowed only as an enrolled typed model adapter with pinned endpoint,
  allowed models and quotas, else refuse.
* **Custom executables / overrides** — `AIDA_AGENT_CMD`, `AIDA_COMPETE_JUDGE`,
  `AIDA_OS_WRAP`, `agents.toml` `args_template` vendors (the `ask_ai_argv` fall-through
  in §1.1), arbitrary extra flags: refuse unless converted to an enrolled, reviewed
  typed adapter configuration.
* **Credential-free vendor modes** — a vendor CLI that runs without any AIDA-managed
  credential (subscription login in host HOME, device-auth cache): the managed launch
  principal must simply lack those credentials and host execution bridges; negative
  tests run **from the untrusted host principal against stale, copied and replaced
  clients** (M1, §4.2 rows N1–N3).

### 2.6 Unmanaged-client exclusion — preserved, not stretched

Out of the promise (`01a0e103`, ADR-61 §5): independent vendor clients launched outside
AIDA by an unrestricted host login using their own credentials — on this host today,
H13. P-enrolled-host prevents such clients obtaining protected AIDA execution
resources; it does not police arbitrary programs on an unconfined login.

**No official AIDA route may be reclassified as unmanaged because it evades the gate.**
An official route still launching with host credentials after cutover is a **FAILED
cutover case**. `bench/agent-surface/run_bench.py` (§1.2) is the live example of the
temptation: it is a shipped, repo-owned surface and stays in the official inventory with
an explicit disposition. If the real host turns out to require an official route to
retain independent host execution, that concrete incompatibility **returns to Joe as a
deployment blocker** (feasibility rows, §5.2), never becomes an inferred exception.

---

## 3. Current-run interruption and migration plan

Existing runs are **UNPROTECTED and are never adopted** as authorized by PID, UUID,
role or state (ADR-61 NOT-YET-GRANTED row 5). Plan for the gate-4 window, prepared now
so the approval request can be concrete:

1. **Freeze intake of new unattended work.** Disable, in order: `night-shift`
   (`aida shift tick`) in this repo; `product-wave-relaunch` in relay and proxy;
   remaining seat-route schedule jobs. These are exactly the H6 consumers; the
   pre-published "what stops" list ADR-61 requires is H4+H5+H6.
2. **Stop wake sources:** `systemctl --user disable --now aida-tick-2b20e8a3.timer`;
   remove the two crontab `aida-schedule-tick` lines. Because of `KillMode=process`
   (H4), then **sweep for surviving launched processes** (`aida ps` + process table)
   rather than trusting unit state.
3. **Drain or stop live workers.** At observation time the live set was one harness
   subagent fan-out and zero full-access drains (H11) — the realistic window is short.
   Policy: let a running wave finish only inside the separately accepted migration
   window with **no protection claim**; otherwise checkpoint (transcript → reviewed
   untrusted artifact) and terminate. Dormant leases are ended (`aida session end`) or
   reaped; orphaned entries garbage-collected.
4. **Cut client routes over** per §4.1 stages, per repo, in enrollment order.
5. **Revoke/replace only AIDA-managed execution credentials** (which identities move
   is a NOT-YET-GRANTED decision, §5.1). Unrelated vendor credentials (H13) are not
   touched.
6. **Checkpoint artifacts are imports, never authority.** A resumed spec re-enters as
   a fresh proposal over validated artifacts.

Rollback for the whole window: §4.3.

---

## 4. Cutover manifest and fixture/test matrix

### 4.1 Cutover manifest (per-stage, per-route; executes only after gates 2–4 close)

| Stage | Action | Routes covered | Verification |
|---|---|---|---|
| 0 | Capture manifest snapshot (this document §1.3 refreshed on the day), publish the "what stops" list, record rollback state (unit files, crontab lines, PATH setup, binary hashes) | all | manifest diff vs live host is empty |
| 1 | Install broker/controller/supervisor/adapters from the independently verified immutable package (SPIKE-91's deliverable); enroll repos per approved mapping | — | installed-ACL/admin-boundary inspection; no agent-writable path in the trust root |
| 2 | Freeze + stop unattended work (§3 steps 1–3) | H4, H5, H6 | zero surviving launch processes; schedule jobs report disabled |
| 3 | Replace scheduler executions with broker-client ticks (wake hints) | H4, H5 | tick fires → proposal only; no local spawn possible (binary lacks it / refuses) |
| 4 | Cut CLI launch verbs to thin clients: A, B, D, E, F routes propose/attach; C (burndown native fan-out) **refuses** until the mediated wave controller ships | §1.1 | per-route fixture rows pass against the real boundary (gate 3 evidence) |
| 5 | Cut secondary surfaces: MCP verb enumeration, TUI, skills, `run_bench.py` disposition, indirect self-reexec routes | §1.2 + indirect | same |
| 6 | Remove stale clients (H2) after their negative-fixture use; dev-activate dev-loop per the F-1 resolution | H1, H2, H3 | `which -a aida` shows only administered client(s) |
| 7 | Revoke/replace AIDA-managed execution credentials; enable schedules deliberately as new finite grants | H6, credentials | old credential use fails; each schedule shows its own grant/counters |

Order rationale: wake sources before clients (a tick that fires mid-cutover must find a
refusing or proposing client, never a half-converted launcher), credentials last (they
are the no-return step and the biggest rollback cost).

### 4.2 Fixture/test matrix (acceptance-required rows)

Hermetic rows run in fake-HOME/temp-registry harnesses; **B**-marked rows additionally
require the disposable real-boundary environment (gate 3; a skipped privileged suite is
a block, not a pass). Every row asserts: typed nonzero refusal (or scoped success),
zero vendor launches on refusal, and no launch-associated mutation before a valid
preparation reservation.

| Row | Fixture | Expected |
|---|---|---|
| U1 | Proposal from unknown repo (`foo`-like temp dir, bare `.aida`) | refuse: not enrolled |
| U2 | Proposal from deleted/renamed enrolled root (registry binds, path mismatch) | refuse; no path-based rebind |
| C1 | Stale client (H2 binary verbatim) proposes with supported schema | proposal accepted; it can never launch locally |
| C2 | Stale client, unsupported schema | refuse before preparation |
| C3 | Copied client binary + copied `.aida-store` (demo-dir shape, H10 class 5) | refuse; copied store transfers nothing |
| C4 | Replaced official wrapper (modified `shell-init.sh` / fake `aida` earlier in PATH) run from untrusted principal **B** | obtains nothing; M1 negative from the untrusted host principal |
| X4 | Repo-A grant used for repo-B launch (valid handle, wrong repo binding) | refuse |
| X5 | Repo-A requests repo-B transcript/log/artifact/console (M2 observation row) | refuse |
| X6 | Sibling-worktree import without validated snapshot manifest; copied store import | refuse |
| N1 | Untrusted host principal replays captured grant/worker IDs over public IPC **B** | nothing; IDs are handles, not authority |
| N2 | Credential-free route (local proxy H12 shape) proposed as unenrolled adapter | refuse; enrolled-typed-adapter variant succeeds scoped **B** |
| N3 | `AIDA_AGENT_CMD`/`AIDA_COMPETE_JUDGE`/`AIDA_OS_WRAP`/`args_template` override | refuse unless enrolled typed adapter |
| S1 | Scheduled grant expiry: tick fires after deadline/start-quota exhausted | refuse; no new root minted by the timer |
| S2 | Duplicate tick (same slot, re-fired unit/cron) | deduplicated by job generation + slot + attempt |
| S3 | Watch expiry/home-transition between wake and spawn | revalidation refuses before transcript copy/log/spawn |
| W1 | `KillMode=process` survivor: wave process outlives disabled unit at cutover | sweep detects it; it is interrupted/checkpointed, never adopted |
| F1 | Forgery negatives at every §1.1 branch (hand-written DrainState + matching UUID + live PID; fabricated PTY answering yes; both IsTerminal forced true; `gate_agent_bypass` consent answered yes by a PTY) | all FAIL to launch (BUG-1669 AC4 reversal) |
| F2 | Config escalation: `agents.toml` flipped to `bypass = true` by a contained agent (E-1 shape) | resolved full-access posture still refuses without admission; config is input, not authority |
| R1 | Rollback drill: restore recorded units/cron/clients from stage-0 snapshot | pre-cutover launch behaviour restored; **no grant, approval or budget restored** |
| G1 | Shipped-script refusals stay pinned (BUG-1688 test) + `run_bench.py` disposition test | refusal/typed behaviour; no silent revival |
| M1r | MCP verb sweep: every `mcp-serve` verb classified launch-reaching vs not; launch-reaching verbs hit the same admission | no MCP-only bypass of CLI admission |

### 4.3 Rollback

Stage-0 snapshot records: both crontab lines verbatim, `aida-tick-2b20e8a3.{service,timer}`
unit files, `~/.bashrc` aida block + `shell-init.sh`, binary hashes of H1/H2 clients,
schedule-job definitions (H6), and the enabled/disabled state of each. Rollback = stop
admission, revoke all live grants, kill workers, restore those artifacts, re-enable
units/cron — restoring **launch behaviour only**: no approval, grant, budget or epoch
is ever restored (ADR-61 recovery rules), and the registry keeps its advanced epoch so
a later re-cutover cannot replay old authority. This is the "rollback that matters on
the night of a cutover" ADR-61 requires gate 4 to arrive with.

---

## 5. What returns to Joe, and unresolved feasibility mismatches

### 5.1 Host facts / permissions needing Joe's separate approval (TASK-1550-owned rows)

Nothing here is granted by this document (ADR-61 NOT-YET-GRANTED table governs):

1. **Repository enrollment mapping** — which of the H10 primaries enroll, each repo's
   canonical root, forge identity and allowed refs, import/export roots.
2. **Current-run migration window** — the named stop/checkpoint window for H4–H6 and
   any live workers; the pre-published "what stops" list (H4+H5+H6 today).
3. **Credential scope** — which AIDA-managed identities move behind adapters
   (shared with SPIKE-91).
4. **Rollback policy** — acceptance of §4.3 semantics (behaviour restored, authority
   never).
5. **Dated risk acceptance** — explicit acceptance that the full BUG-1669 exposure
   persists for the duration of SPIKE-91 + implementation + gate-3 proof + cutover
   (ADR-61 row 9; still outstanding).
6. **`run_bench.py` disposition** — approve the additive refusal spec drafted from
   §1.2 (BUG-1784, filed as draft) or direct otherwise.

### 5.2 Unresolved feasibility mismatches (reported, not resolved here)

* **F-1: The dev loop runs agent-built binaries by design.** `aida dev activate` (H1)
  exists so this repo's checkout *is* the live `aida`, and today's schedulers execute
  that same `target/release` path by absolute reference (H4, H5). Under the boundary an
  agent-built binary can propose but never launch — so AIDA-on-AIDA development needs
  an explicit story (e.g. dev proposals run through the installed broker like any
  client, with the dev binary never in a trusted role). If dogfooding is found to
  require local trusted execution of fresh builds, that is a **deployment blocker to
  put to Joe**, not an exception to infer.
* **F-2: Single-login workstation.** Broker, workers, approval flow and the agents all
  share one physical host whose only interactive login (`joe`) is also the principal
  agents run as. ADR-61 requires the approval endpoint to be a separately
  authenticated surface under a distinct principal; SPIKE-91 owns the mechanism, but
  the *host fact* is that no second principal or device exists today. Concrete
  admin/approval principal custody returns to Joe with SPIKE-91's package.
* **E-1 (exposure, current behaviour):** the "contained" Codex posture can write
  `~/.aida` (H9) — including `agents.toml`, the machine-global knob that injects
  bypass into supervised launches (H8). Combined with `gate_agent_bypass`'s
  IsTerminal-based consent, a contained agent today can both *set* the bypass default
  and *answer* the consent prompt via a PTY. This strengthens, and changes nothing
  about, the already-settled requirement: it is additional evidence for fixture rows
  F1/F2 and for why the registry must live outside agent-writable paths. (No live
  config was changed by this task.)
* **F-3: `KillMode=process` orphans.** Unit state is not process state on this host
  (H4); every stop/rollback step must sweep processes (fixture W1).

---

## 6. Explicit non-claims

* **No protected boundary is deployed.** Every full-access route in §1 launches today
  behind same-user-forgeable corroboration; this document deploys nothing.
* This inventory **changes no live launcher, credential, scheduler, service or
  config** — it is the read-only manifest TASK-1550's acceptance asks for.
* BUG-1669 remains blocked and **implementation-held even after this task completes**
  (`01a0e125`); BUG-1679/PR #2226 remain operator-held; gates 2–5 remain OPEN.
* Line anchors here are HEAD-`ad7ecdb79b` anchors; symbols are authoritative and
  anchors must be refreshed again at implementation time (ADR-61 stale-anchor rule).

<!-- trace:TASK-1550 | ai:claude -->
