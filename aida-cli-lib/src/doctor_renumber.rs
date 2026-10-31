//! `aida doctor --fix id-collisions`: repair numeric ID collisions.
//!
//! Spec IDs that differ only by leading zeros (`TASK-0042` / `TASK-42`) are
//! the same ID (BUG-HOME-046), so a store holding both has a collision group.
//! For each group one object keeps the number (the survivor) and every other
//! object is renumbered to a fresh ID; exact-case, word-boundary mentions of
//! the old form are rewritten in other store objects and in this checkout's
//! git-tracked text (Markdown docs and CHANGELOG whole-file; source files on
//! comment lines only, `trace:` comments included). The old -> new mapping,
//! plus the `origin/main` commits whose message cited the old literal
//! (immutable, so their git-log linkage now points at the survivor), is
//! recorded in the renumbered spec's history.
//!
//! The repair runs in two halves so the store is never written before
//! review: `--dry-run --mapping FILE` plans the renumbers and saves the
//! old -> new mapping; `--tree-only --mapping FILE` applies that mapping to
//! the tracked tree (the reviewable code/doc half); `--mapping FILE --yes`
//! later applies the same mapping to the store (refusing before any write if
//! a target ID has been taken meanwhile).
//!
//! Survivor rule, per group: most commit-message references on `origin/main`
//! (commit messages cannot be rewritten), then most store + tracked-tree
//! references, then the unpadded form.
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

use aida_core::models::{Comment, FieldChange, HistoryEntry, Requirement};

/// Author recorded on renumber history entries.
pub(crate) const RENUMBER_AUTHOR: &str = "aida-renumber";

/// How far past the dispenser's next number a dry-run plans fresh IDs, so
/// the IDs `aida add` mints while the plan is under review do not land on
/// them. The skipped numbers are harmless: minting skips any taken number.
pub(crate) const RENUMBER_HEADROOM: u32 = 100;

/// Options for one `aida doctor --fix id-collisions` run.
#[derive(Debug, Clone, Default)]
pub(crate) struct FixOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub mapping: Option<PathBuf>,
    pub tree_only: bool,
    /// Spec IDs whose own text the store apply leaves as written (the spec
    /// that owns the migration records the old -> new table in its comments).
    /// Empty means [`DEFAULT_PRESERVE`].
    pub preserve: Vec<String>,
}

/// The spec that owns the leading-zero migration (`BUG-HOME-046` on its home
/// node): its mapping table and keep/renumber lists must survive the apply.
/// Absent from other stores, where the default preserves nothing.
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
pub(crate) const DEFAULT_PRESERVE: &str = "BUG-B6B1278890774EF993D2EBC327862470-046";

fn id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '$'
}

