//! Edit-preserving scaffold refresh — the generalization of the starter
//! memory pack's `--refresh` contract to every agent pack AIDA ships
//! (`.claude/skills/`, `.claude/commands/`, `.codex/skills/`,
//! `.antigravity/skills/`, `~/.codex/prompts/`).
//!
//! The contract, identical to the memory pack's:
//!
//! - a file whose recorded scaffold checksum still matches its on-disk body
//!   is **pristine** — the user has not touched it, so a newer version of the
//!   template may be overlaid;
//! - a file whose body no longer matches its recorded checksum is **edited** —
//!   the user's version wins and is never overwritten;
//! - a file with no scaffold marker at all is **unmarked** — AIDA cannot prove
//!   it wrote it, so it is left alone;
//! - a **symlinked** destination is never written through (BUG-718: in the AIDA
//!   dev repo the scaffold files are per-file symlinks into
//!   `aida-core/templates/`, and following one would corrupt the master).
//!
//! The marker is the existing `AIDA Generated: v… | checksum:…` header that
//! `wrap_with_aida_header` already stamps onto every scaffolded file. The one
//! difference from `check_file_status` is what the stored checksum is compared
//! *against*: `check_file_status` compares it to the checksum of the CURRENT
//! template (answering "is this copy stale?"), whereas refresh compares it to
//! the checksum of the file's OWN on-disk body (answering "has the user edited
//! this?"). Both questions are needed — refresh overlays exactly the files that
//! are stale AND unedited.
// trace:TASK-1170 | ai:claude

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Pack-local file listing every skill AIDA has delivered into that skill
/// pack directory, plus the delivered skills the user has since deleted
/// (opt-outs). One manifest per pack directory (`<pack>/.aida-delivered`),
/// whatever the pack's vendor, so a pack that moves (BUG-1639) keeps the
/// same mechanism.
///
/// Format (one skill name per line, `#` lines are comments):
///
/// ```text
/// # aida-delivered v2
/// # ...
/// aida-commit
/// aida-req
/// # opted-out: delivered, then deleted by the user
/// aida-orchestrate
/// ```
///
/// Every name line, in either section, means "AIDA has shipped this name
/// here before", so it is never re-created. A STORY-1475 manifest (no `v2`
/// marker) lists only allow-listed skills and is treated as legacy: it is
/// re-seeded from the directories on disk.
// trace:STORY-1475 | ai:claude
// trace:TASK-1503 | ai:claude
pub const DELIVERED_MANIFEST: &str = ".aida-delivered";

/// First line of a complete (TASK-1503) manifest.
// trace:TASK-1503 | ai:claude
const MANIFEST_V2_MARKER: &str = "# aida-delivered v2";

/// Comment line that starts the opt-out section.
// trace:TASK-1503 | ai:claude
const OPTED_OUT_MARKER: &str = "# opted-out";

/// The parsed contents of one pack's [`DELIVERED_MANIFEST`].
// trace:TASK-1503 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillManifest {
    /// Skills AIDA delivered that are (as of the last write) still installed.
    pub delivered: BTreeSet<String>,
    /// Skills AIDA delivered that the user deleted: the opt-out record.
    pub opted_out: BTreeSet<String>,
    /// `true` when the manifest records EVERY delivered skill (the v2
    /// format). `false` for a STORY-1475 allow-list-only manifest.
    pub complete: bool,
}

impl SkillManifest {
    /// Has AIDA ever shipped `name` into this pack?
    pub fn knows(&self, name: &str) -> bool {
        self.delivered.contains(name) || self.opted_out.contains(name)
    }

    fn parse(text: &str) -> Self {
        let mut manifest = SkillManifest::default();
        let mut in_opted_out = false;
        for line in text.lines().map(str::trim) {
            if line == MANIFEST_V2_MARKER {
                manifest.complete = true;
            } else if line.starts_with(OPTED_OUT_MARKER) {
                in_opted_out = true;
            } else if line.is_empty() || line.starts_with('#') {
                continue;
            } else if in_opted_out {
                manifest.opted_out.insert(line.to_string());
            } else {
                manifest.delivered.insert(line.to_string());
            }
        }
        manifest
            .delivered
            .retain(|n| !manifest.opted_out.contains(n));
        manifest
    }

