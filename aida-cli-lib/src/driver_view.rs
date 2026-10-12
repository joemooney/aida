//! The derived per-spec driver, wired into `aida ps` and `aida list`.
//!
//! Every in-progress spec resolves to exactly one driver — `relay:<run>`,
//! `drain:<pid>`, `agent:<session>`, `queued (batch:X)` or `idle <age>` —
//! computed on each read from the driver registry (`.aida/drivers/`), the
//! session leases, the live drain lock and the spec's batch tags. Nothing here
//! is ever written to the spec. The pure resolution order lives in
//! [`aida_core::driver_registry::resolve_driver`]; this module only gathers
//! the probed inputs and applies the result to the two surfaces.
// trace:STORY-1653 | ai:claude

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aida_core::driver_registry::{self, DerivedDriver, DriverInputs, DriverRecord};
use chrono::{DateTime, Utc};

use crate::{LeaseState, LeaseStateGlyph, PsOrphan, PsRow, SessionLease};

/// A live drain and the specs it covers.
#[derive(Debug, Clone, Default)]
pub(crate) struct DrainCover {
    pub(crate) pid: u32,
    /// Upper-cased member spec ids (lock spec set + running state members).
    pub(crate) specs: HashSet<String>,
    /// `batch:<name>` when the drain is a batch drain.
    pub(crate) batch_tag: Option<String>,
}

/// Everything needed to resolve drivers for many specs, probed once.
// trace:STORY-1653 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct DriverContext {
    pub(crate) now: DateTime<Utc>,
    pub(crate) live: Vec<DriverRecord>,
    pub(crate) drain: Option<DrainCover>,
}

/// Where driver records live for a checkout: the MAIN worktree root, as for
/// session leases. A linked worktree's own `.aida/drivers/` is never used, so
/// a driver registered from any checkout is seen from every other one.
// trace:STORY-1653 | ai:claude
pub(crate) fn driver_registry_root(project_root: &Path) -> PathBuf {
    crate::main_worktree_root_from(project_root)
}

impl DriverContext {
    /// Probe the registry (under the main checkout) and the drain lock under
    /// `project_root`.
    pub(crate) fn load(project_root: &Path, now: DateTime<Utc>) -> Self {
        // trace:STORY-1653 | ai:claude
        let live = driver_registry::live_drivers(&driver_registry_root(project_root), now);
        let drain = crate::drain_lock::read_pid_live_lock(project_root).map(|lock| {
            let mut specs: HashSet<String> =
                lock.specs.iter().map(|s| s.to_ascii_uppercase()).collect();
            let state = crate::drain_state::DrainState::read(project_root);
            if let Some(state) = &state {
                specs.extend(
                    state
                        .members
                        .iter()
                        .filter(|m| m.is_running())
                        .map(|m| m.spec.to_ascii_uppercase()),
                );
            }
            DrainCover {
                pid: lock.pid,
                specs,
                batch_tag: state
                    .and_then(|s| s.batch)
                    .map(|b| format!("batch:{}", b.trim_start_matches("batch:"))),
            }
        });
        Self { now, live, drain }
    }

