//! `aida doctor renumber-id-collisions`: repair numeric ID collisions.
//!
//! Spec IDs that differ only by leading zeros (`TASK-0418` / `TASK-418`) are
//! the same ID (BUG-HOME-046), so a store holding both has a collision group.
//! For each group one object keeps the number (the survivor) and every other
//! object is renumbered to a freshly minted ID; literal word-boundary mentions
//! of the old form are rewritten in other store objects and in this checkout's
//! git-tracked docs (`*.md`) and `trace:` comments. The old -> new mapping, plus
//! the `origin/main` commits whose message cited the old literal (immutable,
//! so their git-log linkage now points at the survivor), is recorded in the
//! renumbered spec's history.
//!
//! Survivor rule, per group: most commit-message references on `origin/main`
//! (commit messages cannot be rewritten), then most store + tracked-tree
//! references, then the unpadded form.
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use uuid::Uuid;

use aida_core::models::{Comment, FieldChange, HistoryEntry, Requirement};

/// Author recorded on renumber history entries.
pub(crate) const RENUMBER_AUTHOR: &str = "aida-renumber";

fn id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '$'
}

/// Byte offsets of every word-boundary, ASCII-case-insensitive occurrence of
/// `literal` in `text`. A match must not be preceded or followed by a
/// character that could continue an id, so `TASK-418` is not found inside
/// `TASK-4180` or `BUG-TASK-418`.
pub(crate) fn find_id_tokens(text: &str, literal: &str) -> Vec<usize> {
    let lit = literal.trim();
    if lit.is_empty() {
        return Vec::new();
    }
    let hay = text.as_bytes();
    let needle = lit.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            let before_ok = i == 0 || !text[..i].chars().next_back().is_some_and(id_char);
            let after = &text[i + needle.len()..];
            // A trailing `-` is fine (`TASK-418-style`) only when not followed
            // by a digit, so `TASK-418-2` (a different id) is not matched.
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
                i += needle.len();
                continue;
            }
        }
        i += 1;
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

/// Git-tracked files under `project_root` eligible for rewriting: Markdown
/// docs anywhere plus any text file (rewritten only on `trace:` lines).
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

