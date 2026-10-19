# Mandate trace exclusion remeasurement

<!-- trace:TASK-1800 | ai:codex -->

The current collector, including additive Rust file/inline-module headers and
unowned module comments, refuses an additional **27/50 PRs (54%)** beyond the 22 named-subject refusals in the frozen
TASK-1790 sample. Full-file and hunk/item modes both refuse 49/50 in total (98%).
Hunk mode falls back on at least one path in **50/50 cases**, with 290 case/path
pairs. All 50 required source reads succeeded; none refused for a symlink,
binary, gitlink or other unreadable evidence in this particular sample.

These replace the historical collector's 25/50 full-file and 24/50 hunk figures
for this sample. They do not establish semantic false-positive rates. The change
adds missing header evidence; it does not reduce conservative fallback or claim
that incidental co-location is a true supervised code touch.

[Case pins, trace-set counts/digests, fallback paths and classification witnesses](task-1800-trace-exclusions.json)
retain the original 50 PR heads and merge bases from
[the original measurement](task-1790-trace-exclusions.md). The newest case is
PR #2705 and the oldest #2647. Both committed versions are read by the actual
collector against the repository's local object database. Trace-set digests hash the sorted ID array
as compact JSON, encoded as UTF-8; the case pins allow reproducing each set.

The classification inputs remain frozen: store commit
`a5dae7145abd11d2480271c466030b12db6e7668` and the original five-entry private
ledger, whose sorted compact JSON SHA-256 is
`753d1a323b18af13ed73a18f20f9a6f7c6242e3584edf01049897d0dba7289af`.
The ledger is not published. The store stayed clean and unchanged during the
read. All nine classification/history functions are byte-identical to the
original audit. Prior named classifications and retained exclusion witnesses
are reused only for these identical frozen inputs; retained witnesses must
still occur in the newly collected set. Previously proven ordinary IDs are
cached; new IDs use the current extracted classifier and committed history.
A witness proves refusal, rather than claiming to enumerate or order all
reasons. No live grant, review, clearance or merge is replayed.

Header coverage now includes a leading module trace separated from a changed
function by another item, inner and outer docs, leading attributes, and a block
comment sharing the first item's line. Whole first-token-line inclusion is
conservative. Three new regressions fail against the parent collector and pass
with the fix; the exact production collector's 20-test standalone suite passes.
Full-crate call-site execution is recorded separately in the PR validation.

This is a non-completing slice. Semantic classification of true touches versus
incidental co-location and fallback-reduction design remain open. Symlink and
binary handling remain fail closed; changing their interpretation requires the
separate sketch on the owning task.

## Additive module-comment follow-up

The next priority slice includes comment lines outside function/impl items and
attached leading prefixes, plus inline-module headers even when attached to an
unchanged first function. This closes module traces between items and at EOF;
other unchanged function/impl prefixes retain item scope. It leaves hunk
attribution, every fallback, and non-Rust handling unchanged.

Rerunning the actual collector on the same 50 pinned base/head pairs adds scoped
IDs in **33/50** cases. Every previous ID remains present, every full-file
set is identical, and all fallback paths are identical (50 cases / 290 pairs).
The JSON records the previous/current collector hashes and updated set digests.
The refusal count remains 22 named plus 27 additional, 49/50 total. This count
uses the frozen classifications above: prior refusal witnesses remain in the
new sets, and each new scoped set is a subset of its unchanged full-file set.
The sole previously allowed full-file case therefore remains allowed. No new
live classification or grant is inferred. This still measures conservative
refusal, not semantic true touches or false positives.

Two new regressions fail on the parent collector and pass on this collector.
The real ship-cover callback fixture separately requires an item trace, module
trace, and file-header trace alongside the named subject. Its workspace runtime
result is recorded in the PR validation, separately from extracted-module tests.