    fn render(&self) -> String {
        let mut out = format!(
            "{MANIFEST_V2_MARKER}\n\
             # Skills AIDA has delivered into this pack. A listed skill is never\n\
             # re-created by `aida init`, `aida scaffold upgrade` or\n\
             # `aida scaffold refresh`, so deleting one sticks. Remove a line to\n\
             # have AIDA deliver that skill again.\n"
        );
        for name in &self.delivered {
            out.push_str(name);
            out.push('\n');
        }
        if !self.opted_out.is_empty() {
            out.push_str(OPTED_OUT_MARKER);
            out.push_str(": delivered, then deleted by the user\n");
            for name in &self.opted_out {
                out.push_str(name);
                out.push('\n');
            }
        }
        out
    }
}

/// Read `pack_dir`'s [`DELIVERED_MANIFEST`].
///
/// Fails closed: only a missing manifest (`NotFound`) means "no manifest"
/// (`Ok(None)`). Any other read failure (permissions, invalid UTF-8, a
/// directory in its place) is an error, so callers never treat an unreadable
/// manifest as empty and re-create a skill the user deleted.
// trace:STORY-1475 | ai:claude
// trace:TASK-1503 | ai:claude
pub fn read_skill_manifest(pack_dir: &Path) -> std::io::Result<Option<SkillManifest>> {
    match std::fs::read_to_string(pack_dir.join(DELIVERED_MANIFEST)) {
        Ok(s) => Ok(Some(SkillManifest::parse(&s))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Replace `pack_dir`'s manifest atomically. Refuses to write into a
/// symlinked pack directory (BUG-718: it may point at a template master).
// trace:TASK-1503 | ai:claude
pub fn write_skill_manifest(pack_dir: &Path, manifest: &SkillManifest) -> std::io::Result<()> {
    if symlink_target(pack_dir).is_some() {
        return Err(std::io::Error::other(format!(
            "{} is a symlink; not writing its delivered-skills manifest",
            pack_dir.display()
        )));
    }
    crate::write_atomic(&pack_dir.join(DELIVERED_MANIFEST), manifest.render())
}

/// If `rel` is `<pack>/<name>/SKILL.md` where `<pack>`'s last component is
/// `skills`, return the pack directory and the skill name. Works for any pack
/// root (`.claude/skills`, `.codex/skills`, `.antigravity/skills`,
/// `.agents/skills`, ...).
// trace:TASK-1503 | ai:claude
pub fn skill_in_pack(rel: &Path) -> Option<(PathBuf, String)> {
    if rel.file_name()? != "SKILL.md" {
        return None;
    }
    let skill_dir = rel.parent()?;
    let name = skill_dir.file_name()?.to_str()?;
    if name.is_empty() || name.starts_with('.') {
        return None;
    }
    let pack = skill_dir.parent()?;
    (pack.file_name()? == "skills").then(|| (pack.to_path_buf(), name.to_string()))
}

/// Is `name` installed in `pack_dir` in any form the user owns: a directory
/// (or symlink) `<name>/`, or the pre-BUG-1135 flat file `<name>.md`?
// trace:TASK-1503 | ai:claude
pub fn skill_present(pack_dir: &Path, name: &str) -> bool {
    pack_dir.join(name).symlink_metadata().is_ok()
        || pack_dir
            .join(format!("{name}.md"))
            .symlink_metadata()
            .is_ok()
}

/// Which command is planning the pack.
// trace:TASK-1503 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestMode {
    /// `aida init` / `scaffold apply` / `scaffold upgrade`: installing. A pack
    /// with no manifest and none of AIDA's skills on disk is a fresh install
    /// and receives every skill.
    Install,
    /// `aida scaffold refresh`: converging an installed pack. A pack without a
    /// complete manifest is legacy, and the first refresh creates nothing.
    Refresh,
}

/// The delivery decision for one skill pack directory.
// trace:TASK-1503 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPackPlan {
    /// The pack directory, relative to the project root.
    pub pack: PathBuf,
    /// Skill names the current binary ships into this pack.
    pub shipped: BTreeSet<String>,
    /// Shipped skills that are missing on disk and must NOT be created:
    /// delivered before (so deleting them was an opt-out), or unknowable
    /// (legacy pack, unreadable manifest).
    pub withheld: BTreeSet<String>,
    /// The manifest to build on. `None` when it is unreadable or the pack
    /// directory is a symlink: nothing is ever written then.
    pub base: Option<SkillManifest>,
    /// The manifest as it was read (`None` when absent or unreadable), so a
    /// write happens only when something changed.
    pub on_disk: Option<SkillManifest>,
    /// Why the pack could not be tracked, for the caller to print.
    pub warning: Option<String>,
}

impl SkillPackPlan {
    /// Shipped skills that are missing on disk and may be created now.
    pub fn deliverable(&self, project_root: &Path) -> BTreeSet<String> {
        let pack_dir = project_root.join(&self.pack);
        self.shipped
            .iter()
            .filter(|n| !self.withheld.contains(*n) && !skill_present(&pack_dir, n))
            .cloned()
            .collect()
    }

    /// The manifest once `delivering` has been written: every shipped skill
    /// present on disk (or being delivered) is delivered; every known one
    /// that is missing is an opt-out. `None` when the pack is untrackable.
    pub fn settled(
        &self,
        project_root: &Path,
        delivering: &BTreeSet<String>,
    ) -> Option<SkillManifest> {
        let mut manifest = self.base.clone()?;
        let pack_dir = project_root.join(&self.pack);
        for name in &self.shipped {
            if skill_present(&pack_dir, name) || delivering.contains(name) {
                manifest.opted_out.remove(name);
                manifest.delivered.insert(name.clone());
            } else if manifest.delivered.contains(name) || self.withheld.contains(name) {
                manifest.delivered.remove(name);
                manifest.opted_out.insert(name.clone());
            }
        }
        manifest.complete = true;
        Some(manifest)
    }

    /// Write [`Self::settled`] if it differs from what is on disk. `Ok(false)`
    /// when nothing was written (no change, or an untrackable pack).
    pub fn record(
        &self,
        project_root: &Path,
        delivering: &BTreeSet<String>,
    ) -> std::io::Result<bool> {
        let Some(manifest) = self.settled(project_root, delivering) else {
            return Ok(false);
        };
        if self.on_disk.as_ref() == Some(&manifest) {
            return Ok(false);
        }
        write_skill_manifest(&project_root.join(&self.pack), &manifest)?;
        Ok(true)
    }
}

/// Decide what may be created in the skill pack at `pack` (relative to
/// `project_root`), given the skill names this binary ships there.
///
/// - Complete manifest: a missing skill is created only when the manifest has
///   never recorded its name.
/// - No manifest (or a STORY-1475 allow-list manifest) and some of AIDA's
///   skills on disk, or any such pack under [`ManifestMode::Refresh`]: a legacy
///   pack. It is seeded from the directories present and nothing is created,
///   because a missing skill may be one the user deleted.
/// - No manifest, no AIDA skill on disk, [`ManifestMode::Install`]: a fresh
///   install; every shipped skill is delivered.
/// - Unreadable manifest or symlinked pack directory: fail closed — nothing is
///   created and nothing is written.
// trace:TASK-1503 | ai:claude
pub fn plan_skill_pack(
    project_root: &Path,
    pack: &Path,
    shipped: BTreeSet<String>,
    mode: ManifestMode,
) -> SkillPackPlan {
    let pack_dir = project_root.join(pack);
    let missing: BTreeSet<String> = shipped
        .iter()
        .filter(|n| !skill_present(&pack_dir, n))
        .cloned()
        .collect();
    let untracked = |warning: String| SkillPackPlan {
        pack: pack.to_path_buf(),
        shipped: shipped.clone(),
        withheld: missing.clone(),
        base: None,
        on_disk: None,
        warning: Some(warning),
    };
    if symlink_target(&pack_dir).is_some() {
        return untracked(format!(
            "{} is a symlink; its missing skills are not created",
            pack.display()
        ));
    }
    let on_disk = match read_skill_manifest(&pack_dir) {
        Ok(m) => m,
        Err(e) => {
            return untracked(format!(
                "delivered-skills manifest in {} is unreadable ({e}); nothing created",
                pack.display()
            ))
        }
    };
    let (base, withheld) = match &on_disk {
        Some(m) if m.complete => {
            let withheld = missing.iter().filter(|n| m.knows(n)).cloned().collect();
            (m.clone(), withheld)
        }
        legacy => {
            let fresh =
                mode == ManifestMode::Install && legacy.is_none() && missing.len() == shipped.len();
            let base = legacy.clone().unwrap_or_default();
            if fresh {
                (base, BTreeSet::new())
            } else {
                (base, missing.clone())
            }
        }
    };
    SkillPackPlan {
        pack: pack.to_path_buf(),
        shipped,
        withheld,
        base: Some(base),
        on_disk,
        warning: None,
    }
}

use anyhow::{Context, Result};

use super::{checksum_for_stored_header, normalize_lf, symlink_target};

/// What a refresh pass may do with a file that already exists on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshDisposition {
    /// No `AIDA Generated` marker — AIDA cannot prove it wrote this file.
    Unmarked,
    /// Marked, but the body no longer hashes to the recorded checksum: the
    /// user has edited it. Their version wins.
    Edited,
    /// Marked and untouched since it was written — safe to overlay.
    Pristine,
}

