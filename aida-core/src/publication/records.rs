//! Validated finite payload/effect vocabulary for the common journal.
//!
//! These records cannot install files, resolve roots or mint a decision. The
//! owner must capture current locked preimages and pin actual root identities
//! before putting them in a journal generation. No assignment format or
//! transport identity is duplicated here.
// trace:BUG-1808 | ai:codex

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_EFFECTS: usize = 65_536;
const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_AGGREGATE_BYTES: u64 = 8 * MAX_PAYLOAD_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum RecordError {
    #[error("publication digest is not canonical SHA-256")]
    Digest,
    #[error("publication target is not a canonical relative path")]
    Path,
    #[error("publication file mode is unsupported")]
    Mode,
    #[error("publication payload exceeds its finite budget")]
    Budget,
    #[error("publication payload does not match its declared length and digest")]
    Payload,
    #[error("publication write set repeats or overlaps a target")]
    Overlap,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(super) struct ContentDigest(String);

impl TryFrom<String> for ContentDigest {
    type Error = RecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(RecordError::Digest);
        }
        Ok(Self(value))
    }
}

impl From<ContentDigest> for String {
    fn from(value: ContentDigest) -> Self {
        value.0
    }
}

impl ContentDigest {
    pub fn of(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(super) struct RelativePath(String);

impl TryFrom<String> for RelativePath {
    type Error = RecordError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        // No normalization: accepting a/../b would change the recorded target.
        // The initial filesystem profile is Linux: colon and backslash are
        // literal filename bytes, not portable separators to reinterpret.
        if value.is_empty()
            || value.len() > 4096
            || value.bytes().any(|b| b == 0)
            || value
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
        {
            return Err(RecordError::Path);
        }
        Ok(Self(value))
    }
}

impl From<RelativePath> for String {
    fn from(value: RelativePath) -> Self {
        value.0
    }
}

impl RelativePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Selects an already pinned allowed root; no absolute journal path is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TargetRoot {
    Repository,
    GitCommonDir,
    Store,
    HomeQueue,
    HomeGrants,
    HomeRegistry,
    WorktreeParent,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileTarget {
    pub root: TargetRoot,
    pub relative: RelativePath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PayloadRef {
    pub digest: ContentDigest,
    pub bytes: u64,
}

impl PayloadRef {
    pub fn for_bytes(bytes: &[u8]) -> Result<Self, RecordError> {
        let result = Self {
            digest: ContentDigest::of(bytes),
            bytes: bytes.len() as u64,
        };
        result.check_budget()?;
        Ok(result)
    }

    fn check_budget(&self) -> Result<(), RecordError> {
        if self.bytes > MAX_PAYLOAD_BYTES {
            return Err(RecordError::Budget);
        }
        Ok(())
    }

    pub fn verify(&self, bytes: &[u8]) -> Result<(), RecordError> {
        self.check_budget()?;
        if self.bytes != bytes.len() as u64 || self.digest != ContentDigest::of(bytes) {
            return Err(RecordError::Payload);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum FileImage {
    Absent,
    Regular { payload: PayloadRef, mode: u32 },
}

impl FileImage {
    fn byte_count(&self) -> Result<u64, RecordError> {
        match self {
            Self::Absent => Ok(0),
            Self::Regular { payload, mode } => {
                // Privilege bits and nonregular inode kinds are not silently
                // stripped. Symlinks/directories require separate effect types.
                // Every exact-image check (verify, apply, retry, restore, a
                // fresh process) reopens the file read-only without chmod, so
                // an image lacking owner-read could be installed but never
                // recognised again. That profile is unsupported, and this runs
                // for every before/after image before any target is touched.
                // trace:BUG-1808 | ai:claude
                if mode & !0o777 != 0 || mode & 0o400 == 0 {
                    return Err(RecordError::Mode);
                }
                payload.check_budget()?;
                Ok(payload.bytes)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileEffect {
    pub target: FileTarget,
    pub before: FileImage,
    pub after: FileImage,
}

/// Only construction through validation, including deserialization. Root
/// aliases must additionally be rejected by the actual root identity resolver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<FileEffect>", into = "Vec<FileEffect>")]
pub(super) struct PreparedFileEffects(Vec<FileEffect>);

impl TryFrom<Vec<FileEffect>> for PreparedFileEffects {
    type Error = RecordError;

    fn try_from(effects: Vec<FileEffect>) -> Result<Self, Self::Error> {
        if effects.len() > MAX_EFFECTS {
            return Err(RecordError::Budget);
        }
        let mut seen = BTreeSet::new();
        let mut aggregate = 0u64;
        for effect in &effects {
            if !seen.insert(effect.target.clone()) {
                return Err(RecordError::Overlap);
            }
            for image in [&effect.before, &effect.after] {
                aggregate = aggregate
                    .checked_add(image.byte_count()?)
                    .ok_or(RecordError::Budget)?;
                if aggregate > MAX_AGGREGATE_BYTES {
                    return Err(RecordError::Budget);
                }
            }
        }
        // All ancestors must be checked, not just adjacent sorted names (a-b
        // can sort between a and a/child). Never install a file over a parent.
        for target in &seen {
            for (index, _) in target.relative.0.match_indices('/') {
                let parent = FileTarget {
                    root: target.root,
                    relative: RelativePath(target.relative.0[..index].to_owned()),
                };
                if seen.contains(&parent) {
                    return Err(RecordError::Overlap);
                }
            }
        }
        Ok(Self(effects))
    }
}

impl From<PreparedFileEffects> for Vec<FileEffect> {
    fn from(value: PreparedFileEffects) -> Self {
        value.0
    }
}

impl PreparedFileEffects {
    pub fn as_slice(&self) -> &[FileEffect] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(path: &str) -> FileEffect {
        FileEffect {
            target: FileTarget {
                root: TargetRoot::Repository,
                relative: RelativePath::try_from(path.to_owned()).unwrap(),
            },
            before: FileImage::Regular {
                payload: PayloadRef::for_bytes(b"peer history").unwrap(),
                mode: 0o600,
            },
            after: FileImage::Regular {
                payload: PayloadRef::for_bytes(b"peer history plus owned effect").unwrap(),
                mode: 0o600,
            },
        }
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn exact_preimage_and_afterimage_roundtrip_without_normalization() {
        let effects =
            PreparedFileEffects::try_from(vec![effect(".aida/sessions/fixture.manifest.toml")])
                .unwrap();
        let encoded = serde_json::to_vec(&effects).unwrap();
        assert_eq!(
            serde_json::from_slice::<PreparedFileEffects>(&encoded).unwrap(),
            effects
        );
        let FileImage::Regular { payload, .. } = &effects.0[0].before else {
            panic!("fixture preimage");
        };
        assert_eq!(payload.verify(b"peer history"), Ok(()));
        assert_eq!(payload.verify(b"peer historY"), Err(RecordError::Payload));
        assert_eq!(
            payload.verify(b"peer history plus owned effect"),
            Err(RecordError::Payload)
        );
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn decoding_cannot_bypass_digest_path_or_effect_validation() {
        for path in [
            "",
            "/etc/escape",
            "../peer",
            "a/../peer",
            "a//b",
            "a/./b",
            "a/",
            "a\0b",
        ] {
            let json = serde_json::to_string(path).unwrap();
            assert!(
                serde_json::from_str::<RelativePath>(&json).is_err(),
                "{path:?}"
            );
        }
        assert!(RelativePath::try_from("literal:message".to_owned()).is_ok());
        assert!(RelativePath::try_from("literal\\name".to_owned()).is_ok());
        for digest in ["a".repeat(63), "A".repeat(64), "z".repeat(64)] {
            assert!(serde_json::from_str::<ContentDigest>(
                &serde_json::to_string(&digest).unwrap()
            )
            .is_err());
        }
        let repeated = vec![effect("a"), effect("a")];
        assert!(serde_json::from_slice::<PreparedFileEffects>(
            &serde_json::to_vec(&repeated).unwrap()
        )
        .is_err());
        let mut unknown = serde_json::to_value(effect("a")).unwrap();
        unknown["ignore_missing_payload"] = serde_json::json!(true);
        assert!(serde_json::from_value::<FileEffect>(unknown).is_err());
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn complete_file_set_rejects_overlap_privilege_and_budget_overflow() {
        assert_eq!(
            PreparedFileEffects::try_from(vec![effect("a"), effect("a-b"), effect("a/b")]),
            Err(RecordError::Overlap)
        );
        assert!(PreparedFileEffects::try_from(vec![effect("a/b"), effect("a/c")]).is_ok());
        let mut privileged = effect("a");
        if let FileImage::Regular { mode, .. } = &mut privileged.after {
            *mode = 0o4600;
        }
        assert_eq!(
            PreparedFileEffects::try_from(vec![privileged]),
            Err(RecordError::Mode)
        );
        let mut huge = effect("a");
        if let FileImage::Regular { payload, .. } = &mut huge.after {
            payload.bytes = MAX_PAYLOAD_BYTES + 1;
        }
        assert_eq!(
            PreparedFileEffects::try_from(vec![huge]),
            Err(RecordError::Budget)
        );
        let aggregate: Vec<_> = (0..5)
            .map(|n| {
                let mut item = effect(&format!("f{n}"));
                for image in [&mut item.before, &mut item.after] {
                    if let FileImage::Regular { payload, .. } = image {
                        payload.bytes = MAX_PAYLOAD_BYTES;
                    }
                }
                item
            })
            .collect();
        assert_eq!(
            PreparedFileEffects::try_from(aggregate),
            Err(RecordError::Budget)
        );
    }
}
