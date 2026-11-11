# SPIKE-714: round-two extraction correction and blocked audit — 2026-11-11

<!-- trace:SPIKE-714 | ai:codex -->

**Incomplete; advisor/host action required.** The review of `8d67d4ff8b52`
correctly identified missing acceptance work. This round fixes the three
truncated closure handlers. It does not claim completion of the 167-command
classification, dependency analysis or first porcelain batch. No finding is
disputed. No crate placement, public API or product behavior is decided here.

## Source and reproduction

The inspected production source and regenerated table are pinned to
`bcb67e299d057c874734b01b13a665cbbbf7d4ee` (the lane HEAD before these edits).
The earlier baseline used `eb1b39c30c9f9f6ce3f5ccf1c5ffd8966e5f8a23`.
The table still contains exactly 167 registrations and `complete = false`;
all 167 classifications remain unresolved. There are still 109 rows with
same-file lexical glob sites and 58 without. These are command-row counts,
not distinct-file counts or verified dependency counts.

From this checkout, run:

```sh
python3 scripts/command-inventory.py
python3 -m unittest discover -s tests -p test_command_inventory.py -v
```

The generator records the current HEAD each time it runs; reproducing this
snapshot requires this revision's production source and this round's generator.
The generated table is lexical evidence. It is not a call graph. The new
extractor reuses `mask_rust` from `scripts/check-store-writers.py` to hide
comments and literals without changing offsets, then balances parentheses,
brackets and braces around the static initializer and `Handler` argument.
It rejects unclosed/mismatched handler delimiters instead of silently returning
a truncated expression. The scanner is not a Rust parser or name resolver.

The four regression tests exercise all three actual closures, nested calls,
raw/escaped strings, character literals, nested comments, invalid delimiters
and static initializer boundaries. All four executed and passed. A full
regeneration retained the 167-entry denominator. Relative to the preceding
snapshot it changes the source commit, the three closure expressions and
the shifted `criteria` source/glob line numbers; it does not fill classification
fields with inferred answers.

## Review disposition

| Review item | Round-two disposition | Remaining condition |
| --- | --- | --- |
| 1. All 167 classifications and reachable writer/gate evidence unresolved | Accepted; open | Handler-level production call closures, with independent authority review and reachable sinks matched to writer IDs |
| 2. Dependency, visibility, move-stage and home-crate analysis absent | Accepted; open; three closure entrypoints inspected below | Item-level analysis for every command; proposed homes remain advisor alternatives |
| 3. First porcelain batch missing | Accepted; open | About twenty verified read-only candidates ranked by actual dependencies, with exact blockers |
| 4. Three closure handlers truncated | Fixed | `_bg-fetch`, `away`, `home` now retain their complete expressions; regression tests cover the actual registrations |

## Source inspection of the corrected entrypoints

These observations come from reading the pinned source and following explicit
calls/imports with `rg`, not from a successful codegraph query. They establish
the listed positive edges only; they do not certify complete call closures.

| Registration | Observed production path | Writer-ledger cross-check |
| --- | --- | --- |
| `away`, `presence_cmd.rs:105` | Closure → `handle_away_command` (`presence_cmd.rs:37`) → `presence::set_away` (`presence.rs:182`) → `write_presence_file` (`presence.rs:164`) → `std::fs::write` (`presence.rs:171`) | Exact ID `aida-cli-lib/src/presence.rs\|write_presence_file\|fs::write`, present in `scripts/store-writers.toml` |
| `home`, `presence_cmd.rs:114` | Closure → `handle_home_command` (`presence_cmd.rs:52`) → `presence::set_home` (`presence.rs:177`) → the same `write_presence_file` sink | Same exact ID; not a sink inferred from merely sharing a file |
| `_bg-fetch`, `session_misc_cmd.rs:197` | Closure parses `BgFetchArgs`, then calls `handle_bg_fetch_command` (`session_misc_cmd.rs:213`). Its invalid-repository and missing-origin branches call the root-imported `write_last_fetch_entry` (`session_misc_cmd.rs:230,237`), defined in `store_sync.rs:1803`; that function writes at `store_sync.rs:1833` | Exact ID `aida-cli-lib/src/store_sync.rs\|write_last_fetch_entry\|fs::write`, present in `scripts/store-writers.toml` |

The presence destination is `~/.aida/presence.toml`, subject to the existing
home resolution in `presence_path`. Both presence setters also reach the
`fs::create_dir_all` ledger entry in `write_presence_file`. They are state
writers, even though they do not write requirements. The presence-command
documentation describes this state as advisory input to mode selection;
that fact does not establish grant/lease authority.

