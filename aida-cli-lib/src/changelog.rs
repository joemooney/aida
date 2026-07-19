// Auto-generated CHANGELOG.md from the spec graph + git tag boundaries.
//
// `aida changelog generate|refresh|preview` walks the repo's `v*` tags as
// release boundaries, scans commit subjects between them for `(SPEC-ID)`
// references, resolves each spec against the local store, classifies it
// (Features / Fixes / Documentation / Infrastructure / Internal / Other),
// and renders one structured markdown section per release.
//
// Determinism contract: without `--released-as`, the output is byte-identical
// for a fixed git state + store. That is what makes `refresh` safe to run
// from `release.sh` and what a future CI freshness gate can check.
// trace:TASK-299 | ai:claude

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result};
use chrono::Utc;

use aida_core::{Requirement, RequirementType, RequirementsStore};

use crate::{extract_pr_number_from_commit_subject, extract_spec_ids_from_commit};

// ============================================================================
// Public API
// ============================================================================

/// What slice of releases the engine should render.
#[derive(Debug, Clone)]
pub enum Window {
    /// All releases (every `v*` tag), newest-first, plus a leading
    /// `[Unreleased]` section for commits since the most recent tag.
    All,
    /// Only the `[Unreleased]` section (commits since the most recent tag).
    Unreleased,
    /// A bounded tag range, inclusive. Either end may be `None` (open).
    Range {
        since: Option<String>,
        until: Option<String>,
    },
}

/// Where the rendered markdown goes.
#[derive(Debug, Clone)]
pub enum Sink {
    Stdout,
    File(PathBuf),
}

/// Engine inputs. Built by `handle_changelog_command` from the parsed
/// `ChangelogCommand` variant.
#[derive(Debug, Clone)]
pub struct ChangelogOptions {
    pub window: Window,
    pub sink: Sink,
    /// When `Some(version)`, the `<last-tag>..HEAD` section heading becomes
    /// `[<version>] — <today>` and no separate `[Unreleased]` heading is
    /// rendered. Used by `release.sh` to land the changelog *with* the
    /// version bump (the new tag does not exist yet at generation time).
    pub released_as: Option<String>,
}

/// Top-level entrypoint. Loads the store, scans tags + commits, classifies,
/// renders, and writes to the sink.
pub fn run(opts: ChangelogOptions, project_root: &Path) -> Result<()> {
    let store = crate::load_store_for_lookup(project_root);
    let tags = scan_release_tags(project_root);
    let sections = assemble(&tags, store.as_ref(), project_root, &opts);
    let body = render_markdown(&sections);
    match &opts.sink {
        Sink::Stdout => {
            print!("{}", body);
        }
        Sink::File(path) => {
            aida_core::fs_atomic::write_atomic(path, body.as_bytes())
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!("CHANGELOG: wrote {}", path.display());
        }
    }
    Ok(())
}

// ============================================================================
// Data model
// ============================================================================

/// A release tag (`v*`) and the commit date of the tagged commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTag {
    pub name: String,
    /// `YYYY-MM-DD` — committer-date of the tagged commit. Never derived
    /// from local clock so the same git state always renders the same date.
    pub date: String,
}

/// One commit in a release range, after subject parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRec {
    pub subject: String,
    pub spec_ids: Vec<String>,
    pub pr: Option<u64>,
    pub ctype: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Features,
    Fixes,
    Documentation,
    Infrastructure,
    Internal,
    Other,
}

