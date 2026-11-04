//! STORY-2324 PR-B: `aida gate migrate`, the one-time gate-ledger rewrite
//! that gives every existing event the canonical `spec_key` its `spec`
//! label resolves to in the committed store snapshot.
//!
//! What it does, and what it never does (advisor amendment A1):
//!
//! - Every ledger file keeps its name, its lines and their order exactly:
//!   per-file `seq` order is unchanged because no line moves. A line whose
//!   `spec` resolves gains a `"spec_key":"<uuid>",` prefix inside its
//!   opening brace and is otherwise byte-for-byte the v1 line. Every other
//!   line (no spec, already keyed, unresolvable, ambiguous, unparseable) is
//!   copied verbatim. Before the swap, every v1 line's SHA-256 is checked
//!   against its v2 line with the stamp removed.
//! - It never writes a verdict, a hold or any other event, so it cannot
//!   synthesize an approval. Record bodies are not touched.
//! - The layout today's readers use (`PR-<N>.jsonl`, `by-spec/<LABEL>.json`
//!   indexes and `by-spec/<LABEL>.jsonl` unbound entries) stays as it is:
//!   readers decide by label until PR-C, so a v2 tree must read exactly as
//!   v1 did. The legacy `.aida/review-verdicts/`, `.aida/merge-holds/`,
//!   `.aida/merge-hold-clearances/` and `.aida/pr-holds/` files are still
//!   read through today, so they are inventoried and reported here and left
//!   in place; importing and removing them belongs with PR-C, which deletes
//!   the read-through in the same change.
//! - `--execute` takes the ledger-wide lock exclusively (every writer holds
//!   it shared), then plans, builds `gate-ledger.v2/`, verifies it, writes
//!   its report as `pending`, moves v1 aside to `gate-ledger.v1-<stamp>/`,
//!   moves v2 into place and marks the report `done`. A failed swap puts
//!   v1 back, setting aside anything found at the ledger's path.
//!   `--rollback` takes the stamps off in place on the live tree (stamping
//!   is reversible line by line), so events written since the migration
//!   are kept.
//!
//! The planning and verification halves are pure over bytes and a resolver,
//! so the tests drive them without a store or a forge.
// trace:STORY-2324 | ai:claude
// mandate-gate

use crate::gate::canon::{Resolution, SpecKey};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// What a stamped line starts with; the v1 line's opening brace follows the
/// key's value and comma.
const STAMP_PREFIX: &str = "{\"spec_key\":\"";
/// Where reports of each run are kept, under `.aida/`.
pub(crate) const REPORTS_DIR: &str = "gate-ledger-migrations";
const V2_DIR: &str = "gate-ledger.v2";
const ARCHIVE_PREFIX: &str = "gate-ledger.v1-";

// The legacy stores PR-B reports but leaves in place, under `.aida/`. They
// are named only in the sanctioned legacy reader.
use super::legacy::LEGACY_STORE_DIRS as LEGACY_DIRS;

/// What one ledger line became.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LineFate {
    /// Blank, or an event with no spec label: copied as is.
    Verbatim,
    Stamped(SpecKey),
    /// Already carries a `spec_key` (a PR-B writer, or an earlier run).
    AlreadyKeyed,
    /// Not a JSON object (a torn write): copied as is.
    Unparsed,
    /// The label names no requirement: copied as is, reported.
    Unresolved(String),
    /// The label names several requirements: copied as is, reported.
    Ambiguous(String, String),
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// The key and the v1 line inside a stamped v2 line, or `None` when `v2`
/// carries no migration stamp.
// trace:STORY-2324 | ai:claude
pub(crate) fn unstamp(v2: &str) -> Option<(SpecKey, String)> {
    let rest = v2.strip_prefix(STAMP_PREFIX)?;
    let uuid = rest.get(..36)?;
    let key = SpecKey(uuid::Uuid::parse_str(uuid).ok()?);
    let tail = rest.get(36..)?.strip_prefix("\",")?;
    Some((key, format!("{{{tail}")))
}

