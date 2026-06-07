# Agent-agnostic vs Claude-specific discipline

**Principle Trace**: `STORY-416` | `feedback_substrate_as_bouncer_not_rules`

AIDA's strongest strategic claim is that it is an **agent-agnostic
substrate**: Claude Code, Codex, Antigravity, Cursor, or any MCP-speaking
agent can participate in the same project against the same spec graph.
But a real project mixes two kinds of discipline, and confusing them is a
recurring trap:

- **Universal discipline** holds for *every* agent (and every human). It
  is enforced by the substrate — git-canonical specs, trace comments,
  commit-subject conventions, pre-commit/reviewer gates — so it cannot be
  bypassed by switching agents.
- **Claude-specific discipline** is the convenience layer that happens to
  be shaped like Claude Code: `.claude/skills/*`, `.claude/commands/*`
  slash commands, `.claude/hooks/*`, `.claude/settings.json`, and the
  starter memory pack under `~/.claude/projects/<slug>/memory/`. These
  *aid* onboarding for one agent; they are not load-bearing for
  correctness.

The rule of thumb: **maximize universal gates, minimize per-agent
template surface area.** Discipline that *must* hold belongs in a gate
(in code, enforced); discipline that merely *aids* onboarding belongs in
docs scaffolded universally plus per-agent setup pointers.

---

## What every agent inherits (universal)

`aida init` scaffolds the universal layer for every project regardless of
which agent will drive it:

- **The spec graph** — git-canonical YAML on the `aida-store` orphan
  branch, plus the `.aida/cache.db` read-cache. The source of truth, not
  any agent's private notes.
- **Trace comments** — `// trace:<SPEC-ID> | ai:<agent>` in code. The
  `<agent>` token is the only part that differs per agent (`ai:claude`,
  `ai:codex`, `ai:antigravity`); the convention itself is universal.
- **Commit-subject convention** — `[AI:tool] type(scope): subject
  (SPEC-ID)`. The auto-bump scanner reads the trailing `(SPEC-ID)` parens
  on every merge, no matter which agent authored the commit.
- **Substrate gates** (the bouncers — see `substrate-as-bouncer.md`):
  the pre-commit gitignored-file check, the pre-commit `cargo fmt` check,
  and the reviewer-phase gate all run on the *commit and the PR*, not on
  the agent. An agent that never read a single skill template still hits
  them.
- **The MCP server** — `aida mcp-serve` exposes the spec graph and the
  coordination tools (claim/release leases, punts, findings, briefs,
  directives) to any MCP-speaking agent. This is the canonical
  machine-to-machine surface, available to every agent equally.
- **`docs/aida/discipline/`** — these guides (you are reading one). They
  describe AIDA-using habits in agent-neutral terms and are scaffolded
  into every project.
- **`docs/agents/`** — the cross-agent onboarding brief
  (`cross-agent-onboarding.md`), per-agent MCP setup docs
  (`codex-mcp-setup.md`, `antigravity-mcp-setup.md`), the install matrix
  (`aida-mcp-install-matrix.md`), and the session-communication reference
  (`session-communication.md`). All scaffolded by `aida init` so a
  Codex- or Antigravity-primary project inherits setup guidance, not just
  a Claude-primary one.
- **`CLAUDE.md` and `AGENTS.md`** — the project orientation. `CLAUDE.md`
  is read by Claude Code (and `@`-imports `.claude/AIDA.md`); `AGENTS.md`
  is the same orientation for Codex and other MCP agents that don't
  expand `@` imports, with the AIDA conventions inlined inside an
  `AIDA-AUTOGEN` delimited block. The two are kept consistent on each
  scaffold; the AGENTS.md block auto-upgrades while content outside it
  stays project-owned.

The litmus test for "is this universal?": **could a brand-new agent that
has never read a skill template still be held to it?** If the answer is
yes — because a git hook, the reviewer, or the auto-bump scanner enforces
it — it is universal. If the only thing carrying it is a prompt the agent
may or may not have loaded, it is per-agent onboarding aid, not a gate.

---

## What is Claude Code-shaped (per-agent)

These are scaffolded by default because Claude Code is AIDA's default
agent today, but they are *one agent's* convenience layer:

| Surface | Claude-specific form | The universal thing underneath |
|---|---|---|
| Workflow skills | `.claude/skills/*/SKILL.md` | `aida` CLI verbs (`aida queue work`, `aida pr ship`, …) |
| Slash commands | `/aida-pickup`, `/aida-pr`, … | the same CLI verbs / MCP tools |
| Hooks | `.claude/hooks/*` + `.claude/settings.json` | the git pre-commit + reviewer gates |
| Starter memories | `~/.claude/projects/<slug>/memory/` | the discipline docs in `docs/aida/discipline/` |
| MCP wiring | `.mcp.json` (Claude Code auto-reads it) | `aida mcp-serve` (any client can connect) |

A Codex or Antigravity user does **not** get `.claude/skills/*`; they get
the universal layer plus their own per-agent setup docs. The
cross-agent skill-invocation map in
`docs/agents/cross-agent-onboarding.md` translates each `/aida-foo` slash
command into the equivalent `aida` CLI verb or MCP tool, so a non-Claude
agent looking at a CLAUDE.md reference to `/aida-pickup` knows it means
`aida queue work`.

**Foundational rule:** the `aida` CLI verbs are the substrate; Claude
Code's slash commands and Codex's skill descriptors *wrap* them. If you
don't know the slash/skill name for your agent type, run the CLI verb
directly — it works for every agent.

---

## Why the split matters at first contact

If a new user picks up AIDA with Codex or Antigravity as their primary
agent, the Claude-specific layer gives them nothing. The agent-agnostic
positioning would ring hollow at first contact if the *only* scaffolding
were Claude-shaped. That is why:

1. The universal gates are written once and enforced forever — they do
   not depend on any agent loading a template.
2. `docs/agents/` and `AGENTS.md` are scaffolded for every project, so a
   non-Claude-primary project still inherits cross-agent setup guidance.
3. This document exists: so a reader (including a future Codex or
   Antigravity user) can tell which discipline is universal and which is
   Claude-specific, and not mistake the absence of `.claude/skills/` for
   the absence of AIDA's discipline.

The per-agent template surface area is a linear cost with drift risk;
the universal gates are a fixed cost paid once. When you add new
discipline, prefer a gate or a universally-scaffolded doc over another
per-agent template — and if you must add a per-agent template, make sure
the universal thing underneath it is what actually enforces correctness.

---

## When adding cross-agent support

- A new agent needs: an MCP setup doc under `docs/agents/<agent>-mcp-setup.md`,
  a row in the install matrix, and (optionally) per-agent skill
  descriptors. It does **not** need a reimplementation of the gates —
  those already cover it.
- A new convention that *must* hold for correctness: ship it as a gate
  (pre-commit hook, reviewer-phase check, auto-bump scanner rule), not as
  a line in one agent's skill template.
- A new convention that *aids* onboarding: add it to the relevant
  `docs/aida/discipline/` guide (scaffolded universally) and, if it has a
  Claude-specific shortcut, note the universal equivalent in the
  cross-agent skill-invocation map.
