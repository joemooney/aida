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
    let mut findings = Vec::new();
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
        if let Some(problem) = work_binding::unsafe_state(&meta, path) {
            findings.push(finding(
                format!("unsafe-state:{}", path.display()),
                format!("ID allocation will refuse: {problem}"),
                repair_of(&problem),
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

    match (node_toml.exists(), legacy.exists()) {
        (true, true) => {
            let node = read_toml::<aida_core::node::NodeConfig>(&node_toml);
            let state = read_toml::<DispenserState>(&legacy);
            if let (Some(node), Some(state)) = (node, state) {
                if state.mode != node.id_mode() {
                    let p = legacy.display();
                    findings.push(finding(
                        "mode-mismatch".to_string(),
                        format!(
                            "ID allocation will refuse: {p} records {} but {} says this clone is node {}",
                            work_binding::mode_label(&state.mode),
                            node_toml.display(),
                            node.node_id
                        ),
                        format!(
                            "if no IDs were dispensed under the other node (its sequences do not \
                             exceed node {n}'s highest ID), run \
                             `cp {p} {p}.bak && sed -i 's/^node_id = .*/node_id = \"{n}\"/' {p}`",
                            n = node.node_id
                        ),
                    ));
                }
            }
        }
        (false, false) => {}
        _ if authoritative => {}
        (node_exists, _) => {
            let (present, missing) = if node_exists {
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
    }
    findings
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

/// The repair clause `unsafe_state` ends with (after the last `; `).
fn repair_of(problem: &str) -> String {
    problem
        .rsplit_once("; ")
        .map(|(_, repair)| repair.to_string())
        .unwrap_or_else(|| "explicit recovery required".to_string())
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
        write(
            &d.path().join(".aida/dispenser.toml"),
            &format!(
                "[mode.Distributed]\nnode_id = \"{dispenser_node}\"\n\n[sequences]\nGLOBAL = 499\n"
            ),
            0o644,
        );
        d
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
        std::fs::write(
            d.path().join(".git/aida-allocation.toml"),
            "version = 1\nauthoritative = true\n",
        )
        .unwrap();
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
}