/// One ledger line (no trailing newline) and what it becomes. Pure.
// trace:STORY-2324 | ai:claude
pub(crate) fn stamp_line(line: &str, resolve: &dyn Fn(&str) -> Resolution) -> (String, LineFate) {
    let keep = |fate| (line.to_string(), fate);
    if line.trim().is_empty() {
        return keep(LineFate::Verbatim);
    }
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(line) else {
        return keep(LineFate::Unparsed);
    };
    if obj.contains_key("spec_key") {
        return keep(LineFate::AlreadyKeyed);
    }
    let Some(label) = obj
        .get("spec")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return keep(LineFate::Verbatim);
    };
    let label = label.to_ascii_uppercase();
    match resolve(&label) {
        Resolution::Hit(key) => {
            if !line.starts_with('{') {
                return keep(LineFate::Unparsed);
            }
            let out = format!("{STAMP_PREFIX}{key}\",{}", &line[1..]);
            // Self-check: the stamp comes off to the exact v1 line, and the
            // stamped line is still one JSON object carrying the key.
            let round_trips = unstamp(&out).is_some_and(|(k, v1)| k == key && v1 == line);
            let parses = matches!(
                serde_json::from_str::<serde_json::Value>(&out),
                Ok(serde_json::Value::Object(o)) if o.get("spec_key").and_then(|v| v.as_str()) == Some(key.to_string().as_str())
            );
            if round_trips && parses {
                (out, LineFate::Stamped(key))
            } else {
                keep(LineFate::Unparsed)
            }
        }
        Resolution::Unknown => keep(LineFate::Unresolved(label)),
        Resolution::Ambiguous(why) => keep(LineFate::Ambiguous(label, why)),
    }
}

/// A whole ledger file and what it becomes, line for line. Pure.
// trace:STORY-2324 | ai:claude
pub(crate) fn stamp_file(
    body: &str,
    resolve: &dyn Fn(&str) -> Resolution,
) -> (String, Vec<LineFate>) {
    let mut out = String::with_capacity(body.len() + body.len() / 8);
    let mut fates = Vec::new();
    for raw in body.split_inclusive('\n') {
        let (line, nl) = match raw.strip_suffix('\n') {
            Some(l) => (l, "\n"),
            None => (raw, ""),
        };
        let (new, fate) = stamp_line(line, resolve);
        out.push_str(&new);
        out.push_str(nl);
        fates.push(fate);
    }
    (out, fates)
}

/// A1's check on one file: v2 has v1's lines, in v1's order, and each v2
/// line is the v1 line or the v1 line with a stamp whose removal gives a
/// line with the same SHA-256. Pure.
// trace:STORY-2324 | ai:claude
pub(crate) fn verify_file(v1: &[u8], v2: &[u8]) -> Result<(), String> {
    let a: Vec<&[u8]> = v1.split_inclusive(|b| *b == b'\n').collect();
    let b: Vec<&[u8]> = v2.split_inclusive(|b| *b == b'\n').collect();
    if a.len() != b.len() {
        return Err(format!("{} line(s) became {}", a.len(), b.len()));
    }
    for (i, (old, new)) in a.iter().zip(&b).enumerate() {
        if old == new {
            continue;
        }
        let (old_body, old_nl) = split_nl(old);
        let (new_body, new_nl) = split_nl(new);
        let restored = std::str::from_utf8(new_body)
            .ok()
            .and_then(unstamp)
            .map(|(_, v1)| v1);
        let same = old_nl == new_nl
            && restored.is_some_and(|r| sha256_hex(r.as_bytes()) == sha256_hex(old_body));
        if !same {
            return Err(format!("line {} is not its v1 line plus a spec_key", i + 1));
        }
    }
    Ok(())
}

fn split_nl(line: &[u8]) -> (&[u8], bool) {
    match line.strip_suffix(b"\n") {
        Some(body) => (body, true),
        None => (line, false),
    }
}

/// One file of the v1 tree and its v2 bytes.
#[derive(Debug, Clone)]
pub(crate) struct FilePlan {
    /// Relative to the ledger directory.
    pub rel: PathBuf,
    pub v1: Vec<u8>,
    pub v2: Vec<u8>,
    pub v1_sha256: String,
    pub mode: Option<u32>,
}

/// Every place an unresolvable label sits.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct LabelSites {
    /// Ledger files (relative), sorted.
    pub files: BTreeSet<String>,
    /// Why, for an ambiguous label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One legacy store directory, inventoried.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct LegacyDir {
    pub dir: String,
    pub files: usize,
    /// Top-level file stems that name a change (`PR-<N>`).
    pub change_keyed: usize,
    /// Top-level file stems that resolve to one requirement.
    pub resolved: usize,
    /// Top-level file stems that resolve to none or several, sorted.
    pub unresolved: Vec<String>,
}

