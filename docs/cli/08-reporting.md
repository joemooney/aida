# Chapter 8 — Reporting & lenses

This chapter is all sight, no mutation. Every command here is a **lens** — a read-only view onto the requirement graph, the lifecycle, or the recorded telemetry. Nothing in this chapter changes a spec's status, moves work through the queue, or touches a branch. The whole skill of this chapter is matching the **question** to the **lens**: "where am I right now?" is a different lens from "what happened this week?", which is different again from "is the autonomous drain actually working?". The commands overlap enough that picking the wrong one is the common mistake — so the entries below lead with *which question each one answers*.

> Manual contract reminder: rationale, not flag tables. `aida <command> --help` is the source of truth for exact flags and defaults. We cover only the options whose *rationale* isn't obvious from their name.

---

## Picking the right lens

Four of these commands look superficially similar — they all "report on the project" — but each answers a distinct question. Internalize this and you'll never reach for the wrong one:

| Question | Lens | Reads from |
|---|---|---|
| **Where am I right now?** (this shell, this branch, this minute) | `aida status` | live session/git/queue/cache state |
| **What's been touched, and how does it stand?** (audit trail) | `aida history` | the orphan-store git log |
| **What's the story of what shipped?** (narrative, for a reader) | `aida digest` | the orphan-store git log, editorially filtered |
| **Why is *this one* spec still open?** | `aida why` | the spec's store signals |
| **Are agents actually lifting load?** (proof, metrics) | `aida metrics` / `aida usage` | the telemetry logs (`~/.aida/*.jsonl`) |

The dividing lines: `status` is *now*, everything else is *over a window*. `history` is the raw machine-readable record; `digest` is the same events run through editorial logic into prose for a human reader. `metrics`/`usage` don't read the graph at all — they read the telemetry substrate. `why` is the only one scoped to a single spec.

---

### `aida status`

**One line** — "where am I right now," one screen, no flag-guessing.

**Mental model.** The spiritual cousin of `git status`, but for your whole AIDA context: the active session/lease covering this directory, the branch and its ahead/behind, the open PR + CI rollup, the queue items routed to your role, cache freshness, and project counts — all in one glance. Each section graceful-degrades when its data isn't available (no `gh`, no session, offline), so it never errors out; it just shows less. This is the **default entry point** for "what's going on here?"

**Reach for it when** — you sit down at a shell and need orientation; or you want the at-a-glance "is there anything awaiting me / needing cleanup" sweep before deciding what to do next.

**Don't reach for it when** — you want history over a window (that's `history`/`digest`), or you want to *fix* the things it surfaces. `status --cleanup` is a glance; the *fix* is `aida doctor heal`. The lens shows you what needs attention; it deliberately does not act.

**Key options (rationale only).**
- `--short` — the one-line role/scope/branch readout, the `aida statusline` cousin. For when you want orientation without the full screen.
- `--awaiting` / `--cleanup` / `--activity` / `--queue` / `--ci` — *focus* flags: each collapses the report to one section. The full `aida status` already leads with "Awaiting you" and footers cleanup/activity when non-empty; these flags are for when you want *only* that lens.
- `--no-ci` — skip the `gh`-backed PR/CI lookup. The offline/fast path; that lookup is the slow part.
- `--json` — machine-readable, with failed sections as `null` so a consumer can tell "section absent" from "section empty." The scripting surface.
- `--verbose` — lifts the per-section item caps (the first-3 / 5-item truncation) when you actually want the full list.

**Gotchas.** The `--cleanup` and `--activity` sections are explicitly *read-only* — they tell you what's wrong, they don't heal it. Don't expect `status` to ever mutate anything. The PR/CI section needs `gh` authenticated on PATH; without it that section is simply omitted (text) or `null` (JSON), not an error.

**Chains with** — the natural first command of a session; what it surfaces routes you onward to `queue work`, `doctor heal`, `review`, or `pull`.

---

### `aida history`

**One line** — the audit trail: what's been touched and how it stands now.

**Mental model.** `history` reads the **orphan-store git log** — the source-of-truth record of every status flip, comment, tag edit, owner change. Two modes: the default **digest** mode is a per-requirement view sorted by last-touch ("what was I up to last session?"); `--events` switches to a **chronological per-event feed** that decodes each commit's YAML diff into one line per change. Digest is cheap and broad; events is slower (it shells out per file per commit) but precise — the mode for inspecting one spec closely.

