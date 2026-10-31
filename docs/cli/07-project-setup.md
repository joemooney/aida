# Chapter 7 — Project setup

The one-time and once-in-a-while plumbing: the commands that *establish* an AIDA project and keep its machinery healthy, as opposed to the daily verbs that *use* it. You touch most of these once at `init` time and then only when something drifts — a clone needs its own identity, the scaffolded skills fall behind the binary, the ID format needs changing, or `doctor` has to heal multi-agent state. Think of this chapter as "the project's settings panel," not its workflow.

> Manual contract reminder: rationale, not flag tables. `aida <command> --help` is the source of truth for exact flags and defaults — we cover only the options whose *why* isn't obvious from their name.

---

### `aida init`

*(Covered deeply in [Chapter 1](01-getting-started.md#aida-init).)* The one-time bootstrap that turns a git repo into an AIDA project — store + cache + selected agent scaffolding, plus first-machine-global defaults. MCP registration is explicit opt-in with `--with-mcp`; the CLI is the default surface for CLI-capable agents. See the [operator route](../operator-setup.md) for adopted projects and sibling-store setup. Everything else in this chapter either *re-runs a slice* of what `init` set up (`scaffold`, `memories`, `statusline`) or *configures* it after the fact (`config`, `type`, `feature`, `node`). If you're reading Chapter 7 to set up a project, start there.

---

### `aida remote`

*(Covered in [Chapter 4](04-git-lifecycle.md#aida-remote).)* The guided "wire up an `origin` for a project that has none" bootstrap. It lives in the git-lifecycle chapter because its payoff is making `aida push`/`pull` work, but it's genuinely a *setup* step — reach for it right after `init` on a fresh project that said "no origin — skipping," then never again.

---

### `aida scaffold`

**One line** — manage the Claude Code furniture `init` wrote (skills, commands, hooks, templates, and MCP config when explicitly requested) after the fact.

**Mental model.** `init` scaffolds once; the *binary* keeps evolving. `scaffold` is the family that reconciles the on-disk scaffolding against the binary's *current* embedded templates: `status` reports drift, `diff` shows it, `upgrade` heals it **category-aware**, `apply` (re)writes it, `extract` pulls templates out for hand-customization. The key insight is the category model — AIDA-owned template files (skills/commands/hooks) are safe to overwrite; user-owned seed files (CLAUDE.md, AGENTS.md) are not — which is exactly why `upgrade` exists instead of a blunt `apply --force`.

**Reach for it when** — you upgraded the `aida` binary and want the project's skills/commands to track the new version (`scaffold status` → if drifted, `scaffold upgrade`); or you want to fork a template to customize it (`scaffold extract`).

**Don't reach for it when** — you want to *initialize* a project (that's `init`); or you want to blast every file back to the embedded version regardless of local edits — `apply --force` will clobber your hand-edited CLAUDE.md, whereas `upgrade` respects the category boundary. Prefer `upgrade`.

**Key options (rationale only).**
- `upgrade` vs `apply --force` — the whole point of `upgrade` is *blast-radius control*: template files get overwritten, seed files (your CLAUDE.md edits) are left alone, missing files are created. `apply --force` has no such conscience. Reach for `upgrade` unless you specifically want the sledgehammer.
- `diff` — exits non-zero on drift, so it pairs with `status`: `status` says "N modified," `diff` shows you *what* before you decide to `upgrade`. CI/pre-commit-hookable.
- `extract` — materializes the embedded templates to disk so you can edit them; the deliberate "I want to diverge from the defaults" path.

**Seed ownership.** An existing `README.md` belongs to the project: authored content is not scaffold drift, and ordinary `upgrade` and `refresh` preserve it. `scaffold diff README.md` also skips the starter comparison and exits 0. A missing README is reported by `status` and by default or explicit `diff` (exit 1); `upgrade` creates the starter when it is absent. Pack-only `refresh` neither creates nor replaces README. Explicit `upgrade --force` can replace it. `CLAUDE.md` import checks and `AGENTS.md` managed-block checks still report AIDA-owned drift, as do stale template files. <!-- trace:BUG-A65B4BF5902D469995F816EFD3A27661-001 | ai:codex -->

**Chains with** — runs after a binary `aida upgrade`; pairs `status` → `diff` → `upgrade`.

---


**Codex parity.** `aida scaffold codex-prompts` writes legacy Codex prompt-body files (default `~/.codex/prompts`) generated from the same embedded masters as `.claude/commands/`; Claude-only commands are excluded with a stated reason; existing files are never overwritten without `--force`. Current Codex CLI releases do **not** discover those files as `/aida-...` slash commands. For an interactive Codex session, use the scaffolded `.codex/skills/` pack through `/skills` or `$aida-capture` / `$aida-...`, or run the matching `aida ...` CLI verb directly. <!-- trace:BUG-1095 | ai:codex -->

**Keeping every agent pack current.** `aida scaffold refresh` (equivalently `aida init --refresh`) brings every *installed* pack level with the binary's embedded templates — `.claude/skills/`, `.claude/commands/`, `.codex/skills/`, `.antigravity/skills/`, `~/.codex/prompts/`, and starter roles under `~/.aida/roles/` — while preserving your edits. A pack file whose body still hashes to the scaffold checksum it was written with is overlaid; a file you edited, a file with no scaffold marker, and a symlinked destination are left exactly as they are; a pack you never installed is not created. For an installed starter role, refresh only appends shipped guidance when `system_prompt` is absent; it preserves all existing text and never replaces a user-supplied prompt. This is the no-`--force` delivery path for a template or starter-guidance fix, and it is the same contract the starter memory pack has always had — so a fix reaches existing Claude, Codex and Antigravity seats through one mechanism instead of one per vendor. A skill AIDA started shipping after a skill pack was installed is added to that pack on refresh. Each skill pack directory keeps a `.aida-delivered` manifest of the skills AIDA has put there; a skill you delete from a pack is recorded there as an opt-out, and neither refresh, `aida scaffold upgrade`, nor a re-run of `aida init` brings it back (remove its line from the manifest to get it back). A pack installed before the manifest existed is seeded from the skills on disk and refresh adds nothing to it, because a missing skill may be one you deleted; an explicit `aida scaffold upgrade` (or `aida init`) installs the missing skills once, and from then on deletions are honoured. To decline such a skill instead, move its line under the manifest's `# opted-out` heading. A project on the `minimal` or `memory-lane` footprint never receives new skills from a refresh. <!-- trace:TASK-1503 | ai:claude --> A memory-lane project created before the footprint was saved is recognised from its skill packs (only the memory-lane skills present), so refresh and `aida doctor` do not push it toward the full skill set. Init, apply, upgrade and refresh never write a skill file through a symlinked directory (the vendor directory, skill pack, skill directory or a folder inside a skill); they skip it and say so, and `aida doctor` treats those files as yours rather than as drift. <!-- trace:BUG-1645 | ai:claude -->

**Refreshing stale git hooks only.** A store write refuses to run while an AIDA-generated git hook (one with the `# AIDA Generated` header) is not this build's shipped hook, because git runs it while the write holds the store lock. The refusal lists every such hook and names `aida scaffold upgrade --only git-hooks`, which copies each stale hook beside itself as `<name>.aida-stale-<seconds>` and replaces it with this build's hook. It touches no other file: custom hooks without the header, symlinked hooks, README.md, CLAUDE.md, AGENTS.md and drifted skills are left alone. Add `--dry-run` to see what it would replace. A refused hook it cannot replace (a symlink, which is never written through, or an AIDA-headed hook under a name AIDA ships no hook for) is listed with what to do instead, and the command exits non-zero. It needs no AIDA store, so it runs from anywhere with `--project-root`. <!-- trace:BUG-1977 | ai:claude --> The same command fixes an AIDA hook that has lost its executable bit: git silently skips such a hook, so it counts as stale even when its content is current, and the refresh sets it to 0755 (no backup is needed when only the mode changes). `aida doctor` (category `git-hooks`) reports both kinds. Every command that writes AIDA hooks (`aida init`, `aida scaffold apply`, `aida scaffold upgrade`) sets 0755 explicitly. <!-- trace:BUG-2009 | ai:claude --> If `core.hooksPath` points the hooks outside the repository's git directory, the command says so first: a shared hooks directory serves every repository that uses it, and a tracked one shows the backups as untracked files. A custom hook that runs `aida` is refused in the same message as the stale generated hooks, so one attempt lists everything to fix. <!-- trace:BUG-2003 | ai:claude -->

### `aida adoption`

**One line** — adopt AIDA in a clone without a single AIDA file on the product branch, and remove it again.

**Mental model.** Shared project settings and guidance live in an `adoption/` namespace on the orphan `aida-store` branch, never on a code branch. Each worktree gets a *projection*: the plane's settings rendered into the git-ignored `.aida/config.toml`, its guidance into `.aida/adoption/guidance/`, and a manifest recording the plane revision and a hash of every file the projection owns. The projection is excluded through `.git/info/exclude`, so the product's tracked `.gitignore` is never edited. Only non-executable policy settings can come from the plane; commands, hooks, credentials and paths stay in the trusted default-branch config or `~/.aida`. A clone finds the plane without any marker file: its own `aida-store` branch, or the one remote copy of it. A clone that keeps its plane elsewhere names it in the clone-local `aida-adoption-locator.toml` in the Git common directory.

- `status` — which plane revision the projection holds: current, stale, damaged (an owned file was edited or removed), or not yet projected.
- `refresh` — project the current plane revision. A launch through `aida agent` does this too.
- `bootstrap` — the CI and agent-environment entry point: projects the plane only when it is at exactly the commit given with `--revision`, then verifies the projection before anything uses it. A different revision, a short id, a missing plane, or a local config that would keep the settings out each refuses. CI checkouts such as `actions/checkout` fetch only the commit under test, so a job must fetch the plane first — `git fetch origin aida-store:refs/remotes/origin/aida-store` — or `bootstrap` finds no plane and refuses.
- `inventory` — the dry-run migration for a project that already tracks AIDA scaffolding. It reads the committed product branch and lists each AIDA file: removable (only generated Markdown outside `.claude/`, `.codex/`, `.agents/`, `.antigravity/` and the `.aida/discipline/` pack — in practice the `docs/agents/*.md` guides — that is byte for byte this build's output and that no tracked file which stays cites by path; publish it as plane guidance and launches deliver it from there), review first (an AIDA header but different bytes, which may be local edits or a copied header), or keep (the `.aida/discipline/` pack, which the scaffolded `CLAUDE.md` imports and AIDA commands and skills read; generated guidance some kept file cites; hooks, commands, skills, harness settings and Codex/Antigravity artifacts, which the plane never carries; authored `AGENTS.md`/`CLAUDE.md`; a `.gitignore` with AIDA entries). Product files such as a `README.md` that still matches AIDA's starter, or a `.gitignore` with no AIDA entries, are not listed. A tracked `.aida/config.toml` is split into the policy settings the plane accepts, printed as a ready `adoption/settings.toml`, and the settings that stay local or trusted. It writes nothing; removing tracked files is a separate, reviewed change to the product branch.
- `uninstall` — remove the projection files the manifest proves it owns, the manifest and its lock, and the AIDA git hooks this build installed, only where a hook's bytes are exactly this build's hook. An edited projection file, an edited or other-version AIDA hook, a symlinked hook and your own hooks are kept and listed. Git hooks are shared by every worktree of the clone, so removing them affects all of them, and neither `refresh` nor `aida agent` reinstalls them. The store, the plane, the locator, node identity and every product file stay. `--dry-run` lists what would go.

**Reach for it when** — you want AIDA in a repository whose history and tracked tree must stay untouched; CI needs the exact reviewed configuration before a job runs (`bootstrap`); you are planning to move an existing scaffolded project onto the plane (`inventory`); or you are leaving a clone (`uninstall`).

**Don't reach for it when** — the project is happy tracking its scaffolding on the default branch: plain `aida init` and `aida scaffold` keep working there, and none of this changes them.

**Key options (rationale only).**
- `bootstrap --revision` takes a full commit id because CI should name the revision it reviewed, not whatever the branch points at when the job starts.
- `uninstall --dry-run` exists because uninstall deletes files; the dry run lists the same rows without removing anything.

**Gotchas.** `aida init` in an adopted clone takes this path automatically, and `aida init --minimal`, `aida init --centralized`, `aida init --sibling` and `aida init --store-path`, which write product files, refuse there: it attaches the store worktree, projects the plane before anything else could create `.aida/config.toml`, and sets up the node identity, writing no `.gitignore`, config or scaffolding. If the clone shows adoption evidence (a locator, or an `adoption/` namespace) but the plane cannot be read, `init` refuses and writes nothing. A requirement write commits to `aida-store`, which is also the conventional plane, so `status` can read *stale* after an `aida add`; the next launch or `refresh` catches up. Harnesses started outside `aida agent` do not receive plane guidance, and tracked `AGENTS.md`/`CLAUDE.md` are never written to simulate it. `uninstall` leaves the clone adopted, so a later launch projects again.

**Chains with** — `aida init` (which calls it), `aida agent` (which refreshes the projection and delivers its guidance), `aida config` (which refuses to edit a projected file and points at the plane).

---

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

### `aida config transport`

Enable the clone-local `work` profile with `aida config transport enable work`,
inspect it with `aida config transport show`, and restore ordinary push/pull
behavior with `aida config transport disable`. Each supports `--json` and
`--format json`. Settings live in `aida-transport.json` under the common Git
directory, shared by linked worktrees; no tracked project file or store object
is changed. Configuration works before AIDA store/projection initialization.
In `show --json`, `pull_refspec` keeps its net mapping
`refs/heads/aida-store:refs/remotes/origin/aida-store`; `pull_fetch_refspec`
names the staging refspec a pull actually fetches and `pull_after_scan` the ref
updates made once the staged history passes the scan (the human output prints
them as `Pull:` and `Pull after scan:`).

The profile pins a fingerprint of origin's single resolved fetch/push endpoint.
Filesystem endpoints resolve symlinks and parent components in filesystem order;
symlink target changes invalidate the pin. Missing intermediate components,
dangling links and unreadable paths refuse. A missing final repository name is
allowed for planning only when its parent resolves unambiguously. Older profiles
whose lexical path pin differs from the real destination refuse until explicitly
disabled/re-enrolled after inspection.
Multiple endpoints, a different push endpoint, mirror mode, or later endpoint
changes refuse planning. To change the pin, inspect the Git configuration and
explicitly disable/re-enable the profile. Output and persisted profile data
contain the fingerprint rather than URLs or credentials. An optional
`--baseline <full-sha>` explicitly enrolls a reviewed public-safe commit from
the local store history for the later outbound scanner. Enrollment records the
operator's decision; it does not scan or certify that history.

**Boundary: ref-only push; fast-forward-only store pull.** While enabled, `aida push --dry-run`
and `aida pull --dry-run` inspect local metadata without fetching, opening a
store cache, or running lifecycle actions. `--json` also selects planning;
JSON plans explicitly report `executed: false` and never execute a push.
Plans name origin, the exact store refspec executed (a pull fetches
`refs/heads/aida-store:refs/aida-transport/incoming` and lists the post-scan
`after_scan` ref updates), locally known source SHA, refs that
execution could move, flag dispositions, and missing prerequisites. Code legs
are forbidden even when `AIDA_PUSH_DEFAULT=code`; `aida push --store-only` and
`aida pull --store-only` are redundant. Executing pushes also print flag dispositions.
`aida push --message <text>` and
`aida pull --auto` refuse. Ordinary clones with no
profile retain their existing behavior, including pull dry-run fetching.
Explicit store overrides such as `aida --file <path> push` refuse when either
clone has an enabled profile; run from the intended clone without the override.

Actual `aida push` requires an existing attached `aida-store` worktree in the
same clone. It refuses dirty stores, active Git operations, unsupported hooks,
embedded repositories, and partial-clone configuration that could fetch objects
implicitly. Hydrate such clones explicitly outside this command.
Under the store writer lock it validates the tip and
cleanliness, then releases that lock before network I/O. It makes exactly one
push attempt of the pinned commit to `refs/heads/aida-store` at the enrolled
origin endpoint: no code refs, tags, submodule pushes, auto-commits, mailbox
publication, ID allocation, reconciliation, fan-out, or retry. The verified
store-only hooks preserve the identity gate; project code hooks do not run.
Relative filesystem origins are resolved and pinned from the invoking checkout;
the executor uses that absolute destination from the attached store. Credentials
use the shared environment-backed routing helper; HTTP secrets stay out of push
arguments. Cleaning authentication userinfo must not select a new URL rewrite.
Concurrent later store commits remain local for a future push. Remote refusal
or network errors require operator inspection; acceptance can be uncertain.

Actual `aida pull` has the same attached-store, cleanliness, partial-clone,
policy and trust-root prerequisites, checked before any network contact. It
fetches only origin's `refs/heads/aida-store` from the pinned endpoint into the
private staging ref `refs/aida-transport/incoming`, with no tags, submodules,
`FETCH_HEAD`, configured refmap or maintenance; fetched objects can remain in
the local object database after a refusal, but no accepted ref moves. Under the
store writer lock it rechecks the local tip and cleanliness, requires a
fast-forward (divergence refuses; nothing is rebased or merged), and scans the
incoming history from the enrolled trust root exactly as a push of that tip
would be scanned. Incoming commits by any identity other than the reviewed
pseudonym, or containing policy matches, therefore refuse. Only then does it
advance `refs/remotes/origin/aida-store`, update the attached store's
index/worktree and advance `refs/heads/aida-store`, each with an old-value
check. A local store already ahead of origin moves only the remote-tracking
ref. A pull does not commit, publish mail, allocate IDs, reconcile, refresh the
cache, record a push receipt or fan out. An interrupted transition leaves a
private recovery journal: push refuses until `aida pull` finds the store
cleanly at one of the two journaled commits; any other state refuses and names
both commits so the operator can restore one.
Raw hook/remote diagnostics are suppressed to avoid exposing identifiers.
The profile baseline is not advanced, and a push does not advance the local
remote-tracking ref. Local hook-profile installation/verification is the existing store Git
policy; it does not change the canonical store.

**Sanitization is refusal-only.** A reviewed policy (below) gates every push
and every store write; see [sanitized intake](#aida-add---sanitized-sanitized-intake).
It can only refuse known shapes and listed tokens, so keep work-filed defects
generic. Plans do not validate
cleanliness, hook compatibility, fast-forward eligibility, or content safety. The profile governs only
`aida push/pull`; raw Git and other sync entry points bypass it. There is no
server-side enforcement: the hub does not scan what it receives, so it can
accept unscanned history from another client and forward it to a public
remote. Keep work-filed defects generic and sanitized before they enter any
store that may become public.

**The end-to-end path.** A generic defect filed with `aida add --sanitized` in
the WORK clone becomes one pseudonymous store commit. `aida push` scans it and
publishes exactly that commit to the hub's `aida-store`; another WORK clone of
the hub receives it with `aida pull`. Forwarding from the hub to a public
remote is the operator's own step (raw Git or an ordinary clone) and is not
guarded by this profile: what arrives publicly is exactly what the hub holds.
Only store refs move: a push moves `refs/heads/aida-store` on the hub; a pull
stages the fetch in `refs/aida-transport/incoming`, then moves the clone's
`refs/remotes/origin/aida-store` and fast-forwards its `refs/heads/aida-store`
(the pull plan names these before running). Product branches, mailbox state,
ID counters and other remotes are untouched by the transport.
<!-- trace:STORY-1742 | ai:codex -->

---

### `aida type`

**One line** — manage the set of requirement types (the 19 built-ins, plus any project-specific ones).

**Mental model.** Types are the spec taxonomy — `task`, `bug`, `epic`, the ADR family, etc. — and each type drives an ID prefix and (sometimes) a docs-projection layer. `aida type` lets you *list* the available types and *add* / *remove* project-specific ones when the built-in 19 don't capture a category your project reasons about.

**Reach for it when** — your project has a recurring spec category the built-in types don't name and you want first-class support for it.

**Don't reach for it when** — `task` already covers it. The built-in 19 are deliberately broad (`task` is the catch-all for chores/docs/tooling); adding a custom type you'll use twice is taxonomy bloat. Reach for this only when the category is load-bearing and recurring.

**Gotchas.** `type list` is the quickest way to see the canonical type set and their prefixes if you've forgotten which `aida add --type` value to pass.

**Chains with** — defines the vocabulary that `aida add --type` consumes.

---

### `aida feature`

**One line** — manage *feature categories* (named groupings with their own ID prefix), distinct from types.

**Mental model.** A *feature* is an organizational bucket with a prefix — a way to give a coherent area of the product its own ID namespace, orthogonal to the type taxonomy. `aida feature add` creates one with a prefix; `list`/`show`/`edit` manage them. Don't confuse it with `aida add --type`: type is *what kind of spec*, feature is *what area of the product*.

**Reach for it when** — you want specs in a subsystem to share a recognizable prefix and group cleanly in views.

**Don't reach for it when** — a tag would do. Features are heavier (they mint a prefix); for a lightweight grouping, tags are cheaper and reversible. Reach for a feature only when the grouping is structural and long-lived.

**Gotchas.** `aida add --feature` takes a feature *name*, not a type — a common mix-up. Define the feature here first, then reference it by name.

**Chains with** — set up at project-shaping time; referenced by `aida add --feature`.

---

### `aida node`

**One line** — manage this clone's *node identity* in the shared multi-clone registry.

**Mental model.** Every clone of a distributed AIDA project gets a unique **node id**, and that id is the namespace for node-aware spec ids (`FR-<node>-NNN`) until the merge gate promotes them to short agreed-ids. `aida node` is how a clone *acquires* its id (a CAS push-loop on the shared registry), *shows* it, *lists* all registered clones, or *releases* it. `aida init` does the acquire for you on a normal bootstrap; you reach for `node` directly only in the fresh-clone / multi-machine cases.

**Reach for it when** — you cloned an existing AIDA project on a new machine and need to *write* new spec ids (a read-only clone auto-attaches the store, but minting ids needs a node — `aida node acquire`); or you want to see which clones are registered (`node list`, current marked `*`).

**Don't reach for it when** — you ran the full `aida init` (it acquired a node already); or you only *read* the store (reads don't need a node). And don't `release` casually — releasing does **not** invalidate ids already issued by that node, and the id is never reused.

**Key options (rationale only).**
- `acquire` — the CAS-loop claim of the next sequential node id; defaults pull `git config user.email` and the hostname for the registry stamp. The one you need on a fresh clone that will *write*.
- `list` — shows one row per known node from the node and label registries, allocation blocks/pools and the recorded root, and stored spec-ID namespaces that are node-shaped (UUID or numeric) or already known from those records, including retired labels and legacy numeric IDs. `*` marks this clone's opaque binding (or legacy node config). Hostname and clone path appear when recorded. Its local allocation role is `work node` or `root` (the wording `aida node show` uses); the shared root designation is shown as `recorded root`. Other clones' configuration roles remain `unknown` because that configuration is local. Labels such as HOME are separate from role and never establish allocation authority. Listing does not acquire, repair, or enroll a node. <!-- trace:BUG-B6B1278890774EF993D2EBC327862470-026 | ai:codex -->
- `release` — removes the registry entry but is deliberately *non-destructive to issued ids*. Reach for it only when retiring a clone for good.
- `alias set <LABEL> [--node <uuid>]` / `alias rename <OLD> <NEW>` / `alias list` / `alias show <LABEL|uuid>` — give a node a short label so its node-local ids read as `TASK-HOME-095` instead of `TASK-<32-hex-node>-095`. Any command that takes a spec id accepts the label form in any case, and human and agent output shows it; `--format json` keeps `spec_id` canonical and adds `display_id`. Pass `--canonical-ids` (or set `AIDA_CANONICAL_IDS=1`) to show canonical ids instead. Labels are for display and input only: AIDA always stores and writes the canonical id. A label is never moved to another node. `rename` retires the old label, which keeps resolving to the same node, and there is no delete. Labels are 2–8 letters or digits starting with a letter; labels made only of hex digits and labels equal to a type prefix are refused.

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
- `setup --install` — writes the Claude Code `settings.json` entry for you; without `--install` it just prints the config to paste. Reach for `--install` unless you want to review first. `setup --client claude|codex|antigravity|all` picks which client's guidance to print/install.
- `--color auto|always|never` — defaults to `auto` (color only on a TTY, respecting `NO_COLOR`); set `never` when piping the segment into a tool that mangles ANSI.
- `--client claude|antigravity` (alias `agy`) — opt in to merging that client's live stdin JSON (model, context-window usage, activity, VCS branch, wherever the client sends them) alongside the AIDA segment. Omitted (the default), `aida statusline` never reads stdin and renders the AIDA-only segment exactly as before — this is purely additive. See `docs/agents/statusline-contract.md` for the field contract and per-client adapter notes.

**Gotchas.** `q:N` (queue depth) is keyed off the *active role's* routed queue and is **omitted when zero** — an absent `q:` segment means an empty queue, not a broken statusline. And the queue is keyed off the shell's `$USER`/`$AIDA_USER`, so a surprising `q:` value usually means the wrong user identity, not a bug. `--client` skips the stdin read entirely on a TTY (a manual `aida statusline --client claude` at a bare shell never blocks), and a piped, non-TTY read is bounded to a ~200ms deadline and a 256KB cap — an open pipe/FIFO with no data waiting, an oversize payload, or a parse failure all silently fall back to the AIDA-only segment rather than hanging the prompt.

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
- `--since <date-or-ref>` — exempts specs completed before a cutoff (a time bound such as `30d`, `2w`, an ISO date at local midnight, or RFC3339, or else a git ref/tag) from the completed-without-commit check, to quiet noise on legacy history predating trace conventions. An invalid value is an error. The way you stop `doctor` nagging about pre-AIDA commits.
- `--json` — machine-readable output for wiring `doctor` into CI / a health dashboard.
- `--fix-sandbox` — a standalone guided printer (not a drift check): brings the OS sandbox (bubblewrap write-confinement) up on *this* host. It detects the current state, prints the exact copy-pasteable steps that host needs — install, the runtime + persist sysctl to permit unprivileged user namespaces, the `[contained] os_wrap` opt-in, and the verify command — with sudo steps clearly marked "run this yourself", then runs a non-sudo self-test smoke. It never runs sudo for you. The single command to run when standing up confinement on a new machine. (Full reference: `docs/agents/claude-bubblewrap-sandbox.md`.)
- the focused subcommands (`verify-relationships`, `validate-trace-comments`, `shell-substitution-holes`, `bare-id-twins`, `scrub-collisions`, `repair-stale-blocks`, `migrate-counter-scope`, `convention-check`) — run *one* category when you know what you're chasing, instead of the full `fsck` sweep. `shell-substitution-holes` reports only the high-signal, same-line syntactic anomalies known to remain after shell substitution; use `--exclude <SPEC-ID>` when the detector's own discussion quotes examples. It never repairs or guesses the lost text. `bare-id-twins` is report-only: it flags bare `TYPE-NNN` ids in descriptions or comments that also name one of this node's local specs (`TYPE-100` vs `TYPE-HOME-100`), so prose meaning the local spec can be rewritten in its alias form. <!-- trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude -->
- `--fix id-collisions` — spec IDs that differ only by leading zeros are the same ID (`<type>-0042`, `<type>-42` and `<type>-00042` all name one spec), so a store holding two such objects has a collision, which `aida doctor` reports under `id-collisions`. The repair keeps the number for the spec with more commit-message references on `origin/main`, gives each other spec a fresh ID, rewrites exact-case mentions of the old ID in the store, Markdown docs, every text file under `docs/` (plans, TSV and other data files), the changelog and code comments (code literals and lowercase file links stay), and records the old ID in the renumbered spec's history. It runs in reviewable halves: `--dry-run --mapping <file>` plans the repair and saves the old -> new mapping without writing the store; `--mapping <file> --tree-only` applies that mapping to this checkout's tracked files only; `--mapping <file> --yes` applies the same mapping to the store, refusing before any write if a new ID was taken meanwhile. `--preserve <SPEC>` (repeatable) leaves that spec's own title, description and comments as written during the store apply, so the spec recording the old -> new table keeps it intact. <!-- trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude -->
- `doctor check <category> --fail-on-findings` — the opt-in gate: exits non-zero when that one category has any finding, so a CI step or scheduled job can key off it. Off by default.

#### Spec ID padding (`[ids]`)

How many digits a spec number is padded to is a project setting in `.aida/config.toml`:

```toml
[ids]
seq_width = 0       # legacy/global IDs: <type>-42; 4 gives <type>-0042
node_seq_width = 3  # node-local IDs: <type>-<node>-007
```

The widths apply when an ID is minted and whenever one is displayed: lists, `show`, JSON `spec_id` fields, the ID `aida add` prints, and suggested trace comments and commit trailers. A width left unset mints at 3 digits and displays every ID exactly as stored. Stored IDs and file names keep their padding, and because leading zeros never change which spec an ID names, changing a width later renames nothing. `aida init --id-width N` sets both widths for a new project; `aida config digits N` sets `seq_width`. The `digits` value in older `metadata.yaml` files no longer has any effect. <!-- trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude -->

**Gotchas.** Focused doctor verbs have their *own* destructive opt-ins (`validate-trace-comments --strip-dangling`, plus the top-level `--heal` / `--category` repair path) — read the relevant `--help` before turning a diagnostic into a write. Bare `aida doctor` and `aida doctor check <category>` are **report-only**: exit code `0` means "ran successfully," not "found nothing" — findings print either way, but nothing about the exit code tells you whether any were found. `fsck` is the one exception and exits non-zero if any check found a problem, so it's CI-suitable as a gate as-is; every other check needs `--fail-on-findings` to get the same signal. Its cache-freshness section also prints one informational line about the history index behind `aida history` (switched off with `AIDA_HISTORY_CACHE=0`, not built yet, up to date, still filling in older history, behind the store, or unreadable). That line never fails `fsck`: a missing or stale history index never makes `aida history` wrong. It only makes the event views slower, and (when it is missing or far behind) makes bare `aida history` show the per-spec digest (with a note) instead of the status flow. <!-- trace:TASK-1508 | ai:claude -->


**Primary-checkout drift.** `aida doctor check main-checkout-drift` reports when the primary checkout (the main worktree that linked worktrees hang off) has uncommitted tracked edits, untracked files, a branch other than `main`, or commits ahead of or behind its upstream. It lists the drifting paths. Where an untracked file's modification time falls inside a live session's window, it names that session as the likely owner so you know whom to ask. It only reads: nothing is deleted or moved. The same condition adds one `main checkout: …` line to the session-start context (set `AIDA_MAIN_CHECKOUT_DRIFT=off` to silence it) and shows a `⌂!` marker in `aida statusline`. <!-- trace:STORY-1648 | ai:claude -->

**Cached compiler-wrapper errors.** `aida doctor` reports a `rustc-cache` finding when Cargo's `target/.rustc_info.json` contains a failed wrapper probe (such as an sccache permission error). It also checks the target selected by `CARGO_TARGET_DIR`; relative values resolve from the invocation directory, as in Cargo. `aida doctor rustc-cache` diagnoses just that cache without running Cargo; `aida doctor rustc-cache --fix` rechecks and removes the failed cache so Cargo can regenerate it. Plain diagnosis and general `--heal` leave it in place. Healthy caches, malformed JSON, and failures unrelated to a compiler wrapper are retained. <!-- trace:TASK-2324 | ai:codex -->

**Refused ID allocation.** When a clone cannot allocate node-local IDs, `aida add` names the cause (for example a group-writable `.aida/node.toml`, with the `chmod go-w` that fixes it, or a dispenser that records a different node than the clone) instead of a generic failure. `aida doctor check allocation` reports the same faults before an `aida add` hits them: allocation state files the reader refuses (group- or other-writable, owned by another user, hard-linked, not a regular file), a legacy `.aida/dispenser.toml` whose node disagrees with `.aida/node.toml`, and a missing half of that pair. Each finding gives the repair. The check is report-only. Rewriting a dispenser's node is safe only when no IDs were dispensed under the other node. <!-- trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude -->

**Read-only git config lock.** A sandboxed shell (for example Claude Code's Bash sandbox) can leave an empty, read-only `config.lock` in the repository's shared git directory. While it is there, git refuses every config write, so new worktrees, branch deletion and `push -u` fail with `could not lock config file .git/config: File exists`. AIDA removes that file before its own worktree and branch commands, and retries once if it reappears. `aida doctor check sandbox-config-lock` reports it, and `aida doctor --heal --category sandbox-config-lock` removes it. Only an empty lock with mode `0444` is removed; a real git lock is never read-only, so any other lock is left in place and git's error is shown. <!-- trace:BUG-2211 | ai:claude -->

**Chains with** — the structural counterpart to the content lenses; pairs with `db` (Ch.10) for store-level repair.

---

### `aida defer`

**One line** — park a spec as primed/conditional work — hidden from the default view, but with a recorded *revisit trigger*, not filed away like archive.

**Mental model.** `defer` is a **view-level flag** (like archive) that does *not* touch the lifecycle status machine. The one thing distinguishing deferred from archived is `--until`: a free-text condition that says *what brings the spec back* ("when a slice verb ships"). Archived is retrospective ("done with this, filed"); deferred is prospective ("not yet, but here's the trigger"). Deferred rows drop out of `list`/`search`/`history` by default and resurface with `--deferred` (only) or `--all` (union).

**Reach for it when** — you've captured a real idea but it's blocked on a future condition and you don't want it cluttering the open-work view — yet you also don't want to lose the *why it'll come back*. The EPIC-shaped "smallest slice now, revisit-trigger filed" discipline lands here.

**Don't reach for it when** — the spec is genuinely *done with* (that's `archive`, Ch.2); or it's actually *blocked by another spec* — that's an `aida edit --blocked-by` relationship, which the graph tracks, not a view flag. Defer is for conditions the graph can't express.

**Key options (rationale only).**
- `--until <condition>` — the whole reason to defer rather than archive. Record the revisit trigger; a deferred spec with no `--until` is just a hidden spec you'll forget. Always set it.

**Gotchas.** Deferred is *orthogonal to status* — a deferred Approved spec is still Approved, just hidden. Don't read "deferred" as a lifecycle state; it's a lens flag stacked on top of whatever status the spec holds.

**Chains with** — the prospective sibling of `archive` (Ch.2); surface deferred work with `list --deferred` when its trigger fires.

---

### `aida export`

*(Covered in [Chapter 10 — Storage & data](10-storage.md).)* Tree-shaped export of a requirement hierarchy for sharing between projects (`export --export-format tree --id FOLDER-001 -o templates.json`). It's *setup-adjacent* — exporting a template tree to seed another project is a setup act — but it lives with the storage commands; pair it with `aida import` on the receiving side.

---

### `aida import`

*(Covered in [Chapter 10 — Storage & data](10-storage.md).)* The receiving side of `export`: lands a tree JSON under a chosen `--parent`, with `--on-conflict skip|rename|replace` deciding how clashes resolve. Reach for it when bootstrapping a new project's spec hierarchy from another's exported template.

---

### `aida docs`

*(Covered in [Chapter 9 — Integrations & servers](09-integrations.md).)* Projects the requirements graph into a layered docs tree (`docs build`, `docs check`, the embedded `docs glossary`). It's setup-adjacent — you wire `docs check` into CI at project-setup time so the docs projection can't drift from the graph — but the command family lives with the other projection/server surfaces.

---

## Clone-local allocation authority

<!-- trace:BUG-2017 | ai:codex -->
Ordinary clones are WORK by default. `aida init` establishes a random opaque
UUID node and persistent per-type counters in the clone's common Git directory,
shared by linked worktrees. IDs have the form `BUG-<node>-001`; restarting or
running `aida node acquire` again retains the binding and consumed counters.
Legacy node files retain their schema and existing object IDs; migration creates
the opaque binding once and carries forward consumed counter high-water marks.
Migration requires a complete, valid legacy node and matching distributed counter;
a missing or contradictory record requires explicit recovery. Cold bootstrap
requires both legacy records to be absent. An interrupted opaque bootstrap can
recover its existing valid counter and UUID without resetting consumed values.
WORK does not register hostname, user, email or friendly node names, claim global
blocks, or assign agreed IDs. Missing bindings outside the supported init/repair
lifecycle refuse writes rather than falling back to global counters.

`aida config allocation show` reads the local mode without opening the store.
The operator may explicitly run `aida config allocation enroll-home` at a direct
human terminal in the intended HOME clone. This writes only that clone's common
Git setting; tracked files, environment variables and a shared store never confer
authority. Enrollment is a deliberate live operator action, separate from source
installation. It is never performed automatically by init or migration. Restart
long-running MCP sessions after enrollment; a captured WORK provider refuses a
stale mode instead of gaining a global fallback.

Only HOME may run `aida db merge-gate`. Promotion adds an agreed-ID alias while
retaining UUID, native node-local ID, relationships, comments and trace text.
Repeated promotion is a no-op. `db retire-legacy-ids` refuses destructive
collapse of native node IDs: those IDs remain permanent lookup aliases, and
`merge-gate` already supplies the canonical display ID. WORK/HUB post-pull processing skips promotion;
Git ref transport does not enroll a clone or allocate display IDs. Conflict audit
remains separate follow-up work. The WORK transport scanner (see
[`aida config transport policy`](#aida-config-transport-policy)) refuses unsafe
content; it does not make an arbitrary union safe.

## Extending aida with external commands

<!-- trace:STORY-1641 | ai:claude -->
Like `git`, `aida` runs an executable named `aida-<name>` from your `PATH` when `<name>` is not one of its own commands. Drop a script called `aida-deploy` somewhere on `PATH`, mark it executable, and `aida deploy --dry-run` runs it with `--dry-run` passed through unchanged. Its stdout, stderr and exit code are the command's; on Unix the extension replaces the `aida` process outright.

Names resolve in a fixed order, and the first match wins:
1. a built-in command (an extension can never shadow one),
2. a built-in alias,
3. your own alias from `~/.aida/aliases.toml` (expanded for interactive use only),
4. an `aida-<name>` executable, first match on `PATH`.

If nothing matches, you get the usual unrecognised-command error and exit code. When an installed extension has a similar name, a hint names it.

The extension name must be the first argument: `aida deploy --format json` works, `aida --format json deploy` does not. On Windows, `aida-<name>.exe`, `.cmd` and `.bat` are found as well.

The extension receives its context in the environment:
- `AIDA_PROJECT_ROOT` — the project root, when run inside an AIDA project (unset otherwise).
- `AIDA_STORE_PATH` — the project's requirements store directory (unset outside a project).
- `AIDA_OUTPUT_FORMAT` — the output format aida would use here: `human`, `json` or `toon`.
- `AIDA_CALLER_KIND` — `human`, `agent` or `headless`.
- `AIDA_BIN` — the path of the `aida` binary that dispatched it.

Extensions should read and change project state through `"$AIDA_BIN" ... --json` (or another machine output format), never by writing to the store or cache files directly. That keeps validation, history and sync in one place.

Installed extensions are listed in an "External commands" group in `aida help`, `aida help --all` and `aida alias list`, and `aida help <name>` runs `aida-<name> --help`.

---

## Where to go next

You've set the project up and can keep its machinery healthy. From here:
- **[Chapter 1 — Getting started](01-getting-started.md)**: the daily verbs on top of this plumbing (`init` lives there in full).
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: `remote` and the two-leg sync that the setup enables.
- **[Chapter 10 — Storage & data](10-storage.md)**: `db` / `cache` — the store internals `doctor` repairs, plus `export`/`import`.
- **[Chapter 11 — Working on AIDA itself](11-dev.md)**: if you're *developing* AIDA rather than using it.

### `aida config transport policy`

<!-- trace:STORY-1742 | ai:codex -->

A WORK push now requires an explicit public-safe baseline and a private,
operator-reviewed policy. Enrollment trusts the baseline's entire reachable
closure; it does not scan or certify that closure. Use synthetic inputs and
review them before enrollment. Never enroll an unsafe tip to silence a refusal.
The original baseline remains the scan boundary after every successful push.

Create an owner-only JSON file in an owner-controlled directory (neither the
file nor directory may be writable by another user):

```json
{
  "version": 1,
  "reviewed": true,
  "deny_tokens": ["synthetic-private-token"],
  "git_name": "AIDA Generic",
  "git_identity": "aida"
}
```

The deny list must include known private organization, customer, project,
short-host, username, path and ticket tokens. An empty list still requires
explicit review. Matching is literal, ASCII case-insensitive substring matching;
tokens contain 2–256 printable ASCII bytes, at most 256 entries. Policy values
are never printed by status or refusal diagnostics.

```bash
aida config transport enable work --baseline <reviewed-full-store-sha>
aida config transport policy --file <owner-only-policy-file> --reviewed
aida config transport show --json
aida push --store-only
```

Private profile, policy, receipt and lock files in the common Git directory
require mode `0600`, current ownership and one link. Symlinks, FIFOs and unsafe
permissions refuse, and the error says how to recover (`chmod 600` a file you
own, or move it aside). The one exception is the profile and lock created by
older releases with mode `0644`: they remain usable when you own them, they have
one link and nothing other than you can write to them. Show never changes them, the lock is
tightened to `0600` in place the next time it is taken, and disable removes the
profile. The policy and receipt never get this exception. Never delete the
permanent lock sidecar. Policy changes require the same review and enrollment command. Disable
removes the profile and successful-push receipt; it retains the private deny
policy. Re-enabling requires an explicit reviewed baseline for export.

The scanner inspects every new commit through every parent, including deleted
historical content, commit names/identities/messages, tree paths and new blobs.
It checks raw and decoded YAML, JSON, JSONL and TOML keys/scalars, the native
`oplog.yaml` operation tags, and plain Markdown/text. Tree paths follow a closed
grammar of the real store layout: fixed root and registry files, sharded
`objects/<TYPE>/<shard>/<ID>.yaml` and `timings/<TYPE>/<shard>/<ID>.json` paths
that must match the store path builders exactly, canonical UUID object and
mailbox files, review-watch mailbox digests, and named files under
`coordination/leases`, `registry/queues` (decoded from the queue filename
escapes), `schedule`, `.aida/agents` and `.aida/burndown`. Only the structural
extension is removed; every user-controlled component (IDs, names, type
directories) is still scanned. Any other path is unsupported and refuses. Emails (including redaction placeholders), IP addresses,
FQDNs, host-bearing URLs, explicit hostname fields and deny tokens refuse.
The enrolled non-email Git identity must also pass the owning project's existing
identity allowlist and mandatory verified store hook; enrollment does not
change that allowlist or configure Git identities.

This version conservatively refuses non-ASCII text, YAML aliases/tags,
percent-encoded payloads, HTML character references (any `&#…` or `&name`
shape, with or without the semicolon), declared opaque encoding/compression/signature data,
data URIs, unsupported/binary containers, signature/extension
commit headers, ambiguous escapes and incomplete/alternate/replaced graphs.
The trusted baseline closure is never enumerated: Git's own `tip ^baseline`
exclusion identifies the new delta. Every object in that delta is packed
privately and rehashed by `git index-pack --strict`, which applies fsck's strict
object and link checks to exactly the objects being published; unrelated
history in the shared object database is not re-verified. Separate limits apply:
1 MiB per document or object, nesting 64, 100,000 new commits, 2,000,000 new
objects, 2 GiB of new uncompressed data, 100,000 entries per tree, 2,000,000
distinct directory walks, and 30 minutes for the whole scan. Limit or deadline
exhaustion refuses without publication. These conservative restrictions
can reject harmless content; they do not prove arbitrary prose free of private
facts. Operator review remains necessary.

After scanning, exactly the inspected SHA is pushed once, with no force or
tracking-ref update. The writer lock is released before hooks/network I/O.
The push has no time limit and runs without a controlling terminal, with Git
terminal prompts disabled. Credentials must come from a credential helper, the
enrolled endpoint or an SSH agent with a known host key, so a prompt fails
immediately instead of hanging. Ctrl+C is forwarded to Git and reported as an
uncertain result.
A private receipt binds successful porcelain acceptance to the endpoint,
store ref, tip, original baseline, policy hash and scanner version. Policy
changes rescan from the original root; receipts never cache scan results or
turn tracking refs into trusted evidence. An uncertain push leaves the receipt
unchanged. If publication succeeds but receipt persistence or policy
revalidation fails, the command reports that partial result and does not retry.

### `aida add --sanitized` (sanitized intake)

<!-- trace:STORY-1742 | ai:claude -->

With a reviewed policy enabled, every requirement-store write from this clone is
checked before anything is persisted. The complete write is prepared first:
object, metadata and `oplog.yaml` bytes, deletions, the clone-local counter and
binding update, and the commit message and effective Git author and committer.
All of it must pass the same scanner as a push, and the effective Git identity
must equal the policy's `git_name`/`git_identity`. The plan is checked again
immediately before the first file changes and is applied once. A refusal leaves
the store, counters, history and refs exactly as they were, and never echoes the
refused text. The private policy is read before the store write lock is taken.

```bash
aida add --sanitized --template > defect.md      # generic problem/reproduction/expected/actual scaffold
aida add --sanitized --preview "Generic failure" --description-from-file defect.md
aida add --sanitized "Generic failure" --description-from-file defect.md
```

`--sanitized` files one draft (a bug unless `--type` says otherwise) with only a
title, description, type and priority. It never adds an owner, tags, feature,
parent or session context, and refuses flags that would. `--template` and
`--preview` work in any clone and write nothing: preview reports the policy
verdict of the store a write would use (an explicitly chosen requirements
file, the detected store, or the project), not of the current directory, and, when the text
passes, shows the candidate. Preview is for your review only; it reserves no
identifier and is not a permission token. A `--sanitized` write requires an
enabled profile.

New requirements omit the optional machine and session filing context and say so
in a `filing_provenance_omission_reason` custom field. Existing filing context
is never changed. Identifiers come from this clone's own counter, allocated only
inside the prepared write; a refused write consumes no number. Global metadata
counters cannot change.

Existing store content is read by its YAML structure, not by its characters:
prose with `&`, `*` or `!` (Markdown emphasis, punctuation) is ordinary text,
while real YAML anchors, aliases and non-core tags refuse. `oplog.yaml` is
bounded per record rather than per file, so a long shared history stays
writable. Content policy covers what a write adds, never history: for
`oplog.yaml` and `metadata.yaml`, intake and `aida push` both scan only the
change against the previous version of the file (appended records, changed
values), while the YAML structure and size checks still cover the whole file.
Every other file is scanned whole. Older text the current policy would refuse
therefore blocks neither a new write nor its push. Written files are always `0644` and directories `0755`, whatever the
umask. A store file or directory that is already group- or other-writable is
refused with its path and the `chmod go-w` fix. Concurrent writers on one clone
wait their turn for up to 30 seconds (5 seconds for a server save) instead of
failing at once.

Supported prepared writes: adding, updating and deleting requirements, whole-store
saves (including their concurrent-edit conflict rules), bulk updates, per-spec
compare-and-swap updates and additive batches, through the CLI, MCP and the
server. MCP `add_requirement` files through the same prepared add; its `parent`
option is refused while a profile is enabled. Writers without a prepared form refuse before any change while a profile
is enabled: imports, queue edits, direct object, registry and dispenser writes,
legacy single-file storage and store initialization. A server discards the
refused change from its in-memory state instead of reporting success; if it
cannot reload, it refuses further saves until a reload succeeds.

**Boundary:** recoverable fast-forward store pull is implemented as described
above. The client guard does not enforce hub receiving hooks or PUBLIC
forwarding, and raw Git, other AIDA writers that do not go through the store
backend, and foreign writers can bypass it. No history rewrite, clean export,
live hub hook or automatic forwarding is provided.
