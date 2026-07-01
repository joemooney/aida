---
name: aida-pr
description: Wrap up the current batch of commits and open a pull request with linked specs and a test plan. Walks `git log <base>..HEAD` to derive REQ-IDs, confirms they're all Done (or Completed), pushes, drafts the PR body in the established batch format, and runs `gh pr create` after user sign-off.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
---

# AIDA PR Skill

## Purpose

Codify the "open the PR for this batch" workflow so the prompt structure isn't re-derived from memory every release. Pairs with `/aida-commit` on the producer side and `/aida-code-review` on the reviewer side.

## When to Use

Use this skill when:
- The current branch has 1+ commits ahead of `main` (or a stacked base) and the user says "open a PR" / "ship this batch" / "wrap it up"
- After `/aida-commit` finishes a final commit and the user wants to send the cluster up for review
- Before invoking `gh pr create` directly — this skill catches half-shipped batches (in-progress REQ-IDs) the manual flow misses

## Core Philosophy

**Every shipped commit links back to a finished requirement.** STORY-86
split the post-implementation lifecycle into two states:

- `Done` — work finished on a branch. This is the expected state for
  every REQ-ID in a fresh PR. `aida queue done` flips here.
- `Completed` — merged to the default branch. `aida pull` auto-bumps
  `Done → Completed` once the referencing commit lands.

A PR composed of commits whose REQ-IDs are still `In Progress` /
`Approved` is a sign the batch isn't actually done — pause and surface
that to the user rather than open a misleading PR. `Done` and
`Completed` are both green-light states for opening the PR.

## Autonomy mode — `$AIDA_ZEN` (STORY-287)

This skill's user-facing prompts carry a `kind:` annotation in an HTML
comment directly above each one:

- `<!-- kind:confirmation -->` — a mechanical yes/no whose default
  (option 1) is obvious: "open this PR?", "grab next item?".
- `<!-- kind:design-fork -->` — a genuine choice between meaningful
  alternatives, where guessing wrong has real cost.

Before surfacing any prompt, check the autonomy mode:

```bash
aida zen status
```

- **`zen`** — *advisor-on-standby* mode, **corroborated** (`aida queue
  work --zen`, or a live `--auto-complete --zen` orchestrator).
  Auto-resolve every `kind:confirmation` prompt to option 1 and proceed,
  printing `↳ zen: auto-resolved "<prompt>" → option 1`. Still surface
  every `kind:design-fork` prompt unchanged.
- **`interactive`** — default mode: surface every prompt, no change.
  `aida zen status` prints `interactive` whenever zen is off *or*
  `AIDA_ZEN=1` is set but its provenance cannot be corroborated — a
  stale / leaked `AIDA_ZEN=1` never silently enables zen (BUG-237).
  Branch off this word, **not** the bare `$AIDA_ZEN` env var. trace:BUG-237

A headless `--no-human` drain (`AIDA_HEADLESS=1`) is the stronger mode and
overrides `--zen`. An un-annotated prompt defaults to `design-fork`
(pause-safe). Author guidance: `docs/aida/discipline/skill-prompt-kinds.md`.
trace:STORY-287

## Workflow

### 1. Determine the base branch

