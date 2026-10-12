//! queue helpers: batch tags, role and user filters, mail identity, effort load, stack/headless/worker commands (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// STORY-106: best-effort emit the "queue empty for this role" workflow
/// hint after a successful `queue done` (or `edit --status completed` on
/// a queue-tracked spec). Detects role+scope from the session env / lease,
/// counts remaining queue entries, and fires the hint only when the
/// active-role queue is now empty. Silent on any detection failure —
/// hints are nice-to-have, not load-bearing.
/// trace:STORY-106 | ai:claude
pub(crate) fn maybe_hint_after_queue_drain(storage: &Storage, user_id: &str) {
    let project_root = match storage.path().parent() {
        Some(p) => p.to_path_buf(),
        None => return,
    };
    let role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty());

    // Match the filter `aida queue list` uses for the active session:
    //   1. role: matches AIDA_SESSION_ROLE
    //   2. scope: matches the active session lease's scope (unscoped
    //      entries pass through, same as queue list inside a session)
    //   3. the entry's requirement still exists in the store — dangling
    //      entries pointing to deleted/renamed UUIDs are noise and `aida
    //      queue list` hides them. Without #3 the hint sees stale queue
    //      cruft and never fires.
    // trace:STORY-106 | ai:claude
    let scope = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(&project_root, &cwd))
        .map(|l| l.scope);

    let remaining = storage.queue_list(user_id, false).unwrap_or_default();
    let store_snapshot = storage.load().ok();
    // Entry counts as "open work for this user" only when its
    // requirement (a) still exists in the store and (b) hasn't already
    // reached terminal status. The second check mirrors `aida queue
    // list`'s terminal-hide behavior (TASK-46) — without it, queue
    // entries left behind after `aida edit --status completed` (which
    // doesn't auto-dequeue) read as "still in flight" and suppress the
    // hint forever.
    let entry_open = |e: &aida_core::QueueEntry| -> bool {
        let Some(snap) = &store_snapshot else {
            return true;
        };
        let Some(req) = snap.requirements.iter().find(|r| r.id == e.requirement_id) else {
            return false;
        };
        !is_terminal_status(&req.status)
    };
    let filtered: Vec<&aida_core::QueueEntry> = remaining
        .iter()
        .filter(|e| entry_open(e))
        .filter(|e| match role.as_deref() {
            Some(r) => e.for_role.as_deref() == Some(r),
            None => true,
        })
        .filter(|e| match scope.as_deref() {
            Some(s) => match e.for_scope.as_deref() {
                Some(es) => es == s,
                None => true,
            },
            None => true,
        })
        .collect();
    if !filtered.is_empty() {
        return;
    }

    let branch = current_branch_at(&project_root);
    let commits_ahead = branch
        .as_deref()
        .and_then(|b| branch_commits_ahead_main(&project_root, b));

    // BUG-232: when the branch carries committed-but-unshipped work, probe
    // whether a PR is already open. This lets the hint sharpen "open a PR"
    // into a pointed "no PR open — run `/aida-pr`" warning — the failure
    // mode where a `--zen` session ends with the spec Done but unmergeable.
    // Probe only when there's something to ship; a `gh` miss / failure
    // stays Unknown so the hint never *asserts* there's no PR on guesswork.
    // trace:BUG-232 | ai:claude
    let pr_state = match (branch.as_deref(), commits_ahead) {
        // STORY-516: forge-routed. trace:STORY-516 | ai:claude
        (Some(b), Some(n)) if n > 0 => match change_lookup_for_branch(&project_root, b) {
            crate::forge::ChangeLookup::Found(c) => workflow_hints::PrState::Open(c.id),
            crate::forge::ChangeLookup::NoChange => workflow_hints::PrState::Absent,
            crate::forge::ChangeLookup::CliMissing
            | crate::forge::ChangeLookup::CliFailed(_)
            | crate::forge::ChangeLookup::Unreachable(_) => workflow_hints::PrState::Unknown,
        },
        _ => workflow_hints::PrState::Unknown,
    };

    workflow_hints::after_queue_drained(
        Some(&project_root),
        role.as_deref(),
        scope.as_deref(),
        commits_ahead,
        branch.as_deref(),
        pr_state,
    );
}

