---
name: check-in-flight-before-rejecting
description: Before rejecting a spec or pivoting its architecture, check whether an implementer is actively working on it.
propagation: scaffolding-pack
metadata:
  type: feedback
---

Before rejecting a spec or pivoting its architecture mid-stream, check whether an active implementer session exists for it. Otherwise an implementer shipping in good faith on the original spec ends up with a committed branch rendered obsolete behind their back — the effort is wasted and the user has to untangle what is salvageable.

**Why:** A rejection that races an in-flight implementer destroys coordination. The code is rarely lost, but the implementer's time is, and the user inherits the cleanup.

**How to apply:** Before `aida edit <SPEC> --status rejected` or filing a "supersedes" spec:

1. Run `aida session leases --all` — is there an active lease for this spec's scope?
2. Run `aida show <SPEC>` — is it In Progress?
3. If either signals active work, pause the rejection and ask the user how to coordinate: finish the work first, interrupt with a scope change, or reject after the current commit lands.
4. If no active work, reject freely; document the supersedes / replaces relationship.

Composes with [[verify-before-filing]] (verify state before acting on it) and [[advisor-role-responsibilities]] (queue gardening includes coordinating with in-flight work).
