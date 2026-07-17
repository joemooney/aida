use super::*;
use crate::cli::Command;
use clap::Parser;

#[test]
fn search_short_and_aliases_parse() {
    for flag in ["--short", "-q", "--ids-only", "--quiet"] {
        let cli = Cli::try_parse_from(["aida", "search", "auth", flag])
            .unwrap_or_else(|e| panic!("`{flag}` should parse: {e}"));
        match cli.command {
            Command::Search { short, json, .. } => {
                assert!(short, "`{flag}` should set short");
                assert!(!json, "`{flag}` should not set json");
            }
            other => panic!("expected search, got {other:?}"),
        }
    }
}

#[test]
fn search_json_parses() {
    let cli = Cli::try_parse_from(["aida", "search", "auth", "--json"]).unwrap();
    match cli.command {
        Command::Search { short, json, .. } => {
            assert!(json);
            assert!(!short);
        }
        other => panic!("expected search, got {other:?}"),
    }
}

#[test]
fn search_short_and_json_are_mutually_exclusive() {
    // Mirrors `aida list`'s --short ⊥ --json. clap rejects the combo.
    assert!(
        Cli::try_parse_from(["aida", "search", "auth", "--short", "--json"]).is_err(),
        "--short and --json must be mutually exclusive"
    );
    assert!(
        Cli::try_parse_from(["aida", "search", "auth", "-q", "--json"]).is_err(),
        "-q and --json must be mutually exclusive"
    );
}
