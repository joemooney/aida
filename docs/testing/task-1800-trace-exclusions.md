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

The [manual witness audit](task-1800-witness-audit.md) breaks down all 290
fallback pairs and reviews the selected refusal reasons. Its incidental-witness
counts are not PR-level false-positive rates; the exhaustive measurement gate
remains open.

## AC2 slice 1: item-level Rust attribution

<!-- trace:TASK-1800 | ai:claude -->

**Result: no case-level change. Refusals stay at 49/50 (22 named + 27
additional) before and after.** The item-level collector loses no
true-touch or unresolved pair, but as signed off it widens rather than narrows
the evidence. It falls back on **414** case/path pairs against 290 for the
fn/impl collector, and its scoped set grows in 34/50 cases and shrinks in none.

Slice 1 makes every syn item a unit (struct, enum, union, const, static, type,
trait and its items, use, extern crate, foreign block, item macro,
`macro_rules!`, mod), with an inline module counted only as its shell. A
same-file reference closure then selects every unit that mentions a name a
selected unit defines, repeating to a fixpoint. Module-comment ownership is
still decided over fn/impl alone. A file falls back to its full-file union, as
in sketch §3, when any of these holds: a parse fails; a changed line is in no
unit; a selected unit holds or overlaps an item macro, `macro_rules!`, an
unparsed item or a glob import; a closure name appears in a macro invocation;
`#[path]` or `include!` is present; or the change is mode-only. Markdown is
unchanged (not in slice 1).

`make task-1800-replay` reproduces this. It replays the fn/impl collector
(`ChangedItemsV1`), the item-level collector and the full file on all 50 frozen
endpoints. The fn/impl and full-file sets reproduce every recorded digest and
fallback set above, and every audited blob hash matches, so there is no corpus
or collector drift. Per changed path, the new set is within the full file and,
where the fn/impl collector did not fall back, contains its set. Case-level
refusals use the frozen per-ID policy classes (local relay evidence, not
published; passed as `POLICY=`).

### The 18 audited cases

Audited (case, ID) pairs, counted as total / kept by fn-impl / kept by item-level:

| Pairs | Total | fn/impl | item-level |
|---|---:|---:|---:|
| scoped/true-touch | 29 | 29 | 29 |
| scoped/unresolved | 30 | 30 | 30 |
| scoped/incidental | 102 | 102 | 102 |
| full-only/true-touch | 2 | 0 | 1 |
| full-only/unresolved | 7 | 0 | 6 |
| full-only/incidental | 24 | 0 | 15 |

All 59 scoped true-touch and unresolved pairs are kept. No scoped incidental
pair is dropped, and 15 of the 24 full-only incidental pairs are added.

| PR | scoped true/unresolved kept | scoped incidental kept | full-only pairs gained | fallback paths | refusal |
|---|---:|---:|---:|---:|---|
| #2647 | 1/1 | 3/3 | 2/2 | 1 → 2 | refused → refused |
| #2652 | 5/5 | 13/13 | 0/0 | 1 → 1 | refused → refused |
| #2653 | 2/2 | 2/2 | 0/0 | 3 → 5 | refused → refused |
| #2654 | 3/3 | 1/1 | 0/0 | 3 → 3 | refused → refused |
| #2656 | 1/1 | 8/8 | 2/3 | 4 → 5 | refused → refused |
| #2666 | 10/10 | 9/9 | 0/0 | 4 → 5 | refused → refused |
| #2668 | 0/0 | 0/0 | 0/0 | 1 → 1 | allowed → allowed |
| #2669 | 3/3 | 2/2 | 0/1 | 1 → 2 | refused → refused |
| #2670 | 6/6 | 9/9 | 16/16 | 1 → 2 | refused → refused |
| #2672 | 10/10 | 8/8 | 0/0 | 3 → 3 | refused → refused |
| #2674 | 2/2 | 12/12 | 0/0 | 18 → 18 | refused → refused |
| #2676 | 2/2 | 12/12 | 0/8 | 4 → 6 | refused → refused |
| #2681 | 2/2 | 0/0 | 0/0 | 1 → 2 | refused → refused |
| #2685 | 1/1 | 0/0 | 0/0 | 1 → 3 | refused → refused |
| #2686 | 3/3 | 1/1 | 1/2 | 1 → 4 | refused → refused |
| #2697 | 5/5 | 12/12 | 0/0 | 3 → 3 | refused → refused |
| #2698 | 2/2 | 7/7 | 1/1 | 4 → 6 | refused → refused |
| #2701 | 1/1 | 3/3 | 0/0 | 2 → 2 | refused → refused |

