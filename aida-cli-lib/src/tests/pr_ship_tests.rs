use super::*;

#[test]
fn ship_wait_cli_accepts_optional_seconds() {
    use clap::Parser;
    for (flags, expected) in [
        (vec![], None),
        (vec!["--wait"], Some(300)),
        (vec!["--wait", "0"], Some(0)),
        (vec!["--wait=12"], Some(12)),
    ] {
        let mut args = vec!["aida", "pr", "ship", "1602"];
        args.extend(flags);
        let cli = crate::command_registry::try_parse_from(args).unwrap();
        match cli.command.registered::<crate::pr_cmd::PrArgs>() {
            crate::pr_cmd::PrArgs {
                inner: crate::cli::PrCommand::Ship { wait, .. },
            } => {
                assert_eq!(wait, expected)
            }
            _ => panic!("expected pr ship"),
        }
    }
    assert!(
        crate::command_registry::try_parse_from(["aida", "pr", "ship", "--wait", "bad"]).is_err()
    );
}

#[test]
fn parse_pr_number_from_create_output_canonical() {
    let out = "https://github.com/joemooney/aida/pull/458\n";
    assert_eq!(parse_pr_number_from_create_output(out), Some(458));
}

/// BUG-434: the `--delete-branch` guard truth table. Delete only when
/// nothing protects the branch; `force` overrides every guard.
#[test]
fn should_delete_branch_truth_table() {
    // Clean: no sibling, no children → delete.
    assert!(should_delete_branch(false, 0, 0, false));
    // Sibling worktree (BUG-289) → keep.
    assert!(!should_delete_branch(true, 0, 0, false));
    // Stacked child branch (stacks.json) → keep.
    assert!(!should_delete_branch(false, 1, 0, false));
    // Open child PR (gh) → keep.
    assert!(!should_delete_branch(false, 0, 2, false));
    // Both stacked signals → keep.
    assert!(!should_delete_branch(false, 3, 2, false));
    // force overrides sibling + children → delete (deliberate orphan).
    assert!(should_delete_branch(true, 5, 5, true));
    assert!(should_delete_branch(false, 1, 1, true));
}

#[test]
fn explicit_plain_pr_based_on_main_does_not_count_itself_as_child() {
    let target = crate::forge::ChangeRef {
        id: 1489,
        url: String::new(),
        branch: "task-1155-work".to_string(),
        base: "main".to_string(),
        title: None,
    };

    let (branch, retarget_base, child_prs) = ship_branch_context("main", Some(&target), &[1489]);

    assert_eq!(branch, "task-1155-work");
    assert_eq!(retarget_base, "main");
    assert!(child_prs.is_empty());
    assert!(should_delete_branch(false, 0, child_prs.len(), false));
}

// BUG-1953: step 5 looks the lease up on `ship_branch`, so it must fall
// back to the cwd branch only when no PR head branch resolved.
// trace:BUG-1953 | ai:claude
#[test]
fn bug_1953_ship_branch_falls_back_to_cwd_branch_only_without_a_pr_branch() {
    let (branch, _, _) = ship_branch_context("task-9-work", None, &[]);
    assert_eq!(branch, "task-9-work", "no PR resolved: use the cwd branch");

    let empty_head = crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: String::new(),
        base: "main".to_string(),
        title: None,
    };
    let (branch, _, _) = ship_branch_context("task-9-work", Some(&empty_head), &[]);
    assert_eq!(
        branch, "task-9-work",
        "PR without a head branch: use the cwd branch"
    );

    let head = crate::forge::ChangeRef {
        branch: "task-123-work".to_string(),
        ..empty_head
    };
    let (branch, _, _) = ship_branch_context("main", Some(&head), &[]);
    assert_eq!(
        branch, "task-123-work",
        "PR head branch wins over the cwd branch"
    );
}

#[test]
fn explicit_stacked_child_pr_still_protects_source_branch() {
    let target = crate::forge::ChangeRef {
        id: 12,
        url: String::new(),
        branch: "parent-work".to_string(),
        base: "main".to_string(),
        title: None,
    };

    let (branch, retarget_base, child_prs) = ship_branch_context("main", Some(&target), &[12, 13]);

    assert_eq!(branch, "parent-work");
    assert_eq!(retarget_base, "main");
    assert_eq!(child_prs, vec![13]);
    assert!(!should_delete_branch(false, 0, child_prs.len(), false));
}

#[test]
fn merge_error_landed_despite_failure_only_when_reprobe_confirms_merged() {
    // BUG-732: a non-zero `gh pr merge` can mean the remote squash merge
    // landed but a post-merge cleanup step failed. Only continue when a
    // fresh PR-state probe confirms MERGED; unknown remains a real failure.
    assert!(merge_error_landed_despite_failure(Some(true)));
    assert!(!merge_error_landed_despite_failure(Some(false)));
    assert!(!merge_error_landed_despite_failure(None));
}

#[test]
fn parse_pr_number_from_create_output_with_preamble() {
    // `gh pr create` sometimes prints "Creating pull request..." lines first.
    let out = "Creating pull request for task-458 into main in joemooney/aida\n\
               \n\
               https://github.com/joemooney/aida/pull/458\n";
    assert_eq!(parse_pr_number_from_create_output(out), Some(458));
}

#[test]
fn parse_pr_number_from_create_output_trailing_whitespace() {
    let out = "https://github.com/joemooney/aida/pull/123   \n";
    assert_eq!(parse_pr_number_from_create_output(out), Some(123));
}

#[test]
fn parse_pr_number_from_create_output_query_string() {
    // Defensive: gh occasionally appends a query string in some flows.
    let out = "https://github.com/joemooney/aida/pull/77?foo=bar\n";
    assert_eq!(parse_pr_number_from_create_output(out), Some(77));
}

#[test]
fn parse_pr_number_from_create_output_missing() {
    let out = "no url here\n";
    assert_eq!(parse_pr_number_from_create_output(out), None);
}

#[test]
fn parse_pr_number_from_create_output_empty() {
    assert_eq!(parse_pr_number_from_create_output(""), None);
}

#[test]
fn derive_pr_title_takes_first_nonempty_line() {
    let msg = "feat(pr): add aida pr ship (TASK-458)\n\nLong body here.\n";
    assert_eq!(
        derive_pr_title_from_commit(msg),
        "feat(pr): add aida pr ship (TASK-458)"
    );
}

#[test]
fn derive_pr_title_skips_leading_blank_lines() {
    let msg = "\n\n  subject only after blanks  \nbody\n";
    assert_eq!(
        derive_pr_title_from_commit(msg),
        "subject only after blanks"
    );
}

#[test]
fn derive_pr_body_drops_subject_and_separator() {
    let msg = "subject\n\nfirst body line\nsecond body line\n";
    assert_eq!(
        derive_pr_body_from_commit(msg),
        "first body line\nsecond body line"
    );
}

