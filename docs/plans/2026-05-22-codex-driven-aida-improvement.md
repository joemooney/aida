# Plan: Codex-driven AIDA improvement program

Date: 2026-05-22
Specs: Strategy / follow-up planning
Status: Draft
Complexity: multi-track program, risk medium

## Approach

AIDA's strongest product thesis is that a durable spec graph, trace links, and
agent-accessible coordination state should make coding agents produce better
work than cold-starting from prose. The next phase should prove and harden that
thesis with Codex as a first-class implementer: measure whether AIDA improves
agent output, tighten traceability into an enforceable invariant, productize the
MCP/Codex surface, and reduce the CLI's internal concentration so agents can
work safely in smaller blast radii.

This plan deliberately separates product-value work from autonomy-plumbing
work. Autonomous drains are valuable, but the tool should not become mostly a
machine for maintaining its own orchestration machinery. Codex should be used
where tasks are bounded, testable, and easy to review.

```
        AIDA thesis
             │
             ▼
   prove agent lift ──► enforce traceability ──► productize Codex/MCP
             │                    │                       │
             └──────────────► refactor CLI seams ◄────────┘
```

## Current Assessment

### What is already strong

- AIDA has a coherent durable-context model: stable SPEC IDs, requirement graph,
  git-canonical object store, SQLite cache, trace comments, queue/session
  lifecycle, and MCP access.
- Codex is no longer merely aspirational. AGENTS.md generation, `.codex/skills`,
  MCP registration, OpenAI/Codex-aware server LLM selection, and the MCP
  coordination surface now exist.
- The core Rust path currently builds: `cargo check -p aida-core -p aida-cli`
  passes.
- The AIDA graph itself is useful operationally: backlog, findings, punts,
  leases, and directives are queryable via MCP.

### What is still weak

- The central product thesis is not measured. We do not yet have repeatable
  evidence that AIDA context improves Codex/Claude output quality, review
  outcome, implementation time, trace coverage, or punt rate.
- Traceability is still partly convention. Scanning and validation exist, but
  the merge/CI story should make trace coverage and live-spec resolution a
  checked invariant.
- `aida-cli/src/main.rs` is too concentrated. At roughly 57k lines, it is a
  risky target for autonomous edits and slows human review.
- The backlog is dominated by autonomous-drain hardening. That work matters,
  but it can crowd out product-facing improvements unless explicitly bounded.

## Decisions

- **Codex as implementer, not unchecked maintainer**: use Codex for scoped
  tasks with concrete tests and file boundaries. **Rationale**: AIDA should
  benefit from agent acceleration without letting agents make broad structural
  changes invisibly.
- **Measurement before deeper autonomy**: build an evaluation harness before
  adding more drain behavior. **Rationale**: more autonomy is only valuable if
  it increases useful shipped work without increasing review burden.
- **MCP is the cross-agent contract**: treat Claude skills as a high-value
  profile, but use MCP + AGENTS.md as the portable surface. **Rationale**:
  Codex, Cursor, and future agents will not share Claude's slash-command model.
- **Refactor around seams already present**: extract from `main.rs` into modules
  that already exist conceptually: trace, doctor, queue, lifecycle, codex, and
  orchestrator. **Rationale**: mechanical extraction is reviewable and lowers
  risk before behavior changes.
- **Prefer CI/checkable policies over prompt discipline**: Codex can follow
  instructions, but repository invariants should be executable. **Rationale**:
  AIDA's value proposition is durable context, not "hope the prompt was read."

## Track 1: Agent-lift evaluation harness

### Goal

Create a repeatable benchmark that answers: does AIDA context improve agent
output?

### Initial metrics

- Implementation success: tests pass, acceptance criteria satisfied.
- Trace coverage: changed source files/functions reference live SPEC IDs.
- Review burden: number and severity of reviewer findings.
- Context efficiency: number of files inspected and wall-clock duration.
- Decision quality: number of punts, design forks, and unresolved assumptions.
- Regression rate: follow-up bugs filed after merge.

### First benchmark shape

Use paired tasks with the same codebase and task prompt:

- Control: Codex gets only repository instructions and the task text.
- AIDA-assisted: Codex must query AIDA first through MCP or CLI, inspect the
  target spec, and preserve traceability.
- Reviewer: separate Codex or Claude review pass evaluates both outputs against
  the same rubric.

### Candidate Codex task prompt

```text
You are implementing a bounded AIDA task. First run `aida show <SPEC>` and
summarize the acceptance criteria. Then inspect only the files needed for this
change. Implement the task, add or preserve trace comments where appropriate,
run the narrowest relevant tests, and report verification.
```

### Build-order work

- Add `docs/experiments/agent-lift/README.md` describing the protocol.
- Add an experiment template for task prompt, control output, AIDA-assisted
  output, reviewer rubric, and measured results.
- Add a small script or CLI subcommand later only if repeated manual runs prove
  the protocol is useful.

### Acceptance criteria

