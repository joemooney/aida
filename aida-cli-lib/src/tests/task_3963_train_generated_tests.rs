// trace:TASK-3963 | ai:claude
use super::*;
use crate::gate::mandate_gates::PathSource;

const DRIVER: &str = include_str!("../../../scripts/repair-generated.py");

fn tuple(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("\"{i}\"")).collect();
    match quoted.as_slice() {
        [one] => format!("({one},)"),
        _ => format!("({})", quoted.join(",")),
    }
}

/// The driver's REGISTRY with comments and whitespace removed and its path
/// constants substituted.
fn python_registry() -> String {
    let mut text: String = DRIVER
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| l.split(" #").next().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for line in DRIVER.lines() {
        if let Some((name, value)) = line.split_once(" = \"") {
            if name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                text = text.replace(
                    &format!("({name},"),
                    &format!("(\"{}\",", value.trim_end_matches('"')),
                );
            }
        }
    }
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn every_train_entry_matches_its_embedded_registry_row() {
    let registry = python_registry();
    for e in TRAIN_GENERATED {
        let row = format!(
            "(\"{}\",{},{},{})",
            e.name,
            tuple(e.outputs),
            tuple(e.inputs),
            tuple(e.argv)
        );
        assert!(
            registry.contains(&row),
            "{} differs from REGISTRY: {row}",
            e.name
        );
    }
    let run: Vec<&str> = TRAIN_GENERATED
        .iter()
        .filter(|e| e.train_run)
        .map(|e| e.name)
        .collect();
    assert!(registry.contains(&format!("TRAIN_ENTRIES={}", tuple(&run))));
    assert_eq!(TRAIN_GENERATED[1].class, TrainClass::KeyedLedger);
}

#[test]
fn no_train_output_is_on_the_protected_floor_and_a_config_match_disables_it() {
    let floor = ProtectedPaths::floor();
    for e in TRAIN_GENERATED {
        let outputs: Vec<String> = e.outputs.iter().map(|p| p.to_string()).collect();
        assert!(
            touches_gates(&outputs, &floor).is_none(),
            "{} is protected",
            e.name
        );
    }
    let (all, warnings) = runnable(&floor);
    assert_eq!(all.len(), 2);
    assert!(warnings.is_empty());
    let mut protected = ProtectedPaths::floor();
    protected.extend(
        PathSource::Committed,
        ["scripts/store-writers.toml".to_string()],
    );
    let (some, warnings) = runnable(&protected);
    assert_eq!(
        some.iter().map(|e| e.name).collect::<Vec<_>>(),
        ["showcase-search"]
    );
    assert!(
        warnings[0].contains("store-writers disabled"),
        "{warnings:?}"
    );
}

#[test]
fn an_entry_runs_only_when_two_cars_touch_its_inputs() {
    let (entries, _) = runnable(&ProtectedPaths::floor());
    let cars = |paths: &[&[&str]]| -> Vec<Vec<String>> {
        paths
            .iter()
            .map(|p| p.iter().map(|s| s.to_string()).collect())
            .collect()
    };
    assert!(select(&entries, &cars(&[&["docs/a.md"], &["README.md"]])).is_empty());
    assert_eq!(
        select(&entries, &cars(&[&["docs/a.md"], &["docs/b.md"]])),
        ["showcase-search"]
    );
    assert_eq!(
        select(&entries, &cars(&[&["x/a.rs"], &["hooks/h"]])),
        ["store-writers"]
    );
    assert!(select(&entries, &cars(&[&["x/a.rsx"], &["docsy"]])).is_empty());
}

fn report(after: &str) -> Vec<RegenEntry> {
    vec![RegenEntry {
        name: "store-writers".into(),
        argv: TRAIN_GENERATED[1]
            .argv
            .iter()
            .map(|s| s.to_string())
            .collect(),
        status: 0,
        outputs: vec![RegenOutput {
            path: "scripts/store-writers.toml".into(),
            before: Some("aa".into()),
            after: Some(after.into()),
        }],
    }]
}

#[test]
fn a_non_canonical_report_refuses() {
    assert!(check_report(&["store-writers"], &report("bb")).is_ok());
    assert!(check_report(&["showcase-search"], &report("bb")).is_err());
    let mut bad = report("bb");
    bad[0].status = 1;
    assert!(check_report(&["store-writers"], &bad).is_err());
    let mut bad = report("bb");
    bad[0].argv.push("--extra".into());
    assert!(check_report(&["store-writers"], &bad).is_err());
    let mut bad = report("bb");
    bad[0].outputs[0].path = "scripts/other.toml".into();
    assert!(check_report(&["store-writers"], &bad).is_err());
}

#[test]
fn the_regen_commit_message_and_body_block_record_every_change() {
    let message = regen_message(&report("bb"));
    assert_eq!(
        message,
        "[AI:aida-train] chore(train): regenerate generated files\n\n\
         Aida-Train-Regenerated: store-writers scripts/store-writers.toml aa -> bb\n\
         Aida-Train-Regen-Command: store-writers python3 scripts/check-store-writers.py --regenerate\n"
    );
    assert!(crate::trailer_coverage::commit_completion_ids(REGEN_SUBJECT).is_empty());
    let regen = RegenCommit {
        commit: "c0ffee".into(),
        report: report("bb"),
    };
    let block = pr_body_block(&regen);
    assert!(block.starts_with(
        "aida-train-regenerated: c0ffee\n- store-writers `scripts/store-writers.toml` aa -> bb"
    ));
    assert!(regen_message(&report("aa")).ends_with("files\n\n\n"));
}

