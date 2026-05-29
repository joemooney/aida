---
name: aida-techdebt
description: End-of-session duplication + debt scan. Read-only sweep for duplicated code, copy-pasted trace comments, dead trace paths (trace → Rejected spec), spec-graph duplicates, and orphan files — each finding gets a one-line recommendation and a filing verb. Use at the end of a working session or when the user says "techdebt", "find duplication", or "clean up".
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
---

# AIDA Tech-Debt Scan Skill

## Purpose

A **read-only** end-of-session hygiene sweep. Per Boris's "Beyond the
Prompt: Claude Code" — run a debt scan at the end of every session to
find and kill duplication before it compounds. This skill is the AIDA
flavour of that habit: it leans on the requirement graph and trace
comments AIDA already carries, so the scan finds debt a generic
duplication linter can't.

The skill **surfaces** debt and **recommends** an action per finding. It
does not delete code or mutate specs on its own — the operator (or a
follow-up `/aida-implement`) does the killing. Every finding ends with a
filing verb so an observation worth keeping lands in the substrate
instead of decaying with the session.

## When to use

- At the end of a working session — the canonical trigger.
- The user says *"techdebt"*, *"find duplication"*, *"any dead code?"*,
  or *"clean up before I commit"*.
- After a large feature land or a multi-agent batch, when copy-paste
  and orphaned scaffolding are most likely.

## Skip if

- The session touched nothing (no diff, no new files) — there's no fresh
  debt to find. A full-repo scan on every trivial turn is noise.
- The user wants a *specific* refactor — use `/aida-implement` against a
  spec instead of a broad scan.

## Workflow

Run the five scans below, then present one consolidated report (Step 6).
Each scan is read-only — `grep`, `Glob`, `aida list`. Nothing here
writes.

### Scan 1: Duplicated code blocks

Boris's original target. Look for copy-pasted logic — the same function
body, match arms, or error-handling block repeated across files.

- Start from the session's own diff so the scan is scoped, not
  whole-repo: `git diff --name-only main...HEAD` (or `git diff --stat`).
- Read the changed files and look for blocks you could hoist into a
  shared helper. The trace graph already knows where helpers live —
  `aida plan helpers <spec>` derives a "don't reimplement this" section
  from sibling/tag-mate trace comments.
- **Recommendation shape:** *"`fn parse_foo` in `a.rs:120` duplicates
  `b.rs:88` — extract to a shared helper."*

### Scan 2: Copy-pasted trace comments (same SPEC-ID in suspicious mass)

A single SPEC-ID stamped across an implausible number of files is a
copy-paste smell — the trace was pasted along with the code, not
authored. Count occurrences per ID:

```bash
grep -rhoE "trace:[A-Z]+-[0-9]+" --include=*.rs . | sort | uniq -c | sort -rn | head -20
```

A high count is not automatically wrong — a genuinely cross-cutting
EPIC legitimately spreads. Judgement call: is the spread *coherent*
(one feature, many files) or *suspicious* (a paste artifact in unrelated
files)? Spot-check the outliers with `Read`.

- **Recommendation shape:** *"`trace:TASK-X` appears in 14 files across 3
  unrelated modules — likely a paste artifact; re-trace the unrelated
  hits to their real specs."*

### Scan 3: Dead trace paths (trace → Rejected/Archived spec)

Code tagged `trace:SPEC-X` where `SPEC-X` is **Rejected** is dead code
pointing at work that was abandoned. Cross-reference:

```bash
# IDs that were rejected
aida list --status rejected --json | grep -oE '"spec_id": *"[^"]+"'
# then grep each rejected ID's trace marker across the tree
grep -rn "trace:<REJECTED-ID>" --include=*.rs .
```

For each hit, confirm with `aida show <ID>` that it really is Rejected
(not just renamed). Live code tracing a Rejected spec is the single
highest-value finding this scan produces.

- **Recommendation shape:** *"`src/x.rs:40` traces `TASK-380` which is
  Rejected — verify the code path is unreachable, then remove."*

### Scan 4: Spec-graph duplicates (TASK-N ≈ TASK-M by title)

Two specs describing the same work fragment the graph and split trace
comments. Pull titles and eyeball near-duplicates:

```bash
aida list --json | grep '"title"'
```

