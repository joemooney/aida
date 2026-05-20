# AIDA Digest

Produce a curated narrative work digest for a time window.

## Instructions

Follow the workflow in `.claude/skills/aida-digest.md`:

1. Pick a window — bare `aida digest` (marker fallback to 24h), `--since 7d`,
   `--since 30d`, or `--since <git-tag>` for a release-anchored cut.
2. Pick an audience — `customer` (default, no SPEC-IDs), `team`, or `self`.
3. Run `aida digest --since <window> --audience <audience>`. Add
   `--format brief|json|plain` for alternate shapes, `--out <path>` to write
   to a file.
4. Present the result. Offer to re-frame (different window/audience), save
   (`--out docs/digests/<date>.md`), or stop.
5. The cadence marker at `.aida/last-digest.toml` advances on every successful
   run; `--reset` clears it.

ARGUMENTS: passed straight through to `aida digest`. The single-word
shortcut `customer|team|self` is read as `--audience <that>`; otherwise the
arguments are forwarded verbatim.