- Default: `main`
- Override: `--base <branch>` (for stacked PRs on a previous batch's branch)
- Print the resolved base so the user can correct it before any state moves

### 2. Walk the commit log

```bash
git log <base>..HEAD --oneline
aida session show --plan 2>/dev/null | grep -iE '^[[:space:]]*batch:' || true
```

For each commit subject, extract the trailing `(REQ-ID)` (e.g. `(STORY-78)`, `(TASK-44)`, `(BUG-67)`). Multiple IDs in one subject (e.g. `(TASK-45/46/47)`) all count. Commits without a REQ-ID are fine for `chore`/`docs` types but should be called out if a `feat`/`fix` is missing one.

**Batch context** (TASK-272). If the second command emitted a `batch:` line,
this branch was built by a `aida queue work --batch NAME` drain — the
implementer accumulated several batch members as commits on one branch
(`/aida-pickup --batch NAME` per member). The commit-log walk already
covers them: every batch member shipped so far is a commit in
`<base>..HEAD`, so step 3 verifies all of them and steps 6/8 cover all of
them automatically. The only batch-specific change is framing — step 8's
title and Summary name the batch instead of an EPIC. Note the batch name
for step 8.

### 3. Verify status on each REQ-ID

```bash
aida show <REQ-ID>            # for each derived id
```

Collect a status table:
- `Completed` — green check (already merged on a previous PR; can ship)
- `Done` — green check (STORY-86: work finished on this branch; expected
  state for fresh batches; will auto-bump to Completed once the PR
  merges and `aida pull` runs)
- `In Progress` / `Approved` — yellow warning, this batch isn't actually done
  *(exception — orchestrated headless drain: see below)*
- `Rejected` — red error, this commit shouldn't be in the batch
- not found — red error, commit references a deleted/typo'd ID

<!-- kind:confirmation -->
If any non-`Done` / non-`Completed` items exist, STOP and report them to the user with the matching commit SHAs. Ask: "ship anyway?" — `--force` (or explicit user confirmation) bypasses; default is to refuse. Option 1 is the safe default (refuse), so under `$AIDA_ZEN` this auto-resolves to STOP-and-report — `--zen` never ships a half-done batch unasked.

**Exception — orchestrated headless drain (TASK-422).** When
`aida orchestrator status` is `orchestrated`, BUG-269's `aida queue done`
gate refuses to flip the spec to Done until an open PR exists — so `In
Progress` is the *expected* state for every covered spec at this point,
and `/aida-pr` IS the command that ships it. Under the corroborated
`orchestrated` mode (never the bare `$AIDA_AUTO_COMPLETE` env var,
following the BUG-233 / BUG-237 pattern used in step 12), treat `In
Progress` as a green-light state for the covered specs and proceed. The
BUG-269 gate still fires on `queue done` after the PR opens (phase 3+
post-merge), which preserves the no-committed-but-unshipped invariant
that BUG-269 enforced. `Rejected` and `not found` stay red errors;
`Approved` stays a warning (an `Approved` spec hasn't been picked up
yet — distinct from `In Progress`, where work is actively underway).
trace:TASK-422

### 4. Pre-flight: cargo fmt --check (Rust only) — trace:TASK-61

Catch format drift here, not on CI. The "format-once-then-drift" pattern (TASK-57 → batch7) wastes a review cycle every time it happens.

```bash
# Detect a Rust workspace: Cargo.toml at the repo root.
test -f Cargo.toml || skip_fmt_check
cargo fmt --all -- --check
```

If `cargo fmt --all -- --check` exits non-zero:

- STOP — do not push, do not run `gh pr create`, do not add comments per step 6
- Report the drifted files (the diff output names them); typical fix is one command:
  ```bash
  cargo fmt --all
  git add -u && git commit -m "[AI:claude] style: cargo fmt --all"
  ```
- Re-run `/aida-pr` once the fmt commit lands

Skip silently for non-Rust projects (no `Cargo.toml` at the repo root). This is a Rust-toolchain check; it's not meaningful for pure-doc / pure-frontend repos.

Bypass: `--skip-fmt-check` for the rare case where drift is intentional (e.g. an in-flight rustfmt config change). Default is to refuse.

### 5. Print the "about to happen" banner — trace:TASK-259

`/aida-pr` performs side effects — per-spec comments, a branch push, the
PR itself, the reviewer-queue story — and the first two land *before*
the step-9 confirm. A first-time user typing `/aida-pr` should never be
surprised by what reaches GitHub. Print a banner that previews the whole
sequence, then hold a short abort window, BEFORE any side effect runs.

**When to print:** after step 4 (the last read-only check) and before
step 6 (`aida comment add`, the first write). Every value the banner
needs is already in hand — the branch from step 1, the commit / file /
LOC counts from step 2, the verified spec IDs + titles from step 3.

**When to skip** the banner entirely (go straight to step 6 — no banner,
no pause) if ANY of these hold:

- `--quiet` is among the skill arguments, or the env var `AIDA_NO_BANNER`
  is set (`AIDA_NO_BANNER=1`). Autonomous flows — `/goal`, STORY-246's
  `aida queue work --auto-complete` orchestrator — print their own phase
  headers and must not stall on an interactive pause.
- stdout is not a TTY (`[ -t 1 ]` is false). Piped or captured output
  shouldn't carry box-drawing or a human-facing countdown — keep logs clean.

**Banner format** — three sections, past → present → future:

```
═══════════════════════════════════════════════════════════════════
▶ /aida-pr — opening pull request for <lead-SPEC>
═══════════════════════════════════════════════════════════════════

  ✓ Completed: <lead-SPEC> — "<title>"
                Branch:  <branch>  (status: Done)
                Commits: <N>  ·  <F> files  ·  +<add> / -<del> LOC
                Specs covered: <SPEC-1>, <SPEC-2>, ...

  ▶ Now I will:
       1. Attach an implementation-summary comment to each covered spec
       2. Push branch `<branch>` + orphan aida-store to origin
       3. Open a pull request against `<base>` with an auto-generated body
       4. Auto-queue a review story for your reviewer role

  ↓ Then you can:
       • aida session end --wait-ci   # block until CI completes
       • aida queue work PR-<n>       # start the review session
       • gh pr merge <n> --squash     # merge once approved
       • aida pull                    # auto-bump the spec to Completed

  (Press Ctrl-C to abort, or wait 3s to continue...)
═══════════════════════════════════════════════════════════════════
```

Fill every `<...>` placeholder with real data — the box rules and glyphs
are literal:

- **✓ Completed** — past tense; what is already done. The lead spec's ID
  + title, the branch name (`status: Done` — verified in step 3), the
  commit count, and the file count + `+add / -del` LOC totals from
  `git diff --shortstat <base>..HEAD`. `Specs covered` lists every
  REQ-ID in the batch (the step-3 set).
- **▶ Now I will** — the side effects this skill is about to perform, in
  execution order: the per-spec comments (step 6), the push (step 7),
  the PR (step 10), the reviewer-queue story (step 11). This list is the
  contract — it must match what the skill actually does. `/aida-pr` does
  **not** transition spec status: step 3 already required every spec to
  be `Done`, so there is deliberately no "mark Done" line.
- **↓ Then you can** — the four next-action commands, in the order the
  user runs them once the PR is open.

**The pause is the safety valve.** After printing the banner, pause ~3
seconds (`sleep 3`) so the user can interrupt (Ctrl-C / Esc) before the
first side effect. If the pause elapses untouched, proceed to step 6.

Keep the emoji glyphs (`▶ ✓ ↓ ═ •`) — they carry the section structure,
and AIDA's CLI output uses them deliberately.

### 6. Attach an implementation summary comment per spec (STORY-81)

For each `Done` / `Completed` REQ-ID derived in step 2, run:

```bash
aida comment add <REQ-ID> "$(cat <<'EOF'
Implemented via PR-N (commit <short-sha>):

- <one-line bullet derived from the matching commit's body>
- <second bullet if commit covers multiple files / behaviors>

Test status: <passing-count>/<total> tests green
Follow-up: <any explicit follow-up note from the commit body or chat output, omit if none>
EOF
)"
```

Mechanically derived — no creative writing required:

- **PR-N** is the eventual PR number; if `gh pr create` hasn't run yet, use the branch name and revise after the PR opens
- **commit short-sha** comes from `git log <base>..HEAD --grep="(REQ-ID)" --pretty=%h`
- **bullets** lift from the commit body's bulleted lines, falling back to `git show --stat` line summaries
- **test status** comes from the latest `cargo test --workspace` run in the session — usually surfaced in the agent's final report
- **follow-up** is optional; only include when the commit body explicitly notes one (e.g. "tracked separately as BUG-NN")

Skip the comment for trivial fixes whose entire commit body is one line (typo, doc bump, lint) — the commit subject is the whole context.

Once the PR opens (step 10) and the URL is known, revise the comments to include the actual PR URL via `aida comment edit`. This step is best-effort — if the user cancels before step 10, the comments still survive as useful "implemented via commit <sha>" markers.

### 7. Push code + orphan store

The orphan-branch store changes have to land before the PR is opened — otherwise reviewers see commits referencing requirements they can't `aida show`.

```bash
aida push                     # one-shot: code + orphan store
```

If `aida push` errors with "branch behind main", surface the rebase prompt rather than carry on. (See TASK-54.)

**BUG-88: never claim a push "extends" a PR without verifying state.** Before reporting that a push went to PR-N, confirm PR-N is still open:

```bash
gh pr list --head <branch> --state open --json number
```

If the branch's previous PR has already merged, `aida push` warns and prompts before continuing — the commit would land on `origin/<branch>` but never reach `main`. Don't say "Pushed `<sha>` to PR-N" if PR-N is merged; the right action is a follow-up PR (`gh pr create --base main --head <branch>`) or cherry-picking onto a fresh branch off `origin/main`.

### 8. Draft the PR title + body

**Title format** (mirrors PR-3 through PR-6):

```
EPIC-N batch M: <one-line summary>
```

Derive `N` from the dominant `@EPIC-N` chip across the batch's requirements; derive `M` from the branch name (`epic-20-batch5` → `batch 5`); summary is a 3–6 word description of the cluster.

**Batch-context title** (TASK-272). When step 2 found a `batch:` line, the
branch is a `batch:NAME` drain rather than an EPIC batch — title it after
the batch instead:

```
batch:<NAME>: <one-line summary>
```

The Summary paragraph then opens by naming the drain — e.g. "Drains the
`workflow-hint-polish` batch: <SPEC-1>, <SPEC-2>, …" — so a reviewer sees
up front that the PR bundles every batch member shipped on the branch.
The `## Per-spec` section still has one entry per REQ-ID (all batch
members the commit-log walk found); no spec is dropped.

A copy-paste skeleton ships alongside this skill at
`examples/pr-description-template.md` — fill the placeholders, delete the
comments, and pass it to `gh pr create --body-file -`.

**Body sections:**

```markdown
## Summary

<2–3 sentence overview of what the batch achieves end-to-end>

## Per-spec

### <REQ-ID-1>: <title>
<1-paragraph body from the matching commit's full message; trim the trailing Co-Authored-By>

### <REQ-ID-2>: <title>
...

## Test plan

- [x] `cargo test --workspace` — <N>/<N> green
- [ ] Manual: <one item per significant spec>
- [ ] <other smoke tests run during development>
```

**Seed the Test plan from each spec's captured verification steps (STORY-698).**
When `aida queue done` closed a spec, the builder may have recorded the
verification steps they actually ran; `aida show <REQ-ID>` prints them under a
**Verification steps** block. For each REQ-ID in the batch, fold any captured
steps into the `## Test plan` list (as `[x]` already-run items) so the PR body
carries the implementation audit trail rather than re-deriving it from memory.
A spec with no captured steps contributes nothing — don't invent items.

### 9. Confirm with the user

<!-- kind:confirmation -->
Show the title and the Summary paragraph. Ask explicitly: "Open this PR?"  The user can:
- Accept (proceed to step 10)
- Edit the title/summary inline (revise and re-confirm)
- Cancel (no `gh pr create` call)

### 10. Open the PR

```bash
gh pr create --base <base> --head <branch> --title "<title>" --body "$(cat <<'EOF'
<body>
EOF
)"
```

Use HEREDOC for the body so markdown formatting and code fences survive intact.

### 11. Auto-queue the review for the reviewer role — trace:STORY-90 BUG-86

Right after `gh pr create` returns the URL (and BEFORE step 12's URL output), file the reviewer story:

```bash
aida pr auto-queue-review
```

This invokes the same logic `aida session end` runs as a backup, but at the moment the agent's context is freshest — PR is just opened, branch is current, commits are in working memory. The command:

- Detects the PR via `gh pr list --head <current-branch>`
- Files a `Review PR-<n>: <title>` story routed to the `reviewer` role
- Adds `implements` relationships from the story to each spec referenced in the commit range
- Is idempotent — re-runs print `ⓘ already exists`, never duplicate-file

**Surface the outcome explicitly — never bury a failure under casual "PR opened" prose.** trace:BUG-86

The command prints one of four shapes. Each MUST be relayed verbatim, with a clear glyph header so the user can tell at a glance whether the reviewer queue actually got an entry:

*Success (✓ filed):*

```
✓ filed STORY-N (covers SPEC-1, SPEC-2, ...) → reviewer queue (PR #<n>)
```

Quote the line verbatim. Step 12's "Next steps" template renders the success path.

*Idempotent re-fire (ⓘ already exists):*

```
ⓘ PR #<n> already has a `Review PR-<n>` story queued — skipping
```

Quote verbatim. Treat the same as success for downstream steps; the reviewer queue is populated.

*By-design skip (ⓘ dim — typically "no PR yet" or "reviewer session shape"):*

```
ⓘ auto-queue: no open PR for branch `<branch>` — reviewer queue not filed
  Re-run manually: `aida pr auto-queue-review --branch <branch>`
```

This is non-fatal but the reviewer queue is empty. Tell the user explicitly: "the auto-queue stepped aside (reason: <quoted>). Re-run with `aida pr auto-queue-review --branch <branch>` after the PR is open / from outside a review session." Don't let this dilute into a vague "fine, moving on" — the user needs to know the reviewer queue is NOT populated.

*Needs-attention failure (⚠ yellow):*

```
⚠ auto-queue: `gh pr list` failed for branch `<branch>` (...) — no reviewer story filed
  Re-run manually: `aida pr auto-queue-review --branch <branch>`
```

The exit code is non-zero on this path. STOP — do not pretend the hand-off succeeded:

1. Tell the user explicitly that step 11 FAILED and the reviewer queue is empty
2. Quote the exact error line + the re-run command
3. The most common causes are `gh` unauthenticated (`gh auth status`), `gh` not on PATH, or a network blip — suggest the user run `gh auth status` first
4. The session-end backup will retry later as a fail-safe, but the user shouldn't depend on that — fixing it now keeps the implementer→reviewer hand-off tight

Step 12's "Next steps" template branches on whether step 11 succeeded — the *auto-queue skipped/failed* variant is for the by-design and needs-attention paths.

### 12. Output the URL + Next steps — trace:TASK-87 trace:TASK-110 trace:TASK-260

<!-- kind:confirmation -->
Print the URL `gh` returned. Then surface a structured next-steps table so
the implementer→reviewer hand-off is explicit rather than improvised. Don't
auto-execute — the user picks.

When zen mode is corroborated (`aida zen status` = `zen`, STORY-287 /
BUG-237) what auto-resolves depends on the mode (`aida orchestrator
status`) — BUG-232:

- **`orchestrated`** — `kind:confirmation`: render the *orchestrator mode*
  table, then auto-take its `⇒` row (exit so the orchestrator continues),
  touching `$AIDA_EXIT_SENTINEL` as its absolute last action (see below).
- **plain `--zen`** (`interactive`) — the PR is now open, which was the
  mechanical step `--zen` auto-resolved on the way here. The genuine
  `kind:design-fork` left — *grab the next queued item or stop* — only the
  standby advisor can answer, and a skill cannot synthesize the final
  Ctrl+D, so render the *plain zen mode* template below and **pause** — do
  not auto-take a row. (Once *stop* is the answer, though, the session-end
  cleanup it implies is mechanical: this session runs `aida session end`
  itself to tear down its worktree + lease before exiting — BUG-500 —
  rather than leaving them for the operator.) There is no orchestrator or
  `$AIDA_EXIT_SENTINEL` under plain `--zen`.
- **default** (no zen) — render the matching table, the user picks.

Never auto-take a `⏏` abort row. Print the one-line `↳ zen:` note only
when a row was actually auto-taken (orchestrated mode).

**Ordering rationale (TASK-110 + TASK-111):** end-implementer comes BEFORE
start-reviewer. The implementer's lease owns the PR/STORY scope; a reviewer
session on the same scope while the implementer lease is held would
conflict (or require `--steal`, which is for stuck-lease recovery, not
normal handoffs). Since TASK-111 shipped, `aida session end` now probes
the PR's CI state and prompts (or waits with `--wait-ci`, skips with
`--skip-ci`) before releasing the lease, so the user no longer has to
sequence `gh run watch` manually — the right move is now just **End
implementer (CI-aware) → Start reviewer**. If CI is red, the End session
refuses by default so fixup commits land in the implementer session
without a lease re-claim.

**Detect state first:**

```bash
aida orchestrator status               # `orchestrated` → corroborated --auto-complete child
aida zen status                        # `zen` → corroborated zen mode (BUG-237)
gh run list --branch <pr-branch> --limit 1 --json status,conclusion 2>/dev/null
aida session show 2>/dev/null | awk '/^Session /{print $2; exit}'   # session-id prefix
```

Combine with step 11's auto-queue outcome (✓ filed / ⓘ already exists /
⚠ skipped).

