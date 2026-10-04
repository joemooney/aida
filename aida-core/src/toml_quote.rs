//! Render a string as a TOML string value for hand-built config files.
//!
//! Several writers build `config.toml` bodies with `format!` so the scaffolded
//! comments survive. Interpolating a raw value between `"` characters is only
//! correct for plain ASCII: a Windows path such as `C:\Users\x` turns into
//! invalid escape sequences, and a value containing `"` ends the string early.
//! [`toml_string`] delegates quoting to the `toml` crate, so the result always
//! parses back to exactly the input. Ordinary values (no backslash, quote or
//! control character) render as `"value"`, byte-identical to the old output.
// trace:BUG-1649 | ai:claude

/// Quote `value` as a TOML string, including the surrounding quotes.
///
/// Use it in place of `key = \"{value}\"` in `format!`/`writeln!` templates:
/// `format!("key = {}", toml_string(value))`.
// trace:BUG-1649 | ai:claude
pub fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

/// Render `key` as a TOML key: bare when it is a non-empty run of
/// `[A-Za-z0-9_-]` (the TOML bare-key alphabet), otherwise a quoted
/// [`toml_string`]. Use it for a table-name segment or a key that comes from
/// data, e.g. `format!("[mcp_servers.{}]", toml_key(name))`.
// trace:BUG-1650 | ai:claude
pub fn toml_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        key.to_owned()
    } else {
        toml_string(key)
    }
}

#[cfg(test)]
mod bug_1650_tests {
    use super::{toml_key, toml_string};

    #[test]
    fn bug_1650_bare_keys_stay_bare() {
        for key in ["aida", "AIDA_AGENT_OUTPUT", "my-server_2"] {
            assert_eq!(toml_key(key), key);
        }
    }

    #[test]
    fn bug_1650_other_keys_are_quoted_and_round_trip() {
        for key in ["", "has.dot", "has space", "q\"uote", "a=b", "caf\u{e9}"] {
            let quoted = toml_key(key);
            assert_eq!(quoted, toml_string(key));
            let body = format!("{quoted} = 1\n[t.{quoted}]\n");
            let parsed: toml::Table = toml::from_str(&body)
                .unwrap_or_else(|e| panic!("{body:?} must parse as TOML: {e}"));
            assert_eq!(parsed[key].as_integer(), Some(1), "key {key:?}");
            assert!(parsed["t"].as_table().unwrap().contains_key(key));
        }
    }
}

#[cfg(test)]
mod bug_1649_tests {
    use super::toml_string;

    fn round_trip(value: &str) -> String {
        let body = format!("key = {}\n", toml_string(value));
        let parsed: toml::Table =
            toml::from_str(&body).unwrap_or_else(|e| panic!("{body:?} must parse as TOML: {e}"));
        parsed["key"].as_str().unwrap().to_owned()
    }

    #[test]
    fn bug_1649_plain_value_renders_as_basic_string() {
        assert_eq!(toml_string(".aida-store"), "\".aida-store\"");
        assert_eq!(toml_string("../aida-store"), "\"../aida-store\"");
        assert_eq!(toml_string("aida-store"), "\"aida-store\"");
    }

    #[test]
    fn bug_1649_hostile_values_round_trip() {
        for value in [
            "C:\\Users\\RUNNER~1\\x",
            "C:\\Users\\RUNNER~1\\AppData\\Local\\Temp\\.tmpAb12\\store",
            "../has\"quote/store",
            "C:\\mixed\\\"quote'and'apostrophe",
            "line\nbreak\ttab",
            "esc\u{1b}ape",
            "",
        ] {
            assert_eq!(round_trip(value), value, "value {value:?} must round-trip");
        }
    }
}
