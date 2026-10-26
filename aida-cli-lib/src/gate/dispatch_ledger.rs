//! Dispatch recovery ledger (STORY-1889, moved verbatim from `dispatch.rs`):
//! the `needs-recovery` fences in `.aida/dispatch/recovery.json`, the bounded
//! inventory that reads them, and the admission and cap decisions they force.
//! A fence is sticky; only an operator removing its entry releases it.
// mandate-gate
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use uuid::Uuid;

use crate::gate::dispatch_authority::{
    emit_dispatch_event, DispatchRequest, DispatchResult, DispatchStatus, DispatchVerb,
};
use crate::gate::dispatch_io::{
    read_bounded_regular_file, recovery_path, request_path, requests_dir, result_path, results_dir,
    sha256_hex, write_private_atomic, MAX_BRIEF_BYTES,
};
use crate::mandate_drive::Outcome;

pub(crate) const RECOVERY_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_RECOVERY_FILE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_DISPATCH_INVENTORY_ENTRIES: usize = 4_096;

/// One non-runnable (`needs-recovery`) record in `.aida/dispatch/recovery.json`
/// (TASK-2344, R4 + Q1).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryFenceEntry {
    pub id: String,
    pub reason: String,
    pub evidence_fingerprint: String,
    pub detected_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_verb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_target_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_dedupe_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_mandate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_requested_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub blocks_all_admission: bool,
}

/// Private versioned recovery index persisted at `.aida/dispatch/recovery.json`
/// (mode 0600, `#[serde(deny_unknown_fields)]`, bounded size/entry count).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryIndex {
    pub schema_version: u32,
    pub updated_at: DateTime<Utc>,
    pub entries: Vec<RecoveryFenceEntry>,
}

/// Shared bounded dispatch inventory combining valid `CompletePair` records and
/// `RecoveryRequired` fences (`fences` + optional `global_block_reason`).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug, Clone, Default)]
pub(crate) struct DispatchInventory {
    pub pairs: Vec<(DispatchRequest, DispatchResult)>,
    pub fences: Vec<RecoveryFenceEntry>,
    pub global_block_reason: Option<String>,
}

impl DispatchInventory {
    pub(crate) fn find_fence(&self, id_or_prefix: &str) -> Option<&RecoveryFenceEntry> {
        let needle = id_or_prefix.trim();
        if needle.is_empty() {
            return None;
        }
        self.fences
            .iter()
            .find(|f| f.id == needle || f.id.starts_with(needle))
    }

    /// Check whether a new request submission `(id, dedupe_key, target_key)` is
    /// blocked by a global inventory error or a `RecoveryRequired` fence.
    pub(crate) fn check_submission_fence(
        &self,
        id: &str,
        dedupe_key: &str,
        target_key: &str,
    ) -> Result<()> {
        if let Some(reason) = &self.global_block_reason {
            bail!(
                "dispatch inventory is in `needs-recovery` state and refuses new admissions: {reason}. {RECOVERY_GUIDANCE}"
            );
        }
        for fence in &self.fences {
            if fence.blocks_all_admission {
                bail!(
                    "dispatch record {} is in `needs-recovery` state with an unknown target/verb ({}), so no new dispatch request is admitted. {RECOVERY_GUIDANCE}",
                    fence.id,
                    fence.reason
                );
            }
            if fence.id == id
                || fence.known_dedupe_key.as_deref() == Some(dedupe_key)
                || fence.known_target_key.as_deref() == Some(target_key)
            {
                bail!(
                    "target `{target_key}` has a dispatch record {} in `needs-recovery` state ({}); automatic replay or replacement is refused. {RECOVERY_GUIDANCE}",
                    fence.id,
                    fence.reason
                );
            }
        }
        Ok(())
    }

    /// A recovery fence, other than `id`'s own, that reserves `target_key`.
    /// Fences reserve their target at execution as well as at submission, so
    /// a request queued before another record on its target lost evidence is
    /// not claimed while that record awaits recovery.
    // trace:STORY-1917 trace:TASK-2344 | ai:claude
    pub(crate) fn execution_target_fence(
        &self,
        id: &str,
        target_key: &str,
    ) -> Option<&RecoveryFenceEntry> {
        self.fences
            .iter()
            .find(|f| f.id != id && f.known_target_key.as_deref() == Some(target_key))
    }