/// Rewrite one tracked file's text: whole-file for Markdown, `trace:` lines
/// only for everything else (source literals stay — they may be fixtures).
pub(crate) fn rewrite_tracked_text(
    path: &str,
    text: &str,
    map: &[(String, String)],
) -> (String, usize) {
    if path.ends_with(".md") {
        return rewrite_all(text, map);
    }
    let mut total = 0;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.contains("trace:") {
            let (l, n) = rewrite_all(line, map);
            total += n;
            out.push_str(&l);
        } else {
            out.push_str(line);
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

/// Plan the renumbers for every numeric collision group in `reqs`.
pub(crate) fn plan(
    reqs: &[Requirement],
    project_root: &Path,
    main_rev: &str,
) -> Vec<PlannedRenumber> {
    let mut groups: BTreeMap<String, Vec<&Requirement>> = BTreeMap::new();
    for r in reqs {
        if let Some(sid) = r.spec_id.as_deref().filter(|s| !s.trim().is_empty()) {
            groups
                .entry(aida_core::id_identity::id_key(sid))
                .or_default()
                .push(r);
        }
    }
    groups.retain(|_, v| {
        v.iter()
            .map(|r| r.id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1
    });
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
                let commit_shas = commits
                    .iter()
                    .filter(|(_, msg)| !find_id_tokens(msg, &literal).is_empty())
                    .map(|(sha, _)| sha.clone())
                    .collect();
                let store_refs: usize = reqs
                    .iter()
                    .filter(|o| o.id != r.id)
                    .map(|o| requirement_text_refs(o, &literal))
                    .sum();
                Member {
                    uuid: r.id,
                    literal: literal.clone(),
                    title: r.title.clone(),
                    commit_shas,
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
/// (`uuid -> (old literal, new id, commit shas)`).
pub(crate) fn build_rewrites(
    reqs: &[Requirement],
    mapping: &BTreeMap<Uuid, (String, String, Vec<String>)>,
) -> (Vec<(String, Requirement)>, Vec<Requirement>) {
    let map: Vec<(String, String)> = mapping
        .values()
        .map(|(old, new, _)| (old.clone(), new.clone()))
        .collect();
    let mut renames = Vec::new();
    let mut referrers = Vec::new();
    for r in reqs {
        let mut row = r.clone();
        let changed = rewrite_requirement_text(&mut row, &map);
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

/// Rewrite this checkout's tracked docs and trace comments. Returns
/// `(path, replacements)` for every file changed.
pub(crate) fn rewrite_tree(
    project_root: &Path,
    map: &[(String, String)],
    dry_run: bool,
) -> Result<Vec<(String, usize)>> {
    let mut out = Vec::new();
    for f in tracked_files(project_root) {
        let path = project_root.join(&f);
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

/// Entry point for `aida doctor renumber-id-collisions`.
pub(crate) fn run(project_root: &Path, store_root: &Path, dry_run: bool, yes: bool) -> Result<()> {
    let backend = aida_core::GitBackend::new(store_root)?;
    let reqs = aida_core::object_store::load_all_objects(&store_root.join("objects"))?;
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
            "  {:<12} keep {} ({} commits, {} refs)  renumber {} ({} commits, {} refs) — {}",
            p.key,
            p.survivor.literal,
            p.survivor.commit_shas.len(),
            p.survivor.other_refs,
            p.renumber.literal,
            p.renumber.commit_shas.len(),
            p.renumber.other_refs,
            p.renumber.title
        );
    }
    if dry_run {
        println!("(dry run: nothing written)");
        return Ok(());
    }
    if !yes {
        anyhow::bail!(
            "renumbering rewrites spec IDs; re-run with --yes to apply (or --dry-run to preview)"
        );
    }
    let mut mapping: BTreeMap<Uuid, (String, String, Vec<String>)> = BTreeMap::new();
    for p in &planned {
        let req = reqs
            .iter()
            .find(|r| r.id == p.renumber.uuid)
            .context("planned object vanished")?;
        let new_id = backend.mint_spec_id(req)?;
        println!("  {} -> {}", p.renumber.literal, new_id);
        mapping.insert(
            p.renumber.uuid,
            (
                p.renumber.literal.clone(),
                new_id,
                p.renumber.commit_shas.clone(),
            ),
        );
    }
    let (renames, referrers) = build_rewrites(&reqs, &mapping);
    backend.renumber_requirements(&renames, &referrers, "doctor")?;
    let map: Vec<(String, String)> = mapping
        .values()
        .map(|(o, n, _)| (o.clone(), n.clone()))
        .collect();
    let files = rewrite_tree(project_root, &map, false)?;
    println!(
        "Renumbered {} spec(s); rewrote {} store referrer(s) and {} tracked file(s).",
        renames.len(),
        referrers.len(),
        files.len()
    );
    for (f, n) in files {
        println!("  {f}: {n}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_respect_id_boundaries() {
        let t = "see TASK-418, not TASK-4180 or BUG-TASK-418 or TASK-418-2; (task-418)";
        assert_eq!(find_id_tokens(t, "TASK-418").len(), 2);
        let (out, n) = replace_id_tokens(t, "TASK-418", "TASK-HOME-007");
        assert_eq!(n, 2);
        assert!(out.starts_with("see TASK-HOME-007, not TASK-4180"));
        assert!(out.ends_with("(TASK-HOME-007)"));
        // The padded and unpadded literals are distinct text.
        assert!(find_id_tokens("TASK-0418", "TASK-418").is_empty());
    }

    fn member(literal: &str, commits: usize, refs: usize) -> Member {
        Member {
            uuid: Uuid::new_v4(),
            literal: literal.into(),
            title: String::new(),
            commit_shas: (0..commits).map(|i| format!("sha{i}")).collect(),
            other_refs: refs,
        }
    }

    #[test]
    fn survivor_has_most_commit_refs_then_refs_then_unpadded() {
        let m = [member("TASK-0418", 3, 0), member("TASK-418", 1, 50)];
        assert_eq!(choose_survivor(&m), 0);
        let m = [member("TASK-0418", 1, 9), member("TASK-418", 1, 2)];
        assert_eq!(choose_survivor(&m), 0);
        let m = [member("TASK-0418", 0, 0), member("TASK-418", 0, 0)];
        assert_eq!(choose_survivor(&m), 1, "tie keeps the unpadded side");
    }

    #[test]
    fn tracked_rewrite_touches_docs_and_trace_lines_only() {
        let map = vec![("TASK-0418".to_string(), "TASK-HOME-001".to_string())];
        let rs = "let fixture = \"TASK-0418\";\n// trace:TASK-0418 | ai:claude\n";
        let (out, n) = rewrite_tracked_text("src/x.rs", rs, &map);
        assert_eq!(n, 1);
        assert!(out.contains("\"TASK-0418\""));
        assert!(out.contains("trace:TASK-HOME-001"));
        let (out, n) = rewrite_tracked_text("CHANGELOG.md", "- fixed TASK-0418\n", &map);
        assert_eq!((out.as_str(), n), ("- fixed TASK-HOME-001\n", 1));
    }

    #[test]
    fn build_rewrites_renames_records_history_and_rewrites_referrers() {
        let mut a = Requirement::new("padded".into(), "dup of TASK-418".into());
        a.spec_id = Some("TASK-0418".into());
        let mut b = Requirement::new("plain".into(), String::new());
        b.spec_id = Some("TASK-418".into());
        let mut c = Requirement::new("referrer".into(), "see TASK-0418".into());
        c.spec_id = Some("TASK-9".into());
        let reqs = vec![a.clone(), b, c];
        let mut mapping = BTreeMap::new();
        mapping.insert(
            a.id,
            (
                "TASK-0418".to_string(),
                "TASK-HOME-001".to_string(),
                vec!["abc".to_string()],
            ),
        );
        let (renames, referrers) = build_rewrites(&reqs, &mapping);
        assert_eq!(renames.len(), 1);
        let (old, row) = &renames[0];
        assert_eq!(old, "TASK-0418");
        assert_eq!(row.spec_id.as_deref(), Some("TASK-HOME-001"));
        let h = row.history.last().unwrap();
        assert_eq!(h.author, RENUMBER_AUTHOR);
        assert_eq!(h.changes[0].old_value, "TASK-0418");
        assert!(h.changes[1].new_value.contains("abc"));
        // The unpadded literal in a's own text is a different literal: kept.
        assert_eq!(row.description, "dup of TASK-418");
        assert_eq!(referrers.len(), 1);
        assert_eq!(referrers[0].description, "see TASK-HOME-001");
    }
}