#[test]
fn derive_pr_body_empty_when_no_body() {
    assert_eq!(derive_pr_body_from_commit("subject only\n"), "");
}

#[test]
fn derive_pr_body_preserves_internal_blank_lines() {
    let msg = "subj\n\npara1\n\npara2\n";
    assert_eq!(derive_pr_body_from_commit(msg), "para1\n\npara2");
}

#[test]
fn extracts_spec_ids_from_pr_title() {
    let title = "feat(store): add cadence (STORY-284)";
    assert_eq!(
        extract_spec_ids_from_text(title),
        vec!["STORY-284".to_string()]
    );
}

#[test]
fn derives_spec_id_from_branch_when_title_has_none() {
    assert_eq!(
        derive_squash_subject_spec_ids("feat(store): add cadence", "task-310", ""),
        vec!["TASK-310".to_string()]
    );
    assert_eq!(
        derive_squash_subject_spec_ids("fix(queue): preserve id", "task-1-127", ""),
        vec!["TASK-1-127".to_string()]
    );
}

#[test]
fn derive_spec_ids_returns_none_when_metadata_has_none() {
    assert!(derive_squash_subject_spec_ids(
        "feat(store): add cadence",
        "feature/store-cadence",
        "no requirement id here",
    )
    .is_empty());
}

#[test]
fn extracts_multiple_spec_ids_in_order() {
    let title = "fix(queue): preserve credits (SPEC-1, SPEC-2)";
    assert_eq!(
        extract_spec_ids_from_text(title),
        vec!["SPEC-1".to_string(), "SPEC-2".to_string()]
    );
}

#[test]
fn squash_subject_appends_missing_spec_id() {
    assert_eq!(
        squash_subject_with_spec_ids(
            "[AI:codex] feat(store): add cadence",
            &["STORY-284".to_string()]
        ),
        "[AI:codex] feat(store): add cadence (STORY-284)"
    );
}

#[test]
fn squash_subject_does_not_duplicate_existing_spec_id() {
    assert_eq!(
        squash_subject_with_spec_ids(
            "[AI:antigravity] fix(mcp): normalize ids (BUG-332)",
            &["BUG-332".to_string()]
        ),
        "[AI:antigravity] fix(mcp): normalize ids (BUG-332)"
    );
}

#[test]
fn trailing_spec_ids_match_auto_bump_shape() {
    assert_eq!(
        extract_trailing_spec_ids_from_subject(
            "[AI:codex] fix(pr-ship): preserve subject (SPEC-410) (#204)"
        ),
        vec!["SPEC-410".to_string()]
    );
    assert!(extract_trailing_spec_ids_from_subject(
        "docs(competitive): implement maintained surface for STORY-260 (#203)"
    )
    .is_empty());
}

#[test]
fn squash_subject_repairs_mid_text_spec_id_before_pr_suffix() {
    let subject =
        "docs(competitive): implement maintained competitive analysis surface for STORY-260 (#203)";
    assert_eq!(
        squash_subject_with_spec_ids(subject, &["STORY-260".to_string()]),
        "docs(competitive): implement maintained competitive analysis surface for STORY-260 (STORY-260) (#203)"
    );
}

#[test]
fn squash_subject_repairs_missing_codex_subject_before_pr_suffix() {
    let subject = "[AI:codex] feat(store): add configurable auto-push cadence (#201)";
    assert_eq!(
        squash_subject_with_spec_ids(subject, &["STORY-284".to_string()]),
        "[AI:codex] feat(store): add configurable auto-push cadence (STORY-284) (#201)"
    );
}

#[test]
fn squash_subject_prefers_pr_title_over_branch_head_merge_commit() {
    let pr_title = "[AI:codex] feat(autopilot): add drift guard (TASK-1020)";
    let branch_head =
        "Merge main into task-1020 (bring current + pick up drift-guard fix) (TASK-1020)";
    assert_eq!(
        derive_squash_subject(pr_title, "task-1020", "", branch_head),
        Some(pr_title.to_string())
    );
}

#[test]
fn squash_subject_falls_back_to_branch_head_when_pr_title_empty() {
    let branch_head = "[AI:codex] fix(pr-ship): preserve branch subject";
    assert_eq!(
        derive_squash_subject("", "task-142", "", branch_head),
        Some("[AI:codex] fix(pr-ship): preserve branch subject (TASK-142)".to_string())
    );
}

#[test]
fn squash_subject_does_not_credit_branch_head_when_title_is_present() {
    let branch_head = "[AI:codex] fix(pr-ship): preserve subject (TASK-140)";
    assert_eq!(
        derive_squash_subject(
            "[AI:codex] fix(pr-ship): preserve subject",
            "feature/pr-ship-subject",
            "",
            branch_head,
        ),
        Some("[AI:codex] fix(pr-ship): preserve subject".to_string())
    );
}

// trace:TASK-1600 | ai:codex
#[test]
fn task_1600_incident_titles_and_bodies_do_not_grant_completion() {
    let title = "[AI:codex] docs: BUG-1802 prose layer — clarify lifecycle";
    assert_eq!(
        derive_squash_subject_resolving(
            title,
            "prose-layer",
            "BUG-1802",
            "fix: old (BUG-1802)",
            |_| true
        ),
        Some(title.into())
    );
    let title = "[AI:codex] refactor: pure move, 57 lines";
    assert_eq!(
        derive_squash_subject_resolving(
            title,
            "module-one",
            "SLICE-1 STORY-1488 TASK-1538",
            "old (TASK-1538)",
            |_| true
        ),
        Some(title.into())
    );
    assert!(
        derive_squash_subject_spec_ids("fix: BUG-1802 prose", "prose-layer", "TASK-1599")
            .is_empty()
    );
}

// trace:TASK-1600 | ai:codex
#[test]
fn task_1600_only_store_resolving_trailers_and_branch_recovery_survive() {
    let resolves = |id: &str| matches!(id, "TASK-1599" | "TASK-1600");
    assert_eq!(
        derive_squash_subject_resolving(
            "docs: BUG-1802 prose layer (SLICE-1 TASK-1599) (#2426)",
            "task-1600",
            "BUG-1802",
            "",
            resolves
        ),
        Some("docs: BUG-1802 prose layer (TASK-1599) (#2426)".into())
    );
    assert_eq!(
        derive_squash_subject_resolving(
            "fix: prose (SLICE-1)",
            "neutral",
            "TASK-1599",
            "",
            resolves
        ),
        Some("fix: prose".into())
    );
    assert_eq!(
        derive_squash_subject_resolving("fix: prose", "task-1600", "TASK-1599", "", resolves),
        Some("fix: prose (TASK-1600)".into())
    );
}

