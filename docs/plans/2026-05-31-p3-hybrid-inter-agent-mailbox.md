# Implementation plan — P3: hybrid inter-agent mailbox

**Date:** 2026-05-31 · **Specs:** STORY-493 (SPIKE-45 P3) · **Status:** Sketch — operator chose the hybrid shape (2026-05-31); needs sign-off on the digest-trigger + consistency model before the wiring slices · **Complexity:** Large

> Agent↔agent peer messaging — "A found X, tells B"; "B asks A to check Y" — that AIDA lacks (briefs are operator→agent work-pickup; directives are top-down control; neither is peer↔peer, and both are `.aida/`-local/ephemeral). Operator picked the **hybrid** shape: a fast `.aida/`-local live layer for exchange **plus** a git-canonical durable digest on the orphan store for replay/audit/cross-clone sharing — the round-2 differentiator competitors' ephemeral local mailboxes lack. Mirrors AIDA's own "git-canonical writer-of-record + cache/handshake for speed" architecture.

## The hybrid model (the core decision to sign off)

```
  send_message ─▶ .aida/mailbox/  (LOCAL, fast, immediate)   ◀─ read_inbox merges
                       │                                          local + canonical
                       │  digest (explicit `aida mailbox sync`,
                       ▼  and on `session end` / drain boundaries)
                  aida-store orphan branch  (CANONICAL: durable,
                  objects/MESSAGE/...        replayable, shareable
                                             across clones/vendors)
```

- **Local is the live truth** for fast exchange (no commit-per-message latency — the round-1 reason briefs/directives are `.aida/`-local).
- **Canonical is the durable record** — messages digested to the orphan store become replayable, auditable, and visible to *another clone's* agents (the differentiator).
- **Reads reconcile both**: `read_inbox` = canonical history ∪ not-yet-digested local, deduped by message id.

