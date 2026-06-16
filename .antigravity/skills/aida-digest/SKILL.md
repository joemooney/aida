---
name: aida-digest
description: Produce a curated narrative work digest — Released / Major progress / Strategic direction / Next iteration / Process artifacts — for a time window. The advisor's primary outward-facing artifact; distinguishes meaningful achievement from churn.
allowed-tools:
  - Bash
  - Read
  - Write
---
<!-- AIDA Generated: v2.0.0 | checksum:65527657 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


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
| `operator`  | CLI-surface diff: what you can now DO  | hidden   | off     |

Customer is the **default** — and the audience to lead with when the user has
not specified. Switching to team / self / operator is a deliberate ask, not an
implicit fallback.

`operator` is the only **capabilities lens**: it answers "what changed in the
CLI surface that I, the day-to-day user, can now do?" — not the work narrative.
The CLI replaces the Released / Major-progress / Strategic sections with
surface-grouped buckets: **New commands / Changed flags & behaviors / Fixes
you'll notice / New skills**, value-framed with SPEC-IDs stripped. Reach for it
when the user asks "what's new for me", "what changed in the CLI", "what can I
do now that I couldn't last week".

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
- "What's new in the CLI for me / what can I do now?" → `--audience operator`

### Step 3: Run the digest

```bash
aida digest --since <window> --audience <audience>
```

Add `--format brief` when the user wants the single-paragraph TL;DR.
Add `--format json` when they want machine-readable output.
Add `--out <path>` to write to a file instead of stdout (useful for
`docs/digests/` archives or release-notes drafts).

### Step 3b: Operator lens — apply the Layer-2 value translation

For `--audience operator`, the CLI emits the deterministic **Layer-1 candidate
set** (run `--format json` to see the raw `capabilities` buckets). The CLI now
**prefers a captured source over git inference**: when any spec completed in the
window carries `interface_changes` recorded at close (`aida queue done`), those
exact surface-delta lines ARE the candidate set — no clippy/internal noise to
filter, because the implementer only recorded user-facing changes. When nothing
in the window has captured `interface_changes` (older history), the CLI falls
back to classifying commit subjects (dev-noise dropped, surface-bucketed,
SPEC-IDs stripped).

Either way the CLI cannot do the non-deterministic part — judging user-impact
and rewriting a dev-task subject into value — so that is **your** job before
presenting:

- **Re-judge the kept set by user IMPACT, not commit type.** The classifier keeps
  `feat`/`fix` and drops `clippy`/`refactor`/`test`, but a kept commit may still
  be invisible to the operator (internal MCP plumbing that changed no surface).
  Drop those. Conversely a `fix` that reads dev-internal is often a real
  user-facing crash/bug — keep + translate it.
- **Rewrite each line from the CLI/value perspective, not the dev-task title:**
  - New command → "you can now `aida X` — <what it does>."
  - Changed flag/behavior → "`aida Y --z` now does W (was V)."
  - Fix → "no longer <bad thing> when <case>." (e.g. `conflict.rs truncate
    panics on non-ASCII` → "AIDA no longer crashes on spec descriptions with
    emoji / non-ASCII.")
- **Keep the surface grouping** (New commands / Changed flags & behaviors /
  Fixes you'll notice / New skills) and keep SPEC-IDs out.

The deterministic buckets are the floor; your translation is what makes the
operator digest readable.

**Capture at close (the durable feed).** The cleanest Layer-1 source is the
implementer recording interface changes when they close a spec, not git
inference at digest time. At `aida queue done` (at a TTY) you are prompted per
surface; non-interactively (or as an agent) pass the lines explicitly:

```
aida queue done STORY-42 \
  --interface-cli "aida mailbox list — new command" \
  --interface-mcp "queue_add — now advisor-gated"
# or, for a no-impact spec (clippy/refactor/test):
aida queue done STORY-43 --no-interface-change
```

The MCP `queue_done` tool mirrors this via `interface_cli` / `interface_mcp` /
`interface_tui` / `interface_other` arrays (or `no_interface_change: true`).
Absent ⇒ the spec never appears in the operator digest. This is the deterministic
filter STORY-542 added; the digest reads it automatically.

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