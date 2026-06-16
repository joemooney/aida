---
name: aida-review
description: Drive a PR review to completion — walk the per-spec checklist for the active PR, post pass/partial/fail verdicts, optionally fix-forward mechanical issues, gate on green CI, merge if green, and mark every covered spec Completed. Dual of /aida-pr on the reviewer side. trace:STORY-91 | ai:claude
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
---
<!-- AIDA Generated: v2.0.0 | checksum:c9071ed6 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


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

## Autonomy mode — `$AIDA_ZEN` (STORY-287)

This skill's user-facing prompts carry a `kind:` annotation in an HTML
comment directly above each one:

- `<!-- kind:confirmation -->` — a mechanical yes/no whose default
  (option 1) is obvious: "is PR-N the target?", "merge?".
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
overrides `--zen` — see the *Headless mode contract* below for the full
ordering invariant and the AskUserQuestion ban. An un-annotated prompt
defaults to `design-fork` (pause-safe). Author guidance:
`docs/aida/discipline/skill-prompt-kinds.md`. trace:STORY-287

## Headless mode contract (`AIDA_HEADLESS=1`) — trace:BUG-280

When `AIDA_HEADLESS=1` is set, four invariants override the default
workflow. The reviewer is running unattended (`claude -p`) with no human
to catch a misstep mid-flight; the contract makes the load-bearing
ordering explicit so the model cannot drift past it.

1. **The verdict file is the first irreversible step.** Step 6a writes
   `.aida/review-verdicts/PR-N.json` BEFORE any other irreversible action
   — before the PR comment, before any merge attempt, before exit. The
   file is the orchestrator's phase-3 → phase-4 handshake artifact; the
   PR comment is its human-facing projection. Posting the comment first
   and then crashing is the BUG-280 failure mode: the orchestrator sees
   no verdict file and stops at phase 3 while a public PASS comment
   already sits on the PR.

2. **AskUserQuestion is forbidden.** Every `kind:confirmation` prompt
   auto-resolves to the verdict-file's default value with no AskUserQuestion
   call. Calling AskUserQuestion under `--no-human=both` is permission-denied
   at the harness layer and crashes the session ~10s in — so the prohibition
   is both a contract and a survival rule. `kind:design-fork` prompts that
   would survive in `--zen` mode are also auto-resolved to the verdict-file
   default under headless (specifically: a design-fork during review routes
   through the verdict — write `RequestChanges` and exit — not an
   AskUserQuestion).

3. **The PR comment posts AFTER the verdict file.** Step 7 fires only
   after step 6a has written the verdict file. If the comment post fails
   (network blip, gh auth flake), the verdict file is already on disk and
   the orchestrator can still advance — the comment is a nice-to-have, not
   a load-bearing artifact.

4. **The reviewer never merges under headless.** Steps 8–11 (merge
   confirm, `gh pr merge`, mark Completed, hand-off) are skipped entirely.
   When `aida orchestrator status` = `orchestrated`, phase 4 reads the
   verdict file and performs the merge. When standalone-headless (no
   orchestrator), the verdict file is written and the reviewer exits;
   a human merges later. Either way, the reviewer's job under headless
   ends at "verdict file on disk + comment posted + sentinel touched."

The interactive workflow still walks steps 1–11 in order; this contract
only takes effect when `AIDA_HEADLESS=1`.

## Fix-forward policy — trace:TASK-333 | ai:claude

The reviewer may **fix-forward** — push a small corrective commit on the PR's
branch instead of returning RequestChanges — only under a tightly scoped,
doc-only allowance. Two risks shape the policy:

1. **CI desync.** A commit pushed after phase-2 CI has finished means the
   merged HEAD differs from what CI validated. For a logic change this is
   unsafe; for a doc/comment change it is harmless (a doc edit cannot change
   build/test behavior). The reviewer must still re-verify
   `mergeStateStatus: CLEAN` on the fix-forward commit before Approving — see
   the procedure below.
2. **Self-grading.** The reviewer writes the fix-forward commit and then
   approves it. The implementer/reviewer separation that the workflow rests
   on is bent. A `kind:reviewer-fix-forward` finding (STORY-285) restores an
   independent checkpoint after the fact, without halting the drain — the
   advisor sees and grades the reviewer's self-applied commit post-hoc.

The policy applies in **both interactive and headless** review. The self-
grading risk is mode-independent; the finding files in either mode.

### When fix-forward is PERMITTED — doc-only, no behavior delta

A change qualifies *only* when it **cannot affect build or test behavior**.
Concrete allowances:

- Documentation prose: `README.md`, files under `docs/`, the prose body of a
  markdown skill template
- Code comments and doc-comments (`//`, `///`, `#`, `"""`) — the prose, not
  the symbol they describe
- Output-example accuracy: a code fence in docs / a skill / a comment that
  shows the literal output of a command and the actual output has drifted
- Typos in any of the above
- `cargo fmt` whitespace drift — purely formatting, no semantic delta

### When fix-forward is FORBIDDEN — return RequestChanges instead

Any change that *can* affect build or test behavior — no exceptions, no
judgment calls. The discriminator is **"would re-running `cargo check` /
`cargo test` reach a different result?"** If the answer is yes or maybe, it
is forbidden. Concrete categories:

- Function bodies, signatures, attributes (`#[cfg(...)]`, `#[test]`,
  `#[derive(...)]`, `#[allow(...)]`)
- Type, struct, enum, or trait definitions — even renaming a field touches
  consumers
- Dependency additions / removals / version bumps (`Cargo.toml`,
  `Cargo.lock`, `package.json`, etc.)
- Control flow, error handling, return types, or anything inside `unsafe`
- Tests (adding, removing, renaming, or changing the body) — a test IS
  executable behavior; its outcome determines whether CI passes
- Build scripts (`build.rs`), `Makefile` recipes, CI workflow YAML, git
  hooks, shell scripts the project executes
- Runtime configuration the binary reads (`.aida/config.toml` schema,
  recognized env-var names)
- The bash blocks inside a skill template — skills execute them, so they
  are behavior, not prose
- An error message string literal — string changes break any test that
  asserts on the message

When in doubt, treat it as forbidden. RequestChanges is reversible (the
implementer iterates); a wrongly-fix-forwarded logic change is not.

### Worked examples — apply the discriminator without ambiguity

