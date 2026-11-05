<!-- aida-plain spec="STORY-1158" source_modified_at="2026-09-16T02:12:29.778267030+00:00" generated_at="2026-09-16T02:19:39.099805136+00:00" -->
# Plain-English Why: STORY-1158

## Rationale

First slice of EPIC-67.

In plain terms: this work exists so someone can understand the reason for the spec without first knowing AIDA's internal vocabulary. It keeps the formal spec unchanged, then adds a separate explanation layer that a human can ask for when the terse contract is not enough.

## Jargon In Plain English

- Spec: the tracked requirement or work item.
- Graph context: the nearby parent, child, blocker, and reference links that explain how this work fits with other work.
- Cache: a saved copy that is reused until the spec changes.
- Surplus context: useful background that is available on demand but not loaded into every agent prompt by default.

## Concrete Novice Example

Imagine a new contributor runs `aida why STORY-1158 --plain` before touching code. They should learn that the current goal is: aida why <spec> --plain prints a plain-English rationale + one concrete example a novice could follow; jargon is defined inline. They can then inspect the linked context below, make the change, and know that this explanatory note will be reused until the spec is edited again.

## Spec Snapshot

- Title: v1: aida why <spec> --plain — generate + cache a separate plain-English rationale + example layer
- Status: InProgress
- Type: Story
- Source modified_at: 2026-09-16T02:12:29.778267030+00:00

## Acceptance In Everyday Words

- aida why <spec> --plain prints a plain-English rationale + one concrete example a novice could follow; jargon is defined inline.
- The result is cached as a separate artifact linked to the spec; a second call with an unchanged spec serves the cache (no regeneration); a changed spec regenerates.
- The spec's own description/acceptance is unchanged (verified: no write to spec text).
- The plain layer does not enter an agent's default context (surplus), but is retrievable on demand.

## Graph Context

- child of: EPIC-67 — Plain-English spec accessibility: a separate parallel layer, never a dilution of the spec (Draft)

## Linked Decisions

- No linked ADR/decision records were found in the immediate graph.
