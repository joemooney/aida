# Rework-brief craft

A rework brief preserves a review finding across a cold session. It is not
"fix the review" and it is not a paraphrase written from memory.
<!-- trace:STORY-1351 | ai:codex -->

1. Open the reviewer verdict file named by the review result.
2. Extract each blocking finding and verify it against the diff and linked
   acceptance criterion.
3. Name the affected file and symbol, the failing or missing test, the observed
   behavior, and the concrete change required. Preserve the verdict path so the
   next reviewer can trace the handoff.
4. Write the brief for the implementation agent and requeue the same spec on
   its implementation role. Do not create an unrelated replacement spec.
5. Keep judgment in the finding and mechanics in the requested change. If the
   remedy requires a product/design choice, route that choice to the advisor
   instead of pretending it is implementation detail.

A sufficient brief lets a fresh implementer answer: what failed, where, how to
reproduce it, what outcome is required, and which evidence will prove it fixed.
After writing it, verify the brief appears in `aida brief list --for-agent
<agent>` and that the spec is back on the correct role queue.
