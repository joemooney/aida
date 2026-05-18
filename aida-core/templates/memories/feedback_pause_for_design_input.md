---
name: pause-for-design-input
description: On UX/design-laden pickups, present concrete design forks for input before implementing — don't sleepwalk them.
propagation: scaffolding-pack
metadata:
  type: feedback
---

When picking up work with real UX / design latitude (empty-state UX, copy, layout, interaction model), pause and present the concrete design decisions as explicit options *before* writing code.

**Why:** Default-implementing silently bakes in arbitrary choices the user would often have made differently. Users tend to have strong opinions on UX surface, and surfacing the fork is cheap — far cheaper than reworking shipped code.

**How to apply:** Read enough of the code first to make the options concrete *and* to surface code-discovered forks the spec did not anticipate. Use a question tool with previews (copy variants, ASCII mockups). Keep it tight: one batched question, recommend a default. Then ship the *minimal* fix and let the bigger vision be a follow-up.

Under a headless / no-human drain the implementer cannot pause for input. The equivalent move is to **punt**: flip the spec to a needs-attention state and post the reason as a comment on the spec, so the human can triage on return. Same underlying judgment — design-fork detection — different surface.

Composes with [[failed-flag-attempts-are-signals]], [[verify-before-filing]], and [[refinements-must-be-acceptance-criteria]] (the input you gather must be captured in a binding form).