/// Split an on-disk scaffolded file into `(stored_checksum, reconstructed_raw)`
/// where `reconstructed_raw` is the pre-header content `wrap_with_aida_header`
/// was called with. `None` when the file carries no recognizable AIDA header.
///
/// Anything unexpected (a header block an older binary wrote differently, a
/// truncated file) simply fails to reconstruct or fails the checksum compare,
/// which classifies the file as `Edited` — i.e. the safe, leave-it-alone side.
fn parse_aida_generated(content: &str) -> Option<(String, String)> {
    let content = normalize_lf(content);

    // The header sits after a shebang line (shell hooks) or after YAML
    // frontmatter (skills/commands), and at the top otherwise. Mirrors the
    // placement rules in `wrap_with_aida_header`.
    let split = if content.starts_with("#!") {
        content.find('\n').map(|nl| nl + 1).unwrap_or(0)
    } else if let Some(after_open) = content.strip_prefix("---\n") {
        after_open
            .find("\n---\n")
            .map(|close| 4 + close + 5)
            .unwrap_or(0)
    } else {
        0
    };
    let (prefix, rest) = content.split_at(split);

    let mut lines = rest.lines();
    let header_line = lines.next()?;

    // Markdown header: two comment lines then a blank separator line.
    if let Some(checksum) = header_line
        .strip_prefix("<!-- AIDA Generated: v")
        .and_then(|tail| tail.split("| checksum:").nth(1))
        .and_then(|tail| tail.split_whitespace().next())
    {
        let mut consumed = header_line.len() + 1;
        for _ in 0..2 {
            match lines.next() {
                Some(l) => consumed += l.len() + 1,
                None => return None,
            }
        }
        let body = rest.get(consumed..)?;
        return Some((checksum.to_string(), format!("{prefix}{body}")));
    }

    // Shell/TOML header: two comment lines, no blank separator.
    if let Some(checksum) = header_line
        .strip_prefix("# AIDA Generated: v")
        .and_then(|tail| tail.split("| checksum:").nth(1))
        .map(|tail| tail.trim())
    {
        let mut consumed = header_line.len() + 1;
        match lines.next() {
            Some(l) => consumed += l.len() + 1,
            None => return None,
        }
        let body = rest.get(consumed..)?;
        return Some((checksum.to_string(), format!("{prefix}{body}")));
    }

    None
}

