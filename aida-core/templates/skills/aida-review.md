---
name: aida-review
description: Drive a PR review to completion — walk the per-spec checklist for the active PR, post pass/partial/fail verdicts, optionally fix-forward mechanical issues, gate on green CI, merge if green, and mark every covered spec Completed. Dual of /aida-pr on the reviewer side. trace:STORY-91 | ai:claude
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
---

# AIDA Review Skill

## Purpose

Codify the reviewer's flow that stabilized across PR-3 through PR-7 so the prompt structure isn't re-derived from scratch each cycle. Pairs with `/aida-pr` on the implementer side and `/aida-code-review` for the orthogonal "exhaustive code-quality audit" surface (which this skill does NOT subsume — `/aida-review` is the PR-merge workflow, `/aida-code-review` is the audit).

## When to Use

Use this skill when:
- You're inside a reviewer-role session whose scope is a PR (e.g. `--owns PR-7`)
- A `Review PR-N: ...` queue item just landed via STORY-66's auto-queue and you're picking it up
- The user says "review PR-7" / "check PR-7" / "merge PR-7" after the implementer has opened it
- Before any `gh pr merge` — this skill catches half-shipped batches (partial verdicts, red CI) the manual flow misses

## Core Philosophy

**A PR merges only when every covered spec passes its acceptance criteria.** A verdict isn't a vibe-check — each spec gets a structured pass/partial/fail with evidence (diff hunks, test names, CI output). Partial counts as "not yet"; the implementer iterates, the reviewer re-runs.

## Workflow

### 1. Identify the PR target

Prefer the active session lease when one exists:

```bash
aida session show              # if --owns PR-7, scope shows PR-7
```

If the active session's scope is `PR-N`, use that. Otherwise accept `--pr N` from the slash-command args. Refuse to proceed without a concrete PR number — never guess.

**Confirm with the user only when the PR was auto-detected** (from the session lease or other heuristics). When `--pr N` was passed explicitly, the user has already decided — skip the prompt and go straight to step 2. The confirm is there to catch a wrong guess, not to second-guess an explicit choice. (TASK-72 polish)

**Flip the review story to In Progress.** STORY-66's auto-queue files a `Review PR-N: ...` Story when /aida-pr runs. Once we've identified the PR target and the user has confirmed (or `--pr N` was explicit), that story belongs to *this* review session — bump it so `aida session show --plan`, `aida queue list`, and the statusline reflect the work as actively underway, not still Approved. Idempotent: a story already In Progress isn't re-edited.

```bash
# Locate the review story by title (STORY-66 uses "Review PR-<N>:" prefix).
# If your project has agreed_ids assigned, both forms resolve.
review_story=$(aida search "Review PR-<N>" --status approved | awk 'NR>2 && $1 ~ /^STORY-/ {print $1; exit}')
if [ -n "$review_story" ]; then
    aida edit "$review_story" --status in-progress
fi
```

If no review story exists (the PR was opened without `/aida-pr` or auto-queue is disabled), this is a silent no-op — the manual review path is unaffected. (BUG-34)

### 2. Generate the per-spec checklist (STORY-67)

```bash
aida review prompt --pr <N> --write .aida/review-prompt-pr-<N>.md
```

`aida review prompt` walks the PR's commit range, extracts every `(REQ-ID)` trailer, loads each spec's acceptance criteria + a diff hint, and emits a structured checklist. Read the generated file; that's the worksheet.

**Dim already-Completed specs — but only when they weren't subject of a commit in this PR.** Before walking the checklist, check each spec's current status AND whether it's the subject of a commit in `<base>..HEAD`:

```bash
# 1. Current status
for spec in <spec-ids-from-checklist>; do
    aida show "$spec" | grep '^Status:'
done

# 2. Is this spec the subject of a commit in this PR's range?
# A commit "subjects" a spec if the spec_id appears in the trailing parens.
git log --pretty=format:%s <base>..HEAD | grep -oE '\([^)]+\)$' | grep -oE '[A-Z]+(-[A-Z0-9_]+)?-[0-9]+' | sort -u
```

