# STORY-585 — Surface unread mailbox into the agent's session context (the read/notice half)

- **Date:** 2026-06-13
- **Specs:** STORY-585 (notice half); child TASK-782 (interpret half — deferred, separate PR)
- **Status:** In progress
- **Complexity:** Medium

## Approach

The mailbox WRITE side works (`send_message`); the gap is that an interactive
agent never *notices* unread mail unless it manually runs `aida mailbox inbox`.
Close the loop with a **substrate-as-bouncer** surface: a hook injects a capped,
role-scoped unread summary into the agent's context every turn, self-clearing
once the agent explicitly reads/acks.

```
send_message ──► .aida/mailbox/*.json ──► [merge w/ canonical] ──► inbox_for(identity)
                                                  │
                       watermark (.aida/mailbox/.read/<id>.txt)
                                                  │
                        unread = msgs past watermark (pure core)
                                                  │
        ┌─────────────────────────┬──────────────┴───────────────┐
        ▼                         ▼                               ▼
 aida mailbox notice      aida mailbox inbox --peek        statusline ⚠ mail
 (capped, plain,          (deliberate view, non-marking)   (same identity set)
  role+user scoped)              │
        │                        └── /aida-read-mail skill (show → offer ack)
        ▼
 SessionStart + UserPromptSubmit hooks (relay stdout → additionalContext)
        │
 explicit ack = plain `aida mailbox inbox` (advances watermark → notice clears)
```

Reading stays **explicit**: the hook/notice/peek never advance the watermark;
only a plain `aida mailbox inbox` (or MCP `read_inbox` with `mark_seen:true`)
consumes. This is acceptance #4 — a glance must not silently consume.

## Decisions

- **Identity = union of {shell user, AIDA_SESSION_ROLE}** (acceptance #5). The
  statusline today only resolves `current_user_id` (shell user), so role-addressed
  mail (e.g. a `--to advisor` handoff) is invisible to it. A shared
  `inbox_identities()` helper fixes both surfaces so they genuinely agree.
- **Two read shapes, not one.** `mailbox notice` = ambient, capped, plain, the
  hook's purpose-built verb (the "slice verb" the hook *calls*, not reimplements —
  TASK-736 discipline). `inbox --peek/--unread` = deliberate, uncapped, for the
  skill + acceptance #1.
- **Plain stdout for hooks** (verified contract): both SessionStart and
  UserPromptSubmit add plain stdout to context, no JSON envelope needed. So the
  hook is a 3-line wrapper around `aida mailbox notice` — robust under /bin/sh.
- **Self-clearing nag.** The notice shows only UNREAD (past watermark); once the
  agent acks, it goes quiet. Per-turn re-injection is the intended bouncer.
- **Interpret half deferred to TASK-782** (operator decision): intent markers +
  act-vs-prompt policy are a schema change + autonomy wiring — separate review.

## Files (build order)

1. `aida-core/src/mailbox.rs` — `unread_inbox()` (filter to past-watermark) +
   `NoticeSummary` builder (pure: identities + messages + watermarks → capped
   summary). Unit-tested.
2. `aida-cli/src/cli.rs` — `Inbox { peek, unread }` flags; new `Notice` variant.
3. `aida-cli/src/main.rs` — inbox handler honors `--peek`/`--unread`; new
   `Notice` handler; `inbox_identities()` helper; route statusline urgent count
   through it.
4. `aida-cli/src/mcp.rs` — `read_inbox`: add `unread` filter + `mark_seen`
   (default false); mirror schema.
5. `aida-core/templates/hooks/aida-mail-notice.sh` — thin wrapper; symlink into
   `.claude/hooks/`.
6. `aida-core/templates/settings.json` — wire the hook into SessionStart +
   UserPromptSubmit.
7. `aida-core/templates/skills/aida-read-mail.md` + `commands/aida-read-mail.md`
   — on-demand read/ack skill; `make sync-templates` symlinks.
8. `docs/mailbox.md` — add the read/notice section.

## Critical files

- `aida-core/src/mailbox.rs` — pure model + `unread_counts`/`inbox_for` reused.
- `aida-cli/src/mailbox_store.rs` — `read_watermark`/`set_watermark` reused as-is.
- `aida-cli/src/main.rs::handle_statusline_command` — `read_urgent_unread_count`
  is the identity precedent to align.

## Reusable helpers (don't reimplement)

- `aida_core::mailbox::{inbox_for, unread_counts, merge_dedup}` — read logic.
- `mailbox_store::{read_local_messages, read_canonical_messages, read_watermark}`.
- `current_user_id(None)` — shell identity (BUG-89 resolution order).
- `resolve_effective_role()` — role resolution the statusline uses.

## Risks + gotchas

- Hooks run under /bin/sh (dash) — no bashisms (memory: statusline-posix). Keep
  the wrapper trivial; let the Rust verb do the work.
- `.claude/hooks/` is NOT touched by `make sync-templates` — symlink by hand.
- UserPromptSubmit fires every turn — keep the verb fast and silent-when-empty.
- Don't advance the watermark from notice/peek/hook (acceptance #4).

## Tests

- core: `unread_inbox_filters_by_watermark`, `notice_summary_caps_and_counts`,
  `notice_dedups_identities`.
- cli: peek does not advance the watermark; `--unread` filters.
- mcp: `read_inbox` `unread` filter; `mark_seen:true` advances watermark.

## Verification

```
cargo test -p aida-core mailbox
cargo test -p aida-cli mailbox
cargo build --release
aida-on
aida mailbox send "probe" --broadcast --from someoneelse
aida mailbox notice            # shows unread, does not mark
aida mailbox inbox --peek      # shows, does not mark
aida mailbox notice            # STILL shows (peek didn't consume)
aida mailbox inbox             # reads + marks
aida mailbox notice            # now silent
bash tests/test_mcp_stdio.sh   # MCP parity
```

## Followups

- TASK-782 — intent markers + act-vs-prompt policy (the interpret half).

## Related

- STORY-569 (write half / --zen handoff), STORY-583 (retract), STORY-539
  (statusline urgent counter), TASK-736 (skill↔CLI symmetry), STORY-82 (CLI↔MCP
  mirror), `[[feedback_substrate_as_bouncer_not_rules]]`.