/// What a run would do (dry run) or did (execute).
#[derive(Debug, Clone, Default)]
pub(crate) struct Plan {
    pub files: Vec<FilePlan>,
    pub lines: usize,
    pub stamped: usize,
    pub already_keyed: usize,
    pub unparsed: usize,
    pub unresolved: BTreeMap<String, LabelSites>,
    pub ambiguous: BTreeMap<String, LabelSites>,
    pub legacy: Vec<LegacyDir>,
}

impl Plan {
    /// Changes (`PR-<N>`) an unresolvable or ambiguous label sits on.
    pub(crate) fn changes_with_unresolved(&self) -> BTreeMap<u64, BTreeSet<String>> {
        let mut out: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
        for (label, sites) in self.unresolved.iter().chain(&self.ambiguous) {
            for file in &sites.files {
                let stem = file.strip_suffix(".jsonl").unwrap_or(file);
                if let Some(change) = super::ChangeKey::parse(stem) {
                    if change.forge == super::ChangeForge::Pr {
                        out.entry(change.number).or_default().insert(label.clone());
                    }
                }
            }
        }
        out
    }
}

/// Plan the rewrite of the ledger directory `ledger`. A symlink anywhere in
/// the tree refuses: the copy must be exactly the files the ledger holds.
// trace:STORY-2324 | ai:claude
pub(crate) fn plan(
    ledger: &Path,
    aida_dir: &Path,
    resolve: &dyn Fn(&str) -> Resolution,
) -> std::io::Result<Plan> {
    let mut plan = Plan::default();
    if ledger.exists() {
        walk(ledger, Path::new(""), resolve, &mut plan)?;
    }
    plan.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    plan.legacy = LEGACY_DIRS
        .iter()
        .map(|d| inventory_legacy(&aida_dir.join(d), d, resolve))
        .collect();
    Ok(plan)
}

fn walk(
    dir: &Path,
    rel: &Path,
    resolve: &dyn Fn(&str) -> Resolution,
    plan: &mut Plan,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir.join(rel))? {
        let entry = entry?;
        let name = PathBuf::from(entry.file_name());
        let rel = rel.join(&name);
        let meta = std::fs::symlink_metadata(dir.join(&rel))?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::other(format!(
                "{} is a symlink; refusing to migrate a ledger that is not plain files",
                rel.display()
            )));
        }
        if meta.is_dir() {
            walk(dir, &rel, resolve, plan)?;
            continue;
        }
        let v1 = std::fs::read(dir.join(&rel))?;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt as _;
            Some(meta.permissions().mode())
        };
        #[cfg(not(unix))]
        let mode = None;
        let is_ledger = rel.extension().and_then(|e| e.to_str()) == Some("jsonl");
        let v2 = match (is_ledger, std::str::from_utf8(&v1)) {
            (true, Ok(body)) => {
                let (out, fates) = stamp_file(body, resolve);
                let rel_s = rel.to_string_lossy().to_string();
                for fate in fates {
                    plan.lines += 1;
                    match fate {
                        LineFate::Verbatim => {}
                        LineFate::Stamped(_) => plan.stamped += 1,
                        LineFate::AlreadyKeyed => plan.already_keyed += 1,
                        LineFate::Unparsed => plan.unparsed += 1,
                        LineFate::Unresolved(l) => {
                            plan.unresolved
                                .entry(l)
                                .or_default()
                                .files
                                .insert(rel_s.clone());
                        }
                        LineFate::Ambiguous(l, why) => {
                            let sites = plan.ambiguous.entry(l).or_default();
                            sites.files.insert(rel_s.clone());
                            sites.why = Some(why);
                        }
                    }
                }
                out.into_bytes()
            }
            _ => v1.clone(),
        };
        plan.files.push(FilePlan {
            v1_sha256: sha256_hex(&v1),
            rel,
            v1,
            v2,
            mode,
        });
    }
    Ok(())
}

fn inventory_legacy(dir: &Path, name: &str, resolve: &dyn Fn(&str) -> Resolution) -> LegacyDir {
    let mut out = LegacyDir {
        dir: name.to_string(),
        ..Default::default()
    };
    out.files = count_files(dir);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut unresolved = BTreeSet::new();
    for entry in rd.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let file = entry.file_name().to_string_lossy().to_string();
        let stem = file.split('.').next().unwrap_or_default().to_string();
        if stem.is_empty() {
            continue;
        }
        if super::ChangeKey::parse(&stem).is_some() {
            out.change_keyed += 1;
        } else if matches!(resolve(&stem), Resolution::Hit(_)) {
            out.resolved += 1;
        } else {
            unresolved.insert(stem.to_ascii_uppercase());
        }
    }
    out.unresolved = unresolved.into_iter().collect();
    out
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => count_files(&e.path()),
            Ok(t) if t.is_file() => 1,
            _ => 0,
        })
        .sum()
}

