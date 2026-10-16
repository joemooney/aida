# Mandate trace exclusion benchmark

<!-- trace:TASK-1790 | ai:codex -->

For an ordinary mandate (without `--include-supervised`), the committed full-file
collector adds refusals for **25 of 50 PRs (50%)**. Hunk/enclosing-item collection
adds refusals for **24 of 50 (48%)**. The accepted >20% rule therefore selects
hunk/item collection, retaining full-file fallback for unattributable changes.
There is no claim that this reduces the rate below 20%, or that every additional
refusal is semantically unnecessary. Named checks already refused 22 of the 50.

The sample is the latest 50 merges to main at capture, ordered by `mergedAt`,
from a complete 100-result `merged:>=2026-10-14` query (limit 1000). Its newest
merge is #2705 at 2026-10-16T20:53:13Z; its oldest is #2647 at
2026-10-15T05:58:02Z. Each case uses the ORIGINAL PR head, with its merge base
against the delivered merge commit's first parent. This retains original
branch changes, rather than substituting squash output for the PR head.

[Machine-readable case pins and results](task-1790-trace-exclusions.json) contain
all 50 base/head/merge IDs and fallback paths. The source collector runs directly
from `mandate_traces.rs` with the repository's locked dependency versions. Current
classification, recorded changes, committed history and the private ledger use
the existing `exclusion_of`, `field_change_versions`, `store_history_versions`
and `read_exclusions` implementations extracted without policy changes. A
private all-ref mirror pins the store during the read-only comparison. The
comparison short-circuits on a refusal, which preserves the refusal boolean;
it does not enumerate all exclusion reasons.

Named subjects use the actual branch/title extraction and original commit
trailer parser. All other mandate gates are held fixed. This measures additional
CLASSIFICATION refusals, rather than replaying grants, approvals or merges.
No store or exclusion-ledger writes are performed by the measurement.

Store snapshot: `a5dae7145abd11d2480271c466030b12db6e7668`.
The host's ledger snapshot has 5 entries; its canonical parsed JSON SHA-256 is
`753d1a323b18af13ed73a18f20f9a6f7c6242e3584edf01049897d0dba7289af`. Ledger contents stay private.
The result is specific to these frozen inputs; `--include-supervised` was not
measured. Two newly refused cases have an unresolved trace as their first
blocker. Other new first blockers are existing sensitive/keystone classes.
Required source reads succeeded for all 50 cases.

Full-file fallback occurs in 46 cases, across 191 case/path pairs. Other source
languages use raw-token full-file fallback; Rust syntax/item attribution failures
and hunks outside functions/impls also fall back. Consequently, fixtures or
unrelated traces can still conservatively refuse coverage. This is evidence
expansion, not complete ownership attribution. Unlisted languages and untraced
code cannot identify unnamed specs.

A fresh author checker compared the optimized collector with the reference:
all 50 pinned cases matched IDs and fallback paths, and 1,813 adversarial
source/range comparisons matched. The tested collector SHA-256 is
`dbd6794c98f33e509a7ee4e7291deaa3dcd2457c20bdab8ee3fbf1113f62901c`. This author-controlled check conveys no
independent review authority. STRICT review is required.
