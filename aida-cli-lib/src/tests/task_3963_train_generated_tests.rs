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

#[test]
fn a_car_that_changes_a_generator_disables_its_entry() {
    let (entries, _) = runnable(&ProtectedPaths::floor());
    let cars: Vec<Vec<String>> = vec![
        vec!["docs/a.md".into()],
        vec!["docs/b.md".into(), "docs/showcase/build.py".into()],
    ];
    let (trusted, warnings) = base_trusted(&entries, &cars);
    assert_eq!(
        trusted.iter().map(|e| e.name).collect::<Vec<_>>(),
        ["store-writers"]
    );
    assert!(
        warnings[0].contains("showcase-search disabled")
            && warnings[0].contains("docs/showcase/build.py"),
        "{warnings:?}"
    );
    assert!(select(&trusted, &cars).is_empty());
    for e in TRAIN_GENERATED.iter().filter(|e| e.train_run) {
        assert!(!e.generators.is_empty(), "{} names no generator", e.name);
    }
}

#[test]
fn a_refusal_is_one_bounded_line() {
    assert_eq!(refusal_line("a\n  b\tc\n"), "a b c");
    let long = "x".repeat(1000);
    assert_eq!(refusal_line(&long).len(), 403);
}

/// The spawn contract: only the checkout is writable, the toolchain and the
/// pinned generators are read-only, HOME and /tmp are empty tmpfs mounts,
/// the network is unshared, and the host root is never bound.
#[test]
fn the_regen_sandbox_binds_only_the_checkout_and_the_toolchain() {
    use std::path::PathBuf;
    let wt = PathBuf::from("/s/tip");
    let pinned = vec![(
        PathBuf::from("/s/generator-0"),
        PathBuf::from("/s/tip/docs/showcase/build.py"),
    )];
    let args = crate::pr_train::regen::sandbox_args(
        &wt,
        &[PathBuf::from("/usr"), PathBuf::from("/lib")],
        "/usr/bin:/bin".as_ref(),
        &pinned,
        "abc",
        "[\"showcase-search\"]",
    );
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let sep = args.iter().position(|a| a == "--").unwrap();
    let want = [
        "--unshare-all",
        "--unshare-net",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--ro-bind-try",
        "/usr",
        "/usr",
        "--ro-bind-try",
        "/lib",
        "/lib",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/aida-train-home",
        "--bind",
        "/s/tip",
        "/s/tip",
        "--ro-bind",
        "/s/generator-0",
        "/s/tip/docs/showcase/build.py",
        "--chdir",
        "/s/tip",
        "--setenv",
        "PATH",
        "/usr/bin:/bin",
        "--setenv",
        "HOME",
        "/aida-train-home",
        "--setenv",
        "LC_ALL",
        "C",
        "--setenv",
        "TMPDIR",
        "/tmp",
        "--setenv",
        "PYTHONDONTWRITEBYTECODE",
        "1",
        "--setenv",
        "PYTHONSAFEPATH",
        "1",
        "--setenv",
        "GIT_CONFIG_NOSYSTEM",
        "1",
        "--setenv",
        "GIT_CONFIG_GLOBAL",
        "/dev/null",
        "--setenv",
        "GIT_OPTIONAL_LOCKS",
        "0",
    ];
    assert_eq!(args[..sep], want);
    assert_eq!(args[sep + 1..sep + 3], ["python3", "-c"]);
    assert_eq!(
        args[sep + 4..],
        ["abc", "train", "[\"showcase-search\"]", "240"]
    );
    // Writable mounts: exactly the checkout.
    let writable: Vec<&String> = args
        .windows(2)
        .filter(|w| w[0] == "--bind")
        .map(|w| &w[1])
        .collect();
    assert_eq!(writable, ["/s/tip"]);
}

/// An unreadable protected-path config (here: no committed gate ref) refuses
/// the whole train's regeneration and records why; nothing else changes.
#[test]
fn an_unreadable_gate_config_disables_regeneration_for_the_train() {
    let dir = tempfile::tempdir().unwrap();
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut train = crate::pr_train::Built {
        base: "b".into(),
        tip: "t".into(),
        cars: Vec::new(),
        dropped: Vec::new(),
        regen: None,
        regen_refused: None,
    };
    let before = train.clone();
    crate::pr_train::regen::regenerate(dir.path(), dir.path(), &mut train);
    let why = train.regen_refused.take().expect("refusal recorded");
    assert!(why.contains("protected-path configuration"), "{why}");
    assert_eq!(train, before);
    let mut refused = before.clone();
    refused.regen_refused = Some(why.clone());
    assert!(crate::pr_train::train_pr_body(&refused)
        .contains(&format!("aida-train-regeneration-refused: `{why}`")));
}

struct Fixture {
    _dir: tempfile::TempDir,
    repo: std::path::PathBuf,
    secret: std::path::PathBuf,
    train: crate::pr_train::Built,
}

impl Fixture {
    fn git(&self, args: &[&str]) -> String {
        fixture_git(&self.repo, args)
    }
}

fn fixture_git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=T", "-c", "user.email=t@e.invalid"])
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