| Diff | Verdict | Reason |
|------|---------|--------|
| Fix typo `recieve` → `receive` in a `//` comment | ✅ Fix-forward | Comment text; no behavior delta |
| Fix typo `recieve` → `receive` in a `pub fn recieve(...)` name | ❌ RequestChanges | Identifier change ripples to every caller |
| Update a markdown code fence in `docs/` to show the *actual* current command output | ✅ Fix-forward | Documentation accuracy; no source touched |
| Update an output-asserting test's expected string to match new output | ❌ RequestChanges | The test IS executable behavior; this is a logic decision |
| Add a missing trailing `.` to an error message string literal | ❌ RequestChanges | String change; tests asserting on the message break |
| Re-run `cargo fmt --all` to clear whitespace drift | ✅ Fix-forward | No semantic delta; CI would have flagged it anyway |
| Replace `.unwrap()` with `?` propagation | ❌ RequestChanges | Logic change — control flow now early-returns on Err |
| Reword the `## Description` prose at the top of a skill template | ✅ Fix-forward | Skill prose is documentation |
| Edit a `bash` block inside a skill template that the reviewer follows | ❌ RequestChanges | Skill bash blocks are behavior — a reviewer following the template runs them |
| Bump a doc link from `v0.5.1` to `v0.5.2` (no code change) | ✅ Fix-forward | Documentation accuracy |
| Gate a flaky test on `#[cfg(unix)]` | ❌ RequestChanges | Test attributes change which tests run on which platform — behavior |

### Procedure — when fix-forward is permitted

1. **Commit on the PR's branch** with a small, atomic, single-purpose message
   in the project's commit format — typically `[AI:claude] docs(scope):
   <description> (PR-<N>)` or `[AI:claude] style: cargo fmt --all`. One
   commit per logical fix; do not bundle.
2. **Push and wait for CI CLEAN.** GitHub re-runs the workflow on the new
   HEAD. Do **not** proceed to the Approved verdict until:

   ```bash
   gh pr view <N> --json mergeStateStatus --jq .mergeStateStatus
   ```

   returns `CLEAN`. If checks are still pending, wait (`gh run watch
   <run-id>`). If checks went red, the fix-forward made things worse —
   revert it (`git revert <sha> && git push`) and switch to RequestChanges.
3. **Record the fix-forward in the consolidated review comment** (step 7) as
   a dedicated section under the verdict table, naming the commit SHA + a
   one-line summary per fix-forward commit:

   ```markdown
   **Fix-forwards** (reviewer-applied, doc-only — `mergeStateStatus: CLEAN` confirmed):
   - `<short-sha>` — Fix typo `recieve` → `receive` in `aida-cli/src/main.rs:1234` comment
   ```

4. **File a `kind:reviewer-fix-forward` finding.** Required in **both**
   interactive and headless mode — self-grading deserves the same
   independent checkpoint either way. The finding rides STORY-285's
   `from-review:PR-N` surface (the same surface step 7b uses):

   ```bash
   aida add --type task --status draft \
     --tags "from-review:PR-<N>,kind:reviewer-fix-forward,severity:cosmetic" \
     --title "<one-line summary of the fix-forward>" \
     --description-stdin <<'EOF'
   Reviewer fix-forwarded a doc-only correction during review of PR-<N>.

   Commit: <full-sha>
   Files:  <file:line[, file:line, ...]>
   Change: <what changed and why — typo, output drift, prose clarity, fmt drift>

   Verdict: Approved (mergeStateStatus CLEAN confirmed on the fix-forward
   commit before approval).
   PR: <PR URL>
   EOF
   ```

   - **Reuse the idempotency probe from step 7b** — a re-review of the same
     PR must not double-file. Skip the `aida add` call when
     `aida list --tags "from-review:PR-<N>,kind:reviewer-fix-forward" --all`
     already names this commit's SHA in the description body.
   - **STORY-285 gate.** If STORY-285's findings surface is not yet shipped
     in this project (`aida findings list` returns a not-implemented error),
     stub the finding by appending a `Fix-forward filing pending:` bullet to
     the consolidated review comment naming the commit SHA + a one-line
     reason, so a human can replay it after STORY-285 lands.
5. **Verdict stays Approved.** The drain continues; the finding is the
   lightweight after-the-fact review of the reviewer's own commit. Do not
   invent an `Approved-with-fixforward` verdict tier — the verdict file
   (step 6a) still routes through the existing `Approved` /
   `RequestChanges` / `Rejected` enum.

### What this policy is NOT

- Not a verdict tier — no `Approved-with-fixforward`. The verdict is
  `Approved`; the fix-forward is recorded in the comment + a finding.
- Not an excuse to skip Step 4's adversarial deep-pass — fix-forward
  addresses misses the reviewer is willing to fix mechanically, not gaps
  that need the implementer's judgment.
- Not retroactive — once the PR is merged, the policy is closed for that
  PR; any new observation routes through a follow-up issue.
- Not a way to fix-forward across PRs — the fix-forward commit lands on
  the PR's branch only, never on `main` or another PR's branch.

## Workflow

### 0a. Delegated review mode (SPIKE-37) — trace:SPIKE-37 | ai:antigravity

When AIDA is configured for delegated review, the reviewer phase is delegated to Claude Code's remote team/enterprise review pipeline rather than being executed locally.

1. **Gate on Configured Mode**:
   Check the review mode from the workspace's `.aida/config.toml`:
   - If `[review] mode = "delegated"`: proceed with delegated review.
   - If `[review] mode = "local"` (or omitted): fall back to the standard local reviewer workflow (don't break ZDR users, avoiding billing surprises).

2. **Trigger Remote Review**:
   If running in delegated mode, or if the `--delegated` flag is explicitly passed to the skill, trigger the remote review by posting a PR comment:
   ```bash
   gh pr comment <PR> --body "@claude review once"
   ```

3. **Verdict Polling and Parsing**:
   - Resolve the current head SHA of the branch.
   - Poll GitHub check-runs for the head SHA:
     ```bash
     gh api repos/:owner/:repo/commits/<SHA>/check-runs
     ```
   - Search the check-run details for the `bughunter-severity:` JSON block (e.g. `bughunter-severity: {"critical": 0, "normal": 0, "cosmetic": 0}`).
   - If `critical > 0` or `normal > 0`, the verdict is `RequestChanges`. Otherwise, it is `Approved`.

4. **Fail-safe to NeedsAttention**:
   - If the severity JSON is missing, malformed, or the polling times out (default 10 minutes), trigger the fail-safe.
   - File a `ReviewerVerdictUnavailable` finding by adding a task to the spec graph to park the spec in `NeedsAttention` for manual operator triage.

### 0. Pre-flight — PR state check (early-exit on merged/closed) — trace:TASK-227

Before any spec resolution or `aida review prompt` invocation, probe the PR's
current state and short-circuit the no-op paths. Zombie fires (scheduled-wakeup
fallbacks that survive the protected event — see TASK-228) and habit-driven
re-invocations are the common triggers; the prelude turns "enter, scan all
specs, recognize merged, no-op" into "enter, recognize merged, exit."

The PR number comes from `--pr N` (explicit) or the session lease (`aida session show` → scope `PR-N`). If neither is available, fall through to Step 1, which refuses to proceed without a concrete PR number.

```bash
# Probe state — gh detection follows BUG-74's PATH-walk + absolute-path
# fallback so a stripped child-process PATH doesn't fool us.
pr_state=$(gh pr view <N> --json state --jq .state 2>/dev/null)
```

Branch on the result:

- **`MERGED`** → exit 0 immediately with a clear log line:

  ```
  ✓ PR-<N> already merged — nothing to review. Exiting.
  ```

  If a `Review PR-<N>: ...` story is still in `Approved` or `In Progress`, mention it and suggest the cleanup: `aida edit STORY-<X> --status completed && aida queue remove STORY-<X>`. Then exit. Don't load the prompt, don't walk specs.

- **`CLOSED`** (closed without merge) → exit 0 with a different message:

  ```
  ⓘ PR-<N> closed without merge — no review needed.
    Review story <STORY-X> can be marked completed/rejected as appropriate:
      aida edit <STORY-X> --status rejected
      aida queue remove <STORY-X>
  ```

  Same rationale — there's nothing actionable here. Exit.

- **`DRAFT`** → warn but proceed:

  ```
  ⚠ PR-<N> is a draft — deep review may be premature. Proceeding anyway since the user explicitly invoked /aida-review.
  ```

  Continue to Step 1. The reviewer may be doing an early read-through.

- **`OPEN`** → silent pass-through, continue to Step 1.

- **`gh pr view` failed** (gh unauthenticated, network blip, or PR number wrong) → don't block the skill; surface the error inline and continue to Step 1, which will catch a bad PR number on its own. The pre-flight is a fast-path optimization, not a hard gate.

**Why before Step 1?** Step 1 confirms the PR target with the user when auto-detected — a useless prompt if we already know it's merged. The pre-flight runs first so the merged-PR case never gets that far.

### 1. Identify the PR target

Prefer the active session lease when one exists:

```bash
aida session show              # if --owns PR-7, scope shows PR-7
```

If the active session's scope is `PR-N`, use that. Otherwise accept `--pr N` from the slash-command args. Refuse to proceed without a concrete PR number — never guess.

<!-- kind:confirmation -->
**Confirm with the user only when the PR was auto-detected** (from the session lease or other heuristics). When `--pr N` was passed explicitly, the user has already decided — skip the prompt and go straight to step 2. The confirm is there to catch a wrong guess, not to second-guess an explicit choice. (TASK-72 polish) Under `$AIDA_ZEN` this `kind:confirmation` prompt also auto-resolves — take the detected PR and proceed.

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

<!-- kind:design-fork -->
If `aida review prompt` returns "no specs found" — the PR has commits without `(REQ-ID)` trailers. STOP and ask the user how to attribute the diff before continuing. This is a `kind:design-fork` — surface it even under `$AIDA_ZEN`; attribution is a real judgement call, not a mechanical confirmation.

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

### 5. Fix-forward (only under the doc-only policy) — trace:TASK-333 | ai:claude

Re-read the *Fix-forward policy* section near the top of this skill. The
discriminator is **"would re-running `cargo check` / `cargo test` reach a
different result?"** — if yes or maybe, the change is FORBIDDEN: skip step 5
and return RequestChanges through step 6a's verdict file. The worked-examples
table in the policy section is the reference; consult it on every borderline
case rather than improvising.

When a blocker qualifies under the policy (doc prose, a comment typo, an
output-example that drifted, `cargo fmt` whitespace), execute the
**Procedure** in the policy section in order:

1. Commit small, atomic, single-purpose
2. Push and wait for `gh pr view <N> --json mergeStateStatus --jq .mergeStateStatus` to return `CLEAN` — never approve on pending or red CI
3. Record each fix-forward commit (SHA + one-line summary) in the
   consolidated review comment (step 7) under a dedicated **Fix-forwards**
   bullet list
4. File a `kind:reviewer-fix-forward` finding (interactive and headless)
5. Verdict stays Approved — the finding is the after-the-fact independent
   checkpoint on the reviewer's self-graded commit

After every fix-forward commit, re-run the affected test plan from step 3.
If any verdict downgrades, the policy's *not retroactive* line applies — do
not stack a second fix-forward on top of the first.

**Forbidden examples that look mechanical but are not.** These have shipped
as fix-forwards in past review cycles; the policy now forbids them. If you
encounter one, RequestChanges:

- `[AI:claude] test(session): gate USERPROFILE assertion on #[cfg(unix)]` —
  a `#[cfg(...)]` attribute on a test changes which tests CI runs on which
  platform; that's behavior.
