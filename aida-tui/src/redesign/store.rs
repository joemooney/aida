//! In-process spec/list reads for the redesign prototype (STORY-693).
//!
//! The prototype used to shell out to a fresh ~287MB `aida` subprocess for
//! every read — the scope item lists (`aida list … --json`) and the show
//! modal (`aida show <id> --no-git`). Each call cold-started the whole AIDA
//! runtime (config load, store attach, cache open + freshness check), which
//! is the source of the show/preview lag.
//!
//! This module opens the cache-backed git backend ONCE
//! ([`aida_core::CachedGitBackend`], the same read path the CLI `list` / `show`
//! use) and serves the scope lists + the show modal from it in-process —
//! microseconds per read, no subprocess. The backend is held on the app state
//! for the lifetime of the TUI session.
//!
//! `why` is deliberately NOT moved here: its classifier lives in
//! `aida-cli/burndown.rs`, not in `aida-core`, so factoring it in-process is a
//! separate task (see the `TODO(why in-process)` in `mod.rs`).
//!
//! trace:STORY-693 | ai:claude

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aida_core::{
    ArchiveFilter, CachedGitBackend, DatabaseBackend, DeferFilter, ListFilter, RelationshipType,
    RequirementSummary, RequirementsStore,
};

use super::state::{EpicRow, Scope, TargetItem};

/// A loaded spec for the show modal: structured fields + the description body,
/// read in-process. Rendered natively by the modal (replacing the captured
/// `aida show` stdout). trace:STORY-693 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSpec {
    pub id: String,
    pub title: String,
    pub req_type: String,
    pub status: String,
    pub priority: String,
    pub tags: Vec<String>,
    /// The raw description (markdown) body.
    pub description: String,
    /// The spec's comments (author + short time + markdown text), surfaced in
    /// the preview modal so the human can READ the advisor's disposition
    /// ("approved because X") inside the TUI. trace:TASK-932 | ai:claude
    pub comments: Vec<LoadedComment>,
    /// The spec's relationship graph — parent epic(s), children, blocked-by /
    /// blocks chains, and references — projected for the preview modal. This is
    /// AIDA's #1 differentiator (the typed requirement graph) surfaced at the
    /// natural dig-in gesture, the same edges `aida show` / `aida graph` print
    /// on the CLI.
    // trace:STORY-739 | ai:claude
    pub graph: SpecGraph,
}

/// One related spec, projected for the preview modal's graph section: its
/// display id, title, and status — enough to render a `<id> <title> [<status>]`
/// row with the cockpit's status glyph.
// trace:STORY-739 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRelation {
    pub id: String,
    pub title: String,
    pub status: String,
}

/// A spec's relationship graph, grouped by relation, for the preview modal.
/// Every group is independently empty-able so the renderer can omit groups that
/// have no edges (no empty headers). The grouping follows the stored convention
/// (TASK-679: a relationship's `rel_type` names the SOURCE's role relative to
/// the target), so a `Parent` edge on this spec points at a CHILD, a `Child`
/// edge points at the PARENT, and so on.
// trace:STORY-739 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecGraph {
    /// Parent epic(s) — resolved from this spec's outgoing `Child` edges.
    pub parents: Vec<LoadedRelation>,
    /// Children — resolved from this spec's outgoing `Parent` edges.
    pub children: Vec<LoadedRelation>,
    /// Hard blockers — this spec's outgoing `BlockedBy` edges (the spec is
    /// un-pickable until each reaches Completed).
    pub blocked_by: Vec<LoadedRelation>,
    /// Specs this one blocks — outgoing `Blocks` edges.
    pub blocks: Vec<LoadedRelation>,
    /// General references — outgoing `References` edges.
    pub references: Vec<LoadedRelation>,
}

impl SpecGraph {
    /// True when the spec has no relationships in any group — the renderer omits
    /// the whole graph section (rather than printing an empty header) in that
    /// case.
    // trace:STORY-739 | ai:claude
    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
            && self.children.is_empty()
            && self.blocked_by.is_empty()
            && self.blocks.is_empty()
            && self.references.is_empty()
    }
}

/// One comment on a spec, projected for the TUI preview: who wrote it, a short
/// timestamp, and the (markdown) body. The disposition the advisor records on a
/// spec lives here, so surfacing these is what makes "approved because X"
/// readable in the TUI. trace:TASK-932 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedComment {
    pub author: String,
    /// A short, human-readable timestamp (local time, `YYYY-MM-DD HH:MM`).
    pub when: String,
    /// The comment body (markdown).
    pub content: String,
}

/// Project a requirement's comments into the TUI's [`LoadedComment`] rows —
/// author, short local timestamp, and body, in stored order. A PURE function of
/// the requirement (no IO) so the extraction/format mapping is unit-testable:
/// a requirement with N comments yields N rows; an empty list yields none.
/// trace:TASK-932 | ai:claude
pub fn comments_from_requirement(req: &aida_core::Requirement) -> Vec<LoadedComment> {
    req.comments
        .iter()
        .map(|c| LoadedComment {
            author: c.author.clone(),
            when: format_comment_time(c.created_at),
            content: c.content.clone(),
        })
        .collect()
}

