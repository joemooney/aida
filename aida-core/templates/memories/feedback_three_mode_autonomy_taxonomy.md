---
name: three-mode-autonomy-taxonomy
description: Autonomy has two orthogonal axes — is a human present, and what do they want asked. Three modes map human role to pause behavior.
propagation: scaffolding-pack
metadata:
  type: feedback
---

"Autonomy" is not one dial. It has two orthogonal axes: **is a human present**, and **what does the human want to be asked**. Conflating them produces a tool that is uncomfortable for the user at the keyboard who does not want 30 mechanical click-yes prompts, AND wrong for the absent user who needs the drain to keep moving.

The three-mode ladder maps the human's *role* to the implementer's *pause behavior*:

| Mode | Human role | Mechanical prompts | Design-fork prompts |
|------|-----------|--------------------|---------------------|
| Default | Driving | Pause + ask | Pause + ask |
| `--zen` | Advisor on standby | Auto-resolve | Pause + ask |
| `--no-human` | Absent | Auto-resolve | Punt (file a finding) |

The discriminator is the *kind* of each prompt: a **confirmation** (mechanical yes/no, obvious default) versus a **design-fork** (a genuine choice with real cost to guessing wrong). Most prompts are confirmations; design-forks are sparse and meaningful. An un-annotated prompt defaults to design-fork — the pause-safe choice.

**Why:** A user watching a drain wants to stay in the loop on real decisions without clicking through noise. That is a distinct, valuable mode — it needs no headless machinery, just prompt classification.

**How to apply:** When designing any agent-pause behavior, ask both questions separately — presence and consultation-appetite. Classify each prompt by kind. Make the first option of every prompt the smallest-valuable-slice / safe default, since auto-resolve picks option 1.

Composes with [[pause-for-design-input]] and [[pushback-on-overengineering]].
