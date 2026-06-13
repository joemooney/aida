# Chapter 1 — Getting started & the daily drivers

The commands you touch on day one and then every day after: `init` once, then `add` / `list` / `show` / `edit` / `done` constantly. Master these six and you can use AIDA; everything in the later chapters is depth on top of this floor.

> Reminder on the manual's contract: this chapter explains *when and why*. For the exact flag list and defaults, `aida <command> --help` is the source of truth — we don't copy it here (that's how reference docs rot). We cover only the options whose *rationale* isn't obvious from their name.

---

### `aida init`

**One line** — turns a git repo into an AIDA project.

**Mental model.** `init` is a *one-time, per-project* bootstrap. It creates four things: the **store** (an orphan `aida-store` branch holding one YAML file per spec — versioned alongside your code but not cluttering your working tree), a rebuildable **cache** (`.aida/cache.db`, gitignored, makes `list`/`search` instant), the **MCP server** registration (so coding agents can query the graph), and the **scaffolding** (skills, hooks, templates, the discipline pack). On the *first* `init` on a machine it also sets machine-global defaults (the role set, the agent permission posture).

**Reach for it when** — you're starting AIDA in a repo for the first time, *after* `git init` (AIDA's store needs a git repo to live in).

**Don't reach for it when** — the project is already initialized (you'll be refused unless you pass `--force`); or you want to *re-scaffold updated templates* into an existing project — that's `--refresh` flows, not a fresh `init`. And don't reach for `--centralized`: it's the deprecated SQLite-canonical mode, kept only for legacy projects; new projects want the git-canonical default.

**Key options (rationale only).**
- `--sibling` — use a separate repo for the store instead of an orphan branch. The whole point is *multi-repo workspaces*: several code repos sharing one requirement graph. A single-repo project should not use this.
- `--agent claude|codex|both` — which agent profiles to scaffold skills for. Default `both` is right unless you know you'll only ever use one.
- `--name` — sets the project's display name; defaults to the directory basename, which is usually what you want — set it explicitly only when the folder name is ugly.
- `--with-memories` — opt into the starter memory pack (generic AIDA-using discipline). Worth it for a project you'll run agents in heavily; skippable for a quick trial.
- `--no-skills` / `--no-hooks` / `--no-roles` / `--no-agent-config` — escape hatches for "I want the store but not the Claude Code furniture." Most users want all of it.

**Gotchas.** It refuses to initialize *over a workspace of nested git repos* (it would capture the whole tree) — that's a feature, not a bug; use `--sibling` for that shape. Non-interactive `init` never prompts (so it's CI-safe), which means the first-machine permission-posture prompt is silently skipped — fine, it defaults to the faithful posture.

**Chains with** — followed by `aida add` (your first spec) and, if the repo has no remote, an offer to wire one. Once a day later, `aida doctor` checks the init stayed healthy.

---

### `aida add`

**One line** — captures an idea as a spec.

