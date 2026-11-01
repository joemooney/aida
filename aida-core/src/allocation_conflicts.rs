//! Allocation-claim conflicts across pinned store commits.
//!
//! Every store synchronization joins allocation claims from more than one
//! clone: global ID ranges (`registry/blocks.yaml`), node bindings
//! (`registry/nodes.toml`), per-type high-water marks
//! (`registry/agreed_counters.toml`) and the display IDs each requirement
//! object holds (its `spec_id` and `agreed_id`, owned by its UUID). Git and the
//! structured unions accept two clones that independently issued the same
//! identity. This module reads the local tip, every incoming tip and ALL their
//! merge bases from immutable commits and reports every conflict among those
//! claims before anything advances.
//!
//! - The loader reads committed trees only (`git ls-tree` and
//!   `git cat-file --batch`, replace refs and lazy fetch off), never a working
//!   tree, and applies no ownership or permission checks to the files it reads.
//! - [`analyze`] is pure over the loaded trees and deterministic.
//! - Any git read that fails or is refused, a resource bound, a malformed or
//!   duplicate row, or missing ancestry makes the report incomplete. An
//!   incomplete report refuses like a conflict; it is never a clean answer.
//! - Report text names only conflict categories, commit IDs, object UUIDs,
//!   fixed path classes and row ordinals: never a node name, host, email,
//!   filename, display ID or parser message.
//!
//! One deliberate narrowing of the "`*` range overlaps a per-type range" rule:
//! a `*` block conflicts with a per-type range only over the part of it the
//! `*` block has already dispensed (`range_start..next`, the furthest `next`
//! any tip records). Established stores hold dormant `*` blocks spanning the
//! ranges later per-type claims are carved from; an undispensed `*` range has
//! issued nothing that can collide, and any dispensing from it shows up as a
//! later `next` in some tip.
// trace:BUG-2017 | ai:claude
use crate::node::{AgreedCounters, BlockRegistry, NodeRegistry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use uuid::Uuid;

/// Largest single allocation blob read.
pub const MAX_BLOB_BYTES: u64 = 4 << 20;
/// Most allocation blobs one pinned tree may hold.
pub const MAX_TREE_BLOBS: usize = 20_000;
/// Most allocation bytes one pinned tree may hold.
pub const MAX_TREE_BYTES: u64 = 128 << 20;
/// Most distinct allocation bytes read across every tip and merge base.
pub const MAX_TOTAL_BYTES: u64 = 512 << 20;

/// The fixed store location a record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathClass {
    Objects,
    Blocks,
    Nodes,
    Counters,
}

impl PathClass {
    pub fn as_str(self) -> &'static str {
        match self {
            PathClass::Objects => "objects",
            PathClass::Blocks => "registry/blocks.yaml",
            PathClass::Nodes => "registry/nodes.toml",
            PathClass::Counters => "registry/agreed_counters.toml",
        }
    }
}

/// An opaque reference to one record: commit, path class, row ordinal within
/// that class in that commit, and the object UUID when the row has one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct RecordRef {
    pub commit: String,
    pub class: PathClass,
    pub ordinal: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
}

impl std::fmt::Display for RecordRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let commit: String = self.commit.chars().take(12).collect();
        match self.uuid {
            Some(uuid) => write!(
                f,
                "{uuid} ({}#{} @ {commit})",
                self.class.as_str(),
                self.ordinal
            ),
            None => write!(f, "{}#{} @ {commit}", self.class.as_str(), self.ordinal),
        }
    }
}

/// What kind of claims disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Two allocations can dispense the same global ID: overlapping ranges,
    /// or one block independently claimed on two sides.
    GlobalRange,
    /// One node id is bound to different registrations, or was registered
    /// independently on two sides.
    NodeBinding,
    /// Different object UUIDs claim one display ID.
    DisplayId,
    /// A global-form display ID lies above every recorded range and counter.
    Unissued,
}

impl ConflictKind {
    /// The fixed sanitized phrase for this category.
    pub fn describe(self) -> &'static str {
        match self {
            ConflictKind::GlobalRange => "global ID ranges can dispense the same ID",
            ConflictKind::NodeBinding => "one node id is bound to different registrations",
            ConflictKind::DisplayId => "different objects claim one display ID",
            ConflictKind::Unissued => "a global display ID lies outside every recorded range",
        }
    }
}

