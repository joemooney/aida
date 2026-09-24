//! Id-resolution collisions: one identifier answering to more than one
//! requirement.
//!
//! A requirement is addressable by its native `spec_id` (the YAML's file name)
//! and by its merge-gate `agreed_id`. When the merge-gate hands out an
//! `agreed_id` that another object already holds as its native `spec_id`, both
//! objects answer to the same id: `aida show`, a `(SPEC-ID)` commit trailer and
//! a `trace:` comment can each land on a different requirement.
//!
//! This module is the pure, store-shaped half of the fix: it enumerates every
//! id that resolves to more than one object (for `aida doctor`), and it orders
//! the candidates for one id deterministically (for the ambiguity warning on
//! the resolution paths). The prevention half lives in
//! `git_ops::merge_gate`.
// trace:BUG-1535 | ai:claude

use std::collections::BTreeMap;

use uuid::Uuid;

use crate::models::Requirement;

/// One object an id resolves to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IdCandidate {
    pub uuid: Uuid,
    pub spec_id: Option<String>,
    pub agreed_id: Option<String>,
    pub title: String,
    pub status: String,
    /// True when the id is this object's NATIVE `spec_id` (its file name),
    /// false when the id only matches its `agreed_id`.
    pub native: bool,
}

impl IdCandidate {
    /// The handle that reaches THIS object and nothing else, given that
    /// `colliding_id` is ambiguous: an agreed-id holder is reached by its
    /// native `spec_id` (its file name); the native owner of the colliding id
    /// — or an object with no spec_id — only by its UUID.
    pub fn unambiguous_handle(&self, colliding_id: &str) -> String {
        match non_empty(&self.spec_id) {
            Some(s) if !s.eq_ignore_ascii_case(colliding_id.trim()) => s.to_string(),
            _ => self.uuid.to_string(),
        }
    }
}

/// Refusal raised when an id resolves to more than one requirement. Every
/// write path (and `aida show`) refuses on it rather than acting on a guess;
/// the message names each candidate's unambiguous handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousIdError {
    pub id: String,
    pub candidates: Vec<IdCandidate>,
}

impl std::fmt::Display for AmbiguousIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The first line must stand alone: agent-mode error rendering keeps
        // only it, so it carries every candidate's handle.
        let handles: Vec<String> = self
            .candidates
            .iter()
            .map(|c| c.unambiguous_handle(&self.id))
            .collect();
        writeln!(
            f,
            "`{}` is ambiguous: it resolves to {} requirements ({}). Refusing to pick one; \
             re-run with one of those handles.",
            self.id.to_ascii_uppercase(),
            self.candidates.len(),
            handles.join(", ")
        )?;
        for line in describe_candidates(&self.id, &self.candidates) {
            writeln!(f, "  - {line}")?;
        }
        write!(
            f,
            "Each line leads with the handle that reaches that requirement alone. \
             `aida doctor --category id-collisions` lists every ambiguous id."
        )
    }
}

impl std::error::Error for AmbiguousIdError {}

/// Classify a lookup: `Ok(None)` = unknown, `Ok(Some(uuid))` = exactly one
/// object, `Err` = ambiguous.
pub fn resolve_candidates(
    id: &str,
    candidates: Vec<IdCandidate>,
) -> Result<Option<Uuid>, AmbiguousIdError> {
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(Some(candidates[0].uuid)),
        _ => Err(AmbiguousIdError {
            id: id.trim().to_string(),
            candidates,
        }),
    }
}

/// UUIDs whose DISPLAYED id (agreed_id when set, else spec_id) is ambiguous
/// and should instead be displayed by their native spec_id: every candidate
/// that matches a colliding id only through its agreed_id.
pub fn relabel_to_native(collisions: &[IdCollision]) -> std::collections::HashSet<Uuid> {
    collisions
        .iter()
        .flat_map(|c| c.candidates.iter())
        .filter(|c| !c.native && c.spec_id.as_deref().is_some_and(|s| !s.trim().is_empty()))
        .map(|c| c.uuid)
        .collect()
}