/// BUG-1919 regression: the #2477 shape — a slice-convention title with
/// no id, branch `task-1538-x`, and a non-completing marker in the body —
/// must land with NO completing trailer from any source.
// trace:BUG-1919 | ai:claude
#[test]
fn bug_1919_regression_marker_suppresses_branch_fallback() {
    let title = "[AI:antigravity] refactor(cli-lib): extract output_format.rs";
    let body = "Pure move of output_format.rs.\n\nNon-completing slice\n\nTraces TASK-1538.";
    let (subject, trailer) = derive_squash_subject_with_trailer(
        title,
        "task-1538-x",
        body,
        "old head (TASK-1538)",
        false,
        |_| true,
        |_| None,
    )
    .unwrap();
    assert_eq!(subject, title);
    assert!(trailer.ids.is_empty());
    assert_eq!(trailer.source, TrailerSource::None);
    assert!(
        trailer.notes[0].contains("PR-body marker"),
        "{:?}",
        trailer.notes
    );
    assert!(extract_trailing_spec_ids_from_subject(&subject).is_empty());
    // The public precedence helper (drain-side calibration) agrees.
    assert!(derive_squash_subject_spec_ids(title, "task-1538-x", body).is_empty());
    // Without the marker the branch fallback still fires (TASK-1600 shape).
    assert_eq!(
        derive_squash_subject_spec_ids(title, "task-1538-x", "Traces TASK-1538."),
        vec!["TASK-1538".to_string()]
    );
}

// trace:BUG-1919 | ai:claude
#[test]
fn bug_1919_no_complete_flag_suppresses_every_source() {
    let trailer = derive_squash_trailer(
        "fix: prose (TASK-1538)",
        "task-1538-x",
        "",
        true,
        |_| true,
        |_| None,
    );
    assert!(trailer.ids.is_empty());
    assert_eq!(trailer.source, TrailerSource::None);
    assert!(
        trailer.notes[0].contains("`--no-complete`"),
        "{:?}",
        trailer.notes
    );
    // trace:BUG-1957 | ai:claude
    assert_eq!(
        trailer.notes[0],
        "non-completing slice (`--no-complete`) — no spec is completed by this merge"
    );
    assert_eq!(
        derive_squash_subject_with_trailer(
            "fix: prose (TASK-1538)",
            "task-1538-x",
            "",
            "",
            true,
            |_| true,
            |_| None,
        )
        .map(|(subject, _)| subject),
        Some("fix: prose".into())
    );
}

// trace:BUG-1919 | ai:claude
#[test]
fn bug_1919_closure_pending_suppresses_branch_fallback() {
    let held = |id: &str| {
        (id == "TASK-1538").then(|| "the spec carries the `closure:pending` tag".to_string())
    };
    let trailer = derive_squash_trailer("fix: prose", "task-1538-x", "", false, |_| true, held);
    assert!(trailer.ids.is_empty());
    assert_eq!(trailer.source, TrailerSource::None);
    assert_eq!(trailer.notes.len(), 1);
    assert!(
        trailer.notes[0].contains("TASK-1538"),
        "{:?}",
        trailer.notes
    );
    assert!(
        trailer.notes[0].contains("closure:pending"),
        "{:?}",
        trailer.notes
    );
    assert_eq!(
        derive_squash_subject_with_trailer(
            "fix: prose",
            "task-1538-x",
            "",
            "",
            false,
            |_| true,
            held
        )
        .map(|(subject, _)| subject),
        Some("fix: prose".into())
    );
    // A held id never blocks an unheld sibling on the same branch name.
    // (`/`-separated: the SPEC-410 extractor reads a hyphen-joined pair as
    // one id, so a hyphen here would not exercise two ids at all.)
    let trailer = derive_squash_trailer(
        "fix: prose",
        "task-1538/task-1601",
        "",
        false,
        |_| true,
        held,
    );
    assert_eq!(trailer.ids, vec!["TASK-1601".to_string()]);
    assert_eq!(trailer.source, TrailerSource::Branch);
    assert_eq!(trailer.notes.len(), 1);
}

// trace:BUG-1919 | ai:claude
#[test]
fn bug_1919_title_trailer_still_honoured() {
    let held = |_: &str| Some("the spec carries the `closure:pending` tag".to_string());
    // An explicit title trailer is a deliberate directive even when the
    // spec is closure-held (the merge-time hold keeps it at Done).
    let trailer = derive_squash_trailer(
        "fix: prose (TASK-1538)",
        "neutral-branch",
        "",
        false,
        |_| true,
        held,
    );
    assert_eq!(trailer.ids, vec!["TASK-1538".to_string()]);
    assert_eq!(trailer.source, TrailerSource::Title);
    assert!(trailer.notes.is_empty());
    // ... and the plain branch recovery still works when nothing holds it.
    let trailer = derive_squash_trailer("fix: prose", "task-1538-x", "", false, |_| true, |_| None);
    assert_eq!(trailer.ids, vec!["TASK-1538".to_string()]);
    assert_eq!(trailer.source, TrailerSource::Branch);
    // TASK-1600: an unresolvable title id still falls through to the branch.
    let trailer = derive_squash_trailer(
        "fix: prose (SLICE-1)",
        "task-1538-x",
        "",
        false,
        |id| id == "TASK-1538",
        |_| None,
    );
    assert_eq!(trailer.ids, vec!["TASK-1538".to_string()]);
    assert_eq!(trailer.source, TrailerSource::Branch);
}

// trace:BUG-2203 | ai:claude
#[test]
fn bug_2203_refs_groups_are_recognized() {
    let refs = extract_trailing_refs_spec_ids_from_subject;
    assert_eq!(refs("feat: x (Refs BUG-2169)"), vec!["BUG-2169"]);
    assert_eq!(
        refs("feat: x (refs: TASK-1, BUG-2) (#12)"),
        vec!["TASK-1", "BUG-2"]
    );
    assert_eq!(refs("feat: x (Ref STORY-3)"), vec!["STORY-3"]);
    assert!(refs("feat: x (Refs BUG-2169) (BUG-2169)").is_empty());
    assert!(refs("feat: x (BUG-2169)").is_empty());
    assert!(refs("refactor(cli): x (References BUG-1)").is_empty());
    assert_eq!(
        refs_spec_ids_in_text("feat: x (Refs BUG-2169) (BUG-2169)\n\n- y (Refs TASK-7)"),
        vec!["BUG-2169", "TASK-7"]
    );
    assert!(refs_spec_ids_in_text("refactor(cli): x (BUG-1) (#3)").is_empty());
}

