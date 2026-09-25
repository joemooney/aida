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

**Mental model.** `history` reads the **orphan-store git log** — the source-of-truth record of every status flip, comment, tag edit, owner change. Three views, from broadest to most detailed:
- **Digest** (default, no SPEC-ID) — a per-requirement view sorted by last-touch ("what was I up to last session?").
- **Status progression** (`aida history <SPEC-ID>`, shorthand for `aida history --id <SPEC-ID>`) — that one spec's status transitions in chronological order, oldest first, each with a timestamp and its old→new status. This is the "how did this spec get here?" view. **Human-only**: a non-interactive caller (piped/redirected stdout, a script, CI, `AIDA_AGENT_OUTPUT` truthy) gets the same single-row digest TOON table `--id` has always produced, not this narrative — see Gotchas below.
- **Full trail** (`aida history events`, or `--full` after a SPEC-ID) — the complete chronological per-event feed, decoding each commit's YAML diff into one line per change (status transitions, comments, tag edits, field edits, …), newest first. Slower than the other two (it shells out per file per commit) — the mode for a forensic read of one spec, or of everything in a time window.

**Reach for it when** — you want the *machine-faithful* record: what changed, when, by whom. "How did this spec get to where it is?" (`aida history <SPEC-ID>`), "did my ship register?" (`--shipped`), "what moved this week?" (`--since`), "show me *everything* that happened to `<spec-id>`" (`aida history <SPEC-ID> --full`, same as `aida history events --id`).

**Don't reach for it when** — you want a *readable narrative* for a person (that's `digest` — same events, editorial prose). And don't reach for `--full`/`events` as a general overview; it's slow by design. Use default digest mode for breadth, a SPEC-ID for one spec's status story, `--full` only when you need the complete trail.

**Key options (rationale only).**
- `aida history <SPEC-ID>` — the positional shorthand for `--id <SPEC-ID>`; either form selects the status-progression view for that one spec instead of the digest's single current-state row.
- `--full` (or the `events` subcommand) — the complete edit/comment trail, not just status changes. Composes after a SPEC-ID: `aida history <spec-id> --full`.
- `--status-changes` / `--comments` — narrow to one event kind. Paired with a SPEC-ID and no `--full`, `--comments` swaps the default status-progression view for a comment timeline; paired with `--full`/`events`, either one narrows the complete trail down. Passing both shows either kind (status changes *or* comments), not neither.
- `--shipped` — the "did my ship register?" view: only recent Done→Completed merges, newest first. Distinct from `--all` (a recency-blind dump of every terminal spec) — `--shipped` answers a question, `--all` widens the net.
- `--all` vs `--archived`/`--deferred` — `--all` is the everything-escape-hatch (active + archived + deferred, symmetric with `aida list --all`); `--archived`/`--deferred` narrow to *only* that shelf. Default `history` hides archived/deferred but keeps freshly-Completed ships visible.
- `--kind <KIND>` — reads the local event feed (`.aida/events.jsonl`) instead of the git log, one event kind at a time. `--kind gate-held` is the non-action view: every gate that refused or held (a merge-hold floor, a stale approval, a review in progress, a blocked pickup, a closure hold, an ambiguous id), a count per gate, and the merge-hold floor's refusals beside its releases for the same window, so a rate has its denominator. `--author me` narrows it to what *you* tried and could not, which tells a blocked seat from an idle one.
- `--max-commits` — bounds how far back it walks the orphan branch. The knob for "this is slow / I only care about recent." `events` mode defaults this to `(limit*5).max(50)` when you don't set it; see Gotchas below for what happens when that default runs out.
- `--since` / `--until` — bound the window. Accepts a relative duration meaning "that far before now", either compact (`30m`, `5h`, `7d`, `2w` — minutes/hours/days/weeks) or as a phrase (`24 hours ago`, `1 week ago`), or an absolute point: a bare ISO date like `2026-05-01` means **local midnight** on that date (not UTC midnight, and not the current time of day); a zone-less ISO datetime (`2026-05-01T10:00` or `2026-05-01 10:00`) is local time; a full RFC3339 timestamp keeps its explicit zone. Local times use the offset in effect on that date, so they stay correct across daylight-saving changes; a local time that falls in a daylight-saving gap or overlap is refused (give an explicit offset instead). Other git date phrases (`yesterday`, `last monday`) are not accepted. <!-- trace:TASK-1502 | ai:claude --> Both flags share the same grammar and compose (`--since 7d --until 5h`); a `--since` that resolves later than `--until` is refused with a clear error rather than silently returning nothing. Human output prints a `Window: …` line showing the resolved bounds in local time, each with the numeric zone offset in effect at that instant (e.g. `2026-05-01 00:00 -0700`), so a relative form — or a bare date's local-midnight resolution — is never ambiguous.

