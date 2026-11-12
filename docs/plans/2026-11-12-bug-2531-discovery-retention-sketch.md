# Gate-ledger completeness and retention decision — proposed, not approved

<!-- trace:BUG-2531 | ai:codex -->

Round 2 findings 1 and 2 are valid. A scan-wide safety cap limits the
per-file-reset regression but does not complete BUG-2531. TASK-3098 remains
Draft; neither that task nor this sketch grants a scope split.

## Decision requested

Approve the full mechanism below, or explicitly approve a split keeping
BUG-2531 open for discovery completeness and automatic archive retention.
The existing partial Refs change must not receive completion credit.

The repository's sketch-first rule covers new file formats and cross-cutting
lifecycle semantics. This proposal introduces both. No approval is recorded.

## Proposed completeness mechanism (finding 1)

Use conservative write-ahead binding records, separately from the lossy by-spec
cache. Before appending any verdict for a spec/change pair, persist its binding
under the same ledger-wide/per-change writer locking protocol. A binding without
an event is harmless extra work; an event without a binding is forbidden for
upgraded writers. Failed binding writes prevent event append. Import, migration,
legacy capture and every verdict writer must use the same rule.

A versioned completeness marker is published only after a bounded, exclusive-lock
bootstrap has strictly scanned all preexisting active AND archived ledgers and
written their bindings. Bootstrap must be an explicit maintenance action, not an
unbounded fallback during lookup. Interrupted or unsupported bootstrap refuses;
missing metadata never means no refusals. Writer compatibility must be enforced
before trusting the marker: an older writer that can append without bindings
invalidates the guarantee. Approve a version fence or restrict all writers to
upgraded binaries before activation. Binding tampering/deletion must refuse,
including stale or missing metadata, rather than silently suppress candidates.

Selection reads only requested labels' complete bindings and their change
ledgers, with existing strict byte/event bounds and cache deduplication. Existing
by-spec indexes remain hints and cannot override complete binding evidence.

An alternative is to keep full scans permanently and accept bounded refusal
above the scan budget; this is simpler but changes the requested availability
contract and therefore requires an explicit scope decision.

## Proposed retention contract (finding 2)

Preserve whole ledgers and their complete binding records indefinitely. Do not
prune by-spec references merely because a change closes. Use one archive path
per change; all strict and lossy direct readers, selection, audit, carry-over,
legacy import and migration resolve it. Missing referenced evidence refuses;
conflicting active/archive copies refuse instead of choosing an approval.

A successful observed closed/merged state triggers automatic rotation under the
exclusive ledger-wide lock. Pending, malformed or failed forge evidence does
not rotate. Do not trust a verdict JSON field such as closed_by_merge as forge
closure evidence. Close observation must be wired to GitHub and GitLab lifecycle
paths, including externally closed changes discovered by maintenance.

Append/reopen acquires the appropriate locking boundary, restores the archived
ledger to active before reading the last sequence/importing/appending, and
retains every old refusal and hold. Readers must not see a transient absence
during rotation/revival: agree on a shared read lock or a recoverable move
protocol before implementation. Index and binding location changes are one
recoverable operation. Archive content cannot require an unbounded archive
scan on a normal selection.

## Required validation before completion

- Measure unrelated bytes and parse attempts with thousands of large ledgers;
  selection work follows relevant bindings after bootstrap.
- Missing/stale by-spec cache still discovers a newer refusal; missing or
  unsupported completeness evidence fails closed.
- Interrupt bootstrap and each binding/append boundary; no old approval wins.
- Verify downgrade/old-writer rejection, concurrency and directory/link attacks.
- Rotate more than 4096 closed changes, then select an unrelated small spec
  without active count exhaustion or archive-wide payload scans.
- Archived indexed refusal beats an active older approval; direct reads, audit,
  hold clearance, carry-over and import retain exactly the same history.
- Reopen/appended archived changes retain monotonically increasing sequences.
- Crash/interleave rotation, selection, revival and append without evidence loss.
- Unknown forge closure retains active history; both PR and MR paths are covered.

The round-2 cap regression measures actual reads and deserialization attempts
with a small injected budget. It is safety evidence only, not evidence for any
of the proposed completeness/retention guarantees above.

## Round 3 disposition — reviewed head ade7de408c3b

Both P2 findings are accepted and remain open. Requeueing the work without a
triage reason is not recorded approval of this proposal or of a scope split.
The disputed premise is that the existing partial remediation can satisfy
BUG-2531; neither the safety ceiling nor Draft TASK-3098 does so.

Finding 1 needs approval of the completeness mechanism and writer compatibility
boundary above. At this head, `store::write_events` synchronizes events before
calling `project::index_spec`; index failures only warn. Thus a missing binding
is an ordinary supported state today, not proof of absence. A regression that
passes by ignoring such a missing index would encode loss of refusal evidence.

Finding 2 needs approval of the archive resolution, rotation and revival
contract above. `store::read_file` resolves only `ledger_file`, and append
derives the next sequence from that active history. Moving files alone can
therefore make direct readers report absence and a reopened writer restart
the sequence. Pruning indexes cannot fix either failure.

The requested decision is concrete: approve the full completeness/version-fence
and archive/read/revival contract, or explicitly approve a partial scope with
both remaining requirements assigned and BUG-2531 completion withheld. Until
then, retain the design-fork publication veto rather than launch another
unchanged implementation/review round. This round changes only this decision
record; no runtime behavior or retention regression is claimed.

Navigation limitation: the configured host client
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph` is absent. The host must
deploy that reviewed client before indexed navigation is available. The
`aida doctor check codegraph` invocation reported zero checks and did not
establish client availability. Source observations above were checked directly.