// trace:BUG-2203 | ai:claude
#[test]
fn bug_2203_refs_title_never_gains_a_completion_trailer() {
    // The PR-2787 shape: a Refs title from a branch named for the spec.
    let t = derive_squash_trailer(
        "feat: slice 2 (Refs BUG-2169)",
        "claude/bug-2169",
        "",
        false,
        |_| true,
        |_| None,
    );
    assert!(t.ids.is_empty(), "{t:?}");
    assert_eq!(t.source, TrailerSource::None);
    assert!(t.notes[0].contains("(Refs BUG-2169)"), "{t:?}");
    // With a `(#N)` suffix too.
    let t = derive_squash_trailer(
        "feat: x (Refs TASK-1952) (#2790)",
        "claude/task-1952",
        "",
        false,
        |_| true,
        |_| None,
    );
    assert!(t.ids.is_empty(), "{t:?}");
    // A bare trailer repeating a Refs id is still a reference; the branch
    // fallback is withheld rather than guessed.
    let t = derive_squash_trailer(
        "feat: x (Refs BUG-2169) (BUG-2169)",
        "claude/bug-2169",
        "",
        false,
        |_| true,
        |_| None,
    );
    assert!(t.ids.is_empty(), "{t:?}");
    // A Refs id never rides in on the branch name either.
    let t = derive_squash_trailer(
        "feat: x (Refs BUG-2169) and more",
        "claude/bug-2169",
        "",
        false,
        |_| true,
        |_| None,
    );
    assert!(t.ids.is_empty(), "{t:?}");
    // A different completing id after a Refs group still completes.
    let t = derive_squash_trailer(
        "feat: x (Refs BUG-1) (BUG-2)",
        "neutral",
        "",
        false,
        |_| true,
        |_| None,
    );
    assert_eq!(t.ids, vec!["BUG-2".to_string()]);
    // The full subject derivation leaves the Refs title untouched.
    let (subject, _) = derive_squash_subject_with_trailer(
        "feat: slice 2 (Refs BUG-2169)",
        "claude/bug-2169",
        "",
        "",
        false,
        |_| true,
        |_| None,
    )
    .unwrap();
    assert_eq!(subject, "feat: slice 2 (Refs BUG-2169)");
    assert_eq!(
        squash_subject_with_spec_ids("feat: x (Refs BUG-1)", &["BUG-1".to_string()]),
        "feat: x (Refs BUG-1)"
    );
}

// trace:BUG-1919 | ai:claude
#[test]
fn bug_1919_provenance_names_source() {
    let title = derive_squash_trailer(
        "fix: x (TASK-1538)",
        "neutral",
        "",
        false,
        |_| true,
        |_| None,
    );
    assert_eq!(
        format_trailer_provenance(&title),
        "completion trailer (TASK-1538) — from the PR title; this merge will complete TASK-1538"
    );
    let branch = derive_squash_trailer("fix: x", "task-1538-x", "", false, |_| true, |_| None);
    assert_eq!(
        format_trailer_provenance(&branch),
        "completion trailer (TASK-1538) — from the branch name; this merge will complete TASK-1538"
    );
    let none = derive_squash_trailer("fix: x", "neutral", "", false, |_| true, |_| None);
    assert_eq!(
        format_trailer_provenance(&none),
        "completion trailer: none — this merge completes no spec"
    );
    let withheld = derive_squash_trailer(
        "fix: x",
        "task-1538-x",
        "",
        false,
        |_| true,
        |_| Some("1 of 45 slice child(ren) still open".to_string()),
    );
    let text = format_trailer_provenance(&withheld);
    assert!(text.starts_with("completion trailer: none"), "{text}");
    assert!(text.contains("\n    branch-name fallback withheld for TASK-1538: 1 of 45 slice child(ren) still open"), "{text}");
    // BUG-1957: the note names the effect, not an unchecked PR-body trace.
    assert!(
        text.contains("still open — not completed by this merge;"),
        "{text}"
    );
    assert!(!text.contains("PR body"), "{text}");
}

// trace:BUG-1919 | ai:claude
#[test]
fn bug_1919_body_marker_detection() {
    for body in [
        "aida: no-complete",
        "AIDA: No-Complete",
        "- aida: no-complete",
        "**Non-completing slice**",
        "Non-completing slice.",
        "Non-completing slice (3 of 45): pure move",
        "intro\n\n> aida: no-complete — slice 1\n\nmore",
        // BUG-1957: whitespace, a dash, clause punctuation, or end of
        // line after the marker, with closing emphasis skipped first.
        "aida: no-complete\tslice 2",
        "aida: no-complete—slice 3",
        "**aida: no-complete** — slice 1 of 3",
        "**Non-completing slice**: pure move",
        "Non-completing slice: pure move",
        "_Non-completing slice_ (3 of 45)",
        "aida: no-complete, slice 2",
        "aida: no-complete(slice 2)",
        "aida: no-complete–slice",
        "aida: no-complete; slice 4",
        // Round 2: sentence punctuation after the marker still declares.
        "aida: no-complete. This is slice 2 of 3.",
        "Non-completing slice. Pure move.",
        "aida: no-complete! slice 2",
        "aida: no-complete) x",
        "aida: no-complete… slice",
        "aida: no-complete?",
        // A `.` followed by closing emphasis or more punctuation still
        // declares; only a `.` running into a word is a mention.
        "**Non-completing slice.** Pure move.",
        "**aida: no-complete.** Slice 2 of 3",
        "_Non-completing slice._ Pure move",
        "aida: no-complete... slice 2",
        "aida: no-complete.) x",
        "aida: no-complete.—slice",
    ] {
        assert!(pr_body_declares_non_completing(body), "{body:?}");
    }
    for body in [
        "",
        "This is not a non-completing slice of anything.",
        "Non-completing slices are documented in OVERVIEW.md",
        "see aida: no-complete in the docs",
        "aida: no-completeness",
        // BUG-1957: a backticked changelog mention is descriptive, not a
        // declaration, even after the backticks are trimmed.
        "- `aida: no-complete` marker is now recognised",
        "- `Non-completing slice` bodies now withhold the trailer",
        "aida: no-complete/slice",
        "aida: no-complete-ish",
        "**aida: no-complete**ness",
        "aida: no-complete.md documents the marker",
        "aida: no-complete.rs is the parser",
        "aida: no-complete_flag",
    ] {
        assert!(!pr_body_declares_non_completing(body), "{body:?}");
    }
}

fn bug_1919_req(spec_id: &str, status: aida_core::RequirementStatus) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("title {spec_id}"), String::new());
    r.spec_id = Some(spec_id.to_string());
    r.req_type = aida_core::RequirementType::Task;
    r.status = status;
    r
}

fn bug_1919_link(parent: &mut aida_core::Requirement, child: &mut aida_core::Requirement) {
    let rel = |rel_type, target_id| aida_core::Relationship {
        rel_type,
        target_id,
        created_at: None,
        created_by: None,
    };
    let (parent_id, child_id) = (parent.id, child.id);
    parent
        .relationships
        .push(rel(aida_core::RelationshipType::Parent, child_id));
    child
        .relationships
        .push(rel(aida_core::RelationshipType::Child, parent_id));
}