/// Byte offsets of every word-boundary, EXACT-CASE occurrence of `literal`
/// in `text`. A match must not be preceded or followed by a character that
/// could continue an id, so `TASK-42` is not found inside `TASK-420` or
/// `BUG-TASK-42`. Case is significant so a lowercase path or link such as
/// `task-42.md` (a file name, not an ID mention) is never touched.
pub(crate) fn find_id_tokens(text: &str, literal: &str) -> Vec<usize> {
    let lit = literal.trim();
    if lit.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(lit) {
        let i = from + rel;
        let before_ok = i == 0 || !text[..i].chars().next_back().is_some_and(id_char);
        let after = &text[i + lit.len()..];
        // A trailing `-` ends the ID only when no alphanumeric follows it,
        // so `TASK-42-2` (a different id) is not matched.
        let after_ok = match after.chars().next() {
            None => true,
            Some('-') => !after[1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric()),
            Some(c) => !id_char(c),
        };
        if before_ok && after_ok {
            out.push(i);
            from = i + lit.len();
        } else {
            from = i + lit.chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// Replace every [`find_id_tokens`] match of `old` with `new`; returns the
/// new text and the number of replacements.
pub(crate) fn replace_id_tokens(text: &str, old: &str, new: &str) -> (String, usize) {
    let hits = find_id_tokens(text, old);
    if hits.is_empty() {
        return (text.to_string(), 0);
    }
    let len = old.trim().len();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for &at in &hits {
        out.push_str(&text[last..at]);
        out.push_str(new);
        last = at + len;
    }
    out.push_str(&text[last..]);
    (out, hits.len())
}

/// Apply every `(old, new)` mapping to `text`.
pub(crate) fn rewrite_all(text: &str, map: &[(String, String)]) -> (String, usize) {
    let mut cur = text.to_string();
    let mut total = 0;
    for (old, new) in map {
        let (next, n) = replace_id_tokens(&cur, old, new);
        cur = next;
        total += n;
    }
    (cur, total)
}

fn rewrite_comments(comments: &mut [Comment], map: &[(String, String)]) -> usize {
    let mut n = 0;
    for c in comments {
        let (text, k) = rewrite_all(&c.content, map);
        if k > 0 {
            c.content = text;
            n += k;
        }
        n += rewrite_comments(&mut c.replies, map);
    }
    n
}

/// Rewrite title, description and comment text of `req`; returns the count.
pub(crate) fn rewrite_requirement_text(req: &mut Requirement, map: &[(String, String)]) -> usize {
    let mut n = 0;
    let (title, k) = rewrite_all(&req.title, map);
    if k > 0 {
        req.title = title;
        n += k;
    }
    let (desc, k) = rewrite_all(&req.description, map);
    if k > 0 {
        req.description = desc;
        n += k;
    }
    n + rewrite_comments(&mut req.comments, map)
}

fn requirement_text_refs(req: &Requirement, literal: &str) -> usize {
    fn comments(cs: &[Comment], literal: &str) -> usize {
        cs.iter()
            .map(|c| find_id_tokens(&c.content, literal).len() + comments(&c.replies, literal))
            .sum()
    }
    find_id_tokens(&req.title, literal).len()
        + find_id_tokens(&req.description, literal).len()
        + comments(&req.comments, literal)
}

/// Evidence for one member of a collision group.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub uuid: Uuid,
    pub literal: String,
    pub title: String,
    pub commit_shas: Vec<String>,
    pub other_refs: usize,
}

impl Member {
    fn padded(&self) -> bool {
        self.literal
            .rsplit_once('-')
            .is_some_and(|(_, seq)| seq.len() > 1 && seq.starts_with('0'))
    }
}

/// Index of the survivor: most commit refs, then most other refs, then the
/// unpadded form, then the lower UUID (total order).
pub(crate) fn choose_survivor(members: &[Member]) -> usize {
    (0..members.len())
        .max_by(|&a, &b| {
            let (x, y) = (&members[a], &members[b]);
            x.commit_shas
                .len()
                .cmp(&y.commit_shas.len())
                .then(x.other_refs.cmp(&y.other_refs))
                .then(y.padded().cmp(&x.padded()))
                .then(y.uuid.cmp(&x.uuid))
        })
        .unwrap_or(0)
}

/// One planned renumber.
#[derive(Debug, Clone)]
pub(crate) struct PlannedRenumber {
    pub key: String,
    pub survivor: Member,
    pub renumber: Member,
}

/// `(sha, message)` for every commit reachable from `rev`, or empty when the
/// rev does not exist (no remote, a fresh fixture).
fn commit_messages(project_root: &Path, rev: &str) -> Vec<(String, String)> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", rev, "--format=%H%x1f%B%x1e"])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .split('\u{1e}')
        .filter_map(|rec| {
            let (sha, body) = rec.trim_start_matches('\n').split_once('\u{1f}')?;
            Some((sha.trim().to_string(), body.to_string()))
        })
        .collect()
}

fn citing_shas(commits: &[(String, String)], literal: &str) -> Vec<String> {
    commits
        .iter()
        .filter(|(_, msg)| !find_id_tokens(msg, literal).is_empty())
        .map(|(sha, _)| sha.clone())
        .collect()
}

/// Git-tracked files under `project_root`.
fn tracked_files(project_root: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "-z"])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// The byte offset where a source line's comment begins, if it has one:
/// a whole-line comment (`//`, `#`, `*`, `/*`, `<!--`, `--`) starts at the
/// first non-blank character, a trailing `//` comment at its `//`.
fn comment_start(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start().len();
    let t = &line[indent..];
    if ["//", "#", "*", "/*", "<!--", "--"]
        .iter()
        .any(|m| t.starts_with(m))
    {
        return Some(indent);
    }
    line.find("//")
}

