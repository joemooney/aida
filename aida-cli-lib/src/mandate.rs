//! `aida mandate` CLI orchestration, display, and store-history helpers.
// trace:STORY-1698 trace:STORY-1889 | ai:claude

use crate::gate::mandate::*;
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use std::path::Path;

// trace:STORY-1728 | ai:claude
#[path = "mandate_args.rs"]
pub(crate) mod mandate_args;
pub(crate) use mandate_args::*;

// trace:BUG-2554 | ai:claude
#[path = "mandate_history_index.rs"]
pub(crate) mod history_index;

/// One historical object body, read leniently: the full requirement when it
/// parses, else just the fields exclusion looks at (an older schema). `Err`
/// only when it is not YAML at all.
// trace:STORY-1698 | ai:claude
fn parse_version(text: &str) -> std::result::Result<aida_core::Requirement, String> {
    if let Ok(req) = serde_yaml::from_str::<aida_core::Requirement>(text) {
        return Ok(req);
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| format!("unparseable version: {e}"))?;
    let field = |k: &str| {
        value
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut req = aida_core::Requirement::new(field("title"), String::new());
    if let Some(tags) = value.get("tags").and_then(|v| v.as_sequence()) {
        req.tags = tags
            .iter()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect();
    }
    req.execution_mode = field("execution_mode").parse().ok();
    if let Some(t) = value
        .get("type")
        .and_then(|t| serde_yaml::from_value(t.clone()).ok())
    {
        req.req_type = t;
    }
    Ok(req)
}

/// BUG-2704: the most resolutions one conflicted version expands to before
/// it is refused as unreadable (fail closed).
// trace:BUG-2704 | ai:claude
const MAX_CONFLICT_RESOLUTIONS: usize = 256;

/// BUG-2704: one historical object body as the versions exclusion judges.
/// A body holding git conflict markers (a stash re-applied in the store
/// worktree and committed, as in store commit 7124c0f) is every resolution
/// of it: each hunk taken as ours, as the diff3 base when recorded, or as
/// theirs, in every combination, so a class one side's fields carry — or
/// only a mix of two hunks' sides makes — is still seen. Every resolution is
/// judged and any exclusion found in one counts. `Err` (fail closed) when
/// the markers are malformed, nested or unterminated, the combinations
/// exceed [`MAX_CONFLICT_RESOLUTIONS`], or any resolution does not parse.
// trace:BUG-2704 | ai:claude
fn parse_versions(text: &str) -> std::result::Result<Vec<aida_core::Requirement>, String> {
    let resolutions = match conflict_resolutions(text) {
        None => return parse_version(text).map(|req| vec![req]),
        Some(resolutions) => {
            resolutions.map_err(|e| format!("unparseable version: conflict markers: {e}"))?
        }
    };
    let total = resolutions.len();
    resolutions
        .iter()
        .enumerate()
        .map(|(i, side)| {
            parse_version(side)
                .map_err(|e| format!("conflicted version, resolution {} of {total}: {e}", i + 1))
        })
        .collect()
}

/// BUG-2704: every resolution of a body holding git conflict markers
/// (recognised by the store writers' own [`aida_core::git_ops::conflict_marker`]),
/// or `None` when it holds none. Lines outside a hunk belong to every
/// resolution; line terminators are kept as written.
// trace:BUG-2704 | ai:claude
fn conflict_resolutions(text: &str) -> Option<std::result::Result<Vec<String>, String>> {
    use aida_core::git_ops::conflict_marker;
    if !text.lines().any(|line| conflict_marker(line).is_some()) {
        return None;
    }
    #[derive(Clone, Copy, PartialEq)]
    enum At {
        Outside,
        Ours,
        Base,
        Theirs,
    }
    let mut state = At::Outside;
    let mut resolutions = vec![String::new()];
    let mut sides: Vec<String> = Vec::new();
    for (n, raw) in text.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        match (state, conflict_marker(line)) {
            (At::Outside, None) => resolutions.iter_mut().for_each(|r| r.push_str(raw)),
            // Inside a hunk `sides` always holds the side being read.
            (_, None) => sides
                .iter_mut()
                .last()
                .into_iter()
                .for_each(|side| side.push_str(raw)),
            (At::Outside, Some("<<<<<<<")) => {
                sides = vec![String::new()];
                state = At::Ours;
            }
            (At::Ours, Some("|||||||")) => {
                sides.push(String::new());
                state = At::Base;
            }
            (At::Ours | At::Base, Some("=======")) => {
                sides.push(String::new());
                state = At::Theirs;
            }
            (At::Theirs, Some(">>>>>>>")) => {
                if resolutions.len() * sides.len() > MAX_CONFLICT_RESOLUTIONS {
                    return Some(Err(format!(
                        "more than {MAX_CONFLICT_RESOLUTIONS} resolutions"
                    )));
                }
                let mut next: Vec<String> = resolutions
                    .iter()
                    .flat_map(|r| sides.iter().map(move |side| format!("{r}{side}")))
                    .collect();
                next.sort_unstable();
                next.dedup();
                resolutions = next;
                state = At::Outside;
            }
            (_, Some(marker)) => {
                return Some(Err(format!("unexpected {marker} at line {}", n + 1)));
            }
        }
    }
    if state != At::Outside {
        return Some(Err("a conflict hunk is never closed".into()));
    }
    Some(Ok(resolutions))
}