/// TASK-241: extract the PR number from a GitHub squash-merge commit
/// subject, which ends with `(#NN)`. Returns None when the subject has
/// no such suffix. trace:TASK-241 | ai:claude
pub(crate) fn parse_squash_pr_number(subject: &str) -> Option<u64> {
    subject
        .trim_end()
        .rsplit_once("(#")
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .and_then(|n| n.parse::<u64>().ok())
}

/// TASK-238/STORY-442: the inline tag-chip body for an `aida queue list`
/// row. `batch:*` and `lifecycle:*` tags come first and are always shown
/// (they alter pickup/routing behavior); plain tags are sorted, capped at
/// 3, and any remainder collapses to a `+N` overflow marker. Returns None
/// when the requirement carries no tags. trace:TASK-238 STORY-442
pub(crate) fn format_tag_chip(tags: &std::collections::HashSet<String>) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    const MAX_PLAIN: usize = 3;
    let mut batch_tags: Vec<&str> = Vec::new();
    let mut lifecycle_tags: Vec<&str> = Vec::new();
    let mut plain_tags: Vec<&str> = Vec::new();
    for t in tags {
        if t.to_ascii_lowercase().starts_with("batch:") {
            batch_tags.push(t.as_str());
        } else if t.to_ascii_lowercase().starts_with("lifecycle:") {
            lifecycle_tags.push(t.as_str());
        } else {
            plain_tags.push(t.as_str());
        }
    }
    batch_tags.sort_unstable();
    lifecycle_tags.sort_unstable();
    plain_tags.sort_unstable();
    let mut shown: Vec<String> = batch_tags.iter().map(|s| s.to_string()).collect();
    shown.extend(lifecycle_tags.iter().map(|s| s.to_string()));
    shown.extend(plain_tags.iter().take(MAX_PLAIN).map(|s| s.to_string()));
    if plain_tags.len() > MAX_PLAIN {
        shown.push(format!("+{}", plain_tags.len() - MAX_PLAIN));
    }
    Some(shown.join(", "))
}

/// TASK-238: does the tag set contain `want` (case-insensitive)? The
/// predicate behind `aida queue list --tag`. trace:TASK-238 | ai:claude
pub(crate) fn tag_matches_exact(tags: &std::collections::HashSet<String>, want: &str) -> bool {
    tags.iter().any(|t| t.eq_ignore_ascii_case(want))
}

/// TASK-238: does any tag start with `prefix` (case-insensitive)? The
/// predicate behind `aida queue list --tag-prefix`. trace:TASK-238
pub(crate) fn tag_matches_prefix(tags: &std::collections::HashSet<String>, prefix: &str) -> bool {
    let lp = prefix.to_ascii_lowercase();
    tags.iter().any(|t| t.to_ascii_lowercase().starts_with(&lp))
}

/// TASK-238: the `batch:*` tag a requirement carries (lowest-sorting
/// when several), used as the group key for `aida queue list
/// --by-batch`. None when the requirement is un-batched.
/// trace:TASK-238 | ai:claude
pub(crate) fn batch_tag_of(tags: &std::collections::HashSet<String>) -> Option<&str> {
    let mut found: Vec<&str> = tags
        .iter()
        .filter(|t| t.to_ascii_lowercase().starts_with("batch:"))
        .map(|s| s.as_str())
        .collect();
    found.sort_unstable();
    found.into_iter().next()
}