A spec gets the **informational** treatment ("STORY-54 [Completed, shipped earlier] — referenced as build dependency") if BOTH:
- its current status is `Completed`, AND
- it does NOT appear in any subject in `<base>..HEAD`.

If the spec IS the subject of a commit in this PR — even if pre-marked Completed by /aida-implement's eager status update — a real PASS / PARTIAL / FAIL verdict is still required. The two-signal check prevents the failure mode where every spec gets marked Completed pre-PR (because /aida-implement does that today; the proper deferral lives behind STORY-86) and the whole checklist degenerates into informational rows. (TASK-72 polish — items 4 & 6)

If `aida review prompt` returns "no specs found" — the PR has commits without `(REQ-ID)` trailers. STOP and ask the user how to attribute the diff before continuing.

### 3. Walk each spec — verdict per item, recorded inline

For each non-informational spec in the checklist, in order:

1. **Read the diff against acceptance criteria** — does each `- [ ]` line in the spec have matching code?
2. **Run the per-spec test plan** — exact commands depend on the spec, but typically `cargo test -p <crate> <test_name>` or a focused subset. Use `cargo test --workspace` only when you can't narrow down.
3. **Record the verdict inline in `.aida/review-prompt-pr-<N>.md`** — the file `aida review prompt --write` generated is your worksheet. Edit it in place, appending a verdict block under each spec's section:
   - ✅ **PASS** — every acceptance bullet covered, tests green, no obvious regression
   - ⚠️ **PARTIAL** — some bullets covered, others missing; name the gap precisely (file + line + which bullet)
   - ❌ **FAIL** — acceptance not met, tests red, or design diverges from the spec

Evidence required for every verdict: a file:line reference for PASS, the missing bullet for PARTIAL, the failing test name + line of the divergent code for FAIL. "Looks good" without a reference is not a verdict.

