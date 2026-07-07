# Chapter 6 — Roles & sessions

This chapter is about **who is working, and on what** — the machinery that lets AIDA keep a single human, a fleet of agents, and a pile of concurrent worktrees from stepping on each other. It's the least "spec-shaped" chapter in the manual and the most *operational*: roles, sessions, leases, the supervised launcher, and the channels agents use to talk to each other.

> Manual contract reminder: rationale, not flag tables. `aida <cmd> --help` is the source of truth for exact flags and defaults — this manual owns *when and why*.

---

## Mental model for the whole chapter

AIDA has a **seat model**. At any moment a working context is wearing one of a small set of *hats* — **advisor** (the persistent strategic + tactical partner: routes work, captures friction, grooms the queue, signs off architecture), **implementer** (writes the code for one spec on one branch), **reviewer** (reads the *diff* against acceptance criteria and votes merge/rework), and the human **product** owner who disposes drafts and sets direction. These aren't job titles bolted on for flavour — they're *authority boundaries*. Approval and queueing are advisor-gated; review verdicts come from the reviewer seat; the implementer's job is to satisfy a spec's contract, not to re-litigate it. `aida role` is how a shell *declares which hat it's wearing*, and that declaration changes what the rest of the CLI lets it do (queue/approve gating) and shows it (scope filters, role-context injection).

A **session** is a unit of work with a *lease* on a *scope*. When an implementer picks up `<spec-id>`, `aida session start` spins a sibling git worktree on a fresh branch and writes a **lease** — a small file that says "this session owns `<spec-id>`, on this branch, in this worktree, held by this owner." The lease is the collision-avoidance substrate: before two agents both grab the same spec, `aida session leases` answers "who holds what right now?" so the second one routes around it. v1 leases are *advisory* (not yet hard-enforced by `edit`/pickup), but in a multi-agent drain they are the coordination contract that keeps the fan-out from double-driving a spec. Note the deliberate split between `aida session leases` (active *scoped work* — the "what's running" view) and `aida session conversations` (historical *Claude Code conversations* — a different, JSONL-backed concept; once `session list`, now a deprecated alias); reach for `leases` when you mean "who's working."

Around that core sit the **launchers** and the **comms**. `aida agent new` and `aida session new`/`start --launch` spawn an actual Claude/Codex process with the right cwd, env, role, and a startup context snapshot — the supervised way to put an agent on a spec rather than copy-pasting a prompt. And because a fleet of agents needs to coordinate without sharing scrollback, two file-backed channels carry the traffic: **briefs** (`aida brief` — one-directional "here's your next pickup," operator → agent) and the **mailbox** (`aida mailbox` — peer↔peer messaging between agents). The `aida advisor` family is the seam where the *live* advisor session registers itself so any headless advisor pass — the away **watch loop** *and* the in-drain escalation tier — forks its full context when registered (cold-booting only as fallback; see `aida advisor` below). Keep the three substrates distinct as you read: **leases** coordinate *work*, **briefs** route *assignments*, the **mailbox** carries *conversation*.

---

### `aida role`

**One line** — declare which hat this shell is wearing, persistently across sessions.

**Mental model.** A role is a *named, resumable context* — a persona (advisor / implementer / reviewer / …) stored under `.aida/roles/` that survives shell restarts. Entering a role does three load-bearing things: it sets `$AIDA_SESSION_ROLE` (which the queue, approval gates, and statusline read), it applies that role's **scope filter** to your views, and it injects that role's **system-prompt addendum** into a launched Claude's context. The role is *who you are* to the rest of AIDA; everything authority-gated keys off it. Because entering/ending a role must mutate the *calling* shell's environment, those subcommands emit shell code — run them through `eval "$(aida role enter advisor)"` on a raw binary, or just `aida role enter advisor` once the `aida dev shell-init` helper is installed (it auto-evals).

**Reach for it when** — at the start of any working shell: declare your seat so gating and views are correct. `aida role enter` to resume an existing persona, `aida role add <name>` to define a new one. `aida role list` to see what's defined; `aida role active` / `current` are the pure, store-free reads scripts and statuslines use.

**Don't reach for it when** — you want a *one-off* command under a different role: don't enter/exit a role around it — prefix the single command with `AIDA_SESSION_ROLE=<role>` (e.g. for an advisor-gated `aida queue add` from a non-advisor shell). Entering a whole role for one command is heavyweight.

