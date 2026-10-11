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
use std::path::Path;

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

impl DriverContext {
    /// Probe the registry and the drain lock under `project_root`.
    pub(crate) fn load(project_root: &Path, now: DateTime<Utc>) -> Self {
        let live = driver_registry::live_drivers(project_root, now);
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
        let drain_pid = self
            .drain
            .as_ref()
            .filter(|d| {
                ids.iter()
                    .any(|id| !id.is_empty() && d.specs.contains(&id.to_ascii_uppercase()))
                    || d.batch_tag.as_ref().is_some_and(|b| tags.contains(b))
            })
            .map(|d| d.pid);
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
}

/// The best spec-scoped lease for `ids`: a lease carrying a driver first,
/// then the most recently started.
pub(crate) fn lease_for_spec<'a>(
    leases: &'a [SessionLease],
    ids: &[&str],
) -> Option<&'a SessionLease> {
    leases
        .iter()
        .filter(|l| {
            ids.iter()
                .any(|id| !id.is_empty() && l.scope.eq_ignore_ascii_case(id))
        })
        .max_by_key(|l| (l.driver.is_some(), l.started_at))
}

/// Spec facts the resolver needs that the ps index does not carry.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpecFacts {
    pub(crate) tags: Vec<String>,
    pub(crate) modified_at: Option<DateTime<Utc>>,
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

/// `aida ps` overlay: resolve the driver of every spec-linked row and every
/// orphan. A row whose own process is gone but whose driver is live (relay,
/// drain or registered agent) is lifted to live with its stalled/salvage
/// dispatch reading cleared — the driver, not the dead lease pid, says the
/// spec is being worked.
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
        let f = facts.get(&spec.to_ascii_uppercase()).unwrap_or(&empty);
        let owner_alive = row.pid.is_some() || matches!(row.state, LeaseState::Live);
        let d = ctx.resolve(
            &[spec.as_str()],
            &f.tags,
            Some(&row.lease),
            owner_alive,
            f.modified_at,
        );
        if d.is_driven() && !owner_alive {
            row.state = LeaseState::Live;
            row.dispatch = None;
        }
        row.driver = Some(d);
    }
    // A spec whose row the driver keeps live is not also an orphan.
    let driven_rows: HashSet<String> = rows
        .iter()
        .filter(|r| r.driver.as_ref().is_some_and(DerivedDriver::is_driven))
        .filter_map(|r| r.spec.as_ref().map(|s| s.to_ascii_uppercase()))
        .collect();
    orphans.retain(|o| !driven_rows.contains(&o.spec.to_ascii_uppercase()));
    for o in orphans.iter_mut() {
        let f = facts.get(&o.spec.to_ascii_uppercase()).unwrap_or(&empty);
        let lease = lease_for_spec(leases, &[o.spec.as_str()]);
        o.driver = Some(ctx.resolve(&[o.spec.as_str()], &f.tags, lease, false, f.modified_at));
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
    let live = crate::process_probe::probe_live_claude_sessions();
    for r in reqs.iter().filter(|r| is_in_progress(r)) {
        let ids: Vec<&str> = [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let lease = lease_for_spec(&leases, &ids);
        let owner_alive = lease
            .is_some_and(|l| matches!(crate::lease_state_for(l, &live, now), LeaseState::Live));
        let d = ctx.resolve(&ids, &r.tags, lease, owner_alive, parse_ts(&r.modified_at));
        out.insert(r.id, d);
    }
    out
}

/// In-progress, in any of the status spellings the cache carries.
pub(crate) fn is_in_progress(r: &aida_core::RequirementSummary) -> bool {
    let s: String = r
        .status
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
