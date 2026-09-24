#![cfg(target_os = "linux")]
//! Black-box edit-path coverage for `aida edit --carve-out/--carve-into`
//! (STORY-1434): the carve-out write must resolve its target through the
//! BUG-1535 unambiguous path (refusing rather than guessing when
//! `--carve-into` names two specs), must refuse (not no-op) when the
//! criterion text isn't found verbatim, and — the atomicity half of the
//! spec — the description strike, the typed edge on both sides, and the
//! reason comment must all land together in one `aida edit` invocation, with
//! `aida show`'s default view and `aida do`'s pickup banner reflecting it.
// trace:STORY-1434 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success());
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .args(args)
        .output()
        .expect("run aida")
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn yaml(fixture: &Fixture, id: &str) -> String {
    std::fs::read_to_string(
        fixture
            .repo
            .join(".aida-store/objects/TASK/000")
            .join(format!("{id}.yaml")),
    )
    .unwrap()
}

/// The `description:` block only, so an assertion about what the
/// description says isn't confused by the same text appearing, deliberately,
/// in the audit-trail CARVE-OUT comment further down the same YAML file.
fn description_of(fixture: &Fixture, id: &str) -> String {
    let body = yaml(fixture, id);
    let start = body.find("description:").expect("description field");
    let rest = &body[start..];
    let end = rest.find("\nstatus:").unwrap_or(rest.len());
    rest[..end].to_string()
}

fn uuid_of(fixture: &Fixture, id: &str) -> String {
    yaml(fixture, id)
        .lines()
        .find_map(|l| l.strip_prefix("id: "))
        .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
        .expect("uuid line")
}

fn new_fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Carve Out Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    ok(&aida(
        &fixture,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    ));
    fixture
}

const CRITERION: &str = "a green nightly proves the GitLab path end to end.";

fn add_source_and_target(fixture: &Fixture) {
    ok(&aida(
        fixture,
        &[
            "add",
            "--title",
            "live smoke test",
            "--type",
            "task",
            "--description",
            &format!("Ship the GitLab forge smoke.\n\n## Acceptance\n- {CRITERION}\n- other unrelated criterion stays put."),
        ],
    ));
    ok(&aida(
        fixture,
        &[
            "add",
            "--title",
            "live-smoke acceptance carve target",
            "--type",
            "task",
            "--description",
            "carries the criterion now",
        ],
    ));
}

/// A colliding pair (TASK-2's agreed_id rewritten to TASK-1, the live
/// store's BUG-34 shape) — TASK-1 is ambiguous, TASK-3 is a clean
/// carve-out source.
fn colliding_fixture() -> Fixture {
    let fixture = new_fixture();
    for title in ["native owner", "agreed holder"] {
        ok(&aida(
            &fixture,
            &[
                "add",
                "--title",
                title,
                "--type",
                "task",
                "--description",
                "d",
            ],
        ));
    }
    let path = fixture
        .repo
        .join(".aida-store/objects/TASK/000/TASK-2.yaml");
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("agreed_id: TASK-2"), "{body}");
    std::fs::write(
        &path,
        body.replace("agreed_id: TASK-2", "agreed_id: TASK-1"),
    )
    .unwrap();
    git(
        &fixture.repo.join(".aida-store"),
        &["commit", "-qam", "collide"],
    );
    ok(&aida(&fixture, &["cache", "rebuild"]));
    ok(&aida(
        &fixture,
        &[
            "add",
            "--title",
            "carve-out source",
            "--type",
            "task",
            "--description",
            &format!("body.\n\n## Acceptance\n- {CRITERION}"),
        ],
    ));
    fixture
}

