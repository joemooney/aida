//! One shared reader for clone-local transport enrollment and reviewed policy.
//! Admission is outer to store/counter locks and is never an allocation grant.
// trace:STORY-1742 | ai:codex
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

#[path = "transport_private.rs"]
mod private;
pub use private::Private;

#[path = "transport_process.rs"]
mod process;
pub use process::bounded;

#[path = "transport_prepared.rs"]
mod prepared;
pub(crate) use prepared::read as read_artifact;
pub(crate) use prepared::{FileEffect, PreparedMutation};

pub const PROFILE_FILE: &str = "aida-transport.json";
pub const POLICY_FILE: &str = "aida-transport-policy.json";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub version: u32,
    pub name: String,
    pub origin_sha256: String,
    pub baseline: Option<String>,
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.name != "work"
            || !hex(&self.origin_sha256, &[64])
            || self.baseline.as_ref().is_some_and(|id| !hex(id, &[40, 64]))
        {
            bail!("Invalid or unsupported clone-local transport profile; refused");
        }
        Ok(())
    }

    pub fn load(common: &Path) -> Result<Option<Self>> {
        match std::fs::symlink_metadata(common.join(PROFILE_FILE)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => bail!("Cannot determine clone-local transport policy; refused"),
            Ok(_) => {}
        }
        let bytes = Private::open(common)?
            .read(PROFILE_FILE)?
            .context("Transport profile changed during discovery; refused")?;
        let profile: Self = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid clone-local transport profile; refused"))?;
        profile.validate()?;
        Ok(Some(profile))
    }
}

fn hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub struct Snapshot {
    pub policy: crate::transport_sanitization::Policy,
    pub hash: String,
    bytes: Vec<u8>,
}

impl Snapshot {
    pub fn load(private: &Private) -> Result<Self> {
        let bytes = private
            .read(POLICY_FILE)?
            .context("Reviewed private transport policy is missing; refused")?;
        let policy: crate::transport_sanitization::Policy = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid transport policy"))?;
        policy
            .validate()
            .map_err(|_| anyhow::anyhow!("Invalid or unreviewed transport policy"))?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        Ok(Self {
            policy,
            hash,
            bytes,
        })
    }

    pub fn revalidate(&self, private: &Private) -> Result<()> {
        if private.read(POLICY_FILE)?.as_deref() != Some(self.bytes.as_slice()) {
            bail!("Transport policy changed; refused");
        }
        Ok(())
    }
}

struct AdmissionContext {
    common: PathBuf,
    private: Private,
    profile: Profile,
    snapshot: Snapshot,
    _lock: std::fs::File,
}

thread_local! {
    static ADMISSIONS: RefCell<HashMap<PathBuf, Weak<AdmissionContext>>> = RefCell::new(HashMap::new());
}

/// A synchronous, thread-bound outer policy admission. It supplies no write
/// permission by itself; a complete prepared mutation must still be checked.
/// Rc intentionally makes this guard !Send. Prepared candidates use a
/// synchronous callback and invalidate detached providers at scope exit.
pub(crate) struct Admission {
    context: Option<Rc<AdmissionContext>>,
    root: PathBuf,
}

impl Admission {
    pub(crate) fn enter(root: &Path) -> Result<Self> {
        let common = owning_common(root)?;
        let active = ADMISSIONS.with(|all| {
            all.borrow()
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        });
        if let Some(context) = active
            .iter()
            .find(|context| Some(&context.common) == common.as_ref())
        {
            context.revalidate()?;
            return Ok(Self {
                context: Some(context.clone()),
                root: root.to_owned(),
            });
        }
        anyhow::ensure!(
            active.is_empty(),
            "Nested WORK mutation changed its owning storage root; refused"
        );
        let Some(common) = common else {
            return Ok(Self {
                context: None,
                root: root.to_owned(),
            });
        };
        let Some(profile) = Profile::load(&common)? else {
            return Ok(Self {
                context: None,
                root: root.to_owned(),
            });
        };
        if crate::db::any_store_write_lock_held() || crate::node::dispense_lock_held() {
            bail!("WORK admission must begin before store and counter locks; refused before persistence");
        }
        let private = Private::open(&common)?;
        let lock = private.lock()?;
        if Profile::load(&common)?.as_ref() != Some(&profile) {
            bail!("Transport enrollment changed during admission; refused");
        }
        let snapshot = Snapshot::load(&private)?;
        let context = Rc::new(AdmissionContext {
            common: common.clone(),
            private,
            profile,
            snapshot,
            _lock: lock,
        });
        ADMISSIONS.with(|all| {
            all.borrow_mut().insert(common, Rc::downgrade(&context));
        });
        Ok(Self {
            context: Some(context),
            root: root.to_owned(),
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.context.is_some()
    }

    pub(crate) fn snapshot(&self) -> Option<&Snapshot> {
        self.context.as_ref().map(|c| &c.snapshot)
    }

    pub(crate) fn revalidate(&self) -> Result<()> {
        if let Some(context) = &self.context {
            if owning_common(&self.root)?.as_ref() != Some(&context.common) {
                bail!("Transport storage ownership changed; refused before persistence");
            }
            context.revalidate()?;
        }
        Ok(())
    }
}

impl AdmissionContext {
    fn revalidate(&self) -> Result<()> {
        self.private.revalidate()?;
        if Profile::load(&self.common)?.as_ref() != Some(&self.profile) {
            bail!("Transport enrollment changed; refused before persistence");
        }
        self.snapshot.revalidate(&self.private)
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.context.take();
        ADMISSIONS.with(|all| all.borrow_mut().retain(|_, v| v.strong_count() > 0));
    }
}

/// Resolve only the actual storage path's Git ancestry. Never server cwd.
/// A damaged discovered Git identity is an error, not ordinary-mode fallback.
pub(crate) fn owning_common(root: &Path) -> Result<Option<PathBuf>> {
    for ancestor in root.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                return crate::allocation_authority::common_dir(ancestor)
                    .map(Some)
                    .map_err(|_| {
                        anyhow::anyhow!("Cannot establish transport policy ownership; refused")
                    })
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.raw_os_error() == Some(libc::ENOTDIR) => {}
            Err(_) => bail!("Cannot establish transport policy ownership; refused"),
        }
        // Explicit bare/override enrollment is not a supported intake owner.
        if ancestor.join(PROFILE_FILE).symlink_metadata().is_ok() {
            bail!("Enabled WORK intake requires an unambiguous attached owning project");
        }
    }
    Ok(None)
}