/// Why `--execute` / `--rollback` may not run here, or `None`. The same
/// guards as `aida own migrate`: an operator at a terminal, in the host
/// shell (outside any contained seat or foreign pid namespace, where a
/// drain cannot be told from a stale lock), with no drain running.
// trace:STORY-2324 | ai:claude
pub(crate) fn execute_refusal(
    stdin_tty: bool,
    pid_ns: Option<u64>,
    contained: bool,
    live_drain: bool,
) -> Option<String> {
    if !stdin_tty {
        return Some(
            "this needs an operator at a terminal; run it from an interactive shell".to_string(),
        );
    }
    if contained {
        return Some("this refuses from a contained seat; run it from the host shell".to_string());
    }
    if pid_ns != Some(aida_core::process_probe::INITIAL_PID_NAMESPACE_INODE) {
        return Some(
            "this refuses outside the host's pid namespace (a sandboxed shell cannot tell a \
             running drain from a stale lock); run it from the host shell"
                .to_string(),
        );
    }
    if live_drain {
        return Some("this refuses while a drain is running; wait for it to finish".into());
    }
    None
}

/// Why the plan may not be executed against the open pull requests
/// `open` (`Err`: the forge did not answer), or `None`.
// trace:STORY-2324 | ai:claude
pub(crate) fn open_pr_refusal(plan: &Plan, open: &Result<BTreeSet<u64>, String>) -> Option<String> {
    let on_changes = plan.changes_with_unresolved();
    if on_changes.is_empty() {
        return None;
    }
    let open = match open {
        Ok(open) => open,
        Err(e) => {
            return Some(format!(
                "labels that name no single requirement sit on {} pull request(s), and the open \
                 pull requests could not be listed ({e}); refusing rather than guessing",
                on_changes.len()
            ))
        }
    };
    let hits: Vec<String> = on_changes
        .iter()
        .filter(|(n, _)| open.contains(n))
        .map(|(n, labels)| {
            format!(
                "PR-{n}: {}",
                labels.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    (!hits.is_empty()).then(|| {
        format!(
            "open pull requests carry labels that name no single requirement; fix or note them \
             first:\n  {}",
            hits.join("\n  ")
        )
    })
}

/// What `--execute` did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Applied {
    /// `false`: nothing needed a stamp, so the ledger was left untouched.
    pub swapped: bool,
    pub stamped: usize,
    pub files: usize,
    /// The archived v1 tree, relative to `.aida/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    /// The run's report, relative to `.aida/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// Where an executed run got to. The report is written `pending` before
/// the swap, so a run that stopped part way is never mistaken for none.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RunState {
    /// Written before the first rename; the swap may or may not have run.
    Pending,
    /// The v2 tree is in place.
    Done,
    /// The run stopped and put v1 back (or never moved it).
    Aborted,
    /// `--rollback` took this run's stamps off again.
    RolledBack,
}

/// One file a stamp changed: the extents rollback needs to take the stamps
/// off in place, whatever was appended after them.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StampedFile {
    pub v1_len: u64,
    pub v1_sha256: String,
    pub v2_len: u64,
    pub v2_sha256: String,
}

/// The record of one executed run, kept under `.aida/gate-ledger-migrations/`.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RunReport {
    pub stamp: String,
    /// The archived v1 tree, relative to `.aida/`.
    pub archive: String,
    pub store_head: String,
    pub stamped: usize,
    pub state: RunState,
    /// Why the run was aborted, or what a rollback kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Relative path -> extents, for every file a stamp changed.
    pub files: BTreeMap<String, StampedFile>,
}

fn report_rel(stamp: &str) -> String {
    format!("{REPORTS_DIR}/{stamp}.json")
}

/// Write `report` in place of its previous state, atomically.
fn write_report(aida_dir: &Path, report: &RunReport) -> std::io::Result<()> {
    let body = serde_json::to_string_pretty(report).map_err(std::io::Error::other)?;
    let path = aida_dir.join(report_rel(&report.stamp));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)
}

/// Mark `report` aborted (best effort: the error being returned matters
/// more than the report saying so).
fn abort_report(aida_dir: &Path, report: &mut RunReport, why: &str) {
    report.state = RunState::Aborted;
    report.note = Some(why.to_string());
    if let Err(e) = write_report(aida_dir, report) {
        eprintln!(
            "warning: could not mark the migration report {} aborted: {e}",
            report_rel(&report.stamp)
        );
    }
}

