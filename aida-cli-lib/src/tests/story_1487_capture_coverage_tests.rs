//! Tests for STORY-1487: the capture-coverage report and the `gap` /
//! `coverage` target of `aida criteria`.
//
// trace:STORY-1487 | ai:claude

use super::*;
use aida_core::models::RequirementType;
use aida_core::{Requirement, RequirementsStore};
use std::path::Path;

fn spec(id: &str, req_type: RequirementType, description: &str, days_ago: i64) -> Requirement {
    let mut r = Requirement::new(format!("{id} title"), description.to_string());
    r.spec_id = Some(id.to_string());
    r.req_type = req_type;
    r.created_at = Utc::now() - chrono::Duration::days(days_ago);
    r
}

fn store_with(reqs: Vec<Requirement>) -> RequirementsStore {
    let mut store = RequirementsStore::new();
    store.requirements = reqs;
    store
}

fn tokens(criterion: &[&str], comments: usize) -> TraceTokens {
    TraceTokens {
        criterion_tokens: criterion.iter().map(|s| s.to_ascii_uppercase()).collect(),
        trace_comments: comments,
        source: "fixture".to_string(),
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@example.invalid"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(root: &Path, rel: &str, body: &str, subject: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", subject]);
}

// --- target resolution (acceptance 4) -------------------------------------------

#[test]
fn story_1487_gap_and_coverage_are_never_spec_ids() {
    assert_eq!(resolve_target("gap"), CriteriaTarget::Coverage);
    assert_eq!(resolve_target("GAP"), CriteriaTarget::Coverage);
    assert_eq!(resolve_target(" coverage "), CriteriaTarget::Coverage);
    assert_eq!(
        resolve_target("TASK-1"),
        CriteriaTarget::Spec("TASK-1".to_string())
    );
    assert_eq!(
        resolve_target("gap-1"),
        CriteriaTarget::Spec("gap-1".to_string())
    );
}

// --- (a) trailers -------------------------------------------------------------------

