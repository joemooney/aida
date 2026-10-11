# Next-batch proposal after batch:night-2026-10-10

Author: night-fable (product/research relay seat), 2026-10-11 00:30 -07:00.
Status: PROPOSAL. The advisor and operator dispose in the morning. This PR is a
reading copy and is never merged; the decisions land as tags on the specs.

## What this is based on

- `aida list --status approved` and `--status in-progress` at 00:14 -07:00.
- The night orchestrator brief (`.aida/handoff/night/latest.md`): 13 queue items,
  concurrency 1, factory PRs merged between items.
- The factory seat handoff: BUG-1919, BUG-1813, TASK-1335, TASK-1717, DOC-6 and
  STORY-1649 have or will have draft PRs; ten specs were marked plan-only.
- The lib-split relay: runs 2 and 3 shipped, run 4 (PR #2494) approved on content
  and rebasing; about 100k lines remain in `lib_part*.rs` at 8–12k lines per run,
  so TASK-1538 needs roughly 8–12 more runs. TASK-1539/1540/1541 stay ineligible
  until it completes.
- Tonight's draft triage (tag `triaged:night`): 31 recommended approve, 1 reject
  (STORY-1657, PR already merged), 1 merge-into (BUG-1946 → BUG-1948).
- The STORY-1654 design sketch (recommends Option 1: the orchestrator launches the
  standalone headless reviewer `aida queue work PR-<n> --role reviewer --no-human`).

## Morning dispositions this proposal assumes

1. STORY-1654 sketch signed off (or amended). Everything in batch A tier 1 hangs on it.
2. The triage recommendations acted on: approve the 31, reject STORY-1657,
   fold BUG-1946 into BUG-1948.
3. The night batch's parked items (`parked:night`) read and either re-queued into
   batch A tier 0 or held.
4. BUG-90 in aida-relay ruled on (relay-side hooks for STORY-1653). Not blocking:
   STORY-1653's AIDA side ships without it.

## Batch A: `batch:2026-10-11` — finish and unblock

Goal: land what the night started, then clear the one chain that blocks relay
merges. Target 10–11 items, concurrency 1, same discipline as the night brief.

### Tier 0: carry-over (whatever the night did not land)

Most likely TASK-1526 (heft 40; the brief said start only with enough night
left), STORY-1653, STORY-1652. Resume in the night's worktrees, never re-dispatch.
Then the remaining factory PRs via the same review-and-merge loop.

### Tier 1: the review-authority chain (one lane, one implementer, in order)

These touch `review_authority.rs`, `pr_ship.rs` and `seat_authority.rs`; run
them serially so they do not conflict.

1. STORY-1654: implement the signed sketch. Skill/prompt edits in
   `aida-core/templates/skills/{aida-orchestrate,aida-burndown,aida-review}.md`
   plus `make sync-templates`; the optional record-time hint guard only if the
   advisor wants it. Small (S).
2. BUG-1918 / PR #2485: take it off hold, rebase, address the advisor REQUEST
   CHANGES, then STORY-1659 (its review spec) closes with the merge.
3. BUG-1934: bind the seat grant to the presenting session. PR #2485 already
   re-validates the grant; do this on top of it, not in parallel.
4. STORY-1655: the solo (single-human) carve-out the gate needs once 2485 lands.
5. BUG-1808: preflight child-seat delegation (heft 2, guided). It answers the
   sketch's first open question (the relay seat must be entered with
   `--delegate-seat reviewer`, otherwise the headless reviewer launch fails closed).

### Tier 2: small verified bugs from tonight's triage (factory-shaped)

Each is one file or one hook, independently reviewable, suitable for the factory
seat's blind-write loop with a night-style fresh review before merge:

- BUG-1917 push/pull `--code-only` / `--store-only` help swap.
- BUG-1922 `/aida-capture` template still says `aida list --search`.
- BUG-1920 `detect_agent_type` checks `GEMINI_*` before `CLAUDE*`.
- BUG-1935 + BUG-1927 together (both in `pr list`: non-required checks shown as
  CI Failing, multi-spec PRs repeated, PRs without an ID dropped).
- BUG-1936 git-guardrails hook false positives on text mentions, no stderr reason.
- BUG-1942 history-index stall guard never retries; hint has no TTY check.
- BUG-1945 digest sort compares mixed timestamp strings.
- BUG-1938 `aida remote mirror` / `init` leave `.aida/config.toml` uncommitted.
- BUG-1937 `make check-ci-fast` skips the CLI-manual drift guard.
- BUG-0445 shell wrapper captures init's stdout and hides its prompts.

### Tier 3: only when its blocker lands

- TASK-1527 (durable refresh request + detached worker) after TASK-1526 merges.

### Explicitly out of batch A

- TASK-1538 and the slice-* stack: the lib-split relay owns it.
- TASK-1607 / PR #2451 and TASK-1608: the operator's hold on 2451 stands.
- STORY-1651 (docs reorg): excluded by the operator.
- EPIC-76, TASK-1612, CR-7, TASK-1511, TASK-1523: guided or plan-only; not relay work.

## Batch B: `batch:2026-10-12` — operability, then the lanes decision

Goal: the operator-visible gaps found this week, plus the prerequisites for the
lanes work so it can be queued the batch after.

1. STORY-1646's siblings as one lane (hooks + statusline, same implementer):
   STORY-1647 (pre-commit refuses commits on main in the primary checkout),
   STORY-1648 (flag primary-checkout drift at session start and in the statusline),
   TASK-1721 (server-side `--no-verify` backstop, if approved).
2. Contained-launch cluster: BUG-1928 and BUG-1929 (headless contained posture
   cannot write; sandbox boundary excludes the store and cache). These are the
   reason the STORY-1654 sketch says "no `--sandbox` for the standalone reviewer
   yet". STORY-1486 (launch profile, operator-only) follows them if the operator
   wants it in this batch.
3. Small CLI features: STORY-1645 (`faq topics`; needs the command-shape decision
   noted in triage, no `aida faq` subcommand exists), STORY-1641 (external
   subcommand dispatch).
4. STORY-1656 (multi-forge init) with BUG-1932 (hub divergence) and BUG-1933 as
   one forge lane; STORY-1656's end-of-init `remote status` is where 1932 surfaces.
5. STORY-1661 (history index advance off the hot path) once STORY-1650 is in.
6. BUG-1816 (orchestrate child model/limits): narrow to the skill template per
   triage; cheap once STORY-1654 has reshaped the same template section.
7. Lanes prerequisite, product work not relay work: file EPIC-74's ADR and apply
   the V4 acceptance amendment to STORY-1481/1482/1483. Until that is done the
   three children still describe the rejected model and must not be dispatched.
8. TASK-1523 (north-star measurement, heft 23): attended day work, not a relay item.

## Sizing note

The night batch had 13 items with one heft-40 item at the end. By the time the
morning reads this, the count landed tells the real throughput. If fewer than
eight landed, cut batch A to tiers 0–1 plus the first five of tier 2 and push the
rest to batch B.

## Open questions for the morning

1. Does the operator want the factory seat to keep running by day? Tier 2 is
   shaped for it; without it, tier 2 is one-implementer serial work (~1 item/hour).
2. STORY-1654 open question 2: should implementers also move out of process, or
   only reviewers? Batch A assumes reviewers only.
3. Is BUG-1934 wanted before or after PR #2485 merges? This proposal says after.
