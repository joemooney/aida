# SPIKE-46 — multi-vendor substrate access: measured, not asserted

**Status:** findings + write-conformance contract · **Spec:** SPIKE-46 (child of SPIKE-44) · **Date:** 2026-05-31

The moat thesis is that AIDA's git-canonical store is an *open knowledge
substrate* a whole fleet of tools can share. SPIKE-44's serialization-surface
inventory (`../on-disk-serialization-surface.md`) argued, from source, that
**read interop is cheap and write interop is bounded**. This spike *measured*
it: `store_reader.py` is a ~120-line, pyyaml-only, NON-AIDA tool run against
this repo's live store (1635 real objects). Reproduce with:

```
python3 docs/architecture/spike-46-store-interop/store_reader.py
```

## What the probe measured (1635 objects)

| Probe | Result |
|---|---|
| READ — stock `yaml.SafeLoader` | **1203/1635** parsed; **432 failed — every one on the `!Custom` YAML tag** |
| READ — SafeLoader + one `!Custom` constructor | **1635/1635** parsed; id+status+relationships reachable on all |
| ROUND-TRIP — pyyaml re-emit vs AIDA's bytes | **0/1635 byte-identical**; **1635/1635 semantically equal** (re-parse == original data) |
| First divergence (ADR-1.yaml, line 10) | AIDA `created_at: 2026-05-09T18:10:08.298638987Z` vs pyyaml `2026-05-09 18:10:08.298638+00:00` |

## Reading the result

**READ is easy-with-one-trick.** A naïve consumer breaks on ~26% of the store —
not on anything exotic, just AIDA's `RelationshipType::Custom`, which serde
emits as a `!Custom <name>` YAML tag that a stock loader has no constructor
for. A *three-line* tag handler (see `AidaLoader` in the prototype) lifts read
to 100%, with every consumer field — `id`, `status`, `tags`, `relationships`
(all five kinds: Parent, Child, BlockedBy, References, Custom) — reachable. So
**any tool can consume the requirement graph trivially**, once it knows the one
tag. That is the multi-vendor win: dashboards, analytics, and *other agents*
can read the substrate directly, no AIDA binary required.

**WRITE is data-safe but not byte-clean — it's an *emitter* contract.** A stock
emitter round-trips the *data* perfectly (1635/1635 semantic) but matches
*zero* objects byte-for-byte. The divergence is pure formatting, and it matters
because AIDA's `write_object_if_changed` (object_store.rs) compares serialized
bytes to skip no-op writes and keep the orphan branch's history meaningful — a
non-conformant writer produces a spurious git diff on every save. The measured
divergence is precise: pyyaml reparses RFC3339 timestamps into datetimes and
re-emits them with a space separator and **microsecond** (lossy) precision,
where AIDA emits nanosecond-precision `...Z` strings.

## The write-conformance contract (what a conformant non-AIDA writer must do)

Grounded in the inventory + this probe. A writer that satisfies these produces
byte-identical, diff-clean YAML:

1. **Timestamps are opaque strings.** Emit `created_at`/`modified_at`/etc.
   verbatim as RFC3339 nanosecond-`Z` strings; never let the YAML library parse
   them into a date type (that is the *first* and most common divergence).
2. **Field order = struct declaration order.** serde_yaml emits in the order
   fields are declared on `Requirement` (id, spec_id, agreed_id, title,
   description, status, …). A read-then-re-emit tool inherits this for free
   (insertion order is preserved on load); a from-scratch writer must replicate
   it.
3. **Sorted collections.** `tags` (set) and `custom_fields` (map keys) ascending
   — AIDA's `yaml_helpers::serialize_sorted_*`.
4. **`RelationshipType`** emits the derived shape, incl. `!Custom <name>` tags;
   omit `created_at`/`created_by` when absent.
5. **Optional-field omission.** Honor `skip_serializing_if` (None / empty Vec /
   false-with-`Not::not`) — omit, don't emit nulls.
6. **Scalar styles** match serde_yaml: `|-` block scalars for multi-line bodies,
   single-quoted where it quotes.

**The conformance test is the round-trip corpus:** read every `objects/**/*.yaml`
→ re-emit → assert byte-identical. `store_reader.py` is the harness skeleton;
the byte-identical count is the score. A conformant writer scores 1635/1635.

## Strategic shape of the moat (the honest, demonstrable version)

The natural multi-vendor architecture falls out of the measurements:

- **READ directly** — trivial (one tag handler), language-agnostic. The graph is
  genuinely open for consumption.
- **WRITE through `aida`** (CLI or MCP) — the binary *is* the conformant writer,
  for free. A vendor that wants to write either funnels through `aida` or
  implements the 6-point emitter contract above and proves it against the
  corpus.

That asymmetry is a *strength*, not a gap: the substrate is open to read, while
writes converge on one conformant implementation — which is exactly where
AIDA's correctness guarantees (and value) live. "Open to read, conformant to
write" is a publishable, defensible contract, and it is now demonstrated rather
than asserted.

## Follow-ups (file as separate specs if pursued)

- Promote `store_reader.py`'s round-trip check into a CI conformance gate (guards
  AIDA's *own* emitter against accidental format drift — a regression here would
  break every vendor's byte-clean writes).
- A reference read-only consumer SDK (Python first) wrapping the `!Custom`
  handler + typed accessors, so vendors start at 100% read without rediscovering
  the tag.
- Empirically test the *write* path: implement the 6-point contract in the
  prototype and measure how close to 1635/1635 it gets (this spike proved the
  need + defined the contract; building the conformant writer is the next slice).
