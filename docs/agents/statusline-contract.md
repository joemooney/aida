# Cross-Client Statusline Contract

Status: implemented TASK-1479. Companion reference to
`docs/cli/07-project-setup.md`'s `aida statusline` entry (the CLI-manual
summary) and `aida statusline setup` (per-client install guidance). This
document is the field contract; the setup command is the installer.

## Why a contract

`aida statusline` already renders a stable, AIDA-only segment — role/session,
active spec (`@SPEC`), queue depth, worktree divergence, cache freshness —
cheap enough (cache + local files, no network) to run on every prompt/footer
render. Some clients separately expose their OWN live state to a
command-backed statusline: the current model, context-window usage, what the
agent is doing right now, VCS branch. That data is client-owned, not AIDA
state — it varies per client, per render, and is never durable. This
document draws the line between the two categories and defines the one
shape client-live fields are normalized into before they're combined with
the AIDA segment.

## The two categories

| | Stable AIDA fields | Client live fields |
|---|---|---|
| **What** | role/session, active spec (`@SPEC`), queue depth, worktree divergence, cache freshness, focus, drain/orchestrator badges, mail/presence markers | model, context-window usage, current activity, VCS branch/dirty |
| **Source** | `.aida/cache.db`, the orphan-store queue YAML, the session lease directory, `.aida/` config | the client's own stdin JSON payload |
| **Owner** | AIDA (`statusline_cmd::handle_statusline_command`) | the client (Claude Code, Antigravity/Agy) |
| **Durability** | none of it is written back — this is a read/render, not a write | never durable; redrawn from the next payload on the next render |
| **Cadence** | recomputed on every render from local/cache state | supplied by the client on every render (when the client pipes one) |
| **Renderer** | the existing `aida statusline` segment builder — **unchanged by this contract** | `statusline_contract::format_live_segment` (shared across clients) |

Nothing about the stable-AIDA half of the contract changed for TASK-1479 —
this work adds the client-live half and the one formatter that combines the
two. Plain `aida statusline` (no `--client`) never reads stdin and renders
exactly what it always has.

## `ClientLiveFields` — the shared shape

