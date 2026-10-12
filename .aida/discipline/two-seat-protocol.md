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
In the aida project, ADR-73 (`aida show ADR-73`) lets the advisor seat merge
an approved PR, whichever vendor runs the seat, only when all six of these
conditions hold:

1. **No recorded authorship.** The advisor's session, harness, seat grant and
   leases are absent from the PR's authorship ledger. Launching the implementer
   for the work counts as authoring it.
2. **Independent APPROVE at head.** An `aida review record` APPROVE on the
   current head from an identity separate from every author.
3. **Green CI on the pinned head.** The required Build check passed on that
   SHA, and the merge pins it (`--match-head-commit <full sha>` or `aida pr ship`).
4. **No holds.** No operator hold, merge-hold label or unresolved REQUEST CHANGES.
5. **Signoff for sensitive changes.** Architecture, security, approval-authority
   and identity changes carry the advisor or operator signoff on the owning spec.
6. **Audit comment.** After merging, comment on the PR or spec with who
   approved which head, the merged SHA and the CI run.

`aida pr ship` is the default merge path. Verified mechanical slice stacks are
the one exception: they may rebase-merge with `gh pr merge <n> --rebase
--match-head-commit <full sha>` once `scripts/split-lib/verify_stack.py` passes
and conditions 1–6 hold. The exception lasts until `aida pr ship` gains a
rebase-merge mode (STORY-1663).

Mechanical enforcement is still pending. It arrives with the identity-bound
approval gate in `aida pr ship` (BUG-1918). Until then, ADR-73 is policy, not
a control: the advisor must check the authorship ledger honestly. Other
projects need their own decision before an advisor merges.

Recurring work is not part of this prose contract. `aida schedule due --seat
product` and `aida schedule due --seat advisor` are the authoritative duty
lists; report completion with `aida schedule done <job>`.