/// Whether a store sync refuses a conflict. Every conflict a sync's collector
/// reports is [`ConflictScope::Introduced`]; only the on-demand audit reports
/// [`ConflictScope::InHead`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictScope {
    /// Newly introduced by joining the tips (it differs between the local
    /// tip, the incoming tips and their merge bases): a sync refuses it.
    Introduced,
    /// Already inside `HEAD`'s own tree and not introduced by any audited
    /// join: a sync that already shares it at the merge base admits it.
    InHead,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Conflict {
    pub kind: ConflictKind,
    pub refs: Vec<RecordRef>,
    pub scope: ConflictScope,
    /// The disputed claim (display ID, node id or range key), so one dispute
    /// found by two passes is reported once. Never serialized: it can hold
    /// store text.
    #[serde(skip)]
    key: String,
}

/// Why the collector could not see every claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// A git read failed or was refused.
    GitRead,
    /// A resource bound was exceeded; nothing past it was read.
    Bound,
    /// A row or file could not be read strictly.
    Malformed,
    /// One record appears twice in one tree.
    Duplicate,
    /// The shared history needed to tell baseline from new claims is missing.
    MissingAncestry,
}

impl GapKind {
    /// The fixed sanitized phrase for this gap.
    pub fn describe(self) -> &'static str {
        match self {
            GapKind::GitRead => "a git read failed or was refused",
            GapKind::Bound => "a resource bound was exceeded",
            GapKind::Malformed => "a record could not be read strictly",
            GapKind::Duplicate => "a record appears twice in one tree",
            GapKind::MissingAncestry => "the shared history is missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Gap {
    pub kind: GapKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<PathClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<usize>,
}

/// Every conflict found, plus what kept the collection from being complete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ConflictReport {
    pub conflicts: Vec<Conflict>,
    pub gaps: Vec<Gap>,
}

impl ConflictReport {
    /// Complete and conflict-free: the only state that admits a sync.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty() && self.gaps.is_empty()
    }

    pub fn is_complete(&self) -> bool {
        self.gaps.is_empty()
    }

    fn gap(
        &mut self,
        kind: GapKind,
        commit: Option<&str>,
        class: Option<PathClass>,
        ordinal: Option<usize>,
    ) {
        self.gaps.push(Gap {
            kind,
            commit: commit.map(str::to_string),
            class,
            ordinal,
        });
    }

    fn normalize(&mut self) {
        for c in &mut self.conflicts {
            c.refs.sort();
            c.refs.dedup();
        }
        self.conflicts.sort();
        self.conflicts.dedup();
        self.gaps.sort();
        self.gaps.dedup();
    }
}

impl std::fmt::Display for ConflictReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.is_complete() {
            "complete"
        } else {
            "incomplete"
        };
        write!(
            f,
            "allocation claims check refused this store sync: {} conflict(s), status {status}",
            self.conflicts.len()
        )?;
        for c in &self.conflicts {
            let refs: Vec<String> = c.refs.iter().map(ToString::to_string).collect();
            write!(f, "\n  {}: {}", c.kind.describe(), refs.join(", "))?;
        }
        for g in &self.gaps {
            write!(f, "\n  incomplete: {}", g.kind.describe())?;
            if let Some(class) = g.class {
                write!(f, " in {}", class.as_str())?;
                if let Some(n) = g.ordinal {
                    write!(f, "#{n}")?;
                }
            }
            if let Some(commit) = &g.commit {
                write!(f, " @ {}", commit.chars().take(12).collect::<String>())?;
            }
        }
        write!(
            f,
            "\nNothing was merged, rebased, reset or pushed. Keep every UUID; repair by \
             claiming a fresh disjoint block on the authoritative clone, recording a \
             reviewed ID mapping, and retrying the sync."
        )
    }
}

/// A store synchronization would join conflicting allocation claims, or could
/// not read every claim. Fatal for every caller, like an evidence refusal.
#[derive(Debug)]
pub struct AllocationRefusal(pub ConflictReport);

impl std::fmt::Display for AllocationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for AllocationRefusal {}

/// Whether `err`, through any added context, is a store-sync refusal: a
/// hypothesis evidence refusal or an allocation refusal. The one predicate
/// best-effort sync callers use to decide that a failure must stop them.
pub fn is_sync_refusal(err: &anyhow::Error) -> bool {
    crate::hypothesis::is_evidence_refusal(err) || err.chain().any(|e| e.is::<AllocationRefusal>())
}

// ---------------------------------------------------------------------------
// Pinned trees
// ---------------------------------------------------------------------------

/// One block row: `(node, TYPE, start, end)` and its dispense point.
#[derive(Debug, Clone)]
pub(crate) struct BlockRow {
    pub key: (String, String, u32, u32),
    pub next: u32,
    pub ordinal: usize,
}