**Gotchas.** The default digest mode is sorted by *last-touch*, not by event time, so it's a "current standing" view, not a timeline. The status-progression view (a SPEC-ID, no `--full`) reads oldest-first — a progression reads forward in time — while the full trail (`--full`/`events`) reads newest-first, matching `git log`; the two views don't share a reading order. The cache does **not** carry history rows; `history` reads the YAML/git log directly, which is why the full trail costs real time. A spec that's real but simply hasn't changed status yet prints a quiet "nothing in this view" note, not an error; an id that never existed at all (typo, or a format that isn't `TYPE-SEQ`) refuses with a clear "not found" error instead of a silent empty view. **Agent/piped output keeps the digest format**: the status-progression narrative is a human-terminal upgrade only — a SPEC-ID under non-interactive/agent output (no TTY, or `AIDA_AGENT_OUTPUT` truthy) still renders the pre-existing single-row digest TOON table, so scripts and agents parsing `aida history --id <spec-id>` never see their machine-readable shape change out from under them. Pin `--format human` to get the narrative from a script anyway, or `--format toon`/`json` to force the digest row even at a TTY. **A short `events`/`--full`/`--shipped` result can mean "that's everything" or "the window ran out first"** — `events` mode bounds its `git log` walk to `--max-commits` commits (default `(limit*5).max(50)`) before it even starts decoding, so a spec-sparse stretch of commit history can use up that window before `--limit` events are found, returning fewer than asked for with no obvious sign why. A human terminal gets a one-line notice on stderr when this happens (skipped once you pass `--max-commits` yourself, or once `--limit` was actually met); agent/piped callers and the MCP `history` tool instead get a `window_exhausted` boolean alongside the event array — `true` means widen the walk (`--max-commits <N>`, or narrow with `--since`/`--until`) before trusting the result as complete. The `Window: …` line is human-output only (suppressed under agent/piped output, matching the rest of the TOON-vs-narrative split).

**Chains with** — the audit counterpart to `status` (now) and `digest` (narrative). Feed a SPEC-ID straight from `list`/`show` to see one spec's status story.

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

**Don't reach for it when** — you want the exact machine record for an audit (that's `history events`), or live orientation (`status`). `digest` is intentionally lossy — it editorializes — so it's the wrong lens when you need every event faithfully.

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

**Mental model.** Two logs, one command family. By default `usage` reads `~/.aida/usage.jsonl` (one privacy-floored line per `aida` invocation — *command shapes only*, never arg values or paths) and shows your top-20 commands over 30 days. `aida usage drains` pivots to a *different* log entirely (`~/.aida/auto-complete.jsonl`) — the autonomous-drain orchestrator's success/failure record. So it's really two lenses sharing a verb: "how am I using the CLI" and "how is the drain doing."

