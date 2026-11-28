//! `aida init --report-allowlists`: detection and the per-agent report.
//! trace:STORY-2838 | ai:claude

use super::*;

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(status.status.success(), "git {args:?} failed: {status:?}");
}

fn claude_and_codex() -> AgentSelection {
    AgentSelection {
        claude: true,
        codex: true,
        antigravity: false,
    }
}

fn setting<'a>(report: &'a AllowlistReport, agent: &str, key: &str) -> &'a SettingEntry {
    report
        .agents
        .iter()
        .find(|a| a.agent == agent)
        .unwrap_or_else(|| panic!("agent {agent} reported"))
        .settings
        .iter()
        .find(|s| s.key == key)
        .unwrap_or_else(|| panic!("{agent} {key} reported"))
}

const GITHUB_HOSTS: [&str; 4] = [
    "api.github.com",
    "codeload.github.com",
    "github.com",
    "objects.githubusercontent.com",
];

/// Any token shaped like an internal spec ID (`ABC-123`).
fn has_spec_id(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .any(|tok| {
            tok.split_once('-').is_some_and(|(prefix, num)| {
                prefix.len() >= 2
                    && prefix.bytes().all(|b| b.is_ascii_uppercase())
                    && !num.is_empty()
                    && num.bytes().all(|b| b.is_ascii_digit())
            })
        })
}

#[test]
fn github_remote_with_claude_and_codex_names_github_hosts_per_agent() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widget.git",
        ],
    );
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "mirror",
            "git@GitLab.Example.com:acme/widget.git",
        ],
    );

    let remotes = read_git_remotes(dir.path());
    let report = build_allowlist_report(claude_and_codex(), forge_hosts_from_remotes(&remotes));

    let hosts: Vec<&str> = report.forge_hosts.iter().map(|f| f.host.as_str()).collect();
    let mut expected: Vec<&str> = GITHUB_HOSTS.to_vec();
    expected.push("gitlab.example.com");
    expected.sort();
    assert_eq!(hosts, expected);
    let gh = report
        .forge_hosts
        .iter()
        .find(|f| f.host == "github.com")
        .unwrap();
    assert_eq!(gh.remotes, vec!["origin".to_string()]);

    let claude = setting(&report, "claude", "sandbox.network.allowedDomains");
    assert_eq!(claude.file, ".claude/settings.json");
    let codex = setting(&report, "codex", "contained.allowed_hosts");
    for host in GITHUB_HOSTS {
        assert!(
            claude.entries.iter().any(|h| h == host),
            "claude lacks {host}"
        );
        assert!(
            codex.entries.iter().any(|h| h == host),
            "codex lacks {host}"
        );
    }
    assert_eq!(
        setting(&report, "codex", "contained.vendors.codex").entries,
        vec!["native".to_string()]
    );
    assert!(report.agents.iter().all(|a| a.agent != "antigravity"));
    assert!(report.dry_run);

    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["dry_run"], serde_json::json!(true));
    assert_eq!(json["agents"][0]["agent"], "claude");

    let human = render_allowlist_report(&report);
    assert!(human.contains("github.com  (origin)"), "{human}");
    assert!(human.contains("would set"), "{human}");
    assert!(!has_spec_id(&human), "internal ID in user text: {human}");
}

#[test]
fn no_remote_reports_no_forge_hosts() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);

    let report = build_allowlist_report(
        claude_and_codex(),
        forge_hosts_from_remotes(&read_git_remotes(dir.path())),
    );
    assert!(report.forge_hosts.is_empty());
    assert!(setting(&report, "claude", "sandbox.network.allowedDomains")
        .entries
        .is_empty());
    assert!(setting(&report, "codex", "contained.allowed_hosts")
        .entries
        .is_empty());
    let human = render_allowlist_report(&report);
    assert!(human.contains("none detected"), "{human}");
    assert!(!has_spec_id(&human), "internal ID in user text: {human}");
}

#[test]
fn local_and_malformed_remotes_contribute_no_hosts() {
    let remotes: Vec<(String, String)> = [
        ("local", "/srv/git/widget.git"),
        ("rel", "../widget"),
        ("file", "file:///srv/git/widget.git"),
        ("ip", "ssh://git@10.0.0.5/widget.git"),
        ("bare", "ssh://localhost/widget.git"),
        ("port", "ssh://git@git.example.org:2222/widget.git"),
    ]
    .iter()
    .map(|(n, u)| (n.to_string(), u.to_string()))
    .collect();
    let hosts = forge_hosts_from_remotes(&remotes);
    assert_eq!(
        hosts,
        vec![ForgeHost {
            host: "git.example.org".to_string(),
            remotes: vec!["port".to_string()],
        }]
    );
}

