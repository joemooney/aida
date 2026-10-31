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

/// One git invocation in the store repository; `Err` carries git's stderr.
// trace:BUG-2179 | ai:claude
fn store_git(
    store_root: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> std::result::Result<Vec<u8>, String> {
    use std::io::Write;
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
    let Some(input) = stdin else {
        // TASK-1662: the history walks (`git log --all -G …` over the whole
        // store) have a deadline; a timeout is an unreadable history, which
        // the ship path already refuses as HistoryUnreadable.
        // trace:TASK-1951 trace:TASK-1662 | ai:claude
        let budget = store_history_timeout();
        return match crate::command_output_with_timeout_detail(cmd, budget) {
            crate::BoundedCommandOutput::Completed(out) => git_output(out),
            crate::BoundedCommandOutput::SpawnFailed => Err("git failed to run".to_string()),
            // TASK-2141: name the knob, so a cold-cache walk on a large
            // store has a way forward. trace:TASK-1951 trace:TASK-2141 | ai:claude
            crate::BoundedCommandOutput::TimedOut => Err(format!(
                "store history read timed out after {}s; raise {STORE_HISTORY_TIMEOUT_ENV} \
                 (seconds) for a large or cold store",
                budget.as_secs()
            )),
        };
    };
    let mut child = cmd.spawn().map_err(|e| format!("git failed to run: {e}"))?;
    // A writer thread, so a large reply cannot deadlock against the input.
    let mut pipe = child.stdin.take().ok_or("git stdin unavailable")?;
    let input = input.to_vec();
    let writer = std::thread::spawn(move || pipe.write_all(&input));
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git failed: {e}"))?;
    writer
        .join()
        .map_err(|_| "git stdin writer panicked".to_string())?
        .map_err(|e| format!("cannot write to git: {e}"))?;
    git_output(out)
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

    /// Read every version of each of `spec_ids`. `Err` when the store history
    /// cannot be walked at all.
    pub(crate) fn read(
        store_root: &Path,
        spec_ids: &[String],
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
            let parsed = body
                .ok_or_else(|| format!("cannot read {key} at {spec}: missing object"))
                .and_then(|text| {
                    parse_version(&String::from_utf8_lossy(text))
                        .map_err(|e| format!("{key} at {spec}: {e}"))
                });
            match parsed {
                Ok(req) => {
                    versions.extend(field_change_versions(&req));
                    versions.push(req);
                }
                Err(e) => {
                    batch.versions.insert(key.clone(), Err(e));
                }
            }
        }
        Ok(batch)
    }
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
pub(crate) fn store_history_alias_map(
    store_root: &Path,
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
    if found.is_empty() {
        return Ok(found);
    }
    let alternation = found.keys().cloned().collect::<Vec<_>>().join("|");
    let pattern = format!("^(spec_id|agreed_id): *['\"]?({alternation})['\"]? *$");
    let line_re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| format!("cannot search store history: {e}"))?;
    let out = store_git(
        store_root,
        &[
            "log",
            "--all",
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
            &format!("-G{pattern}"),
            "--",
            "objects/",
        ],
        None,
    )
    .map_err(|e| format!("cannot search store history for {}: {e}", ids.join(", ")))?;
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
        let (hit, other) = match &pair {
            Some((old, new)) if removed => (Some(old.as_str()), Some(new.as_str())),
            Some((old, new)) => (Some(new.as_str()), Some(old.as_str())),
            None => (None, None),
        };
        // TASK-1800 (comment 24): the history read below finds versions only
        // at an object's canonical path. A hit anywhere else would read as no
        // history and drop the ID as dangling, so it refuses instead.
        // trace:TASK-1800 | ai:claude
        let stem = hit.and_then(canonical_object_stem).ok_or_else(|| {
            format!(
                "store history carries {} outside a canonical object path ({})",
                &caps[2],
                hit.unwrap_or("no file")
            )
        })?;
        let id = caps[2].to_ascii_uppercase();
        // The other side of a rename only adds versions to judge. TASK-2238:
        // a non-canonical other side would be versions never judged, so it
        // refuses like a non-canonical hit rather than being dropped.
        // trace:TASK-2238 | ai:claude
        let renamed = match other.filter(|other| Some(*other) != hit) {
            None => None,
            Some(other) => Some(canonical_object_stem(other).ok_or_else(|| {
                format!(
                    "store history renames {} to or from a non-canonical object path ({other})",
                    &caps[2]
                )
            })?),
        };
        if let Some(aliases) = found.get_mut(&id) {
            for stem in std::iter::once(stem).chain(renamed) {
                let stem = stem.to_ascii_uppercase();
                if stem != id && !aliases.contains(&stem) {
                    aliases.push(stem);
                }
            }
        }
    }
    Ok(found)
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
            reason: mandate.reason.clone(),
            renewed_from: Some(renewal.previous_expires_at.to_rfc3339()),
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(project_root, &event);
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
            reason: mandate.reason.clone(),
            renewed_from: None,
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(project_root, &event);
}

