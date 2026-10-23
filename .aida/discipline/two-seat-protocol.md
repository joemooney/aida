# Product–advisor two-seat protocol

The product and advisor seats cooperate, but remain independent. Product owns
intake, requirement quality, ordering, and keeping eligible work moving.
Advisor owns judgment gates: disposition, design forks, rework sufficiency,
and merge readiness. This separation prevents the author of a direction from
silently becoming its approver. <!-- trace:STORY-1351 | ai:codex -->

## Handoff

1. Product captures the need, sharpens acceptance, and routes buildable work.
2. Implementers build; reviewers write a verdict against the acceptance.
3. Advisor reads the actual verdict. A pass goes to the integrator. Requested
   changes become a concrete rework brief and return to the appropriate queue.
4. Product launches another eligible wave when the scheduler reports that the
   queue is ready and the drain lock is free.

Product never merges implementation or approves its own disputed judgment.
Advisor never writes the implementation it will later gate, never merges its
own code, and never clears an unresolved gate merely to preserve momentum.
The reviewer supplies evidence; the integrator performs only the mechanical
merge cascade. When a decision truly needs the operator, state the choice and
its consequences instead of guessing.

## Advisor merge authority

<!-- trace:TASK-1725 | ai:claude -->
In the aida repository, ADR-73 (`aida show ADR-73`, amended by ADR-75) lets the
advisor seat merge an approved PR, whichever vendor runs the seat, only when all
six of these conditions hold:

1. **No recorded authorship.** The advisor's session, harness, seat grant and
   leases are absent from the PR's authorship ledger. Launching the implementer
   for the work counts as authoring it. The agent's own judgment of what it
   wrote does not count.
2. **Independent APPROVE at head.** An `aida review record` APPROVE on the
   current head from an identity separate from every author (the advisor's
   own review counts when the advisor did not author the PR).
3. **Green CI on the pinned head.** The required Build check passed on that
   SHA, and the merge pins it (`--match-head-commit <full sha>` or `aida pr ship`).
4. **No holds (holds are absolute).** No operator hold, merge-hold label or
   unresolved REQUEST CHANGES.
5. **Signoff for sensitive changes.** An advisor or operator signoff recorded on
   the owning spec before merge is required ONLY when the PR changes:
   - (a) contained-sandbox / confinement: bwrap/OS-wrap, contained write roots,
     host-exec paths from contained sessions;
   - (b) approval authority, seat grants or identity: who may approve, merge,
     edit status or clear holds;
   - (c) credential or secret handling: tokens, credentials in
     argv/env/URLs/logs.
   Every other change merges on the merge seat's fresh, independent APPROVE at
   the pinned green head. Architecture is signed at sketch time; the merge
   reviewer verifies conformance to the signed sketch, and a deviation needs a
   new advisor ruling (dropping the second, merge-time architecture signoff).
   <!-- trace:TASK-1746 | ai:antigravity -->
6. **Audit comment.** After merging, comment on the PR or spec with who
   approved which head, the merged SHA and the CI run.

`aida pr ship` is the required merge path for both squash and rebase merges.
Verified mechanical slice stacks ship via `aida pr ship <n> --rebase` (or
`--merge-method rebase`), which runs the same preflight, independent approval,
merge-hold, and head-pin gates as squash and leaves the owning spec's status
unchanged until the final slice passes `--final-slice` (`--complete-spec`). When
`scripts/split-lib/verify_stack.py` is present and the PR contains
`refactor(cli-lib): extract|extend <m>.rs` slice commits, `aida pr ship --rebase`
runs the stack verifier (`scripts/split-lib/verify_stack.py <base> <head>`)
automatically before merging. Raw `gh pr merge --rebase` is no longer an
exception (STORY-1663).
<!-- trace:STORY-1663 | ai:antigravity -->

### Merge train

<!-- trace:STORY-1690 | ai:claude -->
Two PRs that are each green can still break main together, because neither
one's CI saw the other (#2599/#2597). When more than one approved PR is ready,
the merge seat lands them with `aida pr train <N...>` (or `--all-ready`), the
batched form of `aida pr ship`:

