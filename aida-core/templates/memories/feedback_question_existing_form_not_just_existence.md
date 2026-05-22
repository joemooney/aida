---
name: feedback_question_existing_form_not_just_existence
description: "When the user proposes a design and prior art exists, surface BOTH the prior art AND whether its current implementation form still aligns with the project's principles. Path-dependence ≠ correctness."
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
When the user proposes a design and the advisor finds existing prior art (a primitive, command, doc, or pattern that already solves the proposed problem), the first move is to surface it — *"you've already shipped this; here's how to use it."* That's the easy half. **The equally load-bearing second move is to ask whether the prior art's current implementation form still aligns with the project's stated principles.** Implementation forms get chosen path-dependently — *"bash got there first,"* *"filesystem worked so we stuck with it,"* *"we copy-pasted the shape from the previous feature."* The original choice may have outlived its rationale. Surface the prior art **and** question whether its current shape is still the right one.

**Why:** 2026-05-20 evening. User asked about server-shape orchestration; advisor surfaced \`aida-worker\` (TASK-294) — the existing shell-function-based directive-FIFO runner. *Half-right.* The advisor stopped at *"you have it; here's how to use it."* The user then pointed out the missed move: *"to be honest, we need to figure out if the aida binary itself can handle this instead of introducing different commands and shell dependencies ... from a user perspective I think aida as the vector into all the commands is preferable."* That observation produced STORY-377 (migrate aida-worker shell function → \`aida worker run\` Rust subcommand) — the *right* answer was \"yes you have it, AND it's in the wrong shape, AND here's the consolidation.\"

**How to apply:**
- When surfacing prior art, name the principle the prior art is *supposed* to satisfy (single-vector CLI, filesystem-canonical, agent-agnostic substrate, whatever the project's stated principles are) and check whether the current implementation satisfies it.
- Watch for *path-dependent forms*: \"this was implemented in bash because bash got there first,\" \"this is in shell-init because the previous similar feature was,\" \"this lives in main.rs because we hadn't pulled the module out yet.\" Path-dependence is not correctness; it's history.
- When the user's question implicitly questions the form (*\"can the binary itself handle this?\"* / *\"should this be a separate command?\"* / *\"why does this need to be a hook?\"*), the advisor's job is to answer the form-question, not just the does-it-exist question.
- File the consolidation as a follow-up STORY when the form is wrong. Note explicitly that path-dependence was the original cause; document the principle the new form satisfies.
- This is **not** an invitation to relitigate every implementation choice on every question. Apply when the user's design question crosses a stated principle (CLI surface coherence, transport canonicity, agent-agnosticism, etc.). Routine questions don't trigger it.

**Composes with:**
- [[feedback_run_help_before_suggesting_flags]] — check existing surface first (what exists).
- [[feedback_pushback_on_overengineering]] — challenge architectural drift toward complexity.
- [[feedback_advocate_not_be_passive]] — naming the architectural mismatch is part of advocacy; not naming it is the passive failure mode.
