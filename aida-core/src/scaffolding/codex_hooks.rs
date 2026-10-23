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
pub const HOOKS_JSON_REL: &str = ".codex/hooks.json";
/// Where the handler scripts live, relative to the project root.
pub const HOOKS_DIR_REL: &str = ".codex/hooks";

/// The handlers referenced by the Codex wiring.
///
/// Deliberately a subset of `templates/hooks/`: the git hooks
/// (`aida-commit-msg`, `aida-pre-commit.sh`, `aida-post-commit.sh`) are
/// git-level and vendor-independent — they are installed into `.git/hooks`
/// and must NOT be duplicated here.
pub fn handler_names() -> Vec<String> {
    let text = crate::templates::EMBEDDED_TEMPLATES
        .get("codex-hooks.json")
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
pub fn scaffold_codex_hooks(
    project_root: &Path,
    force: bool,
) -> std::io::Result<CodexHooksOutcome> {
    let mut out = CodexHooksOutcome::default();
    let dir = project_root.join(HOOKS_DIR_REL);
    std::fs::create_dir_all(&dir)?;

    // Handlers first, so the wiring never points at a script that is not there.
    for name in handler_names() {
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

    let wiring = project_root.join(HOOKS_JSON_REL);
    if wiring.exists() && !force {
        out.skipped_existing.push(HOOKS_JSON_REL.to_string());
    } else {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get("codex-hooks.json")
            .copied()
            .unwrap_or("{}");
        std::fs::write(&wiring, format!("{}\n", body.trim_end()))?;
        out.written.push(HOOKS_JSON_REL.to_string());
    }
    Ok(out)
}

/// Where the Antigravity wiring lives, relative to the project root.
// trace:TASK-1744 | ai:antigravity
pub const AGY_HOOKS_JSON_REL: &str = ".agents/hooks.json";
/// Where the Antigravity handler scripts live, relative to the project root.
pub const AGY_HOOKS_DIR_REL: &str = ".agents/hooks";
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
        .get("antigravity-hooks.json")
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
pub fn scaffold_antigravity_hooks(
    project_root: &Path,
    force: bool,
) -> std::io::Result<CodexHooksOutcome> {
    let mut out = CodexHooksOutcome::default();
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
        .get("antigravity-hooks.json")
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
fn set_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(md) = std::fs::metadata(p) {
        let mut perms = md.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(p, perms);
    }
}

#[cfg(not(unix))]
fn set_executable(_p: &Path) {}

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
                .get("codex-hooks.json")
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
            "PreCompact",
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
            let s_entries = sh[ev].as_array().unwrap();
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
            .get("codex-hooks.json")
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
    // trace:TASK-2253 | ai:claude
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
    /// notice comes back as an injected system message.
    // trace:TASK-2253 | ai:claude
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
                 printf 'Current date/time: now \"q\"\\nAwaiting you: 1 mail\\n'\n",
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
            "Current date/time: now \"q\"\nAwaiting you: 1 mail"
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