/// Write the plan's v2 tree, verify it, and swap it in for the ledger
/// under `aida_dir`. `stamp` names the archive and the report. Nothing
/// changes when no line needs a stamp (a second run is a no-op).
///
/// The caller holds the ledger-wide lock exclusively
/// ([`super::store::lock_ledger_exclusive`]) from before it planned until
/// this returns, so no writer appends between the plan and the swap. The
/// report is written `pending` before the first rename and finalized after
/// the swap (`done`) or on the way out of a failure (`aborted`).
// trace:STORY-2324 | ai:claude
pub(crate) fn execute(
    aida_dir: &Path,
    plan: &Plan,
    stamp: &str,
    store_head: &str,
) -> std::io::Result<Applied> {
    if plan.stamped == 0 {
        return Ok(Applied {
            swapped: false,
            stamped: 0,
            files: plan.files.len(),
            archive: None,
            report: None,
        });
    }
    let ledger = aida_dir.join("gate-ledger");
    let v2 = aida_dir.join(V2_DIR);
    let archive_name = format!("{ARCHIVE_PREFIX}{stamp}");
    let archive = aida_dir.join(&archive_name);
    if archive.exists() || aida_dir.join(report_rel(stamp)).exists() {
        return Err(std::io::Error::other(format!(
            "a migration stamped {stamp} already ran"
        )));
    }
    if std::fs::symlink_metadata(&v2).is_ok() {
        // A leftover of a run that stopped before its swap: ours to replace.
        std::fs::remove_dir_all(&v2)?;
    }
    std::fs::create_dir_all(&v2)?;
    let undo_v2 = |e: std::io::Error| {
        let _ = std::fs::remove_dir_all(&v2);
        e
    };
    for f in &plan.files {
        write_file(&v2.join(&f.rel), &f.v2, f.mode).map_err(undo_v2)?;
    }
    for f in &plan.files {
        let written = std::fs::read(v2.join(&f.rel)).map_err(undo_v2)?;
        verify_file(&f.v1, &written).map_err(|why| {
            undo_v2(std::io::Error::other(format!(
                "{} did not verify: {why}",
                f.rel.display()
            )))
        })?;
    }
    let mut report = RunReport {
        stamp: stamp.to_string(),
        archive: archive_name.clone(),
        store_head: store_head.to_string(),
        stamped: plan.stamped,
        state: RunState::Pending,
        note: None,
        files: plan
            .files
            .iter()
            .filter(|f| f.v1 != f.v2)
            .map(|f| {
                (
                    f.rel.to_string_lossy().to_string(),
                    StampedFile {
                        v1_len: f.v1.len() as u64,
                        v1_sha256: f.v1_sha256.clone(),
                        v2_len: f.v2.len() as u64,
                        v2_sha256: sha256_hex(&f.v2),
                    },
                )
            })
            .collect(),
    };
    write_report(aida_dir, &report).map_err(undo_v2)?;
    if let Err(e) = std::fs::rename(&ledger, &archive) {
        abort_report(
            aida_dir,
            &mut report,
            &format!("v1 could not be archived: {e}"),
        );
        return Err(undo_v2(e));
    }
    // Defence in depth under the lock: a writer that appended after the plan
    // read v1 would lose its event, so put v1 back and stop.
    if let Err(why) = unchanged_since_plan(&archive, plan) {
        let restored = restore_v1(aida_dir, &archive, stamp);
        let why = format!(
            "the gate ledger changed while it was being migrated ({why}); nothing was swapped, \
             run it again{}",
            restored_note(&restored)
        );
        abort_report(aida_dir, &mut report, &why);
        return Err(undo_v2(std::io::Error::other(why)));
    }
    if let Err(e) = swap_in(aida_dir, &v2, &archive, stamp) {
        abort_report(aida_dir, &mut report, &e.to_string());
        return Err(undo_v2(e));
    }
    report.state = RunState::Done;
    write_report(aida_dir, &report)?;
    Ok(Applied {
        swapped: true,
        stamped: plan.stamped,
        files: plan.files.len(),
        archive: Some(archive_name),
        report: Some(report_rel(stamp)),
    })
}