### All 50 frozen endpoints

The table lists ID counts (full file / fn-impl / item-level) and fallback paths
(fn-impl → item-level). The JSON `ac2_item_level.rows` holds the full pins, the
new set digests and each fallback path's reasons.

| PR | base | head | IDs full / old / new | fallback paths | refusal |
|---|---|---|---:|---:|---|
| #2705 | `652423b0e4` | `007c853a0f` | 179 / 93 / 156 | 2 → 4 | refused → refused |
| #2707 | `a8bfa2d7d7` | `218d0250cb` | 140 / 74 / 140 | 2 → 5 | refused → refused |
| #2679 | `ed29b2b660` | `92afa7ea3f` | 54 / 53 / 53 | 5 → 7 | refused → refused |
| #2701 | `ed29b2b660` | `9d4002c627` | 16 / 16 / 16 | 2 → 2 | refused → refused |
| #2692 | `4caea8b7a3` | `3ec1b703c0` | 656 / 396 / 531 | 4 → 24 | refused → refused |
| #2683 | `f1663f5eab` | `cf6ecf1bcb` | 983 / 936 / 969 | 18 → 21 | refused → refused |
| #2691 | `0ef2f8227a` | `d8d16437ce` | 763 / 739 / 746 | 8 → 9 | refused → refused |
| #2698 | `fc39d8f0bc` | `06c6c50962` | 134 / 88 / 134 | 4 → 6 | refused → refused |
| #2673 | `0ef2f8227a` | `826c3352e4` | 86 / 83 / 83 | 7 → 8 | refused → refused |
| #2695 | `76790181a0` | `57ee0a5281` | 191 / 121 / 191 | 7 → 10 | refused → refused |
| #2696 | `e0713fdb5e` | `1ddcc54fb8` | 172 / 87 / 129 | 2 → 4 | refused → refused |
| #2684 | `fc39d8f0bc` | `88329ff190` | 302 / 189 / 201 | 16 → 17 | refused → refused |
| #2697 | `e0713fdb5e` | `aa8069e2cb` | 92 / 92 / 92 | 3 → 3 | refused → refused |
| #2693 | `5aa7198cfe` | `45f646c54c` | 345 / 345 / 345 | 3 → 3 | refused → refused |
| #2678 | `75ff185cbf` | `58ce1964dd` | 686 / 686 / 686 | 6 → 5 | refused → refused |
| #2690 | `76790181a0` | `acc8c0c690` | 245 / 191 / 245 | 6 → 8 | refused → refused |
| #2687 | `5fb188c522` | `1fcfa13473` | 886 / 839 / 873 | 10 → 14 | refused → refused |
| #2686 | `696c90c4f4` | `9c7ba29c81` | 19 / 16 / 18 | 1 → 4 | refused → refused |
| #2688 | `463b81d027` | `4a1cacb652` | 523 / 276 / 523 | 5 → 9 | refused → refused |
| #2685 | `8336ec0731` | `d5f63fee91` | 7 / 5 / 7 | 1 → 3 | refused → refused |
| #2689 | `463b81d027` | `99e6baef64` | 190 / 175 / 190 | 5 → 6 | refused → refused |
| #2677 | `696c90c4f4` | `f60fafc68b` | 685 / 685 / 685 | 9 → 9 | refused → refused |
| #2681 | `8336ec0731` | `487bd6dd2d` | 12 / 5 / 12 | 1 → 2 | refused → refused |
| #2680 | `8336ec0731` | `b285957b9f` | 544 / 477 / 516 | 5 → 7 | refused → refused |
| #2674 | `596a91dbc0` | `40a627637e` | 125 / 125 / 125 | 18 → 18 | refused → refused |
| #2671 | `6ddf336ddd` | `d8cea0368f` | 709 / 664 / 700 | 7 → 10 | refused → refused |
| #2672 | `7b51e37f21` | `febe3b0a4f` | 96 / 96 / 96 | 3 → 3 | refused → refused |
| #2676 | `bfddcbee42` | `e1a40a89bc` | 156 / 120 / 123 | 4 → 6 | refused → refused |
| #2667 | `c419f6a5ef` | `c333cec917` | 681 / 681 / 681 | 7 → 8 | refused → refused |
| #2666 | `a46af7481e` | `7c8c72c75f` | 42 / 42 / 42 | 4 → 5 | refused → refused |
| #2664 | `34280b1914` | `79ee6c0887` | 684 / 684 / 684 | 10 → 10 | refused → refused |
| #2675 | `7b51e37f21` | `a230cf860e` | 297 / 191 / 296 | 1 → 6 | refused → refused |
| #2660 | `0c69debffe` | `0e526567f1` | 680 / 680 / 680 | 10 → 10 | refused → refused |
| #2485 | `bfddcbee42` | `6475cfba9b` | 1254 / 1026 / 1223 | 21 → 28 | refused → refused |
| #2657 | `3d85bbde84` | `96da17df62` | 529 / 222 / 529 | 3 → 8 | refused → refused |
| #2451 | `dcc46d8298` | `2f0ef157cb` | 786 / 749 / 786 | 18 → 20 | refused → refused |
| #2670 | `b9f2f5a28b` | `182ef87134` | 114 / 47 / 114 | 1 → 2 | refused → refused |
| #2669 | `596a91dbc0` | `34b992adce` | 40 / 20 / 21 | 1 → 2 | refused → refused |
| #2668 | `3ea566f840` | `0498eac1e9` | 0 / 0 / 0 | 1 → 1 | allowed → allowed |
| #2665 | `3f7bcd28fd` | `1013c63db6` | 1004 / 861 / 970 | 3 → 7 | refused → refused |
| #2653 | `453b2e22c5` | `551163384f` | 53 / 51 / 53 | 3 → 5 | refused → refused |
| #2658 | `6ddf336ddd` | `c4b503beec` | 811 / 769 / 811 | 6 → 8 | refused → refused |
| #2663 | `0d8c0c3b0c` | `099a574c97` | 554 / 228 / 530 | 5 → 10 | refused → refused |
| #2656 | `672c12158b` | `2cc1095b5f` | 60 / 57 / 59 | 4 → 5 | refused → refused |
| #2654 | `efd3bba8de` | `1e5dd25bbe` | 34 / 34 / 34 | 3 → 3 | refused → refused |
| #2655 | `92763a125a` | `a55d76553b` | 280 / 198 / 244 | 3 → 4 | refused → refused |
| #2652 | `266d8f5ef3` | `53548583ac` | 71 / 71 / 71 | 1 → 1 | refused → refused |
| #2541 | `b13bf7d3eb` | `148dc79429` | 748 / 498 / 709 | 19 → 39 | refused → refused |
| #2650 | `732db49601` | `01bc7eb75f` | 159 / 107 / 143 | 1 → 3 | refused → refused |
| #2647 | `403be14edd` | `825ebd7102` | 83 / 51 / 83 | 1 → 2 | refused → refused |