#[test]
fn spec_id_detector_flags_ids_only() {
    assert!(has_spec_id("see STORY-12 now"));
    assert!(!has_spec_id(
        "sandbox.network.allowedDomains api.github.com"
    ));
}

const USER_CONFIG: &str = "# my config\n[contained]\nallowed_hosts = [\"crates.io\"] # mine\n";
const USER_SETTINGS: &str =
    "{\"model\": \"x\", \"sandbox\": {\"network\": {\"allowedDomains\": [\"example.org\"]}}}\n";

/// A github-remote repo with a user's own config and Claude settings.
fn seeded_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["remote", "add", "origin", "git@github.com:acme/widget.git"],
    );
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
    std::fs::write(dir.path().join(".aida/config.toml"), USER_CONFIG).unwrap();
    std::fs::write(dir.path().join(".claude/settings.json"), USER_SETTINGS).unwrap();
    dir
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap()
}

#[test]
fn write_adds_reported_entries_keeps_user_entries_and_is_idempotent() {
    let dir = seeded_repo();
    let root = dir.path();
    run_report_allowlists(root, Some("claude,codex"), false, true).unwrap();

    let settings: serde_json::Value =
        serde_json::from_str(&read(root, ".claude/settings.json")).unwrap();
    let domains = settings["sandbox"]["network"]["allowedDomains"]
        .as_array()
        .unwrap();
    assert_eq!(domains[0], "example.org", "user domain kept first");
    assert_eq!(settings["model"], "x", "other keys kept");
    let config = read(root, ".aida/config.toml");
    assert!(config.contains("# my config") && config.contains("# mine"));
    let parsed: toml::Value = toml::from_str(&config).unwrap();
    let hosts = parsed["contained"]["allowed_hosts"].as_array().unwrap();
    assert_eq!(hosts[0].as_str(), Some("crates.io"), "user host kept first");
    for host in GITHUB_HOSTS {
        assert!(domains.iter().any(|d| d == host), "claude lacks {host}");
        assert!(hosts.iter().any(|h| h.as_str() == Some(host)), "no {host}");
    }
    assert_eq!(hosts.len(), 1 + GITHUB_HOSTS.len(), "no duplicates");
    assert_eq!(
        parsed["contained"]["vendors"]["codex"].as_str(),
        Some("native")
    );

    // A second run plans nothing and leaves both files byte-identical.
    let (before_cfg, before_json) = (config, read(root, ".claude/settings.json"));
    let report = build_allowlist_report(
        claude_and_codex(),
        forge_hosts_from_remotes(&read_git_remotes(root)),
    );
    assert!(plan_allowlist_writes(root, &report).unwrap().is_empty());
    run_report_allowlists(root, Some("claude,codex"), false, true).unwrap();
    assert_eq!(read(root, ".aida/config.toml"), before_cfg);
    assert_eq!(read(root, ".claude/settings.json"), before_json);
}

#[test]
fn without_write_no_file_changes() {
    let dir = seeded_repo();
    let root = dir.path();
    let report = build_allowlist_report(
        claude_and_codex(),
        forge_hosts_from_remotes(&read_git_remotes(root)),
    );
    let edits = plan_allowlist_writes(root, &report).unwrap();
    assert!(!edits.is_empty(), "there is something to write");
    let diff = render_planned_edits(&edits);
    assert!(diff.contains("+ api.github.com"), "{diff}");
    assert!(!has_spec_id(&diff), "internal ID in user text: {diff}");
    // JSON output never prompts, so this is the non-interactive report path.
    run_report_allowlists(root, Some("claude,codex"), true, false).unwrap();
    assert_eq!(read(root, ".aida/config.toml"), USER_CONFIG);
    assert_eq!(read(root, ".claude/settings.json"), USER_SETTINGS);
}

