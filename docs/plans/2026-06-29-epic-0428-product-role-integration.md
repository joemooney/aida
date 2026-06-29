# Plan: TASK-0431 — Product-role recommendations into advisor autopilot

Date: 2026-06-29
Specs: TASK-0431 (parent EPIC-0428) — depends on TASK-0429 (envelope), composes with TASK-0430 (audit)
Status: Draft — **design only, needs master-advisor sign-off before any code**
Complexity: ~80 prod LOC + ~80 test LOC when built, 0 commits now, risk low-medium (the hard bound already exists; this is mostly convention + handoff)

<!-- Do NOT implement. The substrate-as-bouncer for "product can't approve"
     ALREADY EXISTS (the TASK-647 advisor gate). This plan defines the handoff. -->

## Approach

The product role must be able to *feed* advisor autopilot — propose draft specs,
rationale, acceptance criteria, priority, risk flags, a recommended disposition —
**without gaining approval/queue authority**. The crucial finding from the
machinery map: **the hard bound already exists.** There is no first-class
"product" role today (roles are free-form strings; `advisor` is the *only*
privileged one — `permissions.rs:222`, `role_satisfies`). And `aida add --status
approved` / `aida queue add` / draft→approved promotion are *already* gated on
`has_advisor_authority()` (TASK-647 / ADR-3, `main.rs:78745`): a non-advisor
caller is downgraded to Draft with a triage notice. So a product seat **cannot
approve or queue by construction** — the bouncer is already on the door.

This plan therefore does **not** add new authority machinery. It defines:

1. **The product seat** as a named, non-privileged role (`product`) with a role
   file documenting what it *may* produce — drafts + structured recommendation
   metadata — and what it explicitly *cannot* do (the gate already enforces this;
   the role file makes it legible).

2. **The recommendation as evidence, not authority.** A product proposal carries
   its recommended disposition + rationale + risk as **tags and a structured
   comment** on the draft — never a status change. Autopilot reads these as
   **gate-3 evidence** (TASK-0429): they can *raise the grounding* of an
   autopilot decision (a product-supplied recorded rationale can make a Type-C
   fork into a recorded-B) but they can **never** satisfy gates 1, 2, or 4. A
   product `recommend:approve` on a keystone spec is still fenced; on a
   `propose`-authority action it still only proposes.

3. **The handoff pattern** from a product conversation to an autopilot
   disposition: product files draft + `from-product:` provenance +
   `recommend:<disposition>` + a rationale comment → autopilot's groom pass sees
   it in the candidate fence (if eligible) → treats the recommendation as one
   evidence input among several → dispositions under the envelope → the audit
   trail (TASK-0430) records that the decision *consumed a product recommendation*
   (the `--from-product` filter).

### Diagram — evidence flows up, authority does not flow down

```
  PRODUCT seat (role:product, NON-privileged)
        │  may write: draft spec, acceptance, priority,
        │  risk flags, recommend:<disposition>, rationale comment
        │  CANNOT write: status=approved, queue add   ◄── TASK-647 gate refuses
        ▼
  draft spec  ── tags: from-product:<who>, recommend:approve, risk:low
        │       ── comment: "product rationale: …"
        ▼
  ADVISOR AUTOPILOT groom pass (TASK-0429)
        │  reads product recommendation as EVIDENCE for gate 3 (grounding)
        │  gate 1 (fence)   ── product input cannot widen it
        │  gate 2 (authority)── product input cannot raise it
        │  gate 4 (risk)    ── product risk flag can only RAISE, never lower, the ceiling check
        ▼
  disposition under the envelope ──► audit (TASK-0430) records "consumed product rec"
        │
        └─ architecture / cross-cutting ─► still converges on operator / supervised advisor
```

## Decisions

- **Decision: `product` is a named non-privileged role, not new authority.**
  **Rationale**: the privilege model already has exactly one privileged role
  (`advisor`). Adding `product` as a *documented, conventional* role (a role
  file + statusline identity) gives the seat an identity and a routing target
  without touching `role_satisfies`. The TASK-647 gate already does the
  enforcement; we are naming a seat, not granting it power.

- **Decision: recommendations are tags + a structured comment, never a status
  change.** **Rationale**: this is the same evidence-not-authority pattern the
  advisor tier uses for findings. A `recommend:approve` tag + `from-product:<who>`
  provenance + a rationale comment are *durable, in-graph, and inert* — they
  change nothing until an advisor (or autopilot acting as the advisor seat)
  consumes them. Reuses the existing tag + comment substrate (no new store).

- **Decision: product input can RAISE grounding/risk-awareness but never LOWER a
  gate.** **Rationale**: the asymmetry is the safety property. A product rationale
  that cites a recorded preference can move a fork from Type-C ("escalate") to
  recorded-B ("resolvable") — *if and only if the cited preference is genuinely
  recorded*. A product `risk:low` flag can never *lower* the risk classification
  AIDA computes (`classify_risk`); a product `risk:high` flag can *raise* it
  (conservative). Product evidence is monotonic toward caution.

