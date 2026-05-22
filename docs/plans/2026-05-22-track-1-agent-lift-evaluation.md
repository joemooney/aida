# Plan: Track 1 agent-lift evaluation

Date: 2026-05-22
Specs: SPEC-397, SPEC-398, SPEC-399, SPEC-400, SPEC-401, SPEC-402
Status: Draft
Complexity: docs/protocol first, low implementation risk, medium product risk

## Approach

Track 1 builds the evidence loop for AIDA's central thesis: structured,
queryable project context should make Codex produce better engineering work
than a cold prompt over the same repository. The first version should be manual
and disciplined, not automated too early. We need a repeatable protocol, stable
templates, three carefully chosen benchmark tasks, and paired runs comparing a
control Codex attempt against an AIDA-assisted Codex attempt.

The output of Track 1 is not "Codex is good" or "AIDA is good". The output is
a calibrated answer to: which parts of AIDA help agents, which parts add
friction, and what should be improved next?

```
   SPEC-398 protocol
        |
        v
   SPEC-399 templates ---> SPEC-400 task set
        |                         |
        v                         v
   SPEC-401 paired run ----> more paired runs
        |
        v
   SPEC-402 learnings + follow-up tasks
```

## Decisions

- **Manual first**: start with docs/templates, not a new CLI command.
  **Rationale**: we do not yet know which fields matter enough to automate.
- **Paired runs**: every benchmark task gets a control run and an AIDA-assisted
  run. **Rationale**: a single agent run is anecdote; the contrast is the data.
- **Same task, separate context**: use the same task prompt, repo state, and
  verification target, but vary AIDA context access. **Rationale**: isolate
  the thing AIDA claims to improve.
- **Reviewer rubric is fixed before runs**: define the scoring rubric before
  seeing output. **Rationale**: avoid retrofitting success criteria to the run
  we liked.
- **Use bounded tasks only**: first benchmark tasks must be low-to-medium blast
  radius. **Rationale**: broad architecture tasks confound context quality with
  scope management.

## SPEC Breakdown

### SPEC-398: Define Agent-Lift Experiment Protocol

Deliver a protocol document under `docs/experiments/agent-lift/README.md`.

Required sections:

- Purpose and hypothesis.
- Control-run rules.
- AIDA-assisted-run rules.
- Required prompt shape.
- Required artifacts per run.
- Metrics and scoring rubric.
- Reviewer instructions.
- Stop conditions and invalid-run criteria.

Acceptance criteria:

- A human can run the protocol without needing hidden context.
- Codex can read the protocol and know what to produce.
- The protocol names what counts as an invalid comparison.
- The protocol does not rely on Claude-only slash commands.

### SPEC-399: Add Agent-Lift Experiment Templates

Deliver reusable markdown templates under `docs/experiments/agent-lift/`.

Suggested files:

- `benchmark-template.md` - the task definition.
- `run-notes-template.md` - one per agent attempt.
- `review-rubric-template.md` - fixed rubric before execution.
- `result-summary-template.md` - paired comparison.

Acceptance criteria:

- Templates include frontmatter or obvious fields for task ID, run date, agent,
  model if known, prompt, allowed files, verification commands, outcome, and
  reviewer findings.
- Templates distinguish observed facts from reviewer judgment.
- Templates include a place to record AIDA/MCP calls used.

### SPEC-400: Select First Three Benchmark Tasks

Pick three tasks that are appropriate for paired Codex runs.

Selection criteria:

- Clear acceptance criteria.
- Narrow file surface.
- Deterministic verification command.
- Does not require live GitHub, network, or external credentials.
- AIDA context plausibly helps: existing spec, trace requirement, workflow
  convention, or architectural constraint.
- Not dominated by frontend polish or subjective design.

Initial candidate classes:

- Docs-only Codex onboarding update.
- Mechanical extraction from `aida-cli/src/main.rs`.
- Traceability validation improvement with JSON output.

Rejection criteria:

- Requires broad orchestrator changes.
- Requires changing storage semantics.
- Requires judging current external Codex CLI behavior not represented in repo.
- Cannot be verified locally.

