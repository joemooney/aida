# Plan: Track 3 Codex local workflow productization

Date: 2026-05-22
Specs: SPEC-397 program, Track 3 planning task (MCP-assigned ID provisional)
Status: Draft
Complexity: low-to-medium; docs/scaffolding first

## Approach

Track 3 makes Codex useful with AIDA even while MCP support is incomplete. The
first supported path should be local and boring: `AGENTS.md` tells Codex how to
use the AIDA CLI, docs explain the workflow, and review recipes check the
results. MCP remains a future accelerator, not a prerequisite.

```
   aida init --agent codex
          |
          v
      AGENTS.md
          |
          v
   Codex uses local CLI:
      aida show / list / search / doctor / trace
          |
          v
   local verification + review recipes
```

## Decisions

- **No MCP dependency for v1**: Codex onboarding must work with shell commands.
  **Rationale**: the MCP surface is currently inconsistent across local/MCP
  stores and should not block useful Codex workflows.
- **Docs before commands**: do not add `aida codex` commands until repeated
  setup friction is observed. **Rationale**: avoid building wrappers around an
  unstable external CLI.
- **AGENTS.md is the main runtime contract**: generated instructions should
  teach Codex the local AIDA discipline. **Rationale**: Codex reliably reads
  AGENTS.md; it does not need a Claude-style skill system.
- **MCP docs marked optional**: include MCP registration only as an optional
  section with caveats. **Rationale**: honest docs beat aspirational docs.

## Work Packages

### T3.1: Codex quickstart page

Add a concise Codex quickstart focused on local CLI usage.

Required content:

- What AIDA gives Codex.
- `aida init --agent codex` or equivalent current scaffold behavior.
- How Codex should inspect a spec: `aida show <SPEC>`.
- How Codex should search/list context.
- How Codex should verify trace comments locally.
- Example implementation prompt.
- Example review prompt.
- Optional MCP section clearly labeled as incomplete/advanced.

Acceptance criteria:

- A new user can follow the page without MCP.
- Page does not claim Codex can use `.codex/skills` automatically unless that
  behavior is verified.
- Commands are local and copy-pasteable.

### T3.2: Generated AGENTS.md local-first update

Update generated AGENTS.md guidance so Codex defaults to local AIDA CLI.

Required guidance:

- Start implementation tasks with `aida show <SPEC>` when a SPEC is provided.
- Use `aida list`, `aida search`, and `aida doctor validate-trace-comments`.
- Preserve trace comments.
- Do not invent SPEC IDs.
- If MCP is available, it can substitute for CLI reads, but CLI remains the
  fallback.

Acceptance criteria:

- Existing AGENTS.md managed block updates cleanly.
- Claude-specific slash-command references do not leak into Codex guidance.
- Scaffolding tests are updated if snapshots/assertions exist.

### T3.3: Codex review recipes

Document short review prompts for:

- Traceability review.
- Requirements drift review.
- Implementation-plan adherence.
- Unscoped-change review.

Acceptance criteria:

- Recipes work as plain prompts.
- Each recipe includes local commands Codex can run.
- Recipes do not require network or MCP.

### T3.4: Stale Codex plan reconciliation

Update or supersede `docs/plans/2026-03-17-codex-cli-support.md` so it no
longer presents implemented items as missing.

Acceptance criteria:

- Current state is clearly marked.
- Future work is separated from shipped work.
- It points to this local-first plan.

## Files

- `docs/codex-quickstart.md` - new user-facing guide.
- `docs/user-guide.md` - link to Codex quickstart.
- `docs/plans/2026-03-17-codex-cli-support.md` - stale-plan note.
- `aida-core/src/scaffolding/codex_md.rs` - AGENTS.md generator.
- `aida-core/src/scaffolding/aida_md.rs` - shared convention block if needed.
- `aida-core/src/scaffolding/mod.rs` - tests/scaffold artifact behavior.

## Candidate Codex Prompt

```text
Improve Codex local workflow documentation. Do not change runtime behavior.
Create or update docs so Codex can use AIDA through local CLI commands without
MCP. Read the AGENTS.md generator and existing Codex plan first. Keep MCP as an
optional advanced section with caveats. Run relevant docs/scaffolding tests if
you edit scaffolding.
```

## Risks + Gotchas

1. **Risk**: documenting Codex CLI behavior that changes externally.
   **Mitigation**: document AIDA-owned commands and keep Codex-specific
   commands minimal.
2. **Risk**: AGENTS.md becomes too long. **Mitigation**: keep local workflow
   as a short checklist.
3. **Risk**: users interpret MCP caveat as "AIDA is broken." **Mitigation**:
   present local CLI workflow as the supported baseline.
4. **Risk**: `.codex/skills` suggests unsupported behavior. **Mitigation**:
   document them as reference workflow files unless verified otherwise.

## Verification

```bash
cargo test -p aida-core scaffolding
./target/debug/aida scaffold diff --list
rg -n "Codex|AGENTS.md|aida show|validate-trace" docs aida-core/src/scaffolding
```

## Followups

- Add `aida codex doctor` only after repeated setup failures.
- Revisit MCP-first docs after MCP store consistency is fixed.
- Add recorded examples from Track 1 paired benchmarks.

## Related

- `docs/plans/2026-03-17-codex-cli-support.md`
- `docs/plans/2026-05-22-track-1-agent-lift-evaluation.md`
