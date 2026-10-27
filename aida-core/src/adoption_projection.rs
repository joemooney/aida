//! The local projection of the shared adoption plane: one pinned plane
//! revision rendered into the worktree's git-ignored `.aida/` directory, so
//! the existing config readers and the agent launcher consume shared policy
//! without a single tracked product file.
//!
//! Outputs are `.aida/config.toml` (the inert settings, in the layout every
//! existing reader already parses), `.aida/adoption/guidance/<name>.md` and
//! `.aida/adoption/manifest.toml`, which records the revision and the SHA-256
//! of every file the projection owns. A refresh refuses rather than write
//! over anything it cannot prove it owns: a tracked or un-ignored path, a
//! symlink, a file it never wrote, or an owned file someone edited. When the
//! product branch tracks its own `.aida/config.toml` (pre-adoption
//! scaffolding), the settings output is skipped and reported; that file is
//! the product's, and migration is a separate reviewed change.
//!
//! Replacement is crash-safe without a directory swap: the manifest is first
//! rewritten with the new hashes as `pending`, then each file is replaced
//! atomically, then the manifest is finalized. A file matching either hash is
//! owned, so an interrupted refresh is completed by the next one instead of
//! being misread as a user edit. Refreshes in one worktree serialize on a
//! local `.aida/adoption.lock`; nothing here takes a store, cache or
//! dispenser lock, allocates IDs, or writes hooks (executable configuration
//! stays with config-trust).
// trace:STORY-1741 | ai:claude

use crate::adoption_plane::{self, Adoption};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The projected settings, read by the existing config readers.
pub const CONFIG: &str = ".aida/config.toml";
/// The ownership and revision record.
pub const MANIFEST: &str = ".aida/adoption/manifest.toml";
/// Projected guidance documents, one `<name>.md` each.
pub const GUIDANCE_DIR: &str = ".aida/adoption/guidance";
const LOCK: &str = ".aida/adoption.lock";
const MANIFEST_VERSION: u32 = 1;

/// What a projection holds and which files it owns.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub commit: String,
    pub schema: u32,
    pub generator: String,
    /// Worktree-relative path -> SHA-256 of the bytes written.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Hashes of an in-flight refresh; non-empty only after an interruption.
    #[serde(default)]
    pub pending: BTreeMap<String, String>,
}

impl Manifest {
    fn owns(&self, rel: &str, hash: &str) -> bool {
        self.files.get(rel).map(String::as_str) == Some(hash)
            || self.pending.get(rel).map(String::as_str) == Some(hash)
    }
}

/// The result of a refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub commit: String,
    /// False when the projection already held this revision byte for byte.
    pub written: bool,
    /// True when the product branch tracks `.aida/config.toml`, so the
    /// settings were not projected.
    pub config_tracked: bool,
}

/// How a worktree's projection relates to its plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No plane, or a plane without an adoption namespace.
    NotAdopted,
    /// Adopted, but never projected into this worktree.
    NotProjected { plane: String },
    /// `config_tracked`: the product branch tracks `.aida/config.toml`, so
    /// the plane's settings are not applied here.
    Current {
        commit: String,
        config_tracked: bool,
    },
    Stale {
        projected: String,
        plane: String,
        config_tracked: bool,
    },
    /// Owned outputs that were edited, removed, or left mid-refresh.
    Damaged { commit: String, paths: Vec<String> },
}

/// Discover the plane for `worktree`, read it and refresh the projection.
/// `Ok(None)` when the clone is not adopted.
pub fn sync(worktree: &Path) -> Result<Option<Outcome>> {
    let Some(plane) = adoption_plane::discover(worktree)? else {
        return Ok(None);
    };
    let Some(adoption) = adoption_plane::load(&plane)? else {
        return Ok(None);
    };
    refresh(worktree, &adoption).map(Some)
}

