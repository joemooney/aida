// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
use super::*;

#[test]
fn this_repositorys_real_url_shapes_normalize_to_one_hub_each() {
    // GitHub over https and over scp-style ssh.
    assert_eq!(
        normalize_url("https://github.com/joemooney/aida.git"),
        "github.com/joemooney/aida"
    );
    assert_eq!(
        normalize_url("git@github.com:joemooney/aida.git"),
        "github.com/joemooney/aida"
    );
    // GitLab over ssh with a user and a port, and over https.
    assert_eq!(
        normalize_url("ssh://git@gitlab.joemooney.com:2222/ai/aida.git"),
        "gitlab.joemooney.com/ai/aida"
    );
    assert_eq!(
        normalize_url("https://gitlab.joemooney.com/ai/aida/"),
        "gitlab.joemooney.com/ai/aida"
    );
    assert_eq!(normalize_url("HTTPS://GitHub.com/x/y"), "github.com/x/y");
}

#[test]
fn local_paths_keep_their_path() {
    assert_eq!(normalize_url("/srv/hub.git"), "/srv/hub");
    assert_eq!(normalize_url("file:///srv/hub.git"), "/srv/hub");
    assert_eq!(normalize_url("/srv/hub/"), "/srv/hub");
}

#[test]
fn parse_reads_hubs_and_mirrors() {
    let set = HubSet::parse(
        r#"
[store]
mode = "distributed"
[store.hubs]
github = { urls = ["https://github.com/joemooney/aida.git", "git@github.com:joemooney/aida.git"] }
gitlab = "ssh://git@gitlab.joemooney.com:2222/ai/aida.git"
[store.sync]
mirror_remotes = ["mirror"]
"#,
    )
    .unwrap();
    assert_eq!(set.hubs.len(), 2);
    assert_eq!(
        set.get("github").unwrap().urls,
        ["github.com/joemooney/aida"; 2]
    );
    assert_eq!(
        set.hub_for_url("ssh://git@gitlab.joemooney.com/ai/aida")
            .map(|h| h.name.as_str()),
        Some("gitlab")
    );
    assert_eq!(set.mirror_remotes, ["mirror"]);
}

#[test]
fn parse_refuses_bad_hub_names_and_empty_hubs() {
    assert!(HubSet::parse("[store.hubs]\n\"a:b\" = \"x\"\n").is_err());
    assert!(HubSet::parse("[store.hubs]\na = { urls = [] }\n").is_err());
    assert_eq!(HubSet::parse("").unwrap(), HubSet::default());
}

#[test]
fn hub_pool_ids_round_trip() {
    let id = hub_pool_id("github", "bug");
    assert_eq!(id, "hub:github:BUG");
    assert_eq!(parse_hub_pool_id(&id), Some(("github", "BUG")));
    assert_eq!(parse_hub_pool_id("p-1"), None);
    assert_eq!(parse_hub_pool_id("hub:a:b:c"), None);
}

#[test]
fn a_mirror_remote_that_is_also_a_hub_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    crate::git_ops::init(repo).unwrap();
    for (name, url) in [
        ("origin", "https://github.com/joemooney/aida.git"),
        ("gitlab", "ssh://git@gitlab.joemooney.com:2222/ai/aida.git"),
    ] {
        assert!(crate::git_ops::host_git_command(repo)
            .args(["remote", "add", name, url])
            .status()
            .unwrap()
            .success());
    }
    let set = HubSet::parse(
        "[store.hubs]\ngithub = \"github.com/joemooney/aida\"\ngitlab = \"gitlab.joemooney.com/ai/aida\"\n[store.sync]\nmirror_remotes = [\"gitlab\"]\n",
    )
    .unwrap();
    let conflicts = config_conflicts(repo, &set);
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert!(conflicts[0].contains("`gitlab`"), "{conflicts:?}");
    // The mirror is never the remote that reaches a hub.
    assert_eq!(
        remote_reaching(repo, &set, set.get("gitlab").unwrap()),
        None
    );
    assert_eq!(
        remote_reaching(repo, &set, set.get("github").unwrap()).as_deref(),
        Some("origin")
    );
}