/// Classify an existing scaffolded file for a refresh pass.
pub fn refresh_disposition(on_disk: &str) -> RefreshDisposition {
    match parse_aida_generated(on_disk) {
        None => RefreshDisposition::Unmarked,
        Some((stored, raw)) => {
            if checksum_for_stored_header(&raw) == stored {
                RefreshDisposition::Pristine
            } else {
                RefreshDisposition::Edited
            }
        }
    }
}

/// What a refresh pass actually did to one destination path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The file does not exist. Refresh converges what is installed; creating
    /// a pack the user never installed is `init`/`scaffold apply`'s job.
    Missing,
    /// The destination is a symlink — never written through (BUG-718).
    SkippedSymlink(PathBuf),
    /// Pristine and already current.
    Unchanged,
    /// Pristine and stale — overlaid with the current template.
    Refreshed,
    /// The user edited it; kept verbatim.
    KeptEdited,
    /// No scaffold marker; kept verbatim.
    KeptUnmarked,
    /// Unmarked, but the caller opted into adopting the pack into
    /// edit-tracking; the previous content was saved alongside first.
    Adopted(PathBuf),
    /// Absent, and the caller chose to deliver it as a newly shipped file
    /// into a pack the project already has installed.
    // trace:STORY-1475 | ai:claude
    Installed,
}