/// Compare the projection with the plane without writing anything.
pub fn inspect(worktree: &Path) -> Result<State> {
    let Some(plane) = adoption_plane::discover(worktree)? else {
        return Ok(State::NotAdopted);
    };
    let Some(adoption) = adoption_plane::load(&plane)? else {
        return Ok(State::NotAdopted);
    };
    let root = worktree_root(worktree)?;
    let Some(manifest) = read_manifest(&root)? else {
        return Ok(State::NotProjected {
            plane: adoption.revision.commit,
        });
    };
    let config_tracked = !tracked(&root, &[CONFIG])?.is_empty();
    let damaged = damaged_paths(&root, &manifest, config_tracked)?;
    Ok(if !damaged.is_empty() {
        State::Damaged {
            commit: manifest.commit,
            paths: damaged,
        }
    } else if manifest.commit != adoption.revision.commit
        || manifest.generator != adoption.revision.generator
    {
        State::Stale {
            projected: manifest.commit,
            plane: adoption.revision.commit,
            config_tracked,
        }
    } else {
        State::Current {
            commit: manifest.commit,
            config_tracked,
        }
    })
}

/// Render `adoption` into `worktree`'s projection. Deterministic: the same
/// revision and generator always produce the same bytes, and a projection
/// that already holds them is left untouched.
pub fn refresh(worktree: &Path, adoption: &Adoption) -> Result<Outcome> {
    let root = worktree_root(worktree)?;
    for dir in [".aida", ".aida/adoption", GUIDANCE_DIR] {
        ensure_real_dir(&root, dir)?;
    }
    crate::git_ops::ensure_aida_runtime_excluded(&root)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK))
        .with_context(|| format!("Failed to open {LOCK}"))?;
    fs2::FileExt::lock_exclusive(&lock).with_context(|| format!("Failed to lock {LOCK}"))?;

    let config_tracked = !tracked(&root, &[CONFIG])?.is_empty();
    let outputs = render(adoption, !config_tracked)?;
    let previous = read_manifest(&root)?;
    let mut paths: Vec<&str> = outputs.keys().map(String::as_str).collect();
    paths.push(MANIFEST);
    if let Some(found) = tracked(&root, &paths)?.first() {
        bail!(
            "{found} is tracked by the product branch; the adoption projection refused to touch it"
        );
    }
    for rel in &paths {
        if !ignored(&root, rel)? {
            bail!(
                "{rel} is not ignored by Git here (a tracked .gitignore re-includes it?); the \
                 adoption projection refused to write a file the product branch could pick up"
            );
        }
    }

    let hashes: BTreeMap<String, String> = outputs
        .iter()
        .map(|(rel, body)| (rel.clone(), sha256(body.as_bytes())))
        .collect();
    let dropped: Vec<&str> = previous
        .iter()
        .flat_map(|m| m.files.keys().chain(m.pending.keys()))
        .map(String::as_str)
        .filter(|rel| !outputs.contains_key(*rel))
        .collect();
    // A dropped path the product branch now tracks (a checkout replaced the
    // ignored copy) is the product's: forget it, never check or delete it.
    let now_tracked = if dropped.is_empty() {
        Vec::new()
    } else {
        tracked(&root, &dropped)?
    };
    let mut stale: Vec<String> = dropped
        .into_iter()
        .filter(|rel| !now_tracked.iter().any(|t| t == rel))
        .map(str::to_string)
        .collect();
    stale.sort_unstable();
    stale.dedup();
    let mut unchanged = previous.as_ref().is_some_and(|m| {
        m.pending.is_empty()
            && m.commit == adoption.revision.commit
            && m.generator == adoption.revision.generator
            && m.files == hashes
    });
    for rel in hashes.keys().chain(stale.iter()) {
        let Some(existing) = read_regular(&root, rel)? else {
            unchanged &= !hashes.contains_key(rel);
            continue;
        };
        let hash = sha256(&existing);
        if !previous.as_ref().is_some_and(|m| m.owns(rel, &hash)) {
            bail!(
                "{rel} was not written by the adoption projection, or was edited since; refused \
                 and left as is. Move any local change into adoption/settings.toml on the plane, \
                 then delete {rel} and refresh."
            );
        }
        unchanged &= hashes.get(rel) == Some(&hash);
    }
    let finished = Manifest {
        version: MANIFEST_VERSION,
        commit: adoption.revision.commit.clone(),
        schema: adoption.revision.schema,
        generator: adoption.revision.generator.clone(),
        files: hashes.clone(),
        pending: BTreeMap::new(),
    };
    if unchanged {
        return Ok(Outcome {
            commit: finished.commit,
            written: false,
            config_tracked,
        });
    }

    let mut intent = previous.unwrap_or_else(|| Manifest {
        files: BTreeMap::new(),
        ..finished.clone()
    });
    intent.pending.extend(hashes);
    write_manifest(&root, &intent)?;
    for (rel, body) in &outputs {
        crate::write_atomic(&root.join(rel), body)
            .with_context(|| format!("Failed to write {rel}"))?;
    }
    for rel in &stale {
        match std::fs::remove_file(root.join(rel)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("Failed to remove {rel}"))
            }
            _ => {}
        }
    }
    write_manifest(&root, &finished)?;
    Ok(Outcome {
        commit: finished.commit,
        written: true,
        config_tracked,
    })
}