**Why inline in the review-prompt file?** The file is gitignored (BUG-73's `.aida/*` allow-list) so it survives the session as a per-PR audit record without ever landing in the repo. Step 7's consolidated PR comment is generated by summarizing this file — meaning the walk and the comment never disagree. (TASK-72 polish)

### 4. Adversarial deep-pass — trace:STORY-109

After the per-spec walk produces verdicts (step 3) but BEFORE mechanical fix-forward and the CI gate, run an explicit adversarial sweep over the diff. Single-agent reviews bias toward "verify acceptance"; the deep-pass forces a separate cognitive move — *"how could this break?"* — that catches a category of bugs the spec walk consistently misses. PR-15's `/ultrareview` found three real bugs through exactly this framing after `/aida-review` had already merged.

For each touched file in the PR's diff, work through these four probes **in order**. Each is a separate question — don't blend them into a single "looks fine."

**Probe 1: Cross-reference consistency.** When the same string, identifier, or normalized form is parsed/compared/matched in two places, do those places AGREE on:

- Case-sensitivity (lowercase the input vs lowercase the comparison)
- Whitespace tolerance (trim before compare? both sides?)
- Escape / unescape rules (one side decodes, the other doesn't)
- Prefix / suffix stripping (`strip_prefix("Review ")` vs `to_lowercase().contains("review ")`)

When a check is case-insensitive but the consumer is case-sensitive, the matcher reports "yes" and downstream silently falls back. PR-15's bug_007 (TASK-85) was exactly this pattern.

**Probe 2: Format-spec edge cases.** When the code parses, writes, or compares anything that has a published spec (TOML, JSON, YAML, git refs, env-var quoting, URL encoding, semver), ask: *"what does the FORMAT spec allow that the work-spec didn't mention?"* Common landmines:

- TOML inline comments after a value (`key = "v" # cmt`)
- JSON trailing commas (rejected by spec, accepted by some parsers)
- YAML quoted vs unquoted (`yes` is boolean true; `"yes"` is a string)
- Git refs with `:` / `^` / `~` / `@{...}` operators
- Env vars with quotes, backslash escapes, embedded newlines
- URL-encoded characters in user-supplied IDs

PR-15's bug_002 (TASK-84) was a TOML inline-comment miss.

**Probe 3: Multiplicity (0 / 1 / N).** For every loop, iterator, or "find the matching X" call, ask: *"does this work for 0, 1, AND N inputs?"* Common patterns that fail at multiplicity:

- `--steal` ends THE freshest lease — what if there are 2 stale?
- `find().map()` returns first — silently ignores duplicates
- A "should be unique" invariant enforced nowhere
- Default = empty / None — does the loop still produce a meaningful result, or does it silently degenerate?

PR-15's bug_010 (TASK-81) was N>1 leases vs --steal-ends-one.

**Probe 4: Adversarial framing — "looks safe but isn't."** For the touched code, generate 2-3 inputs that LOOK valid but would break the assumption:

- Empty strings, all-whitespace, only-punctuation
- Mixed case where the code expected one case
- Trailing newline / no trailing newline
- Unicode (accented chars, RTL, combining marks, NFC/NFD)
- Comments inside a config value (TOML/JSON/YAML)
- Symbolic links where the code expected real files
- Concurrent writers to the same file (race on read/modify/write)

If you can construct a plausible breaking input that the test plan didn't cover, that's a ⚠️ PARTIAL — file a follow-up bug rather than passing on the assumption.

**Record findings inline.** Each probe finding goes in the review-prompt worksheet as an additional ⚠️ PARTIAL or ❌ FAIL row against the spec whose code it affects (or a new "deep-pass findings" section when the bug touches code outside any covered spec). Step 7's consolidated PR comment renders these alongside the per-spec verdicts — same structure, different provenance label.

**Time budget.** ~30–60 seconds per touched file. The deep-pass doesn't need to be exhaustive; it needs to be SYSTEMATIC. Drift to "looks fine, moving on" defeats the whole point — the probes exist *because* single-framing reviews already bias that way.

**Composes with `/ultrareview`, doesn't replace it.** This adversarial phase closes part of the depth gap a single-agent review historically has against multi-agent fleets. For high-stakes PRs the user can still run `/ultrareview` afterwards; it brings independent framings and remains the depth ceiling. See `docs/positioning/vs-ultrareview.md`.

### 5. Mechanical fix-forward (small commits on the PR's branch)

If the only blockers are mechanical — `cargo fmt` drift, a `#[cfg(unix)]` test fragility, a typo in a comment, an obvious unwrap that should be `?` — fix them on the PR's branch as small, atomic commits with `[AI:claude] style/fix(...)` messages. Don't fix-forward anything semantic; that's an iteration the implementer should drive.

Examples from PR-7's review cycle:
- `[AI:claude] style: cargo fmt --all` (drift introduced after TASK-57's clean)
- `[AI:claude] test(session): gate USERPROFILE assertion on #[cfg(unix)]` (TASK-62, Windows breakage)

After each mechanical fix, re-run the affected test plan from step 3.

### 6. Verify CI is green

```bash
gh run list --branch <pr-branch> --limit 1 --json status,conclusion,url
# or, to wait:
gh run watch <run-id>
```

Block merge until the latest run is `conclusion: success`. If CI is red:
- Walk the failure log: is it caused by this PR (block) or by an unrelated infra/flake (proceed with explicit user confirmation)?
- If caused by this PR, surface to the user and pause — likely a fix-forward (step 5) is the right move.

### 7. Post a consolidated review comment

Summarize `.aida/review-prompt-pr-<N>.md` (with its inline verdicts from step 3) into one comment on the PR. The review-prompt file is the source of truth; the comment is its public projection. Informational rows (already-Completed specs) get a one-liner; PASS/PARTIAL/FAIL rows get the verdict + evidence pulled from the file.

```markdown
## Review: PR-<N>

| Spec | Verdict | Evidence |
|------|---------|----------|
| BUG-71 | ✅ PASS | `.gitignore:94`, 4 unit tests green |
| TASK-61 | ✅ PASS | `aida-pr.md:57-77`, manual repro of refuse-on-drift |
| BUG-72 | ✅ PASS | `main.rs:11212-11258`, `auto_queue_outcome_constructors_...` green |
| TASK-63 | ⚠️ PARTIAL | acceptance covered, but `parse_session_env` doesn't unquote `'` inside a value when adjacent to `\\` |
| STORY-91 | ✅ PASS | this skill |

**CI**: all green (https://github.com/.../actions/runs/...)
**Recommendation**: merge after the TASK-63 quoting tweak.
```

Post via:

```bash
gh pr comment <N> --body "$(cat <<'EOF'
<body>
EOF
)"
```

### 8. Confirm with the user before merge

Show the verdict table. Ask explicitly: "All green — `gh pr merge <N> --squash`?" The user can:
- **Accept** (proceed to step 9)
- **Request changes** (post a `gh pr review --request-changes` instead; STOP)
- **Cancel** (no merge call)

Never auto-merge — the reviewer's `aida-review` is a workflow accelerant, not a YOLO switch.

### 9. Merge

```bash
gh pr merge <N> --squash --delete-branch=false
```

Keep the branch around so a follow-up `aida session end` on the implementer side still resolves naming cleanly. (Branch deletion is the user's call, not the reviewer's.)

### 10. Mark every covered spec Completed (when not already)

For each `REQ-ID` from the checklist whose verdict was ✅ PASS, mark Completed only if it isn't already (eager-marking by /aida-implement is the norm today, so most will be no-ops — that's fine):

```bash
# Idempotent — `aida edit` with the same status is a no-op
aida edit <REQ-ID> --status completed
```

For ⚠️ PARTIAL or ❌ FAIL: leave the spec In Progress (or move it to a follow-up bug). Don't mark partials Completed — the queue is the truth.

Informational rows (already-Completed AND not the subject of a commit in this PR) are skipped — they're not this PR's responsibility.

If the PR carried a `Review PR-N:` story (from STORY-66's auto-queue), close out its lifecycle now. Step 1's In Progress flip moves it Approved → In Progress; the merge moves it In Progress → Completed and dequeues it atomically:

```bash
aida queue done <review-story-id> --yes
```

**Cancel / FAIL handling.** If the user requested changes (step 8 "Request changes" branch) or you concluded with ❌ FAIL and no merge:

- **Iteration expected** (implementer will push fixes) → leave the review story In Progress. A subsequent `/aida-review` run picks up where it left off; no extra bookkeeping needed. For each FAIL/PARTIAL spec that needs implementer follow-up, surface the **one-verb recovery** (TASK-218) in the review comment rather than the three-command sequence:

  ```bash
  aida queue rework <REQ-ID> --work --resume --reason "<one-line summary of what to fix>"
  # or, equivalent top-level alias:
  aida rework <REQ-ID> --work --resume --reason "..."
  ```

  Resolves to: flip Done → InProgress, re-queue for implementer, relaunch the prior claude session with full context, and capture the review's reason as an audit-trail comment on the spec.

- **Review rejected outright** (PR will be closed without merging) → ask the user explicitly: "Mark the Review PR-N story rejected?" If yes:

  ```bash
  aida edit <review-story-id> --status rejected
  aida queue remove <review-story-id> --yes
  ```

Never silently leave a review story in In Progress when the PR was closed without merge — the next session would see it as still-active work. (BUG-34)

### 11. Hand off + Next steps — trace:TASK-87

After the merge lands, surface a structured `Next steps (recommended order):`
block so the post-merge moment is self-guiding instead of relying on
improvised "want to cut a release?" prompts. Don't auto-execute — the user
picks.

**Detect state first:**

```bash
aida session show 2>/dev/null | awk '/^Session /{print $2; exit}'   # session-id prefix
git describe --tags --abbrev=0 2>/dev/null                           # last release tag
git log $(git describe --tags --abbrev=0 2>/dev/null)..main --oneline | wc -l   # commits since
aida queue list --role implementer 2>/dev/null | head -5             # is there more implementer work?
```

- **>5 commits since last tag, or a major-feature PR just merged** →
  release-ready path
- **Otherwise** → standard "next batch" path

**Glyph convention** (consistent across `/aida-pickup`, `/aida-pr`,
`/aida-review`): `▶` = primary recommended action, `⏵` = alternative path,
`🚪` = stop/exit. Recommendations must be CONCRETE — name the next cluster,
the release script, the session ID.

**Templates:**

*Standard "next batch" path:*

```
✓ PR-<N> merged, <M> specs marked Completed. Review story <STORY-X> closed.

Next steps (recommended order):
  1. ▶ Sync + decide next batch → `cd <project-root>` then `aida pull && cargo build --release` then `aida queue work <EPIC-M>`
  2. ⏵ Cut a patch release first → `make release-patch YES=1`
  3. 🚪 End reviewer session, stop → Ctrl+D + `aida session end <session-id>` from parent shell
```

*Release-ready path (>5 commits since last tag, or PR carried a major feature):*

```
✓ PR-<N> merged. ⚠ <K> commits since v<X.Y.Z> — release-ready.

Next steps:
  1. ▶ Cut release → `make release-patch YES=1` (or `release-minor` for new features)
  2. ⏵ Keep merging the queue first → `aida queue work <EPIC-M>`
  3. 🚪 End reviewer session → Ctrl+D + `aida session end <session-id>` from parent shell
```

Print exactly one block — don't dump both templates.

(Don't call `aida session end` from inside the skill — the user runs it from
outside the worktree so their shell's cwd doesn't go stale.)

## Composes With

- `/aida-pr` (STORY-80) — sister skill on the implementer side; same authoring style
- STORY-67 (`aida review prompt --pr`) — generates the per-spec checklist; `/aida-review` consumes its output
- STORY-66 (auto-queue PR review item) — once `/aida-pr` runs, a `Review PR-N` queue item lands for the reviewer; `/aida-pickup` surfaces it, this skill drives it
- TASK-61 (pre-flight `cargo fmt --check` in `/aida-pr`) — once shipped on the implementer side, fmt drift never reaches review; step 5 has less to fix-forward
- `/aida-code-review` — orthogonal exhaustive audit, NOT a substitute. Run it before a release; run `/aida-review` per PR.

## Modes

- **Default**: walk steps 1–11 in order
- `--pr N`: explicit PR number (override session-lease detection)
- `--merge-only`: skip the per-spec walk (steps 2–4, 7) and jump to CI gate + merge + mark-completed. Use when the user has already reviewed manually and just wants the workflow's bookkeeping.

## Common Failure Modes

- **No active session lease and no `--pr`**: refuse — never guess the PR number from cwd alone.
- **`aida review prompt` returns empty**: PR has no `(REQ-ID)` trailers. STOP and ask the user how to attribute the diff.
- **CI red for an unrelated reason**: don't auto-bypass. Surface to the user; they decide whether to override.
- **Spec's acceptance criteria are vague**: ⚠️ PARTIAL by default and ask the user to either tighten the spec or accept the gap explicitly. Don't let vagueness leak through as PASS.
- **Implementer pushes new commits mid-review**: re-run step 2 (`aida review prompt --pr N` again) so the checklist matches the latest range. Verdicts on stale code are worse than none.
- **PR already merged but new commits arrive on the branch (BUG-88)**: don't treat them as extending the original PR. The commits live on `origin/<branch>` but won't reach `main` without a fresh PR. Verify with `gh pr list --head <branch> --state open` before continuing; if empty + `--state merged` returns a hit, ask the implementer to open a follow-up PR (or cherry-pick onto a new branch off `origin/main`) before reviewing further.
