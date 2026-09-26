# Feature lanes: working on the side without touching main

**Verified against**: `main` at commit `8238ac82` (`aida worktree --help`,
`aida-cli-lib/src/worktree.rs`, `aida-cli-lib/src/focus.rs`,
`docs/lifecycle.md`, `docs/autonomous-drain.md`).
**Status**: living explanation. Everything under "Planned: feature lanes" is
**not shipped**. Update this page when STORY-1481, STORY-1482 or STORY-1483
lands.

## The question

> "I want to work on some feature on the side, and I'd like a role or seat
> targeted at that feature, so I know I'm not interfering with the main
> branch."

This page answers it in two parts: what AIDA does today, and what a planned
feature called a *feature lane* would add. The two are kept in separate
sections so the planned part is not read as available.

A few terms used below:

- **Spec**: one requirement in AIDA (a story, task, bug, and so on), with an
  ID such as `STORY-1481`.
- **Epic**: a spec that groups other specs under it as its children.
- **Main line**: the `main` branch, and the AIDA tools that deliver work to
  it.
- **Drain**: an unattended run that picks specs off a work queue, implements
  them, and merges the results to `main` (see
  [autonomous-drain.md](../autonomous-drain.md)).
- **Seat**: the role an agent or person is working in, such as implementer
  or advisor. The **advisor** is the reviewing seat.
- **Focus**: a per-checkout setting that limits AIDA's read commands to one
  epic and the specs under it.

## Shipped: what AIDA does today

`aida worktree enter EPIC-N` creates a separate checkout of the repository
(a git *worktree*) next to the main one and moves your shell into it. It
does the following:

- It cuts a new branch off `origin/main`, named `epic-n-work` by default
  (for example `epic-74-work`). `--branch` picks a different name.
- It places the checkout at `~/ai/aida-<slug>` by default (for example
  `~/ai/aida-epic74`). `--path` picks a different location.
- It sets the checkout's focus to the epic. Inside that checkout,
  `aida list`, `aida status` and `aida queue list` show only the epic and the
  specs under it, and each scoped output says so in a header.
- It adds a `(wt:EPIC-N)` marker to your shell prompt, so you can see which
  checkout you are in.
- If it runs again for the same epic, it reuses the existing checkout rather
  than creating a second one.

`aida worktree exit` takes you back to the main checkout and leaves the side
checkout in place, so you can enter it again later. `aida worktree list`
shows the checkouts AIDA manages and the focus of each.

Your commits in the side checkout land on the side branch. Nothing reaches
`main` until someone opens a pull request and it merges.

Inside the side checkout there is also a guard on starting work outside the
focus. When you run `aida queue work` on a spec that is not under the epic,
AIDA prints a warning by default. Setting `out_of_scope = "block"` under
`[focus]` in `.aida/config.toml` makes it refuse instead, unless you pass
`--force`.

That covers most of the day-to-day need. The limits are in the next section.

## Where today's setup falls short

The isolation holds only while everyone remembers the arrangement. AIDA does
not enforce it.

1. **The scoping works in one direction only.** Inside the side checkout you
   see only the feature. The main-line tools (drains, `aida integrate`,
   grooming, and the shared queue) still see every spec, including the
   feature's. If one of the feature's specs is approved and queued, a
   main-line drain treats it like any other work and ships it to `main`.
2. **There is no status for "merged into the side branch".** Pull requests
   opened by AIDA's own tools target `main`. In AIDA, *Completed* means
   "merged to the default branch" (see [lifecycle.md](../lifecycle.md)).
   Work merged into the side branch can only sit in *Done*, which normally
   means "finished and waiting to reach main". Nothing records that the spec
   is actually waiting on a side branch.
3. **Side branches fall behind `main`.** EPIC-72 is an example. It was built
   on a long-running branch (`feat/jev-integration`), and bringing it home
   took a separate task, TASK-1470, to copy the relevant changes onto a fresh
   branch cut from current `main`. The longer a side branch runs without
   catching up, the more work it takes to land.

## Shipped: how to work on the side today

Until feature lanes exist, this is the working practice. Each step uses a
command or setting that exists on `main` today.

1. **Make the side checkout.** Run `aida worktree enter EPIC-N`.
2. **Keep the feature's specs away from main-line drains.** Drains that take
   the head of a queue only pick specs that have been added to a queue (with
   `aida queue add`). Batch drains pick specs tagged `batch:<name>`, and the
   scheduled shift only picks specs whose execution mode is `drain`. So keep
   the epic's children in Draft, or approved but not added to any queue, not
   tagged with a batch, and not in `drain` execution mode.
3. **Target the side branch by hand.** AIDA's pull-request tools target
   `main`. In particular, `aida ship` rebases onto `origin/main` and merges
   into `main`, and `aida pr ship`, when it has to create the pull request,
   creates it against the default branch. Do not use them to open the
   feature's pull requests. Open them against `epic-n-work` yourself, for
   example `gh pr create --base epic-n-work`.
