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
contains the repository path, enclosing named function (or module/import
binding), operation and SHA-256 prefix of the tokenized reference/call; its value
is the exact multiplicity. Function names are navigation hints, not Rust name
resolution. Whitespace/comments do not change call fingerprints; argument
or alias edits require retiring an old entry. Function scope ends at the body's
closing brace; nested functions restore the outer scope on exit. The total
ceiling is computed from the entries. Removed entries must be pruned.

File moves, split-lib slices and function renames may relocate ledger entries
without migrating their calls: update the navigation keys and retire the old
keys in the same change. The base comparison matches additions against retired
entries by operation plus exact expression digest, including multiplicity; the
total ceiling cannot grow. Retained entries provide no relocation capacity.
Changed expressions/operations and extra duplicate calls still refuse.

Migration ownership (including import bindings and Python harness references):

| Area | Migration owner/scope |
| --- | --- |
| aida-core | R4: storage, config, home, tools and core fixture helpers |
| aida-cli | R4: CLI/MCP entry capture and contract fixtures |
| aida-cli-lib | R4: functional clusters after coordination with PR-C |
| aida-server | R4: explicit request/service boundaries |
| aida-tui | R4: UI/service request boundaries |
| tests | R4: Python harness reads and mutation bindings |

The ledger's `ceiling` field is the source of truth for the current total;
per-area counts derive from its `sites` entries and multiplicities. This is
not a zero-legacy claim. Counts are not duplicated here because main merges
can change the bootstrap inventory before the ledger first lands.

All workspace members are discovered from Cargo.toml, including build scripts,
examples and Rust integration tests; `tests/**/*.rs` and `tests/**/*.py` are
included. Source paths come from `git ls-files`, with working-tree contents
scanned; new source files must be staged to participate. Untracked/ignored
scratch files and virtualenvs are excluded, deleted sources retire entries,
and non-UTF-8 tracked sources fail with their path. No workspace member is
exempt. Reads (`var`, `var_os`, `vars`,
`vars_os`), mutation (`set_var`, `remove_var`), and environment import bindings
are inventoried. All Rust mutations are tracked conservatively, including
production mutations. The only admitted capture is the one exact `vars_os()`
call in `Environment::capture`, with multiplicity one.

The guard recognizes direct/leading-`::` paths, grouped imports, module/function
aliases (including `extern crate std as alias`), raw identifiers, globs and
function references passed into helpers.
A binding to an environment function is itself a source site, so helper
indirection does not exempt that source. Nested comments, raw/ordinary strings,
character literals and lifetimes have mutant coverage, as do absolute paths
after Rust keywords (including `if`, `match`, `while` and `mut`). Unrelated
imports must not be recursively expanded (e.g. `anyhow::{anyhow}` is not an env alias).
Python harnesses use AST scanning of os environment references, imports and
aliases, including references used to mutate `os.environ`. Python fingerprints
hash a canonical JSON AST walk with sorted field names, ordered children and
typed literal values. Locations and absent optional fields are omitted,
including the removed `Constant.kind` field; interpreter-specific `ast.dump`
formatting is never hashed. Literal golden site keys for Rust and Python point
to encoding drift before a whole-ledger comparison fails.

Limits: this is a source-token ratchet, not a Rust compiler or whole-program taint
analysis. It does not resolve cross-module reexports, macro expansion, dynamic
Python imports, libc/FFI environment access, shell environment commands, or
third-party home/environment helpers. Extern-crate alias resolution is file-local;
aliases reexported across modules are outside this scan's resolution. Original
direct Rust sources/reexport bindings and Python os references remain inventoried. Later migration slices
must audit these indirect consumers before claiming the final no-ambient-read
contract; this initial ledger is not proof of that final contract.

Run `python3 scripts/check-exec-context.py` and
`python3 -m unittest tests/test_exec_context_ratchet.py`. CI's fast gate also
compares against the PR or merge-group base ledger: entries may shrink or
relocate identical operation/expression fingerprints; the ceiling may only shrink.
The first introduction bootstraps only when that base has no ledger. A missing
base commit or unreadable/malformed ledger fails. Locally use `--base REF` to
exercise the same growth check. After every merge/update from main and before
every push, run `python3 scripts/check-exec-context.py --write-ledger --base origin/main`.
This one command rescans tracked sources, writes sorted keys and multiplicities,
and computes the ledger's `ceiling`. It bootstraps only when the base has no ledger;
otherwise it checks the candidate against that base before writing and refuses
growth, leaving the existing ledger untouched. Missing or malformed base evidence
also refuses the write. Use the same command after migrating or relocating a site,
then rerun the guard and fixtures before requesting review. Never
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