**Mental model.** `add` is *capture*, not *commitment*. A fresh spec lands as **Draft** by default — written down, not blessed. The cost of capturing is near-zero and the value is high (a thought you didn't file is a thought you lost), so the right discipline is **capture freely**; the gate that decides whether it gets built comes later (approval), not here. The newcomer form is just `aida add "Some title"`; the full form adds type, description, links.

**Reach for it when** — *any* time an idea, bug, or task surfaces and isn't already a spec. Mid-conversation, mid-debugging, mid-review. The bar for filing is low on purpose.

**Don't reach for it when** — the work is already a spec (search first if unsure — `aida search`); or you're tempted to *pre-approve* your own captures by passing `--status approved`. That's allowed but it's the front gate, and on a multi-person/multi-agent project approval is authority-gated — a non-advisor capture should stay Draft and let the disposing seat approve it. Self-blessing every capture defeats the gate that is AIDA's whole point.

**Key options (rationale only).**
- `--type` — the single most consequential field. `task` is the catch-all for chores/docs/tooling; `bug` records a defect; `epic`/`story` for agile hierarchy; the ADR family (`decision`/`principle`/`constraint`/`vision`/`term`) drives the docs projection. Picking the right type is what makes the graph queryable later.
- `--parent` / `--blocked-by` (alias `--depends-on`) — wire the spec into the graph *at creation* rather than with a follow-up `rel add`. Cheap to do now, easy to forget later.
- `--description-from-file` / `--description-stdin` — for descriptions too long or too structured to fit on a command line (acceptance criteria, code blocks). Prefer these over wrestling shell quoting.
- `--status approved` — see "don't reach for it when." Legitimate when *you are* the disposing authority and this capture is genuinely blessed.
- `--prefix` — override the ID prefix (e.g. `SEC`, `PERF`). Rarely needed; the type-derived prefix is usually right.

**Gotchas.** The positional title and `--title` both exist; if you pass both, `--title` wins. Tags are comma-separated; if you mean to *add* a tag to an existing spec later, that's `edit --add-tag`, not `edit --tags` (which *replaces*).

**Chains with** — the returned SPEC-ID is what you then `trace:` in code, reference in a commit `(SPEC-ID)` trailer, and feed to `edit` / `queue add` / `show`. **File the spec first, then write the trace** — guessing the next ID is off-by-one.

---

### `aida list`

**One line** — the everyday "what's in the graph" view.

**Mental model.** `list` is a **lens** — pure sight, no state change — over the *cache*, so it's instant even on a large store. By default it shows the *open, non-archived, non-meta* set: the work that's actually live. Everything else (`--archived`, `--all`, `--include-meta`) is an explicit opt-in to widen the view.

**Reach for it when** — you want the current picture, or a filtered slice of it (`aida list approved`, `aida list --type bug`, `aida list --parent EPIC-12`). The positional status shortcut (`aida list open`) is the fast path.

**Don't reach for it when** — you want to know *why* a specific spec is in some state (that's `aida show` / `aida why`), or you want full-text search (that's `aida search` / `aida grep`). And don't conclude a backlog is "empty" from a *filtered* list — scan the open set across all types before declaring done; a filter that returns nothing often means the filter, not the backlog, is empty.

**Key options (rationale only).**
- the positional `[STATUS]` — `aida list open` / `closed` are aliases (`open` = Draft/Approved/Planned/InProgress/NeedsAttention). The fastest way to "what's live."
- `--tags 'prefix:*'` — the trailing-`*` prefix-glob is how you query a whole namespace (`aida list --tags 'aida:queue:*'`). Without it you're matching an exact tag.
- `--all` vs `--archived` — `--all` is *both* archived and live; `--archived` is *only* archived (for auditing the archive itself). They're different questions.
- `--sync` — pull the store from origin before listing. Opt-in because the fast local path is the common case; reach for it when collaborating or when another machine/session may have written.
- `--parent <ID>` — "what's still open under this EPIC" — composes with the other filters, the everyday rollup query.
- `--no-scope` — bypass your role's scope filter. Needed when a subsystem-scoped role is hiding specs you actually want to see.

**Gotchas.** Archived ≠ a status — it's a *view flag* orthogonal to status, so a freshly-Completed spec is still visible (not archived) until someone archives it. If a queued spec is mysteriously absent from `list`, check whether it got archived (it'll still show in `queue list`, which ignores the archive flag — that split surprises people).

**Chains with** — you `list` to find an ID, then `show` it, `edit` it, or `queue work` it.

---

### `aida show`

**One line** — the full detail view of one spec.

**Mental model.** Where `list` is breadth, `show` is depth on a single spec: its fields, description, acceptance criteria, relationships, and — by default — its **git linkage** (the commits, files, branch, and PR that reference it). That git section is what makes `show` more than a database dump: it answers "what code actually serves this spec."

**Reach for it when** — you're about to work, review, or reason about a specific spec and need its full contract; or you want to see what's already shipped against it.

**Don't reach for it when** — you only need the fields and not the (slower) git scan — pass `--no-git`. Or you want the whole subtree, not one node — that's `--tree`.