/// Move the verified `v2` tree into place. When that rename fails, v1 goes
/// back from `archive` whether or not something now sits at the ledger's
/// path: whatever is found there is set aside (never deleted) first.
// trace:STORY-2324 | ai:claude
pub(crate) fn swap_in(
    aida_dir: &Path,
    v2: &Path,
    archive: &Path,
    stamp: &str,
) -> std::io::Result<()> {
    let ledger = aida_dir.join("gate-ledger");
    let Err(e) = std::fs::rename(v2, &ledger) else {
        return Ok(());
    };
    let restored = restore_v1(aida_dir, archive, stamp);
    Err(std::io::Error::other(format!(
        "could not move {} into place ({e}){}",
        v2.display(),
        restored_note(&restored)
    )))
}

/// Put the archived v1 tree back at `.aida/gate-ledger`. Whatever sits at
/// that path is moved to `gate-ledger.stray-<stamp>` first. Returns the
/// stray's name, if one was set aside.
// trace:STORY-2324 | ai:claude
fn restore_v1(aida_dir: &Path, archive: &Path, stamp: &str) -> std::io::Result<Option<String>> {
    let ledger = aida_dir.join("gate-ledger");
    let mut stray = None;
    if std::fs::symlink_metadata(&ledger).is_ok() {
        let name = format!("gate-ledger.stray-{stamp}");
        std::fs::rename(&ledger, aida_dir.join(&name))?;
        stray = Some(name);
    }
    std::fs::rename(archive, &ledger)?;
    Ok(stray)
}

fn restored_note(restored: &std::io::Result<Option<String>>) -> String {
    match restored {
        Ok(None) => "; v1 was put back".to_string(),
        Ok(Some(stray)) => {
            format!("; v1 was put back, and what stood in its place was moved to .aida/{stray}")
        }
        Err(e) => {
            format!("; v1 could NOT be put back ({e}): it is in the archive the report names")
        }
    }
}

fn write_file(path: &Path, bytes: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

/// Every file of `tree` is the file `plan` read, and there is no other.
fn unchanged_since_plan(tree: &Path, plan: &Plan) -> Result<(), String> {
    let now = file_hashes(tree).map_err(|e| e.to_string())?;
    let then: BTreeMap<String, String> = plan
        .files
        .iter()
        .map(|f| (f.rel.to_string_lossy().to_string(), f.v1_sha256.clone()))
        .collect();
    if now == then {
        return Ok(());
    }
    let changed: Vec<String> = now
        .keys()
        .chain(then.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|k| now.get(*k) != then.get(*k))
        .cloned()
        .collect();
    Err(format!("changed: {}", changed.join(", ")))
}

/// Relative path -> SHA-256 of every file under `tree`.
fn file_hashes(tree: &Path) -> std::io::Result<BTreeMap<String, String>> {
    fn go(root: &Path, rel: &Path, out: &mut BTreeMap<String, String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(root.join(rel))? {
            let entry = entry?;
            let rel = rel.join(entry.file_name());
            let meta = std::fs::symlink_metadata(root.join(&rel))?;
            if meta.is_dir() {
                go(root, &rel, out)?;
            } else {
                let bytes = std::fs::read(root.join(&rel))?;
                out.insert(rel.to_string_lossy().to_string(), sha256_hex(&bytes));
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    if tree.exists() {
        go(tree, Path::new(""), &mut out)?;
    }
    Ok(out)
}

/// What `--rollback` did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RolledBack {
    /// The run whose stamps came off.
    pub stamp: String,
    /// Files whose stamps were taken off.
    pub files: usize,
    /// Lines appended after the migration, kept as written.
    pub kept_lines: usize,
}

/// One file's rollback, computed before anything is written.
struct Unstamp {
    rel: String,
    bytes: Vec<u8>,
    kept_lines: usize,
}

/// Take the newest run's stamps off in place, on the live tree. Stamping is
/// reversible line by line, so every event written since the migration is
/// kept. Each file the run changed must still start with the exact bytes it
/// wrote; that prefix is unstamped line by line and must come back to the
/// v1 bytes (length and SHA-256), and every line after it stays as written.
/// A file that already starts with its v1 bytes was rolled back before and
/// is skipped. Every file is checked before any is written. The caller holds
/// the ledger-wide lock exclusively.
// trace:STORY-2324 | ai:claude
pub(crate) fn rollback(aida_dir: &Path) -> std::io::Result<RolledBack> {
    let reports_dir = aida_dir.join(REPORTS_DIR);
    let mut reports: Vec<RunReport> = std::fs::read_dir(&reports_dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let body = std::fs::read_to_string(e.path()).ok()?;
                    serde_json::from_str::<RunReport>(&body).ok()
                })
                .collect()
        })
        .unwrap_or_default();
    reports.sort_by(|a, b| a.stamp.cmp(&b.stamp));
    let Some(mut report) = reports
        .into_iter()
        .rev()
        .find(|r| matches!(r.state, RunState::Done | RunState::Pending))
    else {
        return Err(std::io::Error::other(
            "no gate-ledger migration to roll back",
        ));
    };
    let ledger = aida_dir.join("gate-ledger");
    let mut work: Vec<Unstamp> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for (rel, extents) in &report.files {
        match unstamp_file(&ledger.join(rel), extents) {
            Ok(Some((bytes, kept_lines))) => work.push(Unstamp {
                rel: rel.clone(),
                bytes,
                kept_lines,
            }),
            Ok(None) => {}
            Err(why) => refused.push(format!("{rel}: {why}")),
        }
    }
    if !refused.is_empty() {
        return Err(std::io::Error::other(format!(
            "the stamps of run {} cannot be taken off safely, so nothing was changed:\n  {}",
            report.stamp,
            refused.join("\n  ")
        )));
    }
    for w in &work {
        replace_file(&ledger.join(&w.rel), &w.bytes)?;
    }
    let kept_lines = work.iter().map(|w| w.kept_lines).sum();
    report.state = RunState::RolledBack;
    report.note = Some(format!(
        "unstamped {} file(s) in place; kept {kept_lines} line(s) written after the migration",
        work.len()
    ));
    write_report(aida_dir, &report)?;
    Ok(RolledBack {
        stamp: report.stamp,
        files: work.len(),
        kept_lines,
    })
}

