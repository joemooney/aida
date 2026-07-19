use super::*;
use crate::forge::ForgeKind;

/// Convenience: collect the (label, value) lines into a single string
/// so assertions can match against the full rendered block.
fn render(forge: ForgeKind, state: &ChangeLinkageState) -> String {
    format_change_linkage(forge, state)
        .into_iter()
        .map(|(l, v)| format!("{l}: {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn github_shipped_uses_pr_noun() {
    let out = render(
        ForgeKind::GitHub,
        &ChangeLinkageState::Shipped { number: Some(42) },
    );
    assert!(out.contains("Branch: merged to main"), "{out}");
    assert!(out.contains("PR: PR-42"), "{out}");
}

#[test]
fn gitlab_shipped_uses_mr_noun() {
    let out = render(
        ForgeKind::GitLab,
        &ChangeLinkageState::Shipped { number: Some(7) },
    );
    assert!(out.contains("Branch: merged to main"), "{out}");
    assert!(out.contains("MR: MR-7"), "{out}");
    // The GitHub-only noun must NOT leak onto a GitLab repo.
    assert!(!out.contains("PR-7"), "{out}");
}

#[test]
fn shipped_without_number_omits_change_line() {
    let out = render(
        ForgeKind::GitLab,
        &ChangeLinkageState::Shipped { number: None },
    );
    assert_eq!(out, "Branch: merged to main");
}

#[test]
fn gitlab_in_flight_found_renders_mr_and_url() {
    let out = render(
        ForgeKind::GitLab,
        &ChangeLinkageState::InFlightFound {
            number: 13,
            url: "https://gitlab.com/joe/aida-gl-test/-/merge_requests/13".to_string(),
        },
    );
    assert!(
        out.contains("MR: MR-13 https://gitlab.com/joe/aida-gl-test/-/merge_requests/13"),
        "{out}"
    );
}

#[test]
fn github_in_flight_found_renders_pr_and_url() {
    let out = render(
        ForgeKind::GitHub,
        &ChangeLinkageState::InFlightFound {
            number: 99,
            url: "https://github.com/joe/aida/pull/99".to_string(),
        },
    );
    assert!(
        out.contains("PR: PR-99 https://github.com/joe/aida/pull/99"),
        "{out}"
    );
}

#[test]
fn no_change_uses_forge_noun() {
    assert_eq!(
        render(ForgeKind::GitHub, &ChangeLinkageState::InFlightNoChange),
        "PR: no PR opened yet"
    );
    assert_eq!(
        render(ForgeKind::GitLab, &ChangeLinkageState::InFlightNoChange),
        "MR: no MR opened yet"
    );
}

#[test]
fn cli_missing_names_the_right_cli_per_forge() {
    let gh = render(ForgeKind::GitHub, &ChangeLinkageState::CliMissing);
    assert!(gh.contains("gh not installed"), "{gh}");
    assert!(gh.contains("PR state unknown"), "{gh}");

    let gl = render(ForgeKind::GitLab, &ChangeLinkageState::CliMissing);
    assert!(gl.contains("glab not installed"), "{gl}");
    assert!(gl.contains("MR state unknown"), "{gl}");
    // The GitLab diagnostic must not name `gh`.
    assert!(!gl.contains("gh "), "{gl}");
}

#[test]
fn pure_git_cli_missing_uses_generic_label() {
    // Pure-git has no forge CLI binary; the diagnostic must not print an
    // empty token ("  not installed").
    let out = render(ForgeKind::None, &ChangeLinkageState::CliMissing);
    assert!(out.contains("the forge CLI not installed"), "{out}");
    assert!(out.contains("change state unknown"), "{out}");
}

#[test]
fn cli_failed_and_unreachable_use_forge_noun() {
    let gl_fail = render(ForgeKind::GitLab, &ChangeLinkageState::CliFailed);
    assert!(gl_fail.contains("glab lookup failed"), "{gl_fail}");
    assert!(gl_fail.contains("MR state unknown"), "{gl_fail}");

    let gl_unreach = render(ForgeKind::GitLab, &ChangeLinkageState::Unreachable);
    assert!(gl_unreach.contains("MR API unreachable"), "{gl_unreach}");
    assert!(gl_unreach.contains("(transient)"), "{gl_unreach}");
}

#[test]
fn branch_not_found_is_forge_independent() {
    for f in [ForgeKind::GitHub, ForgeKind::GitLab, ForgeKind::None] {
        assert_eq!(
            render(f, &ChangeLinkageState::BranchNotFound),
            "Branch: work committed but branch not found locally"
        );
    }
}