/// The allocation claims of one pinned commit.
#[derive(Debug, Clone, Default)]
pub(crate) struct Tree {
    pub commit: String,
    pub has_blocks: bool,
    pub blocks: Vec<BlockRow>,
    /// node id → (registration digest, ordinal).
    pub nodes: BTreeMap<String, (String, usize)>,
    /// canonical display ID → owning UUIDs with their object ordinal.
    pub ids: BTreeMap<String, BTreeMap<Uuid, usize>>,
    pub counters: BTreeMap<String, u32>,
}

/// The fields a requirement object contributes to allocation.
#[derive(Deserialize)]
struct ObjectClaim {
    id: Uuid,
    #[serde(default)]
    spec_id: Option<String>,
    #[serde(default)]
    agreed_id: Option<String>,
}

/// Top-level `id`, `spec_id` and `agreed_id` of an object read line by line.
/// None when the text is not plainly shaped, so the caller parses strictly.
fn scan_claim(text: &str) -> Option<(Uuid, Option<String>, Option<String>)> {
    let mut found: [Option<&str>; 3] = [None, None, None];
    for line in text.lines() {
        if line == "---" || line == "..." || line.starts_with('%') {
            return None;
        }
        for (slot, key) in ["id:", "spec_id:", "agreed_id:"].iter().enumerate() {
            if let Some(rest) = line.strip_prefix(key) {
                if found[slot].is_some() {
                    return None;
                }
                found[slot] = Some(rest.trim());
            }
        }
    }
    let plain = |v: &str| -> Option<String> {
        let v = match (v.chars().next(), v.chars().last()) {
            (Some('"'), Some('"')) | (Some('\''), Some('\'')) if v.len() >= 2 => &v[1..v.len() - 1],
            _ => v,
        };
        (!v.is_empty()
            && v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '$'))
        .then(|| v.to_string())
    };
    let id = Uuid::parse_str(&plain(found[0]?)?).ok()?;
    let optional = |v: Option<&str>| -> Option<Option<String>> {
        match v {
            None => Some(None),
            Some("~") | Some("null") => Some(None),
            Some(v) => plain(v).map(Some),
        }
    };
    Some((id, optional(found[1])?, optional(found[2])?))
}

fn strict_claim(text: &str) -> Option<(Uuid, Option<String>, Option<String>)> {
    let claim: ObjectClaim = serde_yaml::from_str(text).ok()?;
    Some((claim.id, claim.spec_id, claim.agreed_id))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Run one read-only plumbing command on pinned objects: replace refs and
/// lazy fetching off. Errors carry no git output.
fn git_read(
    repo: &Path,
    args: &[&str],
    input: Option<Vec<u8>>,
) -> std::result::Result<Vec<u8>, ()> {
    let mut child = crate::git_ops::host_git_command(repo)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let writer = input.map(|bytes| {
        let mut stdin = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || stdin.write_all(&bytes))
    });
    let output = child.wait_with_output().map_err(|_| ())?;
    if let Some(writer) = writer {
        writer.join().map_err(|_| ())?.map_err(|_| ())?;
    }
    output.status.success().then_some(output.stdout).ok_or(())
}

/// `git` exit status and stdout, for commands whose exit 1 carries meaning.
fn git_status(repo: &Path, args: &[&str]) -> std::result::Result<(i32, String, bool), ()> {
    let output = crate::git_ops::host_git_command(repo)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| ())?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        output.stderr.is_empty(),
    ))
}

/// One listed allocation blob.
struct Entry {
    class: PathClass,
    ordinal: usize,
    oid: String,
}

/// Reads pinned trees, sharing blob text across them.
#[derive(Default)]
struct Loader {
    text: HashMap<String, Vec<u8>>,
    total: u64,
}

