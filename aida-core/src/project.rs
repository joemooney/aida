use anyhow::Result;
use std::env;
use std::path::PathBuf;

use crate::registry::{get_registry_path, Registry};

/// Result of checking for migration status
#[derive(Debug)]
pub enum MigrationCheck {
    /// No migration detected, use this path
    NoMigration(PathBuf),
    /// YAML was migrated to SQLite, use the SQLite path instead
    MigratedToSqlite {
        yaml_path: PathBuf,
        sqlite_path: PathBuf,
    },
    /// SQLite exists alongside YAML but no marker - potential stale data
    PossibleStaleYaml {
        yaml_path: PathBuf,
        sqlite_path: PathBuf,
    },
}

/// Check if a YAML file has been migrated to SQLite
pub fn check_migration_status(yaml_path: &PathBuf) -> MigrationCheck {
    // Check if corresponding SQLite file exists
    let sqlite_path = yaml_path.with_extension("db");

    if yaml_path.exists() && sqlite_path.exists() {
        // Both files exist - check for migration marker at the START of YAML file
        // The marker should be at the beginning, not embedded in nested content
        if let Ok(content) = std::fs::read_to_string(yaml_path) {
            // Check first few lines for the migration marker (comment + field)
            let first_lines: String = content.lines().take(5).collect::<Vec<_>>().join("\n");
            if first_lines.contains("migrated_to:") {
                return MigrationCheck::MigratedToSqlite {
                    yaml_path: yaml_path.clone(),
                    sqlite_path,
                };
            }
        }
        // Both exist but no marker at start - potential stale data situation
        return MigrationCheck::PossibleStaleYaml {
            yaml_path: yaml_path.clone(),
            sqlite_path,
        };
    }

    // Only SQLite exists - use it
    if sqlite_path.exists() && !yaml_path.exists() {
        return MigrationCheck::NoMigration(sqlite_path);
    }

    // Default: use the YAML path (or it doesn't exist yet)
    MigrationCheck::NoMigration(yaml_path.clone())
}

/// Determines the requirements file path to use based on the available information.
/// This version does NOT check for migration - use `determine_requirements_path_with_migration_check`
/// for migration-aware path resolution.
///
/// Resolution is fail-closed: a store is used only when it is local to the
/// current directory (`requirements.db` / `requirements.yaml`) or explicitly
/// named (`-p <project>` or `REQ_DB_NAME`). The global registry's
/// `default_project` and its "only one registered project" shortcut are NOT
/// implicit fallbacks any more — run from a directory outside any project,
/// they silently adopted an unrelated project's legacy store, so a write
/// landed in the wrong project with no warning.
// trace:BUG-1603 | ai:claude
pub fn determine_requirements_path(project_option: Option<&str>) -> Result<PathBuf> {
    let env_project = env::var("REQ_DB_NAME").ok();
    let registry_path = get_registry_path()?;
    // An empty `Path` keeps the local-file result relative ("requirements.db"),
    // exactly as before, while the resolver itself stays cwd-injectable.
    resolve_requirements_path_in(
        std::path::Path::new(""),
        project_option,
        env_project.as_deref(),
        &registry_path,
    )
}

/// [`determine_requirements_path`] with every ambient input injected: the
/// directory to probe for a local store, the `-p` option, the `REQ_DB_NAME`
/// value, and the registry file. Lets tests exercise the resolver against a
/// fake registry and cwd without mutating process env or `HOME`.
///
/// The registry is read only when a project is explicitly named; with no
/// local store and no explicit project this bails with setup guidance and
/// touches nothing on disk.
// trace:BUG-1603 | ai:claude
pub fn resolve_requirements_path_in(
    cwd: &std::path::Path,
    project_option: Option<&str>,
    env_project: Option<&str>,
    registry_path: &std::path::Path,
) -> Result<PathBuf> {
    let env_project = env_project.map(str::trim).filter(|s| !s.is_empty());

    // Priority 1: the `-p` option; priority 2: REQ_DB_NAME. Either one is an
    // explicit choice and suppresses the local-directory store.
    let (project_name, source) = match (project_option, env_project) {
        (Some(p), _) => (p, ""),
        (None, Some(e)) => (e, " from REQ_DB_NAME"),
        (None, None) => {
            // Prefer requirements.db (SQLite; safe for MCP/concurrent access).
            for name in ["requirements.db", "requirements.yaml"] {
                let local = cwd.join(name);
                if local.exists() {
                    return Ok(local);
                }
            }
            anyhow::bail!("{}", no_project_guidance(registry_path));
        }
    };

    if !registry_path.exists() {
        Registry::create_default(registry_path)?;
    }
    let registry = Registry::load(registry_path)?;
    match registry.get_project(project_name) {
        Some(project) => Ok(PathBuf::from(&project.path)),
        None => anyhow::bail!("Project '{project_name}'{source} not found in registry"),
    }
}