/// TASK-270: strip a redundant leading `batch:` prefix off a string,
/// returning `Some(rest)` only when the prefix was present
/// (case-insensitive). `batch:` is the literal tag printed by `aida
/// queue list`; first-users reflexively copy that whole token back into
/// `--batch` or a positional id. trace:TASK-270 | ai:claude
pub(crate) fn strip_batch_prefix(s: &str) -> Option<&str> {
    let prefix = "batch:";
    s.get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

/// TASK-270: normalize a `--batch` flag value so `--batch NAME` and the
/// redundant `--batch batch:NAME` (the literal tag from `aida queue
/// list`) both resolve to `NAME`. trace:TASK-270 | ai:claude
pub(crate) fn normalize_batch_name(s: &str) -> &str {
    strip_batch_prefix(s).unwrap_or(s)
}

/// TASK-310: parse comma-separated batch-chain names. Names are kept in the
/// caller's declared order; `batch:` prefixes are tolerated per TASK-270.
pub(crate) fn parse_batch_chain(raw: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for part in raw.split(',') {
        let name = normalize_batch_name(part.trim()).trim();
        if name.is_empty() {
            anyhow::bail!(
                "batch list contains an empty name — use comma-separated names like `--batches A,B,C`"
            );
        }
        names.push(name.to_string());
    }
    if names.is_empty() {
        anyhow::bail!("batch list is empty — use comma-separated names like `--batches A,B,C`");
    }
    Ok(names)
}

/// TASK-270: resolve the effective positional id and batch name for
/// `aida queue work`. Accepts `batch:NAME` as a positional id
/// (equivalent to `--batch NAME`) and strips a redundant `batch:`
/// prefix off the `--batch` flag value. At most one element of the
/// returned `(id, batch)` pair is `Some` — clap's
/// `conflicts_with(batch, id)` already rejects passing both, and a
/// `batch:`-prefixed positional is rerouted to the batch slot.
/// trace:TASK-270 | ai:claude
/// TASK-322: true when a `queue work` positional is a `next` / `nextN`
/// head-pickup keyword (vs a SPEC-ID or `batch:NAME`). Used to catch the
/// `--batch` + `nextN` collision that otherwise silently dropped the keyword.
/// trace:TASK-322 | ai:claude
pub(crate) fn is_next_keyword_id(id: &str) -> bool {
    id.to_ascii_lowercase()
        .strip_prefix("next")
        .map(|suffix| suffix.is_empty() || suffix.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

pub(crate) fn resolve_queue_work_batch<'a>(
    id: Option<&'a str>,
    batch: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    if let Some(b) = batch {
        return (None, Some(normalize_batch_name(b)));
    }
    if let Some(i) = id {
        if let Some(name) = strip_batch_prefix(i) {
            return (None, Some(name));
        }
        return (Some(i), None);
    }
    (None, None)
}

/// Resolve the mailbox `from` identity at send time (BUG-1533): the same
/// kind of precedence `current_user_id` uses for the queue, but reordered
/// and widened for *authorship* rather than queue routing — an explicit
/// override, then the launched agent's stable process name
/// (`AIDA_AGENT_NAME`), then the opt-in queue identity (`AIDA_USER`), then
/// the active session-role persona (`AIDA_SESSION_ROLE`) — falling back to
/// the bare shell user only last. Returns which tier resolved so the caller
/// can record it on the message instead of the sender looking silently
/// attributed.
///
/// Deliberately a SEPARATE function from `current_user_id`, not a thin
/// wrapper around it: `current_user_id` is the BUG-89 QUEUE key
/// (`AIDA_USER` first, no agent-name/role tiers, and changing it re-shards
/// which queue a shell sees). Mail authorship must be resolvable without
/// ever requiring a seat to set `AIDA_USER` — a properly-launched agent
/// already has `AIDA_AGENT_NAME` — so adopting a stable mail identity never
/// strands that seat's existing queue entries (BUG-1533 acceptance #6).
// trace:BUG-1533 | ai:claude
pub(crate) fn resolve_mail_sender_identity(
    explicit: Option<&str>,
) -> (String, aida_core::mailbox::SenderSource) {
    let shell_user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok();
    aida_core::mailbox::resolve_sender(
        explicit,
        std::env::var("AIDA_AGENT_NAME").ok().as_deref(),
        std::env::var("AIDA_USER").ok().as_deref(),
        std::env::var("AIDA_SESSION_ROLE").ok().as_deref(),
        shell_user.as_deref(),
    )
}

/// The sender's active session role at send time (BUG-1592, AC2): normalized
/// `AIDA_SESSION_ROLE` when it is actually set, `None` when it isn't. This is
/// deliberately NOT `resolve_effective_role`/`effective_role_resolved`, which
/// force an "implementer" default when the env var is absent — a forced
/// default would make every legacy-shaped send look like a resolved
/// "implementer" seat instead of recording that the role was simply unknown.
/// Called alongside [`resolve_mail_sender_identity`] so the envelope records
/// the role next to the agent id when both are known, mirroring how
/// BUG-1533 recorded the id half of "who sent this".
// trace:BUG-1592 | ai:claude
pub(crate) fn resolve_mail_sender_role() -> Option<String> {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|raw| canonical_role_name(&raw))
}

/// The queue identity for DRAINABLE handoff work (`aida backlog groom`): the
/// draining shell's `USER`, deliberately SKIPPING the agent's `AIDA_USER`
/// mailbox id. An advisor agent grooming on the human's behalf must queue where
/// the human's drain (`aida queue work` / `aida burndown run`, which resolve the
/// queue off the shell `USER`) will actually look — keying it to the agent's own
/// `AIDA_USER` made the groomed batch invisible to the drainer (BUG-605).
/// `--user` still overrides for the explicit case. trace:BUG-605 | ai:claude
pub(crate) fn drain_queue_user_id(user_override: Option<&str>) -> String {
    user_override.map(str::to_string).unwrap_or_else(|| {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default".to_string())
    })
}

/// Resolve the `aida list --user <raw>` value into a concrete handle: the
/// special token `me` (any casing) maps to `current_user`; every other value is
/// a literal handle passed through unchanged. Pure so the `me` → current-user
/// substitution is unit-testable without touching env vars.
/// trace:STORY-662 | ai:claude
pub(crate) fn resolve_list_user_filter(raw: &str, current_user: &str) -> String {
    if raw.eq_ignore_ascii_case("me") {
        current_user.to_string()
    } else {
        raw.to_string()
    }
}

/// Resolve the friendly node name to record at registration (STORY-652).
///
/// Resolution order:
/// 1. an explicit `--node-name` flag (validated, used as-is);
/// 2. at a TTY with no flag: prompt `Node name [<default>]: ` and accept the
///    default on empty input;
/// 3. non-interactive (no TTY) with no flag: the computed default silently.
///
/// The default is `<host>-<user>-<seq>` (e.g. `imac-joe-1`). The chosen name is
/// validated with the same charset rule as node ids so it stays slug-clean.
/// trace:STORY-652 | ai:claude
pub(crate) fn resolve_node_name(
    flag: Option<&str>,
    hostname: &str,
    user: &str,
    seq: &str,
) -> Result<String> {
    let default = aida_core::node::default_node_name(hostname, user, seq);
    let chosen = match flag {
        Some(f) if !f.trim().is_empty() => f.trim().to_string(),
        _ => {
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                use std::io::Write;
                print!("Node name [{}]: ", default);
                std::io::stdout().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                let answer = answer.trim();
                if answer.is_empty() {
                    default.clone()
                } else {
                    answer.to_string()
                }
            } else {
                default.clone()
            }
        }
    };
    if let Err(msg) = aida_core::node::validate_node_id(&chosen) {
        anyhow::bail!("Invalid node name: {}", msg);
    }
    Ok(chosen)
}

