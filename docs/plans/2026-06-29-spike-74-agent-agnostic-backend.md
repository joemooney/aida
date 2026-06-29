# Plan: SPIKE-74 — Agent-agnostic drain backend (claude/codex behind a trait)

Date: 2026-06-29
Specs: SPIKE-74 (parent EPIC-56; mirrors EPIC-35 Forge)
Status: Draft (investigation spike — read-only, no implementation)
Complexity: scoping only. Build estimate below.

## Approach

The headline finding rewrites the premise. **The drain is no longer "hardcoded to `claude -p`."** A prior slice — **STORY-683 + TASK-894 + TASK-895** — already introduced a `HeadlessVendor {Claude, Codex}` enum and routed the orchestrator's headless spawn, advisor tier, and interactive launcher through it. The codex adapter exists and is unit-tested. So SPIKE-74's question is not "how hard to make it vendor-agnostic from zero" but **"how hard to (a) consolidate the existing scattered vendor-switch into a single `AgentBackend` trait mirroring Forge, and (b) close the 3-4 real gaps that make the Codex path a stub rather than a working keystone loop."**

The mechanical call-site migration is cheap and largely done. The cost is concentrated in **one hard piece: skill/slash-command resolution** (`/aida-pickup`, `/aida-advise`, `/aida-review` are Claude-Code-native slash commands with no Codex/Gemini equivalent), and secondarily the **stream-json liveness/tee** and the **resume/session model**. The good news that lowers risk: result-parsing is almost entirely **filesystem + git/forge handshakes**, not stdout parsing, so it is already vendor-neutral.

```
 orchestrator (RealPhaseDriver, main.rs)
   ├─ run_implementer ──► spawn `aida queue work <spec> --auto-complete` (recursive)
   │                         └─► exec_claude_headless / spawn_claude_headless ──► resolve_headless_vendor ──► [Claude | Codex]
   ├─ run_reviewer ─────► (same recursive aida queue work PR-N path)
   ├─ advisor tier ─────► advisor_tier_program_and_args(vendor, …)   [VENDOR-NEUTRAL, TASK-894]
   └─ result read ──────► PR via forge::Forge + verdict file + punt/hold signal files   [FILESYSTEM, vendor-neutral]
```

## Decisions (findings)

- **Decision: the abstraction already exists in primitive form** — `HeadlessVendor` (`session.rs:HeadlessVendor`) is the de-facto `AgentBackend`, but expressed as an enum + a scatter of free functions rather than a trait. **Rationale:** STORY-683 chose the minimal enum to preserve byte-identical Claude behavior; SPIKE-74 is the "promote it to a trait like Forge" follow-on.
- **Decision: result-parsing is NOT a vendor gap** — the orchestrator reads outcomes from `forge::Forge` (PR detection), `.aida/review-verdicts/PR-N.json`, and `punt`/`hold` signal files, not from the agent's stdout. **Rationale:** confirmed in `RealPhaseDriver::run_implementer` (signal-file handshakes) and `run_reviewer` (verdict file). Only the **liveness tee/watchdog** (`headless_tee`, `PhaseWatchdog`) consumes `claude -p --output-format stream-json`.
- **Decision: the hardest piece is skill resolution** — see Risks. The seeded-prompt path still embeds the literal token `/aida-advise` / `/aida-assess` (`intake.rs:seed_skill_prompt`), which Claude Code resolves to skill markdown but Codex/Gemini treat as plain text.

## Claude-specific call-site inventory (file:symbol)

Counting **production launch/parse sites** (excluding tests/comments). Grep basis: `Command::new("claude")`, the `claude_*`/`spawn_claude_*`/`exec_claude_*` functions, and the `-p`/`--permission-mode`/`stream-json` argv builders.