**Sign-off points (the consistency model):**
1. **Digest trigger** — explicit `aida mailbox sync` + automatic on `session end` and drain phase-boundaries (NOT per-message; that's the coarse-git trap). Is that the right cadence?
2. **Conflict/ordering** — messages are append-only + id-keyed (HLC or uuid7 timestamp), so two agents digesting concurrently merge cleanly (no edit conflicts). Confirm append-only (no edits/deletes in v1).
3. **Local-only fallback** — if the store leg is unavailable, local still works; digest catches up later (matches `aida pull`'s two-leg resilience).

## Approach (build order — pure core first, like P1/graph-query)

Each slice is its own verifiable PR; the impure store/git wiring lands after the pure core is proven.

1. **Pure message model + inbox/thread logic** (`aida-core/src/mailbox.rs`, NEW): `Message { id, thread_id, from, to (Agent|Broadcast), timestamp, in_reply_to, body }` + pure functions: `inbox_for(agent, &[Message]) -> Vec<&Message>`, `thread(thread_id, &[Message]) -> Vec<&Message>` (ordered), `merge_dedup(local, canonical) -> Vec<Message>` (id-keyed union). Side-effect-free, exhaustively unit-tested. **Safe to build at any time.**
2. **Local store** (`.aida/mailbox/`): read/write the live layer (markdown-or-toml-per-message under `.aida/mailbox/`, mirroring the brief channel's file conventions + the deny-by-default gitignore). `aida mailbox send/inbox/thread` CLI over the pure core.
3. **MCP surface**: `send_message({to, body, thread?, in_reply_to?})` + `read_inbox({agent, thread?})` — contract addition, sketch-first per the one-master-advisor discipline; mirrors the brief MCP tools.
4. **Canonical digest** (the hybrid's defining slice): `aida mailbox sync` writes not-yet-digested local messages as orphan-store objects (reuse the object_store/git_backend write path used for specs); reads merge canonical + local. Auto-trigger on `session end` + drain boundaries.

## Key decisions

- **Append-only, id-keyed.** No edits/deletes in v1 → concurrent digests merge without conflict (matches the spec store's HLC/dispenser model). Use uuid7 (time-ordered) or HLC for ids.
- **`to` = a specific agent OR broadcast.** Broadcast lands in every agent's inbox view (computed, not copied).
- **Reuse, don't reinvent:** the brief channel's file/gitignore conventions for the local layer; the object_store + git_backend write path for the canonical layer; the MCP tool-descriptor + dispatch pattern (as `query_graph` did).
- **NOT per-message commits.** Digest batches — the local layer absorbs the hot loop; git stays the durable record. This is the whole point of "hybrid."

## Critical files

- `aida-cli/src/mcp.rs` — brief MCP tools (`list_briefs`/`read_brief`) as the surface pattern; tool-descriptor + dispatch.
- `aida-cli/src/main.rs` — brief CLI (`brief_dir`, the `.aida/agent-briefs` conventions) for the local-layer pattern; `session end` for the auto-digest hook.
- `aida-core/src/object_store.rs`, `db/git_backend.rs` — the orphan-store object write path to reuse for the canonical digest.
- `.aida/` gitignore deny-by-default block — add the `.aida/mailbox/` runtime convention.

## Reusable helpers (don't reimplement)

- Brief file enumeration / ack pattern (`.aida/agent-briefs/<agent>/`) → `.aida/mailbox/`.
- `current_user_id()` / agent identity resolution for `from`.
- HLC / dispenser (`aida-core/src/hlc.rs`, `dispenser.rs`) for ordered message ids.
- The cached_git_backend write-through pattern (local-then-canonical mirrors it conceptually).

## Risks + gotchas

- **Don't recreate the coarse-git trap.** Per-message commits would reproduce exactly what round-1 says git is bad at. Digest must batch.
- **Surface proliferation / slop.** This is the 4th `.aida/` channel (briefs, directives, escalation, now mailbox). Keep its semantics sharply distinct (peer↔peer conversation) and cross-link the docs so it doesn't blur with briefs (work-pickup) or directives (control).
- **Premature-need caveat (operator override noted).** With one master advisor, peer comms is light today; building it now is deliberately ahead of the SPIKE-10 multi-advisor need, on the operator's call, to have the git-canonical substrate ready.
- **Digest/read consistency** — a message sent locally but not yet digested must still appear in `read_inbox` (the merge handles this); a digest that partially fails must be resumable (idempotent, id-keyed).

## Tests (named)

- `mailbox::inbox_for_returns_messages_to_agent_and_broadcasts`
- `mailbox::thread_orders_by_timestamp_and_reply_chain`
- `mailbox::merge_dedup_unions_local_and_canonical_by_id` (a message in both appears once)
- `mailbox::broadcast_appears_in_every_inbox`
- CLI/MCP: send→inbox round-trip; MCP `send_message`/`read_inbox` parity (in-suite, like the query_graph test).
- Digest: local message → `mailbox sync` → present in canonical → visible after a simulated fresh-clone read.

## Verification

```bash
cargo test -p aida-core mailbox
cargo test -p aida-cli --bin aida mailbox
cargo build -p aida-cli && cargo fmt --all -- --check && bash tests/test_mcp_doc_consistency.sh
# Manual hybrid demo: send locally, read (sees it), sync, inspect orphan store, read from a second clone.
```

## Followups

- Read-receipts / ack (mirror `ack_brief`) if needed.
- Retention/compaction of the canonical message log.
- Subsystem-advisor routing once SPIKE-10 lands (the real consumer).

## Related

- Parent: SPIKE-45 (P3). Siblings: STORY-489 (P2 graph-query, shipped), STORY-490/492 (P5/P1).
- Architecture: round-1 thesis (`docs/competitive-analysis/2026-05-31-git-canonical-substrate-thesis.md`) — git-canonical-writer + cache/handshake, the model this mirrors; round-2 (`...round2-moat-gaps-moves.md`) — P3 = "git-canonical + replayable inter-agent comms" differentiator.
- Adjacent: SPIKE-10 (multi-advisor — the eventual consumer); the brief + directive channels (distinct semantics to preserve).
