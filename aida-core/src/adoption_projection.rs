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
//! the product's, and migration is a separate reviewed change. A local
//! untracked config the projection never wrote is kept and reported the
//! same way, never overwritten.
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

/// Why the plane's settings were not projected into `.aida/config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptConfig {
    /// The product branch tracks the file.
    Tracked,
    /// An untracked file the projection never wrote, such as one an
    /// `aida config set` created before the first projection.
    Local,
}

impl KeptConfig {
    /// What a launch reports about the kept file.
    pub fn note(self) -> &'static str {
        match self {
            KeptConfig::Tracked => {
                "the product branch tracks .aida/config.toml, so the adoption plane's \
                 settings are not applied here"
            }
            KeptConfig::Local => {
                "a local .aida/config.toml the adoption projection did not write is kept, so \
                 the plane's settings are not applied here. A shared policy setting belongs \
                 in adoption/settings.toml on the aida-store plane; a local path or personal \
                 preference belongs in ~/.aida/config.toml. Delete the local file to receive \
                 the plane's settings"
            }
        }
    }
}

/// The result of a refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub commit: String,
    /// False when the projection already held this revision byte for byte.
    pub written: bool,
    /// Set when an existing `.aida/config.toml` was kept, so the settings
    /// were not projected.
    pub config_kept: Option<KeptConfig>,
}

/// How a worktree's projection relates to its plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No plane, a plane without an adoption namespace, or a discovery
    /// refusal with no adoption evidence anywhere.
    NotAdopted,
    /// Adopted, but never projected into this worktree.
    NotProjected { plane: String },
    /// `config_kept`: an existing `.aida/config.toml` was kept, so the
    /// plane's settings are not applied here.
    Current {
        commit: String,
        config_kept: Option<KeptConfig>,
    },
    Stale {
        projected: String,
        plane: String,
        config_kept: Option<KeptConfig>,
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
    let config_kept = kept_config(&root, Some(&manifest))?;
    let damaged = damaged_paths(&root, &manifest, config_kept)?;
    // The owned file set must be the one a refresh would write now: a
    // checkout that stops (or starts) tracking the config changes it with
    // no plane move, and until a refresh the settings are not applied.
    let expected: Vec<String> = render(&adoption, config_kept.is_none())?
        .into_keys()
        .collect();
    let owned: Vec<&String> = manifest
        .files
        .keys()
        .filter(|rel| !(config_kept.is_some() && rel.as_str() == CONFIG))
        .collect();
    let same_files =
        owned.len() == expected.len() && owned.iter().zip(&expected).all(|(a, b)| *a == b);
    Ok(if !damaged.is_empty() {
        State::Damaged {
            commit: manifest.commit,
            paths: damaged,
        }
    } else if manifest.commit != adoption.revision.commit
        || manifest.generator != adoption.revision.generator
        || !same_files
    {
        State::Stale {
            projected: manifest.commit,
            plane: adoption.revision.commit,
            config_kept,
        }
    } else {
        State::Current {
            commit: manifest.commit,
            config_kept,
        }
    })
}

/// Render `adoption` into `worktree`'s projection. Deterministic: the same
/// revision and generator always produce the same bytes, and a projection
/// that already holds them is left untouched.
pub fn refresh(worktree: &Path, adoption: &Adoption) -> Result<Outcome> {
    let root = worktree_root(worktree)?;
    // Only `.aida` itself is made before the lock, which lives in it; the
    // rest of the refresh, `info/exclude` included, runs under the lock.
    ensure_real_dir(&root, ".aida")?;
    let lock = open_lock(&root)?;
    fs2::FileExt::lock_exclusive(&lock).with_context(|| format!("Failed to lock {LOCK}"))?;
    for dir in [".aida/adoption", GUIDANCE_DIR] {
        ensure_real_dir(&root, dir)?;
    }
    crate::git_ops::ensure_aida_runtime_excluded(&root)?;

    let previous = read_manifest(&root)?;
    let config_kept = kept_config(&root, previous.as_ref())?;
    let outputs = render(adoption, config_kept.is_none())?;
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
    // What each owned path holds right now, proven by the check below.
    let mut on_disk: BTreeMap<String, String> = BTreeMap::new();
    for rel in hashes.keys().chain(stale.iter()) {
        let Some(existing) = read_regular(&root, rel)? else {
            unchanged &= !hashes.contains_key(rel);
            continue;
        };
        let hash = sha256(&existing);
        if !previous.as_ref().is_some_and(|m| m.owns(rel, &hash)) {
            bail!(
                "{rel} was not written by the adoption projection, or was edited since; refused \
                 and left as is. {MOVE_LOCAL_CHANGE} Then delete {rel} and refresh."
            );
        }
        unchanged &= hashes.get(rel) == Some(&hash);
        on_disk.insert(rel.clone(), hash);
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
            config_kept,
        });
    }

    // The previous in-flight set is reconciled first: `files` records the
    // bytes each owned path verifiably holds now, and `pending` only this
    // refresh's hashes, so a second interruption never forgets a hash a
    // file still carries.
    let mut intent = previous.unwrap_or_else(|| finished.clone());
    intent.files = on_disk;
    intent.pending = hashes;
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
        config_kept,
    })
}

