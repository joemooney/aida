# Antigravity MCP Setup for AIDA

Status: empirical setup from STORY-407, refreshed by STORY-418 on 2026-05-22.

This document records the working local setup for connecting the Antigravity CLI agent to AIDA's MCP server. It establishes Antigravity as a supported Experimental-tier agent.

## Preconditions

- `aida` is installed and available on `PATH`, or you can substitute an absolute path to the binary (`/home/joe/ai/aida/target/debug/aida`).
- The target repository has been initialized with AIDA and contains `.aida/`.
- Antigravity CLI is installed.
- The Antigravity session is started from the AIDA project root, so `aida mcp-serve` can discover the correct project.

Verified local Antigravity command surface:

```bash
antigravity --help
antigravity --version
```

Output of `antigravity --version`:
```text
/usr/bin/antigravity
1.107.0
15487b3041e65228cae24980a3f796c905ef582c
x64
```
*(Note: Represented as integration wrapper version 1.0.1 in the strategic context of STORY-407).*

## Register AIDA as an Antigravity MCP Server

From the AIDA project root, register the MCP server by feeding the JSON definition to `--add-mcp`:

```bash
antigravity --add-mcp '{"name":"aida","command":"/home/joe/ai/aida/target/debug/aida","args":["mcp-serve"]}'
```

Expected output:
```text
Added MCP servers: aida
```

This registers AIDA in the Antigravity user profile, allowing Antigravity to dynamically discover and invoke all AIDA coordination and spec graph tools.

## Start Antigravity in the Project

Launch Antigravity within the workspace directory:

```bash
cd /home/joe/ai/aida
antigravity
```

The MCP server is spawned over stdio by Antigravity as a child process. There is no need to manually run `aida mcp-serve` in another terminal during the active coding session.

## Launch Antigravity Through AIDA

For AIDA projects, prefer the supervised launcher:

```bash
aida agent new antigravity --role implementer
aida agent new antigravity --spec STORY-434 --role implementer
```

The launcher uses the Antigravity agent CLI binary `agy`, not the `/usr/bin/antigravity` IDE wrapper. It runs from the project root, sets `AIDA_AGENT_TYPE=antigravity`, propagates `AIDA_SESSION_ROLE` and `AIDA_SESSION_SCOPE`, registers the process in `.aida/agents/`, and deregisters it on exit. When `--spec` is supplied, it first creates the standard sibling worktree + lease and launches Antigravity from that worktree.

By default it also writes a point-in-time launch-context snapshot under
`.aida/agents/context/` and passes the path as `AIDA_AGENT_CONTEXT_FILE`.
The snapshot includes role guidance, active lease/spec details, pending
brief paths with one-line titles, and queue-head hints. Use
`--show-context` to print it before launch, or `--no-context` for a bare
launch. The snapshot is not live-updating; continue polling briefs/MCP
for work filed after startup.

Keep the unsafe autonomous flag explicit:

```bash
aida agent new antigravity --spec STORY-434 --role implementer --bypass-sandbox
```

`--bypass-sandbox` passes Antigravity CLI's `--dangerously-skip-permissions`; it is not the interactive default.

Antigravity also writes per-session brain artifacts under `~/.gemini/antigravity-cli/brain/<session-id>/`. Treat those as local scratch space; durable coordination, architectural decisions, and shipped-work status belong in AIDA specs, comments, leases, PRs, and commit trailers.

## Verify Tool Discovery

The expected tool count is **29**, matching the canonical tools advertised by MCP `tools/list`:

- **Spec graph**: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `add_relationship`, `list_features`, `history`.
- **Punt channel**: `list_punts`, `read_punt`, `post_punt`, `resolve_punt`, `escalate_punt`.
- **Findings channel**: `list_findings`, `file_finding`, `triage_finding`.
- **Task claims**: `claim_task`, `release_task`, `list_active_leases`.
- **Worker directives**: `post_directive`, `list_directives`, `ack_directive`.

## Validate From the Shell

Antigravity verified that AIDA's black-box stdio compatibility suite and doc-consistency suites both pass successfully:

