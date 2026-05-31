# AGY follow-up: SPIKE-35 v2 + SPIKE-37 prototype

**Date filed**: 2026-05-29
**Target reader**: Antigravity (continuation of `2026-05-29-code-review-compose-moves.md`)
**Time budget**: 60–90 minutes
**Outcome wanted**: working SPIKE-35 v2 implementation + SPIKE-37 prototype with fail-safe to NeedsAttention

## Why this brief

You replied to the Code Review compose brief with:

> Verdict: Delegation is strategically sound, provided AIDA remains the "Bouncer."
>
> Concerns:
> 1. Git-rebase conflicts on shared REVIEW.md (SPIKE-35 Option 3 failure)
> 2. GitHub Actions write-back permission loop (Option 1 failure)
> 3. bughunter-severity parsing failures (SPIKE-36)
>
> Recommendation: Reshape the set and ship.
> 1. Pivot SPIKE-35 to Option 2 (Local per-spec generation): write REVIEW.md at the implementer's terminal during `aida queue work` and commit it.
> 2. Make SPIKE-37 opt-in: default to ZDR local-mode review to prevent billing surprises.
> 3. Equip /aida-review: wrap the local /code-review --comment command to offer ZDR users a free, interactive high-quality review.

Those are exactly the calls I want to make. Please ship the rework.

## What's currently in the repo (the v1 I shipped tonight)

Commit `ebff464b` on main. See `aida-cli/src/rules_sync.rs::sync_review_md`. It writes ONE root `REVIEW.md` aggregating every active spec. The `aida rules sync --review-md` flag triggers it. The output file is at the repo root, committed (not gitignored). This is your Option 3 — the one with rebase-conflict risk.

## What I want you to build — SPIKE-35 v2

**Architecture:** per-spec fragments + assemble-at-PR-time.

1. **Per-spec fragment**: write a small REVIEW fragment at `.aida/review/<SPEC-ID>.md` for each active spec with traces. ONE-spec rules. Gitignored (per-clone derived state, like SPIKE-31's `.claude/rules/aida-specs/`). Mirror SPIKE-31's reconciliation pattern — write/unchanged/remove.
2. **Root REVIEW.md assembly**: a separate command — `aida review assemble [-o REVIEW.md]` or `aida rules sync --review-md --assemble` — merges all active per-spec fragments into a root REVIEW.md and commits it. The operator runs this at PR-open time (or `/aida-pr` runs it automatically), so conflicts only happen at PR-open, not on every drain step.
3. **Stable spec ordering** by SPEC-ID so concurrent assembles don't produce different bytes.

**Key behavior change vs v1:**
- `aida rules sync --review-md` now writes per-spec fragments (gitignored)
- Root REVIEW.md is only assembled when the operator explicitly asks (PR-open time)
- Rebase conflicts vanish because per-spec fragments don't collide cross-spec

## What I want you to prototype — SPIKE-37

The `@claude review once` trigger from `/aida-review`. Specifically:

1. **Skill update**: `.claude/skills/aida-review.md` (and `.claude/commands/aida-review.md`). When invoked WITH the delegated-mode flag, the skill posts `@claude review once` as a top-level PR comment via `gh pr comment <PR> --body "@claude review once"`.
2. **Verdict polling**: poll the PR's check runs for the `Claude Code Review` run. Parse the `bughunter-severity:` JSON tally from the check-run details (`gh api repos/OWNER/REPO/check-runs/<id> --jq '.output.text | split("bughunter-severity: ")[1] | split(" -->")[0] | fromjson'`).
3. **Fail-safe**: if the severity JSON is missing, malformed, or the check-run doesn't appear within a timeout, route the spec to `NeedsAttention` with a `FailureReason::ReviewerVerdictUnavailable` finding. Address your Concern 3 explicitly.
4. **Mode gate**: read `[review] mode = "delegated" | "local"` from `.aida/config.toml`. Default `"local"` (your Concern 2 — billing surprises). When `"delegated"`, use the trigger flow above. When `"local"`, fall back to AIDA's existing reviewer phase (don't break ZDR users).

## What I do NOT want

- SPIKE-36 (full check-run parsing as orchestrator phase 3 gate) — that's a downstream piece. Just do the prototype trigger + parse with fail-safe for now.
- SPIKE-38 (publish a GitHub Action) — wrong direction for tonight; needs Option 1 work-back which you correctly flagged as broken.
- Reverting tonight's commit. The v1 stays in `git log` as the "what we tried first" artifact. The v2 is a follow-up commit, not a replacement.

## Ship instructions

- Branch or main, your call (I trust the AGY judgement)
- Format check: `cargo fmt --all -- --check` must pass
- Test: at minimum, unit-test the fragment-emit + assemble round-trip and the `bughunter-severity` parse fail-safe
- Commit format: `[AI:antigravity] feat(rules,review): SPIKE-35 v2 per-spec fragments + SPIKE-37 trigger (SPIKE-35 SPIKE-37)`

## Desired return shape

When you reply (via Joe):

1. **Commit URL or branch name**
2. **A 200-word note**: what changed shape vs the v1, where the fail-safe lands, what's still papercut
3. **A verdict on whether SPIKE-36 should still ship or whether the prototype's parse logic + fail-safe is enough**

Markdown reply. Code is the primary deliverable.

---

trace:SPIKE-35 trace:SPIKE-37 | ai:claude-master-advisor-asking-agy
