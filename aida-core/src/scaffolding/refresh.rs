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

use std::path::{Path, PathBuf};

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
            RefreshOutcome::KeptEdited => self.kept_edited.push(path.to_path_buf()),
            RefreshOutcome::KeptUnmarked => self.kept_unmarked.push(path.to_path_buf()),
            RefreshOutcome::SkippedSymlink(_) => self.skipped_symlink.push(path.to_path_buf()),
        }
    }

    /// How many files this pass actually rewrote.
    pub fn changed(&self) -> usize {
        self.refreshed.len() + self.adopted.len()
    }

    /// Merge another pack's tallies into this one.
    pub fn absorb(&mut self, other: &RefreshReport) {
        self.refreshed.extend(other.refreshed.iter().cloned());
        self.adopted.extend(other.adopted.iter().cloned());
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