- `[AI:claude] fix: change .unwrap() to ?` — propagation is control flow; a
  panic-on-Err becomes an early-return-on-Err.
- `[AI:claude] fix: add trailing period to error message` — string-literal
  change; tests asserting on the message break.

### 6. Verify CI is green

```bash
gh run list --branch <pr-branch> --limit 1 --json status,conclusion,url
# or, to wait:
gh run watch <run-id>
```

Block merge until the latest run is `conclusion: success`. If CI is red:
- <!-- kind:design-fork --> Walk the failure log: is it caused by this PR (block) or by an unrelated infra/flake (proceed with explicit user confirmation)? Accepting a red CI is risk acceptance — a `kind:design-fork`, surfaced even under `$AIDA_ZEN`.
- If caused by this PR, surface to the user and pause — likely a fix-forward (step 5) is the right move.

### 6a. Write the verdict file — first irreversible step — trace:BUG-280

The verdict file is the load-bearing orchestrator handshake artifact.
Write it **before** posting the PR comment (step 7), before any merge
attempt, before exit. The PR comment in step 7 is the human-facing
surface; the verdict file is the machine-readable handshake. Reversing
the order is the BUG-280 failure mode — a posted PASS comment with no
verdict file leaves the orchestrator stuck at phase 3 while the public
surface looks shipped.

`aida queue work` sets the env var `AIDA_REVIEW_VERDICT_FILE` to the
file's absolute path:

- the `--auto-complete` **orchestrator** points it at the phase-3 → phase-4
  handshake file — the orchestrator reads the verdict to decide whether to
  merge, and owns phases 4-6 (merge, pull, build) itself;
- a **standalone** `aida queue work <PR-N> --role reviewer` points it at
  the same `.aida/review-verdicts/` location so the run leaves a uniform
  artifact and the command can print an end-of-command summary from it
  (BUG-226 — before this, a standalone reviewer exited with no terminal
  trace of pass/fail, cost, or where the artifacts landed).

