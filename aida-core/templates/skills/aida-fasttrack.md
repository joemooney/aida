---
name: aida-fasttrack
description: "The fast lane for small work — two named tiers on one pipe. TRIVIAL (cosmetic/doc/one-line): CI-only, review skipped. EXPRESS (easy bug / small feature): full CI + reviewer, fast because reliably routed. An eligibility litmus picks the tier; a punt-out invariant keeps it honest."
---
# Fasttrack a small change — two tiers on one lane

The fast lane is not one thing — it is **two named tiers on a single pipe**, told
apart by exactly one question: *does it skip the human-review ceremony?*

- **TRIVIAL** (existing) — cosmetic, doc-only, or one obvious line. Tagged
  `batch:fasttrack` + `lifecycle:no-review`; **CI is the only gate**. Fast
  because it is genuinely too small to review. trace:STORY-587
- **EXPRESS** (quick-fix) — an easy bug or a small single-purpose feature with
  real behavior. Tagged `batch:express`, **no `lifecycle:*` skip** — full CI
  **and** a reviewer run. Fast not because it is *less gated* but because it is
  *reliably routed*: accept implies queue-and-drain, never "filed as a draft you
  have to nudge." trace:TASK-906

<!-- trace:TASK-906 — Followup C of TASK-0438; see docs/plans/2026-06-26-task-0438-fasttrack-lane.md -->

The line is deliberate: **trivial = fast-because-trusted-trivial; express =
fast-because-prioritized-and-routed.** `batch:express` is a routing bucket only,
it never buys less scrutiny.

## The express-eligibility litmus (entry criteria)

Before applying *either* lane tag, a request must pass ALL of these. **Fail any
one ⇒ it does not enter the lane** — file it normally (Approved/Draft for the
standard burndown). This is the same risk vocabulary the `aida assess` fence and
the EPIC-0428 autopilot envelope use, so the three stay consistent.

1. **Bounded & single-purpose** — one bug or one small feature, describable in a
   sentence; not an epic, not "and also."
2. **Low blast radius** — small file count, no public-API / struct / signature
   change that ripples to callers (the "build combined main" hazard class).
3. **Reversible** — a revert cleanly undoes it; no migration, no data write, no
   irreversible external effect.
4. **Excluded classes absent** — NOT architecture, NOT security, NOT autonomy
   machinery (orchestrator / drain / lease / queue / conflict), NOT cross-cutting
   lifecycle, NOT high-risk, NOT ambiguous (acceptance is clear or trivially
   inferable), NOT `vision` / `epic` / `principle` / `constraint` / `decision` /
   `term` types.
5. **Trivial-vs-express split** — given it passes 1–4:
   - *also* cosmetic / doc-only / one-obvious-line ⇒ **TRIVIAL tier**
     (`batch:fasttrack` + `lifecycle:no-review`).
   - real behavior, but still bounded ⇒ **EXPRESS tier** (`batch:express`, full
     gate).
   - **When in doubt between tiers, choose the more-gated one (express over
     trivial). When in doubt about eligibility at all, do not use the lane —
     file normally.**

## The one hard rule

Neither tier skips CI. CI runs and must be **green before merge** (never merge
red). The lane uses at most `lifecycle:no-review` (trivial only), **never**
`lifecycle:no-ci-wait` / `lifecycle:no-build` / `lifecycle:trivial` (those merge
optimistically before integrity gates are green — out of bounds for a trust
lane). Merge + pull/auto-bump never skip in either tier. "Without much ado" = no
human gating on the trivial tier, still integrity-gated everywhere.

## The express disposition contract (advisor side)

When an express request arrives, the advisor's disposition is **bounded and
prompt** — the operator's trust is in *speed of decision*, not just speed of
build. Five verbs, each with a recorded one-line reason; all map to existing
`aida assess` / `/aida-assess` actions:

- **accept** — eligible per litmus + worth doing now ⇒ apply the tier tag,
  **queue it, and drain** (accept *implies* routing; see below). Trivial: `aida
  fasttrack`. Express: `aida edit <ID> --status approved --tags batch:express`
  then queue + drain.
- **reject** — stale / out-of-scope / not-worth-it ⇒ `aida edit <ID> --status
  rejected` + a comment naming why.
- **dedupe** — duplicate of an existing spec ⇒ reject-with-a-pointer: `aida edit
  <ID> --status rejected` + `aida comment add <ID>` naming the superseder.
  (There is no standalone `aida dedupe`.)