**A. Headless drain spawn path (`session.rs`) — mostly ALREADY vendor-neutral:**
1. `session.rs:claude_headless_args` / `claude_headless_args_with_posture` — builds `claude -p --permission-mode bypassPermissions --output-format stream-json --verbose --disallowed-tools AskUserQuestion --session-id`. Codex sibling exists: `codex_headless_args`. Dispatched by `headless_vendor_args`. **Status: DONE.**
2. `session.rs:spawn_claude_headless` → delegates to `spawn_vendor_headless`. **Status: DONE (vendor-neutral).**
3. `session.rs:spawn_vendor_headless` — the generalized spawn (`vendor.program()`, bwrap wrap, `AIDA_HEADLESS=1`). **Status: DONE.**
4. `session.rs:advisor_tier_program_and_args` (TASK-894) — vendor-neutral advisor `/aida-advise` launch. **Status: DONE (Claude resume vs Codex fresh-spawn).**
5. `session.rs:spawn_claude_headless_resume` + `claude_headless_resume_args` — **hardcoded `claude --resume`; no Codex path** (Codex has no session/resume model). **Status: CLAUDE-ONLY gap.**
6. `main.rs:exec_claude_headless` (~line 126971) — still `Command::new("claude")` exec for the in-process headless leg. **Status: CLAUDE-ONLY, not yet routed through `headless_vendor_args`.**

**B. Interactive launcher (`session.rs` + `main.rs`) — partially generalized:**
7. `session.rs:exec_claude` / `claude_session_args` / `exec_claude_with_session` / `spawn_claude_session` — `Command::new("claude")`, passes `/aida-pickup` as positional prompt. **Status: CLAUDE-specific.**
8. `session.rs:exec_codex_session` / `codex_session_args` (TASK-895) — Codex interactive sibling. **Status: DONE.**
9. `session.rs:spawn_claude_resume` (`session.rs:863`), `exec_claude_resume` (`session.rs:2518`) — `claude --resume`. **Status: CLAUDE-ONLY.**
10. `main.rs:agent_new_claude` / `agent_new_codex` / `agent_new_antigravity` — per-vendor dispatch already factored through `AgentLaunchConfig { agent_type, binary, default_args, prompt_style }` + `agent_new_with_config`. **Status: config-struct abstraction already in place (3 vendors live).**