**Whenever `AIDA_REVIEW_VERDICT_FILE` is set, write the verdict file** —
regardless of orchestrator vs standalone context, regardless of headless
vs interactive. The uniform artifact is the point. trace:BUG-226 | ai:claude

```bash
echo "${AIDA_REVIEW_VERDICT_FILE:-}"   # set → a verdict file is expected here
aida orchestrator status               # `orchestrated` → also STOP before merge
```

- **`AIDA_REVIEW_VERDICT_FILE` empty / unset** → no verdict file expected
  (an `aida` predating BUG-226, or a non-`queue work` entry point). Skip
  to step 7.
- **`AIDA_REVIEW_VERDICT_FILE` set** → write the verdict file (below).

**Derive the verdict** from the per-spec verdicts recorded in step 3:

- every spec ✅ PASS → `Approved`
- any ⚠️ PARTIAL (acceptance not fully met, but fixable) → `RequestChanges`
- any ❌ FAIL (a spec is fundamentally unmet / broken) → `Rejected`

**Stamp the `mode`.** Corroborate orchestrator context with `aida
orchestrator status` — a bare `AIDA_AUTO_COMPLETE` env var is not proof (an
unverifiable stale value misfired both ways before BUG-233; only
`orchestrated` checks the corroboration token against a *live* run):

- `aida orchestrator status` = `orchestrated` → `"mode": "orchestrator-phase-3"`
- anything else → `"mode": "standalone"`

Write the file (create its parent dir first). `comment_url` is intentionally
omitted at this stage — step 7a backfills it once the PR comment has been
posted in step 7:

```bash
mkdir -p "$(dirname "$AIDA_REVIEW_VERDICT_FILE")"
cat > "$AIDA_REVIEW_VERDICT_FILE" <<'EOF'
{"verdict": "Approved", "summary": "<one-line rationale>", "mode": "standalone"}
EOF
```

- `verdict` — exactly `Approved`, `RequestChanges`, or `Rejected`.
- `summary` — a one-line rationale.
- `mode` — `standalone` or `orchestrator-phase-3` (corroborated above); lets
  a consumer tell a one-off review from an orchestrator handshake artifact.
- `comment_url` — filled in by step 7a after step 7 posts the comment.
  Omit here; the orchestrator's `read_verdict_file` does not require it
  (only `verdict` is load-bearing).
- `merge` — **escalation handshake.** Normally omit this field. Set it to
  `escalated-to-human` only when, under a headless `--no-human` drain, the
  *merge* decision turns on something you should not decide unattended —
  `--zen` provenance you cannot corroborate, an irreversible call (a schema
  migration, a release tag), genuine strategic uncertainty. The code review
  still stands: write your real `verdict` (`Approved` if the code passed)
  **and** `"merge": "escalated-to-human"`.
- `implementation_complexity` — **advisory, not graded** (STORY-439).
  The diff-grounded complexity the changes actually demanded, one of
  `low` / `med` / `high`. Captured to
  `.aida/complexity-calibration/<SPEC>.yaml` for the three-way
  calibration view (`aida autonomy calibration mismatches`). The
  reviewer is the most objective of the three measurement points
  (pickup → ship → review) because you see the full diff. Never part
  of the PASS / FAIL decision; the field is omitted on older verdict
  files.
- `complexity_agreement` — **advisory, not graded** (STORY-439). Your
  call on whether the implementer's ship-side complexity estimate
  matched the diff: `matched` / `implementer-underestimated` /
  `implementer-overestimated`. Omit when there was no ship-side
  estimate to compare against — `aida` will derive the field
  mechanically from the pickup/ship slot if you skip it.
- `implementation_effort` — **advisory, not graded** (STORY-451).
  Your effort estimate from the observed diff, one of `15m` / `1h` /
  `4h` / `1d` / `1w`. `1d` means 8 work-hours; `1w` means 5
  work-days / 40 work-hours. Captured to
  `.aida/effort-calibration/<SPEC>.yaml` as the review touchpoint.

Example with the STORY-439 fields filled in (`--no-human=both`,
diff was bigger than the implementer claimed):

```bash
cat > "$AIDA_REVIEW_VERDICT_FILE" <<'EOF'
{
  "verdict": "Approved",
  "summary": "ships cleanly",
  "mode": "orchestrator-phase-3",
  "implementation_complexity": "high",
  "complexity_agreement": "implementer-underestimated",
  "implementation_effort": "1d"
}
EOF
```

**Escalating the merge decision.** Escalating is the honest move when you
would otherwise be *guessing* whether to merge — it is distinct from
`RequestChanges` (the code itself needs work) and from a crash. A merge
escalation **still writes the verdict file**: the orchestrator's phase-3
handshake artifact must always exist, so an escalation is never mistaken
for a crashed reviewer that wrote nothing. The orchestrator then stops
cleanly — no merge, exit `0`, *not* a failure — and leaves the PR for a
human to merge. Example:

```bash
cat > "$AIDA_REVIEW_VERDICT_FILE" <<'EOF'
{"verdict": "Approved", "merge": "escalated-to-human", "summary": "code passes, but the migration in this PR is irreversible — a human should own the merge", "mode": "orchestrator-phase-3"}
EOF
```

The orchestrator reads this file after the session exits: `Approved` → it
merges; `merge: escalated-to-human` → it stops cleanly at phase 3 (exit `0`,
no merge, the PR left for a human); anything else → it stops at phase 3 with
exit code 3 and prints the recovery hint. A standalone run's `aida queue
work` reads the same file to print its end-of-command summary (`verdict` +
`comment_url` + the artifact paths); `--quiet` suppresses that summary.

### 7. Post a consolidated review comment

Summarize `.aida/review-prompt-pr-<N>.md` (with its inline verdicts from step 3) into one comment on the PR. The review-prompt file is the source of truth; the comment is its public projection. Informational rows (already-Completed specs) get a one-liner; PASS/PARTIAL/FAIL rows get the verdict + evidence pulled from the file.

**Step 6a must have run first when `AIDA_REVIEW_VERDICT_FILE` is set.** The
verdict file is the load-bearing artifact; the comment posted here is its
human projection. trace:BUG-280

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

Post via — `gh pr comment` prints the comment URL on stdout; capture it
into `COMMENT_URL` so step 7a can backfill `comment_url` into the verdict
file:

```bash
COMMENT_URL=$(gh pr comment <N> --body "$(cat <<'EOF'
<body>
EOF
)") || COMMENT_URL=""
```

`COMMENT_URL` is best-effort — if `gh pr comment` fails (network, gh auth),
leave it empty and step 7a skips the backfill. The verdict file written
in step 6a still satisfies the orchestrator handshake without
`comment_url`. trace:BUG-280

### 7a. Backfill the verdict file with `comment_url`; STOP if orchestrated/headless — trace:BUG-280

Step 6a wrote the verdict file before the PR comment posted, so the file's
`comment_url` field is empty. Now that step 7 has posted the comment, capture
the URL and re-write the verdict file to include it. The re-write is safe —
the orchestrator only reads the file after the session exits.

```bash
if [ -n "${AIDA_REVIEW_VERDICT_FILE:-}" ] && [ -f "$AIDA_REVIEW_VERDICT_FILE" ]; then
  # COMMENT_URL holds the URL captured from `gh pr comment` in step 7.
  # Re-emit the JSON keeping every field 6a wrote (verdict, summary, mode,
  # optional merge), adding "comment_url". Skip the rewrite if no URL was
  # captured (e.g. --merge-only, or the comment post failed).
  :
