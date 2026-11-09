# Porting PR hunks after a file split

<!-- trace:STORY-2692 | ai:codex -->

Run the host-side helper from the merge worktree with the PR's current merge base
and head commits and an explicit list of candidate destination files:

```sh
python3 scripts/port-split-hunks.py --base "$(git merge-base HEAD MERGE_HEAD)" --head <pr-head-sha> \
  --source path/original.rs \
  --destination path/parent.rs --destination path/module.rs
```

During an active merge, `--head` must equal either `HEAD` (merging main into the PR) or `MERGE_HEAD` (merging the PR into main) and `--base` must
be the unique merge base of HEAD and that head. A wrong base refuses before
writes, preventing replay of changes that belong to the destination branch.
Outside a merge, the helper checks ancestry only; selecting the intended PR
range remains the operator's responsibility.

The default previews placements. Repeat with `--apply` to write them and, once source conflicts are staged as resolved, run
`python3 scripts/check-store-writers.py --root <worktree> --regenerate`.
Review the resulting source and inventory diff. New inventory sites retain the
regenerator's blocked policy; regeneration grants no authority. The inventory
must be parseable TOML unless you supply `--inventory-baseline /path/reviewed.toml`.
For a conflicted inventory, extract an explicitly selected policy baseline,
review it, and pass it to the helper:

```sh
git show <reviewed-commit>:scripts/store-writers.toml > /tmp/reviewed-writers.toml
# Add to the helper invocation:
# --apply --inventory-baseline /tmp/reviewed-writers.toml
```

The helper validates baseline version, row structure, required policy fields,
writer identities and duplicate IDs before writing source and forwards it to
the existing `check-store-writers.py --regenerate --baseline <path>` path.
Regeneration preserves that baseline's matching policy rows, scans the merged
source and replaces the conflicted working inventory. Review policy changes on
the other merge side separately: the helper never selects or combines policy
evidence for you. A missing, invalid or empty explicit baseline refuses; reviewed writer rows are required.

Every hunk's complete old text (removed lines plus three-line diff context)
must occur exactly once across all supplied destinations. Matching uses the
original destination contents, including whitespace and final newlines.
Already-present replacement text refuses conservatively to prevent replay, including EOF insertions. Missing, ambiguous, overlapping and context-free hunks refuse the whole plan
before any writes. Repeated matches within one file also refuse. Explicit
candidate selection is the operator's responsibility; include every possible
destination and the surviving parent module.

The helper never infers imports, visibility changes or re-exports. If a PR also
edits parent-facing API wiring that no longer has exact context, the command
refuses it. Supply a surviving parent with matching context or handle the
re-export manually, then validate compilation and behaviour. Exact textual
placement alone does not establish Rust module correctness.

For a conflicted destination, explicitly pass `--destination-conflict-side ours`
(or `theirs`). The helper matches against that index stage and writes the ported
result over the working conflict text. Include the surviving parent as a candidate
to port exact parent hunks as well as module hunks. This discards the other side's
conflict text in that destination: inspect the full PR diff for other changes and
review the result before staging. Without an explicit side, conflicted destinations
refuse before writes. Missing stage content also refuses.

This command operates on working files only. It leaves the index and merge
conflict entries untouched. After reviewing the diff, resolve any remaining
conflict markers and stage the intended resolutions yourself. Use ordinary
merge handling for other PR files, renames, binary or mode-only changes.
Destinations must be existing regular files without symlink components.
CLI help golden regeneration is outside this initial inventory-only slice.

Inventory regeneration runs after source writes only when no unmerged entries
remain except `scripts/store-writers.toml`. Otherwise the command reports a deferred
regeneration and exits successfully with the hunk edits retained. Resolve and stage
the remaining source conflicts, then run the printed regeneration command (with
`python3` if needed). A successful port with deferred regeneration is not a finished
merge; the inventory still needs regeneration and review. If it fails, the command
returns nonzero and retains the source edits and any inventory changes for
inspection. Filesystem write failures may also leave partial writes; review
and restore the worktree before retrying. Do not run concurrent writers on
these files.
