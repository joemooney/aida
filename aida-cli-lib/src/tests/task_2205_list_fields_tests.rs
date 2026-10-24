//! `aida list --fields`: the `opened` field and its aliases, comma and/or
//! whitespace separators, and help text generated from the parser's set.
// trace:TASK-2205 | ai:claude

use super::task970_agent_output_tests::fields_summary;
use super::*;

#[test]
fn fields_split_on_commas_and_or_whitespace() {
    let want = vec!["id", "title", "status", "modified_at"];
    assert_eq!(
        toon_list_fields(Some("id title status modified_at")).unwrap(),
        want
    );
    assert_eq!(
        toon_list_fields(Some("id, title,  status\tmodified_at")).unwrap(),
        want
    );
    // Empty segments (doubled / leading / trailing separators) are ignored.
    assert_eq!(
        toon_list_fields(Some(" ,id,,title , ,status modified_at, ")).unwrap(),
        want
    );
}

#[test]
fn opened_aliases_resolve_and_keep_requested_order() {
    assert_eq!(
        toon_list_fields(Some("opened id")).unwrap(),
        vec!["opened", "id"]
    );
    assert_eq!(
        toon_list_fields(Some("id created_at title")).unwrap(),
        vec!["id", "opened", "title"]
    );
    assert_eq!(
        toon_list_fields(Some("CREATED,id")).unwrap(),
        vec!["opened", "id"]
    );
}

#[test]
fn opened_renders_creation_time_like_modified_at() {
    colored::control::set_override(false);
    let mut r = fields_summary("TASK-1", "thing", "approved", "medium");
    r.created_at = "2026-10-01T12:00:00+00:00".to_string();
    r.modified_at = "2026-10-20T08:30:00+00:00".to_string();
    let routing = (false, false, false);
    assert_eq!(toon_list_cell(&r, routing, "opened"), r.created_at);
    assert_eq!(toon_list_cell(&r, routing, "modified_at"), r.modified_at);

    let selected = toon_list_fields(Some("opened id modified_at")).unwrap();
    let lines = build_list_fields_lines(&[r.clone()], &selected, |_| routing);
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        vec!["Opened", "ID", "Modified"]
    );
    assert_eq!(
        lines[2].split_whitespace().collect::<Vec<_>>(),
        vec![r.created_at.as_str(), "TASK-1", r.modified_at.as_str()]
    );
}

#[test]
fn unknown_field_still_errors_with_valid_list() {
    let err = toon_list_fields(Some("id bogus title"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown --fields entry `bogus`"), "{err}");
    assert!(
        err.contains(&TOON_LIST_KNOWN_FIELDS.join(", ")),
        "error must name the full valid set: {err}"
    );
    assert!(err.contains("opened"), "{err}");
}

/// The rendered `aida list --help` entry for `--fields` names exactly the set
/// the parser accepts — every known field, every alias, nothing else.
#[test]
fn list_fields_help_matches_parser_set() {
    let cmd = crate::command_registry::built_command();
    let list = cmd
        .get_subcommands()
        .find(|s| s.get_name() == "list")
        .expect("list subcommand");
    let arg = list
        .get_arguments()
        .find(|a| a.get_id() == "fields")
        .expect("--fields arg");
    let help = arg.get_help().expect("--fields help").to_string();
    assert_eq!(help, list_fields_help());

    let listed = help
        .split("Valid fields: ")
        .nth(1)
        .and_then(|rest| rest.split(". ").next())
        .expect("help names the valid fields");
    let listed: Vec<&str> = listed.split(", ").collect();
    assert_eq!(listed, TOON_LIST_KNOWN_FIELDS);
    for field in &listed {
        assert_eq!(toon_list_fields(Some(field)).unwrap(), vec![*field]);
    }
    for (alias, field) in TOON_LIST_FIELD_ALIASES {
        assert!(help.contains(&format!("{alias} = {field}")), "{help}");
        assert_eq!(toon_list_fields(Some(alias)).unwrap(), vec![*field]);
    }
}

/// TASK-2234: `aida search --fields` parses through the same
/// [`toon_list_fields`], so its rendered help names exactly the same set.
// trace:TASK-2238 | ai:claude
#[test]
fn search_fields_help_matches_parser_set() {
    let cmd = crate::command_registry::built_command();
    let search = cmd
        .get_subcommands()
        .find(|s| s.get_name() == "search")
        .expect("search subcommand");
    let arg = search
        .get_arguments()
        .find(|a| a.get_id() == "fields")
        .expect("--fields arg");
    let help = arg.get_help().expect("--fields help").to_string();
    assert_eq!(help, search_fields_help());

    let listed = help
        .split("Valid fields: ")
        .nth(1)
        .and_then(|rest| rest.split(". ").next())
        .expect("help names the valid fields");
    let listed: Vec<&str> = listed.split(", ").collect();
    assert_eq!(listed, TOON_LIST_KNOWN_FIELDS);
    for (alias, field) in TOON_LIST_FIELD_ALIASES {
        assert!(help.contains(&format!("{alias} = {field}")), "{help}");
    }
}

/// TASK-2240: a copied TOON header's field list parses; a human display
/// header such as `Modified` is a label, not a field name, and is refused, as
/// the `toon_list_fields` doc now says.
// trace:TASK-2238 | ai:claude
#[test]
fn toon_header_fields_parse_but_human_labels_are_refused() {
    assert_eq!(
        toon_list_fields(Some("id,title,status,type,modified_at")).unwrap(),
        TOON_LIST_DEFAULT_FIELDS
    );
    let err = toon_list_fields(Some("ID Modified"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown --fields entry `modified`"), "{err}");
}