**Reach for it when**
- bare / `aida usage unused` / `aida usage errors` — surface deprecation candidates (commands nobody runs) and UX-gap candidates (commands that error a lot). The substrate for "what should we cut or fix."
- `aida usage drains` / `aida usage health` — diagnose the autonomous drain: which phases fail most (`drains` patterns = where to invest orchestrator fixes), recent failures in full, or the deterministic project-health catalog (`health`).
- `aida usage slowest` / `aida usage events` / `aida usage timeline` — the performance lens, three views over the same per-invocation log: `slowest` ranks command *shapes* by latency (p50/p95/max + count), `events` streams the raw fields (`ts`, `cmd`, `duration_ms`, `exit_code`) for one invocation per row, and `timeline` is the same stream rendered as a dense, scannable one-line-per-invocation feed — local timestamp, compact duration, and a pass/fail mark — sized to eyeball what ran immediately before a slow command.

**Don't reach for it when** — you want the *polished* agent-lift story for a case study or release note (that's `metrics agent-lift`, which presents the same substrate as proof). `usage` is the raw inspection tool; `metrics` is the framed narrative.

**Key options (rationale only).**
- `aida usage unused` vs `aida usage errors` — the two deprecation/UX signals, mutually exclusive because they answer opposite questions ("never used" vs "used and failing"). Both feed the `/aida-insights` review cadence.
- `aida usage drains` — the mode-switch to drain telemetry. Without it you're in CLI-usage mode; drain failure/pattern details live under that subcommand, while the deterministic health catalog lives at `aida usage health`.
- `aida usage timeline` vs `aida usage events` — both walk the same newest-first, `--since`/`--cmd`/`--slower-than`/`--limit`-filtered event stream; they differ only in rendering. `events` prints the raw fields (UTC `ts`, exact `duration_ms`, numeric `exit_code`) for exact-value inspection. `timeline` renders a local timestamp, a compact duration (`245ms`/`26.4s`/`1m05s`), an ellipsized command shape, and a ✓/✗ mark — built for scanning a screenful of recent activity around a slow command, not for reading off exact fields. `--json` on either returns the same `{ts, cmd, duration_ms, exit_code}` shape.
- `--read-write` — the *trace-read-rate audit*: classify the logged command shapes into graph **reads** (`list`/`show`/`search`/`graph`/`why`/`history`/`queue list`/`rel list`/…) vs graph **writes** (`add`/`edit`/`comment add`/`rel add`/`queue add`/`defer`/`archive`/…), skip plumbing (sync/dev/statusline), and report the read:write ratio over the window. The question it answers: *is the intent graph consulted, or just written?* A ratio ≥ 1 is evidence the typed layer earns its keep; writes ≫ reads would suggest the typing is dead weight. Measures CLI telemetry only — MCP read tools aren't in `usage.jsonl` yet (an MCP read counter is a follow-up).
- `--json` — machine consumption (`{cmd, count, errors, avg_ms}` per command; `{reads, writes, read_write_ratio, top_reads, top_writes}` under `--read-write`; `{ts, cmd, duration_ms, exit_code}` per row under `events`/`timeline`).

**Gotchas.** Drain failure/pattern views live under `aida usage drains`, not the default usage view, and the health catalog is `aida usage health`. Telemetry is opt-out (`AIDA_TELEMETRY=0` or `[telemetry] enabled = false`); if the log is empty, telemetry was disabled — the command isn't broken.

**Chains with** — the inspection half of the telemetry surface; `metrics` is the presentation half. The `/aida-insights` skill synthesizes `usage` + `usage drains` into the monthly review.

---

### `aida metrics`

**One line** — agent-lift metrics: the *framed proof* that autonomous drains lift load.

**Mental model.** `metrics` reads the same telemetry substrate as `aida usage drains`, but its job is **presentation, not inspection**. The one subcommand, `agent-lift`, computes the coordination signals — drain success rate, autonomous runs over distinct specs/builds, stale-base recoveries, and the autonomous-vs-human split — and renders them for an *audience*: a case study, release notes, or "proving coordination value." Where `usage drains` is the operator's diagnostic dashboard, `metrics agent-lift` is the slide you'd show someone.