Defined in `aida-cli-lib/src/statusline_contract.rs`. Every field is
`Option` — a client that omits a field (or an older client version that
doesn't send it yet) degrades silently, never an error:

| Field | Type | Meaning |
|---|---|---|
| `model` | `Option<String>` | short model label, e.g. `Sonnet 4.5` |
| `context_remaining_pct` | `Option<u8>` (0-100) | percent of the context window **remaining** (not used) — chosen so a healthy session reads as a big number and a nearly-full context reads as a small, alarming one |
| `activity` | `Option<String>` | free-form current-activity label (e.g. `editing`), when the client sends one |
| `vcs_branch` | `Option<String>` | branch name, when the client is the more current source (AIDA's own segment already reflects git state via the session lease in the common case) |
| `vcs_dirty` | `Option<bool>` | dirty working tree, when the client reports it |

`statusline_contract::format_combined(aida_segment, live, columns)` is the
one formatter every adapter calls. It never re-derives the AIDA segment —
adapters hand it the string `aida statusline` already produced. Fallback
rules:

- no live fields (client omitted `--client`, or sent nothing usable) → the
  AIDA segment alone, unchanged from today;
- an empty AIDA segment (e.g. run outside a project) → the live segment
  alone, not a stray leading separator;
- terminal narrower than `NARROW_TERMINAL_COLUMNS` (60 cols) → the live
  segment sheds fields lowest-priority-first (activity, then whatever's
  still too wide) before either segment is truncated;
- the AIDA segment carries ANSI color (the installed command runs with
  `--color=always`, since the client pipes it through a non-TTY) → the
  hard-truncation fallback is skipped entirely for that line, because
  slicing mid-escape-sequence would corrupt the terminal's color state for
  the rest of the session — worse than an over-wide line. Truncation only
  ever applies to a plain-text (no-ANSI) line.
- unknown/stale AIDA values already degrade the same way the plain
  `aida statusline` segment always has (`cache:?` for unknown freshness,
  `role:X (default)` for an unset role, etc.) — this contract does not
  change that behavior, only what gets appended alongside it.

## Per-client adapters

Adapters are thin: parse the client's stdin JSON into `ClientLiveFields`,
nothing else. They never re-render the AIDA segment and never retain, log,
or echo the raw payload (see **Privacy** below).

### Claude Code (`--client claude`)

`aida-cli-lib/src/statusline_claude_adapter.rs`. Claude Code pipes a JSON
snapshot to a command-backed `statusLine` on every render. Schema notes
(fact-checked against Claude Code's statusline JSON schema docs at TASK-1479
authoring time):

- `model.display_name` (preferred) or `model.id` → `model`.
- `context_window.remaining_percentage`, or `100 - context_window.used_percentage`
  when only the used side is sent → `context_remaining_pct`. Both may be
  `null` early in a session (before the first API call) or right after
  `/compact` — the adapter degrades to `None` in that case, not an error.
- `worktree.branch` → `vcs_branch`. This is the **only** branch info in the
  payload — Claude Code does not include git status/dirty state for the main
  repo at all (by design: the statusline command is expected to shell out to
  git itself if it wants that). `vcs_dirty` is therefore always `None` from
  this adapter.
- There is **no** activity/current-tool field in the payload as of this
  writing — it's a point-in-time snapshot, not a live activity feed.
  `activity` is therefore always `None` from this adapter today; if Claude
  Code adds one later, wire it in tolerantly (absent → `None`, never an
  error).
- Everything else in the payload (`cost`, `rate_limits`, `prompt_cache`,
  `pr`, `vim`, `agent`, `session_id`, `transcript_path`, …) is ignored by
  construction — the parser structs have no `deny_unknown_fields`.

### Antigravity / Agy (`--client antigravity`, alias `agy`)

`aida-cli-lib/src/statusline_agy_adapter.rs`. **No Agy statusline payload
shape is recorded anywhere in this repo** — checked `docs/agents/antigravity-*`,
the `docs/competitive-analysis/**/*agy*` decompose notes, and the SPIKE-27
architecture inventory at TASK-1479 authoring time; none of them capture an
actual statusline JSON sample. This adapter is therefore **speculative and
tolerant by design**: it accepts several plausible key spellings per field
(a bare string or an object for `model`; `context.remaining_percent` /
`remaining_pct` / derived from `used_percent` / `used_pct` / derived from
`used_tokens`+`total_tokens`; `activity` or `status`; `vcs` or `git` for
branch/dirty) and ignores anything else. **Tripwire:** when a real Agy
statusline payload is captured, fact-check the adapter's field table against
it (the module doc comment has the full table) and delete whichever guessed
spellings turned out wrong — the same way the Claude adapter's schema notes
above were fact-checked against Claude Code's actual docs.

### Codex (documentation only — no adapter)

Codex's TUI footer renders a fixed, built-in set of item IDs
(`[tui] status_line = [...]` in `~/.codex/config.toml` or a trusted
project's `.codex/config.toml`); it does not run an arbitrary command and so
has no stdin payload to parse. There is nothing for a
`statusline_codex_adapter` to do. See **Codex setup** below for the
companion-surface guidance (`aida statusline setup --client codex`).

## Privacy — never log or echo raw payloads

Both adapters parse their client's stdin JSON directly into
`ClientLiveFields` and discard the rest. A client payload can carry `cwd`,
session/transcript identifiers, or other session-identifying data that has
no business in a status line or a log file:

- a parse failure degrades to `ClientLiveFields::default()` (no live
  segment), never an error message that echoes the input;
- `read_stdin_payload()` (`statusline_contract.rs`) returns the raw string to
  the caller's parser and nothing else ever holds onto it — it is not
  written to `.aida/`, not logged, not included in any error text;
- adapters extract only the five documented fields; every other key in the
  payload is dropped during deserialization, never forwarded anywhere.

## Fallback behavior (missing fields, narrow terminals, unknown/stale values)

- **Missing client fields** — every `ClientLiveFields` field is optional;
  `format_live_segment` renders only the fields present and returns `None`
  (no segment at all) when every field is absent.
- **No payload piped / a stuck writer** — `read_stdin_payload()` checks
  `stdin.is_terminal()` first and skips the read entirely on a TTY, so a
  human running `aida statusline --client claude` directly at a shell
  renders the AIDA-only segment with no read at all. Separately — and this
  is the actual guarantee for a piped, non-TTY stdin — the read itself runs
  on a background thread with a bounded wait: `STDIN_READ_DEADLINE` (200ms)
  on the calling side, and a `MAX_STDIN_PAYLOAD_BYTES` (256KB) cap on the
  reader thread. An open pipe or FIFO whose write end never sends EOF (a
  real, reproduced failure mode — a stuck client, or a shell redirect left
  open) times out and degrades to the AIDA-only segment instead of hanging
  the prompt; the background thread is not joined on timeout and is simply
  left to exit with the process. The same bound applies to an oversize
  payload (truncated read, degrades to no live segment) and to a plain read
  error. This is a BOUNDED wait, not an unconditional "never blocks" — a
  well-behaved client's payload (a few KB, written promptly) comfortably
  clears the 200ms deadline in practice.
- **Narrow terminals** — below `NARROW_TERMINAL_COLUMNS` (60 columns), the
  live segment sheds its lowest-priority field (activity) before either
  segment is truncated; a pathologically narrow width still falls back to
  the AIDA segment alone, then a hard `…`-truncated line as the last resort
  (plain text only — see the ANSI note above).
- **Unknown/stale AIDA values** — unchanged from the existing behavior:
  `cache:?` for unknown freshness, `role:X (default)` for an unset role,
  etc. This contract only adds the live segment alongside that; it doesn't
  touch how the AIDA segment itself degrades.

## Setup, per client

`aida statusline setup --client <claude|codex|antigravity|all>` prints this
guidance (or, for `claude`/`antigravity`, `--install`s it); the summaries
below are the same content, gathered in one place.

### Claude Code

```bash
aida statusline setup --client claude --install
```

Merges a command-backed `statusLine` into `.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "aida statusline --color=always 2>/dev/null || printf '%s' \"$(pwd)\""
  }
}
```

That's the AIDA-only segment (unchanged default). To also merge Claude
Code's own live fields (model, context-window usage — see the adapter notes
above), hand-edit the installed command to add `--client claude`:

```json
"command": "aida statusline --client claude --color=always 2>/dev/null || printf '%s' \"$(pwd)\""
```

Claude Code pipes its statusLine JSON to this command on stdin already —
`--client claude` is the only change needed to start consuming it.
`statusLine.command` runs under `/bin/sh` (dash, not bash) on Debian/Ubuntu —
see `aida-core/templates/memories/feedback_claude_code_statusline_posix.md`
if you're hand-editing the fallback shell logic.

### Antigravity / Agy

```bash
aida statusline setup --client antigravity --install
```

Merges a command-backed `statusLine` (footer) and `title` (terminal title)
into `~/.gemini/antigravity-cli/settings.json` (or
`$AIDA_ANTIGRAVITY_SETTINGS`, or `--settings-path`), backing up any existing
file first. `--replace-default` swaps `stack_with_default` to `false`. As
with Claude, add `--client antigravity` (alias `agy`) to the installed
`statusLine.command` to merge Agy's live model/context/activity/VCS fields
when Agy pipes that payload on stdin — this is speculative/tolerant (see the
adapter notes above) until a real Agy payload is captured and fact-checked.

### Codex

```bash
aida statusline setup --client codex
```

Print-only — Codex's footer is a fixed set of built-in item IDs
(`[tui] status_line = [...]`); it never runs a command and so never pipes a
JSON payload, which is why there is no `--client codex` adapter (see
**Codex (documentation only)** above). The guidance covers the built-in
fields, disabling Codex's own terminal-title writer
(`terminal_title = null`, so it doesn't fight AIDA/tmux for title
ownership), and the recommended companion surface for the full AIDA segment:

```bash
# tmux
set -g status-right '#(aida statusline --color=never)'
set -g status-interval 15
```

`aida statusbar --plain` is the lower-cost ambient-meter variant for the
same tmux `status-right` slot. Tripwire: re-check upstream Codex at each
competitive refresh for a command-backed `[tui] status_line` item; if that
lands, prefer the native footer over the tmux workaround.

## Trying it

```bash
# AIDA-only segment (unchanged default; no stdin read)
aida statusline

# Merge a client's live payload (reads stdin, bounded to a 200ms deadline;
# degrades to AIDA-only when stdin is a TTY, the read times out, the
# payload doesn't parse, or the payload doesn't fit in 256KB)
echo '{"model": {"display_name": "Sonnet 4.5"}, "context_window": {"remaining_percentage": 62}}' \
  | aida statusline --client claude --color=never

echo '{"model": "gemini-3-pro", "context": {"remaining_percent": 71}, "activity": "editing"}' \
  | aida statusline --client antigravity --color=never
```

See `docs/cli/07-project-setup.md` (`### \`aida statusline\``) for the
CLI-manual entry and per-client setup instructions below.