1. **Same gates per member.** Each PR must be open, not a draft (CI never runs
   Build on drafts, so a draft is refused with that reason), on the default
   branch, free of merge holds (marker and labels), approved at its head or
   carried over a clean merge from main (TASK-1960), not claimed by a
   reviewer for re-review at that head, with a clear verdict corpus, not
   owned by a live drive, and with no spec tagged `review:draft-only`. A spec
   whose execution mode needs supervision lands only for a human at a
   terminal who named that PR, or under an active mandate's cover, as with
   `aida pr ship`; a PR swept in by `--all-ready` always needs the mandate. The
   train never releases a hold. It is GitHub-only for now; it refuses a
   GitLab project.
2. **Attestation carry-over by patch-id.** The train is main plus one squash
   commit per member, with the subject `aida pr ship` would write (including
   `(#N)` and the completing trailer). Each commit's patch-id must equal the
   member's approved diff (`merge-base(main, head)..head`). A member that
   conflicts, or whose diff changes when stacked, is dropped with its reason
   and never auto-resolved. That proof is the train's authority to land the
   members (advisor signoff 2026-10-19).
3. **CI on exactly what main receives.** The stack is opened as a `train/*`
   PR. Required checks run on it unchanged. merge-hold-gate's train mode reads
   the `aida-train-members:` line and fails if any member carries a hold
   without a recorded clearance. That line is advisory: the gate does not
   check it against the commits' `Aida-Train-Member:` trailers, so a
   hand-made `train/*` PR can name any hold-clear PRs (no more than an
   ordinary unlabelled PR could). The binding member check is `aida pr
   train`'s own gate and its pre-land recheck.
4. **Land pinned, only on an unmoved main.** Under the merge lease, the train
   re-gates every member, refuses if main moved since the build (it rebuilds
   and retests instead), checks main once more right before merging, then
   rebase-merges the train PR with `--match-head-commit <tip>`. If the landed
   first commit's parent is not the tested base, it warns loudly. Each member
   PR is closed with a pointer to its commit on main, and `aida pull`
   completes specs from the trailers. A merge lease held by another merger
   means main is about to move, so the train rebuilds instead of aborting.
   A forge error mid-run closes the open train PRs, comments on every member
   left unlanded, and is reported in the run's summary.
5. **Red trains bisect.** A red train is rerun once to rule out a flake, then
   split in halves, two sub-trains at a time. A green half lands at once,
   and the other half is rebuilt on the new main and retested. A one-member
   train that is red leaves that PR open with a comment saying why.

`aida pr train --dry-run` prints each member's gate verdict and the planned
train without pushing anything; `--local-ci <CMD>` additionally simulates the
whole run with a local command as CI. Branch protection's "require up to
date" (`strict`) stays off; the train makes it unnecessary.

`aida pr ship` enforces the same rule for a single PR: it refuses a PR whose
head is behind main, since its green never saw main's newer commits, and
names `aida pr train N` (a one-member train). It does the same when the forge
does not report the branch as behind. No flag overrides the refusal. A head
that already contains main ships as before.

**Drain-to-train handoff.** The drain's merge phase applies the same refusal.
An approved, green PR whose head is behind main is shelved with the
`behind-base` cause (not an ordinary merge failure), and its hint names
`aida pr train N`. The merge seat collects those PRs and lands them with
`aida pr train N...` (or `--all-ready`); it does not update the branch and
re-run the drain.

Mechanical enforcement is still pending. It arrives with the identity-bound
approval gate in `aida pr ship` (BUG-1918). Until then, ADR-73 is policy, not
a control: the advisor must check the authorship ledger honestly. The
operator may supersede or revoke ADR-73 at any time with a successor decision.
It applies to the aida repository only; other projects need their own decision
before an advisor merges.

Recurring work is not part of this prose contract. `aida schedule due --seat
product` and `aida schedule due --seat advisor` are the authoritative duty
lists; report completion with `aida schedule done <job>`.