    /// Check whether claiming/running new work is blocked globally by an unsafe
    /// or unknown-verb `RecoveryRequired` fence, and return `(review_slots,
    /// flow_slots, train_slots)` consumed by active `Running` records PLUS
    /// known-verb `RecoveryRequired` records (without double-counting).
    pub(crate) fn active_and_fenced_cap_usage(
        &self,
        exclude_id: Option<&str>,
    ) -> Result<(usize, usize, usize)> {
        if let Some(reason) = &self.global_block_reason {
            bail!(
                "dispatch inventory is in `needs-recovery` state and refuses new execution claims: {reason}. {RECOVERY_GUIDANCE}"
            );
        }
        let mut reviews = 0usize;
        let mut flows = 0usize;
        let mut trains = 0usize;
        for (req, res) in &self.pairs {
            if exclude_id == Some(req.id.as_str()) {
                continue;
            }
            if res.status == DispatchStatus::Running {
                match req.action {
                    DispatchVerb::Review { .. } => reviews += 1,
                    DispatchVerb::Flow { .. } => flows += 1,
                    DispatchVerb::Train { .. } => trains += 1,
                    DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
                }
            }
        }
        for fence in &self.fences {
            if exclude_id == Some(fence.id.as_str()) {
                continue;
            }
            if fence.blocks_all_admission {
                bail!(
                    "dispatch record {} is in `needs-recovery` state with an unknown verb/target ({}); refusing new execution claims. {RECOVERY_GUIDANCE}",
                    fence.id,
                    fence.reason
                );
            }
            match fence.known_verb.as_deref() {
                Some("review") => reviews += 1,
                Some("flow") => flows += 1,
                Some("train") => trains += 1,
                Some("take") | Some("impl") => {}
                _ => {
                    bail!(
                        "dispatch record {} is in `needs-recovery` state with unknown verb ({}); refusing new execution claims. {RECOVERY_GUIDANCE}",
                        fence.id,
                        fence.reason
                    );
                }
            }
        }
        Ok((reviews, flows, trains))
    }
}

pub(crate) enum HalfRead<T> {
    Missing,
    Valid(T),
    Corrupt(String),
}

pub(crate) fn read_json_half<T: for<'de> Deserialize<'de>>(
    path: &Path,
    label: &str,
) -> HalfRead<T> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return HalfRead::Corrupt(format!("{} is a symlink (refused)", path.display()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HalfRead::Missing,
        Err(e) => return HalfRead::Corrupt(format!("cannot stat {}: {e}", path.display())),
    }
    match read_bounded_regular_file(path, MAX_BRIEF_BYTES, true) {
        Ok(bytes) => match serde_json::from_slice::<T>(&bytes) {
            Ok(val) => HalfRead::Valid(val),
            Err(err) => HalfRead::Corrupt(format!(
                "malformed {label} JSON ({}): {err}",
                path.display()
            )),
        },
        Err(err) => HalfRead::Corrupt(format!("{err:#}")),
    }
}

