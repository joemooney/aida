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
    // `.codex/hooks.json` shares its basename with `.agents/hooks.json`
    // (Antigravity, merged by its own named-key writer), so it is matched on
    // the path before the basename dispatch. trace:BUG-2395 | ai:claude
    if is_codex_hooks_json(path) {
        return CODEX_HOOK_SLOTS;
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    match name {
        "settings.json" => &[
            "/hooks/PreToolUse",
            "/hooks/PostToolUse",
            // trace:STORY-2662 | ai:codex
            "/hooks/PreCompact",
            "/hooks/SessionStart",
            "/statusLine",
            // AIDA owns the MCP pre-approval slot in the committed settings.json
            // so every checked-out worktree (a fresh project path) trusts the
            // scaffolded .mcp.json `aida` server and stops warning "1 setup
            // issue: MCP". The gitignored settings.local.json (BUG-484) doesn't
            // propagate into worktrees; the committed slot does. trace:BUG-501
            "/enabledMcpjsonServers",
            // BUG-014: the reviewer allow entries. Additive (see
            // `ADDITIVE_SLOTS`): AIDA adds its entries, the user's stay.
            // trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
            "/permissions/allow",
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

/// The Codex hook events AIDA wires in `.codex/hooks.json`.
// trace:BUG-2395 | ai:claude
const CODEX_HOOK_SLOTS: &[&str] = &[
    "/hooks/PreToolUse",
    "/hooks/PostToolUse",
    "/hooks/SessionStart",
    "/hooks/UserPromptSubmit",
    "/hooks/SubagentStart",
    "/hooks/SubagentStop",
    "/hooks/Stop",
];

/// A hook entry whose command contains this marker is AIDA's in
/// `.codex/hooks.json`. The grant-expiry notice runs through a `$top/...`
/// indirection, which still contains the marker.
// trace:BUG-2395 | ai:claude
const CODEX_AIDA_HOOK_MARKER: &str = "/.codex/hooks/aida-";

fn is_codex_hooks_json(path: &Path) -> bool {
    path.ends_with(".codex/hooks.json")
}

/// For files where AIDA owns individual hook entries inside each
/// `/hooks/<Event>` slot rather than the whole slot, returns the command
/// marker that identifies an AIDA entry. Today that is only
/// `.codex/hooks.json`: Codex users hand-add hooks next to AIDA's, and a
/// whole-slot merge would delete them on every upgrade.
// trace:BUG-2395 | ai:claude
fn entry_owner_marker(path: &Path) -> Option<&'static str> {
    is_codex_hooks_json(path).then_some(CODEX_AIDA_HOOK_MARKER)
}

fn is_aida_hook(hook: &Value, marker: &str) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(marker))
}

/// The AIDA-owned part of a `/hooks/<Event>` array: each matcher group
/// keeps only its AIDA hooks, and groups left with none are dropped.
fn aida_entries(slot_value: Option<&Value>, marker: &str) -> Vec<Value> {
    let Some(Value::Array(groups)) = slot_value else {
        return Vec::new();
    };
    groups
        .iter()
        .filter_map(|group| {
            let hooks = group.get("hooks")?.as_array()?;
            let ours: Vec<Value> = hooks
                .iter()
                .filter(|h| is_aida_hook(h, marker))
                .cloned()
                .collect();
            if ours.is_empty() {
                return None;
            }
            let mut g = group.clone();
            g["hooks"] = Value::Array(ours);
            Some(g)
        })
        .collect()
}

/// The user-owned part of a `/hooks/<Event>` array: AIDA hooks removed, and
/// a group dropped only when removing them left it empty. Groups that were
/// not AIDA's are kept verbatim, in order.
fn user_entries(slot_value: Option<&Value>, marker: &str) -> Vec<Value> {
    let Some(Value::Array(groups)) = slot_value else {
        return Vec::new();
    };
    groups
        .iter()
        .filter_map(|group| {
            let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
                return Some(group.clone());
            };
            if !hooks.iter().any(|h| is_aida_hook(h, marker)) {
                return Some(group.clone());
            }
            let theirs: Vec<Value> = hooks
                .iter()
                .filter(|h| !is_aida_hook(h, marker))
                .cloned()
                .collect();
            if theirs.is_empty() {
                return None;
            }
            let mut g = group.clone();
            g["hooks"] = Value::Array(theirs);
            Some(g)
        })
        .collect()
}

