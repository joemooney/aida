//! STORY-2324 PR-A: the canonical spec-ID boundary.
//!
//! A requirement is known by several labels — its native `spec_id`, its
//! friendly `agreed_id`, node-label forms (`BUG-HOME-176` for
//! `BUG-<node>-176`) and IDs it was renumbered away from — and the gate
//! ledger is partitioned by whichever one a writer typed. The UUID is the one
//! identity that has never changed, so [`SpecKey`] is the UUID and the native
//! `spec_id` is only its display spelling.
//!
//! Identity comes from ONE committed snapshot of the store ([`Snapshot`]):
//! its HEAD commit's `objects/` and `registry/{node_aliases,retired_ids}.toml`
//! blobs, read through one bounded `git cat-file --batch`. The working tree
//! is never consulted, so uncommitted edits can neither change identity nor
//! block resolution. [`Snapshot::resolve`] accepts only exact,
//! case-insensitive hits on those mappings: no prefix, substring or UUID
//! fragment ever links two labels, a label two requirements claim is an
//! error, and a label nothing claims is reported as unresolved.
//!
//! PR-A only reads: nothing in the gate decides from this module yet. PR-B
//! makes the ledger's writers take a [`GateSpec`] and stamp its `spec_key`;
//! readers still decide by label until PR-C.
// trace:STORY-2324 | ai:claude
// mandate-gate

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use uuid::Uuid;

/// A requirement's canonical identity: its UUID.
// trace:STORY-2324 | ai:claude
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
// trace:STORY-2325 | ai:claude — ownership records persist the spec key.
pub(crate) struct SpecKey(pub Uuid);

impl std::fmt::Display for SpecKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// What the gate needs to know about one requirement, read from a snapshot.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalSpec {
    pub key: SpecKey,
    /// The native `spec_id`: the display spelling.
    pub native: String,
    pub friendly: Option<String>,
    pub mode: Option<aida_core::ExecutionMode>,
    /// Sorted.
    pub tags: Vec<String>,
    pub status: String,
    pub superseded_by: Option<SpecKey>,
    /// Every label the snapshot resolves to this requirement and to nothing
    /// else, upper-cased and sorted: what a ledger event's `spec` may say.
    pub labels: Vec<String>,
}

impl CanonicalSpec {
    /// Does `label` (any case) name this requirement?
    pub(crate) fn answers_to(&self, label: &str) -> bool {
        let up = label.trim().to_ascii_uppercase();
        self.labels.binary_search(&up).is_ok()
    }
}

/// The requirements one operation is about, resolved once.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CanonicalSet {
    /// The resolved members, one per key, sorted by key.
    pub members: Vec<CanonicalSpec>,
    /// Labels that named no requirement, upper-cased, sorted.
    pub unresolved: Vec<String>,
    /// Labels more than one requirement claims, with why.
    pub ambiguous: Vec<(String, String)>,
}

impl CanonicalSet {
    pub(crate) fn member(&self, key: SpecKey) -> Option<&CanonicalSpec> {
        self.members.iter().find(|m| m.key == key)
    }

    /// The member `label` names, if any.
    pub(crate) fn member_for_label(&self, label: &str) -> Option<&CanonicalSpec> {
        self.members.iter().find(|m| m.answers_to(label))
    }
}

/// A snapshot could not be read or does not prove identity: the caller must
/// not answer past it.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotError(pub String);

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "store snapshot: {}", self.0)
    }
}

/// How a label resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolution {
    Hit(SpecKey),
    Unknown,
    Ambiguous(String),
}

/// The committed node-label map, relative to the store root.
pub(crate) const NODE_ALIASES_REL: &str = "registry/node_aliases.toml";
/// The committed retired-ID map, relative to the store root.
pub(crate) const RETIRED_IDS_REL: &str = "registry/retired_ids.toml";
const OBJECTS_REL: &str = "objects";

