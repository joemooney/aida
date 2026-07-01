# What you can now do with AIDA

You have a thought. You type it. The next time you look, it's *merged on main* — implemented, reviewed, and traceable back to the words you started with.

```
$ aida zen "let me filter requirements by a date range"
```

That's it. That's the headline. One plain-English sentence in, one gated-and-reviewed merge out. No spec to write first, no ceremony to learn. You describe what you want; AIDA does the rest of the trip.

The rest of this page shows you that front door — and the three other ways to make the same trip when you want your hands more on the wheel.

---

## The front door: tell AIDA a thought

Most tools make you translate your idea into *their* format before anything happens. AIDA skips that. Hand `aida zen` a sentence in plain words and it:

1. **Drafts a real spec from your thought** — an AI-written title, description, and acceptance criteria, so the idea becomes something durable instead of a throwaway prompt.
2. **Files it as a Draft and runs it past the approve-gate** — your advisor sees it, or it auto-approves within the limits you set. A careless midnight thought can't silently ship itself.
3. **Drives it all the way to merged** — queue, implement, CI, independent review, merge, pull — and bumps it to Completed the moment it lands on main.

```
$ aida zen "warn me when two specs claim the same file"
```

A raw thought becomes gated, reviewed, merged code — in one line.

**Want to look before you leap?** `--dry-run` renders the actual spec it *would* draft from your thought — AI-written title, description, and acceptance criteria — without filing anything:

```
$ aida zen "warn me when two specs claim the same file" --dry-run

▸ would draft + file + drive a new Draft from your thought:

  ◯ Warn when two specs claim the same file
  ▸ Description:
    ...
  ▸ Acceptance (N items):
    • ...
  ✓ AI-drafted from your thought.
```

**Prefer to skip the AI draft?** Set `AIDA_ZEN_NO_AI=1` and your sentence becomes the spec title verbatim — a fast, offline fallback when you'd rather word it yourself.

This is the *"tell your agents a thought"* front door: the lowest-friction way in, with none of the safety quietly removed underneath.

---

## The same trip, four ways

Every change — typed thought or hand-written code — goes through the same chain: **implement → CI → review → merge → pull**. The only real question is *who does which links, and how many at once.* AIDA gives you one command for each honest answer:

| You want to… | Run | Who writes the code | How much you watch |
|---|---|---|---|
| Ship **a thought**, start to finish | `aida zen "<your idea>"` | **AIDA** (drafts the spec too) | Nothing — headless all the way to main |
| Hand off **one spec you've already filed** | `aida zen <spec>` | **AIDA** | Nothing — headless all the way to main |
| Finish **one spec you just coded** | `aida ship <spec>` | **You** | You did the work; AIDA does the paperwork |
| Walk away from a **whole batch** | `aida burndown run` | **AIDA**, in parallel | Goodnight. Read the results over coffee |
| Keep a **queue draining forever** | `aida queue integrate --watch` | (already done) | It merges finished work as it lands |

Pick the row that matches your mood today. The top one needs nothing but a sentence; the rest let you keep more of the trip in your own hands.

### `aida zen` — from a thought, or from a spec

You've already met the headline use: pass free text and AIDA drafts the spec for you. But `zen` takes a spec id just as happily — when the idea is already filed and approved, point it straight at the work:

```
$ aida zen <spec>
```

Either way, **it's fully headless by default** — *tell your agents goodnight.* The implementer runs headless, an independent reviewer always runs as its own gate before the merge, and the change drives all the way to main with nobody in the loop.

Want to keep your hands on the keyboard for the actual coding? `--supervised` lets **you** drive the implementer interactively, while the reviewer and merge stay autonomous behind you:

```
$ aida zen <spec> --supervised
```

Got several independent ideas? Fire several `zen`s, each in its own worktree, and let them run in parallel. Fire-and-forget, by design.

### `aida ship` — you implemented it; let AIDA do the rest

Sometimes you *want* to write the code yourself. You're in a worktree, you're done thinking — but there's still the tedious part: commit it with the right trailer, rebase onto the latest main, push, open the PR, babysit CI, squash-merge, pull, tear the worktree down.

```
$ aida ship
```