/// TASK-618: detect the silent cross-machine queue-collision hazard.
///
/// The distributed queue shards per `user_id`: each writes
/// `registry/queues/<user_id>.yaml` inside the orphan `aida-store` branch.
/// Distinct users (`alice.yaml` / `bob.yaml`) never collide — that's the
/// intended sharding. The hazard is when *two different machines* resolve
/// to the SAME `user_id` and therefore write the SAME file: their
/// concurrent commits produce a merge conflict on the next `aida db sync`
/// rebase (or a rejected non-ff push). This is acute for the BUG-89
/// `"default"` fallback — common in CI/containers where `$USER` /
/// `$AIDA_USER` / `$USERNAME` are all unset, so multiple unconfigured
/// clones silently share one `default.yaml`.
///
/// Per the operator decision on TASK-618 we only WARN (cheap, targets the
/// actual silent case) rather than building conflict-tolerant YAML merges.
/// The warning fires only for the genuinely dangerous shape: the resolved
/// id is the `"default"` fallback AND the existing `default.yaml` already
/// carries at least one entry stamped with a DIFFERENT machine fingerprint
/// than this clone's.
///
/// Pure over its inputs so it's directly unit-testable: pass the resolved
/// `user_id`, this machine's fingerprint, and the iterator of
/// already-on-disk `added_by_machine` values. Returns the foreign
/// fingerprint to name in the warning, or `None` when no warning is due.
/// Entries with no recorded fingerprint (`None` — pre-TASK-618 or non-CLI
/// writers) are ignored: we can't attribute them to a machine, so they
/// never trigger a false alarm.
/// trace:TASK-618 | ai:claude
pub(crate) fn default_queue_collision_fingerprint<'a, I>(
    user_id: &str,
    this_machine: &str,
    existing_fingerprints: I,
) -> Option<String>
where
    I: IntoIterator<Item = Option<&'a str>>,
{
    if user_id != "default" {
        return None;
    }
    existing_fingerprints.into_iter().flatten().find_map(|fp| {
        if !fp.is_empty() && fp != this_machine {
            Some(fp.to_string())
        } else {
            None
        }
    })
}

