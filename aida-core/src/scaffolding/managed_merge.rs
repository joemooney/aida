//! Managed-merge slot-merge for `aida scaffold upgrade` (FR-1-047, the
//! v2 of FR-1-028, design from SPIKE-1-029 §Q3).
//!
//! For files in `FileCategory::ManagedMerge` (today: `.claude/settings.json`
//! and `.mcp.json`), AIDA owns specific JSON Pointer slots and the user
//! owns everything else. `scaffold upgrade` walks the slot list, replaces
//! drifted slots with AIDA's expected values, and writes everything else
//! back unchanged. So changes to AIDA-owned bits (e.g. statusLine command
//! bumped to `--color=always`, new SessionStart hook entry) propagate to
//! existing projects without `--force` clobbering the user's other keys.
//!
//! Slot expressions are RFC 6901 JSON Pointers (`/hooks/PreToolUse` etc.) —
//! simpler than JSONPath and supported natively by `serde_json` via
//! `Value::pointer` / `Value::pointer_mut`. The spike accepted either; we
//! pick Pointer because every slot we use is a fixed path, not a query.
//!
//! trace:FR-1-047 | ai:claude

use serde_json::Value;
use std::path::Path;

/// Result of comparing one slot between the user's on-disk file and the
/// expected AIDA-rendered template.
#[derive(Debug, Clone)]
pub struct SlotChange {
    pub slot: String,
    pub kind: SlotChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotChangeKind {
    /// User's slot value differs from AIDA's; the merge replaces it.
    Replaced,
    /// User has no value at the slot; AIDA's value is added.
    Added,
}

/// Hard-coded slot lists per managed-merge file. Keyed by basename so it
/// works regardless of project root. Returns an empty slice for files
/// that aren't managed-merge — caller is expected to dispatch on
/// `FileCategory` first, but this is a safe no-op fallback.
///
/// Long-term home is the `templates/manifest.toml` proposed in
/// SPIKE-1-029 §Q2; v2 keeps slots in code to ship without the manifest
/// machinery.
/// trace:FR-1-047 | ai:claude
pub fn slots_for_file(path: &Path) -> &'static [&'static str] {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    match name {
        "settings.json" => &[
            "/hooks/PreToolUse",
            "/hooks/PostToolUse",
            "/hooks/SessionStart",
            "/statusLine",
        ],
        "mcp.json" | ".mcp.json" => &["/mcpServers/aida"],
        // settings.local.json — per-user, gitignored Claude Code overrides.
        // AIDA owns the MCP pre-approval slot so a fresh init trusts its own
        // scaffolded .mcp.json server; the user owns the rest of the file.
        // trace:BUG-484
        "settings.local.json" => &["/enabledMcpjsonServers"],
        _ => &[],
    }
}

/// Compute what would change if we slot-merged `user_doc` against
/// `aida_doc` over `slots`. Pure: doesn't write anywhere. Returns the
/// merged document plus a list of changes (or `Vec::new()` when the
/// docs are already aligned at every declared slot).
///
/// trace:FR-1-047 | ai:claude
pub fn slot_merge(user_doc: &Value, aida_doc: &Value, slots: &[&str]) -> (Value, Vec<SlotChange>) {
    let mut result = user_doc.clone();
    let mut changes = Vec::new();

    for slot in slots {
        let aida_val = aida_doc.pointer(slot);
        let user_val = result.pointer(slot);

        match (user_val, aida_val) {
            (None, None) => continue,                 // neither has it
            (Some(u), Some(a)) if u == a => continue, // matching
            (Some(_), Some(a)) => {
                // Drift — replace.
                if let Some(target) = result.pointer_mut(slot) {
                    *target = a.clone();
                }
                changes.push(SlotChange {
                    slot: (*slot).to_string(),
                    kind: SlotChangeKind::Replaced,
                });
            }
            (None, Some(a)) => {
                // User is missing the slot — add, creating intermediate
                // objects as needed.
                set_or_create(&mut result, slot, a.clone());
                changes.push(SlotChange {
                    slot: (*slot).to_string(),
                    kind: SlotChangeKind::Added,
                });
            }
            (Some(_), None) => {
                // AIDA dropped this slot from its template; leave the
                // user's value alone (they may have customized it).
                continue;
            }
        }
    }

    (result, changes)
}

