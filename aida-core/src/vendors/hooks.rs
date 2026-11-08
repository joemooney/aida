//! Codex hook wiring — `.codex/hooks.json` plus the handler scripts.
//!
//! AIDA wires seven Claude Code hook events. Codex supports all seven (its
//! `hooks` feature is stable and on by default), uses the same
//! `matcher`/`hooks`/`hookSpecificOutput` schema, and — verified empirically
//! against codex-cli 0.144.6 — normalizes tool names to Claude's vocabulary,
//! so a `"Bash"` matcher matches a Codex shell call. The wiring therefore
//! ports almost mechanically.
//!
//! ONE SET OF SCRIPTS, TWO WIRINGS. The handlers are NOT forked per vendor.
//! Forking would double every future hook fix and guarantee drift; instead the
//! scripts resolve the project root through a vendor-neutral chain and are
//! copied from the same embedded masters into each vendor's tree. The only
//! per-vendor difference is the wiring file, and `codex_hooks_match_settings`
//! pins it against `settings.json` so the two cannot silently diverge.
//!
//! What differs from the Claude wiring, and why:
//!   - `$CLAUDE_PROJECT_DIR` has no Codex equivalent, so the command uses
//!     `$(git rev-parse --show-toplevel)`, which is what the porting doc
//!     prescribes and which an empirical probe confirmed works.
//!   - handler paths point at `.codex/hooks/` so a codex-only machine needs no
//!     `.claude/` directory at all.
//!
//! trace:TASK-1181 | ai:claude

use std::path::Path;

/// Where the wiring lives, relative to the project root.
// trace:STORY-2539 | ai:codex
pub const HOOKS_JSON_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
    .hooks
    .wiring;
/// Where the handler scripts live, relative to the project root.
pub const HOOKS_DIR_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
    .hooks
    .handlers;

/// The handlers referenced by the Codex wiring.
///
/// Deliberately a subset of `templates/hooks/`: the git hooks
/// (`aida-commit-msg`, `aida-pre-commit.sh`, `aida-post-commit.sh`) are
/// git-level and vendor-independent — they are installed into `.git/hooks`
/// and must NOT be duplicated here.
pub fn handler_names() -> Vec<String> {
    let text = crate::templates::EMBEDDED_TEMPLATES
        .get(
            crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
                .hooks
                .template,
        )
        .copied()
        .unwrap_or("{}");
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(i) = line.find("/.codex/hooks/") {
            let rest = &line[i + "/.codex/hooks/".len()..];
            if let Some(name) = rest
                .split(|c: char| c == '"' || c == '\\' || c.is_whitespace())
                .next()
            {
                let n = name.trim().to_string();
                if !n.is_empty() && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
    }
    names.sort();
    names
}

/// The raw (header-less) body of the Codex handler `name`, exactly as the
/// scaffold preview compares and wraps it. Both Codex handler writers derive
/// their content from this, so `aida init` and `aida scaffold apply` agree.
// trace:BUG-2395 | ai:claude
pub(crate) fn handler_raw_body(name: &str) -> Option<String> {
    crate::templates::EMBEDDED_TEMPLATES
        .get(format!("hooks/{name}").as_str())
        .map(|body| body.trim_end().to_string())
}

/// What a scaffold pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodexHooksOutcome {
    pub written: Vec<String>,
    pub skipped_existing: Vec<String>,
    pub warning: Option<String>,
}

