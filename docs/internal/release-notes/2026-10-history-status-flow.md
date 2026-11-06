# `aida history` shows the status flow by default

Bare `aida history` now lists status changes, newest first, one line each:
`WHEN  ID  from → to  TITLE`. A spec filed in the window shows
`(new) → <status>`. The window flags (`--since`, `--until`, `--limit`,
`--author`, `--type`, `--max-commits`, `--to`, `--from`, `--opened`, `--all`)
narrow it as they do for `aida history events`.

The previous default, the per-spec digest, is now `aida history --digest`.
Its human and TOON output is unchanged byte for byte. `aida history <SPEC-ID>`,
`aida history events` and the MCP `history` tool are unchanged.

## The status flow comes from the history index

The status flow reads only the history index (`aida cache rebuild --history`).
It never reads the full git log, so it stays fast on a large store.

- If the index is a few commits behind the store (at most 16, the usual state
  after a few `aida edit` or `aida add` commits), bare `aida history` catches
  those commits up first and then shows the flow. This takes about as long as
  the digest.
- If the index is further behind, bare `aida history` shows the digest and
  prints a one-line `note:` on stderr. The note names
  `aida history events --limit 1`, which catches the index up in steps. Run
  it again if the note is still there.
- If the index is switched off (`AIDA_HISTORY_CACHE=0`) or not built yet, or
  it can't answer for another reason, you see the digest with a note naming
  `aida cache rebuild --history`. Bare `aida history` never builds the index.

The history index format changed, so the index is rebuilt once after you
upgrade. Until `aida cache rebuild --history` or an event view such as
`aida history events` builds it, bare `aida history` shows the digest with a
note.

## JSON output changed (intentional)

- `aida history --json` (no SPEC-ID) now prints the status flow:
  `{view: "history-flow", count, source, index_tip, window_exhausted,
  transitions: [{when, id, from, to, title, kind, author, sha}]}`. `from` is
  `null` for a spec's filing. Earlier releases printed the events-feed JSON
  here. For that JSON, use `aida history events --json`.
- `aida history --digest --json`, and the JSON printed when the flow falls
  back to the digest, print the digest rows:
  `{view: "history", count, archived_hidden, deferred_hidden,
  history: [{id, type, status, when, change, title}]}`. `change` is `add`,
  `edit` or `del`.

A caller that asks for JSON always gets JSON for the view it received.
`view` tells the two apart, and the stderr note explains a fallback.
