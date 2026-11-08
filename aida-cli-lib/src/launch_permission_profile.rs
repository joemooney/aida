//! Runtime presentation of the scaffold's symbolic recipe. No enforcement claim.
// trace:STORY-1486 | ai:codex
use aida_core::scaffolding::launch_profile::{PROFILE_PATH, RECIPE};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// A role request is not a grant. A stakeholder parent remains read-only even when
/// requesting a development child; ordinary launch delegation still validates it.
pub(crate) fn git_policy(requested: Option<&str>, parent: Option<&str>, downgrade: bool) -> String {
    let stakeholder = [requested, parent]
        .into_iter()
        .flatten()
        .find(|seat| crate::stakeholder_role::is_stakeholder_role(&seat.to_ascii_lowercase()));
    let (mode, reason) = if downgrade {
        ("read-only", "explicit --git-read-only downgrade")
    } else if let Some(seat) = stakeholder {
        (
            "read-only",
            if seat.eq_ignore_ascii_case("guest") {
                "guest seat"
            } else {
                "requester seat"
            },
        )
    } else {
        (
            "write",
            "development baseline; ordinary seat/delegation gates still apply",
        )
    };
    format!("git_policy: {mode} ({reason}; requested seat: {}; parent seat (validated grant or restrictive stakeholder label): {})\n  declared, not enforced by current adapters; no approval exemptions\n",
        requested.unwrap_or("default"), parent.unwrap_or("none"))
}

fn git_path(root: &Path, arg: &str) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", arg])
        .output()?;
    if !output.status.success() {
        bail!(
            "cannot resolve {arg}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    PathBuf::from(String::from_utf8(output.stdout)?.trim())
        .canonicalize()
        .with_context(|| format!("cannot canonicalize {arg}"))
}

/// Canonicalize existing ancestors without creating planned build/cache paths.
fn planned(path: &Path) -> Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            return path
                .canonicalize()
                .with_context(|| format!("cannot resolve root {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot inspect launch root"),
    }
    let parent = path.parent().context("root has no existing ancestor")?;
    Ok(planned(parent)?.join(path.file_name().context("root has no name")?))
}

fn checkout(executable: &Path) -> Option<PathBuf> {
    // An installed standalone binary supplies no source-checkout identity.
    let executable = executable.canonicalize().ok()?;
    let ancestor = executable
        .ancestors()
        .skip(1)
        .find(|root| aida_core::scaffolding::is_aida_repository(root))
        .map(Path::to_path_buf);
    ancestor.or_else(|| {
        // Promoted builds retain their source location beside the executable.
        // This is discovery evidence only; repository identity is checked below.
        let meta = crate::dev_build::read_promotion_meta(executable.parent()?)?;
        let root = PathBuf::from(meta.repo?).canonicalize().ok()?;
        aida_core::scaffolding::is_aida_repository(&root).then_some(root)
    })
}

