# Landing approved work

<!-- trace:STORY-2809 | ai:claude -->

A pull request reaches the default branch through a merge train
(`aida pr train`), which tests the combined result before it merges. This page
describes the CI a pull request must pass before a train admits it. The train
itself is described in the
[git lifecycle reference](../cli/04-git-lifecycle.md).

## Two CI tiers on a pull request

| Tier | Runs when | What it runs |
| --- | --- | --- |
| FAST | every push to a pull request | source guards and a workspace compile check |
| FULL | the head is approved, or carries the `ci:full` label | the complete test shards, the same ones a train runs |

A push always runs FAST, including a push to a pull request that still carries
`ci:full` from an earlier approval. The new head runs FULL only after it is
approved again.

## Full CI starts when a head is approved

`aida review record <ID> --verdict approved --pr <N>` applies the `ci:full`
label to the pull request after it records the verdict. GitHub starts a FULL
run on the head that was approved. If the label is already present from an
earlier head, the command removes and re-adds it, because adding a label that
is already there starts nothing.

An advisor can start the same run by hand:

```bash
gh pr edit <N> --add-label ci:full
```

FULL runs for different pull requests run at the same time, within the
repository's Actions concurrency limits. Only a superseded run of the same
pull request is cancelled.

## The train admits only a green full-tier head

`aida pr train` (named PRs, `--all-ready`, and the advisor's automatic train)
checks each approved member's exact head commit. It admits the member only when
that commit has a FULL run, reported as `ci-tier: full`, whose
`Build (ubuntu-latest)` check passed. A FAST run on the same commit does not
count, and neither does a FULL run on an earlier head.

A member without that run is left out, and the reason names what is missing:

| Reason printed | Meaning | What to do |
| --- | --- | --- |
| `no full-tier CI run exists at its head <sha>` | no FULL run has started on this commit | none; the train requests one, and the PR rides a later train |
| `full-tier CI run <id> at its head <sha> has not finished` | the run is in progress | wait for it, then run the train again |
| `full-tier CI run <id> at its head <sha> is red` | the run failed or was cancelled | fix the branch, or rerun the failed jobs for a flake |

With `--all-ready`, a member that is left out waits for the next train. A PR
named on the command line that is left out makes the command exit non-zero.
`--dry-run` reports the same reasons and requests nothing.

The train still runs its own CI on the combined result before it merges. The
full run on each head removes the failures a single PR would cause by itself;
the train's run catches the ones that appear only when PRs are combined.

## Overriding the check

```bash
aida pr train <N...> --skip-full-check
```

`--skip-full-check` admits members whose head has no green FULL run. Each
admission prints a warning and appends a `train-admit-skip-full-check` entry,
with the PR, head and reason, to `.aida/advisor-activity.jsonl`. The override
applies to that invocation only. Every other train gate still applies.

## Repositories without tiers

The check applies where CI reports a `ci-tier:` check. A repository whose CI
has a single tier is admitted as before.
