//! `aida doctor` check for a clone whose node-local ID allocation would
//! refuse: allocation state the WORK reader rejects (group/other-writable,
//! foreign-owned, hard-linked, not a regular file), and a dispenser whose
//! recorded node disagrees with the clone's node binding. Either fault makes
//! every `aida add` fail; this names the file and the repair.
// trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude

use aida_core::dispenser::{DispenserState, IdMode};
use std::path::{Path, PathBuf};

/// Canonical `aida doctor` category name.
pub(crate) const CATEGORY: &str = "allocation";

/// Scan the store the project resolves to. Silent outside a git-backed
/// store and on an authoritative clone (it does not allocate node-locally).
pub(crate) fn scan(project_root: &Path) -> Vec<crate::DoctorFinding> {
    match aida_core::store_locate::detect_distributed_store_from(project_root) {
        Some(store) => scan_store(&store),
        None => Vec::new(),
    }
}

pub(crate) fn scan_store(store: &Path) -> Vec<crate::DoctorFinding> {
    use aida_core::allocation_authority as authority;
    use aida_core::work_binding;
    if !authority::in_git_clone(store).unwrap_or(false) {
        return Vec::new();
    }
    // An authoritative clone never runs the WORK reader, but its legacy
    // dispenser must still record this clone's node.
    let authoritative = authority::resolve(store).is_ok_and(|m| m.authoritative);
    let mut findings = hub_findings(store);
    let node_toml = store.join(".aida/node.toml");
    let legacy = store.join(".aida/dispenser.toml");
    let common = authority::common_dir(store).ok();
    let binding = common.as_ref().map(|c| c.join(work_binding::BINDING_FILE));
    let counter = common.as_ref().map(|c| c.join(work_binding::COUNTER_FILE));
    let bound = !authoritative && binding.as_ref().is_some_and(|b| b.exists());

    // The legacy pair is read only by the one-time migration, so it matters
    // only until the WORK binding exists.
    let mut paths: Vec<PathBuf> = Vec::new();
    if bound {
        paths.extend(binding.clone());
        paths.extend(counter.clone());
    } else {
        paths.push(node_toml.clone());
        paths.push(legacy.clone());
    }
    for path in paths.iter().filter(|_| !authoritative) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if let Some(state) = work_binding::unsafe_state(&meta, path) {
            findings.push(finding(
                format!("unsafe-state:{}", path.display()),
                format!("ID allocation will refuse: {}", state.problem),
                state.repair,
            ));
        }
    }

    if bound {
        let node = binding
            .as_deref()
            .and_then(read_toml::<work_binding::WorkBinding>)
            .map(|b| b.node_id);
        let state = counter.as_deref().and_then(read_toml::<DispenserState>);
        if let (Some(node), Some(state), Some(counter)) = (node, state, counter.as_ref()) {
            if state.mode
                != (IdMode::Distributed {
                    node_id: node.clone(),
                })
            {
                findings.push(finding(
                    "mode-mismatch".to_string(),
                    format!(
                        "ID allocation will refuse: {} records {} but this clone is bound to node {node}",
                        counter.display(),
                        work_binding::mode_label(&state.mode)
                    ),
                    "explicit recovery required: restore the WORK counter from a backup taken \
                     for this binding; never rewrite its node by hand"
                        .to_string(),
                ));
            }
        }
        return findings;
    }

    let (has_node, has_legacy) = (node_toml.exists(), legacy.exists());
    if has_legacy && (has_node || authoritative) {
        // An unregistered authoritative clone allocates as the default node.
        let node = if has_node {
            read_toml::<aida_core::node::NodeConfig>(&node_toml)
                .map(|n| (n.node_id, format!("{} says so", node_toml.display())))
        } else {
            Some((
                DEFAULT_NODE.to_string(),
                format!("the default, with no {}", node_toml.display()),
            ))
        };
        let state = read_toml::<DispenserState>(&legacy);
        if let (Some((node_id, source)), Some(state)) = (node, state) {
            if state.mode
                != (IdMode::Distributed {
                    node_id: node_id.clone(),
                })
            {
                findings.push(mismatch(&legacy, &state.mode, &node_id, &source));
            }
        }
    } else if has_node != has_legacy && !authoritative {
        let (present, missing) = if has_node {
            (&node_toml, &legacy)
        } else {
            (&legacy, &node_toml)
        };
        findings.push(finding(
            "incomplete-pair".to_string(),
            format!(
                "ID allocation will refuse: {} exists but {} is missing",
                present.display(),
                missing.display()
            ),
            "explicit recovery required: restore the missing file from a backup".to_string(),
        ));
    }
    findings
}

