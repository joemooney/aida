# SPIKE-81 — Why the shipped fast-lane and cluster-drain capabilities are never reached for

Date: 2026-09-18 · Status: findings recorded (proxy for the operator) · Non-goal: building any new drain capability.

## Premise and question

Six lanes are built, tested and documented: `batch:fasttrack` (+ `lifecycle:no-review`), `lifecycle:trivial`,
`batch:express`, `lifecycle:no-ci-wait`, and the cluster drain `--single-branch` / `--sequential`. Measured
2026-09-17, none had been used in the 30-day window; `aida usage` had zero `fasttrack` rows while `queue work`
ran 265 times. The triggering incident: an agent holding CLAUDE.md, the lifecycle doc and the batch help
concluded that "fix/test/review/CI a set of issues as one unit" did not exist and started designing it — it
already existed as `aida queue work --batch NAME --auto-complete --single-branch` (TASK-1003, SPIKE-70).

Zero usage here measures **discoverability and routing**, not value.

## Evidence (measured 2026-09-18)

**Where each lane is documented.** Occurrences of the lane token per surface (`grep -c`):

| lane token            | CLAUDE.md | docs/lifecycle.md | docs/autonomous-drain.md | README | docs/cli/*.md | skills |
|-----------------------|-----------|-------------------|--------------------------|--------|---------------|--------|
| `--single-branch`     | 0         | 0                 | 0                        | 0      | 0             | aida-burndown only |
| `--sequential`        | 0         | 0                 | 0                        | 0      | 0             | aida-burndown only |
| `batch:express`       | 0         | 0                 | 0                        | 0      | 0             | aida-fasttrack only |
| `batch:fasttrack`     | 0         | 0                 | 0                        | 0      | 0             | aida-fasttrack only |
| `lifecycle:no-review` | 0         | 0                 | 0                        | 0      | 1             | aida-fasttrack only |
| `lifecycle:trivial`   | 1         | 0                 | 1                        | 0      | 1             | aida-fasttrack only |
| `lifecycle:no-ci-wait`| 1         | 0                 | 0                        | 0      | 1             | aida-fasttrack only |

`queue work --help` does carry `--single-branch` and `--sequential` (5 hits) — behind a flag list that no
reader reaches; CLAUDE.md's daily-use block does not mention them.

**Nothing proposes a lane.** `grep` of `intake.rs`, `groom*.rs` and `queue_cmd.rs` for any lane tag
assignment: zero non-test hits. `backlog.rs` computes `classify_pair_overlap → PairVerdict::Serialize {shared}`
at groom time — exactly the "these want one branch" signal — and renders it as `serialize` / do-not-parallelize.
`aida do` routes on `execution_mode` only; `next[]` hints never name a lane.

**Contradictory help.** `aida backlog groom --help`: "tags every groomed item with `batch:NAME` so
`aida queue work --batch NAME` can drain them as one cluster". The default batch drain runs one full
lifecycle per member; "one cluster" is only true with `--single-branch`, which the help never names.

**fasttrack history.** Code activity Jun 26–27 (TASK-925/926/927), nothing since; zero invocations in the
30-day window. Its filing convention (`Approved + queued + tags`) is what `aida groom` now does with
`execution_mode` — grooming displaced it.

**Cost basis.** CLAUDE.md said PR CI is "~3-5 min". Measured over the last 23 successful PR runs of
`ci.yml`: median **9.7 min**, p25 9.3, p75 9.9, min 8.6, max 10.7. Stale by ~2×, in the direction that makes
fast lanes look less necessary than they are.

**Today's drains as a control.** Across 2026-09-17/18 (3 batches, 21 members) the dominant cost was review
round-trips (BUG-1197: 4 rounds, BUG-1213: 4, BUG-1207: 3), not CI. `lifecycle:no-review` would have been
wrong for every one of those; the lanes are for a different class of change (docs, papercuts) that this
week's queue simply did not contain in volume.

## Per-lane verdicts (one routing change each)

| lane | classification | the one change |
|------|----------------|----------------|
| `--single-branch` / `--sequential` (cluster drain) | **undiscoverable** | Make the groom's existing `Serialize{shared}` verdict *propose the command*: print `aida queue work --batch NAME --auto-complete --single-branch` (and `--sequential` when order matters) instead of "do not parallelize"; add one line to CLAUDE.md's batch paragraph. |
| `batch:fasttrack` + `lifecycle:no-review` (trivial tier) | **unroutable** (nothing proposes it) | `aida groom` proposes `lifecycle:trivial` for docs-only / single-string / papercut specs (heuristic: files touched ⊆ docs, templates, or a `papercut`/`severity:cosmetic` tag) — the advisor accepts or declines like any other disposition. `aida fasttrack` stays as the one-shot filing verb. |
| `lifecycle:trivial` | keep — becomes the only low-ceremony tier | proposed by groom (above); already correct in CLAUDE.md. |
| `batch:express` | **genuinely redundant** with `execution_mode = drain` (same gates, same routing, only the tag differs) | Retire the tier: `aida fasttrack --express` becomes an alias for `aida add --status approved --queue` + `--mode drain`; drop `batch:express` from docs/skills. |
| `lifecycle:no-ci-wait` | **wrong-default** for the one case it helps | With PR CI at ~10 min and Build now a required check, its value is overlapping the reviewer with CI. Make that the drain's behaviour for docs-only diffs automatically (no tag), keep the tag as the manual override; do not promote it further. |
| `aida fasttrack` verb | keep (thin wrapper), but not the entry point | Its entry point is the groom proposal; the verb is for humans filing by hand. |

## The general problem: where a capability lives after its spec closes

A completed spec is archived; its knowledge leaves the working set. Three carriers survive that, and a
capability needs all three:

1. **The groom proposal.** Grooming is the only moment with the routing signal (overlap verdicts, diff shape,
   tags). A lane that grooming never proposes will not be used, whatever the docs say.
2. **`next[]` / progress hints.** `aida queue progress --batch NAME` and `aida show <batch member>` should
   name the cluster command when the batch carries serialize verdicts; hints are read, flag lists are not.
3. **CLAUDE.md's daily-use block** — one line per lane, no more. Skills are read on demand; CLAUDE.md is
   loaded every session.

## Is the lane set too large?

Yes: six choices for a human at filing time, five of which are never proposed. Collapse to **three**:
`lifecycle:trivial` (no human review, docs/papercuts; groom-proposed), `--single-branch` (cluster; groom-proposed
from serialize verdicts), and the default drain (`mode=drain`, which absorbs `batch:express`; `no-ci-wait`
becomes automatic for docs-only). `--sequential` stays as a modifier of the cluster lane, not a lane.

## Side-effect fixes in this PR

- CLAUDE.md: PR CI cycle corrected from "~3-5 min" to the measured ~9-10 min median.
- CLAUDE.md batch paragraph: names `--single-branch` / `--sequential`.
- `aida backlog groom --help`: says the default drains one member at a time and names `--single-branch`.

## Follow-ups filed

- Groom proposes lanes: serialize verdict → cluster command; docs-only/papercut → `lifecycle:trivial`.
- Retire `batch:express` as an alias of `mode=drain`.
- `queue progress --batch` / `next[]` hints name the cluster command when serialize verdicts exist.
