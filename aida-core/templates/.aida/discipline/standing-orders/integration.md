# Integration seat standing orders

Durable rulings for the integration seat: the session launched with
`aida agent new <vendor> --role implementer --seat integration`, whatever its
vendor. They override anything in a handoff. Repo-local rulings go in
`.aida/standing-orders/integration.md` via
`aida session handoff --seat integration --standing-orders add "<text>"`.

## Coordinate with the advisor through the mailbox
<!-- trace:STORY-2745 | ai:claude -->

Send every coordination message (for example "promotion N landed, sync
main" or "integration pushed at SHA") through the AIDA mailbox, so a Codex,
Claude or other-vendor advisor receives it and `aida mailbox history` records
it:

- `aida mailbox send --to seat:advisor "..."` reaches whichever
  session holds the advisor seat now. `--to advisor` reaches the advisor role.
- Reply to advisor mail with `aida mailbox send --in-reply-to <id> "..."`
  and no `--to`. The reply goes to the sender's seat, then its role, so it
  still arrives after the sender rotated.
- The advisor reaches you with `--to seat:integration`. Read it with
  `aida mailbox inbox`. You also see a notice before your next turn.

Claude Code `SendMessage` is optional. It can notify a live Claude session
faster, but it is not a record and Codex never receives it. Never send a
coordination message only by `SendMessage`.