- At least three benchmark tasks can be run manually end-to-end.
- Results capture both quantitative metrics and reviewer notes.
- The protocol does not depend on Claude-only artifacts.

## Track 2: Traceability governance

### Goal

Make "changed code maps to live requirements" an executable property.

### Current assets

- `aida trace scan`, `aida trace sweep`, and trace-link model.
- `aida doctor validate-trace-comments`.
- Git hook scaffolding in `aida-core/src/scaffolding/hooks.rs`.
- SPEC ID and agreed ID resolution in the git-canonical store.

### Proposed increments

1. Add a CI-friendly trace command that reports JSON and exits non-zero on
   unresolved trace IDs.
2. Add a diff-aware mode that checks only files changed relative to a base ref.
3. Add a policy mode that distinguishes source/test/doc/config expectations.
4. Add an AGENTS.md section telling Codex exactly how to satisfy the policy.
5. Add a review recipe that checks changed files against the trace policy.

### Candidate Codex task prompt

```text
Implement a narrow traceability check improvement. Do not redesign the trace
model. Add a CI-friendly output path to the existing trace/doctor code, keep
legacy output unchanged, and add unit tests for unresolved known/unknown SPEC
IDs. Run `cargo check -p aida-cli` and the focused tests.
```

### Acceptance criteria

- CI can run one command to fail on dangling trace comments.
- The command can scope to changed files for PR checks.
- Output is machine-readable enough for GitHub Actions or local hooks.
- Existing interactive doctor behavior remains unchanged.

## Track 3: Codex/MCP productization

### Goal

Make "use AIDA with Codex" a polished, documented path rather than a set of
pieces that happen to exist.

### Proposed increments

1. Update the stale March Codex plan with a "current state" note or replace it
   with a short status page.
2. Add a Codex quickstart to user docs:
   `aida init --agent codex`, `aida mcp register-agent --print`, `codex mcp add`,
   `aida show`, and one implementation recipe.
3. Expand AGENTS.md generated content to list all current MCP tools, including
   coordination tools, not only the original spec graph tools.
4. Add a `aida codex doctor` or equivalent report only if there is enough
   Codex-specific setup to validate.
5. Add Codex review recipes for traceability, requirements drift, and
   implementation-plan adherence.

### Candidate Codex task prompt

```text
Improve AIDA's Codex onboarding docs only. Do not change Rust behavior. Read
the current AGENTS.md generator, MCP tool descriptors, and docs/plans
2026-03-17-codex-cli-support.md. Produce a concise docs page that reflects the
current implementation and includes one verified smoke workflow.
```

### Acceptance criteria

- A new user can configure Codex + AIDA MCP from docs alone.
- Generated AGENTS.md does not claim unavailable features.
- The docs distinguish project-local `.mcp.json` from Codex CLI registration.
- The setup path is provider-neutral where possible.

## Track 4: CLI modularization

### Goal

Reduce the risk and review cost of future Codex-authored changes by shrinking
the blast radius of `aida-cli/src/main.rs`.

### Extraction order

1. Move trace command implementation into `aida-cli/src/trace.rs`.
2. Move doctor trace-validation helpers into `aida-cli/src/doctor.rs`.
3. Move MCP registration command handling into `aida-cli/src/mcp_register.rs`
   or into `mcp.rs` if dependencies stay clean.
4. Move queue lifecycle helpers that are already pure or near-pure into
   `queue_lifecycle.rs`.
5. Only then consider larger orchestrator extraction.

### Candidate Codex task prompt

```text
Perform a mechanical extraction only. Move the trace command implementation
from `aida-cli/src/main.rs` into a new `aida-cli/src/trace.rs` module. Preserve
all behavior and public CLI shape. Prefer moving existing functions unchanged
over refactoring. Add `mod trace;` and update call sites. Run
`cargo check -p aida-cli`.
```

### Acceptance criteria

- Each extraction commit compiles independently.
- No CLI behavior changes.
- Tests are moved with the code when practical.
- The diff is reviewable as relocation plus minimal visibility/import changes.

## Track 5: Backlog hygiene for Codex work

### Goal

Use AIDA itself to make Codex work queueable and reviewable.

### Rules

- Every Codex task should have a SPEC or TASK before implementation.
- Each task should name its allowed files and explicit non-goals.
- Prefer low/medium blast-radius tasks first.
- Avoid asking Codex to "clean up architecture" without a concrete extraction
  target.
- Findings from Codex or reviewers should be filed as follow-up TASKs, not
  opportunistically bundled.

### Suggested first tasks to file

- Add an agent-lift experiment template under `docs/experiments/`.
- Add Codex quickstart docs reflecting the current MCP surface.
- Extract trace command implementation from `main.rs`.
- Add JSON output to dangling trace validation.
- Add diff-scoped trace validation for PR use.
- Update generated AGENTS.md to include the coordination MCP tool categories.

## Execution Sequence

### Phase 0: Prepare the runway