/// Whether `path` is prose rewritten whole-file: any Markdown file, and
/// every text file under `docs/` (plans, TSV/CSV/YAML data, notes), whose
/// ID mentions are all references, never code fixtures.
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
pub(crate) fn is_prose_path(path: &str) -> bool {
    path.ends_with(".md") || path.starts_with("docs/")
}

/// Rewrite one tracked file's text: whole-file for prose
/// ([`is_prose_path`]), and for every other file only `trace:` lines and the
/// comment part of each line. Code and string literals stay (they may be
/// fixtures), so a lookup literal in code keeps resolving numerically to the
/// survivor.
pub(crate) fn rewrite_tracked_text(
    path: &str,
    text: &str,
    map: &[(String, String)],
) -> (String, usize) {
    if is_prose_path(path) {
        return rewrite_all(text, map);
    }
    let mut total = 0;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let start = if line.contains("trace:") {
            Some(0)
        } else {
            comment_start(line)
        };
        match start {
            Some(at) => {
                let (tail, n) = rewrite_all(&line[at..], map);
                total += n;
                out.push_str(&line[..at]);
                out.push_str(&tail);
            }
            None => out.push_str(line),
        }
    }
    (out, total)
}

fn tree_refs(project_root: &Path, files: &[String], literal: &str) -> usize {
    let map = vec![(literal.to_string(), String::new())];
    files
        .iter()
        .filter_map(|f| {
            std::fs::read_to_string(project_root.join(f))
                .ok()
                .map(|t| (f, t))
        })
        .map(|(f, t)| rewrite_tracked_text(f, &t, &map).1)
        .sum()
}

/// Numeric collision groups in `reqs`: id key -> the objects sharing it
/// (only keys held by two or more distinct UUIDs).
pub(crate) fn collision_groups(reqs: &[Requirement]) -> BTreeMap<String, Vec<&Requirement>> {
    let mut groups: BTreeMap<String, Vec<&Requirement>> = BTreeMap::new();
    for r in reqs {
        if let Some(sid) = r.spec_id.as_deref().filter(|s| !s.trim().is_empty()) {
            groups
                .entry(aida_core::id_identity::id_key(sid))
                .or_default()
                .push(r);
        }
    }
    groups.retain(|_, v| v.iter().map(|r| r.id).collect::<HashSet<_>>().len() > 1);
    groups
}

/// Plan the renumbers for every numeric collision group in `reqs`.
pub(crate) fn plan(
    reqs: &[Requirement],
    project_root: &Path,
    main_rev: &str,
) -> Vec<PlannedRenumber> {
    let groups = collision_groups(reqs);
    if groups.is_empty() {
        return Vec::new();
    }
    let commits = commit_messages(project_root, main_rev);
    let files = tracked_files(project_root);
    let mut out = Vec::new();
    for (key, group) in groups {
        let members: Vec<Member> = group
            .iter()
            .map(|r| {
                let literal = r.spec_id.clone().unwrap_or_default();
                let store_refs: usize = reqs
                    .iter()
                    .filter(|o| o.id != r.id)
                    .map(|o| requirement_text_refs(o, &literal))
                    .sum();
                Member {
                    uuid: r.id,
                    literal: literal.clone(),
                    title: r.title.clone(),
                    commit_shas: citing_shas(&commits, &literal),
                    other_refs: store_refs + tree_refs(project_root, &files, &literal),
                }
            })
            .collect();
        let survivor = choose_survivor(&members);
        for (i, m) in members.iter().enumerate() {
            if i != survivor {
                out.push(PlannedRenumber {
                    key: key.clone(),
                    survivor: members[survivor].clone(),
                    renumber: m.clone(),
                });
            }
        }
    }
    out
}

/// One line of the old -> new mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MappingRow {
    pub old: String,
    pub new: String,
    pub uuid: Uuid,
}

const MAPPING_HEADER: &str = "\
# Spec ID renumber mapping for `aida doctor --fix id-collisions`.
# One renumber per line: old ID <TAB> new ID <TAB> requirement UUID.
# Apply to the store with: aida doctor --fix id-collisions --mapping <this file> --yes
";

