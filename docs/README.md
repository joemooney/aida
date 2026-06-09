# AIDA documentation

Start here. This index orders the docs by where you are in the journey — from
*"what is this?"* to running it for a team to extending it. Not sure AIDA is for
you at all? Jump to [Deciding whether to adopt](#deciding-whether-to-adopt).

> New to AIDA? The three-doc on-ramp is **[Why AIDA?](WHY-AIDA.md)** →
> **[Getting started](getting-started.md)** → **[First project](first-project.md)**.
> That's the 15-minute path from "why does this exist" to a merged spec graph on
> your own machine.

---

## 1. New to AIDA — the on-ramp

| Doc | What it gives you |
|-----|-------------------|
| [WHY-AIDA.md](WHY-AIDA.md) | The problem statement — *why this exists* before *how it works*. |
| [getting-started.md](getting-started.md) | Install (cargo / binary / Docker / source), `aida init`, your first requirement. |
| [first-project.md](first-project.md) | The follow-along: a TODO CLI from a blank repo to a six-spec graph with two features driven to *Completed*. The best "see what it's *for*" doc. |
| [using-aida-with-claude-code.md](using-aida-with-claude-code.md) | How AIDA and Claude Code fit together (skills, MCP, the daily loop). |

## 2. Daily use

| Doc | What it gives you |
|-----|-------------------|
| [user-guide.md](user-guide.md) | Daily-use reference for the CLI and the dashboard. |
| [lifecycle.md](lifecycle.md) | The Draft → Approved → Planned → In Progress → Done → Completed → Released state machine, the verb for each transition, and the edge cases. |
| [autonomous-drain.md](autonomous-drain.md) | The hands-off backlog drain + the three autonomy modes (interactive / `--zen` / `--no-human`), escalation, and calibration. |
| [git-workflow.md](git-workflow.md) | Commit/branch conventions, the `(SPEC-ID)` trailer, and the two-leg code+store sync. |

## 3. Deciding whether to adopt

These cut through the *"should I even use this?"* question honestly.

| Doc | What it gives you |
|-----|-------------------|
| [positioning/](positioning/README.md) | One focused *"AIDA vs X"* comparison per neighbour tool — Spec Kit, Kiro, Agent Teams, subagents, Aider, Continue, SaaS PM, Karpathy markdown. |
| [positioning/when-not-to-use-aida.md](positioning/when-not-to-use-aida.md) | The honest scope limits — six cases where a neighbour tool alone is the right call. |
| [positioning/composition.md](positioning/composition.md) | When the answer is *"use AIDA **with** X"* — concrete recipes + the seam for each. |
| [competitive-analysis/](competitive-analysis/) | The living landscape scan: dated snapshots + per-topic tracking of the AI/dev-tools market. |

## 4. Operating AIDA for a team

| Doc | What it gives you |
|-----|-------------------|
| [admin-guide.md](admin-guide.md) | Storage backends, migration, multi-user setup. |
| [storage-modes.md](storage-modes.md) | Distributed (git-canonical) vs the legacy modes — the full comparison. |
| [multi-user-setup.md](multi-user-setup.md) | PostgreSQL-backed shared projection deployment. |
| [multi-node.md](multi-node.md) | Distributed node identity, IDs, and the merge gate. |
| [multi-advisor-coordination.md](multi-advisor-coordination.md) | Coordinating multiple advisor seats on one project. |
| [session-lifecycle.md](session-lifecycle.md) | Scoped sessions, worktrees, and leases — how concurrent work stays isolated. |
| [agents/](agents/) | Per-agent setup (Claude Code, Codex, Cursor, …), the MCP install matrix, and inter-agent communication. |

## 5. Extending & contributing

| Doc | What it gives you |
|-----|-------------------|
| [UNDERSTANDING_SKILLS.md](UNDERSTANDING_SKILLS.md) | How Claude Code skills and commands differ. |
| [extending-skills.md](extending-skills.md) · [skills-convention.md](skills-convention.md) | Authoring and conventions for AIDA's skills/commands. |
| [user-facing-text-conventions.md](user-facing-text-conventions.md) | The rule that keeps SPEC-IDs out of user-facing output. |
| [forge-providers.md](forge-providers.md) | The forge abstraction (GitHub / GitLab providers). |
| [git-verb-surface.md](git-verb-surface.md) | The convention behind the two-leg git-mirror verbs. |
| [architecture/](architecture/) | Deeper design: autonomy + escalation, the MCP coordination surface, on-disk serialization. |
| [plans/](plans/) | The implementation-plan archive (`YYYY-MM-DD-<slug>.md`, chronological). |

## 6. Vision & background

| Doc | What it gives you |
|-----|-------------------|
| [future-vision.md](future-vision.md) | AIDA in the agentic-coding era — where the bet is heading. |
| [aida/](aida/) | The constitution, vision, constraints, and glossary that govern the project. |
| [presentation/](presentation/README.md) | Audience-targeted slide decks (executive / developer / administrator / user) + a live-demo deck. |

---

*Also at the repo root: [`OVERVIEW.md`](../OVERVIEW.md) (the big-picture vision) and
[`CLAUDE.md`](../CLAUDE.md) (conventions for agents working in this repo).*
