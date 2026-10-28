//! Dry-run migration inventory: what a project with AIDA scaffolding
//! tracked on its product branch would move to the adoption plane and which
//! tracked files it could then remove.
//!
//! The inventory reads the product branch's committed tree (`HEAD`) through
//! Git objects and compares it with what this build's scaffolder generates.
//! It writes nothing: no worktree file, no index entry, no ref, no store
//! object. Removal stays a separate, reviewed product change; the inventory
//! only proposes it. A file is proposed for removal only when it is
//! non-executable guidance the plane can deliver to agent launches
//! (generated Markdown such as `docs/agents/*.md`) and its committed bytes
//! are exactly what this build generates, so an AIDA header alone (copied
//! or spoofed) never makes a file removable. Hooks, commands, skills, harness settings and the
//! Codex/Antigravity artifacts are executable configuration: the plane never
//! carries them (they stay under config-trust), so they are always kept.
//! Authored or mixed instruction files (`AGENTS.md`, `CLAUDE.md`), seed
//! starters such as `README.md`, and `.gitignore` are product content. A
//! tracked `.aida/config.toml` is split into the policy keys the plane
//! admits and the keys that stay in trusted or local configuration;
//! executable settings never become shared settings.
// trace:STORY-1741 | ai:claude

use crate::adoption_plane::{self, PolicySplit};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a tracked file relates to what AIDA generates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Byte for byte what this build generates.
    Generated,
    /// Carries the AIDA generated header but differs from this build's
    /// output: edited, another version, or a spoofed header.
    GeneratedEdited,
    /// Authored text that may hold an AIDA-managed part.
    Mixed,
    /// The tracked project configuration.
    Config,
    /// On an AIDA path, but nothing shows AIDA generated it.
    Ambiguous,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Generated => "generated",
            Class::GeneratedEdited => "generated, edited",
            Class::Mixed => "authored or mixed",
            Class::Config => "project config",
            Class::Ambiguous => "ambiguous",
        }
    }
}

/// What the inventory proposes for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proposal {
    /// It can be removed from the product branch in a reviewed change.
    Remove,
    /// It can be removed once its policy settings are on the plane.
    RemoveAfterPublishing,
    /// Someone has to look before anything happens to it.
    Review,
    /// It stays on the product branch.
    Keep,
}

impl Proposal {
    pub fn label(self) -> &'static str {
        match self {
            Proposal::Remove => "can be removed",
            Proposal::RemoveAfterPublishing => "can be removed after publishing its settings",
            Proposal::Review => "review first",
            Proposal::Keep => "keep",
        }
    }
}

/// One tracked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub class: Class,
    pub proposal: Proposal,
    pub detail: String,
}

/// The whole inventory.
#[derive(Debug, Clone, PartialEq)]
pub struct Inventory {
    /// The product commit inventoried; `None` before the first commit.
    pub commit: Option<String>,
    pub entries: Vec<Entry>,
    /// The tracked config's split, when the product branch tracks one.
    pub config: Option<PolicySplit>,
}

impl Inventory {
    /// Every path the inventory proposes removing outright.
    pub fn removable(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(|e| e.proposal == Proposal::Remove)
            .map(|e| e.path.as_str())
    }
}

const CONFIG: &str = ".aida/config.toml";
const MAX_BLOB_BYTES: u64 = 1024 * 1024;

