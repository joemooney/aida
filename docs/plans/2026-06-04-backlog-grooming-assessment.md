# Backlog grooming assessment — Approved + Planned (for master)

- **Date:** 2026-06-04  ·  **Author:** product agent  ·  **Status:** ASSESSMENT — recommendations only, NO state changed. Master adjudicates every move.

## The volume reality

278 Approved + 3 Planned. But by priority, the near-term backlog is small and the tail is large:

| Priority | Approved | Read as |
|---|---|---|
| High | 33 | genuine near-term |
| Medium | 94 | real backlog, sequence later |
| Low | 151 | **the cleanup target — archive-and-revisit** |

**Headline:** ~87 items (High+Medium) are the real working backlog. The other ~190 are low-priority tail, captured observations, or strategic items needing a decision. Aggressively archiving the Low tail is the single biggest volume win.

## Recommended disposition (heuristic buckets)

| Bucket | Count | Recommended action |
|---|---|---|
| 🟢 Low-hanging fruit | 24 | **Queue now** — bounded papercut/cli-ux/trivial fixes |
| 📦 Captured observations | 21 | **Archive** — dogfood notes / observations; revisit on recurrence |
| 🧭 Strategic / epics | 23 | **Master decides** — epic-shaped, strategic, research dump |
| 📋 Real backlog (High/Med) | 87 | Keep; sequence by priority |
| 🗄️ Low-priority tail (in backlog) | 123 | **Bulk-archive candidate** — revisit later |

> Heuristics: tags (`papercut`/`lifecycle:trivial`/`cli-ux` → queue; `kind:observation`/`from-self-test`/`from-validation` → archive; `strategic`/Epic/research-batch → decide) + type + priority. Titles spot-checked. These are *suggestions*; the master makes the call per item or per bucket.

## 🟢 Low-hanging fruit — recommend QUEUE (24)

- **BUG-412** (Low) — extract_referenced_spec_ids_from_commit pulls code-like (PREFIX-NNN) from commit bodies as false references
- **TASK-312** (Low) — Amend TASK-265 acceptance criteria to match what PR-69 shipped
- **TASK-313** (Low) — Optionally render the plan brief inside the aida show --card box
- **TASK-317** (Low) — aida queue move --to <N> slot numbering can diverge from role-filtered 'aida queue list'
- **TASK-320** (Low) — Widen test_skill_template_glyphs to guard all embedded skill templates
- **TASK-321** (Low) — TASK-292 acceptance criterion 5 lists --no-launch, but --auto-complete clap-conflicts with it
- **TASK-322** (Low) — aida queue work next3 --batch NAME silently swallows the next3 keyword
- **TASK-383** (Low) — Planning-pass discipline: don't leave untracked plan files in main's docs/plans/ (they conflict with later PR merges)
- **TASK-394** (Medium) — Persistent --no-human=both acknowledgment via file marker (eliminate per-loop env-var dance)
- **TASK-395** (Low) — Stream / follow headless drain output (aida logs tail) — pretty rather than raw JSONL
- **TASK-402** (Medium) — aida queue work --resume: resume-after-failure path UX cleanup (4-friction cluster)
- **TASK-405** (Medium) — aida queue work: support PR-only invocation (drive phases 3-6 when implementation shipped outside the orchestrator)
- **TASK-415** (Medium) — aida-pickup.md + aida-pr.md: replace hand-written State preamble blocks with aida state-snapshot calls
- **TASK-463** (Low) — Commit message validator: support [AI:tool1+tool2] multi-author AI tags for cross-agent commits
- **TASK-465** (Low) — Refactor scaffolding-pack tests away from hardcoded counts toward structural assertions
- **TASK-467** (Low) — global_auto_claim_summary should honour per-type auto_claim re-enable in opt-out case
- **TASK-475** (Low) — aida queue list: warn when local orphan store is behind origin/aida-store
- **TASK-494** (Low) — aida push: suppress merged-branch + stale-base warnings when code-leg has nothing to push
- **TASK-502** (Medium) — aida brief --notify: sentinel-file mechanism so idle agent chats surface pending briefs without heartbeat
- **TASK-507** (Medium) — aida history --shipped — recent Done→Completed transitions (the missing 'did my ship register?' view)
- **TASK-527** (Medium) — aida list --tags: support prefix-glob filtering (aida:queue:* → all subcommand tags under that surface)
- **TASK-625** (Low) — gitignore .claude/scheduled_tasks.lock — scheduler runtime lock leaks as untracked file
- **TASK-626** (Low) — aida agent new --spec: signpost that this lane ships a PR + exits (not orchestrated), name the queue-work alternative
- **TASK-636** (Medium) — Trim scaffolded .claude/AIDA.md (prose→pointer) to cut per-session context on consumer projects