**Reach for it when** — you need to *demonstrate* that the autonomy machinery is working: a case study, a release-notes paragraph, a "look what the drains did this month" writeup.

**Don't reach for it when** — you're *debugging* the drain (which phase keeps failing, what halted) — that's `aida drain status` and the phase logs, the diagnostic side. `metrics` summarizes the win; drain inspection dissects the failure.

**Key options (rationale only).**
- `--markdown` — emit pasteable Markdown for release notes / a case study (the default is the colorized terminal view). The flag exists because this command's *output is meant to be shared*.
- `--since <window>` — bound the reporting period (the case-study window).
- `--json` — the computed signals for machine consumers.

**Gotchas.** `metrics` is a parent command — bare `aida metrics` lists subcommands; you want `aida metrics agent-lift`. It and `usage drains` read the *same* `auto-complete.jsonl`, so they never disagree on the numbers — they disagree on *framing*. Pick by whether you're proving or debugging.

**Chains with** — the case-study/release-notes companion to `digest` (narrative) and `usage` (diagnostic).

---

### `aida criteria`

**One line** — show which acceptance criteria for one spec are traced by Rust, pytest, JavaScript/TypeScript, or Go tests.

**Mental model.** `criteria` turns a spec's `## Acceptance` section into stable criterion IDs, then scans Rust `#[test]` functions, pytest `def test_*` functions, Jest/Vitest/Mocha `test(...)` and `it(...)` calls, and Go `func TestXxx(...)` functions. Put a `trace:<SPEC>.<label>` token in a comment directly above the test (`//` in Rust, JS/TS, and Go; `#` in Python) or inside its body. A blank or non-comment line breaks an above-test attachment. It is the smallest anti-drift loop between the store and executable tests: each acceptance criterion should have at least one traced test, and each traced test should point at a real criterion rather than a stale or vague marker.

**Reach for it when** — you want to audit a spec's test coverage at the criterion level, especially before reconstitution/harvest work. The report answers: which criteria have tests, which criteria are untested, and which tests trace a bare spec or an unknown criterion.

**Don't reach for it when** — you want general source trace rot across all files (that's `aida trace check` / `aida doctor validate-trace-comments`), test-runner execution, or AST-complete discovery beyond these supported declaration shapes.

**Key options (rationale only).**
- JSON output — emit the same AC-to-test map and gap lists for scripts or gates when that surface is available.

**Gotchas.** Explicit labels in `## Acceptance` are the most stable IDs (`A1.`, `AC3:`, etc.). Unlabeled criteria get content-hash IDs, which are stable across reorder but change when the criterion text changes; label important criteria when tests will trace them for a long time.

**Chains with** — `aida show <ID>` for the contract, then test edits adding criterion-qualified trace comments, then `aida criteria <ID> --json` for a scriptable gap check.

---

### `aida harvest`

**One line** — pull what a diff *established* back into the spec, so the store stays sufficient to rebuild the behavior.

**Mental model.** `harvest` is the advisory half of the reconstitution loop. It hands a headless agent the diff plus the spec's contract (its acceptance criteria, linked decisions, prior micro-decisions) and asks for the observable, non-obvious facts the diff established that the store does not yet say. Candidates pass a strict selectivity filter, you confirm an **opt-in** checklist (Enter accepts nothing), and confirmed items land per kind: a new labeled line in `## Acceptance` (so `aida criteria` sees it), an `[aida:sem]` marked comment for a micro-decision, or a Draft decision spec linked back. Every run writes an `[aida:harvest]` ledger comment recording what ran, what landed, and what was skipped and why — nothing is dropped silently.

**Reach for it when** — a branch or PR is finished and you want the spec to carry what the code now guarantees, before the knowledge lives only in the diff.

**Don't reach for it when** — the diff is a refactor/format/rename with no new observable behavior (the filter will skip it all, and the ledger will say so), or you want a coverage audit of what is already recorded (that is `aida criteria`).

