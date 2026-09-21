---
name: feedback_reconstruct_provenance_by_bracketing
description: "Date an artifact by its own event, never by a remembered wall-clock, an mtime, or an updatedAt field; then bracket that instant with the nearest durable records on either side to recover a reason nothing stored. Bracketing recovers a reason, it does not verify one."
type: feedback
propagation: scaffolding-pack
---
When an artifact has no durable record of *why* it exists (a label, a gitignored marker, a flag with
no comment), don't give up and don't guess. Two steps, in order:

1. **Date it by its own event.** A label is dated by its own `labeled` timeline event, not by the
   marker file's mtime, not by a record's `updatedAt` field, and never by your memory of when you ran
   a command. Sizing a gap off the wrong clock doesn't just blur the number — it can flip which side
   of an event you land on. A measured mis-read: an `updatedAt` timestamp was used to size a gap and
   the conclusion came out roughly fifteen hours in the wrong direction, making a review-gate failure
   that had actually landed one second after the label look like the label predated any evaluation.
2. **Bracket the instant.** Find the nearest durable records on each side of that dated event and read
   what they say. A hold's own reason can survive nowhere in the hold itself, while an adjacent
   request-changes record and an adjacent verdict record — one just before, one just after — place the
   hold squarely inside a review cycle whose recorded reason *was* the hold's reason.

The same rule applies one level up: date a CI (or any automated) run by its **trigger** event, not its
timestamp — a run list with no trigger/event column cannot distinguish a label-triggered run from a
stale, unrelated one.

**Why:** the artifact that dates a thing is the thing's own event; everything else — mtimes,
`updatedAt`, recollection — is a *different* clock, and a conclusion built on the wrong clock is wrong
in direction, not just in precision. Bracketing then recovers information the substrate never stored
by triangulating from what it did store.

**How to apply:** before claiming an enforcement mechanism did or did not fire, read its own event and
the trigger of whatever ran immediately after it. Before concluding a reason is lost forever, list the
durable writes on either side of the moment in question and read them as a pair.

**The caveat that must ship with the technique, because it is load-bearing, not a closing line:**
bracketing **recovers** a reason; it does not **verify** one. What comes back is what the adjacent
records say, not necessarily what the acting person actually intended — the two may coincide and need
not. Always label bracketed output as *reconstructed*, never cite it as first-hand provenance, and
**name the specific records that bracketed it** so a later reader can re-judge the inference for
themselves. A reconstruction whose brackets are named can be re-checked; one asserted bare becomes
uncheckable "provenance" in whoever's next summary repeats it. A reconstruction that succeeds is also
not evidence that the underlying missing-durability defect is mild — it usually just means the
artifact happened to be created inside a cycle that left records nearby. The instance that matters most
is the one armed *between* cycles, with nothing nearby to bracket, and it is exactly the least
guessable case from context.

Related: [[feedback_check_ignore_names_the_winner]]