Acceptance criteria:

- Three benchmark definitions exist.
- Each has control prompt and AIDA-assisted prompt variants.
- Each has allowed files, non-goals, and verification commands.

### SPEC-401: Run First Paired Codex Benchmark

Run one benchmark task twice from equivalent repo state:

- Control: Codex sees repo instructions and task prompt, but is not told to use
  AIDA.
- AIDA-assisted: Codex must use AIDA first, preferably `aida show <SPEC>` and
  MCP/CLI graph queries where useful.

Required captured data:

- Full prompt given to Codex.
- Agent/model if known.
- Start/end time or wall-clock estimate.
- Files inspected.
- Files changed.
- Commands run and results.
- AIDA tools/commands used.
- Whether trace comments were added/preserved.
- Reviewer findings.
- Final verdict: better, worse, equivalent, or inconclusive.

Acceptance criteria:

- One paired run is recorded using the templates.
- The result summary names at least one concrete protocol improvement.
- No broad conclusion is drawn from a single run.

### SPEC-402: Summarize Benchmark Learnings And Tune Codex Playbook

After at least three paired runs, summarize what changed.

Required output:

- Which AIDA context was useful.
- Which AIDA context was unused or confusing.
- Whether MCP was more useful than CLI, or vice versa.
- Whether trace requirements improved output or caused friction.
- Recommended AGENTS.md changes.
- Recommended Codex task-prompt changes.
- Follow-up AIDA TASKs, all tagged `codex`.

Acceptance criteria:

- Summary is grounded in at least three paired result summaries.
- Recommendations are split into direct edits vs follow-up tasks.
- No recommendation depends on one anomalous run unless labeled as such.

## Experiment Protocol Detail

### Control Run

The control run should simulate a competent Codex user who has repository
instructions but no explicit AIDA workflow.

Rules:

- Provide the same task objective.
- Do not mention `aida show`, MCP tools, SPEC IDs, or trace policy unless the
  task itself naturally includes them.
- Allow normal repository exploration.
- Require normal verification.

Control prompt skeleton:

```text
Implement <task>. Preserve existing behavior unless the task says otherwise.
Inspect the codebase, make the smallest correct change, run relevant
verification, and summarize the result.
```

### AIDA-Assisted Run

The AIDA-assisted run should simulate Codex using AIDA as intended.

Rules:

- Begin by reading the relevant SPEC through AIDA.
- Ask Codex to preserve or add traceability where appropriate.
- Ask Codex to use the smallest set of files needed.
- Require the same verification as control.

AIDA-assisted prompt skeleton:

```text
Implement <task> using AIDA context first. Start by running `aida show <SPEC>`
or the equivalent MCP tool. Summarize the requirement and acceptance criteria
before editing. Preserve trace comments and add trace comments where the change
creates new source behavior tied to the spec. Run relevant verification and
report commands/results.
```

### Reviewer Rubric

Score each run on a 0-2 scale:

- Correctness: does it satisfy acceptance criteria?
- Minimality: did it avoid unrelated changes?
- Maintainability: is the code/doc easy to understand and consistent?
- Traceability: are SPEC links present and accurate where relevant?
- Verification: were appropriate checks run and reported?
- Context discipline: did the agent inspect enough context without wandering?

Record blocking findings separately from scoring. A run can score well on
discipline and still fail correctness.

### Invalid Comparisons

Mark a paired run invalid if:

- The second run starts from a materially different repo state.
- The task prompt changes the actual objective.
- One run has network/tool access the other lacks.
- A reviewer changes the rubric after seeing the first result.
- The task turns out to be too ambiguous to score.

## Files

### `docs/experiments/agent-lift/README.md`

- Add the protocol from SPEC-398.
- Include examples for control and AIDA-assisted prompts.
- Link to templates.

### `docs/experiments/agent-lift/benchmark-template.md`

- Define benchmark task metadata and acceptance criteria.
- Include allowed files, non-goals, and verification.

