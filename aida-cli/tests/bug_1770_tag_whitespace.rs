#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for BUG-1770: a tag value containing
//! whitespace is refused at every write path, and `--remove-tag` reports what
//! it actually did instead of the generic "No changes specified".
//!
//! These tests drive the shipped CLI and then inspect the CANONICAL STORE —
//! the YAML list items and the parsed tag set — never the rendered `tags:`
//! line. The rendering is what hid this class for nine days: it joins tags
//! with spaces, so one malformed blob renders byte-identically to the N tags
//! it was meant to be and cannot witness the regression.
// trace:BUG-1770 | ai:claude

use aida_core::Storage;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git")
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(args)
        .output()
        .expect("run aida")
}

fn init_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(git(&repo, &["init", "-q", "-b", "main"]).status.success());
    assert!(git(&repo, &["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&repo, &["config", "user.name", "BUG-1770 Test"])
        .status
        .success());
    assert!(git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"])
        .status
        .success());

    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    let init = aida(
        &fixture,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "init failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    fixture
}

fn spec_id_from(stdout: &str) -> String {
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing task id in {stdout}"))
        .to_string()
}

/// The parsed tag set straight out of the canonical store.
fn stored_tags(fixture: &Fixture, id: &str) -> Vec<String> {
    let storage = Storage::new(fixture.repo.join(".aida-store"));
    let store = storage.load().expect("load store");
    let req = store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("{id} not in store"));
    let mut tags: Vec<String> = req.tags.iter().cloned().collect();
    tags.sort();
    tags
}

/// The raw YAML list items under `tags:` — the witness the rendered line
/// cannot provide. A whitespace blob is ONE item here; N real tags are N.
fn stored_tag_yaml_items(fixture: &Fixture, id: &str) -> Vec<String> {
    let path = find_object_yaml(&fixture.repo.join(".aida-store").join("objects"), id)
        .unwrap_or_else(|| panic!("no store object file for {id}"));
    let text = std::fs::read_to_string(&path).expect("read store object");
    let mut items = Vec::new();
    let mut in_tags = false;
    for line in text.lines() {
        if line.starts_with("tags:") {
            in_tags = true;
            continue;
        }
        if in_tags {
            match line.strip_prefix("- ") {
                Some(item) => items.push(item.trim().to_string()),
                // any non-item line ends the block (next key, or `tags: []`)
                None => break,
            }
        }
    }
    items
}

fn find_object_yaml(dir: &Path, id: &str) -> Option<PathBuf> {
    let wanted = format!("{id}.yaml");
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() {
            if let Some(found) = find_object_yaml(&path, id) {
                return Some(found);
            }
        } else if path.file_name().map(|n| n == wanted.as_str()) == Some(true) {
            return Some(path);
        }
    }
    None
}

fn add_task(fixture: &Fixture, tags: &str) -> Output {
    aida(
        fixture,
        &[
            "add",
            "--title",
            "BUG-1770 tag fixture",
            "--type",
            "task",
            "--status",
            "approved",
            "--tags",
            tags,
        ],
    )
}

/// AC1 + AC6: a comma-separated argument yields N separate YAML list items, a
/// space-separated one is refused, and the colon-namespaced control still
/// round-trips. The control is the case BUG-1542 was filed against and
/// correctly rejected over — it works today and must keep working, so it stops
/// a regression passing by breaking both forms at once.
#[test]
fn comma_separated_tags_are_separate_items_and_a_whitespace_tag_is_refused_at_add() {
    let fixture = init_fixture();

    let good = add_task(&fixture, "alpha,beta,severity:major");
    assert!(
        good.status.success(),
        "comma form should be accepted: {}",
        String::from_utf8_lossy(&good.stderr)
    );
    let id = spec_id_from(&String::from_utf8_lossy(&good.stdout));

    // The YAML list, not the rendered line.
    let mut items = stored_tag_yaml_items(&fixture, &id);
    items.sort();
    assert_eq!(
        items,
        vec![
            "alpha".to_string(),
            "beta".to_string(),
            "severity:major".to_string()
        ],
        "three comma-separated tags must be three YAML list items"
    );
    // Control: the colon-namespaced tag survives verbatim.
    assert!(stored_tags(&fixture, &id).contains(&"severity:major".to_string()));

    let refused = add_task(&fixture, "gamma delta");
    assert!(
        !refused.status.success(),
        "a whitespace tag must be refused, got success:\nstdout={}",
        String::from_utf8_lossy(&refused.stdout)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        combined.contains("gamma delta"),
        "the refusal must name the offending value: {combined}"
    );
    assert!(
        combined.contains("--tags gamma,delta"),
        "the refusal must show the comma-separated repair: {combined}"
    );
}