- **clarify** — acceptance is ambiguous ⇒ `/aida-clarify` / `aida questions ask
  <ID>` to get the missing criterion, then re-dispose. Ambiguous items **never**
  enter the lane un-clarified.
- **escalate** — touches an excluded class or needs an operator strategic call ⇒
  park `NeedsAttention` / `needs-human` for the operator. This is the EPIC-0428
  boundary: the lane disposes *ordinary low-risk* requests; anything
  keystone / architecture / security escalates.

**Accept implies routing, by policy not by hope.** The named failure is "filed
Approved, then stranded un-queued." So acceptance *means* "on the queue with a
drain owner." Headless: `aida assess --apply --then-drain --only-tag express`
(or `[intake] on_apply = drain`). Keyboard: `aida fasttrack` files Approved-*and*-
queued atomically; the express path does the same plus the `batch:express` tag.

## Instructions

1. Read `$ARGUMENTS` as the change description. Run the **litmus** above. If it
   fails, stop — file it normally and say so. If it passes, pick the tier.
2. Pick a `--type` that fits (`bug` for a papercut/defect, `task` for a
   chore/doc, `story` for a small feature).
3. **File + queue in one shot**, capturing the SPEC-ID:
   - **Trivial tier** — `aida fasttrack` owns the convention (Approved + queued +
     `batch:fasttrack` + `lifecycle:no-review`):
     ```
     aida fasttrack "<description>" --type <type>
     ```
   - **Express tier** — file Approved + queued + `batch:express`, **no
     `lifecycle:*`**:
     ```
     aida add "<description>" --status approved --queue --batch express --type <type>
     ```
4. Implement on a fresh worktree/branch off latest `origin/main`. Add a `//
   trace:<SPEC-ID>` comment if it is code.
5. **Gate per tier:**
   - Trivial — reduced gate: `cargo build -p aida-cli`, `cargo fmt --all --
     --check`, `cargo clippy -p aida-cli -- -D clippy::correctness`, a quick
     smoke. Skip the exhaustive local run — CI runs the full suite, review is
     skipped.
   - Express — **full gate**: the relevant build + `fmt --check` + `clippy` +
     the affected tests locally, then **a reviewer runs in CI** (no
     `lifecycle:no-review`). Express is reviewed, not review-skipped.
6. Commit with the `(SPEC-ID)` trailer; open a PR.
7. **CI is the gate** (both tiers): read the required check = SUCCESS, *then*
   merge (squash). Express additionally needs the reviewer verdict to pass. If
   CI is red, fix-forward or **punt out of the lane** — never merge red.
8. `aida pull` to auto-bump the spec to Completed.

## The punt-out invariant (the trust-keeper)

The lane only stays trustworthy if work that turns out bigger than billed
**leaves the lane loudly** — it must **never silently re-gate itself**.

If, mid-implementation, you discover the work is non-trivial — it touches
architecture / security / autonomy machinery, needs a judgment call, grows
multi-file or high-blast-radius, or otherwise fails the litmus you entered on:

1. **Punt** — `/aida-punt` (or `aida punt`) the spec to **NeedsAttention** with a
   one-line reason. This is a *visible* park, not a quiet downgrade.
2. **File a finding** so it surfaces in `aida findings list`
   (`from-implementer:` source) and the `NeedsAttention` slot of `aida queue
   list`.
3. **Drop the lane tag** — `aida edit <ID> --tags -batch:express` (or
   `-batch:fasttrack` / `-lifecycle:no-review`) so the item re-enters **normal
   review** rather than riding the reduced/routed gate.
4. **Never silently re-gate.** Do not "just fix it anyway" under the lighter
   gate. EPIC-28's park-and-continue machinery already shelves the spec and lets
   a drain proceed; this invariant is the discipline layered on top.

A transient CI flake that parks `NeedsAttention` is *correct* visible-failure
behavior, not a regression — the status surface shows `blocked`; the operator
(or `--max-failures`) decides.

## Batch variant

Several lane items at once: tag each (`--batch fasttrack` or `--batch express`),
then drain that bucket:

```
aida queue work --batch fasttrack --auto-complete   # trivial: CI gates, review skipped
aida queue work --batch express   --auto-complete   # express: full CI + reviewer per item
```

One implement→CI→[review]→merge→pull lifecycle per item. The two buckets are two
filtered drains over the same engine; the only difference is whether
`lifecycle:no-review` is present.

Pairs with the normal review flow (for anything that fails the litmus),
`/aida-assess` (the headless disposition pass), `/aida-punt` (the punt-out step),
and `/aida-commit` (the trailer convention).

ARGUMENTS: $ARGUMENTS
