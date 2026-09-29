//! Invocation-wide CLI cache labels. Arrays retain their existing wire shape.
// trace:TASK-1526 | ai:codex
use aida_core::db::cache_refresh::{cache_read_metadata, CacheReadScope};

thread_local! {
    static TOON_CACHE_OUTPUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Strict history/graph reads still expose the always-present fresh object.
// trace:TASK-1526 | ai:codex
pub(crate) fn set_toon_cache_output(enabled: bool) {
    TOON_CACHE_OUTPUT.with(|c| c.set(enabled));
}

// trace:TASK-1526 | ai:codex
pub(crate) fn json_pretty<T: serde::Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(value)?;
    if let Some(object) = value.as_object_mut() {
        let metadata = cache_read_metadata();
        if let Some(existing) = object
            .get_mut("cache")
            .and_then(serde_json::Value::as_object_mut)
        {
            // Preserve legacy cache fields (heavy status has rows/fresh).
            // Its positive spelling must agree with the invocation label.
            if existing.contains_key("fresh") {
                existing.insert(
                    "fresh".into(),
                    serde_json::json!(!metadata["stale"].as_bool().unwrap()),
                );
            }
            existing.extend(metadata.as_object().unwrap().clone());
        } else {
            object.insert("cache".into(), metadata);
        }
    }
    serde_json::to_string_pretty(&value)
}

// The dispatcher emits exactly one note, after all backends contributed.
// trace:TASK-1526 | ai:codex
pub(crate) fn finish(scope: &CacheReadScope, success: bool, argv: &[String]) {
    if !success
        || (!scope.touched() && !TOON_CACHE_OUTPUT.with(|c| c.get()))
        || argv.iter().any(|arg| arg == "mcp-serve")
    {
        return;
    }
    if let Some(stale) = scope.stale() {
        eprintln!("{}", stale.note());
    }
    let json = crate::output_format_is_json()
        || argv
            .iter()
            .any(|s| s == "--json" || s == "--digest-format=json")
        || argv
            .windows(2)
            .any(|args| args[0] == "--digest-format" && args[1] == "json");
    let advisory = argv
        .iter()
        .any(|arg| matches!(arg.as_str(), "--notice" | "statusline" | "statusbar"));
    if !json && !advisory && crate::agent_output_mode() {
        println!("{}", toon_cache(&scope.metadata()));
    }
}

// trace:TASK-1526 | ai:codex
fn toon_cache(metadata: &serde_json::Value) -> String {
    let fields = metadata
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| format!("  {key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("cache:\n{fields}")
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:TASK-1526 | ai:codex
    #[test]
    fn stale_object_and_array_shapes_preserve_freshness() {
        use aida_core::db::cache_refresh::*;
        for state in [
            RefreshState::Deferred,
            RefreshState::WriterBusy,
            RefreshState::WorkerRunning,
        ] {
            let scope = CacheReadScope::new();
            record_read(Some(StaleServe {
                stale: true,
                cache_head: None,
                store_head: "new".into(),
                built_at: None,
                refreshing: state,
            }));
            record_read(None);
            let object: serde_json::Value =
                serde_json::from_str(&json_pretty(&serde_json::json!({"rows": []})).unwrap())
                    .unwrap();
            assert_eq!(object["cache"], scope.metadata());
            let legacy: serde_json::Value = serde_json::from_str(
                &json_pretty(&serde_json::json!({"cache": {"fresh": true, "rows": 7}})).unwrap(),
            )
            .unwrap();
            assert_eq!(legacy["cache"]["rows"], 7);
            assert_eq!(legacy["cache"]["fresh"], false);
            assert_eq!(legacy["cache"]["stale"], true);
            let array: serde_json::Value =
                serde_json::from_str(&json_pretty(&serde_json::json!([])).unwrap()).unwrap();
            assert!(array.is_array());
        }
        let _scope = CacheReadScope::new();
        record_read(None);
        let object: serde_json::Value =
            serde_json::from_str(&json_pretty(&serde_json::json!({})).unwrap()).unwrap();
        assert_eq!(object["cache"]["stale"], false);
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn untouched_output_keeps_its_shape() {
        let _scope = CacheReadScope::new();
        assert_eq!(json_pretty(&serde_json::json!([1])).unwrap(), "[\n  1\n]");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &json_pretty(&serde_json::json!({})).unwrap()
            )
            .unwrap()["cache"]["stale"],
            false
        );
    }
}