/// Inventory the committed tree of the repository holding `project_root`.
pub fn inventory(project_root: &Path) -> Result<Inventory> {
    let root = PathBuf::from(git(project_root, &["rev-parse", "--show-toplevel"])?.trim());
    let Some(commit) = head(&root)? else {
        return Ok(Inventory {
            commit: None,
            entries: Vec::new(),
            config: None,
        });
    };
    let expected = generated(&root);
    let listing = git(&root, &["ls-tree", "-r", "--full-tree", "-z", &commit])?;
    let mut entries = Vec::new();
    let mut config = None;
    for record in listing.split('\0').filter(|r| !r.is_empty()) {
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        // A seed starter (README.md) is product content even while it still
        // matches AIDA's starter text.
        if is_seed(path) && !on_aida_path(path) {
            continue;
        }
        if !(expected.contains_key(path) || on_aida_path(path)) {
            continue;
        }
        let mut fields = meta.split(' ');
        let (mode, kind, blob) = (fields.next(), fields.next(), fields.next());
        let (Some(mode), Some("blob"), Some(blob)) = (mode, kind, blob) else {
            continue;
        };
        let text = if mode == "120000" {
            None
        } else {
            read_blob(&root, blob)?
        };
        let entry = match text {
            None => Entry {
                path: path.to_string(),
                class: Class::Ambiguous,
                proposal: Proposal::Review,
                detail: "a symlink or binary file; AIDA never generates one".into(),
            },
            Some(text) if path == CONFIG => {
                let (entry, split) = classify_config(&text);
                config = split;
                entry
            }
            Some(text) => {
                let expected = expected.get(path).map(String::as_str);
                // A path off the AIDA paths is the product's own unless
                // AIDA's output or header is in it.
                let generated =
                    expected.is_some_and(|e| crate::scaffolding::generated_text_matches(&text, e));
                if !on_aida_path(path) && !generated && !has_generated_header(&text) {
                    continue;
                }
                let Some(entry) = classify(path, &text, expected) else {
                    continue;
                };
                entry
            }
        };
        entries.push(entry);
    }
    Ok(Inventory {
        commit: Some(commit),
        entries,
        config,
    })
}

/// This build's scaffold output, by path. Read-only: the preview only
/// reads the worktree to describe what a write would do.
fn generated(root: &Path) -> BTreeMap<String, String> {
    let mut scaffolder = crate::scaffolding::Scaffolder::new(
        root.to_path_buf(),
        crate::scaffolding::ScaffoldConfig::default(),
    );
    scaffolder
        .preview(&crate::models::RequirementsStore::new())
        .artifacts
        .into_iter()
        .filter(|a| a.category() != crate::scaffolding::FileCategory::Seed)
        .map(|a| (a.path.to_string_lossy().replace('\\', "/"), a.content))
        .collect()
}

/// Paths AIDA scaffolding writes, whether or not this build generates them.
fn on_aida_path(path: &str) -> bool {
    const FILES: &[&str] = &["AGENTS.md", "CLAUDE.md", ".mcp.json", ".gitignore"];
    const DIRS: &[&str] = &[".aida/", ".codex/", ".agents/", ".antigravity/"];
    let claude_owned = path.strip_prefix(".claude/").is_some_and(|rest| {
        rest == "settings.json"
            || rest.starts_with("hooks/")
            || [
                "skills/aida-",
                "commands/aida-",
                "skills/aida/",
                "commands/aida/",
            ]
            .iter()
            .any(|p| rest.starts_with(p))
    });
    FILES.contains(&path) || DIRS.iter().any(|d| path.starts_with(d)) || claude_owned
}

fn is_seed(path: &str) -> bool {
    crate::scaffolding::FileCategory::from_path(Path::new(path))
        == crate::scaffolding::FileCategory::Seed
}

/// Non-executable guidance the plane can deliver to agent launches as
/// guidance documents: generated Markdown outside the harness directories
/// (the agent docs under `docs/agents/`). `CLAUDE.local.md` is loaded by
/// the harness itself, so it is not.
fn plane_guidance(path: &str) -> bool {
    path.ends_with(".md") && !executable(path) && !is_seed(path) && path != "CLAUDE.local.md"
}

/// Executable harness configuration: hooks, commands, skills, harness
/// settings and the Codex/Antigravity artifacts. The plane never carries
/// these; they stay under config-trust on the product branch.
fn executable(path: &str) -> bool {
    [".claude/", ".codex/", ".agents/", ".antigravity/"]
        .iter()
        .any(|d| path.starts_with(d))
        || path == ".mcp.json"
        || path.ends_with(".sh")
}

const EXECUTABLE_KEPT: &str = "executable harness configuration (hook, command, skill or \
     harness settings); the adoption plane carries only inert settings and guidance, so it \
     stays on the product branch under config-trust";