/// Write the Codex hook wiring and handlers under `project_root`.
///
/// Idempotent: an existing file is left alone unless `force`, matching every
/// other AIDA scaffold. A user who edited their wiring keeps it.
fn install_standalone(project_root: &Path, force: bool) -> std::io::Result<CodexHooksOutcome> {
    // trace:STORY-1741 | ai:claude
    crate::scaffolding::refuse_in_adopted_worktree(project_root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
    let mut out = CodexHooksOutcome::default();
    refuse_hook_links(
        project_root,
        HOOKS_JSON_REL,
        HOOKS_DIR_REL,
        &handler_names(),
    )?;
    let dir = project_root.join(HOOKS_DIR_REL);
    std::fs::create_dir_all(&dir)?;

    // Handlers first, so the wiring never points at a script that is not there.
    for name in handler_names() {
        let Some(raw) = handler_raw_body(&name) else {
            continue;
        };
        let dest = dir.join(&name);
        if dest.exists() && !force {
            out.skipped_existing.push(name);
            continue;
        }
        // One canonical on-disk form for both writers: the header-wrapped
        // body `aida scaffold apply` writes, so a fresh `aida init` is not
        // reported as drift by `aida scaffold status`.
        // trace:BUG-2395 | ai:claude
        let rel = Path::new(HOOKS_DIR_REL).join(&name);
        std::fs::write(&dest, crate::scaffolding::wrap_with_aida_header(&rel, &raw))?;
        set_executable(&dest);
        out.written.push(name);
    }

    let wiring = project_root.join(HOOKS_JSON_REL);
    if wiring.exists() && !force {
        out.skipped_existing.push(HOOKS_JSON_REL.to_string());
    } else {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get(
                crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
                    .hooks
                    .template,
            )
            .copied()
            .unwrap_or("{}");
        std::fs::write(&wiring, format!("{}\n", body.trim_end()))?;
        out.written.push(HOOKS_JSON_REL.to_string());
    }
    Ok(out)
}

/// True when the project already carries AIDA's Codex hook wiring: a
/// `.codex/hooks.json` that references a `/.codex/hooks/` handler. This is
/// the opt-in that brings the Codex files into `aida scaffold status / diff /
/// apply / upgrade`. A Claude-only repo, or a `.codex/hooks.json` that never
/// pointed at AIDA's handlers, is left alone.
// trace:BUG-2395 | ai:claude
pub fn codex_hooks_wired(project_root: &Path) -> bool {
    std::fs::read_to_string(project_root.join(HOOKS_JSON_REL))
        .is_ok_and(|raw| raw.contains("/.codex/hooks/"))
}

/// Where the Antigravity wiring lives, relative to the project root.
// trace:TASK-1744 | ai:antigravity
// trace:STORY-2539 | ai:codex
pub const AGY_HOOKS_JSON_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Agy)
    .hooks
    .wiring;
/// Where the Antigravity handler scripts live, relative to the project root.
pub const AGY_HOOKS_DIR_REL: &str = crate::vendors::profile(crate::vendors::LaunchVendor::Agy)
    .hooks
    .handlers;
/// The named hook key AIDA manages inside `.agents/hooks.json`.
pub const AGY_EXPIRY_NOTICE_KEY: &str = "aida-grant-expiry-notice";

