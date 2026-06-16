---
name: aida-import-plan
description: Import a saved /ultraplan (or other free-floating) plan file into AIDA conventions — detect its target SPEC, move it under docs/plans/, pin it to the spec, parse its sections, and verify its refs. Use after /ultraplan's "teleport back to terminal → save plan to file" hands you a loose markdown file.
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:5078c4b3 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Import Plan Skill

## Purpose

`/ultraplan` (and other cloud planning flows) end by saving the plan to a
loose local markdown file. That file is outside AIDA's conventions — not
in `docs/plans/`, not pinned to a SPEC, refs unverified. This skill closes
the loop: one command turns the loose file into first-class AIDA state.

This is **Direction B** of the AIDA/`/ultraplan` integration. Direction A
is `aida ultraplan <SPEC>` (TASK-113), which assembles the *prompt* you
feed to `/ultraplan`. This skill consumes the *output*.

## When to use

- Right after `/ultraplan` teleports back and you chose "save plan to file".
- Any time you have a plan markdown file that should live under `docs/plans/`.

## Skip if

- The plan is already at `docs/plans/YYYY-MM-DD-<slug>.md` and pinned to a
  spec — there's nothing to import.

## Invocation

```
/aida-import-plan <FILE>                # process, leave the queue alone
/aida-import-plan <FILE> --queue        # also queue the spec for implementer
/aida-import-plan <FILE> --auto-anchor  # auto-fix drifted refs (aida plan verify --fix)
/aida-import-plan <FILE> --dry-run      # report every step, take no action
```

## Workflow

### Step 1: Read the file

Read `<FILE>`. If it doesn't exist, stop and tell the user.

### Step 2: Detect the target SPEC-ID

Try these in order; stop at the first hit:

1. **Frontmatter** — a YAML `spec:` field, or the plan-template header line
   `Specs: SPEC-ID` (TASK-92's template uses `Specs:`).
2. **First heading** — `# Plan: SPEC-ID — …` or `# Plan: SPEC-ID`.
3. **Filename** — a `SPEC-TYPE-N` pattern in the filename (e.g.
   `story-86`, `STORY-86`, `task-114`).

Confirm the detected ID with `aida show <SPEC-ID>`. If it doesn't resolve,
or if step 2 was ambiguous (multiple candidates), **ask the user** which
spec this plan targets — never guess silently.

**Edge case — multi-spec plans:** `/ultraplan` output often names several
specs (e.g. "STORY-86 composes STORY-81"). Pick the **primary** spec (the
one in the title / `Specs:` first position). Note the others for Step 4.

### Step 3: Move the file into docs/plans/

The AIDA convention is `docs/plans/YYYY-MM-DD-<slug>.md`:

- **Date** — today (`date +%Y-%m-%d`).
- **Slug** — kebab-cased from the spec title, truncated to ~6 words.

```bash
git mv <FILE> docs/plans/<YYYY-MM-DD>-<slug>.md   # or `mv` if not tracked
```

**Edge case — a plan file already exists for this spec.** If
`docs/plans/` already has a file mentioning this SPEC, ask the user:
**replace** it, **keep both** (the new file gets a `-v2` / date-disambiguated
suffix), or **cancel**. Never overwrite silently.

### Step 4: Pin the plan to its spec

```bash
aida comment add <SPEC-ID> "Plan imported to docs/plans/<YYYY-MM-DD>-<slug>.md (via /aida-import-plan)."
```

If the plan includes an explicit effort estimate, normalize it to one of
`15m` / `1h` / `4h` / `1d` / `1w` and stamp it as the plan touchpoint:

```bash
aida edit <SPEC-ID> --add-tag effort:plan:<bucket>
```

This is advisory calibration data, not a gate. `1d` means 8 work-hours;
`1w` means 5 work-days / 40 work-hours.

For each **secondary** spec named in a multi-spec plan, add a lighter
note so the plan is discoverable from those specs too:

```bash
aida comment add <OTHER-SPEC> "Related plan: docs/plans/<YYYY-MM-DD>-<slug>.md (primary spec <SPEC-ID>)."
```

### Step 5: Parse the plan sections

Read the moved file and surface what it contains, section by section
(TASK-92's template defines the names):

- **Critical Files** — list them back to the user. No filing needed: once
  the plan is in `docs/plans/` and pinned, `aida queue work <SPEC>`
  (TASK-95) auto-discovers it and pre-populates the session manifest with
  this list. The import just has to land the file correctly.
- **Followups** — list them back to the user. **Do not file them now.**
  `aida queue done` (TASK-96) parses this same section and offers to file
  each as a child TASK at completion time, idempotently. Pre-filing here
  would double up. Just surface the count so the user knows they're there.
- **Verification** — note that the plan carries an executable verification
  script; `/aida-review` and the implementer can run it as the definition
  of done.

**Edge case — the file doesn't follow the 11-section template.** Process
whatever sections exist; warn the user which expected sections
(Critical Files, Verification, Followups, …) are missing.

### Step 6: Verify the plan's refs

```bash
aida plan verify docs/plans/<YYYY-MM-DD>-<slug>.md
```

This (TASK-93) reports drifted `path:line` refs, missing files, and absent
required sections. Surface the findings.

- With **`--auto-anchor`**, re-run as `aida plan verify <path> --fix` to
  rewrite drifted refs in place.
- **Edge case — the plan references symbols not in the current code.**
  `aida plan verify` surfaces these as warnings. Report them; do **not**
  fail the import — the plan may legitimately describe code that doesn't
  exist yet.

### Step 7: Queue the spec (only with `--queue`)

```bash
aida queue add <SPEC-ID> --for implementer
```

So the implementer seat picks the planned spec up next.

## Dry-run mode

With `--dry-run`, perform Steps 1–2 (read + detect) for real, then
**print** every action Steps 3–7 would take — the destination path, the
comment text, the parsed section summary, the `aida plan verify` command,
the queue command — and take **no** side effects. No file move, no
comments, no queue change.

## Final summary

Report: detected SPEC-ID, the new file path, the pinning comment, the
section summary (Critical Files / Followups counts, Verification present),
`aida plan verify` findings, and whether the spec was queued.

## Composes with

- **TASK-92** — the plan template defines the section names parsed here.
- **TASK-93** — `aida plan verify` re-anchors refs after the import.
- **TASK-95** — `aida queue work` reads the Critical Files / Verification
  out of the docs/plans/ file this skill lands.
- **TASK-96** — `aida queue done` files the Followups; this skill only
  surfaces them, to avoid double-filing.
- **TASK-113** — `aida ultraplan <SPEC>` is the prompt-side round-trip
  partner: it feeds `/ultraplan`, this skill consumes the saved output.