/// The refusal shown when no project is resolvable. Names the registry's
/// configured default (if any) so a user who relied on it knows the explicit
/// replacement, without ever using it implicitly.
// trace:BUG-1603 | ai:claude
fn no_project_guidance(registry_path: &std::path::Path) -> String {
    let default_hint = registry_path
        .exists()
        .then(|| Registry::load(registry_path).ok())
        .flatten()
        .and_then(|r| r.default_project.filter(|d| r.projects.contains_key(d)))
        .map(|name| {
            format!(
                "\n\n  Your project registry names `{name}` as the default project. It is \
                 not used implicitly; pass `-p {name}` (or set REQ_DB_NAME={name}) to target it."
            )
        })
        .unwrap_or_default();
    format!(
        "No AIDA project found here: there is no `.aida/config.toml` in this directory \
         or its parents, and no local requirements store.\n\n  \
         Run `aida init` to set up a project in this directory, or `cd` into an \
         existing AIDA project.\n  \
         To target a store explicitly, pass `--file <path>` or `-p <project>`, or set \
         AIDA_STORE or REQ_DB_NAME.{default_hint}\n\n  \
         (Refusing to fall back to another project's store: a write would land in \
         the wrong project.)"
    )
}

/// Lists available projects from the registry
pub fn list_available_projects() -> Result<Vec<(String, String)>> {
    let registry_path = get_registry_path()?;
    if !registry_path.exists() {
        Registry::create_default(&registry_path)?;
    }

    let registry = Registry::load(&registry_path)?;
    let mut projects = Vec::new();

    for (name, project) in &registry.projects {
        projects.push((name.clone(), project.description.clone()));
    }

    Ok(projects)
}

#[cfg(test)]
mod tests {
    //! BUG-1603: outside any project, the resolver must never adopt the
    //! registry's default (or sole) project — an unrelated legacy store.
    //! Every ambient input is injected; no env or HOME mutation.
    // trace:BUG-1603 | ai:claude
    use super::*;
    use tempfile::TempDir;

    struct Fixture {
        _tmp: TempDir,
        cwd: PathBuf,
        registry: PathBuf,
        legacy_db: PathBuf,
    }

    /// An empty cwd, plus a fake registry whose default AND only project is a
    /// legacy `requirements.db` living elsewhere.
    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("outside");
        let other = tmp.path().join("other-project");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let legacy_db = other.join("requirements.db");
        std::fs::write(&legacy_db, b"legacy-bytes").unwrap();
        let registry = tmp.path().join("home").join(".aida.config");
        std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
        std::fs::write(
            &registry,
            format!(
                "projects:\n  other:\n    path: {}\n    description: unrelated\n\
                 default_project: other\n",
                legacy_db.display()
            ),
        )
        .unwrap();
        Fixture {
            _tmp: tmp,
            cwd,
            registry,
            legacy_db,
        }
    }

    #[test]
    fn outside_any_project_refuses_instead_of_adopting_the_registry_default() {
        let f = fixture();
        let registry_before = std::fs::read(&f.registry).unwrap();
        let err = resolve_requirements_path_in(&f.cwd, None, None, &f.registry)
            .expect_err("must not resolve to the registry's default project");
        let msg = err.to_string();
        assert!(msg.contains("No AIDA project found here"), "{msg}");
        assert!(msg.contains("aida init"), "{msg}");
        // The configured default is named with its explicit replacement.
        assert!(msg.contains("-p other"), "{msg}");
        assert!(
            !msg.contains("BUG-"),
            "no internal ids in user prose: {msg}"
        );
        // Nothing written: the legacy store and the registry are untouched,
        // and nothing appeared in the cwd.
        assert_eq!(std::fs::read(&f.legacy_db).unwrap(), b"legacy-bytes");
        assert_eq!(std::fs::read(&f.registry).unwrap(), registry_before);
        assert_eq!(std::fs::read_dir(&f.cwd).unwrap().count(), 0);
    }

    #[test]
    fn a_missing_registry_is_not_created_when_nothing_is_named() {
        let tmp = TempDir::new().unwrap();
        let registry = tmp.path().join(".aida.config");
        assert!(resolve_requirements_path_in(tmp.path(), None, None, &registry).is_err());
        assert!(
            !registry.exists(),
            "a refused lookup must not write a registry"
        );
    }

    #[test]
    fn explicitly_named_project_still_resolves() {
        let f = fixture();
        let via_flag = resolve_requirements_path_in(&f.cwd, Some("other"), None, &f.registry);
        assert_eq!(via_flag.unwrap(), f.legacy_db);
        let via_env = resolve_requirements_path_in(&f.cwd, None, Some("other"), &f.registry);
        assert_eq!(via_env.unwrap(), f.legacy_db);
        let unknown = resolve_requirements_path_in(&f.cwd, None, Some("nope"), &f.registry);
        assert!(unknown
            .unwrap_err()
            .to_string()
            .contains("from REQ_DB_NAME not found"));
    }

    #[test]
    fn a_local_store_in_cwd_still_wins_when_nothing_is_named() {
        let f = fixture();
        let local = f.cwd.join("requirements.db");
        std::fs::write(&local, b"local").unwrap();
        let got = resolve_requirements_path_in(&f.cwd, None, None, &f.registry).unwrap();
        assert_eq!(got, local);
    }

    #[test]
    fn empty_req_db_name_is_treated_as_unset() {
        let f = fixture();
        assert!(resolve_requirements_path_in(&f.cwd, None, Some("  "), &f.registry).is_err());
    }
}
