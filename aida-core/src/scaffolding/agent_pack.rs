//! Exact renderer and filesystem synchronization for the developer portable
//! skill pack, sharing the scaffold inventory and generated-file oracle.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::inventory;
use super::{embedded_template_or_placeholder, wrap_with_aida_header, ScaffoldConfig};

/// Every file the full scaffold writes into portable skill pack `pack`, keyed
/// by path relative to the project root.
// trace:TASK-1520 | ai:codex
pub fn rendered_portable_pack(pack: &str, config: &ScaffoldConfig) -> BTreeMap<PathBuf, String> {
    let mut rendered = BTreeMap::new();
    for skill in inventory::portable_skill_inventory(config).values() {
        for file in &skill.files {
            let rel = Path::new(pack).join(&skill.name).join(&file.rel_path);
            let body = embedded_template_or_placeholder(&file.source_key);
            rendered.insert(rel.clone(), wrap_with_aida_header(&rel, &body));
        }
    }
    rendered
}

/// One way a pack file on disk differs from what the scaffolder would write.
// trace:TASK-1520 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackDrift {
    /// An expected file is absent.
    Missing(PathBuf),
    /// An expected file exists with different bytes.
    Modified(PathBuf),
    /// An expected file is a symlink; Codex 0.157 silently skips a symlinked SKILL.md.
    Symlink(PathBuf),
    /// An `aida-`-prefixed entry under the pack that the derived inventory does not contain.
    Orphan(PathBuf),
}

/// Drift between `root`'s portable pack and what the scaffolder would write.
/// A missing pack directory is a clean no-op for fresh clones and CI.
// trace:TASK-1520 | ai:codex
pub fn check_portable_pack(
    root: &Path,
    pack: &str,
    config: &ScaffoldConfig,
) -> io::Result<Vec<PackDrift>> {
    let pack_root = root.join(pack);
    if !pack_root.exists() {
        return Ok(Vec::new());
    }

    let expected = rendered_portable_pack(pack, config);
    let mut drift = Vec::new();

    // A symlinked skill DIRECTORY has to be found before the per-file loop:
    // `symlink_metadata` on a file *inside* it resolves through the link and
    // reports a pristine regular file, so the pack would read as clean.
    // trace:TASK-1520 | ai:claude
    let mut linked_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for rel in expected.keys() {
        if let Some(link) = symlinked_ancestor(root, pack, rel)? {
            linked_dirs.insert(link);
        }
    }
    for link in &linked_dirs {
        drift.push(PackDrift::Symlink(link.clone()));
    }

    for (rel, bytes) in &expected {
        if linked_dirs.iter().any(|link| rel.starts_with(link)) {
            continue;
        }
        let full = root.join(rel);
        match fs::symlink_metadata(&full) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                drift.push(PackDrift::Missing(rel.clone()));
            }
            Err(e) => return Err(e),
            Ok(meta) if meta.file_type().is_symlink() => {
                drift.push(PackDrift::Symlink(rel.clone()));
            }
            Ok(_) if fs::read(&full)? != bytes.as_bytes() => {
                drift.push(PackDrift::Modified(rel.clone()));
            }
            Ok(_) => {}
        }
    }
    drift.extend(find_orphans(root, pack, &expected)?);
    drift.sort_by(|a, b| drift_path(a).cmp(drift_path(b)));
    drift.dedup();
    Ok(drift)
}

/// What a sync did, for operator-facing output.
// trace:TASK-1520 | ai:codex
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PackSyncReport {
    /// Paths written because they were absent or differed.
    pub written: Vec<PathBuf>,
    /// Paths already containing the exact rendered bytes.
    pub unchanged: Vec<PathBuf>,
    /// Orphaned AIDA paths removed from the pack.
    pub removed: Vec<PathBuf>,
}

