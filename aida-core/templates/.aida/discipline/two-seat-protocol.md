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
3. **The landing mode's CI on the pinned head.** In `ci-gated` mode (the
   default) the required Build check passed on that SHA. In `review-gated`
   mode only `merge-hold-gate` must pass first; CI runs after the merge (see
   Landing modes below). Either way the merge pins the head
   (`--match-head-commit <full sha>` or `aida pr ship`).
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
   the pinned head, with the CI condition 3 asks for. Architecture is signed at sketch time; the merge
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

### Landing modes

<!-- trace:STORY-2897 | ai:claude -->
`[landing] mode` on the default branch decides what a merge waits for:

- **`ci-gated`** (the default): `aida pr ship` waits for CI on the head, and
  several ready PRs land together through the merge train below. Choose it
  when a broken main is expensive: many changes land at once, a revert is
  slow, or nobody watches main after a merge.
- **`review-gated`**: a PR merges on an independent APPROVE at its exact head
  and no hold, without waiting for CI; CI runs on main after the merge. The
  advisor seat (or the operator at a terminal) is the merge hand, there is no
  train (the auto-train stands down), and `aida land run` ships each ready PR
  on its own. Choose it when review rounds queue behind CI and a red main is
  fixed or reverted quickly.

`aida land mode status|set|reconcile` shows and changes the mode with the
forge's required checks (see `docs/advanced/landing.md`).

The red-main watch reports a red main in both modes. It runs only as its own
scheduled job (`land watch`, every 5 minutes where the project enables it) or
by hand (`aida land watch`), never inside a landing path, so nothing that
merges waits on it. On a confirmed red it files a fix-forward bug at the top
of the implementer queue and opens an `aida human` item. It recommends a
revert, never performs one, when main does not build, fails the smoke test,
outlasts `[landing] red_main.fix_window` or needs a design decision.

### Merge train

<!-- trace:STORY-1690 | ai:claude -->
Two PRs that are each green can still break main together, because neither
one's CI saw the other (#2599/#2597). In `ci-gated` mode, when more than one
approved PR is ready, the merge seat lands them with `aida pr train <N...>`
(or `--all-ready`), the batched form of `aida pr ship`:

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
5. **Red trains bisect.** A train with a failed test is not rerun: it is
   split in halves at once, two sub-trains at a time. Only a train whose CI
   was cancelled before it gave a test result is rerun, once. A green half lands at once,
   and the other half is rebuilt on the new main and retested. A one-member
   train that is red leaves that PR open with a comment saying why.

`aida pr train --dry-run` prints each member's gate verdict and the planned
train without pushing anything; `--local-ci <CMD>` additionally simulates the
whole run with a local command as CI. Branch protection's "require up to
date" (`strict`) stays off; the train makes it unnecessary.

In `ci-gated` mode `aida pr ship` enforces the same rule for a single PR: it
refuses a PR whose head is behind main, since its green never saw main's newer
commits, and names `aida pr train N` (a one-member train). It does the same when the forge
does not report the branch as behind. No flag overrides the refusal. A head
that already contains main ships as before.

**Drain-to-train handoff.** The drain's merge phase applies the same refusal.
An approved, green PR whose head is behind main is shelved with the
`behind-base` cause (not an ordinary merge failure), and its hint names
`aida pr train N`. The merge seat collects those PRs and lands them with
`aida pr train N...` (or `--all-ready`); it does not update the branch and
re-run the drain.

**Unset execution mode.** <!-- trace:STORY-2884 | ai:claude --> A spec with
no `execution_mode` is supervised: `aida pr ship` and `aida pr train` hold its
merge for a human or a mandate. A project can change that with
`[gate.unset_mode]` in `.aida/config.toml` (see the configuration guide).
With `default = "drain"`, an unset spec of a type listed in `drain_types` gates
as `drain`. That never applies to a spec that sets its own mode, to a
sensitive spec (keystone, security, authority, containment or
`operator-only`), or to a PR that touches a protected gate path. Every other
gate still applies: an independent approval at the head, green CI and no
holds. The refusal text names the effective mode and its source, for example
`unset (supervised: not defaulted, the change touches ...)`, and `aida show`
prints `Effective mode: drain (project default for unset bug)`. The default
stays `supervised`. Turning the setting on for a project is the operator's
decision, made and recorded on its own, not as part of a feature PR.

Mechanical enforcement is still pending. It arrives with the identity-bound
approval gate in `aida pr ship` (BUG-1918). Until then, ADR-73 is policy, not
a control: the advisor must check the authorship ledger honestly. The
operator may supersede or revoke ADR-73 at any time with a successor decision.
It applies to the aida repository only; other projects need their own decision
before an advisor merges.

Recurring work is not part of this prose contract. `aida schedule due --seat
product` and `aida schedule due --seat advisor` are the authoritative duty
lists; report completion with `aida schedule done <job>`.