/// Whether the user's value at `slot` of the managed-merge file `path`
/// satisfies AIDA's. Entry-owned files compare only AIDA's entries; every
/// other file defers to [`slot_value_matches`]. Every drift, diff, status,
/// and merge check routes through this so they agree.
// trace:BUG-2395 | ai:claude
pub fn managed_slot_matches(
    path: &Path,
    slot: &str,
    user: Option<&Value>,
    aida: Option<&Value>,
) -> bool {
    match entry_owner_marker(path) {
        Some(marker) => aida.is_none() || aida_entries(user, marker) == aida_entries(aida, marker),
        None => slot_value_matches(slot, user, aida),
    }
}

/// Slot-merge `user_doc` against `aida_doc` over every AIDA-owned slot of
/// the managed-merge file `path`. For an entry-owned file each drifted slot
/// becomes the user's own entries followed by AIDA's current entries, so a
/// hand-added hook survives and a stale AIDA entry is replaced. Every other
/// file uses [`slot_merge`] unchanged.
// trace:BUG-2395 | ai:claude
pub fn managed_slot_merge(
    path: &Path,
    user_doc: &Value,
    aida_doc: &Value,
) -> (Value, Vec<SlotChange>) {
    let slots = slots_for_file(path);
    let Some(marker) = entry_owner_marker(path) else {
        return slot_merge(user_doc, aida_doc, slots);
    };
    let mut result = user_doc.clone();
    let mut changes = Vec::new();
    for slot in slots {
        let Some(aida_val) = aida_doc.pointer(slot) else {
            continue;
        };
        let user_val = result.pointer(slot);
        if aida_entries(user_val, marker) == aida_entries(Some(aida_val), marker) {
            continue;
        }
        let kind = if user_val.is_some() {
            SlotChangeKind::Replaced
        } else {
            SlotChangeKind::Added
        };
        let mut merged = user_entries(user_val, marker);
        merged.extend(aida_entries(Some(aida_val), marker));
        set_or_create(&mut result, slot, Value::Array(merged));
        changes.push(SlotChange {
            slot: (*slot).to_string(),
            kind,
        });
    }
    (result, changes)
}

/// Slots whose value is a string array AIDA shares with the user: AIDA owns
/// its entries, not the whole array. A slot matches when every AIDA entry is
/// present, and a merge appends the missing ones, keeping the user's extra
/// entries (a `permissions.allow` list is the user's to widen).
// trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
const ADDITIVE_SLOTS: &[&str] = &["/permissions/allow"];

