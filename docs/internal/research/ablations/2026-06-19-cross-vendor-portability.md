# Experiment: cross-vendor portability — time-to-productive for a new vendor, zero bespoke integration

- **Date:** 2026-06-19
- **Probe:** EPIC-48 (multi-vendor agent coordination) / EPIC-50 verdict-tilt move #4. Spec TASK-870.
- **Status:** Run once (n=1 vendor, n=1 trivial task). A pilot — the headline is a *capability existence-proof*, not a study.
- **The claim under test (verdict-tilt #4):** a brand-new *vendor's* agent can become productive in a running AIDA fleet with **zero bespoke integration** — using only the stock `aida` CLI on its PATH. P8b's business case (theory paper §13.7) rests on this onboarding cost being near-zero; it is the one thing single-vendor incumbents (Claude Agent Teams, Codex sub-agents) structurally cannot show, because a *rival* vendor joining their fleet has no native seat at all.

## Why this is the load-bearing test

The largest red-team hole in P8b is *"the gap existing != a small player capturing it."* The strongest rebuttal is not an argument; it is a stopwatch: drop a genuinely different vendor into a live fleet, give it nothing but the CLI and a one-line "go do your assigned work" prompt, and measure how long until it has *discovered → claimed → shipped* a unit of work — and how many lines of integration code that cost. If the answer is "0 lines, under a minute," the onboarding-cost premise holds for at least one vendor pair.

## Setup

A throwaway, fully self-contained "running fleet" (no real-store pollution):

- Temp project: `mktemp`-style dir → `git init` → seed `README.md` → `aida init --no-skills --no-hooks --no-agent-config`. This is the live AIDA project a new vendor "joins" (its own `.aida-store/`, cache, `AGENTS.md`). The real repo's `.aida-store` was never touched.
- **The fleet operator** (this Claude session, acting as advisor) filed one trivial unit of work and routed it:
  - `aida add --title "Add a greeting helper to README" --type task --status approved` → `TASK-1` (append one line to `README.md`).
  - `aida brief codex TASK-1 --note "...use the stock aida CLI...commit with the (TASK-1) trailer"` → a pickup brief under `.aida/agent-briefs/codex/`.
- **The new vendor: Codex** (`codex-cli 0.139.0`, model `gpt-5.5`) — a genuinely different vendor than Claude. Invoked headless:
  `codex exec --dangerously-bypass-approvals-and-sandbox "<one-line prompt>" </dev/null`
  with CWD pinned to the throwaway project.
- **The prompt — deliberately minimal, no AIDA knowledge baked in:**
  > "You are a new agent joining this project. Use the stock `aida` CLI on your PATH to discover and complete the work assigned to you (agent name: codex). When done, commit with the (SPEC-ID) trailer convention. Stay in this working directory."

  No wrapper script, no AIDA-specific instructions, no list of subcommands, no MCP config. The only AIDA-shaped facts in the prompt are the *name of the binary* (`aida`), the *agent name* (`codex`), and the *commit-trailer convention* — all of which a fleet operator would say in one breath. Everything else (which subcommand discovers work, how to claim it, how status flips) Codex had to learn from the CLI itself.

## Result — Codex's actual path (from the run trace)

Codex was self-directing the entire way. Its sequence:

| Leg | What Codex ran (stock CLI only) | Outcome |
|---|---|---|
| **Discover** | `aida brief list --for-agent codex` + `aida queue list --for implementer` | Found the pending `TASK-1` brief on the **first command**. |
| **Read** | `aida brief read <path>` + `aida show TASK-1` | Read the brief + backing spec before touching code (correct discipline, unprompted). |
| **Claim** | `aida brief ack <path>` + `aida edit TASK-1 --status in-progress` | Acked the brief, moved the spec to In Progress. |
| **Do** | one-line patch to `README.md` + `git diff` review | Made the exact requested change, verified it. |
| **Ship** | `aida edit TASK-1 --status completed` + `git commit -m "[AI:codex] docs(readme): add vendor agent greeting (TASK-1)"` | Committed with the `(TASK-1)` trailer **and** the `[AI:codex]` author tag — unprompted on the author tag. |
| **Verify** | `aida show TASK-1` | Confirmed `✓ Completed` with git linkage filled in (commit linked, "merged to main"). |

### The headline metrics

| Metric | Value |
|---|---|
| **Lines of bespoke integration written** | **0** (stock `aida` CLI only; no wrapper, no MCP config, no glue) |
| Wall-clock, prompt → completed + verified | **~50 s** |
| Time-to-first-productive-action (discovery succeeded) | **first command, < 1 s of CLI time** (`aida brief list --for-agent codex` returned the work) |
| Commands needed to discover work | **1** |
| Did it claim correctly? | Yes (`brief ack` + status flip) |
| Did it ship a trailered commit? | Yes (`(TASK-1)` trailer + `[AI:codex]` tag) |
| Did the spec reach Completed with git linkage? | Yes (`✓ Completed`, commit linked) |
| Human / AIDA-specific intervention during the run | None |

Codex reached a productive action (it *found its assigned work with one stock command*) effectively immediately, and rode the full discover → claim → ship → verify loop to a clean, correctly-trailered, auto-completed spec in ~50 seconds — with zero integration code.

## The one friction point (honest finding)

The CLI surface itself was self-describing enough that Codex never stumbled on it — but the **brief's generated `## Setup` block was wrong for this context.** It hardcoded the *real* AIDA repo path:

```bash
cd /home/joe/ai/aida
git worktree add /home/joe/ai/aida-task-1 -b task-1 origin/main
...
```

i.e. `aida brief` derived the worktree-setup instructions from the binary's own known repo location, not from the project the brief was filed in. **Codex correctly ignored it** — it stayed in its working directory (as the prompt said) and did not blindly `cd` into an unrelated path. So the friction did *not* cost the run, and the outcome is partly a credit to Codex's judgment. But it is a real brief-generation defect: a cold vendor that *had* followed the brief's Setup block literally would have left the project. The portability of the *CLI* is clean; the portability of the *brief template* has a latent path-assumption bug. (Filed as the follow-up below.)

This is the more valuable half of the result: the experiment was designed to surface friction, and the friction it found is a concrete, fixable substrate defect rather than a hand-wave.

## What this does — and does not — establish for P8b

**Establishes (for this vendor pair, this task):**
- The onboarding cost for a *new vendor* into an AIDA fleet can genuinely be **0 lines of integration** — the stock CLI is self-describing enough that a cold Codex, told only the binary name + its agent name + the trailer convention, discovered, claimed, shipped, and verified a unit of work unassisted.
- The discover→claim→ship→verify loop runs entirely over the **public CLI surface** (`brief list/read/ack`, `show`, `edit --status`, plus plain `git`). No vendor-specific code path exists or is needed.
- This is structurally unavailable to single-vendor incumbents: a rival vendor cannot become a peer in Agent Teams or Codex sub-agents at all, let alone with zero glue. The asymmetry is the point.

**Does NOT establish:**
- **n=1 vendor.** One rival (Codex) on one machine, one account. TASK-870's ideal is a 4+ vendor table (Gemini/Jules/Cursor/…); this is the first row. Gemini-CLI / Cursor-agent rows are the obvious next runs.
- **n=1 task, trivial.** A one-line README change. It exercises the *coordination* surface (find/claim/ship/auto-complete), not real implementation difficulty. It deliberately does not test whether a new vendor produces *good code* — only whether it can *participate* with zero integration. (The 2026-06-17 Claude-vs-Codex bake-off already showed Codex producing real, compiling, tested features inside AIDA worktrees — that is the code-quality leg; this is the onboarding-cost leg.)
- **No MCP leg.** This run used the CLI surface only. The MCP tool surface (`list_briefs`/`read_brief`/`ack_brief`/`update_requirement`/…) is the equivalent path for an MCP-speaking client and was not separately timed here.
- **Self-report caveat:** timings and leg-by-leg account are read from Codex's own run trace + the resulting git/AIDA state, both of which were independently verified post-run (final `README.md`, `git log`, `aida show TASK-1 = ✓ Completed`). No LLM judge was involved; grading is deterministic (did the spec reach Completed with a linked trailered commit? yes).

## Pairs with the existing cross-vendor evidence

This is the *onboarding-cost* datapoint; it sits alongside the *operating-in-AIDA* datapoints already on record:

- `2026-06-17-competitive-claude-vs-codex.md` — Codex implementing a real bounded feature headless in an AIDA worktree (code-quality leg).
- `2026-06-18-open-brief-convergence.md` (I2/I3 cross-vendor cells) — Codex operating from AIDA briefs across the gate-vs-rule cells.

Together: Codex can *join with zero integration* (this doc), *operate from the shared substrate* (open-brief / gate-vs-rule), and *produce shippable work* (the bake-off). The three legs are the empirical spine under P8b's "a new vendor is productive in the fleet" claim — still vendor-pair-limited (Claude↔Codex), still wanting more rows.

## Followups

- **Fix the brief `## Setup` path assumption** — `aida brief`'s generated Setup block hardcodes the binary's known repo path rather than deriving from the project the brief targets; a cold vendor following it literally would leave the project. File as a brief-generation bug.
- **Add table rows** — repeat with Gemini-CLI and/or Cursor-agent for a true 4+ vendor time-to-productive table (the TASK-870 ideal).
- **Add an MCP leg** — repeat the same loop through the MCP tool surface to show CLI/MCP parity for onboarding.

<!-- trace:TASK-870 EPIC-48 -->
