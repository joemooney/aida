//! STORY-2220: `aida docs sweep` — scope selection, the stale-command and
//! glossary checks, the PR-mode plan and its file cap, the report mail, the
//! sweep state, and the scaffolded idle-only job.
//! trace:STORY-2220 | ai:claude

use super::*;

fn cli() -> clap::Command {
    let mut root = crate::command_registry::built_command();
    root.build();
    root
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

// --- AC1: the command's scope -----------------------------------------------

#[test]
fn scope_covers_cli_docs_website_glossary_and_readmes_only() {
    let web = "docs/showcase";
    assert!(in_scope("docs/cli/02-specs.md", web));
    assert!(in_scope("docs/showcase/trust.html", web));
    assert!(in_scope("README.md", web));
    assert!(in_scope("aida-core/README.md", web));
    assert!(in_scope(
        "aida-core/templates/.aida/discipline/machinery-glossary.md",
        web
    ));
    assert!(!in_scope("aida-cli-lib/src/docs_sweep.rs", web));
    assert!(!in_scope("docs/showcase/app.js", web));
    assert!(!in_scope("docs/plans/2026-10-31-x.md", web));
    assert!(!in_scope(".claude/skills/aida-doc/README.md", web));
    assert!(!in_scope("target/doc/README.md", web));
}

#[test]
fn first_sweep_reviews_everything_in_scope() {
    let tracked = s(&["docs/cli/a.md", "README.md", "src/main.rs"]);
    assert_eq!(
        select_scope(&tracked, None, "docs/showcase"),
        s(&["docs/cli/a.md", "README.md"])
    );
}

#[test]
fn later_sweep_reviews_only_docs_changed_since_the_reference() {
    let tracked = s(&["docs/cli/a.md", "docs/cli/b.md", "README.md", "src/x.rs"]);
    let changed = s(&["docs/cli/b.md", "src/x.rs", "docs/cli/deleted.md"]);
    assert_eq!(
        select_scope(&tracked, Some(&changed), "docs/showcase"),
        s(&["docs/cli/b.md"])
    );
}

#[test]
fn a_cli_surface_change_widens_the_sweep_to_every_doc() {
    let tracked = s(&["docs/cli/a.md", "README.md"]);
    let changed = s(&["aida-cli-lib/src/queue_cmd_args.rs"]);
    assert_eq!(
        select_scope(&tracked, Some(&changed), "docs/showcase"),
        s(&["docs/cli/a.md", "README.md"])
    );
}

// --- AC1: stale command text ------------------------------------------------

#[test]
fn invocations_come_from_code_spans_html_code_and_code_blocks() {
    assert_eq!(
        extract_invocations("Run `aida queue work` then `git push`.", false),
        s(&["aida queue work"])
    );
    assert_eq!(
        extract_invocations("<p>Use <code>aida show &lt;ID&gt;</code>.</p>", false),
        s(&["aida show <ID>"])
    );
    assert_eq!(
        extract_invocations("$ aida list --status draft", true),
        s(&["aida list --status draft"])
    );
    assert!(extract_invocations("aida list outside a block is prose", false).is_empty());
    assert!(extract_invocations("`aida --version`", false).is_empty());
    assert!(extract_invocations("`aida <command>`", false).is_empty());
}

#[test]
fn a_real_invocation_passes() {
    let root = cli();
    assert_eq!(
        check_invocation(&root, "aida queue work --batch NAME"),
        None
    );
    assert_eq!(check_invocation(&root, "aida show STORY-1 --card"), None);
    assert_eq!(
        check_invocation(&root, "aida docs sweep --since main --report-only"),
        None
    );
    assert_eq!(check_invocation(&root, "aida list | head"), None);
    assert_eq!(check_invocation(&root, "aida schedule status --help"), None);
}

#[test]
fn an_unknown_subcommand_is_a_problem_with_a_suggestion() {
    let root = cli();
    match check_invocation(&root, "aida schedule statsu") {
        Some(CommandProblem::UnknownSubcommand {
            path,
            token,
            suggestion,
        }) => {
            assert_eq!(path, "aida schedule");
            assert_eq!(token, "statsu");
            assert_eq!(suggestion.as_deref(), Some("aida schedule status"));
        }
        other => panic!("expected an unknown subcommand, got {other:?}"),
    }
}

#[test]
fn an_unknown_flag_is_a_problem() {
    let root = cli();
    match check_invocation(&root, "aida docs sweep --sinse main") {
        Some(CommandProblem::UnknownFlag {
            path,
            flag,
            suggestion,
        }) => {
            assert_eq!(path, "aida docs sweep");
            assert_eq!(flag, "--sinse");
            assert_eq!(suggestion.as_deref(), Some("--since"));
        }
        other => panic!("expected an unknown flag, got {other:?}"),
    }
}

#[test]
fn a_finding_carries_file_line_evidence_and_a_proposed_fix() {
    let root = cli();
    let text = "# Title\n\n```bash\naida schedule statsu\n```\n";
    let evidence = ChangeEvidence {
        since: Some("abc1234def".into()),
        cli_commit: Some("1234567 feat(queue): rename".into()),
        doc_commits: BTreeMap::new(),
    };
    let found = command_findings(&root, "docs/cli/x.md", text, &evidence);
    assert_eq!(found.len(), 1, "{found:?}");
    let f = &found[0];
    assert_eq!((f.file.as_str(), f.line), ("docs/cli/x.md", 4));
    assert!(
        f.evidence.contains("has no subcommand `statsu`"),
        "{}",
        f.evidence
    );
    assert!(
        f.evidence.contains("1234567 feat(queue): rename"),
        "{}",
        f.evidence
    );
    assert!(
        f.proposed_fix.contains("aida schedule status"),
        "{}",
        f.proposed_fix
    );
    assert_eq!(f.fix, None, "a guessed rename is never a mechanical fix");
}

#[test]
fn a_generated_file_gets_a_finding_but_no_mechanical_fix() {
    let mut text = String::from("<!-- GENERATED FILE — do not edit by hand. -->\n");
    // A hidden alias would otherwise be mechanically fixable.
    let root = cli();
    let alias = hidden_alias_invocation(&root);
    text.push_str(&format!("Run `{alias}`.\n"));
    let found = command_findings(&root, "docs/cli/12-glossary.md", &text, &Default::default());
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].fix, None);
    assert!(found[0].proposed_fix.contains("generated file"));
}

