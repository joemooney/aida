# Substrate as bouncer, not passive rules

Rules in prompt text are useful, but AIDA's reliability improvements come
from programmatic gates that intercept unsafe lifecycle states.

## Headless text-question gate (BUG-354 / BUG-374)

In `--no-human=both`, a headless implementer must not ask a plain-text
confirmation question and exit with no PR. A final message like "A or B?
Please confirm" has no human recipient inside the subprocess, so it is a
design-fork punt.

The orchestrator now inspects the terminal headless JSONL `result` event
when phase 1 exits cleanly with no PR. If the final answer contains
decision-fork question wording such as "which path", "should I", or
"confirm and I'll proceed?", it files a design-fork punt instead of
returning a generic phase-1 `NoPr` failure. The existing STORY-306 advisor
tier then resolves or escalates the fork.

The `/aida-pickup` instructions still tell agents to punt explicitly; this
gate is the bouncer for the recurring ceiling-pattern case where prompt
discipline fails.
