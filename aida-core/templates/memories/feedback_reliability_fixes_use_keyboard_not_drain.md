---
name: feedback_reliability_fixes_use_keyboard_not_drain
description: "Reliability fixes for the autonomy keystone itself ship best at the keyboard (--zen) with the live advisor watching — NOT through unsupervised headless drain. The fix rides through the broken system; if the broken system's failure rate is non-trivial, the fix gets caught in it."
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
The dogfood-merge pattern ([[feedback_self_test_via_dogfood_merge]]) says *"ship the fix through the system being fixed; the merge exercises the new code path."* That principle holds for **most** fixes — feature work, bug fixes whose failure mode doesn't affect drain machinery itself, anything where the broken-vs-fixed state of the codebase doesn't cascade onto the shipping mechanism.

It does NOT hold for fixes whose failure mode **directly impedes their own shipping pipeline.** Those are *recursive-failure-risk* fixes. Shipping them headless risks them getting caught in the very pattern they're patching.

## When to recognize a recursive-failure-risk fix

- The fix targets the **orchestrator's lease management** (BUG-307: auto-clean dormant leases; BUG-311: --steal silent failure). A drain of these fixes runs in the broken lease environment they're fixing.
- The fix targets the **reviewer skill template under headless** (BUG-280, BUG-327). A drain of these requires the reviewer phase to work correctly to ship the reviewer fix.
- The fix targets the **implementer skill template under headless** (TASK-401, BUG-285). Same shape: implementer phase shipping implementer-phase fixes.
- The fix targets **phase-X recovery/retry/escalation** in the orchestrator (BUG-266, BUG-286). The drain shipping the retry-layer needs working error handling to survive its own drain's transient errors.

These are all "fix touches the layer the drain depends on to ship the fix."

## What to do instead — keyboard-driven `--zen --auto-complete`

For recursive-failure-risk fixes:

1. Run **`aida queue work <spec> --zen --auto-complete`** (NOT `--no-human=both`). Orchestrator drives all 6 phases, BUT the advisor is on standby and you're at the keyboard.
2. Watch phase 1 to confirm the implementer doesn't trip the very fault the fix is patching. If it does, you have eyeballs on it immediately and can intervene.
3. The advisor (this session) reads the implementer's intent + the spec's acceptance + sees the live implementation. Available for any design-fork that surfaces.
4. Phases 2-6 run normally; the fix exercises the system but with your eyes on the result.

## Why this isn't an argument against dogfood-merge

The dogfood-merge pattern surfaces the NEXT gap by running real work through real conditions. That value is preserved here:

- The fix STILL ships through the system being fixed (phases 2-6 run normally).
- The merge still exercises the new code path.
- The dogfood-merge surfaces remaining gaps in the same way.

The only thing changed: **phase 1 (implementer) and phase 3 (reviewer) are watched, not unsupervised.** If they hit the bug the fix patches, you observe it directly instead of waking up to a stalled drain.

## Empirical evidence

2026-05-22 overnight drain queued BUG-307, BUG-311, BUG-310 (three reliability fixes for the orchestrator) at the head, followed by three smaller specs. **The three reliability fixes ALL stalled at phase 1** in the dormant-lease-conflict state they were themselves patching. The implementer completed their work in worktrees but couldn't publish — recovery required manual push + PR + merge in the morning. The same drain shipped the three smaller specs (TASK-451, TASK-452, BUG-312) that follow them cleanly because those specs' failure modes didn't cascade onto their own shipping mechanism.

Net: drain shipped 50% of specs end-to-end; the half that stalled was exactly the recursive-failure-risk half. That's not noise; that's the pattern.

## Tradeoff explicitly named

Recursive-failure-risk fixes ship slower (~1 hour at-keyboard vs ~30 min headless), but they ship reliably. The cost of stalling overnight + recovering in the morning is higher than the cost of watching for ~1 hour at the keyboard.

## Composes with

- [[feedback_self_test_via_dogfood_merge]] — the broader principle this refines for the recursive case
- [[feedback_three_mode_autonomy_taxonomy]] — `--zen` mode is the correct surface for at-keyboard, advisor-on-standby work
- [[feedback_advocate_not_be_passive]] — naming this refinement is advocacy; not naming it lets reliability fixes keep stalling