    /// Resolve one spec. `ids` are the spec's display/origin ids; `lease` is
    /// its spec-scoped lease (if any) and `owner_alive` whether that lease's
    /// own process is alive.
    pub(crate) fn resolve(
        &self,
        ids: &[&str],
        tags: &[String],
        lease: Option<&SessionLease>,
        owner_alive: bool,
        modified_at: Option<DateTime<Utc>>,
    ) -> DerivedDriver {
        let drain_pid = self.drain_pid_for(ids, tags);
        let last_activity = match (lease.map(|l| l.started_at), modified_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        driver_registry::resolve_driver(
            &DriverInputs {
                lease_driver: lease.and_then(|l| l.driver.as_deref()),
                lease_session: lease.map(|l| l.id.as_str()),
                lease_owner_alive: owner_alive,
                drain_pid,
                tags,
                last_activity,
            },
            &self.live,
            self.now,
        )
    }

    /// Pid of the live drain covering the spec (by member id or batch tag).
    fn drain_pid_for(&self, ids: &[&str], tags: &[String]) -> Option<u32> {
        self.drain
            .as_ref()
            .filter(|d| {
                ids.iter()
                    .any(|id| !id.is_empty() && d.specs.contains(&id.to_ascii_uppercase()))
                    || d.batch_tag.as_ref().is_some_and(|b| tags.contains(b))
            })
            .map(|d| d.pid)
    }

    /// True when the spec's coverage is authoritative enough to vouch for a
    /// session whose own process is gone: the lease itself names a live
    /// registered driver, or a live drain covers the spec. A bare batch-tag
    /// match against a relay selector is NOT enough — it can label the spec,
    /// but it says nothing about this particular dead session.
    // trace:STORY-1653 | ai:claude
    pub(crate) fn vouches_for_dead_session(
        &self,
        ids: &[&str],
        tags: &[String],
        lease: Option<&SessionLease>,
    ) -> bool {
        let named = lease
            .and_then(|l| l.driver.as_deref())
            .map(str::trim)
            .is_some_and(|label| self.live.iter().any(|r| r.label() == label));
        named || self.drain_pid_for(ids, tags).is_some()
    }
}

/// The best spec-scoped lease for `ids`, ranked by `(live, carries a driver,
/// most recently started)`, plus whether it is live. `is_live` is consulted
/// per lease, so a fresh live operator lease always beats a stale
/// relay-attributed one — `aida list` and `aida ps` then agree on who owns
/// the spec, and the owner reads alive when ANY of its leases is live.
// trace:STORY-1653 | ai:claude
pub(crate) fn lease_for_spec<'a>(
    leases: &'a [SessionLease],
    ids: &[&str],
    mut is_live: impl FnMut(&SessionLease) -> bool,
) -> Option<(&'a SessionLease, bool)> {
    let mut best: Option<(&'a SessionLease, bool)> = None;
    for l in leases.iter().filter(|l| {
        ids.iter()
            .any(|id| !id.is_empty() && l.scope.eq_ignore_ascii_case(id))
    }) {
        let live = is_live(l);
        let better = match best {
            None => true,
            Some((b, b_live)) => {
                (live, l.driver.is_some(), l.started_at)
                    > (b_live, b.driver.is_some(), b.started_at)
            }
        };
        if better {
            best = Some((l, live));
        }
    }
    best
}

/// Lease liveness for `aida list`, probing the process table only when the
/// lease cannot answer from its own recorded pids: a worktree lease with no
/// `active_pid` is the only shape that needs the live claude-session probe
/// (a full `/proc` walk), and that probe runs at most once per command.
// trace:STORY-1653 | ai:claude
pub(crate) fn lease_is_live_lazy(
    l: &SessionLease,
    probe: &mut Option<Vec<crate::process_probe::LiveSession>>,
    now: DateTime<Utc>,
) -> bool {
    let advisory = (l.review_verb || l.claim_verb) && l.worktree_path.as_os_str().is_empty();
    let needs_probe = !advisory && l.active_pid.is_none() && l.worktree_path.exists();
    let state = if needs_probe {
        let live = probe.get_or_insert_with(crate::process_probe::probe_live_claude_sessions);
        crate::lease_state_for(l, live, now)
    } else {
        crate::lease_state_for(l, &[], now)
    };
    matches!(state, LeaseState::Live)
}

/// Spec facts the resolver needs that the ps index does not carry.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpecFacts {
    pub(crate) tags: Vec<String>,
    pub(crate) modified_at: Option<DateTime<Utc>>,
    /// The spec is In Progress — the only status a driver applies to.
    pub(crate) in_progress: bool,
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Upper-cased spec id → facts, from the cache summaries (cache-fast path).
fn spec_facts(project_root: &Path) -> HashMap<String, SpecFacts> {
    let mut out = HashMap::new();
    let Some(store_path) = crate::detect_distributed_store_from(project_root) else {
        return out;
    };
    let Ok(backend) = crate::advance_backend(&store_path) else {
        return out;
    };
    let filter = aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    };
    for s in backend.list_summaries(&filter).unwrap_or_default() {
        let facts = SpecFacts {
            tags: s.tags.clone(),
            modified_at: parse_ts(&s.modified_at),
            in_progress: status_is_in_progress(&s.status),
        };
        for id in [s.agreed_id.as_deref(), s.spec_id.as_deref()]
            .into_iter()
            .flatten()
        {
            out.insert(id.to_ascii_uppercase(), facts.clone());
        }
    }
    out
}