/// An unsupported mutation must stop before initialization, migration,
/// allocator opening, in-memory server success or a canonical write.
pub fn refuse_unsupported(root: &Path) -> Result<()> {
    if let Some(common) = owning_common(root)? {
        if Profile::load(&common)?.is_some() {
            bail!("This enabled-WORK mutation adapter has no complete prepared admission; refused before persistence");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn enrolled() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        crate::git_ops::init(root.path()).unwrap();
        let common = crate::allocation_authority::common_dir(root.path()).unwrap();
        let private = Private::open(&common).unwrap();
        private
            .write(
                PROFILE_FILE,
                &serde_json::to_vec(&Profile {
                    version: 1,
                    name: "work".into(),
                    origin_sha256: "a".repeat(64),
                    baseline: None,
                })
                .unwrap(),
            )
            .unwrap();
        private
            .write(
                POLICY_FILE,
                &serde_json::to_vec(&crate::transport_sanitization::Policy {
                    version: 1,
                    reviewed: true,
                    deny_tokens: vec!["private-marker".into()],
                    git_name: "AIDA Generic".into(),
                    git_identity: "aida".into(),
                })
                .unwrap(),
            )
            .unwrap();
        root
    }

    #[test]
    fn admission_must_precede_store_and_counter_but_nested_reuse_is_safe() {
        let root = enrolled();
        crate::db::with_store_write_lock(root.path(), || {
            assert!(Admission::enter(root.path()).is_err());
            Ok(())
        })
        .unwrap();
        crate::BlockRegistry::with_dispense_lock(
            &root.path().join(".git/synthetic-counter.toml"),
            || {
                assert!(Admission::enter(root.path()).is_err());
                Ok(())
            },
        )
        .unwrap();
        let outer = Admission::enter(root.path()).unwrap();
        assert!(outer.enabled());
        crate::db::with_store_write_lock(root.path(), || {
            let inner = Admission::enter(root.path())?;
            assert!(inner.enabled());
            inner.revalidate()
        })
        .unwrap();
        outer.revalidate().unwrap();
    }

    #[test]
    fn policy_drift_refuses_and_adapter_ownership_uses_storage_ancestry() {
        let root = enrolled();
        let ordinary = tempfile::tempdir().unwrap();
        crate::git_ops::init(ordinary.path()).unwrap();
        assert!(!Admission::enter(ordinary.path()).unwrap().enabled());
        let storage = root.path().join("nested/storage.yaml");
        assert!(refuse_unsupported(&storage).is_err());
        assert!(!storage.exists());
        let admission = Admission::enter(root.path()).unwrap();
        let common = crate::allocation_authority::common_dir(root.path()).unwrap();
        Private::open(&common)
            .unwrap()
            .write(POLICY_FILE, b"{}")
            .unwrap();
        assert!(admission.revalidate().is_err());
        assert!(Admission::enter(root.path()).is_err());
    }

    #[test]
    fn active_admission_never_falls_back_when_enrollment_disappears_or_root_changes() {
        let root = enrolled();
        let other = tempfile::tempdir().unwrap();
        crate::git_ops::init(other.path()).unwrap();
        let admission = Admission::enter(root.path()).unwrap();
        assert!(Admission::enter(other.path()).is_err());
        let common = crate::allocation_authority::common_dir(root.path()).unwrap();
        Private::open(&common)
            .unwrap()
            .remove(PROFILE_FILE)
            .unwrap();
        assert!(Admission::enter(root.path()).is_err());
        assert!(admission.revalidate().is_err());
    }
}
