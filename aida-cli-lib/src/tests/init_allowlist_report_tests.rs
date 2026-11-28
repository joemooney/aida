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
    assert!(human.contains("nothing was written"), "{human}");
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