### `docs/experiments/agent-lift/run-notes-template.md`

- Capture one run's prompt, context usage, files touched, commands, and result.

### `docs/experiments/agent-lift/review-rubric-template.md`

- Freeze the rubric used before paired runs.

### `docs/experiments/agent-lift/result-summary-template.md`

- Compare control vs AIDA-assisted runs.

## Candidate First Benchmarks

### Benchmark A: Codex onboarding docs

Why useful:

- Low code risk.
- Directly tests whether AIDA context helps Codex understand current product
  state instead of repeating stale March assumptions.

Allowed files:

- `docs/user-guide.md`
- `docs/codex-quickstart.md`
- `docs/plans/2026-03-17-codex-cli-support.md`

Verification:

```bash
rg -n "codex|mcp|AGENTS.md" docs/user-guide.md docs/codex-quickstart.md
```

### Benchmark B: Trace command extraction

Why useful:

- Tests whether AIDA context helps Codex respect a mechanical extraction
  boundary in a large CLI file.

Allowed files:

- `aida-cli/src/main.rs`
- `aida-cli/src/trace.rs`
- `aida-cli/src/cli.rs` only if needed for compile.

Verification:

```bash
cargo check -p aida-cli
```

### Benchmark C: Dangling trace JSON output

Why useful:

- Product-value task directly connected to AIDA's traceability thesis.
- Measurable behavior and tests.

Allowed files:

- `aida-cli/src/main.rs` or `aida-cli/src/doctor.rs` after extraction.
- Focused tests in the same module.

Verification:

```bash
cargo check -p aida-cli
cargo test -p aida-cli trace
```

## Risks + Gotchas

1. **Risk**: Codex learns from the first paired run when running the second.
   **Mitigation**: record runs in separate branches/worktrees or reset state
   between attempts.
2. **Risk**: task selection biases toward AIDA. **Mitigation**: include at
   least one docs task, one mechanical code task, and one traceability task.
3. **Risk**: subjective scoring dominates. **Mitigation**: require concrete
   findings and command results alongside scores.
4. **Risk**: AIDA-assisted prompt becomes too prescriptive. **Mitigation**:
   keep task objective identical; only context acquisition differs.
5. **Risk**: benchmark artifacts rot. **Mitigation**: store them as dated
   files under `docs/experiments/agent-lift/runs/`.

## Verification

For SPEC-398 and SPEC-399:

```bash
test -f docs/experiments/agent-lift/README.md
test -f docs/experiments/agent-lift/benchmark-template.md
test -f docs/experiments/agent-lift/run-notes-template.md
test -f docs/experiments/agent-lift/review-rubric-template.md
test -f docs/experiments/agent-lift/result-summary-template.md
```

For SPEC-400:

```bash
find docs/experiments/agent-lift/benchmarks -type f | sort
```

For SPEC-401:

```bash
find docs/experiments/agent-lift/runs -type f | sort
```

For SPEC-402:

```bash
test -f docs/experiments/agent-lift/summary.md
rg -n "follow-up|codex|AIDA-assisted|control" docs/experiments/agent-lift/summary.md
```

## Codex Assignment Order

1. Assign SPEC-398 and SPEC-399 together as a docs-only task.
2. Human reviews the protocol for bias and missing fields.
3. Assign SPEC-400 to select the first benchmarks.
4. Human approves the benchmark set.
5. Run SPEC-401 manually with Codex in two branches/worktrees.
6. Repeat until there are three paired runs.
7. Assign SPEC-402 to summarize learnings and file follow-up TASKs.

## Followups

- Add an `aida experiment` command only after the manual protocol has repeated
  value.
- Add cost/token capture if Codex exposes stable local accounting.
- Add a benchmark for MCP-only vs CLI-only AIDA access.
- Add an external-project benchmark after AIDA-on-AIDA results are stable.

## Related

- `docs/plans/2026-05-22-codex-driven-aida-improvement.md`
- `docs/plans/2026-03-17-codex-cli-support.md`
- `docs/architecture/mcp-coordination-surface.md`