/// CI bootstrap: refresh the projection only when the discovered plane is
/// exactly `expected` (a full commit id), then prove the projection holds
/// that revision with the plane's settings applied. Every mismatch refuses.
// trace:STORY-1741 | ai:claude
pub fn bootstrap(worktree: &Path, expected: &str) -> Result<Outcome> {
    let full = matches!(expected.len(), 40 | 64)
        && expected
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if !full {
        bail!("--revision must be a full lowercase commit id; refused");
    }
    let plane = adoption_plane::discover(worktree)?
        .context("No adoption plane found for this clone; nothing to bootstrap")?;
    let adoption = adoption_plane::load(&plane)?.with_context(|| {
        format!(
            "Adoption plane {} has no {}; nothing to bootstrap",
            plane.commit,
            adoption_plane::SETTINGS_PATH
        )
    })?;
    adoption_plane::verify(&adoption, expected)?;
    let outcome = refresh(worktree, &adoption)?;
    if let Some(kept) = outcome.config_kept {
        bail!(
            "The projection holds {expected}, but {}; refused",
            kept.note()
        );
    }
    match inspect(worktree)? {
        State::Current { commit, .. } if commit == expected => Ok(outcome),
        state => bail!("The projection did not verify at {expected} ({state:?}); refused"),
    }
}

/// What [`uninstall`] removed, or would remove in a dry run, and kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    /// Owned files, the manifest and the lock, by worktree-relative path.
    pub removed: Vec<String>,
    /// `(path, why)` for each recorded file left in place.
    pub kept: Vec<(String, String)>,
}

/// Remove this worktree's projection: every file the manifest proves it
/// owns, then the manifest, the lock and the projection's own directories
/// once empty. A recorded file that was edited, replaced by something
/// other than a regular file, or is now tracked by the product branch is
/// kept and reported. The plane, the store, the locator and every product
/// file are left alone. The manifest goes last, so an interrupted removal
/// is finished by running it again.
// trace:STORY-1741 | ai:claude
pub fn uninstall(worktree: &Path, dry_run: bool) -> Result<Removal> {
    let root = worktree_root(worktree)?;
    let mut removal = Removal::default();
    let present = |rel: &str| std::fs::symlink_metadata(root.join(rel)).is_ok();
    if !present(MANIFEST) {
        if present(LOCK) {
            removal.removed.push(LOCK.to_string());
            if !dry_run {
                remove_if_present(&root, LOCK)?;
            }
        }
        return Ok(removal);
    }
    if let Some(found) = tracked(&root, &[MANIFEST, LOCK])?.first() {
        bail!("{found} is tracked by the product branch; refused to remove the projection");
    }
    let had_lock = present(LOCK);
    let lock = if dry_run {
        None
    } else {
        let lock = open_lock(&root)?;
        fs2::FileExt::lock_exclusive(&lock).with_context(|| format!("Failed to lock {LOCK}"))?;
        Some(lock)
    };
    let manifest = read_manifest(&root)?.context("The adoption manifest vanished; retry")?;
    let mut recorded: Vec<&str> = manifest
        .files
        .keys()
        .chain(manifest.pending.keys())
        .map(String::as_str)
        .collect();
    recorded.sort_unstable();
    recorded.dedup();
    let now_tracked = if recorded.is_empty() {
        Vec::new()
    } else {
        tracked(&root, &recorded)?
    };
    for rel in recorded {
        if now_tracked.iter().any(|t| t == rel) {
            removal
                .kept
                .push((rel.to_string(), "tracked by the product branch".into()));
            continue;
        }
        let bytes = match read_regular(&root, rel) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(_) => {
                removal
                    .kept
                    .push((rel.to_string(), "not a regular file".into()));
                continue;
            }
        };
        if !manifest.owns(rel, &sha256(&bytes)) {
            removal.kept.push((
                rel.to_string(),
                "edited since the projection wrote it".into(),
            ));
            continue;
        }
        if !dry_run {
            remove_if_present(&root, rel)?;
        }
        removal.removed.push(rel.to_string());
    }
    removal.removed.push(MANIFEST.to_string());
    if had_lock || !dry_run {
        removal.removed.push(LOCK.to_string());
    }
    if !dry_run {
        remove_if_present(&root, MANIFEST)?;
        for dir in [GUIDANCE_DIR, ".aida/adoption"] {
            match std::fs::remove_dir(root.join(dir)) {
                Err(e)
                    if !matches!(
                        e.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    return Err(e).with_context(|| format!("Failed to remove {dir}"))
                }
                _ => {}
            }
        }
        remove_if_present(&root, LOCK)?;
    }
    drop(lock);
    Ok(removal)
}