The background-fetch destination is `~/.aida/cache/last-fetch.toml` under
`aida_home_dir`. Its normal branch delegates to root-imported
`handle_fetch_command` (`session_misc_cmd.rs:245`, definition
`store_push_pull.rs:3617`). Its `LockGuard::drop` also removes a fetch lockfile
(`session_misc_cmd.rs:221`), with the exact ledger ID
`aida-cli-lib/src/session_misc_cmd.rs|handle_bg_fetch_command::impl Drop for LockGuard::drop|fs::remove_file`.
That lockfile is fetch coordination evidence, not proof of an authority lease.
The normal fetch path needs separate Git/deferred-effect analysis; the
error-branch cache-write path alone already disproves read-only behavior.

These three commands must therefore be excluded from a strictly read-only
first batch. Calling them `core-write` in the requested taxonomy would need
an explicit interpretation: these presence/cache writes occur in cli-lib,
not through a core store mutation API. The report leaves that interpretation
to the advisor rather than silently broadening “writes through core.”

## Visibility and move alternatives for these entrypoints

| Commands | Concrete blockers observed | Stage/home alternatives for advisor review |
| --- | --- | --- |
| `away`, `home` | `presence` and `presence_cmd` are private modules (`lib.rs:603–604`). `set_away`, `set_home`, `away_ttl_secs`, `ttl_remaining_label` and `Presence` are `pub(crate)`; the module's `resolve_away_ttl_secs` is private. The handlers use root `config_path_for_project`, `glyph` and `glyphs::Glyph`, plus `chrono` and `colored`. Moving the entire module keeps its local helper private, but crossing the current crate boundary needs an exposed owner API or relocation of those shared items. `write_presence_file` can stay private behind the setters. | Not porcelain views. A presence/application-family owner is an alternative to a named presence extension; `aida-term` could cover rendering but does not supply a presence-state service. Leaf crates alone do not resolve the state-writer API. No move-now claim is made. |
| `_bg-fetch` | `session_misc_cmd` is private (`lib.rs:704`); the entry handler is `pub(crate)`. `bg_fetch_lock_path` and `write_last_fetch_entry` are `pub(crate)` in public `store_sync`; `handle_fetch_command` is `pub(crate)` in private `store_push_pull` (`lib.rs:299`). Their root imports are explicit at `session_misc_cmd.rs:15–19`; root wildcard imports at `lib.rs:392–393` explain the apparent root lookup. The locally defined args and `LockGuard` can move with the handler without public exposure. External helpers require an owner boundary or coordinated move. | Not porcelain views. Keep with store-sync/Git lifecycle family, or consider a host-owned fetch extension after its API and write ownership are settled. `aida-cmdkit` alone does not provide the fetch/write boundary. |

`cmdkit` is already public (`lib.rs:88`), but it still belongs to cli-lib.
Moving these handlers into a crate that cli-lib itself depends on would
create a dependency cycle if that crate imported cli-lib's registration
types. A leaf `aida-cmdkit` could break that particular cycle; it would not
resolve the private services listed above. This is dependency analysis,
not a recommendation to widen every listed helper. Options include moving
an owner module, introducing a narrow owner API, or preserving host injection.

For this three-command subset, the most shared state blocker is the presence
owner API (two commands); the fetch owner helpers affect one. These are
subset counts only. They must not be presented as the requested global
ranking across 167 commands.

## Blocker and recovery

The required reviewed host client is still absent at
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph`: filesystem lookup
returns “No such file or directory.” `aida doctor check codegraph` reports
`codegraph,missing`. There is no successful query and no “no callers” result
to confirm. No lane index was created or refreshed, and no executable was
derived from the seat's `.codegraph` link.

**Host action requested:** deploy the reviewed query client under the primary
checkout's `.codegraph/proxy-state/bin/codegraph`, supply its absolute path
if different from the instructed path, and make the shared main index
reachable. Record the indexed main revision when queries resume. The
specified review-comment URL could not be retrieved in this session; the
authoritative findings supplied in the task and SPIKE-714 comments were used.

**Advisor input requested:** confirm how direct CLI-owned coordination/cache
and generated-file writes fit the three classes, and whether mixed commands
take their strongest effect with verb-level detail. This affects the audit's
interpretation, not a product decision made by this research report.

SPIKE-714 is explicitly punted on review items 1–3 (acceptance items 1–3).
The missing client prevents the mandated navigation evidence; the uncovered
direct writers also require a taxonomy interpretation before asserting
“writes through core.” Item 4 is fixed and reviewable locally. Do not mark
the spike complete or use this table as a migration allow-list. This native
seat leaves edits uncommitted for the launcher; the punt remains a publication
veto until the host/advisor resolves it.