**Key options (rationale only).**
- `--card` (+ `--brief` / `--full`) — renders a boxed "spec card" instead of the linear view. The pickup skill prints this at session start so the spec's contract stays in scrollback. `--brief` is the one-liner for scripts; `--full` is the no-truncation deep dive.
- `--no-git` — skip the git-linkage scan. The right default for read-only/scripted contexts that only need the fields; meaningfully faster on specs with many referencing commits.
- `--tree` (+ `--depth`) — show the spec *and its descendants* as an indented hierarchy. The quick "what's under this EPIC" without leaving `show`.
- `--rels` / `--relations` — force every relationship edge to print. Without it, `show` summarizes (lists edges when few, otherwise a count + pointer to `rel list`) to keep the view readable on hub specs.
- `-c/--comments` — inline the comment bodies (design discussion, doc seeds) rather than just the count.

**Gotchas.** The git-linkage section is the slow part; if `show` feels sluggish in a loop, you almost certainly want `--no-git`.

**Chains with** — the natural stop between `list` (found it) and acting (`edit` / `queue work` / `comment add`).

---

### `aida done`

**One line** — the newcomer-friendly "I finished it."

**Mental model.** `done` exists for one reason: so a new user doesn't have to learn `edit --status completed` (and the done-vs-completed subtlety) on day one. It's a deliberate *on-ramp* — the simple verb that does the obvious thing.

**Reach for it when** — you're early in your AIDA fluency and just want to mark a task finished without the lifecycle vocabulary.

**Don't reach for it when** — you're inside the real lifecycle. On a project with branches, PRs, and review, "finished" is a sequence (`queue done` → branch/PR → review → **merge auto-bumps to Completed**), and reaching for `done` short-circuits the precise vocabulary that the rest of the system relies on. As you gain fluency you'll stop using `done` and use the precise verbs — that's expected, not a failure of `done`.

**Gotchas.** Because it's the simple shortcut, it doesn't carry the nuance of *where* in the lifecycle you are. On a real pipeline, prefer `aida queue done` (work finished on a branch → **Done**) and let the merge earn **Completed**.

**Chains with** — for fluent users, superseded by `aida queue done` + the merge auto-bump (Chapter 4).

---

### `aida edit`

**One line** — change a spec's fields after creation.

**Mental model.** `edit` is the general-purpose mutator for everything `add` set: title, description, type, priority, tags, status, links. The one field with real semantics is **`--status`** — moving a spec along the lifecycle — and the one ergonomic trap is **tags**.

**Reach for it when** — refining a spec (sharpen the description, fix the type), re-linking it, or — for the disposing seat — moving it through the lifecycle (`--status approved`).

**Don't reach for it when** — you want to *add or remove* a tag: use `--add-tag` / `--remove-tag`, **never** `--tags` (which *replaces the whole set* — a classic way to silently wipe a spec's tags). And don't use `edit --status` for the states that have dedicated verbs: **NeedsAttention** is set via `aida punt` (with a reason), not `edit`; **Completed** is normally *earned* by a merge, not hand-set. Hand-setting `completed` is the override for when the auto-bump missed (then prefer `aida db reconcile-status`).

**Key options (rationale only).**
- `--add-tag` / `--remove-tag` (repeatable) — the *safe* tag edit. Adding a present tag or removing an absent one is a harmless no-op, so they're scripting-safe.
- `--tags` — **replaces** the entire tag set. Use only when you genuinely mean "these are now the tags, forget the rest." It conflicts with `--add-tag`/`--remove-tag` precisely to stop you mixing the two mental models.
- `--status` — the lifecycle mover. Approval (Draft→Approved) is authority-gated on multi-agent projects; status transitions are where the governance lives.
- `--blocked-by` (repeatable) — add dependency edges after creation. Same edge `add --blocked-by` makes, just later.

**Gotchas.** Refinements you make in a *comment* do **not** bind an implementer — only the description / acceptance criteria do. If a design decision must be honored, `edit` it into the acceptance criteria; don't leave it in a comment and assume it'll be followed.

**Chains with** — the disposing seat's `edit --status approved` is step 2 of the journey (capture → **approve**). Everyone's everyday refinement tool.

---

## Where to go next

You now have the floor: set up, capture, view, refine, finish. The next layers:
- **[Chapter 2 — Specs](02-specs.md)**: the graph itself — relationships, traces, search, archive, the quality lens.
- **[Chapter 3 — Work & autonomy](03-work-autonomy.md)**: the queue, backlog grooming, and letting agents drain work for you.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: PRs, review, merge, and the done-vs-completed-vs-released distinctions in full.