impl Loader {
    /// List the allocation blobs of `commit`. Gaps go to `report`.
    fn list(
        &mut self,
        repo: &Path,
        commit: &str,
        report: &mut ConflictReport,
    ) -> Option<Vec<Entry>> {
        let Ok(raw) = git_read(
            repo,
            &[
                "ls-tree",
                "-r",
                "-z",
                "-l",
                "--full-tree",
                commit,
                "--",
                "objects/",
                "registry/blocks.yaml",
                "registry/nodes.toml",
                "registry/agreed_counters.toml",
            ],
            None,
        ) else {
            report.gap(GapKind::GitRead, Some(commit), None, None);
            return None;
        };
        let mut entries = Vec::new();
        let mut objects = 0usize;
        let mut bytes = 0u64;
        let mut sizes: Vec<(usize, u64)> = Vec::new();
        for raw in raw.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let parsed = std::str::from_utf8(raw).ok().and_then(|entry| {
                let (meta, path) = entry.split_once('\t')?;
                let mut meta = meta.split_whitespace();
                let (mode, kind, oid, size) =
                    (meta.next()?, meta.next()?, meta.next()?, meta.next()?);
                Some((
                    mode.to_string(),
                    kind.to_string(),
                    oid.to_string(),
                    size.to_string(),
                    path.to_string(),
                ))
            });
            let Some((mode, kind, oid, size, path)) = parsed else {
                report.gap(GapKind::Malformed, Some(commit), None, None);
                continue;
            };
            let class = match path.as_str() {
                "registry/blocks.yaml" => PathClass::Blocks,
                "registry/nodes.toml" => PathClass::Nodes,
                "registry/agreed_counters.toml" => PathClass::Counters,
                _ if path.split('/').count() == 4 && path.ends_with(".yaml") => PathClass::Objects,
                _ => continue,
            };
            let ordinal = if class == PathClass::Objects {
                objects += 1;
                objects - 1
            } else {
                0
            };
            let size = size.parse::<u64>().ok();
            let (true, Some(size)) = (
                kind == "blob" && (mode == "100644" || mode == "100755"),
                size,
            ) else {
                report.gap(GapKind::Malformed, Some(commit), Some(class), Some(ordinal));
                continue;
            };
            if size > MAX_BLOB_BYTES {
                report.gap(GapKind::Bound, Some(commit), Some(class), Some(ordinal));
                continue;
            }
            bytes += size;
            sizes.push((entries.len(), size));
            entries.push(Entry {
                class,
                ordinal,
                oid,
            });
        }
        if entries.len() > MAX_TREE_BLOBS || bytes > MAX_TREE_BYTES {
            report.gap(GapKind::Bound, Some(commit), None, None);
            return None;
        }
        let mut wanted = Vec::new();
        let mut seen = HashSet::new();
        for (i, size) in sizes {
            let oid = &entries[i].oid;
            if !self.text.contains_key(oid) && seen.insert(oid.clone()) {
                self.total += size;
                wanted.push(oid.clone());
            }
        }
        if self.total > MAX_TOTAL_BYTES {
            report.gap(GapKind::Bound, Some(commit), None, None);
            return None;
        }
        if !self.load(repo, &wanted) {
            report.gap(GapKind::GitRead, Some(commit), None, None);
            return None;
        }
        Some(entries)
    }

    /// Read every blob in `wanted`. False on any missing or truncated blob.
    fn load(&mut self, repo: &Path, wanted: &[String]) -> bool {
        if wanted.is_empty() {
            return true;
        }
        let mut input = wanted.join("\n").into_bytes();
        input.push(b'\n');
        let Ok(batch) = git_read(repo, &["cat-file", "--batch"], Some(input)) else {
            return false;
        };
        let mut rest = batch.as_slice();
        for oid in wanted {
            let Some(end) = rest.iter().position(|b| *b == b'\n') else {
                return false;
            };
            let header = String::from_utf8_lossy(&rest[..end]).into_owned();
            rest = &rest[end + 1..];
            let mut fields = header.split(' ');
            let size = match (fields.next(), fields.next(), fields.next()) {
                (Some(got), Some("blob"), Some(size)) if got == oid => size.parse::<usize>().ok(),
                _ => None,
            };
            let Some(size) = size else {
                return false;
            };
            if rest.len() <= size {
                return false;
            }
            self.text.insert(oid.clone(), rest[..size].to_vec());
            rest = &rest[size + 1..];
        }
        true
    }

    /// Build the claims of `commit`. `strict` holds blobs that must be parsed
    /// strictly whatever the line scan says.
    fn tree(
        &self,
        commit: &str,
        entries: &[Entry],
        strict: &HashSet<String>,
        report: &mut ConflictReport,
    ) -> Tree {
        let mut tree = Tree {
            commit: commit.to_string(),
            ..Tree::default()
        };
        let mut uuids: HashSet<Uuid> = HashSet::new();
        for entry in entries {
            let bytes = &self.text[&entry.oid];
            let malformed = |report: &mut ConflictReport| {
                report.gap(
                    GapKind::Malformed,
                    Some(commit),
                    Some(entry.class),
                    Some(entry.ordinal),
                )
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                malformed(report);
                continue;
            };
            match entry.class {
                PathClass::Objects => {
                    let scanned = scan_claim(text);
                    let claim = if scanned.is_none() || strict.contains(&entry.oid) {
                        let parsed = strict_claim(text);
                        if scanned.is_some() && parsed != scanned {
                            None
                        } else {
                            parsed
                        }
                    } else {
                        scanned
                    };
                    let Some((uuid, spec, agreed)) = claim else {
                        malformed(report);
                        continue;
                    };
                    if !uuids.insert(uuid) {
                        report.gap(
                            GapKind::Duplicate,
                            Some(commit),
                            Some(entry.class),
                            Some(entry.ordinal),
                        );
                        continue;
                    }
                    for id in [spec, agreed].into_iter().flatten() {
                        tree.ids
                            .entry(crate::object_store::canonical_spec_id(&id))
                            .or_default()
                            .insert(uuid, entry.ordinal);
                    }
                }
                PathClass::Blocks => {
                    tree.has_blocks = true;
                    let Ok(registry) = serde_yaml::from_str::<BlockRegistry>(text) else {
                        malformed(report);
                        continue;
                    };
                    let mut keys = HashSet::new();
                    for (ordinal, b) in registry.blocks.into_iter().enumerate() {
                        let key = (
                            b.node_id,
                            b.type_prefix.to_uppercase(),
                            b.range_start,
                            b.range_end,
                        );
                        if key.2 > key.3 {
                            report.gap(
                                GapKind::Malformed,
                                Some(commit),
                                Some(entry.class),
                                Some(ordinal),
                            );
                            continue;
                        }
                        if !keys.insert(key.clone()) {
                            report.gap(
                                GapKind::Duplicate,
                                Some(commit),
                                Some(entry.class),
                                Some(ordinal),
                            );
                            continue;
                        }
                        tree.blocks.push(BlockRow {
                            key,
                            next: b.next,
                            ordinal,
                        });
                    }
                }
                PathClass::Nodes => {
                    let Ok(registry) = toml::from_str::<NodeRegistry>(text) else {
                        malformed(report);
                        continue;
                    };
                    for (ordinal, node) in registry.nodes.into_iter().enumerate() {
                        let Ok(payload) = serde_json::to_vec(&node) else {
                            report.gap(
                                GapKind::Malformed,
                                Some(commit),
                                Some(entry.class),
                                Some(ordinal),
                            );
                            continue;
                        };
                        if tree
                            .nodes
                            .insert(node.id, (digest(&payload), ordinal))
                            .is_some()
                        {
                            report.gap(
                                GapKind::Duplicate,
                                Some(commit),
                                Some(entry.class),
                                Some(ordinal),
                            );
                        }
                    }
                }
                PathClass::Counters => {
                    let Ok(counters) = toml::from_str::<AgreedCounters>(text) else {
                        malformed(report);
                        continue;
                    };
                    for (prefix, n) in counters.counters {
                        tree.counters.insert(prefix.to_uppercase(), n);
                    }
                }
            }
        }
        tree
    }
}