**Key options (rationale only).**
- `enter` / `end` — the env-mutating pair; both *output shell code* and must be `eval`'d (or run bare under the shell helper). This is why they feel different from every other subcommand — they can't change your shell from a child process, so they hand you the code to do it.
- `add` (with `--global`) — define a new role; `--global` stores it under `~/.aida/roles/` for personas you carry across projects (triage, code-review) rather than per-project.
- `scope` (`set` / `show` / `clear`) — attach a standing filter to a role so `aida list` and `aida queue list/next` auto-narrow while you wear that hat (the canonical example: a `triage` role scoped to `--tags inbox --status draft`). Override per-command with explicit `--tags`/`--status` or `--no-scope`. This is *why* a role is more than a label — it reshapes what you see.
- `prompt` (`set` / `show` / `clear`) — the per-role system-prompt addendum injected into Claude at session start via the role-context hook. The seam where a role carries *instructions*, not just identity.
- `scaffold` — install the starter `implementer` / `advisor` / `reviewer` set globally; idempotent, safe to re-run. The one-time "give me the standard taxonomy."
- `active` vs `current` — both print the active role, but with different exit-code contracts (`active` exits 1 with empty stdout when none; `current` exits 0 either way, `--check` flips it to 1). Pick by whether your script wants "fail if no role" or "tell me, never fail."
- `repair` — quarantine unparseable activity-log lines and rewrite a corrupted role file cleanly. The recovery hatch for a role whose log got mangled.

**Gotchas.** `advisor` is the canonical role name everywhere (config, `$AIDA_SESSION_ROLE`, queue routing, statusline); `dialog` is a deprecated, silently-accepted alias normalized to `advisor` — so an un-migrated machine keeps working, but write `advisor` in anything new. And remember the whole reason `enter`/`end` emit shell code: forgetting the `eval` (on a raw binary) means the role *looks* like it switched but your env never changed.

**Chains with** — set your role first thing in a shell; the queue ([Chapter 3](03-work-autonomy.md)) and approval/queue gating read it, and `aida session new` / `agent new` propagate it to the launched agent.

---

### `aida session`

**One line** — work sessions and the worktree *leases* that stop two agents colliding on one spec.

**Mental model.** This command spans two genuinely different concepts that share a name — internalize the split or it confuses. (1) **Scoped sessions + leases** (`start` / `end` / `leases` / `show` / `manifest`): a *lease* is a claim on a logical or physical scope (an EPIC, a SPEC-ID, a path glob), backed by a sibling git worktree on a fresh branch. `aida session leases` is the canonical "who holds what scoped work right now" view. (2) **Historical Claude Code conversations** (`conversations` / `resume` / `new` / `prune` / `forget`): a wrapper over `claude --resume`, enriched with the role + spec each.jsonl session was about. The first concept is *work coordination*; the second is *conversation management*. When you want "what's running?", reach for `leases`, **not** `conversations`. (`conversations` was once called `session list`; the old name is kept as a deprecated alias, so muscle-memory `session list` still works.)

**Reach for it when**
- `aida session start --owns <scope>` — beginning isolated work on a spec/epic/path: it creates the worktree, symlinks AIDA state, and writes the lease in one step. `--launch` collapses the usual "start → cd → session new" into a single command.
- `aida session leases` — the standing "is anyone already on this?" check before you (or an agent) claim a spec; the multi-agent drain's collision guard.
- `aida session end` — finishing isolated work: removes the worktree and lease, *leaves the branch alone* (merge/discard is your call), and as a safety net files a reviewer item if the branch has an open PR.
- `aida session conversations` / `resume` — finding and re-entering a *past conversation* by its role+spec context instead of grepping Claude's auto-generated subjects. (`conversations` is the renamed `session list`; the old verb is a deprecated alias.)

**Don't reach for it when** — you want the "who's working" view but typed `conversations` (that's historical conversations — use `leases`). And don't `start` a scoped worktree for trivial in-place work; the worktree-per-scope model pays off for *parallel* or *long-running* work, not a two-line fix on your current branch.