#[test]
fn story_1487_trailer_share_counts_only_trailing_spec_groups() {
    let subjects: Vec<String> = [
        "[AI:claude] feat(x): thing (TASK-1)",
        "fix: another (BUG-2, STORY-3) (#12)",
        "chore: no trailer here",
        "docs: mentions TASK-9 in prose",
        "chore(scope): (scope)",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let share = trailer_share(&subjects);
    assert_eq!(share.numerator, 2);
    assert_eq!(share.denominator, 6);
    assert!(subject_has_spec_trailer("feat: x (TASK-1)"));
    assert!(!subject_has_spec_trailer("feat: x TASK-1"));
    assert_eq!(trailer_share(&[]).percent(), None);
}

// --- (b) authored work specs ----------------------------------------------------------

#[test]
fn story_1487_machine_filed_and_non_work_specs_are_excluded() {
    let authored = spec(
        "TASK-1",
        RequirementType::Task,
        "## Acceptance\n- A1. x\n",
        1,
    );
    assert!(is_authored_work_spec(&authored));

    let mut failure = spec("BUG-2", RequirementType::Bug, "## Acceptance\n- A1. x\n", 1);
    failure.title = "auto-complete failure: phase 2 (ci) on TASK-1".to_string();
    assert!(is_machine_filed(&failure));

    let mut review = spec(
        "STORY-3",
        RequirementType::Story,
        "## Acceptance\n- A1. x\n",
        1,
    );
    review.title = "Review PR-11: something".to_string();
    assert!(is_machine_filed(&review));

    let mut tagged = spec("TASK-4", RequirementType::Task, "", 1);
    tagged.tags.insert("auto-drafted".to_string());
    assert!(is_machine_filed(&tagged));

    let drafted = spec(
        "TASK-5",
        RequirementType::Task,
        "Auto-drafted by `aida queue work --auto-complete`: ...",
        1,
    );
    assert!(is_machine_filed(&drafted));

    let principle = spec(
        "PRIN-6",
        RequirementType::Principle,
        "## Acceptance\n- A1. x\n",
        1,
    );
    assert!(!is_authored_work_spec(&principle));
    assert!(is_work_type(&RequirementType::Spike));
    assert!(!is_work_type(&RequirementType::Folder));
}

// --- the pure report ------------------------------------------------------------------

#[test]
fn story_1487_report_figures_have_numerators_and_denominators() {
    let store = store_with(vec![
        // In window, two criteria, one traced.
        spec(
            "TASK-1",
            RequirementType::Task,
            "## Acceptance\n- A1. does x\n- A2. does y\n",
            5,
        ),
        // In window, no criteria.
        spec("BUG-2", RequirementType::Bug, "Just prose.", 5),
        // Out of the 90-day window, one criterion, traced.
        spec(
            "STORY-3",
            RequirementType::Story,
            "## Acceptance\n- A1. old\n",
            200,
        ),
        // Machine-filed with criteria: excluded everywhere.
        {
            let mut r = spec("BUG-4", RequirementType::Bug, "## Acceptance\n- A1. z\n", 1);
            r.title = "auto-complete failure: phase 1".into();
            r
        },
        // Non-work type: excluded.
        spec(
            "PRIN-5",
            RequirementType::Principle,
            "## Acceptance\n- A1. p\n",
            1,
        ),
    ]);
    let subjects_window: Vec<String> = vec!["feat: a (TASK-1)".into(), "chore: b".into()];
    let subjects_all: Vec<String> = vec![
        "feat: a (TASK-1)".into(),
        "chore: b".into(),
        "fix: c (STORY-3)".into(),
    ];
    let toks = tokens(&["TASK-1.A1", "TASK-1.A1", "STORY-3.A1", "BUG-4.A1"], 7);
    let report = coverage_from_parts(
        &store,
        Utc::now(),
        90,
        &subjects_window,
        &subjects_all,
        &toks,
        Some("abc1234".into()),
    );

    assert_eq!(report.windows.len(), 2);
    let w = &report.windows[0];
    assert_eq!(w.label, "last 90 days");
    assert_eq!(w.window_days, Some(90));
    assert_eq!(
        w.commits_with_trailer,
        Share {
            numerator: 1,
            denominator: 2
        }
    );
    assert_eq!(
        w.specs_with_criteria,
        Share {
            numerator: 1,
            denominator: 2
        }
    );
    assert_eq!(
        w.criteria_with_traced_test,
        Share {
            numerator: 1,
            denominator: 2
        }
    );

    let all = &report.windows[1];
    assert_eq!(all.label, "all time");
    assert_eq!(all.window_days, None);
    assert_eq!(
        all.commits_with_trailer,
        Share {
            numerator: 2,
            denominator: 3
        }
    );
    assert_eq!(
        all.specs_with_criteria,
        Share {
            numerator: 2,
            denominator: 3
        }
    );
    assert_eq!(
        all.criteria_with_traced_test,
        Share {
            numerator: 2,
            denominator: 3
        }
    );

    assert_eq!(report.criterion_trace_tokens, 4);
    assert_eq!(report.criterion_traced_specs, 3);
    assert_eq!(report.trace_comments, 7);
    assert_eq!(report.head.as_deref(), Some("abc1234"));

    // --json carries the same fields.
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["windows"][0]["commits_with_trailer"]["numerator"], 1);
    assert_eq!(json["windows"][0]["commits_with_trailer"]["denominator"], 2);
    assert_eq!(
        json["windows"][1]["criteria_with_traced_test"]["numerator"],
        2
    );
    assert_eq!(json["trace_comments"], 7);
    assert_eq!(json["criterion_traced_specs"], 3);
}