/// True when `.agents/hooks.json` is a valid JSON object containing AIDA's
/// `aida-grant-expiry-notice` entry. A third-party `.agents/hooks.json` that
/// lacks AIDA's key returns `false` so re-init repair does not drop unreferenced
/// handler scripts into a repo that opted out.
// trace:TASK-1744 | ai:antigravity
pub fn antigravity_hooks_wired(project_root: &Path) -> bool {
    let wiring = project_root.join(AGY_HOOKS_JSON_REL);
    let Ok(raw) = std::fs::read_to_string(&wiring) else {
        return false;
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    map.contains_key(AGY_EXPIRY_NOTICE_KEY)
}

/// The handlers referenced by the Antigravity wiring.
// trace:TASK-1744 | ai:antigravity
pub fn antigravity_handler_names() -> Vec<String> {
    let text = crate::templates::EMBEDDED_TEMPLATES
        .get(
            crate::vendors::profile(crate::vendors::LaunchVendor::Agy)
                .hooks
                .template,
        )
        .copied()
        .unwrap_or("{}");
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(i) = line.find("/.agents/hooks/") {
            let rest = &line[i + "/.agents/hooks/".len()..];
            if let Some(name) = rest
                .split(|c: char| c == '"' || c == '\\' || c.is_whitespace())
                .next()
            {
                let n = name.trim().to_string();
                if !n.is_empty() && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
    }
    names.sort();
    names
}

/// Order-preserving JSON value so merging `aida-grant-expiry-notice` into a
/// user-owned `.agents/hooks.json` preserves the exact key order of all
/// third-party hooks and their nested fields without requiring workspace-wide
/// `serde_json/preserve_order`.
// trace:TASK-1744 | ai:antigravity
#[derive(Debug, Clone, PartialEq)]
enum OrderedJson {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<OrderedJson>),
    Object(Vec<(String, OrderedJson)>),
}

impl serde::Serialize for OrderedJson {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Number(n) => n.serialize(serializer),
            Self::String(s) => serializer.serialize_str(s),
            Self::Array(items) => {
                use serde::ser::SerializeSeq;
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Object(entries) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (k, v) in entries {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for OrderedJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = OrderedJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any valid JSON value")
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(OrderedJson::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(OrderedJson::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(OrderedJson::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(OrderedJson::Number(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                serde_json::Number::from_f64(v)
                    .map(OrderedJson::Number)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(OrderedJson::String(v.to_owned()))
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
                Ok(OrderedJson::String(v))
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(elem) = seq.next_element()? {
                    items.push(elem);
                }
                Ok(OrderedJson::Array(items))
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut entries: Vec<(String, OrderedJson)> = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, OrderedJson>()? {
                    if let Some((_, existing)) = entries.iter_mut().find(|(ek, _)| *ek == k) {
                        *existing = v;
                    } else {
                        entries.push((k, v));
                    }
                }
                Ok(OrderedJson::Object(entries))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// Write the Antigravity hook wiring and handlers under `project_root`.
///
/// `.agents/hooks.json` is a shared, user-owned file holding named hook entries.
/// When it already exists as a valid JSON object, only AIDA's own keys
/// (`aida-grant-expiry-notice`, `aida-mail-notice`) are inserted when missing
/// or updated (when `force = true`), preserving all other keys and their
/// original key order. If `.agents/hooks.json` exists and is invalid
/// JSON or not a top-level object, it is never overwritten (even with `force = true`),
/// the handler script is not dropped, and a single warning is recorded on the outcome.
// trace:TASK-1744 | ai:antigravity
fn install_named_entries(project_root: &Path, force: bool) -> std::io::Result<CodexHooksOutcome> {
    // trace:STORY-1741 | ai:claude
    crate::scaffolding::refuse_in_adopted_worktree(project_root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
    let mut out = CodexHooksOutcome::default();
    refuse_hook_links(
        project_root,
        AGY_HOOKS_JSON_REL,
        AGY_HOOKS_DIR_REL,
        &antigravity_handler_names(),
    )?;
    let wiring = project_root.join(AGY_HOOKS_JSON_REL);

    let existing_entries = if wiring.exists() {
        let raw = std::fs::read_to_string(&wiring)?;
        match serde_json::from_str::<OrderedJson>(&raw) {
            Ok(OrderedJson::Object(entries)) => Some(entries),
            _ => {
                let msg = format!(
                    "{AGY_HOOKS_JSON_REL} is not a valid top-level JSON object; leaving it untouched and skipping Antigravity hook installation"
                );
                out.warning = Some(msg);
                return Ok(out);
            }
        }
    } else {
        None
    };

    let dir = project_root.join(AGY_HOOKS_DIR_REL);
    std::fs::create_dir_all(&dir)?;

    for name in antigravity_handler_names() {
        let Some(body) = crate::templates::EMBEDDED_TEMPLATES.get(&format!("hooks/{name}") as &str)
        else {
            continue;
        };
        let dest = dir.join(&name);
        if dest.exists() && !force {
            out.skipped_existing.push(name);
            continue;
        }
        std::fs::write(&dest, format!("{}\n", body.trim_end()))?;
        set_executable(&dest);
        out.written.push(name);
    }

    let template_body = crate::templates::EMBEDDED_TEMPLATES
        .get(
            crate::vendors::profile(crate::vendors::LaunchVendor::Agy)
                .hooks
                .template,
        )
        .copied()
        .unwrap_or("{}");
    match existing_entries {
        None => {
            std::fs::write(&wiring, format!("{}\n", template_body.trim_end()))?;
            out.written.push(AGY_HOOKS_JSON_REL.to_string());
        }
        Some(mut entries) => {
            // Every AIDA-managed key in the template: a missing one is added
            // (so re-init brings an older install up to date), an existing
            // one is replaced only under `force`. trace:TASK-2253 | ai:claude
            let template_entries = match serde_json::from_str::<OrderedJson>(template_body) {
                Ok(OrderedJson::Object(t)) => t,
                _ => Vec::new(),
            };
            let mut changed = false;
            for (key, entry) in template_entries {
                if let Some((_, slot)) = entries.iter_mut().find(|(k, _)| *k == key) {
                    if force {
                        *slot = entry;
                        changed = true;
                    }
                } else {
                    entries.push((key, entry));
                    changed = true;
                }
            }
            if !changed {
                out.skipped_existing.push(AGY_HOOKS_JSON_REL.to_string());
            } else {
                let rendered = serde_json::to_string_pretty(&OrderedJson::Object(entries))
                    .map_err(std::io::Error::other)?;
                std::fs::write(&wiring, format!("{rendered}\n"))?;
                out.written.push(AGY_HOOKS_JSON_REL.to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(unix)]
pub(crate) fn set_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(md) = std::fs::metadata(p) {
        let mut perms = md.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(p, perms);
    }
}

#[cfg(not(unix))]
pub(crate) fn set_executable(_p: &Path) {}

// trace:STORY-2539 | ai:codex
impl super::VendorProfile {
    /// Install independent hook wiring. Shared settings use the full scaffolder.
    pub fn scaffold_hooks(&self, root: &Path, force: bool) -> std::io::Result<CodexHooksOutcome> {
        match self.hooks.backend {
            super::HookBackend::Standalone => install_standalone(root, force),
            super::HookBackend::NamedEntries => install_named_entries(root, force),
            super::HookBackend::Settings => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "shared settings hooks require the full scaffolder",
            )),
        }
    }
}

/// Check the complete write set before creating directories or writing handlers.
/// As with the existing scaffolder, this is a local preflight, not race-proof
/// protection against concurrent replacement of directories.
// trace:STORY-2539 | ai:codex
fn refuse_hook_links(
    root: &Path,
    wiring: &str,
    handlers: &str,
    names: &[String],
) -> std::io::Result<()> {
    for rel in std::iter::once(std::path::PathBuf::from(wiring))
        .chain(names.iter().map(|name| Path::new(handlers).join(name)))
    {
        let full = root.join(rel);
        for path in full
            .ancestors()
            .take_while(|path| *path != root)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("refusing hook write through symlink {}", path.display()),
                    ))
                }
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(())
}

// trace:STORY-2539 | ai:codex
pub fn scaffold_codex_hooks(
    root: &std::path::Path,
    force: bool,
) -> std::io::Result<CodexHooksOutcome> {
    crate::vendors::profile(crate::vendors::LaunchVendor::Codex).scaffold_hooks(root, force)
}

pub fn scaffold_antigravity_hooks(
    root: &std::path::Path,
    force: bool,
) -> std::io::Result<CodexHooksOutcome> {
    crate::vendors::profile(crate::vendors::LaunchVendor::Agy).scaffold_hooks(root, force)
}

#[cfg(test)]
mod story_2539_tests {
    use super::*;
    use crate::vendors::{HookBackend, PROFILES};
    use std::collections::BTreeSet;

    // trace:STORY-2539 | ai:codex
    #[test]
    fn story_2539_hook_layout_matches_embedded_fixtures() {
        for profile in &PROFILES {
            let layout = profile.hooks;
            let raw = crate::templates::EMBEDDED_TEMPLATES[layout.template];
            let value: serde_json::Value = serde_json::from_str(raw).unwrap();
            let events: BTreeSet<&str> = match layout.backend {
                HookBackend::Settings | HookBackend::Standalone => value["hooks"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect(),
                HookBackend::NamedEntries => value
                    .as_object()
                    .unwrap()
                    .values()
                    .flat_map(|entry| entry.as_object().unwrap().keys().map(String::as_str))
                    .collect(),
            };
            assert_eq!(
                events,
                layout.events.iter().copied().collect(),
                "{}",
                layout.wiring
            );
            assert!(raw.contains(&format!("/{}/", layout.handlers)));
            if layout.backend != HookBackend::Settings {
                let tmp = tempfile::tempdir().unwrap();
                profile.scaffold_hooks(tmp.path(), false).unwrap();
                assert_eq!(
                    std::fs::read_to_string(tmp.path().join(layout.wiring)).unwrap(),
                    format!("{}\n", raw.trim_end())
                );
            }
        }
    }

    // trace:STORY-2539 | ai:codex
    #[cfg(unix)]
    #[test]
    fn story_2539_hook_writers_refuse_links_before_any_write() {
        use std::os::unix::fs::symlink;
        for profile in &PROFILES {
            if profile.hooks.backend == HookBackend::Settings {
                continue;
            }
            for force in [false, true] {
                let names = match profile.hooks.backend {
                    HookBackend::Standalone => handler_names(),
                    _ => antigravity_handler_names(),
                };
                for rel in [
                    std::path::PathBuf::from(profile.hooks.wiring),
                    std::path::PathBuf::from(profile.hooks.handlers),
                    Path::new(profile.hooks.handlers).join(&names[0]),
                    Path::new(profile.hooks.wiring)
                        .parent()
                        .unwrap()
                        .to_path_buf(),
                ] {
                    let root = tempfile::tempdir().unwrap();
                    let outside = tempfile::tempdir().unwrap();
                    let target = outside.path().join("sentinel");
                    std::fs::write(&target, "keep").unwrap();
                    let dest = root.path().join(&rel);
                    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                    symlink(&target, &dest).unwrap();
                    let err = profile.scaffold_hooks(root.path(), force).unwrap_err();
                    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
                    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
                    assert!(std::fs::symlink_metadata(dest)
                        .unwrap()
                        .file_type()
                        .is_symlink());
                }
            }
        }
    }
}
