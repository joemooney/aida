---
marp: true
title: "AIDA — Executive Briefing"
description: "10-minute executive overview: the problem, the wedge, the proof, the ask."
paginate: true
theme: default
---

<!--
RENDER:
  npx @marp-team/marp-cli@latest docs/presentation/aida-executive-briefing.md -o aida-executive-briefing.html
  npx @marp-team/marp-cli@latest docs/presentation/aida-executive-briefing.md --pdf -o aida-executive-briefing.pdf
AUDIENCE: executives / non-technical leadership / investors. GOAL: understand the bet + the moat in 10 minutes.
FORMAT: outcome-first, almost no CLI. Speaker notes in HTML comments. Numbers refresh from `aida list --all`.
COMPANION DECKS: developer-deep-dive, administrator-guide, user-walkthrough.
-->

# AIDA

### The agent-collaboration layer

A git-canonical requirement graph that survives across **agents, sessions, and vendors.**

<small>v0.11.0 · executive briefing</small>

<!--
One line to open: "Coding agents are getting dramatically more capable. The bottleneck is no longer writing code — it's remembering why, across many agents and many months. AIDA is the memory."
-->

---

## The shift that creates the problem

- AI coding agents now do **hours of autonomous work** per run.
- Teams run **many** of them — Claude, Codex, Antigravity — and **many** people.
- Each agent starts **cold**: it re-derives yesterday's context every session.
- The link between **code and intent rots silently** — nobody can answer *"why is this here?"* six months on.

> The cost has moved from *writing* software to *remembering* it — across agents, sessions, and vendors.

<!--
Don't pitch yet. Make them feel the pain. If they've run agents at scale, they've felt the amnesia + the drift.
-->

---

## What AIDA is, in one sentence

**Your project's missing index** — a stable, queryable graph of *what exists and why*, served to AI agents over a standard protocol (MCP) and to people through a small CLI.

It turns three expensive questions into one query — for the agent and for you:

- *"Does this already exist?"*
- *"Why did we choose X?"*
- *"Is this code still tied to a live requirement?"*

And it lives in **git** — portable, vendor-neutral, **no SaaS dependency, no lock-in.**

---

## The loop, end to end

**File an idea → walk away → it ships, and the requirement knows it's done.**

```
idea  →  queued  →  agent implements  →  CI  →  agent reviews  →  merged  →  spec auto-marks Completed
```

- A human files a one-line requirement; it gets a **stable ID**.
- The orchestrator drives implement → CI → review → merge **unattended**.
- On merge, the spec **auto-flips to Completed** and a **trace comment lands in the code** linking it back.

> Nobody updated a status by hand. The system did — and it remembers.

<!--
This is the "wow". If there's a terminal, show the recorded cast (docs/presentation/*.cast). Otherwise narrate it as the outcome.
-->

---

## Why this isn't "20 lines of bash"

The surface looks trivial **on purpose** — that's how it gets adopted. The depth compounds:

| Looks like | Actually is |
|---|---|
| a TODO list | a **typed graph** — "what's blocked by what", "what would this unblock" |
| file names that drift | **stable IDs** that survive renames, merges, vendor switches |
| an unchecked comment | **enforced links** from code back to the requirement |
| one agent's scratchpad | a **shared substrate** any agent reads over MCP |
| a status field | a **lifecycle** that advances itself and escalates what it can't decide |

> A simple surface over months of foundation. The surface is copyable in a weekend; the foundation is the moat.

---

## The moat: why a single vendor structurally can't copy it

AIDA lives in **git**, not in any one vendor's cloud.

- A vendor could add a requirement tracker — but it would be **theirs**, locking you to **one** agent.
- AIDA is **vendor-neutral by construction**: one substrate, read by Claude *and* Codex *and* the next tool.
- Anthropic's primitives (`/goal`, `/ultraplan`, MCP) are deliberately **horizontal**. AIDA is **vertical depth on horizontal ground** — and their getting better makes AIDA better.

> The durable wedge is **multi-vendor interop on a substrate you own.** That's the part a single-vendor tool can't ship without giving up its own lock-in.

<!--
The exec question: "Couldn't Anthropic just build this?" Answer: a vertical shrinks their market + competes with their integration partners + only helps teams who track requirements. They have structural reasons to stay horizontal. Backing: docs/positioning/, OVERVIEW.md "vertical depth on horizontal ground".
-->

---

## Proof: AIDA builds AIDA

Fully dogfooded — every feature shipped through the system being demonstrated.

- **1,686** specs in the graph · **991** completed
- **19** releases · **1,439** commits
- **Multi-agent**: Claude + Codex + Antigravity drive one shared git-canonical substrate
- **Autonomous drains** ship batches of work overnight, escalating only what they can't safely decide

> Everything in this deck was built *by* this system, *on* this system — and it gets more capable as it goes.

<!--
Refresh the numbers the morning of: `aida list --all`, `aida list --all --status completed`, `git tag | grep -c '^v'`, `git rev-list --count HEAD`.
-->

---

## Where we are — and the ask

**Today:** a working, dogfooded platform with a humble, adoptable surface and a defensible substrate.

**Next:**
- **Persistent autonomous worker** — continuous queue drain, not one-shot runs
- **Multi-vendor substrate interop** — the durable wedge, hardened
- **Public launch + marketplace distribution**

> The bet: own the **agent-collaboration layer** — the memory every multi-agent team will need — before the category has a name.

<!--
Speak the resource ask out loud rather than putting a number on the slide. Close: "The floor — structured markdown — is commoditized. The layer above it is open, and it's the layer that compounds."
-->

---

## One-line summary

> **AIDA is the memory layer for multi-agent software teams** — a git-canonical requirement graph that any agent can query, that links code to intent, that ships work autonomously, and that no single vendor can lock up.

<small>Deeper dives: developer (`aida-developer-deep-dive`), admin (`aida-administrator-guide`), user (`aida-user-walkthrough`).</small>