/// The projection's files for `adoption`, by worktree-relative path.
pub fn render(adoption: &Adoption, include_config: bool) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if include_config {
        let body = toml::to_string(&adoption.config_table()?)?;
        out.insert(
            CONFIG.to_string(),
            format!(
                "# Generated by AIDA from adoption plane revision {}.\n\
                 # Do not edit: change adoption/settings.toml on the plane, then refresh.\n\n{body}",
                adoption.revision.commit
            ),
        );
    }
    for (name, body) in &adoption.guidance {
        out.insert(format!("{GUIDANCE_DIR}/{name}.md"), body.clone());
    }
    Ok(out)
}

/// What a launch may deliver from the projection in `worktree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Not adopted here.
    None,
    /// Adopted, but the projection is missing, behind the plane, damaged or
    /// unreadable, so nothing is delivered; `reason` says which.
    Withheld { reason: String },
    /// The projected revision, its `(name, body)` guidance documents, and
    /// whether the product branch's tracked config kept the settings out.
    Verified {
        commit: String,
        docs: Vec<(String, String)>,
        config_tracked: bool,
    },
}

/// The guidance a launch from `worktree` may deliver. Decided by
/// [`inspect`], the same predicate the launch preview reports, so the two
/// always agree: only a projection that is `Current` with the discovered
/// plane is delivered.
pub fn delivered_guidance(worktree: &Path) -> Delivery {
    let Ok(root) = worktree_root(worktree) else {
        return Delivery::None;
    };
    let withheld = |reason: String| Delivery::Withheld { reason };
    match inspect(&root) {
        Ok(State::NotAdopted) => Delivery::None,
        Ok(State::Current {
            commit,
            config_tracked,
        }) => verified_guidance(&root, commit, config_tracked)
            .unwrap_or_else(|| withheld("an owned guidance file changed while it was read".into())),
        Ok(State::NotProjected { plane }) => {
            withheld(format!("plane {plane} has not been projected here"))
        }
        Ok(State::Stale {
            projected, plane, ..
        }) => withheld(format!(
            "the projection holds {projected} but the plane is at {plane}"
        )),
        Ok(State::Damaged { paths, .. }) => withheld(format!(
            "owned projection files are edited, missing or mid-refresh ({})",
            paths.join(", ")
        )),
        Err(e) => withheld(format!("the adoption plane could not be read ({e:#})")),
    }
}

fn verified_guidance(root: &Path, commit: String, config_tracked: bool) -> Option<Delivery> {
    let manifest = read_manifest(root).ok()??;
    if !manifest.pending.is_empty() || manifest.commit != commit {
        return None;
    }
    let prefix = format!("{GUIDANCE_DIR}/");
    let mut docs = Vec::new();
    for (rel, hash) in &manifest.files {
        let Some(name) = rel
            .strip_prefix(&prefix)
            .and_then(|n| n.strip_suffix(".md"))
        else {
            continue;
        };
        let bytes = read_regular(root, rel).ok()??;
        if sha256(&bytes) != *hash {
            return None;
        }
        docs.push((name.to_string(), String::from_utf8(bytes).ok()?));
    }
    Some(Delivery::Verified {
        commit,
        docs,
        config_tracked,
    })
}

/// Whether the worktree holding `project_root` has an adoption projection.
/// A plain file check, cheap enough for every writer guard.
pub fn is_projected(project_root: &Path) -> bool {
    project_root.join(MANIFEST).is_file()
}

