// trace:STORY-1741 | ai:claude
use super::*;

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn repo(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.invalid"]);
    git(dir, &["config", "user.name", "t"]);
    dir.canonicalize().unwrap()
}

fn put(top: &Path, rel: &str, body: &str) {
    let path = top.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Every byte under `top`, `.git` included, so any write shows.
fn tree_snapshot(top: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(top, &mut out);
    out
}

const CONFIG_BODY: &str = r#"
[deployment]
store_path = ".aida-store"

[agents]
vendor = "claude"

[terminal]
send_token = "secret"

[custom]
hook = "rm -rf /"
"#;

#[test]
fn the_inventory_classifies_tracked_scaffolding_and_writes_nothing() {
    // trace:STORY-1741.ac676d62 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let expected = generated(&top);
    let (skill, body) = expected
        .iter()
        .find(|(path, _)| path.starts_with(".claude/skills/aida-"))
        .expect("this build generates a Claude skill");
    let (hook, hook_body) = expected
        .iter()
        .find(|(path, _)| path.starts_with(".claude/hooks/aida-") && path.ends_with(".sh"))
        .expect("this build generates a Claude hook");
    let (edited, edited_body) = expected
        .iter()
        .find(|(path, body)| plane_guidance(path) && body.contains("AIDA Generated"))
        .expect("generated guidance with a header");
    put(&top, skill, body);
    put(&top, hook, hook_body);
    put(&top, edited, &format!("{edited_body}\nmy local rule\n"));
    put(
        &top,
        ".aida/discipline/spoofed.md",
        "<!-- AIDA Generated: v9.9.9 | checksum:deadbeef | DO NOT EDIT DIRECTLY -->\nmine\n",
    );
    put(&top, "AGENTS.md", "# Our agents\nhand written\n");
    put(&top, ".gitignore", "target/\n.aida-store/\n");
    put(&top, ".claude/settings.json", "{\"hooks\": {}}\n");
    put(&top, CONFIG, CONFIG_BODY);
    put(&top, "src/main.rs", "fn main() {}\n");
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "product with scaffolding"]);
    // A pre-dirty tree: the inventory reads the committed branch, not this.
    put(&top, "AGENTS.md", "uncommitted\n");
    put(&top, "scratch.txt", "untracked\n");
    let before = tree_snapshot(&top);

    let inv = inventory(&top).unwrap();
    assert_eq!(tree_snapshot(&top), before, "the inventory wrote something");

    let find = |path: &str| {
        inv.entries
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("{path} not inventoried: {:?}", inv.entries))
    };
    // Executable scaffolding the plane never carries is kept even when it is
    // byte for byte this build's output.
    for path in [skill, hook] {
        assert_eq!(find(path).class, Class::Generated);
        assert_eq!(find(path).proposal, Proposal::Keep, "{path}");
    }
    assert_eq!(find(edited).class, Class::GeneratedEdited);
    assert_eq!(find(edited).proposal, Proposal::Review);
    let spoofed = find(".aida/discipline/spoofed.md");
    assert_eq!(spoofed.class, Class::GeneratedEdited);
    assert_eq!(spoofed.proposal, Proposal::Review);
    assert_eq!(find("AGENTS.md").class, Class::Mixed);
    assert_eq!(find("AGENTS.md").proposal, Proposal::Keep);
    assert!(find(".gitignore").detail.contains(".aida-store/"));
    assert_eq!(find(".gitignore").proposal, Proposal::Keep);
    assert_eq!(find(".claude/settings.json").class, Class::Ambiguous);
    assert_eq!(find(".claude/settings.json").proposal, Proposal::Keep);
    assert!(inv.entries.iter().all(|e| e.path != "src/main.rs"));
    assert_eq!(inv.removable().count(), 0, "{:?}", inv.entries);

    let config = find(CONFIG);
    assert_eq!(config.class, Class::Config);
    assert_eq!(config.proposal, Proposal::Keep);
    let split = inv.config.as_ref().unwrap();
    assert_eq!(
        split.settings["agents"]["vendor"].as_str(),
        Some("claude"),
        "{split:?}"
    );
    let mut kept = split.kept.clone();
    kept.sort();
    assert_eq!(
        kept,
        [
            "custom.hook",
            "deployment.store_path",
            "terminal.send_token"
        ]
    );
}

#[test]
fn a_policy_only_config_is_removable_once_published_and_its_settings_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    put(
        &top,
        CONFIG,
        "use_agreed_blocks = true\n[agents]\nvendor = \"codex\"\n[agents.codex]\nmodel = \"m\"\n\
         [store.sync]\nauto_push = \"manual\"\n",
    );
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "config"]);

    let inv = inventory(&top).unwrap();
    let entry = inv.entries.iter().find(|e| e.path == CONFIG).unwrap();
    assert_eq!(entry.proposal, Proposal::RemoveAfterPublishing);
    let split = inv.config.unwrap();
    assert!(split.kept.is_empty(), "{:?}", split.kept);

    // The proposed settings file is accepted by the plane's own reader and
    // renders back to the same configuration.
    let parsed = adoption_plane::parse("c", &split.settings_toml().unwrap()).unwrap();
    assert_eq!(parsed.settings, split.settings);
    let table = parsed.config_table().unwrap();
    assert_eq!(table["use_agreed_blocks"].as_bool(), Some(true));
    assert_eq!(table["agents"]["codex"]["model"].as_str(), Some("m"));
    assert_eq!(table["store"]["sync"]["auto_push"].as_str(), Some("manual"));
}