**Check orchestrator mode first** — it overrides every template below. If
`aida orchestrator status` printed `orchestrated`, `/aida-pr` ran inside a
*corroborated* `aida queue work --auto-complete` session (STORY-246): the
orchestrator owns phases 2-6 and is waiting for this session to exit so it
can continue. Render the *orchestrator mode* template — the manual "End
implementer session / Start review session" rows are wrong under the
orchestrator (it ends the session itself as phase 2 and runs the reviewer
as phase 3). **Do not** key this off the bare `AIDA_AUTO_COMPLETE` env var:
`aida orchestrator status` corroborates it against a live orchestrator run,
so a stray value can't misfire orchestrator mode. trace:TASK-286
trace:BUG-233

**Then check plain zen** — if not `orchestrated` but `aida zen status`
prints `zen` (corroborated — never the bare `$AIDA_ZEN` env var, BUG-237),
`/aida-pr` was reached as the auto-resolved end-of-session step of a plain
`--zen` session (BUG-232). Render the *plain zen mode* template: the PR is
open, and the only fork left — grab next vs stop — is surfaced for the
standby advisor. It overrides the two auto-queue templates below.
trace:BUG-232

**Glyph convention** (consistent across `/aida-pickup`, `/aida-pr`,
`/aida-review`): `▶` = primary recommended action, `⇒` = alternative path,
`⏸` = pause/stop. Recommendations must be CONCRETE — name the PR, the
review story, the session ID. The *orchestrator mode* template uses `⇒`
for its forward move (exit so the orchestrator continues) and the
orchestrator-specific `⏏` (abort the orchestrator chain) — because under
`--auto-complete` the moves differ from the manual menu. trace:BUG-116

