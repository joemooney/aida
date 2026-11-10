// trace:TASK-3780 | ai:codex
use super::*;
use std::path::{Path, PathBuf};

// The whole point of `worktree_parent`: trust INHERITS, so a grant on the
// parent covers every worktree nested under it and doctor stays silent.
// trace:BUG-1700 | ai:claude
#[test]
fn trusted_parent_covers_nested_worktrees() {
    let trusted = vec![PathBuf::from("/home/op/aida-worktrees")];
    assert!(worktree_trust_findings(
        Path::new("/home/op/aida-worktrees"),
        true,
        Some(&trusted),
        None
    )
    .is_empty());
}

// trace:BUG-1700 | ai:claude
#[test]
fn trusted_ancestor_further_up_also_covers() {
    let trusted = vec![PathBuf::from("/home/op")];
    assert!(worktree_trust_findings(
        Path::new("/home/op/aida-worktrees"),
        true,
        Some(&trusted),
        None
    )
    .is_empty());
}

// trace:BUG-1700 trace:BUG-1777 | ai:claude+codex
#[test]
fn untrusted_parent_is_reported_once_and_is_not_auto_healable() {
    let trusted = vec![PathBuf::from("/home/op/other")];
    let f = worktree_trust_findings(
        Path::new("/home/op/aida-worktrees"),
        true,
        Some(&trusted),
        None,
    );
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].category, "agent-launch");
    assert_eq!(f[0].id, "worktree-folder-trust");
    assert!(
        f[0].summary
            .replace('\\', "/")
            .contains("/home/op/aida-worktrees"),
        "the finding must name the directory: {}",
        f[0].summary
    );
    assert!(
        !f[0].safe_heal,
        "granting folder trust is the operator's call — never auto-healed"
    );
}

// A sibling of a trusted path is NOT covered: `starts_with` must compare
// whole path components, not string prefixes, or `/home/op/aida-worktrees2`
// would be wrongly judged trusted by `/home/op/aida-worktrees`.
// trace:BUG-1700 | ai:claude
#[test]
fn string_prefix_sibling_is_not_treated_as_trusted() {
    let trusted = vec![PathBuf::from("/home/op/aida-worktrees")];
    assert_eq!(
        worktree_trust_findings(
            Path::new("/home/op/aida-worktrees2"),
            true,
            Some(&trusted),
            None
        )
        .len(),
        1,
        "a sibling sharing a string prefix must not inherit trust"
    );
}

// Unknown trust (no readable ~/.claude.json) must read as unknown, not as a
// problem — otherwise doctor cries wolf on every host without the file.
// trace:BUG-1700 | ai:claude
#[test]
fn unknown_trust_is_silent() {
    assert!(worktree_trust_findings(Path::new("/home/op/wt"), true, None, None).is_empty());
}

// With no worktree_parent configured the advice is to configure one; with
// one configured the advice is to trust that directory once. Different
// actions, so the operator is never told to do the wrong thing.
// trace:BUG-1700 | ai:claude
#[test]
fn action_differs_on_whether_a_parent_is_configured() {
    let trusted: Vec<PathBuf> = Vec::new();
    let configured = worktree_trust_findings(Path::new("/home/op/wt"), true, Some(&trusted), None);
    assert!(
        configured[0].action.contains("ONCE"),
        "configured: {}",
        configured[0].action
    );
    let unset = worktree_trust_findings(Path::new("/home/op"), false, Some(&trusted), None);
    assert!(
        unset[0].action.contains("worktree_parent"),
        "unset must point at the setting: {}",
        unset[0].action
    );
}

// The regression an independent review caught: `starts_with` is lexical, so a
// relative `worktree_parent` resolving to `<root>/../aida-worktrees` did not
// match the normalised `/home/joe/ai/aida-worktrees` that ~/.claude.json
// records — doctor cried wolf on an ALREADY-trusted directory.
// trace:BUG-1700 | ai:claude
#[test]
fn a_relative_parent_with_dotdot_matches_its_normalised_trusted_form() {
    let trusted = vec![PathBuf::from("/home/joe/ai/aida-worktrees")];
    let unnormalised = Path::new("/home/joe/ai/aida/../aida-worktrees");
    assert!(
        !unnormalised.starts_with(&trusted[0]),
        "precondition: the raw path must NOT lexically match, or this test proves nothing"
    );
    assert!(
        worktree_trust_findings(unnormalised, true, Some(&trusted), None).is_empty(),
        "an already-trusted directory reached via `..` must not be reported"
    );
}

// trace:BUG-1700 | ai:claude
#[test]
fn normalisation_does_not_make_a_sibling_match() {
    let trusted = vec![PathBuf::from("/home/joe/ai/aida-worktrees")];
    assert_eq!(
        worktree_trust_findings(
            Path::new("/home/joe/ai/aida/../aida-worktrees2"),
            true,
            Some(&trusted),
            None
        )
        .len(),
        1,
        "normalising must not loosen the sibling check"
    );
}

// trace:BUG-1700 | ai:claude
#[test]
fn lexical_normalisation_resolves_dot_and_dotdot_without_the_filesystem() {
    for (input, want) in [
        ("/a/b/../c", "/a/c"),
        ("/a/./b", "/a/b"),
        ("/a/b/../../c", "/c"),
        ("/a/b/c/../..", "/a"),
        ("relative/../x", "x"),
    ] {
        assert_eq!(
            normalize_path_lexically(Path::new(input)),
            PathBuf::from(want),
            "normalising {input}"
        );
    }
}

// A `..` with nothing to pop must be KEPT, not silently dropped — dropping it
// would turn one path into a different one.
// trace:BUG-1700 | ai:claude
#[test]
fn normalisation_keeps_a_dotdot_it_cannot_resolve() {
    assert_eq!(
        normalize_path_lexically(Path::new("../x")),
        PathBuf::from("../x")
    );
    assert_eq!(
        normalize_path_lexically(Path::new("/..")),
        PathBuf::from("/")
    );
}

// The resolver must mirror `worktree_pool::pool_path_for`'s parent choice,
// or doctor would advise trusting a directory the pool never uses.
// trace:BUG-1700 | ai:claude
#[test]
fn resolver_mirrors_the_pool_parent_choice() {
    let root = Path::new("/work/myrepo");
    assert_eq!(
        resolve_worktree_parent(root, None),
        PathBuf::from("/work"),
        "unset => the project root's parent (sibling layout)"
    );
    assert_eq!(
        resolve_worktree_parent(root, Some(Path::new("/trusted/wt"))),
        PathBuf::from("/trusted/wt")
    );
    assert_eq!(
        resolve_worktree_parent(root, Some(Path::new("../wt"))),
        PathBuf::from("/work/myrepo/../wt"),
        "relative resolves against the project root, as the pool does"
    );
}
