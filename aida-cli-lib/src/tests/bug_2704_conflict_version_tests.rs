//! BUG-2704: the mandate history reader judges every resolution of a store
//! version committed with git conflict markers.
// trace:BUG-2704 | ai:claude
use super::*;

fn yaml(title: &str) -> String {
    serde_yaml::to_string(&aida_core::Requirement::new(title.into(), String::new())).unwrap()
}

/// Wrap the line starting `field:` in a conflict hunk whose stashed side
/// is `theirs` (the 7124c0f shape is `modified_at`).
fn conflict(body: &str, field: &str, ours: &str, theirs: &str, base: Option<&str>) -> String {
    body.lines()
        .map(|line| {
            if line.starts_with(&format!("{field}:")) {
                let base = base.map_or(String::new(), |b| format!("||||||| base\n{b}\n"));
                format!(
                    "<<<<<<< Updated upstream\n{ours}\n{base}=======\n{theirs}\n>>>>>>> Stashed changes\n"
                )
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

// trace:BUG-2704 | ai:claude
#[test]
fn bug_2704_both_sides_of_a_conflicted_version_are_judged() {
    let body = yaml("plain work");
    let stamp = "modified_at: 2026-11-07T23:59:00Z";
    let marked = conflict(
        &body,
        "modified_at",
        stamp,
        "modified_at: 2026-11-07T23:58:00Z",
        None,
    );
    assert!(parse_version(&marked).is_err(), "markers alone never parse");
    let sides = parse_versions(&marked).unwrap();
    assert_eq!(sides.len(), 2);
    assert!(sides
        .iter()
        .all(|v| v.title == "plain work" && v.tags.is_empty()));

    // A side carrying the operator-only tag is one of the versions.
    let tagged = conflict(
        &body,
        "modified_at",
        stamp,
        &format!("{stamp}\ntags:\n- operator-only"),
        None,
    );
    let sides = parse_versions(&tagged).unwrap();
    assert_eq!(sides.len(), 2);
    assert!(sides.iter().any(|v| v.tags.contains("operator-only")));
    assert!(sides.iter().any(|v| v.tags.is_empty()));

    // diff3: the recorded base is a resolution too.
    let diff3 = conflict(
        &body,
        "modified_at",
        stamp,
        "modified_at: 2026-11-07T23:58:00Z",
        Some(&format!("{stamp}\ntags:\n- operator-only")),
    );
    let sides = parse_versions(&diff3).unwrap();
    assert_eq!(sides.len(), 3);
    assert!(sides.iter().any(|v| v.tags.contains("operator-only")));

    // Two hunks: every combination, so a class only a mix makes is seen.
    let two = conflict(&tagged, "title", "title: plain work", "title: other", None);
    let sides = parse_versions(&two).unwrap();
    assert_eq!(sides.len(), 4);
    assert!(sides
        .iter()
        .any(|v| v.title == "other" && v.tags.contains("operator-only")));
    // CRLF line endings are recognised the same way.
    assert_eq!(
        parse_versions(&marked.replace('\n', "\r\n")).unwrap().len(),
        2
    );
}

// trace:BUG-2704 | ai:claude
#[test]
fn bug_2704_malformed_or_unparseable_conflicts_stay_unreadable() {
    let body = yaml("plain work");
    let ok = conflict(&body, "title", "title: a", "title: b", None);
    for bad in [
        // Unterminated hunk.
        ok.replace(">>>>>>> Stashed changes\n", ""),
        // Separator outside a hunk.
        format!("=======\n{body}"),
        // Nested open.
        ok.replace("=======\n", "<<<<<<< again\n=======\n"),
        // Close before the separator.
        ok.replace("=======\n", ""),
        // One side does not parse.
        conflict(&body, "title", "title: a", "title: [unclosed", None),
    ] {
        assert!(parse_versions(&bad).is_err(), "{bad}");
    }
    // Too many combinations fails closed.
    let mut many = body.clone();
    for i in 0..9 {
        many.push_str(&format!(
            "<<<<<<< a\nx{i}: 1\n=======\nx{i}: 2\n>>>>>>> b\n"
        ));
    }
    let err = parse_versions(&many).unwrap_err();
    assert!(err.contains("more than"), "{err}");
    // An unconflicted body is one version, as before.
    assert_eq!(parse_versions(&body).unwrap().len(), 1);
    // A label-less marker counts; a longer run of the character does not.
    assert!(aida_core::git_ops::conflict_marker("=======").is_some());
    assert!(aida_core::git_ops::conflict_marker("========").is_none());
    assert!(aida_core::git_ops::conflict_marker("<<<<<<<x").is_none());
}
