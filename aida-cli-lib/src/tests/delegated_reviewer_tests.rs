use super::*;

#[test]
fn test_parse_review_mode() {
    let content_local = r#"
[review]
mode = "local"
"#;
    assert_eq!(parse_review_mode(content_local), "local");

    let content_delegated = r#"
[review]
mode = "delegated"
"#;
    assert_eq!(parse_review_mode(content_delegated), "delegated");

    let content_default = r#"
[behavior]
permission_mode = "acceptEdits"
"#;
    assert_eq!(parse_review_mode(content_default), "local");

    let content_comments = r#"
# some comment
[review] # another comment
mode = "delegated" # inline comment
"#;
    assert_eq!(parse_review_mode(content_comments), "delegated");
}

#[test]
fn test_extract_bughunter_severity() {
    let valid_json =
        r#"some text bughunter-severity: {"normal": 2, "nit": 1, "pre_existing": 0} trailing text"#;
    let parsed = extract_bughunter_severity(valid_json).unwrap();
    assert_eq!(parsed["normal"].as_i64().unwrap(), 2);
    assert_eq!(parsed["nit"].as_i64().unwrap(), 1);
    assert_eq!(parsed["pre_existing"].as_i64().unwrap(), 0);

    let quoted_json =
        r#"some text "bughunter-severity": {"normal": 0, "nit": 3, "pre_existing": 1} trailing"#;
    let parsed_quoted = extract_bughunter_severity(quoted_json).unwrap();
    assert_eq!(parsed_quoted["normal"].as_i64().unwrap(), 0);
    assert_eq!(parsed_quoted["nit"].as_i64().unwrap(), 3);
    assert_eq!(parsed_quoted["pre_existing"].as_i64().unwrap(), 1);

    let single_quoted_json = r#"some text 'bughunter-severity': {'normal': 5, 'nit': 0} trailing"#;
    // single quotes inside JSON are technically malformed but our helper searches for start brace and extracts valid JSON
    // Standard JSON does not allow single quoted keys/values.
    assert!(extract_bughunter_severity(single_quoted_json).is_none());
    // Let's test a valid JSON with single quoted marker
    let single_quoted_marker =
        r#"some text 'bughunter-severity': {"normal": 5, "nit": 0, "pre_existing": 2} trailing"#;
    let parsed_sq = extract_bughunter_severity(single_quoted_marker).unwrap();
    assert_eq!(parsed_sq["normal"].as_i64().unwrap(), 5);
    assert_eq!(parsed_sq["nit"].as_i64().unwrap(), 0);
    assert_eq!(parsed_sq["pre_existing"].as_i64().unwrap(), 2);

    let missing = r#"no severity tally here"#;
    assert!(extract_bughunter_severity(missing).is_none());

    let malformed = r#"bughunter-severity: {invalid_json}"#;
    assert!(extract_bughunter_severity(malformed).is_none());
}