fn read_existing_recovery_index(
    main_worktree: &Path,
) -> std::result::Result<Option<RecoveryIndex>, String> {
    let path = recovery_path(main_worktree);
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(format!("{} is a symlink (refused)", path.display()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot stat {}: {e}", path.display())),
    }
    let bytes = read_bounded_regular_file(&path, MAX_RECOVERY_FILE_BYTES, true)
        .map_err(|e| format!("invalid recovery index {}: {e:#}", path.display()))?;
    let idx: RecoveryIndex = serde_json::from_slice(&bytes)
        .map_err(|e| format!("malformed recovery index {}: {e}", path.display()))?;
    if idx.schema_version != RECOVERY_SCHEMA_VERSION {
        return Err(format!(
            "unsupported recovery index schema_version {} in {} (expected {})",
            idx.schema_version,
            path.display(),
            RECOVERY_SCHEMA_VERSION
        ));
    }
    if idx.entries.len() > MAX_DISPATCH_INVENTORY_ENTRIES {
        return Err(format!(
            "recovery index {} exceeds maximum entry count ({})",
            path.display(),
            MAX_DISPATCH_INVENTORY_ENTRIES
        ));
    }
    Ok(Some(idx))
}

fn scan_uuid_dir_entries(
    dir_path: &Path,
    stems: &mut BTreeSet<String>,
    anomalies: &mut Vec<RecoveryFenceEntry>,
    now: DateTime<Utc>,
) -> std::result::Result<(), String> {
    match std::fs::symlink_metadata(dir_path) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(format!(
                "{} is a symlink or not a directory (refused)",
                dir_path.display()
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot stat {}: {e}", dir_path.display())),
    }
    let entries = std::fs::read_dir(dir_path)
        .map_err(|e| format!("cannot read directory {}: {e}", dir_path.display()))?;
    let mut count = 0usize;
    for entry in entries {
        count += 1;
        if count > MAX_DISPATCH_INVENTORY_ENTRIES {
            return Err(format!(
                "directory {} exceeds maximum entry limit ({MAX_DISPATCH_INVENTORY_ENTRIES})",
                dir_path.display()
            ));
        }
        let entry =
            entry.map_err(|e| format!("unreadable entry in {}: {e}", dir_path.display()))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(name_str) = file_name.to_str() else {
            let reason = format!("non-UTF-8 entry in {}", dir_path.display());
            anomalies.push(RecoveryFenceEntry {
                id: format!(
                    "invalid-entry:{}",
                    sha256_hex(file_name.as_encoded_bytes())[..12].to_string()
                ),
                evidence_fingerprint: sha256_hex(reason.as_bytes()),
                reason,
                detected_at: now,
                known_verb: None,
                known_target_key: None,
                known_dedupe_key: None,
                known_mandate_id: None,
                known_requested_at: None,
                blocks_all_admission: true,
            });
            continue;
        };
        if name_str.starts_with('.') {
            // Temporary atomic write files (`..<name>.tmp-<uuid>`) are ignored
            // unless someone planted a symlink or subdirectory.
            if let Ok(m) = std::fs::symlink_metadata(&path) {
                if m.file_type().is_symlink() || m.is_dir() {
                    let reason = format!(
                        "unexpected hidden symlink or directory `{name_str}` in {}",
                        dir_path.display()
                    );
                    anomalies.push(RecoveryFenceEntry {
                        id: format!("invalid-entry:{name_str}"),
                        evidence_fingerprint: sha256_hex(reason.as_bytes()),
                        reason,
                        detected_at: now,
                        known_verb: None,
                        known_target_key: None,
                        known_dedupe_key: None,
                        known_mandate_id: None,
                        known_requested_at: None,
                        blocks_all_admission: true,
                    });
                }
            }
            continue;
        }
        let stem_opt = path.file_stem().and_then(|s| s.to_str());
        let ext_opt = path.extension().and_then(|s| s.to_str());
        if let (Some(stem), Some("json")) = (stem_opt, ext_opt) {
            if Uuid::parse_str(stem).is_ok() {
                stems.insert(stem.to_string());
                continue;
            }
        }
        let reason = format!(
            "unexpected non-UUID or non-JSON entry `{name_str}` in {}",
            dir_path.display()
        );
        anomalies.push(RecoveryFenceEntry {
            id: format!("invalid-entry:{name_str}"),
            evidence_fingerprint: sha256_hex(reason.as_bytes()),
            reason,
            detected_at: now,
            known_verb: None,
            known_target_key: None,
            known_dedupe_key: None,
            known_mandate_id: None,
            known_requested_at: None,
            blocks_all_admission: true,
        });
    }
    Ok(())
}

/// Inspect `.aida/dispatch/` without mutating disk or printing stderr warnings
/// (used by read-only `list`, `show`, `load_all_pairs`, and `pending_request_count`).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn inspect_dispatch_inventory_readonly(
    main_worktree: &Path,
) -> Result<DispatchInventory> {
    let now = Utc::now();
    let mut inv = DispatchInventory::default();

    let existing_recovery = match read_existing_recovery_index(main_worktree) {
        Ok(idx) => idx,
        Err(err) => {
            inv.global_block_reason = Some(err.clone());
            inv.fences.push(RecoveryFenceEntry {
                id: "recovery-index".to_string(),
                evidence_fingerprint: sha256_hex(err.as_bytes()),
                reason: err,
                detected_at: now,
                known_verb: None,
                known_target_key: None,
                known_dedupe_key: None,
                known_mandate_id: None,
                known_requested_at: None,
                blocks_all_admission: true,
            });
            None
        }
    };
    let mut prior_by_id: std::collections::BTreeMap<String, RecoveryFenceEntry> =
        std::collections::BTreeMap::new();
    if let Some(idx) = existing_recovery {
        for entry in idx.entries {
            prior_by_id.insert(entry.id.clone(), entry);
        }
    }

    let mut stems = BTreeSet::new();
    for prior_id in prior_by_id.keys() {
        if Uuid::parse_str(prior_id).is_ok() {
            stems.insert(prior_id.clone());
        }
    }
    let mut dir_anomalies = Vec::new();
    if let Err(err) = scan_uuid_dir_entries(
        &requests_dir(main_worktree),
        &mut stems,
        &mut dir_anomalies,
        now,
    ) {
        inv.global_block_reason = Some(err.clone());
        return Ok(inv);
    }
    if let Err(err) = scan_uuid_dir_entries(
        &results_dir(main_worktree),
        &mut stems,
        &mut dir_anomalies,
        now,
    ) {
        inv.global_block_reason = Some(err.clone());
        return Ok(inv);
    }
    for mut anomaly in dir_anomalies {
        if let Some(prev) = prior_by_id.get(&anomaly.id) {
            if prev.evidence_fingerprint == anomaly.evidence_fingerprint {
                anomaly.detected_at = prev.detected_at;
            }
        }
        inv.fences.push(anomaly);
    }

    for id in stems {
        let req_half: HalfRead<DispatchRequest> =
            read_json_half(&request_path(main_worktree, &id), "request");
        let res_half: HalfRead<DispatchResult> =
            read_json_half(&result_path(main_worktree, &id), "result");

        let fence_opt: Option<RecoveryFenceEntry> = match (req_half, res_half) {
            (HalfRead::Valid(req), HalfRead::Valid(res)) => {
                if req.id != id || res.id != id {
                    let reason = format!(
                        "mismatched inner id on dispatch pair {id} (request.id={}, result.id={})",
                        req.id, res.id
                    );
                    Some(RecoveryFenceEntry {
                        id: id.clone(),
                        evidence_fingerprint: sha256_hex(reason.as_bytes()),
                        reason,
                        detected_at: now,
                        known_verb: Some(req.action.verb_str().to_string()),
                        known_target_key: Some(req.target_key),
                        known_dedupe_key: Some(req.dedupe_key),
                        known_mandate_id: Some(req.mandate_id),
                        known_requested_at: Some(req.requested_at),
                        blocks_all_admission: true,
                    })
                } else if req.action.verb_str() != res.verb
                    || req.target_key != res.target
                    || req.dedupe_key != res.dedupe_key
                {
                    let reason = format!(
                        "mismatched request/result metadata for {id} (req={}/{}/{}, res={}/{}/{})",
                        req.action.verb_str(),
                        req.target_key,
                        req.dedupe_key,
                        res.verb,
                        res.target,
                        res.dedupe_key
                    );
                    Some(RecoveryFenceEntry {
                        id: id.clone(),
                        evidence_fingerprint: sha256_hex(reason.as_bytes()),
                        reason,
                        detected_at: now,
                        known_verb: None,
                        known_target_key: None,
                        known_dedupe_key: None,
                        known_mandate_id: Some(req.mandate_id),
                        known_requested_at: Some(req.requested_at),
                        blocks_all_admission: true,
                    })
                } else if let Some(prev) = prior_by_id.get(&id) {
                    // Once a record has been fenced in `recovery.json`, it stays
                    // fenced until explicit operator recovery removes the fence.
                    Some(prev.clone())
                } else {
                    inv.pairs.push((req, res));
                    None
                }
            }
            (HalfRead::Valid(req), HalfRead::Missing) => {
                let valid_id = req.id == id
                    && req.target_key == req.action.target_key()
                    && req.dedupe_key == req.action.dedupe_key();
                let reason = format!(
                    "result file results/{id}.json is missing for request {id} ({} {}); automatic replay is refused (needs-recovery)",
                    req.action.verb_str(),
                    req.target_key
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: valid_id.then(|| req.action.verb_str().to_string()),
                    known_target_key: valid_id.then(|| req.target_key),
                    known_dedupe_key: valid_id.then(|| req.dedupe_key),
                    known_mandate_id: Some(req.mandate_id),
                    known_requested_at: Some(req.requested_at),
                    blocks_all_admission: !valid_id,
                })
            }
            (HalfRead::Missing, HalfRead::Valid(res)) => {
                let valid_verb = matches!(
                    res.verb.as_str(),
                    "flow" | "review" | "take" | "impl" | "train"
                );
                let valid_meta = res.id == id
                    && valid_verb
                    && !res.target.is_empty()
                    && !res.dedupe_key.is_empty();
                let reason = format!(
                    "request file requests/{id}.json is missing for result {id} (status: {}, target: {}); automatic cleanup or rewrite is refused (needs-recovery)",
                    res.status.as_str(),
                    res.target
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: valid_meta.then(|| res.verb),
                    known_target_key: valid_meta.then(|| res.target),
                    known_dedupe_key: valid_meta.then(|| res.dedupe_key),
                    known_mandate_id: Some(res.mandate_id),
                    known_requested_at: Some(res.requested_at),
                    blocks_all_admission: !valid_meta,
                })
            }
            (HalfRead::Valid(req), HalfRead::Corrupt(res_err)) => {
                let valid_id = req.id == id
                    && req.target_key == req.action.target_key()
                    && req.dedupe_key == req.action.dedupe_key();
                let reason = format!(
                    "corrupt result file for request {id} ({} {}): {res_err} (needs-recovery)",
                    req.action.verb_str(),
                    req.target_key
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: valid_id.then(|| req.action.verb_str().to_string()),
                    known_target_key: valid_id.then(|| req.target_key),
                    known_dedupe_key: valid_id.then(|| req.dedupe_key),
                    known_mandate_id: Some(req.mandate_id),
                    known_requested_at: Some(req.requested_at),
                    blocks_all_admission: !valid_id,
                })
            }
            (HalfRead::Corrupt(req_err), HalfRead::Valid(res)) => {
                // Corrupt request means we cannot verify the canonical request
                // definition; block new admissions while still reporting the
                // result's verb/target for diagnostics.
                let reason = format!(
                    "corrupt request file for result {id} (status: {}): {req_err} (needs-recovery)",
                    res.status.as_str()
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: Some(res.verb),
                    known_target_key: Some(res.target),
                    known_dedupe_key: Some(res.dedupe_key),
                    known_mandate_id: Some(res.mandate_id),
                    known_requested_at: Some(res.requested_at),
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Corrupt(req_err), HalfRead::Missing) => {
                let reason =
                    format!("corrupt orphan request file {id}: {req_err} (needs-recovery)");
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: None,
                    known_target_key: None,
                    known_dedupe_key: None,
                    known_mandate_id: None,
                    known_requested_at: None,
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Missing, HalfRead::Corrupt(res_err)) => {
                let reason = format!("corrupt orphan result file {id}: {res_err} (needs-recovery)");
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: None,
                    known_target_key: None,
                    known_dedupe_key: None,
                    known_mandate_id: None,
                    known_requested_at: None,
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Corrupt(req_err), HalfRead::Corrupt(res_err)) => {
                let reason = format!(
                    "corrupt request and result files for {id}: {req_err}; {res_err} (needs-recovery)"
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: None,
                    known_target_key: None,
                    known_dedupe_key: None,
                    known_mandate_id: None,
                    known_requested_at: None,
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Missing, HalfRead::Missing) => prior_by_id.get(&id).cloned(),
        };

        if let Some(mut fence) = fence_opt {
            if let Some(prev) = prior_by_id.get(&id) {
                if prev.evidence_fingerprint == fence.evidence_fingerprint {
                    fence.detected_at = prev.detected_at;
                }
                if fence.known_verb.is_none() {
                    fence.known_verb = prev.known_verb.clone();
                }
                if fence.known_target_key.is_none() {
                    fence.known_target_key = prev.known_target_key.clone();
                }
                if fence.known_dedupe_key.is_none() {
                    fence.known_dedupe_key = prev.known_dedupe_key.clone();
                }
                if fence.known_mandate_id.is_none() {
                    fence.known_mandate_id = prev.known_mandate_id.clone();
                }
                if fence.known_requested_at.is_none() {
                    fence.known_requested_at = prev.known_requested_at;
                }
                fence.blocks_all_admission |= prev.blocks_all_admission;
            }
            inv.fences.push(fence);
        }
    }

    inv.pairs
        .sort_by(|(a, _), (b, _)| a.requested_at.cmp(&b.requested_at).then(a.id.cmp(&b.id)));
    inv.fences.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(inv)
}

