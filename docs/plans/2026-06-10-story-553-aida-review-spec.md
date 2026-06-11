# STORY-553 — `aida review <spec>`: single verb to drive human review of a held spec

Date: 2026-06-10
Specs: STORY-553
Status: Done
Complexity: med

## Approach

Add a positional `<SPEC>` to the existing `aida review` command (currently subcommand-only:
`prompt` / `assemble`). Mirror the `Brief` pattern (`spec: Option<String>` + `#[clap(subcommand)] cmd: Option<ReviewCommand>`). When a bare spec is given, drive a human-decision review loop; with a subcommand, dispatch as before.

Flow of `aida review <SPEC>`:

1. Resolve `<SPEC>` via `backend.get_requirement_by_spec_id`.
2. Locate the review surface with `collect_git_linkage` (commits / branch / shipped) — gh-free.
   Then `change_lookup_for_branch` for the OPEN PR/MR (forge-neutral). NEVER assert a closed/absent PR.
   - Open PR → that's the surface.
   - Branch + commits, no open PR → "built on branch `<b>`, no open PR".
   - No branch, no commits → "built locally / never pushed" (offer to push + open PR).
3. Spawn the existing headless reviewer tier (`/aida-review`) via `spawn_claude_headless`, with
   `AIDA_REVIEW_VERDICT_FILE` pointed at `.aida/review-verdicts/<spec>.json` so the skill writes its verdict.
   Prompt: `/aida-review --pr N` when a PR exists, else `/aida-review` (reads the diff against `## Acceptance`).
4. Read the verdict file (`reviewer_summary::parse_verdict_file`); present findings + recommended verdict.
5. Human decides via `inquire::Select`: approve & merge | request changes | open the diff | defer.
   - approve → print the paste-ready `gh pr merge --squash` command (NEVER auto-merge; STORY-529 gate).
   - request changes → run `aida queue rework <SPEC>` in-process.
   - open the diff → `gh pr diff N` (or `git diff main...branch`).
   - defer → no-op, leave the spec held.
6. Non-TTY / no verdict → degrade honestly (print surface + recommended next command), no prompt.

## Decisions

- REUSE not rebuild: `collect_git_linkage`, `change_lookup_for_branch`, `spawn_claude_headless`,
  `/aida-review`, `inquire::Select`, `handle_queue_rework`. No new orchestrator machinery.
- ANALYST = reviewer (code-reader), per AC-2.
- NEVER auto-merge — approve prints the command for the human to run.

## Files (build order)

1. `aida-cli/src/cli.rs` — convert `Review(ReviewCommand)` → `Review { spec, cmd }` struct variant.
2. `aida-cli/src/main.rs` — new `fn handle_review_spec(...)`; route the two `Command::Review` dispatch
   sites (run() legacy + handle_git_backend_command()) to it when `spec.is_some()`, else `handle_review_command`.

## Critical Files

- `aida-cli/src/main.rs::collect_git_linkage` — review-surface resolver.
- `aida-cli/src/main.rs::change_lookup_for_branch` — forge-neutral open-PR lookup.
- `aida-cli/src/session.rs::spawn_claude_headless` — reviewer launch.
- `aida-cli/src/reviewer_summary.rs::parse_verdict_file` — verdict presentation.

## Verification

- `cargo build -p aida-cli`
- `cargo test -p aida-cli review`
- `cargo fmt --all -- --check`
- `aida review --help` shows the `<SPEC>` positional; `aida review prompt ...` still works.

## Related

- STORY-71 (`aida session start --owns PR-N`), STORY-529 (self-merge gate), BUG-493 (misleading draft-PR state).