/// Some top-level subcommand reached by a hidden alias, as `aida <alias>`.
fn hidden_alias_invocation(root: &clap::Command) -> String {
    for sub in root.get_subcommands() {
        let visible: BTreeSet<&str> = sub.get_visible_aliases().collect();
        for alias in sub.get_all_aliases().filter(|a| !visible.contains(a)) {
            // Skip aliases a pre-clap rewrite already normalizes.
            let argv = s(&["aida", alias]);
            if crate::builtin_alias_rewrites(&argv).0 == argv {
                return format!("aida {alias}");
            }
        }
    }
    panic!("the CLI has no hidden top-level alias to test with");
}

#[test]
fn a_hidden_alias_gets_the_canonical_name_as_a_mechanical_fix() {
    let root = cli();
    let inv = hidden_alias_invocation(&root);
    match check_invocation(&root, &inv) {
        Some(CommandProblem::HiddenAlias { old, new }) => {
            assert_eq!(old, inv);
            assert_ne!(old, new);
        }
        other => panic!("expected a hidden alias, got {other:?}"),
    }
}

// --- AC6: glossary feed -----------------------------------------------------

#[test]
fn glossary_terms_are_headings_and_bold_lead_terms() {
    let known = glossary_terms_from(
        "# Machinery glossary\n### orchestrator\n### autopilot (autopilot envelope)\n## Adjacent terms\n- **spec / requirement** — see x\n",
    );
    assert!(known.contains("orchestrator"));
    assert!(known.contains("autopilot"));
    assert!(known.contains("spec"));
    assert!(known.contains("requirement"));
}

