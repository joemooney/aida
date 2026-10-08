# Second-opinion brief: SPIKE-32 — the workflow.js compiler thesis

**Date filed**: 2026-05-29
**Target reader**: any independent agent (Codex, Antigravity, web Claude, fresh Claude Code session)
**What's being requested**: an adversarial design review of AIDA's "compile spec graph → emit Claude Code workflow.js" architectural thesis, with specific focus on whether the framing has hidden conflations.
**Time budget**: 30–60 minutes. Aim for a punchy verdict + the 3 strongest objections + the 2 weakest assumptions.

---

## Context for the second-opinion reader

AIDA is an "AI Design Assistant" — a git-canonical requirements / spec-graph substrate that orchestrates coding work across multiple agent CLIs (Claude Code, Codex, Antigravity). Today AIDA's *orchestrator* is a Rust-built 6-phase pipeline (implementer → CI-wait → reviewer → merge → pull → build) that the operator runs from a TUI. The orchestrator IS the runtime — it spawns Claude Code sessions, watches for completion, advances phases, handles failures with shelving/escalation.

On 2026-05-29 a strategic-recompose synthesis (see `docs/competitive-analysis/2026-05-29-strategic-recompose-post-2.1.154.md`) concluded that Anthropic now ships natively a large fraction of what AIDA's orchestrator does — workflows, agent dispatch, agent view, /goal, agent teams. The synthesis verdict: AIDA's orchestrator should transition from runtime to **compiler** — emitting Claude Code workflow.js artifacts that Claude Code's runtime then replays.

**On the same day the user corrected my framing of Claude Code Workflows:** Workflows are the **dynamic-generation lane** (decide fan-out at runtime per invocation), NOT the deterministic-orchestration lane. Determinism in Claude Code comes from saving a run's script and replaying that fixed artifact (`claude workflow save <run> as <name>`).

AIDA's "compile spec → emit workflow.js" therefore lives in the **saved-script lane**: AIDA's compiler runs once per spec change, produces a workflow.js artifact, that artifact gets checked in next to the spec, and Claude Code's runtime replays it on demand. Build artifact, like `Cargo.lock`.

## The current thesis (please grill this)

```
┌──────────────────────────────────┐
│ AIDA spec graph (git-canonical)  │
│  Specs, leases, briefs, history  │
└──────────┬───────────────────────┘
           │ compiles to
           ▼
┌──────────────────────────────────┐
│ AIDA compiler                    │
│  Reads spec + acceptance + children│
│  Emits a workflow.js per drain   │
│  Output checked in to the repo   │
└──────────┬───────────────────────┘
           │ executes via
           ▼
┌──────────────────────────────────┐
│ Claude Code workflow runtime     │
│  16 concurrent / 1000 total      │
│  agent() / pipeline() / parallel │
│  Structured output schemas       │
│  Resumable within session        │
└──────────┬───────────────────────┘
           │ observed by
           ▼
┌──────────────────────────────────┐
│ AIDA supervisor + bus            │
│  Pickability, auto-bump, escalation│
│  Findings, cross-tool routing    │
└──────────────────────────────────┘
```

### Concrete CLI surface being proposed

```bash
aida workflow compile <SPEC> [-o <path>]   # emit workflow.js next to spec
aida workflow compile --batch <NAME>        # emit workflow.js per batch member
aida workflow verify <SPEC>                 # confirm checked-in workflow.js matches what compile would emit
```

The compiled `workflow.js` would import:
- The spec's children as phases
- Implementer → reviewer → merger as `agent()` calls
- `pipeline()` for resumability
- AIDA's MCP server for spec-graph queries during execution

## What I want second-opinion on

### A. Is the saved-script lane the right home?

**My claim:** AIDA's drain loop wants same-plan-every-time. Spec X status flips to InProgress → implementer agent → CI → reviewer → merger → status flips to Completed. This IS the deterministic-plan case. Saved-script lane fits.

**Possible counter:** maybe AIDA actually wants dynamic generation — number of phases depends on spec type, complexity, batch membership? If so we should be in dynamic Workflows lane after all, not saved-script.

**Question to probe:** is the variability AIDA needs (spec-type-dispatch, batch handling, failure routing) so small that one compiled script per spec covers it, or so large that compile-time emission would explode into 10^N script variants?

### B. Where does the IR live?