// trace:BUG-1919 | ai:claude
#[test]
fn bug_1919_hold_reason_reads_tag_and_open_children() {
    use aida_core::RequirementStatus as S;
    let mut parent = bug_1919_req("TASK-1538", S::InProgress);
    let mut open_child = bug_1919_req("TASK-1601", S::Approved);
    let mut done_child = bug_1919_req("TASK-1602", S::Completed);
    bug_1919_link(&mut parent, &mut open_child);
    bug_1919_link(&mut parent, &mut done_child);
    let store = aida_core::RequirementsStore {
        requirements: vec![parent, open_child, done_child],
        ..Default::default()
    };
    let reason = spec_completion_hold_reason(&store, "TASK-1538").expect("open child holds");
    assert!(
        reason.contains("1 of 2 slice child(ren) still open"),
        "{reason}"
    );
    // The children themselves are leaves: nothing holds them.
    assert_eq!(spec_completion_hold_reason(&store, "TASK-1601"), None);
    // An unknown id is never held (resolution is the caller's filter).
    assert_eq!(spec_completion_hold_reason(&store, "TASK-9999"), None);

    // Every child completed ⇒ no hold.
    let mut parent = bug_1919_req("TASK-1538", S::InProgress);
    let mut done_child = bug_1919_req("TASK-1602", S::Completed);
    bug_1919_link(&mut parent, &mut done_child);
    let store = aida_core::RequirementsStore {
        requirements: vec![parent, done_child],
        ..Default::default()
    };
    assert_eq!(spec_completion_hold_reason(&store, "TASK-1538"), None);

    // The closure:pending tag holds a spec with no children at all.
    let mut tagged = bug_1919_req("TASK-1538", S::InProgress);
    tagged
        .tags
        .insert(aida_core::pickability::CLOSURE_PENDING_TAG.to_string());
    let store = aida_core::RequirementsStore {
        requirements: vec![tagged],
        ..Default::default()
    };
    let reason = spec_completion_hold_reason(&store, "TASK-1538").expect("tag holds");
    assert!(reason.contains("closure:pending"), "{reason}");
}

// trace:BUG-1957 | ai:claude
#[test]
fn bug_1957_done_child_does_not_hold_parent() {
    use aida_core::RequirementStatus as S;
    let mut parent = bug_1919_req("TASK-1538", S::InProgress);
    let mut done_child = bug_1919_req("TASK-1602", S::Done);
    let mut completed_child = bug_1919_req("TASK-1603", S::Completed);
    bug_1919_link(&mut parent, &mut done_child);
    bug_1919_link(&mut parent, &mut completed_child);
    let store = aida_core::RequirementsStore {
        requirements: vec![parent, done_child, completed_child],
        ..Default::default()
    };
    assert_eq!(spec_completion_hold_reason(&store, "TASK-1538"), None);
}

// trace:BUG-1957 | ai:claude
#[test]
fn bug_1957_held_ids_skip_closure_pending_branch_fallback() {
    use aida_core::RequirementStatus as S;
    let mut tagged = bug_1919_req("TASK-1538", S::InProgress);
    tagged
        .tags
        .insert(aida_core::pickability::CLOSURE_PENDING_TAG.to_string());
    let store = aida_core::RequirementsStore {
        requirements: vec![tagged, bug_1919_req("TASK-1601", S::InProgress)],
        ..Default::default()
    };
    let held = |id: &str| spec_completion_hold_reason(&store, id);
    assert!(derive_squash_subject_spec_ids_held("fix: prose", "task-1538-x", "", held).is_empty());
    assert_eq!(
        derive_squash_subject_spec_ids_held("fix: prose", "task-1601-x", "", held),
        vec!["TASK-1601".to_string()]
    );
    // Without the hold check the branch name would have credited it.
    assert_eq!(
        derive_squash_subject_spec_ids("fix: prose", "task-1538-x", ""),
        vec!["TASK-1538".to_string()]
    );
}

// trace:BUG-1957 | ai:claude
#[test]
fn bug_1957_dry_run_plan_carries_trailer_provenance() {
    let trailer = derive_squash_trailer(
        "fix: x (TASK-1538)",
        "neutral",
        "",
        false,
        |_| true,
        |_| None,
    );
    assert_eq!(
        format_dry_run_trailer_provenance(Ok(&trailer)),
        "  · completion trailer (TASK-1538) — from the PR title; this merge will complete TASK-1538\n"
    );
    let withheld = derive_squash_trailer("fix: x", "task-1538-x", "", true, |_| true, |_| None);
    let line = format_dry_run_trailer_provenance(Ok(&withheld));
    assert!(
        line.starts_with("  · completion trailer: none — this merge completes no spec"),
        "{line}"
    );
    assert!(line.contains("`--no-complete`"), "{line}");
    assert!(line.ends_with('\n'), "{line:?}");
    assert_eq!(
        format_dry_run_trailer_provenance(Err("no PR yet")),
        "  · completion trailer: not resolved — no PR yet\n"
    );
}

// trace:BUG-1957 | ai:claude
#[test]
fn bug_1957_ship_cli_parses_no_complete() {
    use clap::Parser;
    for (flags, expected) in [(vec![], false), (vec!["--no-complete"], true)] {
        let mut args = vec!["aida", "pr", "ship", "1957"];
        args.extend(flags);
        let cli = crate::command_registry::try_parse_from(args).unwrap();
        match cli.command.registered::<crate::pr_cmd::PrArgs>() {
            crate::pr_cmd::PrArgs {
                inner: crate::cli::PrCommand::Ship { no_complete, .. },
            } => {
                assert_eq!(no_complete, expected)
            }
            _ => panic!("expected pr ship"),
        }
    }
}

#[test]
fn merge_args_include_repaired_subject() {
    let args = merge_args(
        201,
        false,
        Some("[AI:codex] feat(store): add cadence (STORY-284)"),
        None,
    );
    assert_eq!(
        args,
        vec![
            "pr",
            "merge",
            "201",
            "--squash",
            "--subject",
            "[AI:codex] feat(store): add cadence (STORY-284)"
        ]
    );
}

#[test]
fn merge_args_without_repair_keep_existing_shape() {
    let args = merge_args(197, true, None, None);
    assert_eq!(
        args,
        vec!["pr", "merge", "197", "--squash", "--delete-branch"]
    );
}

// TASK-1330: an identity-sanitized squash body rides along as --body,
// which also suppresses GitHub's auto-derived Co-authored-by trailers.
// trace:TASK-1330 | ai:claude
#[test]
fn merge_args_pass_sanitized_squash_body() {
    let args = merge_args(202, false, None, Some("* feat: one (TASK-1)\n\nBody."));
    assert_eq!(
        args,
        vec![
            "pr",
            "merge",
            "202",
            "--squash",
            "--body",
            "* feat: one (TASK-1)\n\nBody."
        ]
    );
}

#[test]
fn gh_checks_empty_output_means_not_registered_yet() {
    assert!(!gh_pr_checks_output_has_registered_checks("", ""));
}

