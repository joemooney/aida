use super::*;

/// `--type doc` and `--type documentation` both resolve to
/// `RequirementType::Doc`. Aliases keep the user-facing surface tolerant
/// to spell-outs, matching the precedent set by `--type adr`/`decision`.
// trace:STORY-104 | ai:claude
#[test]
fn parses_doc_aliases() {
    assert_eq!(parse_requirement_type("doc").unwrap(), RequirementType::Doc);
    assert_eq!(parse_requirement_type("DOC").unwrap(), RequirementType::Doc);
    assert_eq!(
        parse_requirement_type("documentation").unwrap(),
        RequirementType::Doc
    );
}

/// Drift guard (TASK-716): every `RequirementType` variant must have a
/// canonical lowercase string that `parse_requirement_type` round-trips.
/// The `match` below is exhaustive, so adding a new variant to the enum
/// without wiring it into the CLI parser fails to compile here — keeping
/// the documented/user-facing type list from silently drifting behind the
/// model (the 13-vs-19 drift this test was filed to prevent).
// trace:TASK-716 | ai:claude
#[test]
fn every_requirement_type_variant_round_trips() {
    use aida_core::models::RequirementType::*;
    // One representative variant per arm; the exhaustive match means a new
    // enum variant forces a compile error until it is added here.
    let all = [
        Functional,
        NonFunctional,
        System,
        User,
        ChangeRequest,
        Bug,
        Epic,
        Story,
        Task,
        Spike,
        Sprint,
        Folder,
        Meta,
        Principle,
        Vision,
        Constraint,
        Decision,
        Term,
        Doc,
    ];
    assert_eq!(all.len(), 19, "RequirementType is expected to have 19 variants; update docs (CLAUDE.md, --type help, MCP schema) and this guard when it changes");
    // Exhaustiveness check: maps each variant to its canonical CLI token.
    // The compiler enforces every variant is covered.
    for variant in all {
        let token = match &variant {
            Functional => "functional",
            NonFunctional => "non-functional",
            System => "system",
            User => "user",
            ChangeRequest => "change-request",
            Bug => "bug",
            Epic => "epic",
            Story => "story",
            Task => "task",
            Spike => "spike",
            Sprint => "sprint",
            Folder => "folder",
            Meta => "meta",
            Principle => "principle",
            Vision => "vision",
            Constraint => "constraint",
            Decision => "decision",
            Term => "term",
            Doc => "doc",
        };
        assert_eq!(
            parse_requirement_type(token).unwrap(),
            variant,
            "type token {token:?} must parse back to its variant"
        );
    }
}