/// Wall-clock bound on one snapshot git process.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Output bound on the tree listing.
const MAX_LISTING_BYTES: usize = 16 * 1024 * 1024;
/// Output bound on the one `cat-file --batch` read.
const MAX_BATCH_BYTES: usize = 512 * 1024 * 1024;
/// Object files one snapshot may hold.
const MAX_OBJECTS: usize = 200_000;
/// Node-label bindings one snapshot may hold.
const MAX_ALIAS_BINDINGS: usize = 1_024;
/// Retired IDs one snapshot may hold.
const MAX_RETIRED_IDS: usize = 65_536;

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct AliasFile {
    #[serde(default, rename = "binding")]
    bindings: Vec<AliasBinding>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AliasBinding {
    label: String,
    node: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RetiredFile {
    #[serde(default, rename = "retired")]
    entries: Vec<RetiredEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RetiredEntry {
    old: String,
    uuid: String,
}

/// `TYPE-MIDDLE-SEQ` with an alphabetic type and a numeric sequence.
fn split_id(id: &str) -> Option<(&str, &str, &str)> {
    let mut parts = id.trim().splitn(3, '-');
    let (prefix, middle, seq) = (parts.next()?, parts.next()?, parts.next()?);
    let ok = prefix
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && prefix.chars().all(|c| c.is_ascii_alphanumeric())
        && !middle.is_empty()
        && !seq.is_empty()
        && seq.bytes().all(|b| b.is_ascii_digit());
    ok.then_some((prefix, middle, seq))
}

fn norm(label: &str) -> String {
    label.trim().to_ascii_uppercase()
}

/// One committed store snapshot: its requirements and label mappings.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    /// The store commit read; empty for an in-memory snapshot.
    pub head: String,
    specs: BTreeMap<SpecKey, CanonicalSpec>,
    /// Literal native/friendly labels -> claimants.
    literal: BTreeMap<String, BTreeSet<SpecKey>>,
    /// Node-label forms and retired IDs -> claimants. Unioned with the
    /// literal claimants: a label two requirements claim is ambiguous.
    mapped: BTreeMap<String, BTreeSet<SpecKey>>,
}

impl Snapshot {
    /// Build a snapshot from requirements and the mapping files' bodies
    /// (`None`: not committed). Pure; the I/O half is [`Snapshot::at_head`].
    // trace:STORY-2324 | ai:claude
    pub(crate) fn from_parts(
        head: &str,
        reqs: &[aida_core::Requirement],
        node_aliases: Option<&str>,
        retired_ids: Option<&str>,
    ) -> Result<Self, SnapshotError> {
        let aliases: AliasFile = match node_aliases {
            Some(t) => {
                toml::from_str(t).map_err(|e| SnapshotError(format!("{NODE_ALIASES_REL}: {e}")))?
            }
            None => AliasFile::default(),
        };
        let retired: RetiredFile = match retired_ids {
            Some(t) => {
                toml::from_str(t).map_err(|e| SnapshotError(format!("{RETIRED_IDS_REL}: {e}")))?
            }
            None => RetiredFile::default(),
        };
        if aliases.bindings.len() > MAX_ALIAS_BINDINGS || retired.entries.len() > MAX_RETIRED_IDS {
            return Err(SnapshotError(
                "the committed label mappings exceed their bounds".into(),
            ));
        }
        let mut snap = Snapshot {
            head: head.to_string(),
            ..Default::default()
        };
        let ids: BTreeSet<Uuid> = reqs.iter().map(|r| r.id).collect();
        for r in reqs {
            let key = SpecKey(r.id);
            if snap.specs.contains_key(&key) {
                return Err(SnapshotError(format!(
                    "two objects carry the requirement UUID {key}"
                )));
            }
            let native = r.spec_id.clone().unwrap_or_default();
            let superseded_by = r
                .relationships
                .iter()
                .find(|rel| rel.rel_type == aida_core::RelationshipType::SupersededBy)
                .map(|rel| rel.target_id)
                .filter(|t| ids.contains(t))
                .map(SpecKey);
            let mut tags: Vec<String> = r.tags.iter().cloned().collect();
            tags.sort();
            for label in [Some(native.as_str()), r.agreed_id.as_deref()]
                .into_iter()
                .flatten()
                .map(norm)
                .filter(|l| !l.is_empty())
            {
                snap.literal.entry(label).or_default().insert(key);
            }
            if let Some((prefix, node, seq)) = split_id(&native) {
                for b in aliases
                    .bindings
                    .iter()
                    .filter(|b| b.node.trim().eq_ignore_ascii_case(node))
                {
                    let form = norm(&format!("{prefix}-{}-{seq}", b.label.trim()));
                    snap.mapped.entry(form).or_default().insert(key);
                }
            }
            snap.specs.insert(
                key,
                CanonicalSpec {
                    key,
                    native,
                    friendly: r.agreed_id.clone(),
                    mode: r.execution_mode,
                    tags,
                    status: r.status.to_string(),
                    superseded_by,
                    labels: Vec::new(),
                },
            );
        }
        for e in &retired.entries {
            let Ok(uuid) = Uuid::parse_str(e.uuid.trim()) else {
                return Err(SnapshotError(format!(
                    "{RETIRED_IDS_REL}: {:?} is not a UUID",
                    e.uuid
                )));
            };
            if ids.contains(&uuid) && !norm(&e.old).is_empty() {
                snap.mapped
                    .entry(norm(&e.old))
                    .or_default()
                    .insert(SpecKey(uuid));
            }
        }
        // A label belongs to a requirement only when it resolves to it alone.
        let mut labels: BTreeMap<SpecKey, BTreeSet<String>> = BTreeMap::new();
        for label in snap.literal.keys().chain(snap.mapped.keys()) {
            if let Resolution::Hit(k) = snap.resolve_one(label) {
                labels.entry(k).or_default().insert(label.clone());
            }
        }
        for (k, ls) in labels {
            if let Some(spec) = snap.specs.get_mut(&k) {
                spec.labels = ls.into_iter().collect();
            }
        }
        Ok(snap)
    }

    /// One label: its literal native/friendly claimants together with its
    /// node-label and retired-ID claimants. Exact, case-insensitive text
    /// only. A label literally claimed by one requirement but mapped to
    /// another is ambiguous (STORY-2324 sketch sections 2 and 8): no claim
    /// outranks another.
    // trace:STORY-2324 | ai:claude
    pub(crate) fn resolve_one(&self, label: &str) -> Resolution {
        let label = norm(label);
        let hits: BTreeSet<SpecKey> = self
            .literal
            .get(&label)
            .into_iter()
            .chain(self.mapped.get(&label))
            .flatten()
            .copied()
            .collect();
        match hits.iter().collect::<Vec<_>>().as_slice() {
            [] => Resolution::Unknown,
            [one] => Resolution::Hit(**one),
            many => Resolution::Ambiguous(format!("{label} names {} requirements", many.len())),
        }
    }

    pub(crate) fn spec(&self, key: SpecKey) -> Option<&CanonicalSpec> {
        self.specs.get(&key)
    }

    /// Resolve every label once. A change key (`PR-N`) or a blank label is
    /// not a spec label and is skipped.
    // trace:STORY-2324 | ai:claude
    pub(crate) fn resolve<S: AsRef<str>>(&self, labels: &[S]) -> CanonicalSet {
        let mut keys: BTreeSet<SpecKey> = BTreeSet::new();
        let mut set = CanonicalSet::default();
        for raw in labels {
            let label = norm(raw.as_ref());
            if label.is_empty()
                || label == "???"
                || crate::gate::ledger::ChangeKey::parse(&label).is_some()
            {
                continue;
            }
            match self.resolve_one(&label) {
                Resolution::Hit(k) => {
                    keys.insert(k);
                }
                Resolution::Unknown => {
                    if !set.unresolved.contains(&label) {
                        set.unresolved.push(label);
                    }
                }
                Resolution::Ambiguous(why) => {
                    if !set.ambiguous.iter().any(|(l, _)| *l == label) {
                        set.ambiguous.push((label, why));
                    }
                }
            }
        }
        set.unresolved.sort();
        set.ambiguous.sort();
        set.members = keys
            .into_iter()
            .filter_map(|k| self.specs.get(&k).cloned())
            .collect();
        set
    }

    /// The committed snapshot of the distributed store `project_root` uses,
    /// at its HEAD. `Ok(None)` when the project has no git-canonical store.
    // trace:STORY-2324 | ai:claude
    pub(crate) fn at_head(project_root: &Path) -> Result<Option<Self>, SnapshotError> {
        match aida_core::store_locate::detect_distributed_store_from(project_root) {
            Some(store) => Self::of_store(&store).map(Some),
            None => Ok(None),
        }
    }

    /// The committed snapshot of the store repository `store` at its HEAD.
    // trace:STORY-2324 | ai:claude
    pub(crate) fn of_store(store: &Path) -> Result<Self, SnapshotError> {
        let store = store.to_path_buf();
        let top = git_bounded(&store, &["rev-parse", "--show-toplevel"], None, 4096)?;
        let top = std::path::PathBuf::from(String::from_utf8_lossy(&top).trim());
        let same = match (top.canonicalize(), store.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            return Err(SnapshotError(format!(
                "{} is not the top of its own git repository",
                store.display()
            )));
        }
        let head = git_bounded(
            &store,
            &["rev-parse", "--verify", "HEAD^{commit}"],
            None,
            256,
        )?;
        let head = String::from_utf8(head)
            .map_err(|_| SnapshotError("a non-text HEAD".into()))?
            .trim()
            .to_string();
        if head.len() < 40 || !head.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(SnapshotError("an unreadable HEAD".into()));
        }
        let listing = git_bounded(
            &store,
            &[
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                &head,
                "--",
                OBJECTS_REL,
                NODE_ALIASES_REL,
                RETIRED_IDS_REL,
            ],
            None,
            MAX_LISTING_BYTES,
        )?;
        let entries = parse_listing(&listing)?;
        let mut input = String::new();
        for (oid, _) in &entries {
            input.push_str(oid);
            input.push('\n');
        }
        let batch = git_bounded(
            &store,
            &["cat-file", "--batch"],
            Some(input.into_bytes()),
            MAX_BATCH_BYTES,
        )?;
        let blobs = parse_batch(&batch, &entries)?;
        let mut reqs = Vec::new();
        let mut aliases = None;
        let mut retired = None;
        for ((_, path), body) in entries.iter().zip(blobs) {
            let text = std::str::from_utf8(body)
                .map_err(|_| SnapshotError(format!("{path} is not UTF-8")))?;
            match path.as_str() {
                NODE_ALIASES_REL => aliases = Some(text),
                RETIRED_IDS_REL => retired = Some(text),
                _ => reqs.push(
                    serde_yaml::from_str::<aida_core::Requirement>(text)
                        .map_err(|e| SnapshotError(format!("{path} does not parse: {e}")))?,
                ),
            }
        }
        Self::from_parts(&head, &reqs, aliases, retired)
    }
}

/// `(oid, path)` for every requirement object (`objects/TYPE/SHARD/X.yaml`)
/// and mapping file in a `ls-tree -r -z` listing. A non-blob at one of
/// those paths refuses.
fn parse_listing(listing: &[u8]) -> Result<Vec<(String, String)>, SnapshotError> {
    let mut out = Vec::new();
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = std::str::from_utf8(entry)
            .map_err(|_| SnapshotError("a non-UTF-8 tree entry".into()))?;
        let (meta, path) = entry
            .split_once('\t')
            .ok_or_else(|| SnapshotError(format!("an unreadable tree entry {entry:?}")))?;
        let mut meta = meta.split(' ');
        let (_mode, kind, oid) = (meta.next(), meta.next(), meta.next());
        let is_object =
            path.starts_with("objects/") && path.ends_with(".yaml") && path.split('/').count() == 4;
        let is_map = path == NODE_ALIASES_REL || path == RETIRED_IDS_REL;
        if !is_object && !is_map {
            continue;
        }
        let (Some("blob"), Some(oid)) = (kind, oid) else {
            return Err(SnapshotError(format!("{path} is not a committed file")));
        };
        if oid.is_empty() || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(SnapshotError(format!("{path} has an unreadable object id")));
        }
        out.push((oid.to_string(), path.to_string()));
        if out.len() > MAX_OBJECTS {
            return Err(SnapshotError(format!(
                "the store holds more than {MAX_OBJECTS} objects"
            )));
        }
    }
    Ok(out)
}