/// Acceptance 5: a project with no criteria traces at all reports 0 of N and
/// the report still builds (the CLI then exits 0).
#[test]
fn story_1487_no_traces_at_all_reports_zero_of_n() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    commit_file(
        root,
        "src/lib.rs",
        "pub fn f() {}\n",
        "feat: first (TASK-1)",
    );
    commit_file(root, "README.md", "hi\n", "docs: readme");
    let store = store_with(vec![
        spec(
            "TASK-1",
            RequirementType::Task,
            "## Acceptance\n- A1. x\n- A2. y\n",
            1,
        ),
        spec("BUG-2", RequirementType::Bug, "no criteria", 1),
    ]);

    let report = build_coverage_report(root, &store, Utc::now(), 90);
    let w = &report.windows[0];
    assert_eq!(
        w.commits_with_trailer,
        Share {
            numerator: 1,
            denominator: 2
        }
    );
    assert_eq!(
        w.specs_with_criteria,
        Share {
            numerator: 1,
            denominator: 2
        }
    );
    assert_eq!(
        w.criteria_with_traced_test,
        Share {
            numerator: 0,
            denominator: 2
        }
    );
    assert_eq!(report.criterion_trace_tokens, 0);
    assert_eq!(report.criterion_traced_specs, 0);
    assert_eq!(report.trace_comments, 0);
    assert!(report.token_source.contains("git grep"));
    assert!(report.head.is_some());

    // The CLI handler path renders both shapes without error.
    handle_criteria_coverage(root, &store, 90, true).unwrap();
    handle_criteria_coverage(root, &store, 90, false).unwrap();
}

/// Tracked traces are found through `git grep`, criterion tokens are matched
/// case-insensitively to parsed criterion ids, and untracked files do not
/// count.
#[test]
fn story_1487_git_grep_counts_tracked_traces() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    commit_file(
        root,
        "src/tests/t.rs",
        "// trace:TASK-1.A1 | ai:claude\n#[test]\nfn a() {}\n\n// trace:TASK-1 | ai:claude\nfn impl_side() {}\n\n// trace:BUG-7.ac1a2b3 | ai:claude\n#[test]\nfn b() {}\n",
        "test: traced (TASK-1)",
    );
    commit_file(
        root,
        "notes.md",
        "trace:TASK-1.A2 in prose docs\n",
        "docs: note",
    );
    // Untracked source never counts.
    std::fs::write(root.join("scratch.rs"), "// trace:TASK-1.A2 | ai:claude\n").unwrap();

    let toks = collect_trace_tokens(root);
    assert!(toks.source.contains("git grep"));
    // Criterion tokens come from every tracked file (docs included, as the
    // SPIKE-86 script counts them); comments only from source pathspecs.
    let mut sorted = toks.criterion_tokens.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "BUG-7.AC1A2B3".to_string(),
            "TASK-1.A1".to_string(),
            "TASK-1.A2".to_string()
        ],
        "unexpected tokens: {:?}",
        toks.criterion_tokens
    );
    assert_eq!(toks.trace_comments, 3, "three trace: tokens in *.rs");

    let store = store_with(vec![spec(
        "TASK-1",
        RequirementType::Task,
        "## Acceptance\n- A1. x\n- A2. y\n- A3. z\n",
        1,
    )]);
    let report = build_coverage_report(root, &store, Utc::now(), 90);
    assert_eq!(
        report.windows[0].criteria_with_traced_test,
        Share {
            numerator: 2,
            denominator: 3
        }
    );
    assert_eq!(report.criterion_traced_specs, 2);
}

/// Outside a git checkout the filesystem walk answers instead of failing.
#[test]
fn story_1487_filesystem_fallback_outside_git() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src").join("t.rs"),
        "// trace:TASK-1.A1 | ai:claude\n#[test]\nfn a() {}\n// trace:TASK-1 | ai:claude\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target").join("x.rs"), "// trace:TASK-9.A1\n").unwrap();

    let toks = collect_trace_tokens(root);
    assert!(toks.source.contains("filesystem"), "{}", toks.source);
    assert_eq!(toks.criterion_tokens, vec!["TASK-1.A1".to_string()]);
    assert_eq!(toks.trace_comments, 2);

    let store = store_with(vec![spec(
        "TASK-1",
        RequirementType::Task,
        "## Acceptance\n- A1. x\n",
        1,
    )]);
    let report = build_coverage_report(root, &store, Utc::now(), 90);
    assert_eq!(report.windows[0].commits_with_trailer, Share::default());
    assert_eq!(
        report.windows[0].criteria_with_traced_test,
        Share {
            numerator: 1,
            denominator: 1
        }
    );
    assert!(report.head.is_none());
}

#[test]
fn story_1487_share_percent_is_none_on_empty_denominator() {
    assert_eq!(Share::default().percent(), None);
    let half = Share {
        numerator: 1,
        denominator: 2,
    };
    assert!((half.percent().unwrap() - 50.0).abs() < f64::EPSILON);
}