**Render multi-option prompts as a table.** When presenting 2+ paths
forward, render as a markdown table with columns Path / What happens / Why.
Use ▶ ⇒ ⏸ glyphs in the Path cell for the primary / alternate / pause
semantics. Emit it as a real GFM markdown table — *not* wrapped in a code
fence — so Claude Code's terminal draws the box-rule grid instead of raw
pipes. The **Why** column is load-bearing: it explains the role / lease /
worktree implication of each path, never just restates the action. Here
the two rows are sequential steps (end the implementer session, *then*
start the reviewer) — the table's row order is the recommended order, and
the second row's Why states the dependency explicitly. Full convention:
`docs/skills-convention.md`.

**Apply the finish-state communication rubric (TASK-359).** `/aida-pr`'s
closing block is finish-state communication — the same six-element rubric
that governs `/aida-pickup` Step 6 applies here. The orchestrator-mode
template below carries all six: a labelled **State snapshot**, the
**deciding factor** when one is in play, an explicit **`→ Next:` line**
naming the user-action (the recommendation in summary form), a per-option
**downstream consequence + reversibility**, an **advise escape** row,
and decoupled coupled decisions (the reviewer-queue story is filed
independently in step 11, not bundled into the exit row). Full rubric:
`docs/aida/discipline/session-discipline.md` § *Finish-state communication
rubric*. Print the State preamble verbatim above the table — silence is
not an acceptable signal, especially on the conclusion surface.