/// Split `cat-file --batch` output into one body per entry, in order. Each
/// answer must name the requested blob; a missing or truncated one refuses.
fn parse_batch<'a>(
    out: &'a [u8],
    entries: &[(String, String)],
) -> Result<Vec<&'a [u8]>, SnapshotError> {
    let mut rest = out;
    let mut bodies = Vec::with_capacity(entries.len());
    for (oid, path) in entries {
        let nl = rest
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| SnapshotError(format!("the batch read ended before {path}")))?;
        let header = std::str::from_utf8(&rest[..nl])
            .map_err(|_| SnapshotError(format!("an unreadable batch header for {path}")))?;
        let mut parts = header.split(' ');
        let (got, kind, size) = (parts.next(), parts.next(), parts.next());
        let size: usize = match (got, kind, size) {
            (Some(g), Some("blob"), Some(s)) if g == oid => s
                .parse()
                .map_err(|_| SnapshotError(format!("an unreadable size for {path}")))?,
            _ => {
                return Err(SnapshotError(format!(
                    "the batch read did not return {path}'s blob ({header})"
                )))
            }
        };
        let start = nl + 1;
        let end = start
            .checked_add(size)
            .filter(|e| *e < rest.len() && rest[*e] == b'\n')
            .ok_or_else(|| SnapshotError(format!("the batch read truncated {path}")))?;
        bodies.push(&rest[start..end]);
        rest = &rest[end + 1..];
    }
    Ok(bodies)
}

