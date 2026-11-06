# Execution input migration

<!-- trace:STORY-2538 | ai:codex -->

The approved R4 design lives in STORY-2538's sketch and advisor sign-off of
2026-11-06. Slice A introduces the types and inventories below. No production
reader has been migrated and no launch environment has been cleared by this
slice. STORY-2538 remains incomplete.

## Shared inputs and launch boundary

`aida-core/src/exec_context.rs` owns `ExecContext`, an immutable parent
`Environment` snapshot, explicit optional home, project root, authority root,
`ToolPaths`, and a shared injected `Clock`. `Environment::capture` is the one
new ambient capture site. Production request entry points will capture there
and pass a context explicitly; tests construct the same types from pairs.
There is no singleton, thread-local override, implicit home lookup, tool search,
or authority inference. Path selections are inputs, not executable admission.
Consumers still validate required paths, live grants, leases and trust at their
existing locked boundaries. Wall time is sampled when requested; monotonic
budgets use elapsed duration on the same clock instance, never wall time.

`launch_policy::EnvPolicy` is the sole child-environment projection API.
It takes an explicit vendor/kind/recipient and exact forwarding keys. Slice A
introduces no deployed vendor allowlist. STORY-2326's compatibility-gated
allowlist slice will supply the reviewed declarations and activate env_clear.
A `ChildEnv` is a complete projection, not an inheritance overlay; it must not
be applied to an inheriting command without clearing that command first.
The projection uses the existing `OWNED_KEYS`, `BACKGROUND_FORBIDDEN_KEYS` and
`VENDOR_PROCESS_ONLY_KEYS` tables. Decided keys never come from ambient values;
explicit decisions can replace/remove known owned keys. Background seat keys
and tool-shell vendor tokens refuse. Unknown `AIDA_*` forwarding, including
test overrides and required-live declarations, refuses. Ordinary secrets are
absent unless a future reviewed policy explicitly lists their exact keys.
Neither context nor environment/policy types implement Debug or serialization.

PR-C (STORY-2326) owns vendor profiles, moving `prepare_launch`, grant issuer
consolidation, `control_child`/`control_env`, resume/finalization and resource
renderers. Until PR-C merges, R4 must not edit `aida-cli-lib/src/launch/` or
change `LaunchContext::resolve`. This slice edits neither.

## Exact-site legacy ledger

`scripts/exec-context-legacy.json` is a migration exception ledger, not a second
requirements store. Every entry is owned by STORY-2538 slices B–D. Each key
contains the repository path, nearest preceding named function (or module/import
binding), operation and SHA-256 prefix of the tokenized reference/call; its value
is the exact multiplicity. Function names are navigation hints, not Rust name
resolution. Whitespace/comments do not change call fingerprints; argument, alias,
function or path edits can require retiring an old entry. The total ceiling is
computed from the entries. Removed entries must be pruned; replacements cannot
silently borrow another site's allowance.

Initial inventory (including import bindings and Python harness references):

| Area | Legacy sites | Migration owner/scope |
| --- | ---: | --- |
| aida-core | 119 | R4: storage, config, home, tools and core fixture helpers |
| aida-cli | 65 | R4: CLI/MCP entry capture and contract fixtures |
| aida-cli-lib | 889 | R4: functional clusters after coordination with PR-C |
| aida-server | 31 | R4: explicit request/service boundaries |
| aida-tui | 21 | R4: UI/service request boundaries |
| tests | 90 | R4: Python harness reads and mutation bindings |
| **Total** | **1,215** | R4 slices B–D; not a zero-legacy claim |

All workspace members are discovered from Cargo.toml, including build scripts,
examples and Rust integration tests; `tests/**/*.rs` and `tests/**/*.py` are
included. No workspace member is exempt. Reads (`var`, `var_os`, `vars`,
`vars_os`), mutation (`set_var`, `remove_var`), and environment import bindings
are inventoried. All Rust mutations are tracked conservatively, including
production mutations. The only admitted capture is the one exact `vars_os()`
call in `Environment::capture`, with multiplicity one.

