// TASK-1558: `[agents] mcp` must FAIL the launch on a value it does not
// recognise, and that has to include a wrong TYPE. `as_str()` returns None for
// `mcp = true` exactly as it does for an absent key, so without this the caller
// silently falls back to the default `off` while the operator believes they
// configured `aida` or `native` — the guard failing open on the likeliest typo.
// Found by an independent cross-vendor review of PR #2247.
// trace:TASK-1558 | ai:claude

use super::read_agents_string_from_file;

fn agents_toml(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agents.toml");
    std::fs::write(&path, body).unwrap();
    (dir, path)
}

#[test]
fn a_quoted_string_reads_through() {
    for value in ["off", "aida", "native"] {
        let (_d, p) = agents_toml(&format!("[agents]\nmcp = \"{value}\"\n"));
        assert_eq!(
            read_agents_string_from_file(&p, "mcp").unwrap(),
            Some(value.to_string())
        );
    }
}

#[test]
fn an_absent_key_is_a_miss_not_an_error() {
    let (_d, p) = agents_toml("[agents]\ncontained = true\n");
    assert_eq!(read_agents_string_from_file(&p, "mcp").unwrap(), None);
}

#[test]
fn an_absent_agents_table_is_a_miss_not_an_error() {
    let (_d, p) = agents_toml("[other]\nmcp = \"aida\"\n");
    assert_eq!(read_agents_string_from_file(&p, "mcp").unwrap(), None);
}

// The regression this exists for: every non-string type must REFUSE rather than
// read as absent. `mcp = true` is the typo that motivated it.
#[test]
fn a_wrong_typed_value_refuses_instead_of_silently_defaulting() {
    for body in [
        "[agents]\nmcp = true\n",
        "[agents]\nmcp = 3\n",
        "[agents]\nmcp = 1.5\n",
        "[agents]\nmcp = [\"aida\"]\n",
        "[agents]\nmcp = { surface = \"aida\" }\n",
    ] {
        let (_d, p) = agents_toml(body);
        let err = read_agents_string_from_file(&p, "mcp")
            .expect_err(&format!("must refuse a non-string value: {body:?}"));
        let msg = err.to_string();
        assert!(
            msg.contains("[agents] mcp"),
            "the refusal must name the key: {msg}"
        );
        assert!(
            msg.contains("must be a quoted string"),
            "the refusal must say what is expected: {msg}"
        );
    }
}

// The message names the actual type, so the operator does not have to guess what
// the parser objected to.
#[test]
fn the_refusal_names_the_offending_type_and_file() {
    let (_d, p) = agents_toml("[agents]\nmcp = true\n");
    let msg = read_agents_string_from_file(&p, "mcp")
        .unwrap_err()
        .to_string();
    assert!(msg.contains("a boolean"), "names the type: {msg}");
    assert!(
        msg.contains(p.to_string_lossy().as_ref()),
        "names the file: {msg}"
    );
}