fn remove_if_present(root: &Path, rel: &str) -> Result<()> {
    match std::fs::remove_file(root.join(rel)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("Failed to remove {rel}"))
        }
        _ => Ok(()),
    }
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
    /// the existing config that kept the settings out, if any.
    Verified {
        commit: String,
        docs: Vec<(String, String)>,
        config_kept: Option<KeptConfig>,
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
            config_kept,
        }) => verified_guidance(&root, commit, config_kept)
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

fn verified_guidance(
    root: &Path,
    commit: String,
    config_kept: Option<KeptConfig>,
) -> Option<Delivery> {
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
        config_kept,
    })
}

/// Whether the worktree holding `project_root` has an adoption projection.
/// Plain file checks up to the worktree top (the first directory with a
/// `.git`), so a writer rooted in a subdirectory is covered too.
pub fn is_projected(project_root: &Path) -> bool {
    for dir in project_root.ancestors() {
        if dir.join(MANIFEST).is_file() {
            return true;
        }
        if dir.join(".git").exists() {
            return false;
        }
    }
    false
}

/// What discovery says about the clone holding `project_root`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptionState {
    /// No plane, a plane without an adoption namespace, or a discovery
    /// refusal with no adoption evidence anywhere.
    NotAdopted,
    /// Projected here, or the plane carries an adoption namespace (one
    /// that does not parse counts too).
    Adopted,
    /// The plane could not be discovered and the clone carries adoption
    /// evidence (a malformed or non-600 locator, a locator ref that does not
    /// resolve, remotes that disagree where one carries a namespace, ...). The
    /// guards treat this as adopted: uncertainty refuses rather than falling
    /// back to product-file mutation.
    Unknown(String),
}

/// Whether the clone holding `project_root` is adopted, projected here yet
/// or not: a new clone or linked worktree is adopted before its first
/// launch projects anything. The projection is the fast path; otherwise
/// the plane is discovered and its namespace looked up.
// trace:STORY-1741 | ai:claude
pub fn adoption_state(project_root: &Path) -> AdoptionState {
    if is_projected(project_root) {
        return AdoptionState::Adopted;
    }
    match adoption_plane::discover(project_root) {
        Ok(None) => AdoptionState::NotAdopted,
        Ok(Some(plane)) => match adoption_plane::load(&plane) {
            Ok(None) => AdoptionState::NotAdopted,
            _ => AdoptionState::Adopted,
        },
        // A refusal with no adoption evidence (a non-orphan store, remotes
        // that disagree, nothing adopted on either) is a plain clone.
        Err(e) => match adoption_plane::shows_adoption(project_root) {
            Ok(false) => AdoptionState::NotAdopted,
            _ => AdoptionState::Unknown(format!("{e:#}")),
        },
    }
}

/// [`adoption_state`] for guards: fails closed, so a plane that cannot be
/// discovered counts as adopted.
pub fn is_adopted(project_root: &Path) -> bool {
    adoption_state(project_root) != AdoptionState::NotAdopted
}