The guard recognizes direct/leading-`::` paths, grouped imports, module/function
aliases, raw identifiers, globs and function references passed into helpers.
A binding to an environment function is itself a source site, so helper
indirection does not exempt that source. Nested comments, raw/ordinary strings,
character literals and lifetimes have mutant coverage. Unrelated imports must
not be recursively expanded (e.g. `anyhow::{anyhow}` is not an env alias).
Python harnesses use AST scanning of os environment references, imports and
aliases, including references used to mutate `os.environ`.

This is a source-token ratchet, not a Rust compiler or whole-program taint
analysis. It does not resolve cross-module reexports, macro expansion, dynamic
Python imports, libc/FFI environment access, shell environment commands, or
third-party home/environment helpers. Original direct Rust sources/reexport
bindings and Python os references remain inventoried. Later migration slices
must audit these indirect consumers before claiming the final no-ambient-read
contract; this initial ledger is not proof of that final contract.

Run `python3 scripts/check-exec-context.py` and
`python3 -m unittest tests/test_exec_context_ratchet.py`. CI's fast gate also
compares against the PR base ledger: entries and ceiling may only shrink.
The first introduction bootstraps only when that base has no ledger. A missing
base commit or unreadable/malformed ledger fails. Locally use `--base REF` to
exercise the same growth check. After migrating a site, remove its entry (or
reduce its multiplicity), recompute the ceiling, and rerun the guard. Never
regenerate the ledger to admit replacement/new ambient reads or mutations.

## Seam inventory and remaining acceptance

The historical “14 AIDA_TEST seams” is not the present reader count. Separate
fixture keys from executable production override paths during migration:

| Cluster | Observed inputs | Migration handling |
| --- | --- | --- |
| Home and user config | `AIDA_TEST_HOME` in advisor, role, session, glyph config/glyphs, user aliases, forge profiles and core git ops | Explicit home; retain real-home fences until all readers move |
| Cache/store timing | `AIDA_TEST_CACHE_LOCK_WAIT_MS`, `AIDA_TEST_STORE_LOCK_CONTENDED_FILE` | Inject deadlines/fixture notifications; retain live locking |
| Forge probes | `AIDA_TEST_GH_BINARY`, `AIDA_TEST_FORGE_FALLBACK_DIRS` | Explicit tool/fixture inputs; no new tool trust |
| Shutdown/status timing | `AIDA_TEST_SESSION_END_PAUSE_AT`, `AIDA_TEST_SESSION_END_PAUSE_DIR`, `AIDA_TEST_NOTICE_DEADLINE_MS`, `AIDA_TEST_UNSHIPPED_SCAN_BUDGET_MS` | Clock plus fixture synchronization |
| Regeneration | `AIDA_TEST_ALLOW_NO_PYTHON3` | Explicit fixture dependency; no production bypass knob |
| Isolated test children | `AIDA_TEST_PATH_CHILD`, HOME namespace, FIFO/GC/barrier/compete/handle child keys | Preserve exact-child isolation until readers migrate; Command.env inputs stay supported |
| Harness executable selection | `AIDA_TEST_BIN`, `AIDA_TEST_AIDA_BIN`, `AIDA_TEST_CLAUDE_BIN`, real-Claude/model fixture selectors | External subprocess fixtures, not vendor forwarding policy |
| Required live capability | `AIDA_REQUIRE_BWRAP_LIVE`, `AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE` | Runner inputs; preserve across isolated test children, exclude vendor projection |

Slices B–D migrate functional clusters, replace fixture environment guards and
production test switches with explicit inputs, and prune the ledger. Keep the
current PATH subprocess safeguards and environment locks until their readers
move. The final ratchet is zero legacy reads/writers, including wrappers;
child `Command.env` setup is allowed. Required-live tests must fail when the
runner declares the capability and it is unavailable, errors or never runs.
Slice A does not alter these live tests and claims no live containment evidence.
Final acceptance still requires live CI at the reviewed head, the launcher
compatibility matrices, indirect-consumer audit, and the empty legacy ledger.
