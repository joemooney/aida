# What you can now do with AIDA

You had a thought. AIDA can carry it all the way to *merged on main* — and let you choose, per piece of work, exactly how much of that trip you make by hand and how much you hand off.

That's the whole pitch. The rest of this page is just showing you the four ways to make the trip.

---

## The one idea: a thought, all the way to merged

Every change goes through the same chain — **implement → CI → review → merge → pull**. The only real question is *who does which links, and how many at once.* AIDA gives you one command for each honest answer:

| You want to… | Run | Who writes the code | How much you watch |
|---|---|---|---|
| Finish **one spec you just coded** | `aida ship <spec>` | **You** | You did the work; AIDA does the paperwork |
| Hand off **one spec, start to finish** | `aida zen <spec>` | **AIDA** | On standby — step in if you like |
| Walk away from a **whole batch** | `aida burndown run` | **AIDA**, in parallel | Goodnight. Read the results over coffee |
| Keep a **queue draining forever** | `aida queue integrate --watch` | (already done) | It merges finished work as it lands |

Read it as a grid: **one spec → a set → a never-ending queue** across the top, **your hands → fully off** down the side. Pick the cell that matches your mood today.

### `aida ship` — you implemented it; let AIDA do the rest

You're in a worktree. You wrote the code. You're *done thinking* — but there's still the tedious part: commit it with the right trailer, rebase onto the latest main, push, open the PR, babysit CI, squash-merge, pull, tear the worktree down.

```
$ aida ship
```

One command, the whole closing ceremony. It figures out which spec you're on from your branch. Want to stop early? `--no-merge` opens the PR and leaves it for a human; `--no-pr` just rebases and pushes. `--dry-run` shows you the plan first.

This is the everyday workhorse: **you keep the fun part (writing the code), AIDA takes the chores.**

### `aida zen` — hand off the whole thing

Some work you'd rather just *describe* and get back finished. `aida zen` takes an approved spec and drives the entire chain itself — it queues it, implements it, waits for CI, reviews, merges, and pulls.

```
$ aida zen <spec>
```

By default the advisor rides along on standby, so you can glance over and step in. When you genuinely want to walk away — *tell your agents goodnight* — add the headless mode and it needs nothing further from you:

```
$ aida zen <spec> --no-human=both
```

Got five independent specs? Launch five, each in its own worktree, and let them run in parallel. Fire-and-forget, by design.

### `aida burndown run` — drain a whole set while you sleep

`zen` is one spec. `burndown run` is *the backlog*. It takes your advisor-blessed ready set, fans out parallel implementers, integrates each PR as it goes, and loops until there's nothing left to do.

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

Anyone can string `git` commands behind one alias. What makes the hand-off *trustworthy* is what's underneath: AIDA isn't driving loose prompts — it's driving your **requirement backlog**.

- **Stable IDs.** Every piece of work has a durable identifier that doesn't shift as the backlog churns. The autonomous drive always knows *which* thing it's shipping.
- **Code ↔ spec traces.** Inline `trace:` breadcrumbs tie the code back to the spec that asked for it, so "why does this exist?" always has an answer.
- **A real lifecycle.** Draft → Approved → In Progress → Done → Completed isn't decoration; it's the substrate the autonomy reads. "Done" means *finished on a branch*; "Completed" means *merged to main* — and the merge promotes it for you.

That's the difference between *automation* and *coordination*. The work is traceable, ordered, and safe to hand off — not a pile of ad-hoc prompts you have to hold in your head.

---

## Your first autonomous drive (about 2 minutes)

Try the whole loop on something tiny and real.

**1. File a thought.** Anything small and safe.

```
$ aida add --title "Add a --version short flag" --type task --status approved
```

(It needs to be *approved* — that's your "yes, build this" signal. AIDA refuses to autonomously ship a draft.)

**2. Hand it the keys.** Use the SPEC-ID it just printed back:

```
$ aida zen <spec> --no-human=both
```

It queues the spec, spins up an implementer in an isolated worktree, waits for CI, reviews, merges the PR, and pulls — promoting the spec to **Completed** the moment the merge lands on main.

**3. Watch it land.**

```
$ aida burndown status     # what's running right now
$ aida list --status completed   # there it is, on main
```

That's it. You described a change in one line and the next time you looked, it was merged. Now imagine pointing that at a backlog.

---

## Which command when

| Situation | Reach for |
|---|---|
| "I just finished coding this — close it out." | `aida ship` |
| "Open the PR but let a human merge." | `aida ship --no-merge` |
| "Build this one for me, I'll keep half an eye on it." | `aida zen <spec>` |
| "Build this one and don't ask me anything." | `aida zen <spec> --no-human=both` |
| "Drain the whole ready backlog overnight." | `aida burndown run` |
| "Keep merging finished work onto main as it shows up." | `aida queue integrate --watch` |
| "Wait — what's actually running?" | `aida burndown status` · `aida ps` |

Start at the top, where your hands are on everything. Slide down the list as the rhythm gets familiar and the trust gets earned. The power was always there — now it has a name you can type.
