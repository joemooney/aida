# Porting PR hunks after a file split

<!-- trace:STORY-2692 | ai:codex -->

Run the host-side helper from the merge worktree with the PR's original base
and head commits and an explicit list of candidate destination files:

```sh
python3 scripts/port-split-hunks.py --base <base-sha> --head <pr-head-sha> \
  --source path/original.rs \
  --destination path/parent.rs --destination path/module.rs
```

The default previews placements. Repeat with `--apply` to write them and run
`python3 scripts/check-store-writers.py --root <worktree> --regenerate`.
Review the resulting source and inventory diff. New inventory sites retain the
regenerator's blocked policy; regeneration grants no authority. The inventory
must be parseable TOML before regeneration: if it has merge markers, first
restore an explicitly reviewed inventory baseline using the host's normal
merge procedure. The helper does not select a policy baseline for you.

Every hunk's complete old text (removed lines plus three-line diff context)
must occur exactly once across all supplied destinations. Matching uses the
original destination contents, including whitespace and final newlines.
Missing, ambiguous, overlapping and context-free hunks refuse the whole plan
before any writes. Repeated matches within one file also refuse. Explicit
candidate selection is the operator's responsibility; include every possible
destination and the surviving parent module.

The helper never infers imports, visibility changes or re-exports. If a PR also
edits parent-facing API wiring that no longer has exact context, the command
refuses it. Supply a surviving parent with matching context or handle the
re-export manually, then validate compilation and behaviour. Exact textual
placement alone does not establish Rust module correctness.

This command operates on working files only. It leaves the index and merge
conflict entries untouched. After reviewing the diff, resolve any remaining
conflict markers and stage the intended resolutions yourself. Use ordinary
merge handling for other PR files, renames, binary or mode-only changes.
Destinations must be existing regular files without symlink components.

Inventory regeneration runs after source writes. If it fails, the command
returns nonzero and retains the source edits and any inventory changes for
inspection. Filesystem write failures may also leave partial writes; review
and restore the worktree before retrying. Do not run concurrent writers on
these files.