**Key options (rationale only).**
- PR source — harvest a PR's diff instead of the current branch, for post-merge or review-time harvesting.
- Base ref — compare against something other than the default branch when the branch stacks.
- Accept-all — headless runs have no checklist; opt in to every filtered candidate explicitly (the filter still applies).
- From file — confirm a ready candidate set (a `reconstitute` probe's divergence output) instead of running the agent; same filter, same checklist, same ledger.
- Dry run — print the agent brief without launching anything.
- JSON — machine summary of the run.

**Gotchas.** The filter is configured under `[harvest]` in `.aida/config.toml` (`min_confidence` 0.7, `skip_conventional`, `deny_keywords`, `allow_keywords`); start strict and loosen on evidence. Harvest is advisory — it never blocks a merge. An empty candidate list is a valid, good answer.

**Chains with** — `aida harvest <ID> --pr <N>` after review, then `aida criteria <ID>` to see the new criterion as untested, then a traced test for it.

---

### `aida reconstitute`

**One line** — measure whether a spec could be rebuilt from the store alone, and turn the gaps into harvest candidates.

**Mental model.** `reconstitute` is the round-trip verifier of the reconstitution loop. A headless probe agent gets *store-only* context — the spec, its linked decisions, `[aida:sem]` micro-decisions, graph context and the *names* of the symbols its trace comments point at — and no source at all (it runs in an empty scratch directory outside the project and is told not to read anything). It regenerates the tests the criteria imply; a second pass judges, per criterion, whether each real traced test has a regenerated counterpart (matched / partial / missing, with a reason). The per-criterion divergence lines are the product. The score — matched real traced tests over all of them — is a heuristic trend line and is always labelled so. Real tests the store could not reproduce are written as harvest candidates; confirm them with `aida harvest <ID> --from <file>`. The probe never writes to the spec.

**Reach for it when** — a spec has traced tests (`aida criteria` shows them) and you want to know how much of what the code guarantees actually lives in the store, or you want a trend line across releases.

**Don't reach for it when** — the spec has no traced tests yet (trace tests to criteria first; the score would be 0/0), or you want a coverage audit of what is recorded (that is `aida criteria`).

**Key options (rationale only).**
- JSON — the full report (matches, untested criteria, unanchored tests, candidates file) for scripts and trend tracking.
- Dry run — print the probe brief; useful to confirm it carries no source before spending an agent run.

**Gotchas.** Two headless agent runs per probe (regenerate, then judge). Judging is agent-based, so treat the score as a heuristic and read the reasons. Isolation is by construction of the brief plus an empty working directory, not a sandbox: a tool-using agent that ignores its instructions could still look around, so the guard is the brief (a test pins that it carries no test source). Probe artifacts live under `~/.aida/reconstitute/`; each run removes directories older than seven days and retains at most the newest 20 runs per machine. Harvest candidates live under the project's gitignored `.aida/harvest/` directory and are retained until manually reviewed because they may contain unconfirmed proposals.

**Chains with** — `aida criteria <ID>` first, then `aida reconstitute <ID>`, then `aida harvest <ID> --from <candidates-file>` to land what the store was missing, then trace the new criteria from tests.

---

### `aida why`

**One line** — explain why *this one* spec is still open.

**Mental model.** A single-spec drill-down using the same classifier as `burndown explain`: given a SPEC-ID, it derives a **bucket + reason** from the spec's store signals (status, type, tags, blockers, decisions, live leases) and answers "what's keeping this from being done?" Where the other lenses survey the project, `why` is laser-focused on one node — and on the *one* question of why it hasn't moved.

**Reach for it when** — a spec is stuck and you want the machine's read on *why*: blocked by a dependency? awaiting a decision? needs a human? sitting un-queued? It's the fast triage of a single stalled spec.

**Don't reach for it when** — you want the spec's full contract and git linkage (that's `aida show`), or you want to survey *all* the stuck specs (that's `aida backlog`/`burndown explain` across the set). `why` answers one question about one spec.

**Key options (rationale only).**
- `--json` — emits `{spec, bucket, reason, needs_human}`. The `needs_human` boolean is the routing signal — it's what an orchestrator checks to decide "park for triage vs keep going."

**Gotchas.** `why` only explains *open* specs — it's about what's keeping something from being done, so a closed spec has nothing to explain. It reports the classifier's read, which is a heuristic over store signals; it's a strong first hypothesis, not a guarantee.

**Chains with** — `aida show <ID>` for the full picture, `aida graph blocked-by <ID>` to trace the blocker chain `why` named, `aida punt`/`aida triage` to act on the reason.

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

### `aida explain`

<!-- doc-intent: TASK-1470 -->

**One line** — write a plain-language explanation of one spec for a chosen audience, and tell you whether it is still current.

**Mental model.** `explain` extracts a summary, rationale, key constraints, trade-offs, and open questions from the spec body, saves them as a sidecar file at `.aida/expositions/<SPEC-ID>/<audience>.yaml` in the current checkout, and stamps it with a hash of the spec and its immediate neighbors. Later runs reuse the sidecar and mark it STALE when that neighborhood has changed. A quick quality audit checks that critical rules (for example "fail closed") survived the rewrite.

**Reach for it when** — you need to hand a spec to someone who doesn't read AIDA specs every day: an operator, an executive, a new contributor.

**Don't reach for it when** — you want the literal contract (`aida show`) or an AI-written reading of the spec's purpose (`aida intent`). `explain` works offline by default and is extractive, not generative.

**Key options (rationale only).**
- `--audience` — `operator` (default), `executive`, `implementer`, or `contributor`.
- `--refresh` — regenerate even when a sidecar exists. A sidecar a human marked as reviewed is kept unless you also pass `--force`.
- `--force` — overwrite a human-reviewed sidecar.
- `--json` — the sidecar plus `stale`, `human_reviewed`, and the current neighborhood hash.

**Gotchas.** Sidecars are local to each checkout and are not shared: they sit under the gitignored `.aida/` folder, never in the requirement store, so a linked worktree keeps its own set and nothing is synced to other clones. The audit is offline unless `AIDA_JEV_API_KEY` is set. When it is set, `explain` sends the spec text to TypeSafe AI (`api.typesafe.ai`) for an advisory score. That call is network egress, has a 5-second deadline, and a failure is recorded as "unavailable", never as a pass. See the environment-variables chapter.

**Chains with** — `aida wiki build` renders every spec's explanation into a browsable local site.

---

### `aida wiki`

<!-- doc-intent: TASK-1470 -->

**One line** — build and browse a local HTML site of every spec with its plain-language explanation and a diagram of its immediate neighbors.

**Mental model.** `aida wiki build` writes static pages to `.aida/wiki` (or `--out`): an index with freshness counts and an epic map, plus one page per spec. `aida wiki serve` serves that folder on `127.0.0.1` only (default port 8420, change it with `--port`; there is no option to bind another address). It builds the site first if needed; point it at another folder with `--dir`.

**Reach for it when** — you want to click around the requirement graph in a browser, or show a stakeholder the project without giving them the CLI.

**Don't reach for it when** — you want to share the site over the network. Copy the static folder somewhere instead. The server is deliberately loopback-only.

**Gotchas.** Diagrams use a copy of mermaid bundled into `aida`, so the pages work offline and load nothing from the internet. Rebuild after the store changes; the pages are a snapshot.

**Chains with** — `aida explain <ID> --audience <role>` to create or refresh the explanation a page shows.

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

### `aida alias`

**One line** — list AIDA's built-in shortcuts, grouped by surface, all in one place.

**Mental model.** AIDA has accreted many shortcuts, each hidden in its own `--help`: the `aida list` status lenses (`open` / `closed`) and status-token shortcuts (`aida list approved` is `--status approved`), the `aida list <lens>` views that rewrite to another command (`queue`, `why`, `human`, `inflight`, `me`, `user:<name>`), and the command aliases (`intake` reaching `assess`, `advisor assess` reaching `assess`, bare `aida agent` defaulting to the launcher). `aida alias` is the single discoverable view of that sugar — every shortcut, its canonical expansion, and a one-line meaning, grouped by surface. Bare `aida alias` behaves like `aida alias list`. Crucially it is **sourced from the resolvers**, not a hand-maintained second copy: the list-lens rows come from the same table `aida list <lens>` resolves against, and tests pin the registry to the live resolvers so the catalog can't drift from the surface it documents.

**Reach for it when** — you half-remember a shortcut, or you want to learn what sugar exists before reaching for the long form. It is the "what can I type instead?" lookup.

**Don't reach for it when** — you want a command's flags (that's `aida <cmd> --help`) or the rationale layer (that's `aida manual <cmd>`). And it lists *built-in* shortcuts only — user-defined aliases are a separate, deferred question.

**Key options (rationale only).**
- `--json` — emit the grouped registry as JSON for machine consumers.

**Chains with** — sits beside `aida help --all` (the full command inventory) and `aida manual` (the when/why) as the third discoverability lens: inventory, rationale, shortcuts.

<!-- doc-intent: trace:STORY-667 -->

---

### `aida record`

**One line** — inspect or prune the durable per-spec **processing record** — the audit trail of *what was done and why*, captured at completion.

**Mental model.** When a spec reaches completion, AIDA can persist a **processing record** on it: a durable note of what the work actually did and the reasoning behind it — distinct from the `history:` array (which logs *field transitions*) and from git linkage (which shows *commits*). The processing record is the *narrative audit* — the "why," captured while the context is fresh. `aida record list` reads it (for one spec, or every spec carrying one); the block also surfaces inside `aida show`. `aida record prune` trims records to save space **without** touching the spec or its history.

**Reach for it when** — you (or a reviewer/auditor) want the *reasoning trail* behind a completed spec — what a drain decided and why — not just the diff. It's the substrate behind the "explain intent, not just surface" goal: a place the *why* lives after the work is done.

**Don't reach for it when** — you want *field-change* history (that's `aida history events`) or the *commits/files* a spec touched (that's `aida show`'s git linkage). Record is the narrative layer above both.

