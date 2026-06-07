# Deep-Dive Competitive Analysis: ECC vs AIDA (2026-06-05)

**Specs:** SPIKE-27, SPIKE-28, competitive-deep-dive · **Status:** snapshot (dated) · **Evidence:** Repository inspection of `affaan-m/ecc` (v2.0.0-rc.1)
**Inputs:** Independent codebase analysis of `affaan-m/ecc` + comparison against prior drafts.

---

## Executive Verdict

ECC (Everything Claude Code / `ecc-universal`) and AIDA operate in adjacent but structurally distinct categories. 

*   **ECC** is a **cross-harness agent operating pack and distribution platform**. It bundles a massive prompt/instruction catalog (skills, agents, rules), local event-hook runtime enforcement, and a newly introduced Rust-based local control plane (`ecc2/`) containing a TUI dashboard. Its design is optimization-centric and harness-agnostic (targeting Claude Code, Codex, OpenCode, Cursor, Gemini, Zed, etc.).
*   **AIDA** is a **git-canonical project-truth substrate**. It centers on a durable, version-controlled spec graph, ID-stable requirements, advisory leases, queue/orchestrator runtimes (drain loops), and strict code-to-spec trace enforcement.

### The Axis of Competition
They compete directly on **agent orchestration and workspace runtime** (ECC's `ecc2` local daemon vs. AIDA's orchestrator drain loop). ECC offers a richer, more polished operator GUI (OTEL tracing, risk profiling, conflict incidents, cron schedules) and a highly robust, manifest-driven installation mechanism. However, **AIDA maintains a structural moat in its git-canonical, multi-vendor requirement graph**. ECC's control plane relies on a local, ephemeral SQLite database (`~/.claude/ecc2.db`), meaning it lacks stable requirement IDs, code traces, and collaborative team-wide governance.

---

## Peeling Apart the Four Layers of ECC

Our independent analysis reveals that ECC is not a single unified runtime, but rather four distinct architectural layers stacked together:

### Layer 1: Prompt & Process Marketplace (The Catalog)
*   **What it is:** A collection of 251 skills (prompt markdown in `skills/*/SKILL.md`), 63 agents (`agents/`), 79 commands (`commands/`), and 115 rules (`rules/`).
*   **Architecture:** ~90% of this catalog is pure instructions/prompt-text (e.g., `skills/search-first/SKILL.md`) designed to teach the LLM how to execute a workflow. It is substrate-unaware and does not execute active engine code (with some exceptions like `continuous-learning-v2` or `videodb` which package Node/Python shims).
*   **Observations:** The metadata count is prone to drifting (e.g., discrepancies between the counts listed in `SOUL.md` and the actual filesystem structure), requiring a static `catalog:check` validator script to enforce consistency in CI.

### Layer 2: Distribution Engine (The Installer)
*   **What it is:** A manifest-driven installer utilizing `install-plan.js` and `install-apply.js` to enable selective profiles (`minimal`, `core`, `developer`, `security`, `research`, `full`) and granular component installs.
*   **Architecture:** It validates the install plan using JSON schemas (`install-modules.schema.json`, `install-profiles.schema.json`, etc.), writes a state file (`ecc-install-state.json`), and enables a clean uninstall flow that only touches recorded files.
*   **CI Validation:** The `validate-install-manifests.js` script enforces that (a) every path declared in the manifests exists on disk, and (b) no file path is claimed by more than one module. This completely eliminates file-drift and collision risks during packaging.

### Layer 3: Hook Enforcement Runtime
*   **What it is:** Trigger-based automations hooked into Claude Code's native `PreToolUse`, `PostToolUse`, `PreCompact`, and `SessionStart` events via `hooks/hooks.json`.
*   **Architecture:** It executes Node.js dispatchers (e.g., `pre-bash-dispatcher.js`, `suggest-compact.js`, `mcp-health-check.js`, `gateguard-fact-force.js`) to enforce formatting gates, prevent config weakening, verify MCP server health, and manage context window compaction.
*   **Observations:** Highly production-grade dev-hygiene, but it operates as a local lint/check framework rather than enforcing spec-based requirement lifecycles.

### Layer 4: `ecc2/` Rust Control Plane (The Orchestrator)
*   **What it is:** A Rust-based daemon/TUI application (`ecc-tui` utilizing `ratatui`, `tokio`, `rusqlite`, and `git2`).
*   **Architecture:** It manages Git worktrees, sessions, daemonheartbeats, cron scheduling, and auto-merging of clean worktrees.
*   **Durable State:** Unlike AIDA, its state store resides in a local SQLite file (`~/.claude/ecc2.db`). If this file is deleted, the session histories, work-item links, and metrics are permanently lost.
*   **Orchestration Limits:** "Remote dispatch" is actually a local re-spawn command (`current_exe()`). There is no collaborative, cross-machine state.

---

## Competitive Map