/// An id that resolves to two or more distinct objects.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IdCollision {
    /// The colliding id, upper-cased.
    pub id: String,
    /// Every object the id resolves to, in resolution order (see
    /// [`order_candidates`]).
    pub candidates: Vec<IdCandidate>,
}

/// The minimal per-object projection the collision scan needs. Implemented for
/// a full [`Requirement`] and constructible from a cache row.
#[derive(Debug, Clone)]
pub struct IdRow {
    pub uuid: Uuid,
    pub spec_id: Option<String>,
    pub agreed_id: Option<String>,
    pub title: String,
    pub status: String,
}

impl From<&Requirement> for IdRow {
    fn from(r: &Requirement) -> Self {
        IdRow {
            uuid: r.id,
            spec_id: r.spec_id.clone(),
            agreed_id: r.agreed_id.clone(),
            title: r.title.clone(),
            status: r.effective_status(),
        }
    }
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn candidate(row: &IdRow, id: &str) -> Option<IdCandidate> {
    let native = non_empty(&row.spec_id).is_some_and(|s| s.eq_ignore_ascii_case(id));
    let agreed = non_empty(&row.agreed_id).is_some_and(|s| s.eq_ignore_ascii_case(id));
    if !native && !agreed {
        return None;
    }
    Some(IdCandidate {
        uuid: row.uuid,
        spec_id: row.spec_id.clone(),
        agreed_id: row.agreed_id.clone(),
        title: row.title.clone(),
        status: row.status.clone(),
        native,
    })
}

/// Deterministic resolution order: the object whose NATIVE `spec_id` is the id
/// comes first (it is the file the id names on disk, and the one the git
/// backend's `get_requirement_by_spec_id` returns), then agreed-id matches;
/// ties break on UUID so the order never depends on directory iteration.
pub fn order_candidates(candidates: &mut [IdCandidate]) {
    candidates.sort_by(|a, b| b.native.cmp(&a.native).then(a.uuid.cmp(&b.uuid)));
}

/// Every object `id` resolves to (case-insensitive, native `spec_id` or
/// `agreed_id`), in [`order_candidates`] order. Two or more entries means the
/// id is ambiguous.
pub fn candidates_for_id<'a, I>(rows: I, id: &str) -> Vec<IdCandidate>
where
    I: IntoIterator<Item = &'a IdRow>,
{
    let id = id.trim();
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<IdCandidate> = rows
        .into_iter()
        .filter_map(|row| candidate(row, id))
        .filter(|c| seen.insert(c.uuid))
        .collect();
    order_candidates(&mut out);
    out
}

/// Every id (native `spec_id` or `agreed_id`, compared case-insensitively)
/// that resolves to more than one DISTINCT object, sorted by id. An object
/// whose `spec_id` equals its own `agreed_id` counts once.
pub fn find_id_collisions<'a, I>(rows: I) -> Vec<IdCollision>
where
    I: IntoIterator<Item = &'a IdRow>,
{
    let rows: Vec<&IdRow> = rows.into_iter().collect();
    let mut by_id: BTreeMap<String, Vec<&IdRow>> = BTreeMap::new();
    for row in &rows {
        let mut keys: Vec<String> = Vec::new();
        for id in [non_empty(&row.spec_id), non_empty(&row.agreed_id)]
            .into_iter()
            .flatten()
        {
            let key = id.to_ascii_uppercase();
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        for key in keys {
            by_id.entry(key).or_default().push(row);
        }
    }
    by_id
        .into_iter()
        .filter_map(|(id, owners)| {
            let candidates = candidates_for_id(owners.iter().copied(), &id);
            (candidates.len() > 1).then_some(IdCollision { id, candidates })
        })
        .collect()
}

/// Convenience: [`find_id_collisions`] over full requirements.
pub fn find_requirement_id_collisions<'a, I>(reqs: I) -> Vec<IdCollision>
where
    I: IntoIterator<Item = &'a Requirement>,
{
    let rows: Vec<IdRow> = reqs.into_iter().map(IdRow::from).collect();
    find_id_collisions(rows.iter())
}

/// A human-readable one-line-per-candidate rendering, used by the ambiguity
/// refusal and the doctor finding. Each line leads with the candidate's
/// unambiguous handle.
pub fn describe_candidates(colliding_id: &str, candidates: &[IdCandidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|c| {
            let via = if c.native {
                format!(
                    "native id {}",
                    c.spec_id.as_deref().unwrap_or("?").to_ascii_uppercase()
                )
            } else {
                format!(
                    "agreed id {}",
                    c.agreed_id.as_deref().unwrap_or("?").to_ascii_uppercase()
                )
            };
            format!(
                "{} ({}) \u{2014} {} [{}]",
                c.unambiguous_handle(colliding_id),
                via,
                c.title,
                c.status
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(spec: &str, agreed: Option<&str>, title: &str) -> IdRow {
        IdRow {
            uuid: Uuid::new_v4(),
            spec_id: Some(spec.to_string()),
            agreed_id: agreed.map(str::to_string),
            title: title.to_string(),
            status: "Draft".to_string(),
        }
    }

    // The BUG-34 shape: a native fixture holds BUG-34 as its spec_id, and the
    // merge-gate later assigned BUG-34 as another object's agreed_id.
    #[test]
    fn native_vs_agreed_collision_is_detected_with_both_objects() {
        let fixture = row("BUG-34", Some("BUG-34"), "[test] fixture");
        let real = row("BUG-2-081", Some("BUG-34"), "real spec");
        let clean = row("BUG-2-082", Some("BUG-35"), "unrelated");
        let rows = vec![real.clone(), fixture.clone(), clean];
        let found = find_id_collisions(rows.iter());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "BUG-34");
        assert_eq!(found[0].candidates.len(), 2);
        // Native owner first, regardless of input order.
        assert_eq!(found[0].candidates[0].uuid, fixture.uuid);
        assert!(found[0].candidates[0].native);
        assert_eq!(found[0].candidates[1].uuid, real.uuid);
        assert!(!found[0].candidates[1].native);
        assert_eq!(
            found[0].candidates[1].unambiguous_handle("BUG-34"),
            "BUG-2-081"
        );
        assert_eq!(
            found[0].candidates[0].unambiguous_handle("bug-34"),
            fixture.uuid.to_string()
        );
        // Only the agreed-id holder is relabelled for display.
        let relabel = relabel_to_native(&found);
        assert!(relabel.contains(&real.uuid) && !relabel.contains(&fixture.uuid));
        // The refusal names both handles.
        let err = resolve_candidates("bug-34", found[0].candidates.clone()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("BUG-2-081") && msg.contains(&fixture.uuid.to_string()),
            "{msg}"
        );
    }

    #[test]
    fn self_matching_spec_and_agreed_id_is_not_a_collision() {
        let rows = vec![row("TASK-7", Some("TASK-7"), "a"), row("TASK-8", None, "b")];
        assert!(find_id_collisions(rows.iter()).is_empty());
    }

    #[test]
    fn collision_match_is_case_insensitive() {
        let rows = vec![
            row("story-41", None, "a"),
            row("STORY-1-003", Some("STORY-41"), "b"),
        ];
        let found = find_id_collisions(rows.iter());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "STORY-41");
    }

    #[test]
    fn candidate_order_is_deterministic_across_input_order() {
        let a = row("TASK-1-001", Some("TASK-31"), "a");
        let b = row("TASK-2-001", Some("TASK-31"), "b");
        let native = row("TASK-31", None, "native");
        let fwd = vec![a.clone(), b.clone(), native.clone()];
        let rev = vec![native.clone(), b.clone(), a.clone()];
        let x = candidates_for_id(fwd.iter(), "task-31");
        let y = candidates_for_id(rev.iter(), "TASK-31");
        assert_eq!(x, y);
        assert_eq!(x.len(), 3);
        assert_eq!(x[0].uuid, native.uuid);
    }

    #[test]
    fn unique_id_has_one_candidate() {
        let rows = vec![row("FR-1", None, "a"), row("FR-2", None, "b")];
        assert_eq!(candidates_for_id(rows.iter(), "FR-1").len(), 1);
        assert!(candidates_for_id(rows.iter(), "FR-3").is_empty());
    }
}