/// Resolve `rev` to one commit without lazy fetching.
fn resolve(repo: &Path, rev: &str) -> Option<String> {
    let out = git_read(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ],
        None,
    )
    .ok()?;
    let sha = String::from_utf8_lossy(&out).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Collect every allocation conflict among `local` (None for an unborn
/// branch), the `incoming` tips and all merge bases of every pair of tips.
/// Never fails: anything it cannot read is a gap in the report.
pub fn collect(repo: &Path, local: Option<&str>, incoming: &[String]) -> ConflictReport {
    collect_with(repo, local, incoming, true)
}

/// Whether `repo`'s `HEAD` names a branch that has no commit yet. A branch
/// ref that exists but does not resolve is not unborn: that is a read gap.
fn head_is_unborn(repo: &Path) -> bool {
    if resolve(repo, "HEAD").is_some() {
        return false;
    }
    let Ok((0, branch, true)) = git_status(repo, &["symbolic-ref", "--quiet", "HEAD"]) else {
        return false;
    };
    matches!(
        git_status(repo, &["for-each-ref", "--format=%(refname)", &branch]),
        Ok((0, listed, true)) if listed.is_empty()
    )
}

/// The on-demand audit (`aida db audit-ids`): every conflict [`collect`]
/// reports for a sync of `HEAD` with `incoming` ([`ConflictScope::Introduced`]:
/// a sync refuses it), plus the conflicts already inside `HEAD`'s own tree,
/// judged against an empty baseline that no sync applies
/// ([`ConflictScope::InHead`]: a sync that shares it at the merge base admits
/// it). A dispute both passes find is reported once, as introduced. An
/// unborn `HEAD` holds no claims and is not a gap; with an unborn `HEAD` the
/// incoming tips are what the first sync lands in `HEAD`, so their own trees
/// are judged against the empty baseline instead (a lone incoming tip is a
/// sync's own baseline, which would otherwise hide its in-tree conflicts).
// trace:BUG-2017 | ai:claude
pub fn audit(repo: &Path, incoming: &[String]) -> ConflictReport {
    let local = (!head_is_unborn(repo)).then_some("HEAD");
    let mut report = match local {
        Some(head) => collect_with(repo, Some(head), &[], false),
        None if incoming.is_empty() => ConflictReport::default(),
        None => collect_with(repo, None, incoming, false),
    };
    for c in &mut report.conflicts {
        c.scope = ConflictScope::InHead;
    }
    if !incoming.is_empty() {
        let joined = collect(repo, local, incoming);
        report.conflicts.retain(|c| {
            !joined
                .conflicts
                .iter()
                .any(|j| j.kind == c.kind && j.key == c.key)
        });
        report.conflicts.extend(joined.conflicts);
        report.gaps.extend(joined.gaps);
        report.normalize();
    }
    report
}

/// [`collect`], where `lone_tip_is_baseline` decides whether a single
/// distinct tip is its own baseline (a sync) or is judged against an empty
/// baseline (an audit of the claims inside one tree).
fn collect_with(
    repo: &Path,
    local: Option<&str>,
    incoming: &[String],
    lone_tip_is_baseline: bool,
) -> ConflictReport {
    let mut report = ConflictReport::default();
    let mut tips: Vec<String> = Vec::new();
    for rev in local.into_iter().chain(incoming.iter().map(String::as_str)) {
        match resolve(repo, rev) {
            Some(sha) => {
                if !tips.contains(&sha) {
                    tips.push(sha);
                }
            }
            None => {
                let shown = rev.chars().all(|c| c.is_ascii_hexdigit()).then_some(rev);
                report.gap(GapKind::GitRead, shown, None, None);
            }
        }
    }
    // A tip that is an ancestor of another tip claims nothing new: every
    // claim it holds is history the descendant already carries, and joining
    // it is a fast-forward. Counted as its own claimant, a hub that is merely
    // behind looks like a second side that kept an old registration, and a
    // claim changed after the hub's tip (a node marked retired, a renamed
    // display ID) reads as a dispute between two baseline values.
    // trace:BUG-2378 | ai:claude
    let behind: Vec<bool> = tips
        .iter()
        .map(|t| {
            tips.iter().any(|other| {
                t != other
                    && matches!(
                        git_status(repo, &["merge-base", "--is-ancestor", t, other]),
                        Ok((0, _, _))
                    )
            })
        })
        .collect();
    // A pruned tip is still a merge base of the tip that descends from it
    // (a fast-forward joins exactly that pair), so it stays as baseline:
    // a display ID two UUIDs first claim after it is still introduced.
    // trace:BUG-2378 | ai:claude
    let (pruned, kept): (Vec<(String, bool)>, Vec<(String, bool)>) = tips
        .into_iter()
        .zip(behind)
        .partition(|(_, is_behind)| *is_behind);
    let tips: Vec<String> = kept.into_iter().map(|(t, _)| t).collect();
    let pruned: Vec<String> = pruned.into_iter().map(|(t, _)| t).collect();
    // A sync that joins one distinct tip introduces nothing: that tip is its
    // own baseline (an unborn local, or local already equal to incoming).
    // A tip left alone only because the others were pruned is not that
    // case: the pruned tips are its baseline.
    let mut bases: Vec<String> = match tips.as_slice() {
        [only] if lone_tip_is_baseline && pruned.is_empty() => vec![only.clone()],
        _ => pruned,
    };
    let mut unrelated = false;
    for (i, a) in tips.iter().enumerate() {
        for b in tips.iter().skip(i + 1) {
            match git_status(repo, &["merge-base", "--all", a, b]) {
                Ok((0, out, _)) => {
                    for base in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
                        if !bases.iter().any(|x| x == base) {
                            bases.push(base.to_string());
                        }
                    }
                }
                Ok((1, out, true)) if out.is_empty() => unrelated = true,
                _ => report.gap(GapKind::GitRead, Some(a), None, None),
            }
        }
    }
    if unrelated {
        match git_status(repo, &["rev-parse", "--is-shallow-repository"]) {
            Ok((0, out, _)) if out == "false" => {}
            Ok((0, out, _)) if out == "true" => {
                report.gap(GapKind::MissingAncestry, None, None, None)
            }
            _ => report.gap(GapKind::GitRead, None, None, None),
        }
    }

    let mut loader = Loader::default();
    let mut listed: Vec<(String, Vec<Entry>, bool)> = Vec::new();
    for (commit, is_tip) in tips
        .iter()
        .map(|c| (c, true))
        .chain(bases.iter().map(|c| (c, false)))
    {
        match loader.list(repo, commit, &mut report) {
            Some(entries) => listed.push((commit.clone(), entries, is_tip)),
            None if is_tip => {}
            // A merge base that cannot be read leaves baseline unknown.
            None => report.gap(GapKind::MissingAncestry, Some(commit), None, None),
        }
    }
    // An object blob that is not present in every tree is new to someone:
    // its claims are read strictly, never only by the line scan.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (_, entries, _) in &listed {
        let mut once = HashSet::new();
        for e in entries.iter().filter(|e| e.class == PathClass::Objects) {
            if once.insert(e.oid.as_str()) {
                *seen.entry(e.oid.as_str()).or_default() += 1;
            }
        }
    }
    let strict: HashSet<String> = seen
        .into_iter()
        .filter(|(_, n)| *n != listed.len())
        .map(|(oid, _)| oid.to_string())
        .collect();
    let mut tip_trees = Vec::new();
    let mut base_trees = Vec::new();
    for (commit, entries, is_tip) in &listed {
        let tree = loader.tree(commit, entries, &strict, &mut report);
        if *is_tip {
            tip_trees.push(tree);
        } else {
            base_trees.push(tree);
        }
    }
    report.conflicts = analyze(&tip_trees, &base_trees);
    report.normalize();
    report
}