/// The mapping file text for `rows`.
pub(crate) fn render_mapping(rows: &[MappingRow]) -> String {
    let mut out = MAPPING_HEADER.to_string();
    for r in rows {
        out.push_str(&format!("{}\t{}\t{}\n", r.old, r.new, r.uuid));
    }
    out
}

/// Parse a mapping file (blank and `#` lines ignored).
pub(crate) fn parse_mapping(text: &str) -> Result<Vec<MappingRow>> {
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [old, new, uuid] = fields.as_slice() else {
            anyhow::bail!(
                "mapping line {}: expected `old new uuid`, got {line:?}",
                n + 1
            );
        };
        rows.push(MappingRow {
            old: (*old).to_string(),
            new: (*new).to_string(),
            uuid: Uuid::parse_str(uuid)
                .with_context(|| format!("mapping line {}: bad uuid {uuid:?}", n + 1))?,
        });
    }
    Ok(rows)
}

/// Everything wrong with applying `rows` to `reqs`; empty when the mapping
/// is safe to apply. `holder` answers which UUID already holds an ID.
pub(crate) fn mapping_problems(
    reqs: &[Requirement],
    rows: &[MappingRow],
    holder: &dyn Fn(&str) -> Result<Option<Uuid>>,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut new_keys = HashSet::new();
    for r in rows {
        match reqs.iter().find(|q| q.id == r.uuid) {
            None => out.push(format!("{}: no requirement has UUID {}", r.old, r.uuid)),
            Some(q) => {
                let filed = q.spec_id.as_deref().unwrap_or("");
                if !filed.trim().eq_ignore_ascii_case(r.old.trim()) {
                    out.push(format!(
                        "{}: requirement {} is now filed as {filed}",
                        r.old, r.uuid
                    ));
                }
            }
        }
        if aida_core::id_identity::IdKey::parse(&r.new).is_none()
            || !aida_core::object_store::valid_spec_id_format(
                &aida_core::object_store::canonical_spec_id(&r.new),
            )
        {
            out.push(format!("{} -> {}: not a valid spec ID", r.old, r.new));
            continue;
        }
        if !new_keys.insert(aida_core::id_identity::id_key(&r.new)) {
            out.push(format!("{}: listed twice as a new ID", r.new));
        }
        if let Some(h) = holder(&r.new)? {
            out.push(format!(
                "{} -> {}: {} is already used by requirement {h}",
                r.old, r.new, r.new
            ));
        }
    }
    Ok(out)
}

/// The history entry recorded on a renumbered spec.
pub(crate) fn renumber_history(old: &str, new: &str, shas: &[String]) -> HistoryEntry {
    let mut changes = vec![FieldChange {
        field_name: "spec_id".to_string(),
        old_value: old.to_string(),
        new_value: new.to_string(),
    }];
    if !shas.is_empty() {
        changes.push(FieldChange {
            field_name: "renumber_commit_refs".to_string(),
            old_value: String::new(),
            new_value: format!(
                "origin/main commits citing {old} (now resolve to the survivor): {}",
                shas.join(" ")
            ),
        });
    }
    HistoryEntry::new(RENUMBER_AUTHOR.to_string(), changes)
}

/// Build the renamed rows and the rewritten referrers for `mapping`
/// (`uuid -> (old literal, new id, commit shas)`). Requirements in
/// `preserve` keep their title, description and comments as written.
pub(crate) fn build_rewrites(
    reqs: &[Requirement],
    mapping: &BTreeMap<Uuid, (String, String, Vec<String>)>,
    preserve: &HashSet<Uuid>,
) -> (Vec<(String, Requirement)>, Vec<Requirement>) {
    let map: Vec<(String, String)> = mapping
        .values()
        .map(|(old, new, _)| (old.clone(), new.clone()))
        .collect();
    let mut renames = Vec::new();
    let mut referrers = Vec::new();
    for r in reqs {
        let mut row = r.clone();
        let changed = if preserve.contains(&r.id) {
            0
        } else {
            rewrite_requirement_text(&mut row, &map)
        };
        if let Some((old, new, shas)) = mapping.get(&r.id) {
            row.spec_id = Some(new.clone());
            if row
                .agreed_id
                .as_deref()
                .is_some_and(|a| aida_core::id_identity::same_id(a, old))
            {
                row.agreed_id = None;
            }
            row.history.push(renumber_history(old, new, shas));
            row.modified_at = chrono::Utc::now();
            renames.push((old.clone(), row));
        } else if changed > 0 {
            referrers.push(row);
        }
    }
    (renames, referrers)
}

