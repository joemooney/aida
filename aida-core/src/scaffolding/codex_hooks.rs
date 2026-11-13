//! HookBackend installation adapters for standalone and named-entry wiring.
//! Layouts/events come from VendorProfile; shared settings keep their full
//! scaffolder. Shared embedded handlers and compatibility entry points remain
//! here, with BUG-718/BUG-1645 per-file symlink skip-and-warn semantics.
// trace:STORY-2539 | ai:codex
use crate::vendors::hooks::{hook_template, layout_handler_names, AGY_LAYOUT, CODEX_LAYOUT};
pub use crate::vendors::hooks::{
    AGY_EXPIRY_NOTICE_KEY, AGY_HOOKS_DIR_REL, AGY_HOOKS_JSON_REL, HOOKS_DIR_REL, HOOKS_JSON_REL,
};
use crate::vendors::{HookBackend, HookLayout};
use std::path::Path;

pub fn handler_names() -> Vec<String> {
    layout_handler_names(CODEX_LAYOUT)
}
pub fn antigravity_handler_names() -> Vec<String> {
    layout_handler_names(AGY_LAYOUT)
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
fn install_standalone(
    project_root: &Path,
    force: bool,
    layout: HookLayout,
) -> std::io::Result<CodexHooksOutcome> {
    // trace:STORY-1741 | ai:claude
    crate::scaffolding::refuse_in_adopted_worktree(project_root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
    let mut out = CodexHooksOutcome::default();
    let body = hook_template(layout).map_err(std::io::Error::other)?;
    let dir = project_root.join(layout.handlers);

    // Handlers first, so the wiring never points at a script that is not there.
    for name in layout_handler_names(layout) {
        let Some(raw) = handler_raw_body(&name) else {
            continue;
        };
        let dest = dir.join(&name);
        if skip_link(
            project_root,
            &Path::new(layout.handlers).join(&name),
            &mut out,
        ) {
            continue;
        }
        if dest.exists() && !force {
            out.skipped_existing.push(name);
            continue;
        }
        // One canonical on-disk form for both writers: the header-wrapped
        // body `aida scaffold apply` writes, so a fresh `aida init` is not
        // reported as drift by `aida scaffold status`.
        // trace:BUG-2395 | ai:claude
        let rel = Path::new(layout.handlers).join(&name);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&dest, crate::scaffolding::wrap_with_aida_header(&rel, &raw))?;
        set_executable(&dest);
        out.written.push(name);
    }

    let wiring = project_root.join(layout.wiring);
    if skip_link(project_root, Path::new(layout.wiring), &mut out) {
        return Ok(out);
    }
    if wiring.exists() && !force {
        out.skipped_existing.push(layout.wiring.to_string());
    } else {
        std::fs::create_dir_all(wiring.parent().unwrap())?;
        std::fs::write(&wiring, format!("{}\n", body.trim_end()))?;
        out.written.push(layout.wiring.to_string());
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
        .is_ok_and(|raw| raw.contains(&format!("/{}/", CODEX_LAYOUT.handlers)))
}

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
fn install_named_entries(
    project_root: &Path,
    force: bool,
    layout: HookLayout,
) -> std::io::Result<CodexHooksOutcome> {
    // trace:STORY-1741 | ai:claude
    crate::scaffolding::refuse_in_adopted_worktree(project_root)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
    let mut out = CodexHooksOutcome::default();
    let template_body = hook_template(layout).map_err(std::io::Error::other)?;
    let wiring = project_root.join(layout.wiring);
    if skip_link(project_root, Path::new(layout.wiring), &mut out) {
        return Ok(out);
    }

    let existing_entries = if wiring.exists() {
        let raw = std::fs::read_to_string(&wiring)?;
        match serde_json::from_str::<OrderedJson>(&raw) {
            Ok(OrderedJson::Object(entries)) => Some(entries),
            _ => {
                let msg = format!(
                    "{} is not a valid top-level JSON object; leaving it untouched and skipping hook installation", layout.wiring
                );
                out.warning = Some(msg);
                return Ok(out);
            }
        }
    } else {
        None
    };

    let dir = project_root.join(layout.handlers);

    for name in layout_handler_names(layout) {
        let Some(body) = crate::templates::EMBEDDED_TEMPLATES.get(&format!("hooks/{name}") as &str)
        else {
            continue;
        };
        let dest = dir.join(&name);
        if skip_link(
            project_root,
            &Path::new(layout.handlers).join(&name),
            &mut out,
        ) {
            continue;
        }
        if dest.exists() && !force {
            out.skipped_existing.push(name);
            continue;
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&dest, format!("{}\n", body.trim_end()))?;
        set_executable(&dest);
        out.written.push(name);
    }

    match existing_entries {
        None => {
            std::fs::create_dir_all(wiring.parent().unwrap())?;
            std::fs::write(&wiring, format!("{}\n", template_body.trim_end()))?;
            out.written.push(layout.wiring.to_string());
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
                out.skipped_existing.push(layout.wiring.to_string());
            } else {
                let rendered = serde_json::to_string_pretty(&OrderedJson::Object(entries))
                    .map_err(std::io::Error::other)?;
                std::fs::create_dir_all(wiring.parent().unwrap())?;
                std::fs::write(&wiring, format!("{rendered}\n"))?;
                out.written.push(layout.wiring.to_string());
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
pub fn scaffold_hooks(
    root: &Path,
    force: bool,
    layout: HookLayout,
) -> std::io::Result<CodexHooksOutcome> {
    match layout.backend {
        HookBackend::Standalone => self::install_standalone(root, force, layout),
        HookBackend::NamedEntries => self::install_named_entries(root, force, layout),
        HookBackend::Settings => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "shared settings hooks require the full scaffolder",
        )),
    }
}
pub fn scaffold_codex_hooks(root: &Path, force: bool) -> std::io::Result<CodexHooksOutcome> {
    self::install_standalone(root, force, CODEX_LAYOUT)
}
pub fn scaffold_antigravity_hooks(root: &Path, force: bool) -> std::io::Result<CodexHooksOutcome> {
    self::install_named_entries(root, force, AGY_LAYOUT)
}

/// Apply the existing safe-writer policy at every ancestor, including dangling
/// links. A blocked file is skipped; unrelated files can still be installed.
// trace:STORY-2539 | ai:codex
fn skip_link(root: &Path, rel: &Path, out: &mut CodexHooksOutcome) -> bool {
    for component in rel.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if let Some((link, target)) =
            super::symlink_blocking_write(root, component, &root.join(component))
        {
            let warning = format!(
                "{} is a symlink to {}; skipping hook file ({})",
                link.display(),
                target.display(),
                super::SYMLINK_SKIP_REMEDY
            );
            eprintln!("warning: {warning}");
            out.warning = Some(match out.warning.take() {
                Some(previous) => format!("{previous}\n{warning}"),
                None => warning,
            });
            out.skipped_existing.push(rel.display().to_string());
            return true;
        }
    }
    false
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
                scaffold_hooks(tmp.path(), false, layout).unwrap();
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
    fn story_2539_hook_writers_skip_live_and_dangling_links_per_file() {
        use std::os::unix::fs::symlink;
        for profile in &PROFILES {
            let layout = profile.hooks;
            if layout.backend == HookBackend::Settings {
                continue;
            }
            let names = layout_handler_names(layout);
            for force in [false, true] {
                for dangling in [false, true] {
                    for rel in [
                        std::path::PathBuf::from(layout.wiring),
                        std::path::PathBuf::from(layout.handlers),
                        Path::new(layout.handlers).join(&names[0]),
                        Path::new(layout.wiring).parent().unwrap().to_path_buf(),
                    ] {
                        let root = tempfile::tempdir().unwrap();
                        let outside = tempfile::tempdir().unwrap();
                        let target = outside.path().join("sentinel");
                        let is_dir = rel == Path::new(layout.handlers)
                            || rel == Path::new(layout.wiring).parent().unwrap();
                        if !dangling {
                            if is_dir {
                                std::fs::create_dir(&target).unwrap();
                            } else {
                                std::fs::write(&target, "keep").unwrap();
                            }
                        }
                        let dest = root.path().join(&rel);
                        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                        symlink(&target, &dest).unwrap();
                        let outcome = scaffold_hooks(root.path(), force, layout).unwrap();
                        assert!(outcome
                            .warning
                            .as_ref()
                            .unwrap()
                            .contains(super::super::SYMLINK_SKIP_REMEDY));
                        assert!(std::fs::symlink_metadata(&dest)
                            .unwrap()
                            .file_type()
                            .is_symlink());
                        if dangling {
                            assert!(!target.exists(), "dangling target was created");
                        } else if is_dir {
                            assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
                        } else {
                            assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
                        }
                        // No handler directory, handler or wiring is created below
                        // a blocked ancestor; per-file skips allow unrelated writes.
                        for candidate in std::iter::once(std::path::PathBuf::from(layout.wiring))
                            .chain(
                                names
                                    .iter()
                                    .map(|name| Path::new(layout.handlers).join(name)),
                            )
                        {
                            if candidate.starts_with(&rel) && candidate != rel {
                                assert!(!root.path().join(candidate).exists());
                            }
                        }
                        if rel == Path::new(layout.wiring).parent().unwrap() {
                            assert!(outcome.written.is_empty());
                            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
                        }
                        if rel == Path::new(layout.handlers).join(&names[0]) {
                            assert!(root.path().join(layout.wiring).is_file());
                            assert!(names.iter().skip(1).all(|name| root
                                .path()
                                .join(layout.handlers)
                                .join(name)
                                .is_file()));
                        }
                        if rel == Path::new(layout.wiring)
                            && layout.backend == HookBackend::NamedEntries
                        {
                            assert!(!root.path().join(layout.handlers).exists());
                            assert!(outcome.written.is_empty());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn story_2539_handler_parser_uses_layout_and_event_contract() {
        let mut layout = CODEX_LAYOUT;
        layout.handlers = "unreferenced/handlers";
        assert!(layout_handler_names(layout).is_empty());
        layout.events = &["unsupported-event"];
        let root = tempfile::tempdir().unwrap();
        assert!(scaffold_hooks(root.path(), false, layout).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wiring must cover exactly the events `settings.json` wires, with the
    /// same matchers and the same handlers. Without this, editing the Claude
    /// settings silently leaves Codex behind — the drift this whole design is
    /// trying to make impossible.
    #[test]
    fn codex_hooks_match_settings() {
        let settings: serde_json::Value = serde_json::from_str(
            crate::templates::EMBEDDED_TEMPLATES
                .get("settings.json")
                .expect("settings.json embedded"),
        )
        .expect("settings.json parses");
        let codex: serde_json::Value = serde_json::from_str(
            crate::templates::EMBEDDED_TEMPLATES
                .get(
                    crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
                        .hooks
                        .template,
                )
                .expect("codex-hooks.json embedded"),
        )
        .expect("codex-hooks.json parses");

        let sh = settings.get("hooks").and_then(|h| h.as_object()).unwrap();
        let ch = codex.get("hooks").and_then(|h| h.as_object()).unwrap();

        // trace:TASK-1744 | ai:antigravity
        assert!(codex.to_string().contains("aida-grant-expiry-notice.sh"));
        assert!(codex.to_string().contains("--client codex"));
        assert!(handler_names()
            .iter()
            .any(|name| name == "aida-grant-expiry-notice.sh"));
        let tmp = tempfile::tempdir().unwrap();
        scaffold_codex_hooks(tmp.path(), true).unwrap();
        assert!(tmp
            .path()
            .join(HOOKS_DIR_REL)
            .join("aida-grant-expiry-notice.sh")
            .is_file());
        scaffold_antigravity_hooks(tmp.path(), true).unwrap();
        assert!(tmp
            .path()
            .join(AGY_HOOKS_DIR_REL)
            .join("aida-grant-expiry-notice.sh")
            .is_file());
        assert!(tmp.path().join(AGY_HOOKS_JSON_REL).is_file());
        assert!(antigravity_hooks_wired(tmp.path()));

        // Every Claude event Codex supports must be present.
        const CODEX_EVENTS: &[&str] = &[
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PostCompact",
            "UserPromptSubmit",
            "SessionStart",
            "Stop",
            "SubagentStart",
            "SubagentStop",
        ];
        for ev in sh.keys() {
            if !CODEX_EVENTS.contains(&ev.as_str()) {
                continue; // no Codex equivalent; legitimately dropped
            }
            assert!(
                ch.contains_key(ev),
                "settings.json wires `{ev}` but the Codex wiring does not — \
                 regenerate templates/codex-hooks.json"
            );
            // trace:STORY-2662 | ai:codex
            // Only the compaction recovery handler is Claude-only. All other
            // entries still participate in exact event/handler parity.
            let s_entries: Vec<_> = sh[ev]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| {
                    !(ev == "SessionStart"
                        && entry["hooks"].as_array().is_some_and(|hooks| {
                            hooks.len() == 1
                                && hooks[0]["command"]
                                    .as_str()
                                    .is_some_and(|cmd| cmd.contains("/aida-precompact-handoff.sh"))
                        }))
                })
                .collect();
            let c_entries = ch[ev].as_array().unwrap();
            assert_eq!(
                s_entries.len(),
                c_entries.len(),
                "`{ev}` has {} Claude entries but {} Codex entries",
                s_entries.len(),
                c_entries.len()
            );
            for (s, c) in s_entries.iter().zip(c_entries) {
                assert_eq!(
                    s.get("matcher"),
                    c.get("matcher"),
                    "`{ev}` matcher differs — Codex normalizes tool names to \
                     Claude's vocabulary, so matchers must be identical"
                );
                let sn: Vec<&str> = s["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| {
                        h["command"]
                            .as_str()
                            .unwrap()
                            .rsplit('/')
                            .next()
                            .unwrap()
                            .split(|c: char| c == '"' || c.is_whitespace())
                            .next()
                            .unwrap()
                    })
                    .collect();
                let cn: Vec<&str> = c["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| {
                        h["command"]
                            .as_str()
                            .unwrap()
                            .rsplit('/')
                            .next()
                            .unwrap()
                            .split(|c: char| c == '"' || c.is_whitespace())
                            .next()
                            .unwrap()
                    })
                    .collect();
                assert_eq!(sn, cn, "`{ev}` wires different handlers");
            }
        }
    }

    #[test]
    fn the_wiring_carries_no_claude_only_variable() {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get(
                crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
                    .hooks
                    .template,
            )
            .unwrap();
        assert!(
            !body.contains("CLAUDE_PROJECT_DIR"),
            "Codex has no CLAUDE_PROJECT_DIR; use $(git rev-parse --show-toplevel)"
        );
        assert!(
            !body.contains("/.claude/hooks/"),
            "a codex-only machine must not need a .claude/ directory"
        );
        assert!(body.contains("git rev-parse --show-toplevel"));
    }

    #[test]
    fn git_hooks_are_not_duplicated_into_the_codex_tree() {
        // commit-msg / pre-commit / post-commit are git-level and vendor
        // independent; installing them here would double-fire them.
        let names = handler_names();
        for git_hook in [
            "aida-commit-msg",
            "aida-pre-commit.sh",
            "aida-post-commit.sh",
        ] {
            assert!(
                !names.iter().any(|n| n == git_hook),
                "{git_hook} is a git hook and must not be wired into Codex"
            );
        }
        assert!(!names.is_empty(), "expected some handlers");
    }

    #[test]
    fn scaffolding_writes_wiring_and_handlers() {
        let dir = std::env::temp_dir().join(format!("aida-codexhooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let out = scaffold_codex_hooks(&dir, false).unwrap();
        assert!(dir.join(HOOKS_JSON_REL).is_file(), "wiring must exist");
        assert!(
            out.written.iter().any(|w| w == HOOKS_JSON_REL),
            "wiring should be reported as written"
        );
        for name in handler_names() {
            assert!(
                dir.join(HOOKS_DIR_REL).join(&name).is_file(),
                "handler {name} must be written — the wiring points at it"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffolding_is_idempotent_and_preserves_edits() {
        let dir = std::env::temp_dir().join(format!("aida-codexhooks-idem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        scaffold_codex_hooks(&dir, false).unwrap();
        let wiring = dir.join(HOOKS_JSON_REL);
        std::fs::write(&wiring, "{\"hooks\":{}}\n").unwrap();

        let out = scaffold_codex_hooks(&dir, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&wiring).unwrap(),
            "{\"hooks\":{}}\n",
            "a user's edited wiring must survive"
        );
        assert!(out.skipped_existing.iter().any(|s| s == HOOKS_JSON_REL));

        let forced = scaffold_codex_hooks(&dir, true).unwrap();
        assert!(forced.written.iter().any(|w| w == HOOKS_JSON_REL));
        std::fs::remove_dir_all(&dir).ok();
    }

    // trace:TASK-1744 | ai:antigravity
    #[test]
    fn antigravity_scaffolding_merges_existing_object_without_destroying_third_party_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let wiring = root.join(AGY_HOOKS_JSON_REL);
        std::fs::create_dir_all(wiring.parent().unwrap()).unwrap();
        std::fs::write(
            &wiring,
            r#"{"z-user-hook":{"PreInvocation":[{"type":"command","command":"echo z"}]},"b-user-hook":{"PreInvocation":[{"type":"command","command":"echo b"}]}}"#,
        )
        .unwrap();
        assert!(!antigravity_hooks_wired(root));

        let out = scaffold_antigravity_hooks(root, false).unwrap();
        assert!(out.warning.is_none());
        assert!(out.written.iter().any(|w| w == AGY_HOOKS_JSON_REL));
        assert!(antigravity_hooks_wired(root));

        let merged_raw = std::fs::read_to_string(&wiring).unwrap();
        let z_pos = merged_raw
            .find("\"z-user-hook\"")
            .expect("z-user-hook present");
        let b_pos = merged_raw
            .find("\"b-user-hook\"")
            .expect("b-user-hook present");
        let aida_pos = merged_raw
            .find(&format!("\"{AGY_EXPIRY_NOTICE_KEY}\""))
            .expect("aida key present");
        assert!(
            z_pos < b_pos && b_pos < aida_pos,
            "non-alphabetical third-party key order must be preserved, got:\n{merged_raw}"
        );

        // Customize AIDA's own key while keeping z-user-hook before b-user-hook;
        // non-force pass must leave it untouched.
        let customized_text = r#"{
  "z-user-hook": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "echo z"
      }
    ]
  },
  "aida-grant-expiry-notice": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "custom-aida"
      }
    ]
  },
  "b-user-hook": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "echo b"
      }
    ]
  },
  "aida-mail-notice": {
    "SessionStart": [
      {
        "type": "command",
        "command": "custom-mail"
      }
    ]
  },
  "aida-mail-notice-turn": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "custom-mail-turn"
      }
    ]
  }
}
"#;
        std::fs::write(&wiring, customized_text).unwrap();

        let second = scaffold_antigravity_hooks(root, false).unwrap();
        assert!(second
            .skipped_existing
            .iter()
            .any(|s| s == AGY_HOOKS_JSON_REL));
        assert_eq!(std::fs::read_to_string(&wiring).unwrap(), customized_text);

        // Forced pass overwrites only AIDA's key in-place while preserving z-user-hook before b-user-hook.
        let forced = scaffold_antigravity_hooks(root, true).unwrap();
        assert!(forced.written.iter().any(|w| w == AGY_HOOKS_JSON_REL));
        let forced_raw = std::fs::read_to_string(&wiring).unwrap();
        let z_after = forced_raw.find("\"z-user-hook\"").unwrap();
        let aida_after = forced_raw
            .find(&format!("\"{AGY_EXPIRY_NOTICE_KEY}\""))
            .unwrap();
        let b_after = forced_raw.find("\"b-user-hook\"").unwrap();
        assert!(
            z_after < aida_after && aida_after < b_after,
            "in-place forced update must preserve key order, got:\n{forced_raw}"
        );
        let after_force: serde_json::Value = serde_json::from_str(&forced_raw).unwrap();
        assert!(after_force[AGY_EXPIRY_NOTICE_KEY]
            .to_string()
            .contains("aida-grant-expiry-notice.sh"));
    }

    /// An install from before the agy SessionStart wiring gains it on a
    /// non-force pass, leaving the customized expiry key alone.
    // trace:TASK-2253.acd91dca | ai:claude
    #[test]
    fn antigravity_scaffolding_adds_missing_mail_notice_key_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let wiring = root.join(AGY_HOOKS_JSON_REL);
        std::fs::create_dir_all(wiring.parent().unwrap()).unwrap();
        std::fs::write(
            &wiring,
            r#"{"aida-grant-expiry-notice":{"PreInvocation":[{"type":"command","command":"custom-aida"}]}}"#,
        )
        .unwrap();

        let out = scaffold_antigravity_hooks(root, false).unwrap();
        assert!(out.written.iter().any(|w| w == AGY_HOOKS_JSON_REL));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&wiring).unwrap()).unwrap();
        assert_eq!(
            v[AGY_EXPIRY_NOTICE_KEY]["PreInvocation"][0]["command"],
            "custom-aida"
        );
        assert!(v["aida-mail-notice"]["SessionStart"][0]["command"]
            .as_str()
            .unwrap()
            .contains("aida-mail-notice.sh"));
        // An existing install also gains the independently managed turn
        // handler while retaining its customized expiry wiring.
        // trace:STORY-1843.ac0eef56 | ai:codex
        assert!(v["aida-mail-notice-turn"]["PreInvocation"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--event PreInvocation"));
        assert_eq!(
            v["aida-mail-notice-turn"]["PreInvocation"][0]["timeout"],
            10
        );
        assert!(root
            .join(AGY_HOOKS_DIR_REL)
            .join("aida-mail-notice.sh")
            .is_file());
    }

    /// Antigravity names `SessionStart` as a flat handler list whose stdin is
    /// camelCase (`conversationId`, no event name) and whose stdout must be a
    /// JSON object. Run the scaffolded agy SessionStart command, as agy does
    /// (`sh -c`, cwd = the dir holding hooks.json), against a recording fake
    /// `aida`: the payload and the named event reach `aida awaiting --notice`
    /// — the session-start path that runs the host-side dead-row gc — and the
    /// notice comes back as an injected system message. Contract source:
    /// Antigravity CLI 1.4.2 builtin `agy-customizations/docs/hooks.md`,
    /// "### 3. `SessionStart` Contract".
    // trace:TASK-2253.acd91dca | ai:claude
    #[cfg(unix)]
    #[test]
    fn agy_session_start_hook_reaches_the_notice_session_start_path() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let st = crate::test_git::git_command()
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(st.success());
        scaffold_antigravity_hooks(root, false).unwrap();

        let wiring: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(AGY_HOOKS_JSON_REL)).unwrap())
                .unwrap();
        let command = wiring["aida-mail-notice"]["SessionStart"][0]["command"]
            .as_str()
            .expect("agy SessionStart handler is flat")
            .to_string();

        let bin = root.join("fake-bin");
        std::fs::create_dir_all(&bin).unwrap();
        let record = root.join("notice-env");
        let fake = bin.join("aida");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\n[ \"$1 $2\" = \"awaiting --notice\" ] || exit 0\n\
                 {{ printf 'PAYLOAD=%s\\n' \"$AIDA_HOOK_PAYLOAD\"; printf 'EVENT=%s\\n' \"$AIDA_HOOK_EVENT\"; }} > '{}'\n\
                 printf 'Current date/time: now \"q\" a\\\\b\\nAwaiting you: 1 mail\\n'\n",
                record.display()
            ),
        )
        .unwrap();
        set_executable(&fake);

        let payload =
            r#"{"conversationId":"agy-conv-1","workspacePaths":["/w"],"modelName":"auto"}"#;
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(root.join(".agents"))
            .env("PATH", path)
            .env_remove("AIDA_SESSION_PROJECT")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("CODEX_PROJECT_DIR")
            .env_remove("AIDA_HOOK_EVENT")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());

        let seen = std::fs::read_to_string(&record).expect("aida awaiting --notice ran");
        assert!(seen.contains(&format!("PAYLOAD={payload}")), "{seen}");
        assert!(seen.contains("EVENT=SessionStart"), "{seen}");

        let out: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("agy stdout must be JSON ({e}): {:?}", output.stdout));
        assert_eq!(
            out["injectSteps"][0]["systemMessage"]["systemMessage"],
            "Current date/time: now \"q\" a\\b\nAwaiting you: 1 mail"
        );
    }

    // trace:TASK-1744 | ai:antigravity
    #[test]
    fn antigravity_scaffolding_leaves_invalid_or_non_object_hooks_json_untouched_even_with_force() {
        for raw in ["not-json{{{", "[1, 2, 3]\n", "\"string\"\n"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let wiring = root.join(AGY_HOOKS_JSON_REL);
            std::fs::create_dir_all(wiring.parent().unwrap()).unwrap();
            std::fs::write(&wiring, raw).unwrap();

            for force in [false, true] {
                let out = scaffold_antigravity_hooks(root, force).unwrap();
                assert!(
                    out.warning.is_some(),
                    "expected warning for {raw:?} (force={force})"
                );
                assert!(out.written.is_empty());
                assert_eq!(
                    std::fs::read_to_string(&wiring).unwrap(),
                    raw,
                    "invalid/non-object hooks.json must never be overwritten (force={force})"
                );
                assert!(
                    !root
                        .join(AGY_HOOKS_DIR_REL)
                        .join("aida-grant-expiry-notice.sh")
                        .exists(),
                    "handler script must not be dropped when hooks.json is invalid (force={force})"
                );
            }
        }
    }
}