/// Reconcile `.aida/dispatch/` under `tick.lock`: persists `.aida/dispatch/recovery.json`
/// (mode 0600) and emits one audit event per newly detected `(id, evidence_fingerprint)`.
/// Never deletes or rewrites broken request/result files and never replays orphans.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn reconcile_inventory_locked(
    main_worktree: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchInventory> {
    let prior = read_existing_recovery_index(main_worktree).ok().flatten();
    let mut prior_fingerprints = BTreeSet::new();
    if let Some(idx) = &prior {
        for e in &idx.entries {
            prior_fingerprints.insert((e.id.clone(), e.evidence_fingerprint.clone()));
        }
    }

    let inv = inspect_dispatch_inventory_readonly(main_worktree)?;
    if inv.global_block_reason.is_none() {
        for fence in &inv.fences {
            let key = (fence.id.clone(), fence.evidence_fingerprint.clone());
            if !prior_fingerprints.contains(&key) {
                emit_dispatch_event(
                    main_worktree,
                    fence.known_mandate_id.as_deref().unwrap_or(""),
                    fence.known_verb.as_deref().unwrap_or("recovery"),
                    fence.known_target_key.as_deref().unwrap_or(&fence.id),
                    Outcome::Refused,
                    Some(fence.reason.clone()),
                );
            }
        }
        let rec_path = recovery_path(main_worktree);
        if inv.fences.is_empty() {
            if rec_path.exists() {
                let _ = std::fs::remove_file(&rec_path);
            }
        } else {
            let idx = RecoveryIndex {
                schema_version: RECOVERY_SCHEMA_VERSION,
                updated_at: now,
                entries: inv.fences.clone(),
            };
            let body = serde_json::to_vec_pretty(&idx)?;
            // The writer holds itself to the reader's bounds: an index the
            // reader would reject is never written, so the last readable
            // index (and every fence in it) stays in place and the refusal
            // names the overflow. trace:STORY-1917 trace:TASK-2344 | ai:claude
            check_recovery_index_bounds(idx.entries.len(), body.len())?;
            write_private_atomic(&rec_path, &body)?;
        }
    }
    Ok(inv)
}