/// One git invocation in the store repository; `Err` carries git's stderr.
// trace:BUG-2179 | ai:claude
fn store_git(
    store_root: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> std::result::Result<Vec<u8>, String> {
    use std::io::{Seek, Write};
    let mut cmd = aida_core::git_ops::host_git_command(store_root);
    // TASK-2067 (advisor C1): no system config, and no path quoting, decide
    // what a history walk reports. trace:TASK-2067 | ai:claude
    // TASK-2238: nor do refs/replace/* objects, as for the snapshot queries.
    // trace:TASK-2238 | ai:claude
    cmd.args(["-c", "core.quotePath=false"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
    // trace:BUG-2572 | ai:codex
    // File-backed stdin avoids a pipe writer that could block before the
    // bounded runner starts, or survive its deadline waiting for a reader.
    if let Some(input) = stdin {
        let mut file = tempfile::tempfile().map_err(|e| format!("git input file: {e}"))?;
        file.write_all(input)
            .map_err(|e| format!("git input write: {e}"))?;
        file.rewind()
            .map_err(|e| format!("git input rewind: {e}"))?;
        cmd.stdin(std::process::Stdio::from(file));
    }
    let budget = store_history_timeout();
    match crate::command_output_with_timeout_detail(cmd, budget) {
        crate::BoundedCommandOutput::Completed(out) => git_output(out),
        crate::BoundedCommandOutput::SpawnFailed => Err("git failed to run".to_string()),
        crate::BoundedCommandOutput::TimedOut => Err(format!(
            "store history read timed out after {}s; raise {STORE_HISTORY_TIMEOUT_ENV} \
             (seconds) for a large or cold store",
            budget.as_secs()
        )),
    }
}

const STORE_HISTORY_TIMEOUT_ENV: &str = "AIDA_STORE_HISTORY_TIMEOUT_SECS";

/// The deadline for one store-history git walk:
/// `AIDA_STORE_HISTORY_TIMEOUT_SECS`, else 600 s. TASK-2141: the default sits
/// above the measured cold walk of the live store (4m26s, TASK-1662), so a
/// cold cache does not refuse a ship; the deadline still bounds a hung walk.
// trace:TASK-1951 trace:TASK-1662 trace:TASK-2141 | ai:claude
fn store_history_timeout() -> std::time::Duration {
    let secs = std::env::var(STORE_HISTORY_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(600);
    std::time::Duration::from_secs(secs)
}

fn git_output(out: std::process::Output) -> std::result::Result<Vec<u8>, String> {
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// BUG-2179: the store history the mandate exclusion check reads, gathered in
/// a bounded number of git invocations however many specs a PR traces: one
/// `git log --all` walk over every spec's object path, one `git cat-file
/// --batch` for every stored version, and (only when some ID has no current
/// record) one `-G` walk for the aliases. Strictness is the per-spec walks':
/// every ref (`--all`), every version that added or changed the object,
/// and fail-closed per spec when a version cannot be read.
// trace:BUG-2179 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct StoreHistoryBatch {
    /// Upper-cased spec ID → every stored version, each expanded by its
    /// recorded field changes; `Err` when one of its versions is unreadable.
    versions: std::collections::BTreeMap<
        String,
        std::result::Result<Vec<aida_core::Requirement>, String>,
    >,
}

impl StoreHistoryBatch {
    /// Every stored version of `spec_id` (`Ok(empty)` when no ref ever held it).
    pub(crate) fn versions(
        &self,
        spec_id: &str,
    ) -> std::result::Result<Vec<aida_core::Requirement>, String> {
        self.versions
            .get(&spec_id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_else(|| Err(format!("{spec_id} was not part of the history read")))
    }

    /// Read every version of each of `spec_ids`, listed from the
    /// store-history index (BUG-2554). `Err` when the store history cannot be
    /// walked at all.
    // trace:BUG-2554 | ai:claude
    pub(crate) fn read(
        store_root: &Path,
        spec_ids: &[String],
    ) -> std::result::Result<Self, String> {
        Self::read_listed(store_root, spec_ids, |paths| {
            let index = history_index::HistoryIndex::current(store_root)?;
            let mut wanted = Vec::new();
            for (key, rel) in paths {
                for sha in index.versions.get(rel).into_iter().flatten() {
                    wanted.push((key.clone(), format!("{sha}:{rel}")));
                }
            }
            Ok(wanted)
        })
    }

    /// [`Self::read`] by a full `git log --all` walk: the pre-index reading,
    /// kept as the reference the index must equal.
    // trace:BUG-2554 | ai:claude
    #[cfg(test)]
    pub(crate) fn read_full_walk(
        store_root: &Path,
        spec_ids: &[String],
    ) -> std::result::Result<Self, String> {
        Self::read_listed(store_root, spec_ids, |paths| {
            full_walk_versions(store_root, paths)
        })
    }

    /// Resolve each of `spec_ids` to its object path, `list` every stored
    /// version as `(key, "<sha>:<path>")` newest first, then read them all in
    /// one `git cat-file --batch`.
    fn read_listed(
        store_root: &Path,
        spec_ids: &[String],
        list: impl FnOnce(&[(String, String)]) -> std::result::Result<Vec<(String, String)>, String>,
    ) -> std::result::Result<Self, String> {
        let mut batch = Self::default();
        let mut paths: Vec<(String, String)> = Vec::new();
        for id in spec_ids {
            let key = id.to_ascii_uppercase();
            if batch.versions.contains_key(&key) {
                continue;
            }
            match aida_core::object_store::relative_object_path(id) {
                Ok(rel) => {
                    batch.versions.insert(key.clone(), Ok(Vec::new()));
                    paths.push((key, rel));
                }
                Err(e) => {
                    batch
                        .versions
                        .insert(key, Err(format!("no object path for {id}: {e}")));
                }
            }
        }
        if paths.is_empty() {
            return Ok(batch);
        }
        let wanted = list(&paths)?;
        if wanted.is_empty() {
            return Ok(batch);
        }
        let input: String = wanted.iter().map(|(_, obj)| format!("{obj}\n")).collect();
        let bodies = store_git(store_root, &["cat-file", "--batch"], Some(input.as_bytes()))?;
        let mut rest: &[u8] = &bodies;
        for (key, obj) in &wanted {
            let (body, tail) = next_batch_object(rest)
                .map_err(|e| format!("cannot read {obj} from the store: {e}"))?;
            rest = tail;
            let Some(Ok(versions)) = batch.versions.get_mut(key) else {
                continue;
            };
            let spec = obj.split_once(':').map_or(obj.as_str(), |(sha, _)| sha);
            // BUG-2704: a conflicted version is every resolution of it.
            // trace:BUG-2704 | ai:claude
            let parsed = body
                .ok_or_else(|| format!("cannot read {key} at {spec}: missing object"))
                .and_then(|text| {
                    parse_versions(&String::from_utf8_lossy(text))
                        .map_err(|e| format!("{key} at {spec}: {e}"))
                });
            match parsed {
                Ok(reqs) => {
                    for req in reqs {
                        versions.extend(field_change_versions(&req));
                        versions.push(req);
                    }
                }
                Err(e) => {
                    batch.versions.insert(key.clone(), Err(e));
                }
            }
        }
        Ok(batch)
    }
}

/// BUG-2179: every stored version of each `(key, path)` by one full
/// `git log --all` walk, as `(key, "<sha>:<path>")` newest first. The
/// reference reading the BUG-2554 index must equal.
// trace:BUG-2179 trace:BUG-2554 | ai:claude
#[cfg(test)]
fn full_walk_versions(
    store_root: &Path,
    paths: &[(String, String)],
) -> std::result::Result<Vec<(String, String)>, String> {
    // Deletions carry no body to read; every other touching commit does.
    // The walk is limited by each path's top directory, not by every path:
    // git checks each pathspec against every commit, so a PR tracing ~100
    // specs took ~50 s over a ~50k-commit store, while one directory
    // pathspec takes seconds whatever the spec count. The names it lists
    // are filtered to the wanted paths below.
    let mut tops: Vec<&str> = paths
        .iter()
        .map(|(_, rel)| rel.split_once('/').map_or(rel.as_str(), |(top, _)| top))
        .collect();
    tops.sort_unstable();
    tops.dedup();
    // TASK-2067 (advisor C1): renames off explicitly, so the listed names
    // never depend on diff.renames or diff.renameLimit; a renamed-to path
    // is listed as an addition. trace:TASK-2067 | ai:claude
    let mut args: Vec<&str> = vec![
        "log",
        "--all",
        "--format=%x00%H",
        "--no-renames",
        "--diff-filter=ACMRT",
        "--name-only",
        "--",
    ];
    args.extend(tops);
    let log = store_git(store_root, &args, None)?;
    let by_path: std::collections::BTreeMap<&str, &str> = paths
        .iter()
        .map(|(key, rel)| (rel.as_str(), key.as_str()))
        .collect();
    // `<sha>:<path>` per version, in log order (newest first).
    let mut wanted: Vec<(String, String)> = Vec::new();
    for record in String::from_utf8_lossy(&log).split('\0').skip(1) {
        let mut lines = record.lines().map(str::trim).filter(|l| !l.is_empty());
        let Some(sha) = lines.next() else { continue };
        for file in lines {
            if let Some(key) = by_path.get(file) {
                wanted.push((key.to_string(), format!("{sha}:{file}")));
            }
        }
    }
    Ok(wanted)
}

/// One `git cat-file --batch` reply: `(Some(body), rest)` for a found object,
/// `(None, rest)` for a missing one.
// trace:BUG-2179 | ai:claude
fn next_batch_object(out: &[u8]) -> std::result::Result<(Option<&[u8]>, &[u8]), String> {
    let nl = out
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("truncated git cat-file reply")?;
    let header = String::from_utf8_lossy(&out[..nl]);
    let rest = &out[nl + 1..];
    if header.ends_with(" missing") || header.ends_with(" ambiguous") {
        return Ok((None, rest));
    }
    let size: usize = header
        .rsplit(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("unexpected git cat-file header {header:?}"))?;
    if rest.len() < size + 1 {
        return Err("truncated git cat-file body".into());
    }
    Ok((Some(&rest[..size]), &rest[size + 1..]))
}

/// STORY-1698 / TASK-1800: every version of `spec_id` in the store's git
/// history (all refs, so a rewritten local branch still shows what the remote
/// saw), each expanded by its recorded field changes. `Ok(empty)` when the
/// store repository is readable and no ref ever held `spec_id`; `Err` when
/// git history cannot be read or a stored version cannot be parsed.
// trace:STORY-1698 | ai:claude
// trace:TASK-1800 | ai:antigravity
pub(crate) fn store_history_versions_optional(
    store_root: &Path,
    spec_id: &str,
) -> std::result::Result<Vec<aida_core::Requirement>, String> {
    StoreHistoryBatch::read(store_root, &[spec_id.to_string()])?.versions(spec_id)
}

/// TASK-1800 / BUG-2179: for each of `ids`, the stored object IDs whose YAML
/// carried it as its `spec_id` or `agreed_id` in any version on any ref, other
/// than the ID itself. This is how a DELETED spec named by its agreed_id
/// (stored under a different spec_id path) is still found and judged. One
/// `-G` walk serves every ID; each matching added or removed line is
/// attributed to the ID it names. `Err` when an ID is not searchable, the
/// history cannot be searched, or a match is in a file that is not at its
/// canonical object path (whose versions the history read cannot find); the
/// caller refuses rather than treating an ID as dangling.
// trace:TASK-1800 | ai:claude
// trace:BUG-2179 | ai:claude
// trace:BUG-2554 | ai:claude
pub(crate) fn store_history_alias_map(
    store_root: &Path,
    ids: &[String],
) -> std::result::Result<std::collections::BTreeMap<String, Vec<String>>, String> {
    let mut found = searchable_ids(ids)?;
    if found.is_empty() {
        return Ok(found);
    }
    // BUG-2554: answered from the store-history index, which walks only
    // commits it has not seen; a doubtful index rebuilds by a full walk.
    let index = history_index::HistoryIndex::current(store_root)
        .map_err(|e| format!("cannot search store history for {}: {e}", ids.join(", ")))?;
    for (id, aliases) in found.iter_mut() {
        let Some(hit) = index.aliases.get(id) else {
            continue;
        };
        if let Some(refusal) = &hit.refusal {
            return Err(refusal.clone());
        }
        aliases.extend(hit.stems.iter().filter(|stem| *stem != id).cloned());
    }
    Ok(found)
}

/// Each of `ids`, upper-cased, mapped to no aliases yet; `Err` for an ID
/// that is not a plain spec-ID token (and so cannot be searched for).
// trace:TASK-1800 | ai:claude
fn searchable_ids(
    ids: &[String],
) -> std::result::Result<std::collections::BTreeMap<String, Vec<String>>, String> {
    let mut found: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for id in ids {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("cannot search store history for {id:?}"));
        }
        found.insert(id.to_ascii_uppercase(), Vec::new());
    }
    Ok(found)
}

/// [`store_history_alias_map`] by one full `-G` walk for just `ids`: the
/// pre-index reading, kept as the reference the index must equal.
// trace:TASK-1800 trace:BUG-2179 trace:BUG-2554 | ai:claude
#[cfg(test)]
pub(crate) fn store_history_alias_map_full_walk(
    store_root: &Path,
    ids: &[String],
) -> std::result::Result<std::collections::BTreeMap<String, Vec<String>>, String> {
    let mut found = searchable_ids(ids)?;
    if found.is_empty() {
        return Ok(found);
    }
    let alternation = found.keys().cloned().collect::<Vec<_>>().join("|");
    let pattern = format!("^(spec_id|agreed_id): *['\"]?({alternation})['\"]? *$");
    let hits = alias_walk(store_root, &["--all"], &pattern)
        .map_err(|e| format!("cannot search store history for {}: {e}", ids.join(", ")))?;
    for (id, hit) in hits {
        let stems = hit?;
        if let Some(aliases) = found.get_mut(&id) {
            for stem in stems {
                if stem != id && !aliases.contains(&stem) {
                    aliases.push(stem);
                }
            }
        }
    }
    Ok(found)
}

/// One `-G` walk over the commits `range` selects, for `pattern` (whose
/// second group is the ID): every `spec_id` / `agreed_id` line a version
/// added or removed, in log order, as `(ID, stems)` (the canonical object
/// stems that carried it, both sides of a rename) or `(ID, Err(refusal))`
/// for a hit whose versions the history read could not find. `Err` when the
/// history cannot be searched.
// trace:TASK-1800 trace:BUG-2179 trace:BUG-2554 | ai:claude
pub(crate) fn alias_walk(
    store_root: &Path,
    range: &[&str],
    pattern: &str,
) -> std::result::Result<Vec<(String, std::result::Result<Vec<String>, String>)>, String> {
    let line_re = regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| format!("cannot search store history: {e}"))?;
    let g_pattern = format!("-G{pattern}");
    let mut args: Vec<&str> = vec![
        "log",
        "--format=",
        "-p",
        "--unified=0",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-show-signature",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--find-renames",
        "-l0",
        "--regexp-ignore-case",
        &g_pattern,
    ];
    args.extend(range);
    args.extend(["--", "objects/"]);
    let out = store_git(store_root, &args, None)?;
    let mut hits = Vec::new();
    // TASK-2067: the (old, new) paths of the file pair being read. Renames
    // are detected explicitly (`--find-renames -l0` above), whatever the
    // user's diff.renames or diff.renameLimit, so a renumbered object is one
    // pair: a removed line belongs to the old path, an added line to the new
    // one, and both stems of a rename are the same spec.
    // trace:TASK-2067 | ai:claude
    let mut pair: Option<(String, String)> = None;
    for line in String::from_utf8_lossy(&out).lines() {
        if let Some(paths) = line.strip_prefix("diff --git ") {
            pair = paths
                .strip_prefix("a/")
                .and_then(|paths| paths.rsplit_once(" b/"))
                .map(|(a, b)| (a.to_string(), b.to_string()));
            continue;
        }
        if let (Some(from), Some((old, _))) = (line.strip_prefix("rename from "), pair.as_mut()) {
            *old = from.to_string();
            continue;
        }
        if let (Some(to), Some((_, new))) = (line.strip_prefix("rename to "), pair.as_mut()) {
            *new = to.to_string();
            continue;
        }
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            continue;
        }
        let (body, removed) = match (line.strip_prefix('-'), line.strip_prefix('+')) {
            (Some(body), _) => (body, true),
            (None, Some(body)) => (body, false),
            (None, None) => continue,
        };
        let Some(caps) = line_re.captures(body) else {
            continue;
        };
        let id = caps[2].to_ascii_uppercase();
        let (hit, other) = match &pair {
            Some((old, new)) if removed => (Some(old.as_str()), Some(new.as_str())),
            Some((old, new)) => (Some(new.as_str()), Some(old.as_str())),
            None => (None, None),
        };
        // TASK-1800 (comment 24): the history read finds versions only at an
        // object's canonical path. A hit anywhere else would read as no
        // history and drop the ID as dangling, so it refuses instead.
        // trace:TASK-1800 | ai:claude
        let Some(stem) = hit.and_then(canonical_object_stem) else {
            hits.push((
                id,
                Err(format!(
                    "store history carries {} outside a canonical object path ({})",
                    &caps[2],
                    hit.unwrap_or("no file")
                )),
            ));
            continue;
        };
        // The other side of a rename only adds versions to judge. TASK-2238:
        // a non-canonical other side would be versions never judged, so it
        // refuses like a non-canonical hit rather than being dropped.
        // trace:TASK-2238 | ai:claude
        let renamed = match other.filter(|other| Some(*other) != hit) {
            None => None,
            Some(other) => match canonical_object_stem(other) {
                Some(renamed) => Some(renamed),
                None => {
                    hits.push((
                        id,
                        Err(format!(
                            "store history renames {} to or from a non-canonical object path ({other})",
                            &caps[2]
                        )),
                    ));
                    continue;
                }
            },
        };
        let stems = std::iter::once(stem)
            .chain(renamed)
            .map(str::to_ascii_uppercase)
            .collect();
        hits.push((id, Ok(stems)));
    }
    Ok(hits)
}

/// The object ID whose canonical object path is `path`, if it is one.
// trace:TASK-1800 trace:TASK-2067 | ai:claude
fn canonical_object_stem(path: &str) -> Option<&str> {
    let stem = Path::new(path)
        .file_name()?
        .to_str()?
        .strip_suffix(".yaml")?;
    let canonical = aida_core::object_store::relative_object_path(stem).ok()?;
    (canonical == path).then_some(stem)
}

/// TASK-1800: [`store_history_alias_map`] for one ID.
// trace:TASK-1800 | ai:claude
#[cfg(test)]
pub(crate) fn store_history_alias_ids(
    store_root: &Path,
    id: &str,
) -> std::result::Result<Vec<String>, String> {
    let mut map = store_history_alias_map(store_root, &[id.to_string()])?;
    Ok(map.remove(&id.to_ascii_uppercase()).unwrap_or_default())
}

/// STORY-1698: every version of `spec_id` in the store's git history (all
/// refs, so a rewritten local branch still shows what the remote saw), each
/// expanded by its recorded field changes. `Err` when the history cannot be
/// read or holds no version at all — the caller refuses.
// trace:STORY-1698 | ai:claude
#[cfg(test)]
pub(crate) fn store_history_versions(
    store_root: &Path,
    spec_id: &str,
) -> std::result::Result<Vec<aida_core::Requirement>, String> {
    let out = store_history_versions_optional(store_root, spec_id)?;
    if out.is_empty() {
        return Err(format!("no version of {spec_id} in the store's history"));
    }
    Ok(out)
}

/// The audit copy of a renewal in `.aida/events.jsonl`: old expiry -> new
/// expiry, and who renewed. Best-effort; the record's own `renewals` lineage
/// is the authority.
// trace:STORY-B6B1278890774EF993D2EBC327862470-166 | ai:claude
pub(crate) fn emit_renewed(project_root: &Path, mandate: &Mandate, renewal: &Renewal) {
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MandateChanged {
            id: mandate.id.clone(),
            active: true,
            principal: renewal.renewed_by.clone(),
            expires_at: renewal.expires_at.to_rfc3339(),
            include_supervised: mandate.include_supervised,
            include_gate_paths: mandate.include_gate_paths,
            reason: mandate.reason.clone(),
            renewed_from: Some(renewal.previous_expires_at.to_rfc3339()),
            changes: renewal.changes.clone(),
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(project_root, &event);
}

/// The audit copy of a revival in `.aida/events.jsonl`: a start whose
/// `renewed_from` names the expired mandate it re-minted, so the trail links
/// the two, with what the revival changed. Best-effort.
// trace:STORY-2346 | ai:claude
// trace:STORY-2771 | ai:claude
pub(crate) fn emit_revived(
    project_root: &Path,
    mandate: &Mandate,
    old_id: &str,
    receipt: &Renewal,
) {
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MandateChanged {
            id: mandate.id.clone(),
            active: true,
            principal: mandate.principal.clone(),
            expires_at: mandate.expires_at.to_rfc3339(),
            include_supervised: mandate.include_supervised,
            include_gate_paths: mandate.include_gate_paths,
            reason: mandate.reason.clone(),
            renewed_from: Some(old_id.to_string()),
            changes: receipt.changes.clone(),
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(project_root, &event);
}

/// What `aida mandate renew` prints after reviving an expired mandate: the
/// old id and expiry, the new id, scope and flags, and the new expiry.
// trace:STORY-2346 | ai:claude
pub(crate) fn revived_message(old_id: &str, m: &Mandate, receipt: &Renewal) -> String {
    format!(
        "Revived mandate {old_id} (expired {}) as {} ({}, supervised: {}); active until {}. {}; protected paths: {}.",
        receipt.previous_expires_at.to_rfc3339(),
        m.id,
        m.effective_scopes().iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        m.include_supervised,
        m.expires_at.to_rfc3339(),
        // trace:STORY-2771 | ai:claude
        if receipt.changes.is_empty() {
            "Scope, supervised flag and human gates retained"
        } else {
            "Everything not listed above retained"
        },
        m.include_gate_paths
    )
}

/// The audit copy of a start / stop in `.aida/events.jsonl`. Best-effort.
// trace:STORY-1698 | ai:claude
pub(crate) fn emit_changed(project_root: &Path, mandate: &Mandate, active: bool) {
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MandateChanged {
            id: mandate.id.clone(),
            active,
            principal: mandate.principal.clone(),
            expires_at: mandate.expires_at.to_rfc3339(),
            include_supervised: mandate.include_supervised,
            include_gate_paths: mandate.include_gate_paths,
            reason: mandate.reason.clone(),
            renewed_from: None,
            changes: Vec::new(),
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(project_root, &event);
}

// ── Display ─────────────────────────────────────────────────────────────────

/// The statusline segment: `MANDATE 3h12m` while one is active.
// trace:STORY-1698 | ai:claude
pub(crate) fn statusline_segment(state: &MandateState, now: DateTime<Utc>) -> Option<String> {
    // trace:STORY-2346 | ai:codex
    if let Some(warning) = expiry_warning(state, now) {
        return Some(warning);
    }
    match state {
        MandateState::Active(m) if m.is_active_at(now) => Some(format!(
            "MANDATE {}",
            crate::gate::seat_authority::format_remaining(m.expires_at - now)
        )),
        _ => None,
    }
}

/// STORY-1700: the one-line mandate display shared by `aida mandate status
/// --line`, SessionStart, the per-turn notice and `aida session handoff
/// --show`: `MANDATE drive · 5h40m left · by joe · reason · gates: none`.
/// `None` unless a mandate is active. `gates` describes the human gates.
// trace:STORY-1700 | ai:claude
pub(crate) fn status_line(state: &MandateState, now: DateTime<Utc>, gates: &str) -> Option<String> {
    let MandateState::Active(m) = state else {
        return None;
    };
    if !m.is_active_at(now) {
        return None;
    }
    // trace:TASK-2342 | ai:agy
    let kind_label = if m.is_drive() {
        KIND_DRIVE.to_string()
    } else {
        let scopes = m
            .effective_scopes()
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        // A mandate with no effective scope covers nothing; say so rather
        // than borrow a scope it does not hold. trace:TASK-2342 | ai:claude
        if scopes.is_empty() {
            "(no scopes)".to_string()
        } else {
            scopes
        }
    };
    let mut parts = vec![
        format!("MANDATE {kind_label}"),
        format!(
            "{} left",
            crate::gate::seat_authority::format_remaining(m.expires_at - now)
        ),
        format!("by {}", m.principal),
    ];
    if let Some(reason) = &m.reason {
        parts.push(reason.clone());
    }
    parts.push(format!("gates: {gates}"));
    // trace:STORY-2346 | ai:codex
    if let Some(warning) = expiry_warning(state, now) {
        parts.push(warning);
    }
    Some(parts.join(" · "))
}

// trace:STORY-2346 | ai:codex
pub(crate) fn expiry_warning(state: &MandateState, now: DateTime<Utc>) -> Option<String> {
    let MandateState::Active(m) = state else {
        return None;
    };
    let seconds = (m.expires_at - now).num_seconds();
    (seconds > 0 && seconds <= 3600).then(|| {
        format!(
            "MANDATE expires in {}m, run: aida mandate renew",
            (seconds + 59) / 60
        )
    })
}

/// Best-effort delivery, once per recipient and expiry. A failed delivery is
/// retried at the next hook/status/due-jobs read. Stable message IDs deduplicate
/// concurrent readers; the marker is written only after successful delivery.
// trace:STORY-2346 | ai:codex
pub(crate) fn notify_expiry(dir: &Path, root: &Path, state: &MandateState, now: DateTime<Utc>) {
    let Some(body) = expiry_warning(state, now) else {
        return;
    };
    let MandateState::Active(m) = state else {
        return;
    };
    // Keep concurrent hook/status readers from resetting delivery sidecars.
    // Contention defers without blocking the per-turn hook.
    use fs2::FileExt;
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("expiry-notice.lock"))
    else {
        return;
    };
    if lock.try_lock_exclusive().is_err() {
        return;
    }
    let prefix = format!("mandate-expiry-{}-{}", m.id, m.expires_at.timestamp());
    let seen = dir.join(".seen");
    if (0..2).all(|i| expiry_notice_settled(&seen, &format!("{prefix}-{i}"), now)) {
        return;
    }
    let store = root.join(".aida-store");
    let Ok(directory) = crate::mailbox_resolver::RecipientDirectory::load(root, &store) else {
        return;
    };
    for (index, recipient) in [format!("person:{}", m.principal), "advisor".into()]
        .iter()
        .enumerate()
    {
        let id = format!("{prefix}-{index}");
        let marker = seen.join(&id);
        if expiry_notice_settled(&seen, &id, now) {
            continue;
        }
        let Ok(resolved) = directory.resolve_send(recipient, true) else {
            // Negative cache: an unresolvable recipient (e.g. a principal
            // missing from the registry) must not reload the directory on
            // every statusline tick. trace:STORY-2346 | ai:claude
            let _ = write_private(
                &seen.join(format!("{id}.miss")),
                now.timestamp().to_string().as_bytes(),
            );
            continue;
        };
        let Ok((deliveries, _)) = crate::mailbox_resolver::delivery_sidecar(
            &resolved,
            &id,
            now.timestamp_millis(),
            false,
        ) else {
            continue;
        };
        let message = aida_core::mailbox::Message {
            id: id.clone(),
            thread_id: id,
            from: "aida-mandate".into(),
            to: aida_core::mailbox::Recipient::Agent(resolved.address),
            // Delivery may happen after the threshold. Stamp the actual send
            // so a newer inbox read watermark cannot hide this fresh notice.
            timestamp: now.timestamp_millis(),
            subject: Some("Mandate renewal due".into()),
            body: body.clone(),
            in_reply_to: None,
            urgent: false,
            intent: aida_core::mailbox::Intent::Request,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
            from_seat: None,
            relayed_from: None,
        };
        if crate::mailbox_store::write_message_with_deliveries(root, &message, deliveries.as_ref())
            .is_ok()
        {
            let _ = write_private(&marker, b"delivered");
        }
    }
}

/// How long [`notify_expiry`] waits before retrying a recipient it could
/// not resolve.
// trace:STORY-2346 | ai:claude
const EXPIRY_NOTICE_MISS_RETRY_SECS: i64 = 600;

/// A notice needs no work now: delivered, or its recipient failed to
/// resolve less than [`EXPIRY_NOTICE_MISS_RETRY_SECS`] ago.
// trace:STORY-2346 | ai:claude
fn expiry_notice_settled(seen: &Path, id: &str, now: DateTime<Utc>) -> bool {
    if seen.join(id).exists() {
        return true;
    }
    std::fs::read_to_string(seen.join(format!("{id}.miss")))
        .ok()
        .and_then(|t| t.trim().parse::<i64>().ok())
        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
        .is_some_and(|missed| {
            now >= missed && (now - missed).num_seconds() < EXPIRY_NOTICE_MISS_RETRY_SECS
        })
}

/// The [`status_line`] for the mandates in `dir` right now, with the human
/// gates of `audit_root` — what `aida mandate status --line` prints.
// trace:TASK-1955 | ai:claude (TASK-1806)
pub(crate) fn current_status_line(
    dir: &Path,
    audit_root: Option<&Path>,
    now: DateTime<Utc>,
) -> Option<String> {
    let (valid, _) = load_valid(dir, &crate::gate::mandate::operator_principal(), now);
    let state = state_at(&valid, now);
    if let Some(root) = audit_root {
        notify_expiry(dir, root, &state, now);
    }
    status_line(&state, now, &crate::mandate_gates::describe(audit_root))
}

/// STORY-1700: the per-turn notice for `seat`: the [`status_line`] while a
/// mandate is active, and ONE line per seat when it expires or is stopped
/// (marker `<dir>/.seen/<id>.<seat>`, written here). `None` otherwise.
// trace:STORY-1700 | ai:claude
pub(crate) fn notice_line(
    dir: &Path,
    state: &MandateState,
    now: DateTime<Utc>,
    seat: &str,
    gates: &str,
) -> Option<String> {
    let (m, how) = match state {
        MandateState::Active(_) => return status_line(state, now, gates),
        MandateState::None => return None,
        MandateState::Expired(m) => (m, "expired"),
        MandateState::Stopped(m) => (m, "was stopped"),
    };
    // Only a recent end is news; a seat started days later is not told.
    let ended = m.revoked_at.unwrap_or(m.expires_at).min(m.expires_at);
    if now - ended > MAX_TTL {
        return None;
    }
    let seat: String = seat
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let marker = dir.join(".seen").join(format!("{}.{seat}", m.id));
    if std::fs::symlink_metadata(&marker).is_ok() {
        return None;
    }
    write_private(&marker, now.to_rfc3339().as_bytes()).ok()?;
    Some(format!(
        "MANDATE {} {how}; the advisor seat no longer acts under it (a human runs `aida mandate renew` after expiry, or `aida mandate start --ship-held` after a stop)",
        m.id
    ))
}

fn status_json(state: &MandateState, now: DateTime<Utc>) -> serde_json::Value {
    let (status, mandate) = match state {
        MandateState::None => ("none", None),
        MandateState::Active(m) => ("active", Some(m)),
        MandateState::Expired(m) => ("expired", Some(m)),
        MandateState::Stopped(m) => ("stopped", Some(m)),
    };
    let mut out = serde_json::json!({ "status": status });
    if let Some(m) = mandate {
        out["mandate"] = serde_json::to_value(m).unwrap_or_default();
        // trace:STORY-B6B1278890774EF993D2EBC327862470-166 | ai:claude
        out["relay_parent_renewals"] = crate::gate::relay_parent::summary(&m.id);
        out["renewal_count"] = serde_json::json!(m.renewals.len());
        if status == "active" {
            out["remaining_secs"] = serde_json::json!((m.expires_at - now).num_seconds());
        }
    }
    out
}

fn status_text(state: &MandateState, now: DateTime<Utc>) -> String {
    let describe = |m: &Mandate| {
        let mut lines = vec![
            format!("  id:        {}", m.id),
            format!(
                "  minted by: {} at {}",
                m.principal,
                m.issued_at.to_rfc3339()
            ),
            if m.is_drive() {
                format!(
                    "  covers:    drive ({}): the advisor seat ships held PRs, signs off sketches and dispatches fixers; the 15m drive tick routes stalled lanes",
                    m.effective_scopes()
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else if m.effective_scopes() == BTreeSet::from([Scope::ShipHeld]) {
                "  covers:    the advisor seat shipping held PRs".to_string()
            } else {
                format!(
                    "  covers:    {}",
                    m.effective_scopes()
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            format!(
                "  includes sensitive specs: {}",
                if m.include_supervised {
                    "yes (--include-supervised)"
                } else {
                    "no"
                }
            ),
            format!("  includes protected paths: {}", m.include_gate_paths),
        ];
        if let Some(reason) = &m.reason {
            lines.push(format!("  reason:    {reason}"));
        }
        // trace:STORY-B6B1278890774EF993D2EBC327862470-166 | ai:claude
        lines.push(format!(
            "  relay parent renewals: {}",
            crate::gate::relay_parent::summary(&m.id)
        ));
        lines.push(match m.renewals.last() {
            None => "  renewals:  0".to_string(),
            Some(last) => format!(
                "  renewals:  {} (last by {} at {}, from {})",
                m.renewals.len(),
                last.renewed_by,
                last.renewed_at.to_rfc3339(),
                last.previous_expires_at.to_rfc3339()
            ),
        });
        // trace:STORY-2771 | ai:claude
        if let Some(revival) = &m.revived {
            lines.push(format!(
                "  revived:   from {} (expired {}){}",
                revival.from_id,
                revival.receipt.previous_expires_at.to_rfc3339(),
                if revival.receipt.changes.is_empty() {
                    String::new()
                } else {
                    format!("; changed {}", revival.receipt.changes.join("; "))
                }
            ));
        }
        lines.join("\n")
    };
    match state {
        MandateState::None => "No mandate has been started.".to_string(),
        MandateState::Active(m) => format!(
            "Mandate ACTIVE until {} ({} left)\n{}",
            m.expires_at.to_rfc3339(),
            crate::gate::seat_authority::format_remaining(m.expires_at - now),
            describe(m)
        ),
        MandateState::Expired(m) => format!(
            "No active mandate. The last one expired at {}.\n{}",
            m.expires_at.to_rfc3339(),
            describe(m)
        ),
        MandateState::Stopped(m) => format!(
            "No active mandate. The last one was stopped at {} by {}.\n{}",
            m.revoked_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
            m.revoked_by.as_deref().unwrap_or("?"),
            describe(m)
        ),
    }
}

fn state_mandate(state: &MandateState) -> Option<&Mandate> {
    match state {
        MandateState::None => None,
        MandateState::Active(m) | MandateState::Expired(m) | MandateState::Stopped(m) => Some(m),
    }
}

/// The line a renewal or revival prints when it carries the protected-path
/// opt-in forward unchanged, so a kept opt-in is never silent.
// trace:STORY-2771 | ai:claude
pub(crate) fn kept_gate_paths_line(m: &Mandate, renewal: &Renewal) -> Option<String> {
    let changed = renewal
        .changes
        .iter()
        .any(|change| change.starts_with("include-gate-paths:"));
    (m.include_gate_paths && !changed)
        .then(|| "includes protected paths: KEPT (pass --exclude-gate-paths to drop)".to_string())
}

/// The overrides `aida mandate renew`'s flags ask for. An absent flag asks
/// for nothing, so the renewed mandate keeps that value.
// trace:STORY-2771 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn renew_overrides(
    ship_held: bool,
    scopes: &[String],
    drive: bool,
    include_supervised: bool,
    exclude_supervised: bool,
    include_gate_paths: bool,
    exclude_gate_paths: bool,
    reason: Option<String>,
) -> Result<crate::gate::mandate::RenewOverrides> {
    let mut add_scopes = BTreeSet::new();
    if ship_held {
        add_scopes.insert(Scope::ShipHeld);
    }
    for raw in scopes {
        add_scopes.insert(Scope::parse(raw)?);
    }
    let flag = |on: bool, off: bool| match (on, off) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    };
    Ok(crate::gate::mandate::RenewOverrides {
        drive,
        add_scopes,
        relay_seats: Vec::new(),
        include_supervised: flag(include_supervised, exclude_supervised),
        include_gate_paths: flag(include_gate_paths, exclude_gate_paths),
        reason,
    })
}

/// `aida mandate start|renew|stop|status`.
// trace:STORY-1698 | ai:claude
pub(crate) fn handle_mandate(action: &crate::cli::MandateAction) -> Result<()> {
    let dir = mandate_dir()?;
    let audit_root = crate::find_main_worktree_root().ok();
    match action {
        crate::cli::MandateAction::Start {
            ship_held,
            scopes,
            relay_seats,
            drive,
            ttl,
            reason,
            include_supervised,
            exclude_supervised,
            include_gate_paths,
        } => {
            let mut parsed_scopes = BTreeSet::new();
            if *ship_held {
                parsed_scopes.insert(Scope::ShipHeld);
            }
            for raw in scopes {
                parsed_scopes.insert(Scope::parse(raw)?);
            }
            if !drive && parsed_scopes.is_empty() {
                bail!("say what the mandate covers: `aida mandate start --ship-held`, `--scope dispatch`, or `--drive` for a drive mandate");
            }
            let ttl = match ttl {
                Some(raw) => parse_ttl(raw)?,
                // A cap configured below the default lowers the default too.
                // trace:STORY-2828 | ai:claude
                None => DEFAULT_TTL.min(crate::gate::mandate::max_ttl().duration()),
            };
            // STORY-1700: a drive mandate covers sensitive specs unless the
            // human opts out; the gate decides, and binds the human floor.
            // trace:STORY-1700 | ai:claude
            anyhow::ensure!(
                relay_seats.is_empty() || parsed_scopes.contains(&Scope::RelayParentRenewal),
                "--relay-seat requires --scope relay-parent-renewal"
            );
            let m = crate::gate::mandate::start_as_human_for_relay(
                &dir,
                &crate::gate::mandate::operator_principal(),
                ttl,
                reason.clone(),
                *drive,
                *include_supervised,
                *exclude_supervised,
                *include_gate_paths,
                parsed_scopes,
                relay_seats,
                Utc::now(),
            )?;
            if let Some(root) = &audit_root {
                emit_changed(root, &m, true);
            }
            if m.is_drive() {
                println!(
                    "Drive mandate {} started until {}: the advisor seat may ship held PRs with an attested, independent approval and green CI, sign off sketches and dispatch fixers; every 15 minutes the drive tick routes each stalled lane.",
                    m.id,
                    m.expires_at.to_rfc3339()
                );
                if let Some(root) = &audit_root {
                    for warning in crate::mandate_drive::start_warnings(root, &m.principal) {
                        eprintln!("warning: {warning}");
                    }
                }
            } else if m.covers(Scope::ShipHeld) {
                println!(
                    "Mandate {} started: the advisor seat may ship held PRs with an attested, independent approval and green CI until {}.",
                    m.id,
                    m.expires_at.to_rfc3339()
                );
            } else {
                let scope_list = m
                    .effective_scopes()
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "Mandate {} started ({}): active until {}.",
                    m.id,
                    scope_list,
                    m.expires_at.to_rfc3339()
                );
            }
            if !m.include_supervised {
                println!("Sensitive specs (security, containment, authority, guided, keystone) stay human-only.");
            }
            println!("Operator-only and decide-mode specs always stay human-only. Stop it any time: `aida mandate stop`.");
            Ok(())
        }
        // trace:STORY-B6B1278890774EF993D2EBC327862470-166 | ai:claude
        crate::cli::MandateAction::Renew {
            duration,
            ship_held,
            scopes,
            relay_seats,
            drive,
            include_supervised,
            exclude_supervised,
            include_gate_paths,
            exclude_gate_paths,
            reason,
        } => {
            // Same grammar and cap as `start --ttl`, named for this flag.
            let ttl = duration.as_deref().map(parse_renew_for).transpose()?;
            // trace:STORY-2771 | ai:claude
            let mut overrides = renew_overrides(
                *ship_held,
                scopes,
                *drive,
                *include_supervised,
                *exclude_supervised,
                *include_gate_paths,
                *exclude_gate_paths,
                reason.clone(),
            )?;
            overrides.relay_seats = relay_seats.clone();
            let (m, renewal, revived_from) = renew_as_human(
                &dir,
                &crate::gate::mandate::operator_principal(),
                ttl,
                &overrides,
                Utc::now(),
            )?;
            for change in &renewal.changes {
                println!("{change}");
            }
            if let Some(kept) = kept_gate_paths_line(&m, &renewal) {
                println!("{kept}");
            }
            // trace:TASK-3820 | ai:claude — the mandate is renewed first; then
            // every covered grant is extended up to its new expiry.
            let grants = renew_covered_grants(&m, audit_root.as_deref());
            let mut audit = renewal.clone();
            audit.changes.extend(grant_audit_lines(&grants));
            // trace:STORY-2346 | ai:claude
            if let Some(old_id) = &revived_from {
                if let Some(root) = &audit_root {
                    emit_revived(root, &m, old_id, &audit);
                }
                println!("{}", revived_message(old_id, &m, &renewal));
            } else {
                if let Some(root) = &audit_root {
                    emit_renewed(root, &m, &audit);
                }
                println!(
                    "Mandate {} renewed until {} (was {}); {}; protected paths: {} (renewal {}).",
                    m.id,
                    renewal.expires_at.to_rfc3339(),
                    renewal.previous_expires_at.to_rfc3339(),
                    if renewal.changes.is_empty() {
                        "scope, supervised flag and reason retained"
                    } else {
                        "everything else retained"
                    },
                    m.include_gate_paths,
                    m.renewals.len()
                );
            }
            grants_outcome(&grants)
        }
        crate::cli::MandateAction::Stop => {
            let stopped = stop_as_human(
                &dir,
                &crate::gate::mandate::operator_principal(),
                Utc::now(),
            )?;
            if stopped.is_empty() {
                println!("No mandate was active.");
            }
            for m in &stopped {
                if let Some(root) = &audit_root {
                    emit_changed(root, m, false);
                }
                println!(
                    "Mandate {} stopped; the advisor can no longer ship held PRs.",
                    m.id
                );
            }
            Ok(())
        }
        crate::cli::MandateAction::Status { json, line } => {
            let now = Utc::now();
            // trace:STORY-1700 | ai:claude
            if *line {
                if let Some(line) = current_status_line(&dir, audit_root.as_deref(), now) {
                    println!("{line}");
                }
                return Ok(());
            }
            let (valid, warnings) =
                load_valid(&dir, &crate::gate::mandate::operator_principal(), now);
            let state = state_at(&valid, now);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
            print!(
                "{}",
                operator_status_report(&state, now, audit_root.as_deref(), &cwd, *json, &warnings)?
            );
            Ok(())
        }
    }
}

/// TASK-3820: extend the operator's covered grants up to the renewed
/// mandate's expiry, with the seat cap `aida role enter` enforces where the
/// command runs. `Err` when the grant store could not be reached at all.
// trace:TASK-3820 | ai:claude
fn renew_covered_grants(
    m: &Mandate,
    audit_root: Option<&Path>,
) -> Result<crate::gate::seat_authority::GrantExtensionReport> {
    // Grants are checked against the project's roster: outside a project
    // nothing can be extended, and saying so beats a silent success.
    let Some(root) = audit_root else {
        bail!("run `aida mandate renew` inside the project checkout to extend its session grants");
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let policy_root = crate::statusline_project_root_from_with_roots(
        &cwd,
        &aida_core::store_locate::real_temp_roots(),
    );
    crate::gate::seat_authority::extend_operator_grants_as_human(root, &policy_root, m.expires_at)
}

/// The renewal's audit lines for each grant it extended or refused.
// trace:TASK-3820 | ai:claude
fn grant_audit_lines(
    grants: &Result<crate::gate::seat_authority::GrantExtensionReport>,
) -> Vec<String> {
    use crate::gate::seat_authority::ExtensionOutcome;
    let report = match grants {
        Ok(report) => report,
        Err(e) => return vec![format!("grants: not extended: {e:#}")],
    };
    report
        .grants
        .iter()
        .filter_map(|g| match g.outcome {
            ExtensionOutcome::Extended => Some(format!(
                "grant {} ({}): {} -> {}",
                g.id,
                g.role,
                g.previous_expires_at.to_rfc3339(),
                g.expires_at.to_rfc3339()
            )),
            ExtensionOutcome::Refused => Some(format!(
                "grant {} ({}): refused: {}",
                g.id,
                g.role,
                g.reason.as_deref().unwrap_or("unknown")
            )),
            ExtensionOutcome::Kept | ExtensionOutcome::Skipped => None,
        })
        .collect()
}

/// Print the grant table; fail naming every covered grant that was refused.
// trace:TASK-3820 | ai:claude
fn grants_outcome(
    grants: &Result<crate::gate::seat_authority::GrantExtensionReport>,
) -> Result<()> {
    let report = match grants {
        Ok(report) => report,
        Err(e) => bail!("the mandate was renewed, but no session grant was extended: {e:#}"),
    };
    print!("{}", report.text());
    let refused = report.refused();
    if refused.is_empty() {
        return Ok(());
    }
    let named: Vec<String> = refused
        .iter()
        .map(|g| format!("{} ({})", g.id, g.role))
        .collect();
    bail!(
        "the mandate was renewed, but {} covered session grant(s) could not be extended: {}; re-enter those seats at a terminal (`aida role enter <role>`) or relaunch the agents that hold them",
        refused.len(),
        named.join(", ")
    )
}

// trace:TASK-3816 | ai:codex
/// `invoking_dir` is where the operator ran the command: role enter enforces
/// `[seat] max_grant_hours` from the project root found walking up from it
/// (a worktree's own config), not from the main checkout `audit_root`.
fn operator_status_report(
    state: &MandateState,
    now: DateTime<Utc>,
    audit_root: Option<&Path>,
    invoking_dir: &Path,
    json: bool,
    warnings: &[String],
) -> Result<String> {
    let root = audit_root.unwrap_or_else(|| Path::new("."));
    // trace:TASK-3816 | ai:claude — same resolution as `aida role enter`.
    let policy_root = crate::statusline_project_root_from_with_roots(
        invoking_dir,
        &aida_core::store_locate::real_temp_roots(),
    );
    let inventory = crate::gate::seat_authority::operator_grant_inventory(root, &policy_root, now);
    let cap = crate::gate::mandate::max_ttl();
    let report = status_report(state, now, audit_root, json);
    if json {
        let mut out: serde_json::Value = serde_json::from_str(&report)?;
        out["grant_inventory"] = serde_json::to_value(&inventory)?;
        out["mandate_diagnostics"] = serde_json::json!({
            "effective_cap_hours": cap.hours,
            "config_origin": cap.origin(crate::gate::seat_authority::MANDATE_MAX_TTL_KEY),
            "rejected_records": warnings,
        });
        Ok(format!("{out}\n"))
    } else {
        let mut out = format!(
            "{report}{}Mandate cap: {}h ({})\n",
            inventory.text(),
            cap.hours,
            cap.origin(crate::gate::seat_authority::MANDATE_MAX_TTL_KEY)
        );
        for warning in warnings {
            out.push_str(&format!("  rejected record: {warning}\n"));
        }
        Ok(out)
    }
}

/// The full `aida mandate status` output (text or JSON), newline-terminated.
// trace:STORY-1888 | ai:claude
pub(crate) fn status_report(
    state: &MandateState,
    now: DateTime<Utc>,
    audit_root: Option<&Path>,
    json: bool,
) -> String {
    if json {
        let mut out = status_json(state, now);
        if let (Some(root), Some(m)) = (audit_root, state_mandate(state)) {
            out["drive_actions"] = crate::mandate_drive::ledger_json(root, &m.id);
        }
        if let Some(root) = audit_root {
            out["protected_paths"] = crate::mandate_gates::protected_paths_json(root);
        }
        return format!("{out}\n");
    }
    let mut out = format!("{}\n", status_text(state, now));
    if let (Some(root), Some(m)) = (audit_root, state_mandate(state)) {
        let gates = crate::mandate_gates::describe(Some(root));
        out.push_str(&format!("  human gates: {gates}\n"));
        if let Some(summary) = crate::mandate_drive::ledger_text(root, &m.id) {
            out.push_str(&format!("{summary}\n"));
        }
    }
    if let Some(root) = audit_root {
        out.push_str(&crate::mandate_gates::protected_paths_text(root));
    }
    out
}

/// Fire-fighting mandate: a human-minted, time-boxed grant that lets the
/// advisor seat ship held PRs with an attested, independent approval.
///
/// `start`, `renew` and `stop` need a human at an interactive terminal; `status`
/// is open to anyone. Operator-only specs always stay human-only.
// trace:STORY-1698 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "mandate")]
pub(crate) struct MandateArgs {
    #[clap(subcommand)]
    pub action: crate::cli::MandateAction,
}

// STORY-1698: the fire-fighting mandate reads/writes only the user's AIDA
// home (+ a best-effort audit event); no store handle. trace:STORY-1698 | ai:claude
fn run_mandate(
    matches: &clap::ArgMatches,
    _ctx: &crate::cmdkit::DispatchCtx,
) -> anyhow::Result<()> {
    // trace:TASK-3428 | ai:codex
    let args = <MandateArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let MandateArgs { action } = &args;
    return self::handle_mandate(action);
}

// trace:STORY-1728 | ai:claude
pub(crate) static MANDATE: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "mandate",
    help_after: "merge-hold",
    command: <MandateArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run_mandate),
};

#[cfg(test)]
mod task_3816_tests {
    use super::*;
    use crate::gate::seat_authority::{CapSource, HourCap};

    struct ResetCap;
    impl Drop for ResetCap {
        fn drop(&mut self) {
            TEST_MAX_TTL.with(|c| c.set(None));
        }
    }

    // trace:TASK-3816 | ai:codex
    #[test]
    fn task_3816_status_reports_rejected_mandate_and_cap_in_both_formats() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = crate::gate::test_support::AmbientGuard::hermetic(tmp.path(), None);
        let _reset = ResetCap;
        let principal = crate::gate::mandate::operator_principal();
        TEST_MAX_TTL.with(|c| {
            c.set(Some(HourCap {
                hours: 72,
                source: CapSource::Global,
            }))
        });
        let now = Utc::now();
        let dir = tmp.path().join("mandates");
        let m = start(
            &dir,
            &|| Ok(()),
            &principal,
            chrono::Duration::hours(72),
            None,
            false,
            now,
        )
        .unwrap();
        TEST_MAX_TTL.with(|c| {
            c.set(Some(HourCap {
                hours: 24,
                source: CapSource::Project,
            }))
        });
        let (valid, warnings) = load_valid(&dir, &principal, now);
        assert!(valid.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains(&m.id));
        let state = state_at(&valid, now);
        let json: serde_json::Value = serde_json::from_str(
            &operator_status_report(&state, now, Some(tmp.path()), tmp.path(), true, &warnings)
                .unwrap(),
        )
        .unwrap();
        let text =
            operator_status_report(&state, now, Some(tmp.path()), tmp.path(), false, &warnings)
                .unwrap();
        assert_eq!(json["status"], "none");
        assert_eq!(json["mandate_diagnostics"]["effective_cap_hours"], 24);
        let origin = json["mandate_diagnostics"]["config_origin"]
            .as_str()
            .unwrap();
        assert!(origin.contains("project"));
        assert!(text.contains(origin));
        assert!(text.contains("Mandate cap: 24h"));
        assert_eq!(
            json["mandate_diagnostics"]["rejected_records"],
            serde_json::json!(warnings)
        );
        assert!(text.contains(&warnings[0]));
        assert!(json["grant_inventory"]["grants"].is_array());
    }

    /// A worktree lowering `[seat] max_grant_hours` below the main checkout's
    /// must be what the inventory reports when invoked from a subdirectory of
    /// that worktree, the cap `aida role enter` enforces there.
    // trace:TASK-3816 | ai:claude
    #[test]
    fn task_3816_inventory_cap_follows_invoking_worktree_not_main() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(main.join(".aida")).unwrap();
        let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat_and(
            &main,
            "advisor",
            &[],
            &[],
        );
        std::fs::write(
            main.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n[seat]\nmax_grant_hours = 20\n",
        )
        .unwrap();
        let worktree = tmp.path().join("wt");
        let sub = worktree.join("src").join("deep");
        std::fs::create_dir_all(worktree.join(".aida")).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            worktree.join(".aida/config.toml"),
            "[seat]\nmax_grant_hours = 12\n",
        )
        .unwrap();
        let now = Utc::now();
        let state = state_at(&[], now);
        let caps = |invoking: &Path| -> Vec<(u64, String)> {
            let out =
                operator_status_report(&state, now, Some(&main), invoking, true, &[]).unwrap();
            let json: serde_json::Value = serde_json::from_str(&out).unwrap();
            json["grant_inventory"]["grants"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| {
                    (
                        g["effective_cap_hours"].as_u64().unwrap(),
                        g["config_origin"].as_str().unwrap().to_string(),
                    )
                })
                .collect()
        };
        let from_worktree = caps(&sub);
        assert!(!from_worktree.is_empty(), "the operator's grant is listed");
        for (hours, origin) in &from_worktree {
            assert_eq!(*hours, 12, "worktree cap, not main's");
            assert!(origin.contains("project"), "{origin}");
        }
        assert!(caps(&main).iter().all(|(hours, _)| *hours == 20));
    }
}