## 📦 Captured observations — recommend ARCHIVE & revisit (21)

- **BUG-418** (Low) — aida db reconcile-status: real run printed 'No eligible flips' yet the spec ended Completed (misleading output or state confusion)
- **BUG-432** (Low) — aida queue work hard-fails at startup db-sync-pull when the project has no 'origin' remote
- **BUG-433** (Low) — Worktree with .aida-store symlink but no committed .aida/config.toml → plain aida falls to legacy backend + shows WRONG data (inverse of BUG-428)
- **STORY-477** (Medium) — Agent-lift metrics report for AIDA dogfood proof
- **TASK-128** (Medium) — Scaffold commits emit 'Staged files trace to:' warnings listing FOREIGN trace I…
- **TASK-140** (Medium) — aida pr ship squash-subject mismatch: PR-347 (TASK-575) merged with the WRONG s…
- **TASK-253** (Medium) — Add .mcp.json to AIDA dev repo so Claude Code dogfoods aida mcp-serve in our own workflow
- **TASK-456** (Low) — Discipline pack: add 'recursive-failure-risk fixes use keyboard, not drain' section to workflow-patterns.md
- **TASK-468** (Low) — Integration test: full --auto-complete --no-human cycle hitting stale-base block
- **TASK-472** (Medium) — Clarify in cross-agent-onboarding.md: file_finding is for triageable bugs/tasks, NOT session checkpoints
- **TASK-473** (Low) — Punt-resolve UX: add verdict for 'reroute to operator' (advisor says spec is user-only, not implementer-pickup)
- **TASK-480** (Medium) — /aida-review: detect intermediate-only diffs (build artifacts, generated code) and refuse
- **TASK-513** (Low) — aida burn-credits / /aida-credit-burn: suggest tonight's ultrareview + ultraplan picks given a credit budget
- **TASK-516** (Medium) — aida import-plan --request-review: master-review handshake before plan is treated as canonical
- **TASK-539** (Medium) — aida status: surface pending findings tally + per-finding summary (STORY-385 follow-up)
- **TASK-581** (Low) — Codex .codex/skills/<name>/ helper-file parity for folder-form skills
- **TASK-582** (Low) — Migrate more flat skills to folder-form where a template/example helps
- **TASK-593** (Low) — aida queue prune --orphaned misses review-queue rows for already-merged PRs
- **TASK-618** (Medium) — Distributed queue: same user_id on two machines edits one registry/queues/<user>.yaml → orphan-branch conflict
- **TASK-619** (Medium) — Cross-machine duplicate work: leases are local (.aida/sessions), so only the eventually-consistent status flip guards against two people grabbing the same spec
- **TASK-624** (Low) — Verify: re-running 'aida init' on an attached clone (TASK-623 path) re-prints 'Enqueued onboarding task' — does it duplicate the queue entry?

## 🧭 Strategic / epic-shaped — MASTER DECISION (23)