#[test]
fn gh_checks_no_checks_message_means_not_registered_yet() {
    let stderr = "no checks reported on the 'bug-344' branch";
    assert!(!gh_pr_checks_output_has_registered_checks("", stderr));
    assert!(gh_pr_checks_output_is_unregistered("", stderr));
}

#[test]
fn gh_checks_pending_line_means_registered() {
    let stdout = "build\tpending\t0\thttps://github.com/joemooney/aida/actions/runs/1\n";
    assert!(gh_pr_checks_output_has_registered_checks(stdout, ""));
    assert!(!gh_pr_checks_output_is_unregistered(stdout, ""));
}

#[test]
fn gh_checks_failed_line_still_means_registered() {
    let stdout = "test\tfail\t1\thttps://github.com/joemooney/aida/actions/runs/2\n";
    assert!(gh_pr_checks_output_has_registered_checks(stdout, ""));
}

// BUG-417: PR base resolves from origin's default branch, not hardcoded main.
#[test]
fn parse_gh_default_branch_reads_bare_name() {
    assert_eq!(
        parse_gh_default_branch("master\n").as_deref(),
        Some("master")
    );
    assert_eq!(parse_gh_default_branch("main").as_deref(), Some("main"));
    assert_eq!(
        parse_gh_default_branch("  develop  \n").as_deref(),
        Some("develop")
    );
}

#[test]
fn parse_gh_default_branch_none_on_empty() {
    assert_eq!(parse_gh_default_branch(""), None);
    assert_eq!(parse_gh_default_branch("   \n\n"), None);
}

#[test]
fn parse_gh_default_branch_takes_first_nonempty_line() {
    assert_eq!(
        parse_gh_default_branch("\n\nmaster\nextra").as_deref(),
        Some("master")
    );
}

// BUG-417: no .github/workflows YAML ⇒ no CI configured ⇒ skip CI-wait.
#[test]
fn workflow_files_indicate_ci_true_for_yaml() {
    assert!(workflow_files_indicate_ci(["ci.yml"]));
    assert!(workflow_files_indicate_ci(["release.yaml"]));
    assert!(workflow_files_indicate_ci(["README.md", "ci.yml"]));
    assert!(workflow_files_indicate_ci(["CI.YML"])); // case-insensitive
}

#[test]
fn workflow_files_indicate_ci_false_when_no_yaml() {
    let empty: [&str; 0] = [];
    assert!(!workflow_files_indicate_ci(empty));
    assert!(!workflow_files_indicate_ci(["README.md", "notes.txt"]));
    assert!(!workflow_files_indicate_ci([".gitkeep"]));
}

#[test]
fn dry_run_plan_full_sequence() {
    let opts = PrShipOptions {
        pr_number: None,
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: None,
        effort: None,
    };
    let steps = vec![
        ShipStep::ResolvePr {
            create_if_needed: true,
        },
        ShipStep::WatchCi,
        ShipStep::Merge {
            delete_branch: true,
        },
        ShipStep::Pull,
        ShipStep::EndLease,
    ];
    let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitHub);
    assert!(plan.contains("1. resolve PR"), "{plan}");
    assert!(plan.contains("2. gh pr checks"), "{plan}");
    assert!(plan.contains("3. gh pr merge"), "{plan}");
    assert!(plan.contains("4. aida pull"), "{plan}");
    assert!(plan.contains("5. aida session end"), "{plan}");
}

#[test]
fn dry_run_plan_explicit_pr_number() {
    let opts = PrShipOptions {
        pr_number: Some(182),
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: None,
        effort: None,
    };
    let steps = vec![ShipStep::ResolvePr {
        create_if_needed: false,
    }];
    let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitHub);
    assert!(plan.contains("PR-182"), "{plan}");
    assert!(plan.contains("explicit"), "{plan}");
}

#[test]
fn dry_run_plan_notes_skipped_flags() {
    let opts = PrShipOptions {
        pr_number: None,
        no_pull: true,
        no_cleanup: true,
        dry_run: true,
        complexity: None,
        effort: None,
    };
    let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
    assert!(plan.contains("--no-pull"), "{plan}");
    assert!(plan.contains("--no-cleanup"), "{plan}");
}

#[test]
fn dry_run_plan_worktree_aware_merge() {
    let opts = PrShipOptions {
        pr_number: Some(1),
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: None,
        effort: None,
    };
    let steps = vec![ShipStep::Merge {
        delete_branch: false,
    }];
    let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitHub);
    assert!(plan.contains("skip --delete-branch"), "{plan}");
    assert!(plan.contains("sibling worktree"), "{plan}");
}

// STORY-508/TASK-651: the dry-run plan is forge-aware — a GitLab project
// sees glab/MR commands and the glab branch-delete flag, never gh.
#[test]
fn dry_run_plan_is_forge_aware_for_gitlab() {
    let opts = PrShipOptions {
        pr_number: None,
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: None,
        effort: None,
    };
    let steps = vec![
        ShipStep::ResolvePr {
            create_if_needed: true,
        },
        ShipStep::WatchCi,
        ShipStep::Merge {
            delete_branch: true,
        },
        ShipStep::Merge {
            delete_branch: false,
        },
    ];
    let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitLab);
    assert!(plan.contains("resolve MR for current branch"), "{plan}");
    assert!(plan.contains("glab ci status"), "{plan}");
    assert!(
        plan.contains("glab mr merge <N> --squash --remove-source-branch"),
        "{plan}"
    );
    assert!(plan.contains("skip --remove-source-branch"), "{plan}");
    assert!(!plan.contains("gh pr"), "{plan}");
}

// TASK-1582 acceptance 2: the dry-run merge step is the literal argv the
// real merge runs (github_merge_argv over merge_args), pin included.
// trace:TASK-1582.acd089f0 | ai:antigravity
#[test]
fn task_1582_dry_run_merge_step_matches_real_merge_argv_with_pin() {
    let opts = PrShipOptions {
        pr_number: None,
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: None,
        effort: None,
    };
    for delete_branch in [true, false] {
        let real = crate::forge::github_merge_argv(
            4242,
            &crate::forge::MergeOptions {
                method: crate::forge::MergeMethod::Squash,
                squash_subject: None,
                squash_body: None, // trace:TASK-1330 | ai:claude
                delete_branch,
                match_head: Some(DRY_RUN_PIN_PLACEHOLDER.to_string()),
            },
        );
        // Same shape as merge_args, then the pin.
        assert_eq!(
            &real[..real.len() - 2],
            merge_args(4242, delete_branch, None, None).as_slice()
        );
        let expected = format!("3. gh {}", real.join(" ").replace("4242", "<N>"));
        let steps = vec![
            ShipStep::ResolvePr {
                create_if_needed: true,
            },
            ShipStep::WatchCi,
            ShipStep::Merge { delete_branch },
        ];
        let plan = format_dry_run_plan(&opts, &steps, crate::forge::ForgeKind::GitHub);
        assert!(plan.contains(&expected), "missing `{expected}` in:\n{plan}");
        assert!(
            plan.contains("--match-head-commit <approved-head-sha>"),
            "{plan}"
        );
        assert!(
            plan.contains("· <approved-head-sha>: the head the merge is pinned to")
                && plan.contains("the pin is omitted"),
            "{plan}"
        );
    }
}