#[cfg(test)]
mod task_3820_tests {
    use super::*;
    use crate::gate::seat_authority::{ExtensionOutcome, GrantExtension, GrantExtensionReport};

    fn grant(id: &str, outcome: ExtensionOutcome, reason: Option<&str>) -> GrantExtension {
        let at = Utc::now();
        GrantExtension {
            id: id.into(),
            role: "advisor".into(),
            parent: None,
            holders: Vec::new(),
            previous_expires_at: at,
            expires_at: at + chrono::Duration::hours(8),
            outcome,
            reason: reason.map(str::to_string),
            key_note: None,
        }
    }

    fn report(grants: Vec<GrantExtension>) -> Result<GrantExtensionReport> {
        Ok(GrantExtensionReport {
            target_expires_at: Utc::now(),
            effective_cap_hours: 24,
            config_origin: "the default".into(),
            grants,
            diagnostics: Vec::new(),
        })
    }

    /// The renew command exits non-zero naming every refused covered grant,
    /// and zero when only kept, skipped or extended grants remain.
    // trace:TASK-3820 | ai:claude
    #[test]
    fn task_3820_renew_fails_naming_refused_grants_and_audits_them() {
        let ok = report(vec![
            grant("a", ExtensionOutcome::Extended, None),
            grant("b", ExtensionOutcome::Kept, Some("already lasts")),
            grant("c", ExtensionOutcome::Skipped, Some("relay")),
        ]);
        assert!(grants_outcome(&ok).is_ok());
        let lines = grant_audit_lines(&ok);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("grant a (advisor): "));
        let bad = report(vec![
            grant("a", ExtensionOutcome::Extended, None),
            grant(
                "r1",
                ExtensionOutcome::Refused,
                Some("revoked while renewing"),
            ),
            grant("r2", ExtensionOutcome::Refused, Some("unbound")),
        ]);
        let err = grants_outcome(&bad).unwrap_err().to_string();
        assert!(err.contains("2 covered session grant(s)"), "{err}");
        assert!(
            err.contains("r1 (advisor)") && err.contains("r2 (advisor)"),
            "{err}"
        );
        assert!(grant_audit_lines(&bad)
            .iter()
            .any(|l| l == "grant r1 (advisor): refused: revoked while renewing"));
        let unreachable: Result<GrantExtensionReport> = Err(anyhow::anyhow!("no TTY"));
        assert!(grants_outcome(&unreachable).is_err());
        assert_eq!(grant_audit_lines(&unreachable).len(), 1);
    }
}

#[cfg(test)]
#[path = "tests/bug_2704_conflict_version_tests.rs"]
mod bug_2704_conflict_version_tests;