/// Format a comment's creation time as a short local-time stamp
/// (`YYYY-MM-DD HH:MM`). Timestamps render in the user's local timezone for
/// surface output (UTC stays on disk). trace:TASK-932 | ai:claude
fn format_comment_time(ts: chrono::DateTime<chrono::Utc>) -> String {
    ts.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Project a requirement's OUTGOING relationship edges into the grouped
/// [`SpecGraph`] the preview modal renders. `resolve` maps a target uuid to its
/// display row (id + title + status); a target that does not resolve (a
/// dangling edge to a deleted spec) is skipped so the modal never shows a bare
/// uuid. Grouping follows the stored convention (TASK-679: `rel_type` names the
/// SOURCE's role relative to the target) — a `Parent` edge points at a child, a
/// `Child` edge at the parent, `BlockedBy`/`Blocks`/`References` map directly.
/// `Duplicate`/`Verifies`/`VerifiedBy`/`Custom` are not surfaced in the modal's
/// graph section today. A PURE function of `(relationships, resolve)` — no IO —
/// so the rel_type→group mapping, target resolution, and empty-group elision are
/// unit-testable against a fixture.
// trace:STORY-739 | ai:claude
pub fn graph_from_relationships(
    relationships: &[aida_core::Relationship],
    resolve: impl Fn(uuid::Uuid) -> Option<LoadedRelation>,
) -> SpecGraph {
    let mut graph = SpecGraph::default();
    for rel in relationships {
        let Some(row) = resolve(rel.target_id) else {
            continue;
        };
        match &rel.rel_type {
            // This spec is the parent of the target → the target is a CHILD.
            RelationshipType::Parent => graph.children.push(row),
            // This spec is a child of the target → the target is the PARENT.
            RelationshipType::Child => graph.parents.push(row),
            RelationshipType::BlockedBy => graph.blocked_by.push(row),
            RelationshipType::Blocks => graph.blocks.push(row),
            RelationshipType::References => graph.references.push(row),
            // Not surfaced in the preview modal's graph section.
            RelationshipType::Duplicate
            | RelationshipType::Verifies
            | RelationshipType::VerifiedBy
            | RelationshipType::Custom(_) => {}
        }
    }
    graph
}

/// The in-process read handle: a cache-backed git backend opened once from the
/// project root. All redesign reads (scope lists + show modal) go through it,
/// so there is no per-read subprocess cold-start. trace:STORY-693 | ai:claude
pub struct SpecStore {
    backend: CachedGitBackend,
}

impl SpecStore {
    /// Open the cache-backed backend for the project rooted at `project_root`
    /// (the directory holding `.aida/config.toml`). Resolves the orphan-branch
    /// store worktree — including the sibling-worktree case where `.aida-store`
    /// only lives in the main worktree (BUG-331) — and opens (or rebuilds) the
    /// SQLite cache beside it. Returns `None` when no distributed store can be
    /// found (the prototype then falls back to empty lists rather than
    /// crashing). trace:STORY-693 | ai:claude
    pub fn open(project_root: &Path) -> Option<Self> {
        let store_path = resolve_store_path(project_root)?;
        let cache_path = CachedGitBackend::default_cache_path(&store_path);
        // Reads need neither the dispenser (id allocation is a write concern)
        // nor any forge feature — `open` builds a plain GitBackend + cache.
        let backend = CachedGitBackend::open(&store_path, &cache_path).ok()?;
        Some(SpecStore { backend })
    }

    /// Project every spec into the [`aida_core::liveness::SpecLivenessInput`]
    /// the in-process liveness pass needs: the display id, the two id forms a
    /// lease scope can match, whether it is In-Progress (an orphan-pass
    /// candidate), and whether it is a rollup/stateless type (epic / folder /
    /// meta) that never holds a spec-scoped lease. Uses `Both` archive + defer
    /// filters so the scan matches `aida ps`'s full-store sweep (which ignores
    /// the view flags). Cache-fast (`list_summaries`, no YAML parse). Empty on a
    /// read error — the cockpit then shows every row Idle rather than crashing.
    // trace:BUG-677 | ai:claude
    pub fn liveness_inputs(&self) -> Vec<aida_core::liveness::SpecLivenessInput> {
        let filter = ListFilter {
            archive: ArchiveFilter::Both,
            defer: DeferFilter::Both,
            ..Default::default()
        };
        let Ok(summaries) = self.backend.list_summaries(&filter) else {
            return Vec::new();
        };
        summaries
            .into_iter()
            .map(|s| {
                let disp = s
                    .agreed_id
                    .clone()
                    .or_else(|| s.spec_id.clone())
                    .unwrap_or_else(|| s.id.to_string());
                let in_progress = aida_core::RequirementStatus::from_filter_str(&s.status)
                    == Some(aida_core::RequirementStatus::InProgress);
                // Rollup / stateless types never hold a spec-scoped lease, so
                // `aida ps` excludes them from the orphan pass. Match the string
                // form (`ps_orphan_excluded_type_str` in aida-cli). trace:TASK-940
                let orphan_excluded = s.req_type.eq_ignore_ascii_case("Epic")
                    || s.req_type.eq_ignore_ascii_case("Folder")
                    || s.req_type.eq_ignore_ascii_case("Meta");
                aida_core::liveness::SpecLivenessInput {
                    disp,
                    agreed_id: s.agreed_id,
                    spec_id: s.spec_id,
                    in_progress,
                    orphan_excluded,
                }
            })
            .collect()
    }

    /// The target set for a functional scope, read from the cache in-process.
    ///
    /// * [`Scope::Backlog`] → the approved + planned slice.
    /// * [`Scope::Open`]    → every non-terminal spec (not Completed/Rejected),
    ///   mirroring `aida list open`.
    ///
    /// Non-functional scopes return an empty set.
    ///
    /// `focus` is the optional EPIC focus lens (STORY-695): when `Some`, the
    /// scope's item set is narrowed to specs in that set (the focus epic + its
    /// transitive children, as computed by [`Self::descendants_of`]). When
    /// `None`, behavior is unchanged (every scope-matching spec).
    /// trace:STORY-693 trace:STORY-695 | ai:claude
    pub fn scope_items(&self, scope: Scope, focus: Option<&HashSet<String>>) -> Vec<TargetItem> {
        // The Queue scope is NOT a status slice of the spec list — it reads the
        // role-routing queue (the same `aida queue list` data) and projects each
        // entry to a row carrying its routed role. Handled before the list query
        // so the unrouted-vanishing-Draft gap is closed. trace:TASK-948
        if scope == Scope::Queue {
            let mut rows = self.queue_items();
            if let Some(set) = focus {
                rows.retain(|it| focus_includes(set, it));
            }
            return rows;
        }
        // The Mail scope isn't a spec source at all — it reads the local
        // mailbox's unread inbox directly (no `self.backend`/git-cache
        // involvement, and no EPIC focus lens: mail isn't spec-parented, so
        // narrowing it to the focused epic's descendants would be a category
        // error). trace:STORY-701 | ai:claude
        if scope == Scope::Mail {
            return super::mail::fetch_mail_items();
        }
        let filter = ListFilter {
            // Backlog/Open both default to the active view (archived + deferred
            // rows hidden), matching the CLI `list` defaults.
            archive: ArchiveFilter::NonArchivedOnly,
            defer: DeferFilter::NonDeferredOnly,
            ..Default::default()
        };
        let summaries = match self.backend.list_summaries(&filter) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let items = summaries
            .into_iter()
            .filter(|s| scope_includes(scope, &s.status))
            .map(summary_to_item);
        // Apply the EPIC focus lens (if set) — a PURE narrow over the produced
        // rows. trace:STORY-695 | ai:claude
        let mut items: Vec<TargetItem> = match focus {
            Some(set) => items.filter(|it| focus_includes(set, it)).collect(),
            None => items.collect(),
        };
        // Test scope: mark each row that carries a `## Test Plan` section, by
        // loading its description in-process (the summaries don't carry the
        // body). The set is the shipped specs in focus, so this stays small.
        // trace:STORY-699 | ai:claude
        if scope == Scope::Test {
            for it in &mut items {
                if let Ok(Some(req)) = self.backend.get_requirement_by_spec_id(&it.id) {
                    it.has_test_plan = extract_test_plan(&req.description).is_some();
                }
            }
        }
        items
    }

    /// The role-routing queue, projected to [`TargetItem`] rows — the Queue
    /// scope's item set (TASK-948). Reuses the SAME read the CLI `aida queue
    /// list` uses: `DatabaseBackend::queue_list(user_id, false)` on the open
    /// cache-backed git backend (the CLI routes through `Storage::queue_list`,
    /// which delegates to this same git backend for a directory store — no
    /// reimplementation of the queue read).
    ///
    /// The queue user id mirrors the CLI's `current_user_id`: `AIDA_USER` →
    /// `USER` → `USERNAME` → `"default"` (the `--user` override has no TUI
    /// surface). Each entry is resolved to its requirement so the row carries
    /// the spec's id / type / status / title; `for_role` rides along as
    /// [`TargetItem::routed_role`] so the row renders a `->role` badge. Rows
    /// are ordered by queue position (lower = higher priority), matching the
    /// CLI list order. An unresolvable entry (a queued spec the cache can't
    /// read) is skipped rather than crashing the scope.
    // trace:TASK-948 | ai:claude
    pub fn queue_items(&self) -> Vec<TargetItem> {
        let user_id = queue_user_id();
        let mut entries = match self.backend.queue_list(&user_id, false) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        // Queue position is the CLI's display order (lower = higher priority).
        entries.sort_by_key(|e| e.position);
        let mut rows = Vec::with_capacity(entries.len());
        for entry in entries {
            let Ok(Some(req)) = self.backend.get_requirement(&entry.requirement_id) else {
                continue;
            };
            rows.push(TargetItem {
                id: req
                    .spec_id
                    .clone()
                    .or_else(|| req.agreed_id.clone())
                    .unwrap_or_else(|| entry.requirement_id.to_string()),
                title: req.title.clone(),
                req_type: format!("{:?}", req.req_type),
                status: format!("{:?}", req.status),
                priority: format!("{:?}", req.priority),
                body: String::new(),
                has_test_plan: false,
                // The stored `for_role` is already canonical (the CLI writes it
                // through `canonical_role_name`), so display it as-is; `None`
                // is an unrouted/general queue entry. trace:TASK-948
                routed_role: entry.for_role.clone(),
                // Tags drive the `drive` verb's keystone gate. trace:STORY-728
                tags: req.tags.iter().cloned().collect(),
            });
        }
        rows
    }

    /// The transitive descendant closure of `epic_id` (STORY-695): the epic
    /// itself plus every spec whose parent chain reaches it. Returned as the
    /// set of the descendants' display ids (spec_id, or agreed_id when no
    /// spec_id), so the focus lens can be applied to the [`TargetItem`] rows
    /// (which carry the same display id).
    ///
    /// Direction-robust (TASK-929): the closure is computed with the SAME union
    /// walk `aida graph --tree` and `aida queue list --epic` use —
    /// [`aida_core::graph_walk::walk_union`] over OUTGOING `Child` + `Parent`
    /// edges — so a one-directional epic→child edge (recorded as only `Child`
    /// OR only `Parent`, with no inverse) is still traversed. The previous
    /// implementation walked `Parent` edges ALONE, so it missed children whose
    /// edge was stored only as `Child` and showed an incomplete block-of-work
    /// view (4 of ~10 EPIC-54 children). Loads every requirement once,
    /// reconstitutes an in-memory store, and walks it. Returns an empty set
    /// when the epic can't be resolved or the store can't be read.
    /// trace:STORY-695 trace:TASK-929 | ai:claude
    pub fn descendants_of(&self, epic_id: &str) -> HashSet<String> {
        // Resolve the focus epic's UUID (it may be given as a spec_id or an
        // agreed id) so the closure walk keys off the canonical uuid edges.
        let root = match self.backend.get_requirement_by_spec_id(epic_id) {
            Ok(Some(req)) => req.id,
            _ => return HashSet::new(),
        };
        let all = match self.backend.list_requirements(true) {
            Ok(reqs) => reqs,
            Err(_) => return HashSet::new(),
        };
        let store = RequirementsStore {
            requirements: all,
            ..RequirementsStore::new()
        };
        descendant_closure(&store, root)
    }

    /// Load one spec's full record (structured fields + description body) for
    /// the show modal, in-process. `id` is a spec_id (e.g. `STORY-693`) or an
    /// agreed id. Returns `None` when the spec can't be found. The cache is
    /// stale-checked first so a just-edited spec reads fresh.
    /// trace:STORY-693 | ai:claude
    pub fn load_spec(&self, id: &str) -> Option<LoadedSpec> {
        let req = self.backend.get_requirement_by_spec_id(id).ok()??;
        let mut tags: Vec<String> = req.tags.iter().cloned().collect();
        tags.sort();
        // Resolve the spec's relationship targets (uuids on `req.relationships`)
        // to their display id + title + status, so the modal can render the
        // typed graph — AIDA's #1 differentiator — at the dig-in gesture. One
        // cache-backed `list_summaries` read builds a uuid→summary map for the
        // (typically handful of) targets; an unresolved target (deleted spec) is
        // dropped by `graph_from_relationships`. trace:STORY-739 | ai:claude
        let graph = self.spec_graph(&req);
        Some(LoadedSpec {
            id: req
                .spec_id
                .clone()
                .or_else(|| req.agreed_id.clone())
                .unwrap_or_else(|| id.to_string()),
            title: req.title.clone(),
            req_type: format!("{:?}", req.req_type),
            status: format!("{:?}", req.status),
            priority: format!("{:?}", req.priority),
            tags,
            description: req.description.clone(),
            comments: comments_from_requirement(&req),
            graph,
        })
    }

    /// Build the [`SpecGraph`] for `req` by resolving each outgoing-relationship
    /// target uuid to a display row via a single cache-backed `list_summaries`
    /// read (the same cheap read projection `aida list` uses; all three view
    /// tiers included so an archived/deferred neighbour still resolves). Returns
    /// an empty graph when the summary read fails (the modal then omits the
    /// graph section) rather than erroring the whole preview load.
    // trace:STORY-739 | ai:claude
    fn spec_graph(&self, req: &aida_core::Requirement) -> SpecGraph {
        if req.relationships.is_empty() {
            return SpecGraph::default();
        }
        let filter = ListFilter {
            archive: ArchiveFilter::Both,
            defer: DeferFilter::Both,
            ..Default::default()
        };
        let summaries = match self.backend.list_summaries(&filter) {
            Ok(s) => s,
            Err(_) => return SpecGraph::default(),
        };
        let by_uuid: HashMap<uuid::Uuid, RequirementSummary> =
            summaries.into_iter().map(|s| (s.id, s)).collect();
        graph_from_relationships(&req.relationships, |target| {
            by_uuid.get(&target).map(|s| LoadedRelation {
                id: s
                    .spec_id
                    .clone()
                    .or_else(|| s.agreed_id.clone())
                    .unwrap_or_default(),
                title: s.title.clone(),
                status: s.status.clone(),
            })
        })
    }

    /// The OPEN-epic list for the focus picker (STORY-697): every active
    /// (non-archived, non-deferred) spec whose type is Epic and whose status is
    /// non-terminal, projected to [`EpicRow`] (id + title + status). Read once
    /// in-process from the cache when the picker opens. Sorted by id for a
    /// stable list. trace:STORY-697 | ai:claude
    pub fn open_epics(&self) -> Vec<EpicRow> {
        let filter = ListFilter {
            archive: ArchiveFilter::NonArchivedOnly,
            defer: DeferFilter::NonDeferredOnly,
            ..Default::default()
        };
        let summaries = match self.backend.list_summaries(&filter) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut rows = epic_rows_from_summaries(summaries);
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    /// Map each spec display-id in `spec_ids` to its parent EPIC's display-id,
    /// for the branch-inference focus fallback (STORY-697 stretch). Loads every
    /// requirement once and resolves each input id to an Epic-typed hierarchy
    /// neighbor. Ids with no Epic neighbor are dropped. trace:STORY-697 | ai:claude
    ///
    /// Direction-robust (TASK-929): the same one-directional-edge fragility that
    /// broke [`Self::descendants_of`] applies here — a parent↔child link can be
    /// recorded on EITHER endpoint and as EITHER a `Parent` or a `Child` edge.
    /// So instead of indexing only `Parent` edges in one orientation, treat every
    /// `Parent`/`Child` relationship as an UNDIRECTED hierarchy link and a spec's
    /// parent epic is simply an Epic-typed hierarchy neighbor. This tolerates the
    /// inverse-missing edge the same way `walk_union` does for the closure.
    /// trace:TASK-929 | ai:claude
    pub fn parent_epics_of(&self, spec_ids: &[String]) -> Vec<String> {
        let all = match self.backend.list_requirements(true) {
            Ok(reqs) => reqs,
            Err(_) => return Vec::new(),
        };
        // display_id → uuid, and uuid → (display_id, is_epic) for resolution.
        let mut display_to_uuid: HashMap<String, uuid::Uuid> = HashMap::new();
        let mut uuid_display: HashMap<uuid::Uuid, String> = HashMap::new();
        let mut uuid_is_epic: HashMap<uuid::Uuid, bool> = HashMap::new();
        // Undirected hierarchy adjacency over every Parent/Child edge, recorded
        // on both endpoints so a one-directional edge is still seen from the
        // child side. trace:TASK-929 | ai:claude
        let mut adjacency: HashMap<uuid::Uuid, Vec<uuid::Uuid>> = HashMap::new();
        for req in &all {
            let display = req
                .spec_id
                .clone()
                .or_else(|| req.agreed_id.clone())
                .unwrap_or_default();
            if !display.is_empty() {
                display_to_uuid.insert(display.clone(), req.id);
                uuid_display.insert(req.id, display);
            }
            uuid_is_epic.insert(
                req.id,
                format!("{:?}", req.req_type).eq_ignore_ascii_case("epic"),
            );
            for rel in &req.relationships {
                if matches!(
                    rel.rel_type,
                    RelationshipType::Parent | RelationshipType::Child
                ) {
                    adjacency.entry(req.id).or_default().push(rel.target_id);
                    adjacency.entry(rel.target_id).or_default().push(req.id);
                }
            }
        }
        let mut out = Vec::new();
        for id in spec_ids {
            let Some(&uuid) = display_to_uuid.get(id) else {
                continue;
            };
            let Some(neighbors) = adjacency.get(&uuid) else {
                continue;
            };
            let parent = neighbors
                .iter()
                .copied()
                .find(|n| uuid_is_epic.get(n).copied().unwrap_or(false));
            if let Some(parent) = parent {
                if let Some(display) = uuid_display.get(&parent) {
                    out.push(display.clone());
                }
            }
        }
        out
    }
}

/// Filter + project cache summaries into the open-epic picker rows: type is
/// Epic (case-insensitive) and status is non-terminal. Pure over its input so
/// the open-epic source is unit-testable without a store. trace:STORY-697 | ai:claude
fn epic_rows_from_summaries(summaries: Vec<RequirementSummary>) -> Vec<EpicRow> {
    summaries
        .into_iter()
        .filter(|s| is_open_epic(&s.req_type, &s.status))
        .map(|s| EpicRow {
            id: s.spec_id.or(s.agreed_id).unwrap_or_default(),
            title: s.title,
            status: s.status,
        })
        .filter(|r| !r.id.is_empty())
        .collect()
}

/// Is a spec an OPEN epic for the focus picker: type is Epic (case-insensitive)
/// and status is non-terminal (not Completed / Rejected)? Pure predicate so the
/// open-epic filter is unit-testable. trace:STORY-697 | ai:claude
fn is_open_epic(req_type: &str, status: &str) -> bool {
    req_type.eq_ignore_ascii_case("epic") && !is_terminal_status(status)
}

/// Does `scope` include a spec whose cache status string is `status`?
/// Backlog = Approved + Planned; Open = any non-terminal (not Completed /
/// Rejected). Matched case-insensitively against the cache's stored status
/// strings. trace:STORY-693 | ai:claude
fn scope_includes(scope: Scope, status: &str) -> bool {
    match scope {
        Scope::Backlog => {
            status.eq_ignore_ascii_case("approved") || status.eq_ignore_ascii_case("planned")
        }
        Scope::Open => !is_terminal_status(status),
        // Test = the shipped work to verify: Done OR Completed. trace:STORY-699
        Scope::Test => is_testable_status(status),
        // Other scopes have no in-process target set yet.
        _ => false,
    }
}

/// Is a spec TESTABLE for the Test scope (STORY-699): has it shipped, i.e. is
/// it Done (finished on a branch) OR Completed (merged)? These are the recently-
/// shipped specs whose `## Test Plan` the operator walks. Matched
/// case-insensitively against the cache's stored status strings. A pure
/// predicate so the testable filter is unit-testable. trace:STORY-699 | ai:claude
fn is_testable_status(status: &str) -> bool {
    let t = status.trim();
    t.eq_ignore_ascii_case("done") || t.eq_ignore_ascii_case("completed")
}

/// A status string is terminal when the spec is Completed or Rejected. Mirrors
/// `aida-cli`'s `is_terminal_status_str` (STORY-86: "Done" is NOT terminal —
/// work finished on a branch, auto-bumps to Completed once merged).
/// trace:STORY-693 | ai:claude
fn is_terminal_status(status: &str) -> bool {
    let t = status.trim();
    t.eq_ignore_ascii_case("completed") || t.eq_ignore_ascii_case("rejected")
}

/// Project a cache summary into the bottom-panel target row. Priority now flows
/// through (the old `aida list --json` path dropped it; the cache carries it).
/// The body stays empty here — the show modal loads the full record on open.
/// trace:STORY-693 | ai:claude
fn summary_to_item(s: RequirementSummary) -> TargetItem {
    TargetItem {
        id: s.spec_id.or(s.agreed_id).unwrap_or_default(),
        title: s.title,
        req_type: s.req_type,
        status: s.status,
        priority: s.priority,
        body: String::new(),
        // Populated only for the Test scope (the description load happens there);
        // every other scope leaves it false. trace:STORY-699
        has_test_plan: false,
        // Populated only by the Queue read (queue_items); summary rows are
        // unrouted. trace:TASK-948
        routed_role: None,
        // Tags flow through from the cache summary; the `drive` verb's keystone
        // gate reads them. trace:STORY-728
        tags: s.tags,
    }
}

/// Extract the `## Test Plan` section from a spec description (STORY-699): the
/// `## Test Plan` heading through to the next `## ` (level-2) heading or the end
/// of the description, trimmed. Returns `None` when there is no such section so
/// the preview can fall back to the full description. The heading text is
/// matched case-insensitively; level-3+ headings inside the section do NOT end
/// it (only the next level-2 heading does). A PURE function so the extraction is
/// unit-testable with and without the section. trace:STORY-699 | ai:claude
pub fn extract_test_plan(description: &str) -> Option<String> {
    let lines: Vec<&str> = description.lines().collect();
    let start = lines.iter().position(|l| is_test_plan_heading(l))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| is_section_heading(l))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    let section = lines[start..end].join("\n");
    let trimmed = section.trim_end();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Is `line` the `## Test Plan` heading (a level-2 heading whose text is "Test
/// Plan", case-insensitive)? trace:STORY-699 | ai:claude
fn is_test_plan_heading(line: &str) -> bool {
    line.trim()
        .strip_prefix("## ")
        .map(|rest| rest.trim().eq_ignore_ascii_case("Test Plan"))
        .unwrap_or(false)
}

/// Is `line` a level-2 (`## `) section heading — the boundary that ends the Test
/// Plan section? Level-1 (`# `) and level-3+ (`### `) headings do not bound it.
/// trace:STORY-699 | ai:claude
fn is_section_heading(line: &str) -> bool {
    line.trim_start().starts_with("## ")
}

/// Compute the transitive descendant display-id closure of `root` over `store`,
/// using the SAME direction-robust union walk `aida graph --tree` (and
/// `aida queue list --epic`) use: [`aida_core::graph_walk::walk_union`] over
/// OUTGOING `Child` + `Parent` edges. Unioning both relationship types in the
/// outgoing direction traverses the hierarchy whichever side recorded the edge,
/// so a one-directional epic→child link (only `Child`, or only `Parent`, with no
/// inverse) is still followed — the fragility a `Parent`-only walk missed
/// (TASK-929 / SPIKE-71). The queried `root` is re-inserted because
/// `walk_union` excludes it, matching what `--tree` prints (the queried epic
/// leads its own subtree). Display ids prefer `spec_id` then `agreed_id` to line
/// up with [`summary_to_item`]'s [`TargetItem`] row ids; empty ids are dropped.
///
/// A pure function of `(store, root)` — no IO — so the closure logic (one-
/// directional Parent-only / Child-only edges found, unrelated excluded, root
/// included, cycle safety via `walk_union`'s visited set) is unit-testable.
/// The result equals the node set `aida graph <root> --tree` walks.
/// trace:STORY-695 trace:TASK-929 | ai:claude
pub fn descendant_closure(store: &RequirementsStore, root: uuid::Uuid) -> HashSet<String> {
    use aida_core::graph_walk::{walk_union, Direction};
    let specs = [(
        vec![RelationshipType::Child, RelationshipType::Parent],
        Direction::Outgoing,
    )];
    let result = walk_union(store, root, &specs, None);
    let mut out: HashSet<String> = HashSet::new();
    for id in std::iter::once(&root).chain(result.nodes.iter()) {
        if let Some(display) = store
            .get_requirement_by_id(id)
            .and_then(|r| r.spec_id.clone().or_else(|| r.agreed_id.clone()))
            .filter(|d| !d.is_empty())
        {
            out.insert(display);
        }
    }
    out
}

/// Does the EPIC focus lens `set` include this target row? The lens matches on
/// the row's display id (the same id [`descendant_closure`] returns). Pure
/// so the filter narrowing is unit-testable. trace:STORY-695 | ai:claude
fn focus_includes(set: &HashSet<String>, item: &TargetItem) -> bool {
    set.contains(&item.id)
}

/// A coarse lifecycle-bucket tally of a focus set's specs, for the status-line
/// progress summary (e.g. "6 done · 1 queued · 2 approved · 3 draft"). Buckets:
///
/// * `done`     — Done or Completed (finished work).
/// * `in_progress` — InProgress or NeedsAttention (active / parked work).
/// * `approved` — Approved or Planned (groomed, ready to pick up).
/// * `draft`    — Draft (ungroomed).
///
/// Other statuses (e.g. Rejected) fall in none. Pure over the (status-string)
/// inputs so it is unit-testable. trace:STORY-695 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FocusProgress {
    pub done: usize,
    pub in_progress: usize,
    pub approved: usize,
    pub draft: usize,
}

impl FocusProgress {
    /// Tally a focus set's specs into the lifecycle buckets, given an iterator
    /// of their status strings (cache Debug form — "Draft", "InProgress", …).
    /// Matched case-insensitively. trace:STORY-695 | ai:claude
    pub fn tally<'a, I: IntoIterator<Item = &'a str>>(statuses: I) -> Self {
        let mut p = FocusProgress::default();
        for status in statuses {
            let s = status.trim();
            if s.eq_ignore_ascii_case("done") || s.eq_ignore_ascii_case("completed") {
                p.done += 1;
            } else if s.eq_ignore_ascii_case("inprogress")
                || s.eq_ignore_ascii_case("in-progress")
                || s.eq_ignore_ascii_case("needsattention")
                || s.eq_ignore_ascii_case("needs-attention")
            {
                p.in_progress += 1;
            } else if s.eq_ignore_ascii_case("approved") || s.eq_ignore_ascii_case("planned") {
                p.approved += 1;
            } else if s.eq_ignore_ascii_case("draft") {
                p.draft += 1;
            }
        }
        p
    }

    /// A concise one-line summary of the non-zero buckets, e.g.
    /// "6 done · 1 in-progress · 2 approved · 3 draft". Empty buckets are
    /// dropped; an all-empty tally renders "no specs". trace:STORY-695 | ai:claude
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.done > 0 {
            parts.push(format!("{} done", self.done));
        }
        if self.in_progress > 0 {
            parts.push(format!("{} in-progress", self.in_progress));
        }
        if self.approved > 0 {
            parts.push(format!("{} approved", self.approved));
        }
        if self.draft > 0 {
            parts.push(format!("{} draft", self.draft));
        }
        if parts.is_empty() {
            "no specs".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

/// The lifecycle-bucket progress summary for an EPIC focus set, read in-process
/// from the cache. Loads the active (non-archived, non-deferred) spec summaries
/// once, keeps those in `focus`, and tallies them via [`FocusProgress::tally`].
/// Returns the `(FocusProgress, total)` so the caller can render
/// "<EPIC>: <summary>". trace:STORY-695 | ai:claude
impl SpecStore {
    pub fn focus_progress(&self, focus: &HashSet<String>) -> (FocusProgress, usize) {
        let filter = ListFilter {
            archive: ArchiveFilter::NonArchivedOnly,
            defer: DeferFilter::NonDeferredOnly,
            ..Default::default()
        };
        let summaries = match self.backend.list_summaries(&filter) {
            Ok(s) => s,
            Err(_) => return (FocusProgress::default(), 0),
        };
        let in_focus: Vec<String> = summaries
            .into_iter()
            .filter(|s| {
                let id = s
                    .spec_id
                    .clone()
                    .or_else(|| s.agreed_id.clone())
                    .unwrap_or_default();
                focus.contains(&id)
            })
            .map(|s| s.status)
            .collect();
        let progress = FocusProgress::tally(in_focus.iter().map(|s| s.as_str()));
        (progress, in_focus.len())
    }
}

/// The queue identity for the Queue scope read, mirroring aida-cli's
/// `current_user_id` (BUG-89): `AIDA_USER` → `USER` → `USERNAME` → `"default"`.
/// The `--user` override has no TUI surface, so the env chain is the whole
/// resolution. Keep aligned with `current_user_id` so the TUI reads the SAME
/// queue the CLI writes.
// trace:TASK-948 | ai:claude
fn queue_user_id() -> String {
    std::env::var("AIDA_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string())
}

/// Resolve the orphan-branch store worktree for the project rooted at
/// `project_root`. Reads `store_path` from `.aida/config.toml`, tries the
/// local path, then falls back to the main worktree (BUG-331: a sibling
/// `git worktree` has the tracked config but the gitignored `.aida-store/`
/// only lives in the main worktree). Mirrors aida-cli's
/// `detect_distributed_store_from` + `main_worktree_store` for the read path.
/// trace:STORY-693 | ai:claude
fn resolve_store_path(project_root: &Path) -> Option<PathBuf> {
    let mut current = Some(project_root);
    while let Some(dir) = current {
        let config_path = dir.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Some(rel) = store_path_value(&content) {
                let local = dir.join(&rel);
                if local.exists() && local.is_dir() {
                    return Some(local);
                }
                if let Some(main_store) = main_worktree_store(dir, &rel) {
                    return Some(main_store);
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Extract `store_path = "<value>"` from a `config.toml` body (a focused
/// line-scan rather than a full TOML parse, matching aida-cli's reader).
/// trace:STORY-693 | ai:claude
fn store_path_value(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("store_path") {
            if let Some(val) = rest.split('=').nth(1) {
                let val = val.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// BUG-331: resolve `<main-worktree>/<rel_store>` from inside a git worktree
/// via `git rev-parse --git-common-dir` (the shared `.git`; its parent is the
/// main worktree, where the gitignored `.aida-store/` lives). This is the one
/// remaining `git` shell-out, fired ONCE at backend-open (not per read) and
/// only on the sibling-worktree fallback path. trace:STORY-693 | ai:claude
fn main_worktree_store(current: &Path, rel_store: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(current)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let common_dir = Path::new(raw);
    let common_dir = if common_dir.is_absolute() {
        common_dir.to_path_buf()
    } else {
        current.join(common_dir)
    };
    let common_dir = common_dir.canonicalize().ok()?;
    let main_worktree = common_dir.parent()?;
    let store = main_worktree.join(rel_store);
    if store.exists() && store.is_dir() {
        Some(store)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Per-worktree focus persistence (STORY-697)
// ---------------------------------------------------------------------------

/// The per-worktree focus marker path: `<project_root>/.aida/tui-focus`. A pure
/// path-builder (no IO) so the precedence + path logic is unit-testable. The
/// file is auto-gitignored by the `.aida/*` deny-by-default convention, so it
/// is never tracked. trace:STORY-697 | ai:claude
pub fn focus_marker_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("tui-focus")
}

/// Resolve the launch focus epic from the (already-read) `AIDA_TUI_EPIC` env
/// value and the marker-file contents, with precedence **env > marker > none**.
/// A blank / whitespace-only value at either tier is ignored (falls through).
/// PURE — both inputs are passed in, so the precedence logic is unit-testable
/// without touching the environment or the filesystem. trace:STORY-697 | ai:claude
pub fn resolve_focus_epic(env: Option<&str>, marker: Option<&str>) -> Option<String> {
    if let Some(e) = env {
        let t = e.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(m) = marker {
        let t = m.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    None
}

/// Read the per-worktree focus marker (the first non-empty line of
/// `.aida/tui-focus`), or `None` when it is absent / empty. A thin FS wrapper
/// over [`focus_marker_path`]; the precedence logic it feeds lives in the pure
/// [`resolve_focus_epic`]. trace:STORY-697 | ai:claude
pub fn read_focus_marker(project_root: &Path) -> Option<String> {
    let content = std::fs::read_to_string(focus_marker_path(project_root)).ok()?;
    let line = content.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.to_string())
}

/// Write the per-worktree focus marker (one line = `epic`), creating `.aida/`
/// if needed. Best-effort: a write failure is swallowed (persistence is a
/// convenience, not load-bearing). trace:STORY-697 | ai:claude
pub fn write_focus_marker(project_root: &Path, epic: &str) {
    let path = focus_marker_path(project_root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("{}\n", epic.trim()));
}

/// Clear the per-worktree focus marker (remove the file). Best-effort; a
/// missing file is not an error. trace:STORY-697 | ai:claude
pub fn clear_focus_marker(project_root: &Path) {
    let _ = std::fs::remove_file(focus_marker_path(project_root));
}

/// Parse the `(SPEC-ID)` trailers out of a block of git-log subject lines (one
/// subject per line). The convention is the LAST `(SPEC-ID)`-shaped group on a
/// line (an UPPERCASE token + `-` + digits), so each line contributes at most
/// one id; duplicates are kept so the caller can take the mode. PURE over its
/// input. trace:STORY-697 | ai:claude
pub fn parse_spec_trailers(log: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in log.lines() {
        let mut best: Option<String> = None;
        let mut i = 0;
        while let Some(open) = line[i..].find('(') {
            let start = i + open + 1;
            let Some(close_rel) = line[start..].find(')') else {
                break;
            };
            let inner = &line[start..start + close_rel];
            if is_spec_id(inner) {
                best = Some(inner.to_string());
            }
            i = start + close_rel + 1;
        }
        if let Some(id) = best {
            out.push(id);
        }
    }
    out
}

/// Does `s` have the `UPPERCASE-DIGITS` shape of a spec id (e.g. `STORY-697`)?
/// trace:STORY-697 | ai:claude
fn is_spec_id(s: &str) -> bool {
    match s.split_once('-') {
        Some((prefix, num)) => {
            !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_uppercase())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// The most-common element of `ids` (the mode), or `None` when empty. Ties
/// break on the lexically-smallest id for determinism. PURE. trace:STORY-697 | ai:claude
pub fn most_common(ids: &[String]) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for id in ids {
        *counts.entry(id.as_str()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(id, _)| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_includes_backlog_is_approved_and_planned() {
        assert!(scope_includes(Scope::Backlog, "Approved"));
        assert!(scope_includes(Scope::Backlog, "approved"));
        assert!(scope_includes(Scope::Backlog, "Planned"));
        assert!(!scope_includes(Scope::Backlog, "Draft"));
        assert!(!scope_includes(Scope::Backlog, "InProgress"));
        assert!(!scope_includes(Scope::Backlog, "Completed"));
    }

    #[test]
    fn scope_includes_open_is_every_non_terminal() {
        for s in [
            "Draft",
            "Approved",
            "Planned",
            "InProgress",
            "Done",
            "NeedsAttention",
        ] {
            assert!(scope_includes(Scope::Open, s), "{s} should be open");
        }
        assert!(!scope_includes(Scope::Open, "Completed"));
        assert!(!scope_includes(Scope::Open, "Rejected"));
        // STORY-86: Done is NOT terminal — it stays in the Open view.
        assert!(scope_includes(Scope::Open, "Done"));
    }

    #[test]
    fn non_functional_scopes_have_no_in_process_set() {
        assert!(!scope_includes(Scope::Queue, "Approved"));
        assert!(!scope_includes(Scope::Prs, "Open"));
    }

    // --- Test scope: testable filter + ## Test Plan extraction (STORY-699) --

    #[test]
    fn scope_includes_test_is_done_and_completed_only() {
        // The shipped work to verify: Done OR Completed.
        assert!(scope_includes(Scope::Test, "Done"));
        assert!(scope_includes(Scope::Test, "done"));
        assert!(scope_includes(Scope::Test, "Completed"));
        assert!(scope_includes(Scope::Test, "completed"));
        // Everything still in flight is excluded.
        assert!(!scope_includes(Scope::Test, "Draft"));
        assert!(!scope_includes(Scope::Test, "Approved"));
        assert!(!scope_includes(Scope::Test, "Planned"));
        assert!(!scope_includes(Scope::Test, "InProgress"));
        assert!(!scope_includes(Scope::Test, "Rejected"));
    }

    #[test]
    fn testable_filter_intersects_with_focus() {
        // The two halves of the Test scope's target set: Done/Completed AND in
        // the active focus epic. STORY-695/TASK-913 are in focus; STORY-999 is
        // not; TASK-1 is in focus but still Approved (not shipped).
        let mut set = HashSet::new();
        set.insert("STORY-695".to_string());
        set.insert("TASK-913".to_string());
        set.insert("TASK-1".to_string());
        let rows = [
            ("STORY-695", "Done"),      // testable ∧ focus → kept
            ("TASK-913", "Completed"),  // testable ∧ focus → kept
            ("TASK-1", "Approved"),     // focus but not testable → dropped
            ("STORY-999", "Completed"), // testable but not focus → dropped
        ];
        let kept: Vec<String> = rows
            .iter()
            .filter(|(_, status)| scope_includes(Scope::Test, status))
            .map(|(id, status)| target(id, status))
            .filter(|it| focus_includes(&set, it))
            .map(|it| it.id)
            .collect();
        // Both shipped+focused rows survive; the other two are filtered out.
        assert_eq!(kept, vec!["STORY-695".to_string(), "TASK-913".to_string()]);
    }

    #[test]
    fn extract_test_plan_returns_the_section_when_present() {
        let desc = "Intro paragraph.\n\n\
                    ## Acceptance\n- a\n- b\n\n\
                    ## Test Plan\n\n\
                    1. do X → expect Y\n2. do Z → expect W";
        let plan = extract_test_plan(desc).expect("section present");
        assert!(plan.starts_with("## Test Plan"), "leads with the heading");
        assert!(plan.contains("1. do X → expect Y"));
        assert!(plan.contains("2. do Z → expect W"));
        // It does NOT pull in the earlier Acceptance section.
        assert!(!plan.contains("## Acceptance"));
    }

    #[test]
    fn extract_test_plan_stops_at_the_next_level2_heading() {
        let desc = "## Test Plan\n1. step one\n2. step two\n\n\
                    ## Notes\nsome trailing notes that are not the plan";
        let plan = extract_test_plan(desc).expect("section present");
        assert!(plan.contains("1. step one"));
        assert!(plan.contains("2. step two"));
        // The following section is not part of the plan.
        assert!(!plan.contains("## Notes"));
        assert!(!plan.contains("trailing notes"));
    }

    #[test]
    fn extract_test_plan_keeps_nested_subheadings() {
        // A level-3 heading inside the section does not terminate it.
        let desc = "## Test Plan\n### Setup\n1. step\n### Teardown\n2. step";
        let plan = extract_test_plan(desc).expect("section present");
        assert!(plan.contains("### Setup"));
        assert!(plan.contains("### Teardown"));
        assert!(plan.contains("2. step"));
    }

    #[test]
    fn extract_test_plan_is_none_when_absent() {
        assert_eq!(extract_test_plan("Just a description, no plan here."), None);
        assert_eq!(extract_test_plan(""), None);
        // A heading whose text isn't exactly "Test Plan" doesn't match.
        assert_eq!(extract_test_plan("## Testing Plans\n- nope"), None);
    }

    #[test]
    fn extract_test_plan_matches_heading_case_insensitively() {
        let desc = "## test plan\n1. lower-case heading still matches";
        assert!(extract_test_plan(desc).is_some());
    }

    // --- EPIC focus lens (STORY-695 / TASK-929) --------------------------

    /// A requirement with a fixed uuid + spec_id (so edges can target it
    /// deterministically) and no relationships. trace:TASK-929 | ai:claude
    fn rreq(id: u128, spec: &str) -> aida_core::Requirement {
        let mut r = aida_core::Requirement::new(spec.to_string(), String::new());
        r.id = uuid::Uuid::from_u128(id);
        r.spec_id = Some(spec.to_string());
        r
    }

    /// An outgoing relationship of `rt` from the holder to the uuid `target`.
    /// trace:TASK-929 | ai:claude
    fn edge(rt: RelationshipType, target: u128) -> aida_core::Relationship {
        aida_core::Relationship {
            rel_type: rt,
            target_id: uuid::Uuid::from_u128(target),
            created_at: None,
            created_by: None,
        }
    }

    fn store_of(reqs: Vec<aida_core::Requirement>) -> RequirementsStore {
        RequirementsStore {
            requirements: reqs,
            ..RequirementsStore::new()
        }
    }

    fn target(id: &str, status: &str) -> TargetItem {
        TargetItem {
            id: id.to_string(),
            title: String::new(),
            req_type: "Task".into(),
            status: status.to_string(),
            priority: String::new(),
            body: String::new(),
            has_test_plan: false,
            routed_role: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn closure_includes_epic_direct_and_grandchild_excludes_unrelated() {
        // EPIC(1) --Child--> STORY(2) --Child--> TASK(3); unrelated STORY(9).
        let mut epic = rreq(1, "EPIC-54");
        epic.relationships.push(edge(RelationshipType::Child, 2));
        let mut story = rreq(2, "STORY-695");
        story.relationships.push(edge(RelationshipType::Child, 3));
        let task = rreq(3, "TASK-913");
        let unrelated = rreq(9, "STORY-999");
        let store = store_of(vec![epic, story, task, unrelated]);

        let set = descendant_closure(&store, uuid::Uuid::from_u128(1));
        assert!(
            set.contains("EPIC-54"),
            "epic itself (root) is in the closure"
        );
        assert!(set.contains("STORY-695"), "direct child is in");
        assert!(set.contains("TASK-913"), "transitive grandchild is in");
        assert!(!set.contains("STORY-999"), "unrelated spec is out");
        assert_eq!(set.len(), 3);
    }

    /// THE REGRESSION TASK-929 TARGETS: the epic→child edge is recorded only as
    /// a `Parent` edge with NO `Child` inverse. The old `Parent`-only-walk found
    /// this form, but a direction-robust walk must too. trace:TASK-929
    #[test]
    fn closure_finds_child_via_parent_only_edge() {
        // EPIC(1) --Parent--> STORY(2)  (no Child inverse anywhere).
        let mut epic = rreq(1, "EPIC-54");
        epic.relationships.push(edge(RelationshipType::Parent, 2));
        let story = rreq(2, "STORY-695");
        let store = store_of(vec![epic, story]);

        let set = descendant_closure(&store, uuid::Uuid::from_u128(1));
        assert!(set.contains("EPIC-54"), "root included");
        assert!(
            set.contains("STORY-695"),
            "child reached via a Parent-only edge (no Child inverse)"
        );
        assert_eq!(set.len(), 2);
    }

    /// THE OTHER HALF OF THE REGRESSION: the edge is recorded only as a `Child`
    /// edge with NO `Parent` inverse — exactly what the OLD Parent-only walk
    /// MISSED (it showed 4 of ~10 EPIC-54 children). The union walk now finds it.
    /// trace:TASK-929
    #[test]
    fn closure_finds_child_via_child_only_edge() {
        // EPIC(1) --Child--> STORY(2)  (no Parent inverse anywhere).
        let mut epic = rreq(1, "EPIC-54");
        epic.relationships.push(edge(RelationshipType::Child, 2));
        let story = rreq(2, "STORY-695");
        let store = store_of(vec![epic, story]);

        let set = descendant_closure(&store, uuid::Uuid::from_u128(1));
        assert!(set.contains("EPIC-54"), "root included");
        assert!(
            set.contains("STORY-695"),
            "child reached via a Child-only edge (no Parent inverse) — the missed case"
        );
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn closure_is_cycle_safe() {
        // A pathological hierarchy cycle 1<->2 must terminate.
        let mut a = rreq(1, "EPIC-1");
        a.relationships.push(edge(RelationshipType::Child, 2));
        let mut b = rreq(2, "STORY-2");
        b.relationships.push(edge(RelationshipType::Child, 1));
        let store = store_of(vec![a, b]);

        let set = descendant_closure(&store, uuid::Uuid::from_u128(1));
        assert!(set.contains("EPIC-1"));
        assert!(set.contains("STORY-2"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn closure_of_missing_root_is_empty() {
        let store = store_of(vec![rreq(1, "EPIC-1")]);
        let set = descendant_closure(&store, uuid::Uuid::from_u128(42));
        assert!(set.is_empty());
    }

    // --- preview-modal relationship graph (STORY-739) --------------------

    /// A fixture row for the resolver table, mirroring what `load_spec` builds
    /// from `list_summaries`.
    // trace:STORY-739
    fn rel_row(id: &str, title: &str, status: &str) -> LoadedRelation {
        LoadedRelation {
            id: id.to_string(),
            title: title.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn graph_groups_each_relation_type_by_the_stored_convention() {
        // The spec under preview has one of every surfaced edge. Per TASK-679 a
        // `Child` edge points at the PARENT and a `Parent` edge at a CHILD.
        let rels = vec![
            edge(RelationshipType::Child, 10),     // → parent epic
            edge(RelationshipType::Parent, 20),    // → child
            edge(RelationshipType::BlockedBy, 30), // → blocker
            edge(RelationshipType::Blocks, 40),    // → blocked
            edge(RelationshipType::References, 50),
            // Edge types the modal does not surface today:
            edge(RelationshipType::Duplicate, 60),
            edge(RelationshipType::Verifies, 70),
        ];
        let table: HashMap<u128, LoadedRelation> = [
            (10, rel_row("EPIC-7", "the epic", "InProgress")),
            (20, rel_row("TASK-2", "a child task", "Approved")),
            (30, rel_row("BUG-9", "a blocker", "Done")),
            (40, rel_row("STORY-3", "downstream", "Draft")),
            (50, rel_row("DOC-1", "see also", "Completed")),
            (60, rel_row("STORY-99", "dup", "Rejected")),
            (70, rel_row("TASK-88", "verifier", "Draft")),
        ]
        .into_iter()
        .collect();

        let graph = graph_from_relationships(&rels, |t| table.get(&t.as_u128()).cloned());

        assert_eq!(
            graph.parents,
            vec![rel_row("EPIC-7", "the epic", "InProgress")]
        );
        assert_eq!(
            graph.children,
            vec![rel_row("TASK-2", "a child task", "Approved")]
        );
        assert_eq!(
            graph.blocked_by,
            vec![rel_row("BUG-9", "a blocker", "Done")]
        );
        assert_eq!(
            graph.blocks,
            vec![rel_row("STORY-3", "downstream", "Draft")]
        );
        assert_eq!(
            graph.references,
            vec![rel_row("DOC-1", "see also", "Completed")]
        );
        assert!(!graph.is_empty());
    }

    #[test]
    fn graph_omits_empty_groups_and_drops_dangling_targets() {
        // Only a parent edge plus an edge whose target the resolver can't find
        // (a deleted spec). The dangling edge is dropped; every other group is
        // empty.
        let rels = vec![
            edge(RelationshipType::Child, 10),
            edge(RelationshipType::Parent, 999), // unresolved → dropped
        ];
        let table: HashMap<u128, LoadedRelation> =
            [(10u128, rel_row("EPIC-7", "the epic", "InProgress"))]
                .into_iter()
                .collect();

        let graph = graph_from_relationships(&rels, |t| table.get(&t.as_u128()).cloned());

        assert_eq!(graph.parents.len(), 1, "resolved parent kept");
        assert!(graph.children.is_empty(), "dangling child target dropped");
        assert!(graph.blocked_by.is_empty());
        assert!(graph.blocks.is_empty());
        assert!(graph.references.is_empty());
        assert!(
            !graph.is_empty(),
            "the one parent makes the graph non-empty"
        );
    }

    #[test]
    fn graph_of_no_relationships_is_empty() {
        let graph = graph_from_relationships(&[], |_| None);
        assert!(graph.is_empty());
    }

    #[test]
    fn focus_filter_narrows_a_scope_list() {
        let mut set = HashSet::new();
        set.insert("STORY-695".to_string());
        set.insert("TASK-913".to_string());
        let items = [
            target("STORY-695", "Approved"),
            target("TASK-913", "Draft"),
            target("STORY-999", "Approved"), // not in focus
        ];
        let kept: Vec<&str> = items
            .iter()
            .filter(|it| focus_includes(&set, it))
            .map(|it| it.id.as_str())
            .collect();
        assert_eq!(kept, vec!["STORY-695", "TASK-913"]);
    }

    #[test]
    fn progress_summary_counts_buckets() {
        let statuses = [
            "Done",
            "Completed",
            "InProgress",
            "Approved",
            "Planned",
            "Draft",
            "Draft",
            "Rejected", // counted in no bucket
        ];
        let p = FocusProgress::tally(statuses.iter().copied());
        assert_eq!(p.done, 2, "Done + Completed");
        assert_eq!(p.in_progress, 1);
        assert_eq!(p.approved, 2, "Approved + Planned");
        assert_eq!(p.draft, 2);
        let s = p.summary();
        assert!(s.contains("2 done"));
        assert!(s.contains("1 in-progress"));
        assert!(s.contains("2 approved"));
        assert!(s.contains("2 draft"));
    }

    #[test]
    fn progress_summary_drops_empty_buckets_and_handles_none() {
        let p = FocusProgress::tally(["Draft", "Draft"].iter().copied());
        assert_eq!(p.summary(), "2 draft");
        let empty = FocusProgress::tally(std::iter::empty::<&str>());
        assert_eq!(empty.summary(), "no specs");
    }

    #[test]
    fn store_path_value_parses_the_config_line() {
        let cfg =
            "store_type = \"worktree\"\nstore_path = \".aida-store\"\nbranch = \"aida-store\"\n";
        assert_eq!(store_path_value(cfg), Some(".aida-store".to_string()));
        assert_eq!(store_path_value("# nothing here\n"), None);
    }

    // --- EPIC focus picker + persistence (STORY-697) ---------------------

    #[test]
    fn open_epic_predicate_is_epic_and_non_terminal() {
        assert!(is_open_epic("Epic", "Approved"));
        assert!(is_open_epic("epic", "InProgress"));
        assert!(is_open_epic("EPIC", "Draft"));
        // STORY-86: Done is non-terminal → still an open epic.
        assert!(is_open_epic("Epic", "Done"));
        // Terminal statuses drop out.
        assert!(!is_open_epic("Epic", "Completed"));
        assert!(!is_open_epic("Epic", "Rejected"));
        // Non-epic types never qualify.
        assert!(!is_open_epic("Story", "Approved"));
        assert!(!is_open_epic("Task", "Draft"));
    }

    #[test]
    fn resolve_focus_epic_precedence_env_over_marker_over_none() {
        // env wins outright.
        assert_eq!(
            resolve_focus_epic(Some("EPIC-1"), Some("EPIC-2")),
            Some("EPIC-1".to_string())
        );
        // blank env falls through to the marker.
        assert_eq!(
            resolve_focus_epic(Some("   "), Some("EPIC-2")),
            Some("EPIC-2".to_string())
        );
        // env absent → marker.
        assert_eq!(
            resolve_focus_epic(None, Some(" EPIC-3 ")),
            Some("EPIC-3".to_string())
        );
        // both blank / absent → none.
        assert_eq!(resolve_focus_epic(None, None), None);
        assert_eq!(resolve_focus_epic(Some(""), Some("  ")), None);
    }

    #[test]
    fn focus_marker_path_is_under_dot_aida() {
        let p = focus_marker_path(Path::new("/tmp/proj"));
        assert!(p.ends_with(".aida/tui-focus"), "{p:?}");
    }

    #[test]
    fn focus_marker_round_trips_write_read_clear() {
        let dir = std::env::temp_dir().join(format!("aida-tui-focus-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // No marker yet.
        assert_eq!(read_focus_marker(&dir), None);
        write_focus_marker(&dir, "EPIC-54");
        assert_eq!(read_focus_marker(&dir), Some("EPIC-54".to_string()));
        // Overwrite, and ignore surrounding whitespace on read.
        write_focus_marker(&dir, "  EPIC-26  ");
        assert_eq!(read_focus_marker(&dir), Some("EPIC-26".to_string()));
        clear_focus_marker(&dir);
        assert_eq!(read_focus_marker(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_spec_trailers_takes_last_group_per_line() {
        let log = "\
[AI:claude] feat(tui): epic picker (STORY-697)
fix(api): handle null (FR-1) and route (BUG-23)
chore: no trailer here
docs: mentions (lowercase-7) but not an id";
        let trailers = parse_spec_trailers(log);
        // Line 1 → STORY-697; line 2 → the LAST group BUG-23; lines 3/4 → none.
        assert_eq!(trailers, vec!["STORY-697", "BUG-23"]);
    }

    #[test]
    fn is_spec_id_shape() {
        assert!(is_spec_id("STORY-697"));
        assert!(is_spec_id("FR-1"));
        assert!(!is_spec_id("story-1")); // lowercase prefix
        assert!(!is_spec_id("STORY")); // no number
        assert!(!is_spec_id("STORY-")); // empty number
        assert!(!is_spec_id("-7")); // empty prefix
        assert!(!is_spec_id("lowercase-7"));
    }

    // --- Comment projection (TASK-932) -----------------------------------

    #[test]
    fn comments_from_requirement_maps_each_comment() {
        let mut req = aida_core::Requirement::new("TASK-932".to_string(), String::new());
        req.comments.push(aida_core::Comment::new(
            "advisor".to_string(),
            "approved because the slice is well-bounded".to_string(),
        ));
        req.comments.push(aida_core::Comment::new(
            "claude".to_string(),
            "implemented in-process".to_string(),
        ));
        let rows = comments_from_requirement(&req);
        assert_eq!(rows.len(), 2, "N comments → N rows, in order");
        assert_eq!(rows[0].author, "advisor");
        assert_eq!(
            rows[0].content,
            "approved because the slice is well-bounded"
        );
        assert_eq!(rows[1].author, "claude");
        assert_eq!(rows[1].content, "implemented in-process");
        // Each row carries a non-empty short timestamp.
        assert!(!rows[0].when.is_empty());
    }

    #[test]
    fn comments_from_requirement_is_empty_when_none() {
        let req = aida_core::Requirement::new("TASK-1".to_string(), String::new());
        assert!(comments_from_requirement(&req).is_empty());
    }

    #[test]
    fn most_common_returns_mode_with_deterministic_tiebreak() {
        let ids = vec![
            "EPIC-1".to_string(),
            "EPIC-2".to_string(),
            "EPIC-1".to_string(),
        ];
        assert_eq!(most_common(&ids), Some("EPIC-1".to_string()));
        // Tie → lexically-smallest id wins (deterministic).
        let tie = vec!["EPIC-9".to_string(), "EPIC-3".to_string()];
        assert_eq!(most_common(&tie), Some("EPIC-3".to_string()));
        assert_eq!(most_common(&[]), None);
    }
}
