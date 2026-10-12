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

// Whether the last body text written through [`print_body`] left stdout
// mid-line (no trailing newline). The footer is printed by the dispatcher
// after the command returns and cannot see the body, so a command whose body
// may lack a trailing newline prints it via [`print_body`] and the footer
// emitter consults this to start `cache:` on its own line.
// trace:BUG-1975 | ai:claude
static STDOUT_LINE_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Print a command body to stdout verbatim (no newline added) and record
/// whether it ended mid-line, so a following `cache:` footer is separated
/// from it by exactly one newline.
// trace:BUG-1975 | ai:claude
pub(crate) fn print_body(body: &str) {
    print!("{body}");
    note_stdout_tail(body);
}

// trace:BUG-1975 | ai:claude
fn note_stdout_tail(written: &str) {
    if !written.is_empty() {
        STDOUT_LINE_OPEN.store(
            !written.ends_with('\n'),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

/// The footer text to print (followed by a newline): prefixed with one
/// newline only when the body left stdout mid-line, so it always starts on
/// its own line without ever adding a blank line.
// trace:BUG-1975 | ai:claude
fn footer_after_body(footer: &str, line_open: bool) -> String {
    if line_open {
        format!("\n{footer}")
    } else {
        footer.to_string()
    }
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
        // trace:BUG-1975 | ai:claude
        let line_open = STDOUT_LINE_OPEN.load(std::sync::atomic::Ordering::Relaxed);
        println!(
            "{}",
            footer_after_body(&toon_cache(&scope.metadata()), line_open)
        );
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

    // trace:BUG-1975 | ai:claude
    #[test]
    fn footer_starts_on_its_own_line_after_unterminated_body() {
        let body = "last body line";
        let out = format!(
            "{body}{}\n",
            footer_after_body("cache:\n  stale: false", true)
        );
        assert_eq!(out, "last body line\ncache:\n  stale: false\n");
    }

    // trace:BUG-1975 | ai:claude
    #[test]
    fn footer_adds_no_blank_line_after_terminated_body() {
        let body = "last body line\n";
        let out = format!(
            "{body}{}\n",
            footer_after_body("cache:\n  stale: false", false)
        );
        assert_eq!(out, "last body line\ncache:\n  stale: false\n");
        assert!(!out.contains("\n\n"));
    }

    // trace:BUG-1975 | ai:claude
    #[test]
    fn stdout_tail_tracks_the_last_non_empty_write() {
        use std::sync::atomic::Ordering::Relaxed;
        note_stdout_tail("no newline");
        assert!(STDOUT_LINE_OPEN.load(Relaxed));
        note_stdout_tail("");
        assert!(STDOUT_LINE_OPEN.load(Relaxed), "empty writes keep state");
        note_stdout_tail("terminated\n");
        assert!(!STDOUT_LINE_OPEN.load(Relaxed));
    }
}