/// Resolve `--for <role>` / `--all` / active-session-role into the
/// effective queue role filter. Returns `(role_filter, only_unrouted)`:
/// `only_unrouted=true` means filter to entries with no `for_role`
/// (driven by `--for any`); otherwise `role_filter` is `Some(role)` to
/// match `for_role == Some(role)`, or `None` for no role filter.
///
/// `--for X` (non-"any") takes precedence over `--all` — that flag only
/// suppresses the *default* active-role filter, not an explicit override.
/// trace:BUG-87 | ai:claude
pub(crate) fn resolve_queue_role_filter(
    role: Option<&str>,
    all: bool,
    session_role: Option<&str>,
) -> (Option<String>, bool) {
    match role {
        Some(r) if r.eq_ignore_ascii_case("any") => (None, true),
        // TASK-586 / TASK-747: canonicalize the requested role so
        // `--role dialog`→`advisor` and `--role Human`→`human` match the
        // canonical entries written on add. trace:TASK-747 | ai:claude
        Some(r) => (Some(canonical_role_name(r)), false),
        None if all => (None, false),
        None => match session_role {
            Some(s) if !s.is_empty() => (Some(canonical_role_name(s)), false),
            _ => (None, false),
        },
    }
}

/// Predicate: does a queue entry pass the resolved role filter?
/// trace:BUG-87 | ai:claude
pub(crate) fn entry_matches_role_filter(
    for_role: Option<&str>,
    role_filter: Option<&str>,
    only_unrouted: bool,
) -> bool {
    if only_unrouted {
        return for_role.is_none();
    }
    match role_filter {
        Some(r) => for_role == Some(r),
        None => true,
    }
}