#[test]
fn inline_definitions_are_dfn_dt_and_strong_lead_ins() {
    assert_eq!(
        inline_terms("<p><strong>Hub.</strong> The remote.</p>"),
        s(&["Hub"])
    );
    assert_eq!(inline_terms("<dt>Merge hold</dt>"), s(&["Merge hold"]));
    assert_eq!(
        inline_terms("A <dfn>grant</dfn> lets a seat act."),
        s(&["grant"])
    );
    // Labels and sentences are not terms.
    assert!(inline_terms("<strong>Status: works today.</strong>").is_empty());
    assert!(inline_terms("<strong>Some work needs a decision first.</strong>").is_empty());
    assert!(inline_terms("<strong>planned</strong>").is_empty());
}

#[test]
fn a_website_term_with_no_glossary_entry_is_a_finding_with_a_stub_fix() {
    let known = glossary_terms_from("### hub\n### lease\n");
    let page = "<p><strong>Hub.</strong> x</p>\n<p><strong>Pool.</strong> y</p>\n<dt>Leases</dt>\n";
    let mut reported = BTreeSet::new();
    let found = glossary_findings(
        "docs/showcase/air-gapped.html",
        page,
        &known,
        Some("aida-core/templates/.aida/discipline/machinery-glossary.md"),
        &mut reported,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].line, 2);
    assert_eq!(found[0].kind, FindingKind::GlossaryGap);
    assert_eq!(
        found[0].fix,
        Some(Fix::GlossaryStub {
            source: "aida-core/templates/.aida/discipline/machinery-glossary.md".into(),
            term: "pool".into()
        })
    );
    // A second page using the same term does not report it again.
    let again = glossary_findings("docs/showcase/b.html", page, &known, None, &mut reported);
    assert!(again.is_empty());
}

// --- AC4 / AC5: the PR plan, the cap, and the edits --------------------------

fn finding(file: &str, line: usize, fix: Option<Fix>) -> Finding {
    Finding {
        kind: FindingKind::StaleCommand,
        file: file.into(),
        line,
        evidence: format!("e{line}"),
        proposed_fix: "p".into(),
        fix,
    }
}

fn replace(old: &str, new: &str) -> Option<Fix> {
    Some(Fix::Replace {
        old: old.into(),
        new: new.into(),
    })
}

#[test]
fn the_plan_splits_mechanical_fixes_from_drafts_and_honours_the_cap() {
    let findings = vec![
        finding("docs/cli/a.md", 1, replace("x", "y")),
        finding("docs/cli/a.md", 2, replace("x", "y")),
        finding("docs/cli/b.md", 1, replace("x", "y")),
        finding("docs/cli/c.md", 1, None),
        finding("docs/cli/d.md", 1, replace("x", "y")),
    ];
    let p = plan(&findings, 2);
    assert_eq!(
        p.edits.len(),
        3,
        "a second fix in an already-touched file fits"
    );
    assert_eq!(
        p.files,
        ["docs/cli/a.md", "docs/cli/b.md"].map(String::from).into()
    );
    assert_eq!(p.drafts.len(), 1);
    assert_eq!(p.over_cap.len(), 1);
    assert_eq!(p.over_cap[0].file, "docs/cli/d.md");
}

#[test]
fn the_plan_never_edits_code_tests_or_mirrors() {
    for path in [
        "aida-cli-lib/src/x.rs",
        "aida-cli-lib/tests/fixture.md",
        ".aida/discipline/machinery-glossary.md",
        ".claude/skills/x/SKILL.md",
    ] {
        assert!(!path_editable(path), "{path}");
        let p = plan(&[finding(path, 1, replace("x", "y"))], 20);
        assert!(p.edits.is_empty(), "{path}");
        assert_eq!(p.drafts.len(), 1, "{path}");
    }
    assert!(path_editable("docs/cli/a.md"));
    assert!(path_editable("docs/showcase/a.html"));
}

