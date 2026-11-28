//! Leftovers of mailbox creation and reuse, surfaced and reaped.
//!
//! Three kinds of entry outlive the operation that made them:
//!
//! - `<checkout>/.aida/.mailbox-tomb-<nonce>/`: a retired mailbox renamed
//!   aside by a reuse. An empty one (only directories and its creation nonce)
//!   is removed; one holding an entry written after the retirement is mail
//!   and is only ever reported, for an operator to move out by hand.
//! - `<checkout>/.aida/.mailbox.creating.<uuid>/`: a crashed creator's
//!   staging directory.
//! - `<anchor>/nonce-era/<hash>`: a nonce-era marker whose worktree admin
//!   entry was pruned or recreated, and `.<hash>.drop.<id>` asides an
//!   earlier build left while dropping one (a fresh aside whose marker is
//!   gone is linked back).
//!
//! Tombs, staging and asides are touched only once older than
//! [`STALE_AFTER`], so a reuse, creation or drop still in flight never loses
//! its entry. Markers are dropped under the Lifecycle lock. The retention
//! dry run reports; `aida agent gc` reaps.
//!
//! A scoped scan ([`scan_scoped`]) also attributes each candidate to the
//! checkout that owns it and asks the caller whether that owner is live:
//! only a provably dead owner's leftovers are reaped, and an optional age
//! floor applies to every candidate, era markers included. The one
//! exception is an id temp no checkout owns: it is reaped once older than
//! [`ORPHAN_TEMP_AFTER`], since no writer holds a temp that long.
//! trace:BUG-2770 trace:TASK-3478 | ai:claude
//!
//! trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude

use serde::Serialize;
use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Age below which a tomb or staging directory is never reaped.
pub const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// Age past which a [`write_source_id`](super::write_source_id) temp that no
/// checkout owns (one under the anchor, left while writing an era marker) is
/// reaped whatever its owner judge says. A writer holds its temp only for one
/// synced write and a rename, so none is still using one this old.
// trace:TASK-3478 | ai:claude
pub const ORPHAN_TEMP_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Prefix of a retired mailbox renamed aside by a reuse.
pub(crate) const TOMB_PREFIX: &str = ".mailbox-tomb-";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Leftovers {
    /// Tombs holding an entry written after a retirement: never removed.
    pub stranded_tombs: Vec<PathBuf>,
    /// Empty tombs, stale staging directories and stale nonce-era markers:
    /// removed by a reaping scan, listed by a reporting one.
    pub reapable: Vec<PathBuf>,
    /// Era-marker drop asides whose body is neither a marker record nor a
    /// bare admin name (a torn write): never removed or restored here.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-075 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrupt: Vec<PathBuf>,
    /// Candidates kept because their owner is live or cannot be judged.
    // trace:BUG-2770 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<HeldLeftover>,
}

/// Whether the checkout owning a leftover still has a live session or lease.
// trace:BUG-2770 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerLiveness {
    Live,
    Dead,
    /// Not provable either way: kept, never reaped.
    Unknown,
}

/// The checkout a leftover belongs to.
// trace:BUG-2770 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// A checkout root (main checkout, linked worktree or seat clone).
    Checkout(PathBuf),
    /// A linked worktree known only by its admin name (its admin entry is gone).
    Worktree(String),
    /// Nothing attributes it to one checkout.
    Unattributed,
}

/// A leftover kept because its owner is not provably dead.
// trace:BUG-2770 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeldLeftover {
    pub path: PathBuf,
    pub owner: OwnerLiveness,
}

/// What a scoped scan may touch: candidates at least `min_age` old (never
/// younger than [`STALE_AFTER`] for tombs, staging, temps and asides) whose
/// owner `owner` judges [`OwnerLiveness::Dead`].
// trace:BUG-2770 | ai:claude
pub struct Scope<'a> {
    pub min_age: Option<Duration>,
    pub owner: &'a dyn Fn(&Owner) -> OwnerLiveness,
}

fn every_owner_dead(_: &Owner) -> OwnerLiveness {
    OwnerLiveness::Dead
}

/// One scan's settings and findings.
struct Pass<'a> {
    reap: bool,
    now: SystemTime,
    scope: &'a Scope<'a>,
    out: Leftovers,
}