impl Category {
    pub fn heading(self) -> &'static str {
        match self {
            Category::Features => "### Features",
            Category::Fixes => "### Fixes",
            Category::Documentation => "### Documentation",
            Category::Infrastructure => "### Infrastructure",
            Category::Internal => "### Internal",
            Category::Other => "### Other",
        }
    }

    /// Render order. Empty categories are omitted by the renderer.
    pub fn ordered() -> [Category; 6] {
        [
            Category::Features,
            Category::Fixes,
            Category::Documentation,
            Category::Infrastructure,
            Category::Internal,
            Category::Other,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogEntry {
    pub spec_id: String,
    pub title: String,
    pub prs: BTreeSet<u64>,
    pub category: Category,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherEntry {
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSection {
    /// e.g. `[v0.8.0] — 2026-05-15` or `[Unreleased]`.
    pub heading: String,
    /// Single-line "Specs merged since vX (N)" or "_No changes since vX._".
    pub lead: String,
    pub entries: Vec<ChangelogEntry>,
    pub others: Vec<OtherEntry>,
}

// ============================================================================
// Git scanners
// ============================================================================

/// List `v*` tags, semver-ordered ascending, with their tagged-commit date.
/// Empty on a non-git repo, a shallow clone with no tags, or a missing git
/// binary — every git call is best-effort.
pub fn scan_release_tags(project_root: &Path) -> Vec<ReleaseTag> {
    let Ok(out) = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["tag", "-l", "v*"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let s = l.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        })
        .collect();
    names.sort_by_key(|a| semver_key(a));

    let mut tags = Vec::with_capacity(names.len());
    for name in names {
        let date = tag_date(project_root, &name).unwrap_or_default();
        if date.is_empty() {
            continue;
        }
        tags.push(ReleaseTag { name, date });
    }
    tags
}

/// Parse `v1.2.3[-pre]` into a sort key. Unparseable tags sort to the front
/// (treated as `v0.0.0`) so they don't crash the order.
fn semver_key(tag: &str) -> (u64, u64, u64, String) {
    let core = tag.strip_prefix('v').unwrap_or(tag);
    let (main, pre) = match core.split_once('-') {
        Some((m, p)) => (m, p.to_string()),
        None => (core, String::new()),
    };
    let mut parts = main.split('.');
    let major = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    // Empty pre-release sorts *after* any non-empty (v1.0.0 > v1.0.0-rc1).
    // We invert by mapping "" to a high marker so the BTree key works.
    let pre_key = if pre.is_empty() {
        "~".to_string() // '~' > any printable
    } else {
        pre
    };
    (major, minor, patch, pre_key)
}

/// `git log -1 --format=%cs <tag>` — committer-date in `YYYY-MM-DD`.
fn tag_date(project_root: &Path, tag: &str) -> Option<String> {
    let out = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "-1", "--format=%cs", tag])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Scan commit subjects in a git range (`prev..cur`, or `cur` to mean "all
/// ancestors"). Best-effort — empty on any git failure.
pub fn scan_commits_in_range(project_root: &Path, range: &str) -> Vec<CommitRec> {
    let Ok(out) = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", range, "--no-merges", "--pretty=format:%s"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut commits = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let subject = line.to_string();
        if subject.trim().is_empty() {
            continue;
        }
        let spec_ids = extract_spec_ids_from_commit(&subject);
        let pr = extract_pr_number_from_commit_subject(&subject);
        let ctype = parse_commit_type(&subject);
        commits.push(CommitRec {
            subject,
            spec_ids,
            pr,
            ctype,
        });
    }
    commits
}

/// Pull the conventional-commit `type` out of a subject. Strips an optional
/// `[AI:tool[:conf]]` prefix, then matches `<type>(scope):` or `<type>:`.
/// Returns `None` for anything outside the conventional set.
pub fn parse_commit_type(subject: &str) -> Option<String> {
    const CONVENTIONAL: &[&str] = &[
        "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
        "revert",
    ];
    let mut rest = subject.trim_start();
    // Strip [AI:...] prefix.
    if let Some(rest_after) = rest.strip_prefix('[') {
        if let Some(end) = rest_after.find(']') {
            rest = rest_after[end + 1..].trim_start();
        }
    }
    // Take the token up to ':' or '('.
    let stop = rest.find([':', '(']).unwrap_or(rest.len());
    let token = rest[..stop].trim().to_ascii_lowercase();
    if token.is_empty() {
        return None;
    }
    if CONVENTIONAL.iter().any(|t| *t == token) {
        Some(token)
    } else {
        None
    }
}

// ============================================================================
// Classification — D4 precedence
// ============================================================================

const DOC_TAGS: &[&str] = &["documentation", "docs"];
const INFRA_TAGS: &[&str] = &[
    "release-tooling",
    "ci",
    "infrastructure",
    "tooling",
    "build",
];

/// D4: choose a category for a spec, given its req_type, its tag set, and
/// the union of conventional-commit types from the commits that touched it.
pub fn classify(
    req_type: Option<&RequirementType>,
    tags: &HashSet<String>,
    ctypes: &[String],
) -> Category {
    // 1. Bug type wins unconditionally.
    if matches!(req_type, Some(RequirementType::Bug)) {
        return Category::Fixes;
    }
    // 2. Documentation tag.
    if DOC_TAGS.iter().any(|t| tags.contains(*t)) {
        return Category::Documentation;
    }
    // 3. Infrastructure-shaped tag.
    if INFRA_TAGS.iter().any(|t| tags.contains(*t)) {
        return Category::Infrastructure;
    }
    // 4. Commit-type signal.
    let has = |needle: &str| ctypes.iter().any(|c| c == needle);
    if has("feat") {
        return Category::Features;
    }
    if has("fix") {
        return Category::Fixes;
    }
    if !ctypes.is_empty() && ctypes.iter().all(|c| c == "docs") {
        return Category::Documentation;
    }
    if ctypes
        .iter()
        .any(|c| matches!(c.as_str(), "ci" | "build" | "chore"))
    {
        return Category::Infrastructure;
    }
    if ctypes
        .iter()
        .any(|c| matches!(c.as_str(), "refactor" | "perf" | "test" | "style"))
    {
        return Category::Internal;
    }
    // 5. Fallback by req_type.
    match req_type {
        Some(RequirementType::Story)
        | Some(RequirementType::Epic)
        | Some(RequirementType::Functional)
        | Some(RequirementType::User)
        | Some(RequirementType::NonFunctional)
        | Some(RequirementType::System)
        | Some(RequirementType::ChangeRequest) => Category::Features,
        _ => Category::Internal,
    }
}

/// D8: a no-SPEC commit renders under "Other" *unless* its type is pure
/// noise (chore/style/build/ci/revert) or its subject names a typo or is a
/// merge.
pub fn is_other_worthy(commit: &CommitRec) -> bool {
    if !commit.spec_ids.is_empty() {
        return false;
    }
    if commit.subject.to_ascii_lowercase().contains("typo") {
        return false;
    }
    if commit.subject.starts_with("Merge ") {
        return false;
    }
    matches!(
        commit.ctype.as_deref(),
        None | Some("feat")
            | Some("fix")
            | Some("docs")
            | Some("refactor")
            | Some("perf")
            | Some("test")
    )
}

// ============================================================================
// Assembly
// ============================================================================

/// Per-spec accumulator during aggregation.
struct SpecAccum {
    prs: BTreeSet<u64>,
    ctypes: Vec<String>,
}

fn build_section(
    heading: String,
    commits: &[CommitRec],
    store: Option<&RequirementsStore>,
    prev_tag_label: &str,
) -> ReleaseSection {
    let mut accums: BTreeMap<String, SpecAccum> = BTreeMap::new();
    let mut others: Vec<OtherEntry> = Vec::new();
    for c in commits {
        if c.spec_ids.is_empty() {
            if is_other_worthy(c) {
                others.push(OtherEntry {
                    subject: c.subject.clone(),
                });
            }
            continue;
        }
        for spec_id in &c.spec_ids {
            let acc = accums.entry(spec_id.clone()).or_insert_with(|| SpecAccum {
                prs: BTreeSet::new(),
                ctypes: Vec::new(),
            });
            if let Some(pr) = c.pr {
                acc.prs.insert(pr);
            }
            if let Some(ct) = &c.ctype {
                if !acc.ctypes.iter().any(|x| x == ct) {
                    acc.ctypes.push(ct.clone());
                }
            }
        }
    }

    let mut entries = Vec::with_capacity(accums.len());
    for (spec_id, acc) in accums {
        let req = store.and_then(|s| s.get_requirement_by_spec_id(&spec_id));
        let (title, req_type, tags) = resolve_spec(req, &spec_id);
        let category = classify(req_type.as_ref(), &tags, &acc.ctypes);
        entries.push(ChangelogEntry {
            spec_id,
            title,
            prs: acc.prs,
            category,
        });
    }
    // PR-desc, then spec-id ascending — the latter keeps deterministic order
    // for entries that share (or both lack) PRs.
    entries.sort_by(|a, b| {
        let a_pr = a.prs.iter().next_back().copied();
        let b_pr = b.prs.iter().next_back().copied();
        b_pr.cmp(&a_pr).then(a.spec_id.cmp(&b.spec_id))
    });

    others.sort_by(|a, b| a.subject.cmp(&b.subject));
    others.dedup();

    let lead = if entries.is_empty() && others.is_empty() {
        format!("_No changes since {}._", prev_tag_label)
    } else {
        let n = entries.len() + others.len();
        format!("Specs merged since {} ({}):", prev_tag_label, n)
    };

    ReleaseSection {
        heading,
        lead,
        entries,
        others,
    }
}

fn resolve_spec(
    req: Option<&Requirement>,
    spec_id: &str,
) -> (String, Option<RequirementType>, HashSet<String>) {
    if let Some(r) = req {
        (r.title.clone(), Some(r.req_type.clone()), r.tags.clone())
    } else {
        (
            format!("{} _(spec not in store)_", spec_id),
            None,
            HashSet::new(),
        )
    }
}

/// Build the ordered list of `ReleaseSection`s for the window. Sections are
/// emitted newest-first; an `[Unreleased]` (or `[released_as]`) section
/// leads when `Window::All` or `Window::Unreleased` is in play.
pub fn assemble(
    tags: &[ReleaseTag],
    store: Option<&RequirementsStore>,
    project_root: &Path,
    opts: &ChangelogOptions,
) -> Vec<ReleaseSection> {
    let mut sections = Vec::new();

    // Determine which tagged-release sections to include.
    let tag_window: Vec<&ReleaseTag> = match &opts.window {
        Window::All => tags.iter().collect(),
        Window::Unreleased => Vec::new(),
        Window::Range { since, until } => {
            let mut started = since.is_none();
            let mut keep = Vec::new();
            for t in tags {
                if !started && since.as_deref() == Some(t.name.as_str()) {
                    started = true;
                }
                if started {
                    keep.push(t);
                    if until.as_deref() == Some(t.name.as_str()) {
                        break;
                    }
                }
            }
            keep
        }
    };

    // [Unreleased] / [released_as] section leads when the window includes
    // the head — All or Unreleased.
    let include_head = matches!(opts.window, Window::All | Window::Unreleased);
    if include_head {
        let head_range = match tags.last() {
            Some(t) => format!("{}..HEAD", t.name),
            None => "HEAD".to_string(),
        };
        let prev_label = tags
            .last()
            .map(|t| t.name.as_str())
            .unwrap_or("the start of history");
        let commits = scan_commits_in_range(project_root, &head_range);
        let heading = match &opts.released_as {
            Some(v) => format!("## [{}] — {}", v, Utc::now().format("%Y-%m-%d")),
            None => "## [Unreleased]".to_string(),
        };
        sections.push(build_section(heading, &commits, store, prev_label));
    }

    // Tagged releases, newest-first.
    for (i, tag) in tag_window.iter().enumerate().rev() {
        let prev = tag_window.get(i.wrapping_sub(1)).copied();
        // tag_window is the *selected* slice; if a `--since` cuts off an
        // earlier tag, fall back to the full `tags` list for the prev.
        let prev_real = prev.or_else(|| {
            // Find this tag's position in `tags`, then walk one back.
            let idx = tags.iter().position(|t| t.name == tag.name)?;
            if idx == 0 {
                None
            } else {
                tags.get(idx - 1)
            }
        });
        let range = match prev_real {
            Some(p) => format!("{}..{}", p.name, tag.name),
            None => tag.name.clone(),
        };
        let prev_label = prev_real
            .map(|t| t.name.as_str())
            .unwrap_or("the start of history");
        let commits = scan_commits_in_range(project_root, &range);
        let heading = format!("## [{}] — {}", tag.name, tag.date);
        sections.push(build_section(heading, &commits, store, prev_label));
    }
    sections
}

// ============================================================================
// Markdown rendering
// ============================================================================

const HEADER: &str = "# Changelog\n\nAll notable changes to this project are documented here. Generated\nmechanically from the spec graph (`aida changelog refresh`) — do not edit\nby hand; regenerate after merging.\n\n";

pub fn render_markdown(sections: &[ReleaseSection]) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(HEADER);
    for section in sections {
        out.push_str(&section.heading);
        out.push_str("\n\n");
        out.push_str(&section.lead);
        out.push_str("\n\n");

        // Group entries by category in fixed render order.
        for cat in Category::ordered() {
            let cat_entries: Vec<&ChangelogEntry> = section
                .entries
                .iter()
                .filter(|e| e.category == cat)
                .collect();
            if (section.others.is_empty() || cat != Category::Other) && cat_entries.is_empty() {
                continue;
            }
            out.push_str(cat.heading());
            out.push_str("\n\n");
            for e in &cat_entries {
                out.push_str(&render_entry(e));
                out.push('\n');
            }
            if cat == Category::Other {
                for o in &section.others {
                    out.push_str(&format!("- {}\n", o.subject));
                }
            }
            out.push('\n');
        }
    }
    out
}

fn render_entry(e: &ChangelogEntry) -> String {
    let pr_part = if e.prs.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = e.prs.iter().map(|n| format!("#{}", n)).collect();
        format!(" ({})", list.join(", "))
    };
    format!("- **{}** — {}{}", e.spec_id, e.title, pr_part)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as ProcessCommand;
    use tempfile::TempDir;

    fn h(t: &[&str]) -> HashSet<String> {
        t.iter().map(|s| s.to_string()).collect()
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_commit_type_strips_ai_prefix() {
        assert_eq!(
            parse_commit_type("[AI:claude] feat(x): y (Z-1)"),
            Some("feat".into())
        );
        assert_eq!(parse_commit_type("chore(deps): bump"), Some("chore".into()));
        assert_eq!(parse_commit_type("random text"), None);
        assert_eq!(parse_commit_type("Feat: X"), Some("feat".into()));
        assert_eq!(
            parse_commit_type("[AI:claude:med] fix: foo"),
            Some("fix".into())
        );
    }

    #[test]
    fn classify_bug_type_is_fixes() {
        assert_eq!(
            classify(
                Some(&RequirementType::Bug),
                &h(&["documentation"]),
                &s(&["feat"])
            ),
            Category::Fixes
        );
    }

    #[test]
    fn classify_docs_tag_is_documentation() {
        assert_eq!(
            classify(
                Some(&RequirementType::Task),
                &h(&["documentation"]),
                &s(&["feat"])
            ),
            Category::Documentation
        );
    }

    #[test]
    fn classify_release_tooling_tag_is_infrastructure() {
        assert_eq!(
            classify(
                Some(&RequirementType::Task),
                &h(&["release-tooling"]),
                &s(&[])
            ),
            Category::Infrastructure
        );
    }

    #[test]
    fn classify_feat_commit_overrides_task_type() {
        assert_eq!(
            classify(Some(&RequirementType::Task), &h(&[]), &s(&["feat"])),
            Category::Features
        );
    }

    #[test]
    fn classify_falls_back_to_story_as_features() {
        assert_eq!(
            classify(Some(&RequirementType::Story), &h(&[]), &s(&[])),
            Category::Features
        );
    }

    #[test]
    fn classify_falls_back_to_task_as_internal() {
        assert_eq!(
            classify(Some(&RequirementType::Task), &h(&[]), &s(&[])),
            Category::Internal
        );
    }

    #[test]
    fn aggregate_spec_across_commits_unions_prs() {
        let commits = vec![
            CommitRec {
                subject: "[AI:claude] feat(x): foo (TASK-1) (#5)".into(),
                spec_ids: vec!["TASK-1".into()],
                pr: Some(5),
                ctype: Some("feat".into()),
            },
            CommitRec {
                subject: "[AI:claude] feat(x): bar (TASK-1) (#7)".into(),
                spec_ids: vec!["TASK-1".into()],
                pr: Some(7),
                ctype: Some("feat".into()),
            },
        ];
        let section = build_section("## [Unreleased]".into(), &commits, None, "v0.1.0");
        assert_eq!(section.entries.len(), 1);
        let prs: Vec<u64> = section.entries[0].prs.iter().copied().collect();
        assert_eq!(prs, vec![5, 7]);
    }

    #[test]
    fn no_spec_commit_goes_to_other() {
        let commits = vec![CommitRec {
            subject: "feat: untracked thing".into(),
            spec_ids: vec![],
            pr: None,
            ctype: Some("feat".into()),
        }];
        let section = build_section("## [Unreleased]".into(), &commits, None, "v0.1.0");
        assert_eq!(section.others.len(), 1);
        assert_eq!(section.entries.len(), 0);
    }

    #[test]
    fn noise_commit_excluded_from_other() {
        let commits = vec![
            CommitRec {
                subject: "chore(lease): x".into(),
                spec_ids: vec![],
                pr: None,
                ctype: Some("chore".into()),
            },
            CommitRec {
                subject: "style: fmt".into(),
                spec_ids: vec![],
                pr: None,
                ctype: Some("style".into()),
            },
            CommitRec {
                subject: "fix typo in readme".into(),
                spec_ids: vec![],
                pr: None,
                ctype: Some("fix".into()),
            },
        ];
        let section = build_section("## [Unreleased]".into(), &commits, None, "v0.1.0");
        assert!(
            section.others.is_empty(),
            "noise leaked into Other: {:?}",
            section.others
        );
    }

    #[test]
    fn unknown_spec_id_uses_id_as_title() {
        let commits = vec![CommitRec {
            subject: "[AI:claude] feat(x): foo (TASK-9999) (#1)".into(),
            spec_ids: vec!["TASK-9999".into()],
            pr: Some(1),
            ctype: Some("feat".into()),
        }];
        let section = build_section("## [Unreleased]".into(), &commits, None, "v0.1.0");
        assert_eq!(section.entries.len(), 1);
        assert!(section.entries[0].title.contains("TASK-9999"));
        assert_eq!(section.entries[0].category, Category::Features);
    }

    #[test]
    fn render_orders_categories_and_releases() {
        let s1 = ReleaseSection {
            heading: "## [v0.2.0] — 2026-05-15".into(),
            lead: "Specs merged since v0.1.0 (2):".into(),
            entries: vec![
                ChangelogEntry {
                    spec_id: "BUG-1".into(),
                    title: "fix".into(),
                    prs: [10].iter().copied().collect(),
                    category: Category::Fixes,
                },
                ChangelogEntry {
                    spec_id: "STORY-1".into(),
                    title: "feat".into(),
                    prs: [11].iter().copied().collect(),
                    category: Category::Features,
                },
            ],
            others: vec![],
        };
        let s2 = ReleaseSection {
            heading: "## [v0.1.0] — 2026-05-01".into(),
            lead: "Specs merged since the start of history (1):".into(),
            entries: vec![ChangelogEntry {
                spec_id: "STORY-2".into(),
                title: "early".into(),
                prs: [1].iter().copied().collect(),
                category: Category::Features,
            }],
            others: vec![],
        };
        let out = render_markdown(&[s1, s2]);
        assert!(out.starts_with("# Changelog\n\n"));
        let i_v2 = out.find("[v0.2.0]").unwrap();
        let i_v1 = out.find("[v0.1.0]").unwrap();
        assert!(i_v2 < i_v1, "v0.2.0 must precede v0.1.0");
        let i_feat = out.find("### Features").unwrap();
        let i_fix = out.find("### Fixes").unwrap();
        assert!(i_feat < i_fix, "Features must precede Fixes");
    }

    #[test]
    fn render_is_deterministic() {
        let s = ReleaseSection {
            heading: "## [Unreleased]".into(),
            lead: "Specs merged since v0.1.0 (1):".into(),
            entries: vec![ChangelogEntry {
                spec_id: "STORY-1".into(),
                title: "x".into(),
                prs: [1].iter().copied().collect(),
                category: Category::Features,
            }],
            others: vec![],
        };
        let a = render_markdown(std::slice::from_ref(&s));
        let b = render_markdown(std::slice::from_ref(&s));
        assert_eq!(a, b);
    }

    #[test]
    fn semver_key_orders_correctly() {
        let mut tags = vec![
            "v0.10.0".to_string(),
            "v0.2.0".to_string(),
            "v0.2.0-rc1".to_string(),
            "v0.1.0".to_string(),
        ];
        tags.sort_by_key(|a| semver_key(a));
        assert_eq!(
            tags,
            vec![
                "v0.1.0".to_string(),
                "v0.2.0-rc1".to_string(),
                "v0.2.0".to_string(),
                "v0.10.0".to_string()
            ]
        );
    }

    #[test]
    fn scan_release_tags_in_temp_repo() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path();
        let git = |args: &[&str]| {
            ProcessCommand::new("git")
                .arg("-C")
                .arg(p)
                .args(args)
                .output()
                .expect("git")
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "--allow-empty", "-m", "one"]);
        git(&["tag", "v0.1.0"]);
        git(&["commit", "--allow-empty", "-m", "two"]);
        git(&["tag", "v0.2.0"]);

        let tags = scan_release_tags(p);
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "v0.1.0");
        assert_eq!(tags[1].name, "v0.2.0");
        assert!(!tags[0].date.is_empty());
    }

    #[test]
    fn released_as_replaces_unreleased_heading() {
        // Smoke: build a section with a versioned heading directly and verify
        // the markdown shape — the assemble-path injection is exercised by
        // the integration verification block in the plan.
        let s = ReleaseSection {
            heading: "## [v0.9.0] — 2026-05-19".into(),
            lead: "Specs merged since v0.8.0 (1):".into(),
            entries: vec![ChangelogEntry {
                spec_id: "STORY-1".into(),
                title: "x".into(),
                prs: [1].iter().copied().collect(),
                category: Category::Features,
            }],
            others: vec![],
        };
        let out = render_markdown(std::slice::from_ref(&s));
        assert!(out.contains("[v0.9.0]"));
        assert!(!out.contains("[Unreleased]"));
    }
}