- **EPIC-22** (Medium) — Cross-project AIDA primitives: registry, cross-project queue, cross-project relationships
- **EPIC-24** (Medium) — Living documentation: capture rationale, use cases, recipes, and tutorial seeds DURING work — book-as-living-document
- **EPIC-25** (Medium) — Release lifecycle as composite: documenter role + queueable release with auto-decomposed subtasks
- **EPIC-27** (High) — MCP server modernization: mirror the full AIDA CLI surface (queue, session, role, workflow tools)
- **EPIC-28** (High) — Dependency-aware autonomous drain: shelve failures, skip dependents, continue independents (full --auto-complete batch)
- **EPIC-29** (High) — Split aida-tui into a companion project — separate repo, depends on aida-core's public API
- **EPIC-31** (High) — Agent registry + launcher + state tracking — AIDA as agent-lifecycle layer
- **EPIC-33** (High) — Orchestrator correctness floor — the invariants the autonomous drain must hold to be trustworthy (escape the can't-fix-itself recursion)
- **EPIC-34** (High) — Deterministic trace-coverage merge gate — provenance as constraint, not convention
- **EPIC-35** (High) — GitLab as a first-class forge — forge-provider abstraction for the PR/CI/drain lifecycle
- **SPIKE-14** (High) — Claude Code 2.1.154 dynamic workflows — mechanism + workflow definition format + AIDA composition path
- **SPIKE-15** (High) — Claude Code agent view + agent dispatch surface — registry primitive + API exposure
- **SPIKE-16** (High) — Claude Code skills system — frontmatter schema, disable-model-invocation, helper subfolders, /reload-skills
- **SPIKE-17** (High) — Claude Code hooks lifecycle — full event taxonomy + return-value schema
- **SPIKE-18** (Medium) — Claude Code MCP evolution — alwaysLoad, hooks-invoke-MCP, .mcp.json persistence, agent-level mcpServers
- **SPIKE-19** (Medium) — Claude Code background sessions — state persistence, pinned vs unpinned, idle eviction, /resume
- **SPIKE-20** (Medium) — Claude Code worktrees — baseRef, bgIsolation, EnterWorktree, agent-isolation
- **SPIKE-21** (Medium) — Claude Code /goal + completion tracking — overlay panel, composition with workflows + auto mode
- **SPIKE-22** (Medium) — Claude Code permissions / auto mode — autoMode.hard_deny, parentSettingsBehavior, agent permissionMode
- **SPIKE-23** (Low) — Claude Code plugins + marketplace — dependency cascades, manifests, distribution layer
- **SPIKE-29** (High) — Claude Code agent teams (EXPERIMENTAL) — a third orchestration surface beyond subagents + workflows
- **STORY-416** (High) — Cross-agent scaffolding: maximize universal gates, scaffold per-agent docs into new projects
- **STORY-425** (High) — Multi-agent communication model: replace cut-n-paste briefs with substrate-resident agent routing

## 🗄️ Low-priority tail within backlog — BULK-ARCHIVE candidate (123)

The biggest volume lever. These are real-but-low-priority — archiving them to revisit-later cuts the Approved count by ~44%. Master can bulk-archive by criterion (e.g. `aida archive --older-than 30d --dry-run`, default csv completed,rejected — would need a low-priority filter) or review the list below. Full list:

- **BUG-59** (Low) — Stop hook ENOENT when cwd was a removed worktree
- **SPIKE-12** (Low) — SPIKE: pack-scale hygiene threshold — at what memory-pack size does subsystem-scoping become mandatory?
- **SPIKE-42** (Low) — Compose AIDA lease snapshot with Claude Code checkpointing for state-aware /rewind
- **STORY-364** (Low) — aida advisor mentor --child <project> ongoing-relationship verb (SPIKE-10 Track B)
- **STORY-365** (Low) — Bidirectional substrate propagation — propagation:candidate-for-parent tag + promote/decline verbs (SPIKE-10 Track B)
- **STORY-374** (Low) — aida-worker: 'watch batch:NAME' directive that re-issues the drain on completion until file is edited
- **STORY-377** (Low) — Migrate aida-worker shell function to 'aida worker run' Rust subcommand
- **STORY-405** (Low) — aida status: surface live advisor / external state-affecting activity (raw shell recoveries, manual merges, lease cleanups)
- **STORY-450** (Low) — Optional --auto-complete phase 7 — patch-release after each successful spec
- **STORY-471** (Low) — Agent performance metrics + comparative analysis: track on-track-vs-off-track per agent type/instance for substrate-grounded dispatch decisions
- **STORY-51** (Low) — Source-wide pass: update or remove dangling trace comments after EPIC-21 v2
- **STORY-64** (Low) — aida role current: print the active role's name
- **STORY-74** (Low) — Project registry foundation: aida projects register/list/forget
- **STORY-75** (Low) — Cross-project queue routing: aida queue add --to-project
- **STORY-76** (Low) — Cross-project relationships: rel add <project>:<id>
- **STORY-77** (Low) — Cross-project search: aida search --all-projects
- **STORY-94** (Low) — Auto-spawn reviewer session at PR-create-time (alternative trigger to STORY-66 / STORY-90 queue-only)
- **STORY-95** (Low) — Multi-session scheduling: resource caps, priority ordering, idle-detection
- **STORY-96** (Low) — Decision policy: 'what next?' autonomy for auto-spawn trigger
- **STORY-99** (Low) — Workflow dashboard TUI: human-in-the-loop UI for orchestration transitions
- **TASK-101** (Low) — aida statusline + queue list: surface 'base behind by N' for active leases on stale branches
- **TASK-109** (Low) — docs/git-verb-surface.md: design reference naming the convention
- **TASK-230** (Low) — Pipelined ping-pong v2: while blocked on PR-N review, optionally pick up next queue item in secondary worktree
- **TASK-235** (Low) — Backlog: verify 'covers no specs' on auto-queued reviewer story after BUG-85 fix + cluster-mode-drain stabilizes
- **TASK-239** (Low) — Skill next-steps blocks: ASCII fallback for unicode glyphs (avoid terminal interleave corruption)
- **TASK-240** (Low) — aida queue work status-aware error (TASK-217): Done case should suggest 'gh pr merge' when user is PR author
- **TASK-251** (Low) — TUI: surface a notice when a requested scope isn't hosted because crash-recovery filled MAX_TABS
- **TASK-252** (Low) — TUI: overlay session-id truncation byte-slices &s.id[..12] — panic risk on a non-ASCII id
- **TASK-258** (Low) — aida-review skill step 10 documents nonexistent 'aida queue remove --yes' flag
- **TASK-261** (Low) — Wire or drop the unused AIDA_AUTO_COMPLETE env var
- **TASK-271** (Low) — Tighten BUG-114 genuine-miss error message: candidate ids vs --resume semantics
- **TASK-295** (Low) — End-to-end batch drain against a live 3-spec batch (real Claude sessions) — the unit tests mock the driver; a smoke run would exercise `RealBatchDriver`
- **TASK-300** (Low) — STORY-276 — headless implementer phase (`--no-human=both` wires phase 1)
- **TASK-301** (Low) — TASK-297 — skill-template audit + `AIDA_NO_HUMAN` env var
- **TASK-302** (Low) — TASK-298 — stream-json watchdog: parse `permission_denials` / `is_error`
- **TASK-314** (Low) — Retrofit the gRPC `client.rs` status display onto `status_display` once the
- **TASK-315** (Low) — Consider widening `aida list`'s Status column and adding the glyph there too
- **TASK-316** (Low) — Grow the manifest `items` list as `/aida-pickup --batch` picks up each member
- **TASK-323** (Low) — `--no-human` design-fork punt behavior — auto-resolve design-forks under
- **TASK-324** (Low) — Skill-template lint — warn when an `AskUserQuestion`-style prompt lacks a
- **TASK-325** (Low) — Propagate `kind:` annotations to skills beyond the core 4
- **TASK-326** (Low) — JSONL logging of auto-resolve decisions (`auto-resolved: kind=X`) — depends
- **TASK-330** (Low) — Stamp the Claude session ID onto comments so `aida session list` can
- **TASK-332** (Low) — (Filed by advisor — STORY-255 comment additions: skills/local/, `.local.md`
- **TASK-334** (Low) — Audit whether `AIDA_ZEN` has the same can't-verify-provenance weakness
- **TASK-335** (Low) — When STORY-301's drain-state file lands, fold the run-UUID into it and retire
- **TASK-341** (Low) — Audit `AIDA_EXIT_SENTINEL` / `AIDA_REVIEW_VERDICT_FILE` for explicit
- **TASK-342** (Low) — Update `docs/autonomous-drain.md` zen-mode section to mention `aida zen
- **TASK-343** (Low) — `aida drain status` could show PR merge-state (`gh pr view`) per member
- **TASK-344** (Low) — A `drain:NAME N/M` statusline segment sourced from `drain-state.json` (TASK-306)
- **TASK-345** (Low) — `aida session leases --json` could include the drain cross-reference
- **TASK-349** (Low) — A `blocked-dependency` punt should auto-suggest filing a blocked-by relationship
- **TASK-352** (Low) — TASK-298 watchdog: parse headless `permission_denials` as the backstop for an
- **TASK-353** (Low) — Robust session-worktree cleanup for a punted spec (phase-2 `aida session end`
- **TASK-354** (Low) — TASK-297: audit the remaining skill templates for headless-aware punt/auto paths
- **TASK-355** (Low) — `--max-budget-usd` / `--model` cost knobs for the headless drain (SPIKE-7)
- **TASK-356** (Low) — STORY-287: fold `--no-human=both`'s design-fork→punt into the three-mode taxonomy
- **TASK-357** (Low) — `docs/aida-discipline/skill-prompt-kinds.md`: document the headless punt rule
- **TASK-360** (Low) — Opt-in heartbeat: worker touches `.aida/worker.heartbeat`, kills a child whose mtime goes stale
- **TASK-361** (Low) — `aida worker` controls beyond `directives` — e.g. `aida worker pause` / `aida worker stop` as typed wrappers over editing `worker.cmd`
- **TASK-362** (Low) — Promote EPIC-30 if the file-directive channel hits real multi-user / multi-machine limits
- **TASK-363** (Low) — Raise or auto-tune `AIDA_WORKER_SPEC_TIMEOUT` once headless `--auto-complete` durations (incl. CI waits) are measured
- **TASK-365** (Low) — Convert remaining prose "(requires X)" / "blocked on X" comments across the spec base into typed `blocked-by` edges
- **TASK-366** (Low) — `aida queue list --tree` annotation for `blocked-by` edges in the tree view
- **TASK-367** (Low) — Statusline indicator when a session is on a `human-only` spec (shouldn't happen — but cheap defensive UX)
- **TASK-368** (Low) — `aida punt --category blocked-dependency` could auto-offer to file the `blocked-by` rel as part of the punt flow (closes the PuntCategory::BlockedDependency loop noted in `aida-core/src/models.rs:62`)
- **TASK-369** (Low) — Soft "should come after" dependencies as a separate typed rel — out-of-scope per AC, but the obvious next layer
- **TASK-370** (Low) — `/aida-daily-digest` cron sibling skill that runs `aida digest` on a cadence
- **TASK-371** (Low) — AI tone-rewrite pass over the structured digest (explicitly STORY-252 OUT)
- **TASK-372** (Low) — Slack/email posting of digest output
- **TASK-373** (Low) — Cross-project (multi-repo) digest aggregation
- **TASK-374** (Low) — `audience: public` memory-frontmatter convention — document it and tag the
- **TASK-382** (Low) — Reconcile with TASK-299 (auto-generated `CHANGELOG.md`) — shared git-tag /
- **TASK-384** (Low) — CI freshness gate: a workflow step running `aida changelog refresh && git
- **TASK-385** (Low) — `/aida-changelog` skill + slash command, if agent-workflow demand appears
- **TASK-386** (Low) — Optional `.aida/changelog-headlines.toml` per-release headline override
- **TASK-387** (Low) — Shared git-tag + spec-graph extraction module factored out of `changelog.rs`
- **TASK-399** (Low) — Real-world: collect a corpus of actual gh-error stderr lines from
- **TASK-400** (Low) — `aida_core::git_ops::probe_branch_on_origin` — promote the local probe
- **TASK-407** (Low) — A reviewer-merge-escalation lingering-lease cleanup (parallel case, the
- **TASK-408** (Low) — Telemetry: count `escalated_to_human` markers that exit via auto-clean
- **TASK-409** (Low) — **Audit a real headless-drain log corpus.** Once a handful of
- **TASK-410** (Low) — **Telemetry on the inconclusive path.** `~/.aida/auto-complete.jsonl`
- **TASK-411** (Low) — **Auto-retry with bounded backoff.** Out of scope for BUG-266 (a
- **TASK-431** (Low) — parse_plan_followups should filter sentinel bullets (None, N/A, Nothing)
- **TASK-432** (Low) — Wrap phase 5 `aida pull` subprocess in a coarser retry-on-failure that
- **TASK-433** (Low) — Route `gh_pr_list_first`'s gh subprocess through `network_retry` to
- **TASK-434** (Low) — Add a `[orchestrator] retry_backoff_factor` knob so a project that wants
- **TASK-441** (Low) — **Calibration ledger (STORY-347)** is now unblocked — cold-boot and fork can run in parallel for the same punt and record both verdicts. The acceptance criteria mentioned in SPIKE-11 §"Composes with" is now satisfiable
- **TASK-442** (Low) — **Toolset-of-fork investigation** — flagged as a follow-up SPIKE in the SPIKE-11 writeup. The fork inherits the source's tools; whether the advisor should run with a restricted toolset (read-only, no Bash) is undecided
- **TASK-445** (Low) — Composable telemetry: emit a usage-log line for auto-claim events so we can graph "how often does auto-claim fire" across alpha users
- **TASK-446** (Low) — Consider extending the same auto-claim hook to `aida db merge-gate` (which can also create new agreed IDs as it promotes node-aware → short)
- **TASK-450** (Low) — aida init / scaffold: offer to git-add scaffolded files so new projects can commit cleanly
- **TASK-459** (Low) — Add an `aida experiment` command only after the manual protocol has repeated
- **TASK-460** (Low) — Add cost/token capture if Codex exposes stable local accounting
- **TASK-461** (Low) — Add a benchmark for MCP-only vs CLI-only AIDA access
- **TASK-462** (Low) — Add an external-project benchmark after AIDA-on-AIDA results are stable
- **TASK-500** (Low) — BUG-360 follow-up: refactor queue-done gate to pure queue_done_precheck_diagnose function for isolated testability
- **TASK-523** (Low) — lifecycle:no-review auto-remove orphaned Review PR-N queue item after merge
- **TASK-524** (Low) — aida edit warn on unrecognized lifecycle:* tag spellings (typo guard)
- **TASK-525** (Low) — auto-complete telemetry record LifecycleSkip in JSONL event for retro analysis
- **TASK-526** (Low) — aida list --tag-prefix lifecycle: column for fast-track audit view
- **TASK-528** (Low) — TASK: `aida backlog groom`: file-overlap source from `git log --grep "(SPEC-ID)"` so already-committed code joins the conflict set (currently trace+plan only)
- **TASK-529** (Low) — TASK: `aida backlog list --by-risk` (group view, mirrors `aida queue list --by-batch`)
- **TASK-530** (Low) — TASK: scaffolding-pack: link `backlog-grooming.md` from the auto-appended Discipline section in CLAUDE.md
- **TASK-531** (Low) — TASK: `aida backlog groom --interactive` (drive a TTY prompt instead of requiring `--specs` from the CLI side) — once we have a justified pattern for CLI-side interactive prompts
- **TASK-532** (Low) — TASK: cross-machine groom — emit a JSON plan another node can consume via `aida queue add`
- **TASK-533** (Low) — TASK: feed the risk heuristic from STORY-439's calibration data once that lands
- **TASK-576** (Low) — Headless drain forwards --bare to claude -p (10x startup perf)
- **TASK-585** (Low) — Migrate further flat skills to folder-form where a template/example helps
- **TASK-606** (Low) — Mailbox digest auto-triggers: run sync on session-end + drain boundaries (cadence = operator veto)
- **TASK-610** (Low) — Draft AIDA overview slide deck (Marp markdown): architecture, capabilities, fit with Claude Code + other agents
- **TASK-611** (Low) — BUG-425 (bulk-archive batch+progress) is independent — ship separately
- **TASK-612** (Low) — Consider surfacing watchdog/shelve events as a live `aida status` signal during unattended drains
- **TASK-613** (Low) — BUG-425 (bulk-archive batch+progress) is independent — ship separately
- **TASK-614** (Low) — Consider surfacing watchdog/shelve events as a live `aida status` signal during unattended drains
- **TASK-627** (Low) — Docs freshness spot-check before demo — getting-started.md + retire stale slideshow.html
- **TASK-628** (Low) — Wire vs-claude-code-workflows.md into positioning index + CLAUDE.md neighbor list
- **TASK-632** (Low) — Document the `[agents] bypass` knob in the discipline pack / CLAUDE.md agents.toml section
- **TASK-633** (Low) — Consider extending session-new/start to honor AIDA_PERMISSION_MODE env like queue work (deferred — not required by acceptance)
- **TASK-88** (Low) — aida node migrate-identity --to <id>: safely change this clone's node identity to an existing or new registry entry
- **TASK-89** (Low) — aida node verify: detect and surface drift between local node.toml and registry
- **TASK-90** (Low) — aida session end / aida pull: optional local branch auto-cleanup for merged PRs

## Planned (3)

- **SPIKE-4** — Spike 123
- **FR-98** — Drag a requirement to make child
- **CR-3** — scroll wheel should not change selected requirement

## Suggested master workflow

1. Approve the 🟢 QUEUE batch → `aida queue add <ID> --for implementer` (24 quick wins, momentum).
2. Approve the 📦 ARCHIVE batch → `aida archive <ID>` (21 observations).
3. Skim the 🗄️ Low tail (123) → bulk-archive the clearly-stale; promote any that are actually wanted.
4. Decide the 🧭 strategic 23 (esp. the 10 epics + the `batch:research-claude-overlap-2026-05-29` dump — possibly OBE given the date).
5. Leave the ~87 High/Med real backlog; sequence by priority.