/// Set a JSON Pointer path, creating intermediate objects when needed.
/// `serde_json::Value::pointer_mut` only works on existing paths — this
/// fills in `{}` along the way for missing intermediate keys. Bails
/// silently on malformed pointers (e.g. trying to descend into a non-
/// object), since slot-merge would have rejected them upstream.
fn set_or_create(doc: &mut Value, pointer: &str, value: Value) {
    if pointer.is_empty() {
        *doc = value;
        return;
    }
    if !pointer.starts_with('/') {
        return;
    }
    let parts: Vec<&str> = pointer.split('/').skip(1).collect();
    if parts.is_empty() {
        return;
    }

    // Ensure root is an object so we can drill in. (Could be an array
    // root too in theory; AIDA's managed-merge files are all objects.)
    if !doc.is_object() {
        return;
    }

    let mut cursor: &mut Value = doc;
    for (i, raw_key) in parts.iter().enumerate() {
        // Unescape per RFC 6901: ~1 → /, ~0 → ~ (in this order).
        let key = raw_key.replace("~1", "/").replace("~0", "~");
        let is_leaf = i == parts.len() - 1;

        let map = match cursor.as_object_mut() {
            Some(m) => m,
            None => return, // path passes through a non-object; give up
        };

        if is_leaf {
            map.insert(key, value);
            return;
        }
        // Walk one level deeper. Insert an empty object if the key is
        // missing or a non-object.
        if !map.get(&key).map(|v| v.is_object()).unwrap_or(false) {
            map.insert(key.clone(), Value::Object(serde_json::Map::new()));
        }
        cursor = map.get_mut(&key).expect("just inserted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slots_for_settings_json() {
        let slots = slots_for_file(Path::new(".claude/settings.json"));
        assert!(slots.contains(&"/statusLine"));
        assert!(slots.contains(&"/hooks/PreToolUse"));
    }

    #[test]
    fn slots_for_mcp_json() {
        assert_eq!(
            slots_for_file(Path::new(".mcp.json")),
            &["/mcpServers/aida"]
        );
        assert_eq!(slots_for_file(Path::new("mcp.json")), &["/mcpServers/aida"]);
    }

    #[test]
    fn slots_for_unknown_returns_empty() {
        assert!(slots_for_file(Path::new("CLAUDE.md")).is_empty());
    }

    // trace:BUG-484 — settings.local.json's MCP-trust slot is AIDA-managed so
    // re-init slot-merges the pre-approval without clobbering user-owned keys.
    #[test]
    fn slots_for_settings_local_json() {
        assert_eq!(
            slots_for_file(Path::new(".claude/settings.local.json")),
            &["/enabledMcpjsonServers"]
        );
    }

    /// Drifted slot replaced; matching slot untouched; user keys
    /// outside the declared slots preserved verbatim. trace:FR-1-047
    #[test]
    fn slot_merge_replaces_drift_preserves_user_keys() {
        let user = json!({
            "permissions": {"deny": ["rm -rf /"]},
            "hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command"}]}],
                "PostToolUse": [{"matcher": "Bash", "hooks": []}],
                "Stop": [{"matcher": "*", "hooks": []}]
            },
            "statusLine": {"type": "command", "command": "OLD"}
        });
        let aida = json!({
            "hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command"}]}],
                "PostToolUse": [{"matcher": "Bash", "hooks": []}]
            },
            "statusLine": {"type": "command", "command": "NEW"}
        });
        let slots = ["/hooks/PreToolUse", "/hooks/PostToolUse", "/statusLine"];
        let (merged, changes) = slot_merge(&user, &aida, &slots);

        // statusLine drifted → replaced.
        assert_eq!(merged["statusLine"]["command"], "NEW");
        // PreToolUse and PostToolUse matched → no change.
        assert_eq!(merged["hooks"]["PreToolUse"], user["hooks"]["PreToolUse"]);
        // User-owned keys preserved.
        assert_eq!(merged["permissions"]["deny"], json!(["rm -rf /"]));
        assert_eq!(
            merged["hooks"]["Stop"],
            json!([{"matcher": "*", "hooks": []}])
        );
        // One change reported (statusLine).
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].slot, "/statusLine");
        assert_eq!(changes[0].kind, SlotChangeKind::Replaced);
    }

    #[test]
    fn slot_merge_adds_missing_slot() {
        let user = json!({
            "permissions": {"deny": []},
            "hooks": {"PreToolUse": []}
        });
        let aida = json!({
            "hooks": {
                "PreToolUse": [],
                "SessionStart": [{"hooks": [{"type": "command"}]}]
            }
        });
        let slots = ["/hooks/PreToolUse", "/hooks/SessionStart"];
        let (merged, changes) = slot_merge(&user, &aida, &slots);

        assert_eq!(
            merged["hooks"]["SessionStart"],
            json!([{"hooks": [{"type": "command"}]}])
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, SlotChangeKind::Added);
    }

    #[test]
    fn slot_merge_no_changes_when_aligned() {
        let doc = json!({"hooks": {"PreToolUse": []}, "statusLine": {"command": "X"}});
        let slots = ["/hooks/PreToolUse", "/statusLine"];
        let (merged, changes) = slot_merge(&doc, &doc, &slots);
        assert!(changes.is_empty());
        assert_eq!(merged, doc);
    }

    #[test]
    fn slot_merge_aida_missing_slot_preserves_user() {
        // AIDA dropped a slot from its template (e.g. removed a hook).
        // User's value at the slot is left alone.
        let user = json!({"hooks": {"PreToolUse": [{"matcher": "Bash"}]}});
        let aida = json!({"hooks": {}});
        let slots = ["/hooks/PreToolUse"];
        let (merged, changes) = slot_merge(&user, &aida, &slots);
        assert!(changes.is_empty());
        assert_eq!(merged["hooks"]["PreToolUse"], json!([{"matcher": "Bash"}]));
    }

    #[test]
    fn set_or_create_creates_intermediate_objects() {
        let mut doc = json!({});
        set_or_create(&mut doc, "/a/b/c", json!(42));
        assert_eq!(doc, json!({"a": {"b": {"c": 42}}}));
    }

    #[test]
    fn set_or_create_replaces_non_object_intermediate() {
        // Edge case: walking through `a.b` where `b` is a string → can't
        // descend, so we replace with an empty object. (Matches the
        // "AIDA owns the slot" semantics.)
        let mut doc = json!({"a": {"b": "stringy"}});
        set_or_create(&mut doc, "/a/b/c", json!(1));
        assert_eq!(doc, json!({"a": {"b": {"c": 1}}}));
    }

    #[test]
    fn set_or_create_unescapes_pointer_segments() {
        let mut doc = json!({});
        // RFC 6901: `~1` is `/`, `~0` is `~`. So `/a~1b` means key "a/b".
        set_or_create(&mut doc, "/a~1b", json!("v"));
        assert_eq!(doc, json!({"a/b": "v"}));
    }
}
