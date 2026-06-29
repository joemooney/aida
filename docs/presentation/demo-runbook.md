# AIDA management demo — live runbook

**Audience:** technical leadership · **Goal:** buy-in / resources · **Format:** live demo, slides scaffold.
**Pairs with:** `docs/presentation/2026-06-management-demo.md` (the deck).
**Total time:** ~15 min (8 min live, rest slides + Q&A). Rehearse once end-to-end before the room.

---

## 0. Pre-flight (do this the morning of)

```bash
# 1. Fresh numbers for the closing slide
aida list --all | tail -1                 # total specs
aida list --status completed --all | grep -cE '^[A-Z]+-'   # completed
git tag --sort=-v:refname | head -1       # latest release
# → update slide "Where we are" if they've moved.

# 2. Render the deck
npx @marp-team/marp-cli@latest docs/presentation/2026-06-management-demo.md -o /tmp/aida-demo.html
# (or --pdf for a portable fallback copy on a USB stick)

# 3. Record the backup cast (insurance — see §6). Do this even for a live demo.

# 4. Demo environment check:
aida --version                            # confirm dev-activated / release build active
gh auth status                            # demo repo creation needs gh
echo $AIDA_USER $USER                     # queue identity sanity (BUG-89)
```

**Golden rule:** if anything stalls live for >15 seconds, **cut to the recorded cast (§6) and keep talking.** Never debug in front of the room.

---

## 1. LIVE — file a spec (slide "idea → merged")

Use a **throwaway repo**, not the real AIDA repo, so the room sees the full first-run story and nothing real is at stake. The bundled demo script builds one:

```bash
bash scripts/aida-demo.sh        # interactive: Enter-to-continue between sections
```

It creates a timestamped throwaway GitHub repo, clones it, runs `aida init`, and walks: file a spec → implement → commit with the `(SPEC-ID)` trailer → `aida pull` auto-bump.

**If you want a tighter, hand-driven version** (more control, no GH repo creation), in a pre-initialized scratch project:

```bash
aida add --title "Add a /health endpoint" --type task --status approved
# note the returned ID, e.g. TASK-1
aida show TASK-1                 # show the stable ID, status, type
```

**Say:** *"It gets a stable ID. That ID will follow it through the code, the commit, and the PR — and back."*

---

## 2. LIVE — drain it (the centerpiece)

```bash
aida queue work TASK-1 --auto-complete --zen
```

**Narrate while it runs** (don't go silent):
- *"Implementer works in an isolated git worktree — not your tree."*
- *"CI runs remotely."*
- *"A reviewer agent reads the **code**, not the commit message, and votes."*
- *"It merges. I haven't touched the keyboard."*

Then, the payoff:

```bash
aida show TASK-1                 # status: Completed · linked PR · linked commit
git log --oneline -1             # the (TASK-1) trailer
grep -rn "trace:TASK-1" .        # the trace comment that landed in the code
```

**Killer line:** *"I didn't merge that. The orchestrator did — and the requirement knows it's done. Nobody updated a status field by hand."*

> ⏱ A real drain can take minutes. **Decide in advance:** either (a) pre-warm a drain so it completes during the talk, or (b) use the recorded cast (§6) for this segment and keep the live filing from §1. For a 15-min slot, (b) is safer.

---

## 3. LIVE — the graph inside the agent (slide "graph inside the agent")

In a Claude Code session **in the AIDA repo** (MCP server wired via `.mcp.json`):

**Ask Claude, out loud, a real graph question:**
> "Using AIDA, what's blocking EPIC-30, and what would land if STORY-489 completed?"

Let Claude call `query_graph` / `show_requirement` live and answer *from the graph*.

Fallback question if EPIC-30 has no blockers: *"Show me every spec that traces to `git_backend.rs` and whether each is still live."*

**Killer line:** *"It's not grepping a markdown file. It's querying a typed graph. Floor vs. moat."*

---

## 4. Slides — moat + the ask (back to deck)

Return to the deck for "Why not 20 lines of bash?" and "Where we are + the ask."
**Before the room:** fill the ask slide's `<resources>` placeholder with the concrete request (headcount / time / priority). Buy-in needs a number.

---

## 5. Q&A — likely questions + crisp answers

- **"Couldn't Anthropic/GitHub just add this?"** → They'd have to ship the YAML-canonical store, node-aware IDs, the cache model, the MCP server, the trace convention, the graph, the role/session/worktree model, and the lifecycle engine. Months. And ours lives in **git** → vendor-neutral by construction, which a single-vendor tool structurally can't match.
- **"Is this just Jira with extra steps?"** → Jira is a SaaS system of record for humans. This is a **git-canonical graph for agents** — queryable over MCP, with code→spec traces and an autonomous lifecycle. No SaaS, no lock-in.
- **"What's the catch / what's not done?"** → We're in a deliberate **stabilization phase** — clearing the bug backlog before public launch. Honest answer; it reads as discipline.
- **"How much is AI-generated?"** → Most of it, traceably — commits carry `[AI:tool]` tags and `trace:` comments. The system is its own proof.

---

## 6. Backup cast (insurance — record during pre-flight)

```bash
# Record the drain segment so a live stall never derails the talk:
aida --asciinema --cast-title "AIDA drain demo" queue work <ID> --auto-complete --zen
# Cast lands under .aida/casts/ (or ~/.aida/casts/). Play with:
asciinema play <cast-file>
```

Keep the cast open in a second terminal/tab. If §2 stalls, switch to it and narrate over the playback. The audience cannot tell the difference, and you stay in control.

---

## 7. One-line fallbacks cheat-sheet

| If… | Do this |
|---|---|
| live drain stalls | switch to recorded cast (§6), keep narrating |
| MCP/Claude is slow | read a pre-captured `query_graph` result from a slide/notes |
| `gh` auth expired | skip `aida-demo.sh`, use the hand-driven §1 path in a scratch repo |
| projector dies | the `--pdf` render on a USB stick |
| out of time | the loop (§1–2) + the moat slide is the irreducible core — cut graph + backups |
