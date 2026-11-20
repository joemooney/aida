use super::*;
#[test]
fn task_1222_gc_requires_exact_closed_head_and_no_open_incarnation() {
    let row = serde_json::json!({"state":"MERGED", "headRefName":"task-1", "headRefOid":"abc", "isCrossRepository":false});
    let parse = |rows: serde_json::Value| {
        parse_gc_branch_closed(
            &serde_json::to_vec(&rows).unwrap(),
            ForgeKind::GitHub,
            "task-1",
            "abc",
        )
    };
    assert_eq!(parse(serde_json::json!([row.clone()])), Some(true));
    let mut closed = row.clone();
    closed["state"] = "CLOSED".into();
    assert_eq!(parse(serde_json::json!([closed])), Some(true));
    let mut newer = row.clone();
    newer["headRefOid"] = "def".into();
    assert_eq!(parse(serde_json::json!([newer.clone()])), Some(false));
    newer["state"] = "OPEN".into();
    assert_eq!(parse(serde_json::json!([row.clone(), newer])), Some(false));
    let mut fork = row.clone();
    fork["isCrossRepository"] = true.into();
    assert_eq!(parse(serde_json::json!([fork])), Some(false));
    assert_eq!(parse(serde_json::json!([{}])), None);
    assert_eq!(parse(serde_json::json!(vec![row; 100])), None);
    assert_eq!(parse(serde_json::json!([])), Some(false));
}
#[test]
fn task_1222_gc_reviewer_alias_pins_number_repository_and_exact_head() {
    // trace:TASK-1222.acfbfd06 | ai:codex
    assert_eq!(gc_reviewer_pr_number("pr-3647"), Some(3647));
    for branch in ["pr-0", "pr-01", "pr-+1", "pr-1-extra", "task-3647"] {
        assert_eq!(gc_reviewer_pr_number(branch), None);
    }
    let row = serde_json::json!({"number":3647,"state":"MERGED", "headRefName":"task-1222", "headRefOid":"abc", "isCrossRepository":false});
    let parse = |row: &serde_json::Value| {
        parse_gc_reviewer_pr_closed(&serde_json::to_vec(row).unwrap(), 3647, "abc")
    };
    assert_eq!(parse(&row), Some(true));
    for (field, value, expected) in [
        ("state", serde_json::json!("CLOSED"), Some(true)),
        ("state", serde_json::json!("OPEN"), Some(false)),
        ("headRefOid", serde_json::json!("different"), Some(false)),
        ("isCrossRepository", serde_json::json!(true), Some(false)),
        ("number", serde_json::json!(3648), None),
        ("headRefOid", serde_json::Value::Null, None),
    ] {
        let mut changed = row.clone();
        changed[field] = value;
        assert_eq!(parse(&changed), expected, "{field}");
    }
    assert_eq!(parse(&serde_json::json!({})), None);
}

#[test]
fn task_1222_gc_gitlab_requires_same_project_and_head() {
    let row = serde_json::json!({"state":"closed", "source_branch":"task-1", "sha":"abc", "source_project_id":1,"target_project_id":1});
    let parse = |rows: serde_json::Value| {
        parse_gc_branch_closed(
            &serde_json::to_vec(&rows).unwrap(),
            ForgeKind::GitLab,
            "task-1",
            "abc",
        )
    };
    assert_eq!(parse(serde_json::json!([row.clone()])), Some(true));
    let mut fork = row;
    fork["source_project_id"] = 2.into();
    assert_eq!(parse(serde_json::json!([fork])), Some(false));
}