/// `aida ps` overlay: resolve the driver of every In Progress spec-linked row
/// and every orphan. A row whose own process is gone is lifted to live only
/// when its coverage vouches for the dead session (the lease names the live
/// driver, or a live drain covers the spec) — and even then only the
/// misleading stalled-family readings are cleared (see [`lift_dead_row`]).
/// A live relay does not make an uncommitted diff any less at risk.
// trace:STORY-1653 | ai:claude
pub(crate) fn overlay_running_work(
    project_root: &Path,
    leases: &[SessionLease],
    rows: &mut [PsRow],
    orphans: &mut Vec<PsOrphan>,
    now: DateTime<Utc>,
) {
    if rows.iter().all(|r| r.spec.is_none()) && orphans.is_empty() {
        return;
    }
    let ctx = DriverContext::load(project_root, now);
    let facts = spec_facts(project_root);
    apply_overlay(&ctx, &facts, leases, rows, orphans);
}

/// The pure half of [`overlay_running_work`] (test seam).
pub(crate) fn apply_overlay(
    ctx: &DriverContext,
    facts: &HashMap<String, SpecFacts>,
    leases: &[SessionLease],
    rows: &mut [PsRow],
    orphans: &mut Vec<PsOrphan>,
) {
    let empty = SpecFacts::default();
    for row in rows.iter_mut() {
        let Some(spec) = row.spec.clone() else {
            continue;
        };
        // Drivers apply to In Progress specs only; unknown specs are skipped.
        let Some(f) = facts
            .get(&spec.to_ascii_uppercase())
            .filter(|f| f.in_progress)
        else {
            continue;
        };
        let owner_alive = row.pid.is_some() || matches!(row.state, LeaseState::Live);
        let ids = [spec.as_str()];
        let d = ctx.resolve(&ids, &f.tags, Some(&row.lease), owner_alive, f.modified_at);
        if d.is_driven()
            && !owner_alive
            && ctx.vouches_for_dead_session(&ids, &f.tags, Some(&row.lease))
        {
            lift_dead_row(row);
        }
        row.driver = Some(d);
    }
    // A spec whose row reads live under a driven resolution is not also an
    // orphan. A row that only carries a label (a tag match on a dead session
    // that stayed stale) keeps its orphan entry, which then prints under
    // "Driven" — otherwise the spec would vanish from the default view.
    // trace:STORY-1653 | ai:claude
    let driven_rows: HashSet<String> = rows
        .iter()
        .filter(|r| matches!(r.state, LeaseState::Live))
        .filter(|r| r.driver.as_ref().is_some_and(DerivedDriver::is_driven))
        .filter_map(|r| r.spec.as_ref().map(|s| s.to_ascii_uppercase()))
        .collect();
    orphans.retain(|o| !driven_rows.contains(&o.spec.to_ascii_uppercase()));
    for o in orphans.iter_mut() {
        let f = facts.get(&o.spec.to_ascii_uppercase()).unwrap_or(&empty);
        // Orphans have no live lease by construction; pick the best dead one.
        let lease = lease_for_spec(leases, &[o.spec.as_str()], |_| false).map(|(l, _)| l);
        o.driver = Some(ctx.resolve(&[o.spec.as_str()], &f.tags, lease, false, f.modified_at));
    }
}

/// Lift a vouched-for dead-pid row to live. Only `stalled` / `unknown` — the
/// readings that would wrongly say "nothing is working this" — are cleared.
/// Salvageable, awaiting-agent, stopped and awaiting-integration rows keep
/// their dispatch reading AND their own lease state untouched; a moving row
/// keeps its reading and is only marked live.
// trace:STORY-1653 | ai:claude
pub(crate) fn lift_dead_row(row: &mut PsRow) {
    use crate::dispatch_health_ps::DispatchState as D;
    match row.dispatch.as_ref().map(|d| d.state) {
        None | Some(D::Moving) => row.state = LeaseState::Live,
        Some(D::Stalled | D::Unknown) => {
            row.state = LeaseState::Live;
            row.dispatch = None;
        }
        Some(D::Salvageable | D::AwaitingAgent | D::Stopped | D::AwaitingIntegration) => {}
    }
}

