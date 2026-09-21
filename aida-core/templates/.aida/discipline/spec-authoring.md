# Spec authoring: separate shipping proof from outcome measurement

Acceptance on the spec that ships a change must be verifiable before that
change merges. Keep the mechanism, its tests, and observable fixture behavior
on the shipping spec.

An outcome that can only be measured after shipping belongs on a follow-up
measurement spec. Make that spec blocked by the shipping spec and give it:

- the outcome to measure;
- the post-deployment window (for example, the next 20 specs); and
- the threshold that would falsify the change.

Do not weaken or delete the outcome. Split it so shipping cannot deadlock on
evidence that cannot exist yet.

## Worked example

TASK-1291 originally required: “Measured on the next 20 specs: median
rounds-to-merge drops; recorded on this spec as the acceptance evidence.” A
reviewer could never verify that before merging the change, while merging was
blocked on verification. The shipping spec should instead verify the round
tracking mechanism and its behavior on a fixture. A blocked follow-up spec
should measure the next 20 specs and record whether the median fell by the
stated threshold.

`aida criteria` flags common post-deployment shapes as advisory and recommends
this split. The heuristic is intentionally shallow; a human confirms the flag.

<!-- trace:TASK-1293 | ai:codex -->
