# Chapter 9 — Integrations & servers

AIDA's defensible value isn't the CLI — it's the **requirement graph** sitting behind it. This chapter is about the surfaces that *project that graph outward*: to a coding agent (the **MCP server**, the highest-leverage surface of all), to an issue tracker you already live in (**GitHub / GitLab / Jira**), to a remote AIDA **server**, and to a rendered **docs tree** that turns the graph into something a human reads instead of queries. The unifying idea: the YAML store is the source of truth, and everything here is a *bridge* from it to a consumer — an agent, a teammate's Jira board, a docs site — without that consumer having to learn AIDA's storage model.

> Manual contract reminder: rationale, not flag tables. `aida <cmd> --help` is the source of truth for exact flags and defaults. We cover only the options whose *why* isn't obvious from the name.

> **Two near-identical names, two different jobs.** `aida doc` (singular) and `aida docs` (plural) are the most confusable pair in the whole CLI. **`doc`** captures *raw narrative entries* tied to specs (the living-documentation surface). **`docs`** *renders the graph as a layered docs tree* on disk. One writes hand-authored prose into the graph; the other reads the graph out as files. They're covered back-to-back below precisely so the split is unmissable.

---

### `aida mcp-serve`

**One line** — exposes the requirement graph to Claude Code (and other MCP clients) as tools and resources over stdio.

**Mental model.** This is the surface that makes AIDA more than a CLI an agent shells out to. An MCP-speaking agent doesn't parse `aida list` stdout — it *calls structured tools* (`list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `add_relationship`, `query_graph`, `list_features`, `history`) and reads *resources* (`aida://project/summary`, `aida://requirements/tree`) directly into context. The whole point is that the requirement graph becomes part of the agent's *working memory*, queryable mid-task, rather than something it has to remember to invoke and then screen-scrape. You almost never run this by hand — it's launched by the MCP client per the `.mcp.json` that `aida init` wrote.