/// One bounded, config-independent git process in `store`: `input` fed on
/// stdin, stdout capped at `cap` bytes, killed past [`GIT_TIMEOUT`]; a
/// non-zero exit refuses.
fn git_bounded(
    store: &Path,
    args: &[&str],
    input: Option<Vec<u8>>,
    cap: usize,
) -> Result<Vec<u8>, SnapshotError> {
    use std::io::{Read as _, Write as _};
    let fail = |why: String| SnapshotError(format!("git {}: {why}", args[0]));
    let mut cmd = aida_core::git_ops::host_git_command(store);
    cmd.args(["-c", "core.quotePath=false"])
        .args(args)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| fail(e.to_string()))?;
    let writer = match (input, child.stdin.take()) {
        (Some(bytes), Some(mut stdin)) => Some(std::thread::spawn(move || {
            // A refused write (git exited early) shows up as its exit status.
            let _ = stdin.write_all(&bytes);
        })),
        _ => None,
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("no stdout".into()))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout
            .take(cap as u64 + 1)
            .read_to_end(&mut buf)
            .map(|_| buf)
    });
    let deadline = std::time::Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| fail(e.to_string()))? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail("timed out".into()));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    let out = reader
        .join()
        .map_err(|_| fail("reader panicked".into()))?
        .map_err(|e| fail(e.to_string()))?;
    if out.len() > cap {
        let _ = child.kill();
        return Err(fail("output exceeds its bound".into()));
    }
    if !status.success() {
        return Err(fail(format!("exited {status}")));
    }
    Ok(out)
}

