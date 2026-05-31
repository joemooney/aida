---
name: aida-digest
description: Produce a curated narrative work digest — Released / Major progress / Strategic direction / Next iteration / Process artifacts — for a time window. The advisor's primary outward-facing artifact; distinguishes meaningful achievement from churn.
allowed-tools:
  - Bash
  - Read
  - Write
---

# AIDA Digest Skill

## Purpose

Produce a curated narrative report of project work in a time window — releases,
EPIC progress, strategic filings, in-flight / queued items — that is shareable
across audiences. The editorial logic is deterministic Rust in `aida digest`;
this skill picks a sensible window + audience for the situation and presents the
output.

This is the **advisor seat's** primary outward-facing artifact:

- "What shipped this week?" (release-ready story for friends / colleagues)
- "Where are we?" (mid-cycle internal status for the team)
- "What did I learn?" (process-rich self-retrospective)

## When to use

- The user asks for a digest, recap, weekly summary, what-shipped report, or
  release notes seed.
- At a release boundary (`scripts/release.sh` cuts a tag) — run with
  `--since <prev-tag>` to seed release notes.
- The advisor session is winding down and the user wants a snapshot to share.
- Retrospective time: `--audience self --include-process` surfaces memory
  entries + pivots alongside the work.

## Skip if

- The user wants a single spec's detail — use `aida show <SPEC-ID>` instead.
- The user wants the in-progress queue picture — use `aida queue list` or
  `aida drain status`.
- The user is asking for raw commit history — `git log` is the right tool.

## Latest marker

!`test -f .aida/last-digest.toml && echo "Last digest window ended: $(grep window_end .aida/last-digest.toml | head -1 | cut -d'=' -f2)" || echo "No marker — next digest defaults to 24h."`

## Audience reference

| Audience    | Framing                                | SPEC-IDs | Process |
|-------------|----------------------------------------|----------|---------|
| `customer`  | feature framing, release-centric       | hidden   | off     |
| `team`      | technical depth, cluster-PR shape      | shown    | on      |
| `self`      | full retrospective, memory + pivots    | shown    | on      |

Customer is the **default** — and the audience to lead with when the user has
not specified. Switching to team / self is a deliberate ask, not an implicit
fallback.

## Workflow

### Step 1: Pick a window

The default — bare `aida digest` — runs **since the last marker**, falling
back to **24h** when the marker is absent (first run). When the situation
suggests a different window, pick one:

- Weekly recap → `--since 7d`
- Monthly status → `--since 30d`
- Since last release → `--since v0.X.Y` (the immediately previous tag)
- Anchored to a date → `--since 2026-05-15`

Surface the choice to the user before running so they can correct.

### Step 2: Pick an audience

If unclear, ask. The three concrete reads:

- "Share with the world / friends / a colleague" → `--audience customer`
- "Show the team where we are" → `--audience team`
- "I want a retrospective for myself" → `--audience self`

### Step 3: Run the digest

```bash
aida digest --since <window> --audience <audience>
```

Add `--format brief` when the user wants the single-paragraph TL;DR.
Add `--format json` when they want machine-readable output.
Add `--out <path>` to write to a file instead of stdout (useful for
`docs/digests/` archives or release-notes drafts).

### Step 4: Present the result

Render the digest in scrollback. If it landed in a file (`--out`), name the
path. Offer the obvious next moves:

- Edit for tone / framing if the user wants a human pass before sharing.
- Re-run with a wider / narrower window if the output feels off-balance.
- Re-run with a different audience if the framing missed (e.g., team output
  ended up in a customer-facing thread).

### Step 5: Cadence marker

Every successful run (anything that isn't `--reset`) stamps
`.aida/last-digest.toml` with the window end, so the next bare `aida digest`
picks up where this one left off. To re-digest the same window without
advancing the cursor, use `--since <explicit>` rather than `--reset`.

`aida digest --reset` clears the marker — use it when the next digest should
re-start from a wider window than the marker would suggest.

## Editorial rules (what the CLI does, not what you do)

- **Drops noise commits** — `docs:`, `style:`, `chore:`, `revert:` subjects,
  and anything containing "typo".
- **Collapses cluster-PRs** — a PR carrying ≥2 distinct spec IDs renders as
  one theme line, not N spec lines.
- **Keeps only superseded rejections** — a rejected spec appears in
  "Strategic direction" only when it carries a `supersedes` / `pivoted-from`
  link or tag (the rejection led somewhere; the work didn't vanish).
- **Strips SPEC-IDs in customer mode** — `STORY-NNN` / `BUG-NNN` /
  `TASK-NNN` tokens are removed from titles, theme lines, and parens.
- **Process artifacts are best-effort** — reads `~/.claude/projects/<slug>/
  memory/MEMORY.md`; missing memory pack silently skips the section.

## Composition

- **`aida release`** — at release-cut time, run with `--since <prev-tag>`
  to seed release notes.
- **`role:advisor` (advisor seat)** — natural home; the editorial judgment
  about "what was meaningful" is advisor work.
- **`aida usage`** — sibling telemetry surface (per-command analytics);
  complementary, not overlapping.
- **`docs/plans/`** — `aida digest` scans them for "Notable plans"; keep
  the date-prefixed filename convention so they appear correctly.

## Next steps

After producing a digest, the natural moves form a small table:

| Path | What happens | Why |
|------|--------------|-----|
| ▶ Save it | `aida digest --since <window> --audience <a> --out docs/digests/<date>.md` | Archives the snapshot; advances the marker so the next digest picks up after it |
| ⇒ Re-frame | re-run with a different `--audience` or window | Lets the user compare framings before sharing |
| ⏸ Discard | nothing | The digest output stayed in scrollback; the marker advanced regardless |

## Related commands

- `aida digest --reset` — clear the cadence marker
- `aida queue list` — what's queued for next iteration (complements digest)
- `aida drain status` — live `--auto-complete` orchestrator state
- `aida usage` — per-command telemetry