fi
```

If step 7 did not post a comment (e.g. `--merge-only`), or `gh pr comment`
failed, leave the field absent — the orchestrator does not require it (only
`verdict` is load-bearing).

**Orchestrator mode (`aida orchestrator status` = `orchestrated`) — STOP
after the verdict.** Do NOT merge, do NOT mark specs Completed, do NOT do
the hand-off — the orchestrator performs phases 4-6 itself. **Standalone
mode — continue to step 8** (confirm + merge + hand-off) as usual: the
verdict file is just an artifact there, the reviewer still owns the merge
decision. trace:BUG-226 | ai:claude

**Under `AIDA_HEADLESS=1` (standalone or orchestrator) — also STOP.** The
reviewer never merges under headless: AskUserQuestion is forbidden (see the
*Headless mode contract* near the top of this skill), so the merge confirm
in step 8 cannot run, and silently auto-merging is not the contract. The
verdict file is on disk; step 7b files any findings; then the session
exits. A standalone-headless reviewer's verdict sits at
`.aida/review-verdicts/PR-N.json` for a human to act on; an
orchestrator-headless reviewer's verdict is read by phase 4. trace:BUG-280

  **End the session with a loud, explicit exit instruction.** The
  orchestrator cannot advance until this reviewer Claude exits — but nothing
  about writing a verdict file tells the user that. Don't end on a vague
  "Session is done."; the user should never have to *infer* that they should
  now press Ctrl+D. After writing the verdict file, make the final block of
  your message a distinct, visually-loud hand-off that names the exact key
  to press and what happens after. Pick the block by the mode + verdict, and
  substitute the real PR number + covered spec IDs — a concrete
  "PR-57 / BUG-219, STORY-261" is the point; placeholders defeat it.

  *Orchestrator mode (`aida orchestrator status` = `orchestrated`), verdict `Approved`:*

  ```
  ✓ Verdict written: Approved
  ✓ Reviewer session is done — nothing left for this Claude to do.

  ▶ Press Ctrl+D to exit. The --auto-complete orchestrator then runs:
    - Phase 4/6  Merge PR-<N>      (gh pr merge <N> --squash --delete-branch)
    - Phase 5/6  Pull + auto-bump  (<SPEC-IDS> Done → Completed)
    - Phase 6/6  Build verify      (cargo build --release)
  ```

  *Orchestrator mode, verdict `RequestChanges` or `Rejected`:*

  ```
  ⚠ Verdict written: <RequestChanges|Rejected>
  ✓ Reviewer session is done — nothing left for this Claude to do.

  ▶ Press Ctrl+D to exit. The --auto-complete orchestrator then stops at
    phase 3/6 (exit code 3) and surfaces the recovery hint — it will NOT
    merge. The implementer iterates from there.
  ```

  *Standalone headless mode (`AIDA_HEADLESS=1`, `aida orchestrator status` ≠ `orchestrated`) — trace:BUG-280:*

  ```
  ✓ Verdict written: <Approved|RequestChanges|Rejected> → .aida/review-verdicts/PR-<N>.json
  ✓ Reviewer session is done — under --no-human the reviewer does not merge.

  ▶ The session will exit (sentinel touched). A human merges later by reading
    the verdict file, or re-runs `aida queue work PR-<N> --role reviewer`
    interactively to drive the merge.
  ```

  This loud exit block fires in orchestrator mode (`aida orchestrator status` =
  `orchestrated`) **and** in standalone headless mode (`AIDA_HEADLESS=1`,
  orchestrator not corroborated) — both cases skip the interactive
  step 8/11 entirely. In interactive standalone review the reviewer owns the
  merge, so step 11's hand-off table stays the end-of-session surface — don't
  render the Ctrl+D block there. A standalone reviewer always sets
  `AIDA_REVIEW_VERDICT_FILE` (BUG-226), so corroborate the orchestrator branch
  with `aida orchestrator status` rather than the env var alone.
  trace:TASK-291 trace:BUG-226 trace:BUG-280 | ai:claude

  **Under `$AIDA_ZEN` or a headless drain — touch the exit sentinel (TASK-329).**
  A skill cannot synthesize the Ctrl+D the block above names. The
  orchestrator instead reaps the reviewer by polling for a sentinel file. So
  when `$AIDA_EXIT_SENTINEL` is set, once the verdict file is written **and**
  step 7b has finished any `findings_filed` rewrite — as the **absolute last
  action of the session** — run:

  ```bash
  [ -n "${AIDA_EXIT_SENTINEL:-}" ] && touch "$AIDA_EXIT_SENTINEL"
  ```

  The orchestrator reaps the REPL within ~100ms. Touch it **once, last** —
  the verdict and findings must already be on disk, or they race the reap.
  Then print the zen annotation and stop:

  ```
  ↳ zen: auto-resolved "next step" → ⇒ Exit — orchestrator will reap in ~100ms
  ```

  In default interactive mode leave the sentinel untouched and let the user
  press Ctrl+D. Full protocol: `docs/aida/discipline/skill-prompt-kinds.md`.
  trace:TASK-329 | ai:claude

### 7b. File non-blocking findings as draft TASKs (headless drain) — trace:STORY-278

Under a headless `--no-human` drain there is no human to read the
consolidated comment and feed the reviewer's non-blocking findings back to
the advisor role for follow-up filing. Without this step those
follow-ups are lost the moment the drain moves on. So the headless reviewer
files them itself — as draft TASKs the advisor triages later via
`aida findings list`.

**This step runs only when `AIDA_HEADLESS=1`** — the variable AIDA sets
when it launches a headless `claude -p` reviewer. In an interactive review
a human is present and triages straight from the comment; skip 7b entirely.

```bash
echo "${AIDA_HEADLESS:-}"
```

- **Empty / unset** → interactive review. Skip 7b — the human triages.
- **`1`** → headless. Continue.

**What counts as a finding.** A *finding* is a non-blocking follow-up the
review surfaced — a ⚠️ PARTIAL row, or an adversarial deep-pass (step 4)
observation — that did NOT sink the verdict. A ❌ FAIL is a merge *blocker*,
not a finding: it routes through the verdict, not here. If the review found
nothing worth a follow-up, 7b files nothing.

**Idempotency — probe before filing.** A re-review of the same PR must not
double-file. Check for findings already filed against this PR (`--all` is
required — a finding the advisor already promoted/dismissed is
terminal-status and hidden by default):

```bash
existing=$(aida list --tags "from-review:PR-<N>" --all | grep -cE '^[A-Z]+-[0-9]+' || true)
```

If `existing` is non-zero, **skip the rest of 7b** — this PR's findings
were filed on an earlier review.

**File each finding** — one draft TASK apiece:

```bash
aida add --type task --status draft \
  --tags "from-review:PR-<N>,severity:<cosmetic|minor|major>" \
  --title "<one-line summary>" \
  --description-stdin <<'EOF'
