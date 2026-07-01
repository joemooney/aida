# What's new in AIDA — June 2026

A self-improvement loop ran for a few weeks and shipped a stack of changes. None of them are big new subsystems; almost all of them are *the moment you actually touch AIDA feeling better* — the spec preview that reads your mind, the footer that tells you the next move, the cockpit row that glows when something's live.

This page is organized by **who benefits**, not by what shipped. Read the section that's you.

- [For the human at the keyboard](#for-the-human-at-the-keyboard) — the first five minutes, and the small delights after
- [For the agent on the other end](#for-the-agent-on-the-other-end) — leaner output, no silent no-ops, always a next step
- [For the cockpit](#for-the-cockpit-aida-tui) — `aida tui` now shows liveness, depth, and reasons at a glance
- [Quietly faster, quietly tougher](#quietly-faster-quietly-tougher) — perf and overnight-drive reliability

---

## For the human at the keyboard

### See the spec a thought *would* become — before you commit to anything

`aida zen "<a sentence>"` turns a plain thought into a fully-driven, merged change. Now `--dry-run` doesn't just say "I'd do something" — it renders the actual spec it would draft from your words: the AI-written title, the description, and the full acceptance list.

```
$ aida zen "let me filter requirements by a date range" --dry-run

▸ would draft + file + drive a new Draft from your thought:

  ◯ Filter requirements by a date range

  ▸ Description:
    Add the ability to restrict requirement listings to those falling
    within a user-specified date range...

  ▸ Acceptance (5 items):
    • Listing requirements with a start and end date returns only
      requirements whose date falls on or between those two dates...
    • A range whose start is later than its end is rejected...
    ...
  ✓ AI-drafted from your thought.

  …then: approve ▸ implement ▸ CI ▸ review ▸ merge, fully headless.
  Run without --dry-run to drive it.
```

**Why it's better:** the "wait — it just *knew* what I meant" moment now happens *before* you spend a single CI minute. You see the machine's reading of your idea, agree or reword, then drive it for real.

### Every command tells you the next move

File a spec and you no longer stare at a bare confirmation line wondering what to type next. `aida add` closes with a plain-English `Next:` block and a trace breadcrumb — guidance that used to be reserved for agents, now offered to humans too.

```
$ aida add --type task --title "Add a date-range filter"
Added: TASK-2 - Add a date-range filter

Next:
  ▸ aida edit TASK-2 --status approved   approve it
  ▸ aida edit TASK-2 --status rejected   reject it
  ↳ Link your code to it: add a // trace:TASK-2 comment where you implement it.
```

**Why it's better:** the path forward is on the screen. No second guess, no `--help` detour.

### `aida history` shows *your* work, not the plumbing

Every project is seeded with a handful of META requirements (the editable AI prompts). They used to crowd the top of `aida history` on a fresh project — six rows of scaffolding before your first real spec. Now `aida history` hides them by default.

```
$ aida history               # your specs, clean
$ aida history --include-meta # the scaffolding too, when you want it
```

**Why it's better:** day one, `aida history` is about what *you* did — not what `aida init` did for you.

### An empty queue is an invitation, not an error

Run `aida queue work` before you've queued anything and you get a calm signpost instead of a red failure.

```
$ aida queue work
ℹ Nothing queued yet — that's the expected day-one state. Approve a draft
  and queue it in one step: `aida add "<what you're building>" --queue`,
  or queue an existing spec: `aida queue add <id>`.
```

Exit code `0` — because nothing went wrong. **Why it's better:** an empty queue on a new project is the *normal* state, and the tool finally treats it that way.

### Closing the loop *feels* like closing the loop

When a spec reaches Completed — merged on main — AIDA marks the moment instead of printing a flat "updated" line.

```
$ aida edit TASK-2 --status completed
✓ TASK-2 reached Completed — the loop closed.
  Add a date-range filter
  ↳ filed ▸ built ▸ merged ▸ completed
  ↳ aida show TASK-2  to see the commit that landed it.
```

**Why it's better:** the whole point of AIDA is that a thought travels filed ▸ built ▸ merged ▸ completed. When it arrives, you should *feel* it. (This is the human surface only — agent/TOON output stays terse.)

### `aida pull` warns when your binary is stale

If you build AIDA from source (`aida dev activate`), pulling new commits used to leave you silently running yesterday's binary until you remembered to rebuild. Now `aida pull` notices and nudges you.

```
$ aida pull
...
  ⚠ your aida binary is now behind HEAD — run `cargo build` to pick up the
    pulled changes.
```

**Why it's better:** no more "I fixed that already — why is it still broken?" The warning only fires for dev-activated in-repo builds (a released binary on `PATH` is *expected* to differ, so it stays quiet).

---

## For the agent on the other end

### Lean TOON output across `list`, `search`, and `graph`

In agent mode (`AIDA_AGENT_OUTPUT=1`, or any non-TTY pipe), the query surfaces now emit compact, header-once TOON instead of a wide human table — and that includes `--fields`-selected columns, the default `search`, and `graph`.

```
$ AIDA_AGENT_OUTPUT=1 aida search "date filter"
count: 2 results
specs[2]{id,title,status,type}:
  TASK-1,Add a date-range filter,completed,task
  TASK-2,Date filter UI,in-progress,task
next[1]{cmd,to}:
  aida show <id>,detail
```

**Why it's better:** fewer tokens for the same rows. The column header is declared once (`specs[2]{...}`), the rows are bare CSV, and a `next[]` block tells the agent how to drill in — so an agent spends its budget on the work, not on parsing tables.

### `aida queue done` never silently does nothing

Run in a non-TTY (an agent, a script, a drain), `queue done` used to hit a confirmation prompt, read EOF, and quietly cancel — the spec looked "still queued" for no visible reason. Now it auto-confirms when there's no human at the keyboard, and the gated-write guard errors put the override flag (`--force`) on the *first* line, where an agent's error summary will actually see it.

```
$ AIDA_AGENT_OUTPUT=1 aida queue done TASK-3
✓ TASK-3 marked done and removed from queue.
  (run `aida queue next` to see what's next)
next[2]{cmd,to}:
  aida pull,completed
  aida show TASK-3,detail
```

**Why it's better:** an agent's "I marked it done" is now *true*. No silent no-op, no invisible escape hatch.

### A `next[]` step on every chain verb — including `queue done → aida pull`

The chain verbs end with machine-readable guidance for the next link. Finishing work with `queue done` now points explicitly at `aida pull` — the step that auto-bumps the spec from Done to Completed once the merge lands.

**Why it's better:** an agent can follow the whole filed ▸ done ▸ completed chain without a human spelling out each hop. The nudge to `aida pull` (not raw `git pull`) is the one that promotes the spec.

---

## For the cockpit (`aida tui`)

### Every row says whether something's live

The cockpit's targets list now carries a per-row liveness glyph, fed by `aida ps`, so you can see at a glance what's actually running.

| Glyph | Means |
|---|---|
| `●` (green) | **live** — a session is genuinely working this spec right now |
| `⚠` (amber) | **stale** — a lease or In-Progress flag with no live process behind it |
| `◦` (dim)   | **idle** — nothing running |

**Why it's better:** "is anyone working on this?" is now an ambient property of the row, not a separate `aida ps` you have to remember to run. A stale `⚠` row is a leaked lease asking to be cleaned up.

### The preview reveals the relationship graph

Open a spec's preview in the cockpit and it now surfaces the spec's immediate graph — its parent epic(s), its children, what it's blocked by, and what it blocks — resolved from the requirement relationships.

**Why it's better:** this is the Trojan-horse depth made visible. The TUI looks like a thin list; dig into one row and the graph that's been there all along comes up to meet you. You see *where this spec sits* without leaving the cockpit for `aida graph`.

### The backlog verbs work, and parked specs explain themselves

The two cockpit verbs that used to be greyed-out stubs are now wired:

- **`groom`** runs the advisor disposition pass in its *safe, propose-only* mode — it reads the backlog and shows you the approve / reject / park / queue plan in a modal, and writes nothing until you act on it.
- **`archive`** marks the specs you've selected as archived, one store write per target.

Both stay advisor-gated. And in the advisor-backlog panel, every **parked** spec now states *why* it's parked inline — its revisit trigger, punt note, or finding — with the advisor's pending-queue depth shown in the panel title.

**Why it's better:** the cockpit can now triage, not just observe. And a parked item no longer sits there mute; it tells you the condition that will bring it back.

---

## Quietly faster, quietly tougher

Not headline features — but the kind of thing you feel as "huh, that used to take longer" or "the overnight run actually finished."

- **`aida status --full` is dramatically faster.** The git fan-out that dominated it (~22s on a busy repo) was collapsed and parallelized, and the non-git floor (the `/proc` liveness probe + a full backend load) was trimmed. The rich snapshot now returns in seconds.
- **Two new housekeeping verbs for a long-lived store.** `aida queue gc` sweeps dead routed queue entries whose backing spec is archived/completed/rejected — the corpses that linger in the queue file after the work shipped. `aida store compact` (alias `aida store gc`) deep-repacks the orphan-store git repo to relieve the substrate tax of a long, never-compacted history — safe and non-destructive; no history rewrite, `aida history` unaffected.
- **The unattended drive is harder to derail.** Overnight `aida burndown` / drive runs gained sleep-prevention (the machine won't doze mid-drain), exponential backoff on transient failures, an end-of-run exit summary, and preserve-on-fail so a wedged run leaves its evidence behind instead of cleaning it away.
- **`aida ps` tells the truth about fan-out work.** A pooled worktree reused for a new spec no longer carries the old lease, and specs being worked by a fanned-out implementer are no longer falsely alarmed as "orphaned." The running-work table now reflects what's genuinely live.

---

## Where to go next

- New to the autonomy commands? [`aida-power-features.md`](aida-power-features.md) is the friendly tour of `aida zen` / `aida ship` / `aida burndown run` / `aida queue integrate --watch` — the thought-to-merged front door.
- Want the full lifecycle vocabulary? [`lifecycle.md`](lifecycle.md).
- Driving unattended drains? [`autonomous-drain.md`](autonomous-drain.md).
