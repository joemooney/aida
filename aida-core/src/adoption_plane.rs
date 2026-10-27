//! The shared AIDA adoption plane: non-executable project settings and
//! guidance kept in an `adoption/` namespace on the orphan `aida-store`
//! branch, so adopting AIDA never has to write to a product branch.
//!
//! Discovery starts from filesystem Git identity only. It never reads the
//! project's tracked `.aida` configuration (that file is what adoption keeps
//! off the product branch, so it cannot be the way to find the plane), never
//! walks past the nearest repository, and never adopts a temp root. A clone
//! that keeps its plane somewhere other than its own `aida-store` branch says
//! so in a clone-local locator inside the Git common directory, which no
//! branch can track.
//!
//! Every read pins the selected ref to one commit and reads that commit's
//! objects, so a ref that moves mid-read can never mix two revisions, and a
//! caller (CI) can demand the exact revision it reviewed with [`verify`].
//!
//! The settings allowlist is the existing BUG-1723 classification: only keys
//! that `scripts/config-trust.toml` already marks `policy` for the
//! branch-local copy may appear. A `[settings.<section>]` name is the
//! manifest's `[keys.worktree.<section>]` name verbatim: `_top` holds
//! top-level config keys and a dotted name (`"store.sync"`) is that nested
//! config table, so [`Adoption::config_table`] maps every admitted key to
//! exactly one config location. Executable, credential and location keys
//! keep coming from the trusted default-branch copy or `~/.aida`; the plane
//! adds no new trust anchor.
//!
//! Nothing here writes. Publishing plane content is a store write and goes
//! through the same prepared outbound path as any other store change.
// trace:STORY-1741 | ai:claude

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Clone-local locator file in the Git common directory.
pub const LOCATOR_FILE: &str = "aida-adoption-locator.toml";
/// The conventional plane: this clone's orphan store branch.
pub const STORE_REF: &str = "refs/heads/aida-store";
/// Where the settings live inside a plane commit.
pub const SETTINGS_PATH: &str = "adoption/settings.toml";
/// The one settings schema this build reads.
pub const SCHEMA: u32 = 1;
/// The reader/generator identity a revision is stamped with.
pub const GENERATOR: &str = concat!("aida/", env!("CARGO_PKG_VERSION"));

const MAX_SETTINGS_BYTES: u64 = 64 * 1024;
const TRUST_MANIFEST: &str = include_str!("../../scripts/config-trust.toml");

/// How a plane was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// This clone's own `aida-store` branch, or the one remote copy of it.
    Conventional { reference: String },
    /// A ref in this clone named by the locator.
    LocatorRef { reference: String },
    /// The `aida-store` branch of a separate repository named by the locator.
    Sibling { store: PathBuf },
}

/// A located plane, pinned to one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    /// The repository whose objects hold the plane.
    pub repo: PathBuf,
    pub source: Source,
    pub commit: String,
}

/// The exact revision a set of settings was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub commit: String,
    pub schema: u32,
    pub generator: String,
}

/// Inert settings and guidance read from one plane revision.
#[derive(Debug, Clone, PartialEq)]
pub struct Adoption {
    pub revision: Revision,
    /// `section -> key -> value`, every key a `policy` row of the manifest.
    pub settings: BTreeMap<String, BTreeMap<String, toml::Value>>,
    /// Guidance documents by name.
    pub guidance: BTreeMap<String, String>,
}