/// STORY-333: print a one-line warning if placing `req` at `intended_position`
/// inverts a `BlockedBy` ordering with any already-queued spec. Two shapes:
///
/// - **Forward**: `req` itself is the dependent (has `BlockedBy` edges) and
///   lands ahead of a queued blocker — surface `dependent queued ahead of
///   blocker`.
/// - **Reverse**: `req` is the *blocker* of an already-queued dependent that
///   sits ahead of the position `req` will land at — surface the same
///   message from the dependent's perspective.
///
/// Never refuses (AC8) — staging work ahead of its blocker is sometimes
/// deliberate (e.g. branching from the blocker's branch and building atop
/// it). Caller passes the intended position so we can detect inversion
/// against `i64::MAX` sentinel values (which `queue_add` resolves to
/// "max+1000"). trace:STORY-333 | ai:claude
/// Build only the requirement subset `warn_if_queued_ahead_of_blocker` reads —
/// the spec itself, its `BlockedBy` targets (forward check), and every
/// currently-queued spec (reverse check) — via TARGETED cache-backed lookups,
/// instead of `storage.load()` (a scan of every YAML on the queue write path).
/// The warn helper only does `store.requirements.iter().find(|r| r.id == …)` for
/// exactly these ids, so a store holding their closure is behaviorally
/// identical; a dangling id resolves to `None` here and to a `None` find in the
/// full store alike.
// trace:BUG-634 | ai:claude
pub(crate) fn build_queue_warn_subset(
    backend: &aida_core::CachedGitBackend,
    req: &aida_core::Requirement,
    storage: &Storage,
    user_id: &str,
) -> aida_core::RequirementsStore {
    use aida_core::DatabaseBackend;
    let mut seen: HashSet<Uuid> = HashSet::new();
    seen.insert(req.id);
    let mut requirements: Vec<aida_core::Requirement> = vec![req.clone()];
    let mut want: Vec<Uuid> = Vec::new();
    for rel in &req.relationships {
        if matches!(rel.rel_type, aida_core::RelationshipType::BlockedBy) {
            want.push(rel.target_id);
        }
    }
    if let Ok(entries) = storage.queue_list(user_id, true) {
        for e in entries {
            want.push(e.requirement_id);
        }
    }
    for id in want {
        if seen.insert(id) {
            if let Ok(Some(r)) = backend.get_requirement(&id) {
                requirements.push(r);
            }
        }
    }
    aida_core::RequirementsStore {
        requirements,
        ..Default::default()
    }
}

pub(crate) fn warn_if_queued_ahead_of_blocker(
    req: &aida_core::Requirement,
    intended_position: i64,
    store: &aida_core::RequirementsStore,
    storage: &Storage,
    user_id: &str,
) {
    let raw_entries = match storage.queue_list(user_id, true) {
        Ok(v) => v,
        Err(_) => return,
    };
    let dep_display = |r: &aida_core::Requirement| -> String {
        r.agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("?")
            .to_string()
    };

    // Forward: this spec has BlockedBy edges pointing at queued blockers
    // that sit at a *later* position than `intended_position`.
    for rel in req
        .relationships
        .iter()
        .filter(|r| matches!(r.rel_type, aida_core::RelationshipType::BlockedBy))
    {
        let Some(target) = store.requirements.iter().find(|r| r.id == rel.target_id) else {
            continue;
        };
        if matches!(target.status, aida_core::RequirementStatus::Completed) {
            continue;
        }
        let Some(blocker_entry) = raw_entries.iter().find(|e| e.requirement_id == target.id) else {
            continue;
        };
        if intended_position < blocker_entry.position {
            eprintln!(
                "  {}  {} queued ahead of {}, which blocks it",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                dep_display(req).bold(),
                dep_display(target).bold(),
            );
        }
    }

    // Reverse: this spec *is* the blocker. Walk queued specs and warn
    // about any whose BlockedBy points at us and whose queued position
    // would land ahead of where this spec is going.
    for entry in &raw_entries {
        if entry.requirement_id == req.id {
            continue;
        }
        let Some(other) = store
            .requirements
            .iter()
            .find(|r| r.id == entry.requirement_id)
        else {
            continue;
        };
        let points_at_us = other.relationships.iter().any(|r| {
            matches!(r.rel_type, aida_core::RelationshipType::BlockedBy) && r.target_id == req.id
        });
        if points_at_us && entry.position < intended_position {
            eprintln!(
                "  {}  {} queued ahead of {}, which blocks it",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                dep_display(other).bold(),
                dep_display(req).bold(),
            );
        }
    }
}