/// `path` with the run's stamps taken off its prefix, and how many lines
/// follow that prefix; `None` when it already starts with its v1 bytes.
fn unstamp_file(path: &Path, extents: &StampedFile) -> Result<Option<(Vec<u8>, usize)>, String> {
    let now = std::fs::read(path).map_err(|e| e.to_string())?;
    let prefix = |len: u64| now.get(..usize::try_from(len).ok()?);
    let Some(head) = prefix(extents.v2_len).filter(|p| sha256_hex(p) == extents.v2_sha256) else {
        if prefix(extents.v1_len).is_some_and(|p| sha256_hex(p) == extents.v1_sha256) {
            return Ok(None);
        }
        return Err("it no longer starts with the lines the migration wrote".to_string());
    };
    let tail = &now[head.len()..];
    let mut v1 = Vec::with_capacity(head.len());
    for line in head.split_inclusive(|b| *b == b'\n') {
        let (body, nl) = split_nl(line);
        match std::str::from_utf8(body).ok().and_then(unstamp) {
            Some((_, original)) => v1.extend_from_slice(original.as_bytes()),
            None => v1.extend_from_slice(body),
        }
        if nl {
            v1.push(b'\n');
        }
    }
    if v1.len() as u64 != extents.v1_len || sha256_hex(&v1) != extents.v1_sha256 {
        return Err("its unstamped lines do not hash to the v1 file".to_string());
    }
    let kept_lines = tail
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .count();
    v1.extend_from_slice(tail);
    Ok(Some((v1, kept_lines)))
}

