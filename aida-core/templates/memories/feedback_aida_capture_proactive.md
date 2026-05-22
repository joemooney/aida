---
name: Run /aida-capture proactively
description: Behavioral commitment to capture requirements as work happens, not in retrospective backfill — eats AIDA's own dog food.
type: feedback
propagation: scaffolding-pack
originSessionId: cebef59f-90ed-4e05-b21f-3f63c310f98b
---
In any project that uses AIDA (AIDA itself or any aida-init'd project), proactively capture requirements as work happens — don't let the requirements DB drift behind the git log.

**Why:** On 2026-05-04 the user pointed out that ~28 commits had landed in the AIDA repo since 2026-05-02 with only 2 corresponding requirements created (EPIC-1-001 and FR-1-002). All 28 commits were trace-tagged `EPIC-1-001` even though the work spanned 7+ distinct themes (dev workflow, roles, queue routing, statusline, build banner, etc.). Trace signal had become meaningless. AIDA's whole pitch is "durable agent-readable specs" and AIDA itself wasn't using it. The user did a 15-minute backfill creating EPIC-1-004 through EPIC-1-010 to close the hole.

**How to apply:**

1. **Spec-first for new themes.** When a conversation introduces something that's clearly its own scope (new command surface, new field in a core model, new skill, new architectural pattern), pause and `aida add --type epic --status in-progress` BEFORE the implementation commits. Cost: ~2 minutes per real EPIC. Saves: hours of backfill.

2. **Don't reuse one EPIC as a catchall.** If the work being done is no longer "what the EPIC was originally about," that's a signal to create a new EPIC, not stretch the existing one. Trace signal degrades fast when one EPIC absorbs unrelated work.

3. **Run `/aida-capture` at natural session-pause points** — when the user is about to step away, when context is nearing compaction, when ending the working session, or when explicitly asked. It's a 5-minute pass that catches missed reqs.

4. **Heuristic threshold:** if more than ~5 commits have landed in a session without a corresponding requirement entry, treat that as a yellow flag and offer to capture before continuing.

5. **Trace comments should match reality.** `// trace:EPIC-1-001 | ai:claude` on code that has nothing to do with EPIC-1-001 is misinformation that compounds. If unsure which EPIC a piece of work belongs to, that's also a signal it needs its own.

This applies symmetrically: it's true for AIDA's own development AND for any project where the user has run `aida init`. The skill is the same; the repo is different.
