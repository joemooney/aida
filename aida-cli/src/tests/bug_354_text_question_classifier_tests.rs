use super::*;

#[test]
fn final_result_with_decision_question_classifies_as_punt() {
    let log = r#"{"type":"result","subtype":"success","is_error":false,"result":"I found three viable paths.\n\nWhich path do you want me to take? My recommendation is B first because it is lowest-risk."}"#;
    let punt = pending_text_question_from_headless_log(log).expect("question must classify");
    assert!(
        punt.detail.contains("Which path do you want me to take?"),
        "{}",
        punt.detail
    );
    assert!(
        punt.lean.contains("recommendation"),
        "lean should preserve recommendation: {}",
        punt.lean
    );
}

#[test]
fn final_result_with_should_i_question_classifies_as_punt() {
    let text = "The fixture can be fixed two ways. Should I update the parser or wait for upstream? I recommend updating the parser.";
    let punt = pending_text_question_from_result_text(text).expect("should-I fork must classify");
    assert!(punt.detail.contains("Should I update the parser"));
    assert!(punt.lean.contains("recommend"));
}

#[test]
fn final_result_with_confirm_and_proceed_question_classifies_as_punt() {
    // BUG-374: STORY-444's headless implementer described a real fork,
    // recommended a path, then ended with a confirmation question. The
    // pre-BUG-374 marker list missed this wording and let the run fall
    // through to a generic phase-1 NoPr failure instead of advisor-tier
    // routing.
    let text = "There are two implementation paths: A all-in-one, or B split into two passes.\n\nRecommendation: take A because the changes are tightly coupled.\n\nConfirm and I'll proceed?";
    let punt = pending_text_question_from_result_text(text)
        .expect("confirm-and-proceed fork must classify");
    assert!(punt.detail.contains("Confirm and I'll proceed?"));
    assert!(punt.lean.contains("Recommendation: take A"));
}

#[test]
fn ordinary_summary_question_does_not_classify_without_fork_marker() {
    let text =
        "Implemented the change. Tests answer the question: does the parser handle aliases? Yes.";
    assert!(pending_text_question_from_result_text(text).is_none());
}

#[test]
fn recommendation_plus_unrelated_question_does_not_classify() {
    let text = "I recommend filing a follow-up. Tests answer the question: does the parser handle aliases? Yes.";
    assert!(pending_text_question_from_result_text(text).is_none());
}

#[test]
fn error_result_does_not_classify() {
    let log = r#"{"type":"result","subtype":"error","is_error":true,"result":"Which path should I take?"}"#;
    assert!(pending_text_question_from_headless_log(log).is_none());
}

#[test]
fn no_question_mark_does_not_classify() {
    let text = "My recommendation is B first. I will proceed with that path.";
    assert!(pending_text_question_from_result_text(text).is_none());
}

#[test]
fn detects_unmanaged_nested_repos_excluding_submodules_and_dotdirs() {
    // BUG-446: a workspace-of-projects — two sibling project repos.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("projA/.git")).unwrap();
    std::fs::create_dir_all(root.join("projB/.git")).unwrap();
    // a plain subdir (no own .git) must NOT count
    std::fs::create_dir_all(root.join("docs")).unwrap();
    // a dotted child repo (e.g. .aida-store worktree) must be skipped
    std::fs::create_dir_all(root.join(".aida-store/.git")).unwrap();
    // a registered submodule is intentional → excluded
    std::fs::create_dir_all(root.join("vendored/.git")).unwrap();
    std::fs::write(
        root.join(".gitmodules"),
        "[submodule \"vendored\"]\n\tpath = vendored\n\turl = https://example/x\n",
    )
    .unwrap();

    let found = unmanaged_nested_projects(root);
    assert_eq!(found, vec!["projA".to_string(), "projB".to_string()]);
}

#[test]
fn attached_store_present_detects_objects_dir() {
    // BUG-433: distributed mode is detectable from the store's shape
    // (.aida-store/objects/) even with no config marker / no branch.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(!attached_store_present(root)); // bare project — no store
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    assert!(!attached_store_present(root)); // .aida-store but not git-canonical
    std::fs::create_dir_all(root.join(".aida-store").join("objects")).unwrap();
    assert!(attached_store_present(root)); // objects/ present → attached store
}

#[test]
fn single_project_has_no_unmanaged_nested_repos() {
    // BUG-446: a normal single project — plain subdirs, no nested repos.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    assert!(unmanaged_nested_projects(root).is_empty());
}

