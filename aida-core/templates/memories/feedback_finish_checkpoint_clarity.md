---
name: Implementer finish-checkpoints need state → deciding factor → recommendation → consequence-laden options → advise-escape
description: When the implementer presents a "how to finish?" menu, it must surface state, the deciding factor, a recommendation with rationale, and each option's downstream drain-consequence — plus an explicit advisor-escape. Decouple coupled decisions.
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
When the implementer presents a finish-checkpoint menu — "Mark done, push, open PR / Push, hold PR / Hold local / Type something" or anything similar at the end of phase 1 — the menu MUST include all six of these:

1. **State snapshot** — commits, push status, PR status, drain phase, test/fmt status, plan file. The reader (human OR a future headless advisor) should not have to *infer* current state.
2. **The deciding factor** — any load-bearing risk that frames the choice (smoke-test gate, plan deviation, change size). Surfaced *next to* the options, not only in the upstream preamble.
3. **A recommendation with rationale** — not a flat neutral menu. The implementer has the analysis; lead with *"I recommend X because Y"* and present alternatives as variants.
4. **Per-option downstream consequence** — each option needs a line on what the orchestrator / drain does next (advances to phase N, halts cleanly, likely stalls under `--zen`) AND a reversibility note.
5. **An explicit `advise` escape** — route the checkpoint to the advisor. Today: a surfaced option for the user to relay manually. Once STORY-306 ships: automatic. Treat it as a first-class option, not a fallback "Type something."
6. **Decouple coupled decisions** — push/PR is one decision; followup-filing is another; merge timing a third. Bundling them into one option locks them; ask in sequence.

**Why:** 2026-05-19 — STORY-306's finish-checkpoint presented a 4-option flat menu (mark-done-push-PR / push-hold-PR / hold-local / type-something). The deciding factor — unsmoke-tested `claude -p` subprocess plumbing requiring a SPIKE-7 smoke before merge — was flagged in the implementer's preamble but disconnected from the choices. Option 1 bundled the followup-filing decision. No `advise` escape was offered (mildly ironic on STORY-306's *own* finish — the spec exists to route exactly these calls to the advisor). The user's verbatim feedback: *"I was not presented with clear instructions by the implementer."*

**How to apply:**
- When *writing or reviewing* a finish-checkpoint skill template, audit against the six elements above.
- When *acting as* the finish-checkpoint advisor (user relays a menu for the call), restructure the menu before deciding — surface state, recommendation, consequences — even if the implementer's prompt didn't.
- For the future headless advisor (STORY-306): these six elements are the rubric the advisor applies. Same structure, no relay.

**Composes with** [[feedback_check_orchestrator_before_manual_steps]], [[feedback_pause_for_design_input]], [[feedback_parallel_vs_sequential_ui]].