// trace:STORY-0368 | ai:claude
pub(crate) fn effort_display_id(req: &Requirement) -> &str {
    req.agreed_id
        .as_deref()
        .or(req.spec_id.as_deref())
        .unwrap_or("?")
}

pub(crate) fn latest_effort_for_req(
    project_root: &std::path::Path,
    req: &Requirement,
) -> Option<(
    effort_calibration::EffortTouchpoint,
    effort_calibration::EffortBucket,
)> {
    let spec = effort_display_id(req);
    effort_calibration::read_capture(project_root, spec)
        .and_then(|r| r.latest_effort())
        .or_else(|| {
            let tags: Vec<String> = req.tags.iter().cloned().collect();
            [
                effort_calibration::EffortTouchpoint::Review,
                effort_calibration::EffortTouchpoint::Impl,
                effort_calibration::EffortTouchpoint::Plan,
                effort_calibration::EffortTouchpoint::Open,
            ]
            .into_iter()
            .find_map(|t| effort_calibration::effort_from_tags(&tags, t).map(|e| (t, e)))
        })
}

pub(crate) fn print_effort_load_for_requirements<'a>(
    project_root: &std::path::Path,
    title: &str,
    requirements: impl Iterator<Item = &'a Requirement>,
) {
    let mut known = Vec::new();
    let mut unknown = 0usize;
    for req in requirements {
        match latest_effort_for_req(project_root, req) {
            Some((touchpoint, bucket)) => {
                known.push((effort_display_id(req).to_string(), touchpoint, bucket))
            }
            None => unknown += 1,
        }
    }
    let total: u32 = known.iter().map(|(_, _, b)| b.minutes()).sum();
    println!(
        "{}: {} across {} estimated item{} ({} unknown)",
        title.bold(),
        effort_calibration::format_minutes(total).cyan(),
        known.len(),
        if known.len() == 1 { "" } else { "s" },
        unknown
    );
    for (spec, touchpoint, bucket) in known.iter().take(12) {
        println!("  {:<12} {:<6} {}", spec, touchpoint.as_str(), bucket);
    }
    if known.len() > 12 {
        println!("  …and {} more", known.len() - 12);
    }
}

pub(crate) fn queued_requirement_ids(storage: &Storage, user_id: &str) -> Result<HashSet<Uuid>> {
    Ok(storage
        .queue_list(user_id, false)?
        .into_iter()
        .map(|e| e.requirement_id)
        .collect())
}