/// [`collect`] as a gate: Ok only for a complete, conflict-free report.
pub fn refuse_conflicts(
    repo: &Path,
    local: Option<&str>,
    incoming: &[String],
) -> anyhow::Result<()> {
    let report = collect(repo, local, incoming);
    if report.is_clean() {
        Ok(())
    } else {
        Err(anyhow::Error::new(AllocationRefusal(report)))
    }
}

// ---------------------------------------------------------------------------
// Pure collector
// ---------------------------------------------------------------------------

/// Whether a keyed claim's tip values conflict. `values` holds one entry per
/// tip that has the key; `baseline` every value any merge base holds.
///
/// Differing values conflict unless exactly one is new and the other equals a
/// merge base (one side changed it). An equal value introduced on two tips
/// independently conflicts only when `equal_new_conflicts`.
fn keyed_conflict<T: Ord + Clone>(
    values: &[T],
    baseline: &BTreeSet<T>,
    equal_new_conflicts: bool,
) -> bool {
    let distinct: BTreeSet<&T> = values.iter().collect();
    if distinct.len() >= 2 {
        let introduced = distinct.iter().filter(|v| !baseline.contains(**v)).count();
        return !(introduced == 1 && distinct.len() == 2);
    }
    equal_new_conflicts
        && values.len() >= 2
        && distinct
            .iter()
            .next()
            .is_some_and(|v| !baseline.contains(*v))
}