/// `None` when the file carries nothing of AIDA's (a product-only
/// `.gitignore`).
fn classify(path: &str, text: &str, expected: Option<&str>) -> Option<Entry> {
    let entry = |class, proposal, detail: &str| {
        Some(Entry {
            path: path.to_string(),
            class,
            proposal,
            detail: detail.to_string(),
        })
    };
    let generated = expected.is_some_and(|e| crate::scaffolding::generated_text_matches(text, e));
    match path {
        "AGENTS.md" | "CLAUDE.md" => {
            return entry(
                Class::Mixed,
                Proposal::Keep,
                "authored agent instructions; AIDA never removes or rewrites them. An AIDA \
                 block in it can be deleted by hand once the plane carries that guidance",
            )
        }
        ".gitignore" => {
            let lines: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with('#') && l.contains(".aida"))
                .collect();
            if lines.is_empty() {
                return None;
            }
            return entry(
                Class::Mixed,
                Proposal::Keep,
                &format!(
                    "AIDA entries can be deleted by hand (adoption ignores its files \
                     through .git/info/exclude): {}",
                    lines.join(", ")
                ),
            );
        }
        _ => {}
    }
    let class = if generated {
        Class::Generated
    } else if has_generated_header(text) {
        Class::GeneratedEdited
    } else {
        Class::Ambiguous
    };
    if executable(path) {
        return entry(class, Proposal::Keep, EXECUTABLE_KEPT);
    }
    match class {
        Class::Generated if plane_guidance(path) => entry(
            class,
            Proposal::Remove,
            "generated guidance matching this build's output; publish it as plane \
             guidance and agent launches deliver it from there instead",
        ),
        Class::Generated => entry(
            class,
            Proposal::Keep,
            "matches this build's output, but it is not guidance the plane delivers",
        ),
        Class::GeneratedEdited => entry(
            class,
            Proposal::Review,
            "has an AIDA generated header but is not this build's output (edited, another \
             version, or a copied header); review it for local changes before removing",
        ),
        _ => entry(
            class,
            Proposal::Review,
            if path.ends_with(".json") || path.ends_with(".toml") || path.contains("/hooks") {
                "settings with no AIDA marker; they may hold your own entries"
            } else {
                "on an AIDA path but not generated by this build; review it"
            },
        ),
    }
}

fn classify_config(text: &str) -> (Entry, Option<PolicySplit>) {
    let entry = |proposal, detail: String| Entry {
        path: CONFIG.to_string(),
        class: Class::Config,
        proposal,
        detail,
    };
    let Ok(table) = toml::from_str::<toml::Table>(text) else {
        return (
            entry(
                Proposal::Review,
                "does not parse as TOML; nothing can be moved automatically".into(),
            ),
            None,
        );
    };
    let split = adoption_plane::split_policy(&table);
    let moved: usize = split.settings.values().map(BTreeMap::len).sum();
    let destination = adoption_plane::SETTINGS_PATH;
    let e = if split.kept.is_empty() {
        entry(
            Proposal::RemoveAfterPublishing,
            format!("{moved} policy setting(s) move to {destination} on the aida-store plane"),
        )
    } else {
        entry(
            Proposal::Keep,
            format!(
                "{moved} policy setting(s) can move to {destination} on the aida-store plane; \
                 {} other setting(s) stay in trusted or local configuration: {}",
                split.kept.len(),
                split.kept.join(", ")
            ),
        )
    };
    (e, Some(split))
}

/// An AIDA generated header within the first lines (after any shebang or
/// frontmatter).
fn has_generated_header(text: &str) -> bool {
    text.lines()
        .take(12)
        .any(|l| l.contains("AIDA Generated: v"))
}

fn head(root: &Path) -> Result<Option<String>> {
    let out = crate::git_ops::host_git_command(root)
        .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .output()
        .context("Failed to run git rev-parse")?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// A blob's text, or `None` for a binary or oversized blob.
fn read_blob(root: &Path, blob: &str) -> Result<Option<String>> {
    let size: u64 = git(root, &["cat-file", "-s", blob])?.trim().parse()?;
    if size > MAX_BLOB_BYTES {
        return Ok(None);
    }
    let out = crate::git_ops::host_git_command(root)
        .args(["cat-file", "blob", blob])
        .output()
        .context("Failed to run git cat-file")?;
    if !out.status.success() {
        bail!("git cat-file blob {blob} failed");
    }
    Ok(String::from_utf8(out.stdout)
        .ok()
        .filter(|t| !t.contains('\0')))
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git_ops::host_git_command(dir)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

#[cfg(test)]
#[path = "adoption_migration_tests.rs"]
mod tests;