**Key options (rationale only).**
- `start --owns <scope>` (alias `--spec`) — the scope is the lease's identity: an EPIC, any SPEC-ID, a `path/**` glob, or a free-form `feature:auth` tag. Path globs are *stored, not validated* — they're advisory coordination hints.
- `start --reuse-branch` — check out an existing branch instead of forking fresh: the fixup-on-an-existing-PR-branch flow. Without it, an explicitly-named existing `--branch` is reused automatically and an auto-derived name always forks.
- `start --launch` (+ `--title` / `--name` / `--role` / `--permission-mode` / `--sandbox`) — create the worktree *and* exec Claude inside it. The role is **derived from the scope** by default (`PR-N`/`MR-N` → reviewer, everything else → implementer); `--role` overrides, but when it disagrees with the scope the scope wins with a warning — because the scope is the more reliable signal of what the work *is*.
- `start --force-claim` — claim a spec in an *ambiguous* state (In-Progress with no local lease, or NeedsAttention awaiting triage). It deliberately does **not** override Done/Completed/Rejected/Draft — those always refuse. The escape hatch for "yes, I'm taking over this stuck claim."
- `end --force` — two effects in one flag: force-terminate live `claude` processes in the worktree *and* discard uncommitted changes. The refusal-without-it is the guard against an orphaned-claude-with-dangling-cwd leak and against silently nuking dirty work.
- `end --wait-ci` vs `--watch-ci` — both block until the PR's CI is terminal before releasing the lease; `--wait-ci` is the *silent* poll (overnight builds), `--watch-ci` is the *live* stream (interactive). Same decision tree once CI resolves; pick by how much screen noise you want.
- `new --permission-mode` — the faithful-launcher knob: omit it and Claude uses its native posture (prompts); pass `bypassPermissions`/`acceptEdits`/`auto`/etc. to opt in. Fleet-wide bypass is `[agents] bypass = true` in `agents.toml`, not a baked-in default.
- `[agents] vendor = "codex"` (agents.toml, user-global `~/.aida/agents.toml` base overridable by project `.aida/agents.toml`) — the set-once default-vendor knob for codex-first/codex-mandated machines: every launch surface (interactive `queue work` host, orchestrator headless phases, TUI tabs, the `queue add --work` chain) falls back to it when no per-surface flag/env/config picks a vendor. Per-surface knobs keep priority.
- `leases --verbose` / `--all` — `-v` probes live claude PIDs (and flags `(deleted)`-cwd zombies); `--all` includes stale leases with a state column. The default view hides stale leases to stay signal-dense.
- `manifest` — record which specs a session *intends* to work, so other commands can surface "planned by another session" cues. Coordination *before* the lease, for a planned cluster.
- `prune` vs `forget` — `prune` is bulk disk-cleanup of old session.jsonl files by age (and skips dirs with active leases, defensively); `forget` is single-target display-management. Bulk-by-age vs one-by-id.

**Gotchas.** The `leases` ≠ `conversations` split is the one that bites: `aida session conversations` (formerly `session list`) will *not* show the scoped leases that `aida session start` created — they're different substrates. `aida session show` defaults to the lease covering your cwd (or matched by ancestor PID), so running it from inside a worktree "just works." And `end` leaves the branch untouched by design — ending a session is *not* merging or discarding the work.

**Chains with** — `aida queue work` ([Chapter 3](03-work-autonomy.md)) leases under the hood; `aida session start --launch` is the manual equivalent. After review + merge, `aida session end` tears the worktree down. Cross-reference `aida agent new` for the multi-process launcher.

---

### `aida agent`

**One line** — launch and track real agent processes (Claude / Codex / Antigravity) on a spec.

**Mental model.** `aida agent new <tool>` spawns a *one-shot* agent with project-correct cwd/env, a role, registry tracking, and a startup **context snapshot** — the supervised way to put an agent on work instead of opening a terminal and pasting a prompt. The critical framing in its own help: this lane *does its work, ships a PR, and exits* — it is **not** the orchestrated pipeline. It does not run CI, the reviewer phase, or the merge. For a supervised end-to-end drain (implementer → CI → reviewer → merge → pull), that's `aida queue work <SPEC> --auto-complete` ([Chapter 3](03-work-autonomy.md)), not this.