/// `aida stack {show,list}` (STORY-248). Reads `.aida/stacks.json` —
/// no requirement-store dependency, so dispatches pre-storage like
/// `aida drain status`. Prints "No stacked branches." (exit 0) on an
/// empty graph so a quiet project never errors. `--prune-stale` (show
/// only) drops entries whose branch no longer exists locally or on
/// origin; the prune writes the graph back atomically.
/// trace:STORY-248 | ai:claude
pub(crate) fn handle_stack_command(cmd: &StackCommand) -> Result<()> {
    let project_root = find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut graph = stacks::load(&project_root);

    match cmd {
        StackCommand::Show { json, prune_stale } => {
            if *prune_stale {
                let stale: Vec<String> = graph
                    .entries
                    .values()
                    .filter(|e| !branch_exists_anywhere(&project_root, &e.branch))
                    .map(|e| e.branch.clone())
                    .collect();
                if !stale.is_empty() {
                    for s in &stale {
                        stacks::remove(&mut graph, s);
                    }
                    stacks::save(&project_root, &graph)?;
                    if !*json {
                        eprintln!(
                            "  {} pruned {} stale entr{}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            stale.len(),
                            if stale.len() == 1 { "y" } else { "ies" }
                        );
                    }
                }
            }
            if *json {
                let chains: Vec<Vec<&stacks::StackEntry>> = stacks::chains(&graph);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "chains": chains }))?
                );
                return Ok(());
            }
            if graph.is_empty() {
                println!("No stacked branches.");
                return Ok(());
            }
            for chain in stacks::chains(&graph) {
                // Root parent of the chain is the first entry's parent —
                // typically `main`, sometimes a since-merged branch.
                if let Some(first) = chain.first() {
                    println!("{}", first.parent_branch.dimmed());
                }
                for (depth, entry) in chain.iter().enumerate() {
                    let indent = "  ".repeat(depth + 1);
                    let spec = entry
                        .spec_id
                        .as_deref()
                        .map(|s| format!(" {}", format!("({})", s).dimmed()))
                        .unwrap_or_default();
                    println!("{}└─ {}{}", indent, entry.branch.cyan(), spec);
                }
            }
            Ok(())
        }
        StackCommand::List { json } => {
            let chains = stacks::chains(&graph);
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "chains": chains }))?
                );
                return Ok(());
            }
            if chains.is_empty() {
                println!("No stacked branches.");
                return Ok(());
            }
            for chain in chains {
                let root = chain
                    .first()
                    .map(|e| e.parent_branch.as_str())
                    .unwrap_or("main");
                let rest: Vec<&str> = chain.iter().map(|e| e.branch.as_str()).collect();
                println!("{} → {}", root.dimmed(), rest.join(" → ").cyan());
            }
            Ok(())
        }
    }
}

/// `aida headless tail` — clean tailer for `.aida/headless-logs/<spec>-<lease>.jsonl`
/// (TASK-398). Wraps the right JSONL filtering so the user doesn't have to
/// remember (and debug) the non-obvious jq pipeline that picks text content
/// out of multi-block assistant messages. trace:TASK-398
pub(crate) fn handle_headless_command(cmd: &HeadlessCommand) -> Result<()> {
    match cmd {
        HeadlessCommand::Tail {
            target,
            list,
            with_tools,
            tools_only,
            include_user,
            no_follow,
            since,
        } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let since_duration = match since {
                Some(s) => Some(headless_tail::parse_since(s)?),
                None => None,
            };
            let color = std::io::IsTerminal::is_terminal(&std::io::stdout())
                && std::env::var_os("NO_COLOR").is_none();
            let opts = headless_tail::TailOptions {
                selector: target.clone(),
                list: *list,
                with_tools: *with_tools,
                tools_only: *tools_only,
                include_user: *include_user,
                follow: !*no_follow,
                since: since_duration,
                color,
            };
            headless_tail::handle_tail(&project_root, &opts)
        }
    }
}

/// `aida worker directives` — list the FIFO of pending directives the
/// `aida-worker` shell function will act on next (TASK-294). Reads
/// `.aida/worker.cmd` only — no requirement-store dependency, so it
/// dispatches pre-storage like `aida drain status`. Prints "No pending
/// directives." (exit 0) when the file is empty or absent so a quiet
/// project never errors. trace:TASK-294 | ai:claude
///
/// `aida worker gc` prunes stale drain orders — spec-targeted drain lines
/// whose target spec is already archived / Completed / Rejected. The worker
/// dispatch stays pre-storage; the gc arm opens the cache-backed backend
/// itself to resolve target-spec statuses.
// trace:BUG-723 | ai:claude
pub(crate) fn handle_worker_command(cmd: &WorkerCommand) -> Result<()> {
    match cmd {
        WorkerCommand::Directives { json } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let path = worker::worker_cmd_path(&project_root);
            let directives = worker::parse_directives(&path);
            if *json {
                println!("{}", worker::render_json(&directives));
                return Ok(());
            }
            if directives.is_empty() {
                println!("No pending directives.");
            } else {
                print!("{}", worker::render_human(&directives));
            }
            Ok(())
        }
        // trace:BUG-723 | ai:claude
        WorkerCommand::Gc { dry_run } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            run_worker_gc(&project_root, *dry_run)
        }
    }
}
