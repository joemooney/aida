//! TASK-2067: a trace ID that names a renumbered spec resolves to the spec's
//! new stem whatever the user's rename configuration, and linking the new
//! stem only adds versions to judge (advisor conditions C1 and C2).
// trace:TASK-2067 | ai:claude
use super::*;
use crate::mandate::{judge_exclusion_spec, store_history_alias_map, MandateRefusal};

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn object(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    root.join(aida_core::object_store::relative_object_path(id).unwrap())
}

fn write(root: &std::path::Path, id: &str, agreed: Option<&str>) {
    let path = object(root, id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut req = aida_core::Requirement::new("renumbered".into(), "a long enough body".into());
    req.req_type = aida_core::RequirementType::Bug;
    req.execution_mode = Some(aida_core::ExecutionMode::Drive);
    req.spec_id = Some(id.into());
    req.agreed_id = agreed.map(Into::into);
    std::fs::write(path, serde_yaml::to_string(&req).unwrap()).unwrap();
}

/// A store whose `BUG-1-040` and `BUG-1-051` were renumbered, in one commit,
/// to `BUG-26` and `BUG-21` (the frozen store's shape). The renumbered
/// `BUG-26` then gained the `security` tag; the old stems never carried it.
fn renumbered_store(config: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    for (key, value) in config {
        git(root, &["config", key, value]);
    }
    write(root, "BUG-1-040", Some("BUG-26"));
    write(root, "BUG-1-051", Some("BUG-21"));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "v1"]);
    // Renumbering moves the object and rewrites only its ID lines.
    for (old, new) in [("BUG-1-040", "BUG-26"), ("BUG-1-051", "BUG-21")] {
        let body = std::fs::read_to_string(object(root, old)).unwrap();
        let body: String = body
            .lines()
            .filter(|line| !line.starts_with("agreed_id:"))
            .map(|line| match line.starts_with("spec_id:") {
                true => format!("spec_id: {new}\n"),
                false => format!("{line}\n"),
            })
            .collect();
        std::fs::remove_file(object(root, old)).unwrap();
        let path = object(root, new);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "renumber"]);
    let path = object(root, "BUG-26");
    let body = std::fs::read_to_string(&path).unwrap();
    let mut req: aida_core::Requirement = serde_yaml::from_str(&body).unwrap();
    req.tags.insert("security".into());
    std::fs::write(&path, serde_yaml::to_string(&req).unwrap()).unwrap();
    git(root, &["commit", "-qam", "tag"]);
    tmp
}

const CONFIGS: &[&[(&str, &str)]] = &[
    &[],
    &[("diff.renames", "false")],
    &[("diff.renames", "copies")],
    &[("diff.renameLimit", "1")],
];

#[test]
fn a_renumbered_stem_is_found_whatever_the_rename_config() {
    for config in CONFIGS {
        let store = renumbered_store(config);
        let ids = vec!["BUG-1-040".to_string(), "BUG-1-051".to_string()];
        let map = store_history_alias_map(store.path(), &ids).unwrap();
        assert_eq!(map["BUG-1-040"], vec!["BUG-26".to_string()], "{config:?}");
        assert_eq!(map["BUG-1-051"], vec!["BUG-21".to_string()], "{config:?}");
    }
}

/// The trace-derived old ID is judged by the renumbered spec's versions, so
/// the `security` tag added after the renumber still refuses it.
#[test]
fn a_renumbered_trace_id_is_judged_by_its_new_stem() {
    for config in CONFIGS {
        let store = renumbered_store(config);
        let specs =
            resolve_exclusion_specs(None, store.path(), &[], ["BUG-1-040".to_string()].into());
        let [spec] = &specs[..] else {
            panic!("{specs:?}")
        };
        assert_eq!(spec.aliases, vec!["BUG-26".to_string()], "{config:?}");
        let refusal = judge_exclusion_spec(spec, &Default::default(), false);
        assert!(
            matches!(refusal, Err(MandateRefusal::Excluded { ref spec, .. }) if spec == "BUG-1-040"),
            "{config:?}: {refusal:?}"
        );
    }
}

/// Advisor C2: an unreadable version of the renamed-to stem refuses the old
/// ID as unreadable history; linking never turns it into a dangling ID.
#[test]
fn an_unreadable_version_of_the_renamed_to_stem_refuses() {
    for config in CONFIGS {
        let store = renumbered_store(config);
        let root = store.path();
        std::fs::write(object(root, "BUG-26"), "{ not: [yaml\n").unwrap();
        git(root, &["commit", "-qam", "corrupt"]);
        let specs = resolve_exclusion_specs(None, root, &[], ["BUG-1-040".to_string()].into());
        let refusal = judge_exclusion_spec(&specs[0], &Default::default(), true);
        assert!(
            matches!(refusal, Err(MandateRefusal::HistoryUnreadable { ref spec, .. }) if spec == "BUG-1-040"),
            "{config:?}: {refusal:?}"
        );
    }
}

/// Comment 24 holds on the side a line really came from: a removed line is
/// read at the rename's OLD path, so an old path that is not canonical
/// refuses even though the new one is.
#[test]
fn a_removed_line_at_a_non_canonical_old_path_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, "BUG-26", Some("BUG-1-040"));
    let stray = root.join("objects/BUG/BUG-1-040.yaml");
    std::fs::rename(object(root, "BUG-26"), &stray).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "flat layout"]);
    std::fs::remove_file(&stray).unwrap();
    write(root, "BUG-26", None);
    git(root, &["add", "-A"]);
    git(
        root,
        &["commit", "-qm", "drop the agreed id while moving it"],
    );
    let err = store_history_alias_map(root, &["BUG-1-040".to_string()]).unwrap_err();
    assert!(err.contains("outside a canonical object path"), "{err}");
    assert!(err.contains("objects/BUG/BUG-1-040.yaml"), "{err}");
}