/// AC1 + AC2: the refusal reaches the edit surface too, on both the replacing
/// `--tags` form and the incremental `--add-tag` form, and a refused edit
/// leaves the stored tag set exactly as it was.
#[test]
fn edit_refuses_a_whitespace_tag_on_both_the_replace_and_the_add_form() {
    let fixture = init_fixture();
    let good = add_task(&fixture, "alpha,severity:major");
    assert!(good.status.success());
    let id = spec_id_from(&String::from_utf8_lossy(&good.stdout));
    let before = stored_tags(&fixture, &id);

    let replace = aida(&fixture, &["edit", &id, "--tags", "gamma delta"]);
    assert!(
        !replace.status.success(),
        "`edit --tags \"gamma delta\"` must be refused"
    );
    let replace_text = format!(
        "{}{}",
        String::from_utf8_lossy(&replace.stdout),
        String::from_utf8_lossy(&replace.stderr)
    );
    assert!(
        replace_text.contains("--tags gamma,delta"),
        "edit --tags refusal must show the comma repair: {replace_text}"
    );

    let add = aida(&fixture, &["edit", &id, "--add-tag", "gamma delta"]);
    assert!(
        !add.status.success(),
        "`edit --add-tag \"gamma delta\"` must be refused"
    );
    let add_text = format!(
        "{}{}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(
        add_text.contains("--add-tag gamma --add-tag delta"),
        "the --add-tag refusal must suggest the repeatable form, not a comma list: {add_text}"
    );

    assert_eq!(
        before,
        stored_tags(&fixture, &id),
        "a refused tag edit must not mutate the stored tag set"
    );
}

/// AC3 + AC4: `--remove-tag` reports the outcome per tag. A flag that matched
/// nothing must never render as "No changes specified" — that message tells
/// the caller to pass a flag they already passed, which is what made a silent
/// no-op indistinguishable from success.
#[test]
fn remove_tag_reports_its_outcome_instead_of_claiming_no_changes_were_specified() {
    let fixture = init_fixture();
    let good = add_task(&fixture, "alpha,severity:major");
    assert!(good.status.success());
    let id = spec_id_from(&String::from_utf8_lossy(&good.stdout));

    let miss = aida(&fixture, &["edit", &id, "--remove-tag", "nosuchtag"]);
    let miss_text = String::from_utf8_lossy(&miss.stdout).to_string();
    assert!(
        !miss_text.contains("No changes specified"),
        "a passed tag flag must not report that no changes were specified: {miss_text}"
    );
    assert!(
        miss_text.contains("no matching tag to remove: nosuchtag"),
        "the miss must be named: {miss_text}"
    );

    let hit = aida(&fixture, &["edit", &id, "--remove-tag", "alpha"]);
    let hit_text = String::from_utf8_lossy(&hit.stdout).to_string();
    assert!(
        hit_text.contains("removed 1 tag: alpha"),
        "a successful removal must say so: {hit_text}"
    );
    assert_eq!(
        stored_tags(&fixture, &id),
        vec!["severity:major".to_string()],
        "the removal must land in the canonical store"
    );

    // The control for AC3's narrowing: with NO tag flag and no other field,
    // "No changes specified" is still the right answer.
    let bare = aida(&fixture, &["edit", &id]);
    let bare_text = String::from_utf8_lossy(&bare.stdout).to_string();
    assert!(
        bare_text.contains("No changes specified"),
        "with no flags at all the generic message is still correct: {bare_text}"
    );
}