#[test]
fn write_keeps_a_user_set_vendor_posture_and_skips_unselected_agents() {
    let dir = seeded_repo();
    let root = dir.path();
    let cfg = format!("{USER_CONFIG}\n[contained.vendors]\ncodex = \"aida\"\n");
    std::fs::write(root.join(".aida/config.toml"), &cfg).unwrap();
    run_report_allowlists(root, Some("antigravity,codex"), false, true).unwrap();
    let parsed: toml::Value = toml::from_str(&read(root, ".aida/config.toml")).unwrap();
    assert_eq!(
        parsed["contained"]["vendors"]["codex"].as_str(),
        Some("aida")
    );
    assert!(parsed["contained"]["allowed_hosts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h.as_str() == Some("github.com")));
    // Claude is not selected: its settings file is untouched.
    assert_eq!(read(root, ".claude/settings.json"), USER_SETTINGS);
}

#[test]
fn a_dir_nested_in_an_enclosing_repo_reports_no_remotes() {
    let dir = seeded_repo();
    let nested = dir.path().join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(read_git_remotes(&nested).is_empty());
    assert!(!read_git_remotes(dir.path()).is_empty());
}

#[test]
fn write_refuses_before_any_change_when_a_target_is_unusable() {
    let dir = seeded_repo();
    let root = dir.path();
    std::fs::remove_file(root.join(".aida/config.toml")).unwrap();
    let err = run_report_allowlists(root, Some("claude"), false, true).unwrap_err();
    assert!(err.to_string().contains("aida init"), "{err}");
    assert_eq!(read(root, ".claude/settings.json"), USER_SETTINGS);
    assert!(!root.join(".aida/config.toml").exists());
}

#[cfg(unix)]
#[test]
fn write_refuses_a_linked_settings_folder_that_leads_outside() {
    let dir = seeded_repo();
    let root = dir.path();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("settings.json"), USER_SETTINGS).unwrap();
    std::fs::remove_dir_all(root.join(".claude")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join(".claude")).unwrap();
    let err = run_report_allowlists(root, Some("claude"), false, true).unwrap_err();
    assert!(err.to_string().contains("linked folder"), "{err}");
    assert!(
        !has_spec_id(&err.to_string()),
        "internal ID in user text: {err}"
    );
    assert_eq!(read(outside.path(), "settings.json"), USER_SETTINGS);
    assert_eq!(
        read(root, ".aida/config.toml"),
        USER_CONFIG,
        "refused before any change"
    );
}

#[cfg(unix)]
#[test]
fn write_keeps_the_users_key_order_and_replaces_the_file_atomically() {
    use std::os::unix::fs::MetadataExt;
    let dir = seeded_repo();
    let root = dir.path();
    let body = "{\"zeta\": 1, \"sandbox\": {\"network\": {\"allowedDomains\": []}, \
                \"enabled\": true}, \"alpha\": [{\"z\": 1, \"a\": 2}]}\n";
    std::fs::write(root.join(".claude/settings.json"), body).unwrap();
    let before = std::fs::metadata(root.join(".claude/settings.json"))
        .unwrap()
        .ino();
    run_report_allowlists(root, Some("claude"), false, true).unwrap();
    let after = read(root, ".claude/settings.json");
    let pos = |needle: &str| {
        after
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {after}"))
    };
    assert!(pos("\"zeta\"") < pos("\"sandbox\"") && pos("\"sandbox\"") < pos("\"alpha\""));
    assert!(pos("\"network\"") < pos("\"enabled\"") && pos("\"z\"") < pos("\"a\""));
    assert!(after.contains("api.github.com"), "{after}");
    let ino = std::fs::metadata(root.join(".claude/settings.json"))
        .unwrap()
        .ino();
    assert_ne!(before, ino, "replaced by rename, not rewritten in place");
    let names: Vec<_> = std::fs::read_dir(root.join(".claude")).unwrap().collect();
    assert_eq!(names.len(), 1, "no staging file left behind");
}

#[cfg(unix)]
#[test]
fn write_aborts_on_an_unreadable_settings_file_and_leaves_it_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let dir = seeded_repo();
    let path = dir.path().join(".claude/settings.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads a 0000 file anyway, so there is no read error to provoke.
    if std::fs::read(&path).is_err() {
        let err = run_report_allowlists(dir.path(), Some("claude"), false, true).unwrap_err();
        assert!(
            format!("{err:#}").contains("nothing was written"),
            "{err:#}"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o000, "file not replaced");
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read(dir.path(), ".claude/settings.json"), USER_SETTINGS);
}

#[cfg(unix)]
#[test]
fn write_keeps_the_settings_mode_and_creates_a_new_file_0600() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = seeded_repo();
    let path = dir.path().join(".claude/settings.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let uid = std::fs::metadata(&path).unwrap().uid();
    run_report_allowlists(dir.path(), Some("claude"), false, true).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    assert!(read(dir.path(), ".claude/settings.json").contains("api.github.com"));
    assert_eq!(meta.mode() & 0o7777, 0o600, "0600 kept, not widened");
    assert_eq!(meta.uid(), uid, "owner kept");

    std::fs::remove_file(&path).unwrap();
    run_report_allowlists(dir.path(), Some("claude"), false, true).unwrap();
    let mode = std::fs::metadata(&path).unwrap().mode();
    assert_eq!(mode & 0o7777, 0o600, "new settings file is owner-only");
}