**Reach for it when** — you want the *machine-faithful* record: what changed, when, by whom. "Did my ship register?" (`--shipped`), "what moved this week?" (`--since`), "show me everything that happened to `<spec-id>`" (`--events --id`).

**Don't reach for it when** — you want a *readable narrative* for a person (that's `digest` — same events, editorial prose). And don't reach for `--events` as a general overview; it's slow by design. Use default digest mode for breadth, `--events` only when you're drilling into one spec or one transition type.

**Key options (rationale only).**
- `--events` — the chronological decode. It's the slow, precise mode; pair it with `--id` (one spec) or `--status-changes`/`--comments` (one event kind) so you're not decoding the whole log.
- `--shipped` — the "did my ship register?" view: only recent Done→Completed merges, newest first. Distinct from `--all` (a recency-blind dump of every terminal spec) — `--shipped` answers a question, `--all` widens the net.
- `--all` vs `--archived`/`--deferred` — `--all` is the everything-escape-hatch (active + archived + deferred, symmetric with `aida list --all`); `--archived`/`--deferred` narrow to *only* that shelf. Default `history` hides archived/deferred but keeps freshly-Completed ships visible.
- `--max-commits` — bounds how far back it walks the orphan branch. The knob for "this is slow / I only care about recent."

**Gotchas.** The default digest mode is sorted by *last-touch*, not by event time, so it's a "current standing" view, not a timeline — switch to `--events` for an actual chronology. The cache does **not** carry history rows; `history` reads the YAML/git log directly, which is why `--events` costs real time.

**Chains with** — the audit counterpart to `status` (now) and `digest` (narrative). Feed an `--id` from `list`/`show` to drill into one spec's life.

---

### `aida report`

**One line** — generate a structured project report (currently: AI-integration status).

**Mental model.** A small family of *generated-document* commands. Today it has one subcommand, `ai-integration`, which renders a report on how AIDA + AI tooling is wired into the project (scaffolding status, integration surface) as markdown or HTML. Think of it as the "produce a document about the project's setup" lens, distinct from the activity lenses (`history`/`digest`) — it reports on *configuration/integration state*, not on *what happened*.

**Reach for it when** — you want a shareable artifact describing the project's AI-integration posture (for onboarding docs, a status writeup, an audit).

**Don't reach for it when** — you want activity or shipped-work narrative (that's `digest`), or live orientation (`status`). `report` is about the project's integration shape, not its timeline.

**Key options (rationale only).**
- `--format markdown|html` + `--output` — it's a document generator, so the natural knobs are format and where-to-write. HTML for a browsable artifact, markdown to paste into docs.
- `--include-scaffold` — fold the scaffolding-status check into the report rather than reporting integration alone. Reach for it when the report is meant to answer "is this project fully set up?"

**Gotchas.** `report` is a parent command with subcommands — bare `aida report` prints the subcommand list, not a report. You want `aida report ai-integration`.

**Chains with** — a one-off documentation artifact; pairs with the Chapter 7 setup commands it reports on.

---

### `aida digest`

**One line** — the narrative advisor report: a readable story of what shipped, for a window.

**Mental model.** `digest` and `history` read the *same* events; the difference is **editorial logic**. `digest` runs those events through a mechanical filter — drop typo/chore/style commits, collapse cluster-PRs to one theme line, keep rejected specs only when they carry a supersedes link, strip SPEC-IDs in customer mode — and renders them as prose under fixed headings (Released / Major progress / Strategic direction / Next iteration / Process artifacts). Where `history` is the raw ledger, `digest` is the write-up. It's **audience-aware**: the same window reads differently for a customer, a teammate, yourself, or a power-user operator.

**Reach for it when** — you need to *tell someone* what happened: a customer changelog, a team update, a "what did I get done" self-review, or (`--audience operator`) a power-user "what changed in the CLI surface today."

**Don't reach for it when** — you want the exact machine record for an audit (that's `history --events`), or live orientation (`status`). `digest` is intentionally lossy — it editorializes — so it's the wrong lens when you need every event faithfully.