If compile is once-per-spec-change, AIDA needs an intermediate representation that survives between source (spec YAML) and output (workflow.js). Options:
- **No IR** — direct AST emission. Simple, but spec-change → recompile-all
- **JSON IR** — checked in alongside workflow.js. Diffable. Lets us version the contract independently
- **Procedural macro / build.rs** — compile happens at AIDA-binary build time, not runtime

**Question:** has any reader of this brief done a real compiler design? What does experience say about IR placement for this scale of compile (handful of nodes per workflow)?

### C. Cross-machine implications

AIDA's substrate is git-distributed. Two clones can independently flip a spec to InProgress and run drains. If workflow.js is checked in, both clones replay the same script — good. But if both clones recompile the spec at the same time (different content), we get a merge conflict on workflow.js.

**Mitigation candidates:**
1. Only emit on `aida pull` (server-side compile, push-time conflict)
2. Don't emit at all — workflow.js is generated per-drain, gitignored, never committed (back to dynamic-generation framing… contradicts the thesis)
3. Lock the workflow.js to a content-addressed name keyed off spec.yaml's SHA so concurrent compiles don't collide

**Question to probe:** is the conflict scenario as bad as I'm imagining? Or am I overweighting it?

### D. Failure routing — does the compiled script handle it?

AIDA's orchestrator today handles a lot of failure-routing logic: shelving on CI red, escalating punts to advisor, pausing for human ack. That logic lives in `aida-cli/src/auto_complete.rs` and friends.

If we emit workflow.js, that failure-routing has to either:
- Live in the compiled script (script complexity grows)
- Live in Claude Code's runtime (not currently exposed as workflow hooks)
- Live in AIDA's supervisor reading the workflow.js execution output

**Question:** is the compose pattern viable when failure-routing is this rich?

### E. What's the right killer demo?

If we ship SPIKE-32, what's the 60-second screencap that makes the value visceral?
- "Edit a spec's acceptance, see `git diff` show the workflow.js changed too" (build-artifact framing)
- "Run aida workflow compile, copy the URL of the resulting workflow.js into Claude Code, watch it execute"
- Something else?

## What I do NOT want second-opinion on (already addressed elsewhere)

- The choice of git-canonical substrate over SQLite — debated and decided in `docs/plans/2026-05-02-git-canonical-storage.md`
- The 3-mode autonomy ladder (default/--zen/--no-human) — see `docs/architecture/autonomy-and-escalation.md`
- Whether AIDA should compete with Claude Code on the runtime surface — synthesis concluded no, compose
- Whether AIDA should compete on agent dispatch (--bg, claude agents) — SPIKE-34 shipped; conclusion was wrap, don't reimplement

## Files / specs to read for grounding

- `docs/competitive-analysis/2026-05-29-strategic-recompose-post-2.1.154.md` — the original synthesis
- `docs/competitive-analysis/2026-05-29-claude-code-2.1.154-decompose/01-dynamic-workflows.md` — SPIKE-14 write-up (note: had the wrong "deterministic" framing; corrected via memory)
- `aida list --tags from-strategic-recompose` — SPIKEs 30-34 (shipped) plus SPIKE-32 (this one)
- Anthropic's docs at <https://code.claude.com/docs/en/workflows> — the actual workflows surface

## Desired return shape

Please reply with:

1. **Verdict (1 paragraph):** is the saved-script-lane thesis sound? If yes, what's the strongest objection to it. If no, what's the better frame?

2. **Top 3 objections** to the proposed architecture (compiler → workflow.js artifact → Claude Code replay → supervisor observes). For each: claim, why it might be wrong, what evidence would settle the question.

3. **Top 2 hidden assumptions** I'm making that aren't load-bearing in the brief above. (Things I'd defend if asked but didn't bother to state.)

4. **One concrete proposal** for the killer 60-second demo if SPIKE-32 shipped.

5. **Recommendation:** ship it as scoped (months-not-weeks, design pass before code), reshape it (different lane, different surface), or kill it (compose pattern works without a compiler).

Brevity matters — under 600 words preferred. Reply as markdown.

---

*This brief was generated by AIDA's master advisor session. The user (Joe Mooney) will hand it to you outside of AIDA's own substrate; your reply is ground-truth to me only when Joe relays it back. trace:SPIKE-32 trace:from-strategic-recompose-round-2*
