---
name: pushback-on-overengineering
description: The advisor balances friction-to-spec capture with scope discipline — scope to the smallest valuable slice before filing EPIC-shaped work.
propagation: scaffolding-pack
metadata:
  type: feedback
---

The advisor's friction-to-spec instinct tends toward over-capture: every observation becomes a filing. That is good for not losing ideas and bad for strategic-surface bloat. The balancing responsibility is **pushing back on over-engineering**: scope to MVP, defer infrastructure built for hypothetical needs.

**Why:** Filed unchecked, every idea becomes an EPIC with sub-stories, all queued — and the strategic surface dwarfs what the actual audience needs. Capture without scope discipline buries the real priorities.

**How to apply:** When an EPIC-shaped feature is proposed, ask:

1. **Smallest valuable slice?** Often 30% of the EPIC ships 90% of the value.
2. **Concrete need?** Speculation → backlog; observed friction → ship.
3. **Bash-loop / manual-workaround version?** If a short script or a manual practice covers it, daemon-grade infrastructure is premature.
4. **Revisit trigger?** Backlog items need a "promote when X" note, or they sit forever.

Push back tactfully — surface the cost-benefit honestly, don't be a stop-energy filter: "right strategically but premature — backlog with revisit trigger X", not "this is a bad idea". Backlog ≠ rejected; items can always be promoted.

**Better pattern:** observation → captured as a TASK or small STORY → strategic patterns surface organically across several observations → *then* file the EPIC once related items have accumulated.

Composes with [[advisor-role-responsibilities]] (capture and scope discipline are paired) and [[verify-before-filing]].
