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
    let err = parse_config("[docs_sweep]\nmode = \"auto\"\n").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown [docs_sweep] mode 'auto' (expected report or pr)"
    );
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
    assert!(gate.contains("queue.project_work_idle == true"), "{gate}");
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
    let (all, _, since, _) = sweep(root, &cfg, None).unwrap();
    assert_eq!(since, None);
    assert_eq!(all.len(), 2, "{all:?}");

    let (recent, head, since, scope) = sweep(root, &cfg, Some("HEAD~1")).unwrap();
    assert_eq!(scope, s(&["README.md"]));
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

// --- Full runs: the mail, the state, and PR mode against a fake forge -------

fn git_out(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Write `files`, commit only them (the sweep's own `.aida/` runtime files
/// stay untracked), and push `main` when there is an `origin`.
fn commit_files(root: &Path, files: &[(&str, &str)], msg: &str) {
    for (path, body) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
        git_ok(root, &["add", "--", path]);
    }
    git_ok(root, &["commit", "-qm", msg]);
    if !git_out(root, &["remote"]).is_empty() {
        git_ok(root, &["push", "-q", "origin", "main"]);
    }
}

/// A project repository under `tmp/work`, optionally with a bare
/// `tmp/origin.git` it pushes `main` to.
fn project(tmp: &Path, with_origin: bool) -> std::path::PathBuf {
    let root = tmp.join("work");
    std::fs::create_dir_all(&root).unwrap();
    git_ok(&root, &["init", "-q", "-b", "main"]);
    git_ok(&root, &["config", "user.email", "t@example.com"]);
    git_ok(&root, &["config", "user.name", "t"]);
    if with_origin {
        let origin = tmp.join("origin.git");
        git_ok(
            tmp,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git_ok(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
    }
    root
}

fn mails(root: &Path) -> Vec<aida_core::mailbox::Message> {
    crate::mailbox_store::read_local_messages(root).unwrap()
}

fn normal_run() -> SweepOpts<'static> {
    SweepOpts {
        since: None,
        report_only: false,
        json: true,
    }
}

/// A forge and spec store that record what a run asked of them.
#[derive(Default)]
struct FakeHost {
    open: std::cell::RefCell<Option<OpenPr>>,
    created: std::cell::RefCell<Vec<(String, String)>>,
    specs: std::cell::RefCell<Vec<NewSpec>>,
}

impl SweepHost for FakeHost {
    fn open_sweep_pr(&self, _: &Path) -> Result<Option<OpenPr>> {
        Ok(self.open.borrow().clone())
    }
    fn create_pr(&self, _: &Path, base: &str, title: &str, _: &str) -> Result<String> {
        self.created
            .borrow_mut()
            .push((base.to_string(), title.to_string()));
        let url = format!("https://forge.example/pr/{}", self.created.borrow().len());
        *self.open.borrow_mut() = Some(OpenPr {
            url: url.clone(),
            title: title.to_string(),
        });
        Ok(url)
    }
    fn filed_keys(&self, _: &Path) -> Result<BTreeSet<String>> {
        Ok(self
            .specs
            .borrow()
            .iter()
            .flat_map(|s| s.tags.clone())
            .filter(|t| t.starts_with(KEY_TAG_PREFIX))
            .collect())
    }
    fn file_spec(&self, _: &Path, spec: NewSpec) -> Result<String> {
        self.specs.borrow_mut().push(spec);
        Ok(format!("TASK-{}", self.specs.borrow().len()))
    }
}

impl FakeHost {
    fn ids_with(&self, status: aida_core::RequirementStatus) -> Vec<String> {
        self.specs
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.status == status)
            .map(|(i, _)| format!("TASK-{}", i + 1))
            .collect()
    }
}

/// AC2/AC3: a normal report-mode run records the swept revision and sends
/// exactly one mail with the fresh findings; a clean rerun sends none; a
/// later run mails only what is new.
#[test]
fn a_report_run_records_the_revision_and_mails_only_fresh_findings_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = project(tmp.path(), false);
    commit_files(
        &root,
        &[("docs/cli/a.md", "Run `aida schedule statsu`.\n")],
        "one",
    );
    let host = FakeHost::default();

    run_with(&root, normal_run(), &host).unwrap();
    let state = load_state(&root);
    assert_eq!(
        state.last_swept_rev.as_deref(),
        Some(git_out(&root, &["rev-parse", "HEAD"]).as_str())
    );
    assert_eq!((state.findings, state.mode), (1, Some(SweepMode::Report)));
    let first = mails(&root);
    assert_eq!(first.len(), 1, "one mail per run");
    assert_eq!(
        first[0].to,
        aida_core::mailbox::Recipient::Agent("product".into())
    );
    assert!(
        first[0].body.contains("docs/cli/a.md:1 ["),
        "{}",
        first[0].body
    );
    assert!(host.specs.borrow().is_empty() && host.created.borrow().is_empty());

    // Nothing changed since the recorded revision: no findings, no mail.
    run_with(&root, normal_run(), &host).unwrap();
    assert_eq!(mails(&root).len(), 1, "a clean run sends no mail");
    assert_eq!(load_state(&root).findings, 0);

    // A new stale line beside the already-reported one: only it is mailed.
    commit_files(
        &root,
        &[(
            "docs/cli/a.md",
            "Run `aida schedule statsu`.\nThen `aida shwo X`.\n",
        )],
        "two",
    );
    run_with(&root, normal_run(), &host).unwrap();
    let seen: BTreeSet<String> = first.iter().map(|m| m.id.clone()).collect();
    let new: Vec<_> = mails(&root)
        .into_iter()
        .filter(|m| !seen.contains(&m.id))
        .collect();
    assert_eq!(new.len(), 1);
    assert!(new[0].body.contains("docs/cli/a.md:2 ["), "{}", new[0].body);
    assert!(
        !new[0].body.contains("docs/cli/a.md:1 ["),
        "{}",
        new[0].body
    );
    assert_eq!(new[0].subject.as_deref(), Some("Docs sweep: 1 finding"));
    assert_eq!(load_state(&root).findings, 2);
}

