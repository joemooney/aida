# Reviewer verdict handoff

Status: proposed; requires advisor signoff before implementation.

<!-- trace:BUG-2564 | ai:codex -->

Choose option (a): the launch parent records a structured child draft after
the vendor exits. Do not introduce a credential into the vendor sandbox.

The existing standalone collector is not sufficient: it requires at least
one candidate carrying the child reviewer's recording grant. The standalone
prompt also explicitly requires `aida review record`, which can fail before
writing that candidate when tool-call ancestry is unavailable.

## Proposed boundary

1. Before launch, the parent pins the PR number, full head SHA, covered specs,
   reviewer child grant and an exclusive per-launch draft location inside the
   review worktree. Carry the absolute draft location and JSON contract in
   prompt text, including on resume; environment transport alone is insufficient.
2. The standalone child writes data only: version, PR number, full reviewed SHA,
   verdict, summary and complete findings. It must not supply recorder identity,
   attestations, release receipts, covered specs or ledger destinations. Update
   the standalone prompt and review skill so this route supersedes in-sandbox
   recording. A failed legacy record attempt must still leave this draft.
3. After exit, the parent opens only that launch's draft with the existing
   bounded, nonblocking, no-follow regular-file checks, including link count
   and directory anchoring. Reject malformed, oversized or ambiguous drafts;
   never recover approval by parsing conversational logs or replaying commands.
4. Revalidate the launch's reviewer grant, its subject binding and separation
   from the implementer/launcher. Require the full reviewed SHA to match both
   the launch snapshot and the freshly resolved forge head. Missing evidence
   refuses; an advanced head requires another review.
5. Pass the validated data through the normal record-time policy and authority
   checks using the parent's verified launch context, deriving recorder evidence
   from the child grant rather than trusting draft fields. Preserve publication,
   hold and finding semantics. Do not merely inject a grant ID into arbitrary
   JSON or relax the existing collector's stamped-source requirement.
6. Append the canonical ledger result with replay-safe launch/draft identity.
   A failed publication retains its recoverable pending state. Remove or mark
   the draft consumed only after durable recording; retries must not create
   another round. Conflicting legacy evidence refuses rather than choosing an
   approval. Interactive and existing grant-bound recording remain supported.

## Validation required after signoff

- A stub child with no bindable ancestry writes a complete draft after a
  refused record attempt; the parent records it at the exact head under the
  issued reviewer grant, preserving findings and summary.
- Reject wrong PR, changed/missing head, stale launch draft, invalid/expired or
  wrong-role grant, launcher/implementer identity, conflicting records, and
  draft-supplied authority fields.
- Reject symlinks, FIFOs, hard links and oversized/malformed data without hangs.
- Retry after recording/publication failure without duplicate rounds; preserve
  request-changes holds and existing interactive collection.
- Verify prompts choose the draft route for fresh and resumed standalone
  headless reviews, with explicit absolute paths.
- After deployment, observe 24 hours of headless logs for the identity refusal
  and distinguish missing drafts from rejected drafts. This operational
  acceptance cannot be established by lane tests.

## Navigation and implementation scope

Start at `standalone_reviewer_record_anchor`,
`launch_standalone_reviewer`'s launch/collection flow in
`aida-cli-lib/src/queue_cmd/work.rs`, `StandaloneVerdictChecks`, and
`collect_standalone_verdict` in `aida-cli-lib/src/gate/review_verdict.rs`.
Review prompt masters live under `aida-core/templates/skills/aida-review.md`;
regenerate mirrors through the repository template workflow.

The configured host query client
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph` is absent in this
seat; `aida doctor check codegraph` reports the tool missing. Host deployment
and its confirmed absolute path are needed for call-graph navigation.