/// Refuse a write to `path` when it is a file the adoption projection
/// owns. Fails closed: a manifest that exists but cannot be read refuses
/// too, since it may own the path. Every config writer goes through
/// [`crate::fs_atomic::write_config_atomic`], which calls this.
pub fn refuse_owned_write(path: &Path) -> Result<()> {
    let Some(root) = path
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|n| n == ".aida"))
        .and_then(Path::parent)
    else {
        return Ok(());
    };
    let Ok(rel) = path.strip_prefix(root) else {
        return Ok(());
    };
    let rel = rel.to_string_lossy();
    match read_manifest(root) {
        Ok(None) => Ok(()),
        Ok(Some(m))
            if !m.files.contains_key(rel.as_ref()) && !m.pending.contains_key(rel.as_ref()) =>
        {
            Ok(())
        }
        Ok(Some(_)) => bail!(owned_write_refusal(path)),
        Err(e) => bail!(
            "{}: {e:#}. It may record {} as generated from the shared adoption plane, so the \
             write was refused; refresh the projection with a fresh `aida agent` launch.",
            root.join(MANIFEST).display(),
            path.display()
        ),
    }
}

/// Whether `path` is a file the adoption projection owns, or might own
/// behind an unreadable manifest.
pub fn owns(path: &Path) -> bool {
    refuse_owned_write(path).is_err()
}

/// Where a change to a projected file belongs. It names both
/// destinations, since the plane accepts only shared policy keys: a local
/// path or preference never belongs on it.
const MOVE_LOCAL_CHANGE: &str = "A shared policy setting belongs in adoption/settings.toml on \
     the aida-store plane. A local path or personal preference does not: set it in your user \
     config, ~/.aida/config.toml, or in the config tracked on the default branch if the project \
     agreed to keep one.";

/// The refusal a writer reports for an owned projection file.
pub fn owned_write_refusal(path: &Path) -> String {
    format!(
        "{} is generated from the shared adoption plane; refused to edit it. {MOVE_LOCAL_CHANGE}",
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
fn damaged_paths(
    root: &Path,
    manifest: &Manifest,
    config_kept: Option<KeptConfig>,
) -> Result<Vec<String>> {
    let mut damaged: Vec<String> = manifest.pending.keys().cloned().collect();
    for (rel, hash) in &manifest.files {
        if config_kept.is_some() && rel == CONFIG {
            continue;
        }
        let intact = read_regular(root, rel)?.is_some_and(|bytes| sha256(&bytes) == *hash);
        if !intact && !damaged.contains(rel) {
            damaged.push(rel.clone());
        }
    }
    Ok(damaged)
}

/// Whether an existing `.aida/config.toml` keeps the plane's settings out:
/// the product branch tracks it, or it is a local file `manifest` does not
/// record. A recorded file is owned, so an edit to it is damage instead.
fn kept_config(root: &Path, manifest: Option<&Manifest>) -> Result<Option<KeptConfig>> {
    if !tracked(root, &[CONFIG])?.is_empty() {
        return Ok(Some(KeptConfig::Tracked));
    }
    let recorded =
        manifest.is_some_and(|m| m.files.contains_key(CONFIG) || m.pending.contains_key(CONFIG));
    let exists = match std::fs::symlink_metadata(root.join(CONFIG)) {
        Ok(meta) if meta.is_file() => true,
        Ok(_) => bail!("{CONFIG} is not a regular file; the adoption projection refused"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    Ok((exists && !recorded).then_some(KeptConfig::Local))
}

/// The top of the worktree holding `path`.
fn worktree_root(path: &Path) -> Result<PathBuf> {
    let top = git(path, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(top.trim()).canonicalize()?)
}

/// `rel` under `root` as a real directory, created if missing; a symlink or
/// non-directory anywhere on the way refuses. A concurrent creator winning
/// the race is re-checked, not reported as a failure.
fn ensure_real_dir(root: &Path, rel: &str) -> Result<()> {
    let path = root.join(rel);
    for _ in 0..2 {
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => return Ok(()),
            Ok(_) => bail!("{rel} is not a plain directory; the adoption projection refused"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&path) {
                    Ok(()) => return Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e).with_context(|| format!("Failed to create {rel}")),
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    bail!("{rel} kept changing while it was created; the adoption projection refused")
}

/// Open the refresh lock without following a symlink: a planted link must
/// never create or truncate a file outside the projection.
fn open_lock(root: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = root.join(LOCK);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => {
            bail!("{LOCK} is not a regular file; the adoption projection refused")
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("Failed to open {LOCK}"))
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