**C. Direct `Command::new("claude")` headless spawns in `main.rs`:** lines `6011`, `91542`, `92095`, `116419` (all call `spawn_claude_headless` → already vendor-neutral); `126909`/`127060` (`spawn_claude_headless_resume` — gap #5); `127114` (`spawn_claude_headless`).

**Raw count:** ~**10 launch functions** carry a `claude`/`codex` identity in `session.rs`; ~**8 production spawn call sites** in `main.rs`. Of these, the **headless drain spawn + advisor tier + interactive codex are already done**; the **remaining claude-only sites are ~4-5**: `exec_claude_headless`, `spawn_claude_headless_resume`/`exec_claude_resume`, `exec_claude`/`spawn_claude_session` (interactive pickup), and the resume-argv builders.

## Generic-vs-vendor split per site

| Concern | Generic (already neutral) | Vendor-specific | Codex equiv? | Gemini equiv? |
|---|---|---|---|---|
| Headless single-shot flag | `-p` (claude) | `exec` (codex) | Yes (`codex exec`) | Likely (`gemini -p`/non-interactive) — unverified |
| Permission/sandbox bypass | concept | `--permission-mode bypassPermissions` vs `--dangerously-bypass-approvals-and-sandbox` | Yes | Unknown |
| Skill / slash command | the *intent* ("run pickup") | **`/aida-pickup` literal token** resolved only by Claude Code | **NO** — must materialize skill body into prompt | **NO** |
| Prompt format | positional trailing prompt | both use positional | Yes | Likely |
| Result parsing | **PR (forge), verdict file, punt/hold signal files** — all filesystem/git | — | N/A (neutral) | N/A |
| Liveness/tee | the watchdog *concept* | `--output-format stream-json --verbose` JSONL schema (`headless_tee`, `auto_complete.rs:2842`) | partial (codex has no matching stream schema) | NO |
| Resume/session | minted UUID concept | `--session-id` / `--resume` | **NO** (codex fresh-spawn only) | NO |
| MCP wiring | `.mcp.json` is read by each CLI from cwd | Claude Code auto-loads `.mcp.json`; codex/gemini have their own MCP config files | partial | partial |

## Proposed `AgentBackend` trait sketch (mirroring `forge::Forge`)

Mirrors `ForgeKind {GitHub,GitLab,None}` + `Forge` trait + `forge_for()` factory + `[forge] provider` config. Direct analogue: `AgentKind {Claude,Codex,Gemini}` + `AgentBackend` trait + `agent_backend_for()` + existing `[orchestrator] headless_vendor` config and `AIDA_HEADLESS_VENDOR` env (already implemented in `resolve_headless_vendor`).

```rust
pub enum AgentKind { Claude, Codex, Gemini }   // mirrors ForgeKind

pub trait AgentBackend {
    fn kind(&self) -> AgentKind;

    // 1. spawn one headless single-shot run; returns exit status. Wraps the
    //    program/argv decision currently in headless_vendor_args + spawn_vendor_headless.
    fn spawn_headless(&self, req: HeadlessRun) -> Result<ExitStatus>;

    // 2. render a skill invocation into a vendor-correct prompt. Claude → bare
    //    "/aida-pickup"; Codex/Gemini → materialized skill markdown body + args.
    //    THIS is the load-bearing method (see Risks).
    fn skill_prompt(&self, skill: SkillRef, args: &str, project_root: &Path) -> String;

    // 3. resume/fork support — Claude returns Some(argv); Codex/Gemini return None
    //    (caller forced to cold-boot, as advisor_tier_program_and_args already does).
    fn resume(&self, session_id: &str, skill: SkillRef) -> Option<Vec<String>>;

    // 4. liveness: parse the vendor's stream into the watchdog/tee event the
    //    drain consumes. Claude → stream-json JSONL; others → coarse (exit + git poll).
    fn liveness_source(&self) -> LivenessKind;   // StreamJson | ExitAndGitPoll

    // 5. (thin) interactive launch — the agent_new_* path; already factored as AgentLaunchConfig.
    fn interactive_launch(&self, cfg: &AgentLaunchConfig) -> Result<()>;
}

pub fn agent_backend_for(project_root: &Path) -> Box<dyn AgentBackend>; // mirrors forge_for
```

Note: methods 1, 3, 5 are **already implemented** as free functions (`spawn_vendor_headless`, `advisor_tier_program_and_args`, `agent_new_with_config`) — promoting them to trait methods is mechanical. Methods **2 and 4 are the new work.**

## Cost estimate

- **Sites to touch:** ~4-5 remaining claude-only launch functions + ~8 spawn call sites, most via a 1-line redirect through the trait. Bulk (headless drain, advisor tier, interactive codex) is done.
- **Trait methods:** 5 (3 are wrappers over existing code; 2 are net-new).
- **Hardest piece (≈70% of the cost): skill-invocation resolution.** `/aida-pickup`, `/aida-advise`, `/aida-review`, `/aida-assess` are Claude-Code slash commands. `intake.rs:seed_skill_prompt` currently embeds the literal `/aida-advise` token even in the "seeded" cold-boot prompt — Codex receives that token as **plain text and does not load the skill**, so today's Codex advisor/implementer arms are effectively **stubs that compile and spawn but won't reliably produce a PR + verdict file.** Closing this means materializing each skill's `.claude/skills/*.md` body into the prompt for non-Claude vendors (the `seed_skill_prompt` hook is the right insertion point) and re-validating that the embedded skill still drives the file-handshake correctly.
- **Second-hardest: liveness/tee + watchdog** depend on `--output-format stream-json`. Codex/Gemini have no equivalent schema, so the headless watchdog (`PhaseWatchdog`, `BUG-420`) degrades to exit-code + git-poll only — acceptable but must be wired and tested.
- **Rough build size:** ~250-400 prod LOC (mostly the trait + skill-materialization), ~200 test LOC, 3-4 commits, **risk: medium** — concentrated entirely in the skill-resolution correctness, not the plumbing.

## Known gaps (what Codex/Gemini can't do that `claude -p` can)

1. **Skill / slash-command resolution** — no `/aida-*` resolver. Must inline skill markdown. (Hardest.)
2. **Subagent fan-out + skill invocation under headless** — per the SPIKE-51 finding ("reuse the proven skill fan-out"), headless `claude -p` can both invoke skills *and* spawn subagents (the burndown fan-out). Codex `exec` is single-shot with no subagent pool; Gemini unverified. The burndown/fan-out drain is therefore **Claude-only for now** even after the trait lands.
3. **Resume/fork-from-live session** — `claude --resume <uuid>` powers the advisor fork-from-live context path; Codex has no session model (`advisor_tier_program_and_args` already forces Codex to cold-boot per punt). Gemini: NO.
4. **stream-json liveness** — the tee/watchdog signal; non-Claude falls back to exit + git-poll.
5. **`.mcp.json` auto-wiring** — Claude Code auto-loads project `.mcp.json` (the `mcp__aida__*` tools); Codex/Gemini need their own MCP config translation, otherwise a headless drain agent can't call `claim_task`/`file_finding`/etc.

## Do-now / spike-more / defer recommendation

**Recommendation: SPIKE-MORE (one empirical end-to-end), then DEFER the full trait refactor; do a small DO-NOW consolidation only if cheap.**

- **DO-NOW (cheap, ~½ day):** none structurally required — the plumbing already exists. Optionally promote the existing `HeadlessVendor` + free functions into the `AgentBackend` trait *as a pure refactor* (no behavior change) to make the abstraction legible and to give EPIC-56's cross-vendor thesis a citable surface. Low risk, but no user-visible capability gain.
- **SPIKE-MORE (the real unknown, ~1-2 days):** run **one** spec end-to-end through `AIDA_HEADLESS_VENDOR=codex` and answer the single load-bearing question: **does a Codex implementer, given a materialized `/aida-pickup` skill body, actually produce a branch + PR + verdict/punt signal file the orchestrator can read?** This is the gate. If Codex can't reliably drive the file-handshake without slash-command resolution, the whole keystone loop is Claude-only regardless of how clean the trait is. This spike retires the highest-risk assumption for the least cost.
- **DEFER (until the spike proves the loop survives):** the full skill-materialization layer, Gemini adapter, MCP-config translation, and stream-json liveness parity. These are 3-4x the cost of the spike and are wasted if the empirical gate fails. The fan-out/subagent burndown drain stays Claude-only and should be documented as such.

The strategic stance (keep cross-vendor open to feed the research thesis) is **already partially honored in-tree** (STORY-683/TASK-894/TASK-895) at near-zero ongoing cost. The cheapest way to advance the thesis further is the one empirical Codex end-to-end run, not a speculative trait refactor.

## Followups (out of scope here)

- Materialize `.claude/skills/*.md` into prompts for non-Claude vendors (the `seed_skill_prompt` hook).
- `.mcp.json` → codex/gemini MCP config translation for headless drain agents.
- Document the fan-out/subagent burndown drain as Claude-only.

## Critical Files

- `aida-cli/src/session.rs` — `HeadlessVendor`, `headless_vendor_args`, `spawn_vendor_headless`, `advisor_tier_program_and_args`, `claude_headless_args*`, `codex_headless_args` (the de-facto backend; promote to trait here)
- `aida-cli/src/main.rs` — `RealPhaseDriver::{run_implementer,run_reviewer,resume_implementer}`, `agent_new_claude/codex/antigravity`, `AgentLaunchConfig`, `exec_claude_headless`
- `aida-cli/src/forge.rs` — the `Forge`/`ForgeKind`/`forge_for` pattern to mirror exactly
- `aida-cli/src/intake.rs` — `seed_skill_prompt`/`seeded_advise_prompt` (the skill-invocation seam; the hardest-piece insertion point)
- `aida-cli/src/auto_complete.rs` — `PhaseDriver` trait + the verdict/signal-file result-parsing contract the backend must keep neutral