One command, the whole closing ceremony. It figures out which spec you're on from your branch. Want to stop early? `--no-merge` opens the PR and leaves it for a human; `--no-pr` just rebases and pushes. `--dry-run` shows you the plan first.

This is the everyday workhorse for hand-written work: **you keep the fun part (writing the code), AIDA takes the chores.**

### `aida burndown run` — drain a whole set while you sleep

`zen` is one idea. `burndown run` is *the backlog*. It takes your advisor-blessed ready set, fans out parallel implementers, integrates each PR as it goes, and loops until there's nothing left to do.

```
$ aida burndown run            # drain the blessed set; --dry-run to preview first
$ aida burndown status         # is a drain running, and what's it doing?
```

Only queued, pickable, unblocked, decision-free work is eligible — so it never guesses on something that needed your judgment. You bless the set; it burns it down.

### `aida queue integrate --watch` — the tap that never closes

The other three *produce* finished work. This one *consumes* it. Point it at your project and it watches for any spec that's Done-with-an-open-PR, then merges them onto main one at a time, in order — forever, or until you stop it.

```
$ aida queue integrate --watch
```

Your implementers (you, agents, teammates) ship in parallel and never fight over main. The integrator is the single, calm, serial merge authority. Producers produce; this consumes. No coordination meeting required.

---

## Why this is more than a clever wrapper

Anyone can string `git` commands behind one alias. Anyone can pipe a sentence into a code model. What makes the hand-off *trustworthy* is what's underneath: AIDA isn't driving loose prompts — it turns your thought into a tracked requirement and drives *that*.

- **Stable IDs.** The moment your sentence becomes a spec, it gets a durable identifier that doesn't shift as the backlog churns. The autonomous drive always knows *which* thing it's shipping.
- **Code ↔ spec traces.** Inline `trace:` breadcrumbs tie the shipped code back to the thought that asked for it, so "why does this exist?" always has an answer — even for a one-liner you barely remember typing.
- **The approve-gate.** A free-text thought lands as a *Draft* and passes the approve-gate before anything builds. The convenience of a single sentence, without the danger of a careless one auto-shipping.
- **A real lifecycle.** Draft → Approved → In Progress → Done → Completed isn't decoration; it's the substrate the autonomy reads. "Done" means *finished on a branch*; "Completed" means *merged to main* — and the merge promotes it for you.

That's the difference between *automation* and *coordination*. Your thought becomes traceable, ordered, and safe to hand off — not a prompt you have to hold in your head.

---

## Your first autonomous drive (about 2 minutes)

Try the whole loop on something tiny and real.

**1. Tell AIDA what you want.** One plain sentence — anything small and safe:

```
$ aida zen "add a short -V flag as an alias for --version"
```

That single line drafts a spec from your words, files it, runs it past the approve-gate, then queues it, spins up an implementer in an isolated worktree, waits for CI, reviews, merges the PR, and pulls — promoting the spec to **Completed** the moment the merge lands on main.

(Curious before committing? Add `--dry-run` to see what it *would* draft and drive without filing a thing.)

**2. Watch it land.**

```
$ aida burndown status     # what's running right now
$ aida list --status completed   # there it is, on main
```

That's it. You described a change in one line and the next time you looked, it was merged. Now imagine pointing that at a backlog.

---

## Which command when

| Situation | Reach for |
|---|---|
| "I have an idea — just build it." | `aida zen "<your idea>"` |
| "Show me what that thought would become first." | `aida zen "<your idea>" --dry-run` |
| "Build this spec I already filed, don't ask me anything." | `aida zen <spec>` |
| "Build it, but let *me* write the code." | `aida zen <spec> --supervised` |
| "I just finished coding this — close it out." | `aida ship` |
| "Open the PR but let a human merge." | `aida ship --no-merge` |
| "Drain the whole ready backlog overnight." | `aida burndown run` |
| "Keep merging finished work onto main as it shows up." | `aida queue integrate --watch` |
| "Wait — what's actually running?" | `aida burndown status` · `aida ps` |

Start at the top, where a sentence is all it takes. Slide down the list as the rhythm gets familiar and you want more of the trip in your own hands. The power was always there — now the front door is a single thought you can type.

---

**What changed lately?** See [`whats-new-2026-06.md`](whats-new-2026-06.md) for the June 2026 round of refinements — organized by who benefits (human, agent, cockpit).