#[test]
fn verify_regen_refuses_other_paths_edited_messages_and_empty_commits() {
    let regen = RegenCommit {
        commit: "c".into(),
        report: report("bb"),
    };
    let touched = vec!["scripts/store-writers.toml".to_string()];
    let message = regen_message(&regen.report);
    assert!(verify_regen(&message, &touched, &regen).is_ok());
    let extra = vec![touched[0].clone(), "src/lib.rs".to_string()];
    assert!(verify_regen(&message, &extra, &regen).is_err());
    assert!(verify_regen(&format!("{message}smuggled\n"), &touched, &regen).is_err());
    let noop = RegenCommit {
        commit: "c".into(),
        report: report("aa"),
    };
    assert!(verify_regen(&regen_message(&noop.report), &[], &noop).is_err());
}

#[test]
fn ledger_rows_keep_reviewed_policy_and_new_rows_start_blocked() {
    let row = r#"{ id = "a", disposition = "host_only", blocker = "x" }"#;
    let inv = |rows: &str| format!("version = 2\nwriter = [\n  {rows},\n]\n");
    assert_eq!(ledger_violation(&inv(row), &inv(row)), None);
    let added = format!(r#"{row}, {{ id = "n", disposition = "blocked" }}"#);
    assert_eq!(ledger_violation(&inv(row), &inv(&added)), None);
    let edited = row.replace("\"x\"", "\"y\"");
    assert!(ledger_violation(&inv(row), &inv(&edited))
        .unwrap()
        .contains("reviewed row a"));
    let open = format!(r#"{row}, {{ id = "n", disposition = "s2_typed" }}"#);
    assert!(ledger_violation(&inv(row), &inv(&open))
        .unwrap()
        .contains("non-blocked row n"));
    assert!(ledger_violation("<<<<<<<", &inv(row)).is_some());
}

/// The live step on a real repository: two cars edit different guides, the
/// tip's search data is stale, and the one regeneration commit fixes only it.
/// A tip whose generated files are current gets no commit.
#[test]
fn the_train_tip_gets_one_regen_commit_and_a_clean_tip_gets_none() {
    let probe = std::process::Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "true"])
        .status();
    if !probe.is_ok_and(|s| s.success()) {
        eprintln!("skipped: bwrap cannot run here");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.join("docs/showcase")).unwrap();
    std::fs::write(
        repo.join("docs/showcase/build.py"),
        "from pathlib import Path\nd = Path('docs')\n\
         Path('docs/showcase/search-data.js').write_text(''.join(sorted(p.read_text() for p in d.glob('*.md'))))\n",
    )
    .unwrap();
    for (name, body) in [("a.md", "a\n"), ("b.md", "b\n")] {
        std::fs::write(repo.join("docs").join(name), body).unwrap();
    }
    std::fs::write(repo.join("docs/showcase/search-data.js"), "a\nb\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@e.invalid",
        "commit",
        "-qm",
        "base",
    ]);
    let base = git(&["rev-parse", "HEAD"]);
    let mut members = Vec::new();
    for (pr, name) in [(1u64, "a.md"), (2, "b.md")] {
        git(&["checkout", "-q", "-B", &format!("m{pr}"), &base]);
        std::fs::write(repo.join("docs").join(name), format!("{name} edited\n")).unwrap();
        git(&[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e.invalid",
            "commit",
            "-qam",
            "edit",
        ]);
        let head = git(&["rev-parse", "HEAD"]);
        members.push(crate::pr_train::Member {
            pr,
            head,
            subject: format!("edit {name}"),
            ..Default::default()
        });
    }
    let train = crate::pr_train::build(repo, &base, &members).unwrap();
    assert_eq!(train.cars.len(), 2, "{:?}", train.dropped);
    let (entries, _) = runnable(&ProtectedPaths::floor());
    let regen = crate::pr_train::regen::regen_commit(repo, repo, &train, &entries)
        .unwrap()
        .unwrap();
    assert_eq!(
        git(&["rev-parse", &format!("{}^", regen.commit)]),
        train.tip
    );
    assert_eq!(
        git(&[
            "show",
            &format!("{}:docs/showcase/search-data.js", regen.commit)
        ]),
        "a.md edited\nb.md edited"
    );
    let message = git(&["show", "-s", "--format=%B", &regen.commit]);
    assert!(
        message.starts_with(REGEN_SUBJECT)
            && message.contains("Aida-Train-Regenerated: showcase-search")
    );
    // Regenerating the regenerated tip again is a no-op: no second commit.
    let mut clean = train.clone();
    clean.tip = regen.commit.clone();
    clean.cars.last_mut().unwrap().commit = regen.commit.clone();
    assert_eq!(
        crate::pr_train::regen::regen_commit(repo, repo, &clean, &entries).unwrap(),
        None
    );
    // A generator that writes outside its outputs refuses; nothing is committed.
    std::fs::write(
        repo.join("docs/showcase/build.py"),
        "open('docs/x.md', 'w').write('x')\n",
    )
    .unwrap();
    let mut evil = train.clone();
    git(&["checkout", "-q", "--detach", &train.tip]);
    git(&[
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@e.invalid",
        "commit",
        "-qam",
        "evil generator",
    ]);
    evil.tip = git(&["rev-parse", "HEAD"]);
    evil.cars.last_mut().unwrap().commit = evil.tip.clone();
    let refused = crate::pr_train::regen::regen_commit(repo, repo, &evil, &entries).unwrap_err();
    assert!(refused.contains("unregistered outputs"), "{refused}");
}