fn commits_on_sweep_branch(origin: &Path) -> Vec<String> {
    lines_of(git_out(
        origin,
        &["log", "--format=%s", &format!("main..{SWEEP_BRANCH}")],
    ))
    .into_iter()
    .filter(|s| s.starts_with("[AI:aida]"))
    .collect()
}

/// AC4/AC5: one PR per run, the next run extends the open PR (even when
/// this clone lost the spec it recorded), judgment calls become draft specs
/// filed once, and the batch spec takes the light review tier.
#[test]
fn pr_mode_opens_one_pr_then_extends_it_and_files_drafts_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = project(tmp.path(), true);
    let origin = tmp.path().join("origin.git");
    let inv = hidden_alias_invocation(&cli());
    let canonical = match check_invocation(&cli(), &inv) {
        Some(CommandProblem::HiddenAlias { new, .. }) => new,
        other => panic!("{other:?}"),
    };
    commit_files(
        &root,
        &[
            (".aida/config.toml", "[docs_sweep]\nmode = \"pr\"\n"),
            (
                "docs/cli/a.md",
                &format!("Run `{inv}`.\nAnd `aida schedule statsu`.\n"),
            ),
        ],
        "one",
    );
    let host = FakeHost::default();

    // Run 1: a new PR from the default branch, one draft, one batch spec.
    run_with(&root, normal_run(), &host).unwrap();
    let batch = host.ids_with(aida_core::RequirementStatus::Approved);
    let drafts = host.ids_with(aida_core::RequirementStatus::Draft);
    assert_eq!((batch.len(), drafts.len()), (1, 1), "{:?}", host.specs);
    let batch = batch[0].clone();
    {
        let specs = host.specs.borrow();
        let batch_spec = specs
            .iter()
            .find(|s| s.status == aida_core::RequirementStatus::Approved);
        let tags = &batch_spec.unwrap().tags;
        assert!(tags.contains(&"batch:fasttrack".to_string()), "{tags:?}");
        let skip = crate::auto_complete::LifecycleSkip::from_tags(tags.iter().map(String::as_str));
        assert!(
            skip.no_review,
            "the batch spec skips the reviewer model phase"
        );
    }
    assert_eq!(
        host.created.borrow().clone(),
        vec![(
            "main".to_string(),
            format!("[AI:aida] docs(sweep): batched documentation fixes ({batch})")
        )]
    );
    assert_eq!(commits_on_sweep_branch(&origin).len(), 1);
    let fixed = git_out(&origin, &["show", &format!("{SWEEP_BRANCH}:docs/cli/a.md")]);
    assert!(fixed.contains(&format!("`{canonical}`")) && !fixed.contains(&format!("`{inv}`")));
    let mut state = load_state(&root);
    assert_eq!(state.pr_url.as_deref(), Some("https://forge.example/pr/1"));
    assert_eq!(state.pr_spec.as_deref(), Some(batch.as_str()));
    let body = &mails(&root)[0].body;
    assert!(
        body.contains("Sweep PR: https://forge.example/pr/1"),
        "{body}"
    );
    assert!(body.contains("filed as draft specs"), "{body}");

    // Run 2: a new doc on main while the PR is open, and this clone has
    // lost the spec it recorded. The run extends the PR and keeps the spec
    // its title names.
    state.pr_spec = None;
    save_state(&root, &state).unwrap();
    commit_files(
        &root,
        &[("docs/cli/b.md", &format!("Use `{inv}`.\n"))],
        "two",
    );
    run_with(&root, normal_run(), &host).unwrap();
    assert_eq!(host.created.borrow().len(), 1, "the open PR is extended");
    assert_eq!(
        host.ids_with(aida_core::RequirementStatus::Approved),
        vec![batch.clone()]
    );
    let commits = commits_on_sweep_branch(&origin);
    assert_eq!(commits.len(), 2, "{commits:?}");
    assert!(
        commits.iter().all(|c| c.ends_with(&format!("({batch})"))),
        "{commits:?}"
    );
    assert_eq!(load_state(&root).pr_spec.as_deref(), Some(batch.as_str()));

    // Run 3: a full sweep again (the state is gone). The draft is not filed
    // twice, and every fix is already on the branch, so nothing is
    // committed or filed.
    std::fs::remove_file(root.join(STATE_FILE)).unwrap();
    run_with(&root, normal_run(), &host).unwrap();
    assert_eq!(load_state(&root).findings, 3, "both docs were reviewed");
    assert_eq!(host.specs.borrow().len(), 2, "{:?}", host.specs);
    assert_eq!(commits_on_sweep_branch(&origin).len(), 2);
    assert_eq!(host.created.borrow().len(), 1);
}