/// Whether `path` is a file the adoption projection owns, so a config
/// writer must not edit it in place.
pub fn owns(path: &Path) -> bool {
    let Some(root) = path
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|n| n == ".aida"))
        .and_then(Path::parent)
    else {
        return false;
    };
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let rel = rel.to_string_lossy();
    read_manifest(root)
        .ok()
        .flatten()
        .is_some_and(|m| m.files.contains_key(rel.as_ref()) || m.pending.contains_key(rel.as_ref()))
}

/// The refusal a writer reports for an owned projection file.
pub fn owned_write_refusal(path: &Path) -> String {
    format!(
        "{} is generated from the shared adoption plane; refused to edit it. Change \
         adoption/settings.toml on the aida-store plane instead.",
        path.display()
    )
}

pub fn read_manifest(root: &Path) -> Result<Option<Manifest>> {
    let Some(bytes) = read_regular(root, MANIFEST)? else {
        return Ok(None);
    };
    let manifest: Manifest = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| toml::from_str(text).ok())
        .with_context(|| format!("{MANIFEST} is not a valid adoption manifest; refused"))?;
    if manifest.version != MANIFEST_VERSION {
        bail!(
            "{MANIFEST} version {} is not supported; refused",
            manifest.version
        );
    }
    let safe = |rel: &String| {
        rel.starts_with(".aida/")
            && !rel.contains('\\')
            && Path::new(rel)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
    };
    if !manifest
        .files
        .keys()
        .chain(manifest.pending.keys())
        .all(safe)
    {
        bail!("{MANIFEST} names a path outside .aida; refused");
    }
    Ok(Some(manifest))
}

fn write_manifest(root: &Path, manifest: &Manifest) -> Result<()> {
    crate::write_atomic(&root.join(MANIFEST), toml::to_string(manifest)?)
        .with_context(|| format!("Failed to write {MANIFEST}"))
}

/// Owned files that are pending, edited or missing. A recorded config the
/// product branch now tracks is the product's, not damage: the next refresh
/// forgets it.
fn damaged_paths(root: &Path, manifest: &Manifest, config_tracked: bool) -> Result<Vec<String>> {
    let mut damaged: Vec<String> = manifest.pending.keys().cloned().collect();
    for (rel, hash) in &manifest.files {
        if config_tracked && rel == CONFIG {
            continue;
        }
        let intact = read_regular(root, rel)?.is_some_and(|bytes| sha256(&bytes) == *hash);
        if !intact && !damaged.contains(rel) {
            damaged.push(rel.clone());
        }
    }
    Ok(damaged)
}

/// The top of the worktree holding `path`.
fn worktree_root(path: &Path) -> Result<PathBuf> {
    let top = git(path, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(top.trim()).canonicalize()?)
}

/// `rel` under `root` as a real directory, created if missing; a symlink or
/// non-directory anywhere on the way refuses.
fn ensure_real_dir(root: &Path, rel: &str) -> Result<()> {
    let path = root.join(rel);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => bail!("{rel} is not a plain directory; the adoption projection refused"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&path).with_context(|| format!("Failed to create {rel}"))
        }
        Err(e) => Err(e.into()),
    }
}

/// The bytes of `rel` when it is a regular file; a symlink or other file
/// type refuses so a write can never follow it out of the projection.
fn read_regular(root: &Path, rel: &str) -> Result<Option<Vec<u8>>> {
    let path = root.join(rel);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(Some(std::fs::read(&path)?)),
        Ok(_) => bail!("{rel} is not a regular file; the adoption projection refused"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn tracked(root: &Path, paths: &[&str]) -> Result<Vec<String>> {
    let mut args = vec!["ls-files", "--"];
    args.extend_from_slice(paths);
    Ok(git(root, &args)?.lines().map(str::to_string).collect())
}

fn ignored(root: &Path, rel: &str) -> Result<bool> {
    let out = crate::git_ops::host_git_command(root)
        .args(["check-ignore", "-q", "--", rel])
        .output()
        .context("Failed to run git check-ignore")?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "git check-ignore failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = crate::git_ops::host_git_command(dir)
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
#[path = "adoption_projection_tests.rs"]
mod tests;
