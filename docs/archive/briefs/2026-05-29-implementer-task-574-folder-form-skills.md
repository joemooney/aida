# Implementer brief: TASK-574 — land folder-form skills (PR-350)

**Date**: 2026-05-29
**Target**: an implementer agent (Codex or Antigravity)
**Spec**: TASK-574 — "Skill helper subfolders: allow `.claude/skills/<name>/{SKILL.md, templates/, examples/}`"
**Branch / PR**: `task-574` / PR-350 (currently NeedsAttention, shelved on CI red)
**Estimated effort**: ~30–45 min (the hard parts are already done — see below)

## Good news first (refreshed against current main, 2026-05-29)

This is **not** a conflict slog. A trial rebase of `task-574` onto current `main` is **clean — zero conflicts** (exit 0), even though the branch touches `scaffolding/mod.rs` and `templates.rs`, which were just refactored by TASK-584 (aida-recover removal) and TASK-481 (techdebt). The edits are in non-overlapping regions, so git auto-merges them.

The folder-form transition is also **already correct on the branch**:
- `.claude/skills/aida-pr.md` → **deleted** (D)
- `.claude/skills/aida-pr` → **added** as a symlink (the new folder form)
- `aida-core/templates/skills/aida-pr.md` → **renamed** `R099` to `aida-core/templates/skills/aida-pr/SKILL.md`
- `aida-core/templates/skills/aida-pr/examples/pr-description-template.md` → added

So the file→dir conversion is done right (old single-file form removed, not coexisting with the dir).

## Why it's shelved (the actual blocker)

PR-350's CI (run 26619373730) failed on **two** things, neither a code-logic bug:
1. **BUG-103**: `cargo did not resolve to the real cargo binary (stale rustup-init cache)` — flaky CI infra. Usually clears on a fresh run (a rebase + push triggers one). Do NOT use `gh run rerun` (re-runs the same SHA); push the rebased commit or an empty commit to trigger fresh CI (see `feedback_gh_run_rerun_is_not_fresh_ci`).
2. **`error: untracked working tree files would be overwritten by merge`** — the file→dir collision (`aida-pr.md` vs `aida-pr/`) surfacing during a checkout/merge step. The branch content is correct; this is an environmental/working-tree issue during the git operation, not a content bug.

Also relevant: earlier this session a *stale* `embedded_templates.rs` referencing a deleted `templates/skills/aida-pr/SKILL.md` broke a local build (cross-worktree cargo-fingerprint, TASK-396). The `build.rs` `embed_directory` walk handles `skills/aida-pr/SKILL.md` correctly once regenerated — so a clean build is the gate.

## The task

1. **Rebase** `task-574` onto current `origin/main` (`aida pr rebase 350`, or manual — it's clean). Force-push-with-lease.
2. **Verify the embed + build**: `cargo build -p aida-core -p aida-cli`. Confirm `build.rs` embeds `skills/aida-pr/SKILL.md` (the folder form) without the stale-template break. If you hit the stale `embedded_templates.rs` error, `touch` any `aida-core/templates/skills/*.md` to force `build.rs` to regenerate (or `cargo clean -p aida-core`).
3. **Run `make sync-templates`** — the new folder-form skill needs its `.claude/skills/aida-pr` symlink reconciled; confirm it links to the `aida-pr/` template dir (a symlink to a directory, not a file).
4. **Full test gate**: `cargo test -p aida-core -p aida-cli`, `cargo fmt --all -- --check`. There may be a scaffolding test asserting the skill-listing or the folder-form layout — make it pass.
5. **Ship**: `aida pr ship 350`. **Watch the squash/checkout step** for the `untracked working tree files would be overwritten` collision — if it recurs, the main worktree has a stale `.claude/skills/aida-pr.md` or `aida-pr/` artifact; clean it (`git clean`/`rm` the stale path) and retry the merge. CI must be green first (BUG-103 should clear on the fresh post-rebase run).
6. If CI is red on something *other* than BUG-103 or the file/dir issue, that's a real finding — `aida findings add` it and stop; don't force it.

## Verify-before-claiming

Per `feedback_verify_edits_landed_before_claiming_done`: gate the ship behind a green local build + fmt + test, and confirm CI is actually green (not just "re-run, probably fine") before merge. Don't narrate a merge you didn't observe land.

## Master sign-off

The folder-form-skills *design* (TASK-574) is already approved + implemented on the branch — this is a land-it task, not a design change. If the rebase surfaces something that changes the folder-form *contract* (e.g. build.rs needs a structural change to embed subdirs), that's architecture-impacting → flag the master before merging, per `feedback_one_master_advisor_until_subsystems`.

## Return shape

Reply (via Joe) with: PR-350 merge commit + a 2-line note (did the rebase stay clean? did the file/dir collision recur in the ship step, and how did you clear it? CI green on what run?). If you shelved again, the finding + why.

---

trace:TASK-574 | ai:claude-master-advisor-briefing-implementer
