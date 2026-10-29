# Your first project with AIDA

This walkthrough follows a person and an AIDA-aware agent taking a small TODO application from intent to reviewed work. The conversation is illustrative; identifiers and agent responses depend on the project. The agent performs the routine AIDA operations.

## Before the conversation

An operator has installed AIDA, connected their coding agent, and initialized the Git repository using [Getting started](getting-started.md). Appropriate advisor/reviewer authority and a forge/CI setup are needed for the PR lifecycle. AIDA does not grant those permissions merely because the example asks for them.

## 1. Describe the product

**You:**

> Build a small TODO command-line app. It should add tasks, list them and mark them done. Record the requirements in AIDA. Let's agree the behavior before you implement it.

**The agent's work:** inspect existing specs, propose an epic and child stories, and capture drafts. For example:

```text
TODO application
├── Add and list tasks
├── Mark a task done
└── Persist tasks between runs
```

The agent uses `aida add` or the typed MCP intake tool. Those calls belong to the agent; you review the proposed scope. Display IDs may be node-local, aliased or agreed short IDs. Use the actual returned IDs rather than assuming one global counter across types.

## 2. Resolve the decisions

**You:**

> Keep it local and single-user. Use a JSON file. Listing an empty file should show an empty list, not fail. A task's completed status must survive restart.

**The agent's work:** record those choices and acceptance criteria on the relevant specs, identify dependencies and ask about anything still unresolved. A decision that blocks execution remains explicit instead of becoming an agent's hidden assumption.

Ask the agent to show the criteria before work is approved. An advisor-authorized session handles approval and routing. This keeps filing a draft separate from committing the team to build it.

## 3. Choose the execution mode

**You:**

> Start with the add-and-list feature in supervised delivery. Build it and run CI, then stop for my review.

**The agent's work:** recommend or select `drive` within its authority, queue the approved spec and launch the implementation in an isolated worktree. The stored mode defines the handoff: implementation through CI, followed by human review and merge. It is not a permission bypass.

For a design-heavy story, choose `guided`. For bounded, authorized work with settled decisions, `drain` runs the agent lifecycle through independent review and merge. `operator` leaves the work to a human; `decide` blocks it until an answer exists.

## 4. Follow progress in ordinary language

**You:**

> What is active, what is blocked, and do you need a decision from me?

**The agent's work:** inspect session/queue/status evidence and report what is actually happening. It should distinguish an open PR from a merged change, a completed session from completed work, and waiting for CI from waiting for a human.

The agent can use `aida status`, `aida ps`, `aida awaiting` and spec reads. Operators can also run those commands directly. People should not have to recreate the workflow by manually chaining command-line mutations.

## 5. Review the outcome

**You:**

> Show me how each acceptance criterion was checked, the diff and test results, and the review findings. Tell me what is still outstanding.

**The agent's work:** connect the spec to the implementation, relevant tests, exact reviewed commit and PR. Review must follow the current [review policy](review-process.md); the implementer's summary is not independent verification.

In supervised delivery you take the review/merge step. Under autonomous delivery the independent reviewer and integrator operate within their grants and gates. An exception can still stop the work for a decision.

## 6. Start the next session without starting over

**You:**

> Pick up the persistence story. Read the decisions and completed work first. Keep the same behavior for empty data.

The new session can recover scope from the graph and traces rather than relying on a previous chat being present. That continuity is the point of the exercise: the requirement, decision and evidence outlive the agent session.

## What you should have at the end

- A versioned epic/story graph with explicit behavior and dependencies.
- Work isolated by session, with the selected human handoff respected.
- Code and commits linked to requirements.
- Validation and review evidence attached to the change.
- Outstanding choices visible rather than silently guessed.

A merged feature is not a production deployment. Configure deployment and release policy separately. Continue with [configurability](configuration.md), [the lifecycle](lifecycle.md), or the [agent/operator CLI reference](cli/README.md).

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