**Key options (rationale only).**
- `--audience customer|team|self|operator` — the single most consequential flag: it sets both the framing *and* SPEC-ID visibility. `customer` strips SPEC-IDs (they're internal breadcrumbs, noise to a user); `operator` is the CLI-surface diff for power-users. Pick the reader.
- `--since` — the window start, accepting a duration, an ISO date, *or a git tag/ref*. The tag form ("everything since `v0.12.0`") is the release-notes path.
- `--include-next` / `--include-process` — toggle the forward-looking and memory-pack sections; defaults differ by audience (process is on for team/self, off for customer) so the right reader gets the right depth.
- `--copy` / `--out` — it's a document you'll paste somewhere, so clipboard and file-write are first-class and compose.
- `--reset` — clears the cadence marker. `digest` remembers its last window in `.aida/last-digest.toml` and auto-resumes; `--reset` is how you break that chain when the next digest shouldn't continue from here.

**Gotchas.** The default window is *not* a fixed 24h — it's the cadence marker's `window_end` (resuming from the last digest), falling back to 24h only when there's no marker. If a digest looks like it starts in an odd place, that's the marker; `--since` overrides it and `--reset` clears it. Default audience is `customer`, which **strips SPEC-IDs** — pass `--audience team`/`self` if you want them.

**Chains with** — the human-readable counterpart to `history`. The advisor's `/aida-digest` skill wraps this; release notes draw `--since <tag>`.

---

### `aida usage`

**One line** — inspect locally-recorded CLI usage and the orchestrator's drain telemetry.

**Mental model.** Two logs, one command. By default `usage` reads `~/.aida/usage.jsonl` (one privacy-floored line per `aida` invocation — *command shapes only*, never arg values or paths) and shows your top-20 commands over 30 days. With `--auto-complete` it pivots to a *different* log entirely (`~/.aida/auto-complete.jsonl`) — the autonomous-drain orchestrator's success/failure record. So it's really two lenses sharing a verb: "how am I using the CLI" and "how is the drain doing."

**Reach for it when**
- bare / `--unused` / `--errors` — surface deprecation candidates (commands nobody runs) and UX-gap candidates (commands that error a lot). The substrate for "what should we cut or fix."
- `--auto-complete` (+ `--failures` / `--pattern` / `--health`) — diagnose the autonomous drain: which phases fail most (`--pattern` = where to invest orchestrator fixes), every recent failure in full (`--failures`), or the deterministic project-health catalog (`--health`).

**Don't reach for it when** — you want the *polished* agent-lift story for a case study or release note (that's `metrics agent-lift`, which presents the same substrate as proof). `usage` is the raw inspection tool; `metrics` is the framed narrative.

**Key options (rationale only).**
- `--unused <Nd>` vs `--errors` — the two deprecation/UX signals, mutually exclusive because they answer opposite questions ("never used" vs "used and failing"). Both feed the `/aida-insights` review cadence.
- `--auto-complete` — the mode-switch to drain telemetry. Without it you're in CLI-usage mode; the `--failures`/`--pattern`/`--health` sub-flags only mean anything *with* it.
- `--json` — machine consumption (`{cmd, count, errors, avg_ms}` per command).

**Gotchas.** `--failures`, `--pattern`, and `--health` are no-ops without `--auto-complete` — they qualify the drain-telemetry mode, not the default usage view. Telemetry is opt-out (`AIDA_TELEMETRY=0` or `[telemetry] enabled = false`); if the log is empty, telemetry was disabled — the command isn't broken.

**Chains with** — the inspection half of the telemetry surface; `metrics` is the presentation half. The `/aida-insights` skill synthesizes `usage` + `usage --auto-complete` into the monthly review.

---

### `aida metrics`

**One line** — agent-lift metrics: the *framed proof* that autonomous drains lift load.

**Mental model.** `metrics` reads the same telemetry substrate as `aida usage --auto-complete`, but its job is **presentation, not inspection**. The one subcommand, `agent-lift`, computes the coordination signals — drain success rate, autonomous runs over distinct specs/builds, stale-base recoveries, and the autonomous-vs-human split — and renders them for an *audience*: a case study, release notes, or "proving coordination value." Where `usage --auto-complete` is the operator's diagnostic dashboard, `metrics agent-lift` is the slide you'd show someone.

**Reach for it when** — you need to *demonstrate* that the autonomy machinery is working: a case study, a release-notes paragraph, a "look what the drains did this month" writeup.