/// Materialize the portable pack under `root` as regular files, retaining
/// byte-identical files so an idempotent sync does not change mtimes.
// trace:TASK-1520 | ai:codex
pub fn sync_portable_pack(
    root: &Path,
    pack: &str,
    config: &ScaffoldConfig,
) -> io::Result<PackSyncReport> {
    let pack_root = root.join(pack);
    fs::create_dir_all(&pack_root)?;
    let expected = rendered_portable_pack(pack, config);
    let orphans = find_orphans(root, pack, &expected)?;
    let mut report = PackSyncReport::default();

    for (rel, bytes) in &expected {
        let full = root.join(rel);
        // Clear a symlinked skill directory before `create_dir_all`, or every
        // write below lands in the link's target — plausibly a template master
        // in this same working tree. trace:TASK-1520 | ai:claude
        while let Some(link) = symlinked_ancestor(root, pack, rel)? {
            fs::remove_file(root.join(&link))?;
            report.removed.push(link);
        }
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::symlink_metadata(&full).is_ok_and(|meta| meta.file_type().is_symlink()) {
            // Remove the link first: writing through it could corrupt a template master in this tree.
            fs::remove_file(&full)?;
        }
        match fs::read(&full) {
            Ok(current) if current == bytes.as_bytes() => report.unchanged.push(rel.clone()),
            Ok(_) => {
                fs::write(&full, bytes.as_bytes())?;
                report.written.push(rel.clone());
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::write(&full, bytes.as_bytes())?;
                report.written.push(rel.clone());
            }
            Err(e) => return Err(e),
        }
    }

    for drift in orphans {
        let PackDrift::Orphan(rel) = drift else {
            continue;
        };
        if !rel
            .strip_prefix(Path::new(pack))
            .ok()
            .and_then(|path| path.components().next())
            .is_some_and(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .starts_with(inventory::AIDA_SKILL_PREFIX)
            })
        {
            continue;
        }
        let full = root.join(&rel);
        let meta = fs::symlink_metadata(&full)?;
        if meta.file_type().is_dir() {
            fs::remove_dir_all(full)?;
        } else {
            fs::remove_file(full)?;
        }
        report.removed.push(rel);
    }
    report.written.sort();
    report.unchanged.sort();
    report.removed.sort();
    Ok(report)
}

