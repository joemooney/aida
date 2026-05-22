# Plan: Track 2 traceability governance

Date: 2026-05-22
Specs: SPEC-397 program, Track 2 planning task (MCP-assigned ID provisional)
Status: Draft
Complexity: medium; mostly CLI behavior and tests

## Approach

Track 2 turns AIDA traceability from a convention into a checkable local
policy. Because MCP support is incomplete, the first usable path must be pure
CLI: commands that work in a checkout, produce human and machine-readable
output, and can run from CI or a git hook without an agent in the loop.

The initial target is not perfect coverage enforcement. The safe first target
is live-reference enforcement: every `trace:<SPEC-ID>` found in checked files
must resolve to a known requirement or agreed ID. After that is stable, add
diff scoping and optional coverage policy.

```
   existing trace comments
          |
          v
   resolve against AIDA store
          |
          +--> human report
          |
          +--> JSON report + exit code
          |
          +--> diff-scoped CI policy
```

## Decisions

- **CLI first, MCP later**: trace policy must run via `aida doctor` or
  `aida trace`, not MCP. **Rationale**: CI and hooks need local deterministic
  behavior.
- **Resolve-first policy**: fail on dangling trace IDs before enforcing
  coverage. **Rationale**: dangling links are objective; coverage expectations
  need tuning by file type.
- **Report-only before gate**: add `--format json` / report mode before adding
  hard defaults. **Rationale**: avoid breaking users with noisy false positives.
- **Diff scope is separate from full scan**: keep full-project validation and
  PR validation as distinct modes. **Rationale**: local maintenance and CI have
  different noise budgets.

## Work Packages

### T2.1: CI-friendly dangling trace report

Extend `aida doctor validate-trace-comments` or add an adjacent command so it
can emit JSON and exit non-zero when unresolved trace IDs are found.

Acceptance criteria:

- Human output remains unchanged by default.
- JSON output includes `spec_id`, file path, line, and count.
- Exit code is zero when all trace IDs resolve.
- Exit code is non-zero when any trace ID is unresolved.
- Unit tests cover known, unknown, and agreed-ID resolution.

Candidate Codex prompt:

```text
Implement CI-friendly JSON output for dangling trace validation. Do not change
default human output. Add focused tests for all-resolved and unresolved
trace IDs. Use the existing validation logic; do not redesign trace storage.
Run `cargo check -p aida-cli`.
```

### T2.2: Diff-scoped validation

Add a way to validate only changed files relative to a base ref.

Acceptance criteria:

- Command accepts a base ref such as `origin/main` or `HEAD~1`.
- Only files changed relative to that ref are scanned.
- Deleted files are ignored.
- Rename behavior is documented.
- Full-project mode remains available.

Candidate Codex prompt:

```text
Add diff-scoped trace validation using existing git helpers where possible.
Keep full-project validation unchanged. Validate only changed files, ignore
deleted files, and document rename behavior. Run focused tests and
`cargo check -p aida-cli`.
```

### T2.3: Policy levels

Introduce explicit policy levels:

- `resolve`: all existing trace IDs must resolve.
- `changed-source-has-trace`: changed source files must contain at least one
  trace ID.
- `strict`: future mode for stronger function/symbol-level expectations.

Acceptance criteria:

- `resolve` is the default CI-safe policy.
- Higher policies are opt-in.
- Output explains which policy failed.
- AGENTS.md can explain the policy in one short section.

### T2.4: CI and hook snippets

Document how to run trace validation locally and in CI.

Acceptance criteria:

- Docs include a copy-paste local command.
- Docs include a GitHub Actions snippet.
- Docs call out report-only vs fail modes.
- No dependency on MCP or Codex.

## Files

- `aida-cli/src/main.rs` - current location of doctor/trace validation.
- `aida-cli/src/cli.rs` - flags for output format, base ref, policy.
- `aida-core/src/scaffolding/hooks.rs` - optional later hook updates.
- `docs/user-guide.md` or new `docs/traceability-governance.md`.
- `aida-core/src/scaffolding/aida_md.rs` / `codex_md.rs` - later AGENTS.md
  policy guidance.

## Risks + Gotchas

1. **Risk**: false positives on generated/vendor files. **Mitigation**:
   support ignore patterns before strict coverage enforcement.
2. **Risk**: full scan is slow on large repos. **Mitigation**: diff mode first
   for CI; optimize only if measured.
3. **Risk**: trace IDs in docs/help text are mistaken for implementation
   traces. **Mitigation**: policy should distinguish source paths from docs.
4. **Risk**: line numbers drift between report and review. **Mitigation**:
   reports are diagnostic, not persistent trace records.

## Verification

```bash
cargo check -p aida-cli
cargo test -p aida-cli trace
./target/debug/aida doctor validate-trace-comments
./target/debug/aida doctor validate-trace-comments --format json
```

## Followups

- Add SARIF output only if GitHub code scanning is a real target.
- Add language-aware symbol-level trace coverage.
- Add repo-level ignore config for trace policy.
- Add MCP exposure after local CLI behavior stabilizes.

## Related

- `docs/plans/2026-05-22-codex-driven-aida-improvement.md`
- `docs/plans/2026-05-22-track-1-agent-lift-evaluation.md`