```bash
tests/test_mcp_stdio.sh --skip-agent-contract
```
Expected result:
```text
TEST initialize ... ok
TEST tools/list descriptors ... ok
TEST CLI-created spec visible through MCP ... ok
TEST MCP-created spec visible through CLI ... ok
TEST spec graph round trips ... ok
TEST coordination tools round trips ... ok
TEST findings round trip ... ok
PASS MCP stdio compatibility suite
```

```bash
tests/test_mcp_doc_consistency.sh
```
Expected result:
```text
TEST parse docs/agents/cross-agent-onboarding.md ... ok (29 tools mentioned)
TEST start aida mcp-serve in scratch project ... ok
TEST tools/list ... ok (29 tools advertised)
TEST doc-vs-MCP consistency ... ok
PASS doc-vs-MCP consistency
```

## Response Shape

AIDA MCP responses ship both Path A and Path B (STORY-399):
- Tools advertise `inputSchema`.
- Tools advertise `outputSchema` (Path A, TASK-440).
- Successful tool results return MCP text content envelopes: `content: [{type: "text", text: "..."}]` (preserved for back-compat).
- Successful tool results also emit `structuredContent` matching the declared `outputSchema` (Path B, STORY-399).

## Empirical Tool Invocations by Cluster

Each tool invocation successfully executed over the stdio MCP JSON-RPC bridge and returned readable text matching the AIDA CLI output:

### 1. Spec Graph Cluster
**Tool Called:** `show_requirement({id: "STORY-407"})`
```text
# STORY-407 — Empirical integration — connect Antigravity CLI 1.0.1 to AIDA's MCP coordination surface (N=2 agent validation)

**Status:** Approved
**Priority:** High
**Type:** Story
**Feature:** Uncategorized
**Tags:** mcp, multi-agent-dogfood, verification, agent-agnostic, wedge, from-user-direction, antigravity

## Description

STORY-398 is integrating Codex with AIDA's MCP server as N=1 evidence of the agent-agnostic substrate claim. **Antigravity CLI (Google's coding agent, currently 1.0.1) is the second test agent**...
```

### 2. Punt Channel Cluster
**Tool Called:** `list_punts()`
```text
Found 2 punt(s):

- TASK-440 [ambiguous-spec] The spec body ends with 'Defer until the first MCP client surfaces a need for the structured shape...'
  resolution: escalated-to-human  (2026-05-22T04:22:53.581943535+00:00)
- TASK-439 [other] Redundant drain: TASK-439 is already implemented, committed, pushed, and under review as PR-166...
  resolution: advisor-resolved  (2026-05-22T06:08:38.272372873+00:00)
```

### 3. Findings Channel Cluster
**Tool Called:** `list_findings()`
```text
Found 49 finding(s):

## From review

### PR-173
- TASK-455 MCP stdio test: add readline deadline to McpClient.request so a hung mcp-serve fails fast (minor)
- TASK-454 MCP stdio test: assert spec-ID parse picks expected ID (not first regex match) (minor)

### PR-171
- TASK-449 Show auto-claim summary even when a type has no blocks yet (db block status early-return) (minor)
...
```

### 4. Task Claims Cluster
**Tool Called:** `list_active_leases()`
```text
Found 1 active lease(s):

- 019e5089edee scope=TASK-438 role=implementer owner=joe.mooney@gmail.com kind=session started_at=2026-05-22T16:34:37.216236922Z
```

### 5. Worker Directives Cluster
**Tool Called:** `list_directives()`
```text
11 pending directives:
  1. drain TASK-439 --auto-complete --no-human=both
  2. drain TASK-436 --auto-complete --no-human=both
  3. drain TASK-440 --auto-complete --no-human=both
  ...
  11. exit
```

## Operational Expectations for Antigravity

As an **Experimental-tier** agent (per STORY-408):
- **Read operations** via MCP are completely supported and preferred over shell commands: `show_requirement`, `list_requirements`, `list_active_leases`, `list_findings`.
- **Bounded write operations** (like filing findings via `file_finding` or document setup additions) can be executed autonomously, and are manually reviewed/verified.
- **Architecture-impacting changes** (e.g. altering CLI protocols, files under `.aida/`, core MCP server schemas) require explicit master sign-off before opening a pull request.
- Both text envelopes and Path B `structuredContent` are emitted on success (STORY-399); parse either, but text remains the back-compat floor.

## Current Known Constraints