<full finding text — what, where (file:line), why it matters>

Raised in review of PR-<N>: <PR or review-comment URL>
EOF
```

- **Always `--type task`** — even a bug-shaped finding. The advisor can
  `aida edit <ID> --type bug` on promote if warranted; the reviewer does
  not expand the taxonomy.
- **`from-review:PR-<N>`** (required) and **`severity:<level>`** (required)
  tags. Add context tags freely (`clippy`, `docs`, `fmt`, …).
- **Severity rubric:** `cosmetic` = fmt/clippy/comment nits; `minor` = a
  real but small bug or gap; `major` = a design concern worth a
  conversation. When unsure, round down — the advisor re-grades on triage.
- Capture each printed `<ID>` (e.g. `TASK-303`).

**Record what was filed in the verdict file.** If step 7a wrote a verdict
file, re-write it now so it also carries the filed IDs — the orchestrator
reads the file only after the session exits, so a re-write here is safe:

```bash
if [ -n "${AIDA_REVIEW_VERDICT_FILE:-}" ] && [ -f "$AIDA_REVIEW_VERDICT_FILE" ]; then
  # Re-emit the JSON with an added "findings_filed" array — keep every
  # field step 7a wrote (verdict, summary, mode, comment_url), e.g.:
  # {"verdict": "...", "summary": "...", "mode": "...", "comment_url": "...",
  #  "findings_filed": ["TASK-303","TASK-304"]}
  :