/// Declared-hub config errors: a remote that is both a store mirror and a
/// declared hub, and more than one hub with no enrolled root (claims are
/// refused until a root splits the number space).
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
fn hub_findings(store: &Path) -> Vec<crate::DoctorFinding> {
    let Ok(hubs) = aida_core::store_hubs::committed(store) else {
        return Vec::new();
    };
    let mut out: Vec<_> = aida_core::store_hubs::config_conflicts(store, &hubs)
        .into_iter()
        .enumerate()
        .map(|(i, problem)| {
            finding(
                format!("mirror-and-hub:{i}"),
                problem,
                "remove the remote from [store.sync] mirror_remotes, or remove its URL from [store.hubs], in .aida/config.toml".to_string(),
            )
        })
        .collect();
    if let Ok(aida_core::allocation_authority::StoreMode::MultiHubNoRoot { hubs }) =
        aida_core::allocation_authority::store_mode(store)
    {
        out.push(finding(
            "multi-hub-no-root".to_string(),
            format!(
                "block claims refuse: [store.hubs] declares {} hubs ({}) and the store has no allocation root",
                hubs.len(),
                hubs.join(", ")
            ),
            aida_core::allocation_authority::MULTI_HUB_NEEDS_ROOT.to_string(),
        ));
    }
    out
}

/// The node an unregistered clone allocates as (`rel_cards::load_node_id`).
const DEFAULT_NODE: &str = "1";

fn mismatch(legacy: &Path, recorded: &IdMode, node_id: &str, source: &str) -> crate::DoctorFinding {
    finding(
        "mode-mismatch".to_string(),
        format!(
            "ID allocation will refuse: {} records {} but this clone is node {node_id} ({source})",
            legacy.display(),
            aida_core::work_binding::mode_label(recorded),
        ),
        format!(
            "if no IDs were dispensed under the other node (its sequences do not exceed node \
             {node_id}'s highest ID), back up {} and set its `[mode.Distributed]` value to \
             `node_id = {}`",
            legacy.display(),
            aida_core::toml_quote::toml_string(node_id)
        ),
    )
}

