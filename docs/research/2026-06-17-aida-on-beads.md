# AIDA on Beads? — storage-abstraction compose-vs-own (SPIKE-65)

- **Date:** 2026-06-17
- **Probe:** EPIC-48 (build-vs-buy-vs-compose). Spec SPIKE-65.
- **Question:** can AIDA sit *on top of* Beads via a storage-abstraction layer (a `BeadsBackend` impl of `DatabaseBackend`), and does composing-on-Beads beat owning the store?
- **Verdict:** **Composing on Beads is not recommended.** A one-way **Beads→AIDA importer** is the right "compose" play. The honest EPIC-48 finding: *composing on a neighbor trades one lock-in for another and adds a thick impedance layer; the value of owning the store is portability + inspectability, not cost.*

## The storage/model split (what a backend actually owns)

AIDA's `DatabaseBackend` trait (`aida-core/src/db/traits.rs`) is the seam: backends implement I/O + transactions + caching + locking + versioning over a `RequirementsStore`; the **model is invariant across backends** (no backend gets to reshape it). Five impls exist (Yaml, Sqlite, Postgres, Git, CachedGit). A `BeadsBackend` would be a sixth — but Beads is not just a different *store*, it's a different *model*.

## The impedance (why the adapter is thick, not thin)

| AIDA concept | Maps to Beads? |
|---|---|
| issue ↔ spec, title/desc, tags, parent-child, blocks | **clean** |
| 7-state enforced lifecycle (Draft→…→Completed, NeedsAttention) | **lossy** — Beads has 3 unenforced states; can't enforce AIDA's approval gate |
| per-field `history[]` audit (field/old/new) | **lossy** — Beads is event-level, not field-level |
| processing_record, attention_reason, decision_request, intent, interface_changes | **lossy** — no Beads equivalent |
| typed relationship taxonomy + `RelationshipDefinition` constraints | **lossy** — Beads' taxonomy is smaller + unconstrained |
| code-to-spec `trace:` links | **impossible** — no Beads equivalent (AIDA's uncontested wedge) |
| orphan-branch git-canonical YAML | **impossible** — Beads v1.0 is Dolt (versioned SQL) source-of-truth |
| distributed IDs (HLC + dispenser + blocks) | **impossible/unknown** — Beads' scheme undocumented |
| the queue / coordination / team / RBAC layers (this week's work) | **impossible** — AIDA-specific orchestration |

A `BeadsBackend` is therefore a ~3–4× thicker translation layer than `SqliteBackend` (Dolt mapping + a lifecycle-enforcement wrapper + history reconstruction + an ID bridge + queue stubs), coupled to a **fast-churning** target (Beads removed SQLite in v0.58, made Dolt canonical in v1.0, demoted `issues.jsonl` to optional). Coupling AIDA's correctness to a moving SQL schema is the opposite of a stability win.

## The cost/benefit, honestly (the EPIC-48 point)

- **"Composing reduces roll-your-own cost" — false here.** AIDA doesn't roll a store from scratch; it adopts proven backends (YAML/SQLite/Postgres) behind a thin trait + adds distributed-ID coordination. Replacing that with "bridge Dolt + translate an unstable, partly-undocumented schema + stub the AIDA-only layers" is a cost **shift to a less-proven target**, not a reduction.
- **"Zero lock-in" — false.** It trades AIDA's git-YAML lock-in (inspectable, no engine required) for Beads/Dolt lock-in (SQL engine required) **plus** a maintenance tax on schema churn.
- **What owning the store actually buys:** git-readable portability, full lifecycle authority, per-field audit, and code-to-spec traces — none of which survive the Beads round-trip intact.

## The right "compose" move instead

A **one-way Beads→AIDA importer** (small effort: a JSONL/`bd export` reader + field-mapper + type-classifier): let Beads' large user base opt *into* AIDA's governance layer (approval gate + traces + per-field history) without AIDA inheriting Dolt. This is "give neighbors a path to your wedge," not "become a neighbor's backend." (Verify the live `bd export` schema first — it churns.) Builds on the SPIKE-44/46 interop contract (substrate **open to read**, writes converge on one conformant writer).

## Carried back to the theory paper

- Strengthens §12 (roll-your-own verdict): the compose option is not free; the adapter is the cost. "Own the store" is justified by **portability + inspectability**, and the only uncontested differentiator is **code-to-spec traces** (which require the enforced lifecycle Beads lacks).
- Honest limit: Beads' exact `issues.jsonl` schema, relationship taxonomy, ID scheme, and actor attribution are UNKNOWN from in-repo docs — verify against a live `bd export` before any importer work. Several competitor claims here are dated observations, not standing facts.

<!-- trace:SPIKE-65 | ai:claude -->