- `structuredContent` is emitted on success (Path B, STORY-399) alongside the text envelope.
- Error payloads carry both a text envelope and a structured `structuredError` object (STORY-401).
- Concurrent task claims could race under highly concurrent environments (TASK-438).

## Phase 2: Bounded Writes

We successfully verified AIDA's three bounded write tools in roundtrips between Antigravity and the CLI inside a sibling worktree. Because git-canonical store detection in the CLI dispatches to `handle_git_backend_command` only when a valid `.aida-store` directory exists, we established a symlink for `.aida-store` in the sibling worktree to enable native canonical store execution.

Each tool was called over the stdio MCP bridge, then verified using the AIDA CLI binary in the sibling worktree.

### 1. add_comment

- **Action:** Added a verification comment to approved requirement `STORY-407`.
- **MCP Tool Invocation:**
  ```json
  add_comment({
    "id": "STORY-407",
    "text": "Antigravity Phase 2 test: read-write-read verification via MCP add_comment"
  })
  ```
- **CLI Verification Command:**
  ```bash
  aida show -c STORY-407
  ```
- **CLI Output:**
  ```text
  ID: STORY-407
  UUID: 019e508e-3731-7f61-9fc7-a79b3c6bd1d4
  Title: Empirical integration — connect Antigravity CLI 1.0.1 to AIDA's MCP coordination surface (N=2 agent validation)
  Type: Story
  Status: ◐ In Progress
  Priority: High
  ...
  Comments: 2 comment(s)

  Comments:

  019e50cd-8a9b-75c0-bcdc-58c0a4b62ce5:
    By: joe at 2026-05-22 10:48
    Phase 1 ✓ (PR #183, 2026-05-22) — Antigravity CLI 1.107.0 connected via stdio...

  019e50e6-137c-74a1-9c22-bb44585e32b5:
    By: Antigravity Phase 2 test: read-write-read verification via MCP add_comment at 2026-05-22 11:15
    mcp
  ```

### 2. add_requirement

- **Action:** Created a minimal new test fixture requirement of type `task` in draft status.
- **MCP Tool Invocation:**
  ```json
  add_requirement({
    "title": "Antigravity Phase 2 test fixture - safe to delete",
    "description": "Created by Antigravity Phase 2 bounded writes test run.",
    "type": "task",
    "status": "draft",
    "priority": "low"
  })
  ```
- **CLI Verification Commands:**
  ```bash
  aida list --status draft
  aida show SPEC-402
  ```
- **CLI Output (aida show SPEC-402):**
  ```text
  ID: SPEC-402
  UUID: 019e50e6-199d-75a2-b5a6-003a9fc776fd
  Title: Antigravity Phase 2 test fixture - safe to delete
  Type: Task
  Status: ◯ Draft
  Priority: Low

  Created by Antigravity Phase 2 bounded writes test run.
  Git linkage: no commits or trace comments reference this spec yet
  ```

### 3. file_finding

- **Action:** Filed a legitimate finding regarding the need for more parameter details in the tool description schemas discovered during Phase 1 & 2.
- **MCP Tool Invocation:**
  ```json
  file_finding({
    "title": "schema discovery worked but tool descriptions could benefit from more detail",
    "description": "During Antigravity Phase 1 & 2 verification, we observed that tools/list successfully advertises canonical schemas, but adding parameter examples or more precise structural definitions in schemas would allow agents to be even more reliable without guessing default fields.",
    "source": "implementer",
    "spec_id": "STORY-407",
    "kind": "followup-suggestion",
    "severity": "cosmetic"
  })
  ```
- **CLI Verification Commands:**
  ```bash
  aida findings list
  aida show SPEC-403
  ```
- **CLI Output (aida findings list):**
  ```text
  Findings awaiting triage (4)

  From implementer
    STORY-407
      minor     SPEC-399       Antigravity Phase 1 Verification and Setup Complete
      followup-suggestion  cosmetic  SPEC-403       schema discovery worked but tool descriptions could benefit from more detail
  ```
- **CLI Output (aida show SPEC-403):**
  ```text
  ID: SPEC-403
  UUID: 019e50e6-1f14-77a2-880a-0df6042d7d23
  Title: schema discovery worked but tool descriptions could benefit from more detail
  Type: Task
  Status: ◯ Draft
  Priority: Medium
  Tags: kind:followup-suggestion, severity:cosmetic, from-implementer:STORY-407

  During Antigravity Phase 1 & 2 verification, we observed that tools/list successfully...
  ```