impl Pass<'_> {
    /// May `path` be removed? Too young: silently not a candidate. Owner not
    /// provably dead: held and reported. `floor` is the kind's own minimum age.
    fn admit(&mut self, path: &Path, floor: Duration, owner: impl FnOnce() -> Owner) -> bool {
        self.admit_with(path, floor, None, owner)
    }

    /// [`Pass::admit`], but an unattributed candidate at least `orphan_after`
    /// old (and past `min_age`) is admitted without its owner being judged.
    // trace:TASK-3478 | ai:claude
    fn admit_with(
        &mut self,
        path: &Path,
        floor: Duration,
        orphan_after: Option<Duration>,
        owner: impl FnOnce() -> Owner,
    ) -> bool {
        let floor = self.scope.min_age.map_or(floor, |age| age.max(floor));
        if !is_older(path, self.now, floor) {
            return false;
        }
        let owner = owner();
        let liveness = (self.scope.owner)(&owner);
        if liveness == OwnerLiveness::Dead {
            return true;
        }
        if let Some(orphan) = orphan_after {
            if owner == Owner::Unattributed && is_older(path, self.now, floor.max(orphan)) {
                return true;
            }
        }
        self.out.held.push(HeldLeftover {
            path: path.to_path_buf(),
            owner: liveness,
        });
        false
    }
}

/// Scan every checkout the anchor of `project_root` has a source for (and
/// `project_root` itself). With `reap`, remove what is reapable; `reapable`
/// then lists what was removed.
pub fn scan(project_root: &Path, reap: bool) -> Leftovers {
    scan_at(project_root, reap, SystemTime::now())
}

pub(crate) fn scan_at(project_root: &Path, reap: bool, now: SystemTime) -> Leftovers {
    let scope = Scope {
        min_age: None,
        owner: &every_owner_dead,
    };
    scan_scoped_at(project_root, reap, now, &scope)
}

/// [`scan`] limited by `scope`: only old-enough leftovers of dead owners are
/// reaped; the rest of the candidates land in `held`.
// trace:BUG-2770 | ai:claude
pub fn scan_scoped(project_root: &Path, reap: bool, scope: &Scope<'_>) -> Leftovers {
    scan_scoped_at(project_root, reap, SystemTime::now(), scope)
}

pub(crate) fn scan_scoped_at(
    project_root: &Path,
    reap: bool,
    now: SystemTime,
    scope: &Scope<'_>,
) -> Leftovers {
    let mut pass = Pass {
        reap,
        now,
        scope,
        out: Leftovers::default(),
    };
    let mut aida_dirs = BTreeSet::from([project_root.join(".aida")]);
    let anchor = super::locate_anchor(project_root).ok().flatten();
    if let Some(anchored) = &anchor {
        for mailbox in source_mailboxes(&anchored.anchor) {
            if let Some(dir) = mailbox.parent() {
                aida_dirs.insert(dir.to_path_buf());
            }
        }
    }
    for dir in &aida_dirs {
        let owner = dir
            .parent()
            .map_or(Owner::Unattributed, |c| Owner::Checkout(c.to_path_buf()));
        scan_aida_dir(dir, &owner, &mut pass);
        scan_id_temps(dir, &owner, &mut pass);
    }
    if let Some(anchored) = &anchor {
        scan_era_markers(&anchored.anchor, &anchored.common, &mut pass);
        scan_id_temps(&anchored.anchor, &Owner::Unattributed, &mut pass);
    }
    pass.out
}

/// The checkout an era marker's admin name stands for: the main checkout, a
/// seat clone's root, or a linked worktree read from its admin `gitdir`; a
/// pruned linked worktree is known only by name.
// trace:BUG-2770 | ai:claude
fn marker_owner(common: &Path, admin: &str) -> Owner {
    if admin.is_empty() {
        return common
            .parent()
            .map_or(Owner::Unattributed, |c| Owner::Checkout(c.to_path_buf()));
    }
    if let Some(root) = admin.strip_prefix("seat-clone:") {
        return Owner::Checkout(PathBuf::from(root));
    }
    let gitdir = std::fs::read_to_string(super::admin_dir(common, admin).join("gitdir"));
    match gitdir.ok().as_deref().map(str::trim).map(Path::new) {
        Some(dot_git) if dot_git.is_absolute() => dot_git
            .parent()
            .map_or(Owner::Unattributed, |c| Owner::Checkout(c.to_path_buf())),
        _ => Owner::Worktree(admin.to_string()),
    }
}