// trace:TASK-1582.acd089f0 | ai:antigravity
#[test]
fn task_1582_gitlab_dry_run_merge_step_states_abbreviation_and_pin() {
    let line = dry_run_merge_cmd(crate::forge::ForgeKind::GitLab, true).unwrap();
    assert!(
        line.starts_with("glab mr merge <N> --squash --remove-source-branch"),
        "{line}"
    );
    assert!(line.contains("abbreviated"), "{line}");
    assert!(
        line.contains("checked against <approved-head-sha>"),
        "{line}"
    );
    assert_eq!(dry_run_merge_cmd(crate::forge::ForgeKind::None, true), None);
}

// TASK-1582 acceptance 3: step 5 names scope, role and worktree path.
// trace:TASK-1582.ac6f4c6f | ai:antigravity
#[test]
fn task_1582_step5_messages_name_scope_role_and_worktree() {
    let wt = std::path::Path::new("/dummy/aida-worktrees/aida-task-9");
    let (line, detail) =
        step5_inside_worktree_messages("01abc", "TASK-9", Some("integrator"), wt, "task-9");
    for text in [&line, &detail] {
        assert!(text.contains("lease 01abc"), "{text}");
        assert!(text.contains("scope TASK-9"), "{text}");
        assert!(text.contains("role integrator"), "{text}");
        assert!(
            text.contains("worktree /dummy/aida-worktrees/aida-task-9"),
            "{text}"
        );
        assert!(text.contains("`aida session end 01abc`"), "{text}");
    }
    assert!(line.contains("your own active session"), "{line}");
    // Legacy leases without a role still say so explicitly.
    let ending = describe_lease_for_step5("01abc", "TASK-9", None, wt);
    assert_eq!(
        ending,
        "lease 01abc (scope TASK-9, role unrecorded, worktree /dummy/aida-worktrees/aida-task-9)"
    );
}

// trace:BUG-1953.ac81b0b1 | ai:claude
#[test]
fn bug_1953_step5_no_lease_messages_name_the_ship_branch() {
    let (line, detail) = step5_no_lease_messages("task-123-work");
    assert_eq!(
        line,
        "  step 5: no AIDA lease for branch task-123-work — nothing to clean up"
    );
    assert_eq!(detail, "no lease for branch task-123-work");
}

#[test]
fn dry_run_plan_includes_complexity_capture_when_set() {
    let opts = PrShipOptions {
        pr_number: Some(7),
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: Some(crate::complexity_calibration::ComplexityLevel::High),
        effort: None,
    };
    let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
    assert!(plan.contains("--complexity high"), "{plan}");
    assert!(plan.contains(".aida/complexity-calibration/"), "{plan}");
}

#[test]
fn dry_run_plan_omits_complexity_line_when_absent() {
    let opts = PrShipOptions {
        pr_number: Some(7),
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: None,
        effort: None,
    };
    let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
    assert!(!plan.contains("complexity"), "{plan}");
}

#[test]
fn dry_run_plan_includes_effort_capture_when_set() {
    let opts = PrShipOptions {
        pr_number: Some(7),
        no_pull: false,
        no_cleanup: false,
        dry_run: true,
        complexity: None,
        effort: Some(crate::effort_calibration::EffortBucket::OneDay),
    };
    let plan = format_dry_run_plan(&opts, &[], crate::forge::ForgeKind::GitHub);
    assert!(plan.contains("--effort 1d"), "{plan}");
    assert!(plan.contains(".aida/effort-calibration/"), "{plan}");
}

#[test]
fn activity_event_includes_step_status_and_pr() {
    let ev = format_activity_event(
        "2026-05-22T18:30:00Z",
        Some(458),
        &ShipStep::Merge {
            delete_branch: true,
        },
        &StepOutcome::Ok,
    );
    // Must be valid JSON.
    let v: serde_json::Value = serde_json::from_str(&ev).unwrap();
    assert_eq!(v["command"], "aida pr ship");
    assert_eq!(v["step"], "pr-merge");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["pr"], 458);
    assert_eq!(v["ts"], "2026-05-22T18:30:00Z");
    assert!(v.get("detail").is_none(), "ok should not emit detail");
}

#[test]
fn activity_event_failed_carries_detail() {
    let ev = format_activity_event(
        "2026-05-22T18:30:00Z",
        Some(7),
        &ShipStep::WatchCi,
        &StepOutcome::Failed("CI red on build job".into()),
    );
    let v: serde_json::Value = serde_json::from_str(&ev).unwrap();
    assert_eq!(v["status"], "failed");
    assert_eq!(v["detail"], "CI red on build job");
}

#[test]
fn activity_event_skipped_carries_reason() {
    let ev = format_activity_event(
        "2026-05-22T18:30:00Z",
        None,
        &ShipStep::Pull,
        &StepOutcome::Skipped("--no-pull".into()),
    );
    let v: serde_json::Value = serde_json::from_str(&ev).unwrap();
    assert_eq!(v["status"], "skipped");
    assert_eq!(v["detail"], "--no-pull");
    assert!(v.get("pr").is_none());
}

#[test]
fn branch_pr_resolution_uses_existing_open_pr() {
    // TASK-141: no-arg `pr ship` resumes the branch PR when lookup finds it.
    let lookup = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
        id: 1420,
        url: "https://github.com/joemooney/aida/pull/1420".to_string(),
        branch: "task-1020".to_string(),
        base: "main".to_string(),
        title: Some("fix".to_string()),
    });
    assert_eq!(
        branch_pr_resolution_from_lookup(&lookup),
        BranchPrResolution::Found(1420)
    );
}

#[test]
fn branch_pr_resolution_creates_only_after_definitive_no_change() {
    // TASK-141: only a clean empty lookup means it is safe to create.
    assert_eq!(
        branch_pr_resolution_from_lookup(&crate::forge::ChangeLookup::NoChange),
        BranchPrResolution::Create
    );
}

#[test]
fn branch_pr_resolution_does_not_create_after_lookup_failure() {
    // TASK-141: auth/network/parser failures are inconclusive; falling
    // through to create hides the real resume problem behind gh's
    // "a pull request already exists" error.
    for lookup in [
        crate::forge::ChangeLookup::CliMissing,
        crate::forge::ChangeLookup::CliFailed("auth required".to_string()),
        crate::forge::ChangeLookup::Unreachable("connection refused".to_string()),
    ] {
        match branch_pr_resolution_from_lookup(&lookup) {
            BranchPrResolution::LookupFailed(msg) => {
                assert!(msg.contains("aida pr ship <N>"), "{msg}");
            }
            other => panic!("expected LookupFailed, got {other:?}"),
        }
    }
}