4. **Catch up with main often.** Merge `main` into the side branch regularly,
   for example `git fetch origin && git merge origin/main` inside the side
   checkout, rather than once at the end.
5. **Land deliberately.** When the feature is ready, open one pull request
   from `epic-n-work` into `main`. `aida pull` moves a spec from Done to
   Completed when a commit that names it, such as one ending in
   `(STORY-1481)`, reaches `main` (see
   [commit-trailer-convention.md](../commit-trailer-convention.md)). If the
   landing pull request is squash-merged, make sure the squash commit message
   still names every spec the side branch delivered.

## Planned: feature lanes (not shipped)

> **Not shipped.** Nothing in this section exists on `main` yet. There is no
> `aida lane` command, no lane record and no "Done (in lane)" status. The
> design below comes from EPIC-74 and its three stories. It needs a design
> sketch and advisor sign-off, recorded as an architecture decision, before
> implementation starts, and the details may change during that review.

A *lane* is a side feature that AIDA knows about and enforces, bound to one
epic. The plan is delivered in three slices, each owned by one spec.

### Slice 1 (planned, STORY-1481): the lane record, the lane seat, and main-line exclusion

This slice alone delivers "I cannot interfere with main", so it comes first.

- `aida lane open EPIC-N` would record on the epic that it lives in a lane,
  and which branch the lane uses (`epic-n-work` by default). The record would
  live in the shared requirements store, so every machine and every agent
  sees it after `aida pull`.
- The session working in the lane would get a *lane seat*: its focus, target
  branch and statusline label (`lane:EPIC-N`) would come from that shared
  record, not from settings in one shell or one checkout.
- Main-line tools would skip every spec under a laned epic, and say so.
  Drains, `aida integrate`, grooming and the main queue views would each
  print one line, such as `EPIC-N: 4 specs in a feature lane (excluded)`, so
  nothing disappears without notice.
- Pointing a main-line command directly at a lane spec would be refused,
  with a message naming the lane and how to enter it. `--force` would
  override the refusal.
- `aida lane list` would show open lanes, their branches, how many specs
  each holds and how far each is behind `main`. `aida lane close EPIC-N`
  would remove a lane without landing it, and its specs would rejoin the
  main line.

### Slice 2 (planned, STORY-1482): pull requests target the lane, and Done (in lane)

- Inside a lane, every AIDA path that opens or merges a pull request (drains,
  `aida pr`, `aida ship` and the review and merge steps) would target the lane
  branch instead of `main`.
- When a pull request merges into the lane branch, its spec would move to
  **Done (in lane EPIC-N)**, not Completed. `aida pull` would not promote
  these specs to Completed while the lane is open. Completed would keep its
  meaning of "on main".
- `aida show` and `aida status` would display these specs as
  "Done (in lane EPIC-N)".

### Slice 3 (planned, STORY-1483): keeping up with main, and landing the lane

- `aida lane sync` would bring `main`'s changes into the lane branch. It
  would merge by default, which is safe when several people share the
  branch, and offer `--rebase` for a lane only one person uses. A conflict
  would stop the command with guidance; nothing would be resolved
  automatically.
- AIDA would show how many commits the lane is behind `main`, in
  `aida status` inside the lane and in `aida lane list`, and warn above a
  configurable threshold, so conflicts show up while they are still small.
- `aida lane land EPIC-N` would open a single pull request from the lane into
  `main`. It would go through normal advisor review and CI, and the
  implementer could not merge it themselves. When it merges, every
  Done-in-lane spec would become Completed and the lane would close. If the
  pull request is closed without merging, the lane would stay open and its
  specs would stay Done-in-lane.

## Shipped versus planned at a glance

| Need | Today (shipped) | With lanes (planned) |
|---|---|---|
| Separate checkout and branch | `aida worktree enter EPIC-N` | Same, via `aida lane open EPIC-N` |
| Read commands scoped to the feature | Yes, inside the side checkout | Yes, from the shared lane record |
| Main-line drains skip the feature | No; keep specs unqueued by hand | Yes, with a visible exclusion line (STORY-1481) |
| Pull requests target the side branch | By hand (`gh pr create --base ...`) | Automatic inside the lane (STORY-1482) |
| Status for "merged into the side branch" | None; the spec sits in Done | Done (in lane EPIC-N) (STORY-1482) |
| Catching up with main | `git merge origin/main` by hand | `aida lane sync` and a drift warning (STORY-1483) |
| Landing | One pull request opened by hand | `aida lane land EPIC-N`, reviewed (STORY-1483) |

## See also

- [session-lifecycle.md](../session-lifecycle.md): sessions, worktrees and
  leases.
- [lifecycle.md](../lifecycle.md): what Done and Completed mean.
- [autonomous-drain.md](../autonomous-drain.md): how drains choose work.
- `aida show EPIC-74`: the feature-lanes epic and its current status.