/// The operator procedure for a `needs-recovery` record. Fences are sticky:
/// removing a record's request or result file does not release one; only
/// deleting that record's entry from `recovery.json` does.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) const RECOVERY_GUIDANCE: &str = "To recover: find out what the record really did (`aida dispatch show <id>`, its log in .aida/dispatch/logs/<id>.log, and the PR or lane it targeted); stop every `aida dispatch serve`, `tick` and `run` process; move that record's files out of .aida/dispatch/requests/ and .aida/dispatch/results/; then delete only that record's entry from .aida/dispatch/recovery.json. Deleting the whole file releases every fence at once";

/// Refuse a recovery index the reader ([`read_existing_recovery_index`])
/// would reject for its entry count or size.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn check_recovery_index_bounds(entries: usize, bytes: usize) -> Result<()> {
    if entries > MAX_DISPATCH_INVENTORY_ENTRIES || bytes > MAX_RECOVERY_FILE_BYTES {
        bail!(
            "dispatch needs recovery for {entries} record(s), more than the recovery index can hold ({MAX_DISPATCH_INVENTORY_ENTRIES} entries, {MAX_RECOVERY_FILE_BYTES} bytes); dispatch refuses new work until records are recovered. {RECOVERY_GUIDANCE}"
        );
    }
    Ok(())
}