### Why it widens, and why the fail-closed rules cannot be relaxed

The new collector records a reason for each of its fallbacks: 120 for
`#[path]` (nearly every source file here declares
`#[cfg(test)] #[path = "tests/…"] mod tests;`), 129 for a closure name
inside a macro invocation (`format!`, `assert!` and similar), 116 for
non-Rust files, 49 for opaque units and 3 for uncovered lines. A
fallback for one reason can hide another, so these counts overlap.

Two relaxations were measured with temporary switches and not shipped:

| Variant | fallback paths | lost true/unresolved pairs | scoped incidental kept | refusals |
|---|---:|---|---:|---|
| as signed off | 414 | none | 102/102 | 49 → 49 |
| without the `#[path]` rule | 327 | PR 2697: unresolved TASK-140, PR 2697: true-touch TASK-963 | 99/102 | 49 → 49 |
| without the `#[path]` and macro-name rules | 172 | 7 pairs, including #2697 TASK-963 | 88/102 | 49 → 49 |

Each relaxation drops a true supervised touch, which the gate forbids. Even
the unsafe variant leaves every case-level refusal unchanged. The true touches
that fallback carries are reached by cross-file or macro-mediated use that a
same-file syntactic closure cannot prove. Within this design, the
incidental rate cannot fall without losing true touches.

Under signoff condition 3, case-level refusals did not change across the 50, so
markdown scoping is dropped. Further mandate-coverage work belongs to resolving
the unresolved IDs (store history and aliases) as its own spec.