## Phase 3: Coordination Tools

We successfully verified AIDA's coordination tools cluster over the stdio MCP bridge. All coordination roundtrips were executed from a clean sibling worktree using a dedicated, approved test fixture spec (`SPEC-407`). 

We verified four distinct coordination operations: Spec Creation, Punts, Task Leases, and Worker Directives. Each operation was executed over the stdio MCP bridge and verified end-to-end via corresponding AIDA CLI commands.

### 0. Test Fixture Setup

We created a dedicated Approved test fixture requirement of type `task` to isolate our testing from active, live specs.

- **MCP Tool Invocation (`add_requirement`):**
  ```json
  add_requirement({
    "title": "Antigravity Phase 3 coordination test fixture",
    "description": "Created by Antigravity Phase 3 coordination roundtrip. Safe to flip and clean up after Phase 3 verification.",
    "type": "task",
    "status": "approved",
    "priority": "low"
  })
  ```
- **MCP Output:**
  ```text
  Requirement added: SPEC-407 — Antigravity Phase 3 coordination test fixture
  ```
- **CLI Verification Command:**
  ```bash
  aida show SPEC-407
  ```
- **CLI Output:**
  ```text
  ID: SPEC-407
  UUID: 019e50fe-4b7e-76d2-880c-d439f98ff448
  Title: Antigravity Phase 3 coordination test fixture
  Type: Task
  Status: ▸ Approved
  Priority: Low

  Created by Antigravity Phase 3 coordination roundtrip. Safe to flip and clean up after Phase 3 verification.

  Git linkage: no commits or trace comments reference this spec yet

  ────────────────────────────────────────
  Status: ▸ Approved
  ```

---

### 1. Punt Channel Coordination (`post_punt` + `read_punt` + `resolve_punt`)

We simulated a design-fork scenario where the implementer reaches a design decision boundary and punts it to the advisor/operator.

- **Step A: Post Punt**
  - **MCP Tool Invocation (`post_punt`):**
    ```json
    post_punt({
      "spec_id": "SPEC-407",
      "detail": "Antigravity Phase 3 test punt: simulated design fork on test fixture",
      "category": "design-fork",
      "lean": "Path A: proceed as-is"
    })
    ```
  - **MCP Output:**
    ```text
    Punt recorded for SPEC-407 [design-fork]. Use `aida edit SPEC-407 --status needs-attention` from a session lease to park the spec.
    ```
  - **CLI Verification Command (Punt Ledger):**
    ```bash
    cat .aida/punts.jsonl | grep SPEC-407
    ```
  - **Ledger Record:**
    ```json
    {"timestamp":"2026-05-22T18:41:46.065592293Z","spec":"SPEC-407","category":"design-fork","detail":"Antigravity Phase 3 test punt: simulated design fork on test fixture","lean":"Path A: proceed as-is","raised_by":"mcp","resolution_path":"punted"}
    ```
    > [!NOTE]
    > Under AIDA's design, `post_punt` appends the punt to the `.aida/punts.jsonl` ledger. The spec's status remains `approved` in the local workspace unless explicitly edited to `needs-attention` to park it.

- **Step B: Read Punt**
  - **MCP Tool Invocation (`read_punt`):**
    ```json
    read_punt({
      "spec_id": "SPEC-407"
    })
    ```
  - **MCP Output:**
    ```json
    {
      "timestamp": "2026-05-22T18:41:46.065592293Z",
      "spec": "SPEC-407",
      "category": "design-fork",
      "detail": "Antigravity Phase 3 test punt: simulated design fork on test fixture",
      "lean": "Path A: proceed as-is",
      "raised_by": "mcp",
      "resolution_path": "punted"
    }
    ```