/// BUG-2395: the shared hook scripts must work unchanged under Codex. These
/// run the embedded masters against fixture repos and a fake HOME only.
// trace:BUG-2395 | ai:claude
#[cfg(all(test, unix))]
mod bug_2395_vendor_neutral_hooks {
    use std::path::Path;
    use std::process::{Command, Stdio};

    fn write_hook(dir: &Path, name: &str) -> std::path::PathBuf {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get(format!("hooks/{name}").as_str())
            .copied()
            .expect("embedded hook");
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn git(repo: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// Runs `aida-commit-msg` with `staged` (path, body) files staged and
    /// returns its combined output.
    fn commit_msg_output(staged: &[(&str, &str)]) -> String {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (rel, body) in staged {
            let p = repo.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            git(&repo, &["add", rel]);
        }
        let hook = write_hook(tmp.path(), "aida-commit-msg");
        let msg = tmp.path().join("MSG");
        std::fs::write(&msg, "chore(scaffold): refresh hooks\n").unwrap();
        let out = Command::new("bash")
            .arg(&hook)
            .arg(&msg)
            .current_dir(&repo)
            .env("HOME", tmp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("AIDA_RELEASE")
            .env_remove("AIDA_COMMIT_STRICT")
            .output()
            .unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    #[test]
    fn commit_msg_ignores_foreign_traces_in_codex_and_agents_mirrors() {
        let traced = "#!/bin/bash\n# trace:TASK-9999 | ai:claude\n";
        let mirrors = commit_msg_output(&[
            (".codex/hooks/aida-x.sh", traced),
            (".agents/hooks/aida-y.sh", traced),
        ]);
        assert!(
            !mirrors.contains("TASK-9999") && !mirrors.contains("AI-traced"),
            "scaffold mirrors must not trigger trace warnings: {mirrors}"
        );
        let source = commit_msg_output(&[("src/x.sh", traced)]);
        assert!(
            source.contains("AI-traced"),
            "control: a source file still warns: {source}"
        );
    }

    /// Runs `aida-capture-prompt.sh` with `payload` on stdin and returns stdout.
    fn capture_prompt(home: &Path, payload: &str, envs: &[(&str, &str)]) -> String {
        let hook = write_hook(home, "aida-capture-prompt.sh");
        let mut cmd = Command::new("bash");
        cmd.arg(&hook)
            .current_dir(home)
            .env("HOME", home)
            .env_remove("CODEX_PROJECT_DIR")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("AIDA_SESSION_PROJECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
    }

    #[test]
    fn capture_prompt_counts_codex_skill_and_speaks_both_vendors() {
        if Command::new("jq").arg("--version").output().is_err() {
            eprintln!("skipping: jq not installed");
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();
        let transcript = home.join("t.jsonl");
        let payload = format!(
            r#"{{"session_id":"s1","transcript_path":"{}"}}"#,
            transcript.display()
        );

        std::fs::write(&transcript, "{\"text\":\"this is worth noting\"}\n").unwrap();
        let fired = capture_prompt(home, &payload, &[]);
        assert!(fired.contains("/aida-capture"), "{fired}");
        assert!(
            fired.contains("$aida-capture"),
            "Codex wording missing: {fired}"
        );

        std::fs::write(
            &transcript,
            "{\"text\":\"this is worth noting\"}\n{\"text\":\"ran $aida-capture\"}\n",
        )
        .unwrap();
        assert!(
            capture_prompt(home, &payload, &[]).is_empty(),
            "$aida-capture counts as filing activity"
        );

        // A Codex payload without transcript_path never reads ~/.claude.
        let project = home.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let encoded = project.display().to_string().replace('/', "-");
        let claude_dir = home.join(".claude/projects").join(encoded);
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("s2.jsonl"), "{\"text\":\"worth noting\"}\n").unwrap();
        let project_str = project.display().to_string();
        let no_path = r#"{"session_id":"s2"}"#;
        assert!(
            !capture_prompt(home, no_path, &[("CLAUDE_PROJECT_DIR", &project_str)]).is_empty(),
            "control: Claude falls back to its transcript layout"
        );
        assert!(
            capture_prompt(home, no_path, &[("CODEX_PROJECT_DIR", &project_str)]).is_empty(),
            "Codex must not fall back to the Claude transcript layout"
        );
    }
}
