# BUG-1747 AC4 — architecture sketch: should a failed merge-hold label sync fail closed?

- **Spec:** BUG-1747 (acceptance criterion 4)
- **Seat:** advisor
- **Date:** 2026-09-30
- **Status:** signed off by the advisor seat; AC1–AC3 and AC5–AC6 implementable against it
- **Scope of this sketch:** the *fail direction* only. It does not design the doctor probe (AC1),
  the provisioning command (AC2), or the error taxonomy (AC3) beyond the contract they must honour.

## The question as filed

> Decide and record whether a failed label sync should **block** the hold-dependent flow rather
> than only recording `Unsynced` — i.e. whether Layer 2 being unavailable should fail closed.

## What the call graph actually is

`merge_hold::sync_label(root, pr, held)` has **9 live call sites**. They are not one flow, and the
fail direction is not uniform across them. Measured on `bug-1693-rework` @ `043c1845ab`:

### Arming sites (`held = true`) — 6

| site | flow | on `Err` today |
|---|---|---|
| `lib.rs:35384` | `aida merge-hold add <pr>` — operator command | prints, **exits 0** |
| `lib.rs:34910` | `aida merge-hold list --fix` — the remediation path itself | prints |
| `lib.rs:58654` | auto-merge refusal — already `return false` | prints |
| `lib.rs:92919` | hold armed when a review verdict is recorded | prints to stderr |
| `lib.rs:105071` | drain arms the hold at PR detection | prints to stderr |
| `lib.rs:109969` | drain/auto-complete arms the hold | prints to stderr |

### Clearing sites (`held = false`) — 3

| site | flow | on `Err` today |
|---|---|---|
| `lib.rs:35603` | `aida merge-hold clear <pr>` | warns |
| `lib.rs:35660` | `--sweep` of stale holds | warns |
| `pr_cmd.rs:2608` | `aida pr ship` releasing a hold | warns |

In **every** case the marker is written (or removed) *before* the label call, and the label call's
`Err` is swallowed into a print. That uniformity is what makes the current behaviour read as a
policy when it is really an absence of one.

## Three facts that decide the answer

**1. The clearing sites already fail closed, by construction.** A failed *removal* leaves
`aida:merge-hold` on the PR, so the required `merge-hold-gate` check stays red and the merge stays
shut. `pr_cmd.rs:2610` says so in as many words ("branch protection will keep the merge closed").
Making these sites block would strand an operator who cannot release a hold **because the release
half failed** — it converts a safe failure into an availability outage. **No change here.**

**2. Layer 1 is already a real gate at every AIDA-mediated merge, and it does not need the label.**
`pr_has_merge_hold` (`lib.rs:45383`) is `marker.is_some() || label_present` — a disjunction, so the
local marker alone blocks. `aida pr ship`, drain's auto-merge, and the status/cleanup paths all go
through it or through `read_hold*` directly. So when the label never lands, an AIDA-mediated merge
is **still refused**.

**3. Therefore the fail-open exposure is exactly the merges AIDA never sees.** A human or an agent
running `gh pr merge` directly is gated by the required `merge-hold-gate` check and by nothing else.
That is the hole BUG-1747 describes, and **it is not reachable from the arming site's return value**:
by the time the unsupervised merge happens, the `aida merge-hold add` process has long exited.

This is the crux. Blocking at the sync site cannot close the hole, because the hole is in a process
AIDA does not run.

## Options considered

**Option A — report-only (status quo).** Keep the print and `LabelState::Unsynced`.
**Rejected.** This is BUG-1236's regression verbatim, and `sync_label_with`'s own doc comment names
it. An `Unsynced` record is a *report*; nothing reads it as a precondition. The spec exists because
A is insufficient.

**Option B — abort the hold-dependent flow on sync failure.** Return `Err` upward: abort
`merge-hold add` non-zero and un-write the marker; abort drain's arming; abort verdict recording.
**Rejected, and it is strictly worse than A.** It trades a one-layer failure for a zero-layer
failure — the marker is the half that works offline and is the half that actually stops every
AIDA-mediated merge (fact 2). It also converts a transient forge 502 into "drain proceeds with no
hold at all", which is the precise outcome the spec calls unacceptable. Aborting *without*
un-writing the marker is not Option B; it is Option C's exit-code limb.

