# BUG-2500: separate review waits from implementer changes

<!-- trace:BUG-2500 | ai:codex -->

Status: approved by advisor ruling 2026-11-09 01:50 UTC: single `waiting`
verdict and fresh-review resume rule. Implementation must accompany this sketch;
the docs-only head must not merge alone. Launcher punt publication veto remains.

## Problem and observed code paths

The reported PR-3671, PR-3651 and PR-3657 failures describe CI waits or
host-only evidence being recorded as request-changes. This investigation
inspected source; it did not independently retrieve those PRs.

`gate/review_verdict.rs::VerdictKind` has approval, request-changes,
rejection and unknown outcomes. Its `blocks_done()` predicate is also used
to select rework findings, arm Rework holds and identify stranded work.
`queue_cmd.rs::resolve_rework_findings` selects spec/change aliases, and
`review_verdict_display.rs::rework_findings_comment` renders blocking findings.
The queue rework path compares consecutive rendered blocks and files
"Repeated unchanged review findings" before refusing another requeue.

`auto_complete.rs::Verdict` separately maps reviewer outcomes into
`VerdictRequestChanges`/`VerdictReject` phase failures. `gate/effective.rs`
and `gate/review_verdict.rs::reconcile_artifacts_for_sha` also project the
closed vocabulary. Changing only a reviewer prompt or parser would leave
these consumers inconsistent.

## Proposed contract

1. Add one canonical `waiting` verdict, displayed as `WAITING`, for review
   blocked solely by external evidence or action. Require a summary naming
   the dependency, responsible actor and resume condition. Examples:
   "CI: required Build is pending; resume review when this head settles"
   and "Host: run the mandated host-only probe; resume with its evidence".
   Keep this additive to existing JSON/ledger record shapes.
2. Separate merge/completion blocking from implementer actionability.
   Waiting is never an approval. Only request-changes/rejected produce
   implementer rework findings or count toward recurrence. Audit every
   `blocks_done()` caller rather than globally changing its meaning.
3. Reject queue rework/pickup for an effective waiting review before status,
   queue, calibration or worktree mutations. Report its dependency and
   resume condition. Existing explicit operator overrides must retain their
   existing audit/authority rules; introduce no silent override.
4. Preserve a distinct waiting state through ledger projection,
   reconciliation, status output, standalone reviewer collection and the
   orchestrator handshake. Stop before merge without consuming an
   implementer retry, filing a repeat finding, or arming a Rework hold.
   Retain independent holds. Mixed substantive findings still require
   request-changes; a wait cannot conceal those findings.
5. A wait ends through a fresh recorded review against the relevant head.
   CI green alone does not convert waiting to approval. A changed head also
   needs ordinary current-head review. Do not add a model poller, automatic
   resume service or historical prose classifier in this fix.
6. Update generated reviewer prompts, vendor-neutral role guidance, review
   skill templates and CLI documentation together. Waiting on CI or host
   evidence alone is not an implementer finding; failing checks caused by
   code defects still are.

## Proposed validation

Use the existing CLI-lib fixtures for a targeted BUG-2500 regression that
records waiting under spec and PR aliases, round-trips the canonical word,
checks merge/completion refusal, and attempts repeated rework. Assert no
implementer queue entry, no recurrence finding, no status mutation and no
Rework hold. Verify a later approved recording clears the wait through
ordinary review selection, while substantive request-changes still trigger
the existing repeated-findings guard. Cover an advanced head so waiting
does not accidentally inherit the old refusal's advance-past clearance.

Run targeted tests through `aida lane job run -- cargo test --profile agent
-p aida-cli-lib --lib -- bug_2500`, requiring an executed-test result, then
`make check-ci-fast`, per the advisor's LIGHT gate. No implementation or
test execution is claimed by this sketch.

## Advisor decision

The binding ruling of 2026-11-09 01:50 UTC approves one `waiting` verdict
and the fresh-review resume rule. Separate waiting-ci and host-action-required
words were declined because both resume by a fresh review at the head.
Implement on the PR-3698 branch with the validation above; the docs-only head
must not merge alone. Retain the launcher's punt publication veto. No resume
service or historical prose classifier is authorized.