fn finding(id: String, summary: String, action: String) -> crate::DoctorFinding {
    crate::DoctorFinding {
        category: CATEGORY.to_string(),
        id,
        summary,
        action,
        safe_heal: false,
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const NODE: &str =
        "node_id='1'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n";

    fn store(dispenser_node: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        aida_core::git_ops::init(d.path()).unwrap();
        std::fs::create_dir_all(d.path().join(".aida")).unwrap();
        write(&d.path().join(".aida/node.toml"), NODE, 0o644);
        let mut state = DispenserState::new(IdMode::Distributed {
            node_id: dispenser_node.to_string(),
        });
        state.sequences.insert("GLOBAL".to_string(), 499);
        write(
            &d.path().join(".aida/dispenser.toml"),
            &toml::to_string(&state).unwrap(),
            0o644,
        );
        d
    }

    // trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
    #[test]
    fn a_remote_that_is_mirror_and_hub_and_two_unrooted_hubs_are_reported() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        let git = |args: &[&str]| {
            assert!(aida_core::git_ops::host_git_command(p)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                .status()
                .unwrap()
                .success())
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "remote",
            "add",
            "gitlab",
            "ssh://git@gitlab.joemooney.com:2222/ai/aida.git",
        ]);
        std::fs::create_dir_all(p.join(".aida")).unwrap();
        std::fs::write(
            p.join(".aida/config.toml"),
            "[store.hubs]\ngithub = 'https://github.com/joemooney/aida.git'\ngitlab = 'https://gitlab.joemooney.com/ai/aida'\n[store.sync]\nmirror_remotes = ['gitlab']\n",
        )
        .unwrap();
        git(&["add", ".aida/config.toml"]);
        git(&["commit", "-qm", "config"]);
        let findings = hub_findings(p);
        let ids: Vec<_> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            ["mirror-and-hub:0", "multi-hub-no-root"],
            "{findings:?}"
        );
        assert!(findings[1].action.contains("aida node enroll-root"));
    }

    fn make_authoritative(d: &tempfile::TempDir) {
        std::fs::write(
            d.path().join(".git/aida-allocation.toml"),
            "version = 1\nauthoritative = true\n",
        )
        .unwrap();
    }

    fn write(path: &Path, body: &str, mode: u32) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn healthy_legacy_pair_is_silent() {
        let d = store("1");
        assert!(
            scan_store(d.path()).is_empty(),
            "{:?}",
            scan_store(d.path())
        );
    }

    #[test]
    fn group_writable_node_toml_names_the_chmod() {
        let d = store("1");
        let node = d.path().join(".aida/node.toml");
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o664)).unwrap();
        let f = scan_store(d.path());
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].category, CATEGORY);
        assert!(f[0]
            .summary
            .contains("group- or other-writable (mode 0664)"));
        assert_eq!(f[0].action, format!("run `chmod go-w {}`", node.display()));
        assert!(!f[0].summary.contains("BUG-"));
    }

    #[test]
    fn dispenser_on_another_node_names_both_and_the_repair() {
        let d = store("2");
        let f = scan_store(d.path());
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].id, "mode-mismatch");
        assert!(f[0].summary.contains("records node 2"), "{}", f[0].summary);
        assert!(f[0].summary.contains("this clone is node 1"));
        assert!(f[0].action.contains("node_id = \"1\""), "{}", f[0].action);
    }

    #[test]
    fn authoritative_clone_still_reports_a_dispenser_on_another_node() {
        let d = store("2");
        make_authoritative(&d);
        let node = d.path().join(".aida/node.toml");
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o664)).unwrap();
        let f = scan_store(d.path());
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].id, "mode-mismatch");
    }

    #[test]
    fn missing_half_of_the_legacy_pair_is_reported() {
        let d = store("1");
        std::fs::remove_file(d.path().join(".aida/dispenser.toml")).unwrap();
        let f = scan_store(d.path());
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].id, "incomplete-pair");
    }

    /// rel_cards opens the legacy dispenser as node "1" when node.toml is
    /// absent, so a dispenser on another node refuses every `aida add`.
    #[test]
    fn authoritative_clone_without_node_toml_compares_against_the_default_node() {
        let d = store("2");
        make_authoritative(&d);
        std::fs::remove_file(d.path().join(".aida/node.toml")).unwrap();
        let f = scan_store(d.path());
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].id, "mode-mismatch");
        assert!(
            f[0].summary.contains("this clone is node 1 (the default"),
            "{}",
            f[0].summary
        );
        assert!(f[0].action.contains("node_id = \"1\""), "{}", f[0].action);

        let healthy = store("1");
        make_authoritative(&healthy);
        std::fs::remove_file(healthy.path().join(".aida/node.toml")).unwrap();
        assert!(scan_store(healthy.path()).is_empty());
    }

    /// A store path with spaces or shell metacharacters stays one word in the
    /// pasted repair command.
    #[test]
    fn repair_commands_quote_the_path() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("my store; rm -rf x");
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        aida_core::git_ops::init(&root).unwrap();
        write(&root.join(".aida/node.toml"), NODE, 0o664);
        write(&root.join(".aida/dispenser.toml"), "", 0o644);
        let f = scan_store(&root);
        let unsafe_node = f
            .iter()
            .find(|f| f.id.starts_with("unsafe-state:"))
            .unwrap();
        let path = root.join(".aida/node.toml");
        assert_eq!(
            unsafe_node.action,
            format!("run `chmod go-w '{}'`", path.display())
        );
        assert_eq!(
            aida_core::work_binding::shell_quote("it's"),
            "'it'\\''s'",
            "single quotes are closed, escaped and reopened"
        );
    }

    /// Every `aida` command a refusal or repair hint names must parse.
    #[test]
    fn named_doctor_command_exists() {
        let cmd = aida_core::work_binding::DOCTOR_ALLOCATION_COMMAND;
        let args: Vec<&str> = cmd.split_whitespace().collect();
        crate::command_registry::try_parse_from(args.clone())
            .unwrap_or_else(|e| panic!("`{cmd}` does not parse: {e}"));
        assert_eq!(
            crate::doctor_root::normalize_doctor_category(args.last().unwrap()).unwrap(),
            CATEGORY
        );
    }
}
