# Explicit constraint verification

Implements STORY-1673 under the advisor's signed 2026-10-15 08:35 schema and
operator option A. Independent STRICT review and operator merge are required.

The optional top-level `verify` value on a requirement remains data until the
explicit `aida verify constraints` command validates it. Retaining malformed
JSON-compatible blocks lets the report label them `invalid` without breaking
store loading. A closed Rust enum selects the six signed probes; no subprocess,
shell, transcript, sync, drain, or hook runs a probe. Validation happens before
any host lookup. Values compare as booleans or exact integers, memory in bytes.

The command reads canonical rows, selects constraint rows with a verify block,
and emits one match/drift/unknown/invalid result per row. No block means no
probe, including for legacy constraints. `--fail-on-drift` is the only
report-result exit gate; ordinary load/I/O command failures still fail normally.
Probe read failures become unknown. Missing paths/commands are known false.

Paths must be absolute without parent traversal, control characters, or shell
metacharacters. Command names use the signed ASCII allow-list and are only
looked up for executable regular files in PATH. Disk rotational state comes
from Linux sysfs for the block device (or its containing disk for partitions).
CPU count uses logical CPUs from procfs; memory uses MemTotal in bytes; PSI
checks availability of the three standard procfs pressure files.

Canonical/YAML serialization must preserve valid and invalid blocks, including
explicit null. Archived backends must refuse writes of a verify block rather
than silently discard it. No legacy feature/schema migration is added. A
data-only `aida edit --verify <JSON>` and `--clear-verify` flags use the
existing protected canonical edit path, with history and atomic block merging.
They do not execute a probe. Legacy edit flags refuse rather than disappear.
The explicit null block remains distinguishable from an absent block, including
when a concurrent edit merges. MCP gets no new tool parameters in this slice.

Verification: schema table covering every probe/type/op/unknown-key boundary;
injected host fixture reads for match/drift/unknown; executable sentinel proving
PATH lookup does not execute; shell payload rejection before lookup; valid,
invalid, absent and null row roundtrips; CLI parsing/report formatting and exit
gate tests; canonical readback; required fast gate and targeted Rust tests.

<!-- trace:STORY-1673 | ai:codex -->

## Verified source and binding evidence

At source `dddf4c5d97`, the final targeted core run passed all 12 new tests;
the final targeted CLI run passed all 6 tests. Existing merge, export and
edit/rebase regressions passed 60, 6 and 13 tests respectively using the fresh
compiled test artifacts. These are targeted runtime results, not a full
workspace runtime claim. Logs are `/tmp/regen-relay7-1673-{core-final,cli-final,
conflict,export,edit-rebase}.log`.

The generator ran successfully and its real output passed standalone TypeScript
checking (`--noEmit --skipLibCheck --target ES2020 --moduleResolution bundler
--module esnext shared/types.ts`). The generated diff includes JsonValue and
verify, required existing FilingProvenance/ExecutionMode/Origin declarations,
and previously stale existing model fields; ts-rs trailing spaces are normalized.
Semantic diff: `/tmp/regen-relay7-1673-types-semantic.diff`.

Three fresh-context AUTHOR source checks were used. Round 1 found sync field
loss and incorrect PATH permission detection; round 2 found creating/migrating
read paths and bad YAML arguments dropping rows. Both rounds' findings were
fixed. Round 3 reported no new blocking source finding and ran bounded checks of
the actual probe module. Subsequent binding declarations and portable fixture
strings received compiler/runtime/static checks. This is not independent review
or approval; no fourth author check was run.

The first fast-gate attempt failed only at the portability ratchet on hardcoded
sample payload paths. Fixture strings were corrected, and the isolated guard
passed. Publication requires a complete final fast-gate run at the committed
head; the exact completed result belongs in the PR and spec trail. Independent
STRICT review and operator merge remain outstanding.
