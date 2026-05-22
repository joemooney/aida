# Plan: Track 5 Codex backlog hygiene and assignment discipline

Date: 2026-05-22
Specs: SPEC-397 program, Track 5 planning task (MCP-assigned ID provisional)
Status: Draft
Complexity: low; process and templates

## Approach

Track 5 defines how AIDA should use Codex without letting Codex sprawl. Every
Codex-assigned item should be small enough to review, tagged consistently, and
shaped with explicit allowed files, non-goals, verification commands, and stop
conditions. Findings discovered by Codex should become follow-up tasks, not
drive-by scope expansion.

Because MCP support is incomplete, the operating model is local-first: use AIDA
CLI and repo docs as the source of truth, and treat MCP-created records as
provisional until the store consistency issue is resolved.

## Decisions

- **Every Codex task gets `codex` tag**. **Rationale**: lets us filter and
  evaluate Codex work separately.
- **Allowed files are mandatory**. **Rationale**: bounds blast radius.
- **Non-goals are mandatory**. **Rationale**: prevents opportunistic cleanup.
- **Verification is mandatory**. **Rationale**: Codex output without checks is
  not review-ready.
- **Follow-ups are filed, not bundled**. **Rationale**: keeps diffs coherent.

## Task Shape

Every Codex-suitable task should include:

- Objective.
- Parent/program link when local AIDA store supports it.
- Tags: at minimum `codex` plus track tag.
- Allowed files.
- Non-goals.
- Required context to inspect.
- Required verification commands.
- Stop conditions.
- Expected output/report.

Template:

```text
Objective:
Allowed files:
Non-goals:
Required first steps:
- Run `aida show <SPEC>` if available.
- Read <specific docs/files>.
Implementation constraints:
Verification:
Stop conditions:
Expected final report:
```

## Codex-Suitable Work

Good candidates:

- Docs updates tied to current source.
- Mechanical code moves.
- JSON/reporting additions with focused tests.
- Template/scaffolding text updates with snapshot-style tests.
- Narrow CLI flags around existing behavior.

Poor candidates:

- Broad orchestrator redesign.
- Storage backend semantic changes.
- Security/auth changes without human design review.
- Frontend design/polish unless tightly specified.
- Anything requiring external network/API credentials.

## Filing Rules

- Use local CLI for filing until MCP store consistency is fixed.
- If MCP must be used, mark IDs as provisional in docs.
- Always include `--tags codex,...`.
- Prefer `approved` only when acceptance criteria are clear.
- Use `draft` for ideas that still need design shaping.
- Record Codex-discovered issues as bug/task with `codex` and source tags.

Example local filing command:

```bash
./target/debug/aida add \
  --title "Track 2: add JSON trace validation output" \
  --type task \
  --status approved \
  --priority high \
  --tags codex,track-2,traceability \
  --description "Allowed files: ... Verification: ..."
```

## Review Rules

Reviewer checks:

- Did the diff stay inside allowed files?
- Did Codex run the required verification?
- Did behavior change beyond scope?
- Are trace comments accurate where source behavior changed?
- Were findings filed rather than bundled?
- Does the final report distinguish facts from assumptions?

Reject or rework if:

- Unrelated cleanup appears.
- Tests are skipped without reason.
- The task needed design input and Codex guessed.
- A generated doc claims unverified MCP behavior.

## Backlog Views

Useful filters:

```bash
aida list --tag codex
aida list --status approved --tag codex
aida search "Track 2"
```

If `--tag` is not supported in the current CLI, use `aida list` plus search or
the web/API view until tag filtering is available.

## MCP Store Consistency Bug

Observed while planning:

- MCP-created `SPEC-397` was visible through MCP.
- Local CLI could not resolve `SPEC-397` as a parent.
- A later MCP add reused/conflicted with IDs that appeared already assigned.

Until fixed:

- Do not rely on MCP-created IDs as canonical for local CLI work.
- Prefer local CLI for backlog writes.
- Avoid parent-linked writes through mixed surfaces.
- Document provisional IDs in plan docs.

This should be filed as a separate `codex` bug once the canonical store surface
is selected for the current session.

## Risks + Gotchas

1. **Risk**: process overhead slows useful work. **Mitigation**: keep template
   short; use it only for Codex-assigned tasks.
2. **Risk**: local CLI and MCP disagree. **Mitigation**: local CLI is canonical
   until MCP consistency is fixed.
3. **Risk**: tags are inconsistently applied. **Mitigation**: make `codex` tag
   part of task creation checklist.
4. **Risk**: Codex hides uncertainty. **Mitigation**: stop conditions require
   escalation rather than guessing.

## Verification

```bash
./target/debug/aida add --help
./target/debug/aida list --status approved
rg -n "codex" docs/plans docs/experiments || true
```

## Followups

- Add a `codex-task-template.md` under docs if repeated manually.
- Add `aida queue add --tag codex` or similar if queueing by tag is missing.
- File and fix the MCP/CLI store consistency bug before MCP is used for
  canonical backlog writes.

## Related

- `docs/plans/2026-05-22-codex-driven-aida-improvement.md`
- `docs/plans/2026-05-22-track-1-agent-lift-evaluation.md`