#[test]
fn the_default_cap_is_twenty_files() {
    let findings: Vec<Finding> = (0..25)
        .map(|i| finding(&format!("docs/cli/{i}.md"), 1, replace("x", "y")))
        .collect();
    let p = plan(&findings, SweepConfig::default().max_files);
    assert_eq!(p.files.len(), 20);
    assert_eq!(p.over_cap.len(), 5);
}

#[test]
fn edits_apply_on_their_line_and_skip_a_line_that_moved() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs/cli")).unwrap();
    std::fs::write(
        dir.path().join("docs/cli/a.md"),
        "one\nrun `aida old x`\nthree\n",
    )
    .unwrap();
    let changed = apply_edits(
        dir.path(),
        &[
            finding("docs/cli/a.md", 2, replace("aida old", "aida new")),
            finding("docs/cli/a.md", 3, replace("aida old", "aida new")),
        ],
    )
    .unwrap();
    assert_eq!(changed, ["docs/cli/a.md".to_string()].into());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("docs/cli/a.md")).unwrap(),
        "one\nrun `aida new x`\nthree\n"
    );
}

#[test]
fn a_glossary_stub_lands_before_the_trailing_sections() {
    let dir = tempfile::tempdir().unwrap();
    let src = "g/machinery-glossary.md";
    std::fs::create_dir_all(dir.path().join("g")).unwrap();
    std::fs::write(
        dir.path().join(src),
        "# G\n\n### lease\n\ntext\n\n## Adjacent terms\n\n- **x**\n",
    )
    .unwrap();
    let mut f = finding("docs/showcase/a.html", 3, None);
    f.fix = Some(Fix::GlossaryStub {
        source: src.into(),
        term: "pool".into(),
    });
    apply_edits(dir.path(), &[f.clone(), f]).unwrap();
    let text = std::fs::read_to_string(dir.path().join(src)).unwrap();
    assert_eq!(text.matches("### pool").count(), 1, "{text}");
    assert!(text.find("### pool").unwrap() < text.find("## Adjacent terms").unwrap());
    assert!(text.contains("docs/showcase/a.html"));
}

// --- AC3: report mode -------------------------------------------------------

#[test]
fn mode_defaults_to_report_and_parses_pr() {
    assert_eq!(parse_config("").unwrap(), SweepConfig::default());
    assert_eq!(SweepConfig::default().mode, SweepMode::Report);
    let cfg = parse_config("[docs_sweep]\nmode = \"pr\"\nmax_files = 5\n").unwrap();
    assert_eq!((cfg.mode, cfg.max_files), (SweepMode::Pr, 5));
    assert!(parse_config("[docs_sweep]\nmode = \"auto\"\n").is_err());
}

#[test]
fn one_report_mail_per_run_and_none_when_clean() {
    assert_eq!(
        report_mail_body(&[], SweepMode::Report, None, None, 0),
        None
    );
    let findings: Vec<Finding> = (0..50)
        .map(|i| finding("docs/cli/a.md", i + 1, None))
        .collect();
    let body = report_mail_body(&findings, SweepMode::Report, Some("abc"), None, 0).unwrap();
    assert!(body.starts_with("Documentation sweep since abc: 50 new findings (mode: report)."));
    assert!(body.contains("file and order"));
    assert!(body.contains("10 more"), "the mail lists a bounded number");
}

// --- AC2 / AC7: the job, the state, and schedule status ---------------------

#[test]
fn the_scaffold_registers_a_daily_idle_only_docs_sweep_job() {
    let section = crate::init_cmd::init_schedule_config_section();
    let value: toml::Value = toml::from_str(section).unwrap();
    let job = value["schedule"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["name"].as_str() == Some("docs-sweep"))
        .expect("docs-sweep job scaffolded");
    assert_eq!(job["command"].as_str(), Some("docs sweep"));
    assert_eq!(job["every"].as_str(), Some("1d"));
    let gate = job["while"].as_str().unwrap();
    assert!(gate.contains("queue.depth == 0"), "{gate}");
    assert!(gate.contains("drain.lock_free == true"), "{gate}");
    assert_eq!(job["enabled"].as_bool(), Some(true));
}

