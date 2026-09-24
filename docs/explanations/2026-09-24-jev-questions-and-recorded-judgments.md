# Jev today: three advisory questions, no shared catalog yet

**Verified against**: `aida-cli-lib/src/exposition.rs`, `wiki.rs`, `graded_review.rs`,
`contradictions.rs`, `evaluator.rs` on `main` (commit `eb680ef3`), plus
`docs/cli/08-reporting.md` and `docs/environment-variables.md`.
**Status**: living explanation — refresh if a new Jev call site is added, or
if a reusable question catalog / recorded-judgments store is built.

## What Jev is

Jev is an optional external AI judgment service (TypeSafe AI's "System One")
that a few AIDA commands can consult for an advisory second opinion. It is
off by default and never required: every feature that can call it also has a
deterministic, offline fallback. The single switch is one environment
variable, `AIDA_JEV_API_KEY` — set it and the call can happen; leave it
unset and AIDA never reaches the network for this. If a call is slow, errors,
or the key is missing, AIDA doesn't guess or wait indefinitely: it falls back
to the offline behavior and records the fallback as "unavailable," never as
if the advisory check had passed.

Historically each integration was just a one-off prompt buried in the
feature that used it. This explains what's really there today, in plain
terms, separate from any future plan.

## Where Jev is actually wired in

Three places, each with its own hand-written question:

### Explaining a requirement in plain language (`aida explain`)

Turning a spec into a plain-language summary is done first by
pattern-matching the spec text — no AI involved in that extraction step.
What follows is a quality check on the result. Offline, that check is purely
mechanical: does it look jargon-heavy, and did important language like "fail
closed" survive the rewrite? When the Jev key is configured, one question is
sent instead: *does this plain-language version still preserve the
original's safety constraints and acceptance requirements?* Jev's answer
replaces the mechanical score for that one dimension (constraint
preservation) only.

The readability figure shown next to it stays mechanical either way — it is
not a Jev judgment, and, importantly, it is not a measurement of whether an
actual person understood the text. Measuring genuine human comprehension of
these explanations is a separate effort that has not been built yet.

### Reviewing a diff against a spec's acceptance criteria

This runs inside the automated pull-request completion flow, not as a
command you invoke directly. Any acceptance criterion that can't be checked
by simply running a command is put to Jev on its own: *does this diff
satisfy this specific criterion?* Each criterion gets its own yes/no-leaning
answer and a confidence figure, and the overall verdict only settles as pass
or fail once the confidence clears a threshold — an answer that isn't
confident enough is left open for a person to decide, rather than rounded
into a pass or a fail.

### Scanning the requirement store for possible contradictions (`aida doctor --contradictions`)

Ordinary text and reference matching finds candidate pairs first; only then
is a pair handed to Jev to classify as compatible, one superseding the
other, or genuinely contradicting each other. Jev never discovers a
candidate pair on its own — it only classifies pairs the mechanical pass
already found.

### What's *not* wired in

A fourth kind of question — using a judgment like this to route work to the
right kind of reviewer (developer, reviewer, advisor, architect, research) —
is a natural extension of the same idea, but it is not implemented. Nothing
in the code today asks Jev that question or acts on an answer to it.

## What happens to each answer

This differs by feature, because there is no shared place these get
recorded yet:

- The `aida explain` audit is saved into that spec's own exposition sidecar
  file on disk (`.aida/expositions/<SPEC-ID>/<audience>.yaml`), so the score
  and any findings are visible again later without re-asking Jev.
- The pull-request review verdict, including each criterion's individual
  score and confidence, is saved to a small file per pull request
  (`.aida/review-verdicts/PR-<n>-graded.json`).
- The contradiction scan's answers are not saved anywhere. Each run re-asks
  Jev from scratch, and only that run's own terminal/JSON output shows what
  it decided.

None of the three keeps the fuller kind of record a shared log would need —
a hash of exactly what was asked, which version of the question was used, a
redacted copy of the exact payload, timing, and so on. A single reusable
catalog of named questions, and one place where every answer from any of
them is recorded with that kind of detail, is a real and worthwhile
direction, but it does not exist yet. Nothing above should be read as
already providing it.

## Jev advises; it never decides alone

In every case Jev only advises. What a feature does with its answer is that
feature's own choice: the pull-request flow leaves an unclear answer for a
human, the `aida explain` audit flags "needs revision" past a threshold, and
the contradiction scan simply reports what it found for someone to act on. A
project could choose to treat the same kind of answer differently — that
policy lives with the feature consuming the judgment, not with Jev itself.

## See also

- `docs/cli/08-reporting.md` — `aida explain` and `aida wiki` command
  reference (flags, gotchas).
- `docs/environment-variables.md` — the `AIDA_JEV_API_KEY` /
  `AIDA_JEV_ENDPOINT` / `AIDA_JEV_MODEL` variables and exactly which commands
  read each one.