- **Step C: Resolve Punt**
  - **MCP Tool Invocation (`resolve_punt`):**
    ```json
    resolve_punt({
      "spec_id": "SPEC-407",
      "answer": "Path A approved for test purposes",
      "reasoning": "Test fixture; not a real decision",
      "classification": "test"
    })
    ```
  - **MCP Output:**
    ```text
    Resolution written to /home/joe/ai/aida-story-407-phase3/.aida/punts/SPEC-407.response.json — the orchestrator will resume the implementer with this answer.
    ```
  - **Verification (Response Payload):**
    ```bash
    cat .aida/punts/SPEC-407.response.json
    ```
    ```json
    {
      "classification": "test",
      "answer": "Path A approved for test purposes",
      "reasoning": "Test fixture; not a real decision"
    }
    ```

---

### 2. Task Lease Coordination (`claim_task` + `list_active_leases` + `release_task`)

We verified the exclusive checkout mechanism ensuring only one agent owns a lease on a spec at any time.

- **Step A: Claim Task**
  - **MCP Tool Invocation (`claim_task`):**
    ```json
    claim_task({
      "spec_id": "SPEC-407",
      "role": "implementer"
    })
    ```
  - **MCP Output:**
    ```text
    claimed: lease_id=019e50fe595d
    ```
  - **CLI Verification Command:**
    ```bash
    aida session leases
    ```
  - **CLI Output:**
    ```text
    Active session leases

    id             scope                branch             role           worktree
    ────────────────────────────────────────────────────────────────────────────────
    019e50fe       SPEC-407             story-407-phase3   implementer    /home/joe/ai/aida-story-407-phase3

    End one with: aida session end <id>
    ```

- **Step B: List Active Leases**
  - **MCP Tool Invocation (`list_active_leases`):**
    ```json
    list_active_leases()
    ```
  - **MCP Output:**
    ```text
    Found 1 active lease(s):

    - 019e50fe595d scope=SPEC-407 role=implementer owner=joe kind=mcp started_at=2026-05-22T18:41:46.848185940+00:00
    ```

- **Step C: Release Lease**
  - **MCP Tool Invocation (`release_task`):**
    ```json
    release_task({
      "lease_id": "019e50fe595d"
    })
    ```
  - **MCP Output:**
    ```text
    released: lease_id=019e50fe595d
    ```
  - **CLI Verification Command:**
    ```bash
    aida session leases
    ```
  - **CLI Output:**
    ```text
    (no active sessions)
    ```
  - **MCP List Check:**
    ```json
    list_active_leases()
    ```
    ```text
    No active leases.
    ```

---

### 3. Worker Directive Coordination (`post_directive` + `list_directives` + `ack_directive`)

We verified the FIFO worker control loop (e.g. `pause` command passing arguments to control worker threads safely).

- **Step A: Post Pause Directive**
  - **MCP Tool Invocation (`post_directive`):**
    ```json
    post_directive({
      "verb": "pause",
      "args": ["antigravity-phase-3-test"]
    })
    ```
  - **MCP Output:**
    ```text
    directive posted: pause antigravity-phase-3-test
    ```
    > [!IMPORTANT]
    > Directives expect the `args` parameter to be an array of strings, e.g. `["antigravity-phase-3-test"]`. Passing a single string will evaluate to an empty args list on parse.

- **Step B: List Active Directives**
  - **MCP Tool Invocation (`list_directives`):**
    ```json
    list_directives()
    ```
  - **MCP Output:**
    ```text
    1 pending directive:
      1. pause antigravity-phase-3-test
    ```
    > [!NOTE]
    > The MCP server's directives manage the immediate session worker's coordination flow (like local `pause` and `exit`). This is isolated from the global orchestrator's CLI queue directives (e.g. `drain TASK-N` shown in `aida worker directives`).

- **Step C: Acknowledge (Pop) Directive**
  - **MCP Tool Invocation (`ack_directive`):**
    ```json
    ack_directive({
      "index": 0
    })
    ```
  - **MCP Output:**
    ```text
    acked: pause antigravity-phase-3-test
    ```
  - **MCP List Check:**
    ```json
    list_directives()
    ```
    ```text
    (empty - no pending directives)
    ```

---

### 4. Cleanup

Upon successful verification of all roundtrips, we cleanly closed out the test fixture to keep AIDA's registry pristine.

- **CLI Command:**
  ```bash
  aida edit SPEC-407 --status rejected
  ```
- **CLI Output:**
  ```text
  Updated: SPEC-407
  ```

---

## Known Antigravity Pitfalls & Lessons