/// A base whose `build.py` is `generator`, and a two-car train where each
/// car edits a different guide, so the tip's search data is stale.
fn fixture(generator: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, "host-secret").unwrap();
    std::fs::create_dir_all(repo.join("docs/showcase")).unwrap();
    let git = |args: &[&str]| fixture_git(&repo, args);
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(
        repo.join("docs/showcase/build.py"),
        generator.replace("SECRET", &secret.to_string_lossy()),
    )
    .unwrap();
    for (name, body) in [("a.md", "a\n"), ("b.md", "b\n")] {
        std::fs::write(repo.join("docs").join(name), body).unwrap();
    }
    std::fs::write(repo.join("docs/showcase/search-data.js"), "a\nb\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    let mut members = Vec::new();
    for (pr, name) in [(1u64, "a.md"), (2, "b.md")] {
        git(&["checkout", "-q", "-B", &format!("m{pr}"), &base]);
        std::fs::write(repo.join("docs").join(name), format!("{name} edited\n")).unwrap();
        git(&["commit", "-qam", "edit"]);
        members.push(crate::pr_train::Member {
            pr,
            head: git(&["rev-parse", "HEAD"]),
            subject: format!("edit {name}"),
            ..Default::default()
        });
    }
    let train = crate::pr_train::build(&repo, &base, &members).unwrap();
    assert_eq!(train.cars.len(), 2, "{:?}", train.dropped);
    Fixture {
        _dir: dir,
        repo,
        secret,
        train,
    }
}

/// Reads every guide, plus any host file it can reach, into the search data.
const HONEST: &str = "from pathlib import Path\n\
    def peek(p):\n    try:\n        return Path(p).read_text()\n    except OSError:\n        return ''\n\
    leak = peek('SECRET') + ''.join(peek(p) for p in Path.home().glob('*'))\n\
    d = Path('docs')\n\
    Path('docs/showcase/search-data.js').write_text(leak + ''.join(sorted(p.read_text() for p in d.glob('*.md'))))\n";

fn regen(
    f: &Fixture,
    train: &crate::pr_train::Built,
    launch: &crate::pr_train::regen::Launch,
) -> Result<Option<RegenCommit>, String> {
    let (entries, _) = runnable(&ProtectedPaths::floor());
    crate::pr_train::regen::regen_commit_with(&f.repo, train, &entries, launch)
}

/// A sandbox that never answers is stopped and refused; one that cannot
/// start is refused. Neither leaves a commit.
#[test]
fn a_generator_timeout_or_launch_failure_refuses() {
    let f = fixture(HONEST);
    let stub = f.repo.parent().unwrap().join("slow-bwrap");
    std::fs::write(&stub, "#!/bin/sh\nexec sleep 30\n").unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let slow = crate::pr_train::regen::Launch {
        program: stub.into(),
        timeout: std::time::Duration::from_secs(1),
    };
    let started = std::time::Instant::now();
    assert_eq!(
        regen(&f, &f.train, &slow).unwrap_err(),
        "the generators timed out"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let missing = crate::pr_train::regen::Launch {
        program: f.repo.join("no-such-bwrap").into(),
        timeout: std::time::Duration::from_secs(5),
    };
    assert_eq!(
        regen(&f, &f.train, &missing).unwrap_err(),
        "the regeneration sandbox could not start"
    );
}

fn bwrap_live() -> bool {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--unshare-all",
            "--ro-bind",
            "/usr",
            "/usr",
            "--",
            "/usr/bin/true",
        ])
        .status();
    if probe.is_ok_and(|s| s.success()) {
        return true;
    }
    assert!(
        std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
        "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot run here"
    );
    eprintln!("SKIP (bwrap unavailable): task_3963 live regeneration");
    false
}

/// The live step on a real repository: the one regeneration commit fixes
/// only the stale search data, using the BASE generator, which cannot read a
/// host file or the home directory. A regenerated tip gets no second commit,
/// and a car that changes the generator disables the entry.
#[test]
fn task_3963_bwrap_the_train_tip_gets_one_regen_commit_from_the_base_generator() {
    if !bwrap_live() {
        return;
    }
    let f = fixture(HONEST);
    let launch = crate::pr_train::regen::Launch::default();
    let regen_c = regen(&f, &f.train, &launch).unwrap().unwrap();
    assert_eq!(
        f.git(&["rev-parse", &format!("{}^", regen_c.commit)]),
        f.train.tip
    );
    assert_eq!(
        f.git(&[
            "show",
            &format!("{}:docs/showcase/search-data.js", regen_c.commit)
        ]),
        "a.md edited\nb.md edited",
        "the generator read {} or HOME",
        f.secret.display()
    );
    let message = f.git(&["show", "-s", "--format=%B", &regen_c.commit]);
    assert!(
        message.starts_with(REGEN_SUBJECT)
            && message.contains("Aida-Train-Regenerated: showcase-search")
    );
    let mut clean = f.train.clone();
    clean.tip = regen_c.commit.clone();
    clean.cars.last_mut().unwrap().commit = regen_c.commit.clone();
    assert_eq!(regen(&f, &clean, &launch).unwrap(), None);
    // A car that rewrites the generator to leak: the entry is not run.
    f.git(&["checkout", "-q", "--detach", &f.train.tip]);
    std::fs::write(
        f.repo.join("docs/showcase/build.py"),
        "open('docs/showcase/search-data.js', 'w').write('pwned')\n",
    )
    .unwrap();
    f.git(&["commit", "-qam", "evil generator"]);
    let mut evil = f.train.clone();
    evil.tip = f.git(&["rev-parse", "HEAD"]);
    evil.cars.last_mut().unwrap().commit = evil.tip.clone();
    assert_eq!(regen(&f, &evil, &launch).unwrap(), None);
}

/// A base generator that writes outside its outputs refuses; nothing is
/// committed.
#[test]
fn task_3963_bwrap_a_generator_writing_outside_its_outputs_refuses() {
    if !bwrap_live() {
        return;
    }
    let f = fixture("open('docs/x.md', 'w').write('x')\n");
    let refused = regen(&f, &f.train, &crate::pr_train::regen::Launch::default()).unwrap_err();
    assert!(refused.contains("unregistered outputs"), "{refused}");
}