/// Remove `<parent>/.<name>.creating.*` staging directories older than
/// [`STALE_AFTER`]. Best effort: a failure leaves the directory for later.
pub(crate) fn reap_stale_staging(parent: &Path, name: &str, now: SystemTime) {
    let prefix = format!(".{name}.creating.");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with(&prefix) && is_stale_dir(&path, now) {
            let _ = remove_staging(&path);
        }
    }
}

/// The mailbox paths of the anchor's source records.
fn source_mailboxes(anchor: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(anchor.join(super::SOURCES_DIR)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| {
            super::read_json::<super::SourceRecord>(&e.path())
                .ok()
                .flatten()
        })
        .map(|r| r.mailbox)
        .collect()
}

fn scan_aida_dir(dir: &Path, owner: &Owner, pass: &mut Pass<'_>) {
    let reap = pass.reap;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with(TOMB_PREFIX) {
            if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir()) {
                continue;
            }
            // Anything but directories and the nonce is mail; an unreadable
            // tomb counts as mail too.
            if super::guard::check_empty_tree(&path, true).is_err() {
                pass.out.stranded_tombs.push(path);
            } else if pass.admit(&path, STALE_AFTER, || owner.clone())
                && (!reap || remove_empty_tomb(&path).is_ok())
            {
                pass.out.reapable.push(path);
            }
        } else if name.starts_with(".mailbox.creating.")
            && std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir())
            && pass.admit(&path, STALE_AFTER, || owner.clone())
            && (!reap || remove_staging(&path).is_ok())
        {
            pass.out.reapable.push(path);
        }
    }
}

fn scan_era_markers(anchor: &Path, common: &Path, pass: &mut Pass<'_>) {
    let reap = pass.reap;
    let Ok(entries) = std::fs::read_dir(anchor.join(super::NONCE_ERA_DIR)) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let Ok(Some(bytes)) = super::read_checked(&path) else {
            continue;
        };
        let record = serde_json::from_slice::<super::EraMarker>(&bytes).ok();
        if let Some(marker) = aside_marker(&path) {
            // An aside of a pre-record marker (the bare admin name) is judged
            // against that admin name's entry.
            // trace:TASK-b6b1278890774ef993d2ebc327862470-072 | ai:claude
            let (admin, admin_dir) = match record {
                Some(record) => (record.admin, record.admin_dir),
                None => match bare_admin_name(&bytes) {
                    Some(admin) => (admin.to_string(), super::admin_dir(common, admin)),
                    None => {
                        pass.out.corrupt.push(path);
                        continue;
                    }
                },
            };
            if pass.admit(&path, STALE_AFTER, || marker_owner(common, &admin))
                && (!reap || settle_aside(&path, &marker, &admin_dir))
            {
                pass.out.reapable.push(path);
            }
            continue;
        }
        // Only a marker that names its admin entry is judged here; a
        // pre-record one is judged when its checkout next resolves.
        let Some(record) = record else {
            continue;
        };
        if super::era_marker_stale(&path, &record.admin_dir)
            && pass.admit(&path, Duration::ZERO, || {
                marker_owner(common, &record.admin)
            })
            && (!reap
                || super::drop_stale_era_marker(anchor, &path, &record.admin_dir).unwrap_or(false))
        {
            pass.out.reapable.push(path);
        }
    }
}

/// A pre-record marker body: the bare admin name. Empty is the main
/// checkout's; any other is a linked worktree's, a single path component (at
/// most 255 bytes, no `/` or NUL) that cannot be a fragment of a JSON record
/// (no brace or quote). Anything else is a torn or foreign body and is
/// undecidable.
// trace:TASK-b6b1278890774ef993d2ebc327862470-075 | ai:claude
fn bare_admin_name(bytes: &[u8]) -> Option<&str> {
    let name = std::str::from_utf8(bytes).ok()?;
    let component = name.len() <= 255
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| matches!(c, '/' | '\0' | '{' | '}' | '"'));
    let valid = name.is_empty() || component;
    valid.then_some(name)
}