/// Overlay `expected` onto `dest` if and only if `dest` is an existing,
/// non-symlinked, pristine (unedited) scaffold file.
///
/// `adopt_unmarked` is the one-time migration door for a pack that shipped
/// before it carried a marker: the previous content is copied to a `.aida-bak`
/// sibling and the current template is written. Nothing is ever lost, and from
/// the next run on the file is marked, so the precise contract applies.
pub fn refresh_file(dest: &Path, expected: &str, adopt_unmarked: bool) -> Result<RefreshOutcome> {
    if let Some(target) = symlink_target(dest) {
        return Ok(RefreshOutcome::SkippedSymlink(target));
    }
    let Ok(existing) = std::fs::read_to_string(dest) else {
        return Ok(RefreshOutcome::Missing);
    };
    if normalize_lf(&existing) == normalize_lf(expected) {
        return Ok(RefreshOutcome::Unchanged);
    }
    match refresh_disposition(&existing) {
        RefreshDisposition::Pristine => {
            std::fs::write(dest, expected)
                .with_context(|| format!("refreshing {}", dest.display()))?;
            Ok(RefreshOutcome::Refreshed)
        }
        RefreshDisposition::Edited => Ok(RefreshOutcome::KeptEdited),
        RefreshDisposition::Unmarked if adopt_unmarked => {
            let mut backup = dest.as_os_str().to_os_string();
            backup.push(".aida-bak");
            let backup = PathBuf::from(backup);
            std::fs::write(&backup, &existing)
                .with_context(|| format!("backing up {}", dest.display()))?;
            std::fs::write(dest, expected)
                .with_context(|| format!("refreshing {}", dest.display()))?;
            Ok(RefreshOutcome::Adopted(backup))
        }
        RefreshDisposition::Unmarked => Ok(RefreshOutcome::KeptUnmarked),
    }
}

/// Per-disposition tallies for one pack (or a whole refresh run).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    pub refreshed: Vec<PathBuf>,
    pub adopted: Vec<PathBuf>,
    /// Newly shipped files delivered into an already-installed pack.
    // trace:STORY-1475 | ai:claude
    pub installed: Vec<PathBuf>,
    pub kept_edited: Vec<PathBuf>,
    pub kept_unmarked: Vec<PathBuf>,
    pub skipped_symlink: Vec<PathBuf>,
    pub unchanged: usize,
    pub missing: usize,
}

impl RefreshReport {
    /// Fold one file's outcome in.
    pub fn record(&mut self, path: &Path, outcome: RefreshOutcome) {
        match outcome {
            RefreshOutcome::Missing => self.missing += 1,
            RefreshOutcome::Unchanged => self.unchanged += 1,
            RefreshOutcome::Refreshed => self.refreshed.push(path.to_path_buf()),
            RefreshOutcome::Adopted(_) => self.adopted.push(path.to_path_buf()),
            RefreshOutcome::Installed => self.installed.push(path.to_path_buf()),
            RefreshOutcome::KeptEdited => self.kept_edited.push(path.to_path_buf()),
            RefreshOutcome::KeptUnmarked => self.kept_unmarked.push(path.to_path_buf()),
            RefreshOutcome::SkippedSymlink(_) => self.skipped_symlink.push(path.to_path_buf()),
        }
    }

    /// How many files this pass actually rewrote.
    pub fn changed(&self) -> usize {
        self.refreshed.len() + self.adopted.len() + self.installed.len()
    }

    /// Merge another pack's tallies into this one.
    pub fn absorb(&mut self, other: &RefreshReport) {
        self.refreshed.extend(other.refreshed.iter().cloned());
        self.adopted.extend(other.adopted.iter().cloned());
        self.installed.extend(other.installed.iter().cloned());
        self.kept_edited.extend(other.kept_edited.iter().cloned());
        self.kept_unmarked
            .extend(other.kept_unmarked.iter().cloned());
        self.skipped_symlink
            .extend(other.skipped_symlink.iter().cloned());
        self.unchanged += other.unchanged;
        self.missing += other.missing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn install_skill(pack_dir: &Path, name: &str) {
        std::fs::create_dir_all(pack_dir.join(name)).unwrap();
        std::fs::write(pack_dir.join(name).join("SKILL.md"), "body\n").unwrap();
    }

    // trace:STORY-1475 | ai:claude
    // trace:TASK-1503 | ai:claude
    #[test]
    fn manifest_missing_is_none_but_unreadable_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_skill_manifest(tmp.path()).unwrap(), None);

        let manifest = SkillManifest {
            delivered: names(&["aida-req"]),
            opted_out: names(&["aida-orchestrate"]),
            complete: true,
        };
        write_skill_manifest(tmp.path(), &manifest).unwrap();
        assert_eq!(read_skill_manifest(tmp.path()).unwrap(), Some(manifest));