**Don't reach for it when** — you're *debugging* the drain (which phase keeps failing, what halted) — that's `aida usage --auto-complete --pattern`/`--failures`/`--health`, the diagnostic side. `metrics` summarizes the win; `usage` dissects the failure.

**Key options (rationale only).**
- `--markdown` — emit pasteable Markdown for release notes / a case study (the default is the colorized terminal view). The flag exists because this command's *output is meant to be shared*.
- `--since <window>` — bound the reporting period (the case-study window).
- `--json` — the computed signals for machine consumers.

**Gotchas.** `metrics` is a parent command — bare `aida metrics` lists subcommands; you want `aida metrics agent-lift`. It and `usage --auto-complete` read the *same* `auto-complete.jsonl`, so they never disagree on the numbers — they disagree on *framing*. Pick by whether you're proving or debugging.

**Chains with** — the case-study/release-notes companion to `digest` (narrative) and `usage` (diagnostic).

---

### `aida why`

**One line** — explain why *this one* spec is still open.

**Mental model.** A single-spec drill-down using the same classifier as `burndown explain`: given a SPEC-ID, it derives a **bucket + reason** from the spec's store signals (status, type, tags, blockers, decisions, live leases) and answers "what's keeping this from being done?" Where the other lenses survey the project, `why` is laser-focused on one node — and on the *one* question of why it hasn't moved.

**Reach for it when** — a spec is stuck and you want the machine's read on *why*: blocked by a dependency? awaiting a decision? needs a human? sitting un-queued? It's the fast triage of a single stalled spec.

**Don't reach for it when** — you want the spec's full contract and git linkage (that's `aida show`), or you want to survey *all* the stuck specs (that's `aida backlog`/`burndown explain` across the set). `why` answers one question about one spec.

**Key options (rationale only).**
- `--json` — emits `{spec, bucket, reason, needs_human}`. The `needs_human` boolean is the routing signal — it's what an orchestrator checks to decide "park for triage vs keep going."

**Gotchas.** `why` only explains *open* specs — it's about what's keeping something from being done, so a closed spec has nothing to explain. It reports the classifier's read, which is a heuristic over store signals; it's a strong first hypothesis, not a guarantee.

**Chains with** — `aida show <ID>` for the full picture, `aida graph <ID> --blocked-by` to trace the blocker chain `why` named, `aida punt`/`aida triage` to act on the reason.

---

### `aida intent`

**One line** — show a plain-terms read of *why this spec exists* — its purpose, distilled from the spec and its graph neighborhood.

**Mental model.** Where `aida why` is a deterministic state classifier (a heuristic over store signals: blocked? awaiting decision? un-queued?), `intent` is an AI synthesis of *meaning*: it reads the spec plus the specs around it and writes a short comprehension of what the work is really for. The result is cached and drift-stamped — generated on first call, printed from cache after, with a STALE marker when the neighborhood has moved since it was generated. So `why` answers "what's keeping this from being done?" and `intent` answers "what is this even for, in human terms?"

**Reach for it when** — you've just loaded an unfamiliar spec (or an agent has) and the title plus description don't yet add up to *why it matters*. It's the orientation pass before you plan or implement: get the gist, then dig into the contract.

**Don't reach for it when** — you want the spec's literal contract, status, and git linkage (that's `aida show`), or you want the deterministic "why is it stuck" classification (that's `aida why`). `intent` is interpretive synthesis, not a substrate fact — treat it as a strong summary, not the source of truth.

**Key options (rationale only).**
- `--audience` — `layman` (default) writes prose for a human skimmer; `llm` writes a denser, structured register for an agent loading the spec into context. Pick by who's reading.
- `--refresh` — force regeneration when the cached comprehension is stale or the spec changed in ways the drift stamp didn't catch.
- `--json` — machine-readable envelope (`spec`, `audience`, `comprehension`, `generated_at`, `model`, `stale`) for downstream consumers.

**Gotchas.** The output is an LLM synthesis, so it costs a generation on the first call (and on `--refresh`); thereafter it's a cache read. The STALE marker is your cue that the neighborhood drifted — re-run with `--refresh` if the cached read no longer fits.

**Chains with** — pairs with `aida show <ID>` (the literal contract) and `aida why <ID>` (the stuck-state classifier): `intent` for the *why it exists*, `why` for the *why it's still open*, `show` for the facts.