**Gotchas.** `record prune` is **propose-by-default** — it shows what it *would* trim and only writes with `--apply`. So a bare `aida record prune` is safe to run as a preview. Pruning loses the narrative, not the spec or its transition history.

**Chains with** — populated at completion (the audit the governance/intent story leans on); read via `record list` or inline in `aida show`; complements `aida history` (transitions) and `aida digest` (the outward-facing surface-change summary).

---

## Where to go next

You now have every read-only lens: live orientation (`status`), the audit trail (`history`), the narrative write-up (`digest`), the configuration report (`report`), the telemetry surfaces (`usage` / `metrics`), the single-spec drill-down (`why`), and the docs launcher (`user-guide`). The questions they answer route back into the rest of the manual:

- **[Chapter 1 — Getting started](01-getting-started.md)**: `list` / `show` — the graph lenses these reporting views send you to drill into.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: the transitions `history` and `digest` are *reporting on* — where Done, Completed, and Released come from.
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: `backlog` / `burndown` — the survey-the-stuck-set counterparts to single-spec `why`, and the drains that `metrics`/`usage drains` measure.

## Monitor contract

### `aida contract`

Print the versioned JSON field subset promised to read-only external monitor
consumers. See [the monitor contract](../monitor-contract.md) for compatibility
rules, fixtures, and the event-follow example. Use `aida contract --json` in
automation; monitor consumers must not parse human-formatted command output.