/// Replace `path` atomically with `bytes`, keeping its permissions.
fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let perms = std::fs::metadata(path)?.permissions();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.rollback"));
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, perms)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// The open pull requests on origin.
fn open_prs(main: &Path) -> Result<BTreeSet<u64>, String> {
    let out = std::process::Command::new("gh")
        .current_dir(main)
        .args([
            "pr", "list", "--state", "open", "--limit", "1000", "--json", "number",
        ])
        .output()
        .map_err(|e| format!("gh could not run: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("gh answered malformed JSON: {e}"))?;
    Ok(rows
        .iter()
        .filter_map(|r| r.get("number")?.as_u64())
        .collect())
}

/// Which way `aida gate migrate` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMode {
    DryRun,
    Execute,
    Rollback,
}

/// `aida gate migrate`.
// trace:STORY-2324 | ai:claude
pub(crate) fn run(mode: RunMode, json: bool) -> anyhow::Result<()> {
    use std::io::IsTerminal as _;
    let project_root = crate::find_project_root()?;
    let main = crate::main_worktree_root_from(&project_root);
    let root = super::store_root(&main)?;
    let aida_dir = root.join(".aida");
    if mode != RunMode::DryRun {
        let pid_ns = aida_core::process_probe::ProbeSource::system()
            .pid_namespace(std::process::id())
            .ok();
        if let Some(why) = execute_refusal(
            std::io::stdin().is_terminal(),
            pid_ns,
            super::contained(),
            crate::drain_lock::read_pid_live_lock(&root).is_some(),
        ) {
            anyhow::bail!("{why}");
        }
    }
    if mode == RunMode::Rollback {
        // trace:STORY-2324 | ai:claude — no writer appends mid-rollback.
        let _lock = super::store::lock_ledger_exclusive(&root)?;
        let done = rollback(&aida_dir)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&done)?);
        } else {
            println!(
                "took the stamps of run {} off {} file(s) in place; kept {} line(s) written \
                 since",
                done.stamp, done.files, done.kept_lines
            );
        }
        return Ok(());
    }
    let snapshot = crate::gate::canon::Snapshot::at_head(&root)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| anyhow::anyhow!("this project has no git-canonical requirements store"))?;
    let resolve = |label: &str| snapshot.resolve_one(label);
    // Asked before the lock is taken: writers wait while it is held.
    let open = open_prs(&main);
    // trace:STORY-2324 | ai:claude — `--execute` holds the ledger-wide lock
    // from its plan through its swap, so no writer appends in between.
    let lock = if mode == RunMode::Execute {
        Some(super::store::lock_ledger_exclusive(&root)?)
    } else {
        None
    };
    let plan = plan(&super::ledger_dir(&root), &aida_dir, &resolve)?;
    let applied = if mode == RunMode::Execute {
        if let Some(why) = open_pr_refusal(&plan, &open) {
            anyhow::bail!("{why}");
        }
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        Some(execute(&aida_dir, &plan, &stamp, &snapshot.head)?)
    } else {
        None
    };
    drop(lock);
    let blocking = open_pr_refusal(&plan, &open);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(
                &plan,
                &snapshot.head,
                blocking.as_deref(),
                applied.as_ref()
            ))?
        );
    } else {
        print_report(&plan, &snapshot.head, blocking.as_deref(), applied.as_ref());
    }
    Ok(())
}

fn report_json(
    plan: &Plan,
    store_head: &str,
    blocking: Option<&str>,
    applied: Option<&Applied>,
) -> serde_json::Value {
    serde_json::json!({
        "mode": if applied.is_some() { "execute" } else { "dry-run" },
        "store_head": store_head,
        "files": plan.files.len(),
        "lines": plan.lines,
        "stamped": plan.stamped,
        "already_keyed": plan.already_keyed,
        "unparsed": plan.unparsed,
        "unresolved": plan.unresolved,
        "ambiguous": plan.ambiguous,
        "open_pr_blockers": blocking,
        "legacy": plan.legacy,
        "applied": applied,
    })
}

fn print_report(plan: &Plan, store_head: &str, blocking: Option<&str>, applied: Option<&Applied>) {
    let mode = if applied.is_some() {
        "execute"
    } else {
        "dry run"
    };
    println!(
        "gate-ledger migration ({mode}) against store {}",
        store_head.get(..12).unwrap_or(store_head)
    );
    println!("  files            {}", plan.files.len());
    println!("  lines            {}", plan.lines);
    println!("  to stamp         {}", plan.stamped);
    println!("  already keyed    {}", plan.already_keyed);
    println!("  unparseable      {} (copied as is)", plan.unparsed);
    for (title, labels) in [
        ("unresolved", &plan.unresolved),
        ("ambiguous", &plan.ambiguous),
    ] {
        if labels.is_empty() {
            continue;
        }
        println!("  {title} labels (kept without a key):");
        for (label, sites) in labels {
            let files: Vec<&str> = sites.files.iter().map(String::as_str).collect();
            match &sites.why {
                Some(why) => println!("    {label} ({why}): {}", files.join(", ")),
                None => println!("    {label}: {}", files.join(", ")),
            }
        }
    }
    match blocking {
        Some(why) => println!("  --execute would refuse: {why}"),
        None => println!("  no open pull request carries an unresolved label"),
    }
    println!("legacy stores (read through until the cutover; left in place):");
    for d in &plan.legacy {
        println!(
            "  .aida/{:<22} {} file(s): {} change-keyed, {} resolved, {} unresolved",
            d.dir,
            d.files,
            d.change_keyed,
            d.resolved,
            d.unresolved.len()
        );
    }
    match applied {
        Some(a) if a.swapped => println!(
            "stamped {} line(s); v1 archived at .aida/{}; report at .aida/{}",
            a.stamped,
            a.archive.as_deref().unwrap_or_default(),
            a.report.as_deref().unwrap_or_default()
        ),
        Some(_) => println!("nothing to stamp; the ledger was left as it is"),
        None => {}
    }
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
