//! `claim_authority`: one case per row of the single-hub sketch's table.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
use super::*;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::git_ops::host_git_command(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A clone on branch `main` whose committed `.aida/config.toml` carries
/// `config`, with `remotes` configured.
fn clone_with(config: &str, remotes: &[(&str, &str)], authoritative: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(p.join(".aida")).unwrap();
    std::fs::write(p.join(".aida/config.toml"), config).unwrap();
    git(p, &["add", ".aida/config.toml"]);
    git(p, &["commit", "-qm", "config"]);
    for (name, url) in remotes {
        git(p, &["remote", "add", name, url]);
    }
    if authoritative {
        write_mode(p, AllocationRole::Root, None).unwrap();
    }
    dir
}

const ONE_HUB: &str = "[store.hubs]\ngithub = \"https://github.com/joemooney/aida.git\"\n";
const TWO_HUBS: &str = "[store.hubs]\ngithub = \"https://github.com/joemooney/aida.git\"\ngitlab = \"ssh://git@gitlab.joemooney.com:2222/ai/aida.git\"\n";

#[test]
fn one_hub_reached_by_a_remote_is_hub_cas_on_that_remote() {
    // This clone names the hub `upstream`, reaching it over scp-style ssh.
    let dir = clone_with(
        ONE_HUB,
        &[("upstream", "git@github.com:joemooney/aida.git")],
        false,
    );
    let authority = claim_authority(dir.path()).unwrap();
    assert_eq!(
        authority,
        ClaimAuthority::HubCas {
            remote: "upstream".into(),
            hub: "github".into()
        }
    );
    assert_eq!(authority.remote(), "upstream");
    assert_eq!(authority.pool_id("bug").as_deref(), Some("hub:github:BUG"));
}

#[test]
fn one_hub_no_matching_remote_refuses_naming_the_hub() {
    let dir = clone_with(
        ONE_HUB,
        &[("origin", "https://example.com/other.git")],
        false,
    );
    let err = claim_authority(dir.path()).unwrap_err().to_string();
    assert!(err.contains("hub `github`"), "{err}");
    assert!(err.contains("github.com/joemooney/aida"), "{err}");
}

#[test]
fn two_hubs_without_a_root_refuse_naming_enroll_root() {
    let dir = clone_with(
        TWO_HUBS,
        &[("origin", "https://github.com/joemooney/aida.git")],
        true,
    );
    let err = claim_authority(dir.path()).unwrap_err().to_string();
    assert!(err.contains("aida node enroll-root"), "{err}");
    assert!(err.contains("github, gitlab"), "{err}");
}

#[test]
fn a_root_record_keeps_rooted_mode_whatever_the_hub_count() {
    let dir = clone_with(TWO_HUBS, &[], true);
    std::fs::create_dir_all(dir.path().join("registry")).unwrap();
    std::fs::write(
        dir.path().join(crate::allocation_lineage::ROOT_YAML_REL),
        "holder_node: '7'\nepoch: 1\n",
    )
    .unwrap();
    assert_eq!(claim_authority(dir.path()).unwrap(), ClaimAuthority::Rooted);
    // A non-root clone of a rooted store still may not claim.
    std::fs::remove_file(dir.path().join(".git").join(CONFIG_FILE)).unwrap();
    assert!(claim_authority(dir.path()).is_err());
}

#[test]
fn no_hub_declared_keeps_the_legacy_root_and_refuses_others() {
    let root = clone_with("", &[], true);
    assert_eq!(
        claim_authority(root.path()).unwrap(),
        ClaimAuthority::Rooted
    );
    let node = clone_with("", &[], false);
    let err = claim_authority(node.path()).unwrap_err().to_string();
    assert!(err.contains("[store.hubs]"), "{err}");
}

#[test]
fn an_uncommitted_hub_declaration_does_not_count() {
    let dir = clone_with(
        "",
        &[("origin", "https://github.com/joemooney/aida.git")],
        false,
    );
    std::fs::write(dir.path().join(".aida/config.toml"), ONE_HUB).unwrap();
    assert_eq!(store_mode(dir.path()).unwrap(), StoreMode::Undeclared);
    assert!(claim_authority(dir.path()).is_err());
    git(dir.path(), &["commit", "-qam", "declare the hub"]);
    assert!(matches!(
        claim_authority(dir.path()).unwrap(),
        ClaimAuthority::HubCas { .. }
    ));
}

#[test]
fn a_mirror_remote_never_reaches_a_hub() {
    let config = format!("{ONE_HUB}[store.sync]\nmirror_remotes = [\"origin\"]\n");
    let dir = clone_with(
        &config,
        &[("origin", "https://github.com/joemooney/aida.git")],
        false,
    );
    assert!(claim_authority(dir.path()).is_err());
}
