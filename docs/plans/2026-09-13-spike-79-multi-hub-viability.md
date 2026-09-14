# SPIKE-79 — Is multi-hub development viable for an AIDA project?

Date: 2026-09-13
Specs: SPIKE-79
Status: Assessment complete (recommendation below)
Complexity: Medium (assessment); High (if "active multi-hub" is pursued)

## Question

Can an AIDA project sustain genuine **multi-hub development** — two live git hubs
that both take writes — rather than the current *canonical + mirror* shape
(github `origin` is the writer of record; self-hosted `gitlab.joemooney.com` is a
best-effort mirror)?

Motivated by the operator wanting gitlab.joemooney.com kept in decent shape and
possibly *actively maintained* later.

## Today's shape (verified this session)

- `origin` (github) is canonical for both `main` and the orphan `aida-store`.
- gitlab is a mirror: `[store.sync] mirror_remotes = ["gitlab"]` fans the store
  push out best-effort. In practice it had drifted (aida-store +210, main +2)
  because the fan-out is best-effort ("warns, never fails") and the glab token
  had expired — so the mirror silently fell behind.
- Reconciled by fast-forward (mirror was cleanly *behind*, not diverged): no
  force-push needed. Release v0.15.0 + 4 assets created on gitlab.
- `aida remote status` / `aida doctor --category remote-drift` already *detect*
  the three legs (main, aida-store, releases) drifting.

So the mirror model works, but its sync is one-directional and best-effort, and
it degrades silently when auth breaks.

## What "active multi-hub" would require (the real concerns)

1. **The aida-store orphan branch is the writer of record.** Two hubs both
   taking spec writes need conflict-free merge. `conflict.rs` unions the
   append-only `oplog.yaml` (CRDT-style) on merge — that is the substrate for
   this — but it has only ever run canonical→mirror. Bidirectional active writes
   need a real reconcile path (`aida remote reconcile` union-merge) exercised and
   tested under genuine concurrent divergence, not just fast-forward.

2. **Node id / spec-id allocation across hubs.** The dispenser/node model
   allocates stable spec ids from a node identity. Two hubs allocating
   concurrently must not collide. Needs verification that the node-id scheme
   partitions the id space (or that merge resolves collisions) when two active
   hubs both file specs offline.

3. **One-way mirror vs bidirectional reconcile.** `mirror_remotes` fan-out is a
   push, not a merge. Active multi-hub needs pull-and-union from each hub, drift
   detection on a cadence, and a reconcile that never force-pushes a shared
   branch (the standing safety rule).

4. **Release / tag / asset fan-out.** Tags mirror via git, but *release objects*
   + binary assets do not (created v0.15.0 on gitlab by hand this session). A
   multi-hub release story needs the release + assets fanned to every hub, and
   the drift detector must recognize a release whose assets are attached as
   *links* (BUG-1137 / TASK-1223 — the detector currently false-negatives).

5. **Auth durability.** The mirror silently fell behind on an expired glab
   token. Active multi-hub magnifies this: every leg's auth must be monitored,
   and a failed leg must surface loudly (not "warn, never fail" into silent
   drift).

## Recommendation

**Do NOT invest in active bidirectional multi-hub now.** Keep the
**canonical(github) + mirror(gitlab)** model, and harden the mirror instead —
that gets "gitlab in decent shape / actively maintained" at a fraction of the
cost and risk:

- **Fix the silent-drift failure mode** (the actual pain): make the store
  fan-out failure *visible* — a failed mirror leg should raise a doctor finding
  / notify, not just warn into drift. (Small, high-value.)
- **Complete the release-drift detector** (TASK-1223 / BUG-1137) so gitlab
  releases are recognized without manual env setup.
- **Keep `aida remote status` / `doctor remote-drift` as the health surface**;
  reconcile by fast-forward when the mirror is merely behind.

Revisit *active* multi-hub only if a concrete need appears (a second team/site
genuinely committing to gitlab). At that point the gating work is, in order:
(1) exercise + test `conflict.rs` union under real bidirectional divergence,
(2) verify node-id/spec-id allocation across concurrent hubs,
(3) a tested `aida remote reconcile` bidirectional path,
(4) release+asset fan-out.

## Followups (filed / to file)

- TASK-1223 — release-drift check reads the glab config token (in flight).
- Consider a small task: "store mirror fan-out failure raises a doctor finding
  instead of silent drift" (the highest-value hardening from this assessment).

## Related

- `docs/multi-hub-sync.md` (the three legs + reconcile procedure)
- `docs/git-verb-surface.md` (two-leg mirror verbs)
- `aida-core/src/conflict.rs`, `dispenser.rs`, `node.rs`