fn global_form(id: &str) -> Option<(&str, u32)> {
    let (prefix, n) = id.rsplit_once('-')?;
    let valid_prefix = prefix
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    (valid_prefix && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .then(|| n.parse().ok().map(|n| (prefix, n)))
        .flatten()
}

/// Every conflict among the claims of `tips`, with `bases` (all merge bases)
/// as the baseline. Deterministic for equal inputs.
pub(crate) fn analyze(tips: &[Tree], bases: &[Tree]) -> Vec<Conflict> {
    let mut out = Vec::new();
    let object_ref = |t: &Tree, uuid: Uuid, ordinal: usize| RecordRef {
        commit: t.commit.clone(),
        class: PathClass::Objects,
        ordinal,
        uuid: Some(uuid),
    };
    let row_ref = |t: &Tree, class: PathClass, ordinal: usize| RecordRef {
        commit: t.commit.clone(),
        class,
        ordinal,
        uuid: None,
    };

    // Display IDs: the set of owning UUIDs per tip.
    let display: BTreeSet<&String> = tips.iter().flat_map(|t| t.ids.keys()).collect();
    for id in &display {
        let values: Vec<BTreeSet<Uuid>> = tips
            .iter()
            .filter_map(|t| t.ids.get(*id).map(|m| m.keys().copied().collect()))
            .collect();
        let baseline: BTreeSet<BTreeSet<Uuid>> = bases
            .iter()
            .filter_map(|t| t.ids.get(*id).map(|m| m.keys().copied().collect()))
            .collect();
        let shared_in_tip = values.iter().any(|v| v.len() >= 2 && !baseline.contains(v));
        if shared_in_tip || keyed_conflict(&values, &baseline, false) {
            let refs = tips
                .iter()
                .flat_map(|t| {
                    t.ids
                        .get(*id)
                        .into_iter()
                        .flat_map(move |m| m.iter().map(move |(u, o)| object_ref(t, *u, *o)))
                })
                .collect();
            out.push(Conflict {
                kind: ConflictKind::DisplayId,
                refs,
                scope: ConflictScope::Introduced,
                key: format!("display:{id}"),
            });
        }
    }

    // Node bindings: one registration digest per tip.
    let node_ids: BTreeSet<&String> = tips.iter().flat_map(|t| t.nodes.keys()).collect();
    for id in node_ids {
        let values: Vec<String> = tips
            .iter()
            .filter_map(|t| t.nodes.get(id).map(|(d, _)| d.clone()))
            .collect();
        let baseline: BTreeSet<String> = bases
            .iter()
            .filter_map(|t| t.nodes.get(id).map(|(d, _)| d.clone()))
            .collect();
        if keyed_conflict(&values, &baseline, true) {
            let refs = tips
                .iter()
                .filter_map(|t| {
                    t.nodes
                        .get(id)
                        .map(|(_, o)| row_ref(t, PathClass::Nodes, *o))
                })
                .collect();
            out.push(Conflict {
                kind: ConflictKind::NodeBinding,
                refs,
                scope: ConflictScope::Introduced,
                key: format!("node:{id}"),
            });
        }
    }

    // Global ranges: the union of every tip's blocks, by key.
    type Key = (String, String, u32, u32);
    let mut union: BTreeMap<Key, (u32, Vec<RecordRef>)> = BTreeMap::new();
    let mut holders: BTreeMap<Key, usize> = BTreeMap::new();
    for t in tips {
        for b in &t.blocks {
            let slot = union.entry(b.key.clone()).or_insert((b.next, Vec::new()));
            slot.0 = slot.0.max(b.next);
            slot.1.push(row_ref(t, PathClass::Blocks, b.ordinal));
            *holders.entry(b.key.clone()).or_default() += 1;
        }
    }
    let base_keys: Vec<HashSet<&Key>> = bases
        .iter()
        .map(|t| t.blocks.iter().map(|b| &b.key).collect())
        .collect();
    let in_base = |k: &Key| base_keys.iter().any(|s| s.contains(k));
    let baseline_pair = |a: &Key, b: &Key| base_keys.iter().any(|s| s.contains(a) && s.contains(b));
    for (key, count) in &holders {
        if *count >= 2 && !in_base(key) {
            out.push(Conflict {
                kind: ConflictKind::GlobalRange,
                refs: union[key].1.clone(),
                scope: ConflictScope::Introduced,
                key: format!("range:{key:?}"),
            });
        }
    }
    let blocks: Vec<(&Key, &(u32, Vec<RecordRef>))> = union.iter().collect();
    for (i, (a, (a_next, a_refs))) in blocks.iter().enumerate() {
        for (b, (b_next, b_refs)) in blocks.iter().skip(i + 1) {
            let (a_star, b_star) = (a.1 == "*", b.1 == "*");
            // A `*` block only collides over the span it has dispensed.
            let span = |k: &Key, next: u32, star: bool| -> Option<(u32, u32)> {
                if star && (a_star != b_star) {
                    let end = next.saturating_sub(1).min(k.3);
                    (end >= k.2).then_some((k.2, end))
                } else {
                    Some((k.2, k.3))
                }
            };
            let collide = if a_star == b_star {
                a.1 == b.1 && a.0 != b.0
            } else {
                true
            };
            let (Some(sa), Some(sb)) = (span(a, *a_next, a_star), span(b, *b_next, b_star)) else {
                continue;
            };
            if collide && sa.0 <= sb.1 && sb.0 <= sa.1 && !baseline_pair(a, b) {
                out.push(Conflict {
                    kind: ConflictKind::GlobalRange,
                    refs: a_refs.iter().chain(b_refs.iter()).cloned().collect(),
                    scope: ConflictScope::Introduced,
                    key: format!("pair:{a:?}|{b:?}"),
                });
            }
        }
    }

    // Unissued global display IDs, judged against every recorded range and
    // counter in any tree. Only stores that keep a block registry issue
    // global IDs from ranges.
    if tips.iter().chain(bases).any(|t| t.has_blocks) {
        let mut ceiling: HashMap<String, u32> = HashMap::new();
        for t in tips.iter().chain(bases) {
            for b in &t.blocks {
                let c = ceiling.entry(b.key.1.clone()).or_default();
                *c = (*c).max(b.key.3);
            }
            for (prefix, n) in &t.counters {
                let c = ceiling.entry(prefix.clone()).or_default();
                *c = (*c).max(*n);
            }
        }
        let star = ceiling.get("*").copied().unwrap_or(0);
        for t in tips {
            for (id, owners) in &t.ids {
                let Some((prefix, n)) = global_form(id) else {
                    continue;
                };
                if n <= star.max(ceiling.get(prefix).copied().unwrap_or(0)) {
                    continue;
                }
                for (uuid, ordinal) in owners {
                    let baseline = bases
                        .iter()
                        .any(|b| b.ids.get(id).is_some_and(|m| m.contains_key(uuid)));
                    if !baseline {
                        out.push(Conflict {
                            kind: ConflictKind::Unissued,
                            refs: vec![object_ref(t, *uuid, *ordinal)],
                            scope: ConflictScope::Introduced,
                            key: format!("unissued:{id}:{uuid}"),
                        });
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "allocation_conflicts_tests.rs"]
mod tests;