/// The nearest directory between `<pack>` and `rel` that is itself a symlink.
///
/// This is the case the per-file symlink check cannot see. Codex 0.157.0 does
/// read a symlinked skill *directory* (BUG-1639's matrix), and the `.claude/`
/// side of `make sync-templates` links folder-form skills exactly that way, so
/// an operator can reach this state by hand. Left undetected, the pack reads as
/// clean and the next sync writes through the link into its target.
// trace:TASK-1520 | ai:claude
fn symlinked_ancestor(root: &Path, pack: &str, rel: &Path) -> io::Result<Option<PathBuf>> {
    let pack_path = Path::new(pack);
    let Ok(inner) = rel.strip_prefix(pack_path) else {
        return Ok(None);
    };
    let mut parts: Vec<_> = inner.components().collect();
    // The file itself is the per-file check's business, not this one's.
    parts.pop();
    let mut probe = pack_path.to_path_buf();
    for part in parts {
        probe = probe.join(part);
        match fs::symlink_metadata(root.join(&probe)) {
            Ok(meta) if meta.file_type().is_symlink() => return Ok(Some(probe)),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

fn drift_path(drift: &PackDrift) -> &PathBuf {
    match drift {
        PackDrift::Missing(p)
        | PackDrift::Modified(p)
        | PackDrift::Symlink(p)
        | PackDrift::Orphan(p) => p,
    }
}

fn find_orphans(
    root: &Path,
    pack: &str,
    expected: &BTreeMap<PathBuf, String>,
) -> io::Result<Vec<PackDrift>> {
    let pack_root = root.join(pack);
    if !pack_root.exists() {
        return Ok(Vec::new());
    }
    let pack_path = Path::new(pack);
    let skills: BTreeSet<_> = expected
        .keys()
        .filter_map(|path| path.strip_prefix(pack_path).ok()?.components().next())
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let mut orphans = Vec::new();
    for entry in fs::read_dir(&pack_root)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name
            .to_string_lossy()
            .starts_with(inventory::AIDA_SKILL_PREFIX)
        {
            continue;
        }
        let rel = Path::new(pack).join(&name);
        if !skills.contains(&name) {
            orphans.push(PackDrift::Orphan(rel));
        } else if entry.file_type()?.is_dir() {
            collect_unexpected_files(root, &rel, expected, &mut orphans)?;
        }
    }
    Ok(orphans)
}

fn collect_unexpected_files(
    root: &Path,
    rel_dir: &Path,
    expected: &BTreeMap<PathBuf, String>,
    out: &mut Vec<PackDrift>,
) -> io::Result<()> {
    for entry in fs::read_dir(root.join(rel_dir))? {
        let entry = entry?;
        let rel = rel_dir.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_unexpected_files(root, &rel, expected, out)?;
        } else if !expected.contains_key(&rel) {
            out.push(PackDrift::Orphan(rel));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ScaffoldConfig) {
        (TempDir::new().unwrap(), ScaffoldConfig::default())
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn sync_then_check_is_clean() {
        let (root, cfg) = setup();
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        assert!(root
            .path()
            .join(inventory::PORTABLE_PACK)
            .join("aida-capture/SKILL.md")
            .is_file());
        assert!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn mutating_one_synced_byte_is_drift() {
        let (root, cfg) = setup();
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        let rel = Path::new(inventory::PORTABLE_PACK).join("aida-capture/SKILL.md");
        let full = root.path().join(&rel);
        let mut bytes = fs::read(&full).unwrap();
        bytes.push(b'x');
        fs::write(full, bytes).unwrap();
        assert_eq!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap(),
            vec![PackDrift::Modified(rel)]
        );
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn every_synced_skill_md_is_a_regular_file() {
        let (root, cfg) = setup();
        let inventory = inventory::portable_skill_inventory(&cfg);
        assert!(
            inventory.len() >= 20,
            "inventory unexpectedly small: {}",
            inventory.len()
        );
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        for name in inventory.keys() {
            let path = root
                .path()
                .join(inventory::PORTABLE_PACK)
                .join(name)
                .join("SKILL.md");
            let meta = fs::symlink_metadata(path).unwrap();
            assert!(meta.file_type().is_file() && !meta.file_type().is_symlink());
        }
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn folder_form_helpers_land_at_the_scaffolder_sub_paths() {
        let (root, cfg) = setup();
        let inventory = inventory::portable_skill_inventory(&cfg);
        assert!(
            inventory.contains_key("aida-pr"),
            "re-point this test at the replacement folder-form skill"
        );
        let oracle = rendered_portable_pack(inventory::PORTABLE_PACK, &cfg);
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        for sub in ["SKILL.md", "examples/pr-description-template.md"] {
            let rel = Path::new(inventory::PORTABLE_PACK)
                .join("aida-pr")
                .join(sub);
            let path = root.path().join(&rel);
            assert!(fs::symlink_metadata(&path).unwrap().file_type().is_file());
            assert_eq!(fs::read_to_string(path).unwrap(), oracle[&rel]);
        }
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn skill_md_bytes_equal_the_scaffolder_oracle() {
        let cfg = ScaffoldConfig::default();
        let skills = inventory::portable_skill_inventory(&cfg);
        let rendered = rendered_portable_pack(inventory::PORTABLE_PACK, &cfg);
        assert!(!skills.is_empty());
        for name in skills.keys() {
            let rel = Path::new(inventory::PORTABLE_PACK)
                .join(name)
                .join("SKILL.md");
            assert_eq!(
                rendered[&rel].as_bytes(),
                crate::scaffolding::rendered_pack_skill(inventory::PORTABLE_PACK, name).as_bytes()
            );
        }
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn a_non_aida_directory_is_untouched_by_a_sync() {
        let (root, cfg) = setup();
        for (dir, file, bytes) in [
            ("typesafe-ai", "SKILL.md", b"third party".as_slice()),
            ("local", "README.md", b"local skill".as_slice()),
        ] {
            let path = root
                .path()
                .join(inventory::PORTABLE_PACK)
                .join(dir)
                .join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        let drift = check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        for (dir, file, bytes) in [
            ("typesafe-ai", "SKILL.md", b"third party".as_slice()),
            ("local", "README.md", b"local skill".as_slice()),
        ] {
            let rel = Path::new(inventory::PORTABLE_PACK).join(dir).join(file);
            assert_eq!(fs::read(root.path().join(&rel)).unwrap(), bytes);
            let dir_rel = Path::new(inventory::PORTABLE_PACK).join(dir);
            assert!(!drift.iter().any(|d| drift_path(d).starts_with(&dir_rel)));
        }
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn an_orphan_aida_directory_is_drift_and_a_sync_removes_it() {
        let (root, cfg) = setup();
        let rel = Path::new(inventory::PORTABLE_PACK).join("aida-not-a-real-skill");
        fs::create_dir_all(root.path().join(&rel)).unwrap();
        fs::write(root.path().join(&rel).join("SKILL.md"), "orphan").unwrap();
        assert!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg)
                .unwrap()
                .contains(&PackDrift::Orphan(rel.clone()))
        );
        let report = sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        assert!(!root.path().join(&rel).exists());
        assert!(report.removed.contains(&rel));
        assert!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn an_unexpected_file_inside_a_managed_skill_is_drift() {
        let (root, cfg) = setup();
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        let rel = Path::new(inventory::PORTABLE_PACK).join("aida-capture/LEFTOVER.md");
        fs::write(root.path().join(&rel), "extra").unwrap();
        assert!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg)
                .unwrap()
                .contains(&PackDrift::Orphan(rel.clone()))
        );
        assert!(
            sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg)
                .unwrap()
                .removed
                .contains(&rel)
        );
    }

    #[cfg(unix)]
    #[test]
    // trace:TASK-1520 | ai:codex
    fn a_symlinked_skill_md_is_drift_and_a_sync_replaces_it_without_writing_through() {
        let (root, cfg) = setup();
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        let rel = Path::new(inventory::PORTABLE_PACK).join("aida-capture/SKILL.md");
        let full = root.path().join(&rel);
        let scratch = root.path().join("scratch-outside-pack");
        fs::write(&scratch, b"safe bytes").unwrap();
        fs::remove_file(&full).unwrap();
        symlink(&scratch, &full).unwrap();
        assert_eq!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap(),
            vec![PackDrift::Symlink(rel.clone())]
        );
        let oracle = rendered_portable_pack(inventory::PORTABLE_PACK, &cfg)[&rel].clone();
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        let meta = fs::symlink_metadata(&full).unwrap();
        assert!(meta.file_type().is_file() && !meta.file_type().is_symlink());
        assert_eq!(fs::read(&full).unwrap(), oracle.as_bytes());
        assert_eq!(fs::read(scratch).unwrap(), b"safe bytes");
    }

    /// The write-through hazard: a symlinked skill directory must be drift, and
    /// a sync must replace it without touching the link's target.
    #[cfg(unix)]
    #[test]
    // trace:TASK-1520 | ai:claude
    fn a_symlinked_skill_directory_is_drift_and_a_sync_never_writes_through_it() {
        let (root, cfg) = setup();
        let pack = Path::new(inventory::PORTABLE_PACK);
        // Stands in for a template master inside the working tree.
        let master = root.path().join("pretend-templates/aida-capture");
        fs::create_dir_all(&master).unwrap();
        fs::write(master.join("SKILL.md"), b"pristine master").unwrap();
        fs::create_dir_all(root.path().join(pack)).unwrap();
        let linked = pack.join("aida-capture");
        symlink(&master, root.path().join(&linked)).unwrap();

        let drift = check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        assert!(
            drift.contains(&PackDrift::Symlink(linked.clone())),
            "a symlinked skill directory must be reported as a symlink: {drift:?}"
        );
        assert!(
            !drift.iter().any(|d| matches!(
                d,
                PackDrift::Modified(p) | PackDrift::Missing(p) if p.starts_with(&linked)
            )),
            "the link, not its contents, is the finding: {drift:?}"
        );

        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        let meta = fs::symlink_metadata(root.path().join(&linked)).unwrap();
        assert!(meta.file_type().is_dir() && !meta.file_type().is_symlink());
        assert_eq!(
            fs::read(master.join("SKILL.md")).unwrap(),
            b"pristine master",
            "the sync wrote through the symlink and corrupted its target"
        );
        assert!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn a_missing_pack_directory_is_a_no_op() {
        let (root, cfg) = setup();
        assert!(
            check_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg)
                .unwrap()
                .is_empty()
        );
        assert!(!root.path().join(inventory::PORTABLE_PACK).exists());
    }

    #[test]
    // trace:TASK-1520 | ai:codex
    fn a_second_sync_writes_nothing() {
        let (root, cfg) = setup();
        sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        let second = sync_portable_pack(root.path(), inventory::PORTABLE_PACK, &cfg).unwrap();
        assert!(second.written.is_empty());
        assert!(second.removed.is_empty());
        assert!(!second.unchanged.is_empty());
    }
}