/// Review round 1: an edit whose line moved applies nothing, so no spec is
/// filed and no PR is opened.
#[test]
fn a_run_whose_edits_all_skip_files_no_spec_and_opens_no_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let root = project(tmp.path(), true);
    commit_files(&root, &[("docs/cli/a.md", "Nothing stale here.\n")], "one");
    let host = FakeHost::default();
    let p = plan(
        &[finding(
            "docs/cli/a.md",
            1,
            replace("aida gone", "aida here"),
        )],
        20,
    );
    assert_eq!(p.edits.len(), 1);
    let out = open_or_extend_pr(&root, &SweepConfig::default(), &host, &p).unwrap();
    assert_eq!(out, None);
    assert!(host.specs.borrow().is_empty());
    assert!(host.created.borrow().is_empty());
}

/// AC acec3f70: a regenerate step that touches anything beyond the edited
/// docs (and same-named doc copies), or exceeds their combined cap, commits,
/// pushes and files nothing.
// trace:STORY-2220 | ai:codex
#[test]
fn a_regenerate_step_that_reaches_past_the_docs_commits_nothing() {
    let changed: BTreeSet<String> =
        ["aida-core/templates/.aida/discipline/machinery-glossary.md".to_string()].into();
    let mirror = ".aida/discipline/machinery-glossary.md".to_string();
    assert!(check_staged(
        &[changed.first().unwrap().clone(), mirror.clone()],
        &changed,
        20
    )
    .is_ok());
    assert!(
        check_staged(&[mirror.clone()], &changed, 0).is_err(),
        "derived copies count against the cap"
    );
    assert!(check_staged(&["src/lib.rs".into()], &changed, 20).is_err());
    assert!(check_staged(&["docs/other.md".into()], &changed, 20).is_err());

    let staged = vec![changed.first().unwrap().clone(), mirror];
    assert!(check_staged(&staged, &changed, 2).is_ok());
    let err = check_staged(&staged, &changed, 1).unwrap_err();
    assert!(err.to_string().contains("total 2 files"), "{err}");
    assert!(err.to_string().contains("cap of 1 by 1"), "{err}");

    let tmp = tempfile::tempdir().unwrap();
    let root = project(tmp.path(), true);
    let origin = tmp.path().join("origin.git");
    commit_files(&root, &[("docs/cli/a.md", "Run `aida old`.\n")], "one");
    let host = FakeHost::default();
    let cfg = SweepConfig {
        regenerate: vec!["echo x > touched.rs".into()],
        ..SweepConfig::default()
    };
    let p = plan(
        &[finding("docs/cli/a.md", 1, replace("aida old", "aida new"))],
        20,
    );
    let err = open_or_extend_pr(&root, &cfg, &host, &p).unwrap_err();
    assert!(err.to_string().contains("touched.rs"), "{err}");
    assert!(host.specs.borrow().is_empty());
    assert!(host.created.borrow().is_empty());
    let branch = crate::test_git::git_command()
        .current_dir(&origin)
        .args(["rev-parse", "--verify", "--quiet", SWEEP_BRANCH])
        .output()
        .unwrap();
    assert!(!branch.status.success(), "nothing was pushed");

    // A legitimate regenerated mirror must share the source's cap. With
    // room for only the source, refuse the whole run before a local commit,
    // push, spec filing or PR creation. With room for both, commit both.
    for max_files in [1, 2] {
        let cfg = SweepConfig {
            max_files,
            regenerate: vec!["mkdir -p docs/mirror && cp docs/cli/a.md docs/mirror/a.md".into()],
            ..SweepConfig::default()
        };
        let out = open_or_extend_pr(&root, &cfg, &host, &p);
        if max_files == 1 {
            let err = out.unwrap_err();
            assert!(err.to_string().contains("cap of 1 by 1"), "{err}");
            assert!(host.specs.borrow().is_empty());
            assert!(host.created.borrow().is_empty());
            assert_eq!(
                git_out(&root, &["rev-parse", SWEEP_BRANCH]),
                git_out(&root, &["rev-parse", "main"]),
                "the refused run made no local commit"
            );
            let branch = crate::test_git::git_command()
                .current_dir(&origin)
                .args(["rev-parse", "--verify", "--quiet", SWEEP_BRANCH])
                .output()
                .unwrap();
            assert!(!branch.status.success(), "the refused run pushed nothing");
        } else {
            let outcome = out.unwrap().unwrap();
            assert_eq!(outcome.files, 2);
            assert!(outcome.files <= max_files);
            let committed = lines_of(git_out(
                &origin,
                &["diff", "--name-only", &format!("main..{SWEEP_BRANCH}")],
            ));
            assert_eq!(committed, s(&["docs/cli/a.md", "docs/mirror/a.md"]));
            assert!(committed.len() <= max_files);
            assert_eq!(commits_on_sweep_branch(&origin).len(), 1);
            assert_eq!(host.specs.borrow().len(), 1);
            assert_eq!(host.created.borrow().len(), 1);
        }
    }
}

