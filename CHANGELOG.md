# Changelog

All notable changes to this project are documented here. Generated
mechanically from the spec graph (`aida changelog refresh`) — do not edit
by hand; regenerate after merging.

## [v0.11.0] — 2026-06-01

Specs merged since v0.10.0 (126):

### Features

- **TASK-623** — Make 'aida init' idempotent toward node-id: complete node-id setup when store is attached but this clone hasn't acquired one (#431)
- **TASK-621** — Enhancement: read-only commands auto-attach the .aida-store worktree on fresh clone (so 'aida list' just works pre-init) (#429)
- **TASK-587** — validate and confirm custom agent roles in aida agent new (#415)
- **TASK-135** — TASK-268 'both-at-once trap' recurred a 3rd time on 2026-05-28. Pattern: I add … (#414)
- **TASK-607** — BUG-416 slice 1: pure worktree_occupant detection core (live-lease-on-worktree check) (#405)
- **FR-282** — aida graph / query_graph: traverse Custom("...") edge types (--follow/--type flag) (#403)
- **TASK-605** — Mailbox git-canonical digest: aida mailbox sync + merge-on-read (STORY-493 slice 4) (#402)
- **TASK-604** — Mailbox MCP tools: send_message + read_inbox (STORY-493 slice 3) (#401)
- **TASK-603** — Mailbox local store + aida mailbox CLI (send/inbox/thread) (STORY-493 slice 2) (#400)
- **TASK-602** — Mailbox pure core: Message model + inbox_for/thread/merge_dedup (STORY-493 slice 1) (#399)
- **TASK-601** — Resume decision core: ResumeFacts + phase_postcondition_met + resume_plan (STORY-492 slice 2a) (#398)
- **TASK-598** — Resumable-drain pure decision logic: classify_resumability + reconcile_resume_phase (STORY-491 slice 1) (#390)
- **STORY-489** — aida graph: cross-spec relationship queries (BlockedBy chains, epic rollup, cross-feature impact) via CLI + MCP (#388, #389)
- **TASK-597** — MCP query_graph tool: agent-queryable cross-spec graph (STORY-489 slice 3) (#388)
- **TASK-595** — aida graph CLI subcommand: blocked-by/blocks/tree/impact + --json (STORY-489 slice 2) (#386, #387)
- **TASK-594** — graph_walk core primitive: cycle-safe transitive relationship walk + status rollup (STORY-489 slice 1) (#385)
- **STORY-490** — Drain legibility: surface shelved/escalated (NeedsAttention) count in aida queue progress (#384)
- **TASK-583** — SPIKE-35 v2 follow-ups: align bughunter-severity schema, regen stale REVIEW.md, drop thinking-comment (#354, #367)
- **SPIKE-35** — Emit REVIEW.md from spec graph (substrate-as-bouncer for managed Code Review) (#353)
- **SPIKE-37** — Trigger Code Review via '@claude review once' from /aida-review (#353)
- **TASK-568** — aida list --tree: parent/child clustering view (parallel to aida queue list --tree) (#351)
- **TASK-574** — Skill helper subfolders: allow .claude/skills/<name>/{SKILL.md, templates/, examples/} (#350)
- **STORY-481** — /aida-techdebt skill: end-of-session duplication scan (#349)
- **TASK-577** — /aida-insights skill wrapper around 'aida usage' for monthly pattern view (#348)
- **TASK-569** — aida list --show-tags: surface spec tags in the list view (#345)
- **TASK-126** — Empirical 2026-05-25: scripts/release.sh's ecosystem-watch verification refuses… (#344)
- **TASK-570** — aida doctor --heal should detect + clean up orphan queue entries (delegate to aida queue prune --orphaned) (#342)
- **STORY-473** — Publish AIDA as a Claude Code plugin/marketplace package (#341)
- **SPIKE-30** — Integrate `claude agents --json` into `aida status`
- **SPIKE-31** — Emit path-gated `.claude/rules/` from the spec graph (substrate-as-bouncer)
- **SPIKE-33** — Emit `claude-cli://` deep links from `aida brief` + paste-ready prompts
- **SPIKE-34** — Re-shape `aida agent new claude` as a `claude --bg --agent <subagent-def>` wrapper
- **STORY-479** — /aida-learn skill: substrate-aware 'update CLAUDE.md so you don't repeat this' verb
- **TASK-1-109** — aida queue clear --completed is a no-op on the git-canonical backend (the defau…
- **TASK-120** — aida findings add --severity vocabulary only accepts major / minor / cosmetic. …
- **TASK-136** — 12-hour drain stall after single transient GH API failure during phase-1 verifi…
- **TASK-37** — Recurrence threshold of 3 is hard-coded in the recur handler; consider making i…
- **TASK-537** — aida queue prune --orphaned: detect + remove queue entries whose backing spec was deleted
- **TASK-572** — CLAUDE.local.md scaffolding: 'aida init' should write a structured personal-notes template + auto-gitignore

### Fixes

- **BUG-429** — Node-reg failure hard-aborts clone-init even though the store worktree was already attached (should soft-warn + continue) (#428)
- **BUG-428** — Fresh clone silently reads legacy requirements.yaml instead of the git-canonical store (shows stale pre-migration data) (#427)
- **BUG-427** — aida role enter: 'Resumed role' echo doesn't shell-escape the purpose — apostrophes/parens in a role purpose break the eval (#425)
- **BUG-424** — aida archive --older-than --dry-run panics on emoji/unicode spec titles (byte-index truncation not char-boundary-safe) (#418)
- **BUG-421** — Correct 'aida agent list-roles' output: fabricated lease-limits + inaccurate role summaries + missing 'aida role list' cross-reference (#417)
- **TASK-608** — Trim 'aida role scaffold' defaults to the agent-wired role set (drop architect/triage, add advisor) (#416)
- **TASK-560** — aida queue work --auto-complete + --resume: improve UX (currently conflict with terse error) (#412)
- **BUG-416** — Co-located agents (aida agent new x2 in one worktree) share session scope → aida add hint bleed (#409)
- **BUG-408** — aida agent new --show-context is not dry: it starts the session (worktree + lease + status→InProgress) before printing context (#406)
- **BUG-410** — auto-bump re-completes a manually-reopened spec referenced by an older merged commit (#404)
- **BUG-414** — lifecycle:no-ci-wait silently ignored — orchestrator always blocks on CI (#397)
- **BUG-413** — auto-bump leaves stale failure_reason on Completed in 3 of 4 flip paths (false CI-red findings) (#396)
- **BUG-411** — aida graph --impact (CLI + MCP) misses specs blocked via unidirectional Blocks edges (#395)
- **TASK-600** — Adversarial-review fixes: canonical MCP query_graph mode + conservative reconcile_resume_phase (#394)
- **BUG-51** — REQ_ID_PATTERN rejects (EPIC-19 v1) suffix variants (#392)
- **BUG-298** — aida scaffold apply: prune obsolete 'aida-' prefixed skills/hooks/commands from previous AIDA versions (#376)
- **BUG-289** — aida pr rebase <N> trips when a worktree (typically pr-N reviewer) already holds the conflicting ref (#375)
- **BUG-105** — queue work pre-populate-manifest uses only first matching plan file (TASK-95 DP-2 followup) (#374)
- **BUG-334** — Decide: should post_punt auto-flip spec status to needs-attention, or stay decoupled? (#374)
- **BUG-104** — queue done followups extraction drops bullets in some edge cases (TASK-96 DP-1 followup) (#373)
- **BUG-90** — BUG-89 follow-up: add integration tests for queue user_id consistency (#373)
- **BUG-91** — review_title_matches vs format_review_story_display case-sensitivity divergence (#372)
- **BUG-92** — read_config_workflow_hints doesn't strip TOML inline comments (#372)
- **BUG-93** — aida config hints source-display is misleading on unrecognized AIDA_HINTS value (#372)
- **BUG-407** — aida doctor --heal hangs forever on an interactive y/N prompt (blocks on stdin / unix_stream_read_generic) (#371)
- **BUG-406** — aida --help does not indent wrapped command descriptions (#366)
- **BUG-251** — Spec YAML parser fails hard on unknown enum variants / unknown fields — needs forward-compat reading for cross-binary version skew (#365)
- **BUG-331** — Substrate gap: sibling worktrees fall back to centralized mode because .aida-store detection doesn't traverse up the worktree tree (#364)
- **BUG-270** — aida db reconcile-status: recognize 'SPEC-ID:' prefix format on merge commits (#363)
- **BUG-404** — aida pr ship: auto-bump misses the just-merged spec — in-process pull reports 'Already up to date', spec stays Done (#361, #362)
- **BUG-405** — auto-bump: a NeedsAttention spec whose referencing PR merges is stranded — never promoted to Completed (BUG-328 sibling) (#360)
- **BUG-366** — aida queue list 'Next:' hint suggests 'aida queue work PR-N' but the command rejects PR numbers (#359)
- **TASK-1-113** — aida db reconcile-status: dry-run and apply DIVERGE for specs whose agreed_id ≠… (#356)
- **TASK-131** — aida queue list displays spec status as '▸ Approved' even when the underlying r… (#343)
- **BUG-420** — Headless implementer degenerates into filler-spin (echo counters + sleeps) AFTER committing its work — never advances to open-PR/next-phase
- **DOC-1-101** — Competitive analysis: agent memory libraries (Mem0/LangMem/Graphiti/Cognee/Letta/Dreams)
- **TASK-1-107** — aida list multi-filter is broken — appears to use OR not AND between flags. Emp…
- **TASK-1-108** — aida queue work phase-1 startup transitions status before acquiring lease — lea…
- **TASK-1-110** — TASK-268 'both-at-once trap' recurred a 3rd time on 2026-05-28. Pattern: I add …
- **TASK-534** — Rename stale test discipline_pack_scaffolds_seven_docs_plus_readme
- **TASK-535** — backlog: replace literal STORY-444 in user-facing error with generic placeholder
- **TASK-536** — backlog: drop redundant !is_terminal_status filter on Approved candidates

### Documentation

- **TASK-622** — README + CLAUDE.md: 'Cloning an existing AIDA project' note (reads auto-attach; writing needs a node id) (#430)
- **TASK-620** — Document shared-vs-local substrate model (queue/leases) in multi-advisor-coordination.md (#426)
- **TASK-617** — Capture the two-advisor functional-split coordination protocol (multi-advisor-coordination.md) (#423, #424)
- **SPIKE-46** — SPIKE-44 follow-through: publish a write-conformance spec + prove read-easy with a non-Rust prototype (#411)
- **TASK-590** — Inventory: every aida-core symbol that serializes/deserializes the on-disk object format (#408)
- **TASK-589** — aida help --glossary: surface the machinery/lifecycle glossary from the CLI (#407)
- **STORY-491** — Resumable orchestrator drain checkpointing: --resume re-enters a crashed --auto-complete at the right phase (#389)
- **SPIKE-43** — Git-canonical knowledge-substrate thesis: competitive validation + multi-vendor opening (#377, #378, #382, #383)
- **TASK-592** — Positioning doc: vs-kiro.md (nearest competitor) + README index (#380, #381)
- **TASK-591** — Positioning doc: vs-spec-kit.md (nearest competitor) + README index (#379)
- **SPIKE-45** — Competitive capability roadmap: match the multi-agent frontier, harden the spec-graph moat (#378)
- **TASK-588** — AGENTS.md: add a compact 'direct user assignment — implement BUG/TASK-N' runbook (no queued brief) (#370)
- **TASK-580** — README: trim narrative metadata to push loaded-context delta negative (#357)
- **SPIKE-32** — Compile spec graph to workflow.js targeting Claude Code's workflows runtime (#352)
- **TASK-567** — Marketplace publication security checklist (#340)
- **TASK-566** — Document AIDA MCP install matrix for major agent clients (#339)
- **TASK-565** — Marketplace research refresh: AIDA positioning + improvement roadmap (#338)
- **DOC-1-103** — Competitive analysis: AI coding agents (Cline/Aider/Plandex/Goose/Continue) — framework lens
- **SESSION-60** — SESSION-60 _(spec not in store)_
- **SLICE-2** — SLICE-2 _(spec not in store)_
- **TASK-121** — AIDA stores per-requirement transition history INSIDE each .aida-store/objects/…
- **TASK-575** — disable-model-invocation frontmatter on destructive skills

### Internal

- **TASK-609** — SPIKE-46 follow-up: CI conformance gate guarding AIDA's own on-disk YAML format against serializer drift (#413)
- **TASK-599** — Regression tests: query_graph MCP tool (functional + descriptor advertisement) (#391)
- **TASK-586** — Rename dialog role → advisor as the canonical identifier (supersedes TASK-279) (#368, #369)
- **TASK-132** — aida list multi-filter is broken — appears to use OR not AND between flags. Emp… (#358)
- **TASK-584** — Consolidate aida-recover into aida-doctor (remove the ~80%-overlap skill) (#355)
- **TASK-573** — @path imports in scaffolded CLAUDE.md to trim per-session context budget (#346)

### Other

- [AI:claude] docs(aida): skill-catalog slop audit — trio verdict + doctor/recover overlap
- [AI:claude] docs(briefs): Codex SPIKE-32 narrow-POC + AGY SPIKE-35 v2 rework
- [AI:claude] docs(briefs): Codex resume-bridge investigation — SPIKE-32's one blocker
- [AI:claude] docs(briefs): implementer brief for TASK-574 folder-form skills
- [AI:claude] docs(briefs): skill-catalog slop audit — consolidation per operator steer
- [AI:claude] docs(competitive): SPIKE-14 dynamic workflows decompose — COMPOSE verdict
- [AI:claude] docs(competitive): SPIKE-14 update — verified at operator's keyboard (Claude Code 2.1.156)
- [AI:claude] docs(competitive): SPIKE-15 agent view — COMPOSE+Divest verdict
- [AI:claude] docs(competitive): strategic recompose post Claude Code 2.1.154
- [AI:claude] docs(glossary): document existing --human-only mechanism + apply to TASK-115/116/123
- [AI:claude] docs(queue clear): explanatory help + flag the --completed no-op on git backend
- [AI:claude] docs(spike): SPIKE-46 — measured multi-vendor store interop (read-easy, write-bounded) (#410)
- [AI:claude] feat(archive): progress heading + throttled [k/N] ticks on bulk --older-than sweep (#422)
- [AI:claude] feat(rules): SPIKE-35 emit REVIEW.md + Round 2 strategic recompose
- [AI:claude] feat(triage+small-fixes): TASK-350 glossary + TASK-381 digest --copy + TASK-388 plan template
- [AI:claude] fix(preflight): orchestrator-aware messages for InProgress refusal + claim warning (TASK-1-108 follow-up)
- [AI:claude] fix(test): seed CLAUDE.local.md in 'idempotent_when_both_blocks_present' (TASK-572 follow-up)

## [v0.10.0] — 2026-05-26

Specs merged since v0.9.1 (103):

### Features

- **TASK-563** — scripts/aida-demo.sh: gh-backed throwaway hello-world walkthrough for first-user testing (#332)
- **TASK-559** — aida queue work --force-claim flag forwarding to internal session start invocation (#329)
- **STORY-464** — aida status integrates passive aida doctor scan — surface 1-2 findings under a 'Hygiene' section (#328)
- **STORY-465** — aida status: top-priority 'Awaiting you' section aggregating all human-gate items (PRs ready to merge, briefs for you, findings to triage, escalations) (#325)
- **TASK-556** — aida agent new: auto-inject first-message directive so agent starts working on --spec without operator typing (#321)
- **TASK-549** — Integrate orphan-worktree salvage patch: MCP stdio test additions (resources/list, resources/read, isError envelopes) (#319)
- **TASK-547** — aida queue work: pull from backlog with one verb (eliminate the two-step add + work) (#318)
- **TASK-557** — aida agent new: read per-agent config for default flags (skip-permissions etc.) — eliminates manual flag re-typing (#317)
- **TASK-554** — Context snapshot: when --spec is supplied, Active Session section makes scope-binding explicit (#316)
- **TASK-541** — aida brief --depends-on <SPEC>: explicit pickup-order constraint between briefs (#313)
- **TASK-551** — MCP server: expose 'add_relationship' tool (CLI parity gap surfaced by BUG-377 blast-radius mapping) (#310)
- **STORY-463** — SQLite cache lock: retry with backoff + lock-holder visibility + stale-lock → aida doctor heal path (#306)
- **STORY-462** — aida doctor — diagnostic + heal command for multi-agent state drift (orphan leases, brief/spec/lease disconnects, stale branches) (#305)
- **TASK-543** — aida agent register <pid> [--type <type>] [--role <role>]: backfill registry for already-running raw-launched agents (#302)
- **TASK-518** — aida brief <agent> <PR-N>: resolve PR number to backing spec (parallel to BUG-366 for queue work) (#286, #287, #288, #299, #301)
- **TASK-515** — aida status: derive agent visibility from active leases when launcher-provenance is absent (lease-as-agent fallback) (#298)
- **TASK-538** — MCP server: expose 'history' tool (parity with 'aida history' CLI) (#297)
- **TASK-510** — aida init: enqueue an 'initial commit' TASK so new users know the next step is to commit the scaffolded files (#289)
- **TASK-517** — aida ultraplan: include 'Reserved namespaces and conventions' in assembled prompt so /ultraplan avoids namespace collisions up-front (#285)
- **STORY-451** — Effort estimation at four lifecycle touchpoints + queue/backlog load aggregates + calibration trend (re-filed after STORY-447 ID-allocation race) (#283)
- **STORY-444** — /aida-backlog-groom skill + aida backlog CLI: curate Approved items into queue with risk + conflict analysis (#282)
- **TASK-506** — aida brief read <PATH>: CLI verb to read a brief without a direct file read (matches list + ack pattern) (#279)
- **STORY-442** — Lifecycle short-circuit tags: lifecycle:no-ci-wait / no-review / no-build / trivial for small-blast-radius specs (#276)
- **STORY-325** — Punt ledger — record every design-fork decision as structured metadata; analyze for patterns that become recorded principles (#275)
- **STORY-248** — Stacked-branch awareness: aida queue work --stack + auto-rebase on pull for parallel implementation pipelining (#272)
- **STORY-439** — Assistance + complexity calibration layer — predicted vs actual at pickup, ship, and review (#270)
- **STORY-244** — TUI architecture pivot: launcher + bash-wrapper re-entry (replaces PTY-host model) (#269)
- **STORY-436** — Role-context auto-injection on agent session start (EPIC-31 Phase 5, optional) (#266)
- **STORY-435** — MCP heartbeat busy/idle tracking for agent registry (EPIC-31 Phase 4) (#263)
- **TASK-503** — Pre-commit git hook runs cargo fmt --all (substrate-as-bouncer for the recurring fmt-CI failure pattern) (#258)
- **STORY-434** — aida agent new antigravity — Antigravity CLI launcher (EPIC-31 Phase 3b) (#257)
- **STORY-433** — aida agent new codex — Codex CLI launcher (EPIC-31 Phase 3a) (#256)
- **STORY-432** — aida agent new claude — Claude Code launcher with role context (EPIC-31 Phase 2) (#255)
- **TASK-490** — aida status: surface In Progress queue items prominently (currently buried in '... 11 more') (#253)
- **TASK-499** — aida session start: accept --spec as alias for --owns (CLI flag-name consistency with rest of substrate) (#251)
- **STORY-431** — Agent process registry + aida status integration (EPIC-31 Phase 1, subsumes TASK-498) (#249)
- **TASK-491** — aida queue move <id> --to-top: explicit operator control over queue head priority (#248)
- **TASK-493** — aida mcp-serve: detect binary update + advise/auto-restart so long-running MCP servers don't serve stale code (#247)
- **STORY-385** — aida status --cleanup: surface cleanup-actionable state (stale leases, sticky In-Progress, missed auto-bumps, open PRs, orphan dirs)
- **STORY-441** — Rethink aida history filter model: introduce archive concept + show recent terminal-status by default
- **STORY-448** — Shelve-on-orchestrator-failure into NeedsAttention with FailureReason
- **STORY-449** — Dependency-aware batch drain — continue past shelved, skip dependents
- **STORY-459** — aida queue: add --for-agent routing to complement --for-role (per-agent queue dispatch)
- **STORY-467** — aida findings add: advisor-driven observation entry — capture 'noticed but not yet actionable' patterns for audit/triage
- **TASK-542** — aida agent new --name <name>: explicit naming + default <agent>-<role>-<seq> auto-naming
- **TASK-548** — aida-pickup: skip 'confirm pickup' step when SPEC-ID is explicit (operator already committed)

### Fixes

- **BUG-385** — Cross-platform CI: claim_task_records_explicit_worktree_path asserts against non-canonical path (Windows + macOS fail) (#331)
- **TASK-561** — aida doctor heal spec-status-drift: when In Progress + no active lease, revert status to Approved (currently ambiguous) (#330)
- **TASK-558** — STORY-463 retry budget: bump defaults to handle production schema-apply contention (50/200/500ms → exponential up to ~30s) (#327)
- **BUG-384** — Auto-bump on aida session start is non-atomic — status drifts to In Progress when lease creation subsequently fails (#326)
- **BUG-379** — Implementer ceiling: 'aida session start --owns SPEC' creates lease but doesn't bump spec status from Approved to In Progress (#312)
- **BUG-381** — MCP list_requirements silently returns empty on any status filter (#311)
- **BUG-377** — MCP add_comment misroutes text arg to author field; silent data loss (#308)
- **TASK-550** — Map BUG-377 blast radius: systematic test of all MCP write tools for field-mapping inversions (#308)
- **BUG-378** — Antigravity/Codex scratchpad drift: agent re-reads its local task.md and reports 'done' while ignoring AIDA brief queue (#304)
- **BUG-376** — Implementer ceiling: agent ships + queue-done correctly, then lingers watching CI instead of exiting (interactive variant of BUG-361 family) (#303)
- **BUG-375** — Codex skills scaffolded by 'aida init' missing YAML frontmatter — 18 skills skipped on Codex launch (#294)
- **TASK-504** — claim_task should canonicalize worktree_path at record-time (or lease_covers_cwd should canonicalize both sides) (#290)
- **BUG-374** — Headless implementer text-question-and-clean-exit recurrence (BUG-354 family) — orchestrator misclassifies as phase-1 failure (#284)
- **BUG-273** — Phase 2 'gh run watch' pollutes tee-captured drain logs with hundreds of redrawn blocks (#281)
- **BUG-372** — Two clones claimed STORY-446 independently (ID-allocation race despite TASK-281 block-claim) — re-file after orphan-store rebase lost BUG-1-085 (#280)
- **BUG-369** — Implementer cannot punt when spec is still Approved during early phase 1 (forces clean exit, drain counts as failure) (#274)
- **TASK-508** — FR-0226 pre-commit hook: remove leftover DEBUG echoes from production hook (substrate-as-bouncer gitignored path check) (#265)
- **TASK-509** — aida-validate-commit hook: accept multi-agent attribution like [AI:antigravity+claude] (#264)
- **BUG-352** — Queue cluster-derivation routes new spec to active lease's scope without checking appropriateness (caused TASK-488 failure) (#261)
- **BUG-357** — Orchestrator reconcile-against-reality misattributes another spec's merged PR to the dispatched spec (#260)
- **BUG-367** — session end warns about unshipped commits after squash-merged aida pr ship (#259)
- **BUG-364** — stale-base re-check after auto-rebase fires against stale gh pr view cache (false-positive phase-3 failure) (#252)
- **TASK-474** — aida add scope-detection heuristic misroutes hints when cwd has no lease (#250)
- **BUG-380** — aida show <ID> emits 'fatal: Not a valid object name main' inline on repos with non-main default branch

### Documentation

- **TASK-564** — Document cross-agent hook pause/abort/defer semantics (#334)
- **TASK-512** — Document aida:<subcommand> tag-namespace convention in docs/aida/ + CLAUDE.md + scaffolding-pack (#271)
- **STORY-443** — Reshape aida init docs/ namespace: discipline pack to docs/aida/, keep docs/plans/ for project-authored plans (#268)
- **TASK-476** — docs/multi-node.md: replace line-ref to current_user_id with symbol-ref (#262)
- **TASK-540** — Sync docs/agents/codex-mcp-setup.md live copy → master template

### Infrastructure

- **TASK-552** — Clean up 8 dead-code warnings in aida-cli (audit intent + add allow attrs or remove per-item) (#315)
- **TASK-511** — Migrate aida-* flat tags to aida:* colon-namespaced convention (matches batch: lifecycle: severity: pattern) (#278)
- **TASK-505** — make build (and build-release) kill running aida mcp-serve processes so clients respawn with the fresh binary (#254)

### Internal

- **TASK-501** — TASK-491 regression test exercises --to N path, not --top arithmetic (#291)
- **TASK-521** — Audit AIDA_TEST_* process-global env var usage in tests; isolate via per-test temp paths (BUG-371 root-cause sweep) (#277)
- **TASK-514** — aida ultraplan: accept --copy as no-op alias (clipboard is the default; flag explicitly affirms the action)

### Other

- [AI:claude] docs(claude-md): mention scripts/aida-demo.sh for first-user validation (#335)
- [AI:claude] feat(demo): cleanup section uses box_title + note_box
- [AI:claude] feat(demo): final Notes section in note_box + drop stale BUG-386 framing
- [AI:claude] feat(demo): option [1] runs real work via claude -p + glossary path fix
- [AI:claude] feat(demo): option [3] doctor wraps explanation in note_box
- [AI:claude] feat(demo): option [4] search wraps explanation in note_box
- [AI:claude] feat(demo): option [5] findings now frames the AIDA feedback loop
- [AI:claude] feat(demo): step 8 also shows 'aida list' so the new STORY appears in backlog
- [AI:claude] fix(demo): box_title and note_box share fixed width 80
- [AI:claude] fix(demo): explore menu clears screen on pick, presses enter on return
- [AI:claude] fix(demo): explore menu in note_box, Ctrl-C trap, box_title Unicode align
- [AI:claude] fix(demo): option-1 anatomy framing + glossary fallback + visible commands
- [AI:claude] fix(demo): pause + clear after 'aida pull' before 'aida show'
- [AI:claude] fix(demo): pause after grep output + before explore menu
- [AI:claude] fix(demo): scaffold commit no longer emits foreign-trace warning
- [AI:claude] fix(demo): true-rectangle note_box + auto-pause between steps
- [BUG-386] fix(scaffold): scaffold all skill + command templates on aida init (#333)
- [STORY-305] feat(scaffolding): per-project skill extensions via local/ + .local.md (#210)
- [TASK-553] docs(discipline): implementer-discipline.md — six rules + bouncer map
- [TASK-555] docs(agents): cross-agent skill-invocation surface map
- feat(demo + status): TUI polish for the first-user walkthrough (#337)
- fix(demo + status): show commands inline; cleanup summary surfaces categories (#336)

## [v0.9.1] — 2026-05-23

Specs merged since v0.9.0 (22):

### Features

- **TASK-496** — Auto-invoke /aida-capture via Claude Code Stop hook to catch missed requirement filings (#245)
- **TASK-489** — aida session end: accept --spec / --branch to resolve lease ID (user has spec context, not opaque lease ID) (#241)
- **STORY-429** — Orchestrator reviewer pre-flight: auto-rebase on stale-base detection (STORY-281 evolution) (#240)
- **STORY-325** — Punt ledger — record every design-fork decision as structured metadata; analyze for patterns that become recorded principles (#233, #239)
- **STORY-426** — STORY-425 Level 2: MCP brief surface — list_briefs / read_brief / ack_brief + per-agent /aida-pickup skill (#234)
- **STORY-316** — /aida-recover skill — advisor diagnostic playbook for session/orchestrator/runtime-state divergence (#231)
- **TASK-492** — aida brief: generate per-agent brief files at .aida/agent-briefs/<agent>/ (STORY-425 Level 1) (#230)
- **STORY-423** — aida --asciinema: top-level wrapper flag for recording any aida invocation (#228)
- **TASK-486** — aida status: surface cross-platform CI status so 'ready to cut a release' reflects the actual release gate (#226)

### Fixes

- **BUG-361** — Ceiling variant: agent commits locally + verifies + exits without running aida queue done OR aida pr ship (no lifecycle command invoked) (#246)
- **BUG-358** — Cross-platform CI: 4 new Windows-only test failures from STORY-426 + TASK-486 + TASK-492 (regression after BUG-346 fix) (#243)
- **TASK-488** — Pre-commit hook: skip AI-tag/trace warnings on mechanical release-script commits (#238)
- **BUG-354** — Headless implementer text-question bypass: model asks question in markdown output (no AskUserQuestion tool call), then exits — BUG-342 doesn't catch (#237)
- **SPEC-411** — aida pr ship post-merge pull assumes aida is on PATH (#227)
- **BUG-360** — BUG-269 regression: aida queue done allowed dequeue with commits-ahead-of-origin but no open PR (TASK-489 exit-without-PR)

### Documentation

- **TASK-495** — Antigravity /aida-pickup skill — consume STORY-426's MCP brief tools (list_briefs/read_brief/ack_brief) (#236)
- **TASK-414** — simple-mode-with-more-items State preamble PR row should include 'no PR yet' variant (#232)
- **TASK-487** — aida-cli/src/cli.rs: 75 other --help doc-comments still embed SPEC-IDs (TASK-268 convention) (#229)

### Internal

- **TASK-448** — auto_claim_summary: drop unused 'lower' local; eq_ignore_ascii_case handles case both sides (#235)
- **TASK-417** — describe_drain_mode duplicates STATE_QUEUED/STATE_COMPLETED string literals (#224)
- **TASK-426** — headless_tee env-var tests share global state — comment is misleading (#220)
- **TASK-497** — aida --asciinema: project-local default directory + spec-aware filename/title derivation (STORY-423 refinement)

## [v0.9.0] — 2026-05-22

Specs merged since v0.8.0 (114):

### Features

- **TASK-479** — STORY-305: harden <skill>.local.md merge beyond AIDA.md instruction (#222)
- **TASK-481** — Pre-commit hook (scaffolded): refuse commits touching gitignored paths without --allow-intermediate (#218)
- **SPEC-403** — schema discovery worked but tool descriptions could benefit from more detail (#206)
- **STORY-281** — Reviewer phase: detect stale PR base + auto-rebase (or block) before headless review (#202)
- **TASK-310** — aida queue work --batches A,B,C: chain multiple batches in one auto-complete drain (#199)
- **TASK-449** — Show auto-claim summary even when a type has no blocks yet (db block status early-return) (#198)
- **TASK-458** — aida pr ship [<N>] — one-command create-if-needed + watch CI + merge + pull + worktree-aware cleanup (#189)
- **TASK-440** — MCP tool_descriptors: add outputSchema to coordination tools (and original 7) (#172)
- **TASK-444** — Surface `[block_allocation]` knobs in `aida db block status` output so users see their effective threshold/size at a glance (#171)
- **TASK-346** — Windows: production read paths need transient-open retry — read_atomic helper + concurrent-read-site audit (#165)
- **STORY-347** — Cold-boot vs live-advisor calibration ledger (toggleable learning mode) (#164)
- **TASK-281** — aida db block: auto-claim a new block on spec creation when available IDs cross threshold (#163)
- **STORY-361** — Extend AIDA MCP server with coordination tools (SPIKE-9 outcome) (#162)
- **STORY-360** — Implement fork-from-live advisor with cold-boot fallback (SPIKE-11 outcome) (#161)
- **TASK-308** — aida pr rebase <N>: CLI command to rebase a PR onto its base default (clean or abort) (#146)
- **TASK-307** — Tee headless Claude output to terminal (high-signal events) during --no-human phases (#143)
- **TASK-404** — aida findings dismiss: accept --reason flag to record rationale in the audit comment (#133)
- **TASK-398** — aida headless tail — clean tailer for headless drain logs (#132)
- **TASK-391** — aida state-snapshot --spec <ID>: emit the finish-state State preamble deterministically (#131)
- **TASK-401** — Headless /aida-pickup: enforce push + /aida-pr before exit; never commit-and-exit (#127)
- **TASK-358** — Clean up lingering worktrees from unresumed --escalate-blocks punts (#125)
- **TASK-351** — aida edit: add --add-tag / --remove-tag — --tags is replace-only and silently clobbers (#120)
- **STORY-306** — Advisor escalation tier for --no-human: design-forks punt to a headless advisor before reaching the human (#118)
- **STORY-252** — /aida-digest skill: advisor-curated narrative report of project work (customer / team / self perspectives) (#114)
- **STORY-333** — Typed blocked-by + human-only markers — orchestrator + queue skip un-pickable specs at pickup instead of phase-1-failing on them (#113)
- **TASK-294** — aida-worker bash function: MVP queue-drain loop with file-directive control (.aida/worker.cmd) (#112)
- **STORY-276** — aida queue work --auto-complete --no-human=both: headless implementer phase (requires /aida-punt) (#111)
- **STORY-332** — /aida-punt mechanism + NeedsAttention lifecycle status — the design-fork punt safety net for --no-human (#110)

### Fixes

- **BUG-346** — Cross-platform CI: 3 Windows-specific test failures blocking v0.9.0 release (#225)
- **BUG-342** — BUG-280 recurrence: skill-template AskUserQuestion ban is paper enforcement; need programmatic gate (#221)
- **BUG-344** — aida pr ship: 'no checks reported' interpreted as CI failure; doesn't wait for CI startup window (#213)
- **BUG-345** — aida pr ship: post-merge 'aida pull' fails when master's main worktree is on a feature branch (#213)
- **TASK-470** — Handle file-rename edge case in stale-base overlap detection (#211)
- **BUG-220** — Multi-node queue identity: queue appears empty from a second node despite same shell user (joe) (#207)
- **BUG-339** — aida pr ship: validate final squash subject ends with (SPEC-ID) before merging (#205)
- **SPEC-410** — aida pr ship squash merge can drop spec ID from merge commit subject (#204)
- **BUG-332** — MCP add_requirement: reject (or auto-normalize) non-canonical SPEC-N IDs; enforce type-prefix taxonomy (#195)
- **STORY-407** — Empirical integration — connect Antigravity CLI 1.0.1 to AIDA's MCP coordination surface (N=2 agent validation) (#183, #187, #191, #194)
- **BUG-328** — aida pull auto-bump: also promote Approved-with-merged-PR specs to Completed (not just Done specs) (#192)
- **TASK-419** — aida headless tail --list: split lease into its own column (#184)
- **TASK-438** — MCP tool_claim_task: TOCTOU race lets two concurrent claims on the same spec both succeed (#180)
- **BUG-327** — BUG-280 hole: /aida-review skill template's AskUserQuestion ban is paper enforcement — reviewer reasoned past it, no verdict file written (#179)
- **BUG-311** — aida queue work --steal silently fails to end dormant lease — orchestrator re-emits 'pass --steal' error despite the flag being passed (#178)
- **BUG-310** — MCP-created specs are not consistently visible to local CLI (#177)
- **BUG-307** — Orchestrator: auto-detect + auto-clean dormant leases (process dead) before refusing on lease conflict (#176)
- **BUG-312** — aida session leases display: 8-char prefix collides on HLC-derived UUIDs in the same generation window — show enough chars to disambiguate (#175)
- **TASK-437** — MCP tool_file_finding: pr-as-string produces non-canonical from-review tag (#168)
- **TASK-436** — MCP tool_post_directive: validate verb against drain|pause|exit (#167)
- **TASK-439** — MCP resolve_punt/escalate_punt: use write_atomic, not std::fs::write, for punt-response files (#166)
- **BUG-286** — Orchestrator phases 3-6: retry transient gh/git network errors before classifying as failure (#159)
- **BUG-285** — BUG-269 gate has a hole — 'aida queue done --yes' succeeds with no open PR (tonight's TASK-413/TASK-416 evidence) (#158)
- **TASK-416** — state-snapshot --json: PlanRow serialization shape-inconsistent with PrRow (#157)
- **TASK-429** — TUI: arm TERMINAL_NEEDS_RESTORE gate BEFORE enable_raw_mode to close micro-race window (#155)
- **BUG-280** — Reviewer skill under --no-human=both posts verdict to PR but skips verdict-file AND tries to AskUserQuestion (#152)
- **BUG-238** — Plan ## Followups parse is skipped when a spec is marked Completed directly (bypasses queue done + auto-bump) (#151)
- **TASK-328** — Repo-wide audit: trace: markers in /// doc comments leak SPEC-IDs into --help text (#147)
- **BUG-110** — TUI: install SIGTERM/SIGINT handler so killing the process restores terminal state (raw mode + cursor) (#144)
- **TASK-364** — aida-worker: leading-whitespace directives never pop from .aida/worker.cmd (#140)
- **BUG-269** — aida queue done: refuse when commits exist without an open PR — programmatic complement to TASK-401 (#136)
- **BUG-249** — aida queue move <id> reports ✓ Moved even when <id> is not in the queue (silent no-op) (#134)
- **BUG-266** — Headless implementer: transient Anthropic API errors (529/overloaded) should be inconclusive, not failed (#126)
- **BUG-257** — Orchestrator phase-1 misclassifies transient GH-API network error during PR lookup as 'no PR opened' failure (#123)
- **BUG-254** — aida pull / orchestrator phase 5 reports 'phase 5 complete' when code-leg git pull failed (silent failure) (#122)
- **BUG-115** — aida db block: 'low warning' fires on lowest block even when higher block has capacity (warn-on-stale-info) (#119)
- **BUG-245** — Orchestrator reports the dispatched spec as 'shipped' when phase-1 actually shipped a different spec (#116)

### Documentation

- **TASK-420** — findings dismiss/promote --help embeds example SPEC-IDs (BUG-254, PR-219) per TASK-268 convention (#223)
- **STORY-418** — Antigravity: own and propagate Antigravity MCP setup scaffolding into new AIDA projects (#219)
- **TASK-482** — Scaffold docs/extending-skills.md into target projects (STORY-305 follow-up) (#216)
- **TASK-333** — Make /aida-review's fix-forward policy explicit — reviewers improvise inconsistently (one refuses, one fix-forwards) (#215)
- **STORY-417** — Codex: own and propagate Codex MCP setup scaffolding into new AIDA projects (#214)
- **TASK-484** — Restore or remove AGENTS.md (CLAUDE.md references it as scaffolded but it doesn't exist) (#214)
- **TASK-485** — aida init: propagate docs/agents/ directory to new projects via templates (#214)
- **STORY-318** — Periodic ecosystem review — recurring scan of Claude Code + neighbor-tool capabilities, fed into AIDA's backlog (#212)
- **TASK-339** — docs/architecture/autonomy-and-escalation.md — the autonomy modes, escalation cascade, and inter-agent communication architecture (#208)
- **SPIKE-6** — Spike: skillfold compatibility — can AIDA's skill templates compile to skillfold YAML for cross-platform (Cursor, Codex)? (#200)
- **TASK-288** — TUI prior-art study: git-clone + analyze Claude Squad / crystal / vibe-kanban / vibe-tree / cmux / Conductor (prereq for STORY-244) (#197)
- **TASK-447** — aida queue done --skip-pr-check flag has blank --help description (#196)
- **STORY-398** — Empirical verification — connect Codex to AIDA's MCP coordination surface (the agent-agnostic moat made operational) (#181)
- **TASK-435** — Recovery doc: recipe 1's --delete-branch still surfaces cosmetic 'branch in use by worktree' error (#170)
- **TASK-423** — aida queue done --force flag has blank --help description (#169)
- **TASK-406** — Recovery flow: gh pr merge --delete-branch trips on main-in-worktree; document worktree-aware merge sequence (#160)
- **TASK-413** — Closing summary in aida-pickup Step 6 over-claims State preamble for orchestrator template (#156)
- **TASK-412** — simple-mode-empty template ▶ row contradicts Step 5c — recommend Stop here when PR already open (#154)
- **TASK-422** — queue-done gate creates chicken-and-egg with /aida-pr status check (#153)
- **TASK-338** — Glossary: pin down AIDA's orchestration / session / autonomy machinery vocabulary (#150)
- **TASK-337** — docs/positioning/vs-claude-code-subagents.md — where AIDA sits vs Claude Code's /agents subagents (#149)
- **TASK-319** — Codify the 'don't retroactively edit dated historical artifacts' convention (SPIKE / PROMPT_HISTORY / dated docs) (#148)
- **SPIKE-10** — SPIKE: Multi-advisor coordination — subsystem-scoped advisors + parent→sibling initiation + bidirectional substrate propagation (#141)
- **SPIKE-9** — SPIKE: MCP server as the inter-agent communication bus — evaluate vs file-handshakes (#139)
- **SPIKE-11** — Evaluate session-forking as the rich-context advisor path (#138)
- **TASK-390** — Push the finish-state rubric markers into batch/cluster/simple-mode templates too (#130)
- **TASK-393** — Make the ⊕ advise-row inclusion explicit in the orchestrator-mode templates (#129)
- **TASK-392** — Align "→ next:" / "→ Next:" capitalization between finish-state rubric and skill templates (#128)
- **TASK-359** — Implementer finish-checkpoint UX: structured menu with state, recommendation, consequences, and an advise escape (#121)
- **TASK-283** — pre-release-check.sh: fix comment about '-q' yielding empty string (it yields tab via @tsv on null) (#117)
- **TASK-299** — Auto-generated CHANGELOG.md from spec graph + git tag boundaries (#115)

### Infrastructure

- **TASK-290** — CI: upgrade actions to Node.js 24-compatible versions (actions/checkout v4→v5, arduino/setup-protoc) before Sep 2026 deadline (#142)
- **TASK-421** — Clean up 3 build warnings in aida-cli main.rs (origin_pushed unused, capitalize dead) (#137)
- **TASK-284** — pre-release-check.sh: use portable date parsing (date -d is GNU-only; falls back to always-dispatch on BSD/macOS) (#135)
- **TASK-389** — pre-release-check.sh: no-runs path never triggers — empty array produces tab-separated null row, not empty string (#124)

### Internal

- **TASK-454** — MCP stdio test: assert spec-ID parse picks expected ID (not first regex match) (#217)
- **TASK-455** — MCP stdio test: add readline deadline to McpClient.request so a hung mcp-serve fails fast (#217)
- **TASK-469** — Add unit test for classify_stale_base with N>1 overlapping files (#209)
- **TASK-471** — E2E integration test: full --auto-complete --no-human cycle hits stale-base block (#209)
- **SPEC-398** — MCP stdio suite should derive agent contract checks from canonical docs or tools/list (#182)
- **TASK-452** — Test: every tool + arg documented in docs/agents/cross-agent-onboarding.md must appear in MCP tools/list (#174)
- **TASK-451** — Land Codex's MCP stdio compatibility tests — tests/test_mcp_stdio.py + tests/test_mcp_stdio.sh (#173)
- **TASK-336** — Fold the orchestrator run-UUID into STORY-301's drain-state file (BUG-233 followup) (#145)

### Other

- [AI:claude] docs(readme): add orchestrator vs Claude-sessions process diagram for --auto-complete
- [AI:claude] docs(readme): honest empirical-proof framing + LLM-skeptic-aware positioning (TASK-403 prep) (#190)
- [AI:claude] docs(scaffolding): sync 22 scaffolding-pack memories + 1 discipline doc to master templates (#193)
- [AI:codex+claude] docs+scripts: land 2026-05-22 substrate (Codex strategic plans + scrollback dedup utility) (#185)
- [AI:codex] feat(store): add configurable auto-push cadence (#201)
- docs(competitive): implement maintained competitive analysis surface for STORY-260 (#203)

## [v0.8.0] — 2026-05-19

Specs merged since v0.7.0 (81):

### Features

- **STORY-285** — Implementer findings reach advisor under --no-human (mirror of STORY-278 for phase 1) (#107)
- **STORY-301** — aida drain status: show the active orchestrator command, batch progress, and what happens on session exit (#106)
- **TASK-306** — Statusline + flag clarity for --no-human scope (currently reviewer-only) (#105)
- **STORY-255** — aida init: ship a starter discipline pack — generic memories + docs/aida-discipline/ (#86)
- **TASK-329** — Orchestrator graceful-exit signal: skill touches sentinel file, orchestrator reaps the Claude Code REPL (#83)
- **TASK-264** — aida session forget <id>: explicit removal of a specific tracked Claude session from aida session list (#81)
- **STORY-287** — Three-mode autonomy taxonomy: default / --zen / --no-human (with prompt kind classification + punt behavior) (#78)
- **TASK-293** — aida queue work next / nextN keyword: explicit head pickup + drain-N-from-head form (#77)
- **TASK-292** — aida queue work --auto-complete (no SPEC id) should pick queue head, matching no-arg behavior (#76)
- **TASK-280** — aida queue move: support --to <position> and --to-front flags (matching mental model from edit/list) (#74)
- **TASK-286** — Skill end-of-session menus: detect --auto-complete context, show orchestrator-aware path (not manual) (#73)
- **TASK-272** — /aida-pickup: detect batch context, offer cluster-mode continuation before /aida-pr (#72)
- **TASK-282** — aida statusline: hide redundant sess:/wt: indicators; show only when they diverge from the current scope (#71)
- **TASK-269** — aida show: color-code Status field + reprint as last line (always-visible glance signal) (#70)
- **TASK-265** — /aida-pickup: pretty-print the picked-up spec at session start (in-terminal context, no separate aida show) (#69)
- **STORY-278** — Headless reviewer findings reach advisor for triage and follow-up filing (#65)
- **STORY-263** — aida queue work --auto-complete --no-human: headless reviewer (and optionally implementer) for true overnight autonomy (#64)
- **TASK-285** — aida queue work: allow --batch + --auto-complete composition (drain batch via orchestrator) (#61)
- **TASK-266** — aida --auto-complete: failure telemetry + auto-draft BUG on phase failures for recurse-fix dogfood (#59)
- **TASK-291** — /aida-review: surface 'Press Ctrl+D to exit and let --auto-complete proceed' loudly at end of orchestrator-driven review (#58)
- **TASK-270** — aida queue work: accept 'batch:NAME' as positional, or suggest --batch flag in the error (#54)
- **TASK-267** — aida session end: workflow hint stops short — mention merge step + offer self-merge path (#52)
- **TASK-278** — /aida-review: fire gh pr review --approve on positive verdict (so gh reviewDecision = APPROVED) (#50)
- **TASK-250** — aida queue list: distinguish 'review in progress' from 'awaiting merge' in the Done section (#49)
- **TASK-259** — /aida-pr: print 'about to happen' banner — completed / now / then-you sections (#47)
- **STORY-246** — aida queue work --auto-complete: orchestrate full implementer→CI→reviewer→merge→pull lifecycle per spec (#46)
- **STORY-132** — AIDA TUI shell: PTY-host a Claude session + status strip + prefix-key exit (#41)
- **TASK-228** — Cancellable fallback wakeups: cancel scheduled re-entries when the protected event completes (#38)
- **TASK-244** — PS1 'role:X' prefix shows shell-persistent role, not active session's role — surfaces mismatch when they disagree (#37)
- **TASK-243** — aida session end: print role + claude-session-id in ambiguity-resolution prompt (#36)
- **TASK-242** — aida goal: derive machine-checkable completion conditions from AIDA metadata (for /goal, /schedule, etc.) (#35)
- **TASK-246** — Auto-complete review story when implementer fixes pass + user self-merges without re-review iteration (#34)
- **TASK-245** — aida session start: support --reuse-branch (or auto-detect existing branch) for fixup-on-existing-PR-branch flow (#32)
- **TASK-112** — aida queue work --resume: relaunch claude with prior session's conversation history (#31)

### Fixes

- **BUG-246** — Windows: concurrent_writers_never_tear_the_file fails — reader hits transient ERROR_ACCESS_DENIED during atomic rename — blocks v0.8.0 (#109)
- **BUG-244** — Starter-memory-template parser fails on CRLF — 2 STORY-255 tests fail on Windows, blocks v0.8.0 (#108)
- **BUG-111** — scripts/release.sh: discover intra-workspace dep pins generically, not via hardcoded list (#104)
- **BUG-241** — Orchestrator declares FALSE phase failures — must reconcile against reality (PR/spec state) before failing ANY phase (#103)
- **BUG-237** — AIDA_ZEN has no provenance corroboration — a leaked AIDA_ZEN=1 silently enables zen mode and can auto-merge (#102)
- **BUG-236** — Auto-complete recovery-hint chain dead-ends — every suggested command bounces off the next layer (#101)
- **BUG-231** — aida findings promote: TASK-327 ended up Approved without joining any queue (silent failure or wrong-role-route) (#100)
- **BUG-229** — aida queue list confuses reviewer-snapshot pr-N branch with implementer's PR-source branch; reports 'no PR opened yet' for an actively-reviewed PR (#99)
- **BUG-226** — Standalone aida queue work --role reviewer --no-human exits silent (no pass/fail summary) (#98)
- **BUG-232** — Plain --zen without --auto-complete: end-of-session stalls at 'say the word' with PR unopened (#97)
- **TASK-327** — AIDA_ZEN treated as non-empty=on — AIDA_ZEN=0 counter-intuitively enables zen (#96)
- **TASK-331** — Audit non-atomic std::fs::write calls — convert concurrent-writer paths to write_atomic (BUG-228 follow-up) (#95)
- **BUG-233** — Orchestrator-spawned child can't verify its parentage — misidentifies legit orchestrator context (no env leak; BUG-233 was misdiagnosed) (#94)
- **BUG-223** — orchestrator phase-1 detection: false-negative when /aida-pr swaps branch (BUG-88 guard fires; lease's recorded branch is stale) (#85)
- **BUG-227** — aida show --help leaks trace:STORY-62 / STORY-78 / TASK-241 SPEC-IDs into --tree/--sync/--no-git/--verbose help text (#84)
- **BUG-228** — ~/.aida/roles/implementer.toml: stray quote at line 58 from corrupted activity-log append makes aida role show fail (#82)
- **BUG-112** — aida session list: INITIAL TOPIC column shows stale data for long-running sessions (misleading) (#80)
- **TASK-318** — aida queue move --help leaks trace:STORY-72 via the --after field doc comment (#79)
- **BUG-116** — Skill templates: apply the ⇒ and ⏸ glyphs uniformly (TASK-260 design-refinement didn't propagate) (#75)
- **BUG-225** — no-launch headless hint omits --session-id and reorders prompt vs real launch (#68)
- **BUG-224** — aida plan verify FAILs on STORY-263 plan — stale ref to docs/aida-discipline/autonomous-drain.md (#67)
- **BUG-113** — auto-bump + reconcile-status: review stories with populated Covers section still stuck at Done after merge (BUG-106 follow-up) (#60)
- **BUG-219** — auto-bump: complete review stories at Approved when their PR merges without ever being In Progress (BUG-113 sibling) (#57)
- **BUG-218** — aida queue work --auto-complete: recovery hint says 'CI is red' even when the failure was a spawn error (not CI) (#56)
- **BUG-217** — aida queue work --auto-complete phase 2: ENOENT spawning 'aida session end' — orchestrator subprocess PATH issue (#55)
- **TASK-268** — Strip internal SPEC-ID citations from user-facing workflow hints (e.g., 'per TASK-85') (#53)
- **BUG-114** — aida queue work --auto-complete: phase-1 lease disambiguation fails when multiple session leases appear (#48)
- **BUG-109** — TUI empty state is undiscoverable: black screen, no help text, no key hints — users kill the process (#44)
- **TASK-248** — TermGuard::enter() leaves raw mode on if alt-screen entry fails (#42)
- **BUG-103** — CI macos-latest job intermittently fails: cargo resolves to rustup-init (stale cache) (#40)
- **BUG-106** — Auto-bump misses implementer specs after squash-merge of cluster PR (PR title lacks individual spec IDs) (#33)

### Documentation

- **TASK-289** — README: AIDA's defensible niche statement (8 dimensions from 2026-05-16 competitive analysis) (#93)
- **TASK-279** — Rename role:dialog's user-facing identity to 'advisor' (keep internal role name) (#92)
- **TASK-277** — Sample 'first project' walkthrough in docs/ — concrete AIDA-from-zero example (#91)
- **TASK-276** — README: surface docs/positioning/ links (vs-ultraplan / vs-ultrareview / vs-karpathy-md / vs-saas-pm) (#90)
- **TASK-274** — README: 'Getting started in 5 minutes' section — aida init → add → pickup → PR walkthrough (#89)
- **TASK-273** — Add spec-lifecycle flow diagram + State/Verb table to README (alpha onboarding) (#88)
- **TASK-275** — README: install instructions for prebuilt binary tarball + build-from-source paths (#87)
- **SPIKE-7** — Spike: investigate headless Claude behavior with AIDA skills (claude -p) — does it work, what breaks, how to detect stuck? (#63)

### Infrastructure

- **TASK-257** — Split CI: Linux-only on PRs, Windows + macOS on nightly cron + manual dispatch (#51)
- **TASK-233** — aida session end --watch-ci: live CI progress display (vs silent --wait-ci) (#39)

### Internal

- **TASK-303** — Fix 3 new clippy::doc_lazy_continuation warnings on claude_headless_args (#66)
- **TASK-260** — Skill next-steps: render multi-option prompts as Path/Action/Why tables, not numbered bullets (#48)

### Other

- EPIC-26 batch 3: TUI overlay, multi-tab, crash recovery + autonomous drains (#43)
- [AI:claude] docs(competitive): living competitive-analysis directory + Claude Code plugin ecosystem entry
- docs(plans): land EPIC-26 TUI implementation plan from /ultraplan
- docs: log Session 55 — implementer queue drain (10 specs, PRs #31-#40)

## [v0.7.0] — 2026-05-15

Specs merged since v0.6.0 (6):

### Features

- **TASK-107** — aida fetch: cheap two-leg refresh of remote refs (no merge, no worktree change) (#28)

### Documentation

- **EPIC-26** — AIDA TUI: process-supervisor shell wrapping claude code sessions + workflow orchestration
- **STORY-125** — AIDA integration framework: project config + smart recommendations for complementary tools (/ultraplan, /ultrareview, etc.)

### Other

- EPIC-23 batch 7: friction-fixes — auto-bump correctness + skill polish (9 specs) (#27)
- EPIC-24 batch:plan-tooling — plan lifecycle: template, verify, helpers, queue integration, /ultraplan round-trip (7 specs) (#29)
- Implementer queue drain: git-verb surface + show/queue/session display polish (10 specs) (#30)

## [v0.6.0] — 2026-05-14

Specs merged since v0.5.2 (7):

### Features

- **TASK-218** — aida queue rework SPEC (+ aida rework SPEC alias): single verb for the flip-requeue-rework pattern (#24)
- **STORY-86** — New status 'Done': distinguish 'work finished on branch' from 'merged to main' (Completed) (#21)

### Fixes

- **BUG-85** — STORY-90 auto-queue 'covers N specs' over-counts: includes referenced specs (trace comments) not just delivered ones (#23)
- **BUG-87** — TASK-71 follow-up: aida queue list --all --for X ignores --for filter (returns all items, not just X-routed) (#23)
- **BUG-88** — Agent guidance: pushing to a branch with a merged PR should warn 'PR is merged; new commits are stranded — open a follow-up PR' (#23)

### Other

- EPIC-23 batch 5: session/queue/parse polish (9 specs) (#25)
- EPIC-23 batch 6: observability — queue progress, status, batch drain, usage telemetry (6 specs) (#26)

## [v0.5.2] — 2026-05-13

Specs merged since v0.5.1 (14):

### Features

- **EPIC-24** — Living documentation: capture rationale, use cases, recipes, and tutorial seeds DURING work — book-as-living-document (#17)
- **STORY-104** — aida doc data model + aida doc add command — first-class documentation type (#17)
- **STORY-105** — /aida-doc skill — proactive doc capture at natural work checkpoints (#17)

### Fixes

- **BUG-96** — aida persist-path deletes YAML files that fail to parse (silent data loss) (#22)
- **BUG-89** — aida queue list (default) and aida queue add use different user resolution → items invisible to their own queuer (#18)
- **TASK-81** — STORY-42 follow-up: aida queue work --steal for In-Progress override (currently refuses) (#15, #16)
- **TASK-84** — STORY-42 follow-up: default permission-mode to bypassPermissions when invoked inside an AIDA worktree (.aida/ present) (#15, #16)
- **TASK-85** — STORY-42 follow-up: aida queue work PR-N resolves to the queued review story for that PR (#15, #16)

### Documentation

- **STORY-107** — Positioning + ecosystem comparison doc: 'why AIDA, how it fits' kept current as the AI/dev-tools landscape evolves (#17)
- **EPIC-23** — Session orchestration & autonomy: automated transitions between batch / PR / review lifecycles (#15)
- **STORY-86** — New status 'Done': distinguish 'work finished on branch' from 'merged to main' (Completed)

### Infrastructure

- **TASK-97** — aida pull --autorebase: opt-in safe-rebase for code-side divergence

### Internal

- **TASK-86** — /aida-pickup skill: skip the 'want me to start?' confirm in cluster mode (or add --auto flag) (#15)

### Other

- EPIC-23 batch 2: workflow hints + adversarial review augment (#19)

## [v0.5.1] — 2026-05-12

Specs merged since v0.5.0 (16):

### Features

- **STORY-42** — Story 2b: Auto-suffix preferred id when taken; prompt to confirm (#13)
- **TASK-31** — aida session start --launch: pass --name to claude (derived from scope+branch+role) for /resume picker + terminal title clarity (#12)
- **TASK-32** — [FR-215 verify] child with parent (#12)
- **TASK-33** — aida queue list --tree: group entries by parent EPIC for visual cluster overview (#12)
- **TASK-34** — TASK-76 follow-up: add 'retry later' option to held-branch pre-flight prompt + accept Ctrl+C / empty input as cancel (#12)
- **TASK-36** — aida add: warn loudly when agreed-id block is exhausted (silently switches to node-aware format today) (#12)
- **TASK-78** — aida pull: integrate merge-gate run (--gate flag or AIDA_AUTO_MERGE_GATE config) (#12)

### Fixes

- **BUG-83** — BUG-81 follow-up: aida role enter's 'Queued for this role' section shows spec_id, not agreed_id (#14)
- **BUG-84** — commit-msg hook rejects comma-separated scopes (fix(a,b): ...); conventional-commits spec allows them (#14)
- **BUG-82** — aida db merge-gate assigns agreed-IDs that collide with existing requirements (skipped collision check) (#13)
- **BUG-34** — /aida-review skill doesn't flip the auto-filed review story's status (stays Approved, never In Progress/Completed) (#12)
- **BUG-81** — aida queue list displays spec_id (long form) instead of agreed_id (short form) when both exist (#12)

### Internal

- **TASK-79** — scripts/release.sh: handle non-interactive (no-tty) invocation gracefully — auto-confirm with --yes / AIDA_RELEASE_YES env (#14)
- **TASK-80** — BUG-82 follow-up: aida db check --collisions audit command for surfacing existing agreed-id collisions (#14)
- **TASK-82** — aida init scaffolding: include Claude Code settings.json fragment that pre-allows aida-family bash commands (#14)
- **TASK-83** — STORY-42 follow-up: accept Claude Code's 'auto' permission mode (research preview); consider as default (#14)

## [v0.5.0] — 2026-05-11

Specs merged since v0.4.5 (71):

### Features

- **STORY-90** — Auto-queue PR for reviewer at PR-create time, not session-end (redesign STORY-66 trigger; idempotent re-fire as backup) (#10)
- **STORY-98** — Session manifest: capture cluster intent on /aida-pickup; show 'planned:by-X' chip in queue list (#10)
- **STORY-48** — EPIC-20 v2: lease enforcement — aida edit / aida-pickup honor session scope (#1)
- **STORY-55** — aida statusline: @SPEC defaults to session scope when no in-session activity yet (#1)
- **EPIC-19** — aida doctor: maintenance & migration commands (per-repo, hidden top-level)
- **EPIC-20** — aida session start: scoped concurrent sessions with worktree + lease
- **EPIC-21** — Code↔store commit pairing: pin orphan store SHA per code commit
- **FR-1-070** — FR-1-070 _(spec not in store)_
- **FR-1-071** — FR-1-071 _(spec not in store)_
- **FR-1-074** — FR-1-074 _(spec not in store)_
- **FR-1-077** — FR-1-077 _(spec not in store)_
- **FR-215** — CLI add command should support --parent option
- **FR-271** — id_format counter_scope: per-type vs global counter mode
- **FR-281** — aida db block verify — consistency check between nodes.toml and blocks.yaml
- **SPIKE-2** — Ability to edit existing comment
- **STORY-42** — Story 2b: Auto-suffix preferred id when taken; prompt to confirm
- **STORY-43** — Story 4: aida node acquire --hijack <id> with mark-in-place when reachable
- **STORY-44** — EPIC-9 Story 4: ~/.aida/preferences.toml — preferred_node_id + email defaults
- **STORY-46** — Expand aida add interactive prompts: title + type + description + priority
- **TASK-20** — Scaffolder: emit SessionStart hook in generate_claude_settings_json
- **TASK-24** — Write missing aida-status skill body to match its command stub
- **TASK-27** — Scaffolder: re-extract .claude/hooks/ scripts on aida scaffold apply
- **TASK-39** — Case-insensitive SPEC-ID lookup in CLI + commit-msg hook + alphanumeric node id support
- **TASK-41** — aida rel add/remove: accept positional FROM TO instead of requiring --from/--to

### Fixes

- **BUG-74** — STORY-66 auto-queue: gh detection false-negatives when gh IS on PATH (#10)
- **BUG-1-065** — BUG-1-065 _(spec not in store)_
- **BUG-1-066** — BUG-1-066 _(spec not in store)_
- **BUG-1-069** — BUG-1-069 _(spec not in store)_
- **BUG-24** — commit-msg validator: REQ_ID_PATTERN rejects multi-id parens like (TASK-20, TASK-27)
- **BUG-29** — aida show prints both 'ID:' and 'Origin ID:' even when identical
- **BUG-31** — Dispenser allocates IDs that already exist in the store (BUG-25/28/29/30 collisions)
- **BUG-35** — Human-facing CLI timestamps shown in UTC instead of local time
- **BUG-36** — aida init banner leaks raw-git instructions instead of native aida commands
- **BUG-37** — aida init silently skips node-id auto-acquire when no origin remote
- **BUG-38** — aida init --verbose hint is misleading: doesn't work after first init
- **BUG-39** — aida init fails on stale worktree registration after .aida-store dir deleted
- **BUG-40** — aida init auto-acquire requires origin, blocking solo users from getting their preferred node id
- **BUG-41** — aida docs build doesn't surface the README path in its output
- **BUG-42** — aida status flags seed-category files (CLAUDE.md, AGENTS.md) as STALE on every project
- **BUG-43** — .claude/AIDA.md flagged as STALE on a fresh init even though it's template-category and just got written
- **BUG-44** — aida push prints 'Pushing store...' header even when there's no origin to push to
- **BUG-45** — aida add with no args silently creates 'FR-N - Untitled' instead of showing help or prompting
- **BUG-46** — aida add --help / aida edit --help missed docs-layer types (vision/principle/decision/constraint/term)
- **BUG-47** — aida edit/add accepts arbitrary --status (typos land as custom_status silently)
- **BUG-48** — aida edit/add silently drops invalid --type, says 'No changes specified'
- **BUG-49** — aida edit --type silently keeps the old prefix on spec_id (no warning)
- **BUG-52** — aida session start fails to symlink .aida/ runtime subdirs when .aida/ has tracked content

### Infrastructure

- **EPIC-15** — Scaffold upgrade workflow: template/seed/managed-merge file categories
- **FR-1-076** — FR-1-076 _(spec not in store)_
- **TASK-18** — Enforce skill<->command parity in scaffolding template (CI check or make target)

### Internal

- **STORY-41** — Review PR-11: EPIC-20 batch 11: session_start robustness + extractor/search FTS5 fixes + workflow polish
- **TASK-19** — Switch aida dev activate PS1 to splice semantics for clean composition with roles

### Other

- EPIC-20 batch 11: session_start robustness + extractor/search FTS5 fixes + workflow polish (#11)
- EPIC-20 batch 2: store-walkup + session fixes + parent/tree + scope fallback (#2)
- EPIC-20 batch 3: medium-priority cluster (statusline + sessions + queue routing + review) (#3)
- EPIC-20 batch 4: session lifecycle hygiene (PS1 + sysinfo + auto-branch + show + leak detection) (#4)
- EPIC-20 batch 5: session ergonomics + activity tracking + review polish (#5)
- EPIC-20 batch 6: CI repair + `--parent` atomicity + session/queue/role ergonomics (#6)
- EPIC-20 batch 7: cache freshness pipeline + session hygiene + queue/role ergonomics (#7)
- EPIC-20 batch 8: session lifecycle hygiene + /aida-review skill + workflow polish (#8)
- EPIC-20 batch 9: listing surface consistency + CLI ergonomics polish (#9)
- [AI:claude] feat(blocks+merge-gate): counter floor + Phase 3 common types + idempotent merge-gate (FR-1-073, FR-1-258, list alignment fix)
- [AI:claude] feat(dev): activate pinning + stale-build warning + PS1 marker
- [AI:claude] feat(doctor): aida doctor scaffold + migrate-counter-scope (EPIC-19 v1)
- [AI:claude] feat(init): auto-push orphan + auto-acquire node (BUG-23 + EPIC-9 story 1)
- [AI:claude] feat(upgrade): --diff flag to vet unreleased commits before shipping
- [AI:claude] feat(ux): seven walkthrough findings — name/META/origin/comments/push/validator (BUG-25..30, FR-264)
- [AI:claude] fix(scaffold): inject AIDA header AFTER shebang for shell scripts (BUG-21 follow-up)
- [AI:claude] fix(walkthrough): six polish items from 2026-05-09 walkthrough (BUG-17 .. BUG-22)
- docs(plans): kernel-module audit — capture user mark-ups (A verdict, resolved K?, decisions)
- docs: expanded pitch (without/with framing) + docs-layers module proposal + kernel-module audit

## [v0.4.5] — 2026-05-05

Specs merged since v0.4.4 (7):

### Features

- **EPIC-1-052** — EPIC-1-052 _(spec not in store)_
- **FR-1-002** — FR-1-002 _(spec not in store)_
- **FR-1-012** — FR-1-012 _(spec not in store)_
- **FR-1-064** — FR-1-064 _(spec not in store)_

### Fixes

- **BUG-1-040** — BUG-1-040 _(spec not in store)_
- **BUG-1-051** — BUG-1-051 _(spec not in store)_

### Other

- docs: shrink OVERVIEW.md to vision + architecture; move use-case tutorials to user-guide.md

## [v0.4.4] — 2026-05-05

Specs merged since v0.4.3 (5):

### Features

- **FR-2-004** — FR-2-004 _(spec not in store)_
- **FR-2-005** — FR-2-005 _(spec not in store)_
- **TASK-1-048** — TASK-1-048 _(spec not in store)_

### Fixes

- **BUG-1-049** — BUG-1-049 _(spec not in store)_
- **BUG-1-050** — BUG-1-050 _(spec not in store)_

## [v0.4.3] — 2026-05-04

Specs merged since v0.4.2 (34):

### Features

- **EPIC-1-001** — EPIC-1-001 _(spec not in store)_
- **FR-1-011** — FR-1-011 _(spec not in store)_
- **FR-1-013** — FR-1-013 _(spec not in store)_
- **FR-1-027** — FR-1-027 _(spec not in store)_
- **FR-1-035** — FR-1-035 _(spec not in store)_
- **FR-1-037** — FR-1-037 _(spec not in store)_
- **FR-1-041** — FR-1-041 _(spec not in store)_
- **FR-1-043** — FR-1-043 _(spec not in store)_
- **FR-1-044** — FR-1-044 _(spec not in store)_
- **FR-1-047** — FR-1-047 _(spec not in store)_
- **TASK-1-015** — TASK-1-015 _(spec not in store)_
- **TASK-1-018** — TASK-1-018 _(spec not in store)_
- **TASK-1-020** — TASK-1-020 _(spec not in store)_
- **TASK-1-021** — TASK-1-021 _(spec not in store)_
- **TASK-1-022** — TASK-1-022 _(spec not in store)_
- **TASK-1-030** — TASK-1-030 _(spec not in store)_
- **TASK-1-045** — TASK-1-045 _(spec not in store)_

### Fixes

- **BUG-1-014** — BUG-1-014 _(spec not in store)_
- **BUG-1-017** — BUG-1-017 _(spec not in store)_
- **BUG-1-025** — BUG-1-025 _(spec not in store)_
- **BUG-1-034** — BUG-1-034 _(spec not in store)_
- **BUG-1-038** — BUG-1-038 _(spec not in store)_
- **BUG-1-039** — BUG-1-039 _(spec not in store)_
- **BUG-1-046** — BUG-1-046 _(spec not in store)_

### Documentation

- **SPIKE-1-029** — SPIKE-1-029 _(spec not in store)_

### Infrastructure

- **TASK-1-031** — TASK-1-031 _(spec not in store)_
- **TASK-1-032** — TASK-1-032 _(spec not in store)_

### Other

- [AI:claude] docs(plans): requirements DB vetting pass summary (2026-05-04)
- [AI:claude] feat(scaffold): aida scaffold upgrade — category-aware upgrades (FR-1-028 v1)
- [AI:claude] fix(history): convert YAML modified_at from UTC to local time
- [AI:claude] fix(history): pad columns before colorizing for proper alignment
- [AI:claude] fix(scaffold): seeds use marker-presence semantics, not whole-file equality (FR-1-028 v1.1)
- docs(scaffolding): strengthen /aida-capture guidance in scaffolded CLAUDE.md
- fix(scaffolding): hook commands use \$CLAUDE_PROJECT_DIR, not relative paths

## [v0.4.2] — 2026-05-03

Specs merged since v0.4.1 (2):

### Features

- **EPIC-1-001** — EPIC-1-001 _(spec not in store)_

### Other

- fix(scaffolding): don't prepend HTML-comment header to JSON files

## [v0.4.1] — 2026-05-03

Specs merged since v0.4.0 (2):

### Features

- **EPIC-1-001** — EPIC-1-001 _(spec not in store)_

### Other

- docs: README install section now points devs at the dev workflow

## [v0.4.0] — 2026-05-02

Specs merged since v0.3.0 (20):

### Features

- **EPIC-1-001** — EPIC-1-001 _(spec not in store)_
- **EPIC-3** — Authentication Epic: User Authentication & Authorization
- **FR-10** — PIN-based Web Client Authentication

### Other

- [AI:claude] feat(core): add Jira Cloud integration with configurable field mapping
- [AI:claude] feat(core): add analytics engine and REST endpoint
- [AI:claude] feat(core): review enhancements — rule IDs, config, catalog, trending, diff-aware, cargo-deny, vale
- [AI:claude] feat(jira): add sync command with drift detection
- [AI:claude] feat(skills): add /aida-code-review — exhaustive code quality review
- [AI:claude] feat(skills): add /aida-docs-review — exhaustive documentation quality review
- [AI:claude] feat(web): add "me" button to owner field for quick self-assignment
- [AI:claude] feat(web): add Jira Sync dashboard tab with REST endpoint
- [AI:claude] feat: first-class Codex CLI support and crates.io prep
- [AI:claude] fix(cli): MCP server works with distributed git backend
- [AI:claude] fix(jira): fix API compatibility and default type mapping
- [AI:claude] fix(web): My Activity matches authors with @ prefix and name variants
- agreed_id
- docs: add code review skill research — 686 lines covering Rust tooling and best practices
- docs: archive 22 stale planning + design docs to docs/archive/
- docs: major surgery on CLAUDE.md (352 -> ~150 lines)
- new docs/UNDERSTANDING_SKILLS.md

## [v0.3.0] — 2026-03-17

Specs merged since v0.2.0 (5):

### Other

- [AI:claude] feat(core): add telemetry/observability layer for measuring AIDA effectiveness
- [AI:claude] feat(core): daemon dispenser (Phase 3) and oplog integration
- docs: add competitive analysis — requirements management in the agentic era
- docs: fix inconsistencies identified in docs review
- docs: rewrite README, add future vision, docs review report

## [v0.2.0] — 2026-03-17

Specs merged since v0.1.0 (5):

### Other

- [AI:claude] feat(skills): vertical slice planning, grill skill, git guardrails
- [AI:claude] feat: crates.io ready, GitHub sync, React agreed_id, skills review
- [AI:claude] feat: multi-repo workspace, operation log, and 4 new skills
- docs: update PROMPT_HISTORY with final session entries
- docs: update PROMPT_HISTORY with worktree mode, docs, and v0.1.0 release

## [v0.1.0] — 2026-03-16

Specs merged since the start of history (681):

### Features

- **EPIC-0365** — EPIC-0365 _(spec not in store)_
- **FR-0146** — FR-0146 _(spec not in store)_
- **FR-0148** — FR-0148 _(spec not in store)_
- **FR-0152** — FR-0152 _(spec not in store)_
- **FR-0153** — FR-0153 _(spec not in store)_
- **FR-0172** — FR-0172 _(spec not in store)_
- **FR-0175** — FR-0175 _(spec not in store)_
- **FR-0183** — FR-0183 _(spec not in store)_
- **FR-0184** — FR-0184 _(spec not in store)_
- **FR-0188** — FR-0188 _(spec not in store)_
- **FR-0191** — FR-0191 _(spec not in store)_
- **FR-0226** — FR-0226 _(spec not in store)_
- **FR-0227** — FR-0227 _(spec not in store)_
- **FR-0232** — FR-0232 _(spec not in store)_
- **FR-0281** — FR-0281 _(spec not in store)_
- **FR-0283** — FR-0283 _(spec not in store)_
- **FR-0285** — FR-0285 _(spec not in store)_
- **FR-0295** — FR-0295 _(spec not in store)_
- **FR-0297** — FR-0297 _(spec not in store)_
- **FR-0298** — FR-0298 _(spec not in store)_
- **FR-0299** — FR-0299 _(spec not in store)_
- **FR-0309** — FR-0309 _(spec not in store)_
- **FR-0315** — FR-0315 _(spec not in store)_
- **FR-0316** — FR-0316 _(spec not in store)_
- **FR-0318** — FR-0318 _(spec not in store)_
- **FR-0319** — FR-0319 _(spec not in store)_
- **REQ-0231** — REQ-0231 _(spec not in store)_
- **SPEC-198** — SPEC-198 _(spec not in store)_
- **STORY-0321** — STORY-0321 _(spec not in store)_
- **STORY-0322** — STORY-0322 _(spec not in store)_
- **STORY-0323** — STORY-0323 _(spec not in store)_
- **STORY-0324** — STORY-0324 _(spec not in store)_
- **STORY-0325** — STORY-0325 _(spec not in store)_
- **STORY-0326** — STORY-0326 _(spec not in store)_
- **STORY-0327** — STORY-0327 _(spec not in store)_
- **STORY-0369** — STORY-0369 _(spec not in store)_
- **STORY-0372** — STORY-0372 _(spec not in store)_
- **STORY-0374** — STORY-0374 _(spec not in store)_
- **STORY-0375** — STORY-0375 _(spec not in store)_
- **TASK-0373** — TASK-0373 _(spec not in store)_

### Fixes

- **BUG-0308** — BUG-0308 _(spec not in store)_
- **BUG-0381** — BUG-0381 _(spec not in store)_
- **STORY-0367** — STORY-0367 _(spec not in store)_
- **TASK-0374** — TASK-0374 _(spec not in store)_

### Documentation

- **EPIC-0320** — EPIC-0320 _(spec not in store)_
- **FR-0220** — FR-0220 _(spec not in store)_
- **FR-0221** — FR-0221 _(spec not in store)_
- **FR-0222** — FR-0222 _(spec not in store)_
- **REQ-0219** — REQ-0219 _(spec not in store)_
- **REQ-0303** — REQ-0303 _(spec not in store)_
- **STORY-0376** — STORY-0376 _(spec not in store)_

### Infrastructure

- **BUG-0380** — BUG-0380 _(spec not in store)_
- **STORY-0379** — STORY-0379 _(spec not in store)_

### Other

- Revert "[AI:claude] feat: add resizable splitter to Settings sidebar"
- Revert "[AI:claude] fix: Settings sidebar width adapts to font size"
- [AI:claude:high] docs: add commit message AI attribution guidance to aida-implement skill
- [AI:claude:high] docs: document commit message attribution format
- [AI:claude:high] docs: fix aida comment add syntax in skill files
- [AI:claude:high] docs: fix delete command to use 'del' in user guide
- [AI:claude:high] docs: use placeholder SPEC-ID in trace comment examples
- [AI:claude:high] feat: add AIDA_AUTHOR environment variable for AI authorship
- [AI:claude:high] feat: add Purple Rain theme and vertical sidebar tabs in settings
- [AI:claude:high] feat: add WASM browser client (aida-web)
- [AI:claude:high] feat: add make targets to stop running servers
- [AI:claude:high] feat: add platform abstraction layer for unified GUI (Phase 1)
- [AI:claude:high] feat: add shared UI components for native/WASM code reuse
- [AI:claude:high] feat: add version/checksum protection for scaffolded files
- [AI:claude:high] feat: support positional arguments for aida comment add
- [AI:claude:high] fix: add Purple Rain to Ctrl+T theme cycling
- [AI:claude:high] fix: constrain settings dialog content width to prevent overflow
- [AI:claude:high] fix: display timestamps in local time instead of UTC
- [AI:claude:high] fix: handle SQLite in save_with_conflict_detection and update_atomically
- [AI:claude:high] fix: make settings dialog fixed size with scrollable content
- [AI:claude:high] fix: settings dialog layout - top alignment and close button position
- [AI:claude:high] fix: settings dialog subtab width and Purple Rain theme selector
- [AI:claude:high] fix: update scaffolding templates with grep examples and placeholders
- [AI:claude] docs(make): add AIDA_DEV_MODE and ANTHROPIC_API_KEY to make help
- [AI:claude] docs(spike): git scaling test - one-file-per-object viable at 100K
- [AI:claude] docs: add implementation plan for My Activity feature
- [AI:claude] docs: add plan archival workflow and update docs
- [AI:claude] docs: rewrite README for current project state
- [AI:claude] docs: update OVERVIEW, CLAUDE.md, and PROMPT_HISTORY for React dashboard
- [AI:claude] docs: update OVERVIEW, CLAUDE.md, and PROMPT_HISTORY for scaffolding modernization
- [AI:claude] docs: update PROMPT_HISTORY and OVERVIEW with evaluate endpoint
- [AI:claude] docs: update docs for sprint view feature
- [AI:claude] docs: update documentation for My Activity feature
- [AI:claude] docs: update documentation for advanced query builder feature
- [AI:claude] docs: update documentation for owner-scoped queues feature
- [AI:claude] feat(admin): add runtime API key management via Settings UI
- [AI:claude] feat(ci): add GitHub Actions release workflow and package metadata
- [AI:claude] feat(cli): add GitHub pull (import issues) and update docs
- [AI:claude] feat(cli): add MCP server for Claude Code integration (Phase 4)
- [AI:claude] feat(cli): add `aida init` command for project bootstrapping
- [AI:claude] feat(cli): add aida init --distributed and git backend CLI integration
- [AI:claude] feat(cli): add db status, conflict-aware sync, updated init output
- [AI:claude] feat(cli): add edit, delete, search, comment commands to git backend
- [AI:claude] feat(cli): add orphan branch + worktree as default distributed mode
- [AI:claude] feat(cli): add sync, relationships, export-git, and REST API support
- [AI:claude] feat(cli): auto-detect distributed store from .aida/config.toml
- [AI:claude] feat(core): add GitHub integration — client, config, and CLI commands
- [AI:claude] feat(core): add Meta requirement type for database configuration
- [AI:claude] feat(core): add SqliteDispenser — Phase 2 sequence generation
- [AI:claude] feat(core): add UUID v7, HLC timestamps, and sequence dispenser
- [AI:claude] feat(core): add conflict detection and resolution module
- [AI:claude] feat(core): add git operations, node registration CAS loop, and wire GitBackend into CLI
- [AI:claude] feat(core): add git-backed DatabaseBackend using sharded YAML object store
- [AI:claude] feat(core): add node identity, workspace config, and deployment mode
- [AI:claude] feat(core): add sharded YAML object store for git-based storage
- [AI:claude] feat(core): auto-commit git changes + SqliteDispenser (Phase 2)
- [AI:claude] feat(core): implement two-tier ID scheme with merge gate
- [AI:claude] feat(core): wire Dispenser into RequirementsStore ID generation
- [AI:claude] feat(dev): add make dev workflow with PostgreSQL + hot-reload
- [AI:claude] feat(docker): add Docker quickstart with single-container deployment
- [AI:claude] feat(gui): add UrlOpenMode for URL links (FR-URL-OPEN)
- [AI:claude] feat(ops): add multi-user PostgreSQL setup script and serve target
- [AI:claude] feat(scaffolding): add 6 new skills — test, review, onboard, sprint, search, standup (Phase 3)
- [AI:claude] feat(scaffolding): add hooks, update CLAUDE.md gen, bump to v1.2.0 (Phase 3)
- [AI:claude] feat(scaffolding): add org template layer, bump to v2.0.0 (Phase 5)
- [AI:claude] feat(server): add .env file support via dotenvy
- [AI:claude] feat(server): add reload endpoint and automatic mtime-based reload
- [AI:claude] feat(skills): add web UI skill invocation with compiler-warnings pilot
- [AI:claude] feat(web): add Completed stat and status breakdown to Activity stats bar
- [AI:claude] feat(web): add Metrics tab to Sprint Planning with sprint picker
- [AI:claude] feat(web): add My Activity page with planned vs. actual work reconciliation
- [AI:claude] feat(web): add New Project scaffolding tab in Settings
- [AI:claude] feat(web): add React dashboard with kanban, list, and detail views
- [AI:claude] feat(web): add Settings view with backend CRUD endpoints
- [AI:claude] feat(web): add Timeline view with chronological event feed
- [AI:claude] feat(web): add advanced query builder to List View
- [AI:claude] feat(web): add colored text markdown syntax via ::color[text]
- [AI:claude] feat(web): add docs browser view with markdown rendering
- [AI:claude] feat(web): add expand, preview, and markdown help to description editor
- [AI:claude] feat(web): add global Create Requirement button with quick-create dropdown and full modal
- [AI:claude] feat(web): add inline editing to detail panel
- [AI:claude] feat(web): add open-in-new-tab for docs viewer
- [AI:claude] feat(web): add open-in-new-tab for requirement detail panel
- [AI:claude] feat(web): add parent/child tree toggle to List View
- [AI:claude] feat(web): add refresh button to header for server data reload
- [AI:claude] feat(web): add search to skills browser
- [AI:claude] feat(web): add skills browser view with API and components
- [AI:claude] feat(web): add sprint create, archive, and charts
- [AI:claude] feat(web): add sprint edit, close, and carry-over modals
- [AI:claude] feat(web): add sprint summary and clickable status cards on dashboard
- [AI:claude] feat(web): add sprint view with planning and backlog management
- [AI:claude] feat(web): add syntax highlighting for code blocks in markdown
- [AI:claude] feat(web): add tag filtering, structured search, and markdown descriptions
- [AI:claude] feat(web): add ts-rs TypeScript type generation for all models
- [AI:claude] feat(web): make requirement type editable in detail panel
- [AI:claude] feat(web): navigate sprint status cards to list with advanced sprint filter
- [AI:claude] feat(web): render skill content as markdown preview
- [AI:claude] feat(web): search skill content in skills browser
- [AI:claude] feat(web): show active sprint in sidebar with progress
- [AI:claude] feat: add aida-evaluate, aida-commit, aida-sync to scaffold manifest
- [AI:claude] feat: add git hooks checking to /aida-sync skill
- [AI:claude] feat: add resizable splitter to Settings sidebar
- [AI:claude] feat: propagate agreed_id to TypeScript types, proto, and gRPC
- [AI:claude] fix(chat): show actual error details and update default model
- [AI:claude] fix(cli): improve show/search display with agreed IDs and relations
- [AI:claude] fix(cli): wire FileDispenser into git backend for distributed IDs
- [AI:claude] fix(core): merge gate uses requirement type prefix for agreed IDs
- [AI:claude] fix(docker): use bind-mount for project database instead of named volume
- [AI:claude] fix(gui): exclude Templates view from general keyboard navigation
- [AI:claude] fix(server): check WAL file mtime for SQLite auto-reload
- [AI:claude] fix(skills): show diff summary and suggest re-run after auto-fix
- [AI:claude] fix(skills): use axum 0.7 route syntax (:name not {name})
- [AI:claude] fix(web): dedupe React in Vite config to fix Advanced filter crash
- [AI:claude] fix(web): exclude Folder/Meta/Sprint types from sprint filter URL
- [AI:claude] fix(web): fix kanban drag-and-drop between columns
- [AI:claude] fix(web): move title to own row in EditSprintModal for long titles
- [AI:claude] fix(web): resolve TypeScript strict-mode errors for Docker build
- [AI:claude] fix(web): show all activity when userId is 'default'
- [AI:claude] fix(web): start advanced query panel collapsed even when aq= param is present
- [AI:claude] fix(web): use sprint UUID for advanced filter instead of spec_id
- [AI:claude] fix(web+server): persist requirement edits by invalidating detail panel cache
- [AI:claude] fix: Settings sidebar scales with font size
- [AI:claude] fix: Settings sidebar width adapts to font size
- [AI:claude] fix: make Settings dialog resizable
- [AI:claude] fix: update stale test assertions and add missing tempfile dev-dep
- [AI:claude] perf(web): disable React StrictMode for faster dev experience
- [AI:claude] refactor(docker): move compose to .aida/ to avoid conflicts with project Docker files
- [AI:claude] refactor(scaffolding): consolidate templates + add frontmatter (Phase 1)
- [AI:claude] refactor: simplify AI attribution format - high confidence now implied
- docs: add AIDA capabilities slideshow presentation
- docs: add FR-0159 for icon editor 4-column grid layout
- docs: add FR-0166 for Settings/AI Prompts and Skills subtabs
- docs: add FR-0167 for auto-populate user settings
- docs: add FR-0168 for AIDA-aware CLAUDE.md and /aida-capture skill
- docs: add FR-0170 for IMPL type and separate exports
- docs: add HTML versions of documentation with dark mode support
- docs: add IMPL-0171 for stateless type implementation
- docs: add IMPL-0174 for --parent option implementation
- docs: add Markdown Help split panel entry to PROMPT_HISTORY
- docs: add SPEC-ID implementation completion summary
- docs: add Session 13 entry for right-click context menu
- docs: add Sprint/Epic planning design and requirements
- docs: add UUID ↔ SPEC-ID mapping verification
- docs: add WHY-AIDA.md — strategic vision, competitive analysis, and roadmap
- docs: add administrator's guide and update user's guide with storage info
- docs: add comprehensive Developer's Guide
- docs: add comprehensive documentation for context menu workarounds
- docs: add comprehensive storage modes guide
- docs: add database export step to release workflow
- docs: add detailed use cases and tutorials for new features
- docs: add distributed architecture & identity specification v0.5
- docs: add final integration recommendation
- docs: add final status summary
- docs: add git scaling spike results — one-file-per-object viable at 100K
- docs: add integration complete summary
- docs: add integration documentation index
- docs: add integration plan for ai-provenance
- docs: add integration review - cleanup complete
- docs: add integration summary
- docs: add main branch improvements plan and update distributed spec flexibility notes
- docs: add missing plan files for React dashboard, sprint view, sprint metrics, and timeline view
- docs: add missing requirements for Save As and Session Tracking
- docs: add multi-user PostgreSQL setup guide
- docs: add pre-commit hook section to CLAUDE.md
- docs: add prior art research on storing metadata in git
- docs: add project documentation files
- docs: add resizable panel divider entry to PROMPT_HISTORY
- docs: add screenshots to AIDA slideshow presentation
- docs: add simplified integration approach
- docs: add sprint close/carryover to prompt history and plans
- docs: add strategic growth plan — bootstrapping, auth, Docker, YAML/MCP, CLAUDE.md
- docs: add title bar styling session to PROMPT_HISTORY
- docs: add two-tier ID scheme design document
- docs: archive auto-export yaml hook plan
- docs: archive rename plan to docs/plans/
- docs: design for adding SPEC-ID as alternate key in Requirement
- docs: document `aida init` command in CLAUDE.md
- docs: enhance user guide with comprehensive feature coverage
- docs: fix aida-req skill to use correct CLI type values
- docs: fix duplicate SPEC-IDs and add FR-0165 for smart parent selection
- docs: mark tag filtering plan as completed
- docs: modernize OVERVIEW.md, user guide, and add Getting Started guide
- docs: record pre-commit hook setup in PROMPT_HISTORY and OVERVIEW
- docs: refine WHY-AIDA.md competitive analysis — AI-native vs AI-bolted-on
- docs: regenerate HTML versions of guides
- docs: regenerate user-guide.html from updated markdown
- docs: rename req to aida and add cross-links between guides
- docs: revise bootstrapping strategy — full scaffold by default, not tiers
- docs: rewrite getting-started guide for new users
- docs: update CLAUDE.md and PROMPT_HISTORY.md for Timeline view
- docs: update CLAUDE.md with distributed mode documentation
- docs: update CLAUDE.md, OVERVIEW.md, PROMPT_HISTORY.md for description editor enhancements
- docs: update OVERVIEW and PROMPT_HISTORY for unified storage
- docs: update OVERVIEW.md with aida-desktop naming consistency
- docs: update OVERVIEW.md with gRPC server documentation
- docs: update PROMPT_HISTORY and OVERVIEW for GitLab integration
- docs: update PROMPT_HISTORY and OVERVIEW for multi-project support
- docs: update PROMPT_HISTORY and OVERVIEW with distributed architecture session
- docs: update PROMPT_HISTORY with --force option session
- docs: update PROMPT_HISTORY with Docker fixes and captured requirements
- docs: update PROMPT_HISTORY with Edit view layout gap fix
- docs: update PROMPT_HISTORY with Edit/Add form redesign
- docs: update PROMPT_HISTORY with STORY-0321/0322 implementation
- docs: update PROMPT_HISTORY with STORY-0323 implementation
- docs: update PROMPT_HISTORY with STORY-0324 implementation
- docs: update PROMPT_HISTORY with STORY-0325 implementation
- docs: update PROMPT_HISTORY with Templates view implementation
- docs: update PROMPT_HISTORY with add menu popup feature
- docs: update PROMPT_HISTORY with database change detection feature
- docs: update PROMPT_HISTORY with deployment fixes
- docs: update PROMPT_HISTORY with distributed architecture implementation session
- docs: update PROMPT_HISTORY with layout fix and Developer Guide
- docs: update PROMPT_HISTORY with layout-aware form views
- docs: update PROMPT_HISTORY with list panel max width fix
- docs: update PROMPT_HISTORY with recent session work
- docs: update PROMPT_HISTORY with resizable panels and keyboard nav fix
- docs: update PROMPT_HISTORY with seamless edit transition fix
- docs: update PROMPT_HISTORY with session bug fixes and captured requirements
- docs: update PROMPT_HISTORY with simplified list panel fix
- docs: update PROMPT_HISTORY with stacked layout Edit view fix
- docs: update PROMPT_HISTORY with theme files and modal constraints
- docs: update PROMPT_HISTORY with title truncation fix
- docs: update PROMPT_HISTORY.md with FR-0295 session
- docs: update PROMPT_HISTORY.md with aida-gui rename session
- docs: update documentation for GUI gRPC client support
- docs: update documentation for WASM browser client
- docs: update documentation for dual-target GUI compilation
- docs: update documentation for shared UI components
- docs: update integration summary to reflect completion
- docs: update prompt history and CLAUDE.md for dashboard sprint summary
- feat(core): add meta seeding and prompt fallback for database-stored prompts
- feat(core,cli,gui): add Meta requirement type and tree export/import
- feat(docker): add AIDA containerization with PostgreSQL support
- feat(docker): add HTTP routers for Cloudflare tunnel support
- feat(docker): add cloudflared to docker-compose for self-contained tunnel
- feat(gui): add Ctrl+Enter as save shortcut for browser
- feat(gui): add Templates view for browsing embedded skills and prompts
- feat(gui): add Web Preview tab for URL iframe display
- feat(gui): add keyboard navigation to Templates view
- feat(gui): add leader key '=' for zoom/theme and 'P' for project picker
- feat(gui): add resizable divider between list and detail panels
- feat(gui): add resizable panels to Timeline, Queue, and Templates views
- feat(gui): add right-click context menu for requirements
- feat(gui): add storage type selection to Create New Project dialog
- feat(multi-project): add multi-project support for aida-server and GUI
- feat(scaffolding): add Claude Code hooks for AIDA integration
- feat(web): simplify saved views menu and editable view settings
- feat: add $USER-XXX meta-type IDs for users
- feat: add '?' hotkey to show keyboard shortcuts help
- feat: add 'A' (shift+a) hotkey for AI Actions popup menu
- feat: add 'Copy for Claude Code' button to AI menu
- feat: add 'Show Parents' toggle to filter panel for tree views
- feat: add 'a' hotkey popup for new sibling/child requirements
- feat: add 'del' command to delete requirements
- feat: add 'f' hotkey for feature picker with fuzzy search
- feat: add 'p' key shortcut for priority popup with generic quick-change system
- feat: add 'r' detail tabs and 'T' type picker popup menus
- feat: add '✦' as status icon for AI status
- feat: add --force option to aida-server to kill existing processes
- feat: add /aida-capture skill and improve scaffolding CLAUDE.md
- feat: add /aida-compiler-warnings skill
- feat: add /aida-evaluate slash command and skill
- feat: add 200ms hover delay to AI submenu in Actions dropdown
- feat: add AI Integration Report and scaffold status check
- feat: add AI Integration tab with report generation
- feat: add AI integration design document and initial AI menu
- feat: add AI prompt configuration for AIDA project
- feat: add AI prompts configuration and template
- feat: add Agile requirement types and project templates
- feat: add Apply button for AI suggested description improvements
- feat: add Bug requirement type
- feat: add Bug, Epic, Task, Spike as built-in types
- feat: add Claude Code skills for requirements-driven development
- feat: add Clone action for requirements
- feat: add Ctrl+Arrow keys to move Kanban cards between columns
- feat: add Ctrl+H/L to move Kanban cards between columns
- feat: add Ctrl+N keybinding for new requirement
- feat: add Ctrl+S save shortcut for Edit/Add forms
- feat: add Ctrl+T keyboard shortcut to cycle through themes
- feat: add Docs Dark theme inspired by documentation site styling
- feat: add ESC key handling with unsaved changes confirmation for Add/Edit forms
- feat: add Enter key to edit selected requirement
- feat: add Epic and Spike Agile types with custom fields
- feat: add FR-0148 for background AI Find Duplicates
- feat: add GUI integration for relationship definitions (Phase 4)
- feat: add GUI keyboard navigation improvements and queue view fixes
- feat: add ID migration support with validation
- feat: add ID prefix filtering and management
- feat: add IMPL type and separate spec/impl exports
- feat: add Makefile with comprehensive build targets
- feat: add Markdown support for requirement descriptions
- feat: add My Queue and Other User Queue to View menu
- feat: add New Window menu option to open additional instances
- feat: add Nord Light theme
- feat: add Open Project menu and sample project database
- feat: add Open Report button for AI Integration Report
- feat: add PIN-based user authentication for WASM web client
- feat: add Page Up/Down, Home/End, and mouse wheel navigation
- feat: add Planned status and /aida-plan skill for pre-implementation planning
- feat: add Prompts and Skills subtabs to Settings/AI section
- feat: add Quit option to Menu dropdown
- feat: add Reference picker modal to Links tab
- feat: add Restart option to Menu with cargo run detection
- feat: add SPEC-ID as alternate key in Requirement model
- feat: add Save As menu option and improve load error logging
- feat: add Storage Backend section to Settings/Db tab
- feat: add Team Management requirements (FR-0184 to FR-0187)
- feat: add Timeline view for requirements history and external integration architecture
- feat: add Vibrant Light theme with colorful accents
- feat: add WASM compatibility feature flags to aida-core and aida-gui
- feat: add aida grep command for searching requirements
- feat: add aida slash commands for requirements workflow
- feat: add aida slash commands to project scaffolding
- feat: add aida-docs skill for documentation management
- feat: add aida-release skill to scaffold
- feat: add arrow key navigation for requirements list
- feat: add background AI evaluation system with AI tab in GUI
- feat: add baseline/versioning infrastructure for requirements
- feat: add baselines management UI
- feat: add change history tracking to requirements
- feat: add clear button to search boxes and fix arrow navigation
- feat: add clickable Markdown help modal in form view
- feat: add code-to-requirement traceability (TraceLink, ImplementationInfo)
- feat: add collaborative session tracking with heartbeat
- feat: add collapse/expand buttons for Requirements List panel
- feat: add collapseable comment trees to GUI
- feat: add collapsible detail panel for list-only view
- feat: add configurable UI title heading size
- feat: add configurable emoji reactions for comments
- feat: add configurable title bar styling in detail view
- feat: add configurable toast notifications for queue operations
- feat: add context/scope system for keybindings
- feat: add created_by tracking to requirements and relationships
- feat: add custom ID prefix override for requirements
- feat: add custom type definitions with type-specific statuses and fields
- feat: add customizable AI prompts configuration
- feat: add customizable dim/weak text color for completed items
- feat: add customizable keyboard shortcuts in settings
- feat: add customizable status and priority icons
- feat: add database abstraction layer with SQLite support
- feat: add database change detection with auto-reload
- feat: add database name stored in YAML and displayed in window title
- feat: add database title and description fields
- feat: add delete button for custom themes
- feat: add delete/archive menu popup (d key) and update Kanban shortcut to Shift+K
- feat: add detail modal and edit for KanBan cards
- feat: add distinct sizes for markdown headings H1-H6
- feat: add double-click support for layout menu
- feat: add double-click to edit in Detail view
- feat: add export command for mapping file
- feat: add external URL links to requirements
- feat: add file attachments support for requirements
- feat: add file locking for multi-user support and Settings IDs subtabs
- feat: add flexible relationship system for requirements
- feat: add gRPC-Web and REST API support to server
- feat: add git hooks for code traceability validation
- feat: add highlight search mode with vim-style navigation
- feat: add inline word diff for Timeline changes and fix ghost highlight
- feat: add keyboard navigation to Kanban board
- feat: add keyboard navigation to Settings sidebar
- feat: add keyboard navigation to Timeline view
- feat: add layout-aware form views matching current view mode
- feat: add live preview for appearance settings
- feat: add long-press layout menu with click-to-cycle behavior
- feat: add manual prefix text input in Edit form
- feat: add missing relationship definitions for Sprint planning
- feat: add multi-select tag picker popup with 't' hotkey
- feat: add navigation lock for synchronized list scrolling
- feat: add personal work queue for user-managed task prioritization
- feat: add preferred view setting to user preferences
- feat: add project settings tab for requirement ID configuration
- feat: add quick status change popup via 's' key shortcut
- feat: add recursive tree view for relationships with cycle protection
- feat: add relationship definition system with constraints
- feat: add resizable split to stacked detail view layout
- feat: add right-click context menu with Cut/Copy/Paste to text fields
- feat: add run-server-force make target
- feat: add search and filter dialog for both lists in split layouts
- feat: add search scope filters (Title, Description, Comments, ID)
- feat: add skill editor modal with markdown preview and edit
- feat: add spacebar to expand/collapse tree nodes
- feat: add split panel for second requirements list view
- feat: add split panel to Markdown Help with syntax reference and preview
- feat: add sprint selection in Sprint Planning view
- feat: add status and priority colors to theme editor
- feat: add status and priority filtering in GUI
- feat: add status visualization for requirements
- feat: add success color and hyperlink preview to theme editor
- feat: add theme defaults for HighContrastDark, SolarizedDark, and Nord
- feat: add theme editor with full visual customization
- feat: add theme selection in user preferences
- feat: add two-level filter system for root/children in tree views
- feat: add type definition editor in Settings
- feat: add unified entry point for native/WASM builds (Phase 3 prep)
- feat: add unified layout controls to menu bar
- feat: add unified storage abstraction with StorageClient trait
- feat: add user queue picker (q u) to view another user's items
- feat: add user-defined theme files in ~/.config/aida/themes/
- feat: add view picker hotkey 'v' with two-key sequence
- feat: add view presets to save perspective, direction, and filter combinations
- feat: add web-serve-force make target
- feat: add weight field and 'w' hotkey for effort/story points
- feat: align Sprint Planning view behavior with Timeline view
- feat: apply theme editor changes globally for live preview
- feat: auto-populate title from description in Add form
- feat: auto-populate user settings from git config and environment
- feat: auto-repair duplicate SPEC-IDs on load
- feat: auto-scroll requirements list when dragging near edges
- feat: change default perspective to Parent/Child
- feat: collapsible left panel in edit/add mode
- feat: constrain modal windows to percentage of window size
- feat: double-click description to enter Edit mode with focus
- feat: double-click on requirement opens it for editing
- feat: dynamically update ID preview when Type changes in Edit form
- feat: embed DejaVu Sans font for cross-platform Unicode support
- feat: enable aida-gui dual-target compilation for native and WASM
- feat: enhance Sprint Planning view with detail panel, sprint picker, and drag-and-drop
- feat: enhance filter dialogs with View Settings including perspective and direction
- feat: enrich trace comment format with title, date, and author
- feat: externalize templates with build.rs embedding for release binaries
- feat: implement AI integration with Claude CLI backend
- feat: implement Sprint Planning feature with Planning View
- feat: implement focus tracking between List 1 and List 2
- feat: implement full CRUD operations in GUI
- feat: implement tabbed interface with history in GUI
- feat: implement threaded comment system
- feat: implement type-specific priorities in Add/Edit form
- feat: improve Settings dialog UX with conditional Save/Cancel
- feat: improve form layout with full-width title and description
- feat: improve recursive relationship tree display
- feat: improve search UX and tone down loud colors
- feat: improve stateless type support in Settings UI
- feat: improve status popup behavior
- feat: increase Status & Priority Icons dialog size
- feat: integrate storage_client into RequirementsApp
- feat: load system font with better Unicode support
- feat: make GrpcStorageClient WASM-compatible with dual-target support
- feat: make KanBan card detail click action configurable
- feat: make comment content field full width
- feat: make scaffolding database-aware
- feat: make split list panels equal width when detail is hidden
- feat: multi-column keyboard shortcuts help based on window width
- feat: redesign Edit/Add form to match Detail View layout
- feat: refactor app.rs to use platform abstraction (Phase 2)
- feat: rename application to AIDA with project management
- feat: replace Archive/Delete buttons with Quick Actions dropdown
- feat: responsive detail view layout based on orientation
- feat: restore missing traceability requirements and add concurrency bug
- feat: restructure into workspace with CLI and GUI
- feat: run AI evaluation in background thread with toast notifications
- feat: select and scroll to newly added requirement
- feat: show greyed-out ancestors for filtered tree items
- feat: show selected text preview in context menu
- feat: show theme name in menu bar when cycling themes
- feat: smart parent selection for new requirements
- feat: suppress hover highlight in Timeline after click/keyboard nav
- feat: use exponential scaling for markdown headings
- feat: use ✦ icon for AI tab in Settings dialog
- feat: wrap status/priority icons after every 4 items in icon editor
- fix(docker): add REST API routing for multi-project support
- fix(docker): add default server URL redirect for WASM client
- fix(docker): add explicit service links for Traefik routers
- fix(docker): add router priorities for REST vs gRPC routing
- fix(docker): fix podman heredoc parsing and WASM compile errors
- fix(docker): load .env file for ANTHROPIC_API_KEY in container
- fix(docker): resolve Cloudflare SRI and SSL issues
- fix(docker): route aida-server traffic through proxy network
- fix(docker): route gRPC-Web to correct port 50051
- fix(docker): use fully-qualified image names for podman compatibility
- fix(gui): add auto-scroll and fix keyboard conflicts in Planning/Kanban views
- fix(gui): add scrollbars to detail view tabs and content
- fix(gui): deserialize projects response wrapper correctly
- fix(gui): handle camelCase field names from REST API
- fix(gui): improve KanBan view keyboard navigation
- fix(gui): resolve three UI bugs in hotkeys, toasts, and type change
- fix(gui): revert Timeline view to use internal layout
- fix(gui): skip gRPC load when no project selected in WASM
- fix(gui): update KanBan detail preview when navigating
- fix(gui): use Rc<RefCell> for async loading flags in project selector
- fix(gui): use egui context_menu for reliable right-click handling
- fix(gui,server): fix GitLab polling errors and compile issues
- fix(server): avoid blocking_write in async runtime context
- fix(server): resolve .claude/ and docs/ relative to database path
- fix(server): resolve docs/ relative to database path, not CWD
- fix: AI action hotkeys now use actual implementation instead of placeholder
- fix: CLI --file argument now properly overrides auto-detection
- fix: DocsDark theme now properly shows dim/weak text
- fix: ESC key saves and returns to detail view in Edit mode
- fix: Markdown Help modal resizing and scroll alignment
- fix: Markdown Help scrollbar positioning
- fix: Sprint Planning drag-and-drop now properly creates relationships
- fix: Storage now auto-detects SQLite vs YAML by file extension
- fix: add DocsDark to Ctrl+T theme cycling
- fix: add Folder type to vertical form layout dropdown
- fix: add ListOnly layout mode for single-list view
- fix: add OpenFeaturePicker to KeyAction::all() for settings migration
- fix: add SQLite backend support for add_requirement_atomic
- fix: add attachments field to gRPC convert.rs for Requirement struct
- fix: add horizontal scrollbar to Timeline detail panel
- fix: add missing themes to settings dropdown
- fix: add unique id_salt to theme editor ScrollAreas
- fix: adjust dark theme text colors to match egui defaults
- fix: all built-in themes now properly show dim/weak text
- fix: allow Requirements Panel to resize narrower with horizontal scroll
- fix: always show search mode toggle button
- fix: arrow key navigation follows tree view display order
- fix: auto-title sync now works in correct form functions
- fix: block list navigation when status popup is open
- fix: capture text selection continuously while TextEdit has focus
- fix: clear search box when restoring default view
- fix: consolidate search bars to use helper function
- fix: constrain left panel width in web client
- fix: constrain list item width to available space in split layout
- fix: correct GitHub Actions rust toolchain action name
- fix: correct IMPL-0189 -> IMPL-0190 to respect global numbering
- fix: correct aida rel add syntax in documentation and skill files
- fix: correct keyboard shortcuts in help popup
- fix: correct parent/child relationship cardinalities
- fix: correct sample project format and improve YAML parsing
- fix: disable auto-horizontal scroll when selecting requirements
- fix: display lowercase letters for keys in Settings/Keys
- fix: drag-and-drop, panel layout, and type prefixes in Sprint Planning view
- fix: enable WASM builds by configuring tonic without transport
- fix: enable arrow key navigation in Detail view
- fix: enable status icons by default in Settings/Appearance
- fix: ensure relationship tree follows consistent traversal direction
- fix: exclude Folder type from KanBan view
- fix: exclude requirements data files from trace detection in commit hook
- fix: expand CORS headers for gRPC-Web browser clients
- fix: expand baselines view to fill available window space
- fix: implement --file argument for opening projects
- fix: improve KanBan drag-and-drop between columns
- fix: improve Timeline hover suppression with position-based tracking
- fix: improve WASM compatibility and server graceful shutdown
- fix: improve feature dropdown keyboard navigation and hotkey focus
- fix: improve feature picker dropdown size and scroll behavior
- fix: improve hotkey detection for '/', 'v', and '?' keys
- fix: improve migration warning dialog UX
- fix: improve relationship display clarity
- fix: improve search highlighting visibility and reliability
- fix: improve search mode toggle visibility
- fix: increase Actions dropdown width to 280px for better zoom support
- fix: instant resize for Markdown Help modal
- fix: keep selected requirement visible when scrolling reqlist
- fix: keep split panel visible when hiding detail view
- fix: make '/' hotkey work globally to focus search bar
- fix: make List 2 selection independent from List 1
- fix: make keyboard shortcuts dialog resizable
- fix: markdown help modal auto-sizes to fit content
- fix: match Edit view layout with Detail view for ListDetailsSide mode
- fix: match Edit view layout with Detail view for ListDetailsStacked mode
- fix: migrate keybindings to include new actions on settings load
- fix: migration marker detection now checks first 5 lines only
- fix: migration warning respects "don't show again" + focus description on Add/Edit
- fix: mouse wheel scrolls view, keyboard navigates selection
- fix: move close button to same line as Edit in details panel
- fix: multiple improvements to view picker and keyboard shortcuts
- fix: only show expand arrow when there are expandable children
- fix: open cloned requirement in edit mode automatically
- fix: open filter dialog from all layouts
- fix: preserve dim text colors when theme editor is open
- fix: preserve text selection when right-click opens context menu
- fix: preserve tree view in highlight search mode
- fix: prevent 'd' key from triggering delete menu while detail tab menu is open
- fix: prevent 'v' and '?' popups from closing immediately after opening
- fix: prevent Edit mode trigger after status popup selection
- fix: prevent YAML parse error after SQLite migration in GUI
- fix: prevent double-click from cycling layout and widen dropdown
- fix: prevent duplicate list panels when detail panel is collapsed
- fix: prevent ghost highlight in Timeline view navigation
- fix: prevent hotkeys from triggering when search box has focus
- fix: prevent list panel auto-expansion in Edit view
- fix: prevent navigation keybindings in Add/Edit form views
- fix: prevent requirements list navigation from overriding Timeline view
- fix: prevent status popup when pressing 's' in add menu, sanitize all form fields
- fix: proper indentation for comment threads and +/- icons
- fix: queue view navigation, click selection, and button layout
- fix: regenerate spec_id when prefix override changes
- fix: remove duplicate gRPC-Web layer causing 400 errors
- fix: remove hover highlight from List 2 to match List 1 behavior
- fix: resolve duplicate SPEC-ID FR-0175 and add IMPL-0181
- fix: resolve duplicate SPEC-ID FR-0176
- fix: reuse existing Uncategorized feature instead of creating duplicates
- fix: revert AI submenu to standard menu_button pattern
- fix: rewrite KanBan drag-and-drop to use pointer position
- fix: sanitize control characters from form fields before saving
- fix: scale Actions menu width with zoom level
- fix: scale submenu widths with zoom level
- fix: seamless transition between Detail and Edit views
- fix: search highlighting now shows yellow/orange colors in Highlight mode
- fix: search now finds all matching requirements regardless of filters
- fix: search now shows flat list to find all matching requirements
- fix: selection jumps to stay visible when scrolling reqlist
- fix: settings sidebar width scales with font size
- fix: show folder icon for Folder type requirements
- fix: simplify hotkey blocking to use form view check instead of focus detection
- fix: stacked detail view content clipping in horizontal layout
- fix: sync aida-release skill template with deployed version
- fix: theme editor layout to show full content
- fix: theme editor preserves built-in theme colors
- fix: theme editor preview with AIDA-specific UI examples
- fix: theme editor respects light themes when opening
- fix: truncate long titles in Details View to keep buttons visible
- fix: update AI Global Context example to software-defined radio domain
- fix: update main list view search bar with new mode toggle
- fix: update queue selection to follow item after move in Queue view
- fix: update theme defaults to match custom theme values
- fix: use Area instead of Window for instant Markdown Help modal
- fix: use fixed-size expand/collapse buttons for consistent width
- fix: use pulldown-cmark for proper markdown to HTML conversion
- fix: use simple Unicode symbols for status icons
- fix: use simplified list panel in Edit view to match Detail View
- fix: widen Actions dropdown to prevent AI submenu occlusion
- fix: wrap long comment text within panel width
- initial commit
- many improvements
- refactor(templates): rename templates to follow aida-* naming convention
- refactor: clean up View Settings dialog with popup multi-selects
- refactor: move Kanban to end of view picker list
- refactor: move Users from Admin tab to dedicated Users tab
- refactor: move settings file to ~/.config/aida/aida_gui_settings.yaml
- refactor: move split panel to left side (next to main list)
- refactor: rename aida-gui to aida-desktop
- refactor: reorganize Settings tabs - rename Project to IDs, Admin to Db
- refactor: simplify layout controls to 4 predefined modes with cycle button
- refactor: simplify text color handling in theme editor
- security: remove .env from git and add to gitignore
- yaml data protection