/// Whether the user's value at `slot` satisfies AIDA's. Exact equality,
/// except for an [`ADDITIVE_SLOTS`] array, where every AIDA entry must be
/// present. Every slot-equality check (drift, diff, status, merge) uses this.
// trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
pub fn slot_value_matches(slot: &str, user: Option<&Value>, aida: Option<&Value>) -> bool {
    if ADDITIVE_SLOTS.contains(&slot) {
        if let (Some(Value::Array(u)), Some(Value::Array(a))) = (user, aida) {
            return a.iter().all(|entry| u.contains(entry));
        }
    }
    user == aida
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
            (None, None) => continue, // neither has it
            (Some(u), Some(a)) if slot_value_matches(slot, Some(u), Some(a)) => continue,
            (Some(_), Some(a)) => {
                // Drift — replace, or for an additive array append AIDA's
                // missing entries. trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
                if let Some(target) = result.pointer_mut(slot) {
                    match (ADDITIVE_SLOTS.contains(slot), target, a) {
                        (true, Value::Array(user_entries), Value::Array(aida_entries)) => {
                            for entry in aida_entries {
                                if !user_entries.contains(entry) {
                                    user_entries.push(entry.clone());
                                }
                            }
                        }
                        (_, target, a) => *target = a.clone(),
                    }
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
        // trace:BUG-501 — committed MCP pre-approval slot so every worktree
        // (a fresh project path) trusts the scaffolded aida server.
        assert!(slots.contains(&"/enabledMcpjsonServers"));
    }

    /// BUG-014: AIDA's allow entries are added to a user's list, never
    /// replace it, and a list holding them all is not drift.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
    #[test]
    fn bug_014_permissions_allow_merges_additively() {
        let slots = slots_for_file(Path::new(".claude/settings.json"));
        let aida = json!({"permissions": {"allow": ["Bash(aida)", "Bash(aida *)"]}});
        let user = json!({"permissions": {"allow": ["Bash(make *)", "Bash(aida)"]}});
        let (merged, changes) = slot_merge(&user, &aida, slots);
        assert_eq!(
            merged["permissions"]["allow"],
            json!(["Bash(make *)", "Bash(aida)", "Bash(aida *)"])
        );
        assert_eq!(changes.len(), 1);
        let (_, again) = slot_merge(&merged, &aida, slots);
        assert!(again.is_empty(), "a superset is not drift: {again:?}");

        let bare = json!({"hooks": {}});
        let (merged, _) = slot_merge(&bare, &aida, slots);
        assert_eq!(merged["permissions"]["allow"], aida["permissions"]["allow"]);
    }

    fn codex_hook(name: &str) -> Value {
        json!({"type": "command", "command": format!("$(git rev-parse --show-toplevel)/.codex/hooks/{name}")})
    }

    /// BUG-2395: `.codex/hooks.json` is entry-owned. A hand-added hook in the
    /// same event (in its own group or sharing AIDA's group) survives an
    /// upgrade, a stale AIDA entry is replaced, and the result is not drift.
    // trace:BUG-2395 | ai:claude
    #[test]
    fn bug_2395_codex_hooks_json_merges_per_entry() {
        let path = Path::new(".codex/hooks.json");
        assert!(slots_for_file(path).contains(&"/hooks/Stop"));
        let mine = json!({"type": "command", "command": "./scripts/my-hook.sh"});
        let user = json!({
            "hooks": {
                "SessionStart": [
                    {"hooks": [codex_hook("aida-session-context.sh"), mine.clone()]},
                    {"matcher": "startup", "hooks": [mine.clone()]}
                ],
                "Stop": [{"hooks": [codex_hook("aida-old-stop.sh")]}]
            },
            "userKey": true
        });
        let aida = json!({
            "hooks": {
                "SessionStart": [{"hooks": [codex_hook("aida-session-context.sh"), codex_hook("aida-role-context.sh")]}],
                "Stop": [{"hooks": [codex_hook("aida-capture-prompt.sh")]}]
            }
        });

        for slot in ["/hooks/SessionStart", "/hooks/Stop"] {
            assert!(!managed_slot_matches(
                path,
                slot,
                user.pointer(slot),
                aida.pointer(slot)
            ));
        }
        let (merged, changes) = managed_slot_merge(path, &user, &aida);
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(
            merged["hooks"]["SessionStart"],
            json!([
                {"hooks": [mine.clone()]},
                {"matcher": "startup", "hooks": [mine]},
                {"hooks": [codex_hook("aida-session-context.sh"), codex_hook("aida-role-context.sh")]}
            ])
        );
        assert_eq!(
            merged["hooks"]["Stop"], aida["hooks"]["Stop"],
            "stale entry replaced"
        );
        assert_eq!(merged["userKey"], json!(true));

        let (_, again) = managed_slot_merge(path, &merged, &aida);
        assert!(again.is_empty(), "a merged doc is not drift: {again:?}");
        for slot in slots_for_file(path) {
            assert!(managed_slot_matches(
                path,
                slot,
                merged.pointer(slot),
                aida.pointer(slot)
            ));
        }
    }

    /// Whole-slot ownership still applies to `.claude/settings.json`, and
    /// `.agents/hooks.json` is not mistaken for the Codex file.
    // trace:BUG-2395 | ai:claude
    #[test]
    fn bug_2395_entry_ownership_is_codex_only() {
        let settings = Path::new(".claude/settings.json");
        let user = json!({"hooks": {"PreToolUse": [{"hooks": [{"command": "mine"}]}]}});
        let aida = json!({"hooks": {"PreToolUse": [{"hooks": [{"command": "aida"}]}]}});
        let (merged, _) = managed_slot_merge(settings, &user, &aida);
        assert_eq!(merged["hooks"]["PreToolUse"], aida["hooks"]["PreToolUse"]);
        assert!(slots_for_file(Path::new(".agents/hooks.json")).is_empty());
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