**Option C — fail LOUD and fail closed at the enforcement point, never at the sync site. ADOPTED.**

## Adopted design

**Invariant:** *The local marker is never sacrificed for the forge label. A hold whose Layer 2 could
not be armed is reported as partially armed, exits non-zero when a human asked for it, and stays an
open integrity finding until the label lands.*

Five limbs:

1. **Marker first, always; never roll it back on a label failure.** Codifies today's accidental
   behaviour as a stated rule, with a test. Layer 1 is the floor.

2. **Distinguish definition-missing from transient** (this is AC3, and the fail direction depends on
   it). `sync_label_with`'s error becomes a typed variant: a *deterministic, operator-fixable*
   `LabelDefinitionMissing { names }` versus everything else. The discriminator is verified in the
   spec body: `gh pr edit` exits 1 with `'<name>' not found` when a name is **not defined in the
   repository**, atomically applying nothing. Do **not** retry a definition-missing failure — the
   existing 1 s retry is for transients and burns a second to no purpose here.

3. **Exit code, at the operator-invoked site only.** `aida merge-hold add` exits **non-zero** on
   `LabelDefinitionMissing` (deterministic; the operator must run the provisioning command) and
   keeps **exit 0 with a warning** on a transient (retryable; `--fix` exists). A script that places a
   hold and checks the exit status is then not misled about Layer 2. The autonomous sites
   (`:92919`, `:105071`, `:109969`) **never** abort their flow — they emit the typed event and carry
   on with Layer 1 armed. `--fix` (`:34910`) stays report-only; it is the remediation.

4. **The durable finding is the real fail-closed limb.** `LabelState::Unsynced` is upgraded from a
   log line to a state that the `merge-hold-integrity` doctor category reads, and that does not
   clear until the label is observed on the forge. A live hold with `Unsynced` is a doctor finding
   and an `aida human` item. This is what makes the condition un-ignorable without making the
   substrate un-operable — and it is the only limb that reaches the operator *before* the
   unsupervised merge.

5. **Prevention beats detection here: AC2's provisioning is the primary fix.** A definition that
   exists cannot be missing. The sketch's judgement is that AC2 (idempotent provisioning) and AC1
   (the repo-level definition probe) carry most of this spec's value, and AC4's answer is
   deliberately modest so they are not delayed by it.

## Implementation notes the implementer must not get wrong

- **`ForgeLabel` is the wrong type for AC1's probe.** `ForgeLabel::{Present, Absent, NoForge,
  Unknown}` (`merge_hold.rs:1394`) is derived `from_fetch` of a *pinned change* and means
  "is this label on this PR". `Absent` is the correct, healthy state of every cleared PR. AC1 needs a
  **repo-scoped, PR-independent** probe of label *definitions* (`gh label list` / the labels
  endpoint). Conflating the two would make the probe fire on every clean PR. Add a distinct type;
  do not widen `ForgeLabel`.
- **One undefined name poisons a comma list.** `--add-label "aida:merge-hold,aida:merge-hold-recorded"`
  exits 1 and lands *neither*. AC6's unit test pins this against the argv builder's contract. It also
  means provisioning must cover **all three** names before any hold is placed, not just the one
  being added.
- **GitLab self-provisions and GitHub does not** (per the spec's branch survey, unverified against a
  live GitLab). The probe and the provisioning path must tolerate a forge where this is a no-op
  rather than asserting a uniform need.
- **Degrade to "could not determine", never to a pass.** AC1 says this; it is also limb 4's
  precondition. An unreachable forge or a token without scope is `Unknown`, which is a finding of its
  own kind — not silence.

## Advisor signoff

Signed off on the record above: **Option C, not Option B.** The sync site does not block; the marker
is never rolled back; the operator command exits non-zero only on the deterministic class; the
durable finding is the enforcement. AC1, AC2, AC3, AC5 and AC6 may proceed against this contract.

A later spec may revisit limb 3 for the autonomous sites **if** evidence appears that a drain run
armed a Layer-1-only hold and an unsupervised merge then landed anyway. That evidence does not exist
today, and designing for it now would cost drain availability on every forge hiccup.
