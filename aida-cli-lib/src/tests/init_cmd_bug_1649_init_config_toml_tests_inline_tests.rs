use super::{distributed_worktree_config_head, sibling_store_config_head};

const HOSTILE: [&str; 3] = [
    "C:\\Users\\RUNNER~1\\x",
    "../has\"quote/.aida-store",
    "C:\\Users\\RUNNER~1\\\"both'\\store",
];

fn deployment(body: &str) -> toml::Table {
    let parsed: toml::Table =
        toml::from_str(body).unwrap_or_else(|e| panic!("invalid TOML ({e}):\n{body}"));
    parsed["deployment"].as_table().unwrap().clone()
}

#[test]
fn bug_1649_worktree_head_keeps_plain_values_byte_identical() {
    let body = distributed_worktree_config_head(".aida-store", "aida-store");
    assert!(body.contains(
        "mode = \"distributed\"\nstore_path = \".aida-store\"\nstore_type = \"worktree\"\nbranch = \"aida-store\"\n"
    ));
}

#[test]
fn bug_1649_worktree_head_round_trips_backslash_and_quote_paths() {
    for value in HOSTILE {
        let d = deployment(&distributed_worktree_config_head(value, value));
        assert_eq!(d["store_path"].as_str(), Some(value));
        assert_eq!(d["branch"].as_str(), Some(value));
        assert_eq!(d["store_type"].as_str(), Some("worktree"));
    }
}

#[test]
fn bug_1649_sibling_head_keeps_plain_values_byte_identical() {
    let body = sibling_store_config_head("../aida-store");
    assert!(body.contains("store_path = \"../aida-store\"\nstore_type = \"sibling\"\n"));
}

#[test]
fn bug_1649_sibling_head_round_trips_backslash_and_quote_paths() {
    for value in HOSTILE {
        let d = deployment(&sibling_store_config_head(value));
        assert_eq!(d["store_path"].as_str(), Some(value));
        assert_eq!(d["store_type"].as_str(), Some("sibling"));
    }
}
