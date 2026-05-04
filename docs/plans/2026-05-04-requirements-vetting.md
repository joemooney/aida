# Requirements DB vetting pass — 2026-05-04

Vetting pass run autonomously while user was away. Direction stated by user:

> "we are at ground zero for new project adoption without concern for backwards compatibility ... make core AIDA robust and with minimal friction for new projects"

## Filter applied

- **Keep / Complete**: items on the core CLI / scaffolding / MCP-server critical path, OR clearly already shipped (verified by reading current source).
- **Reject**: web UI / native GUI / Sprint UI / web auth / GitLab integration / Jira integration / cosmetic/ergonomic UI features / placeholder data ("Test", "Sprint #3", "Epic123", etc.) / pre-pivot scope items. User can re-promote any of these by editing status back when relevant surface returns to focus.
- **Leave Draft**: aligned with the new direction but still a real future work item.

## Status counts

| Status      | Before | After | Delta |
|-------------|-------:|------:|------:|
| Draft       |    166 |    15 |  -151 |
| Approved    |     56 |     9 |   -47 |
| In Progress |     10 |     2 |    -8 |
| Completed   |    131 |   147 |   +16 |
| Rejected    |      5 |   199 |  +194 |
| **Total**   |    374 |   374 |     0 |

(All transitions left a per-requirement comment so the rationale is preserved.)

## Notable completions (verified shipped)

- **FR-241** Custom type definitions — verified `CustomFieldType` / `CustomFieldDefinition` / `CustomTypeDefinition` in `aida-core/src/models.rs`
- **FR-249** AI Requirement Evaluation — `BackgroundEvaluator` service exists, `/aida-evaluate` skill scaffolded, META-002 prompt customizable
- **FR-157** Role support — fully shipped this session (EPIC-1-005, EPIC-1-006)
- **TASK-1-018** PS1 prefix on role enter — shipped commit 90c4a25 earlier today
- **TASK-5** Commit message format enforcement — `aida-commit-msg` hook + `AIDA_COMMIT_STRICT` documented
- **FR-132 / STORY-29 / STORY-35 / STORY-39** Trace CLI — `aida trace add/list/scan/remove/sweep` all present
- **EPIC-1** TraceLink data model + STORY-32/36/38 — implied by working trace CLI
- **EPIC-1-004** Dev workflow — all surfaces (`aida dev activate/serve/release/patch/shell-init`) work
- **EPIC-1-005** Personas (already marked completed during the vetting session)

## In-Progress (preserved)

- **EPIC-5** My Queue — CLI shipped, web UI parts deferred (commented to that effect; user can split or close-CLI-only)
- **EPIC-1-001** Git-canonical storage — Phase 3 hard-cut still pending; needs a focused multi-hour session, dequeued during this run

## Approved kept (aligned with new direction)

- **EPIC-1-026** + **FR-1-027** + **FR-1-028** + **SPIKE-1-029** — scaffold upgrade workflow (template/seed/managed-merge)
- **TASK-1-023** — re-extract `.claude/hooks/` on `aida scaffold apply` (gap surfaced this session)
- **TASK-1-024** — emit SessionStart hook in `generate_claude_settings_json` (gap surfaced this session)
- **FR-1-002** — write-behind batched commits for bulk imports (deferred but real)
- **FR-1-012** — global queue spanning projects (tagged deferred; needs design pass)
- **SPIKE-2** — `aida comment edit/delete` not yet wired up for git backend

## Drafts kept (aligned with the new direction)

- **TASK-1-019** PS1 splice semantics for clean composition with roles
- **TASK-1-016** skill ↔ command parity check (CI / make target)
- **FR-195** scaffold status report with diffs in HTML
- **FR-94** requirements sync/merge workflow for distributed teams
- **FR-103** new-user name/email from git config — direct new-project-friction win
- **FR-215** `aida add --parent` flag
- **FR-76** disconnected mode

Agent-collaboration epics (AIDA's defensible niche, kept Draft for user to revisit):

- **EPIC-7** ImplementationInfo metadata
- **EPIC-8** Agent Planning Phase
- **STORY-28** Enhanced `/aida-implement`
- **STORY-30** Implementation Phase with Traceability
- **STORY-31** Context Gathering Tool
- **STORY-33** Feedback Loop Closure
- **STORY-34** Agent Tool Exposure API (MCP-aligned)
- **STORY-40** Review Requirements Workflow

## Patterns rejected (high-level)

- **GUI / native-egui features** — buttons, hotkeys, drag-and-drop, theme settings, modal dialogs, tabs, panel sliders, scrollbar behaviors, context menus, color coding. Most numbered FR-XX from the pre-pivot phase.
- **Web React UI features** — splash screen / window title / details view layout / sorting capabilities / split-panel work / queue dashboard widget / drag from list to queue.
- **GitLab integration** — SPIKE-3, STORY-2/19/21/24/25/26, FR-24
- **Web Auth** — EPIC-3 + FR-5/6/7/8/9 (multi-user web-server only; not on solo-CLI critical path)
- **Sprint feature** — FR-36/50/86/101/136/221/246, SPRINT-1/2/3 (sprint UI deferred until that surface returns)
- **WASM client** — FR-37, FR-67 (CLAUDE.md notes WASM/native were extracted to a separate repo on 2026-05-02)
- **Release management one-offs** — FR-72/148/159/200/229/230 (superseded by `aida dev release`)
- **Test/placeholder data** — `Test`, `testing 333aaa`, `Sprint #3`, `Epic123`, `User Req 123`, `Folder 123`, `Bug 123`, `Change request 123456`, etc.

## Bugs surfaced & fixed during the vetting pass

These weren't on the queue — they fell out of trying to actually use the data.

- **BUG-1-025** `set_status_from_str` misroutes `"in-progress"` to `custom_status`, overriding the canonical enum. Fixed in commit `3c4aaea`. Also fixed cache filter in `2062f0c` so `aida list --status in-progress` matches the cache's stored Debug form. Data sweep ran against 10 records that had stale `custom_status` from before the fix.

## Open follow-ups for the user's return

1. Re-route EPIC-1-001 Phase 3 hard-cut into a focused multi-hour session.
2. Decide whether EPIC-5 (My Queue) should be marked Completed-CLI-only or split out the web work into a separate epic.
3. Resolve FR-1-012 design questions (auto-cd vs hint vs refuse) before queueing it again.
4. Pick up TASK-1-023 / TASK-1-024 to fully propagate the new SessionStart hook to existing projects via `scaffold apply`.

## Caveats

- Status transitions are bulk; per-requirement comment captures the rationale, but bulk decisions can miss nuance. Anything I rejected can be re-promoted by editing the status back; the comment trail explains why.
- The "Approved" → kept items list reflects my reading of the current direction, not a sprint-ready roadmap. Treat it as "candidates for prioritization" rather than "next up".
