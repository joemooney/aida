# Changelog

All notable changes to this project are documented here. Generated
mechanically from the spec graph (`aida changelog refresh`) — do not edit
by hand; regenerate after merging.

## [v0.15.0] — 2026-09-12

Specs merged since v0.14.0 (959):

### Features

- **TASK-1209** — aida drain tail: subcommand forwarding to aida tail drain (drain namespace completeness) (#1763)
- **TASK-1208** — aida drain status: append a context-aware next-command hint (watch live / triage shelved / awaiting) (#1762)
- **STORY-1029** — aida notify: rule-gated operator alerts through one configured shell command (idle-with-open-work, shelve, escalation) (#1733, #1761)
- **TASK-1199** — aida statusline setup --client antigravity: emit/install the Antigravity CLI statusLine block (same shape as Claude) and document the title script (#1747)
- **STORY-1027** — aida commands: flat, grep-able listing of every command and subcommand from clap's own tree (#1732)
- **STORY-1024** — merge queue: Done-with-PR specs land through one serialized integrator loop — aida integrate --run (#1730)
- **STORY-1006** — /aida-handoff: capture conversation residue to the substrate, then recommend fresh-session vs compact (#1720)
- **STORY-1003** — per-vendor model selection: [agents.<vendor>] model config + --model flag passed through the launch adapter (#1717)
- **TASK-1200** — add product to the starter role set — intake/PO seat missing from the default roles (#1715)
- **STORY-1002** — every queue mutation names its destination fully: identity, local-vs-role queue, routed role, and the exact pickup command (#1712)
- **STORY-993** — session conversations shows PID and TTY per live session so a human can find the terminal, and the live glyph means a process is alive (#1706)
- **STORY-991** — aida agent new offers to resume a recent session and alerts on a live same-role agent before launching (#1703)
- **STORY-976** — aida status leads with a welcome-back digest after a long absence (#1692)
- **STORY-975** — Drain self-retries once on transient shelve causes before parking NeedsAttention (#1688)
- **STORY-818** — Ask-AI escalation at prompts: `a` one-shot vendor judgment + guided-review agent shell (STORY-809 slice 2) (#1679)
- **STORY-822** — session list/resume shows the agent (claude/codex/agy) per row and includes non-Claude sessions (#1670)
- **STORY-948** — drain status shows pacing: per-spec finish times, current item elapsed + last-output age, quiet warning (#1663)
- **STORY-837** — aida help <term>: concept index + FTS over help corpus + AI escape hatch (#1662)
- **STORY-833** — TUI cockpit gains a live drain progress panel (#1659)
- **STORY-830** — init --footprint minimal: orphan store + .aida/config.toml + .gitignore block, nothing else committed (#1657)
- **STORY-829** — relocate the discipline pack from docs/aida/discipline/ to .aida/discipline/ (allow-listed) (#1655)
- **STORY-828** — machine-global post-init hooks: ~/.aida/init.d/*.sh run after successful init (#1654)
- **STORY-827** — init seeds a root README.md and registers the project in ~/.aida/projects.toml (#1653)
- **STORY-824** — statusline and statusbar show a live-drain segment (busy indicator) (#1651)
- **STORY-823** — init picks the remote from named machine-global forge profiles (work gitlab / home gitlab / github) (#1649)
- **STORY-821** — role enter offers to resume a recent session for that role (#1647)
- **STORY-807** — Agent selection is explicit and revisable: PATH-detected prompt at init, list/enable/disable in aida config menu (#1644)
- **STORY-809** — Ask-at-the-prompt: every consequential interactive fork offers ? (deterministic context card) and a (one-shot AI judgment); guided-review agent shell (#1643)
- **STORY-791** — Agent registry: one row per session, a description per agent, and singleton-per-role unless scoped (#1636)
- **STORY-780** — aida init: bootstrap a project from nothing (create dir + scaffold + remote) in one ordered command (#1635)
- **STORY-790** — aida agent resume <name|SPEC>: reattach a previously-launched agent session after a long gap (#1634)
- **TASK-1184** — aida agent gc: prune dead-PID registry entries (registry grows without bound) (#1632)
- **STORY-784** — Cycle time and rework rate segmented by AI involvement (#1619)
- **STORY-783** — aida metrics ai-lift: trailer coverage and attribution over git history (#1616)
- **STORY-785** — aida why <file:line>: git-blame trailer fallback — answer from the last-touch commit when no trace comment exists (#1608)
- **TASK-1181** — Port AIDA's hooks to Codex: scaffold .codex/hooks.json and make the handlers vendor-neutral (#1607)
- **TASK-1180** — aida dev activate: explain the prompt marker it just installed (#1605)
- **TASK-1179** — CHAIN slice (FR-284 child): suggest next-spec handoff after a reap (#1603)
- **FR-284** — Spec-boundary handoff: session reap + chained next-spec launch (no human at every boundary) (#1601)
- **TASK-1176** — no terminal 'superseded'/'accepted' state for decision (ADR) specs — adopted-then-replaced ADRs must masquerade as rejected (#1600)
- **TASK-1177** — Reap slice (FR-284 child): supervisor reaps finished+merged+EXITED sessions on substrate state (#1599)
- **TASK-1178** — pre-commit guard warns when a scoped session's commit touches paths outside its worktree (detect-not-prevent) (#1598)
- **TASK-1175** — burndown order=queue: implement TRUE added_at insertion order (not id order); added_at tiebreak in priority mode; keep priority default (#1595)
- **TASK-1173** — aida tail: prefix each streamed line with its real per-event timestamp (local time), so drain pacing is visible (#1593)
- **TASK-1137** — **TASK (checkpoint):** per-member `--zen` pause / `--no-human=both` auto-continue hook. ~40 LOC (#1591)
- **TASK-1172** — burndown ready-set ordering: input-order (default) vs priority, first-class config + --order flag (#1589)
- **TASK-1136** — **TASK (cluster PR):** one PR linking all member SPEC-IDs + Done→Completed bump for all (#1588)
- **TASK-1170** — Unify scaffold upgrade: one convergent-idempotent verb upgrades ALL agent packs (skills/commands/hooks/memories/codex/antigravity), edit-preserving, no --force (#1587)
- **TASK-1169** — drain integration-wait architecture: foreground-block CI/merge waits + bounded ceiling + punt-not-abort, all launcher-set (BUG-755 follow-up, ratified) (#1582)
- **TASK-1022** — TASK-0432 — does a product seat change anything under `--no-human`/solo? (No new authority, but the audit `mode` should note product-sourced decisions taken during a headless drain.) (#1575)
- **TASK-1014** — TASK-0432 — the `mode` field records which composition mode produced the decision (autopilot / zen+autopilot / solo+autopilot) (#1574)
- **TASK-1019** — TASK-0431 — product-role recommendations as *evidence* feeding gate 3, never as authority (#1573)
- **TASK-1167** — aida tail <session|spec|drain> — stream a live session's log by id, resolving the path the operator shouldn't have to know (#1572)
- **TASK-1009** — `aida worktree pool adopt` for migrating pre-pool sibling worktrees (risk 1) (#1571)
- **TASK-1013** — TASK-0431 — product-sourced decisions must be filterable (`--from-product`); the evidence field records the product handoff (this plan reserves the filter, TASK-0431 fills the source) (#1570)
- **TASK-994** — A verbose `aida watch --all` TUI feed for live debugging (#1569)
- **TASK-1018** — TASK-0430 — durable audit + one-command reversal for every `Execute` outcome (depends on the `Decision`/`Outcome` shapes defined here) (#1564)
- **TASK-997** — Calibration: count benign-absorbed vs actionable events per drain in the exit summary to prove the lever empirically (#1560)
- **STORY-722** — 2-agent bake-off (claude+codex): fan → CI + blind reviewer-judge → pick winner (#1552)
- **STORY-634** — Multi-repo Tier 2: Origin{repo,component} field on Requirement + repo-qualified linkage (GATED on SPIKE-62 D1-D5) (#1548)
- **TASK-975** — CI-failure auto-fix loop before shelve (+ in-drain merge-conflict rebase) (#1545)
- **TASK-1162** — guided-implement vendor parity: validate + wire the codex rendering into mode dispatch; fence AGY by dispatch policy (#1544)
- **TASK-1097** — Code-leg fan-out: aida remote mirror + pre-push hook mirrors code branches to every hub (#1542)
- **TASK-1075** — TUI drive: full auto-remedy loop — gate hold to inline advisor remedy to auto re-run zen, one gesture (#1537)
- **FR-283** — First-class numeric weight/score field on specs (beyond high/med/low priority) (#1534)
- **TASK-1160** — aida worktree exit + ambient worktree PS1 indicator: symmetric step-out verb and always-visible where-am-I (#1528)
- **STORY-776** — Execution-mode dimension + aida do universal dispatch: the advisor classifies HOW each spec runs at bless-time; one verb routes to the right harness with an explicit human contract (#1521)
- **TASK-1159** — config default [burndown] verbose: stream drain progress by default; --quiet restores the buffered launch (#1519)
- **STORY-715** — aida statusbar: ambient OSC terminal-title meter (gnhf-style), read-only, not dispatch (#1516)
- **TASK-1146** — Fold worker directives into aida awaiting so aida human audit's enqueue surfaces in the unified inbox (#1513)
- **TASK-1158** — aida dev activate: default pin becomes release (auto 'freshest wins' demoted to explicit opt-in) (#1512)
- **TASK-1098** — "aida help commands" a command that will print a comprehensive list of all commands and subcommands (#1511)
- **TASK-937** — Batch-approve from the needs-approval group (#1510)
- **TASK-1117** — aida edit <spec> with no flags opens $EDITOR on a markdown buffer (git-commit style) (#1508)
- **TASK-1145** — aida worktree gc: prune dead/merged agent worktrees (lifecycle hygiene gap) (#1504)
- **TASK-1157** — aida dev ps1: fast PS1-embeddable staleness probe — show when the active binary is behind HEAD (rebuild needed) (#1503)
- **TASK-1155** — aida do <spec>: one discoverable verb — implement in a worktree, stop at the PR, print a next-steps checkpoint (#1489)
- **TASK-1080** — integrate --strategy stacked: auto-promote deferred children via stack-aware force-push rebase (#1418)
- **TASK-1147** — Autopilot auditability + reversal surface (inspect/undo/challenge decisions); defer real auto-approval authority (#1417)
- **TASK-1150** — Distinct-user identity guard slice (prevent queue/lease identity mixups); defer roster/RBAC (#1416)
- **TASK-1149** — Doc-gating: gate documentation at RELEASE (not PR) — cherry-pick from EPIC-25 (#1415)
- **TASK-958** — STORY-711 commit-side authorization gate (a commit refusing to land work for a spec (#1414)
- **STORY-769** — Fold time + last-human-input gap injection into the awaiting-notice hook; expose the timestamp as a human-presence oracle (#1412)
- **TASK-1120** — Opt-in --panes tmux: burndown/queue-work fan-out spawns implementers into titled tmux windows (dedicated drain session) (#1410)
- **STORY-764** — Piped/captured aida output silently switches to TOON — document loudly or add a --format escape hatch (#1409)
- **STORY-766** — /aida-fleet-watch skill: substrate-first fleet monitor with terminal census, digest + per-item recommended action (#1407)
- **STORY-768** — aida human audit: CLI verb to trigger the /aida-human-audit pass (enqueue directive + --inject tmux) (#1404)
- **TASK-1048** — aida zen --agent {claude|codex|agy} — vendor selector (foundation for the bake-off) (#1402)
- **STORY-767** — /aida-review-human skill: reconcile every aida human item so the report is true from the graph (#1401)
- **TASK-1143** — Consider surfacing locks in `aida ps` (a locked-by column) once slice 1 exists (#1400)
- **TASK-1045** — Materialize `.claude/skills/*.md` into prompts for non-Claude vendors (the `seed_skill_prompt` hook) (#1399)
- **TASK-1116** — Per-invocation vendor override: --agent/--vendor flag on zen (and relevant drive commands) (#1397)
- **TASK-1086** — aida human unblock --interactive: auto-clear the needs-design tag on decision-capture (make it drive-ready) (#1395)
- **TASK-1092** — Add cross-vendor routing config for role-to-vendor fallback order (#1394)
- **TASK-1046** — `.mcp.json` → codex/gemini MCP config translation for headless drain agents (#1393)
- **TASK-1140** — STORY-711 slice 2: automatic advisor-lock enforcement gate (substrate-as-bouncer pre-work check) (#1390)
- **TASK-1090** — Implement CLI-first dispatch health report with resume/fallback hints (#1389)
- **STORY-701** — Mailbox reason-group + send-mail action in the cockpit board (#1388)
- **STORY-711** — Advisor-directed branch/worktree lock: an agent verifies it works at the behest of an advisor on a branch/worktree that advisor authorized (#1386, #1387)
- **TASK-1122** — BUG-715 follow-ups: redact git author of store commits + doctor detection + dry-run preview (#1382)
- **TASK-1124** — aida doctor: rule-delivery-rot check — hash deployed vendor prompts/scaffolding against source templates, flag drift (#1381)
- **STORY-763** — Skills parity on Codex: ship the AIDA skill/command masters as Codex custom prompts (#1361)
- **TASK-1112** — Kill remaining hard-coded claude defaults on the launch/identity paths (#1361)
- **TASK-1114** — STORY-763 slice 1: aida scaffold codex-prompts — command set as Codex custom prompts (#1360)
- **TASK-1113** — STORY-762 slice 1: claude-shell audit + doctor vendor-binary check + graceful codex-only degrades (#1359)
- **STORY-761** — One default-vendor knob: [agents] vendor = codex|claude resolved by every launch surface (#1358)
- **STORY-760** — Prevent github/gitlab remote drift: detect + fan-out push + reconcile (#1353, #1355, #1356)
- **TASK-1096** — Store-sync fan-out: [store.sync] mirror_remotes pushes aida-store to every hub (#1354)
- **TASK-1095** — aida remote status + doctor: detect and report github/gitlab drift (read-only) (#1353)
- **TASK-1094** — Slice 1b: wire the dispatch liveness delta-comparator with cross-invocation snapshot persistence (#1352)
- **STORY-759** — Slice 1: CLI-first dispatch health/recovery report + commit-early discipline (SPIKE-76) (#1351)
- **STORY-754** — aida why <file:line> — answer 'why does this code exist?' from the nearest trace comment (the 60-second magic moment) (#1343, #1349)
- **STORY-757** — aida init --minimal — the markdown-only first-run (specs/ folder + a runnable aida why demo, zero machine) (#1346)
- **STORY-755** — aida why <file:line> plain-markdown fallback — resolve intent from a spec .md file when no git-canonical store is attached (#1344)
- **TASK-1087** — aida human unblock --interactive: make the advisor-handoff step legible — split into Groom / Groom+approve options + a '?' help item with examples (#1338)
- **TASK-345** — `aida session leases --json` could include the drain cross-reference (#1331)
- **TASK-330** — Stamp the Claude session ID onto comments so `aida session list` can (#1330)
- **TASK-101** — aida statusline + queue list: surface 'base behind by N' for active leases on stale branches (#1329)
- **TASK-349** — A `blocked-dependency` punt should auto-suggest filing a blocked-by relationship (#1328)
- **TASK-939** — Diff-driven doc prompt at PR-open: nudge aida doc add for new public surface (#1325)
- **TASK-1082** — did-you-mean: suggest nearest existing spec id when show/edit hits 'not found' (#1323)
- **STORY-700** — First-run experience: feel the core loop in session one (W5) (#1321)
- **STORY-702** — Who-must-act lens: pivot the board by owner (you / advisor / role) (#1320)
- **STORY-698** — Human test plan per spec: capture builder's verification steps + aggregate into PR bodies (#1317)
- **STORY-750** — aida human unblock --interactive: human resolves hurdles inline before a drain (#1316)
- **TASK-1076** — TUI spawn_drive omits --solo: a drive on an epic-parented spec routes into the epic worktree (#1315)
- **STORY-749** — Slice 1: declarative scenario library on a ScenarioDriver (PhaseDriver) covering every orchestrator branch in-process (#1313)
- **TASK-1081** — Slice 0: override the hardcoded agent binary via AIDA_AGENT_CMD so a mock can slot into the real spawn path (#1312)
- **TASK-1063** — Dead-queue pruning split across queue prune --orphaned/--merged + queue gc — fold into queue prune --terminal or sharpen the cross-references so the right verb is obvious (#1310)
- **TASK-1010** — `post_create` hook recipe to pre-warm `cargo build` on a freshly-created pool tree (turn cold-create into warm-on-first-use) (#1308)
- **TASK-769** — Decouple the away presence escalation default from `away_drain` into its own `[presence]` knob (today escalation rides the away_drain rung; (c) questions deferred to STORY-555) (#1302)
- **TASK-1012** — Telemetry: pool hit-rate (reuse vs create) to prove the warm-cache payoff empirically (substrate learning loop) (#1301)
- **TASK-993** — events.jsonl rotation / per-drain truncation at `run-started` (#1299)
- **TASK-841** — integrate: stacked-branch accumulation strategy (#1297)
- **STORY-744** — TUI drive verb: surface zen suitability-gate holds + clarify follow-on affordance (#1293)
- **STORY-742** — aida worktree enter <spec>: one bare command to create-if-missing + cd into a worktree for a single spec (no agent), not just epics (#1287)
- **STORY-718** — aida integrator — continuous focus-scoped queue-drainer (the integrator seat) (#1286)
- **TASK-1073** — Integrator role first-class: integrator.md agent config + statusline seat + queue list --role integrator routing (STORY-718 slice 2d) (#1286)
- **STORY-741** — One can't-miss per-turn coordination signal: fold mail into the Awaiting-you report + surface all channels per-turn (not just mail) (#1283)
- **STORY-703** — Advisor-backlog opacity panel: surface WHY the advisor parked each item (#1280)
- **STORY-740** — Cockpit depth: wire the stubbed groom/archive verbs + advisor park-reason inline (STORY-703) (#1280)
- **STORY-739** — TUI preview modal surfaces the relationship graph (blocked-by/blocks, parent/children) — the dig-in gesture reveals AIDA's #1 differentiator (#1279)
- **TASK-978** — TUI: per-row liveness glyph in the Targets list (●live / ⚠STALE / idle) (#1278)
- **STORY-738** — Completion crescendo: a spec reaching Completed renders a felt loop-closed moment, not the generic Updated: line (#1274)
- **STORY-737** — Newcomer delight: humans get the post-add next-step nudge agents already get; history hides META scaffolding; empty queue work is a soft signpost not a red error (#1273)
- **STORY-736** — aida zen <thought> --dry-run renders the spec it would draft (title + description + acceptance), not a flat echo — the it-just-knew moment (#1272)
- **ADR-7** — One per-spec orchestration engine — zen/burndown/integrate differ only in scope + lifetime, never in the per-spec lifecycle (#1264)
- **TASK-1052** — Queue-GC: prune routed queue entries whose spec is archived/completed/rejected (#1261)
- **STORY-735** — aida guided-implement: structured step-by-step decision dialog for keystone/architecture/security specs (#1258)
- **STORY-734** — aida list --fields <csv>: select + order the displayed columns (humans + agents) (#1257)
- **STORY-733** — Compact the orphan aida-store branch (15.7k commits) — the substrate tax behind every write/gc/rebase (#1256)
- **STORY-730** — Make aida status the durable morning-after report (since-you-were-away banner) (#1252)
- **STORY-732** — Per-spec recovery legibility: inline failure reason, fix why-vs-status contradiction, default aida findings to list (#1251)
- **STORY-731** — Single-spec drive failure: name it as shelved-not-crashed + point to findings list (parity with batch) (#1250)
- **STORY-729** — Error messages that name their fix + clear Done-vs-Completed language (#1249)
- **STORY-726** — Drive liveness: implementer-phase heartbeat + discoverable drain status (#1248)
- **STORY-727** — Per-spec next-steps for humans + clearer why language (#1247)
- **STORY-728** — Add a drive/zen verb to the cockpit so you can kick off an autonomous drive from the TUI (#1246)
- **STORY-725** — aida zen free-text front door + help cleanup (#1243)
- **TASK-1053** — aida queue work <spec> --dry-run: preview the single-spec plan (#1242)
- **STORY-724** — Polish the now-default TUI: drop the prototype label, no dead scopes/verbs (#1240)
- **STORY-723** — Front-door actionable launcher: no-args/status/list show live actionable work + lead with zen/ship (#1239)
- **TASK-1051** — Make the EPIC-54 TUI redesign (AIDA_TUI_REDESIGN) the default (#1234, #1238)
- **TASK-1037** — aida zen slice 2: suitability gate + scope-routing (refuse epic/keystone/blocked, warn under-specified/coupled, default-route-to-scope, --solo) (#1237)
- **TASK-1050** — aida integrate slice 2b: own-checkout guard (BUG-650) — unblocks unattended use (#1236)
- **STORY-721** — aida zen <spec> — one-shot autonomous implement+ship (auto-queue + approval-gate + parallel fire-and-forget) (#1230)
- **TASK-1036** — aida integrate slice 2a: event-driven + focus-scoped --watch (convert timer-poll) (#1229)
- **STORY-720** — aida ship — one-shot human-implementer finish (commit, rebase, PR, CI, merge, pull, cleanup) (#1228)
- **TASK-1008** — TTL auto-expiry on `leased_at` (reservation-leak mitigation, risk 4) (#1227)
- **TASK-1007** — Autopilot slice 2: autopilot::evaluate four-gate envelope contract (pure, wired to nothing) (#1225)
- **TASK-1034** — aida integrate slice 1: throughput view (read-only) — focus-scoped queue + live throughput + active fan-out (#1223)
- **TASK-1003** — SPIKE-70 core: --single-branch coupled-sequential drain mode (engine + flag + cluster-PR) (#1218)
- **TASK-992** — Supervisor consumes wakes via Monitor (STORY-712 slice 4, skills) (#1215)
- **TASK-991** — advisor_watch event-driven — the token-savings land here (STORY-712 slice 3) (#1213)
- **TASK-985** — Worktree warm-pool: flip acquire-on-start default to ON (#1210, #1212)
- **TASK-990** — aida watch — streaming event classifier (STORY-712 slice 2) (#1211)
- **TASK-987** — Event substrate + classifier core (events.rs) — STORY-712 slice 1a (#1209)
- **TASK-988** — Emit events at drain state-changes — STORY-712 slice 1b (#1209)
- **TASK-983** — Worktree warm-pool: replace --force worktree removals with tiered destroy (#1207)
- **TASK-942** — TUI create ('n') should link the new draft to the active focus epic (#1204)
- **TASK-948** — TUI: no queue/routing visibility — can't see what's routed to advisor/implementer queues (#1203)
- **TASK-954** — STORY-710 part B: grey out UPDATE verbs when nothing is selected (require explicit selection) (#1202)
- **TASK-947** — TUI: grey out inapplicable verbs instead of hiding them (discoverability) (#1201)
- **TASK-949** — TUI: no 'reject' verb — advisor can approve a draft but not reject it (#1200)
- **TASK-953** — TUI: 'status' verb — per-spec live work-state lens (queued/leased/in-progress/STALE) (#1199)
- **TASK-982** — Worktree warm-pool: acquire-from-pool on session/agent/queue-work start (#1197)
- **STORY-717** — Focus-scope drift guard: configurable [focus] policy (off/warn/block) at work-start (#1196)
- **TASK-981** — Worktree warm-pool slice 1: pool primitives + CLI + session end --return (#1194)
- **STORY-716** — aida worktree namespace: add + enter (create-or-enter an epic-scoped workspace) (#1192)
- **TASK-974** — Centralized next-step help table keyed by (command, state) (#1191)
- **TASK-972** — Agent-mode output polish: truncate show + structured errors on stdout + count-lines (#1189)
- **TASK-964** — Agent-facing token-efficient output mode (adopt AXI output principles; keep emoji human path) (#1188)
- **TASK-968** — Idle CI timeout that re-arms on base-branch movement (#1187)
- **TASK-967** — Drain exit summary with token accounting + diff stats to usage.jsonl (#1184)
- **TASK-970** — Content-first bare aida + default row-cap for agents (#1183)
- **SPIKE-73** — Reproduce the AXI MCP-vs-token-efficient-CLI benchmark on AIDA tools; decide MCP weighting (#1182)
- **TASK-966** — Drain hard caps: --max-tokens / --max-iterations / --max-runtime stop conditions (#1180)
- **TASK-965** — Worktree-tangle spawn gate: assert worktree root != primary checkout before fan-out (#1179)
- **TASK-961** — STORY-621 Slice 1: route-only forge wiring (pr ship lookup + merged-lookup + child-PRs + queue-recover create) (#1176)
- **STORY-706** — aida focus core: set/show/clear + scope reads + auto-link adds + loud visibility (#1175)
- **TASK-957** — Agent-tool fan-out leases are generic `harness-worktree` scopes, not spec-scoped — (#1173)
- **TASK-845** — Cross-machine person unification: reconcile one human across different $USER/user_id per machine (#1172)
- **STORY-708** — aida groom: unify the advisor disposition verb (rename assess/intake) + scoped 'aida groom <spec|epic>' + reconcile backlog-groom (#1169)
- **TASK-927** — (Followup C) skill lint: glyph-lint + `aida plan verify` on any plan ref (#1164)
- **TASK-955** — aida list --parent <id> --recursive: transitive subtree filter (epic + ALL descendants), composes with status/open (#1159)
- **STORY-709** — aida usage performance lens: --slowest (latency by command) + --events (raw event stream with durations) (#1155)
- **TASK-951** — Case-insensitive user identity matching in queue/queries/owner comparisons (#1154)
- **TASK-950** — aida db block list: column-align, merge contiguous ranges, rename exhausted -> full (#1153)
- **TASK-917** — **`--no-verify` direct detection.** Today a `--no-verify` bypass is caught indirectly (it surfaces as CI red on the bypassed check). A direct pre-commit-hook-side detector (record when `--no-verify` w (#1150)
- **TASK-896** — In-agent status-footer parity for Codex (statusline richness survives only in shell prompt/tmux) (#1149)
- **STORY-696** — aida ps: global running-work table — every active session/agent with spec, role, pid, started, elapsed, live-vs-STALE (companion to aida status <spec>) (#1146)
- **EPIC-54** — aida tui redesign: action->target command-palette model (#1145)
- **TASK-923** — aida queue list --epic/--parent <ID>: filter the queue to an epic + its children (#1144)
- **STORY-694** — aida status <spec>: liveness for in-progress work — show linked session/pid/worktree/started/elapsed + a live-vs-STALE verdict (#1141)
- **EPIC-51** — Deterministic immediate-response layer: Ctrl-D from chat to a TUI AIDA action surface, act instantly, resume the conversation (#1131, #1134, #1139)
- **TASK-909** — Wire `Ctrl-D` capture from the hosted chat as an alternate palette-open trigger (#1139)
- **SPIKE-67** — Field instrumentation of stated-rule violations in real drains — the only remaining gate-vs-rule evidence path (#1078, #1138)
- **STORY-692** — Fasttrack express tier: batch:express filing verb (no lifecycle skip) (#1137)
- **TASK-905** — aida fasttrack status: lane projection (requested to shipped) (#1136)
- **TASK-907** — Wire fasttrack express into the EPIC-0428 advisor-autopilot envelope (#1135)
- **STORY-680** — EPIC-51 Slice 3: inject a palette result into the chat via PTY stdin on resume (#1134)
- **STORY-679** — EPIC-51 Slice 2: deterministic AIDA action palette (no LLM) while the chat is suspended (#1131)
- **TASK-894** — Headless advisor tier (/aida-advise) on Codex — vendor-neutral spawn for the punt-resolution loop (#1130)
- **TASK-904** — Surface live aida intake proposals in the cockpit board (TASK-901 follow-up) (#1129)
- **STORY-691** — aida tui: color-code structured preview fields (status/priority/tags) with a themeable color map (#1128)
- **TASK-906** — aida-fasttrack skill: express tier, eligibility litmus, punt-out discipline (#1127)
- **TASK-895** — TUI can host a Codex session (app.rs spawn_tab threads Claude --session-id/--resume + spawn_claude_session) (#1125)
- **STORY-673** — aida status default output is overwhelming — terse default + opt-in detail (Trojan-horse: quiet depth, not a wall) (#1122)
- **STORY-689** — aida tui: render preview as markdown + color-coded structured fields (themeable), not captured CLI text (#1121)
- **TASK-901** — Surface `aida intake` proposals + advisor backlog explicitly in "needs approval" (#1120)
- **STORY-672** — Fleet-wide queue view: opt-in aida queue list --all-users (coordination bird's-eye) (#1119)
- **STORY-684** — Advisor-role code-write invariant must be a SUBSTRATE gate, not just a Claude PreToolUse hook (evaporates for Codex) (#1117)
- **STORY-681** — EPIC-52 Slice 1: self-sufficient aida tui launch — in-process dispatch replaces the fd-3/bash-wrapper (#1116)
- **STORY-683** — Headless orchestrator drain on Codex — generalize spawn_claude_headless to spawn_vendor_headless (codex exec) (#1115)
- **TASK-900** — Real `aida list --limit` + load-more pagination (TASK-897 follow-up) (#1110)
- **STORY-686** — Blocked/waiting board as TUI home view — why-not-moving taxonomy grouped by reason, owner, action (#1107)
- **TASK-897** — TUI panels: cap Backlog/History rows to most-recent N (e.g. 100) to keep the list light to build/render/scroll (#1106)
- **STORY-685** — EPIC-52 TUI interaction model: two-pane Nav<->list focus navigation + active-panel focus colors (#1100)
- **TASK-877** — User/project-defined aliases (aida alias add/list/remove) — full personal+project (decided 2026-06-22) (#1096)
- **STORY-682** — EPIC-52 Slice 2 core: fuzzy command matcher + clap command-surface enumeration (pure, unit-tested module) (#1094)
- **STORY-678** — EPIC-51 Slice 1: PTY suspend/resume mechanic — prefix key SIGSTOPs the chat child, overlay, SIGCONT + clean repaint (#1091)
- **TASK-0424** — Update Codex MCP setup and project templates for migration readiness (#1090)
- **STORY-677** — aida config menu: enum + integer in-place edits (expand beyond the bool MVP) (#1089)
- **STORY-675** — aida init: detect a shared/known store and prompt to JOIN (vs create new) + auto-register the project into ~/.aida/projects.toml (#1088)
- **STORY-676** — Explicit --store-path <PATH> flag for aida init; --sibling becomes sugar; document the store-location model (#1088)
- **STORY-674** — aida init --sibling: auto-attach a second repo to an EXISTING shared store (write config + acquire node id + rebuild cache, no re-seed) (#1087)
- **STORY-669** — aida config menu: in-place toggle/edit (write-through to config.toml) (#1083)
- **TASK-891** — SPIKE-67 follow-up: drain-vs-interactive + headless attribution for field-study observations (#1081)
- **STORY-670** — Substrate gate: PreToolUse hook makes advisor-edits-code loud (symmetric to TASK-647's queue gate) (#1080)
- **TASK-882** — MCP history tool lags aida history CLI: exposes only spec_id+since, missing --events/--type/--author/--until/--limit/--shipped/--all etc. (#1071)
- **TASK-881** — bare 'aida queue' dumps the full subcommand help (exit 2) instead of routing to 'aida queue list' (#1063)
- **TASK-863** — aida push: consider warning when there is uncommitted/unstaged work (#1056)
- **TASK-862** — Personal-view shortcuts: aida mylist / aida myqueue (and the aida list default-scope question) (#1053)
- **TASK-878** — Auto-GC merged Agent-tool worktrees (worktree-agent-*) — they accumulate + cause stale-branch false-positives (#1052)
- **TASK-879** — aida solo discoverability: surface it in getting-started + help (the command exists but is hidden) (#1051)
- **TASK-880** — Confirm + ensure the solo statusline marker renders when the (warm) solo loop is active (#1051)
- **STORY-668** — /aida-solo skill — the WARM interactive solo-driver (live session as advisor+integrator) (#1050)
- **STORY-667** — aida alias / aida alias list — a discoverable registry of built-in shortcuts (#1047)
- **STORY-666** — Autonomous doctor --heal: destructive fixes require sign-off (never blind -y); safe fixes proceed (#1045)
- **TASK-876** — Per-machine os_wrap override (AIDA_OS_WRAP env) so the sandbox can be enabled per-host without editing shared config (#1042, #1044)
- **STORY-665** — Guided bwrap sandbox setup: doctor surfaces the exact remediation + a repeatable install path for new machines (#1043)
- **TASK-864** — Wire bwrap os_wrap into the interactive agent-launch path (aida agent new), not just headless drains (#1042)
- **TASK-872** — Trace-read-rate audit: prove the graph is consulted, not just written (#1041)
- **TASK-859** — aida init should prompt for configurable items (optionally launch aida config menu) (#1040)
- **STORY-661** — aida config menu — a navigable TUI menu of configurable items (#1038)
- **TASK-869** — Cross-vendor judge for aida compete (kill the self-evaluation caveat on the apex evidence) (#1037)
- **TASK-866** — Expose [contained] os_wrap / read_allowlist / managed_domains_only in aida config show (#1036)
- **STORY-662** — aida list --help: document lens aliases + statuses one-per-line; add me / user:<name> filters (#1035)
- **TASK-860** — aida init should detect the forge CLI (gh / glab) and configure it; warn when missing (#1034)
- **STORY-663** — aida commit — produce a conventional-format-compliant commit message (#1033)
- **TASK-868** — Trace-rot detector + machine-checkable trace-coverage gate (aida trace check) (#1032)
- **TASK-865** — aida init + aida doctor: detect bwrap availability (on PATH + userns preflight) and report (#1031)
- **TASK-861** — aida help <topic> should show help for a topic group (it lists topics but cannot open one) (#1030)
- **TASK-858** — aida agent (bare) should alias to aida agent new (#1028)
- **SPIKE-66** — Flagship TUI unused 33d in telemetry despite "the TUI is the product" framing — is it discoverable/used? (#1016)
- **STORY-660** — aida compete slice 2: rubric judge + clean diff signal (the gate alone cannot pick a winner) (#1002)
- **TASK-853** — aida health polish: severity-order the issue list (graft from the Codex arm) (#1001)
- **STORY-659** — aida compete <spec>: run a spec through N vendors headless + objective gate (competition-as-QA, slice 1) (#1000)
- **STORY-657** — Spec interview mode: advisor/product-level clarifying-question loop to raise spec quality (#998)
- **STORY-658** — aida health: at-a-glance project health read (OPEN-brief bake-off subject) (#994)
- **STORY-656** — aida spec dryrun <id>: implementer readiness pre-check + AI gap report before work (#991)
- **STORY-654** — aida node set-owner / set-name + aida team unset-role: backfill legacy node identity + clean stray role keys (#986)
- **STORY-652** — Node friendly names + owner identity: name (<host>-<user>-<seq>) + $USER string on each node, settable at init/acquire (#984)
- **STORY-651** — Team dashboard slice C2: web interactivity — drag-to-reassign, role editor, live refresh (Tier 3) (#983)
- **STORY-650** — Team dashboard slice C1: server write endpoints (PUT assignee w/ notify+queue, PUT role) (Tier 3) (#982)
- **STORY-649** — Team dashboard slice B: web Team page (roster, assignment board, who-holds-what, burndown) (Tier 3) (#981)
- **STORY-648** — Team dashboard slice A: aida-server /team + /coordination endpoints + ts-rs DTOs (Tier 3) (#980)
- **STORY-647** — Team RBAC slice 2: finer-grained op gating + protected specs/tags + strict mode (Tier 3) (#979)
- **STORY-646** — Team RBAC slice 1: per-user roles in the roster + effective_role enforcement (Tier 3) (#978)
- **STORY-644** — Assignment + mention notifications: assigning/@mentioning a user notifies them via the mailbox (Tier 2) (#976)
- **STORY-645** — Conflict auto-merge completeness: union-merge comments + relationships arrays on pull (Tier 2, finishes MU-203) (#975)
- **STORY-643** — Auto mailbox sync: messages flow between users on pull/push (Tier 2) (#974)
- **STORY-640** — Team identity & awareness: aida team roster + status coordination view + distinct-identity guard + onboarding (#973)
- **STORY-641** — History union-merge: concurrent same-spec edits union the history array instead of conflicting (MU-204) (#971)
- **STORY-639** — Assignment + my-work: aida assign --to, --mine/--assigned views, queue routing (#970)
- **STORY-638** — Cross-clone drain + solo locks: shared coordination (slice 2, closes MU-505/506) (#968)
- **STORY-637** — Cross-clone leases: shared lease registry on the store (slice 1, closes MU-504) (#967)
- **TASK-793** — config show: central policy registry so new knobs auto-surface (anti-drift slice 2 of BUG-533) (#964)
- **TASK-775** — aida schema --all: detail every storable object's fields in one shot (not one positional at a time) (#962)
- **TASK-818** — Unify agent addressing: briefs route by short type name, mailbox by stable name (post-BUG-558) (#961)
- **TASK-842** — integrate: multi-spec PR completion semantics (#955)
- **TASK-843** — integrate: multiple open PRs for one spec (#955)
- **TASK-836** — aida queue integrate: strategy completeness (the gaps surfaced running it inside the solo loop) (#954)
- **TASK-827** — Solo mode as a drain autonomy posture: drains honor the solo flag (maximum-discretion safe-backlog mode) (#953)
- **STORY-633** — Phase 4: aida config glyph — CLI surface for themes + per-symbol overrides over the registry (#951)
- **STORY-632** — Deterministic spec centrality/heft (inbound+outbound degree) in the cache; feeds prioritization + the intent pass (#949)
- **TASK-838** — aida-pickup loads the spec's aida intent comprehension at the top of implementer context (#947)
- **STORY-631** — aida intent <spec>: AI-generated plain-terms comprehension of WHY a spec exists (cached doc, drift-refreshed) (#946)
- **STORY-630** — aida schema --explain: per-field semantics + object lifecycle (when/why/who/how), drift-guarded (#945)
- **TASK-837** — aida agent new (no agent type): arrow-key picker of agent types, like the role picker (#944)
- **STORY-629** — Phase 2: [glyphs] custom per-symbol override table layered on the registry (#943)
- **STORY-628** — Phase 1: glyph registry + curated ASCII profile + [ui] glyphs selector (opt-in, default unicode) (#942)
- **STORY-627** — aida solo legibility: run/stop/status verbs + responsive stop + .aida/solo.lock pid sentinel + per-step progress heartbeat (#941)
- **TASK-834** — aida list inflight: per-agent activity/stuck indicator (last-commit/mtime; flag alive-but-idle Nm) (#940)
- **STORY-626** — Cold-boot context seeding: live advisor maintains .aida/advisor-context.md; the assess/burndown cold-boot prepends it (#933, #935)
- **TASK-833** — aida list inflight / burndown status: show OPEN PRs (work-in-flight, any source) — the signal leases miss (#934)
- **TASK-809** — STORY-615 — headless default-deny egress via managed-settings `allowManagedDomainsOnly` (#932)
- **STORY-619** — Launched agents have no cross-vendor mailbox-delivery awareness (only Claude gets a prompt-bound hook) (#931)
- **TASK-817** — Role picker: arrow-key navigation (up/down to move selection), not numeric-only (#930)
- **STORY-614** — Express a 'not-parallel' (serialize/conflicts-with) relationship so the drain never fans collision-prone specs into one wave (#929)
- **STORY-623** — Rename aida intake → aida assess (alias for aida advisor assess); establish ASSESS vs REVIEW vocabulary (#927)
- **TASK-832** — STORY-621 Slice 0: resolve_glab_binary + forge-keyed CLI dispatch (foundation) (#926)
- **TASK-829** — aida advisor: surface pipeline activity (drain running? in-flight? last/next pass?) so the worklist reads active-vs-static (#925)
- **TASK-831** — aida list inflight: alias for aida burndown status (active work — leased specs + drain in-flight) (#924)
- **STORY-625** — Solo mode must be an ACTIVE loop, not just a flag: a supervisor that runs assess→queue→implement→integrate on a cadence (#923)
- **TASK-830** — STORY-625 slice 1: aida solo --watch loop skeleton + --dry-run + cycle composition (#923)
- **STORY-622** — aida doctor / garden pass: audit the human worklist for false-positive (non-operator) items (#922)
- **TASK-828** — aida list advisor: alias for aida advisor (round out the list lens family) (#921)
- **STORY-624** — aida solo: enter/indicate solo-mode work state + surface it in the statusline (#920)
- **TASK-822** — aida list queue: alias for aida queue list (parallels aida list open) (#918)
- **TASK-824** — aida list why: bulk per-spec why-open report (alias/surfacing of burndown explain) under the aida list lens family (#918)
- **STORY-620** — Seat policy: make the operator/advisor worklist partition configurable via [seats] in config.toml (#912)
- **STORY-618** — Bare 'aida advisor' = the advisor's bottleneck worklist (mirror of 'aida human') — the self-service half of EPIC-42 (#908)
- **TASK-814** — serialize-group wave MVP: /aida-burndown fans at most one spec per serialize:<group> tag per wave (#904)
- **TASK-813** — aida queue integrate parks supervised / review:draft-only specs (keystone-exclusion) instead of auto-merging — solo mode works the SAFE backlog (#903)
- **TASK-812** — aida queue integrate acquires the BUG-538 drain lock (no double-drive with burndown run / a second integrate) — safe to leave --watch running (#902)
- **STORY-615** — Sandbox: headless default-deny egress via managed-settings network.allowManagedDomainsOnly (#900)
- **STORY-612** — Sandbox slice 2: OS boundary (bubblewrap) around the whole headless claude -p process (#895)
- **TASK-805** — In-flight/Scheduled marking in queue list + burndown plan (facet b of STORY-604) (#893)
- **TASK-802** — aida config show: surface the [contained] posture block (enable + allowed_hosts) (#892)
- **TASK-806** — aida burndown status subcommand: is a drain running + in-flight/shipped/left + log pointer (ask #3 of STORY-604) (#891)
- **TASK-804** — burndown run --verbose: stream-json tee + live per-event progress (facet a of STORY-604) (#888)
- **TASK-801** — burndown plan: Supervised section (slice 1b) — show supervised specs as their own queue-work block (#878)
- **TASK-799** — aida schema --all: dump every object's full field detail in one pass (+ make --json with no object include fields) (#877)
- **STORY-610** — Supervised-only marker + burndown plan partition: separate 'queue work these' (keyboard) from the drain ready set; relabel 'awaiting sign-off' (#869)
- **TASK-800** — Supervised-marker exclusion: burndown ready-set excludes specs tagged 'supervised' (STORY-610 slice 1a) (#869)
- **STORY-613** — aida review (and sub-sessions) hand findings/leftover-questions to the advisor via mailbox/findings on finish — close the cut-n-paste loop (#867)
- **STORY-611** — aida human: add a 'Reviews awaiting you' bucket + an 'aida human review' sweep — the review queue is invisible despite the banner promising it (#864)
- **STORY-605** — Sandbox slice 1: harden the existing contained posture with a default-deny network-egress allowlist (#862)
- **TASK-792** — Burndown drain posts its completion summary to the advisor mailbox (close the paste loop) — burndown analog of STORY-569 --zen handoff (#849)
- **TASK-790** — Surface message intent in mailbox notice + read-mail skill (fold into STORY-585 surface) (#846)
- **TASK-791** — aida questions answer: add 'type something' (counter-proposal note) + 'chat' (drop into clarify) at the interactive prompt (#842)
- **TASK-782** — Mailbox 'interpret' half: message intent markers (fyi|request|handoff) + act-vs-prompt [mailbox] policy (#836)
- **STORY-602** — /aida-decide skill: sweep the outstanding questions/needs-human inbox and interactively resolve each (the human-decision drain as a skill) (#834)
- **STORY-585** — Surface unread mailbox into the agent's session context — the read/notice half of inter-agent comms (#832)
- **TASK-777** — Thin 'aida fasttrack <title>' CLI primitive owns the fasttrack convention; /aida-fasttrack skill delegates to it (DRY) (#829)
- **TASK-779** — aida decide <spec>: human-resolution entry alias (routes to questions answer if a DecisionRequest is pending, else questions clarify) (#827)
- **TASK-781** — Advisor garden pass runs 'aida questions sweep' — keep the decision inbox populated with pickable questions (#825)
- **STORY-587** — fasttrack: a low-ceremony lane for trivial items — tag + batch bucket + one-command drain (CI stays the only gate) (#820)
- **TASK-776** — advisor watch: event-driven mail firing + --triage-only conservative mode (#818)
- **STORY-586** — aida advisor watch: presence-gated fork-from-live advisor loop (garden + mailbox-triage + escalate while away) (#817)
- **STORY-584** — deferred: a third view-state between active and archived (primed/conditional work, hidden from the open work view) (#816)
- **STORY-582** — Durable work-processing record: capture what an agent did + why when a brief/spec is processed (audit trail) (#815)
- **STORY-583** — aida mailbox retract + delete as configurable policy (default allowed) (#814)
- **STORY-569** — Wire the --zen build to advisor review handoff through the agent mailbox (brief), not the operator clipboard (#811)
- **STORY-405** — aida status: surface live advisor / external state-affecting activity (raw shell recoveries, manual merges, lease cleanups) (#809)
- **TASK-758** — aida zen finish: bias the auto-exit decision by operator presence (STORY-561) (#808)
- **STORY-567** — Contained execution posture: opt-in blast-radius containment for autonomous/headless drains (substrate-as-bouncer for the filesystem) (#807)
- **STORY-568** — Autonomous research/spike lane: dispatch SPIKE specs to a research agent (deliverable = analysis + recommendation), escalate only the decision (#805)
- **TASK-770** — `aida human away/home` namespacing (SPIKE-57 Phase 2-4) — these aliases + the `aida human` vector; this build keeps the existing `aida away`/`home`/`presence` verbs (#804)
- **TASK-760** — aida init: ship a commented [intake] example block in the config.toml template (#801)
- **TASK-772** — aida plan verify: support a '(new)' annotation for to-be-created file refs (#800)
- **TASK-715** — aida schema: expose over MCP (resource or tool) for native agent consumers (#709)
- **STORY-1052** — Product-role backlog nudge loop: detect stuck work and nudge the advisor (or the operator when no advisor is live) — the ship-now stop-gap
- **STORY-1054** — Vendor activity/liveness adapter: one seam giving the watchdog, idle detector, and supervisor a per-vendor 'is this session alive and producing output' signal (Claude+Codex tier-1, Antigravity tier-2)
- **STORY-753** — Agent-surface TOON coverage gap: ps / backlog list / history emit human tables under AIDA_AGENT_OUTPUT
- **STORY-789** — Project-identity checklist at init — offer the thesis, never demand it
- **TASK-1127** — scripts/publish-positioning-pdfs.sh — batch-render docs/positioning/*.md to PDF into the artifacts repo
- **TASK-1128** — positioning-PDF renderer: size table columns by content, not equal-split (narrow label columns)
- **TASK-773** — aida list open: exclude perpetual standing-artifact types (vision/principle/term/constraint/folder/meta) from the open-work view by default
- **TASK-783** — statusline: show 'away' + TTL-remaining when the operator is away (hide when home)
- **TASK-784** — aida whoami: print the caller identity AIDA resolved (role, agent-type, agent-name, user, headless, session)

### Fixes

- **BUG-1063** — Headless phases die waiting: sessions either end their turn expecting a background notification that never arrives under claude -p, or block in one long gh run watch that emits nothing until it return (#1748)
- **TASK-153** — aida ps still shows lease role for a long-lived advisor whose transcript conversations calls advisor (#1741)
- **BUG-909** — Watchdog is blind to a working Codex implementer: codex exec prints nothing to stdout until it finishes, so 'no session output for 10m' kills every Codex phase that thinks for 10 minutes (#1740)
- **BUG-908** — STORY-975 retry cannot reclaim the lease attempt 1 left behind — attempt 2 dies in seconds with 'scope is owned by lease … worktree DIRTY' (#1739)
- **BUG-1021** — cross-platform nightly red on macOS two consecutive nights (main d6cdad0 and 18e7ee5) (#1738)
- **BUG-912** — review record --pr writes the phase-3 handshake under AIDA_DRIVE_ROOT (the worktree) and ignores AIDA_REVIEW_VERDICT_FILE, so a headless verdict lands where phase 4 never looks (#1737)
- **BUG-1056** — queue rework leaves the spec NeedsAttention, which the unified pickability policy refuses — reworks became undrainable (#1736)
- **TASK-154** — git push hook calls missing aida remote mirror-push (#1735)
- **BUG-1052** — integrate --run: propagate headless mode to member re-drives and optionally wait on in-flight CI (#1731)
- **BUG-1044** — aida status leads with a no-active-role banner after reboot — last-used role and a copyable re-enter command (#1727)
- **BUG-1043** — MCP approval authority can disagree with the active shell role — refusals must name both roles and the relaunch command (#1726)
- **BUG-1038** — reviewer phase-child hits the queue-membership identity refusal — phase children need the pipeline's queue user, not just the phase role (#1725)
- **BUG-1037** — reviewer preflight checks resolve the forge as pure-git on a github-origin project — one forge resolution for the whole pipeline (#1724)
- **TASK-1202** — retire PROMPT_HISTORY from AIDA-scaffolded discipline — substrate and digest are the session record (#1723)
- **BUG-1025** — aida pull autostash can leave conflict markers inside .aida/config.toml — silently corrupting config for every later command (#1721)
- **BUG-1020** — aida agent type picker lists disabled vendors — the enabled-agents profile must gate the picker (#1718)
- **BUG-1019** — duplicate-agent guard offers Launch another anyway, then refuses anyway — the yes path must work or the offer must not exist (#1714)
- **BUG-1018** — Draft status glyph is the oversized circle — use the small circle to match the status column (#1713)
- **BUG-1017** — unified queue pickability policy — Done entries selectable by next/dry-run, NeedsAttention accepted without triage or force (#1711)
- **BUG-916** — submodule init on worktree creation ineffective in the field — empty gitlinks recur on 0.14.0 despite the BUG-899 fix (#1710)
- **BUG-915** — worktree .git file breaks git inside dev containers — gitdir points at a host path the container cannot see (#1709)
- **BUG-914** — worktrees expose .aida/ and .aida-store as untracked files — stage-into-MR hazard; use worktree-local git excludes (#1708)
- **BUG-913** — no-launch and dry-run paths bypass vendor resolution — disabled claude displayed and completion instructions hardcode claude (#1707)
- **BUG-906** — phase retry collides with its own dead predecessor's lease — retry must reap a lease whose process exited (#1702)
- **BUG-902** — BUG-898 vendor preflight is headless-gated — interactive queue work still resolves disabled claude and mutates state first (#1701)
- **BUG-901** — role-routed queue entries must be pickable by the orchestrator regardless of the adder's user identity — BUG-900 over-rotation (#1700)
- **BUG-900** — queue routing fractures across user identities — advisor requeue succeeds into a queue the implementer never sees (#1699)
- **BUG-899** — AIDA-created worktrees leave recursive submodules uninitialized — builds fail on empty gitlinks (#1698)
- **BUG-898** — queue work launches the disabled claude vendor instead of the enabled codex — after already creating the worktree (#1697)
- **BUG-896** — auto-open resolves the forge-less provider in the phase-3 recovery context — no-op open_change returns id 0 as success (#1695)
- **BUG-895** — phase-3 no-pr preflight must invoke the BUG-893 auto-open using the pushed branch after the worktree is gone (#1694)
- **BUG-893** — orchestrator opens the PR itself when the branch is pushed but no PR exists (#1693)
- **BUG-890** — interactive human review refuses on a stale lease instead of offering release-and-proceed (#1691)
- **BUG-888** — node-qualified spec ids (TASK-1-127) are mangled to a different id (TASK-151) by resolvers and branch/trailer derivation (#1690)
- **STORY-974** — Every drain shelve names a typed cause that a stranger can read (#1687)
- **BUG-882** — reviewer-scoped queue work on a Done spec is refused by the implementer preflight (#1686)
- **BUG-881** — from-pr resolver misses open PRs and must share the forge-first resolution (#1685)
- **BUG-880** — tail drain resolves a finished pipeline as live and exits silently on plain-text logs (#1684)
- **BUG-879** — phase 3 fabricates PR-0 when the implementer never opened a PR — reviewer launches against a nonexistent PR (#1683)
- **BUG-878** — phase-2 session teardown proceeds with unpushed implementer commits — CI and review run on the stale head (#1682)
- **BUG-876** — human review loses the spec's open PR once the lease is released — claims 'never pushed' despite trailered commit and open PR (#1680)
- **BUG-875** — reviewer-phase watchdog keys on commits/file-changes — kills every review longer than the window (#1678)
- **BUG-872** — pacing last-output age resolves the previous attempt's log — current session id must win (#1677)
- **BUG-862** — queue drain drives reviewer-routed items with implementer sessions — degenerate 10m watchdog kills (#1676)
- **BUG-868** — from-pr reviewer session reviews nothing when the spec is already Done — does queue cleanup instead of reviewing the PR (#1675)
- **BUG-866** — drain status pacing anchors spec/phase elapsed to drain start, not spec start (#1674)
- **BUG-864** — auto-filed phase-failure drafts do not dedupe — one doomed retry loop filed 7 duplicate records (#1673)
- **BUG-852** — drain exit summary double-counts retried specs and exits 0 despite shelves (#1669)
- **BUG-816** — human review open-PR hint emits gh pr create --fill, which fails from main — must pass --head <branch> (#1668)
- **BUG-836** — remote status labels a pure fast-forward lag as DIVERGED — should say BEHIND and suggest a plain push (#1667)
- **BUG-853** — session-end 'unshipped commits for PR-N with no open PR' warning false-positives on open and merged PRs (#1666)
- **BUG-851** — queue rework re-queues with empty for_role at the queue tail — unroutable and unordered (#1664)
- **STORY-835** — init's antigravity profile actually wires agy: AGENTS.md reach check + one-time --add-mcp offer + doctor probe (#1661)
- **BUG-842** — tail drain resolves a weeks-stale burndown log and exits — the live queue-work drain is invisible to it (#1660)
- **TASK-1194** — aida status leads with a live-drain line when a drain is in flight (#1658)
- **TASK-1193** — docs/plans/ is created on first use, not at init (#1656)
- **BUG-840** — role enter full-store load makes entering a role take 30s — route the title lookup through the cache (#1652)
- **BUG-839** — queue work --drain ignores --dry-run and launches a real drain (#1650)
- **BUG-838** — init silently skips a pre-existing AGENTS.md — AIDA-AUTOGEN block is never injected (#1648)
- **BUG-837** — session list role column misses AIDA hook-format role markers so known-role sessions show '-' (#1646)
- **BUG-826** — phase-1 launch resilience: retry an instant vendor death with backoff; auto-release the lease a zero-output session leaves behind (#1645)
- **TASK-1192** — auto-filed phase-failure drafts should auto-resolve when their spec completes (#1642)
- **TASK-1189** — review verdict findings[] has a consumer but no producer (#1641)
- **TASK-1191** — rework findings lookup can attribute another spec's verdict (len==1 fallback) (#1640)
- **TASK-1190** — rework findings block lacks the commit-or-punt contract sentence (#1639)
- **BUG-814** — queue rework never surfaces the RequestChanges findings — rework loops produce identical verdicts by construction (#1638)
- **BUG-800** — codex sessions repeatedly mis-invoke named tests (test name passed to the wrong binary) — 3 occurrences across independent sessions (#1630)
- **TASK-150** — Codex pickup prompt references unavailable aida-pickup skill (#1629)
- **BUG-807** — flaky test: mailbox publish_for_sync_stages_canonical fails intermittently even in near-isolation (#1628)
- **BUG-795** — queue work --auto-complete: items routed --for implementer are un-drainable and every error hides the way out (#1627)
- **BUG-810** — epic rollup counts an ACCEPTED ADR child as remaining — epic never closes (#1623)
- **BUG-809** — phase-3 verdict handshake: env anchor does not survive vendor tool sandbox — anchor via prompt text + sibling-checkout sweep (#1620)
- **BUG-794** — aida init --agent should be a strict scaffold allow-list (#1618)
- **BUG-806** — phase sessions run stale aida binaries and phase 3 ignores the spec-keyed verdict it already has (#1617)
- **BUG-804** — codex headless implementer stalls at pickup step 4: command body says confirm with the user, argument-as-consent rule lives only in the skill (#1615)
- **BUG-802** — reviewer verdict lands in the wrong directory: teach the existing "aida review verdict" verb the drive-root anchor and put it in the prompt (#1614)
- **BUG-799** — codex exec does not expand /aida-* prompts — drain phases send literal slash-commands; inline-render at the launch boundary (#1613)
- **BUG-790** — aida rel add silently binds a foreign spec ID to a colliding local requirement (#1612)
- **BUG-797** — codex /aida-review prompt drops the verdict-file handshake — every codex drain review shelves at phase 3 (#1611)
- **BUG-793** — Scaffolded .codex/config.toml carries project_trust_level, making Codex silently ignore the whole file (#1607)
- **BUG-789** — aida init pushes the store branch but silently leaves its own scaffold commit unpushed on main (#1604)
- **BUG-788** — aida list open shows accepted decisions while bare aida list hides them — the 'open' shortcut bypasses BUG-781's accepted-is-terminal exclusion (#1602)
- **BUG-784** — burndown plan --candidates and backlog list offer non-work types (decision/vision/term) as blessable candidates (#1596)
- **TASK-1171** — aida wrapper eval hardening (BUG-779 half 3): emit eval-able shell on a dedicated channel or behind --print-eval, so human output is never an eval candidate (#1594)
- **BUG-783** — aida list prints verbose 'N archived hidden / N deferred hidden' hint lines on every default/open view — noise for a daily driver (#1592)
- **BUG-782** — aida tail on a fan-out subagent session dead-ends with 'no live log' — should redirect to 'aida tail drain' (the parent that carries the stream) (#1590)
- **BUG-781** — accepted decisions render as Approved in the open list and do not auto-leave the default view (reads as pending work) (#1586)
- **BUG-779** — aida shell wrapper evals error output, turning any failure into 'command not found' noise (#1585)
- **BUG-780** — aida worktree exit is cwd-gated but the (wt:) PS1 marker is session-env-based — exit refuses ('nothing to step out of') while the marker persists after cd-out or external session-end (#1584)
- **BUG-778** — aida ps misreads a manually-entered worktree (worktree enter, agent not yet launched) as 'stalled: process dead' + suggests re-dispatch (#1583)
- **BUG-775** — queue done / merge accepts a spec whose latest review verdict is RequestChanges (#1581)
- **BUG-774** — queue add --for <role> is invisible to other users in that role (#1580)
- **BUG-777** — Orphaned leases block queue work with no self-service recovery path (#1579)
- **BUG-776** — session end auto-files an empty 'Review PR-0' story on repos with no remote (#1578)
- **BUG-773** — queue add succeeds ('✓ Added') but queue list count doesn't reflect it — write/read inconsistency stalls a drain (specs read as awaiting_signoff despite being queued) (#1577)
- **TASK-149** — burndown plan: serialize-held specs are mislabeled as awaiting_signoff (#1576)
- **BUG-771** — cache reported a Rejected epic (EPIC-7) as 'draft' in list/show — status drift misled an advisor into decomposing closed work; the add parent-gate (reading the store) caught it (#1568)
- **BUG-772** — queue list footer says 'needs you: resolve it out (--status rejected)' for a spec blocked-by an IN-QUEUE predecessor — advises rejecting authorized chained work the drain will unblock itself (#1567)
- **TASK-1165** — Thread the project root into the two remaining cwd-resolving events::emit sites (#1566)
- **TASK-1168** — aida ps role + spec columns truncate short identifiers to ~13 chars ('harness-workt…', 'general-p…') despite the table auto-sizing to content (#1565)
- **BUG-770** — Unit tests pollute the real .aida/events.jsonl via finish_escalated resolving project root from cwd (#1559)
- **BUG-769** — aida ps 'elapsed' column still shows lease-age (583h) for an adopted lease whose process has been up 3m — headline number contradicts the annotation BUG-763 added (#1558)
- **BUG-768** — BUG-764 rollup re-derivation regresses force-closed epics with zero open children back to in-progress (#1557)
- **BUG-767** — awaiting mail_unread scope-shifts to fleet-wide when the operator inbox is empty — deleting 3 made unread jump 3→18 (#1556)
- **TASK-146** — aida watch follow-mode replays entire historical events.jsonl backlog on start (#1554)
- **TASK-144** — pre-commit ///-provenance gate false-positives on pure code moves (#1551)
- **BUG-727** — Keystone 'do not merge, advisor-review' directive is text-only — pr ship auto-merged two keystones past it; needs a substrate marker pr ship honors (#1546)
- **BUG-725** — Queue registry is per-USER and non-mergeable, so the same user on two machines causes aida pull failures (#1543)
- **BUG-764** — epic rollup not re-derived when children complete via pull auto-bump — epics with zero open children stuck In Progress; comment-add doesn't refresh either (#1541)
- **BUG-766** — drive/drain child sessions resolve bare 'aida' from raw PATH — the installed months-old binary (0.9.1) ran unguarded bulk saves, the true writer behind ALL five deferred-shelf wipes (#1540)
- **BUG-763** — aida ps 'started' column is date-less — a 23-day-old persistent harness lease reads 'started 11:55 / elapsed 559h', an apparent self-contradiction (#1539)
- **BUG-765** — queue list per-item 'stalled — no live session lease' contradicts its own drain banner for burndown-fanout in-flight specs — last surface off the shared liveness probe (#1538)
- **BUG-762** — runtime counter/registry writes leave the orphan-store worktree perpetually dirty — every drain launch warns 'uncommitted changes; skipping pull' (#1535)
- **BUG-760** — first 'aida dev activate' in a fresh shell executes the INSTALLED aida (PATH not yet prepended) — old pin semantics can select debug; needs repo-binary self-resolution (#1533)
- **BUG-759** — aida drain status reports 'No drain in progress' while a live burndown run parent + heartbeated lock should exist — residual of BUG-755's lock-lifetime symptom (#1532)
- **BUG-761** — accepted decision specs are unarchivable without --force — approved IS terminal for the decision class; open lens accumulates ADRs forever (#1531)
- **TASK-1161** — full-store save: modified_at stale-write guard for core fields + convert auto-bump to targeted writes (BUG-756 follow-up) (#1530)
- **BUG-758** — skill-suggested merge commands include --delete-branch while the spec worktree provably holds the branch — guaranteed scary refusal + broken && chains (3rd occurrence) (#1529)
- **BUG-757** — cache self-heal misses a MISSING requirements_fts table when schema_version reads current — every cache op hard-errors until manual db deletion (#1527)
- **BUG-756** — deferred flag silently lost: TASK-919 + TASK-911 deferred with triggers on 07-17, found Approved/undeferred on 07-18 — twice — mechanism unknown (#1520)
- **BUG-754** — burndown harness-fanout implementer takes the lease but never flips the spec to In Progress — queue list shows Approved and in-flight for the same spec (#1518)
- **BUG-755** — burndown headless drain exits after wave 1 with unmerged PRs 'awaiting CI' and unstarted blessed specs — the promised background merge watches die with the session (#1517)
- **BUG-751** — decision-class specs cannot be marked accepted: --status approved refused (even --force), 'accepted' not in the status enum — yet ADR-6/ADR-10/ADR-11 already sit at approved/completed (#1515)
- **BUG-752** — Agent-tool (claude harness) subagent sessions read 'dead process, salvageable' in aida ps while alive — third liveness false-negative variant (#1514)
- **BUG-753** — queue list footer is drain-blind: recommends 'aida burndown run' for specs already scheduled in the LIVE drain its own banner reports (#1509)
- **BUG-734** — session end on a worktree-less lease scans an EMPTY worktree path for live processes — refused ending stale STORY-760 reviewer lease citing '2 live claude processes inside worktree ' (blank) (#1507)
- **BUG-723** — worker.cmd directive queue has no GC: stale drain orders for completed/rejected specs accumulate (latent headless-drain landmine) (#1506)
- **BUG-731** — codex prompt pack: 34 of 46 prompts drop the spec-id argument (no $ARGUMENTS placeholder) and leak Claude-native tool references (#1502)
- **BUG-741** — headless codex implementer sessions always read STALE/dead — lease liveness never sees the codex exec child process (#1501)
- **BUG-749** — implementer phase watchdog kills a session that is attached to 'aida pr ship' watching CI — CI-wait counts as no-progress 'degenerate spin' (#1500)
- **BUG-748** — batch drain panics mid-run: 'orchestrated implementer running without a live drain lock' — a shelve/skip path drops drain.lock, killing the rest of the batch (#1499)
- **BUG-740** — aida do / queue work --auto-complete launches the interactive implementer without a TTY — late confusing 'stdin is not a terminal' failure instead of an upfront gate (#1498)
- **TASK-142** — pr ship squash subject derives from branch HEAD commit — a merge commit yields an uninformative main-history subject (#1497)
- **TASK-141** — aida pr ship (no-arg) fails to resume an existing open PR (#1496)
- **BUG-733** — pr ship: bogus stacked-PR warning names the BASE branch as the merged branch ('keeping branch main — #NNNN stacked on it, retarget to main') (#1495)
- **BUG-732** — pr ship: merge step reports failure and abandons the lifecycle when the remote squash-merge SUCCEEDED but local branch delete failed (branch held by a worktree) (#1494)
- **BUG-743** — interactive codex launches ignore [agents] bypass=true — operator gets per-command approval prompts despite the recorded bypass posture (#1493)
- **BUG-742** — through-ci drive: the headless implementer merges the PR itself — /aida-pickup's 'aida pr ship' instruction overrides the stop-at-PR contract (#1492)
- **BUG-739** — queue add auto-stamps the generic harness-worktree lease scope onto entries — later drives route into the shared main checkout and refuse on the live lease (#1491)
- **TASK-1156** — worktree enter: Next-block is misleading — stale cd, unexplained source, premature session-end, and no actual next action (#1490)
- **BUG-730** — questions answer: a defer resolution still auto-queues the spec — evaluate_unpark ignores the deferred flag (#1488)
- **BUG-714** — Store mirror fan-out can only PUSH — a gitlab-only commit from another machine causes permanent divergence + recurring aida push failure (#1419)
- **BUG-688** — Cross-platform e2e suites fail on macOS (aida init exit 1, no output) + Windows (agent_error_channel empty stderr) (#1408)
- **BUG-726** — aida questions answer: a freeform (non-verb) resolution reads as an error ('unrecognized resolution X — recorded only') when it's a valid recorded directive (#1406)
- **BUG-724** — Machine-identity redaction is opt-in, so a work clone pushed a raw corporate email into the PUBLIC store (BUG-715 follow-up) (#1405)
- **BUG-722** — aida human reviews-awaiting is branch-derived, not spec-state-aware: false-positives on the aida-store store branch + deferred/archived specs (#1403)
- **BUG-721** — aida review launches the headless reviewer subprocess even on non-interactive stdin (no TTY gate) (#1398)
- **TASK-1141** — aida tui mail: unify project-root resolution with aida mailbox send (nested-worktree divergence) (#1396)
- **BUG-720** — aida human review resolves a spec's branch to the orphan aida-store + offers to PR the requirements store (#1392)
- **TASK-962** — STORY-621 Slice 3: route GitLabForge CI reads off broken 'glab ci list -F json' onto glab api pipelines (#1391)
- **TASK-1089** — Sharpen the completed-without-commit doctor scan: exempt non-code work-types + date-scope to post-git-canonical migration (#1383)
- **BUG-719** — Stale binary resurrects deliberately-deleted scaffold templates (no tombstone) (#1380)
- **BUG-718** — aida scaffold upgrade/apply corrupts master templates when run in the AIDA dev repo (writes through .claude symlinks) (#1379)
- **BUG-716** — aida pr ship self-merge guard (BUG-710) doesn't cover the SUPERVISED/interactive implementer — reviewer bypassed in --supervised drives (#1377, #1378)
- **BUG-715** — aida node acquire commits RAW machine identity (corporate email + hostname) to the store — leaks employer content to mirrors and would leak to the public origin (#1375)
- **BUG-698** — Residual parallel-test flake after BUG-697 — bug670 still fails ~rarely in CI (read_queue_depth None + git untracked-files co-symptom) (#1373)
- **BUG-702** — aida pull emits 'fatal: Not a valid object name <sha>' when a registered agent's build_sha was purged by a history rewrite (#1372)
- **BUG-694** — aida session end leaves orphaned mcp-claim.<spec>.toml behind — no reaper (#1371)
- **BUG-710** — Headless implementer can self-merge, bypassing zen's promised independent reviewer (#1370)
- **BUG-713** — Codex prompts reference unreadable .claude/skills paths — codex loses the workflow (TASK-1114 conversion gap) (#1370)
- **BUG-711** — zen AlreadyMerged/terminal-skip path leaks implementer lease + pool worktree (skips phase-2 session-end) (#1368)
- **BUG-708** — aida zen under-specified gate fires on optional EARS clarity nits + misattributes them as missing acceptance (#1366)
- **BUG-709** — Post-gh-pr-create PR-index verify hangs when the PR already merged (searches only OPEN PRs) (#1366)
- **BUG-703** — no-human acknowledgement warning leaks a trace SPEC-ID marker into user-facing CLI output (#1364)
- **BUG-706** — session end --force still prompts Continue? [y/N] on non-TTY — headless lease cleanup impossible without piping y (#1364)
- **BUG-704** — Project-level [agents] vendor knob is invisible to drain phases — .aida/agents.toml is gitignored so pool/agent worktrees silently fall back to claude (#1363)
- **BUG-705** — no-human phase-1 implementer launch ignores the headless vendor — exec path hard-builds claude argv, so codex-routed drains always try claude (#1362)
- **BUG-701** — aida add is ~8.5s (edit is 1.6s) — the ~7s delta is spec-id allocation, not the git commit or store bloat (#1357)
- **BUG-700** — Employer-identifying content published in public aida-store branch — needs history-rewrite scrub (#1353)
- **BUG-699** — Fix aida usage: command-shape leaks arg ids (privacy-floor violation) + 30d aggregates mislead vs recent behavior (#1350)
- **BUG-697** — Flaky CI test: bug670_agent_status_tests::actionable_matches_queue_list_on_a_padded_fixture fails intermittently, passes locally (#1342)
- **TASK-1088** — aida ps: emit TOON in agent mode instead of the human running-work table (STORY-753 slice) (#1341)
- **BUG-696** — salvage_worktree_patch omits untracked file content — aida doctor --heal loses untracked files in orphan-worktrees (#1340)
- **BUG-695** — aida awaiting ignores AIDA_AGENT_OUTPUT — leaks box-drawing + emoji to agents instead of TOON (#1339)
- **BUG-693** — aida integrate running-work view shows stale leases that aida ps hides — surface inconsistency (#1337)
- **BUG-691** — aida pull strands its autostash on index.lock contention — the hard-reset step fails, stash left un-restored (#1335)
- **BUG-692** — git-guardrails hook false-positives on destructive-git tokens inside a quoted argument (blocks legit aida commands) (#1334)
- **BUG-677** — Lift the /proc liveness probe + lease classifiers from aida-cli into aida-core so aida tui computes liveness in-process (no aida ps shell-out) — durable follow-up to BUG-676 (#1327)
- **BUG-686** — EARS lint conflicting-constraint heuristic false-positives on each/none co-occurrence, holding well-specified specs from the zen gate (#1326)
- **BUG-675** — BUG-89 identity unification: queue_depth's user-resolution path must match the queue list command path so it's always trustworthy (not just suppressed on mismatch — BUG-670 remainder) (#1322)
- **BUG-680** — Plan Followups filed twice: SPIKE-70 followups re-filed as TASK-1038..1044, duplicating already-merged TASK-1003/1005 (#1319)
- **TASK-1071** — Info/notice glyphs render raw and ignore AIDA_GLYPHS=ascii — register them and have glyph-lint flag raw glyphs (#1300, #1318)
- **BUG-687** — Windows pid_is_alive(0) reads true (System Idle Process) — missing pid==0 guard in non-unix impl (#1314)
- **TASK-1074** — Reconcile descendant_ids vs graph --tree subtree membership (43 vs 44 on EPIC-54) (#1311)
- **TASK-977** — Stranded-primary alarm: count only LIVE leases (process-probe) to suppress false positives (#1309)
- **TASK-1078** — zen suitability gate must hold needs-design specs, not just lint-under-specified ones (#1307)
- **BUG-682** — zen help promises "fire several in parallel" but the global drain.lock refuses concurrent solo drives (#1298)
- **BUG-684** — Empty aida list dead-ends a fresh user; queue-next hint uses deprecated 'dialog'; queue done <draft> silently skips lifecycle; agent error quoting inconsistent (#1295)
- **BUG-683** — Corrupt .aida/cache.db is a no-escape dead-end — every cache read fails with a misleading WAL error and 'aida cache rebuild' also fails; self-heal on corruption (#1294)
- **BUG-681** — Per-turn coordination-notice hook (aida awaiting --notice) can exceed Claude Code's 5s timeout under cache-lock contention — must bail instantly, never block a prompt (#1292)
- **BUG-678** — aida focus rollup counts rejected specs as "open" (#1291)
- **BUG-679** — Mail to an unrecognized recipient strands silently — no warning at send, no stranded-mail surface (#1288)
- **BUG-676** — aida tui liveness poll spawns aida ps (~1.3s) every 3s and processes pile up — makes the machine sluggish while the TUI is open (TASK-978 regression) (#1285)
- **BUG-674** — The punt ledger never closes: no CLI resolve/dismiss verb, resolving the spec doesn't clear the punt, and session-end warnings bury genuine punts (#1284)
- **BUG-670** — aida status agent-mode: queue_depth contradicts queue_actionable and queue list (different user-resolution paths, BUG-89 surface) — unify the path + lead with the actionable count (#1282)
- **BUG-673** — next[] guidance drops out at queue work/done — after queue done, an agent isn't told to aida pull (strands the spec at Done) (#1277)
- **BUG-672** — aida search (default) + aida graph emit human formatting to agents, not TOON, and lack next[] guidance (#1276)
- **BUG-671** — agent-mode write verbs: queue done silently no-ops on EOF; guard errors hide the --force/-y override an agent needs (#1275)
- **BUG-669** — Pool-worktree reuse inherits the prior session's stale lease — aida ps mislabels the work (an ADR-7 guided session showed as TASK-0439); acquiring a pool worktree must reset the lease to the new spec (#1271)
- **TASK-1064** — aida ps: in-progress specs being worked by an advisor fan-out (non-spec-linked harness-worktree lease) should not list under 'Orphaned' — surface them as 'likely worked by a fan-out' vs genuinely orph (#1271)
- **BUG-658** — aida list --tree EPIC group header shows raw stored status, not the BUG-626 rollup (#1269)
- **BUG-656** — Auto-followups recurred on EPIC-0428 plan completions — filed cross-plan followup cruft (TASK-1019-1032) despite BUG-655 (#1268)
- **BUG-667** — Flaky CI test gc_aggressive_reduces_pack_count ('failed to run repack') — harden the STORY-733 compaction test against the CI git environment (#1267)
- **BUG-665** — aida pull leaves a stale binary silently — warn + suggest cargo build when HEAD moves past the built binary's SHA (#1266)
- **BUG-668** — aida search --fields has no agent/TOON output path — completes STORY-734 humans+agents parity (search costs agents 2x tokens) (#1265)
- **BUG-657** — auto-complete drain of an already-completed spec spawns a failing implementer + auto-files duplicate failure bugs (#1263)
- **TASK-1054** — Drive exit codes: distinguish shelved/parked from hard-fail (preserve EPIC-28 exit-2 contract) (#1263)
- **BUG-660** — Unattended-drain robustness: no sleep-prevention / exit-summary / backoff (gnhf gap) (#1260)
- **BUG-666** — guided-implement: work the spec directly (no Explore/Agent sub-agents) + a simple one-command finish (#1259)
- **BUG-664** — Cache readers block ~25s behind a writer; cache status does a full-store load for a count (#1254)
- **BUG-663** — Store auto-gc repack runs inline on writes — move it off the write path (6-16s spike regression) (#1253)
- **BUG-662** — git-guardrails also false-blocks feature-branch deletion (analogous to BUG-661 force-push) (#1244)
- **BUG-661** — git-guardrails false-positive blocks worktree feature-branch force-pushes (checks CWD HEAD not the push target) (#1241)
- **TASK-1049** — aida zen should run an independent reviewer phase before hand-off (slice 1 skips it) (#1232)
- **TASK-0439** — Tree output hides real parent details in group headers (#1231)
- **TASK-1011** — Cross-platform file-lock parity (treehouse has `lock_unix.go` / `lock_windows.go`); AIDA must match for the nightly cross-platform matrix (#1226)
- **BUG-655** — Auto-followups duplicates when a plan with a Followups section is committed by multiple agents/PRs (#1219)
- **BUG-653** — aida agent new --spec <epic> dead-ends: says transition-to-Approved but epic status is a read-only rollup (cant approve) (#1216)
- **BUG-654** — aida worktree enter silently no-ops the cd when the installed shell wrapper is stale (no hint) (#1216)
- **BUG-651** — Pre-commit advisor-code-gate aborts when PATH aida is stale, forcing agents to --no-verify (#1214)
- **BUG-652** — session end --return refuses on a dirty pool worktree, silently breaking reuse (#1208)
- **TASK-943** — TUI fuzzy filter targets the focused panel — typing to find an item filters Scopes instead (#1205)
- **BUG-638** — TUI: verb palette must gate by ROLE — advisor-only verbs (approve) show + are selectable for an implementer (#1198)
- **BUG-643** — aida dev activate auto-pin does not flip to the newer build; remediation advice is wrong (#1195)
- **BUG-642** — CLAUDE.md references aida-on / aida-off which no longer exist (shell-init uses the aida() wrapper) (#1193)
- **TASK-298** — Headless orchestrator watchdog: parse stream-json for permission_denials; detect 'is_error' false-positive (#1190)
- **TASK-969** — Config trust boundary: load code-executing config from the trusted default branch SHA (#1186)
- **BUG-641** — aida findings list: finding-ID vs linked-spec reversed in layout (agents misread it) (#1185)
- **BUG-640** — Patch-id force-push guard: refuse force-push unless remote commits are incorporated (#1178)
- **TASK-971** — SessionStart context hook: emit to stdout (not stderr) + wire in settings.json (#1177)
- **BUG-639** — GitLabForge MR JSON-read methods use non-existent glab flag (--output json) — broken vs real glab 1.36.0 (#1176)
- **BUG-637** — Duplicate dispatch: two agents worked the same In-Progress spec (BUG-634) with no spec-scoped claim — add a claim/lock fan-outs respect (#1170)
- **BUG-628** — BUG-626 epic-rollup edge cases: archived-completed children excluded from rollup; rel-add guard reads stored status while display reads derived (#1165)
- **BUG-629** — Pre-commit /// gate rejects ANY SPEC-ID token in a doc comment, not just trace: markers — over-broad vs documented rule (#1163)
- **BUG-632** — Flaky test: queue_add_then_list_same_shell_is_consistent (false CI red under --workspace parallelism) (#1162)
- **TASK-940** — aida ps: exclude rollup-types (epics) from the orphan In-Progress pass (#1161)
- **BUG-636** — PERF: cache fully rebuilds on every store HEAD move — incremental update needed for multi-agent stores (#1158)
- **BUG-634** — PERF: single-spec writes take ~13-20s — write path full-scans all 2488 YAML (backend.load + find_by_uuid) (#1156)
- **BUG-631** — queue-add advisor-authority gate is too broad: 'aida queue add --for advisor' shouldn't need advisor authority (#1151)
- **BUG-627** — Cache freshness check is SHA-based and schema-blind — a schema change (e.g. blocked column) leaves a FRESH-but-stale cache that hard-errors instead of self-healing (#1147)
- **TASK-928** — SPIKE-71 source fix: parent/child rel-add bidirectional by default + tag-to-edge + cache invalidation (#1143)
- **BUG-626** — An epic should not be In-Progress with no children — derive epic status from child rollup, or warn/block the manual set (#1142)
- **BUG-623** — aida why / session leases report a long-idle in-flight lease as 'being worked now' — a hung/abandoned session reads as active (#1141)
- **BUG-625** — Statusline inbox count never clears — out of sync with read-watermark (#1140)
- **BUG-624** — STORY-684 pre-commit /// gate scans WHOLE FILES — flags pre-existing debt, forcing agents to --no-verify (which bypasses the advisor-code-gate too) (#1133)
- **BUG-622** — STORY-684 advisor-code-gate false-blocks worktree implementer agents — AIDA_SESSION_ROLE=advisor leaks from parent session into agent env (#1126)
- **BUG-619** — aida tui lags navigating to PRs — fetch_prs() is a synchronous gh call per navigation; load async + refresh, don't block the cursor (#1114)
- **BUG-621** — aida queue footer self-contradicts: 'needs you: ... nothing to do' for in-flight items (render_path_to_empty) (#1113)
- **BUG-618** — Optimize aida queue list --json to resolve titles via SQLite cache summaries (1s->0.2s) (#1112)
- **TASK-857** — aida add network floor: replace unconditional pull+rebase/push with a lighter id-collision check (batched/deferred push) (#1111)
- **BUG-620** — aida tui tab still labeled 'dialog' — should display 'advisor' (canonical per TASK-586) (#1109)
- **BUG-609** — Fleet-state hygiene: aida status counts dead-PID agent corpses + lists abandoned worktrees (reap/hide by default) (#1108)
- **BUG-616** — TUI panel reads use full-scan paths — queue panel ran aida status (~3.75s), row preview ran aida show with git-linkage (~1s/item) (#1105)
- **BUG-615** — aida add --parent X --blocked-by Y silently drops the blocked-by edge (#1104)
- **BUG-614** — git worktree leak: finished sessions/Agent-tool worktrees never GC'd — 63 accumulated, inflates status residual + caused an agent-worktree-removal stall (#1102)
- **BUG-613** — aida status is O(store-size) — spawns ~1 git process per spec (2186 = 27s); TUI calls it at startup + per-panel → 20-30s launch + 3s nav lag (#1101)
- **BUG-608** — aida init --sibling creates a NESTED store (repo/aida-store), not a true sibling (../aida-store) — defeats multi-repo sharing + contradicts its own message (#1085)
- **BUG-610** — aida init --sibling against an EXISTING shared store destructively re-initializes it (deletes other repos' specs, re-seeds META) with NO reset prompt — silent data loss; needs an attach path (#1085)
- **BUG-605** — aida backlog groom queues under the grooming session's identity/role — groomed work invisible to the human's drain shell (#1084)
- **BUG-607** — Stale drain-state.json from a killed/crashed drain makes a fresh burndown falsely hold (no orchestrator_pid liveness gate on the read) (#1084)
- **BUG-606** — aida doctor completed-without-commit scan false-positives en masse (1464 reported; specs DO have commits) (#1082)
- **BUG-604** — aida init git-init success line is a double-verb: "Created initialized a git repository here" (#1079)
- **BUG-602** — Concurrent set-field merge resurrects tombstones (removed tags/relationships/dependencies come back via 2-way union) (#1075)
- **BUG-599** — User typo of a spec ID triggers misleading 'binary version mismatch — rebuild aida' guidance (#1074)
- **BUG-600** — aida show exits 0 on not-found; non-advisor status-promotion refusal also exits 0 (exit-code-0-on-error) (#1074)
- **BUG-601** — aida graph not-found gives wrong 'no aida store found / cd into project root' guidance from a valid project root (#1074)
- **TASK-887** — aida rel add --type <typo> silently creates a Custom relationship (no warning / did-you-mean for standard-type typos) (#1073)
- **TASK-888** — Error-path message-quality nits (grouped): empty-title accepted silently, bare-digit show routes to PR with no hint, unbounded title length (#1073)
- **TASK-884** — aida lint passes empty-body specs clean; advisor worklist has no decompose lane for draft epics (#1072)
- **BUG-597** — Onboarding walkthroughs show wrong 'aida add' output AND tell newcomers to use --status approved that silently downgrades for the default role (#1070)
- **BUG-598** — docs/user-guide.md is broadly stale (v0.1.0 footer): wrong env vars, removed commands, wrong type prefixes, 4-state model, desktop-app sections (#1069)
- **BUG-593** — aida advisor / aida advisor status include seeded META prompts as groomable drafts (BUG-464 fix did not reach the advisor surface) (#1068)
- **BUG-594** — Advisor auto-gates (assess/backlog groom --pickable) do not honor keystone/supervised tags — only queue integrate does (TASK-813 partial) (#1068)
- **BUG-595** — assess risk heuristic over-flags well-specified specs as high-risk, fencing them out while thin specs pass (#1068)
- **BUG-596** — questions sweep flags approved specs for 'missing acceptance' but never the thin DRAFTS the advisor's distill section points at (#1068)
- **BUG-592** — Fresh project's advisor surface shows phantom advisor-decisions (TASK-340/403/440) backfilled from AIDA's own dev punts (#1067)
- **BUG-589** — MCP status-gate refusals tell agents to 'role_enter advisor' — but role_enter is peek-only and can't unlock the gate (#1066)
- **BUG-590** — MCP triage_finding dismiss returns malformed 'dismissd' (missing 'e') in agent-facing success text (#1066)
- **BUG-591** — MCP list_requirements + search_requirements ignore the archive/deferred view tiers — show archived rows the CLI hides, and offer no archived/deferred/all filter (#1066)
- **TASK-883** — MCP post_punt accepts 'reason' as an alias for 'detail' but inputSchema only advertises 'detail' (hidden accepted arg) (#1066)
- **BUG-588** — aida edit writes no history: array row to the spec YAML (audit trail missing; --id history empty) (#1065)
- **BUG-586** — Concurrent same-spec different-scalar-field edits silently lose one side (object LWW ignores unioned oplog) (#1064)
- **BUG-587** — aida edit --tags writes no oplog operation (tag changes bypass the CRDT substrate) (#1064)
- **BUG-584** — aida status shows perpetual 'Store drift' false alarm during normal use — alarming to new users (#1062)
- **BUG-585** — init teaches 'aida done <ID>' as the loop's final step, but it fails non-interactively with circular guidance (#1061)
- **BUG-583** — Brief generated ## Setup block hardcodes the binary repo path, not the target project — misdirects a cold vendor (#1057)
- **BUG-582** — aida human reviews-awaiting flags Completed/merged-PR specs (stale local branch false-positive) (#1049)
- **BUG-581** — Flaky os_wrap tests: AIDA_OS_WRAP env leak across parallel test threads (#1048)
- **BUG-580** — Generated-marker HTML comment leaks into slash-command help display (/aida-pr shows the checksum DO-NOT-EDIT line) (#1029)
- **BUG-579** — aida human counts AwaitingDecision specs (no posed question) in the total but renders no bucket for them — phantom "1 item needs a human" (#1027)
- **BUG-578** — HLC is dead code: scalar merge uses wall-clock LWW with ours-wins tie-break (non-deterministic across clones), not the HLC the docs claim (#1018)
- **BUG-577** — rel list rejects --spec/--id (clap exit 2) though --spec is valid elsewhere — likely bulk of the 50% rel-list error rate (#1014)
- **BUG-576** — aida compete default gate is narrower than CI (build+test only) — a winner can pass the gate but fail CI on fmt/clippy/glyph-lint (#1004)
- **BUG-575** — aida compete commits the vendor run-log into the candidate branch — pollutes the diff + corrupts diff-size signal + leaks on merge (#1002)
- **BUG-574** — Reliability papercuts: pr ship 25% err (avg 207s) + session start 35% err — root-cause (#997)
- **BUG-573** — rel list (+ read-only verbs) exit non-zero on dangling-relationship warnings — poisons usage telemetry + papercuts hooks (#992)
- **BUG-572** — Ungroomed to-groom draft with human_only=true leaks onto aida human as a phantom decision (no question) — should stay advisor grooming work (#990)
- **BUG-571** — Web dashboard loads a 6.5MB all-requirements blob on every view — slow clicks/nav; needs a summary endpoint + detail-on-click (#987)
- **STORY-653** — Dashboard roster identity coherence: group/key on the person identity matching roles+queues, friendly labels + node names (#985)
- **BUG-570** — aida init on a clone silently commits a scaffold dump to the code branch (#965)
- **TASK-844** — aida init: push main before aida-store on push-to-create so the forge default branch is main (BUG-559 root-cause prevention; needs GitHub+GitLab live verification) (#963)
- **BUG-559** — GitLab: aida init push-to-create makes orphan aida-store the default branch, breaking fresh-clone auto-attach (#916, #957)
- **BUG-569** — Advisor writes emit "agent target antigravity is ambiguous" warning when 2+ antigravity agents are registered (#956)
- **BUG-568** — Sibling/workspace mode: auto-bump, reconcile-status, show-linkage & trace-scan silently scan only the local repo — cross-repo completions never bump, linkage incomplete (#952)
- **BUG-566** — cwd-unreadable warning misattributes ALL current_dir() errors to 'aida session end' + discards the real io::Error (network-share/permission cases get a misleading message) (#950)
- **BUG-567** — Project/store env vars fall back silently: AIDA_STORE set-but-unusable is ignored with no notice; AIDA_PROJECT_ROOT is outbound-only yet reads like a read-side hint (#950)
- **BUG-564** — aida list human / aida human omits human-only-marked specs (view layer feeds human_required a hardcoded false) — contradicts the queue's human-only bucket (#939)
- **BUG-565** — aida init: scaffolding auto-commit is all-or-nothing — one gitignored path (.claude) aborts the whole git add, silently stranding the onboarding task (#939)
- **BUG-563** — doctor: detect+heal per-clone runtime files (node.toml/dispenser.toml) wrongly TRACKED on the orphan aida-store branch (#938)
- **BUG-562** — aida solo --watch: every cycle step fails to spawn (ENOENT) after the binary is rebuilt mid-run — current_exe() goes stale (#937)
- **BUG-561** — aida intake candidate fence does not honor the deferred view-flag (surfaces parked specs as bless candidates) (#936)
- **BUG-554** — Sequential-chain agents stack branches on the prior unmerged commit (reused worktree, no reset-to-main) → semantic rebase conflicts + spec↔branch mis-attribution (#928)
- **TASK-823** — aida human/advisor worklist UX: conditional footer (drop empty/misleading pointers) + reframe groom guidance away from blind-approve (#917)
- **TASK-821** — BUG-559 substrate slice: actionable guidance when a fresh clone has aida-store checked out (auto-attach resilience) (#916)
- **BUG-560** — aida status leaks raw gh GitHub-auth error on a GitLab remote (PR row); should degrade cleanly for non-GitHub forges (#915)
- **BUG-558** — aida agent new does not set AIDA_USER: launched agents read the human's mailbox, not their own (#911)
- **BUG-557** — aida mailbox send --in-reply-to is accepted but doesn't thread the reply (opens a new thread) (#910)
- **BUG-556** — aida human / burndown explain still list structurally-deferred specs (aida defer flag); collect_open_facts skips archived but not the deferred view-flag (#907)
- **BUG-555** — Inter-agent mailbox is unreliable: unread count won't clear / seen-ack doesn't stick / handoffs re-deliver — blocks brief-driven autonomous integrator (#901)
- **BUG-553** — aida human reviews bucket attributes a spec to the WRONG branch when its commit is reachable from multiple branches (stacked/reused-worktree) (#899)
- **TASK-807** — aida human 'ungroomed' bucket: relabel second-person/action-oriented — 'awaiting advisor' reads as a third party when the operator IS the advisor (#898)
- **TASK-808** — aida human 'umbrella' bucket: only surface epics that NEED action (decompose if childless/thin, ready-to-close via BUG-543) — drop merely-in-flight epics as noise (#898)
- **BUG-543** — Epics don't auto-complete (or surface) when their rollup hits all-children-completed — fully-delivered epics linger as Draft (#890)
- **BUG-538** — burndown run / queue work --auto-complete has no concurrency guard — a second drain double-drives the same tree (#889)
- **BUG-550** — aida human reviews bucket misses open PRs whose spec isn't Done (gates candidate set on Done-status) — should also catch open PRs on Approved/In-Progress specs (#887)
- **BUG-536** — Squash-merge of an umbrella PR leaves child specs stuck (Draft/Done) — auto-bump only sees the one trailered spec (#886)
- **BUG-541** — burndown drain exits before its LAST PR's CI goes green — leaves it open+unmerged (orphaned) (#885)
- **BUG-552** — git-guardrails: short '-f' trailing AFTER the refspec bypasses the force-push guard (e.g. 'git push origin main -f') (#883)
- **BUG-548** — aida-git-guardrails.sh allows force-push to main via --force-with-lease — should block ANY force-push to protected branches (#882)
- **BUG-551** — burndown plan Supervised section lists non-actionable statuses (Done/Completed) — supervised check runs before the status filter (#881)
- **TASK-798** — Config coherence: contained ENABLE is [agents] contained but the egress allowlist is [contained] allowed_hosts — two sections for one feature (#880)
- **BUG-549** — burndown subagents never run 'aida queue done' — specs land at Approved, normal Done→Completed auto-bump never fires (#879)
- **BUG-546** — aida human reviews-bucket (+ PR linkage) resolves spec↔PR by BRANCH NAME — misses PRs whose branch is named for a different spec than the commit trailers (#876)
- **BUG-544** — aida human counts ALL findings (17) as 'awaiting triage' while aida findings list shows 0 — inflated count + bogus 'N open items' total (#875)
- **BUG-545** — aida edit --tags REPLACES all tags instead of adding — silent clobber (no --add-tag/--remove-tag) (#874)
- **BUG-542** — aida schema [OBJECT] help says 'currently: requirement' but finding/punt/lease/etc. all resolve now (#872)
- **BUG-547** — AIDA Book nav 404: mdBook rewrites README.md links to README.html but outputs index.html (#871)
- **BUG-540** — aida review 'Open the diff' runs invalid 'gh -C <dir> pr diff' — gh has no -C flag, so the option errors (#868)
- **BUG-539** — aida review re-reviews already-Completed/merged specs + offers a stale review→merge menu (should short-circuit) (#866)
- **BUG-530** — burndown run blessed-set is stale/inconsistent: reported draining Completed + unqueued specs (TASK-778, BUG-520), violating its own contract (#865)
- **BUG-535** — aida human lists bare SPEC-IDs without titles — same scannability gap as burndown plan (BUG-532) (#859)
- **BUG-537** — burndown plan / resolve_burndown_sets doesn't exclude deferred specs — deferred:true specs pollute awaiting-sign-off (#855)
- **BUG-534** — aida graph --tree renders a FLAT list, not an indented tree — parents and siblings are indistinguishable (#852)
- **BUG-533** — aida config show only renders ID config — expand to the full effective policy surface (agent bypass posture, mailbox, advisor, archive, telemetry, intake, presence) (#850)
- **BUG-532** — aida burndown plan is ID-only — add the title/subject per spec for scannability (mirror aida list / queue list) (#848)
- **BUG-531** — aida search lacks the output-mode flags aida list has: mirror --short/-q/--ids-only/--quiet (and --json) for surface parity (#847)
- **BUG-529** — aida queue remove accepts --for <role> but ignores it — removes the user's entry regardless of role filter (#845)
- **BUG-527** — aida show omits queue membership — can't tell if a spec is queued / for whom / at what position (#844)
- **BUG-528** — aida add --queue mis-routes: no --for flag, defaults to the FILER's role — advisor-filed implementation work lands in the advisor queue (#843)
- **BUG-526** — questions answer: when the answerer has advisor authority, promote a Draft spec to Approved so the auto-queue fires (#841)
- **BUG-522** — session namespace collides two unrelated concepts: leases (active scoped work) vs list (historical Claude conversations) (#840)
- **BUG-521** — aida agent pause does not pause the process — verb misleads (only sets a dispatch marker; stop is what terminates) (#839)
- **TASK-778** — CLI consistency audit: papercuts + inconsistencies surfaced writing the CLI manual (checklist) (#838)
- **BUG-520** — help-all omits real commands (rules, doctor, defer) — they exist with full --help but appear in no group (#837)
- **BUG-523** — Real SPEC-IDs leak into --help output (aida review shows STORY-553, digest STORY-541, why STORY-543) (#833)
- **BUG-519** — statusline: do not warn-glyph a deliberately-started general-purpose session (#823)
- **BUG-518** — statusline: cache:? is noise in a store-less worktree — show cache:n/a (or omit) when no .aida-store is attached (#822)
- **BUG-524** — aida show: display Opened (created_at) + Modified (modified_at) timestamps, in local time (#821)
- **BUG-517** — queue list: terminal-hidden hint is noise on the default role view (keep it only on --all) (#819)
- **BUG-513** — mailbox list operator-overview omits unread broadcasts to not-yet-materialized agent inboxes (disagrees with statusline) (#812)
- **BUG-514** — burndown classify: parking-tag should win over the spike-lane reason (deferred spike mislabeled 'dispatch to research') (#810)
- **TASK-755** — aida doctor completed-without-commit: exclude non-code spec types + bound the historical tail (currently 403 [manual] findings = noise that buries real drift) (#806)
- **TASK-761** — Substrate-grade gate: refuse aida edit --status approved for do-not-approve classes by type (#803)
- **STORY-580** — Enable WAL journal_mode on the cache for non-blocking concurrent reads (salvaged from STORY-543, fresh against the current retry-ladder cache) (#802)
- **BUG-506** — aida pull auto-bump catches only ONE (SPEC-ID) trailer when a commit references multiple — BUG-503 stayed Approved after a (BUG-503) (BUG-504) merge (#799)
- **BUG-509** — panel-review workflow: args/{target} not interpolated — analysts get a placeholder and silently fork onto different subjects (#798)
- **BUG-510** — aida review <spec> should pre-flight a behind-main/stale-base check + warn before reviewing (parity with the reviewer-role STORY-281 pre-flight) (#797)
- **BUG-511** — aida review <spec> takes no lease / in-flight marker — queue footer + aida why still say 'review it' while a review is running (#797)
- **BUG-1023** — rework sessions work on a <spec>-pr<N> branch while phase 2 drives <spec> — systematic branch-mismatch refusals
- **BUG-1074** — queue work on a fresh spec reused an unrelated recent PR branch — BUG-1023 head-reuse misfires beyond reworks
- **BUG-1078** — transient-retry lease reclaim races predecessor teardown: Live verdict seconds after death shelves a recoverable retry
- **BUG-1082** — headless implementer re-polls `aida queue next` and implements a different spec than its orchestrator assignment
- **BUG-516** — aida-generate-types omits InterfaceChanges decl — regen produces types.ts with undefined 'InterfaceChanges', breaks web dashboard type-check
- **BUG-659** — Tree output repeats parent rows under Unscoped
- **BUG-690** — auto-complete failure: phase 5 (pull) on BUG-680
- **BUG-707** — aida-demo.sh broken by TOON agent output + top-level 'local'
- **BUG-712** — drain.lock left with dead pid on clean drive exit (process::exit skips DrainGuard Drop)
- **BUG-717** — MCP status_unified counts META/stateless specs; CLI status/list excludes them (surface parity drift)
- **BUG-735** — aida do <spec> refuses on the git-canonical backend — 'Command not yet supported for git backend' (wired only into legacy dispatch)
- **BUG-897** — phase-3 reviewer spawn must be PR-scoped — spec-scoped relaunch refuses when the implementer already dequeued the spec
- **BUG-917** — scaffold status reports permanent false drift in the AIDA source repo (symlinked templates)
- **TASK-1129** — positioning-PDF renderer: break long inline-code tokens so they don't overflow narrow table cells
- **TASK-1130** — SPIKE-73 harness: reseed per-run for chained_followup (fix false-negative) + bring current 4-condition harness into main
- **TASK-1195** — one-shot Ask-AI arm should run with a read-only tool posture
- **TASK-152** — aida ps and session conversations report different roles for the same pid (lease role vs jsonl hook role)

### Documentation

- **TASK-1203** — discipline: agents re-read lease/brief state before acting on remembered identifiers — idempotent not-found outcomes are not bugs (#1728)
- **TASK-1201** — identify the auto-relaunch path that re-drove STORY-993 unprompted (#1722)
- **TASK-1188** — Codex statusline parity: Codex now owns the terminal title — update guidance to disable [tui] terminal_title and document the tmux status-right path (#1633)
- **TASK-0426** — Refine external-facing AIDA positioning for agent migration audiences (#1622)
- **TASK-1183** — Document the commit trailer as the standalone, portable adoption unit (#1621)
- **TASK-1021** — Followup TASK (file at sign-off): wire `[autopilot]` into `docs/environment-variables.md` and the scaffolded `.aida/config.toml` comment block (#1563)
- **TASK-995** — Wire `Monitor`-based consumption into the `/goal` overnight loop skill template (skills-side, not Rust) (#1562)
- **TASK-1017** — Followup: the §8 autonomy-doc edit is a *living-doc* update — leave dated SPIKE/snapshot artifacts frozen (`feedback_dated_artifacts_immutable`) (#1561)
- **TASK-145** — handle_status_spec doc comment describes 'aida focus', but the function renders 'aida status <spec>' (#1550)
- **SPIKE-75** — Evaluate Anthropic advisor tool (executor+advisor in-inference pairing) as a per-vendor EXECUTION capability behind the vendor adapter (#1536)
- **TASK-0434** — Design first-class subsystem representation (#1536)
- **TASK-0435** — Design specialized advisor routing by subsystem (#1536)
- **TASK-0436** — Connect subsystem focus to memories docs and context loading (#1536)
- **TASK-0437** — Define cross-subsystem arbitration and master advisor authority (#1536)
- **TASK-1125** — Paper: write up the TASK-1123 supervised reviewer-bypass as a field observation (cause taxonomy for SPIKE-67 / section 13) (#1526)
- **TASK-1163** — aida-assess skill drift: references missing .claude/skills/aida-intake.md and nonexistent 'backlog groom --pickable --apply --risk' flags (#1525)
- **TASK-1058** — Multi-vendor coordination research + positioning doc refresh (June 2026) (#1522)
- **TASK-1126** — Add a second workflow-walkthrough note to docs/flow-smoke.md (guard verification) (#1384)
- **TASK-1123** — Add a workflow-walkthrough note to docs/flow-smoke.md (#1376)
- **TASK-1119** — Flow smoke-test 3: append a third dated marker line to docs/flow-smoke.md (#1369)
- **TASK-1118** — Flow smoke-test 2: append a second dated marker line to docs/flow-smoke.md (#1367)
- **TASK-1115** — Flow smoke-test: add a dated marker line to docs/flow-smoke.md (#1365)
- **SPIKE-76** — Resilience for multi-vendor agent dispatch: survive connection drops, agent crashes, and single-vendor dependence (#1347)
- **STORY-756** — Surface aida why <file:line> as the README hero — lead with the 60-second magic, demote probe-framing (#1345)
- **TASK-1079** — TUI drive-gate probe reads stderr but CLI emits --json errors on stdout (#1315)
- **TASK-1047** — Document the fan-out/subagent burndown drain as Claude-only (#1306)
- **TASK-1077** — Misplaced doc comment: print_focus_rollup prose now documents struct FocusTally (PR-1291) (#1304)
- **TASK-976** — Outcome-only vocabulary firewall in advisor discipline doc (#1296)
- **TASK-1070** — What's-new distillation: a human-friendly guide to the capabilities the self-improvement loop shipped, organized by user benefit (#1281)
- **TASK-1006** — Autopilot slice 1: autonomy-doc section 8 + orthogonal-axes framing (the shared mental model) (#1222)
- **TASK-1005** — SPIKE-70: bless existing drain_batch as --sequential + route coupled work in the burndown docs (#1220)
- **TASK-986** — Re-frame MCP positioning: typed/structural option, CLI is primary (benchmark-backed) (#1206)
- **STORY-714** — Worktree warm-pool: return-to-pool reset instead of destroy-and-recreate (#1181)
- **TASK-935** — Second usage-grounded streamlining pass (W2) (#1168)
- **TASK-848** — Refresh the dated market-landscape snapshot feeding P8a (immutable, dated) (#1160)
- **TASK-0438** — Design reliable fasttrack intake-to-implementation lane (#1124)
- **TASK-875** — Spec Kit composition-seam demo: scaffold there, graph + trace + lifecycle here (#1103)
- **TASK-0427** — Define migration decision matrix for Claude Code versus Codex CLI versus AIDA substrate (#1099)
- **TASK-0422** — Design Codex replacement for Claude defer and session-communication flows (#1098)
- **TASK-0423** — Evaluate Codex-compatible outer sandbox strategy for AIDA launches (#1098)
- **TASK-0425** — Create end-to-end Codex migration validation runbook (#1093)
- **TASK-0421** — Inventory Claude-specific workflow surfaces that need Codex replacements (#1092)
- **TASK-0420** — Review Claude-to-Codex migration guidance for accuracy and tone (#1086)
- **TASK-886** — docs/getting-started.md: 'aida comment add --content' flag does not exist (positional only); minor walkthrough drifts (#1070)
- **TASK-885** — OVERVIEW.md drift: wrong command names (goal-helper, db rel-def), stale skill count (22), incomplete type list (12/19), aida-tui crate omitted from workspace layout (#1069)
- **TASK-874** — aida compete regression-catch rate at n>1 (productize the QA framing) (#1060)
- **TASK-870** — Cross-vendor portability test: time-to-productive for a new vendor agent, zero bespoke integration (#1055, #1059)
- **TASK-871** — Roll-your-own friction benchmark: markdown+git+grep vs AIDA at fleet scale (#1058, #1059)
- **TASK-873** — Lifecycle-authority gate demonstration vs the fields merge-time gates (#1054)
- **TASK-867** — Document the bwrap os_wrap knobs in docs/environment-variables.md + a real config reference; reconcile the untracked bubblewrap doc (#1039)
- **EPIC-48** — Coordinating Multi-Vendor AI Agent Fleets (research project) (#1024, #1025)
- **TASK-847** — Develop the theory paper related-work (section 14) into real citations (#1019)
- **TASK-846** — Red-team the remaining load-bearing probe findings (P1, P2, P4, P5) (#1017)
- **EPIC-49** — AIDA streamlined + niche-fit: pare the surface, sharpen the wedge, make the niche legible (#1003)
- **TASK-855** — Weave the run findings into the theory paper (P1 scoping, substrate-drives-convergence, competition=QA, attention-distance) (#999)
- **STORY-597** — CLI manual: drift-guard — completeness gate (every help-all command has a manual entry; flag stale entries) (#989)
- **STORY-601** — CLI manual: keep entries machine-digestible so aida-tutor can generate tutorials from them (#989)
- **TASK-787** — Finalize + merge the CLI manual (cli-manual branch): catch up to current CLI, advisor-review the 8 drafts, wire verify-manual.py into CI (#989)
- **TASK-819** — Doc drift: CLAUDE.md says aida history --spec <id>, actual flag is --id (#914)
- **TASK-820** — Land the EPIC-35 gh-to-glab forge wiring-gap inventory (2026-06-14 GitLab Tier-2 audit) (#913)
- **TASK-816** — Capture cross-vendor mailbox demo (Claude/Codex/Antigravity) as dated positioning evidence (#909)
- **TASK-815** — Solo-mode MVP: docs/solo-mode.md runbook composing intake --apply --then-drain → queue integrate --watch (lock + keystone-park + mailbox escalation) (#905)
- **TASK-796** — Doc-intent part 4: drift-guard asserts surface-changing specs' interface_changes are REFLECTED in the docs (#897)
- **TASK-795** — Doc-intent part 3: each manual entry backlinks to its shaping spec(s) via a machine-readable convention (#896)
- **TASK-794** — Doc-intent part 2: documenter consumes the spec graph (--help + interface_changes + description/acceptance) into intent-bearing entries (#894)
- **TASK-789** — CLI manual catch-up: document commands added since the overnight draft (undefer, mailbox retract/delete, status --activity, queue advance, …) (#873)
- **TASK-797** — Doc: clarify [contained] allowed_hosts = [] means 'no restriction' not 'deny all' (#863)
- **STORY-607** — The AIDA Book: unified user-facing mdBook including the CLI manual as a section (#861)
- **STORY-600** — aida manual <command> (a.k.a. --doc): print the CLI-manual rationale section for a command inline (#860)
- **STORY-603** — Doc-intent protocol: surface-changing specs populate interface_changes; the documenter consumes spec INTENT (not just --help); docs trace back to specs (#858)
- **STORY-609** — Split + beautify the lifecycle diagram: ~5 focused diagrams + a styling standard (hero SVG vs themed mermaid) (#857)
- **STORY-608** — Glossary chapter: surface aida docs glossary (generated) as a book page (#856)
- **STORY-606** — Docs library index: a categorized map of every doc by audience (the missing top-level orientation) (#854)
- **EPIC-40** — Comprehensive CLI reference manual: rationale (when/why/when-not) for every command + a lifecycle narrative spine, drift-guarded (#853)
- **TASK-786** — docs: consolidated environment-variables reference (docs/environment-variables.md) — all AIDA_* vars, one chapter (#835)
- **TASK-736** — When any slice verb ships, update the parent skill to *call* it (no re-impl) (#826)
- **TASK-780** — aida questions subcommands: make WHO-runs-it / WHEN explicit in each command's help (#824)
- **EPIC-46** — Multi-user / multi-clone test coverage
- **EPIC-47** — Team-of-users support
- **EPIC-52** — aida tui redesign: clean Claude-Code-like fuzzy-command front door (replaces the dashboard wall)
- **EPIC-53** — TUI flow cockpit: surface what is stuck, why, and who must act — move work forward
- **SPIKE-62** — Multi-repo → one .aida-store: repo/component dimension + multi-repo-aware git scanning (design)
- **SPIKE-64** — Cross-vendor competitive implementation + synthesis (Claude vs Codex headless; Antigravity human-arm) — L5 probe experiment
- **SPIKE-65** — AIDA on Beads: storage abstraction layer — can AIDA compose on top of Beads instead of owning the store?
- **SPIKE-77** — Adopt cmux's validated UX signals: attention badges in aida tui + burndown fan-out into WezTerm panes
- **STORY-522** — Async structured-decision protocol: escalations become batched question+choices the human answers OUTSIDE any agent; the loop resumes from the answer
- **TASK-1091** — Add a documented commit-early/push-early policy to vendor pickup docs
- **TASK-1132** — Fold two recovery-playbook symptoms into aida-doctor skill (main-on-feature-branch, lease-misregistered-at-parent)
- **TASK-1186** — Create a cold-reader positioning path for agent migration audiences
- **TASK-1187** — Align public README/overview AIDA pitch with migration positioning

### Infrastructure

- **TASK-1204** — CI: compile the workspace once — share the release build with the test step instead of a second dev-profile compile (#1743)
- **TASK-1121** — paper-pdf.sh: repeatable pandoc/xelatex PDF build for research docs (#1505)
- **TASK-1154** — Add make build-fast (CARGO_INCREMENTAL=1) for ~43% faster iteration rebuilds (#1487)
- **TASK-1085** — Swap protoc → protox: drop the external protoc build dependency (#1332)
- **STORY-743** — Behavioral journey self-test: drive the real binary through the human + agent core loops end-to-end so shipped behavior can't silently regress (#1290)
- **TASK-984** — Catch raw-glyph-literal lint locally/earlier, not just at CI (recurring agent friction) (#1214)
- **TASK-903** — Pre-commit gate: block bare SPEC-IDs in cli.rs /// doc comments (the recurring --help-leak trap) (#1163)
- **TASK-835** — Phase 3: migrate remaining raw glyph literals to the registry + CI lint blocking new ones (#948)
- **TASK-825** — Add .gitlab-ci.yml mirroring the GitHub CI gates (Build + fmt --check + clippy correctness + mcp stdio suite) (#919)
- **TASK-1206** — CI trace gate: fetch aida-store into a remote-tracking ref (a prior step leaves the branch checked out in .aida-store), and fail hard when the store cannot be loaded
- **TASK-1213** — Delete PROMPT_HISTORY.md and add a CI guard so no agent can recreate it (it duplicates the substrate and conflicts on every parallel rebase)

### Internal

- **TASK-1185** — findings count fails when cache table is absent (#1624)
- **TASK-1174** — eval-channel wrapper: add zsh coverage (TASK-1171 tests drive bash only) (#1597)
- **TASK-148** — drain_lock lifetime test flaky: sibling test leaks AIDA_DRAIN_BORROW/FORCE process-wide (#1555)
- **TASK-147** — Inline fallback pre-commit hook body in hooks.rs has drifted from master template (#1553)
- **STORY-771** — SPIKE-78 follow-up: extract the keystone command handlers (pr/mcp/orchestrator/drain/queue/status/solo/zen) out of main.rs (#1549)
- **TASK-963** — STORY-621 Slice 2: Forge::change_metadata + reviewer-preflight/reconcile metadata-read consolidation (#1547)
- **STORY-772** — SPIKE-78 follow-up: split aida-cli into a thin binary + handler lib crate + test crate (#1524)
- **TASK-1151** — Extract handle_git_backend_command (5.4K lines) out of main.rs into git_backend_cmd.rs (#1484)
- **STORY-770** — SPIKE-78 follow-up: relocate the 1,524 inline tests out of aida-cli/src/main.rs (#1483)
- **SPIKE-78** — Shrink aida-cli's 157k-line main.rs: measure build cost, then extract tests + handler crates incrementally (#1421, #1422, #1423, #1425, #1426, #1427, #1428, #1429, #1430, #1431, #1432, #1433, #1434, #1435, #1436, #1437, #1438, #1439, #1440, #1441, #1442, #1443, #1444, #1445, #1446, #1447, #1448, #1449, #1450, #1451, #1452, #1453, #1454, #1455, #1456, #1457, #1458, #1459, #1460, #1461, #1462, #1463, #1464, #1465, #1466, #1467, #1468, #1469, #1470, #1471, #1472, #1473, #1474, #1475, #1476, #1477, #1478, #1479, #1480, #1481, #1482)
- **ADR-10** — ADR-7 enforcement: autonomy mode becomes a uniform first-class typed engine parameter (--zen no longer an env-only side-channel) (#1413)
- **TASK-1060** — Make --zen a typed engine parameter (kill the AIDA_ZEN env-var side-channel) — fully realize ADR-7 uniform autonomy parameterization (#1336)
- **ADR-9** — ADR-7 enforcement: CI guardrail asserts per-spec drivers route through the one engine; burndown fan-out is the single allow-listed exception (#1333)
- **TASK-1065** — status --full remaining floor: cache-back the hygiene/cleanup doctor scans + the single still-needed backend.load() (deferred from TASK-1061 to avoid output drift) (#1324)
- **TASK-1072** — aida ps is ~1.3s standalone (the /proc probe + session-lease scan) — profile and speed up the running-work table (#1289)
- **TASK-1061** — status --full non-git floor: cut the ~5s CPU + ~6s blocked (live-session /proc probe + backend.load()) now-dominant after the git fan-out fix (#1270)
- **TASK-1056** — status --full real bottleneck: ~32k git subprocesses fanning over worktrees x branches (not the gh probes) (#1262)
- **TASK-1055** — Efficiency quick-wins: parallel status --full probes, path-scoped history --id, --slower-than ms suffix (#1255)
- **TASK-1033** — AIDA store should gc periodically — heavy drives bloat loose objects below gc.auto threshold and tax every targeted write ~4x (#1224)
- **TASK-960** — Same-host two-clone test harness: automate the untested two-clone flows from the multi-user-test-cases.md catalog (#1174)
- **TASK-956** — Apply the TASK-935 surface cuts: HIDE 6 zero-call parents + MERGE 2 + HIDE 3 subcommands (grep-verified) (#1171)
- **TASK-938** — MU-60x concurrent-same-clone harness case (#1167)
- **TASK-854** — interview apply smoke test (#1166)
- **STORY-707** — Fast aida status: a sub-second cache snapshot; move heavy diagnostics (PR/CI/liveness/hygiene) to aida doctor (#1152)
- **TASK-926** — (Followup B) `lane_status_maps_needsattention_to_blocked_or_punted` (#1148)
- **TASK-902** — Cockpit paint floor: aida list --blocked does a full backend.load (~1.4s) — cache-project the blocked flag (#1118)
- **STORY-671** — Central policy registry: config knobs self-register so config show/menu/edit + anti-drift test derive from one source (BUG-533 slice 2) (#1097)
- **TASK-889** — Stress-test + lock down distributed ID-allocation uniqueness across concurrent nodes (#1076)
- **STORY-655** — Run the gate-vs-rule ablation (P1: does substrate-as-bouncer beat a stated rule?) (#996, #1005, #1006, #1007, #1008, #1009, #1010, #1011, #1012, #1013, #1021, #1022, #1023)
- **TASK-849** — spec dryrun polish: graft Codex micro-improvements (get_requirement_by_spec_id reuse + &static str dimension names) (#1020)
- **TASK-856** — aida add latency ~8s avg (telemetry) — profile and trim the spec-add write path (#1015)
- **TASK-851** — Streamline surface: collapse presence + identity verbs (away/home → presence; team/whoami under node) (#995)
- **TASK-852** — Streamline surface: hide reporting/power verbs behind parents/--advanced (metrics/load/autonomy/lifecycle/solo/stack/manual; intent→why --intent) (#995)
- **TASK-850** — Streamline surface: hide deprecated `intake` alias + cut 5 zero-call verbs (#993)
- **STORY-642** — Multi-host harness cases: prove cross-host TTL reclaim + foreign-host claim honored (Phase 2) (#972)
- **STORY-636** — Same-host multi-clone test harness (scripts/multi-clone-harness.sh) (#966)
- **TASK-840** — EPIC-45 glyph long-tail: migrate the 18 allow-listed files to the registry, tightening the lint per file (#958, #959, #960)
- **TASK-803** — Extract resolve_burndown_sets per-spec classification into a pure, unit-testable helper (#884)
- **TASK-1-127** — aida add exited 1 five times on 2026-09-06 in under 50ms; cause unverified
- **TASK-1020** — TASK-0432 — precedence when autopilot composes with `--zen` / `--no-human` / solo / `intake --apply`; ratify the surface (flag vs posture vs mode)
- **TASK-1153** — Remove 13 dead cli/aida-core imports from main.rs left by SPIKE-78 extractions

### Other

- Revert "chore: scaffold AIDA"
- STORY-772 PR-1: in-package lib boundary — main.rs becomes lib.rs, bin is a 3-line stub (#1523)
- [AI:claude] docs(bench): SPIKE-73 v2 re-run validates BUG-717 fix — MCP status_snapshot 0/3->3/3, cost ~2x holds
- [AI:claude] docs(bench): commit the recovered 72-cell SPIKE-73 agent-surface run
- [AI:claude] docs(cli): strip STORY-769 spec-id refs leaking into the user-facing manual (CLI-manual drift-guard fix)
- [AI:claude] docs(competitive): gnhf vs AIDA autonomy-layer analysis (#1233)
- [AI:claude] docs(competitive): living marketplace roster — substrate + orchestration projects we track
- [AI:claude] docs(history): log session 65 — keystone reliability + solo-mode foundations (#906)
- [AI:claude] docs(positioning): refresh all vs-* comparisons for latest AIDA + marketplace state
- [AI:claude] docs(presence): correct stale "READ-ONLY, no consumers" module header
- [AI:claude] docs(research): g2 slide — 'The AI Agent Stack' title, L5 'NEXT FRONTIER' chip (was 'white space')
- [AI:claude] docs(research): multi-vendor coordination slide graphics — 5 standalone SVGs
- [AI:claude] docs(research): paper update pass — field-null (SPIKE-67), agent-surface economics (SPIKE-73), fleet economics, contested-L5 marketplace, de-emphasize the artifact
- [AI:claude] docs: SPIKE-61 sandbox-execution competitive analysis + recommendation (#851)
- [AI:claude] docs: archive EPIC-0428 advisor-autopilot design plans (policy-envelope, audit-reversal, product-role, mode-composition) (#1221)
- [AI:claude] docs: archive SPIKE-74 design + agent-fleet-economics research synthesis (#1217)
- [AI:claude] docs: feature aida zen "<thought>" — the thought→merged front door (#1245)
- [AI:claude] docs: fix stale SPIKE-73 bench-report path in CLAUDE.md
- [AI:claude] feat(help): tiered help leads with the aida why magic (Strategy B, STORY-758) (#1348)
- [AI:claude] feat(model): add risk_notes/test_coverage_notes/implementation_summary to Requirement (narrowed ImplementationInfo, EPIC-7) (#1411)
- [AI:claude] test(xplat): gate binary-driving e2e suites to Linux for the BUG-688 nightly gate (#1374)
- docs(agents): add Claude Code → Codex CLI porting guide
- docs(agents): bwrap sandbox — recommend per-binary AppArmor userns profile over the host-wide sysctl (#1077)
- docs(competitive): 2026-08-29 market delta — the champion-product pivot (#1609)
- docs(competitive): AXI ecosystem positioning + dated note — interface-layer neighbor, MCP-vs-CLI challenge
- docs(competitive): SPIKE-58 governance verification — repositioning is partly-true, narrow the claim
- docs(competitive): add lavish-axi to the AXI roster (presentation layer, orthogonal — no action)
- docs(competitive): add manaflow cmux to roster; flag tui-prior-art CMux name collision
- docs(competitive): refresh marketplace roster with June-2026 landscape scan (#988)
- docs(decisions): promote the four subsystem proposals to accepted — store ADRs 17-20 ratified
- docs(decisions): rename proposed subsystem ADR files to PROPOSAL-* slugs — ADR-15/16 already exist as accepted store decisions
- docs(plans): add implementation plan for the config-menu in-place edit
- docs(power-features): add the supervision-spectrum section — queue work vs /aida-solo vs burndown vs zen
- docs(presentation): add OS-sandbox (os_wrap/bwrap) slide with measured overhead to the admin deck (#1046)
- docs(research): add the author's coda to the epilogue — decision to plough on
- docs(research): author's coda in Joe's own words (plough on)
- docs(research): fold Yegge "be there first" + SPIKE-73 reproduction into the paper; add the epilogue
- docs(research): rename coordination paper to 2026-07-08 (living-draft revision date), update all 10 refs
- docs(review): add review-process.md — who reviews by execution mode
- docs(review): correct fork-vs-cold-boot — the in-drain advisor tier forks-from-live when registered (not cold-boot-only)
- docs: PROMPT_HISTORY Session 66 — gate-vs-rule terminus + research QA + product polish (#1026)
- docs: add AIDA_IN_MY_OWN_WORDS — human-authored project reflection
- docs: calibrate ECC + agent-teams moat claims — precise, not absolute
- docs: document the perf + legibility work (aida status/ps, epic-rollup, cache self-heal, fast writes, case-insensitive matching) + EPIC-53 seam + session log
- docs: freeze PROMPT_HISTORY — session history lives in the AIDA substrate
- docs: human-friendly guide to AIDA's autonomy taxonomy (#1235)
- docs: list MCP inter-agent coordination tools (mailbox/briefs/punts) in CLAUDE.md
- docs: profile rule for aida developers — pin release, debug is session-scoped
- docs: what's-new July distillation — worktree-enter, awaiting inbox, zen suitability, self-heal
- docs: what's-new — distill the July throughput-run capabilities (did-you-mean, doc suggest, base-behind, leases --json, session-id comments, owner lens, …)

## [v0.14.0] — 2026-06-12

Specs merged since v0.13.0 (60):

### Features

- **STORY-561** — aida home / aida away: operator-presence state the autonomy ladder keys on (not per-command flags) (#796)
- **STORY-560** — Headless advisor INTAKE pass: an advisor AGENT reads all open specs, proposes approve/reject/queue, and grooms the autonomous-able set (autonomous sibling of STORY-558) (#795)
- **STORY-564** — aida queue work --zen: auto-exit on a clean finish (no human needed), pause only when a human IS needed (#793)
- **STORY-555** — Close the aida questions -> burndown loop: answering a decision APPLIES the resolution + unparks the spec into the ready set (the human-decision complement of aida burndown run) (#792)
- **STORY-554** — aida backlog groom --pickable: auto-select decision-free backlog into the queue via the burndown pickability gate (#791)
- **TASK-721** — burndown/skill: prune merged implementer worktree + branch after auto-merge (#790)
- **TASK-756** — STORY-561 slice: presence PRIMITIVE — aida home/away/status + timestamped file + TTL + TTY auto-flip + statusline (consumers deferred) (#789)
- **TASK-754** — aida add --queue: file + approve + enqueue in one shot (record the advisor's filing-time autonomy judgment, skip backlog limbo) (#787)
- **STORY-565** — Queue 'how do I get to zero?' must be answerable in one glance — disambiguate clear-vs-drain + always show the blocked remainder + the single next action (#786)
- **STORY-566** — aida queue advance: one command that walks each queued item to its next step — drain the autonomous, dispatch the human-required interactively (#785)
- **TASK-752** — aida doctor: detect + heal tracked legacy-store cruft (requirements*.yaml / scaffold-report.html) in a git-canonical project (#784)
- **TASK-753** — Close the status --cleanup ↔ doctor seam: signpost the read→heal path + clarify --cleanup is read-only (#783)
- **TASK-744** — Split overloaded 'needs-human' into 'needs-decision' (answer-to-unblock) vs 'needs-supervised-build' (keyboard build, not a question) (#777, #778)
- **TASK-747** — **Phase 3: `--for human` queue routing.** Accept `human` as a `QueueEntry::for_role` target; union explicitly-routed with derived membership in the view. Establishes `human` as a first-class route target symmetric with agent roles (#774)
- **STORY-563** — aida human unblock: deterministic CLI that emits a paste-ready advisor prompt to groom blocked items into the burndown (#773)
- **TASK-746** — **Phase 2: name the predicate + the `aida human` vector.** Add `burndown::human_required`, `Command::Human`, `handle_human_command` (bare → delegate to `handle_list_human`). The `aida human` front door + canonical predicate (#772)
- **STORY-562** — aida list human (status-alias sibling to open/closed): the discoverable 'what needs me?' view over the human-attention set (#767)
- **TASK-742** — Phase 3: `aida lifecycle --empirical --diff` — reconstruct the observed machine from `history:` arrays (TASK-121) and diff against the declared table (#766)
- **STORY-559** — aida advisor status: expand into the advisor situational dashboard (fold registration in; flag for narrow output) (#764)
- **TASK-737** — Phase 1: `aida lifecycle --diagram` + the committed-vs-generated doc pin (generate-only, zero behavior change) (#763)
- **TASK-743** — aida list --short (--ids-only): print bare spec IDs one per line for pipe/loop consumption (#761)
- **STORY-558** — Advisor burndown-prep: one guided pass from drafts → approved → queued (sign-off preserved, NOT auto-approve) (#760)
- **STORY-556** — aida help: group commands by function + lead with the novice daily-drivers (tier the full surface) (#758)
- **STORY-557** — /aida-clarify <spec>: advisor interrogates the human to author acceptance criteria for an under-specified spec (agentic complement to questions sweep) (#757)
- **STORY-553** — aida review <spec>: single verb to drive human review of a held spec (dual of queue work) (#752)
- **STORY-552** — aida init: offer to 'git init' when not in a git repo (complete the onboarding funnel, don't just bail) (#750)
- **STORY-64** — aida role current: print the active role's name (#749)
- **TASK-723** — burndown explain: findings-link + residual-note (prefixed comment) + multi-reason — STORY-548 safe slice (#746)

### Fixes

- **BUG-503** — aida why <spec> errors 'not in the open set' for human-only/parked specs — refuses the items most needing explanation (#794)
- **BUG-504** — aida queue list shows ARCHIVED specs (SPIKE-53 lingers in the queue after archive) — archive should prune queue entries / queue view should hide archived (#794)
- **BUG-502** — aida human unblock misclassifies review:draft-only specs as QUEUE/CLARIFY instead of a REVIEW bucket (#782)
- **BUG-500** — aida queue work --zen: the 'stop' path should run 'aida session end' (clean up worktree+lease), not just pause (#781)
- **BUG-501** — MCP 'setup issue' warning recurs in every AIDA worktree — pre-approve the aida server in scaffolded settings (extend BUG-484 to worktrees) (#780)
- **BUG-499** — burndown plan 'awaiting sign-off' hint should clarify that 'aida queue add' IS the advisor sign-off (role-gated), not a generic human action (#779)
- **BUG-496** — Parallel burndown can merge two PRs that each pass CI alone but break main together — no integrated-main re-verify (#769)
- **BUG-498** — Operator acting as advisor still seated as 'implementer' by default — statusline misleads; suggest 'role enter advisor' on advisor-style activity (#765)
- **BUG-497** — aida archive --older-than: silent multi-second window for large sweeps — the BUG-425 'one fast commit' premise breaks at scale (#762)
- **BUG-486** — MCP status gate is role-blind: refuses advisor-seat approve/plan despite AIDA_SESSION_ROLE=advisor (CLI↔MCP authority inconsistency) (#759)
- **BUG-495** — aida questions sweep output refinements: skip non-implementable/built specs + clarify the misleading '--apply to write' message (#756)
- **BUG-492** — aida archive has no guard against archiving non-terminal/queued specs — agent loop archived 128 Approved + active queued work (#751)
- **BUG-494** — aida burndown plan footer falsely claims 'There is no aida burndown run' — but the run subcommand exists (STORY-545) (#748)
- **BUG-493** — aida why over-claims 'held as a draft PR' from the review:draft-only TAG when the actual draft PR is closed/absent (#747)
- **BUG-418** — aida db reconcile-status: real run printed 'No eligible flips' yet the spec ended Completed (misleading output or state confusion) (#745)
- **BUG-59** — Stop hook ENOENT when cwd was a removed worktree (#743)

### Documentation

- **FR-173** — describe default requirement types (#788)
- **SPIKE-57** — SPIKE: formalize 'human' as a first-class role — the escalation terminus — with an 'aida human' vector (classification, NOT assignment) (#768)
- **SPIKE-56** — SPIKE: aida lifecycle — one declared spec-state transition model that generates the diagram, enforces the guards, and diffs declared-vs-empirical from history (#755)
- **SPIKE-55** — SPIKE: audit skills for missing deterministic-CLI slices + formalize the slice/launcher/pure-agentic convention (NOT 1:1 CLI-clone every skill) (#754)
- **TASK-733** — docs/lifecycle.md: Mermaid spec-lifecycle state diagram — orthogonal regions (status × archived × queued × lease × park) + 3 trigger-colors (CLI / LLM / git-event) (#753)

### Internal

- **TASK-740** — Phase 2c: migrate the merge auto-bump eligibility (`auto_bump_eligible_status`) behind the model's GitEvent guards (#776)
- **TASK-741** — Phase 2d: express the BUG-492 / BUG-493 cross-axis invariants as `INVARIANTS` rows consulted by the archive + held-for-review paths (#775)
- **TASK-739** — Phase 2b: migrate the advisor-authority gate (`status_requires_advisor_authority`) behind `validate_transition` (#771)
- **TASK-738** — Phase 2a: introduce `aida-core/src/lifecycle.rs` with `LIFECYCLE` + `validate_transition`, migrate `forbidden_attention_transition` behind it with a parity test (#770)
- **TASK-500** — BUG-360 follow-up: refactor queue-done gate to pure queue_done_precheck_diagnose function for isolated testability (#744)

### Other

- [AI:claude] docs(mailbox): user-guide chapter for the inter-agent mailbox (#692)
- [AI:claude] fix(cli): pass in_flight_scopes to question_sweep_candidate — unbreak main
- docs(competitive): Beads + Gas Town vs AIDA snapshot, SPIKE-53 research
- docs(competitive): weekly scan Lane B — agent orchestration & swarm frontier (2026-06-09) (#717)
- docs(lifecycle): add status-vs-pickability section + plain glossary
- docs: commit ECC competitive-analysis review + STORY-523 triage-sweep prototype (curated WIP) (#640)

## [v0.13.0] — 2026-06-10

Specs merged since v0.12.0 (99):

### Features

- **TASK-732** — Init greeting: teach the code-to-spec link bridge between 'aida list' and 'aida done' (#742)
- **TASK-730** — aida list: when finished work clutters the default view, point to 'aida list open' (what's left) (#737)
- **TASK-728** — Novice-first: aida add type-picker leads with relatable types; init greeting shows the full loop (incl. aida done) (#731)
- **TASK-726** — Novice-first: aida show tells you HOW to link code when there's no linkage yet (#729)
- **TASK-725** — Novice-first: declutter the aida init greeting (the loop + role, jargon to --verbose) (#728)
- **TASK-0417** — Add optional EARS-style requirement linting (#725)
- **TASK-0418** — Add context-grounded pre-plan scan for spec imports and generation (#724)
- **STORY-545** — aida burndown run: headless PARALLEL overnight drain over the gated ready set (#718)
- **STORY-547** — aida burndown plan: derived 'why still open' reason for EVERY open spec (+ aida why <ID>) (#715)
- **STORY-542** — Capture interface_changes at spec close (cli/mcp/tui) → the deterministic source for the operator digest (#713)
- **TASK-0415** — Add list status shortcuts and open/closed aliases (#712)
- **STORY-528** — Agent registry paused-availability state + brief-time budget warning (#711)
- **TASK-714** — aida schema: full per-object field detail (Finding/Brief/Punt/Directive/Lease/QueueItem) (#710)
- **STORY-544** — burndown plan: next-step footer + plain-language selector (new-user dead-end) (#708)
- **TASK-0414** — Add opt-in AIDA-aware statusline bootstrap path (#706)
- **FR-98** — Drag a requirement to make child (#703)
- **STORY-539** — Mailbox UX: operator overview (list + inbox --all), originator/time display, light urgency flag + surfacing (#701)
- **STORY-537** — aida remote create: guided origin bootstrap (gh / GitLab push-to-create / glab / attach-existing) when a project has no remote (#700)
- **STORY-541** — aida digest --audience operator (capabilities lens): new USER-FACING CLI/UX changes + how to try them, since a window (#699)
- **TASK-717** — aida doctor: verify-and-prune stale REMOTE branches (doctor --heal is local-only) (#698)
- **TASK-713** — Role picker should stay scannable in narrow terminals (#695)
- **STORY-538** — aida schema: introspect the storable substrate (object catalog + per-object field/enum detail) (#691)
- **TASK-707** — File the active-loop specs (`aida-integrate` skill, `aida queue integrate`) (#684)
- **STORY-531** — EPIC-36 Tier-2: threshold-triggered LLM health fact-finding (extend /aida-insights) (#683)
- **STORY-536** — EPIC-27: remaining MCP tools (db/cache/plan/goal/ultraplan/status/usage) (#682)
- **STORY-535** — EPIC-27: MCP resources for live state (queue/in-flight, session/leases, pr/N, batch/N) (#680)
- **STORY-534** — EPIC-27: role MCP tools (role_enter/end/show/list) (#679)
- **STORY-460** — Integrator role — delegate merge-cascade conflict resolution off the advisor (#664, #677)
- **STORY-533** — EPIC-27: session MCP tools (session_start/end/leases/status/manifest) (#676)
- **TASK-705** — burndown: skip specs that already have an open PR / in-flight work (#675)
- **STORY-493** — P3 hybrid inter-agent mailbox: fast .aida/ local layer + git-canonical durable digest (#674)
- **STORY-530** — EPIC-36 Tier-1: remaining deterministic health metrics catalog (#673)
- **STORY-532** — EPIC-27: queue MCP tools (queue_list/add/work/done/next/progress/rework/move/remove) (#672)
- **TASK-706** — EPIC-36 MVP: session-vs-drain misclassification-gap metric (deterministic) (#671)
- **STORY-527** — /aida-burndown skill: selector → ready/bounded fan-out → integrate (punt-and-continue) — the encoded autonomous-drain path (#666, #668, #669)
- **TASK-703** — STORY-527 slice 1: aida burndown plan CLI (selector + pickability gate) (#668)
- **TASK-699** — aida doctor external-import-bleed: opt-in heal (remove stray ancestor instruction files) (#667)
- **TASK-702** — Wire SubagentStart/Stop -> AIDA lease register/release (TASK-694 passive-observe, gated on the probe) (#663)
- **STORY-529** — Enforce draft-for-review for handed-off agents — briefed 'draft PR' isn't honored (3x self-merge) (#662)
- **TASK-697** — ultraplan suggested-threshold should key on spec thinness, not acceptance-bullet count (#661)
- **TASK-698** — aida init: prompt for agent permission posture + populate ~/.aida/agents.toml (surface the STORY-495 bypass knob) (#660)
- **TASK-696** — aida doctor: flag an ancestor CLAUDE.md whose @-imports resolve outside the project (external-import bleed) (#657)
- **STORY-523** — Decision-question PRODUCER: sweep the scoped backlog, detect human-decision-needed specs, formulate + attach DecisionRequests (feeds aida questions) (#655)
- **TASK-686** — aida init: guard against scaffolding into a parent-of-projects directory (CLAUDE.md bleeds into all children) (#652)
- **TASK-691** — STORY-335 follow-up: .aida/config.toml [integrate] strategy project default (flag overrides) (#650)
- **TASK-562** — scripts/release.sh ecosystem-watch prompt: auto-decide by age threshold + actionable refresh hint (#649)
- **TASK-634** — SPIKE-41 slice: WorktreeCreate/WorktreeRemove hooks → register/release an AIDA lease (substrate-capture for harness-orchestrated worktrees) (#648)
- **TASK-693** — STORY-472 follow-up: aida release --after-pr <N> (watch PR CI + merge before releasing) (#647)
- **TASK-692** — STORY-472 slice 1: top-level 'aida release' verb (--patch/--minor/--major, --check, --skip-xplat-check) (#644)
- **TASK-690** — STORY-335 slice 3: --strategy {per-item|one-branch|stacked} selector on aida queue integrate (#643)
- **TASK-689** — STORY-335 slice 2: rebase step in aida queue integrate (--rebase, opt-in) (#642)
- **TASK-688** — STORY-335 slice 1: read-only rebase-conflict forecast in aida queue integrate --dry-run (#641)
- **STORY-546** — burndown gate requires QUEUED (advisor sign-off): set = approved + queued + pickable

### Fixes

- **BUG-491** — Windows cross-platform CI red: bug483 lease test hand-rolls TOML with unescaped backslash worktree_path (#741)
- **TASK-731** — Skill-command audit: 'aida list --format X' + 'aida feature list' + 'requirements.yaml' drift across docs/sprint/release skills (#740)
- **BUG-490** — /aida-req skill: broken context commands (--format brief, feature list) + legacy requirements.yaml/aida-desktop refs (#739)
- **BUG-489** — /aida-onboard skill uses 'aida list --format summary' (doesn't exist) → falsely reports 'No AIDA database found' (#738)
- **BUG-488** — aida search leaks seeded META AI-prompt specs; aida list hides them — inconsistent (#733, #734)
- **BUG-487** — Commit hook nags a hand-coding novice to add [AI:tool] for a plain trace: comment (no ai: marker) (#730)
- **BUG-485** — Cache FTS schema drift: 'requirements_fts has no column named external_refs' on db sync, not auto-rebuilt (#707)
- **BUG-484** — Fresh aida init project shows confusing '⚠ 1 setup issue: MCP' on every Claude launch — scaffolded .mcp.json server is never pre-approved (#702)
- **TASK-712** — Quality-sweep: batch of low-severity robustness findings (7) (#697)
- **BUG-477** — Merge-driven done→completed auto-bump + reconcile write NO history entry (history-as-source-of-truth contract broken) (#694)
- **BUG-483** — aida session end force-removes a worktree without checking other leases/agents share it (#693)
- **BUG-480** — MCP tool_queue_add bypasses the advisor-authority gate the CLI enforces (TASK-647) — MCP agent can self-queue work for execution (#690)
- **BUG-481** — MCP update_requirement lets a non-advisor self-advance Draft→InProgress/Done (status gate checks only target, not source) (#690)
- **BUG-482** — CLI 'aida edit' advisor gate misses NeedsAttention→Approved (only Draft source is gated) (#690)
- **BUG-478** — Failed --resume-drain is misclassified as 'deliberately shelved' → cannot be re-resumed (BUG-438 defeated) (#689)
- **BUG-479** — Implementer that commits then exits non-zero is treated as 'no work happened' → status restored, lease/worktree/commits stranded (#689)
- **BUG-474** — Concurrent 'aida add' can dispense duplicate agreed-IDs (BlockRegistry load→dispense→save has no lock) (#688)
- **BUG-254** — aida pull / orchestrator phase 5 reports 'phase 5 complete' when code-leg git pull failed (silent failure) (#687)
- **BUG-476** — aida pull exits 0 over a stale code tree when code leg fails + store has no worktree/origin (BUG-254 contract bypassed) (#687)
- **BUG-475** — conflict.rs truncate() panics on non-ASCII descriptions (byte-slice after byte-length check) (#686)
- **BUG-473** — 3 clippy correctness errors on main (dead boolean guard + 2 loops-never-loop), hidden by CI's -W mode (#678)
- **TASK-704** — burndown gate: park needs-design-signoff + operator-action tags (found dogfooding STORY-527) (#670)
- **TASK-700** — aida questions sweep: add --dry-run (default preview) before it writes DecisionRequests (#658)
- **BUG-471** — aida doctor --heal aborts on first error instead of continuing (idempotency: 'lease X no longer exists') (#646)
- **BUG-472** — aida status 'Awaiting you' findings count includes non-draft specs (overcounts vs aida findings list) (#645)

### Documentation

- **TASK-729** — Align getting-started.md to the new simple flow (aida add "X" / aida done / TASK ids) (#732, #735)
- **CR-1-116** — Reposition the 'missing index' headline (intent/lifecycle) — operator decision (#723)
- **STORY-551** — Positioning: reclaim 'intent/spec traceability + lifecycle truth' — 'missing index' collides with free auto-code-graph tools (#723)
- **STORY-107** — Positioning + ecosystem comparison doc: 'why AIDA, how it fits' kept current as the AI/dev-tools landscape evolves (#720)
- **TASK-722** — Seed positioning decision-aids: when-not-to-use-aida + composition (+ index→lifecycle recommendation) (#720)
- **TASK-720** — Positioning doc: AIDA vs Claude Code Agent Teams (vs-agent-teams.md) (#719)
- **STORY-540** — Single-writer scope-lane (docs lane): scope-owned agent fed by needs-docs flag + periodic sweep — SPIKE-10 MVP (#705)
- **TASK-0413** — Document and scaffold Codex status-line support (#704)
- **TASK-716** — Doc-sync: CLAUDE.md lists 13 requirement types but the model has 19 (change-request/principle/vision/constraint/decision/term undocumented) (#696)
- **STORY-475** — Remote/auth-capable AIDA MCP transport (#665)
- **SPIKE-13** — SPIKE: multi-agent budget-aware dispatching surface (per-agent headroom in aida status; brief-time budget check) (#659)
- **SPIKE-8** — SPIKE: /ultraplan output quality — local vs web vs no-plan empirical comparison (#654)
- **TASK-695** — STORY-527 slice 1: autonomous-burndown discipline-pack doc (companion to /aida-burndown) (#653)
- **STORY-549** — Weekly multi-agent competitive deep-dive ritual (brief-driven) → adversarial deep-research via ultraplan

### Infrastructure

- **TASK-710** — Finish the clippy -D warnings cleanup (clear ~255 style/pedantic warnings, then flip CI) (#685)
- **TASK-709** — CI gate: deny clippy correctness lints so logic bugs can't merge (the substrate bouncer for BUG-473's class) (#681)
- **TASK-687** — Surface nightly cross-platform failures sooner (it was red ~3 days unnoticed, blocked v0.12.0) (#639)

### Other

- [AI:claude] docs: front-door discoverability — nearest competitors, decision-aids, docs/ index (#721)
- docs(history): close Session 63 novice-first loop (R10-15, TASK-731 + honest stop)
- docs(history): session 62 — autonomy keystone validated + newcomer-adoption docs push (#722)
- docs(history): session 63 — backlog reset + the novice-first friction loop (#736)

## [v0.12.0] — 2026-06-07

Specs merged since v0.11.0 (198):

### Features

- **STORY-519** — Parallel plan-only fan-out: speculative DESIGN pre-work over the partitioned approved backlog (durable refs, no merge contention) (#633)
- **STORY-520** — Integrator role + producer/consumer drain split: parallel implementers produce PRs, a single integrator serializes phases 2-6 (consumes the Done→open-PR signal) (#632)
- **STORY-265** — Decoupled plan + implement phases: --plan-only / --implement / --with-plan modes (formalize Approved → Planned → InProgress) (#630)
- **STORY-363** — aida advisor handoff --to <project> --focus <topic> CLI command (SPIKE-10 Track A) (#629)
- **STORY-452** — Remote-aware agent registry: cloud /ultraplan sessions + cross-machine sessions visible in aida status (re-filed after STORY-448 ID-allocation race) (#628)
- **STORY-477** — Agent-lift metrics report for AIDA dogfood proof (#627)
- **TASK-255** — TUI mission-control empty state: queue + leases + PRs + suggested actions, not just keybindings (#626)
- **STORY-362** — Subsystem-tagged memory pack + --focus loading (SPIKE-10 Track A) (#625)
- **STORY-476** — External issue refs for Linear/Jira/GitHub composition (#623)
- **TASK-256** — TUI theming: Catppuccin (mocha/macchiato/frappe/latte) + Dark + Light + Nord palettes (#622)
- **TASK-516** — aida import-plan --request-review: master-review handshake before plan is treated as canonical (#621)
- **STORY-262** — Advisor scheduled tasks: recurring maintenance/research lands in queue on cadence (refresh-comp-analysis, audit-spec-graph, etc.) (#619)
- **STORY-499** — Diff-level trace-coverage check in CI (gated on the coverage-definition spike) (#618)
- **STORY-384** — aida queue recover <id> — interactive wizard for failed-phase-1 spec recovery (#617)
- **TASK-298** — Headless orchestrator watchdog: parse stream-json for permission_denials; detect 'is_error' false-positive (#616)
- **TASK-405** — aida queue work: support PR-only invocation (drive phases 3-6 when implementation shipped outside the orchestrator) (#615)
- **TASK-630** — aida queue work --resume accepts a deliberate-hold spec (BUG-250 criterion 5) (#614)
- **STORY-522** — Async structured-decision protocol: escalations become batched question+choices the human answers OUTSIDE any agent; the loop resumes from the answer (#611)
- **STORY-511** — [EPIC-35 s5] End-to-end GitLab drain + MR linkage in aida show + docs + init UX (#608)
- **STORY-469** — Agent invocation structural guards: spec-ID validation, mandatory substrate-sketch-before-implement, local-vs-substrate reality check (#607)
- **TASK-99** — aida queue work: also pull code branch (not just orphan store) before creating worktree (#605)
- **STORY-416** — Cross-agent scaffolding: maximize universal gates, scaffold per-agent docs into new projects (#601)
- **STORY-49** — EPIC-21 v2: aida store checkout <code-sha> time-travels orphan worktree (#600)
- **STORY-510** — [EPIC-35 s4] GitLab CI — ci_status via glab pipelines, wired into the drain CI-wait (#598)
- **TASK-340** — Autonomy metric — record autonomous-time vs waiting-for-human + intervention count per drain, to track maturity (#595)
- **TASK-457** — aida init: scaffold .antigravity/ directory + integration files (mirrors .codex/ pattern) (#594)
- **STORY-474** — MCP tool profiles and safe default surface (#593)
- **STORY-509** — [EPIC-35 s3] GitLab provider — open_change/status/merge/comment/list via glab (+ REST fallback) (#592)
- **STORY-399** — MCP tools: emit structuredContent matching declared outputSchema (Path B follow-up to TASK-440) (#591)
- **TASK-305** — Plan-archival reconciliation: extract web /ultraplan plan from PR back to docs/plans/ (#590)
- **TASK-631** — aida init: commit the scaffolding itself — auto when non-interactive, prompt (default-Y) when TTY — scoped to init's own paths (#587)
- **STORY-82** — Modernize the 7 existing MCP tools: bring schemas up to current CLI vocabulary (#586)
- **SPIKE-48** — Isolated sandbox store for play/test scenarios (first-user learning + dev drain-testing) (#585)
- **STORY-127** — Workflow state-aware guidance: AIDA always answers 'where am I / what should I do?' without user mental-model overhead (#584)
- **STORY-410** — Existing-project substrate-drift discovery: surface when an existing project's scaffolding-pack is behind master (#583)
- **TASK-618** — Distributed queue: same user_id on two machines edits one registry/queues/<user>.yaml → orphan-branch conflict (#582)
- **TASK-680** — Release-time doc-coverage gate: flag specs completed since last tag that lack an aida doc entry (#580)
- **TASK-480** — /aida-review: detect intermediate-only diffs (build artifacts, generated code) and refuse (#579)
- **STORY-456** — aida status: surface worktrees + open PRs in unified view (STORY-385 follow-up) (#577)
- **FR-265** — aida node acquire --remote-only for retroactive backfill (#575)
- **STORY-498** — CI spec-id validity gate: reject a PR whose commits reference a SPEC-ID that isn't a live spec (#574)
- **TASK-130** — /aida-pickup has no SPIKE-aware path. When queue head is a Spike, the implement… (#573)
- **STORY-447** — aida deps sweep — infer likely dependencies from trace graph + file overlap (#571)
- **TASK-578** — aida queue work --drain: convenience alias for --auto-complete --no-human=both --max N (#570)
- **TASK-662** — TASK-539 follow-up: findings JSON emission + delta-since-last-run in aida status (#569)
- **STORY-401** — MCP server: stable error shapes — isError:true + structured {code, message, tool, recoverable} (#568)
- **TASK-670** — aida list: leading work-routing glyph (queued ↑ / in-flight ▶ / blocked ⊘) — surface the axis status doesn't carry (#567)
- **TASK-579** — aida findings promote: warn / auto-bump when origin-ID already shipped (#566)
- **TASK-661** — Disposition/triage lease: enforce ONE disposing advisor per scope (ADR-3 gate covers WHO, not HOW-MANY) (#563)
- **TASK-684** — STORY-265 slice 2: aida queue work --plan-only (interactive plan launch) (#559)
- **TASK-683** — STORY-265 slice 1: aida plan promote — Approved→Planned when a plan file exists (#558)
- **TASK-304** — Config + heuristic for aida ultraplan invocation cadence (.aida/config.toml [ultraplan] mode) (#549)
- **TASK-671** — queue list 'Awaiting commit' Next-hint: substitute the real SPEC-ID, be remote-aware (no 'open a PR' for local-only), show a literal commit example (#547)
- **TASK-673** — aida doctor integrity check: flag Completed specs with no commit referencing them (spec-graph ⟂ git tripwire) (#546)
- **TASK-677** — EPIC-35 list op: route detect_open_pr_for_spec + detect_merged_pr_for_branch through the Forge trait (#543)
- **TASK-676** — EPIC-35 comment op: route gh pr comment through Forge::comment (checkout has no live sites) (#542)
- **TASK-675** — EPIC-35 create op: route gh pr create through Forge::open_change (#541)
- **TASK-674** — EPIC-35 ci-watch (pr ship): route gh pr checks --watch through Forge::watch_ci (#540)
- **TASK-672** — EPIC-35 ci op (point-probe): route probe_ci_state_for_branch through Forge::ci_probe_for_branch (#539)
- **STORY-516** — EPIC-35 slice 1b: route the gh call sites behind the Forge trait (+ pure-git ship/drain) (#480, #481, #483, #538)
- **TASK-669** — EPIC-35 slice-1b: route the orchestrator phase-4 merge through Forge::merge_change (needs retry-sink injection) (#537)
- **TASK-668** — EPIC-35 slice-1b: route the aida pr ship merge through Forge::merge_change (#536)
- **TASK-525** — auto-complete telemetry record LifecycleSkip in JSONL event for retro analysis (#535)
- **TASK-315** — Consider widening `aida list`'s Status column and adding the glyph there too (#530)
- **TASK-593** — aida queue prune --orphaned misses review-queue rows for already-merged PRs (#529)
- **TASK-240** — aida queue work status-aware error (TASK-217): Done case should suggest 'gh pr merge' when user is PR author (#528)
- **TASK-253** — Add .mcp.json to AIDA dev repo so Claude Code dogfoods aida mcp-serve in our own workflow (#524)
- **TASK-666** — STORY-457 persistence slice: .aida/last-status.toml + untracked-history.toml (recent-vs-stale + unblocks TASK-662 delta) (#523)
- **TASK-524** — aida edit warn on unrecognized lifecycle:* tag spellings (typo guard) (#522)
- **TASK-665** — STORY-456 core: worktrees pane in aida status (#519)
- **TASK-664** — TASK-662 JSON slice: findings detail in aida status --json (#518)
- **TASK-663** — TASK-539 core: pending-findings text section in aida status (#516)
- **TASK-102** — aida show: enumerate relationships inline (not just the count) (#515)
- **TASK-660** — STORY-457 core: aida status working-tree section (modified/staged/untracked + safe-to-remove) (#514)
- **STORY-446** — Add --blocked-by / --depends-on flag to aida add and aida edit (#513)
- **TASK-654** — [EPIC-35 s2] Forge-aware pr-ship dry-run plan (TASK-651 slice 3) (#509)
- **TASK-653** — [EPIC-35 s2] Forge-aware orchestrator recovery hints (TASK-651 slice 2) (#508)
- **TASK-652** — [EPIC-35 s2] Per-forge command vocabulary + status_cleanup hint conversion (TASK-651 slice 1) (#507)
- **TASK-650** — [EPIC-35 s2] Forge-aware hint helper + workflow_hints conversion (STORY-508 foundation slice) (#506)
- **TASK-502** — aida brief --notify: sentinel-file mechanism so idle agent chats surface pending briefs without heartbeat (#505)
- **TASK-648** — Triage-inbox workflow: extend /aida-triage into a draft-inbox clearer + surface inbox depth (#504)
- **TASK-647** — Advisor-gate the approve transition + queue-for-work (non-advisor/headless filers land draft) (#503)
- **TASK-646** — aida agent new: prompt for the child's role when none provided (interactive); default implementer + notice when not (#502)
- **TASK-645** — Implementer = read-side default role (unset → implementer everywhere) + surface it at init (#501)
- **TASK-644** — aida role enter: show an interactive role picker instead of erroring (when stdin is a TTY) (#500)
- **TASK-394** — Persistent --no-human=both acknowledgment via file marker (eliminate per-loop env-var dance) (#499)
- **EPIC-30** — aida queue worker — persistent autonomous executor with control plane + advisor communication (#497)
- **TASK-313** — Optionally render the plan brief inside the aida show --card box (#496)
- **TASK-383** — Planning-pass discipline: don't leave untracked plan files in main's docs/plans/ (they conflict with later PR merges) (#495)
- **TASK-636** — Trim scaffolded .claude/AIDA.md (prose→pointer) to cut per-session context on consumer projects (#494)
- **TASK-507** — aida history --shipped — recent Done→Completed transitions (the missing 'did my ship register?' view) (#493)
- **TASK-527** — aida list --tags: support prefix-glob filtering (aida:queue:* → all subcommand tags under that surface) (#492)
- **TASK-475** — aida queue list: warn when local orphan store is behind origin/aida-store (#491)
- **STORY-513** — EPIC-35 slice 1a: forge-provider foundation (trait + 3 providers + [forge] config + init auto-detection) (#470)
- **STORY-496** — aida doctor: reap dead-PID agent-registry entries (stale agents) — new check+heal category (#434)
- **STORY-492** — P1 resumable drain — slice 2: phase-postcondition probing + live --resume re-entry (sign-off-gated)
- **STORY-495** — Faithful launcher: all agent launchers honor each tool's native permission default + single uniform bypass opt-in
- **TASK-638** — aida init should bootstrap the default role set (role scaffold) — fresh clone shows empty 'aida role list'

### Fixes

- **BUG-470** — Windows test flake: aida-tui state.rs temp_root() collides under coarse clock → load_is_none_when_absent_or_malformed fails (#638)
- **BUG-469** — Windows test build broken: forge.rs fake_output uses Unix-only ExitStatusExt::from_raw (EPIC-35) (#637)
- **BUG-468** — Windows build broken: libc::ETXTBSY un-gated in command_output_retrying_etxtbsy (BUG-463 regression) (#636)
- **BUG-467** — Harness dirs .claude/worktrees/ + .claude/workflows/ not gitignored → pollute clean-tree (blocks release gate) (#635)
- **BUG-466** — Windows: add_pending_brief stores OS-native path separators → cross-platform CI red (#634)
- **TASK-679** — Normalize parent/child relationship edges: rel add --type parent writes inverted+duplicated Parent edge with no reciprocal (#631)
- **TASK-402** — aida queue work --resume: resume-after-failure path UX cleanup (4-friction cluster) (#620)
- **TASK-297** — Audit skill templates for interactive prompts; add headless-aware paths via AIDA_NO_HUMAN env var (#613)
- **BUG-433** — Worktree with .aida-store symlink but no committed .aida/config.toml → plain aida falls to legacy backend + shows WRONG data (inverse of BUG-428) (#610)
- **BUG-464** — aida status miscounts seeded META prompts as 'untriaged drafts' on a fresh init (#609)
- **BUG-455** — Headless drain phase fails un-shelvably on 'database is locked' (SQLite cache lock contention with concurrent aida processes) (#606)
- **BUG-463** — Flaky pr_ship_environment_tests in CI: 'untracked working tree files would be overwritten by merge' + panic on branch PRs (#599)
- **BUG-445** — Onboarding first-task 'Commit AIDA scaffolding' breaks under 'aida queue work' worktree isolation — scaffolding absent + git add . embeds gitlink/symlinks (#589)
- **TASK-619** — Cross-machine duplicate work: leases are local (.aida/sessions), so only the eventually-consistent status flip guards against two people grabbing the same spec (#588)
- **TASK-125** — Empirical 2026-05-25: 'aida init' followed immediately by 'aida status' reports: (#581)
- **TASK-681** — Audit all mutating MCP tools for authority/lifecycle gate parity (EPIC-38) (#578)
- **FR-267** — Cross-repo trace link resolution (#576)
- **BUG-415** — Fresh-init: aida status reports Total 7 / Draft 6 but aida list --all shows only 1 requirement (#572)
- **TASK-667** — Shell wrapper signals presence via env var; binary tailors auto-eval hints (bare vs eval form) (#565)
- **BUG-417** — aida pr ship: assumes base 'main' (ignores origin default branch) + CI-wait blocks on repos with no workflows (#562)
- **BUG-449** — MCP update_requirement bypasses the TASK-647 advisor-authority status gate (enforced in add_requirement) — agent self-advances spec to Completed on uncommitted code (#555, #556)
- **BUG-447** — aida queue work session-resume matches prior claude sessions by spec-id GLOBALLY (~/.claude/projects scan, no project scoping) — a deleted/other project's TASK-007 session bleeds into a fresh project (#554)
- **BUG-446** — aida init has no guard against initializing over a workspace-of-projects — silently git-inits a tree of nested .git repos and roots the store there (#553)
- **BUG-448** — aida graph --tree (epic rollup) omits children parented via 'aida rel add --type parent' — parent FIELD vs Parent EDGE are two divergent sources of truth (#552)
- **BUG-462** — Headless --no-human=both: implementer clean-exit-with-question is not routed to the advisor tier (BUG-459 sibling) (#551)
- **BUG-460** — Advisor-gate (TASK-647) blocks the orchestrator's OWN auto-queue in headless drains — reviewer phase fails on 'needs advisor authority' (#550)
- **BUG-453** — Headless drain: phase-1 watchdog kills PRODUCTIVE implementer sessions (no-commit-in-10m fires despite real file edits) (#545)
- **TASK-431** — parse_plan_followups should filter sentinel bullets (None, N/A, Nothing) (#532)
- **TASK-271** — Tighten BUG-114 genuine-miss error message: candidate ids vs --resume semantics (#527)
- **TASK-252** — TUI: overlay session-id truncation byte-slices &s.id[..12] — panic risk on a non-ASCII id (#525)
- **BUG-444** — Phase-1 NoPr false-failure: empty-but-successful 'gh pr list' (eventual-consistency lag) classified as definitive NoPr, no retry — the dominant drain-failure cause (#520)
- **TASK-287** — Strip trace:/SPEC-ID markers from clap doc comments so 'aida <cmd> --help' is clean (#517)
- **BUG-432** — aida queue work hard-fails at startup db-sync-pull when the project has no 'origin' remote (#511)
- **TASK-467** — global_auto_claim_summary should honour per-type auto_claim re-enable in opt-out case (#490)
- **TASK-494** — aida push: suppress merged-branch + stale-base warnings when code-leg has nothing to push (#488)
- **TASK-322** — aida queue work next3 --batch NAME silently swallows the next3 keyword (#487)
- **BUG-412** — extract_referenced_spec_ids_from_commit pulls code-like (PREFIX-NNN) from commit bodies as false references (#484)
- **BUG-443** — MCP tool-count drift: docs/agents/cross-agent-onboarding.md says 26, source advertises 29 (self-test asserts stale 26) (#482)
- **TASK-140** — aida pr ship squash-subject mismatch: PR-347 (TASK-575) merged with the WRONG s… (#476)
- **TASK-128** — Scaffold commits emit 'Staged files trace to:' warnings listing FOREIGN trace I… (#472)
- **BUG-442** — Fresh clone with no local .aida silently reads a WRONG ambient store instead of auto-attaching origin/aida-store (TASK-621 doesn't fire) (#471)
- **BUG-409** — Flaky CI: story_429_auto_rebase_tests::clean_auto_rebase_proceeds_without_retrying_preflight (git-state-sensitive) (#469)
- **BUG-440** — queue work PR-N / reviewer bails on a PR with multiple backing specs instead of preferring the trailer-credited spec (#468)
- **BUG-434** — aida pr ship --delete-branch orphans stacked children — add a stacked-children guard before deleting the base branch (#467)
- **BUG-426** — Auto-bump false-completes umbrella specs from cores/plan commits (agreed-id matching) — recurring, mechanism unclear vs push_paren code (#459)
- **TASK-133** — aida queue work phase-1 startup transitions status before acquiring lease — lea… (#438)
- **TASK-629** — BUG-431 #2: reviewer resolves PR backing epic+child to the most-specific spec (drop ancestors) (#436)
- **BUG-422** — aida session end hangs indefinitely on the CI/PR probe when stdin is not a TTY (no timeout) (#435)
- **BUG-423** — Flaky CI: agent_launcher_tests::tracked_fake_antigravity_receives_env_args_and_registry_is_removed fails on temp-binary spawn under CI load (#433)
- **BUG-250** — Orchestrator outcome model: 'PR deliberately held' is mis-classified as phase-1 failure (BUG-241 extension)
- **BUG-425** — Bulk 'aida archive --older-than' is silent + slow: per-spec commits (679), no progress output, pollutes orphan-store history
- **BUG-431** — queue work --no-human drain is unusable for multiple stories under one epic (epic-scope contention + multi-spec PR + cascade lease-block)
- **BUG-436** — Reviewer phase refuses when backing spec is Done — BUG-379 preflight blocks the PR-scoped review session
- **BUG-438** — Resume→reviewer robustness: fast-resume collides with the crashed implementer's lease, and a phase-failed resume clears drain-state
- **STORY-501** — Reviewer phase should review PR-N without taking ownership of the backing spec
- **TASK-615** — Implement drain-reliability wiring (slice 2): inconclusive→retry-then-shelve + phase no-progress/ceiling watchdog

### Documentation

- **TASK-685** — Refresh multi-user-setup.md: drop removed db migrate / db export-git commands (#612)
- **SPIKE-47** — Define 'trace coverage' for a diff — what code requires a // trace:, and the exemptions (#604)
- **TASK-627** — Docs freshness spot-check before demo — getting-started.md + retire stale slideshow.html (#603)
- **STORY-266** — [EPIC-29 1/5] Define aida-core public API contract — audit + freeze types/methods aida-tui consumes (#597)
- **TASK-311** — Dialog-role spec audit: verify acceptance criteria match the primary caller's environment before filing (#596)
- **TASK-682** — Retire/redirect docs/slideshow.html — the OBE 2025-12 deck is still discoverable and actively misleading (#560)
- **TASK-632** — Document the `[agents] bypass` knob in the discipline pack / CLAUDE.md agents.toml section (#534)
- **TASK-530** — TASK: scaffolding-pack: link `backlog-grooming.md` from the auto-appended Discipline section in CLAUDE.md (#533)
- **TASK-109** — docs/git-verb-surface.md: design reference naming the convention (#531)
- **TASK-456** — Discipline pack: add 'recursive-failure-risk fixes use keyboard, not drain' section to workflow-patterns.md (#526)
- **TASK-258** — aida-review skill step 10 documents nonexistent 'aida queue remove --yes' flag (#521)
- **ADR-2** — Role onboarding: implementer is the default role; surface roles at init; never leave the user role-undefined (#512)
- **ADR-3** — Intake-triage model: captures land draft; approval is advisor-gated (role-or-interactive-human); advisor dispositions the inbox (#512)
- **TASK-472** — Clarify in cross-agent-onboarding.md: file_finding is for triageable bugs/tasks, NOT session checkpoints (#510)
- **TASK-415** — aida-pickup.md + aida-pr.md: replace hand-written State preamble blocks with aida state-snapshot calls (#498)
- **TASK-317** — aida queue move --to <N> slot numbering can diverge from role-filtered 'aida queue list' (#489)
- **SPIKE-41** — AIDA hook bundle for lifecycle integration (SessionEnd→aida pull, PreCompact→snapshot, Stop→lease validate) (#479)
- **SPIKE-29** — Claude Code agent teams (EXPERIMENTAL) — a third orchestration surface beyond subagents + workflows (#478)
- **SPIKE-17** — Claude Code hooks lifecycle — full event taxonomy + return-value schema (#477)
- **SPIKE-16** — Claude Code skills system — frontmatter schema, disable-model-invocation, helper subfolders, /reload-skills (#475)
- **TASK-626** — aida agent new --spec: signpost that this lane ships a PR + exits (not orchestrated), name the queue-work alternative (#474)
- **TASK-628** — Wire vs-claude-code-workflows.md into positioning index + CLAUDE.md neighbor list (#474)
- **SPIKE-44** — Multi-vendor substrate access: can a non-AIDA tool read/write the git-canonical store? (#466)
- **SPIKE-49** — [EPIC-35 slice 0] Forge-provider SPIKE — inventory gh coupling, define the trait, prove a glab MR round-trip (#465)
- **SPIKE-26** — Codex task/state model — does Codex track work across sessions? How does state persist? (#464)
- **SPIKE-25** — Codex MCP + tool model — protocol parity with Claude Code + AIDA's mcp-serve (#463)
- **SPIKE-24** — Codex CLI — architecture overview + AGENTS.md bilingual scaffolding + native multi-agent surface (#462)
- **SPIKE-28** — Antigravity multi-agent + skill surface — compare with Claude Code workflows + Codex (#461)
- **SPIKE-27** — Antigravity (AGY) CLI — architecture overview, registry, configuration surface (#460)
- **TASK-637** — Doc refresh to v0.11.0 state + audience-targeted slide decks (exec/dev/admin/user)

### Infrastructure

- **TASK-453** — Flip MCP stdio CI gate to full roundtrip once BUG-310 ships (#624)
- **TASK-649** — CI guard: assert every embedded-template path exists + is uniquely owned (template-drift defense) (#564)
- **TASK-224** — Add aida-web-react build (tsc -b + vite build) to CI (#561)
- **TASK-625** — gitignore .claude/scheduled_tasks.lock — scheduler runtime lock leaks as untracked file (#473)
- **TASK-425** — Verify actions/download-artifact@v5 multi-artifact layout still works on next release tag

### Internal

- **TASK-262** — Add automated coverage for RealPhaseDriver subprocess wiring (#602)
- **TASK-225** — TYPE_CONFIG: design proper palette for Principle/Vision/Constraint/Decision/Term/Doc (#548)
- **TASK-465** — Refactor scaffolding-pack tests away from hardcoded counts toward structural assertions (#486)
- **TASK-320** — Widen test_skill_template_glyphs to guard all embedded skill templates (#485)

### Other

- [AI:claude] docs: AIDA<->Claude Code coexistence — hooks-as-substrate-bridge + quizdom worked example
- [AI:claude] docs: add 'What AIDA loads into context — and when' section
- [AI:claude] docs: trim CLAUDE.md 42.3k->34.4k (reference prose -> pointers) (#445)
- [AI:claude] feat(forge): route the orchestrator ci-watch through Forge::stream_ci_for_branch (TASK-678) — EPIC-35 routing complete (#544)
- docs(plans): record the 168-spec backlog disposition sweep (2026-06-06) (#557)

## [v0.11.0] — 2026-05-31

Specs merged since v0.10.0 (124):

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
- **TASK-403** — Demo: record an overnight autonomy-keystone drain with recovery narrative (asciinema + writeup) (#190)
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

Specs merged since v0.4.5 (73):

### Features

- **STORY-90** — Auto-queue PR for reviewer at PR-create time, not session-end (redesign STORY-66 trigger; idempotent re-fire as backup) (#10)
- **STORY-98** — Session manifest: capture cluster intent on /aida-pickup; show 'planned:by-X' chip in queue list (#10)
- **STORY-48** — EPIC-20 v2: lease enforcement — aida edit / aida-pickup honor session scope (#1)
- **STORY-55** — aida statusline: @SPEC defaults to session scope when no in-session activity yet (#1)
- **EPIC-19** — aida doctor: maintenance & migration commands (per-repo, hidden top-level)
- **EPIC-20** — aida session start: scoped concurrent sessions with worktree + lease
- **EPIC-21** — Code↔store commit pairing: pin orphan store SHA per code commit
- **EPIC-9** — Identity UX: auto-acquire on init, string node ids, per-user preferences
- **FR-1-070** — FR-1-070 _(spec not in store)_
- **FR-1-071** — FR-1-071 _(spec not in store)_
- **FR-1-073** — FR-1-073 _(spec not in store)_
- **FR-1-074** — FR-1-074 _(spec not in store)_
- **FR-1-077** — FR-1-077 _(spec not in store)_
- **FR-1-258** — FR-1-258 _(spec not in store)_
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
- **BUG-17** — aida add: --description-from-file PATH and --description-stdin for multi-line input
- **BUG-21** — Scaffolder writes .git/hooks/commit-msg without execute bit
- **BUG-22** — aida add: warn when title contains unquoted shell-special chars (backticks, unbalanced quotes)
- **BUG-23** — aida init should push the orphan branch automatically when origin exists
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
- [AI:claude] feat(dev): activate pinning + stale-build warning + PS1 marker
- [AI:claude] feat(upgrade): --diff flag to vet unreleased commits before shipping
- [AI:claude] feat(ux): seven walkthrough findings — name/META/origin/comments/push/validator (BUG-25..30, FR-264)
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

Specs merged since v0.4.2 (33):

### Features

- **EPIC-1-001** — EPIC-1-001 _(spec not in store)_
- **FR-1-011** — FR-1-011 _(spec not in store)_
- **FR-1-013** — FR-1-013 _(spec not in store)_
- **FR-1-027** — FR-1-027 _(spec not in store)_
- **FR-1-028** — FR-1-028 _(spec not in store)_
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
- [AI:claude] fix(history): convert YAML modified_at from UTC to local time
- [AI:claude] fix(history): pad columns before colorizing for proper alignment
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
- **FR-0187** — FR-0187 _(spec not in store)_
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