#[test]
fn state_round_trips_and_feeds_the_status_line() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(status_detail(dir.path()), None);
    let state = SweepState {
        last_swept_rev: Some("0123456789abcdef".into()),
        last_run_at: Some(Utc::now()),
        findings: 3,
        mode: Some(SweepMode::Report),
        pr_url: None,
        pr_spec: None,
        reported: vec!["abc".into()],
    };
    save_state(dir.path(), &state).unwrap();
    assert_eq!(load_state(dir.path()), state);
    let line = status_detail(dir.path()).unwrap();
    assert!(line.contains("3 findings"), "{line}");
    assert!(line.contains("mode report"), "{line}");
    assert!(line.contains("swept to 0123456789"), "{line}");
}

fn git_ok(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_report_only_run_finds_stale_text_since_a_ref_and_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git_ok(root, &["init", "-q", "-b", "main"]);
    git_ok(root, &["config", "user.email", "t@example.com"]);
    git_ok(root, &["config", "user.name", "t"]);
    std::fs::create_dir_all(root.join("docs/cli")).unwrap();
    std::fs::write(root.join("docs/cli/a.md"), "Run `aida schedule statsu`.\n").unwrap();
    std::fs::write(root.join("README.md"), "Run `aida show X`.\n").unwrap();
    git_ok(root, &["add", "-A"]);
    git_ok(root, &["commit", "-qm", "one"]);
    std::fs::write(root.join("README.md"), "Run `aida shwo X`.\n").unwrap();
    git_ok(root, &["commit", "-qam", "two"]);

    let cfg = SweepConfig::default();
    let (all, _, since) = sweep(root, &cfg, None).unwrap();
    assert_eq!(since, None);
    assert_eq!(all.len(), 2, "{all:?}");

    let (recent, head, since) = sweep(root, &cfg, Some("HEAD~1")).unwrap();
    assert!(since.is_some());
    assert_eq!(recent.len(), 1, "{recent:?}");
    assert_eq!((recent[0].file.as_str(), recent[0].line), ("README.md", 1));
    assert!(recent[0].evidence.contains("two"), "{}", recent[0].evidence);
    assert_eq!(head.len(), 40);

    run_in(
        root,
        SweepOpts {
            since: None,
            report_only: true,
            json: true,
        },
    )
    .unwrap();
    assert!(
        !root.join(STATE_FILE).exists(),
        "--report-only records nothing"
    );
    assert!(sweep(root, &cfg, Some("no-such-ref")).is_err());
}

// --- False positives a real sweep of this repository surfaced ---------------

#[test]
fn alternatives_quoted_values_and_pre_clap_aliases_are_not_stale() {
    let root = cli();
    for inv in [
        "aida queue list/next",
        "aida push/pull",
        "aida · <project> · role:<name>",
        "aida deputy --allow-tty 'aida merge-hold clear' --ttl 8h",
        "aida mylist --status open",
        "aida myqueue",
        "aida help --all",
    ] {
        assert_eq!(check_invocation(&root, inv), None, "{inv}");
    }
}

#[test]
fn step_labels_in_bold_are_not_glossary_terms() {
    for label in [
        "<li><strong>Discuss the outcome.</strong> A person…</li>",
        "<li><strong>Review independently.</strong> x</li>",
        "<li><strong>Integrate under policy.</strong> x</li>",
    ] {
        assert!(inline_terms(label).is_empty(), "{label}");
    }
    assert_eq!(
        inline_terms("<li><strong>Store ref.</strong> The branch.</li>"),
        s(&["Store ref"])
    );
}

#[test]
fn a_finding_keeps_its_key_when_its_line_and_history_move() {
    let a = finding("docs/cli/a.md", 3, None);
    let mut b = a.clone();
    b.line = 9;
    b.evidence = "same problem; doc last changed in 1234567 x".into();
    assert_eq!(a.key(), b.key());
    let mut c = a.clone();
    c.file = "docs/cli/b.md".into();
    assert_ne!(a.key(), c.key());
}