fi
```

`findings_filed` is for cross-reference only — the orchestrator does not
act on it. An empty array (nothing filed, or an idempotency skip) is fine.

**The advisor picks these up** on its next session: `aida findings list`
surfaces them grouped by PR and severity-sorted, `aida findings promote
<ID>` sends one to the work queue, `aida findings dismiss <ID>` rejects it.
Both accept `--reason "<text>"` so the rationale lands in the audit comment
in one command (TASK-404).

### 8. Confirm with the user before merge

**Headless gate (`AIDA_HEADLESS=1`) — SKIP this step entirely.** The
reviewer never merges under headless (see the *Headless mode contract* near
the top of this skill). Step 7a's STOP block has already fired; the session
exits via the sentinel touch. Step 8's merge confirmation prompt is a
`kind:confirmation` that would call AskUserQuestion, which is forbidden
under `--no-human=both` and crashes the session — BUG-280's exact failure
mode. trace:BUG-280

**Synthesize the overall verdict** from the per-spec verdicts recorded in step 3 — same routing as the 6a verdict file: every covered spec ✅ PASS → *positive*; any ⚠️ PARTIAL or ❌ FAIL → *negative*; no per-spec verdicts reached (e.g. `--merge-only`) → *abstention*.

**If the verdict is positive, record the formal approval before asking about the merge** — see "Approve path" below. Recording it here, *before* the merge question, means `gh reviewDecision` flips to `APPROVED` even when the user defers the merge: TASK-250's State 3 ("reviewed + approved, awaiting merge") reads exactly that signal, so an approved-but-unmerged PR displays correctly instead of falling back to "start review". trace:TASK-278 | ai:claude

<!-- kind:confirmation -->
Show the verdict table. Ask explicitly: "All green — `gh pr merge <N> --squash`?" The user can:
- **Accept** (proceed to step 9)
- **Request changes** (see "Request-changes path" below — comment-only mode is the default; STOP)
- **Cancel** (no merge call — a formal approval, if recorded, still stands; the PR shows as approved-but-unmerged)

Never auto-merge in default mode — the reviewer's `aida-review` is a workflow accelerant, not a YOLO switch. When zen mode is **corroborated** (`aida zen status` prints `zen` — see the Autonomy mode section) this is a `kind:confirmation` prompt and auto-resolves to **Accept** — but note the safety floor: a *non-positive* verdict (any ⚠️ PARTIAL / ❌ FAIL) STOPs at the Request-changes path and never reaches this prompt, so zen only ever auto-merges an all-PASS PR, with the verdict table on screen for the advisor at the keyboard. A bare `AIDA_ZEN=1` with no corroborated provenance prints `interactive` — the merge prompt is surfaced for a human, never auto-resolved (BUG-237). trace:STORY-287 trace:BUG-237

**Approve path** — trace:TASK-278 | ai:claude

Fires when the overall verdict is positive — every covered spec earned a ✅ PASS, with no ⚠️ PARTIAL or ❌ FAIL. Anything less does not approve: abstention and partial/fail verdicts skip this path entirely. The consolidated comment from step 7 is their signal, and a verdict the user decides to act on routes through the Request-changes path below.

Before calling `gh pr review --approve`, check whether the reviewer is the PR author. GitHub blocks `addPullRequestReview` with `APPROVE` on your own pull request — the error `Can not approve your own pull request` is guaranteed in solo-developer / author-is-reviewer scenarios, and surfacing the failure is just noise. (Symmetric with the Request-changes path's own-PR skip — same family of own-author handling.)

Detect own-PR cheaply:

```bash
PR_AUTHOR=$(gh pr view <N> --json author --jq '.author.login' 2>/dev/null)
ME=$(gh api user --jq '.login' 2>/dev/null)
```

Then:

- **If `$PR_AUTHOR` != `$ME` (cross-author PR)**: post the formal approval. This flips `gh reviewDecision` to `APPROVED`, lighting up TASK-250's State 3 display for the approved-but-unmerged window:

  ```bash
  gh pr review <N> --approve --body "<one-line summary, optionally the spec ids covered — e.g. 'All specs PASS: BUG-71, TASK-61, STORY-91'>"
  ```

- **If `$PR_AUTHOR` == `$ME` (own PR)**: SKIP `gh pr review --approve` — solo developers reviewing their own PR can't formally approve it (GitHub blocks self-approval). The consolidated comment from step 7 is the signal. Surface a one-line note so the user understands the skip:

  ```
  ℹ Own-PR: skipped `gh pr review --approve` (GitHub blocks self-approval). The consolidated review comment above is the signal; merge directly with `gh pr merge`.
  ```

- **If `gh` isn't on PATH or either lookup fails**: degrade to comment-only with a note: "ℹ `gh` user lookup unavailable — formal approval review skipped; the comment above is the signal." Don't block the workflow on metadata lookups.

**Request-changes path** — trace:TASK-216 | ai:claude

Before calling `gh pr review --request-changes`, check whether the reviewer is the PR author. GitHub blocks `addPullRequestReview` with `REQUEST_CHANGES` on your own pull request — the error `Can not request changes on your own pull request` is guaranteed in solo-developer / author-is-reviewer scenarios, and surfacing the failure is just noise. The consolidated review comment posted in step 7 already serves the request-changes signal for humans; the formal GitHub review type only matters when branch protection requires a non-author approval (which by definition can't be the author anyway).

Detect own-PR cheaply:

```bash
PR_AUTHOR=$(gh pr view <N> --json author --jq '.author.login' 2>/dev/null)
ME=$(gh api user --jq '.login' 2>/dev/null)
```

Then:

- **If `$PR_AUTHOR` == `$ME` (own PR)**: SKIP `gh pr review --request-changes`. The consolidated comment from step 7 is the request-changes signal. Surface a one-line note so the user understands the skip:

  ```
  ℹ Own-PR: skipped `gh pr review --request-changes` (GitHub blocks it on author-reviewed PRs). The consolidated review comment above is the signal.
  ```

- **If `$PR_AUTHOR` != `$ME` (cross-author PR)**: post the formal review:

  ```bash
  gh pr review <N> --request-changes --body "See the consolidated review comment posted above for the per-spec verdicts."
  ```

- **If `gh` isn't on PATH or either lookup fails**: degrade to comment-only with a note: "ℹ `gh` user lookup unavailable — formal request-changes review skipped; the comment above is the signal." Don't block the workflow on metadata lookups.

In all three paths the consolidated comment from step 7 has already been posted, so the reviewer signal is delivered regardless of which branch runs.

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

- **Review rejected outright** (PR will be closed without merging) → <!-- kind:confirmation --> ask the user explicitly: "Mark the Review PR-N story rejected?" — the rejection decision is already made, so this is a `kind:confirmation` (bookkeeping follow-through); under `$AIDA_ZEN` auto-resolve to yes. If yes:

  ```bash
  aida edit <review-story-id> --status rejected
  aida queue remove <review-story-id>
  ```

Never silently leave a review story in In Progress when the PR was closed without merge — the next session would see it as still-active work. (BUG-34)

### 11. Hand off + Next steps — trace:TASK-87 trace:TASK-110 trace:TASK-260

<!-- kind:confirmation -->
After the merge lands, surface a structured next-steps table so the
post-merge moment is self-guiding instead of relying on improvised "want to
cut a release?" prompts. Don't auto-execute — the user picks.

Under `$AIDA_ZEN` (STORY-287) this menu is a `kind:confirmation` prompt:
still render the table (the scrollback record), then proceed with the
**primary row** (`▶`) automatically instead of waiting for a pick. Never
auto-take a `⏸` row. Print the one-line `↳ zen:` note naming the row taken.

**Ordering rationale (TASK-110):** end-reviewer comes FIRST — the merge has
landed, the review-story scope is closed, and the lease on it is now stale.
Holding the lease while starting the next batch (new implementer session) or
cutting a release would conflict with anything else that wants the same
scope, and the statusline still shows `role:reviewer` even though there's
no reviewer work to do. Release the lease, then make the next move from a
clean shell outside the worktree.

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
`/aida-review`): `▶` = primary recommended action, `⇒` = alternative path,
`⏸` = pause/stop. Recommendations must be CONCRETE — name the next cluster,
the release script, the session ID. trace:BUG-116

**Render multi-option prompts as a table.** When presenting 2+ paths
forward, render as a markdown table with columns Path / What happens / Why.
Use ▶ ⇒ ⏸ glyphs in the Path cell for the primary / alternate / pause
semantics. Emit it as a real GFM markdown table — *not* wrapped in a code
fence — so Claude Code's terminal draws the box-rule grid instead of raw
pipes. The **Why** column is load-bearing: it explains the role / lease /
worktree implication of each path, never just restates the action. The
rows are listed in recommended order — top-to-bottom is the sequence to
follow, with ⏸ the pause alternative. Full convention:
`docs/skills-convention.md`.

**Templates.** Each shows a prose lead-in line followed by the next-steps
table — print the lead-in as normal text, then the table as a real GFM
markdown table (no surrounding code fence):

*Standard "next batch" path:*

✓ PR-<N> merged, <M> specs marked Completed. Review story <STORY-X> closed.

| Path | What happens | Why |
|------|--------------|-----|
| ▶ End reviewer session | Ctrl+D, then `aida session end <session-id>` from the parent shell | The merge landed — the PR-<N>/STORY-<X> scope is closed and the lease is stale; release it before anything else claims the scope |
| ⇒ Sync + decide next batch | From the parent shell: `aida pull && cargo build --release && aida queue work <EPIC-M>` | New scope → new lease + worktree; `aida pull` fast-forwards local main past the merge first |
| ⏸ Stop here, pick up later | Just end the session | The merge is done — no lease held, the next batch can wait |

*Release-ready path (>5 commits since last tag, or PR carried a major feature):*

✓ PR-<N> merged. ⚠ <K> commits since v<X.Y.Z> — release-ready.

| Path | What happens | Why |
|------|--------------|-----|
| ▶ End reviewer session | Ctrl+D, then `aida session end <session-id>` from the parent shell | The merge landed — release the stale PR-<N>/STORY-<X> lease before cutting a release |
| ▶ Sync local main | From the parent shell, after the end: `aida pull` | Fast-forwards local main to the merge commit AND auto-bumps the Done specs it referenced — skip it and `make release-patch` tags the pre-merge HEAD |
| ⇒ Verify auto-bump fired | `aida show <one-of-the-merged-specs>` | Expect Completed; if still Done, `aida db reconcile-status` (TASK-226) or `aida edit <id> --status completed` recovers it |
| ⇒ Cut release | From the parent shell: `make release-patch YES=1` (or `release-minor` for new features) | Tags + pushes; the release workflow then builds and publishes the binary tarballs |
| ⏸ Stop here, cut release later | Just end the session | The merge is done — the tag can wait until you're ready |

Don't skip the "Sync local main" row — local main is behind origin/main immediately after the merge, and `make release-patch` would tag the pre-merge HEAD. trace:BUG-101 | ai:claude

Print exactly one block — don't dump both templates.

(Don't call `aida session end` from inside the skill — the user runs it from
outside the worktree so their shell's cwd doesn't go stale.)

### 11a. Hand the outcome to the advisor via the substrate — close the cut-n-paste loop — trace:STORY-613

When this interactive review finishes with a verdict, flagged findings, or
leftover "want me to X?" questions, the operator should NOT have to cut-n-paste
the outcome to the advisor session. That hand-off is what the **substrate** is
for — the reviewer-side analog of the burn-down's drain→advisor mailbox
hand-off (`/aida-burndown` step 6, TASK-792). Post it once, from here, and the
advisor sees it in its inbox/findings and acts. trace:relates:TASK-792

**This step is for the interactive standalone review.** The headless paths
already feed the advisor through their own channels — skip 11a when either:

```bash
echo "${AIDA_HEADLESS:-}"   # `1` → headless: step 7b already filed findings; the
                            #        orchestrator/advisor consumes the verdict file. Skip 11a.
