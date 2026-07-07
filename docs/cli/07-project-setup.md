# Chapter 7 — Project setup

The one-time and once-in-a-while plumbing: the commands that *establish* an AIDA project and keep its machinery healthy, as opposed to the daily verbs that *use* it. You touch most of these once at `init` time and then only when something drifts — a clone needs its own identity, the scaffolded skills fall behind the binary, the ID format needs changing, or `doctor` has to heal multi-agent state. Think of this chapter as "the project's settings panel," not its workflow.

> Manual contract reminder: rationale, not flag tables. `aida <command> --help` is the source of truth for exact flags and defaults — we cover only the options whose *why* isn't obvious from their name.

---

### `aida init`

*(Covered deeply in [Chapter 1](01-getting-started.md#aida-init).)* The one-time bootstrap that turns a git repo into an AIDA project — store + cache + MCP registration + scaffolding, plus first-machine-global defaults. Everything else in this chapter either *re-runs a slice* of what `init` set up (`scaffold`, `memories`, `statusline`) or *configures* it after the fact (`config`, `type`, `feature`, `node`). If you're reading Chapter 7 to set up a project, start there.

---

### `aida remote`

*(Covered in [Chapter 4](04-git-lifecycle.md#aida-remote).)* The guided "wire up an `origin` for a project that has none" bootstrap. It lives in the git-lifecycle chapter because its payoff is making `aida push`/`pull` work, but it's genuinely a *setup* step — reach for it right after `init` on a fresh project that said "no origin — skipping," then never again.

---

### `aida scaffold`

**One line** — manage the Claude Code furniture `init` wrote (skills, commands, hooks, MCP config, templates) after the fact.

**Mental model.** `init` scaffolds once; the *binary* keeps evolving. `scaffold` is the family that reconciles the on-disk scaffolding against the binary's *current* embedded templates: `status` reports drift, `diff` shows it, `upgrade` heals it **category-aware**, `apply` (re)writes it, `extract` pulls templates out for hand-customization. The key insight is the category model — AIDA-owned template files (skills/commands/hooks) are safe to overwrite; user-owned seed files (CLAUDE.md, AGENTS.md) are not — which is exactly why `upgrade` exists instead of a blunt `apply --force`.

**Reach for it when** — you upgraded the `aida` binary and want the project's skills/commands to track the new version (`scaffold status` → if drifted, `scaffold upgrade`); or you want to fork a template to customize it (`scaffold extract`).

**Don't reach for it when** — you want to *initialize* a project (that's `init`); or you want to blast every file back to the embedded version regardless of local edits — `apply --force` will clobber your hand-edited CLAUDE.md, whereas `upgrade` respects the category boundary. Prefer `upgrade`.

**Key options (rationale only).**
- `upgrade` vs `apply --force` — the whole point of `upgrade` is *blast-radius control*: template files get overwritten, seed files (your CLAUDE.md edits) are left alone, missing files are created. `apply --force` has no such conscience. Reach for `upgrade` unless you specifically want the sledgehammer.
- `diff` — exits non-zero on drift, so it pairs with `status`: `status` says "N modified," `diff` shows you *what* before you decide to `upgrade`. CI/pre-commit-hookable.
- `extract` — materializes the embedded templates to disk so you can edit them; the deliberate "I want to diverge from the defaults" path.

**Gotchas.** `status` reporting "modified" on a file you *intended* to edit (CLAUDE.md) is expected, not an error — that's a seed file; drift there is your content, and `upgrade` will correctly leave it. Don't reflexively "fix" it.

**Chains with** — runs after a binary `aida upgrade`; pairs `status` → `diff` → `upgrade`.

---


**Codex parity.** `aida scaffold codex-prompts` writes the AIDA command set as Codex CLI custom prompts (default `~/.codex/prompts`) so `/aida-...` works inside a Codex session — generated from the same embedded masters as `.claude/commands/`; Claude-only commands are excluded with a stated reason; existing files are never overwritten without `--force`.

### `aida config`

**One line** — set the project's ID scheme (prefix format, numbering, digits) and a couple of user-level preferences.

**Mental model.** `config` governs *how spec IDs are shaped* — single-level (`<spec-id>`) vs two-level (`<spec-id>-042`), per-type vs global numbering, digit width — plus a few orthogonal odds and ends (`config user` for `~/.aida` preferences, `config hints` for the inline workflow-hint toggle). The ID-format settings are decisions you want to make **early**, because `config migrate` exists precisely because changing them later is a renumbering chore.

**Reach for it when** — at project setup you want a different ID shape than the default; or later you want to toggle the inline workflow hints (`config hints false`) or stash a preferred node id / fallback email (`config user`).

**Don't reach for it when** — you want to add a new *requirement type* (that's `aida type`) or a *feature prefix* (that's `aida feature`) — `config` shapes the ID, those define what gets an ID.

**Key options (rationale only).**
- `format` / `numbering` / `digits` — the three ID-shape knobs. Decide them at setup; they compose (two-level + global counter + 3 digits = `<spec-id>-042`). Changing them post-hoc means `migrate`.
- `migrate` — rewrites existing IDs to a new format. The escape hatch for "I picked the wrong scheme," not a routine command — it touches every spec.
- `hints` — toggles the state-transition hints printed inline (queue drained → open PR, etc.). `false` quiets them project-wide; the `AIDA_HINTS=false` env var overrides per-shell without writing config.
- `user` — writes `~/.aida/preferences.toml` (preferred node id, fallback email). Machine-global, not per-project — it seeds defaults for future `init`s.
- `menu` — a navigable TUI listing every configurable item — per row the knob's name, current value, built-in default, where it was set (scope: `.aida/config.toml` / env / built-in default), and a one-line explanation. The visual companion to `config show`: same resolved surface (it reuses the same policy registry), one scrollable screen, arrow-key navigation (↑/↓ or j/k, PgUp/PgDn, g/G, q/Esc). Read-only for now — edit knobs with the matching `config` subcommand or your `config.toml`. Needs a TTY; without one it prints a pointer to `config show` and exits cleanly.

**Gotchas.** `config user` is *machine-global* (`~/.aida/`), while `config show`/`format`/`menu`/etc. are *project-local*. Same command noun, two scopes — don't expect `config user` to behave per-project.

**Chains with** — set once near `init`; `migrate` is the rare follow-up. Pairs with `type` / `feature` to define the ID *taxonomy*.

---

### `aida type`

**One line** — manage the set of requirement types (the 19 built-ins, plus any project-specific ones).

**Mental model.** Types are the spec taxonomy — `task`, `bug`, `epic`, the ADR family, etc. — and each type drives an ID prefix and (sometimes) a docs-projection layer. `aida type` lets you *list* the available types and *add* / *remove* project-specific ones when the built-in 19 don't capture a category your project reasons about.

**Reach for it when** — your project has a recurring spec category the built-in types don't name and you want first-class support for it.

**Don't reach for it when** — `task` already covers it. The built-in 19 are deliberately broad (`task` is the catch-all for chores/docs/tooling); adding a custom type you'll use twice is taxonomy bloat. Reach for this only when the category is load-bearing and recurring.

**Gotchas.** `type list` is the quickest way to see the canonical type set and their prefixes if you've forgotten which `--type` value to pass to `aida add`.

**Chains with** — defines the vocabulary that `aida add --type` consumes.

---

### `aida feature`

**One line** — manage *feature categories* (named groupings with their own ID prefix), distinct from types.

**Mental model.** A *feature* is an organizational bucket with a prefix — a way to give a coherent area of the product its own ID namespace, orthogonal to the type taxonomy. `aida feature add` creates one with a prefix; `list`/`show`/`edit` manage them. Don't confuse it with `--type`: type is *what kind of spec*, feature is *what area of the product*.

**Reach for it when** — you want specs in a subsystem to share a recognizable prefix and group cleanly in views.

**Don't reach for it when** — a tag would do. Features are heavier (they mint a prefix); for a lightweight grouping, `--tags` is cheaper and reversible. Reach for a feature only when the grouping is structural and long-lived.

**Gotchas.** `--feature` on `aida add` takes a feature *name*, not a type — a common mix-up. Define the feature here first, then reference it by name.

**Chains with** — set up at project-shaping time; referenced by `aida add --feature`.

---

### `aida node`

**One line** — manage this clone's *node identity* in the shared multi-clone registry.

**Mental model.** Every clone of a distributed AIDA project gets a unique **node id**, and that id is the namespace for node-aware spec ids (`FR-<node>-NNN`) until the merge gate promotes them to short agreed-ids. `aida node` is how a clone *acquires* its id (a CAS push-loop on the shared registry), *shows* it, *lists* all registered clones, or *releases* it. `aida init` does the acquire for you on a normal bootstrap; you reach for `node` directly only in the fresh-clone / multi-machine cases.

**Reach for it when** — you cloned an existing AIDA project on a new machine and need to *write* new spec ids (a read-only clone auto-attaches the store, but minting ids needs a node — `aida node acquire`); or you want to see which clones are registered (`node list`, current marked `*`).

**Don't reach for it when** — you ran the full `aida init` (it acquired a node already); or you only *read* the store (reads don't need a node). And don't `release` casually — releasing does **not** invalidate ids already issued by that node, and the id is never reused.

**Key options (rationale only).**
- `acquire` — the CAS-loop claim of the next sequential node id; defaults pull `git config user.email` and the hostname for the registry stamp. The one you need on a fresh clone that will *write*.
- `release` — removes the registry entry but is deliberately *non-destructive to issued ids*. Reach for it only when retiring a clone for good.

**Gotchas.** "Read works but I can't add a spec" on a fresh clone is the classic node-missing symptom — the store auto-attaches for reads, but writes need `aida node acquire` (or a full `init`).

**Chains with** — part of fresh-clone setup; the node id feeds `aida db merge-gate`, which promotes node-aware ids to their short agreed form.

---

### `aida rules`

**One line** — sync Claude Code path-gated rule files from the spec graph, so a spec's scope lands in an agent's context exactly when it edits the relevant code.

**Mental model.** For each *active* spec (In Progress or Done) that has `trace:` markers in code, `rules sync` emits `.claude/rules/aida-specs/<SPEC-ID>.md` with a `paths:` glob matching the traced files. Claude Code loads that rule on-demand when an implementer reads or edits one of those files — so the spec's acceptance criteria arrive *just-in-time*, not as always-on context bloat. It's the substrate-as-bouncer idea applied to context delivery: the rule fires on the file, not on a prompt nobody read.

**Reach for it when** — you want in-flight specs' scope to auto-surface to coding agents working the relevant files; run `rules sync` after specs go active or trace comments move.

**Don't reach for it when** — there's no active traced work (it'll write nothing useful); or you want context loaded *unconditionally* — that's CLAUDE.md / an always-on import, and the whole point of `rules` is to *avoid* that.

**Gotchas.** `sync` is reconciling, not additive — it *removes* rule files whose spec is no longer active. So a rule disappearing after a spec completes is correct behavior, not a bug.

**Chains with** — driven by the `trace:` graph (Ch.2); re-run as active work shifts.

---

### `aida memories`

**One line** — check the local starter-memory pack against the binary's embedded master and report drift.

**Mental model.** `aida init --with-memories` writes a generic discipline memory pack; the binary carries the master copy. `aida memories check` is a **read-only** drift report — which pack files are missing, stale, edited, or up-to-date — and it tells you the one fix it recommends (`aida init --with-memories --refresh`). It never writes; it's a diagnostic, not a sync.

**Reach for it when** — you upgraded the binary and want to know whether the embedded memory pack moved ahead of your local copy.

**Don't reach for it when** — you want to *apply* the update. `check` only reports; the fix is the `init --with-memories --refresh` it points you at (which overlays newer versions of files you haven't edited).

**Gotchas.** "edited" in the report is not "broken" — a file you intentionally customized shows as edited, and `--refresh` will correctly *preserve* it. The report distinguishes edited (yours, kept) from stale (theirs moved, refreshable).

**Chains with** — pairs with the binary `aida upgrade`; the recommended fix is `init --with-memories --refresh`.

---

### `aida statusline`

**One line** — the sub-50ms one-line project+role summary for shell prompts and Claude Code's `statusLine.command`.

**Mental model.** `aida statusline` prints a compact segment — `aida · <project> · role:<name> · @SPEC · q:N · cache:fresh|stale` — cheap enough (reads the cache + the queue YAML) to run on every prompt render. `statusline setup` is the *installer*: it prints (or, with `--install`, writes) the right `settings.json` entry so the segment shows up in Claude Code.

**Reach for it when** — you want the AIDA-aware segment in your shell prompt or Claude Code statusline (`statusline setup --install` once); or a tool wants the one-line state programmatically.

**Don't reach for it when** — you want the *full* "what's going on here" view — that's `aida status` (Ch.8), which is richer and not prompt-cheap. `statusline` is deliberately terse.

**Key options (rationale only).**
- `setup --install` — writes the Claude Code `settings.json` entry for you; without `--install` it just prints the config to paste. Reach for `--install` unless you want to review first.
- `--color auto|always|never` — defaults to `auto` (color only on a TTY, respecting `NO_COLOR`); set `never` when piping the segment into a tool that mangles ANSI.

**Gotchas.** `q:N` (queue depth) is keyed off the *active role's* routed queue and is **omitted when zero** — an absent `q:` segment means an empty queue, not a broken statusline. And the queue is keyed off the shell's `$USER`/`$AIDA_USER`, so a surprising `q:` value usually means the wrong user identity, not a bug.

**Chains with** — a setup-time one-shot (`setup --install`); thereafter rendered by your prompt. The fuller counterpart is `aida status`.

---

### `aida doctor`

**One line** — diagnose (and optionally heal) multi-agent state drift across the store, registry, and trace graph.

**Mental model.** `doctor` is the project's **fsck**. Multi-agent, multi-clone use accrues subtle drift — orphaned id blocks from clones that never registered, duplicate spec ids from bad imports, dangling relationship targets, dead trace comments pointing at deleted specs, STORY/BUGs missing acceptance headings. `doctor` runs focused checks (or `fsck` for the full sweep) and, **only with `--heal`**, applies safe fixes. Default is read-only — it *reports* the drift, you decide whether to heal.

**Reach for it when** — something feels off in a multi-clone project (ids colliding, relationships pointing nowhere, a clone's blocks orphaned), or periodically as hygiene. `aida doctor fsck` is the "run everything and tell me" entry point.

**Don't reach for it when** — you want a *content* lens (what specs exist, their status) — that's `list`/`status`. `doctor` checks *structural integrity*, not project state. And don't pass `--force` casually: it permits riskier destructive fixes (branch deletion).

**Key options (rationale only).**
- `--heal` — the gate between *diagnose* and *repair*. Without it `doctor` never writes; with it, only **safe** fixes apply. This read-only-by-default posture is deliberate — you see the drift before anything changes.
- `--force` — escalates to riskier fixes (e.g. branch deletion). The explicit "yes, I accept destructive repairs" opt-in; don't reach for it on a hunch.
- `--since <ref-or-date>` — exempts specs completed before a cutoff from the completed-without-commit check, to quiet noise on legacy history predating trace conventions. The way you stop `doctor` nagging about pre-AIDA commits.
- `--json` — machine-readable output for wiring `doctor` into CI / a health dashboard.
- `--fix-sandbox` — a standalone guided printer (not a drift check): brings the OS sandbox (bubblewrap write-confinement) up on *this* host. It detects the current state, prints the exact copy-pasteable steps that host needs — install, the runtime + persist sysctl to permit unprivileged user namespaces, the `[contained] os_wrap` opt-in, and the verify command — with sudo steps clearly marked "run this yourself", then runs a non-sudo self-test smoke. It never runs sudo for you. The single command to run when standing up confinement on a new machine. (Full reference: `docs/agents/claude-bubblewrap-sandbox.md`.)
- the focused subcommands (`verify-relationships`, `validate-trace-comments`, `scrub-collisions`, `repair-stale-blocks`, `migrate-counter-scope`, `convention-check`) — run *one* category when you know what you're chasing, instead of the full `fsck` sweep.

**Gotchas.** `validate-trace-comments` and `verify-relationships` have their *own* destructive opt-ins (`--strip-dangling`, `--repair`) — read the per-subcommand `--help`, because the top-level `--heal` isn't the only write-gate in this family. `fsck` exits non-zero if any check found a problem, so it's CI-suitable as a gate.

**Chains with** — the structural counterpart to the content lenses; pairs with `db` (Ch.10) for store-level repair.

---

### `aida defer`

**One line** — park a spec as primed/conditional work — hidden from the default view, but with a recorded *revisit trigger*, not filed away like archive.

**Mental model.** `defer` is a **view-level flag** (like archive) that does *not* touch the lifecycle status machine. The one thing distinguishing deferred from archived is `--until`: a free-text condition that says *what brings the spec back* ("when a slice verb ships"). Archived is retrospective ("done with this, filed"); deferred is prospective ("not yet, but here's the trigger"). Deferred rows drop out of `list`/`search`/`history` by default and resurface with `--deferred` (only) or `--all` (union).

**Reach for it when** — you've captured a real idea but it's blocked on a future condition and you don't want it cluttering the open-work view — yet you also don't want to lose the *why it'll come back*. The EPIC-shaped "smallest slice now, revisit-trigger filed" discipline lands here.

**Don't reach for it when** — the spec is genuinely *done with* (that's `archive`, Ch.2); or it's actually *blocked by another spec* — that's a `--blocked-by` relationship, which the graph tracks, not a view flag. Defer is for conditions the graph can't express.

**Key options (rationale only).**
- `--until <condition>` — the whole reason to defer rather than archive. Record the revisit trigger; a deferred spec with no `--until` is just a hidden spec you'll forget. Always set it.

**Gotchas.** Deferred is *orthogonal to status* — a deferred Approved spec is still Approved, just hidden. Don't read "deferred" as a lifecycle state; it's a lens flag stacked on top of whatever status the spec holds.

**Chains with** — the prospective sibling of `archive` (Ch.2); surface deferred work with `list --deferred` when its trigger fires.

---

### `aida export`

*(Covered in [Chapter 10 — Storage & data](10-storage.md).)* Tree-shaped export of a requirement hierarchy for sharing between projects (`export --format tree --id FOLDER-001 -o templates.json`). It's *setup-adjacent* — exporting a template tree to seed another project is a setup act — but it lives with the storage commands; pair it with `aida import` on the receiving side.

---

### `aida import`

*(Covered in [Chapter 10 — Storage & data](10-storage.md).)* The receiving side of `export`: lands a tree JSON under a chosen `--parent`, with `--on-conflict skip|rename|replace` deciding how clashes resolve. Reach for it when bootstrapping a new project's spec hierarchy from another's exported template.

---

### `aida docs`

*(Covered in [Chapter 9 — Integrations & servers](09-integrations.md).)* Projects the requirements graph into a layered docs tree (`docs build`, `docs check`, the embedded `docs glossary`). It's setup-adjacent — you wire `docs check` into CI at project-setup time so the docs projection can't drift from the graph — but the command family lives with the other projection/server surfaces.

---

## Where to go next

You've set the project up and can keep its machinery healthy. From here:
- **[Chapter 1 — Getting started](01-getting-started.md)**: the daily verbs on top of this plumbing (`init` lives there in full).
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: `remote` and the two-leg sync that the setup enables.
- **[Chapter 10 — Storage & data](10-storage.md)**: `db` / `cache` — the store internals `doctor` repairs, plus `export`/`import`.
- **[Chapter 11 — Working on AIDA itself](11-dev.md)**: if you're *developing* AIDA rather than using it.