- Review and approve this plan.
- File the suggested first tasks in AIDA with clear acceptance criteria.
- Pick one low-risk documentation task and one mechanical extraction task for
  Codex trial runs.

### Phase 1: Prove Codex can work safely in this repo

- Run Codex on docs-only onboarding work.
- Run Codex on one mechanical extraction.
- Review for drift, overreach, test quality, and trace discipline.
- Capture the results in the agent-lift experiment log.

### Phase 2: Turn traceability into policy

- Add CI-friendly trace validation.
- Add diff-scoped validation.
- Add Codex review recipes that enforce the same policy.

### Phase 3: Measure AIDA's lift

- Run at least three paired benchmark tasks.
- Compare control vs AIDA-assisted runs.
- Use results to tune AGENTS.md, MCP tool descriptions, and task templates.

### Phase 4: Continue modularization

- Extract additional low-risk modules from `main.rs`.
- Keep behavior-preserving extraction separate from feature changes.
- Use the smaller modules as future Codex task boundaries.

## Risks + gotchas

1. **Risk**: Codex makes broad opportunistic changes. **Mitigation**: give
   file-bound prompts, require verification, and reject diffs outside scope.
2. **Risk**: benchmark results become anecdotal. **Mitigation**: use paired
   tasks, same rubric, and record failures as well as successes.
3. **Risk**: traceability policy creates false positives. **Mitigation**:
   start with report-only mode, then fail only on unresolved trace IDs before
   enforcing coverage percentages.
4. **Risk**: extraction churn conflicts with active orchestrator work.
   **Mitigation**: extract stable surfaces first: trace, doctor, MCP
   registration.
5. **Risk**: docs drift as Codex CLI behavior changes. **Mitigation**: keep
   docs focused on AIDA-owned commands and include a small smoke test rather
   than hard-coding too much vendor behavior.

## Tests

- `trace_validation_json_reports_unknown_spec_ids` — JSON output includes
  unresolved spec IDs and locations.
- `trace_validation_json_exits_zero_when_all_specs_resolve` — happy path.
- `trace_validation_diff_scope_ignores_unchanged_files` — PR mode stays narrow.
- `generated_agents_md_lists_current_mcp_categories` — AGENTS.md mentions spec
  graph plus coordination tools.
- `trace_module_extraction_preserves_cli_dispatch` — command dispatch still
  reaches trace handlers after extraction.

## Verification

Run after each code-bearing task:

```bash
cargo check -p aida-core -p aida-cli
cargo test -p aida-cli trace
./target/debug/aida mcp register-agent --print
./target/debug/aida doctor validate-trace-comments
```

Run after docs/scaffolding tasks:

```bash
cargo test -p aida-core scaffolding
./target/debug/aida scaffold diff --list
```

Run after Codex onboarding work:

```bash
./target/debug/aida mcp register-agent --print
codex mcp list
```

The last command is optional in CI because Codex may not be installed.

## Codex Operating Playbook

Use this pattern for early Codex assignments:

```text
Task: <one sentence>
Spec: <SPEC-ID or docs plan section>
Allowed files: <explicit list>
Non-goals: <explicit list>
Required first steps:
- Run `aida show <SPEC>` if a spec exists.
- Inspect the named files only unless blocked.
- State the implementation plan before edits.
Required verification:
- <commands>
Stop conditions:
- Unexpected unrelated changes.
- Need to edit files outside allowed scope.
- Test failure that requires design change.
```

Good first Codex runs:

```text
Docs-only: Create a Codex quickstart page from the current MCP registration and
AGENTS.md generator. Allowed files: docs/user-guide.md, docs/codex-quickstart.md,
docs/plans/2026-03-17-codex-cli-support.md. Non-goal: no Rust changes.
```

```text
Mechanical extraction: Move trace command implementation from main.rs to
trace.rs without behavior changes. Allowed files: aida-cli/src/main.rs,
aida-cli/src/trace.rs, aida-cli/src/cli.rs only if compile requires it.
Non-goal: no trace behavior changes.
```

```text
Policy increment: Add JSON output to dangling trace validation. Allowed files:
aida-cli/src/main.rs or trace/doctor module after extraction, tests in the same
module. Non-goal: no diff-scoped validation yet.
```

## Followups

- Add a dedicated `aida codex doctor` only if repeated setup failures show it
  is worth a command.
- Add GitHub Actions snippets for trace validation after the local command
  stabilizes.
- Add provider-neutral task templates for Cursor or other MCP agents after the
  Codex path is proven.
- Consider a top-level `docs/experiments/` index if agent-lift experiments
  become a recurring practice.

## Related

- `docs/plans/2026-03-17-codex-cli-support.md` — older Codex support plan,
  partially implemented and now stale in places.
- `docs/architecture/mcp-coordination-surface.md` — current MCP coordination
  design.
- `docs/archive/PROJECT_EVALUATION_2026-02-28.md` — prior project evaluation.
- `docs/autonomous-drain.md` — no-human / advisor / drain behavior.
- `docs/storage-modes.md` — git-canonical storage model.