#[test]
fn a_file_not_reviewed_again_keeps_its_reported_findings() {
    let a = finding("docs/cli/a.md", 1, None);
    let b = finding("docs/cli/b.md", 1, None);
    let previous = vec![a.state_key(), b.state_key(), "legacy-bare-key".into()];
    // Only b.md was reviewed, and it is clean now: a.md's finding stays
    // reported, b.md's is forgotten, and the pre-file key format is dropped.
    assert_eq!(
        next_reported(&previous, &s(&["docs/cli/b.md"]), &[]),
        vec![a.state_key()]
    );
    let c = finding("docs/cli/b.md", 2, replace("x", "y"));
    let mut c = c;
    c.proposed_fix = "other".into();
    let mut want = vec![a.state_key(), c.state_key()];
    want.sort();
    assert_eq!(next_reported(&previous, &s(&["docs/cli/b.md"]), &[c]), want);
}

#[test]
fn a_replacement_matches_whole_words_only() {
    assert_eq!(find_bounded("Run `aida st` now.", "aida st"), Some(5));
    assert_eq!(find_bounded("Run `aida status` now.", "aida st"), None);
    assert_eq!(find_bounded("`xaida st`, `aida st`", "aida st"), Some(13));
}

#[test]
fn the_open_prs_spec_comes_from_its_title_trailer() {
    assert_eq!(
        spec_from_title("[AI:aida] docs(sweep): batched documentation fixes (TASK-12)"),
        Some("TASK-12".into())
    );
    assert_eq!(spec_from_title("docs: something (draft)"), None);
    assert_eq!(spec_from_title("no trailer"), None);
}