Look for titles that differ only cosmetically, or a TASK that restates an
existing STORY's acceptance. `aida search "<phrase>"` confirms whether a
candidate already has a near-twin. (The seeded META-003 "Find Duplicates"
prompt and `/aida-evaluate` go deeper if the user wants AI-scored
similarity.)

- **Recommendation shape:** *"`TASK-512` and `TASK-511` both describe the
  tag-namespace sweep — fold one into the other and `aida edit <dupe>
  --status rejected` with a `subsumes:` note."*

### Scan 5: Orphan files (no SPEC reference anywhere)

Source files carrying **zero** trace comments *and* not referenced by any
spec. Treat this scan as the noisiest — many legitimate files (entry
points, generated code, configs) have no trace and that's correct. Orphan
≠ dead; it's a *prompt to check*, not a verdict.

```bash
# files with no trace marker at all
for f in $(git ls-files '*.rs'); do grep -qE "trace:[A-Z]+-[0-9]+" "$f" || echo "$f"; done
```

Filter aggressively before surfacing — only flag a file that looks like
abandoned scaffolding (a half-finished module, a stale experiment), not
every untraced file.

- **Recommendation shape:** *"`src/experiments/old_dispatch.rs` has no
  trace and nothing imports it — confirm it's abandoned, then remove or
  file a TASK to wire it up."*

### Step 6: Present the consolidated report

One report, grouped by scan, severity-ordered (dead trace paths and code
duplication first; orphan files last because they're the noisiest). Keep
it a short table, not a wall of grep output:

```
## Tech-debt scan — <N> findings

### Dead trace paths (highest value)
- src/x.rs:40  →  trace:TASK-380 (Rejected)  ·  verify unreachable, remove

### Duplicated code
- a.rs:120 ≈ b.rs:88 (parse_foo)  ·  extract shared helper

### Copy-pasted traces
- trace:TASK-X ×14 across 3 modules  ·  re-trace unrelated hits

### Spec-graph duplicates
- TASK-512 ≈ TASK-511  ·  fold + reject one with subsumes: note

### Orphan files (verify before acting)
- src/experiments/old_dispatch.rs  ·  confirm abandoned, then remove
```

### Step 7: Capture — compose with `aida findings add`

Debt the operator won't fix *this* session should not evaporate when the
context does. For each finding worth keeping, file it:

```bash
# A debt observation awaiting triage (promote at recurrence, dismiss if noise)
aida findings add \
  --note "Dead trace path: src/x.rs:40 traces TASK-380 (Rejected). Verify unreachable, then remove." \
  --kind observation --severity minor \
  --tags "aida:techdebt,duplication" --linked-specs TASK-380
```

For actionable cleanup the operator wants done, file a real TASK instead:

```bash
aida add --title "Extract shared parse_foo helper (a.rs/b.rs dup)" \
  --type task --status approved --priority low --tags "techdebt,duplication"
```

Filing verbs at a glance:

- **`aida findings add`** — an observation to triage later (the default
  for "noticed, not urgent"). Recurrence ≥ 3 is the promote signal.
- **`aida add --type task`** — concrete, scoped cleanup the operator
  endorses now.
- **`aida edit <dupe> --status rejected`** — retire a spec-graph
  duplicate, with a `subsumes:`/`linked:` tag pointing at the survivor.

## Two traps to watch

- **Every command in this skill is a real shell line.** No invented
  flags. If you want a scan the CLI doesn't expose, file a TASK — don't
  fake it in the prose.
- **Read-only by default.** This skill *finds and recommends*; it does
  not delete code or flip specs unprompted. Surface the finding, name the
  verb, let the operator (or a follow-up `/aida-implement`) act. Orphan
  and copy-paste scans especially are heuristic — present them as
  "verify this", never "this is dead, removing it".

## Related skills / commands

- `/aida-implement` — drive the actual cleanup against a filed TASK.
- `/aida-compiler-warnings` — the warning-debt sibling of this scan.
- `/aida-capture` — session-end requirement capture; pairs naturally
  with a techdebt sweep at session close.
- `aida findings list` — triage what this scan filed.
- `aida plan helpers <spec>` — the "don't reimplement this" section that
  prevents Scan 1's duplication in the first place.