/// Remove [`write_source_id`](super::write_source_id) temp files in `dir`
/// older than [`STALE_AFTER`], left by a writer that crashed before placing
/// them. An unattributed one (under the anchor) whose owner cannot be
/// judged goes once older than [`ORPHAN_TEMP_AFTER`] instead of staying.
// trace:TASK-b6b1278890774ef993d2ebc327862470-075 | ai:claude
// trace:TASK-3478 | ai:claude
fn scan_id_temps(dir: &Path, owner: &Owner, pass: &mut Pass<'_>) {
    let reap = pass.reap;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with(super::ID_TEMP_PREFIX) && name.ends_with(".tmp")
        })
        .map(|e| e.path())
        .collect();
    paths.sort();
    for path in paths {
        let file = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        if file
            && pass.admit_with(&path, STALE_AFTER, Some(ORPHAN_TEMP_AFTER), || {
                owner.clone()
            })
            && (!reap || std::fs::remove_file(&path).is_ok())
        {
            pass.out.reapable.push(path);
        }
    }
}

/// The marker a drop aside (`.<marker>.drop.<id>`, left by a build that
/// renamed markers aside to drop them) belongs to.
// trace:TASK-b6b1278890774ef993d2ebc327862470-067 | ai:claude
fn aside_marker(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let (marker, _) = name.strip_prefix('.')?.split_once(".drop.")?;
    Some(path.with_file_name(marker))
}

/// Settle an old drop aside: a fresh one whose marker is absent (its
/// dropper crashed before linking it back) is linked back first, never over
/// a marker that stands; then the aside is removed. Gone counts as settled.
// trace:TASK-b6b1278890774ef993d2ebc327862470-067 | ai:claude
fn settle_aside(aside: &Path, marker: &Path, admin_dir: &Path) -> bool {
    if !super::era_marker_stale(aside, admin_dir) {
        match std::fs::hard_link(aside, marker) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) => return false,
        }
    }
    match std::fs::remove_file(aside) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Is `path` a real directory last changed at least [`STALE_AFTER`] before
/// `now`? Its change time counts, so a rename into place restarts the clock.
fn is_stale_dir(path: &Path, now: SystemTime) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
        && is_older(path, now, STALE_AFTER)
}

/// Was `path` last changed (or renamed) at least `floor` before `now`?
fn is_older(path: &Path, now: SystemTime, floor: Duration) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let changed = u64::try_from(meta.ctime())
        .ok()
        .map(|s| UNIX_EPOCH + Duration::new(s, u32::try_from(meta.ctime_nsec()).unwrap_or(0)));
    let latest = match (meta.modified().ok(), changed) {
        (Some(m), Some(c)) => m.max(c),
        (m, c) => match m.or(c) {
            Some(t) => t,
            None => return false,
        },
    };
    now.duration_since(latest).is_ok_and(|age| age >= floor)
}

/// Remove an empty tomb: its subdirectories, its nonce, then itself. A file
/// that appears meanwhile makes a step fail and the tomb stays.
fn remove_empty_tomb(tomb: &Path) -> std::io::Result<()> {
    super::guard::remove_empty_tree_in(tomb, true)?;
    remove_nonce(tomb)?;
    std::fs::remove_dir(tomb)
}

/// Remove a staging directory: its nonce, then itself. Anything else in it
/// makes `remove_dir` fail and it stays.
fn remove_staging(dir: &Path) -> std::io::Result<()> {
    remove_nonce(dir)?;
    std::fs::remove_dir(dir)
}

fn remove_nonce(dir: &Path) -> std::io::Result<()> {
    let nonce = dir.join(super::SOURCE_ID_FILE);
    match std::fs::symlink_metadata(&nonce) {
        Ok(meta) if super::is_source_id(super::SOURCE_ID_FILE, &meta) => {
            std::fs::remove_file(&nonce)
        }
        Ok(_) => Err(std::io::Error::other(format!(
            "{} is not a creation nonce",
            nonce.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