/// STORY-2324 PR-B: a spec label as a gate-ledger writer received it,
/// together with the canonical key it resolves to. Writers take this, not a
/// bare string, so the label is resolved once where it enters (E1-E4) and
/// every event and hold it reaches carries the same `spec_key`.
///
/// `key` is `None` when the label names no requirement in the committed
/// store snapshot (or the project has none): the label is still written as
/// the writer typed it, so nothing about today's decision changes, and
/// `aida gate migrate` reports it.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateSpec {
    /// Upper-cased, trimmed: the spelling today's readers index by.
    pub label: String,
    pub key: Option<SpecKey>,
}

impl GateSpec {
    /// `label` with an already-known key (or none).
    pub(crate) fn with_key(label: &str, key: Option<SpecKey>) -> Self {
        Self {
            label: norm(label),
            key,
        }
    }

    /// Resolve `label` against the committed snapshot of the store the
    /// project at `start` uses. Ambiguous and unknown labels carry no key.
    // trace:STORY-2324 | ai:claude
    pub(crate) fn resolve(start: &Path, label: &str) -> Self {
        Self::resolve_in(writer_snapshot(start).as_deref(), label)
    }

    /// [`GateSpec::resolve`] over several labels against one snapshot,
    /// located (and its HEAD probed) once.
    // trace:STORY-2324 | ai:claude
    pub(crate) fn resolve_all<S: AsRef<str>>(start: &Path, labels: &[S]) -> Vec<Self> {
        let snap = writer_snapshot(start);
        labels
            .iter()
            .map(|l| Self::resolve_in(snap.as_deref(), l.as_ref()))
            .collect()
    }

    fn resolve_in(snap: Option<&Snapshot>, label: &str) -> Self {
        let key = snap.and_then(|snap| match snap.resolve_one(label) {
            Resolution::Hit(k) => Some(k),
            Resolution::Unknown | Resolution::Ambiguous(_) => None,
        });
        Self::with_key(label, key)
    }
}

/// The committed snapshot writers resolve against, memoised per store and
/// store HEAD: a process that writes several events reads the store once,
/// and a store that moved is read afresh. `None` when the project has no
/// git-canonical store or its snapshot does not read (said once per
/// process): the writer then records the label without a key.
// trace:STORY-2324 | ai:claude
fn writer_snapshot(start: &Path) -> Option<std::sync::Arc<Snapshot>> {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, Arc<Snapshot>>>> = OnceLock::new();
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let store = aida_core::store_locate::detect_distributed_store_from(start)?;
    let head = git_bounded(
        &store,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        None,
        256,
    )
    .ok()
    .map(|h| String::from_utf8_lossy(&h).trim().to_string())?;
    let memo = MEMO.get_or_init(Default::default);
    if let Some(hit) = memo.lock().ok().and_then(|m| m.get(&store).cloned()) {
        if hit.head == head {
            return Some(hit);
        }
    }
    match Snapshot::of_store(&store) {
        Ok(snap) => {
            let snap = Arc::new(snap);
            if let Ok(mut m) = memo.lock() {
                m.insert(store, snap.clone());
            }
            Some(snap)
        }
        Err(e) => {
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("warning: gate ledger writes carry no canonical spec key: {e}");
            }
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/story_2324_canon_tests.rs"]
mod tests;