- **Decision: architecture / cross-cutting recommendations always converge on
  the operator or supervised advisor.** **Rationale**: acceptance criterion. A
  product `recommend:approve` on anything `is_keystone_class` is fenced at gate 1
  regardless — the recommendation rides along as context for the human, never as
  an autopilot green light. Same bound as everything else; product input does not
  get a special lane.

- **Decision: provenance is first-class so the audit can show it.**
  **Rationale**: TASK-0430's `--from-product` filter needs a durable marker. The
  `from-product:<who>` tag + the consumed-evidence field on `AutopilotDecision`
  make "which autopilot decisions acted on product input" queryable — the review
  surface the operator needs to spot a product seat steering the queue.

## Files (in build-order)

### `aida-cli/src/main.rs` — add `product` to the starter role set

- Add a `("product", "<purpose>")` entry to the `STARTER_ROLES` const (`main.rs`, alongside `implementer`/`advisor`/`reviewer`/`integrator`) — the starter roles are a code const scaffolded into global `~/.aida/roles/` by `scaffold_starter_roles` (TASK-608/TASK-638), **not** template TOML files. The purpose string documents the product seat's *outputs* (draft, acceptance, priority, risk flag, `recommend:<disposition>`, rationale) and its *non-authority* (cannot approve/queue — the gate enforces it). `product` is a plain non-privileged role name (`role_satisfies` privileges only `advisor`).

### `aida-cli/src/findings.rs` — provenance constants (or a small `product.rs` (new))

- `FROM_PRODUCT_PREFIX = "from-product:"` and `RECOMMEND_PREFIX = "recommend:"` — the durable evidence tags. Parsers: `product_recommendation(tags) -> Option<Disposition>`, `product_provenance(tags) -> Option<String>`.

### `aida-cli/src/autopilot.rs` (new) — consume as evidence (created by TASK-0429)

- Extend the `Decision`/grounding path: when a candidate carries `from-product:` + `recommend:` + a rationale comment, the cold-boot advisor weighs it as evidence. A pure helper `apply_product_evidence(grounding, risk, product_input) -> (grounding, risk)` implements the *monotonic-toward-caution* rule (raise-only). **Unit-testable** — the asymmetry is the test surface.

### `aida-cli/src/autopilot_log.rs` (new) — record consumption (created by TASK-0430)

- `AutopilotDecision.evidence` includes a `product:<who>` marker when consumed; powers `aida autopilot list --from-product`.

### `aida-core/templates/skills/aida-assess.md` — the handoff discipline

- A "Product recommendations" subsection: how the groom pass reads `from-product:`/`recommend:` tags + rationale, treats them as evidence (never authority), and the raise-only rule. Explicit: a product `recommend:approve` does NOT relax propose-authority or the fence.

### `docs/aida/discipline/` — new `product-role.md` (and README pointer)

- Document the product seat, the evidence-not-authority contract, and the conversation→draft→autopilot handoff pattern (acceptance: "document the handoff pattern"). Add the row to the discipline README table.

### `aida-core/templates/skills/aida-req.md` / `aida-capture.md` — product entry points

- Note that a product-seat conversation captures via the existing `/aida-req`/`/aida-capture` paths, adding the `from-product:`/`recommend:` tags. No new capture command — reuse.

## Critical Files

- `aida-cli/src/main.rs` (the `STARTER_ROLES` const)
- `aida-cli/src/findings.rs` (provenance constants)
- `aida-cli/src/autopilot.rs` (new) — created by TASK-0429
- `aida-cli/src/autopilot_log.rs` (new) — created by TASK-0430
- `aida-core/templates/skills/aida-assess.md`
- `docs/aida/discipline/product-role.md` (new)

## Reusable helpers (do not reimplement)

- `has_advisor_authority` + `status_requires_advisor_authority` + the `aida add --status approved` downgrade (`aida-cli/src/main.rs:78745, 78622, 24502`) — the **existing** bouncer that makes "product cannot approve" true. This plan relies on it; it does not re-implement or weaken it.
- `permissions::role_satisfies` / `gated_effective_role` (`aida-cli/src/permissions.rs:222, 242`) — the privilege model; `product` is added as a *non*-privileged role, so nothing here changes.
- `findings::FROM_ADVISOR_PREFIX` pattern (`aida-cli/src/findings.rs:28`) — the tag-provenance convention to copy for `from-product:`.
- `backlog::classify_risk` / `classify_risk_with_reason` (`aida-cli/src/backlog.rs:180, 189`) — the AIDA-computed risk; product flags compose with it raise-only, never replace it.
- The tag + `aida comment add` substrate — drafts + recommendation metadata ride existing fields; no new store.
- `scaffold_starter_roles` + the `STARTER_ROLES` const (`aida-cli/src/main.rs`, TASK-608/TASK-638) — the `product` entry joins this code const; the scaffold writes it into `~/.aida/roles/`. No new template file.

## Risks + gotchas