/// True when a live driver works this orphan — it belongs in the driven
/// section, not under "Orphaned".
pub(crate) fn orphan_is_driven(o: &PsOrphan) -> bool {
    o.driver.as_ref().is_some_and(DerivedDriver::is_driven)
}

/// Insert `driver` / `driver_kind` (null when unresolved) into a JSON object.
pub(crate) fn insert_json_fields(v: &mut serde_json::Value, d: Option<&DerivedDriver>) {
    if let Some(obj) = v.as_object_mut() {
        let (label, kind) = match d {
            Some(d) => (d.label().into(), d.kind().into()),
            None => (serde_json::Value::Null, serde_json::Value::Null),
        };
        obj.insert("driver".into(), label);
        obj.insert("driver_kind".into(), kind);
    }
}

/// `(driver, driver_kind)` TOON cells (blank when unresolved).
pub(crate) fn toon_cells(d: Option<&DerivedDriver>) -> [String; 2] {
    match d {
        Some(d) => [d.label(), d.kind().to_string()],
        None => [String::new(), String::new()],
    }
}

/// The `aida ps` live-cell text: the state label, followed by the driver
/// when one is resolved (`live · relay:lib-split`).
pub(crate) fn ps_live_label(row: &PsRow) -> String {
    let base = format!("{} {}", row.state.glyph(), row.state.label());
    match row.driver.as_ref().filter(|d| d.is_driven()) {
        Some(d) => format!("{base} · {}", d.label()),
        None => base,
    }
}

/// Print driven orphans (no live session of their own, but a live relay or
/// drain covers them) under their own informational header.
pub(crate) fn print_driven_orphans(orphans: &[PsOrphan]) {
    use colored::Colorize;
    let driven: Vec<&PsOrphan> = orphans.iter().filter(|o| orphan_is_driven(o)).collect();
    if driven.is_empty() {
        return;
    }
    let info = crate::glyph(crate::glyphs::Glyph::Info);
    println!();
    println!(
        "{}",
        "Driven (no session of their own; a live driver covers them)"
            .bold()
            .cyan()
    );
    for o in driven {
        let label = o
            .driver
            .as_ref()
            .map(DerivedDriver::label)
            .unwrap_or_default();
        println!(
            "  {} {}  {}  {}",
            info.cyan(),
            o.spec.cyan().bold(),
            crate::truncate(&o.title, 48).dimmed(),
            label.cyan(),
        );
    }
}