**Templates.** Each shows the State preamble, an optional Deciding-factor
line, prose lead-in lines, the next-steps table, and a *loud closing
block* naming the next user-action — print the State preamble (from
`aida state-snapshot --spec <SPEC-ID>`, emitted verbatim) as
fixed-width text, the lead-ins as normal sentences, the table as a real
GFM markdown table (no surrounding code fence), and the closing block as
its own visually-distinct stanza:

*Orchestrator mode (`aida orchestrator status` = `orchestrated`) — TASK-286:*

Print the State preamble first — generate it with `aida state-snapshot
--spec <SPEC-ID>` (TASK-391) and emit its output verbatim above the table:
one deterministic command, not seven manual lookups (it labels/omits absent
rows itself, never a vague placeholder). Example output:

```
State:
  Spec:    <SPEC-ID>  <title>     (Status: Done)
  Branch:  <branch>   <N> commits ahead of main   pushed
  PR:      #<N> open: <url>
  Drain:   phase 1/6 → phase 2/6 next   orchestrator on
  Tests:   <last cargo test summary>
  Fmt:     <cargo fmt --check summary>
  Plan:    <docs/plans/...md>   |  none
```

Then the Deciding-factor line *only when one is in play* (otherwise omit
entirely):

```
Deciding factor: <one sentence — e.g. "Reviewer story STORY-X covers PR-N;
nothing here blocks merge.">
```