#[test]
fn a_repository_without_commits_has_an_empty_inventory() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    put(&top, "AGENTS.md", "x\n");
    let inv = inventory(&top).unwrap();
    assert_eq!(inv.commit, None);
    assert!(inv.entries.is_empty());
}

#[test]
fn a_byte_identical_generated_hook_is_never_removable() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let expected = generated(&top);
    let hooks: Vec<_> = expected
        .iter()
        .filter(|(path, _)| {
            path.starts_with(".claude/hooks/aida-")
                || path.starts_with(".claude/commands/aida-")
                || path.starts_with(".codex/")
                || path.starts_with(".agents/")
                || path.starts_with(".antigravity/")
        })
        .collect();
    assert!(
        hooks
            .iter()
            .any(|(p, _)| p.starts_with(".claude/hooks/aida-")),
        "this build generates a Claude hook"
    );
    for (path, body) in &hooks {
        put(&top, path, body);
    }
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "hooks"]);

    let inv = inventory(&top).unwrap();
    assert_eq!(inv.removable().count(), 0, "{:?}", inv.entries);
    for (path, _) in &hooks {
        let entry = inv.entries.iter().find(|e| &e.path == *path).unwrap();
        assert_eq!(entry.class, Class::Generated, "{path}");
        assert_eq!(entry.proposal, Proposal::Keep, "{path}");
        assert!(entry.detail.contains("config-trust"), "{}", entry.detail);
    }
}

#[test]
fn product_files_are_not_inventoried() {
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    // AIDA's own README starter, byte for byte, is still the product's README.
    let mut scaffolder = crate::scaffolding::Scaffolder::new(
        top.clone(),
        crate::scaffolding::ScaffoldConfig::default(),
    );
    let readme = scaffolder
        .preview(&crate::models::RequirementsStore::new())
        .artifacts
        .into_iter()
        .find(|a| a.path == Path::new("README.md"))
        .expect("this build seeds a README starter");
    put(&top, "README.md", &readme.content);
    put(&top, ".gitignore", "target/\n# build output\n*.log\n");
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "product"]);

    let inv = inventory(&top).unwrap();
    assert!(inv.commit.is_some());
    assert!(inv.entries.is_empty(), "{:?}", inv.entries);
}

#[test]
fn byte_identical_generated_guidance_is_removable() {
    // trace:STORY-1741.ac676d62 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let expected = generated(&top);
    // A docs/agents guide nothing else in this tree cites.
    let (guide, body) = expected
        .iter()
        .find(|(path, _)| path.starts_with("docs/agents/") && plane_guidance(path))
        .expect("this build generates docs/agents guidance");
    put(&top, guide, body);
    put(&top, "src/main.rs", "fn main() {}\n");
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "guidance"]);

    let inv = inventory(&top).unwrap();
    let entry = inv.entries.iter().find(|e| &e.path == guide).unwrap();
    assert_eq!(entry.class, Class::Generated);
    assert_eq!(inv.removable().collect::<Vec<_>>(), vec![guide.as_str()]);
}

#[test]
fn generated_discipline_readme_is_never_removable() {
    // trace:STORY-1741.ac676d62 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let readme = ".aida/discipline/README.md";
    // The pack AIDA ships, byte for byte, whichever route writes it.
    let body = generated(&top)
        .get(readme)
        .cloned()
        .or_else(|| {
            crate::templates::EMBEDDED_TEMPLATES
                .get(readme)
                .map(|b| b.to_string())
        })
        .expect("this build ships the discipline README");
    put(&top, readme, &body);
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "discipline"]);

    let inv = inventory(&top).unwrap();
    let entry = inv.entries.iter().find(|e| e.path == readme).unwrap();
    assert_ne!(entry.proposal, Proposal::Remove, "{}", entry.detail);
    assert_eq!(inv.removable().count(), 0, "{:?}", inv.entries);
}

#[test]
fn generated_guidance_a_kept_file_cites_is_not_removable() {
    // trace:STORY-1741.ac676d62 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let top = repo(&tmp.path().join("p"));
    let expected = generated(&top);
    let (guide, body) = expected
        .iter()
        .find(|(path, _)| path.starts_with("docs/agents/") && plane_guidance(path))
        .expect("this build generates docs/agents guidance");
    put(&top, guide, body);
    put(
        &top,
        "AGENTS.md",
        &format!("# Agents\nRead `{guide}` first.\n"),
    );
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "cited guidance"]);

    let inv = inventory(&top).unwrap();
    let entry = inv.entries.iter().find(|e| &e.path == guide).unwrap();
    assert_eq!(entry.proposal, Proposal::Keep);
    assert!(entry.detail.contains("AGENTS.md"), "{}", entry.detail);
    assert_eq!(inv.removable().count(), 0, "{:?}", inv.entries);
}