/// The suffix for a genuine orphan line: its queued/idle reading.
pub(crate) fn orphan_driver_suffix(o: &PsOrphan) -> String {
    o.driver
        .as_ref()
        .map(|d| format!(" · {}", d.label()))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// aida list
// ---------------------------------------------------------------------------

/// Resolve the driver of every in-progress row of an `aida list` result.
/// Keyed by spec uuid; non-in-progress rows are absent.
// trace:STORY-1653 | ai:claude
pub(crate) fn list_driver_map(
    project_root: Option<&Path>,
    reqs: &[aida_core::RequirementSummary],
) -> HashMap<uuid::Uuid, DerivedDriver> {
    let mut out = HashMap::new();
    let Some(root) = project_root else {
        return out;
    };
    if !reqs.iter().any(is_in_progress) {
        return out;
    }
    let now = Utc::now();
    let ctx = DriverContext::load(root, now);
    let leases = crate::list_leases(root);
    // The /proc-walking probe runs lazily, at most once (lease_is_live_lazy).
    let mut probe = None;
    for r in reqs.iter().filter(|r| is_in_progress(r)) {
        let ids: Vec<&str> = [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let chosen = lease_for_spec(&leases, &ids, |l| lease_is_live_lazy(l, &mut probe, now));
        let lease = chosen.map(|(l, _)| l);
        let owner_alive = chosen.is_some_and(|(_, live)| live);
        let d = ctx.resolve(&ids, &r.tags, lease, owner_alive, parse_ts(&r.modified_at));
        out.insert(r.id, d);
    }
    out
}

/// In-progress, in any of the status spellings the cache carries.
pub(crate) fn is_in_progress(r: &aida_core::RequirementSummary) -> bool {
    status_is_in_progress(&r.status)
}

fn status_is_in_progress(status: &str) -> bool {
    let s: String = status
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    s.eq_ignore_ascii_case("inprogress")
}

/// Valid `--driver` filter values.
pub(crate) const DRIVER_FILTER_KINDS: &[&str] = &["relay", "drain", "agent", "queued", "idle"];

/// Apply `aida list --driver <kind>`: keep only rows whose derived driver is
/// that kind (which implies in-progress).
pub(crate) fn retain_by_driver(
    reqs: &mut Vec<aida_core::RequirementSummary>,
    drivers: &HashMap<uuid::Uuid, DerivedDriver>,
    want: &str,
) -> anyhow::Result<()> {
    let want = want.trim().to_ascii_lowercase();
    if !DRIVER_FILTER_KINDS.contains(&want.as_str()) {
        anyhow::bail!(
            "unknown --driver `{want}`; valid values: {}",
            DRIVER_FILTER_KINDS.join(", ")
        );
    }
    reqs.retain(|r| drivers.get(&r.id).is_some_and(|d| d.matches_filter(&want)));
    Ok(())
}

/// TOON/`--fields` cell for the two driver fields; `None` for other fields.
pub(crate) fn list_cell(
    drivers: &HashMap<uuid::Uuid, DerivedDriver>,
    r: &aida_core::RequirementSummary,
    field: &str,
) -> Option<String> {
    let d = drivers.get(&r.id);
    match field {
        "driver" => Some(d.map(DerivedDriver::label).unwrap_or_default()),
        "driver_kind" => Some(d.map(|d| d.kind().to_string()).unwrap_or_default()),
        _ => None,
    }
}

/// Default TOON list columns gain `driver`, `driver_kind` when any row
/// resolved a driver and the caller did not choose `--fields`.
pub(crate) fn with_driver_columns(
    mut fields: Vec<String>,
    explicit_fields: bool,
    drivers: &HashMap<uuid::Uuid, DerivedDriver>,
) -> Vec<String> {
    if !explicit_fields && !drivers.is_empty() {
        for f in ["driver", "driver_kind"] {
            if !fields.iter().any(|x| x == f) {
                fields.push(f.to_string());
            }
        }
    }
    fields
}

/// The human status-cell suffix for one row: ` · relay:lib-split`.
pub(crate) fn status_suffix(
    drivers: &HashMap<uuid::Uuid, DerivedDriver>,
    r: &aida_core::RequirementSummary,
) -> String {
    drivers
        .get(&r.id)
        .map(|d| format!(" · {}", d.label()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    // trace:STORY-1653 | ai:claude
    use super::*;
    use crate::dispatch_health_ps::DispatchState;
    use aida_core::driver_registry::DriverKind;
    use std::path::PathBuf;

    fn lease(id: &str, scope: &str, driver: Option<&str>, age_secs: i64) -> SessionLease {
        SessionLease {
            id: id.to_string(),
            scope: scope.to_string(),
            slug: scope.to_ascii_lowercase(),
            owner: "tester".into(),
            worktree_path: PathBuf::from("/nonexistent/story1653"),
            branch: scope.to_ascii_lowercase(),
            started_at: Utc::now() - chrono::Duration::seconds(age_secs),
            hostname: "h".into(),
            role: Some("implementer".into()),
            creator_pid: None,
            creator_pid_start_time: None,
            active_pid: None,
            active_pid_start_time: None,
            cargo_target_dir: None,
            parent_project_root: None,
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
            zen_intent_token: None,
            escalated_to_human: None,
            parent_branch: None,
            parent_branch_sha: None,
            review_verb: false,
            claim_verb: false,
            manual_enter_at: None,
            interrupted_at: None,
            interrupted_reason: None,
            driver: driver.map(String::from),
        }
    }

    fn row(spec: &str, driver: Option<&str>, dispatch: Option<DispatchState>) -> PsRow {
        PsRow {
            lease: lease("aaaabbbbcccc", spec, driver, 60),
            state: LeaseState::Stale,
            role: Some("implementer".into()),
            lease_role: Some("implementer".into()),
            pid: None,
            pid_started_at: None,
            elapsed_secs: 60,
            spec: Some(spec.into()),
            dispatch: dispatch.map(|state| crate::PsDispatch {
                state,
                hint: Some("hint".into()),
                dirty: state == DispatchState::Salvageable,
                ahead_of_main: 0,
                untracked_only: false,
                commit_movement_from: None,
            }),
            locked_by: None,
            mail_identity: None,
            activity: None,
            driver: None,
        }
    }

    fn ctx() -> DriverContext {
        let now = Utc::now();
        DriverContext {
            now,
            live: vec![DriverRecord {
                kind: DriverKind::Relay,
                id: "lib-split".into(),
                pid: 4242,
                pid_start_time: None,
                selector: Some("batch:next".into()),
                started_at: now,
                heartbeat_at: now,
                ttl_secs: None,
                seat: None,
                handoff: None,
            }],
            drain: None,
        }
    }

    fn overlay(rows: &mut [PsRow], in_progress: bool) {
        let facts = HashMap::from([(
            "TASK-1".to_string(),
            SpecFacts {
                tags: vec!["batch:next".into()],
                modified_at: None,
                in_progress,
            },
        )]);
        let mut orphans = Vec::new();
        apply_overlay(&ctx(), &facts, &[], rows, &mut orphans);
    }

    #[test]
    fn tag_match_alone_labels_but_never_lifts_a_dead_row() {
        let mut rows = [row("TASK-1", None, Some(DispatchState::Stalled))];
        overlay(&mut rows, true);
        assert_eq!(rows[0].state, LeaseState::Stale);
        assert_eq!(
            rows[0].dispatch.as_ref().map(|d| d.state),
            Some(DispatchState::Stalled)
        );
        assert_eq!(
            rows[0].driver.as_ref().map(DerivedDriver::label).as_deref(),
            Some("relay:lib-split")
        );
    }

    #[test]
    fn lease_named_driver_clears_only_stalled_family_readings() {
        let named = Some("relay:lib-split");
        let mut rows = [
            row("TASK-1", named, Some(DispatchState::Stalled)),
            row("TASK-1", named, Some(DispatchState::Unknown)),
            row("TASK-1", named, Some(DispatchState::Salvageable)),
            row("TASK-1", named, Some(DispatchState::AwaitingAgent)),
            row("TASK-1", named, Some(DispatchState::AwaitingIntegration)),
            row("TASK-1", named, Some(DispatchState::Stopped)),
        ];
        overlay(&mut rows, true);
        for r in &rows[..2] {
            assert_eq!(r.state, LeaseState::Live);
            assert!(r.dispatch.is_none());
        }
        for (r, want) in rows[2..].iter().zip([
            DispatchState::Salvageable,
            DispatchState::AwaitingAgent,
            DispatchState::AwaitingIntegration,
            DispatchState::Stopped,
        ]) {
            assert_eq!(r.state, LeaseState::Stale, "{want:?} row keeps its state");
            assert_eq!(r.dispatch.as_ref().map(|d| d.state), Some(want));
        }
    }

    #[test]
    fn non_in_progress_rows_are_never_lifted_or_labelled() {
        let mut rows = [row(
            "TASK-1",
            Some("relay:lib-split"),
            Some(DispatchState::Stalled),
        )];
        overlay(&mut rows, false);
        assert_eq!(rows[0].state, LeaseState::Stale);
        assert!(rows[0].dispatch.is_some());
        assert!(rows[0].driver.is_none());
    }

    #[test]
    fn lease_for_spec_prefers_a_live_lease_over_a_stale_relay_lease() {
        let leases = vec![
            lease("relay-old", "TASK-1", Some("relay:gone"), 3600),
            lease("operator", "TASK-1", None, 7200),
        ];
        let (l, live) = lease_for_spec(&leases, &["TASK-1"], |l| l.id == "operator").unwrap();
        assert_eq!(l.id, "operator");
        assert!(live);
        // Nothing live: the driver-carrying lease wins and the owner reads dead.
        let (l, live) = lease_for_spec(&leases, &["TASK-1"], |_| false).unwrap();
        assert_eq!(l.id, "relay-old");
        assert!(!live);
    }
}
