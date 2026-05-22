# Plan: Track 4 CLI modularization for safer Codex edits

Date: 2026-05-22
Specs: SPEC-397 program, Track 4 planning task (MCP-assigned ID provisional)
Status: Draft
Complexity: medium; behavior-preserving extraction

## Approach

Track 4 reduces the risk of future Codex-authored changes by shrinking
`aida-cli/src/main.rs`. The goal is not architectural redesign. The goal is a
sequence of mechanical extractions that compile independently and create
smaller modules where later feature work can happen safely.

The first extractions should target stable, low-coupling surfaces: trace
commands, doctor validation helpers, MCP registration command handling, and
pure queue lifecycle helpers. Larger orchestrator extraction waits until these
seams are proven.

```
   main.rs (~57k LOC)
        |
        +--> trace.rs
        +--> doctor.rs
        +--> mcp_register.rs
        +--> queue_lifecycle.rs
        |
        └--> orchestrator extraction later
```

## Decisions

- **Mechanical moves before refactors**: move functions mostly unchanged.
  **Rationale**: relocation diffs are reviewable; behavior diffs are not.
- **One module per PR**: each extraction compiles and lands independently.
  **Rationale**: lowers conflict risk with active AIDA work.
- **No public CLI changes**: dispatch and help stay unchanged. **Rationale**:
  this track is preparation, not product behavior.
- **Extract tests with code when practical**: keep focused tests near helpers.
  **Rationale**: future Codex tasks need local context.

## Extraction Order

### T4.1: `trace.rs`

Move trace command handling and helpers from `main.rs`.

Likely symbols:

- `handle_trace_command`
- `trace_add`
- `trace_list`
- `trace_remove`
- `trace_scan`
- `trace_sweep`
- trace printing/parsing helpers used only by trace commands

Acceptance criteria:

- `cargo check -p aida-cli` passes.
- `aida trace --help` remains unchanged.
- No trace behavior changes.

### T4.2: `doctor.rs`

Move doctor command handling and trace-validation helpers after or alongside
trace extraction.

Likely symbols:

- `handle_doctor_command`
- `doctor_validate_trace_comments`
- `walk_source_for_traces`
- `strip_dangling_traces`
- collision/relationship/fsck helpers if dependencies allow

Acceptance criteria:

- `cargo check -p aida-cli` passes.
- `aida doctor --help` remains unchanged.
- Existing doctor tests move or keep passing.

### T4.3: `mcp_register.rs` or fold into `mcp.rs`

Move local MCP registration command code, not the server implementation.

Likely symbols:

- `handle_mcp_command`
- `register_mcp_agent`
- `resolve_aida_exe`

Acceptance criteria:

- `aida mcp register-agent --print` output remains equivalent.
- Tests for `resolve_aida_exe` remain.
- No MCP server behavior changes.

### T4.4: `queue_lifecycle.rs`

Extract pure helpers used by queue work/auto-complete decisions.

Candidate helper classes:

- status transition predicates
- reconcile verdict helpers
- branch swap pure logic
- auto-complete phase classification

Acceptance criteria:

- Only pure or near-pure helpers move in the first pass.
- Orchestrator subprocess spawning remains untouched.
- Focused unit tests stay with moved helpers.

## Codex Assignment Shape

Use prompts like:

```text
Perform a mechanical extraction only. Move <symbols> from
`aida-cli/src/main.rs` to `aida-cli/src/<module>.rs`. Preserve behavior and CLI
shape. Prefer `pub(crate)` wrappers over redesign. Do not combine with feature
changes. Run `cargo check -p aida-cli`.
```

Stop conditions:

- More than three unrelated import cycles appear.
- Required changes spill into unrelated modules.
- Tests fail for behavior reasons rather than visibility/import reasons.

## Files

- `aida-cli/src/main.rs`
- `aida-cli/src/trace.rs`
- `aida-cli/src/doctor.rs`
- `aida-cli/src/mcp_register.rs` or `aida-cli/src/mcp.rs`
- `aida-cli/src/queue_lifecycle.rs`
- `aida-cli/src/cli.rs` only if visibility/imports require it

## Risks + Gotchas

1. **Risk**: helper functions share too much implicit state from `main.rs`.
   **Mitigation**: extract stable surfaces first; stop before redesign.
2. **Risk**: relocation diffs are huge and hard to review. **Mitigation**:
   one module per PR; no formatting-only churn.
3. **Risk**: Codex "improves" behavior during extraction. **Mitigation**:
   prompt explicitly forbids behavior changes; review diff for logic edits.
4. **Risk**: active branches conflict in `main.rs`. **Mitigation**:
   land mechanical moves before feature work on the same surface.

## Verification

```bash
cargo check -p aida-cli
./target/debug/aida trace --help
./target/debug/aida doctor --help
./target/debug/aida mcp register-agent --print
```

## Followups

- Add module-level ownership comments after extraction.
- Extract orchestrator only after smaller modules prove the pattern.
- Consider `cargo machete`/dead-code cleanup after all extraction lands.

## Related

- `docs/plans/2026-05-22-codex-driven-aida-improvement.md`