// ── Display ─────────────────────────────────────────────────────────────────

/// The statusline segment: `MANDATE 3h12m` while one is active.
// trace:STORY-1698 | ai:claude
pub(crate) fn statusline_segment(state: &MandateState, now: DateTime<Utc>) -> Option<String> {
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
    Some(parts.join(" · "))
}

/// The [`status_line`] for the mandates in `dir` right now, with the human
/// gates of `audit_root` — what `aida mandate status --line` prints.
// trace:TASK-1955 | ai:claude (TASK-1806)
pub(crate) fn current_status_line(
    dir: &Path,
    audit_root: Option<&Path>,
    now: DateTime<Utc>,
) -> Option<String> {
    let (valid, _) = load_valid(dir, &crate::current_user_id(None), now);
    let state = state_at(&valid, now);
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
        "MANDATE {} {how}; the advisor seat no longer acts under it (a human starts a new one with `aida mandate start`)",
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
        ];
        if let Some(reason) = &m.reason {
            lines.push(format!("  reason:    {reason}"));
        }
        // trace:STORY-B6B1278890774EF993D2EBC327862470-166 | ai:claude
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

/// `aida mandate start|renew|stop|status`.
// trace:STORY-1698 | ai:claude
pub(crate) fn handle_mandate(action: &crate::cli::MandateAction) -> Result<()> {
    let dir = mandate_dir()?;
    let audit_root = crate::find_main_worktree_root().ok();
    match action {
        crate::cli::MandateAction::Start {
            ship_held,
            scopes,
            drive,
            ttl,
            reason,
            include_supervised,
            exclude_supervised,
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
                None => DEFAULT_TTL,
            };
            // STORY-1700: a drive mandate covers sensitive specs unless the
            // human opts out; the gate decides, and binds the human floor.
            // trace:STORY-1700 | ai:claude
            let m = start_as_human(
                &dir,
                &crate::current_user_id(None),
                ttl,
                reason.clone(),
                *drive,
                *include_supervised,
                *exclude_supervised,
                parsed_scopes,
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
        crate::cli::MandateAction::Renew { duration } => {
            // Same parser and 24h cap as `start --ttl`, named for this flag.
            let ttl = duration
                .as_deref()
                .map(|raw| {
                    parse_ttl(raw)
                        .map_err(|e| anyhow::anyhow!(format!("{e:#}").replace("--ttl", "--for")))
                })
                .transpose()?;
            let (m, renewal) =
                renew_as_human(&dir, &crate::current_user_id(None), ttl, Utc::now())?;
            if let Some(root) = &audit_root {
                emit_renewed(root, &m, &renewal);
            }
            println!(
                "Mandate {} renewed until {} (was {}); its scope, flags and reason are unchanged (renewal {}).",
                m.id,
                renewal.expires_at.to_rfc3339(),
                renewal.previous_expires_at.to_rfc3339(),
                m.renewals.len()
            );
            Ok(())
        }
        crate::cli::MandateAction::Stop => {
            let stopped = stop_as_human(&dir, &crate::current_user_id(None), Utc::now())?;
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
            let (valid, warnings) = load_valid(&dir, &crate::current_user_id(None), now);
            let state = state_at(&valid, now);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            print!(
                "{}",
                status_report(&state, now, audit_root.as_deref(), *json)
            );
            Ok(())
        }
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
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <MandateArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let MandateArgs { action } = &args;
    return self::handle_mandate(action);
}

// trace:STORY-1728 | ai:claude
pub(crate) static MANDATE: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "mandate",
        help_after: "merge-hold",
        command: <MandateArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_mandate),
    };