#[test]
fn gitlink_file_counts_as_nested_repo() {
    // BUG-446: a submodule worktree NOT registered in .gitmodules has a
    // `.git` FILE (gitlink) — still a separate project, must be detected.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("stray")).unwrap();
    std::fs::write(
        root.join("stray/.git"),
        "gitdir: /elsewhere/.git/modules/stray\n",
    )
    .unwrap();
    assert_eq!(unmanaged_nested_projects(root), vec!["stray".to_string()]);
}

#[test]
fn child_aida_project_counts_even_without_git() {
    // TASK-686: a parent of AIDA projects (the ~/ai/ case) — a child with a
    // `.aida/` dir but no `.git` must still be detected, so an init scaffold
    // doesn't leak into a parent that every child inherits via ancestor
    // CLAUDE.md.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("proj-a/.aida")).unwrap();
    std::fs::create_dir_all(root.join("proj-b/.git")).unwrap();
    std::fs::create_dir_all(root.join("plain-dir/src")).unwrap();
    assert_eq!(
        unmanaged_nested_projects(root),
        vec!["proj-a".to_string(), "proj-b".to_string()]
    );
}

#[test]
fn git_init_decision_flag_wins_everywhere() {
    // STORY-552: explicit --git-init opts into auto-init regardless of TTY,
    // so scripted/non-TTY use can complete the onboarding funnel.
    assert_eq!(git_init_decision(true, false), GitInitDecision::Yes);
    assert_eq!(git_init_decision(true, true), GitInitDecision::Yes);
}

#[test]
fn git_init_decision_prompts_at_tty_bails_otherwise() {
    // STORY-552: at a TTY (no flag) we offer interactively; without a TTY
    // and without the flag we keep the safe bail — no surprising side
    // effect in scripts.
    assert_eq!(git_init_decision(false, true), GitInitDecision::Prompt);
    assert_eq!(git_init_decision(false, false), GitInitDecision::Bail);
}

#[test]
fn which_way_do_you_want_to_go_classifies_as_punt() {
    // BUG-462: the TASK-457 validation drain — the headless implementer hit
    // a real strategic fork, presented two options, and ended with "Which
    // way do you want to go?". The pre-BUG-462 marker list (which path /
    // do you want me to / want me to) missed this exact phrasing, so the
    // run fell through to a phase-1 NoPr failure instead of advisor routing.
    let text = "This is onboarding feature work during the bugs-before-marketing phase. \
                    A: implement the .antigravity scaffolding now. B: capture the corrected \
                    understanding and revisit when onboarding is in-season. \
                    I recommend B. Which way do you want to go?";
    let punt = pending_text_question_from_result_text(text)
        .expect("'which way do you want to go' fork must classify (BUG-462)");
    assert!(punt.detail.contains("Which way do you want to go?"));
    assert!(punt.lean.contains("recommend"));
}

#[test]
fn fork_question_after_a_rhetorical_aside_classifies() {
    // BUG-462: scan ALL question sentences, not just the first `?`. Here the
    // first question is a rhetorical aside; the operative fork question comes
    // later and must still be found.
    let text = "Does that make sense? There are two paths. Should we split this \
                    into two passes or do it in one? I lean toward one pass.";
    let punt = pending_text_question_from_result_text(text)
        .expect("buried fork question must classify (BUG-462)");
    assert!(punt.detail.contains("Should we split this"));
}

#[test]
fn novel_question_with_letter_options_block_classifies() {
    // BUG-462: options-block fallback — even when the question dodges every
    // phrase marker, a letter-labelled choice block makes the fork explicit.
    let text =
        "Two ways to model this.\nA) inline the helper\nB) extract a module\n\nHow do we land it?";
    let punt = pending_text_question_from_result_text(text)
        .expect("letter-option block + question must classify (BUG-462)");
    assert!(punt.detail.contains("How do we land it?"));
}

#[test]
fn numbered_summary_with_rhetorical_question_does_not_classify() {
    // BUG-462 false-positive guard: a numbered ACCOMPLISHMENTS list (steps,
    // not choices) with a trailing self-answered question must NOT punt.
    // Digit labels are excluded from the options-block fallback for exactly
    // this reason.
    let text =
        "Done. Implemented:\n1. parser fix\n2. test added\nDoes everything pass? Yes, all green.";
    assert!(pending_text_question_from_result_text(text).is_none());
}

#[test]
fn all_question_sentences_splits_on_terminators() {
    let qs = all_question_sentences("First. Is this one? Then more! And another?");
    assert_eq!(qs, vec!["Is this one?", "And another?"]);
}