/// `[docs_sweep]` comes from the default branch's copy: a branch-local edit
/// (say, a `regenerate` command) is not honoured.
#[test]
fn the_sweep_config_is_read_from_the_default_branch_not_the_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = project(tmp.path(), false);
    commit_files(
        &root,
        &[(".aida/config.toml", "[docs_sweep]\nmode = \"pr\"\n")],
        "one",
    );
    std::fs::write(
        root.join(".aida/config.toml"),
        "[docs_sweep]\nmode = \"report\"\nregenerate = [\"rm -rf /\"]\n",
    )
    .unwrap();
    let cfg = load_config(&root).unwrap();
    assert_eq!(cfg.mode, SweepMode::Pr);
    assert!(cfg.regenerate.is_empty());
}

/// Advisor B1/B2: exercise the real snapshot/tick and canonical filing adapter,
/// including evidence a caller-local meter or advisory cache cannot see.
// trace:STORY-2220 | ai:codex
#[test]
fn docs_schedule_boundary_regression() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::schedule_predicate::{self, Field};
    use aida_core::DatabaseBackend;
    let tmp = tempfile::tempdir().unwrap();
    let root = project(tmp.path(), false);
    let store = root.join(".aida-store");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(&store).unwrap();
    git_ok(&store, &["init", "-q", "-b", "aida-store"]);
    git_ok(&store, &["config", "user.name", "Test"]);
    git_ok(&store, &["config", "user.email", "t@example.com"]);
    let config = "[deployment]\nstore_path = \".aida-store\"\n[docs_sweep]\nmode = \"pr\"\n\n[[schedule.jobs]]\nname = \"docs-sweep\"\ncommand = \"docs sweep\"\nevery = \"1d\"\nwhile = \"queue.project_work_idle == true && drain.lock_free == true\"\nenabled = true\n";
    commit_files(
        &root,
        &[
            (".aida/config.toml", config),
            ("docs/cli/a.md", "Run `aida schedule statsu`.\n"),
        ],
        "fixture",
    );
    let queues = store.join("registry/queues");
    std::fs::create_dir_all(&queues).unwrap();
    let fields = BTreeSet::from([Field::ProjectWorkIdle, Field::DrainLockFree]);
    let gate =
        schedule_predicate::parse("queue.project_work_idle == true && drain.lock_free == true")
            .unwrap();
    let runs = std::cell::Cell::new(0);
    let tick = |day| {
        crate::maintenance_schedule::tick_project_registry_for_test(
            &root,
            Utc::now() + chrono::Duration::days(day),
            &|command| {
                assert_eq!(command, "docs sweep");
                runs.set(runs.get() + 1);
                Ok(0)
            },
        )
        .unwrap()
    };
    let check = |want| {
        let snap = crate::maintenance_schedule::collect_snapshot(&root, &fields, None);
        assert_eq!(snap.project_work_idle, want);
        assert_eq!(schedule_predicate::eval(&gate, &snap).ok(), want);
    };
    check(Some(true));
    let local = queues.join("other-user.yaml");
    let row = aida_core::QueueEntry {
        user_id: "other-user".into(),
        requirement_id: uuid::Uuid::new_v4(),
        position: 0,
        added_by: "peer".into(),
        note: None,
        added_at: Utc::now(),
        for_role: Some("implementer".into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    std::fs::write(&local, serde_yaml::to_string(&vec![row]).unwrap()).unwrap();
    check(Some(false));
    tick(0);
    assert_eq!(runs.get(), 0);
    std::fs::write(&local, "[]\n").unwrap();
    let global = crate::home_dir().unwrap().join(".aida/queue");
    std::fs::create_dir_all(&global).unwrap();
    for (day, role) in ["implementer", "reviewer"].iter().enumerate() {
        let path = global.join(format!("{role}.yaml"));
        let row = crate::global_queue::GlobalQueueEntry {
            requirement_id: uuid::Uuid::new_v4(),
            project_root: root.clone(),
            project_name: "fixture".into(),
            spec_id: None,
            agreed_id: None,
            title: None,
            position: 0,
            added_by: "peer".into(),
            added_at: Utc::now(),
            note: None,
            for_role: role.to_string(),
        };
        std::fs::write(&path, serde_yaml::to_string(&vec![row]).unwrap()).unwrap();
        check(Some(false));
        tick(day as i64 + 1);
        assert_eq!(runs.get(), 0);
        std::fs::write(&path, "[]\n").unwrap();
    }
    // Parse failure and a read failure both refuse, even when own queue is empty.
    std::fs::write(&local, "not a queue").unwrap();
    check(None);
    assert!(tick(3).iter().any(|s| s.contains("evidence is unknown")));
    std::fs::remove_file(&local).unwrap();
    std::fs::create_dir(&local).unwrap();
    check(None);
    tick(4);
    assert_eq!(runs.get(), 0);
    std::fs::remove_dir(&local).unwrap();
    let global_path = global.join("reviewer.yaml");
    std::fs::write(&global_path, "null").unwrap();
    check(None);
    tick(5);
    assert_eq!(runs.get(), 0);
    std::fs::remove_file(&global_path).unwrap();
    std::fs::create_dir(&global_path).unwrap();
    check(None);
    tick(6);
    assert_eq!(runs.get(), 0);
    std::fs::remove_dir(&global_path).unwrap();
    check(Some(true));
    tick(7);
    assert_eq!(runs.get(), 1);

    // Build a cache before the key is filed, then hold it stale while an
    // independent writer adds the key to the canonical object.
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store);
    let backend = crate::test_env::home_fixture_cached_backend(&store, &cache_path).unwrap();
    let mut req = aida_core::Requirement::new("existing docs finding".into(), "body".into());
    req.spec_id = Some("TASK-1".into());
    let mut req = backend.add_requirement(req).unwrap();
    backend
        .list_summaries(&aida_core::ListFilter::default())
        .unwrap();
    let (findings, _, _, _) = sweep(&root, &load_config(&root).unwrap(), None).unwrap();
    assert_eq!(findings.len(), 1);
    let tag = format!("{KEY_TAG_PREFIX}{}", findings[0].key());
    req.tags.insert(tag.clone());
    crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .update_requirement(&req)
        .unwrap();
    let _foreign = crate::stale_cache_fixture::hold_foreign_refresh(&cache_path);
    assert!(backend
        .list_summaries(&aida_core::ListFilter::default())
        .unwrap()
        .iter()
        .all(|r| !r.tags.contains(&tag)));
    assert!(LiveHost.filed_keys(&root).unwrap().contains(&tag));
    run_with(&root, normal_run(), &LiveHost).unwrap();
    assert_eq!(
        aida_core::object_store::list_objects_strict(&store.join("objects"))
            .unwrap()
            .len(),
        1
    );
    let mut state = load_state(&root);
    state.last_swept_rev = None;
    save_state(&root, &state).unwrap();
    let before_state = std::fs::read(root.join(STATE_FILE)).unwrap();
    let before_mail = mails(&root).len();
    let path = aida_core::object_store::object_path(
        &store.join("objects"),
        req.spec_id.as_deref().unwrap(),
    )
    .unwrap();
    let original = std::fs::read(&path).unwrap();
    for unreadable in [false, true] {
        std::fs::remove_file(&path).unwrap();
        if unreadable {
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(&path, "broken: [").unwrap();
        }
        assert!(LiveHost.filed_keys(&root).is_err());
        assert!(run_with(&root, normal_run(), &LiveHost).is_err());
        assert_eq!(std::fs::read(root.join(STATE_FILE)).unwrap(), before_state);
        assert_eq!(mails(&root).len(), before_mail);
        assert_eq!(
            aida_core::object_store::list_objects_strict(&store.join("objects"))
                .unwrap()
                .len(),
            1
        );
        if unreadable {
            std::fs::remove_dir(&path).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::write(&path, &original).unwrap();
    }
}