#[test]
fn recovery_hint_names_step_pr_and_action() {
    let h = recovery_hint(
        &ShipStep::Merge {
            delete_branch: true,
        },
        Some(458),
        crate::forge::ForgeKind::GitHub,
    );
    assert!(h.contains("458"), "{h}");
    assert!(h.contains("gh pr merge"), "{h}");
}

#[test]
fn recovery_hint_uses_gitlab_merge_vocabulary() {
    let h = recovery_hint(
        &ShipStep::Merge {
            delete_branch: true,
        },
        Some(458),
        crate::forge::ForgeKind::GitLab,
    );
    assert!(h.contains("458"), "{h}");
    assert!(h.contains("glab mr merge"), "{h}");
    assert!(!h.contains("gh pr"), "{h}");
}

#[test]
fn recovery_hint_pull_mentions_reconcile() {
    let h = recovery_hint(&ShipStep::Pull, Some(1), crate::forge::ForgeKind::GitHub);
    assert!(h.contains("aida db reconcile-status"), "{h}");
}

// --- Cluster PR (coupled single-branch drain) --------------------------
// trace:TASK-1136 | ai:claude

fn cluster_members() -> Vec<String> {
    vec![
        "TASK-1134".to_string(),
        "TASK-1135".to_string(),
        "TASK-1138".to_string(),
    ]
}

/// The ONE cluster PR's title names EVERY member, in drain order, in the
/// trailing spec-id group the completion scan reads.
#[test]
fn cluster_pr_title_names_every_member_in_drain_order() {
    let title = cluster_pr_title("tui-redesign", &cluster_members());
    assert!(
        title.ends_with("(TASK-1134 TASK-1135 TASK-1138)"),
        "{title}"
    );
    assert!(title.contains("3 members"), "{title}");
    assert_eq!(
        extract_trailing_spec_ids_from_subject(&title),
        cluster_members()
    );
}

/// One member is still a cluster of one — singular wording, same shape.
#[test]
fn cluster_pr_title_handles_single_member() {
    let title = cluster_pr_title("solo", &["BUG-777".to_string()]);
    assert!(title.contains("1 member on one branch"), "{title}");
    assert!(title.ends_with("(BUG-777)"), "{title}");
}

/// A halted drain that committed nothing yields a title with no trailing
/// group rather than an empty `()` the id parsers would choke on.
#[test]
fn cluster_pr_title_with_no_members_has_no_empty_group() {
    let title = cluster_pr_title("empty", &[]);
    assert!(!title.contains("()"), "{title}");
    assert!(extract_trailing_spec_ids_from_subject(&title).is_empty());
}

/// The body's `## Covers` list gives the reviewer the whole cluster and the
/// shared branch it landed on.
#[test]
fn cluster_pr_body_covers_every_member_and_names_the_branch() {
    let body = cluster_pr_body(
        "tui-redesign",
        "single-branch/tui-redesign-abc1234",
        &cluster_members(),
    );
    assert!(body.contains("## Covers"), "{body}");
    assert!(
        body.contains("single-branch/tui-redesign-abc1234"),
        "{body}"
    );
    for (i, id) in cluster_members().iter().enumerate() {
        assert!(
            body.contains(&format!("- member {} ({})", i + 1, id)),
            "{body}"
        );
    }
}

/// THE load-bearing round-trip: the cluster PR title → the squash subject
/// the merge writes onto the default branch → the `aida pull`
/// Done→Completed scan. Every member must come back out, not just the
/// first. This is what makes ONE cluster merge complete ALL N specs
/// through the existing single-spec auto-bump machinery.
#[test]
fn cluster_pr_title_round_trips_all_members_through_the_completion_scan() {
    let members = cluster_members();
    let branch = "single-branch/tui-redesign-abc1234";
    let title = cluster_pr_title("tui-redesign", &members);
    let subject = derive_squash_subject(&title, branch, "", "")
        .expect("cluster title yields a squash subject");
    // GitHub appends its `(#N)` suffix to the landed squash commit.
    let landed = format!("{subject} (#1590)");
    assert_eq!(crate::extract_spec_ids_from_commit(&landed), members);
}

/// Second belt: when the squash body carries the PR body, the `## Covers`
/// list credits the same member set through the referenced-id extractor
/// (the path a squash-merged umbrella PR's folded specs already ride).
#[test]
fn cluster_pr_body_covers_list_is_harvestable_by_the_referenced_id_scan() {
    let members = cluster_members();
    let title = cluster_pr_title("tui-redesign", &members);
    let body = cluster_pr_body("tui-redesign", "single-branch/x", &members);
    // A squash commit: subject line, blank line, then the body.
    let commit = format!("{title}\n\n{body}");
    let mut seen = crate::extract_spec_ids_from_commit(&commit);
    seen.extend(crate::extract_referenced_spec_ids_from_commit(&commit));
    for id in &members {
        assert!(seen.contains(id), "{id} missing from {seen:?}");
    }
}

/// A spec-id-shaped batch name (`epic-54`) must NOT be mined as a shipped
/// spec — otherwise the cluster merge would falsely complete it alongside
/// the real members. The scope is neutralized; only the members survive the
/// squash-subject id sweep.
#[test]
fn cluster_pr_title_does_not_leak_a_spec_id_shaped_batch_name() {
    let members = cluster_members();
    let title = cluster_pr_title("epic-54", &members);
    assert!(title.contains("feat(epic_54)"), "{title}");
    // The broad sweep the squash-subject repair runs sees ONLY the members.
    assert_eq!(extract_spec_ids_from_text(&title), members);
    let subject = derive_squash_subject(&title, "single-branch/epic-54-abc1234", "", "")
        .expect("cluster title yields a squash subject");
    assert_eq!(
        crate::extract_spec_ids_from_commit(&format!("{subject} (#1590)")),
        members
    );
}

/// A batch name that cannot parse as a spec id is left exactly as written.
#[test]
fn cluster_pr_title_keeps_an_ordinary_batch_name_verbatim() {
    let title = cluster_pr_title("tui-redesign", &cluster_members());
    assert!(title.contains("feat(tui-redesign)"), "{title}");
}

/// A cluster title must not lose members when the merge path re-derives the
/// subject from the branch name instead (branch names carry no ids), and
/// must not duplicate ids when it re-appends.
#[test]
fn cluster_squash_subject_is_idempotent() {
    let members = cluster_members();
    let title = cluster_pr_title("tui-redesign", &members);
    let once = squash_subject_with_spec_ids(&title, &members);
    let twice = squash_subject_with_spec_ids(&once, &members);
    assert_eq!(once, title);
    assert_eq!(twice, title);
}
