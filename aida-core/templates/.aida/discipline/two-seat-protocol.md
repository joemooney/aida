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

Recurring work is not part of this prose contract. `aida schedule due --seat
product` and `aida schedule due --seat advisor` are the authoritative duty
lists; report completion with `aida schedule done <job>`.