**Reach for it when** — you want to *dispatch* a bounded agent run: `aida agent new claude --role implementer --spec <spec-id>` spins the worktree+lease and launches Claude already pointed at the work. `aida agent ls` (alias `status`) to see what's running; `pause` / `resume` to mark an agent budget-exhausted/rate-limited so brief-time dispatch and `aida status` flag it (without stopping the process).

**Don't reach for it when** — you want the full lifecycle driven for you (CI, review, merge) — that's `aida queue work --auto-complete`, not `agent new`. And `pause` does **not** stop a process — it's a *status marker* for dispatch logic; use `aida agent stop` to actually terminate one.

**Key options (rationale only).**
- `new <claude|codex|antigravity>` — the tool is a subcommand, not a flag, because each agent has a slightly different spawn shape. `list-roles` enumerates the supported role taxonomy.
- `new claude --spec <ID>` — creates a scoped worktree + lease *before* launch, so the agent starts already isolated on its spec.
- `new claude --show-context` / `--no-context` — print, or suppress, the launch-context snapshot. The snapshot is a *startup* picture only; a launched agent must keep polling briefs/MCP for work filed after launch.
- `new claude --bg` — detach to Claude Code's background supervisor (`claude --bg`); the session shows up in `claude agents` and `aida status`, and with `--spec` the captured sessionId is recorded on the lease so the cross-substrate view links them.
- `new claude --permission-mode` / `--sandbox` — the same faithful-launcher posture as `session new`: omit for native (prompts), opt in explicitly, or `--sandbox` for contained mode.
- `pause` / `resume` — budget/rate-limit signalling for the fleet, distinct from `stop` (which terminates).

**Gotchas.** The startup context snapshot is *not* live — it's a point-in-time brief; an agent that only reads it will miss anything filed after it launched. The "one-shot, ships-a-PR-and-exits" lane is easy to mistake for the orchestrated drain — they are different tools with different guarantees.

**Chains with** — `aida brief` files the assignment, `aida agent new` launches the agent to pick it up; `aida session leases` / `aida agent ls` show the result running. For the full reviewed pipeline instead, `aida queue work --auto-complete`.

---

### `aida advisor`

**One line** — register the live advisor session, and run its presence-gated maintenance loops.

**Mental model.** The advisor seat is the persistent live Claude session wearing the advisor hat (by convention, one per project today); `aida advisor` is the seam that lets the rest of the system *find* it and run its maintenance loops. `aida advisor register` writes `~/.aida/advisor.toml` recording the current Claude session as the live advisor — and that one fact changes how *every* headless advisor pass boots:

- **Registered** → the pass **forks from your live session** (copy-then-resume its transcript) so it inherits your *full in-flight context* — ~$0.03 warm. This is the good path, and it applies to **both** the `--no-human=both` *in-drain escalation tier* (when an implementer punts mid-drain) **and** the `aida advisor watch` away-loop.
- **Not registered** → the pass **cold-boots** a fresh headless advisor: same model, but only the *persistent* substrate (memory, CLAUDE.md, discipline, spec graph), none of your conversation. Cold-boot is the *fallback*, not the default.

So registration is what turns "a stranger handles the punt" into "your advisor, with everything it knows, handles it" — across the board, in-drain and away alike. Beyond registration, the family carries the advisor's *operational* loops: a situational dashboard, the watch loop, and recurring scheduled tasks.

**Reach for it when**
- `aida advisor register` — at the start of a live advisor session, so headless escalations can fork *you* instead of a cold boot. `unregister` to clear it (idempotent).
- `aida advisor status` — the advisor's read-only dashboard: one screen of counts (intake drafts, pending decisions, findings, backlog, queue depth, live sessions), each row pointing at the command to act on it. The "what needs my attention?" entry point for the advisor seat.
- `aida advisor watch` — while you're `away`, periodically fork the live session and run a bounded garden + mailbox-triage + escalate pass headless. The "keep gardening while I'm gone, but only safe work" loop.
- `aida advisor schedule` — register recurring maintenance/research tasks that land in the queue on a cadence (no daemon — they fire on `aida pull`).

**Don't reach for it when** — you want a *fresh* headless advisor regardless of context: just don't register (the orchestrator cold-boots when no live advisor is on file). And `watch` is opt-in by invocation and presence-gated — don't run it expecting it to act aggressively; `--triage-only` makes the fork *surface* mailbox items without acting on them.