Lead-in:

PR-<N> opened: <url>

Print the `ⓘ` note as a normal line above the table:

ⓘ Under `--auto-complete` the orchestrator now drives phases 2-6 (end
session → wait CI → review → merge → pull → build). Don't run `aida session
end` yourself — that is the orchestrator's phase 2; just exit cleanly.

| Path | What happens | Why |
|------|--------------|-----|
| ⇒ Exit — let the orchestrator continue ← recommended | Interactive: press Ctrl+D. Under `$AIDA_ZEN` / headless: the skill instead runs `touch "$AIDA_EXIT_SENTINEL"` as its absolute last action (see below) | The PR is open and the State snapshot above shows no blocker — exiting hands phases 2-6 (CI → review → merge → pull → build) to the orchestrator. Reversible: a `RequestChanges` verdict halts the chain at phase 3 with the recovery hint. |
| ⊕ Route to the advisor *(include only when the State snapshot or deciding factor flags a risk the implementer cannot grade — omit the whole row otherwise)* | `/aida-advise <SPEC-ID>` — today: copy this block to the advisor seat; STORY-306's tier handles punts automatically | When the State snapshot or deciding factor above flags something the implementer cannot grade (a risk that should hold the PR, a deviation worth a human's eyes), park the call with the advisor instead of releasing the chain. |
| ⏏ Abort the chain | Ctrl+C, then `aida session end <session-id> --force` from the parent shell | Hard-stops the orchestrator — PR-<N> stays open but CI / review / merge will not run. Reversible: re-queue and re-run `aida queue work <SPEC-ID>` later. |

**Graceful exit signal (TASK-329).** A skill cannot synthesize the Ctrl+D
the `⇒` row names. So under `$AIDA_ZEN` (or a headless drain), where this
`confirmation` menu auto-resolves to the `⇒` row, the **absolute last action
of the session** — after the PR is opened and pushed and the reviewer-queue
story is filed (step 11) — is:

```bash
[ -n "${AIDA_EXIT_SENTINEL:-}" ] && touch "$AIDA_EXIT_SENTINEL"
```

The `--auto-complete` orchestrator polls for that sentinel and reaps the
otherwise-idle REPL within ~100ms. Touch it **once, last** — anything done
after the touch races the reap and may be killed mid-flight. Then print the
zen annotation and stop:

```
↳ zen: auto-resolved "next step" → ⇒ Exit — orchestrator will reap in ~100ms
```

In default (non-`$AIDA_ZEN`) interactive mode, do **not** touch the sentinel
— render the table and let the user press Ctrl+D. Full protocol:
`docs/aida/discipline/skill-prompt-kinds.md`.

**Loud closing block — name the next user-action (TASK-359).** After the
table (interactive) or in place of it (auto-resolved zen / headless), the
session's final stanza must be a visually-distinct hand-off that names
the exact action and what happens after it. Don't end on a vague *"Final
state"* listing that omits the user-action — the gap from BUG-245 was
precisely this: the implementer summarized what shipped but never told
the user to press Ctrl+D, and the orchestrator silently waited.

Pick the variant by mode (substitute real PR number, spec IDs, drain
flag — concrete is the point):

  *Interactive (`AIDA_ZEN` unset, `AIDA_HEADLESS` unset) — user-driven exit:*

  ```
  ✓ PR-<N> opened: <url>
  ✓ Implementer session is done — nothing left for this Claude to do.

  → Next: press Ctrl+D to exit. The --auto-complete orchestrator then runs:
    - Phase 2/6  End session         (aida session end)
    - Phase 3/6  Review              (auto-queued story <STORY-X>)
    - Phase 4/6  Merge PR-<N>        (gh pr merge <N> --squash --delete-branch)
    - Phase 5/6  Pull + auto-bump    (<SPEC-IDS> Done → Completed)
    - Phase 6/6  Build verify        (cargo build --release)
  ```

  *Under `$AIDA_ZEN` or `--no-human=both` (skill auto-touches the sentinel) — no user-action required:*

  ```
  ✓ PR-<N> opened: <url>
  ✓ Implementer session is done — nothing left for this Claude to do.

  → Next: session will auto-exit via $AIDA_EXIT_SENTINEL; nothing else needed.
    The orchestrator reaps this REPL within ~100ms and continues with
    phases 2-6 (end → review → merge → pull → build).
  ```

The closing block fires **only** in orchestrator mode (`aida orchestrator
status` = `orchestrated`). Plain `--zen` (without `--auto-complete`)
renders its own template below; the auto-queue and ⚠ recovery templates
below carry their own simpler hand-offs since there is no orchestrator
chain to advance. trace:TASK-359 trace:TASK-393

Render this block instead of the three below whenever `aida orchestrator
status` is `orchestrated`. The reviewer-queue story still gets filed (step 11) — the orchestrator
runs the reviewer itself as phase 3, so the queued story is the
manual-recovery fallback if the chain is later aborted. trace:TASK-286
trace:TASK-329

*Plain zen mode (`aida zen status` = `zen`, `aida orchestrator status` = `interactive`) — BUG-232 / STORY-564:*

`/aida-pr` was reached as the auto-resolved end-of-session step of a plain
`--zen` session — opening the PR is the mechanical move `--zen` takes for
you, so the spec never lands committed-but-unshipped. What happens next
depends on whether a **human was actually needed** this session. Ask the
substrate gate — do **not** decide by feel:

```bash
aida zen finish        # prints `auto-exit` or `pause`
```

On a CLEAN finish (`auto-exit`, or a `pause` that fired *only* because of
`--pause-always`), `aida zen finish` also files a **review brief** to the
advisor's mailbox (`.aida/agent-briefs/<advisor>/`) carrying the spec, the
PR number, and the pointer to review against — so the reviewing session
learns about the PR through the substrate, never an operator copy-paste. It
prints `ⓘ review brief filed to the <advisor> mailbox: …` to stderr when it
does. The target is `[zen] review_brief_agent` in `.aida/config.toml`
(default `advisor`; set to `""` to disable). The handoff is idempotent (it
skips when a pending brief for the spec already exists) and fail-open (a
notify hiccup never blocks the finish). You do **not** run `aida brief`
yourself — the gate does it.

- **`auto-exit`** — a CLEAN finish: this session never paused for the
  operator, raised no punt, and `--pause-always` / `[zen] auto_exit = false`
  is not set. There is no decision left for a human, so **do not render the
  table.** Run the session-end cleanup yourself (the *⏸ Stop* mechanic
  below) and exit — this is the BUG-500 friction erased: a clean `--zen`
  finish tears down its own worktree + lease with no operator round-trip.
  Print the lead-in, then run `aida session end` (see the cleanup block
  below) and tell the user the worktree + lease are released:

  PR-<N> opened: <url>. Review story filed (step 11); review brief sent to the advisor mailbox. Clean finish — auto-ending this session (no human needed).

- **`pause`** — a HUMAN-NEEDED finish (this session marked itself
  human-needed via `aida zen needs-human`, or raised a punt, or
  `--pause-always`/`[zen] auto_exit = false` is in force). The operator IS
  needed, so **render the grab-next/stop table and pause** — exactly as
  before. Run `aida queue next` first and drop the `▶` row when the queue
  is empty (the `⏸` row is then the only move). Print the lead-in as normal
  text:

  PR-<N> opened: <url>. Review story filed (step 11).

> **Marking the session human-needed.** The auto-exit decision is only as
> honest as the signal. The instant this `--zen` session pauses to ask the
> standby advisor a real `kind:design-fork` question — or raises a punt —
> run `aida zen needs-human --reason "<one line>"`. That marker (plus the
> punt ledger, which the gate reads directly) is what flips `aida zen
> finish` from `auto-exit` to `pause`. A session that sailed through on
> mechanical auto-resolves only never marks itself, and so auto-exits.

The grab-next/stop table (rendered **only** on `pause`):

| Path | What happens | Why |
|------|--------------|-----|
| ▶ Grab the next queued item | End this session (Ctrl+D), then `aida queue work <NEXT-SPEC>` from the parent shell | PR-<N> now owns this branch — the next spec needs its own branch + worktree, so the implementer lease must be released first |
| ⏸ Stop here | This session runs `aida session end <session-id>` itself (removes the worktree, clears the implementer lease), then you press Ctrl+D | PR-<N> is open and the review story filed — a clean stopping point. `--zen` has no orchestrator to reap the session, so *stopping* means cleaning up now rather than leaving a dangling worktree (`~/ai/aida-<spec>`) + lease for the operator to `aida session end` by hand later (BUG-500) |

**On a clean `auto-exit` finish, or the ⏸ Stop choice, run the cleanup
yourself.** Unlike grab-next (which hands the parent shell a fresh `aida
queue work` that releases this lease by claiming the new scope), stopping
has no follow-on command — so the right move is for this session to do the
teardown before it exits, exactly as the orchestrator does for free in
`--auto-complete` mode. `--zen` has no orchestrator, so without this the
worktree + lease dangle until the operator ends the session by hand
(BUG-500). Resolve the session id
(`aida session show 2>/dev/null | awk '/^Session /{print $2; exit}'`, or
omit the arg entirely and let `aida session end` resolve the sole active
lease) and run: trace:BUG-500

```bash
aida session end <session-id>   # removes the worktree + clears the lease; no-ops gracefully if already gone
```

`aida session end` is CI-aware (TASK-111) — under `--zen` the PR is open
but not yet merged, so it auto-probes CI; pass `--skip-ci` to release the
lease without waiting, or `--wait-ci` to block until green. It self-removes
its own worktree even when run from inside it (BUG-108 / BUG-416), and is
idempotent: if the lease/worktree is already gone it prints
`(no active sessions)` and exits clean. After it returns, tell the user the
worktree + lease are released, then press Ctrl+D. Ending before the PR
merges also sidesteps the TASK-721 merged-worktree-can't-delete papercut.
Do **NOT** run `aida session end` on the ▶ grab-next row — that lease is
released by the *next* `aida queue work` claiming the new scope, not torn
down here.

Do **not** touch `$AIDA_EXIT_SENTINEL` here — that sentinel is the
orchestrator-mode handoff only, and there is no orchestrator under plain
`--zen`. On a `pause`, do **not** auto-take the ▶ grab-next row —
grab-next-vs-stop is a real choice only the standby advisor answers; but
once *stop* is chosen (or `aida zen finish` returned `auto-exit`), the
session-end cleanup is mechanical, so you run it yourself rather than
deferring it to the operator. trace:BUG-232 trace:BUG-500 trace:STORY-564

*Auto-queue succeeded (✓ filed or ⓘ already exists):*

PR-<N> opened: <url>
<STORY-X> filed as review story; reviewer queue has it at head.

| Path | What happens | Why |
|------|--------------|-----|
| ▶ End implementer session (CI-aware) | Ctrl+D, then `aida session end <session-id>` from the parent shell | Releases the implementer lease; auto-probes CI and refuses if red so fixups land here without re-claiming the lease — `--wait-ci` blocks until green, `--skip-ci` releases now |
| ⇒ Start review session | From the parent shell: `aida queue work <STORY-X>` (or `aida queue work PR-<N>`) | Reviewer role on the PR scope — needs the implementer lease released first (the ▶ row) or the two leases conflict |

*Auto-queue skipped/failed (⚠ outcome from step 11):*

PR-<N> opened: <url>
⚠ Auto-queue review didn't fire (gh unauthenticated or PATH-broken).

| Path | What happens | Why |
|------|--------------|-----|
| ▶ End implementer session (CI-aware) | Ctrl+D, then `aida session end <session-id>` from the parent shell | Releases the implementer lease; probes CI — pass `--wait-ci` / `--skip-ci` as needed |
| ⇒ Open reviewer manually (or merge inline) | From the parent shell: `eval "$(aida role enter reviewer --owns PR-<N>)"` + `/aida-review --pr <N>` — or `gh pr merge <N> --squash` if you're the sole reviewer | Auto-queue filed no review story, so the reviewer hand-off is manual; still needs the implementer lease released first |

Print exactly one block — don't dump all four templates.

## Composes With

- `/aida-commit` — commit first, then PR. Skill chain: commit → pr.
- `aida pr ship` — collapses the post-PR sequence (watch CI → squash-merge
  → `aida pull` → `aida session end`) into one command. STORY-439 adds
  `aida pr ship --complexity {low|med|high}` for the ship-side capture
  of the implementer's self-assessed actual complexity; the punt count
  is read automatically from `.aida/punts.jsonl`. Both feed the
  three-way calibration view (`aida autonomy calibration mismatches`).
  Best-effort, not graded — never a merge gate.
- `/aida-code-review` — sister skill on the reviewer side; opens automatically once `aida pr auto-queue-review` (step 11) fires.
- STORY-66 / STORY-90 (auto-queue PR for reviewer) — primary trigger is step 11 here; `aida session end` re-fires the same logic as an idempotent backup so a forgotten /aida-pr (or a raw `gh pr create`) still ends up routed to the reviewer.
- BUG-74 — gh detection uses an explicit PATH walk + absolute-path fallback so the auto-queue isn't fooled by a stripped child-process PATH. `AIDA_DEBUG_GH=1` prints the search trace when gh ends up not found.

## Common Failure Modes

<!-- kind:design-fork -->
The recovery prompts below (half-shipped batch, REQ-ID typo) are
`kind:design-fork`: each is a genuine choice with real cost. Surface them
even under `$AIDA_ZEN` — advisor-on-standby still answers the real
questions; only mechanical confirmations auto-resolve.

- **No base divergence**: `git log <base>..HEAD` is empty. Either you're on `main` or no commits have landed yet — report and exit.
- **Stale local branch**: remote has commits we don't. Surface a `git pull --rebase` prompt before pushing.
- **Half-shipped batch**: one of the REQ-IDs is still `In Progress`. Report which commit references it and ask the user to either `aida edit <id> --status completed` first or drop the commit. If the spec needs another round of work (reviewer found gaps, CI red, etc.), `aida queue rework <id> --work --resume` (TASK-218) is the one-verb recovery — flips status back to InProgress, re-queues for implementer, and chains the session relaunch.
- **REQ-ID typo**: `aida show` returns "not found". Report the commit SHA and the bad ID; ask the user to amend the commit or file the missing requirement.
- **`cargo fmt --all --check` drift (TASK-61)**: refuse the PR and walk the user through `cargo fmt --all` + commit. Don't push the drifted code so CI doesn't have to catch it.

## Ship-time checklist

- Shipped a new CLI slice verb in this batch? Update its parent skill to call it (no re-impl) — see `docs/aida/discipline/skill-cli-symmetry.md`. A skill that re-derives a verb's logic in prose is a second copy that drifts the moment the verb changes; the symmetry is cheapest to honour in the same PR. <!-- trace:TASK-736 -->