| Vector | ECC (`ecc-universal` / `ecc2`) | AIDA | Comparison Verdict |
| :--- | :--- | :--- | :--- |
| **Category** | Cross-harness operating pack + marketplace with local session orchestration. | Project-truth substrate with speculative execution orchestrator. | Complementary packages that compete on the local session/orchestration plane. |
| **Truth Moat** | Local SQLite database (`~/.claude/ecc2.db`). Ephemeral and local-only. | Git-canonical orphan branch (`aida-store`). Rebuildable local SQLite cache. | **AIDA Moat:** Git-canonical state survives machine resets, vendor changes, and is team-collaborative. |
| **Identities (IDs)** | Ephemeral session UUIDs; free-text external work-item keys. | Stable cross-cutting specification IDs (`TASK-N`, `BUG-N`) surviving merges. | **AIDA Moat:** Stable IDs enable long-term traceability and relation graphs. |
| **Traceability** | None (free-text decision logs). | Source code trace comments (`// trace:SPEC-ID`) + commit-trailer auto-bump. | **AIDA Moat:** Strict code-to-spec trace loops protect against drift. |
| **Lifecycle** | Ephemeral process states + free-string Kanban lanes. | Merge-anchored requirement lifecycle (`Draft` -> `Approved` -> `Completed` -> `Released`). | **AIDA Moat:** Spec graph status tied to git history. |
| **Distribution** | Manifest-driven packaging (`install-modules.json`), schemas, dry-runs, CI validator. | Simple `aida init` with manual symlinking and copy rules. | **ECC Lead:** Highly mature, multi-harness installer and dry-run planner. |
| **Orchestration** | Ratatui TUI dashboard, Git worktrees, auto-merging, cron, OTEL spans. | Advisory lease system, drain queue CLI, PR-anchored phases, TUI. | **Mixed:** ECC has richer telemetry and dashboard features; AIDA has spec-grounded governance. |

---

## Synthesis of Prior Analyses & Critique

We compared our findings against the two provided drafts:
1.  **Draft 1 (General Comparison)** is directionally correct in highlighting the contrast between ECC's operating pack model and AIDA's project-truth substrate. It correctly identifies the manifest-driven installer and `ecc2` as areas of interest.
2.  **Draft 2 (The Four-Layer Refinement)** is significantly sharper. It accurately calls out the "substance problem" of the prompt catalog (~90% prompt bulk that drifts), exposes the local SQLite limitation of `ecc2.db`, and details the harness adapters (native vs. instruction-backed). It also correctly frames the competitive asymmetry: AIDA can close the distribution gap easily, whereas ECC cannot easily adopt a git-canonical specs moat without re-architecting its core database.

### Pushbacks and Refinements
*   **The Telemetry and TUI Gap:** Draft 2 suggests grafting specific features (OTEL spans, risk scoring) onto AIDA's existing TUI (`EPIC-26`) rather than building a thin dashboard. This is correct. AIDA should keep the TUI centered on the spec graph rather than creating a separate local-only session tracker.
*   **Ecosystem Breadth:** Draft 1 credits ECC with broad multi-harness distribution (Cursor, Zed, Gemini, Qwen). However, Draft 2 is correct that only Claude Code and OpenCode have native/adapter runtime hook enforcement; the others are mostly instruction-backed or reference-only copies of rules. AIDA should adopt the manifest system, but limit its native adapter profiles to high-value harnesses rather than attempting to copy the broad, instruction-backed list.
*   **The Distribution Threat:** While AIDA has the architectural moat, ECC has immense distribution reach via its 182k-star lineage and `npx` install. A weak-truth orchestrator with superior distribution can still capture mindshare. AIDA's distribution gap is not cosmetic; it is the channel through which our truth-moat becomes visible.

---

## Action Plan for AIDA

We recommend filing a high-priority AIDA Epic: **"Manifest-Driven Multi-Harness Distribution."** Rather than trying to package 250 prompt skills, AIDA should focus on packaging its MCP setup, `AGENTS.md`, and project-level templates.

The implementation should be divided into the following sequential slices:

### Slice 1: The CI Manifest Path & Ownership Validator (Highest Leverage)
*   **Goal:** Eliminate drift and collision risks in AIDA's dual-copy template system (symlinks + `build.rs` embedding).
*   **Action:** Write a Cargo test or CI script (e.g., `tests/test_manifest_conformance.rs`) asserting that every template file path declared in AIDA's distribution manifests exists on disk and is uniquely owned.

### Slice 2: Install-State Record & Clean Uninstall
*   **Goal:** Keep track of files written during `aida init` to allow safe diff-based refreshes and clean removals.
*   **Action:** Generate an `aida-install-state.json` recording checksums of written files, allowing `aida init --refresh` and `aida init --uninstall`.

### Slice 3: Honesty-Graded Harness Adapter Matrix
*   **Goal:** Document and grade AIDA's support across harnesses (Native vs. Adapter vs. Instruction-backed) under `docs/positioning/` to manage user expectations.

### Slice 4: Telemetry Features in the AIDA TUI
*   **Goal:** Absorb `ecc2`'s TUI telemetry advantages.
*   **Action:** Graft OTEL tracing support, conflict warnings, and active queue telemetry directly onto AIDA's existing spec-graph TUI (`EPIC-26`).

---

## References

*   **ECC Repository:** [affaan-m/ecc](https://github.com/affaan-m/ecc)
*   **ECC Manifests:** [install-profiles.json](file:///home/joe/ai/aida/ecc-clone/manifests/install-profiles.json), [install-modules.json](file:///home/joe/ai/aida/ecc-clone/manifests/install-modules.json), [install-components.json](file:///home/joe/ai/aida/ecc-clone/manifests/install-components.json)
*   **ECC CI Validator:** [validate-install-manifests.js](file:///home/joe/ai/aida/ecc-clone/scripts/ci/validate-install-manifests.js)
*   **ECC Control Plane:** [ecc2/src/main.rs](file:///home/joe/ai/aida/ecc-clone/ecc2/src/main.rs), [store.rs](file:///home/joe/ai/aida/ecc-clone/ecc2/src/session/store.rs), [Cargo.toml](file:///home/joe/ai/aida/ecc-clone/ecc2/Cargo.toml)
*   **ECC Harness Adapter Matrix:** [cross-harness.md](file:///home/joe/ai/aida/ecc-clone/docs/architecture/cross-harness.md), [harness-adapter-compliance.md](file:///home/joe/ai/aida/ecc-clone/docs/architecture/harness-adapter-compliance.md)