**Key options (rationale only).**
- `register --uuid` — defaults to `$CLAUDE_CODE_SESSION_ID` (Claude sets it every session), so the bare `register` is usually right; pass `--uuid` only to register a *specific* session id.
- `status --json` / `--registration` — `--json` for machine consumers; `--registration` narrows to just the live-advisor block (back-compat for muscle memory and scripts that predate the full dashboard).
- `watch --dry-run` / `--once` / `--triage-only` — `--dry-run` previews each tick's decision *and the estimated fork cost* without forking (a warm fork is ~$0.03 but accrues over a long away window, so the preview matters); `--once` is the cron-friendly single tick; `--triage-only` is the conservative "surface, don't act" mode. `--fork-interval` / `--poll-interval` tune cost vs responsiveness.
- `schedule add` / `list` / `enable` / `disable` / `remove` / `run` — the no-daemon scheduler: due schedules fire on every `aida pull`; `run` force-fires now (the same logic `pull` uses).
- `handoff --to <project> --focus <topic>` — generate a checked-in advisor handoff brief for a *sibling* project: a dated Markdown template (parent identity auto-filled, strategic sections as placeholders the operator authors). The cross-project knowledge-transfer surface.

**Gotchas.** Registration is the switch, and it governs *both* headless paths the same way: with a live advisor registered, the `--no-human=both` in-drain escalation tier *and* `aida advisor watch` both **fork** from your context; with none registered, both **cold-boot** a stranger. An unregistered advisor session silently gets the worse path everywhere — so `aida advisor register` at the start of a live advisor session is the cheap, high-leverage habit. The watch forks aren't free (~$0.03 warm each over the away window) — `--dry-run` previews the cost. (The "two verdicts fire" double-run is *calibration mode* only, not normal operation.)

**Chains with** — `aida advisor register` pairs with the headless drain ([Chapter 3](03-work-autonomy.md), `--no-human=both`); `aida advisor watch` is gated by the away/home presence verbs (Chapter 3); `aida advisor status` is the advisor seat's daily entry point alongside `aida status` ([Chapter 8](08-reporting.md)).

---

### `aida brief`

**One line** — route a pickup assignment to a specific agent, without sharing scrollback.

**Mental model.** A brief is a *one-directional* "here's your next thing to work" note, written by the operator (or advisor) to a named agent, landing as a file under `.aida/agent-briefs/<agent>/`. It exists because a fleet of agents can't share one terminal's scrollback — the brief is how an assignment reaches an agent that wasn't in the room when you decided it. The agent reads it (`read`), works it, and `ack`s it so it stops re-appearing. Briefs are *runtime, per-clone* state; the equivalent MCP tools (`list_briefs` / `read_brief` / `ack_brief`) let MCP-speaking agents do the same without the CLI.

**Reach for it when** — assigning bounded work to a specific agent (`aida brief codex <spec-id> --note "..."`); checking what's pending for an agent (`aida brief list --for-agent codex`); an agent reading or acknowledging its queue.

**Don't reach for it when** — you want a *conversation* (two-way, threaded) — that's `aida mailbox`, not a brief. A brief is fire-and-forget assignment; the mailbox is dialogue. And don't rely on a brief to *interrupt* an idle agent unless you mark it `--notify` (otherwise it's FYI-only and waits to be polled).

**Key options (rationale only).**
- `--note` — operator context for the pickup; `-` reads a multi-line note from stdin (for anything past a one-liner).
- `--notify` — mark the brief *urgent*: writes a `.pending` sentinel so an idle agent's `aida status` / statusline surfaces it without a heartbeat. Omit it for FYI briefs that shouldn't interrupt — the flag *is* the interrupt/no-interrupt choice.
- `--depends-on <SPEC-ID>` — gate the brief behind another spec landing first; the ordering hint for chained assignments.
- `--as-deep-link` — print a `claude-cli://open` deep link so a click opens Claude Code in the spec's worktree with the pickup prompt pre-loaded, eliminating the paste step (needs a recent Claude Code for the URL scheme).
- `list --for-agent` / `--include-acked` — narrow to one agent, or include already-acked briefs (for audit; the default hides them).