#[test]
fn carve_into_an_ambiguous_target_refuses() {
    let fixture = colliding_fixture();
    let before_source = yaml(&fixture, "TASK-3");
    let native_uuid = uuid_of(&fixture, "TASK-1");

    let out = aida(
        &fixture,
        &[
            "edit",
            "TASK-3",
            "--carve-out",
            CRITERION,
            "--carve-into",
            "TASK-1",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must refuse; stderr:\n{stderr}");
    assert!(stderr.contains("ambiguous"), "{stderr}");
    assert!(stderr.contains(&native_uuid), "{stderr}");

    // Neither the source nor either candidate was touched.
    assert_eq!(yaml(&fixture, "TASK-3"), before_source);
}

#[test]
fn carve_into_a_missing_target_refuses() {
    let fixture = new_fixture();
    add_source_and_target(&fixture);
    let before = yaml(&fixture, "TASK-1");

    let out = aida(
        &fixture,
        &[
            "edit",
            "TASK-1",
            "--carve-out",
            CRITERION,
            "--carve-into",
            "TASK-999",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must refuse; stderr:\n{stderr}");
    assert!(
        stderr.contains("not found") || stderr.contains("Requirement not found"),
        "{stderr}"
    );
    assert_eq!(yaml(&fixture, "TASK-1"), before, "source must be untouched");
}

#[test]
fn carve_out_text_not_found_leaves_the_store_unchanged() {
    let fixture = new_fixture();
    add_source_and_target(&fixture);
    let before_source = yaml(&fixture, "TASK-1");
    let before_target = yaml(&fixture, "TASK-2");

    let out = aida(
        &fixture,
        &[
            "edit",
            "TASK-1",
            "--carve-out",
            "a red nightly proves the Bitbucket path",
            "--carve-into",
            "TASK-2",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must refuse; stderr:\n{stderr}");
    assert!(stderr.contains("not found verbatim"), "{stderr}");
    assert_eq!(yaml(&fixture, "TASK-1"), before_source, "source untouched");
    assert_eq!(
        yaml(&fixture, "TASK-2"),
        before_target,
        "target untouched — no stray inverse edge"
    );
}

#[test]
fn carve_out_end_to_end_strikes_description_writes_both_edges_and_the_comment() {
    let fixture = new_fixture();
    add_source_and_target(&fixture);
    let target_uuid = uuid_of(&fixture, "TASK-2");
    let source_uuid = uuid_of(&fixture, "TASK-1");

    ok(&aida(
        &fixture,
        &[
            "edit",
            "TASK-1",
            "--carve-out",
            CRITERION,
            "--carve-into",
            "TASK-2",
            "--carve-reason",
            "live smoke fails before forge-smoke markers emit",
        ],
    ));

    // 1. Description: stale text gone, pointer present. Scoped to the
    //    `description:` field alone — the audit-trail comment below
    //    deliberately quotes the same criterion text, which must not make
    //    this assertion pass for the wrong reason.
    let source_description = description_of(&fixture, "TASK-1");
    assert!(
        !source_description.contains(CRITERION),
        "{source_description}"
    );
    assert!(
        source_description.contains("carved out"),
        "{source_description}"
    );
    assert!(
        source_description.contains("other unrelated criterion stays put"),
        "{source_description}"
    );

    let source_yaml = yaml(&fixture, "TASK-1");
    // 2. Both typed edges, in both YAML objects.
    assert!(source_yaml.contains("carved-out-to"), "{source_yaml}");
    assert!(source_yaml.contains(&target_uuid), "{source_yaml}");
    let target_yaml = yaml(&fixture, "TASK-2");
    assert!(target_yaml.contains("carved-from"), "{target_yaml}");
    assert!(target_yaml.contains(&source_uuid), "{target_yaml}");

    // 3. The reason comment, on the source.
    assert!(source_yaml.contains("CARVE-OUT:"), "{source_yaml}");
    assert!(
        source_yaml.contains("live smoke fails before forge-smoke markers emit"),
        "{source_yaml}"
    );

    // 4. `aida show`'s DEFAULT view (no -c): the criterion never renders as
    //    a LIVE acceptance line, and the carve-out reason surfaces without
    //    asking for it. The criterion text legitimately reappears inside the
    //    "Corrections / carve-outs" block (quoted, clearly framed as
    //    superseded) — the property under test is that the part of the
    //    output rendering the description doesn't carry it as current.
    let show = ok(&aida(&fixture, &["show", "TASK-1", "--no-git"]));
    let (before_corrections, corrections_on) = show
        .split_once("Corrections / carve-outs")
        .expect("corrections section present");
    assert!(
        !before_corrections.contains(CRITERION),
        "{before_corrections}"
    );
    assert!(corrections_on.contains("CARVE-OUT:"), "{corrections_on}");
    assert!(corrections_on.contains(CRITERION), "{corrections_on}");
    assert!(show.contains("TASK-2"), "{show}");

    // 5. `aida do` warns at pickup, before routing, for a drain spec.
    let out = aida(&fixture, &["do", "TASK-1", "--mode", "drain"]);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("carved a criterion out to TASK-2"),
        "{combined}"
    );
}
