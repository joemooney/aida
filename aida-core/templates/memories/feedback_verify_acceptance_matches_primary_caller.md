---
name: verify-acceptance-matches-primary-caller
description: Before filing, name the feature's primary caller(s) and verify each acceptance criterion holds in THEIR environment (TTY/headless, which role, MCP vs CLI) — not just internal consistency.
propagation: scaffolding-pack
metadata:
  type: feedback
---

Before treating a spec's acceptance criteria as done, **name the primary caller(s)** — the feature's top 1-3 invocation paths — and verify each criterion holds *in their environment*. A criterion is **environment-coupled** when its truth depends on the runtime context the feature runs in.

**Why:** A criterion can read as logically consistent in isolation yet contradict the feature's primary caller environment, making the feature degrade (or never appear) in its own main use case. The implementer catches this at the design-checkpoint pause — but that is the last line of defense, not the first. The filing-time check catches it before the wrong criterion ships into the spec.

**The discriminator** — a criterion is environment-coupled when its truth depends on:

- TTY vs piped stdout
- headless (`claude -p`) vs interactive Claude Code
- a user-typed CLI vs a skill invoked through the Bash tool (**always non-TTY**) vs a git/Claude Code hook vs the MCP server
- first-time vs returning user (memory + state)
- solo node vs multi-node sync
- same worktree vs cross-worktree

**How to apply:** Name the primary caller (or top 2-3 callers). For *each* acceptance criterion, ask: *does this criterion hold in those caller environments?* If a criterion would degrade the feature in its primary caller's environment, the criterion is wrong — fix it before filing. Don't stop at "the spec is internally coherent"; walk each criterion against the named caller.

**Worked failure (TASK-265):** a criterion read *"non-TTY mode degrades to a single-line summary"* — but the primary caller (`/aida-pickup` via the Bash tool) is **always** non-TTY, so the card would never render in its own use case. Naming the caller first turns an abstract mode-toggle into a check the feature must pass.

**Pattern observed across four specs in 36 hours** (TASK-260, TASK-267, BUG-224, TASK-265): advisor files a criterion that is consistent in isolation but contradicts the primary caller → implementer surfaces the contradiction at the design checkpoint → advisor corrects the spec → work proceeds. The system worked, but the filing-time discipline that would have prevented the contradiction wasn't yet codified.

Composes with [[refinements-must-be-acceptance-criteria]] (the sibling rule: criterion fixes belong in acceptance, not comments — this rule adds the upstream "don't file the flawed criterion in the first place") and [[pause-for-design-input]] (the implementer-side last line of defense this rule reduces dependency on). Origin: TASK-311.