aida orchestrator status    # `orchestrated` → the orchestrator owns the hand-off. Skip 11a.
```

Otherwise (interactive standalone), do BOTH of the following on finish:

**(1) Post the outcome to the advisor mailbox.** Carry the verdict, any flagged
findings, and any leftover questions in one message. Use `--intent request`
when there is a "want me to X?" question the advisor must decide; use
`--intent fyi` for a clean APPROVE/merge with nothing outstanding:

```bash
aida mailbox send --to advisor --intent request "Review of PR-<N> finished — verdict: <Approved|RequestChanges|Rejected>.

Covered specs: <SPEC-IDS + per-spec PASS/PARTIAL/FAIL>
Flagged findings: <one line each — what, where (file:line), severity; or 'none'>
Leftover questions: <e.g. 'want me to file slice 2 of STORY-X?' / housekeeping; or 'none'>
PR: <PR URL>"
```

- `--intent fyi` instead of `request` when there is **nothing** the advisor must
  decide — a clean merge with no flagged findings and no leftover questions.
  Add `--urgent` only if the verdict surfaced something the advisor must see
  out-of-band (e.g. a `main`-breaking concern).
- Keep the body to the lines above — verdict, covered specs, flagged findings,
  leftover questions, the PR URL. Omit a heading that has nothing under it.
  One message per finished review.

**(2) File actionable items as findings** so they're triageable rather than
trapped in chat. For each non-blocking follow-up the review surfaced (a ⚠️
PARTIAL row, a step-4 deep-pass observation, or a "want me to X?" that is really
a deferrable work item), file a finding the advisor picks up via
`aida findings list`:

```bash
aida findings add --note - --severity <major|minor|cosmetic> --linked-specs <SPEC-ID> \
  --tags "from-review:PR-<N>" <<'EOF'
<full finding text — what, where (file:line), why it matters>

Raised in review of PR-<N>: <PR URL>
EOF
```

- **Idempotency.** A re-review of the same PR must not double-file. Probe first
  and skip the `aida findings add` calls when this PR already has findings:

  ```bash
  existing=$(aida list --tags "from-review:PR-<N>" --all | grep -cE '^[A-Z]+-[0-9]+' || true)
  ```

  If `existing` is non-zero, the findings were filed on an earlier review — post
  the mailbox message (1) but skip the `aida findings add` calls. This shares
  the `from-review:PR-<N>` surface with step 7b's headless filer, so the two
  paths never double-up on the same PR.
- **Severity rubric** (same as step 7b): `cosmetic` = fmt/clippy/comment nits;
  `minor` = a real but small bug or gap; `major` = a design concern worth a
  conversation. When unsure, round down — the advisor re-grades on triage.

**Best-effort.** A failed `mailbox send` or `findings add` must **never**
retroactively fail an otherwise-complete review — note it and continue. The
merge has already landed; the hand-off is the courier-replacement, not a gate.

## Composes With

- `/aida-pr` (STORY-80) — sister skill on the implementer side; same authoring style
- STORY-67 (`aida review prompt --pr`) — generates the per-spec checklist; `/aida-review` consumes its output
- STORY-66 (auto-queue PR review item) — once `/aida-pr` runs, a `Review PR-N` queue item lands for the reviewer; `/aida-pickup` surfaces it, this skill drives it
- TASK-61 (pre-flight `cargo fmt --check` in `/aida-pr`) — once shipped on the implementer side, fmt drift never reaches review; step 5 has less to fix-forward
- `/aida-code-review` — orthogonal exhaustive audit, NOT a substitute. Run it before a release; run `/aida-review` per PR.
- `/aida-burndown` (TASK-792) — sister drain→advisor mailbox hand-off on the burn-down side; step 11a here is its reviewer-side analog (STORY-613). Same substrate-handoff principle: post the outcome, don't make the operator the courier.

## Modes

- **Default**: walk steps 1–11 in order
- `--pr N`: explicit PR number (override session-lease detection)
- `--merge-only`: skip the per-spec walk (steps 2–4, 7) and jump to CI gate + merge + mark-completed. Use when the user has already reviewed manually and just wants the workflow's bookkeeping.

## Common Failure Modes

- **No active session lease and no `--pr`**: refuse — never guess the PR number from cwd alone.
- **`aida review prompt` returns empty**: PR has no `(REQ-ID)` trailers. STOP and ask the user how to attribute the diff.
- **CI red for an unrelated reason**: don't auto-bypass. Surface to the user; they decide whether to override.
- **Spec's acceptance criteria are vague**: <!-- kind:design-fork --> ⚠️ PARTIAL by default and ask the user to either tighten the spec or accept the gap explicitly. Don't let vagueness leak through as PASS. This is a `kind:design-fork` — surface it even under `$AIDA_ZEN`.
- **Implementer pushes new commits mid-review**: re-run step 2 (`aida review prompt --pr N` again) so the checklist matches the latest range. Verdicts on stale code are worse than none.
- **PR already merged but new commits arrive on the branch (BUG-88)**: don't treat them as extending the original PR. The commits live on `origin/<branch>` but won't reach `main` without a fresh PR. Verify with `gh pr list --head <branch> --state open` before continuing; if empty + `--state merged` returns a hit, ask the implementer to open a follow-up PR (or cherry-pick onto a new branch off `origin/main`) before reviewing further.