# AIDA PR

Wrap up the current batch of commits and open a pull request with linked specs and a test plan.

## Usage

```
/aida-pr                           Auto-derive everything from the current branch + git log
/aida-pr --base epic-20-batch4     Stack on a previous batch's PR (defaults to `main`)
/aida-pr --quiet                   Skip the preview banner + 3s pause (autonomous flows)
```

## Instructions

Follow the workflow in `.claude/skills/aida-pr.md`:

1. Walk `git log <base>..HEAD --oneline` and extract `(REQ-ID)` suffixes from each commit subject
2. Verify every derived REQ-ID is in `Completed` status (pause and ask if any are still open)
3. Pre-flight `cargo fmt --all -- --check` on Rust workspaces — refuse to proceed if drift exists (TASK-61)
4. Print the "about to happen" banner — Completed / Now I will / Then you can — then pause ~3s so the user can abort before any side effect; `--quiet` or `AIDA_NO_BANNER=1` skips it (TASK-259)
5. Push code + orphan store (`aida push` or equivalent two-step) before opening the PR
6. Compose a PR title (`EPIC-N batch M: <one-line summary>`) and a body that mirrors recent PRs (per-spec sections, test plan)
7. Show the title + first paragraph to the user and require sign-off
8. Run `gh pr create` (HEREDOC body for proper formatting); print the URL

Pairs with `/aida-commit` (commit first, then PR) and `/aida-code-review` (reviewer side after the PR opens).