---

### `aida user-guide`

**One line** — open the rendered user guide in the default browser.

**Mental model.** A convenience launcher, not a report: it opens AIDA's user guide in your browser. The thinnest possible "lens" — it points your eyes at the docs rather than computing anything from the store.

**Reach for it when** — you want the prose user guide and would rather read it in a browser than dig through the repo.

**Don't reach for it when** — you're in a headless/no-browser context (it has nothing to open), or you want command-specific facts — `aida <cmd> --help` is faster and authoritative for that.

**Key options (rationale only).**
- `--dark` — opens in dark mode. A reading-comfort toggle, nothing more.

**Chains with** — orientation alongside `aida status` and this very manual; no lifecycle role.

---

### `aida manual`

**One line** — print this manual's rationale entry for a command, inline in the terminal.

**Mental model.** `aida manual <cmd>` is the bridge between this prose manual and your shell. `--help` tells you *what* a command does and *which* flags it takes; this manual tells you *when, why, and when not*. `aida manual <cmd>` pulls the matching `### \`aida `<cmd>`\`` section out of these chapters and prints it next to where you're working — so the rationale is one command away instead of a context-switch to the browser. It pages the output when a pager is available, otherwise prints plain. `--help` stays the source of truth for flags and defaults; `manual` never reproduces them.

**Reach for it when** — you know roughly which command you want but aren't sure it's the *right* one for the situation, or you want the "don't reach for it when" guidance before committing to an approach. It's the fast in-terminal lookup for the judgment layer.

**Don't reach for it when** — you want the exact flag list, defaults, or argument syntax (that's `aida <cmd> --help`, always authoritative and never drifting), or you want to read the whole journey end-to-end (open the manual's index for the narrative spine and cross-links).

**Gotchas.** It matches the command's *entry header*, so it works for any command this manual documents — including ones covered under a shared header with sibling commands. If a command has no manual entry yet, it exits non-zero and says so rather than printing nothing; that's also a hint the manual is lagging the binary.

**Chains with** — the natural follow-on to `aida <cmd> --help`: read the facts, then read the rationale. Pairs with `aida user-guide` (browser, whole-guide) for the in-terminal, one-command slice.

---

### `aida record`

**One line** — inspect or prune the durable per-spec **processing record** — the audit trail of *what was done and why*, captured at completion.

**Mental model.** When a spec reaches completion, AIDA can persist a **processing record** on it: a durable note of what the work actually did and the reasoning behind it — distinct from the `history:` array (which logs *field transitions*) and from git linkage (which shows *commits*). The processing record is the *narrative audit* — the "why," captured while the context is fresh. `aida record list` reads it (for one spec, or every spec carrying one); the block also surfaces inside `aida show`. `aida record prune` trims records to save space **without** touching the spec or its history.

**Reach for it when** — you (or a reviewer/auditor) want the *reasoning trail* behind a completed spec — what a drain decided and why — not just the diff. It's the substrate behind the "explain intent, not just surface" goal: a place the *why* lives after the work is done.

**Don't reach for it when** — you want *field-change* history (that's `aida history --events`) or the *commits/files* a spec touched (that's `aida show`'s git linkage). Record is the narrative layer above both.

**Gotchas.** `record prune` is **propose-by-default** — it shows what it *would* trim and only writes with `--apply`. So a bare `aida record prune` is safe to run as a preview. Pruning loses the narrative, not the spec or its transition history.

**Chains with** — populated at completion (the audit the governance/intent story leans on); read via `record list` or inline in `aida show`; complements `aida history` (transitions) and `aida digest` (the outward-facing surface-change summary).

---

## Where to go next

You now have every read-only lens: live orientation (`status`), the audit trail (`history`), the narrative write-up (`digest`), the configuration report (`report`), the telemetry surfaces (`usage` / `metrics`), the single-spec drill-down (`why`), and the docs launcher (`user-guide`). The questions they answer route back into the rest of the manual:

- **[Chapter 1 — Getting started](01-getting-started.md)**: `list` / `show` — the graph lenses these reporting views send you to drill into.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: the transitions `history` and `digest` are *reporting on* — where Done, Completed, and Released come from.
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: `backlog` / `burndown` — the survey-the-stuck-set counterparts to single-spec `why`, and the drains that `metrics`/`usage --auto-complete` measure.
