# EPIC-53 cockpit ↔ EPIC-54 redesign — implementation seam

Date: 2026-06-26 · Specs: EPIC-53 (content), EPIC-54 (shell) · Status: prep (implement after EPIC-54 shell stabilizes)

Prep pass so the two epics don't build the same gestures twice. EPIC-53 and EPIC-54 live in the same files (`board.rs`, `app.rs`, `launcher.rs`); EPIC-54 is being actively rewritten by a separate agent (PR #1145). This defines the boundary + each child's acceptance so EPIC-53 can be driven cleanly once the shell lands.

## The boundary

- **EPIC-53 owns the CONTENT layer**: what surfaces (reasons, owners, mail), how items classify, what actions each row offers, the underlying data sources. Pure, shell-independent, unit-testable.
- **EPIC-54 owns the INTERACTION SHELL**: row selection, multi-select, the lens/toggle, panel placement, the action→target gesture grammar, dispatch.

## The contract (the seam)

A cockpit **row/item** exposes: `{ classification: Reason, owner: Reason::owner(), actions: Vec<{label, action_fn}> }`.

- EPIC-53 builds this data/action layer — **testable without the shell**.
- EPIC-54's shell consumes it: renders rows, provides selection / multi-select / lens gestures, dispatches the chosen action.

**Rule:** if a child's work is a *gesture* (select, multi-select, toggle, navigate), it belongs to EPIC-54 — EPIC-53 only provides the *data* the gesture acts on.

Grounding (current `board.rs`, will move under the redesign but the seam holds): `enum Reason` (29), `Reason::owner()` (75 — already exists), `classify()` (164), `is_advisor_backlog()` (305); mail: `unread_inbox`/`inbox_for` (`mailbox.rs`), `read_local_messages` (`mailbox_store`).

## Per-child split + build order

1. **STORY-703 — advisor-opacity panel** (SAFEST FIRST: most content, least gesture).
   - EPIC-53: enrich `is_advisor_backlog` items with each park reason (revisit trigger / finding / punt note) + total advisor-queue depth. A pure data projection + a render.
   - Needs from EPIC-54: a content-panel slot. Minimal shell coupling — buildable nearly standalone.

2. **STORY-701 — mailbox group + send-mail**.
   - EPIC-53: a Mail content source (`unread_inbox` → rows, owner=you, unread count) + a send action (`send_message`). A `Reason::Mail` variant.
   - Needs from EPIC-54: action-registration — a row exposes a reply/send `action_fn` the shell dispatches in the action→target model.

3. **STORY-702 — who-must-act lens** (GESTURE — EPIC-54 absorbs).
   - EPIC-53: ensure `Reason::owner()` covers the full owner set + the "you" aggregation (needs-approval + needs-answer + mail). Classification only.
   - EPIC-54: the lens/toggle gesture that re-groups by owner. **Do NOT build the toggle in EPIC-53.**

4. **TASK-937 — batch-approve** (GESTURE — EPIC-54 absorbs).
   - EPIC-53: the approvable-set (drafts + intake proposals) + a batch-approve action (approve+queue N specs in one call).
   - EPIC-54: the multi-select gesture. **Do NOT build multi-select UI in EPIC-53.**

## Sequencing

Implement after EPIC-54's redesign shell stabilizes (PR #1145+). The data-layer pieces (STORY-703's enrichment, STORY-701's sources, STORY-702's owner-classification, TASK-937's approvable-set + batch action) can be built slightly ahead **as pure functions** (testable without the shell); the rendering/gesture wiring waits for the shell. Coordinate STORY-702 + TASK-937 with the EPIC-54 agent so the gestures aren't built twice.
