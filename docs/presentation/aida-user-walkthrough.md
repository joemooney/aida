---
marp: true
title: "AIDA — User Walkthrough"
description: "Daily use: file a requirement, hand it to an agent, ship it, keep code tied to intent."
paginate: true
theme: default
---

<!--
RENDER:
  npx @marp-team/marp-cli@latest docs/presentation/aida-user-walkthrough.md -o aida-user-walkthrough.html
  npx @marp-team/marp-cli@latest docs/presentation/aida-user-walkthrough.md --pdf -o aida-user-walkthrough.pdf
AUDIENCE: developers who will USE AIDA day to day. GOAL: "I know the loop and the commands."
COMPANION DECKS: executive-briefing, developer-deep-dive, administrator-guide.
-->

# AIDA — daily use

### File it · queue it · ship it · keep code tied to intent

<small>v0.11.0 · user walkthrough</small>

<!--
Framing: "AIDA is requirement-first. You don't start by writing code — you start by filing what you're about to do. Everything else hangs off that one habit."
-->

---

## The one habit: requirement-first

Before implementing anything, make sure a spec exists.

```
   file a spec ──► it gets a stable ID ──► code traces back to it ──► merge auto-completes it
```

- The ID is a breadcrumb that follows the work through code, commits, the PR, and history.
- You (and every agent) can later ask *"does this exist?"*, *"why?"*, *"is this code still live?"*

> If you work conversationally and forget, `/aida-capture` at session end reviews what was discussed and files it.

---

## The daily loop

```
aida queue list                 # what's mine to do?
aida queue work <ID>            # pick it up — spawns a session in a fresh worktree
   …implement, with trace comments…
/aida-pr                        # push branch + open PR
/aida-review                    # review against each linked spec's acceptance
gh pr merge --squash            # merge
aida pull                       # auto-bumps the spec Done → Completed
```

Or collapse the middle into one command — the **autonomous drain** (next slides).

---

## Filing a requirement

```bash
aida add --title "User login validation" --type story --status approved \
         --tags "auth,security" --priority high
```

- `--type`: `functional` · `non-functional` · `system` · `user` · `bug` · `epic` ·
  `story` · `task` · `spike` · `sprint` · `folder` · `meta` · `doc`
  *(use `task` for chores / tooling / docs)*
- `--status`: `draft` → `approved` → `planned` → `in-progress` → `done` → `completed`
- Link it: `aida add ... ` then `aida edit <ID>` / `aida db rel-def`, or relationships via the graph.

> Or just say `/aida-req` in Claude Code and describe it in plain language.

---

## Finding things

```bash
aida list                       # cache-backed, sub-ms; hides archived by default
aida list --status draft
aida search "login validation"  # full-text (FTS5)
aida show STORY-249             # details + git linkage (commits / files / branch / PR)
aida graph EPIC-30 --blocked-by # transitive: what's blocking this?
aida graph EPIC-30 --impact     # reverse: what would finishing this unblock?
aida graph EPIC-30 --tree       # epic rollup
```

> The graph queries are the payoff of typed relationships — *"what's blocked by what"* is one command, not a spelunk.

---

## Picking up work

```bash
aida queue work <ID>            # the head if you omit <ID>
aida queue work <ID> --resume   # continue a prior Claude session
aida queue list                 # queued + "Done — awaiting merge" in-flight section
```

- Each pickup runs in an **isolated git worktree** — parallel work never collides.
- The plan (if one exists) **rides in**: `## Critical Files`, `## Followups`, `## Verification`
  are pre-loaded so you get the blast radius + definition of done up front.

> The queue is **yours** (keyed to your shell user). Empty when you expected items? Check `echo $USER` / `echo $AIDA_USER`.

---

## The autonomous drain — `--auto-complete`

One command runs implement → CI → review → merge → pull:

```bash
aida queue work <ID> --auto-complete            # one spec, full lifecycle
aida queue work --batch NAME --auto-complete    # drain a whole batch
aida queue work next3 --auto-complete           # drain 3 from the head
```

**Autonomy is an explicit dial — quality vs throughput:**

- **default** — you're at the keyboard; the agent pauses on design forks.
- **`--zen`** — proceeds on defensible defaults; pauses only on real forks.
- **`--no-human`** — headless; on a fork it can't resolve, it **punts** → a headless
  **advisor** resolves it from recorded principle or **escalates to you**.

<!--
Pick per session: drive known design-fork specs at the keyboard; drain mechanical batches headless. A shelvable failure (CI red, etc.) parks the spec and the batch keeps moving — triage with `aida findings list` (exit code 2).
-->

---

## Keep code tied to intent

**Trace comment** — drop it on the code you write for a spec:

```rust
// trace:STORY-249 | ai:claude
fn validate_login() { ... }
```

**Commit** — the `(SPEC-ID)` trailer closes the loop on merge:

```
[AI:claude] feat(auth): add login validation (STORY-249)
```

- `/aida-commit` makes sure every changed file is linked before you commit.
- Only trailer a spec when **this merge finishes it** — for a partial slice, trailer a child task.

> The trailer is what auto-flips the spec to **Completed** when the PR lands. Spec IDs stay in code/commits — never in user-facing output.

---

## The lifecycle

```
 Draft ─approve─► Approved ─plan─► Planned ─queue work─► In Progress ─/aida-pr─► Done
                                                              │ punt
                                                              ▼
                                                       Needs Attention   (a fork to triage)
 Done ─merge + aida pull (auto-bump)─► Completed ─release─► Released
```

- You rarely set `Completed` by hand — **the merge promotes it.**
- `Needs Attention` is the one off-mainline state: an agent punted a fork it couldn't safely resolve.
- Missed an auto-bump? `aida db reconcile-status` replays it.

---

## Skills: say it in plain language

`aida init` scaffolds slash commands for Claude Code. Daily drivers:

| Skill | What it does |
|---|---|
| `/aida-req` | file a requirement from a description |
| `/aida-implement` | implement a spec with traces |
| `/aida-commit` | commit with the right trailer + links |
| `/aida-capture` | end-of-session: capture specs discussed but not filed |
| `/aida-plan` · `/aida-pickup` | plan a spec · pick up the next queued item |
| `/aida-review` · `/aida-pr` | review against acceptance · open the PR |
| `/aida-search` · `/aida-drain-queue` | unified search · drain your queue |

> Run `aida` (no args) for the full CLI, or `ls .claude/skills/` for the full catalog.

---

## The TUI

```bash
aida tui          # shipped default-on
```

- Hosts your Claude Code session as a child process.
- Drop out to a **status overlay** (queue, drain progress, spec state); drop back into the same conversation.
- Quick-action review / queue / merge / pull; switch between multiple sessions.

> The TUI is the friendly surface; everything in the other decks is what's underneath it.

---

## Cheat sheet

```bash
# capture
aida add --title "…" --type task --status approved
/aida-req                                   # or just describe it

# find
aida list · aida search "…" · aida show <ID> · aida graph <ID> --blocked-by

# do
aida queue work <ID>                        # interactive, isolated worktree
aida queue work <ID> --auto-complete        # autonomous (add --zen / --no-human)

# tie code to intent
// trace:<ID> | ai:claude                   # in code
(<ID>) trailer in the commit               # auto-completes on merge

# sync
aida pull                                    # code + store + auto-bump
```

<small>Why it matters: `aida-executive-briefing`. Under the hood: `aida-developer-deep-dive`. Operating it: `aida-administrator-guide`.</small>