impl Adoption {
    /// The settings in the `.aida/config.toml` layout the existing readers
    /// parse. Refuses a section whose name collides with a scalar key.
    pub fn config_table(&self) -> Result<toml::Table> {
        let mut root = toml::Table::new();
        for (section, keys) in &self.settings {
            let mut table = &mut root;
            if section != "_top" {
                for part in section.split('.') {
                    let slot = table
                        .entry(part.to_string())
                        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                    table = slot.as_table_mut().with_context(|| {
                        format!("Adoption section {section} collides with a setting; refused")
                    })?;
                }
            }
            for (key, value) in keys {
                if table.insert(key.clone(), value.clone()).is_some() {
                    bail!("Adoption setting {section}.{key} collides with a section; refused");
                }
            }
        }
        Ok(root)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Locator {
    version: u32,
    #[serde(rename = "ref")]
    reference: Option<String>,
    store: Option<PathBuf>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    schema: u32,
    #[serde(default)]
    settings: toml::Table,
    #[serde(default)]
    guidance: BTreeMap<String, String>,
}

/// Locate the plane for the repository holding `start`. `Ok(None)` means
/// this clone has no plane (not adopted); every uncertain case refuses.
pub fn discover(start: &Path) -> Result<Option<Plane>> {
    discover_with_roots(start, &crate::store_locate::real_temp_roots())
}

fn discover_with_roots(start: &Path, temp_roots: &[PathBuf]) -> Result<Option<Plane>> {
    let Some(top) = nearest_repo(start)? else {
        return Ok(None);
    };
    let roots = crate::store_locate::canonicalize_roots(temp_roots);
    if crate::store_locate::is_in_canonical_roots(&top, &roots) {
        return Ok(None);
    }
    let common = crate::allocation_authority::common_dir(&top)?;
    let plane = match read_locator(&common)? {
        Some(Locator {
            reference: Some(reference),
            store: None,
            ..
        }) => {
            check_ref_name(&reference)?;
            let commit = resolve_commit(&top, &reference)?
                .with_context(|| format!("Adoption locator names {reference}, which does not resolve to a commit; refused"))?;
            Some(Plane {
                repo: top.clone(),
                source: Source::LocatorRef { reference },
                commit,
            })
        }
        Some(Locator {
            reference: None,
            store: Some(store),
            ..
        }) => Some(sibling(&common, &store)?),
        Some(_) => bail!(
            "Adoption locator must name exactly one of `ref` or `store`; refused. \
             Fix or remove {LOCATOR_FILE} in the Git common directory."
        ),
        None => conventional(&top)?,
    };
    if let Some(plane) = &plane {
        ensure_off_product_history(&top, plane)?;
    }
    Ok(plane)
}

/// The plane is an orphan: it shares no commit with the product's `HEAD`.
/// A locator naming a product branch (or a store grown out of product
/// history) refuses rather than reading settings off a code branch.
fn ensure_off_product_history(top: &Path, plane: &Plane) -> Result<()> {
    if resolve_commit(top, "HEAD")?.is_none() || resolve_commit(top, &plane.commit)?.is_none() {
        return Ok(());
    }
    let out = crate::git_ops::host_git_command(top)
        .args(["merge-base", &plane.commit, "HEAD"])
        .output()
        .context("Failed to run git merge-base")?;
    match out.status.code() {
        Some(1) => Ok(()),
        Some(0) => bail!(
            "Adoption plane {} shares history with the product branch; refused. \
             The plane must be an orphan branch such as {STORE_REF}.",
            plane.commit
        ),
        _ => bail!(
            "git merge-base failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

/// The nearest ancestor of `start` holding a `.git` entry.
fn nearest_repo(start: &Path) -> Result<Option<PathBuf>> {
    let start = std::path::absolute(start)?;
    for dir in start.ancestors() {
        match std::fs::symlink_metadata(dir.join(".git")) {
            Ok(_) => return Ok(Some(dir.canonicalize()?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(None)
}

fn read_locator(common: &Path) -> Result<Option<Locator>> {
    match std::fs::symlink_metadata(common.join(LOCATOR_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let bytes = crate::transport_policy::Private::open(common)
        .and_then(|private| private.read(LOCATOR_FILE))
        .map_err(|_| {
            anyhow::anyhow!(
                "Adoption locator {LOCATOR_FILE} in the Git common directory must be a \
                 regular file only its owner can read (chmod 600); refused"
            )
        })?
        .context("Adoption locator changed during discovery; refused")?;
    let locator: Locator = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| toml::from_str(text).ok())
        .context("Adoption locator is not a valid version-1 locator; refused")?;
    if locator.version != 1 {
        bail!(
            "Adoption locator version {} is not supported; refused",
            locator.version
        );
    }
    Ok(Some(locator))
}

fn conventional(top: &Path) -> Result<Option<Plane>> {
    let plane = |reference: String, commit| Plane {
        repo: top.to_path_buf(),
        source: Source::Conventional { reference },
        commit,
    };
    if let Some(commit) = resolve_commit(top, STORE_REF)? {
        return Ok(Some(plane(STORE_REF.to_string(), commit)));
    }
    let listed = git(
        top,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/remotes/*/aida-store",
        ],
    )?;
    let remotes: BTreeMap<String, String> = listed
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(sha, name)| (sha.to_string(), name.to_string()))
        .collect();
    match remotes.len() {
        0 => Ok(None),
        1 => {
            let (_, name) = remotes.into_iter().next().unwrap();
            let commit = resolve_commit(top, &name)?
                .with_context(|| format!("{name} does not resolve to a commit; refused"))?;
            Ok(Some(plane(name, commit)))
        }
        _ => bail!(
            "Remotes disagree about the aida-store branch ({}); refused. Create the local \
             aida-store branch from the one you mean, or name it in {LOCATOR_FILE}.",
            remotes.into_values().collect::<Vec<_>>().join(", ")
        ),
    }
}

fn sibling(common: &Path, store: &Path) -> Result<Plane> {
    if !store.is_absolute() {
        bail!("Adoption locator `store` must be an absolute path; refused");
    }
    let store = store
        .canonicalize()
        .ok()
        .filter(|path| path.join(".git").symlink_metadata().is_ok())
        .context("Adoption locator `store` is not a Git repository; refused")?;
    if crate::allocation_authority::common_dir(&store)? == *common {
        bail!(
            "Adoption locator `store` names this clone; remove {LOCATOR_FILE} to use the \
             conventional aida-store branch"
        );
    }
    let commit = resolve_commit(&store, STORE_REF)?
        .context("Adoption locator `store` has no aida-store branch; refused")?;
    Ok(Plane {
        repo: store.clone(),
        source: Source::Sibling { store },
        commit,
    })
}

/// A locator ref is a full `refs/` name with nothing Git would reinterpret.
fn check_ref_name(reference: &str) -> Result<()> {
    let ok = reference.starts_with("refs/")
        && !reference.contains("..")
        && !reference.contains("@{")
        && !reference.ends_with('/')
        && !reference.ends_with(".lock")
        && !reference
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    if !ok {
        bail!("Adoption locator `ref` is not a full ref name; refused");
    }
    Ok(())
}

/// Read the pinned plane revision. `Ok(None)` when the plane commit has no
/// adoption namespace yet.
pub fn load(plane: &Plane) -> Result<Option<Adoption>> {
    let spec = format!("{}:{SETTINGS_PATH}", plane.commit);
    let entry = git(
        &plane.repo,
        &["ls-tree", &plane.commit, "--", SETTINGS_PATH],
    )?;
    let Some((meta, _)) = entry.split_once('\t') else {
        return Ok(None);
    };
    if !meta.starts_with("100644 blob ") {
        bail!(
            "{SETTINGS_PATH} at {} is not a regular file; refused",
            plane.commit
        );
    }
    let size: u64 = git(&plane.repo, &["cat-file", "-s", &spec])?
        .trim()
        .parse()?;
    if size > MAX_SETTINGS_BYTES {
        bail!("{SETTINGS_PATH} exceeds {MAX_SETTINGS_BYTES} bytes; refused");
    }
    let text = git(&plane.repo, &["cat-file", "blob", &spec])?;
    parse(&plane.commit, &text).map(Some)
}

/// Refuse unless `adoption` was read from exactly `expected` (a full commit
/// id) under this build's schema.
pub fn verify(adoption: &Adoption, expected: &str) -> Result<()> {
    if adoption.revision.commit != expected {
        bail!(
            "Adoption plane is at {}, not the selected revision {expected}; refused",
            adoption.revision.commit
        );
    }
    if adoption.revision.schema != SCHEMA {
        bail!(
            "Adoption schema {} is not supported; refused",
            adoption.revision.schema
        );
    }
    Ok(())
}

fn parse(commit: &str, text: &str) -> Result<Adoption> {
    let file: SettingsFile = toml::from_str(text)
        .map_err(|e| anyhow::anyhow!("{SETTINGS_PATH} at {commit} is invalid: {}", e.message()))?;
    if file.schema != SCHEMA {
        bail!(
            "{SETTINGS_PATH} schema {} is not supported by this AIDA; refused",
            file.schema
        );
    }
    let policy = policy_keys();
    let mut settings = BTreeMap::new();
    for (section, body) in file.settings {
        let toml::Value::Table(body) = body else {
            bail!("[settings.{section}] must be a table; refused");
        };
        let mut keys = BTreeMap::new();
        for (key, value) in body {
            let rows = policy.get(&section);
            let named = rows.is_some_and(|rows| rows.contains(&key));
            let dynamic = rows.is_some_and(|rows| rows.iter().any(|row| row == "*"));
            if !((named && inert(&value, false)) || (dynamic && inert(&value, true))) {
                bail!(
                    "Adoption setting {section}.{key} is not a policy key; refused. Commands, \
                     hooks, credentials and paths stay in trusted or local configuration."
                );
            }
            keys.insert(key, value);
        }
        settings.insert(section, keys);
    }
    for (name, body) in &file.guidance {
        let named = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !named || body.contains('\0') {
            bail!(
                "Adoption guidance `{name}` must have a lowercase slug name and text body; refused"
            );
        }
    }
    Ok(Adoption {
        revision: Revision {
            commit: commit.to_string(),
            schema: file.schema,
            generator: GENERATOR.to_string(),
        },
        settings,
        guidance: file.guidance,
    })
}

/// A value that can only tune: a scalar or an array of scalars, or, under a
/// dynamic-key section, a table of those.
fn inert(value: &toml::Value, table: bool) -> bool {
    match value {
        toml::Value::Array(items) => items.iter().all(|v| !v.is_table() && !v.is_array()),
        toml::Value::Table(rows) => table && rows.values().all(|v| inert(v, false)),
        _ => true,
    }
}

/// `section -> keys` for every `[keys.worktree.<section>]` row classified
/// `policy` in the config-trust manifest. A `"*"` (dynamic key) row counts
/// only in a section whose every row is `policy`, so a grandfathered or
/// executable row added beside it can never be admitted under a wildcard.
fn policy_keys() -> BTreeMap<String, Vec<String>> {
    policy_keys_from(TRUST_MANIFEST)
}

fn policy_keys_from(manifest: &str) -> BTreeMap<String, Vec<String>> {
    let manifest: toml::Table = toml::from_str(manifest).expect("config-trust manifest parses");
    let worktree = manifest
        .get("keys")
        .and_then(|keys| keys.get("worktree"))
        .and_then(toml::Value::as_table)
        .expect("config-trust manifest has [keys.worktree]");
    worktree
        .iter()
        .filter_map(|(section, rows)| {
            let rows = rows.as_table()?;
            let all_policy = rows.values().all(|class| class.as_str() == Some("policy"));
            let keys = rows
                .iter()
                .filter(|(key, class)| {
                    class.as_str() == Some("policy") && (all_policy || key.as_str() != "*")
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            (!keys.is_empty()).then(|| (section.clone(), keys))
        })
        .collect()
}

fn resolve_commit(repo: &Path, reference: &str) -> Result<Option<String>> {
    let out = crate::git_ops::host_git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{reference}^{{commit}}"))
        .output()
        .context("Failed to run git rev-parse")?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8(out.stdout)?.trim().to_string()))
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git_ops::host_git_command(repo)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

#[cfg(test)]
#[path = "adoption_plane_tests.rs"]
mod tests;
