---
name: Capture doc seeds during work, not after
description: Every meaningful design discussion or UX insight should land as a doc-seed comment on the relevant req, structured for EPIC-24's eventual aida doc generate
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
When working in this project, capture documentation seeds as comments on relevant reqs **during** the conversation that produced them — not later, not summarized — proactively.

**Why:** AIDA has rich design rationale buried in chat transcripts that evaporates when sessions end. The user has explicitly said: "this is the type of detail we need to capture in documentation and in-depth tutorials. If we were writing an Aida book this would be ideal." EPIC-24 captures this need; STORY-107 (positioning docs) is one child. But until EPIC-24's tooling ships, capture is manual via structured comments.

**How to apply — capture a doc seed when:**

- Designing a feature in detail (use case, when-useful, alternatives, gotchas)
- Comparing AIDA to other tools (/ultrareview, Linear, Karpathy-md, etc.)
- Establishing a workflow recipe (implementer→reviewer cycle, etc.)
- Identifying an anti-pattern or trap (cwd-resolution-from-worktree, BUG-67 gitignored-cruft, etc.)
- Choosing between design approaches (Done status vs Scheduled, single-item vs cluster pickup, etc.)
- Discovering a tool's cost/license/quota model that affects positioning

**Format convention:**

```markdown
## Doc seed: <topic> (YYYY-MM-DD)

[Structured content — tables, scenarios, code examples, alternatives, anti-patterns]

---

*This comment is a documentation seed per EPIC-24 (living documentation). Future `aida doc generate` could extract sections like this into `docs/tutorials/` or `docs/positioning/` chapters.*
```

**Add as a comment via:**
```bash
aida comment add <SPEC-ID> "..."
```

**Pick the spec_id to attach the seed to**:
- If discussing a specific feature/req → its spec_id
- If comparing tools → STORY-107 (positioning) or a related EPIC
- If workflow pattern → relevant EPIC (e.g., EPIC-23 for orchestration patterns)
- If lesson learned → BUG/TASK that surfaced it

**Behavioral rule:** if I find myself writing a detailed explanation in chat that the user might want to reference later, ALSO capture it as a doc seed on the relevant req. The chat text isn't searchable from `aida show`; the req comment is.

**Verify when:** before stating a tool's cost/quota/behavior, fact-check rather than capitulate. User pushed back on /ultrareview cost claim 2026-05-12; I softened ("might just consume Max quota") without evidence; turned out my original "billed" framing was right but understated (3 free uses, then **$5-$20 USD per use**, even on Max). When a UI/prompt shows a cost estimate, that IS canonical — don't paraphrase, don't soften, capture verbatim. Three corrections in one session on the same fact = clear signal that cost data deserves first-read fidelity, not second-guessing.