/// Rewrite this checkout's tracked text for `map` (see
/// [`rewrite_tracked_text`]), skipping `exclude` (the mapping file itself).
/// Returns `(path, replacements)` for every file changed (or that would be,
/// under `dry_run`).
pub(crate) fn rewrite_tree(
    project_root: &Path,
    map: &[(String, String)],
    dry_run: bool,
    exclude: Option<&Path>,
) -> Result<Vec<(String, usize)>> {
    let exclude = exclude.and_then(|p| std::fs::canonicalize(p).ok());
    let mut out = Vec::new();
    for f in tracked_files(project_root) {
        let path = project_root.join(&f);
        if exclude.is_some() && std::fs::canonicalize(&path).ok() == exclude {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (new_text, n) = rewrite_tracked_text(&f, &text, map);
        if n > 0 {
            if !dry_run {
                std::fs::write(&path, new_text)
                    .with_context(|| format!("rewrite {}", path.display()))?;
            }
            out.push((f, n));
        }
    }
    Ok(out)
}

fn pairs(rows: &[MappingRow]) -> Vec<(String, String)> {
    rows.iter()
        .map(|r| (r.old.clone(), r.new.clone()))
        .collect()
}

fn print_files(files: &[(String, usize)]) {
    for (f, n) in files {
        println!("  {f}: {n}");
    }
}

/// UUIDs of the `--preserve` specs (spec or agreed ID, any padding, node
/// labels expanded). A spec named explicitly that is not in the store is an
/// error; the [`DEFAULT_PRESERVE`] default may be absent.
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
pub(crate) fn preserved_uuids(reqs: &[Requirement], ids: &[String]) -> Result<HashSet<Uuid>> {
    let defaults = [DEFAULT_PRESERVE.to_string()];
    let explicit = !ids.is_empty();
    let ids = if explicit { ids } else { &defaults[..] };
    let mut out = HashSet::new();
    for id in ids {
        let want = aida_core::node_alias::expand_or_keep(id);
        let names = |r: &Requirement| {
            [r.spec_id.as_deref(), r.agreed_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| aida_core::id_identity::same_id(s, &want))
        };
        match reqs.iter().find(|r| names(r)) {
            Some(r) => {
                out.insert(r.id);
            }
            None if explicit => anyhow::bail!("--preserve {id}: no such spec in the store"),
            None => {}
        }
    }
    Ok(out)
}

/// Entry point for `aida doctor --fix id-collisions`.
pub(crate) fn run(project_root: &Path, store_root: &Path, opts: &FixOptions) -> Result<()> {
    if !opts.dry_run && !opts.tree_only && !opts.yes {
        anyhow::bail!(
            "renumbering rewrites spec IDs in the store; preview with --dry-run, \
             then re-run with --yes to apply"
        );
    }
    let backend = aida_core::GitBackend::new(store_root)?;
    let reqs = aida_core::object_store::load_all_objects(&store_root.join("objects"))?;
    let saved = match &opts.mapping {
        Some(p) if p.exists() => Some(parse_mapping(
            &std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?,
        )?),
        _ => None,
    };
    if opts.tree_only {
        let rows = saved.context("--tree-only needs an existing --mapping file")?;
        let files = rewrite_tree(
            project_root,
            &pairs(&rows),
            opts.dry_run,
            opts.mapping.as_deref(),
        )?;
        println!(
            "{} {} tracked file(s) for {} renumber(s); the store is untouched.",
            if opts.dry_run {
                "Would rewrite"
            } else {
                "Rewrote"
            },
            files.len(),
            rows.len()
        );
        print_files(&files);
        return Ok(());
    }
    let rows = match saved {
        Some(rows) => {
            println!(
                "Using the mapping in {} ({} renumber(s)).",
                opts.mapping.as_deref().unwrap_or(Path::new("?")).display(),
                rows.len()
            );
            rows
        }
        None => {
            if !opts.dry_run && opts.mapping.is_some() {
                anyhow::bail!("mapping file not found; create it with --dry-run --mapping <file>");
            }
            let planned = plan(&reqs, project_root, "origin/main");
            if planned.is_empty() {
                println!("No numeric ID collisions: every spec number is used once.");
                return Ok(());
            }
            println!(
                "{} numeric ID collision(s) — keep / renumber (commit refs on origin/main, other refs):",
                planned.len()
            );
            for p in &planned {
                println!(
                    "  {:<12} keep {} ({} commits, {} refs)  renumber {} ({} commits, {} refs)",
                    p.key,
                    p.survivor.literal,
                    p.survivor.commit_shas.len(),
                    p.survivor.other_refs,
                    p.renumber.literal,
                    p.renumber.commit_shas.len(),
                    p.renumber.other_refs,
                );
            }
            let mut reserved = HashSet::new();
            let mut rows = Vec::new();
            for p in &planned {
                let req = reqs
                    .iter()
                    .find(|r| r.id == p.renumber.uuid)
                    .context("planned object vanished")?;
                let new = if opts.dry_run {
                    backend.peek_spec_id(req, RENUMBER_HEADROOM, &reserved)?
                } else {
                    backend.mint_spec_id(req)?
                };
                reserved.insert(aida_core::id_identity::id_key(&new));
                rows.push(MappingRow {
                    old: p.renumber.literal.clone(),
                    new,
                    uuid: p.renumber.uuid,
                });
            }
            rows
        }
    };
    println!("Old -> new:");
    for r in &rows {
        let title = reqs
            .iter()
            .find(|q| q.id == r.uuid)
            .map_or("", |q| q.title.as_str());
        println!("  {:<14} -> {:<14} {title}", r.old, r.new);
    }
    let problems = mapping_problems(&reqs, &rows, &|id| backend.id_holder(id))?;
    let commits = commit_messages(project_root, "origin/main");
    let mapping: BTreeMap<Uuid, (String, String, Vec<String>)> = rows
        .iter()
        .map(|r| {
            (
                r.uuid,
                (r.old.clone(), r.new.clone(), citing_shas(&commits, &r.old)),
            )
        })
        .collect();
    let preserve = preserved_uuids(&reqs, &opts.preserve)?;
    let (renames, referrers) = build_rewrites(&reqs, &mapping, &preserve);
    if opts.dry_run {
        for p in &problems {
            println!("  warning: {p}");
        }
        if let Some(path) = opts.mapping.as_deref().filter(|p| !p.exists()) {
            std::fs::write(path, render_mapping(&rows))
                .with_context(|| format!("write {}", path.display()))?;
            println!("Saved the mapping to {}.", path.display());
        }
        let files = rewrite_tree(project_root, &pairs(&rows), true, opts.mapping.as_deref())?;
        println!(
            "Would renumber {} spec(s), rewrite {} store referrer(s) and {} tracked file(s).",
            renames.len(),
            referrers.len(),
            files.len()
        );
        print_files(&files);
        println!("(dry run: nothing written to the store)");
        return Ok(());
    }
    if !problems.is_empty() {
        anyhow::bail!(
            "the mapping cannot be applied; nothing was written:\n  {}",
            problems.join("\n  ")
        );
    }
    backend.renumber_requirements(&renames, &referrers, "doctor")?;
    let files = rewrite_tree(project_root, &pairs(&rows), false, opts.mapping.as_deref())?;
    println!(
        "Renumbered {} spec(s); rewrote {} store referrer(s) and {} tracked file(s).",
        renames.len(),
        referrers.len(),
        files.len()
    );
    print_files(&files);
    let after = aida_core::object_store::load_all_objects(&store_root.join("objects"))?;
    println!(
        "Numeric ID collision groups remaining: {}.",
        collision_groups(&after).len()
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/bug_046_doctor_renumber_tests.rs"]
mod tests;
