//! `aida queue_cmd` clap types, owned by `queue_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Personal work queue commands.
// trace:STORY-368 | ai:claude
// trace:TASK-487 | ai:claude
// why: clap-derived subcommand enum — boxing a variant's fields would break the
// derive's flat arg parsing; the size skew is inherent to CLI command modeling.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum QueueCommand {
    /// List items in your queue. When a role is active and --role is not
    /// passed explicitly, defaults to filtering on that role. Without
    /// --global / --local, merges the local (per-project) queue with the
    /// active role's global queue and tags global entries with the
    /// originating project (`[origin:<project>]`).
    // trace:FR-1-012 | ai:claude
    List {
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Include completed requirements
        #[clap(long)]
        include_completed: bool,
        /// Filter to items routed to a specific role (e.g., "implementer").
        /// Pass `--for any` (or `--role any`) to filter to UNROUTED items
        /// (entries with no `for_role`), matching the write-side semantic
        /// of `aida queue add --for any`. Use `--all` (alone or combined
        /// with `--for X`) to override the active-role default. `--for X`
        /// composes with `--all` and takes precedence — i.e.
        /// `--all --for reviewer` shows only reviewer-routed items.
        /// `--for <role>` is the canonical form, matching
        /// `aida queue add --for` and `aida queue next --for`; `--role`
        /// is kept as a hidden alias for back-compat.
        // trace:TASK-71 BUG-87
        #[clap(long = "for", visible_alias = "role")]
        role: Option<String>,
        /// Override the active-role default filter (show all roles).
        /// Combines with `--for X` — `--for X` always wins.
        // trace:BUG-87
        #[clap(long)]
        all: bool,
        /// Bypass the active role's scope_tags / scope_status filters.
        // trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,
        /// Show only the global, role-scoped queue at `~/.aida/queue/<role>.yaml`.
        /// Mutually exclusive with --local.
        // trace:FR-1-012 | ai:claude
        #[clap(long, conflicts_with = "local")]
        global: bool,
        /// Show only the local, per-project queue (skip the global merge).
        /// Mutually exclusive with --global.
        // trace:FR-1-012 | ai:claude
        #[clap(long)]
        local: bool,
        /// Pull from `origin/aida-store` before listing. Opt-in freshness
        /// for collaborators / multi-session workflows; fast path stays
        /// default. No-op when the local orphan branch is already
        /// current; warns and falls back to the local view when offline.
        // trace:STORY-78 | ai:claude
        #[clap(long)]
        sync: bool,
        /// Include Completed/Rejected entries in the listing. Default
        /// hides them with a footer count so the queue view stays
        /// focused on actionable work.
        // trace:TASK-46 | ai:claude
        #[clap(long)]
        include_terminal: bool,
        /// Filter to entries whose `for_scope` matches one of these
        /// scopes (comma list). Pass `none` to filter to entries with
        /// no `for_scope` set (the "uncategorized" pile).
        /// Mirrors the `--scope` shape on `queue add`. Distinct from
        /// `--no-scope`, which bypasses the active role's
        /// scope_tags/scope_status filters — different axis.
        // trace:TASK-52, STORY-57 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "CSV")]
        scope: Option<String>,
        /// Group entries by their parent EPIC for a visual cluster view.
        /// EPICs are sorted by item count desc; unscoped items appear
        /// under a final "Unscoped" group. Within each group, queue
        /// position order is preserved.
        // trace:TASK-33 | ai:claude
        #[clap(long)]
        tree: bool,
        /// Suppress the "Done — awaiting merge" in-flight section. By
        /// default `aida queue list` appends Done specs (work finished
        /// on a branch, not yet merged to main) so the natural "what am
        /// I waiting on" view stays complete. Pass this to get only the
        /// queued (Pending/Approved/Planned/InProgress) view.
        // trace:TASK-222 | ai:claude
        #[clap(long)]
        no_in_flight: bool,
        /// Show only the "Done — awaiting merge" in-flight section,
        /// suppressing the regular queue entries. Useful for "what am I
        /// waiting on a PR for" snapshots.
        // trace:TASK-222 | ai:claude
        #[clap(long, conflicts_with = "no_in_flight")]
        in_flight_only: bool,
        /// Filter to queued items tagged `batch:NAME`. Composes with
        /// `aida queue work --batch NAME` — the same tag drives both
        /// the audit view and the per-item drain.
        // trace:TASK-229 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "NAME")]
        batch: Option<String>,
        /// Filter to entries whose requirement carries this exact tag
        /// (case-insensitive). The general form of `--batch`.
        // trace:TASK-238 | ai:claude
        #[clap(long, value_name = "TAG")]
        tag: Option<String>,
        /// Filter to entries with any tag starting with this prefix —
        /// e.g. `--tag-prefix batch:` for all batched items.
        // trace:TASK-238 | ai:claude
        #[clap(long, value_name = "PREFIX")]
        tag_prefix: Option<String>,
        /// Group the queue by `batch:*` tag value — each batch under
        /// its own header, un-batched items under "No batch". Queue
        /// position order is preserved within each group.
        // trace:TASK-238 | ai:claude
        #[clap(long)]
        by_batch: bool,
        /// Emit the queue as JSON (cache-fast machine read) instead of the
        /// human table. Used by the TUI queue panel to avoid the full
        /// status scan.
        // trace:BUG-616 | ai:claude
        #[clap(long)]
        json: bool,
        /// Fleet-wide bird's-eye: aggregate every user's queue (not just your
        /// shell identity), read-only, grouped by user then role with the
        /// owning user shown per row. Opt-in for the coordinator seat;
        /// composes with `--all` to span all roles. Default scoping (your
        /// user + active role) is unchanged without this flag.
        // trace:STORY-672
        #[clap(long)]
        all_users: bool,
        /// Narrow the listing to items whose spec is in this epic's
        /// TRANSITIVE descendant tree — the epic itself plus all its
        /// children and grandchildren, computed in-process from the
        /// requirement graph (the same hierarchy closure `aida graph
        /// <ID> --tree` walks). Filters the single queue; it does NOT
        /// create a per-epic queue. ANDs with the role / scope filters
        /// (`--for`, `--all`, `--global`, `--local`). `--parent` is an
        /// alias.
        // trace:TASK-923 | ai:claude
        #[clap(long, visible_alias = "parent", value_name = "ID")]
        epic: Option<String>,
        /// Ignore the active focus for this listing — show the whole queue
        /// instead of the focused subtree. No-op when no focus is set or when
        /// `--epic` was passed explicitly.
        #[clap(long)]
        no_focus: bool,
    },
    /// Add a requirement to your queue
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// Add to top of queue
        #[clap(long)]
        top: bool,
        /// Add to bottom of queue (default)
        #[clap(long)]
        bottom: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Note explaining why this was queued
        #[clap(long)]
        note: Option<String>,
        /// Route this item to a specific role queue (e.g., "implementer",
        /// "architect"). The other role wearer can then `aida queue list`
        /// while in that role to see incoming work. Routing `--for human`
        /// files the item into the human-attention set — it then surfaces in
        /// `aida list human` alongside specs that need a person by status/tag.
        // trace:TASK-747 | ai:claude
        #[clap(long)]
        r#for: Option<String>,
        /// Restrict routing to sessions whose lease scope matches this
        /// (e.g., an EPIC id). Default-populated to the active session's
        /// scope when adding from inside a session worktree, unless
        /// --no-scope is passed. Pairs with --for so two implementer
        /// sessions don't see each other's incoming work.
        // trace:STORY-57 | ai:claude
        #[clap(long)]
        scope: Option<String>,
        /// Restrict routing to one specific session, by 8+ char lease id
        /// prefix. Mutually exclusive with --no-scope (passing both is
        /// nonsense — --for-session implies a scope match too).
        // trace:STORY-57 | ai:claude
        #[clap(long = "for-session", conflicts_with = "no_scope")]
        for_session: Option<String>,
        /// Suppress the auto-default scope when adding from inside a
        /// session worktree. The entry stays scope-unrouted (visible to
        /// any session in --for's role).
        // trace:STORY-57 | ai:claude
        #[clap(long)]
        no_scope: bool,
        /// Add to the role's GLOBAL queue at `~/.aida/queue/<role>.yaml`
        /// instead of the local per-project queue. Requires --for or an
        /// active role context (AIDA_SESSION_ROLE).
        // trace:FR-1-012 | ai:claude
        #[clap(long)]
        global: bool,
        /// Bypass the guard that refuses queueing a Completed or
        /// Rejected requirement. Use for legitimate re-open scenarios;
        /// the default error message hints at this flag.
        // trace:TASK-45 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force: bool,
    },
    /// Sum latest effort estimates for queued items.
    // trace:STORY-451 | ai:codex
    // trace:TASK-956 | ai:claude — hidden from `queue --help` (still runs).
    #[clap(hide = true)]
    Load {
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
    },
    /// Remove a requirement from your queue
    Remove {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Remove from the role's GLOBAL queue (requires --for or active role).
        // trace:FR-1-012 | ai:claude
        #[clap(long)]
        global: bool,
        /// When --global, the role whose global queue to remove from.
        /// Defaults to the active role (AIDA_SESSION_ROLE).
        #[clap(long)]
        r#for: Option<String>,
    },
    /// Move a queue item to a new position
    Move {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// User ID whose queue to reorder (defaults to AIDA_USER or the
        /// shell's $USER), matching every other queue verb (add/remove/list).
        // trace:BUG-1487 | ai:claude
        #[clap(long)]
        user: Option<String>,
        /// Move to the front of the queue (slot 1). `--to-front` and
        /// `--to-top` are accepted aliases for the same action. When
        /// the target is already at slot 1, the command is a friendly
        /// no-op that prints `<id> is already at queue head`.
        // trace:TASK-280 | ai:claude
        // trace:TASK-491 | ai:claude
        #[clap(long, visible_aliases = ["to-front", "to-top"])]
        top: bool,
        /// Move to the back of the queue (last slot). `--to-back` is an
        /// accepted alias for the same action.
        // trace:TASK-280 | ai:claude
        #[clap(long, visible_alias = "to-back")]
        bottom: bool,
        /// Move to an absolute 1-indexed slot among this user's live
        /// (non-terminal) queue entries — `--to 1` is the front, `--to 3`
        /// the third slot. An N past the end clamps to the last slot rather
        /// than erroring. These match a bare `aida queue list`; a
        /// role/scope-filtered list shows a subset, so the visible slot may
        /// differ there — prefer `--to-front`/`--to-back` under a role filter.
        // trace:TASK-280 | ai:claude — trace:TASK-317 (soften "same as queue list")
        #[clap(long, value_name = "N", conflicts_with_all = ["top", "bottom", "before", "after"])]
        to: Option<usize>,
        /// Move before this requirement ID
        #[clap(long)]
        before: Option<String>,
        /// Move immediately after this requirement ID — symmetric to
        /// `--before`. Useful for "put X right after Y" reasoning, and
        /// for inserting a cluster of items after a single anchor
        /// without each move shifting the previous one further from the
        /// anchor.
        // trace:STORY-72 | ai:claude
        // trace:TASK-318 | ai:claude
        #[clap(long)]
        after: Option<String>,
        /// Allow moving an entry whose spec has terminal status
        /// (Completed/Rejected). The default refuses with an error so
        /// `queue move` doesn't silently no-op on a stale entry the
        /// queue list view hides. Has no effect when the entry isn't in
        /// the queue at all — that case still errors.
        // trace:BUG-249 | ai:claude
        #[clap(long)]
        force: bool,
    },
    /// Bulk-remove ALL queue entries for the user (DOES NOT touch the specs
    /// themselves — their status, content, and relationships stay intact;
    /// only the queue-membership records are removed). The next
    /// `aida queue list` shows an empty queue; affected specs survive in
    /// `aida list`. The destructive verb in the queue family — there is no
    /// undo short of re-adding each entry with `aida queue add`.
    ///
    /// Sibling verbs (use one of these instead when the scope is narrower):
    ///   • `aida queue remove <id>`     — remove ONE specific entry
    ///   • `aida queue prune --orphaned` — remove entries whose backing
    ///                                     spec was deleted (the
    ///                                     "??? (deleted)" ghosts)
    ///   • `aida queue done <id>`       — atomic "mark complete + remove"
    ///
    // trace:TASK-1-109 | ai:claude — plain `//` (not `///`) so the
    // trace marker doesn't leak into user-facing --help output per
    // TASK-268. The user-facing prose above is intentionally SPEC-ID-
    // free.
    Clear {
        /// User ID whose queue to clear (defaults to AIDA_USER or the
        /// shell's $USER). Does not affect other users' queues.
        #[clap(long)]
        user: Option<String>,
        /// Only remove queue entries whose backing spec has status
        /// `Completed`. Useful for tidying up after a batch ship: keeps
        /// the in-flight Approved / InProgress / Done entries, removes
        /// the ones whose work is already shipped. Orphan entries
        /// (backing spec deleted) are left in place — clean those up
        /// with `aida queue prune --orphaned`.
        #[clap(long)]
        completed: bool,
    },
    /// Prune dead queue entries by predicate. Two flags, two entry classes:
    /// `--orphaned` (target spec was DELETED) and `--merged` (auto-queued
    /// reviewer row whose PR already shipped). Combine them to sweep both.
    /// Neither touches an entry whose spec still exists but is done-with
    /// (archived / Completed / Rejected) — that class is `aida queue gc`'s
    /// job. Rule of thumb: spec gone → `prune --orphaned`; PR merged but the
    /// review row lingers → `prune --merged`; spec archived / terminal → `gc`.
    /// Pair any predicate with `--dry-run` to preview before applying.
    // trace:TASK-537 | ai:claude — plain `//` so SPEC-ID doesn't leak into
    // user-facing --help output per TASK-268 convention.
    // trace:TASK-1063 | ai:claude
    Prune {
        /// Remove queue entries whose backing spec no longer exists in the
        /// store (the "??? (deleted)" ghosts in `aida queue list`). Auto-
        /// queued reviewer entries from `aida pr` / `session end` can become
        /// orphans when the spec they cover is later deleted or rejected.
        /// Sibling verbs: `--merged` (PR shipped) and `aida queue gc` (spec
        /// still present but archived / terminal).
        #[clap(long)]
        orphaned: bool,
        /// Remove auto-queued reviewer entries whose PR has already merged. A
        /// review row ("Review PR-N: …") lingers when its PR merges outside the
        /// reviewer's `aida queue done` flow (e.g. a hand-merge); the backing
        /// spec is often still non-terminal so `--orphaned` misses it. Checks
        /// each review row's PR state with `gh`. Combine with `--orphaned` to
        /// sweep both. Sibling verb for done-with-but-not-deleted specs:
        /// `aida queue gc`.
        #[clap(long)]
        merged: bool,
        /// Preview the entries that would be removed; don't actually remove
        /// them. Pair with `--orphaned` / `--merged` for safe inspection.
        #[clap(long)]
        dry_run: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Restrict prune to entries routed to this role. Default: all roles
        /// (with the active-role default applied). Useful when only the
        /// reviewer queue has orphan entries.
        #[clap(long = "for", visible_alias = "for-role")]
        r#for: Option<String>,
    },
    /// Garbage-collect dead routed queue entries — remove every entry whose
    /// backing spec is archived, completed, or rejected (terminal corpses that
    /// linger in the queue file after the work shipped), OR whose routed
    /// review's own PR already merged (the review story's status alone can't
    /// say whether the review it wraps is still needed). The default `aida
    /// queue list` view already hides them, but the underlying queue file
    /// still carries them; this sweeps them and reports the count and why.
    /// Sibling of `aida queue prune --orphaned`, which targets DELETED specs.
    /// Use `--dry-run` to preview. Still-actionable entries
    /// (Draft/Approved/Planned/InProgress/Done) survive unless their PR
    /// already merged.
    // trace:TASK-1052 | ai:claude — plain `//` so the SPEC-ID doesn't leak
    // into user-facing --help output per the TASK-268 convention.
    // trace:TASK-1063 | ai:claude
    // trace:BUG-1512 | ai:claude
    Gc {
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Restrict the sweep to entries routed to this role. Default: every
        /// role's entries in this user's queue.
        #[clap(long = "for", visible_alias = "for-role")]
        r#for: Option<String>,
        /// Preview the entries that would be removed; don't actually remove
        /// them.
        #[clap(long)]
        dry_run: bool,
    },
    /// Peek at the top item in your queue without removing it. When a role
    /// is active and --role is not passed, defaults to filtering on it.
    /// Use this between work items to see what's next. Considers local +
    /// global queues by default (local wins on tiebreaks).
    // trace:FR-1-012 | ai:claude
    Next {
        /// Filter to items routed to a specific role. Pass `--for any` to
        /// peek the top UNROUTED item (matches `aida queue add --for any`
        /// write-side semantic). `--for <role>` is the canonical form,
        /// matching `aida queue add --for` and `aida queue list --for`;
        /// `--role` is kept as an alias. Composes with `--all`; `--for X`
        /// always takes precedence.
        // trace:TASK-71 BUG-87 | ai:claude
        #[clap(long = "for", visible_alias = "role")]
        role: Option<String>,
        /// Override the active-role default filter (show top from any
        /// role). Combines with `--for X` — `--for X` always wins.
        // trace:BUG-87
        #[clap(long)]
        all: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Bypass the active role's scope_tags / scope_status filters.
        // trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,
        /// Look only in the global queue (skip local).
        #[clap(long, conflicts_with = "local")]
        global: bool,
        /// Look only in the local queue (skip global).
        #[clap(long)]
        local: bool,
    },
    /// Walk the queue and advance each item to its next step — drain the
    /// autonomous, dispatch the human-required (review / --zen / decision)
    /// interactively. Processes to a resolution, never hides work.
    // trace:STORY-566 | ai:claude
    Advance {
        /// Advance just this queued spec; omit to walk the whole queue.
        id: Option<String>,
        /// Non-interactive: take only the unambiguous autonomous step per
        /// item, skip anything needing a human.
        #[clap(long, short = 'y')]
        yes: bool,
        /// Override the queue user (defaults to AIDA_USER / system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Mark a requirement as done AND remove it from your queue in one
    /// atomic step. Convenience for the implementer's done-then-pickup-next
    /// loop. Equivalent to: `aida edit <id> --status done && aida queue remove <id>`.
    Done {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Skip confirmation
        #[clap(long, short = 'y')]
        yes: bool,
        /// Bypass the commits-without-PR pre-check. Use only when the spec
        /// was implemented on a different branch that is already merged,
        /// so the local branch has no PR by design. Logs the bypass.
        // trace:BUG-269 TASK-423 | ai:claude
        #[clap(long)]
        force: bool,
        /// Bypass the commits-without-PR pre-check (same effect as `--force`
        /// for this gate). Named for intent — `--force` is a general escape
        /// hatch, `--skip-pr-check` is specifically the pre-check gate. Logs
        /// the bypass.
        // trace:BUG-285 TASK-447 | ai:claude
        #[clap(long)]
        skip_pr_check: bool,
        /// Record a user-facing CLI surface change for the operator digest.
        /// Repeatable. e.g. `--interface-cli "aida foo — new command"`. The
        /// deterministic source for `aida digest --audience operator`; absent ⇒
        /// this spec never appears there. Skips the interactive capture prompt
        /// when any --interface-* flag is given.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-cli", value_name = "LINE")]
        interface_cli: Vec<String>,
        /// Record a user-facing MCP surface change (new tool, gating, schema).
        /// Repeatable. See `--interface-cli`.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-mcp", value_name = "LINE")]
        interface_mcp: Vec<String>,
        /// Record a user-facing TUI surface change (keybinding, pane, overlay).
        /// Repeatable. See `--interface-cli`.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-tui", value_name = "LINE")]
        interface_tui: Vec<String>,
        /// Record some other user-facing interface change (not cli/mcp/tui).
        /// Repeatable. See `--interface-cli`.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-other", value_name = "LINE")]
        interface_other: Vec<String>,
        /// Explicitly mark this spec as having NO user-facing interface change
        /// (clippy/refactor/test/internal). Skips the capture prompt and
        /// records nothing — the spec stays out of the operator digest.
        // trace:STORY-542 | ai:claude
        #[clap(long)]
        no_interface_change: bool,
        /// Record a verification step the builder actually ran (the
        /// implementation audit trail). Repeatable — one flag per step, e.g.
        /// `--test-plan "cargo test -p aida-cli" --test-plan "manual: aida
        /// queue done at a TTY"`. Stored in implementation_info and surfaced in
        /// the PR body. Any `--test-plan` skips the interactive capture prompt.
        // trace:STORY-698 | ai:claude
        #[clap(long = "test-plan", value_name = "STEP")]
        test_plan: Vec<String>,
        /// Skip the verification-step capture prompt and record nothing (the
        /// spec's existing test-plan notes, if any, are left untouched).
        // trace:STORY-698 | ai:claude
        #[clap(long)]
        no_test_plan: bool,
    },
    /// Pick up a queued item (or scope-cluster of queued items) and launch
    /// claude in a fresh session worktree, with the role + skill routed
    /// from the item's metadata. Collapses the 5-7 manual steps (pull,
    /// session start, cd, role enter, claude /aida-pickup) into one
    /// command. With no `id`, picks the head of the active role's queue.
    /// With `id` matching a queued entry, that single item is the pickup
    /// target. With `id` resolving to an EPIC/STORY whose children are
    /// queued, drains the cluster — pre-populates the session manifest
    /// and routes the right skill.
    ///
    /// When `id` resolves to a known spec that ISN'T queued (and has no
    /// queued children), the error is status-aware: the recovery hint
    /// depends on the spec's current status — Done → `aida queue
    /// rework`; Planned → promote to Approved first; Completed/Rejected →
    /// re-open with `--force`.
    // trace:STORY-42, TASK-217 | ai:claude
    // trace:TASK-487 | ai:claude
    Work {
        /// Queued requirement ID (UUID, SPEC-ID, or agreed-id) for item
        /// pickup, OR an EPIC/STORY id with queued children for cluster
        /// pickup, OR `batch:NAME` for batch pickup (the positional
        /// equivalent of `--batch NAME` — accepts the literal tag printed
        /// by `aida queue list`), OR the `next` keyword: `next` picks the
        /// queue head (explicit form of the no-arg pickup), and `nextN`
        /// (e.g. `next3`) paired with `--auto-complete` drains the next N
        /// items from the head. Omit to pick up the head of the active
        /// role's queue.
        // trace:TASK-293 | ai:claude — plain `//` so the marker stays out
        // of `--help` output (TASK-268 user-facing-text convention).
        id: Option<String>,
        /// Count for the `next` keyword's spaced form — `aida queue work
        /// next 3` is equivalent to the compact `aida queue work next3`.
        /// Only meaningful directly after the `next` keyword.
        // trace:TASK-293 | ai:claude — plain `//` keeps the marker out of `--help`.
        count: Option<String>,
        /// Permission mode for the launched claude. Resolution order:
        ///   1. this flag
        ///   2. `AIDA_PERMISSION_MODE` env var
        ///   3. `.aida/config.toml [behavior] permission_mode`
        ///   4. AIDA-managed worktree default → `bypassPermissions`
        ///      (the worktree is git-sandboxed; the prompt flood was
        ///      eating autonomous overnight runs and Claude Code keeps
        ///      the rm -rf circuit breakers on)
        ///   5. fallback → `acceptEdits`
        ///      Common values: `auto` (research preview: auto-approves with
        ///      background safety checks; pairs with the pre-allow list),
        ///      `bypassPermissions` (legacy `--dangerously-skip-permissions`),
        ///      `plan` (read-only), `default` (prompt on everything). Passed
        ///      through unvalidated to `claude --permission-mode`.
        // trace:STORY-42, TASK-82, TASK-83, TASK-84 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "MODE")]
        permission_mode: Option<String>,
        /// Launch Claude in contained mode: strict Bash sandboxing, no
        /// unsandboxed fallback, destructive-command deny rules, and
        /// project-relative edit auto-allow only. With --auto-complete it
        /// contains the whole drain and every agent child it spawns. A
        /// vendor that cannot run contained (codex, agy) is refused.
        // trace:BUG-2144 BUG-2145 | ai:claude
        #[clap(long)]
        sandbox: bool,
        /// Run the full setup (worktree, lease, manifest) but skip the
        /// final `claude` exec. Useful for scripting / debugging the
        /// resolver.
        #[clap(long)]
        no_launch: bool,
        /// Plan-only mode: launch a PLANNING session for the spec instead of
        /// an implementing one. Runs `/aida-plan <SPEC>` (not `/aida-pickup`)
        /// and defaults the permission mode to `plan` (read-only) so the
        /// session writes a `docs/plans/` file without touching code. After
        /// the plan lands, promote the spec with `aida plan promote <SPEC>`
        /// (Approved -> Planned). Lets you plan work ahead while another spec
        /// is being implemented (the parallel-pipelining workflow). Headless
        /// auto-complete plan-phase is separate; this flag is interactive
        /// only, so it conflicts with --auto-complete for now.
        // trace:STORY-265 | ai:claude
        #[clap(long, conflicts_with = "auto_complete")]
        plan_only: bool,
        /// Guided keystone-implementation mode: launch a structured,
        /// step-by-step decision dialog for the spec instead of a normal
        /// pickup. Runs `/aida-guided-implement <SPEC>` — the agent surfaces
        /// each major architectural / security / keystone fork as a
        /// structured question (options + consequences + a recommendation +
        /// an "enter your own" escape), records each answer as a traceable
        /// ADR linked to the spec, agrees the approach before any code, then
        /// implements between answers. For supervised work that must NOT run
        /// unattended, so it is interactive-only and conflicts with the
        /// autonomous drain. Finishes with a PR for human review (no
        /// auto-merge). Pass an explicit spec id.
        // trace:STORY-735 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, conflicts_with_all = ["autonomous", "plan_only"])]
        guided: bool,
        /// Plan-then-ship: run a PLAN phase before the implementer phase, then
        /// the normal autonomous drain. The plan phase produces a `docs/plans/`
        /// file (the same planning session as `--plan-only`) and promotes the
        /// spec Approved -> Planned; the drain then implements, CIs, reviews,
        /// merges, pulls, and builds as usual. Opt-in — without this flag the
        /// drain starts straight at the implementer phase (default unchanged).
        /// Only meaningful with an autonomous drain (`--auto-complete` /
        /// `--drain`). Composes with `--no-human` (the plan phase runs headless
        /// too when the implementer does).
        // trace:STORY-265 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, requires = "autonomous")]
        with_plan: bool,
        /// Override the inferred role. Without this, the role is derived
        /// from the queue items' `for_role` (single-item: that item's
        /// role; cluster: majority with a warning about minority items).
        /// Falls back to active shell role when items have no `for_role`.
        #[clap(long, value_name = "NAME")]
        role: Option<String>,
        /// Skip the pre-pickup `aida db sync --pull`. By default queue
        /// work pulls the orphan-store branch first so the queue view is
        /// fresh; pass this when offline or when the user has just
        /// pulled in another shell.
        #[clap(long)]
        no_pull: bool,
        /// Cluster-mode filter: when `id` resolves to a parent scope,
        /// only drain queued children whose requirement type matches.
        /// E.g. `--type bug` to grab just the bug cluster under a
        /// given epic. Case-insensitive. Ignored in single-item / head
        /// pickup modes.
        #[clap(long = "type", value_name = "TYPE")]
        type_filter: Option<String>,
        /// Override the auto-derived branch name. Mirrors
        /// `aida session start --branch`.
        #[clap(long)]
        branch: Option<String>,
        /// Override the auto-derived worktree path. Mirrors
        /// `aida session start --path`.
        #[clap(long)]
        path: Option<String>,
        /// Stack this session's branch atop the most recent un-merged
        /// in-flight implementer branch instead of forking from
        /// `origin/main`. Lets the user start spec Y while spec X's PR is
        /// still in CI/review — when X merges, `aida pull` auto-rebases Y
        /// onto the new main (slice 3). The resolver picks the freshest
        /// implementer lease whose branch is not yet merged; the lease
        /// covering cwd is skipped so a session can't stack on itself.
        /// Mutually exclusive with `--base`.
        // trace:STORY-248 | ai:claude
        #[clap(long, conflicts_with = "base")]
        stack: bool,
        /// Explicit base branch for this session — fork from `BRANCH`
        /// instead of `origin/main`. The branch must exist locally or on
        /// origin. Refuses if the branch's PR has already merged
        /// (suggests `aida pull` + a fresh `--base main`); pass
        /// `--force-base` to override.
        // trace:STORY-248 | ai:claude
        #[clap(long, value_name = "BRANCH", conflicts_with = "stack")]
        base: Option<String>,
        /// Bypass the `--base BRANCH` safety check that refuses an
        /// already-merged base. Use when you know the rebase will still
        /// apply cleanly (e.g. base merged but branch tip hasn't been
        /// reused). Only meaningful with `--base`.
        // trace:STORY-248 | ai:claude
        #[clap(long = "force-base")]
        force_base: bool,
        /// Override the In-Progress guard when the target scope is already
        /// held by another active lease. Without --steal, `aida queue work`
        /// refuses and names the holding lease. With --steal, the holding
        /// session is ended first (`aida session end <id>` — clean exit;
        /// uncommitted work still blocks, exit those manually or use
        /// `aida session end --force`; a live process using the worktree
        /// also blocks, stop it or use `aida session end --include-in-use`)
        /// and the new session takes over.
        // trace:TASK-81 | ai:claude
        // trace:BUG-1986 | ai:claude
        #[clap(long)]
        steal: bool,
        /// Claim a spec whose status is ambiguous: already In Progress with
        /// no local lease, or Needs Attention after an autonomous punt. Also
        /// the one recovery verb for an ORPHANED lease — a lease whose owning
        /// session has exited is released and the scope taken, but only when
        /// that session's process is verifiably gone AND its worktree is
        /// clean. A live session, or an orphan holding uncommitted work, still
        /// refuses. Terminal statuses and Draft still refuse. Does NOT cross
        /// role routing: a spec routed to another role still requires `aida
        /// role enter <role>` first.
        // trace:TASK-559 | ai:codex
        // trace:BUG-777 | ai:claude
        // trace:BUG-1126 | ai:claude
        #[clap(long)]
        force_claim: bool,
        /// Override the focus-scope drift guard: start work on a
        /// spec outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        /// Always overrides regardless of the configured policy.
        // trace:STORY-717 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        force: bool,
        /// Tag-driven batch pickup: instead of resolving from `id`, pick
        /// the head queued item tagged `batch:<NAME>`. Repeat the
        /// invocation to drain the batch one session per item (each
        /// session exits independently; the next pickup picks the new
        /// head). Mutually exclusive with positional `id` and
        /// `--type`. `--dry-run --batch NAME` lists members in pickup
        /// order without acting. Composes with `--auto-complete`:
        /// `--batch NAME --auto-complete` drains the whole batch
        /// autonomously — one full lifecycle per member — instead of one
        /// session per re-invocation.
        // trace:TASK-229, TASK-285 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "NAME", conflicts_with_all = ["id", "type_filter"])]
        batch: Option<String>,
        /// Ordered multi-batch auto-complete drain. Comma-separated names are
        /// drained left-to-right, exhausting one `batch:NAME` before the next.
        /// Equivalent comma-separated values are also accepted by `--batch`
        /// when paired with `--auto-complete`.
        // trace:TASK-310 | ai:codex
        #[clap(
            long,
            value_name = "A,B,C",
            requires = "auto_complete",
            conflicts_with_all = ["id", "batch", "type_filter", "dry_run"]
        )]
        batches: Option<String>,
        /// Coupled-sequential drain: with `--batch NAME --auto-complete`, drive
        /// the batch members on ONE shared branch in ONE worktree — each member
        /// is implemented + CI'd and committed in place (no per-member
        /// merge-to-main, no reset between members), then ONE cluster PR is
        /// opened linking every member. For TIGHTLY-COUPLED work that ships
        /// together, where later increments build on earlier commits. A member
        /// failure HALTS the drain, keeping prior members' commits on the
        /// branch. Contrast the default batch drain, which merges each member to
        /// main as its own PR. Requires `--batch` or `--batches`.
        // trace:TASK-1003, SPIKE-70 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, requires = "autonomous")]
        single_branch: bool,
        /// Coupled-ordered drain: with `--batch NAME --auto-complete`, drive the
        /// batch members in pickup order — each member forks off the
        /// freshly-pulled main, runs its full lifecycle, and merges as its OWN
        /// PR. For coupled-but-independently-shippable work that must land in
        /// order (each increment stays a reviewable PR to main). A member
        /// failure SHELVES that member and the drain continues with the rest —
        /// contrast `--single-branch`, which accumulates every member on one
        /// branch and HALTS on a failure. This names + guards that ordered,
        /// per-member-PR SHAPE; it does not itself pin concurrency. A
        /// single-batch drain (`--batch NAME`) runs strictly one member at a
        /// time at the default `[drain] pipeline_depth = 1`; raise that (max 3)
        /// and the pipelined scheduler starts a later member's implementer/CI
        /// leg while an earlier member waits, with merges still serialized one
        /// at a time. A `--batches A,B,C` chain ignores the depth entirely: the
        /// batches run in turn, and each batch's members one at a time. Requires
        /// `--batch` or `--batches`.
        // trace:TASK-1005, SPIKE-70 | ai:claude — plain `//` keeps the marker out of `--help`.
        // trace:TASK-185 | ai:claude — STORY-1091 made `[drain] pipeline_depth`
        // live for the SINGLE-batch drain, so the old "concurrency is pinned to
        // 1" sentence advertised a landed feature as absent. The depth is scoped
        // to that path: `handle_auto_complete_batch` calls
        // `drain_batch_pipelined_with_caps`, but `handle_auto_complete_batches`
        // goes through `drain_batch_chain_with_caps`, which is typed on the
        // non-pipelined `BatchDriver` and always calls `drain_batch_with_caps` —
        // so an UNQUALIFIED depth claim would be false for `--batches`.
        #[clap(long, requires = "autonomous", conflicts_with = "single_branch")]
        sequential: bool,
        /// Preview without acting. For a single spec: print the resolved
        /// plan — session id, worktree path, branch, role, skill, and the
        /// lease it WOULD set up — then exit, creating no worktree, no
        /// lease, and launching no session. With `--batch`: print the
        /// matching queue entries in pickup order. Lets you safely see what
        /// a pickup will do before committing to it.
        // trace:TASK-229 | ai:claude
        // trace:TASK-1053 | ai:claude
        #[clap(long)]
        dry_run: bool,
        /// Resume a prior `claude` conversation for this scope instead of
        /// cold-launching. Bare `--resume` continues the most recent
        /// recorded session for the scope; `--resume <session-id>` (or a
        /// unique prefix) continues a specific one. The conversation's
        /// JSONL persists after `aida session end` removes the worktree,
        /// so a session can be resumed days later. Mutually exclusive
        /// with `--fresh`.
        // trace:TASK-112 | ai:claude
        #[clap(
            long,
            value_name = "SESSION-ID",
            num_args = 0..=1,
            default_missing_value = "",
            conflicts_with = "fresh"
        )]
        resume: Option<String>,
        /// Force a cold launch even when prior `claude` sessions exist
        /// for this scope — suppresses the default resume prompt.
        // trace:TASK-112 | ai:claude
        #[clap(long)]
        fresh: bool,
        /// List the recorded `claude` sessions for this scope (most
        /// recent first) and exit without launching a session.
        // trace:TASK-112 | ai:claude
        #[clap(long)]
        list_sessions: bool,
        /// Caller-minted Claude conversation id (a UUID) to launch with,
        /// instead of the auto-generated one. The AIDA TUI passes this so
        /// it can track + later resume the conversation deterministically.
        /// Implies a fresh launch — mutually exclusive with `--resume`.
        // trace:STORY-132, EPIC-26 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "UUID", conflicts_with = "resume")]
        session_id: Option<String>,
        /// Which vendor CLI runs this work: `claude`, `codex`, or `agy` (alias:
        /// `--agent`, matching `aida agent new <vendor>`). Governs BOTH the
        /// interactive host (the AIDA TUI passes `--vendor codex` to host a
        /// Codex tab; Codex has no caller-minted session id, so it hosts a
        /// fresh session and ignores `--session-id` / `--resume`) AND, on an
        /// autonomous `--auto-complete` drain, the headless implementer (and
        /// any headless orchestrator phase) — so `--vendor codex --auto-complete
        /// --no-human=both` really drains on Codex instead of being silently
        /// ignored. Headless-vendor resolution order: this flag > the
        /// `AIDA_HEADLESS_VENDOR` env > the `[orchestrator] headless_vendor` /
        /// `[agents] vendor` config knob > `claude` (default).
        // trace:TASK-895 STORY-761 TASK-1116 | ai:claude
        #[clap(long, visible_alias = "agent", value_name = "VENDOR")]
        vendor: Option<String>,
        /// Opaque model name passed through to the selected vendor CLI for this
        /// run. Overrides `[agents.<vendor>] model`; empty config means vendor
        /// default. AIDA does not validate model names.
        // trace:STORY-1003 | ai:codex
        #[clap(long, value_name = "MODEL")]
        model: Option<String>,
        /// Drive the full implementer → CI → reviewer → merge → pull →
        /// build lifecycle for one SPEC in a single command, instead of
        /// running the 5+ steps by hand. The orchestrator spawns each
        /// Claude session, waits for it, and advances; the human only
        /// interacts inside the sessions themselves. Bare `--auto-complete`
        /// runs all six phases; the value picks a variant that stops early:
        ///   through-ci    — stop after CI is green (PR routed to reviewer)
        ///   through-merge — stop after the merge (skip pull + build)
        ///   skip-build    — phases 1-5 (skip the final build verify)
        /// Exit code is 0 on success, else the 1-based index of the phase
        /// that failed (1 implementer, 2 CI, 3 review, 4 merge, 5 pull,
        /// 6 build). Takes a SPEC id; with no id it picks the head of the
        /// active role's queue, the same way the no-arg `aida queue work`
        /// does. Or pass `--batch NAME` to drain a whole batch:
        /// `--batch NAME --auto-complete` runs one full lifecycle per batch
        /// member until the batch is empty, `--max` is reached, or a phase
        /// fails. The `nextN` keyword (e.g. `aida queue work next3
        /// --auto-complete`) is the un-pre-tagged equivalent — it drains the
        /// next N items from the queue head in order.
        // trace:STORY-246, TASK-285, TASK-293 | ai:claude
        // trace:TASK-487 | ai:claude
        // TASK-560: `resume` is intentionally NOT in this clap conflict list.
        // The pair is still rejected, but by a manual check in the handler that
        // explains WHY they conflict and names both recovery paths — clap's
        // auto-generated "cannot be used with" message left the operator
        // stuck. trace:TASK-560 | ai:claude
        #[clap(
            long,
            value_name = "MODE",
            num_args = 0..=1,
            default_missing_value = "full",
            group = "autonomous",
            conflicts_with_all = [
                "no_launch", "fresh", "list_sessions",
                "dry_run", "type_filter",
            ]
        )]
        auto_complete: Option<String>,
        /// Drain the queue: try to ship every drivable queued item in order,
        /// fully autonomously, skipping items that can't be driven (blocked,
        /// human-only, already in flight). A discoverable shorthand for
        /// `--auto-complete --no-human=both --max <queue-size>`. Composes with
        /// `--batch` (drain that batch instead of the whole queue) and
        /// `--max-failures` (cap the shelving budget). Explicit `--auto-complete`,
        /// `--no-human`, or `--max` flags override the shorthand's defaults.
        // trace:TASK-578 | ai:claude — plain `//` keeps the marker out of `--help`.
        // `group = "autonomous"` lets `--drain` satisfy the `requires` on the
        // auto-complete-only flags (`--max-failures`, `--json`, …) the same way
        // an explicit `--auto-complete` does. trace:TASK-578 | ai:claude
        #[clap(long, group = "autonomous")]
        drain: bool,
        /// With `--auto-complete`: emit one JSON line per phase transition
        /// on stdout (machine-readable progress for TUI / scripting) instead
        /// of the human-readable progress lines.
        // trace:STORY-246 | ai:claude
        // TASK-578: `requires = "autonomous"` so `--drain` (a group member)
        // satisfies it the same way `--auto-complete` does.
        #[clap(long, requires = "autonomous")]
        json: bool,
        /// With `--batch NAME --auto-complete`: stop the batch drain after
        /// N members ship, even when the batch has more queued. Without it
        /// the drain runs until the batch is empty for the role. Only
        /// meaningful for a batch drain.
        // trace:TASK-285 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain` satisfies it.
        #[clap(long, value_name = "N", requires = "autonomous")]
        max: Option<usize>,
        /// With `--auto-complete`: after this many phase failures shelve in
        /// a single batch, stop the drain entirely rather than continue
        /// past the wreck (the environment is probably broken — gh
        /// rate-limited, CI infra down, etc.). Default is 5. Set to 0 to
        /// fall back to the historical "first failure stops" behaviour.
        /// Per-batch — a `--batches A,B,C` chain gets an independent budget
        /// for each.
        // trace:EPIC-28 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain --max-failures`
        // composes (the spec's named composition case).
        #[clap(long, value_name = "N", requires = "autonomous")]
        max_failures: Option<usize>,
        /// With `--auto-complete`: hard budget cap on cumulative reported tokens
        /// (input + output + cache) across the whole drain. Once the headless
        /// phases' accumulated usage crosses this, the drain stops cleanly at
        /// the next spec boundary rather than burning more quota on a wedged
        /// chunk. A backstop for unattended drains; composes with
        /// `--max-failures` and the goal condition (whichever fires first).
        // trace:TASK-966 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "N", requires = "autonomous")]
        max_tokens: Option<u64>,
        /// With `--auto-complete`: stop the drain before starting the next spec
        /// once this many specs have been acted on (shipped / punted /
        /// escalated / shelved). A hard iteration cap for unattended drains.
        // trace:TASK-966 | ai:claude
        #[clap(long, value_name = "N", requires = "autonomous")]
        max_iterations: Option<u64>,
        /// With `--auto-complete`: wall-clock budget for the whole drain. A bare
        /// number is minutes; suffixes / compounds work too (`90s`, `45m`,
        /// `2h`, `1h30m`). The drain stops between specs once the deadline
        /// passes — in-flight work is never interrupted mid-phase.
        // trace:TASK-966 | ai:claude
        #[clap(long, value_name = "DUR", requires = "autonomous")]
        max_runtime: Option<String>,
        /// With `--auto-complete`: minutes a *headless* phase may make no
        /// commit / file-change before the watchdog kills + shelves it (a
        /// degenerate echo/sleep spin). Default 10; `0` disables. Overrides
        /// `[drain] no_progress_minutes`.
        // trace:BUG-420 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain` satisfies it.
        #[clap(long, value_name = "MIN", requires = "autonomous")]
        no_progress_minutes: Option<u64>,
        /// With `--auto-complete`: hard wall-clock ceiling (minutes) per
        /// *headless* phase — a backstop in case progress-detection misses.
        /// Default 45; `0` disables. Overrides `[drain] phase_ceiling_minutes`.
        // trace:BUG-420 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain` satisfies it.
        #[clap(long, value_name = "MIN", requires = "autonomous")]
        phase_ceiling_minutes: Option<u64>,
        /// Resume a crashed `--auto-complete` drain from `.aida/drain-state.json`
        /// instead of starting fresh. Probes git/PR/spec reality to re-enter at
        /// the first incomplete phase (never re-merging a merged PR), and
        /// refuses if the original orchestrator process is still alive
        /// (double-drive guard). Pair with `--dry-run` to preview the plan
        /// without re-entering. Distinct from `--resume` (which continues a
        /// Claude *session*).
        // trace:STORY-492 | ai:claude
        #[clap(long, requires = "auto_complete")]
        resume_drain: bool,
        /// With `--resume-drain`: only resume if the recorded drain matches this
        /// id (its run UUID or start-timestamp prefix) — a guard against
        /// resuming a stale state file.
        // trace:STORY-492 | ai:claude
        #[clap(long, value_name = "ID", requires = "resume_drain")]
        drain_id: Option<String>,
        /// With `--resume-drain` or `--from-pr`: print the reconciled re-entry
        /// plan (which phase the drive would re-enter at) WITHOUT re-entering.
        /// The safe way to preview a resume / PR-only drive. (The plain
        /// `--dry-run` cannot be combined with `--auto-complete`, hence a
        /// dedicated flag.)
        // trace:STORY-492 | ai:claude
        // trace:TASK-405 | ai:claude — now also previews a `--from-pr` drive.
        #[clap(long, requires = "auto_complete")]
        resume_dry_run: bool,
        /// Refuse to launch the drain unless this dev build of aida matches
        /// (or is ahead of) the default branch HEAD. Without it, a stale dev
        /// build only prints a one-line warning and the wave runs it. Also
        /// enabled by `[drain] require_head = true` in `.aida/config.toml`.
        /// Released binaries are never checked.
        // trace:STORY-1414 | ai:claude
        #[clap(long, requires = "auto_complete")]
        require_head: bool,
        /// PR-only invocation: implementation already shipped OUTSIDE the
        /// orchestrator (a PR is already open for the spec), so SKIP the
        /// implementer phase and drive the remaining phases
        /// (reviewer → CI → merge → pull → build). Probes the PR's real state
        /// to pick the entry phase: CI not green → reviewer (CI-wait is
        /// implementer-coupled and cannot be re-run by a fresh process);
        /// reviewer done → merge; etc. Refuses cleanly if no open PR exists,
        /// the PR is already merged, or the spec is already Completed.
        /// Composes with `--auto-complete` and its `through-ci` / `through-merge`
        /// / `skip-build` variants. Distinct from `--resume-drain` (which
        /// recovers a CRASHED orchestrator's own drain from its state file);
        /// `--from-pr` engages a FRESH orchestrator on a PR that progressed
        /// outside it.
        // trace:TASK-405 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, requires = "auto_complete", conflicts_with = "resume_drain")]
        from_pr: bool,
        /// Run `--auto-complete` phases headless (`claude -p`) so the drain
        /// needs no Ctrl+D.
        ///
        /// reviewer-only (default, also bare `--no-human`): phase 3
        /// (reviewer) runs headless; phase 1 (implementer) stays interactive
        /// and the drain PAUSES there for you.
        ///
        /// both: full headless drain, implementer included. On a design-fork
        /// it cannot safely resolve the implementer punts the spec to Needs
        /// Attention rather than guessing — triage punts with
        /// `aida findings list`.
        ///
        /// Headless sessions force `--permission-mode bypassPermissions` and
        /// log JSON output under `.aida/headless-logs/`. Aliases:
        /// `--unattended`, `--headless`.
        // trace:STORY-263, TASK-306, STORY-276 | ai:claude — plain `//` so the
        // marker stays out of `--help` (TASK-268 user-facing-text convention).
        #[clap(
            long,
            value_name = "MODE",
            num_args = 0..=1,
            default_missing_value = "reviewer-only",
            aliases = ["unattended", "headless"]
        )]
        no_human: Option<String>,
        /// Under `--no-human=both`: leave a spec parked in Needs Attention
        /// when the headless advisor escalates its design-fork to a human,
        /// and advance the drain (the default). A paused spec beats a
        /// guessed one — the conservative choice. Mutually exclusive with
        /// `--escalate-defaults`; only meaningful with `--no-human=both`.
        // trace:STORY-306 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, conflicts_with = "escalate_defaults")]
        escalate_blocks: bool,
        /// Under `--no-human=both`: when the headless advisor escalates a
        /// design-fork, resume the implementer to ship the defensible
        /// default rather than parking the spec, and file a needs-human
        /// finding for post-hoc review. For mechanical batches where
        /// throughput beats per-spec correctness. Only meaningful with
        /// `--no-human=both`.
        // trace:STORY-306 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        escalate_defaults: bool,
        /// "Advisor on standby" autonomy mode: auto-resolve mechanical
        /// confirmation prompts (open PR? grab next item? end session?)
        /// without pausing, while still pausing for genuine design-fork
        /// questions. Use it when you are at the keyboard but don't want to
        /// click-yes through every mechanical step. Sets `AIDA_ZEN=1` in the
        /// launched session (exporting `AIDA_ZEN=1` yourself is equivalent).
        /// When combined with `--no-human`, `--no-human` wins (it is the
        /// stronger mode) and a warning is printed.
        // trace:STORY-287 | ai:claude — plain `//` so the marker stays out
        // of `--help` output (TASK-268 user-facing-text convention).
        #[clap(long)]
        zen: bool,
        /// With `--zen`: always pause at the grab-next/stop checkpoint after
        /// the PR opens, even on a clean finish — for when you want to drive
        /// grab-next by hand. By default a clean `--zen` finish (no human ever
        /// needed) auto-runs `aida session end` and exits. Equivalent to
        /// `[zen] auto_exit = false` in `.aida/config.toml`, per-invocation.
        /// No effect without `--zen`.
        // trace:STORY-564 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        pause_always: bool,
        /// Suppress the end-of-command summary a standalone
        /// `aida queue work <PR-N> --role reviewer` prints (verdict, cost,
        /// artifact paths). For scripted consumers that read the verdict
        /// file `.aida/review-verdicts/PR-N.json` directly. No effect on
        /// non-reviewer or orchestrator-driven runs.
        // trace:BUG-226 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        quiet: bool,
        /// Suppress the high-signal headless tee under `--no-human`. By
        /// default the parent prints `│ [headless] ...` lines for the
        /// headless Claude's `system.init`, `assistant.text`, `tool_use`,
        /// and `result` events so the operator can follow progress without
        /// a second terminal. With this flag (or `AIDA_TEE_HEADLESS=0`),
        /// the chatter is silenced but failures (`is_error`,
        /// `permission_denials`) still stream — they must NEVER hide.
        // trace:TASK-307 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_tee_headless: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Calibration mode override — turn the cold-boot vs fork-from-live
        /// advisor calibration ledger ON for *this* drain regardless of
        /// `[advisor] calibration_mode`. With it on, every punt produces
        /// both a cold-boot verdict (which drives the drain) and a
        /// fork-from-live verdict (shadow only) when a live advisor is
        /// registered. Recorded to `.aida/punts/<punt-id>/calibration.yaml`.
        /// Only meaningful with `--no-human=both`.
        // trace:STORY-347 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, conflicts_with = "no_calibrate")]
        calibrate: bool,
        /// Calibration mode override — turn calibration OFF for *this*
        /// drain regardless of `[advisor] calibration_mode`. Pair with
        /// `--calibrate` to flip the toggle per-drain.
        // trace:STORY-347 | ai:claude
        #[clap(long)]
        no_calibrate: bool,
        /// Opt out of the reviewer pre-flight stale-base refusal. When
        /// the orchestrator (phase 3) or a direct `aida queue work
        /// <PR-N> --for reviewer` detects that the PR's base is behind
        /// origin AND a file the PR touches has also moved on the base
        /// since the PR forked, the reviewer is refused so reviews
        /// don't run against stale code. Pass this flag to override
        /// the refusal and review against stale anyway. The "stale +
        /// no file overlap" case prints a warning either way.
        // trace:STORY-281 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        allow_stale_base: bool,
        /// Opt out of the reviewer pre-flight intermediate-only refusal.
        /// By default the reviewer refuses a PR whose diff changes ONLY
        /// intermediate/generated files (build outputs, gitignored
        /// paths, lockfiles with no source change) because such a fix is
        /// not reproducible — it's overwritten on the next build. Pass
        /// this flag when the PR is a deliberate regeneration of
        /// checked-in build output.
        // trace:TASK-480 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        allow_intermediate_only: bool,
        /// Opt out of phase-3 auto-rebase recovery. By default a fully
        /// headless auto-complete drain that hits the reviewer stale-base
        /// pre-flight attempts one clean `aida pr rebase` before refusing.
        // trace:STORY-429 | ai:claude
        #[clap(long)]
        no_auto_rebase: bool,
        /// Implementer's pickup-time complexity estimate:
        /// `low` / `med` / `high`. Captured to
        /// `.aida/complexity-calibration/<SPEC>.yaml` AND stamped onto
        /// the spec as a `complexity:<level>` tag so existing tag
        /// tooling (e.g. `aida queue list --tag-prefix complexity:`)
        /// works on the new dimension. Best-effort, not graded — feeds
        /// the three-way calibration view, never an approval gate.
        /// Absent ⇒ no pickup slot is captured.
        // trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_enum, value_name = "LEVEL")]
        complexity: Option<crate::complexity_calibration::ComplexityLevel>,
        /// Implementer's pickup-time assistance estimate:
        /// `none` / `advisor` / `human` — how much help the spec is
        /// expected to need. Captured alongside `--complexity` and
        /// stamped as `estimated-assistance:<level>`. The actual
        /// intervention count comes from the punt ledger
        /// (`.aida/punts.jsonl`); this flag captures only the
        /// prediction.
        // trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long = "assist-est", value_enum, value_name = "LEVEL")]
        assist_est: Option<crate::complexity_calibration::AssistanceLevel>,
        /// Pickup-time effort estimate: 15m, 1h, 4h, 1d, or 1w.
        /// Captured as the `plan` touchpoint for queued work pickup
        /// (post-plan/implementation-brief estimate). `1d` is 8
        /// work-hours; `1w` is 40 work-hours.
        // trace:STORY-451 | ai:codex
        #[clap(long, value_enum, value_name = "BUCKET")]
        effort: Option<crate::effort_calibration::EffortBucket>,
        /// Host each fanned-out implementer in its own titled terminal window
        /// instead of an invisible background process. Currently supports tmux:
        /// pass `--panes` (defaults to tmux) or `--panes tmux`. Each implementer
        /// runs in a window named by its spec under a dedicated `aida-drain` tmux
        /// session, so the fan-out is visible and the windows survive the
        /// launching terminal closing (tmux server persistence) — you own pane
        /// cleanup. Degrades gracefully: outside tmux (or if tmux is unavailable)
        /// the drain falls back to the background spawn with a one-line notice,
        /// never failing. Drain semantics (leases, phases, exit codes) are
        /// unchanged.
        // trace:TASK-1120 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "HOST", num_args = 0..=1, default_missing_value = "tmux")]
        panes: Option<String>,
        /// Refuse auto-queuing Approved-but-not-queued specs. Under --strict,
        /// aida queue work refuses with the status-aware recovery hint.
        // trace:TASK-547 | ai:antigravity
        #[clap(long)]
        strict: bool,
    },
    /// Show what an active session has shipped so far alongside what
    /// remains. Bucketed view (Shipped / In flight / Working now /
    /// Remaining) with a net summary line. Default resolves to the
    /// most-recent active session; `--session ID` targets a specific
    /// lease; `--batch NAME` reads members of a `batch:NAME` tag instead
    /// of a session manifest.
    ///
    /// Buckets:
    ///   Shipped     — status Completed/Rejected (merged to default branch)
    ///   In flight   — status Done (work finished on a branch; PR open)
    ///   Working now — status InProgress
    ///   Remaining   — Approved/Planned/Draft (still queued)
    ///
    // trace:TASK-232 | ai:claude
    Progress {
        /// Specific session id (8+ char prefix) to read the manifest of.
        /// Default: the lease covering cwd, else the most-recent active
        /// lease.
        // trace:TASK-232 | ai:claude
        #[clap(long, value_name = "SESSION_ID")]
        session: Option<String>,
        /// Resolve items from a `batch:NAME` tag instead of a session
        /// manifest. Composes with the batch-drain convention: items
        /// tagged `batch:NAME` (set via `aida add --tags` or
        /// `aida edit --tags`) are members of the batch.
        /// Mutually exclusive with `--session`.
        // trace:TASK-232, TASK-229 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "NAME", conflicts_with = "session")]
        batch: Option<String>,
        /// Filter items to those modified after this timestamp
        /// (RFC3339, an ISO date interpreted as local midnight, a zone-less
        /// ISO datetime in local time, or `<N>{m,h,d,w}` ago, e.g. `2d`,
        /// `12h`, `2w`, `24 hours ago`). Useful for
        /// "what changed since yesterday" snapshots when no manifest
        /// or batch tag applies.
        // trace:TASK-232 | ai:claude
        #[clap(long, value_name = "TS")]
        since: Option<String>,
        /// Show every member by id under each bucket, not just the
        /// first few. Default truncates Remaining at 8 items with a
        /// `…and N more` tail.
        // trace:TASK-232 | ai:claude
        #[clap(long)]
        verbose: bool,
        /// Machine-readable JSON: `{queued, in_progress, done, shipped,
        /// unresolved, shelved, total, buckets}`. Same document as
        /// `--format json`.
        // trace:BUG-1289 | ai:claude
        #[clap(long)]
        json: bool,
    },
    /// Flip a spec's status, route it to a role's queue, and (optionally)
    /// launch a session — encapsulates the recurring implementer →
    /// reviewer → fixup recovery sequence into a single verb. Replaces
    /// the manual three-step:
    ///
    ///   aida edit SPEC --status in-progress
    ///   aida queue add SPEC --for ROLE
    ///   aida queue work SPEC
    ///
    /// Smart status transitions (overridable with `--status`). Metadata-only
    /// rework leaves the spec CLAIMABLE, so it never targets InProgress —
    /// that status is only valid when `--work` establishes the lease behind it.
    ///
    ///   current          metadata-only        with --work
    ///   ───────────────  ───────────────────  ───────────────────
    ///   Draft            Approved (advisor)   no flip
    ///   Approved         no flip              no flip
    ///   Planned          Approved             InProgress
    ///   InProgress       Approved             no flip
    ///   NeedsAttention   Approved (advisor)   InProgress (advisor)
    ///   Done             Approved             InProgress
    ///   Completed        Approved             InProgress
    ///   Rejected         Approved             Approved
    ///   Superseded       no flip              no flip
    ///
    ///   (advisor) — promoting an un-triaged Draft or a punted
    ///     NeedsAttention spec is an advisor act; run as advisor or from an
    ///     interactive terminal, exactly as with `aida edit` and `queue add`.
    ///   Completed and Rejected additionally require `--force`, since
    ///     re-opening closed work is usually a mistake.
    ///   Reworking an already-InProgress spec WARNS and proceeds; `--force`
    ///     silences the warning. It is refused only while another session
    ///     holds a live claim on the spec, and `--force` does not override that.
    ///
    /// With no ID, triage the specs parked in Needs Attention: at a terminal,
    /// each one shows what a requeue would do, then takes one key:
    /// [r] requeue, [s] skip, [o] show, [d] decide (only when a decision is
    /// open), [q] quit. Without a terminal it prints the requeue command for
    /// each parked spec and exits 0.
    ///
    // The doc block above is a TABLE plus a literal three-command sequence;
    // clap reflows doc comments by default, which ran the rows together into
    // unreadable prose. It was already doing that to the three-step block
    // before this change. verbatim_doc_comment keeps the line breaks.
    // trace:BUG-1470 | ai:claude
    #[clap(verbatim_doc_comment)]
    // trace:TASK-218 | ai:claude
    Rework {
        /// Requirement ID (UUID or SPEC-ID). Omit it to triage the parked
        /// specs one keystroke each.
        // trace:STORY-1429 | ai:claude
        id: Option<String>,
        /// Also launch a session for the spec (chains `aida queue work`).
        /// Without this, rework is metadata-only: reset to a claimable status
        /// and queue it. With this, the launched session owns InProgress.
        #[clap(long)]
        work: bool,
        /// Override the routing role. Default: existing queue route,
        /// falling back to implementer.
        #[clap(long)]
        r#for: Option<String>,
        /// Re-queue at the tail instead of the default head position.
        // trace:BUG-851 | ai:codex
        #[clap(long)]
        tail: bool,
        /// Override the smart target status. Pass any status string
        /// (`in-progress`, `approved`, `planned`, …); the smart table
        /// above is bypassed.
        #[clap(long, value_name = "STATE")]
        status: Option<String>,
        /// Capture a comment on the spec at rework time (added via the
        /// same path as `aida comment add`). Useful for the audit trail
        /// of why a Done or Completed spec is being re-opened.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        reason: Option<String>,
        /// Chain `aida queue work --resume` to resume a prior claude
        /// session for this spec instead of cold-launching. Implies
        /// `--work`.
        #[clap(long)]
        resume: bool,
        /// Bypass the terminal-status guard (Completed/Rejected) and
        /// the "already in progress" guard. Mirrors `queue add --force`.
        // trace:TASK-45 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force: bool,
        /// When chaining `--work`, end any session holding the target
        /// scope first (passes through to `aida queue work --steal`).
        // trace:TASK-81 | ai:claude
        #[clap(long)]
        steal: bool,
        /// Permission mode (only used with `--work`; passed to
        /// `aida queue work --permission-mode`).
        #[clap(long, value_name = "MODE")]
        permission_mode: Option<String>,
        /// Skip the pre-pickup `aida db sync --pull` (only used with
        /// `--work`; passed to `aida queue work --no-pull`).
        #[clap(long)]
        no_pull: bool,
        /// User ID (defaults to AIDA_USER or system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Recover a spec from a failed phase-1 implementer session — an
    /// interactive wizard over the existing recovery primitives.
    ///
    /// After a phase-1 failure (an Anthropic 529, a commit-and-exit without a
    /// PR, partial work, an external crash), recovery is a mechanical sequence
    /// of git/gh/aida commands whose exact shape depends on the spec's state:
    /// does a lease still hold it? is there an open/merged PR? are there commits
    /// ahead of `origin/main` that were never pushed? is the worktree dirty?
    /// This command inspects that state, recommends a recovery path, and steps
    /// through it interactively — instead of you remembering the dance each time.
    ///
    /// It is a FRONT-END over existing primitives, not new mechanism: it reuses
    /// the same lease probes as `aida session leases`, the same PR/branch probes
    /// the orchestrator uses, drives phases 3-6 via the PR-only orchestrator
    /// path when a PR is already open, and falls back to `aida pull` / session
    /// cleanup / re-queue for the other cases. Recommended actions:
    ///   • open PR (pushed)        → drive phases reviewer → merge → pull → build
    ///   • commits, not pushed     → push + open PR + drive phases
    ///   • commits + dirty worktree→ commit WIP, then push + PR + drive
    ///   • no commits + dirty      → commit WIP and park for resumption
    ///   • no commits + clean      → end the lease and re-queue
    ///   • PR merged / spec done   → pull / nothing to do (already shipped)
    Recover {
        /// Spec to recover (UUID or SPEC-ID).
        id: String,
        /// Print the inspection result + recommended recovery plan WITHOUT
        /// executing anything. The safe way to see the state read before acting.
        #[clap(long)]
        dry_run: bool,
        /// Skip all confirmation prompts and run the recommended path
        /// non-interactively (for scripted / headless use). NOT the default.
        #[clap(long, visible_alias = "yes")]
        auto: bool,
        /// User ID (defaults to AIDA_USER or system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Integrate finished work: serialize the back-end merge phases over every
    /// spec that's Done with an open PR.
    ///
    /// This is the consumer half of a producer/consumer split. Implementers
    /// (the producers, working in parallel on isolated scopes) finish their
    /// work, flip a spec to Done, and leave an open PR — they never merge. This
    /// command (the single, serial consumer) watches for that pair — Done + open
    /// PR — and drives the remaining phases (reviewer → CI → merge → pull →
    /// build) on each in turn, ONE AT A TIME. Merging into the default branch is
    /// a shared, serial resource, so there is exactly one merge authority.
    ///
    /// The handoff is the substrate itself: the loop polls for the Done +
    /// open-PR state, no message bus needed. Each ready spec is driven through
    /// the same PR-only orchestrator path as `aida queue work <id>
    /// --auto-complete --from-pr` (implementation shipped outside this loop, so
    /// the implementer phase is skipped). A spec whose PR is already merged, or
    /// has no open PR, or whose PR probe is inconclusive (gh missing / auth /
    /// network) is reported and skipped — never re-driven, never guessed.
    Integrate {
        /// Inspect the ready-for-integration set and print what WOULD be driven,
        /// without merging anything. The safe way to see the loop's decision.
        #[clap(long)]
        dry_run: bool,
        /// Run a single pass over the ready set and exit (the default). Kept as
        /// an explicit flag for symmetry with --watch.
        #[clap(long)]
        once: bool,
        /// Keep watching: after each pass, BLOCK until a drain event lands (a PR
        /// is ready / CI reaches a verdict / a PR merges) and rescan, integrating
        /// newly-ready specs as producers ship them. Event-driven — it wakes on
        /// real activity, not a blind timer; the idle backstop only bounds the
        /// quiet-period rescan. Without this the command makes a single pass.
        #[clap(long)]
        watch: bool,
        /// Idle backstop (seconds) for --watch: the longest the loop waits with no
        /// drain event before a full rescan. Superseded by --idle-minutes when
        /// that is given. Default 60.
        #[clap(long, value_name = "SECS", default_value_t = 60)]
        interval: u64,
        /// Cap the number of specs integrated this run (across all passes). 0 =
        /// no cap. A guardrail for a first cautious run.
        #[clap(long, value_name = "N", default_value_t = 0)]
        max: usize,
        /// Wait for in-flight CI on ready PRs before deciding whether to merge.
        /// Default behavior; accepted for explicit scripts.
        // trace:BUG-1052 | ai:codex
        #[clap(long, conflicts_with = "no_wait_ci")]
        wait_ci: bool,
        /// Restore the old one-pass behavior: skip PRs whose CI is still running.
        // trace:BUG-1052 | ai:codex
        #[clap(long)]
        no_wait_ci: bool,
        // trace:STORY-335 | ai:claude
        /// Rebase each member's PR branch onto current main before merging it.
        /// A deferred batch cuts every branch from the same stale main, so
        /// without this they merge un-rebased. Opt-in: composes `pr rebase`
        /// (force-push-with-lease); a rebase conflict skips that member and
        /// continues. Preview with --dry-run first.
        #[clap(long)]
        rebase: bool,
        // trace:STORY-335 trace:TASK-691 | ai:claude
        /// Accumulation strategy for the batch. `per-item` (default) = a branch
        /// + PR per item, rebased and merged in order. `one-branch` and
        ///   `stacked` are accepted but not built yet (they error with a pointer).
        ///   When omitted, falls back to `[integrate] strategy` in
        ///   .aida/config.toml, then `per-item`.
        #[clap(long, value_enum)]
        strategy: Option<crate::integrate::IntegrateStrategy>,
        // trace:TASK-1036 | ai:claude
        /// Scope the scan to a focus epic/spec and its transitive descendants —
        /// only PRs under that subtree are integrated. Overrides the per-worktree
        /// `aida focus` marker / AIDA_FOCUS for this run. Omit to use the active
        /// focus (if any), else scan the whole project.
        #[clap(long, value_name = "ID")]
        focus: Option<String>,
        // trace:TASK-1036 | ai:claude
        /// Idle backstop for `--watch`, in minutes: the longest the loop waits
        /// with no drain event before a full rescan. The loop is event-driven —
        /// it wakes immediately on a ready PR / CI verdict / merge — so this only
        /// bounds the quiet-period rescan. Defaults to `--interval` seconds.
        #[clap(long, value_name = "N")]
        idle_minutes: Option<u64>,
        // trace:STORY-647 | ai:claude
        /// Bypass the team RBAC guardrail (`[team.permissions] integrate`). The
        /// gate is a guardrail, not security — the bypass is recorded in history.
        #[clap(long)]
        force: bool,
        /// User ID (defaults to AIDA_USER or system user).
        #[clap(long)]
        user: Option<String>,
    },
}