1. **Risk: a product seat games the system — files draft + `recommend:approve` +
   a fabricated "recorded preference" rationale to push autopilot to auto-approve.**
   **Mitigation**: the grounding gate (TASK-0429 gate 3) requires the cited
   preference to be *genuinely recorded* (a real memory name / doc path / prior
   decision the advisor can verify), not asserted in the rationale text. Product
   *claims* of grounding are not self-certifying — the cold-boot advisor must
   find the substrate. Unrecorded → escalate. Plus the conservative default
   (`approve = propose`) means even a well-grounded product approve is *held for
   review* until a project widens.

2. **Risk: provenance laundering — product input gets re-tagged as advisor
   input, hiding that the queue is product-steered.** **Mitigation**:
   `from-product:` is durable on the draft and the `AutopilotDecision` records
   the consumed-evidence marker; `aida autopilot list --from-product` surfaces
   exactly this. The audit makes steering visible.

3. **Risk: role-name collision / `aida role enter product` already "works"**
   (any string is a valid role). **Mitigation**: harmless — entering `product`
   today grants no authority (`role_satisfies` only privileges `advisor`). The
   role file just gives it documented shape. No migration needed.

4. **Risk: scope creep — building a full product-management surface.**
   **Mitigation**: out of scope by `feedback_pushback_on_overengineering`. The
   product seat reuses `/aida-req` + tags + comments. The smallest valuable slice
   is the *evidence contract*, not a new capture UI.

## Tests (named)

- `product_cannot_approve_downgrades_to_draft` — the existing gate, asserted from a product role (regression guard).
- `apply_product_evidence_raises_grounding_only_when_substrate_recorded` — the core asymmetry.
- `apply_product_evidence_never_lowers_risk` — monotonic-toward-caution.
- `apply_product_risk_high_raises_ceiling_check` — product can tighten.
- `product_recommend_approve_on_keystone_still_fenced` — gate 1 dominates product input.
- `product_recommend_does_not_relax_propose_authority` — gate 2 dominates.
- `autopilot_decision_records_product_provenance` — audit consumption marker.
- `product_provenance_survives_to_from_product_filter` — TASK-0430 filter.

## Verification

```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
TMP=$(mktemp -d); cd "$TMP" && git init -q && "$AIDA_BIN" init >/dev/null

# Product seat files a draft with a recommendation — CANNOT approve.
AIDA_SESSION_ROLE=product "$AIDA_BIN" add --title "add a --quiet flag" --type task \
  --status approved --tags "from-product:pat,recommend:approve,risk:low"
"$AIDA_BIN" show TASK-... | grep -i 'status: *draft'      # gate downgraded it — product has no approve authority
"$AIDA_BIN" show TASK-... | grep -i 'from-product:'        # provenance durable

# Autopilot groom reads the recommendation as EVIDENCE, still bounded by the envelope.
printf '\n[autopilot]\napprove = "propose"\n' >> .aida/config.toml   # default: held
"$AIDA_BIN" groom --autopilot --dry-run | grep -i 'recommend'        # recommendation surfaced as evidence
"$AIDA_BIN" show TASK-... | grep -i 'status: *draft'                 # approve still held (propose), product can't widen

# Keystone + product recommend:approve → still fenced.
AIDA_SESSION_ROLE=product "$AIDA_BIN" add --title "rework auth" --type task \
  --status draft --tags "from-product:pat,recommend:approve,architecture"
"$AIDA_BIN" groom --autopilot --dry-run | grep -i 'fenced.*architecture'
```

## Followups

- TASK-0432 — does a product seat change anything under `--no-human`/solo? (No new authority, but the audit `mode` should note product-sourced decisions taken during a headless drain.)
- Followup TASK (file at sign-off): SPIKE — is a dedicated product *advisor* subsystem (SPIKE-10 subsystem-scoped advisors) the right long-term home, or does the evidence-tag contract suffice?

## Related

- TASK-0429 (envelope), TASK-0430 (audit), TASK-647 / ADR-3 (advisor gate), `permissions.rs`, SPIKE-10 (subsystem-scoped advisors), `feedback_advisor_grooms_dont_shift_to_operator`.

## Recommendation + smallest first slice

**Recommendation**: do not build authority — the bouncer (TASK-647 advisor gate)
already guarantees a product seat cannot approve or queue. Build the **evidence
contract**: a named non-privileged `product` role, durable `from-product:` /
`recommend:` tags + rationale comments, and the *raise-only* rule by which
autopilot's grounding/risk gates consume that evidence without ever being
relaxed by it. The safety property is the asymmetry — product input is monotonic
toward caution and can never satisfy gates 1, 2, or 4.

**Smallest first slice**: ship the `apply_product_evidence` pure function (the
raise-only grounding/risk rule) + the `from-product:`/`recommend:` tag parsers +
their unit tests, and the `product.toml` role file + `docs/aida/discipline/
product-role.md`. This lands the *contract and its guardrails* (provably
asymmetric, fully unit-tested) and the documented seat, with **no change to the
authority model** — the riskiest-sounding task is actually the lowest-risk
because the enforcement already exists. Wiring the consumption into the live
groom pass is the second slice, after TASK-0429's `evaluate` ships.