pub(crate) fn validate_recipe(project: &Path) -> Result<()> {
    let recipe_path = project.join(PROFILE_PATH);
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&recipe_path);
    match opened {
        Ok(file) => {
            const LIMIT: u64 = 16 * 1024;
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file() && metadata.len() <= LIMIT,
                "launch profile must be a regular file of at most 16 KiB"
            );
            let mut text = String::new();
            file.take(LIMIT + 1).read_to_string(&mut text)?;
            anyhow::ensure!(text.len() as u64 <= LIMIT, "launch profile exceeds 16 KiB");
            let actual: toml::Value = toml::from_str(&text).context("invalid launch profile")?;
            let expected: toml::Value = toml::from_str(RECIPE)?;
            if actual != expected {
                bail!(
                    "unsupported launch profile {}; regenerate with aida scaffold upgrade",
                    recipe_path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot read launch profile"),
    }
    Ok(())
}

pub(crate) fn render_roots(project: &Path, executable: &Path) -> Result<String> {
    validate_recipe(project)?;
    let project = project.canonicalize()?;
    let git_dir = git_path(&project, "--git-dir");
    let common = git_path(&project, "--git-common-dir");
    let mut diagnostics = String::new();
    let store = crate::store_sync::detect_distributed_store_from(&project)
        .unwrap_or_else(|| project.join(".aida-store"));
    let mut writable = vec![
        ("project", project.clone()),
        ("project-state", planned(&project.join(".aida"))?),
        ("store", planned(&store)?),
        ("cache", planned(&project.join(".aida/cache-db"))?),
        ("build", planned(&project.join("target"))?),
    ];
    match (&git_dir, &common) {
        (Ok(dir), Ok(common)) => {
            writable.push(("git-dir", dir.clone()));
            writable.push((
                "git-common-dir (shared across linked worktrees)",
                common.clone(),
            ));
        }
        _ => {
            diagnostics.push_str("git metadata unavailable; no Git administrative roots declared\n")
        }
    }
    let mut read_only = Vec::new();
    let mut executables = vec![executable.to_path_buf()];
    let mut note = "standalone installation; no AIDA checkout inferred";
    if let Some(aida) = checkout(executable) {
        // Compare actual Git common directories, never directory names or role env.
        let aida_common = git_path(&aida, "--git-common-dir");
        let same_repo = matches!((&aida_common, &common), (Ok(a), Ok(b)) if a == b);
        if aida_common.is_err() {
            diagnostics.push_str("AIDA Git identity unavailable; checkout remains read-only\n");
        }
        note = if same_repo {
            "verified AIDA repository identity"
        } else {
            "consumer repository"
        };
        if same_repo {
            writable.push(("aida-checkout", aida.clone()));
        } else {
            read_only.push(aida.clone());
        }
        for profile in ["debug", "release"] {
            executables.push(aida.join("target").join(profile).join("aida"));
        }
    }
    let overlaps = read_only
        .iter()
        .any(|read| writable.iter().any(|(_, write)| read.starts_with(write)));
    let mut out = format!("launch_profile: declared roots, not applied confinement ({note})\n");
    out.push_str(&diagnostics);
    out.push_str("writable_roots:\n");
    for (kind, path) in writable {
        out.push_str(&format!("  {kind}: {}\n", path.display()));
    }
    out.push_str("read_only_roots:\n");
    for path in read_only {
        out.push_str(&format!("  {}\n", path.display()));
    }
    if overlaps {
        out.push_str("  overlap: read-only roots exclude nested AIDA paths from writable ancestors in this declaration; current adapters do not enforce that exclusion\n");
    }
    out.push_str("executable_locations (ordinary approval rules apply):\n");
    for path in executables {
        out.push_str(&format!("  {}\n", path.display()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        assert!(crate::test_git::git_command()
            .arg("init")
            .arg(path)
            .output()
            .unwrap()
            .status
            .success());
    }
    fn aida(root: &Path) -> PathBuf {
        repo(root);
        std::fs::create_dir_all(root.join("aida-core")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"aida-core\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("aida-core/Cargo.toml"),
            "[package]\nname = \"aida-core\"\n",
        )
        .unwrap();
        let exe = root.join("target/debug/aida");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "fixture").unwrap();
        exe
    }
    // trace:STORY-1486.ac91839b | ai:codex
    #[test]
    fn consumer_profile_keeps_aida_checkout_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = aida(&tmp.path().join("tool"));
        let consumer = tmp.path().join("consumer");
        repo(&consumer);
        let output = render_roots(&consumer, &exe).unwrap();
        let read_only = output.split("read_only_roots:\n").nth(1).unwrap();
        assert!(read_only.starts_with(&format!(
            "  {}\n",
            exe.ancestors().nth(3).unwrap().display()
        )));
        assert!(output.contains("target/debug/aida"));
        assert!(output.contains("target/release/aida"));
        assert!(!consumer.join(".aida").exists());
    }
    // trace:STORY-1486.ac91839b | ai:codex
    #[test]
    fn aida_self_development_uses_verified_repo_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("arbitrary-name");
        let exe = aida(&root);
        let output = render_roots(&root, &exe).unwrap();
        assert!(output.contains("verified AIDA repository identity"));
        assert!(output.contains(&format!("aida-checkout: {}", root.display())));
        // A consumer with the same basename is still a different repository.
        let other = tmp.path().join("other/arbitrary-name");
        repo(&other);
        assert!(render_roots(&other, &exe)
            .unwrap()
            .contains("consumer repository"));
    }
    // trace:STORY-1486.ac325a99 | ai:codex
    #[test]
    fn declared_git_policy_reports_normal_guest_and_explicit_downgrade() {
        for (requested, parent, downgrade, mode, reason) in [
            (
                Some("implementer"),
                Some("advisor"),
                false,
                "write",
                "development baseline",
            ),
            (Some("guest"), None, false, "read-only", "guest seat"),
            (
                Some("requester"),
                None,
                false,
                "read-only",
                "requester seat",
            ),
            (
                Some("implementer"),
                Some("requester"),
                false,
                "read-only",
                "requester seat",
            ),
            (
                Some("implementer"),
                Some("guest"),
                false,
                "read-only",
                "guest seat",
            ),
            (
                Some("implementer"),
                Some("advisor"),
                true,
                "read-only",
                "explicit --git-read-only",
            ),
        ] {
            let output = git_policy(requested, parent, downgrade);
            assert!(output.starts_with(&format!("git_policy: {mode}")));
            assert!(output.contains(reason));
            assert!(output.contains("declared, not enforced"));
        }
    }
    #[test]
    fn git_read_only_flag_parses_before_or_after_vendor() {
        use crate::agent_launch::{AgentArgs, AgentCommand};
        for args in [
            vec![
                "aida",
                "agent",
                "new",
                "--git-read-only",
                "codex",
                "--no-exec",
                "--verbose",
            ],
            vec![
                "aida",
                "agent",
                "new",
                "claude",
                "--git-read-only",
                "--no-exec",
                "--verbose",
            ],
            vec![
                "aida",
                "agent",
                "new",
                "antigravity",
                "--git-read-only",
                "--no-exec",
                "--verbose",
            ],
        ] {
            let cli = crate::command_registry::try_parse_from(args).unwrap();
            let AgentArgs {
                inner: AgentCommand::New { git_read_only, .. },
            } = cli.command.registered::<AgentArgs>()
            else {
                panic!("expected agent new")
            };
            assert!(git_read_only);
        }
    }

    // trace:STORY-1486.acc62dfa | ai:codex
    #[test]
    fn debug_release_switch_needs_no_rescaffold() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tool");
        let debug = aida(&root);
        let release = root.join("target/release/aida");
        std::fs::create_dir_all(release.parent().unwrap()).unwrap();
        std::fs::write(&release, "fixture").unwrap();
        let consumer = tmp.path().join("consumer");
        repo(&consumer);
        std::fs::create_dir_all(consumer.join(".aida")).unwrap();
        std::fs::write(consumer.join(PROFILE_PATH), RECIPE).unwrap();
        for exe in [debug, release] {
            let output = render_roots(&consumer, &exe).unwrap();
            assert!(output.contains("target/debug/aida"));
            assert!(output.contains("target/release/aida"));
            assert!(output.contains("consumer repository"));
        }
        assert_eq!(
            std::fs::read_to_string(consumer.join(PROFILE_PATH)).unwrap(),
            RECIPE
        );
    }

    #[test]
    fn invalid_recipe_refuses_without_creating_roots() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(tmp.path().join(PROFILE_PATH), "version = 99").unwrap();
        assert!(validate_recipe(tmp.path())
            .unwrap_err()
            .to_string()
            .contains("unsupported launch profile"));
        assert!(!tmp.path().join("target").exists());
    }
    #[test]
    fn linked_aida_worktree_uses_common_git_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("primary");
        let exe = aida(&root);
        let git = |args: &[&str]| {
            let output = crate::test_git::git_command()
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["add", "Cargo.toml", "aida-core/Cargo.toml"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ]);
        let linked = tmp.path().join("linked");
        git(&["worktree", "add", "--detach", linked.to_str().unwrap()]);
        let output = render_roots(&linked, &exe).unwrap();
        assert!(output.contains("verified AIDA repository identity"));
        assert!(output.contains("git-common-dir (shared across linked worktrees)"));
        assert!(output.contains(&format!(
            "git-dir: {}",
            git_path(&linked, "--git-dir").unwrap().display()
        )));
    }

    #[test]
    fn dangling_root_symlink_is_not_a_planned_output() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("target");
        std::os::unix::fs::symlink(tmp.path().join("missing"), &path).unwrap();
        assert!(planned(&path).is_err());
    }
    #[test]
    fn promoted_binary_discovers_validated_source_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("source");
        aida(&root);
        let live = tmp.path().join("live");
        std::fs::create_dir_all(&live).unwrap();
        let exe = live.join("aida");
        std::fs::write(&exe, "fixture").unwrap();
        let meta = crate::dev_build::PromotionMeta {
            sha: "fixture".into(),
            allow_dirty: false,
            promoted_at: "fixture".into(),
            repo: Some(root.display().to_string()),
        };
        std::fs::write(
            live.join(crate::dev_build::PROMOTED_FILE),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
        let consumer = tmp.path().join("consumer");
        repo(&consumer);
        let output = render_roots(&consumer, &exe).unwrap();
        assert!(output.contains("consumer repository"), "{output}");
        assert!(output.contains(&format!("read_only_roots:\n  {}\n", root.display())));
        assert!(output.contains("target/debug/aida"));
        assert!(output.contains("target/release/aida"));
    }

    #[test]
    fn unavailable_git_metadata_keeps_preview_and_read_only_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("source");
        let exe = aida(&root);
        std::fs::remove_dir_all(root.join(".git")).unwrap();
        let consumer = tmp.path().join("consumer");
        std::fs::create_dir_all(&consumer).unwrap();
        let output = render_roots(&consumer, &exe).unwrap();
        assert!(output.contains("git metadata unavailable"));
        assert!(output.contains("AIDA Git identity unavailable"));
        assert!(output.contains(&format!("read_only_roots:\n  {}\n", root.display())));
        assert!(!output.contains("verified AIDA repository identity"));
    }

    #[test]
    fn ambient_git_routing_cannot_forge_repository_identity() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("source");
        let exe = aida(&root);
        let consumer = tmp.path().join("consumer");
        repo(&consumer);
        let git_dir = root.join(".git");
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("GIT_DIR", git_dir.to_str().unwrap()),
            ("GIT_COMMON_DIR", git_dir.to_str().unwrap()),
            ("GIT_WORK_TREE", root.to_str().unwrap()),
        ]);
        let output = render_roots(&consumer, &exe).unwrap();
        assert!(output.contains("consumer repository"), "{output}");
        assert!(!output.contains("verified AIDA repository identity"));
        assert!(output.contains(&format!("git-dir: {}", consumer.join(".git").display())));
    }
    #[test]
    fn stakeholder_preview_is_read_but_real_launch_remains_refused() {
        for vendor in ["claude", "codex", "antigravity"] {
            for seat in ["guest", "requester"] {
                let mut preview = crate::command_registry::try_parse_from([
                    "aida",
                    "agent",
                    "new",
                    vendor,
                    "--no-exec",
                    "--verbose",
                ])
                .unwrap();
                crate::stakeholder_role::enforce_stakeholder_role_capabilities_for_role(
                    &mut preview.command,
                    seat,
                )
                .unwrap();
                let mut launch = crate::command_registry::try_parse_from([
                    "aida",
                    "agent",
                    "new",
                    vendor,
                    "--verbose",
                ])
                .unwrap();
                assert!(
                    crate::stakeholder_role::enforce_stakeholder_role_capabilities_for_role(
                        &mut launch.command,
                        seat,
                    )
                    .is_err()
                );
            }
        }
    }
}
