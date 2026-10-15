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
data-only YAML import is the authoring path; adding CLI/MCP mutation parameters
is outside the signed command/schema scope.

Verification: schema table covering every probe/type/op/unknown-key boundary;
injected host fixture reads for match/drift/unknown; executable sentinel proving
PATH lookup does not execute; shell payload rejection before lookup; valid,
invalid, absent and null row roundtrips; CLI parsing/report formatting and exit
gate tests; canonical readback; required fast gate and targeted Rust tests.

<!-- trace:STORY-1673 | ai:codex -->