**Gotchas.** A brief without `--notify` is *passive* — it sits until the agent polls; if you expected an idle agent to wake on it, you wanted `--notify`. Briefs are per-clone runtime state, not synced specs — they live in the working tree, not the orphan store.

**Chains with** — file a brief, then `aida agent new` launches (or an already-running agent polls) to pick it up; the agent `ack`s it on completion. For two-way coordination instead, `aida mailbox`.

---

### `aida mailbox`

**One line** — peer↔peer messaging between agents (send / inbox / thread), durable across clones.

**Mental model.** Where a *brief* is operator→agent assignment, the **mailbox** is agent↔agent *conversation* — threaded messages addressed to a peer (or broadcast to all), with an inbox, threads, retract/delete, and a git-canonical sync. It's the channel a fleet uses to say "I'm taking the auth specs, you take storage" or "heads up, my PR touches your file." Two layers: a fast *local* layer for the working session, and `aida mailbox sync` which digests that local layer into the durable orphan-branch store so messages are replayable and shareable across clones. The operator views (`list` / `inbox --all`) give a fleet-wide read without being a participant.

**Reach for it when** — agents coordinating directly: `aida mailbox send --to codex "..."` (or `--broadcast`), `aida mailbox inbox` to read (which marks it seen), `aida mailbox thread` to see a full conversation. The operator's `aida mailbox list` for "who has mail waiting, who has unread/urgent."

**Don't reach for it when** — you're *assigning* work rather than discussing it — that's a `aida brief` (one-directional, with `--notify` for urgency). And don't expect a message to survive a fresh clone until you `aida mailbox sync` — the local layer is per-clone until digested into the store.

**Key options (rationale only).**
- `send --to` vs `--broadcast` — single recipient or everyone; mutually the two addressing modes (omit `--to` and pass `--broadcast` to reach all).
- `send --thread` / `--in-reply-to` — attach to an existing conversation rather than starting a new thread; how a back-and-forth stays grouped.
- `send --urgent` — surface out-of-band (statusline nag) instead of sitting unseen in a chronological inbox. Lightweight: normal-vs-urgent only, the same interrupt/no-interrupt choice `brief --notify` makes.
- `send --from` — override the sender id (default is this shell's agent/user identity); for when you're sending on another identity's behalf.
- `inbox --all` — the operator-wide read-only view across *every* agent, distinct from a single agent's `inbox` (which marks that agent's mail seen).
- `sync` — digest local → git-canonical store and commit on the orphan branch; idempotent. The durability/shareability step; until you run it, messages are local-only.
- `retract` vs `delete` — `retract` leaves a withdrawn *tombstone* visible in views (you take back what you said, transparently); `delete` removes it with a marker so sync won't resurrect it. Different intents: walk-it-back vs erase.

**Gotchas.** Reading an `inbox` *marks it seen* — that's a side effect, so use `inbox --all` (read-only, operator view) when you want to peek without clearing unread state. And the local-vs-synced split surprises people: a message is durable and cross-clone only *after* `aida mailbox sync`.

**Chains with** — agents launched via `aida agent new` coordinate over the mailbox; the advisor's `watch` loop runs a mailbox-triage pass; briefs route *assignments*, the mailbox carries the *conversation* around them.

---

## Where to go next

You now own the seat model and the coordination substrates: who's wearing which hat, who holds which scoped work, and how agents are launched and talk to each other. Next:
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: the queue, pickup, and the autonomous drain — where leases get taken and the away/home presence verbs that gate `advisor watch`.
- **[Chapter 7 — Project setup](07-project-setup.md)**: `node` (per-clone identity, the writer-of-record this chapter's leases and mailbox sync assume) and the rest of the machine-global setup.
- **[Chapter 8 — Reporting](08-reporting.md)**: `aida status` — the unified "what's going on here?" view that the advisor dashboard complements.
- [`docs/review-process.md`](../review-process.md) — **who reviews by mode**, and the precise reviewer-vs-advisor-tier distinction (the reviewer gates code in every mode; the cold-boot advisor tier fires only on a punt). The authoritative source for the seat/review lore in this chapter.
- The autonomy + escalation architecture in full — the three-mode ladder, the implementer→advisor→human cascade, the file-based handshake substrate — is [`docs/architecture/autonomy-and-escalation.md`](../architecture/autonomy-and-escalation.md).