        // Invalid UTF-8: an error, never treated as empty.
        let path = tmp.path().join(DELIVERED_MANIFEST);
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_skill_manifest(tmp.path()).is_err());

        // A directory in the manifest's place: an error too.
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join(DELIVERED_MANIFEST)).unwrap();
        assert!(read_skill_manifest(other.path()).is_err());
    }

    /// A STORY-1475 manifest (no v2 marker) parses as incomplete, and every
    /// name in it still counts as known.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn story_1475_manifest_parses_as_incomplete() {
        let m = SkillManifest::parse(
            "# Skills AIDA has delivered into this pack.\n# comment\naida-orchestrate\n",
        );
        assert!(!m.complete);
        assert!(m.knows("aida-orchestrate"));
        assert!(!m.knows("aida-req"));
    }

    #[test]
    fn skill_in_pack_accepts_any_pack_root() {
        // trace:TASK-1503 | ai:claude
        for pack in [
            ".claude/skills",
            ".codex/skills",
            ".antigravity/skills",
            ".agents/skills",
            "vendor/x/skills",
        ] {
            let rel = PathBuf::from(pack).join("aida-req/SKILL.md");
            assert_eq!(
                skill_in_pack(&rel),
                Some((PathBuf::from(pack), "aida-req".to_string())),
                "{pack}"
            );
        }
        assert_eq!(
            skill_in_pack(Path::new(".claude/skills/local/README.md")),
            None
        );
        assert_eq!(
            skill_in_pack(Path::new(".claude/commands/x/SKILL.md")),
            None
        );
        assert_eq!(skill_in_pack(Path::new(".claude/skills/aida-req.md")), None);
    }

    /// The manifest is per pack DIRECTORY: an arbitrary pack root is planned,
    /// delivers a new skill, withholds a deleted one and records the opt-out,
    /// and a sibling pack keeps its own manifest.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn manifest_works_for_an_arbitrary_pack_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pack = Path::new(".agents/skills");
        let pack_dir = root.join(pack);
        install_skill(&pack_dir, "aida-req");
        install_skill(&pack_dir, "aida-commit");
        write_skill_manifest(
            &pack_dir,
            &SkillManifest {
                delivered: names(&["aida-req", "aida-commit"]),
                opted_out: BTreeSet::new(),
                complete: true,
            },
        )
        .unwrap();
        // The user deletes aida-commit; AIDA now also ships aida-new.
        std::fs::remove_dir_all(pack_dir.join("aida-commit")).unwrap();
        let shipped = names(&["aida-req", "aida-commit", "aida-new"]);

        for mode in [ManifestMode::Install, ManifestMode::Refresh] {
            let plan = plan_skill_pack(root, pack, shipped.clone(), mode);
            assert_eq!(plan.withheld, names(&["aida-commit"]), "{mode:?}");
            assert_eq!(plan.deliverable(root), names(&["aida-new"]), "{mode:?}");
        }
        let plan = plan_skill_pack(root, pack, shipped, ManifestMode::Refresh);
        assert!(plan.record(root, &plan.deliverable(root)).unwrap());
        let m = read_skill_manifest(&pack_dir).unwrap().unwrap();
        assert!(m.complete);
        assert_eq!(m.delivered, names(&["aida-new", "aida-req"]));
        assert_eq!(m.opted_out, names(&["aida-commit"]), "opt-out recorded");

        // One manifest per pack directory: the sibling pack has none.
        let sibling = root.join(".codex/skills");
        install_skill(&sibling, "aida-req");
        assert_eq!(read_skill_manifest(&sibling).unwrap(), None);
        assert!(pack_dir.join(DELIVERED_MANIFEST).is_file());
    }

    /// Legacy pack (no manifest, some AIDA skills on disk): seeded from what
    /// is present, and nothing is created, under both modes. The seeded
    /// manifest records the missing names as opt-outs so a later pass still
    /// creates nothing that may have been deleted.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn legacy_pack_is_seeded_and_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pack = Path::new(".codex/skills");
        install_skill(&root.join(pack), "aida-req");
        let shipped = names(&["aida-req", "aida-commit"]);
        for mode in [ManifestMode::Install, ManifestMode::Refresh] {
            let plan = plan_skill_pack(root, pack, shipped.clone(), mode);
            assert!(plan.deliverable(root).is_empty(), "{mode:?}");
        }
        let plan = plan_skill_pack(root, pack, shipped.clone(), ManifestMode::Refresh);
        plan.record(root, &BTreeSet::new()).unwrap();
        let m = read_skill_manifest(&root.join(pack)).unwrap().unwrap();
        assert_eq!(m.delivered, names(&["aida-req"]));
        assert_eq!(m.opted_out, names(&["aida-commit"]));
        let again = plan_skill_pack(root, pack, shipped, ManifestMode::Refresh);
        assert!(again.deliverable(root).is_empty());
        assert!(!again.record(root, &BTreeSet::new()).unwrap(), "idempotent");
    }

    /// A fresh install (no manifest, none of AIDA's skills present) delivers
    /// everything; refresh on the same empty pack creates nothing.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn fresh_install_delivers_everything_but_refresh_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pack = Path::new(".codex/skills");
        std::fs::create_dir_all(root.join(pack)).unwrap();
        let shipped = names(&["aida-req", "aida-commit"]);
        let install = plan_skill_pack(root, pack, shipped.clone(), ManifestMode::Install);
        assert_eq!(install.deliverable(root), shipped);
        let refresh = plan_skill_pack(root, pack, shipped, ManifestMode::Refresh);
        assert!(refresh.deliverable(root).is_empty());
    }

    /// Unreadable manifest: fail closed, nothing deliverable, nothing written.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn unreadable_manifest_withholds_everything_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pack = Path::new(".codex/skills");
        let pack_dir = root.join(pack);
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(pack_dir.join(DELIVERED_MANIFEST), [0xff, 0xfe]).unwrap();
        let plan = plan_skill_pack(root, pack, names(&["aida-req"]), ManifestMode::Install);
        assert!(plan.warning.is_some());
        assert!(plan.deliverable(root).is_empty());
        assert!(!plan.record(root, &BTreeSet::new()).unwrap());
        assert_eq!(
            std::fs::read(pack_dir.join(DELIVERED_MANIFEST)).unwrap(),
            vec![0xff, 0xfe]
        );
    }

    /// A user-owned symlinked skill counts as present: it is recorded as
    /// delivered, never withheld-then-created, and never written through.
    // trace:TASK-1503 | ai:claude
    #[cfg(unix)]
    #[test]
    fn symlinked_skill_counts_as_present() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pack = Path::new(".codex/skills");
        let pack_dir = root.join(pack);
        std::fs::create_dir_all(&pack_dir).unwrap();
        let target = root.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, pack_dir.join("aida-req")).unwrap();
        let plan = plan_skill_pack(root, pack, names(&["aida-req"]), ManifestMode::Refresh);
        assert!(plan.deliverable(root).is_empty());
        plan.record(root, &BTreeSet::new()).unwrap();
        let m = read_skill_manifest(&pack_dir).unwrap().unwrap();
        assert!(m.delivered.contains("aida-req"));
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    }

    use crate::scaffolding::wrap_with_aida_header;
    use std::path::Path;

    fn skill(body: &str) -> String {
        format!("---\nname: demo\n---\n{body}")
    }

    #[test]
    fn pristine_marked_file_round_trips() {
        let raw = skill("# Demo\n\noriginal body\n");
        let on_disk = wrap_with_aida_header(Path::new(".claude/skills/demo.md"), &raw);
        assert_eq!(
            refresh_disposition(&on_disk),
            RefreshDisposition::Pristine,
            "{on_disk}"
        );
    }

    #[test]
    fn edited_body_is_detected() {
        let raw = skill("# Demo\n\noriginal body\n");
        let on_disk = wrap_with_aida_header(Path::new(".claude/skills/demo.md"), &raw);
        let edited = on_disk.replace("original body", "my own body");
        assert_eq!(refresh_disposition(&edited), RefreshDisposition::Edited);
    }

    #[test]
    fn unmarked_file_is_detected() {
        assert_eq!(
            refresh_disposition("---\nname: demo\n---\n# Demo\n"),
            RefreshDisposition::Unmarked
        );
        assert_eq!(
            refresh_disposition("plain text\n"),
            RefreshDisposition::Unmarked
        );
    }

    #[test]
    fn plain_markdown_and_shell_headers_round_trip() {
        let plain = wrap_with_aida_header(Path::new("docs/thing.md"), "# Plain\n\nbody\n");
        assert_eq!(refresh_disposition(&plain), RefreshDisposition::Pristine);

        let shell = wrap_with_aida_header(Path::new(".claude/hooks/x"), "#!/bin/sh\necho hi\n");
        assert_eq!(refresh_disposition(&shell), RefreshDisposition::Pristine);
        let tampered = shell.replace("echo hi", "echo mine");
        assert_eq!(refresh_disposition(&tampered), RefreshDisposition::Edited);
    }

    #[test]
    fn crlf_checkout_is_still_pristine() {
        let raw = skill("# Demo\n\noriginal body\n");
        let on_disk = wrap_with_aida_header(Path::new(".claude/skills/demo.md"), &raw);
        let crlf = on_disk.replace('\n', "\r\n");
        assert_eq!(refresh_disposition(&crlf), RefreshDisposition::Pristine);
    }

    #[test]
    fn refresh_overlays_pristine_keeps_edited_and_unmarked() {
        let tmp = tempfile::tempdir().unwrap();
        let path = |n: &str| tmp.path().join(n);

        let old = wrap_with_aida_header(
            Path::new(".claude/skills/demo.md"),
            &skill("# Demo\n\nold body\n"),
        );
        let new = wrap_with_aida_header(
            Path::new(".claude/skills/demo.md"),
            &skill("# Demo\n\nnew body\n"),
        );

        // (a) pristine + stale → overlaid
        std::fs::write(path("pristine.md"), &old).unwrap();
        assert_eq!(
            refresh_file(&path("pristine.md"), &new, false).unwrap(),
            RefreshOutcome::Refreshed
        );
        assert_eq!(std::fs::read_to_string(path("pristine.md")).unwrap(), new);

        // (b) pristine + already current → no-op
        assert_eq!(
            refresh_file(&path("pristine.md"), &new, false).unwrap(),
            RefreshOutcome::Unchanged
        );

        // (c) user-edited → kept verbatim
        let edited = old.replace("old body", "MY body");
        std::fs::write(path("edited.md"), &edited).unwrap();
        assert_eq!(
            refresh_file(&path("edited.md"), &new, false).unwrap(),
            RefreshOutcome::KeptEdited
        );
        assert_eq!(std::fs::read_to_string(path("edited.md")).unwrap(), edited);

        // (d) unmarked → kept verbatim
        let unmarked = "---\nname: demo\n---\n# Demo\n\nhand written\n";
        std::fs::write(path("unmarked.md"), unmarked).unwrap();
        assert_eq!(
            refresh_file(&path("unmarked.md"), &new, false).unwrap(),
            RefreshOutcome::KeptUnmarked
        );
        assert_eq!(
            std::fs::read_to_string(path("unmarked.md")).unwrap(),
            unmarked
        );

        // (e) missing → never created by refresh
        assert_eq!(
            refresh_file(&path("absent.md"), &new, false).unwrap(),
            RefreshOutcome::Missing
        );
        assert!(!path("absent.md").exists());
    }

    // BUG-718: the AIDA dev repo symlinks .claude/skills/* into
    // aida-core/templates/. Writing through the link would corrupt the master.
    #[cfg(unix)]
    #[test]
    fn symlinked_destination_is_never_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let master = tmp.path().join("master.md");
        let old = wrap_with_aida_header(
            Path::new(".claude/skills/demo.md"),
            &skill("# Demo\n\nold body\n"),
        );
        let new = wrap_with_aida_header(
            Path::new(".claude/skills/demo.md"),
            &skill("# Demo\n\nnew body\n"),
        );
        std::fs::write(&master, &old).unwrap();

        let link = tmp.path().join("linked.md");
        std::os::unix::fs::symlink(&master, &link).unwrap();

        let outcome = refresh_file(&link, &new, false).unwrap();
        assert!(
            matches!(outcome, RefreshOutcome::SkippedSymlink(_)),
            "symlinked destination must be skipped, got {outcome:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&master).unwrap(),
            old,
            "the symlink target (a source-of-truth master) must be untouched"
        );
    }

    #[test]
    fn adopt_unmarked_backs_up_before_overlaying() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("legacy.md");
        std::fs::write(&dest, "stale unmarked prompt\n").unwrap();

        let new = wrap_with_aida_header(Path::new("legacy.md"), "# Fresh\n\nbody\n");
        let outcome = refresh_file(&dest, &new, true).unwrap();
        let RefreshOutcome::Adopted(backup) = outcome else {
            panic!("expected adoption, got {outcome:?}");
        };
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), new);
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "stale unmarked prompt\n"
        );
        // Second pass: now marked + current → no-op, no second backup.
        assert_eq!(
            refresh_file(&dest, &new, true).unwrap(),
            RefreshOutcome::Unchanged
        );
    }
}