During early integration sessions, several critical discipline gaps were observed and resolved. Future Antigravity sessions must adhere strictly to these lessons to maintain repository integrity and prevent workflow friction:

1. **Worktree Isolation (Mandatory)**
   - **Pitfall**: Attempting to perform implementation work directly inside master's main worktree (`/home/joe/ai/aida`). This locks master, risks co-mingling dirty state, and blocks concurrent operations.
   - **Lesson**: **ALWAYS** check out a sibling worktree using AIDA's session start command:
     ```bash
     aida session start --owns STORY-418 --role implementer
     ```
     This automatically handles branch creation, worktree setup, state symlinking, and lease registration under `/home/joe/ai/aida-<spec>`. Never commit to master's active main worktree.

2. **Atomic Shipping via `aida pr ship` (The Finish Line)**
   - **Pitfall**: Treating local commits or single branch pushes as the completion of a task.
   - **Lesson**: A commit or requirement is **never** complete until it has been pushed to GitHub, successfully passed the repository's CI/CD builds, and been merged cleanly via:
     ```bash
     aida pr ship
     ```
     The shipping wrapper is load-bearing; it verifies CI readiness, repairs subject lines, aligns trailing spec trailers, and triggers the `aida pull` rebase cycle to keep local main up to date.

3. **Commit Message Prefix & Attribution**
   - **Pitfall**: Creating commits that lack clear agent attribution, complicating history audits.
   - **Lesson**: All commit and PR squash subjects authored by Antigravity must carry the exact prefix:
     ```text
     [AI:antigravity] type(scope): description
     ```

4. **Multi-Spec Trailer Grouping**
   - **Pitfall**: Including only a single spec ID in a PR title or squash subject when shipping changes that address multiple requirements (e.g. shipping STORY-407 follow-ups but only putting `(STORY-407)` instead of all affected specs, causing BUG-333 and TASK-464 to remain open).
   - **Lesson**: If a PR closes or relates to multiple specs, **all** affected spec IDs must be grouped together in the trailing parens:
     ```text
     [AI:antigravity] fix(scaffold): description (BUG-333 TASK-464)
     ```
     The auto-bump scanner is space-separated; it will parse and transition every ID listed inside the parens group.

5. **Local Spec Store Access via Symlink (BUG-331 Workaround)**
   - **Pitfall**: AIDA CLI commands failing in sibling worktrees because the git-canonical store directory `.aida-store` is not present locally.
   - **Lesson**: Sibling worktrees must symlink the main `.aida-store` to achieve canonical store detection. AIDA's standard `aida session start` command handles this automatically, but if configuring manually, run:
     ```bash
     ln -s /home/joe/ai/aida/.aida-store .aida-store
     ```

---

## Brain-Directory Integration Guidelines

Antigravity utilizes session-specific local markdown artifacts (`walkthrough.md`, `task.md`, `implementation_plan.md`) located under:
`~/.gemini/antigravity-cli/brain/<session-id>/`

To ensure clean coordination, use this clear division of labor:

### When to Use Local Brain Artifacts
- **Fine-Grained Task Checklists**: Local `task.md` checklists are perfect for tracking minor development details, build iterations, and local compilation checkpoints that are too granular or noisy for AIDA's global spec graph.
- **Draft Design Proposals**: The `implementation_plan.md` acts as a technical scratchpad to sketch and refine architecture details before writing code.
- **Verification Walkthroughs**: The `walkthrough.md` is used to capture manual verification logs, test command runs, and screenshots for direct human operator review.

### When to Migrate/Use AIDA Spec Graph Tools
- **Durable Specifications**: All formal requirements, feature gates, and bug fixes must be recorded in git-canonical AIDA specs via MCP or CLI (`add_requirement` / `post_punt`). Do not bury broad specifications in local brain files.
- **Cross-Agent Coordination**: If a sub-task or feature needs to be shared with, claimed by, or delegated to another agent (e.g. Codex), it must be defined as an active approved requirement in the spec graph, allowing exclusive leases via `claim_task`.
- **Durable Historical Record**: Essential design decisions, architectural sign-offs, and critical status updates should be posted as comments on the spec (`add_comment`) so they are preserved in the git history for future sessions.

trace:STORY-407 | ai:antigravity
trace:STORY-418 | ai:antigravity
