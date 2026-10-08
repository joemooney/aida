//! Finite regular-file installation under the common owner's existing locks.
//!
//! This is not a publisher: it cannot discover repositories, acquire locks,
//! decide COMMIT, project history or release a process. The owner must retain
//! a durable journal before calling it, validate the root identities and keep
//! home/resource exclusion throughout. Git refs, directories, symlinks and
//! object closure require their respective adapters, not regular-file writes.
// trace:BUG-1808 | ai:codex

#![cfg(target_os = "linux")]

use super::durable::{DurableDirectory, DurableWriteError};
use super::records::{
    ContentDigest, FileImage, FileTarget, PreparedFileEffects, RecordError, TargetRoot,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

pub(super) type Payloads = BTreeMap<ContentDigest, Vec<u8>>;

#[derive(Debug, thiserror::Error)]
pub(super) enum EffectError {
    #[error("publication payload validation failed: {0}")]
    Record(#[from] RecordError),
    #[error("publication payload is missing")]
    MissingPayload,
    #[error("publication target root is missing")]
    MissingRoot,
    #[error("publication targets alias the same directory entry")]
    AliasedTarget,
    #[error("publication target matches neither recorded image: {0:?}")]
    Conflict(FileTarget),
    #[error("publication target could not be inspected: {0}")]
    Read(#[from] io::Error),
    #[error("publication file installation failed: {0}")]
    Write(#[from] DurableWriteError),
}

/// Selected by the publication owner from validated decision evidence ONLY.
/// A storage fault or an unreadable/malformed COMMIT is never `Restore`.
#[derive(Debug, Clone, Copy)]
pub(super) enum Direction {
    Restore,
    Complete,
}

struct PinnedEffect<'a> {
    parent: DurableDirectory,
    name: &'a str,
    target: &'a FileTarget,
    before: &'a FileImage,
    after: &'a FileImage,
}

/// Borrowed exact bytes cannot change between full closure validation and
/// application. Parent directory descriptors survive pathname replacement.
/// Locks remain the owner's responsibility; FDs do not serialize peer writes.
pub(super) struct VerifiedFileSet<'a> {
    effects: Vec<PinnedEffect<'a>>,
    payloads: &'a Payloads,
}

impl<'a> VerifiedFileSet<'a> {
    pub fn verify(
        effects: &'a PreparedFileEffects,
        payloads: &'a Payloads,
        roots: &BTreeMap<TargetRoot, DurableDirectory>,
    ) -> Result<Self, EffectError> {
        // Validate ALL before/after payloads before even inspecting targets.
        // An already-installed image never excuses missing rollback bytes.
        for effect in effects.as_slice() {
            for image in [&effect.before, &effect.after] {
                if let FileImage::Regular { payload, .. } = image {
                    payload.verify(
                        payloads
                            .get(&payload.digest)
                            .ok_or(EffectError::MissingPayload)?,
                    )?;
                }
            }
        }
        let mut pinned = Vec::with_capacity(effects.as_slice().len());
        let mut entries = BTreeSet::new();
        for effect in effects.as_slice() {
            let mut parent = roots
                .get(&effect.target.root)
                .ok_or(EffectError::MissingRoot)?
                .duplicate()?;
            let mut parts = effect.target.relative.as_str().split('/').peekable();
            let name = loop {
                let part = parts.next().expect("validated nonempty relative path");
                if parts.peek().is_none() {
                    break part;
                }
                parent = parent.open_child(part)?;
            };
            if !entries.insert((parent.identity(), name)) {
                return Err(EffectError::AliasedTarget);
            }
            pinned.push(PinnedEffect {
                parent,
                name,
                target: &effect.target,
                before: &effect.before,
                after: &effect.after,
            });
        }
        let result = Self {
            effects: pinned,
            payloads,
        };
        result.check_all()?;
        Ok(result)
    }

    fn check_all(&self) -> Result<(), EffectError> {
        for effect in &self.effects {
            let current = effect.read()?;
            if !matches_image(&current, effect.before, self.payloads)
                && !matches_image(&current, effect.after, self.payloads)
            {
                return Err(EffectError::Conflict(effect.target.clone()));
            }
        }
        Ok(())
    }

    /// Idempotent with exact before/after predicates. A failure retains every
    /// already-applied effect for journal recovery; there is no automatic undo
    /// that might reverse an uncertain or committed publication.
    pub fn apply(&self, direction: Direction) -> Result<(), EffectError> {
        // Preflight the complete set again before the first write, so an
        // observed peer conflict in the last target cannot mutate the first.
        self.check_all()?;
        let indices: Box<dyn Iterator<Item = usize>> = match direction {
            Direction::Restore => Box::new((0..self.effects.len()).rev()),
            Direction::Complete => Box::new(0..self.effects.len()),
        };
        for index in indices {
            let effect = &self.effects[index];
            let (wanted, other) = match direction {
                Direction::Restore => (effect.before, effect.after),
                Direction::Complete => (effect.after, effect.before),
            };
            let current = effect.read()?;
            if matches_image(&current, wanted, self.payloads) {
                effect.parent.sync_entry(effect.name)?;
                continue;
            }
            if !matches_image(&current, other, self.payloads) {
                return Err(EffectError::Conflict(effect.target.clone()));
            }
            match wanted {
                FileImage::Absent => effect.parent.remove_file(effect.name)?,
                FileImage::Regular { payload, mode } => effect.parent.replace_file(
                    effect.name,
                    &self.payloads[&payload.digest],
                    *mode,
                )?,
            }
            if !matches_image(&effect.read()?, wanted, self.payloads) {
                return Err(EffectError::Conflict(effect.target.clone()));
            }
        }
        Ok(())
    }
}

impl PinnedEffect<'_> {
    fn read(&self) -> Result<Option<(Vec<u8>, u32)>, EffectError> {
        let limit = [self.before, self.after]
            .into_iter()
            .map(|image| match image {
                FileImage::Absent => 0,
                FileImage::Regular { payload, .. } => payload.bytes,
            })
            .max()
            .unwrap_or(0);
        let limit = usize::try_from(limit).map_err(|_| RecordError::Budget)?;
        Ok(self.parent.read_regular(self.name, limit)?)
    }
}

fn matches_image(current: &Option<(Vec<u8>, u32)>, image: &FileImage, payloads: &Payloads) -> bool {
    match (current, image) {
        (None, FileImage::Absent) => true,
        (Some((bytes, actual_mode)), FileImage::Regular { payload, mode }) => {
            actual_mode == mode && bytes == &payloads[&payload.digest]
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::records::{FileEffect, PayloadRef, RelativePath};
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

    fn image(payloads: &mut Payloads, bytes: &[u8], mode: u32) -> FileImage {
        let payload = PayloadRef::for_bytes(bytes).unwrap();
        payloads.insert(payload.digest.clone(), bytes.to_vec());
        FileImage::Regular { payload, mode }
    }

    fn effect(root: TargetRoot, name: &str, before: FileImage, after: FileImage) -> FileEffect {
        FileEffect {
            target: FileTarget {
                root,
                relative: RelativePath::try_from(name.to_owned()).unwrap(),
            },
            before,
            after,
        }
    }

    fn write(path: &std::path::Path, bytes: &[u8], mode: u32) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    // Real filesystem component witness; no production publication is claimed.
    // trace:BUG-1808 | ai:codex
    #[test]
    fn finite_restore_and_complete_preserve_peer_bytes_modes_and_sidecars() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(repo.path().join("objects")).unwrap();
        write(&repo.path().join("objects/spec"), b"peer history", 0o640);
        write(&home.path().join("queue"), b"peer row", 0o600);
        write(&home.path().join("queue.lock"), b"", 0o600);
        let sidecar_inode = fs::metadata(home.path().join("queue.lock")).unwrap().ino();
        let mut payloads = Payloads::new();
        let effects = PreparedFileEffects::try_from(vec![
            effect(
                TargetRoot::Repository,
                "objects/spec",
                image(&mut payloads, b"peer history", 0o640),
                image(&mut payloads, b"peer history + owned", 0o600),
            ),
            effect(
                TargetRoot::HomeQueue,
                "queue",
                image(&mut payloads, b"peer row", 0o600),
                image(&mut payloads, b"peer row + owned", 0o640),
            ),
            effect(
                TargetRoot::Repository,
                "new",
                FileImage::Absent,
                image(&mut payloads, b"owned", 0o700),
            ),
        ])
        .unwrap();
        let roots = BTreeMap::from([
            (
                TargetRoot::Repository,
                DurableDirectory::open_existing(repo.path()).unwrap(),
            ),
            (
                TargetRoot::HomeQueue,
                DurableDirectory::open_existing(home.path()).unwrap(),
            ),
        ]);
        let set = VerifiedFileSet::verify(&effects, &payloads, &roots).unwrap();
        for _ in 0..2 {
            set.apply(Direction::Complete).unwrap();
        }
        assert_eq!(
            fs::read(repo.path().join("objects/spec")).unwrap(),
            b"peer history + owned"
        );
        assert_eq!(
            fs::metadata(repo.path().join("new")).unwrap().mode() & 0o777,
            0o700
        );
        for _ in 0..2 {
            set.apply(Direction::Restore).unwrap();
        }
        assert_eq!(
            fs::read(repo.path().join("objects/spec")).unwrap(),
            b"peer history"
        );
        assert_eq!(
            fs::metadata(repo.path().join("objects/spec"))
                .unwrap()
                .mode()
                & 0o777,
            0o640
        );
        assert_eq!(fs::read(home.path().join("queue")).unwrap(), b"peer row");
        assert!(!repo.path().join("new").exists());
        assert_eq!(
            fs::metadata(home.path().join("queue.lock")).unwrap().ino(),
            sidecar_inode
        );
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn missing_or_corrupt_payload_and_late_peer_conflict_write_nothing() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a"), b"old", 0o600);
        write(&root.path().join("b"), b"old", 0o600);
        let mut payloads = Payloads::new();
        let before = image(&mut payloads, b"old", 0o600);
        let after = image(&mut payloads, b"new", 0o600);
        let effects = PreparedFileEffects::try_from(vec![
            effect(TargetRoot::Repository, "a", before.clone(), after.clone()),
            effect(TargetRoot::Repository, "b", before, after),
        ])
        .unwrap();
        let roots = BTreeMap::from([(
            TargetRoot::Repository,
            DurableDirectory::open_existing(root.path()).unwrap(),
        )]);
        let mut incomplete = payloads.clone();
        incomplete.remove(&ContentDigest::of(b"old"));
        assert!(matches!(
            VerifiedFileSet::verify(&effects, &incomplete, &roots),
            Err(EffectError::MissingPayload)
        ));
        incomplete.insert(ContentDigest::of(b"old"), b"bad".to_vec());
        assert!(matches!(
            VerifiedFileSet::verify(&effects, &incomplete, &roots),
            Err(EffectError::Record(RecordError::Payload))
        ));
        let set = VerifiedFileSet::verify(&effects, &payloads, &roots).unwrap();
        write(&root.path().join("b"), b"peer", 0o600);
        assert!(set.apply(Direction::Complete).is_err());
        assert!(set.apply(Direction::Restore).is_err());
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"old");
        assert_eq!(fs::read(root.path().join("b")).unwrap(), b"peer");
        write(&root.path().join("b"), b"old", 0o600);
        set.apply(Direction::Complete).unwrap();
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"new");
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn aliases_symlink_parents_and_hardlinks_are_not_regular_effects() {
        let root = tempfile::tempdir().unwrap();
        let peer = tempfile::tempdir().unwrap();
        write(&peer.path().join("file"), b"peer", 0o600);
        symlink(peer.path(), root.path().join("link")).unwrap();
        let mut payloads = Payloads::new();
        let new = image(&mut payloads, b"new", 0o600);
        let roots = BTreeMap::from([
            (
                TargetRoot::Repository,
                DurableDirectory::open_existing(root.path()).unwrap(),
            ),
            (
                TargetRoot::Store,
                DurableDirectory::open_existing(root.path()).unwrap(),
            ),
        ]);
        let linked = PreparedFileEffects::try_from(vec![effect(
            TargetRoot::Repository,
            "link/file",
            FileImage::Absent,
            new.clone(),
        )])
        .unwrap();
        assert!(VerifiedFileSet::verify(&linked, &payloads, &roots).is_err());
        let aliases = PreparedFileEffects::try_from(vec![
            effect(
                TargetRoot::Repository,
                "file",
                FileImage::Absent,
                new.clone(),
            ),
            effect(TargetRoot::Store, "file", FileImage::Absent, new.clone()),
        ])
        .unwrap();
        assert!(matches!(
            VerifiedFileSet::verify(&aliases, &payloads, &roots),
            Err(EffectError::AliasedTarget)
        ));
        fs::hard_link(peer.path().join("file"), root.path().join("file")).unwrap();
        let linked_file = PreparedFileEffects::try_from(vec![effect(
            TargetRoot::Repository,
            "file",
            FileImage::Absent,
            new,
        )])
        .unwrap();
        assert!(VerifiedFileSet::verify(&linked_file, &payloads, &roots).is_err());
        assert_eq!(fs::read(peer.path().join("file")).unwrap(), b"peer");
    }
}