**Why an agent reaches for the MCP tools over the CLI.** Three reasons. (1) **Structured I/O** — a tool call returns typed JSON the agent can reason over, where CLI stdout is text it must parse and can misread. (2) **Resources load into context** — `aida://project/summary` and `aida://requirements/tree` are pulled in as readable context, not fetched-and-piped. (3) **Schema-as-contract** — the tool schemas mirror the CLI surface (status/type enums, `list` filters, `show`'s git linkage, the gated status transitions), so the agent gets the *same governance* (approval-gated transitions, merge-driven completion) without re-implementing it. For an agent doing requirement-aware work, this is a tighter loop than spawning a subprocess per query.

**Reach for it when** — essentially never *manually*. It's the daemon side of an MCP client connection; the client spawns it. You'd run it directly only to debug the JSON-RPC handshake (it reads JSON-RPC 2.0 from stdin, writes to stdout).

**Don't reach for it when** — you want a human-readable answer at your own terminal. That's the plain CLI. `mcp-serve` speaks a wire protocol, not to you.

**Gotchas.** It takes **no flags** — configuration lives in the client's `.mcp.json`, not on this command line. Long-running servers **self-respawn** after a handled request when the on-disk `aida` reports a newer version or build SHA, so the *next* request runs on the new binary (the current response is flushed first); if a client still looks stale, kill that agent's `aida mcp-serve` process and let the client restart it. When you add a CLI filter or field, mirror it onto the matching MCP tool schema + handler or the two surfaces drift — this dev repo dogfoods its own MCP server precisely so that drift surfaces here first.

**Chains with** — registered by `aida init`; consumed by the agent that then walks the graph and does the work the other chapters describe.

---

### `aida server`

**One line** — talk to a *remote* AIDA server instead of the local store.

**Mental model.** Where everything else in AIDA reads the local git-canonical store, `aida server` is the thin client for a deployment that centralizes the graph behind `aida-server` (REST + gRPC). Its subcommands — `status`, `ping`, `list`, `get` — are deliberately read-only connectivity probes: "is the server up, and can I see requirements through it?" It's not an alternative storage backend for daily work; it's the verb for *checking on / reading from* a remote instance.

**Reach for it when** — you run a shared `aida-server` and want to confirm it's reachable (`ping`/`status`) or read a spec through it (`list`/`get`) without a local store. Useful in a CI box or a thin client that has the server URL but no checkout.

**Don't reach for it when** — you have a local AIDA project. The git-canonical store + cache is the fast, offline-capable path; routing through a server adds a network hop and buys you nothing locally. Use plain `aida list`/`show`.

**Key options (rationale only).** The server target isn't a subcommand flag — it's the **top-level** `-s/--server <host:port>` (or `AIDA_SERVER` env). Every `server` subcommand *requires* one of those to be set; that's why the command's help reads "requires `--server` or `AIDA_SERVER`."

**Gotchas.** It's the only command group keyed off the global `--server`/`AIDA_SERVER` rather than the local store, so a forgotten env var is the usual "why does this say no server configured?" The subcommands are read-only by design — there's no `server add`/`server edit`. Writes still go through the normal local-store path.

**Chains with** — stands apart from the lifecycle; it's an out-of-band read/health surface for a remote deployment.

---

## Issue-tracker bridges: `github` / `gitlab` / `jira`

These three follow the **same shape**: a `config` step to wire credentials, a `test` to prove the connection, then `list`/`show` to read the foreign tracker, and push/pull/sync verbs to move issues across the boundary. Treat them as *bridges*, not mirrors — AIDA's graph stays the source of truth, and these verbs reconcile it against an external board you already use. (Note the asymmetry across the three: GitHub and Jira have `push`/`pull`/`sync`; GitLab leans on `refresh`/`poll` for *change detection* rather than a one-shot `push`.)

### `aida github`

**One line** — bridge AIDA requirements to and from GitHub Issues.

**Mental model.** A full round-trip bridge: `config` (set `--repo owner/repo` + token, or read `AIDA_GITHUB_TOKEN`), `test`, then `list`/`show` to read issues, `push <ID>` to publish a requirement as an issue, `pull` to import issues as requirements, and `sync` to *detect drift* between the two sides. The mental hinge is **`sync` is dry-run by default** — it shows you what would change and only writes with `--apply`. That keeps a cross-system reconcile from silently rewriting either side.

**Reach for it when** — your team files in GitHub Issues but you want the AIDA graph (types, relationships, traces) on top; `pull` brings the issues in, `push` publishes specs out, `sync --apply` keeps them aligned.

**Don't reach for it when** — you want the *git* side of GitHub (PRs, merges, CI). That's the lifecycle chapter's `pr`/`pull`/`push` over `gh`, not this issue bridge. This command is about *Issues*, not pull requests.

**Key options (rationale only).**
- `github sync --apply` — the safety switch. Without it, `sync` is a *preview* (dry-run); with it, the detected drift is written. Reach for the preview first, always.
- `github sync --linked-only` — restrict reconciliation to items already linked (`[GH-N]` prefix or a github URL), so a sync doesn't pull in the whole unlinked backlog when you only meant to refresh known links.
- `github pull --dry-run` / `--labels` / `--limit` — `--dry-run` previews the import; `--labels`/`--limit` scope it so a one-off pull doesn't drag in hundreds of unrelated issues.

**Gotchas.** Credentials come from `--token` or `AIDA_GITHUB_TOKEN`; a missing token is the usual `test` failure. `sync`'s default-dry-run is a feature — if "nothing changed," check whether you forgot `--apply`.

**Chains with** — `config` → `test` → `pull`/`push` → `sync` on a cadence.

### `aida gitlab`

**One line** — bridge AIDA requirements to GitLab Issues, with background change-polling.

**Mental model.** Same `config`/`test`/`list`/`show` front door as the GitHub bridge, but its reconciliation model is **poll-based** rather than push/pull-on-demand: `refresh` checks GitLab for changes to linked items, and `poll` runs that check on an interval in the background. `status` shows the sync state of linked items, and `labels` manages the AIDA-tag ↔ GitLab-label mapping. The design assumption is a longer-lived link kept fresh by polling, versus the GitHub bridge's discrete sync runs.

**Reach for it when** — your tracker is GitLab and you want linked items kept current without re-running a sync by hand; `poll start` does that.

**Don't reach for it when** — you want a one-shot import/export like GitHub's `pull`/`push`. The GitLab surface is oriented around *keeping links fresh*, not bulk-moving issues; `refresh` is the closest single-shot verb.

**Key options (rationale only).**
- `gitlab poll [start|stop|status]` with `--interval <secs>` (default 300) — `poll` is a background watcher; the positional action picks lifecycle (default `status`), and `--interval` trades freshness against API load. 300s is a sane default; tighten it only if you genuinely need near-real-time.
- `gitlab refresh [ID]` with `--force` — refresh one item or all; `--force` overrides the "recently checked, skip it" throttle when you *know* something changed and don't want to wait.

**Gotchas.** Polling is a *background* process — remember to `poll stop` it; a forgotten poller keeps hitting the GitLab API. The label mapping (`labels`) is what makes AIDA tags and GitLab labels line up; mismatched mappings are the usual "why didn't this label come across?"

**Chains with** — `config` → `test` → `poll start`, then `status`/`refresh` to inspect.

### `aida jira`

**One line** — bridge AIDA requirements to and from Jira, with a configurable field mapping.

**Mental model.** Structurally the GitHub twin — `config`/`test`/`list`/`show`/`push`/`pull`/`sync` — but with Jira's heavier data model it adds an explicit **field mapping**: `config --show-mapping` reveals how AIDA fields project onto Jira fields. The same dry-run-then-`--apply` discipline as the other bridges applies to `sync`. The reason `jira` carries a mapping concept the others don't is that Jira's custom-field sprawl means "which AIDA field becomes which Jira field" can't be assumed.

**Reach for it when** — your org standardizes on Jira and you want the AIDA graph as the *authoring* layer while keeping Jira as the system of record management sees; `push`/`pull`/`sync` move work across, `config --project` + the field mapping pin the projection.

**Don't reach for it when** — you only need lightweight issue tracking. Jira's mapping/config overhead is justified only when Jira is *already* mandated; don't adopt the bridge to get a tracker you don't otherwise need.

**Key options (rationale only).**
- `jira config --url` / `--project` / `--email` / `--show-mapping` — the connection triple (Cloud URL, project key, auth email) plus the mapping inspector. `--show-mapping` is the one to run first when a sync mis-projects a field — it tells you the contract before you debug the data.
- `jira sync` — like the others, reconciles AIDA ↔ linked Jira issues; treat it as preview-first.

**Gotchas.** Auth is email-based (Jira Cloud API tokens); `config --show` confirms what's wired. The field mapping is the part that bites — a spec that round-trips with the wrong priority or type is almost always a mapping issue, not a sync bug, so check `--show-mapping`.

**Chains with** — `config` → `test` → `push`/`pull` → `sync`.

---

## The docs projection: `doc` vs `docs`

This is the pair the chapter intro warned about. Read both entries together once and the distinction sticks: **`doc` (singular) writes narrative *into* the graph; `docs` (plural) renders the graph *out* as files.** Same root word, opposite direction of data flow.

### `aida doc`

**One line** — capture living-documentation entries: hand-written narrative tied to the specs it explains.

**Mental model.** A *Doc* is a first-class spec (`DOC-N`) whose body is prose — rationale, a scenario walkthrough, a recipe, a gotcha — linked to one or more other specs via `--about` (which creates a `References` edge). That edge is the magic: `aida doc show <SPEC>` walks it backward to surface "every doc that explains this spec." So documentation stops being a wiki that rots out of sync with the graph and becomes *part of* the graph, queryable by the spec it describes. This is the raw entry surface that powers AIDA's book/tutorial projection.

**Reach for it when** — during work you write down *why* something is the way it is, a recovery recipe, or a scenario worth preserving, and you want it attached to the spec(s) it concerns (`doc add --about SPEC-1,SPEC-2`). Capture freely — same discipline as `add`.

**Don't reach for it when** — you want the *rendered tree* of constitution/vision/glossary layers. That's `aida docs` (plural), below. And you don't reach for `doc` to record a *decision* as a graph node — that's a `decision`/ADR-type spec via `aida add`; a Doc *explains*, it doesn't *govern*.

**Key options (rationale only).**
- `doc add --about <ID>` (repeat or comma-separate) — the load-bearing flag. It's what links the entry to the specs it documents via `References`, and therefore what `doc show <SPEC>` and `doc list --about <ID>` later resolve against. A Doc with no `--about` is an orphan narrative.
- `doc add --scenario` / `--audience` — filter axes, not decoration: `--scenario` labels the *situation* ("muddle recovery"), `--audience` (`user`/`agent`/`developer`) labels *who it's for*, and both become filters on `doc list`. Tag them so the projection can slice by them.
- `doc add --description-from-file` / `--description-stdin` — for prose too long or structured for a command line (mirrors `aida add`). Prefer these over fighting shell quoting on multi-paragraph narrative.
- `doc coverage --since <REF>` / `--json` — the release-time gate: which specs reached Completed since the last tag with *no* doc about them. It's **warn-only (exits 0)** by design, so you can wire it into a release flow as a nudge without it blocking the release.

**Gotchas.** `doc show <id>` is overloaded: pass a *Doc* id (`DOC-3`) and you get that entry's full detail; pass *any other* spec id and you get every Doc that References it. That dual behavior is intentional but surprises people the first time. And `doc coverage` never fails the build — if you want a hard gate, you have to check its output yourself.

**Chains with** — written during/after implementation, surfaced by `doc show <SPEC>`, gated at release by `doc coverage`, and ultimately consumed by `aida docs` when the tree is rendered.

### `aida docs`

**One line** — render the requirement graph as a layered documentation tree on disk.

**Mental model.** Where `doc` *writes* narrative in, `docs` *reads the whole graph out* as a structured docs site under `docs/aida/`. Each layer — constitution (principles), vision, constraints, decisions (ADRs), quality, glossary — is rendered from its corresponding `RequirementType`. **The graph is the source; this is the projection.** That's the key property: you never hand-edit the rendered tree, you edit specs and re-render, so the docs can't drift from the graph (and `docs check` *proves* they haven't).

**Reach for it when** — you want a human-readable, browsable docs tree (for a README site, onboarding, or review) derived from the specs you've already filed; `docs build` renders it, `docs check` verifies it in CI.

**Don't reach for it when** — you want to *author* a narrative entry. That's `aida doc` (singular). And don't hand-edit files under `docs/aida/` — they're generated; your edits get clobbered on the next `build`. Change the *spec*, then re-render.

**Key options (rationale only).**
- `docs build --dry-run` — show what *would* be written without touching disk. The safe preview before a render, especially the first time or after big graph changes.
- `docs build -o/--output` — redirect the tree somewhere other than `docs/aida/` (e.g. a separate docs-site repo). The default is right for in-repo docs.
- `docs check` — verify the on-disk tree matches what the graph *would* render, **exiting non-zero on drift**. That non-zero exit is the whole value: wire it into pre-commit/CI and a stale docs tree fails the build instead of silently lying.
- `docs glossary` (+ `--machinery` / `--lifecycle`) — print AIDA's *embedded* machinery + lifecycle glossary (orchestrator, drain, lease, …; committed/merged/completed/released). It reads the binary's copy, so it stays correct even when the project's scaffolded glossary file is missing or stale — the reliable vocabulary lookup.

**Gotchas.** `docs check` is the drift-guard, but it only catches drift *if you run it* — an un-wired `check` lets the tree rot just like any other generated artifact. And the rendered tree is only as good as the typed graph beneath it: if your principles aren't filed as `principle` specs and your decisions aren't `decision` specs, those layers render empty. The projection rewards correct typing at `add` time.

**Chains with** — fed by everything filed via `add`/`doc`; rendered with `build`; guarded with `check`; the vocabulary surfaced by `glossary` is the same one the rest of this manual uses.

---

## Where to go next

You've now seen how the graph reaches *outward* — to agents, trackers, remote servers, and rendered docs. Related threads:
- **[Chapter 7 — Project setup](07-project-setup.md)**: `aida init` (which registers the MCP server and scaffolds `.mcp.json`), plus `scaffold` and `memories` — the rest of the agent-integration furniture.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: the *git* side of GitHub/GitLab (PRs, merges) that the issue bridges here deliberately don't touch.
- **[Chapter 2 — Specs](02-specs.md)**: the typed graph (`decision`/`principle`/`vision`/`term`) that the `docs` projection renders — get the types right there and the docs tree fills itself in.
- For MCP client setup across editors, see [`docs/agents/aida-mcp-install-matrix.md`](../agents/aida-mcp-install-matrix.md).
