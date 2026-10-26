//! Unspent clone-local counter plans. The existing store -> counter -> legacy
//! lock order stays intact; no live dispenser is attached to candidate models.
// trace:STORY-1742 | ai:codex
// trace:BUG-2017 | ai:codex
use super::*;
use crate::dispenser::{Dispenser, DispenserState, IdMode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct Detached {
    state: Mutex<DispenserState>,
    owner: std::thread::ThreadId,
    active: AtomicBool,
    sealed: AtomicBool,
}
impl Detached {
    fn check(&self, allocate: bool) -> Result<()> {
        anyhow::ensure!(
            self.owner == std::thread::current().id() && self.active.load(Ordering::SeqCst),
            "candidate scope is no longer active on its owning thread"
        );
        anyhow::ensure!(
            !allocate || !self.sealed.load(Ordering::SeqCst),
            "candidate allocation is sealed"
        );
        Ok(())
    }
}
impl Dispenser for Detached {
    fn node_local_only(&self) -> bool {
        true
    }
    fn next(&self, prefix: &str) -> Result<u32> {
        self.check(true)?;
        anyhow::ensure!(
            !prefix.strip_prefix('$').unwrap_or(prefix).is_empty()
                && prefix
                    .strip_prefix('$')
                    .unwrap_or(prefix)
                    .chars()
                    .all(|c| c.is_ascii_alphabetic()),
            "invalid node-local allocation prefix"
        );
        anyhow::ensure!(prefix.len() <= 128, "invalid node-local allocation prefix");
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("candidate state unavailable"))?;
        let counter = state.sequences.entry(prefix.to_uppercase()).or_default();
        *counter = counter
            .checked_add(1)
            .context("node-local sequence exhausted")?;
        Ok(*counter)
    }
    fn peek(&self, prefix: &str) -> Result<u32> {
        self.check(false)?;
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("candidate state unavailable"))?
            .sequences
            .get(&prefix.to_uppercase())
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .context("node-local sequence exhausted")
    }
    fn state(&self) -> Result<DispenserState> {
        self.check(false)?;
        Ok(self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("candidate state unavailable"))?
            .clone())
    }
    fn format_id(&self, prefix: &str, seq: u32) -> Result<String> {
        self.check(false)?;
        anyhow::ensure!(
            !prefix.strip_prefix('$').unwrap_or(prefix).is_empty()
                && prefix.len() <= 128
                && prefix
                    .strip_prefix('$')
                    .unwrap_or(prefix)
                    .chars()
                    .all(|c| c.is_ascii_uppercase()),
            "invalid node-local allocation prefix"
        );
        anyhow::ensure!(seq > 0, "node-local sequence is exhausted or invalid");
        match self.state()?.mode {
            IdMode::Distributed { node_id } => Ok(format!("{prefix}-{node_id}-{seq:03}")),
            _ => bail!("invalid WORK candidate mode"),
        }
    }
}

pub(crate) struct Candidate<'scope> {
    root: &'scope Path,
    binding: WorkBinding,
    original: DispenserState,
    initial_counter: Option<DispenserState>,
    publish_binding: bool,
    detached: Arc<Detached>,
    _scope: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub(crate) struct Sealed<'scope> {
    candidate: &'scope Candidate<'scope>,
    counter_bytes: Option<Vec<u8>>,
    binding_bytes: Option<Vec<u8>>,
}

impl Candidate<'_> {
    pub(crate) fn node(&self) -> &str {
        &self.binding.node_id
    }
    pub(crate) fn dispenser(&self) -> crate::models::DispenserHandle {
        crate::models::DispenserHandle(self.detached.clone())
    }
    pub(crate) fn admit(
        &self,
        incoming: &[crate::Requirement],
        existing: &[crate::Requirement],
    ) -> Result<()> {
        self.detached.check(false)?;
        crate::allocation_authority::admit_candidate(self.node(), incoming, existing)
    }
    pub(crate) fn seal(&self) -> Result<Sealed<'_>> {
        self.detached.check(true)?;
        let state = self
            .detached
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("candidate state unavailable"))?;
        self.detached.sealed.store(true, Ordering::SeqCst);
        let changed =
            state.mode != self.original.mode || state.sequences != self.original.sequences;
        // Reading/updating existing rows must not bootstrap or migrate the
        // allocator. A cold/legacy snapshot is published only when this
        // candidate actually consumes a sequence.
        let counter_bytes = if changed {
            Some(counter_bytes(&state)?)
        } else {
            None
        };
        let binding_bytes = if changed && self.publish_binding {
            Some(toml::to_string_pretty(&self.binding)?.into_bytes())
        } else {
            None
        };
        Ok(Sealed {
            candidate: self,
            counter_bytes,
            binding_bytes,
        })
    }
}

/// Preserve the exact dispenser state using valid inline TOML. Dotted table
/// headers would look like hostnames to the raw scanner. Only syntax changes;
/// the existing typed reader still owns the provider representation.
fn counter_bytes(state: &DispenserState) -> Result<Vec<u8>> {
    let IdMode::Distributed { node_id } = &state.mode else {
        bail!("invalid WORK candidate mode");
    };
    let mut text = format!(
        "mode = {{ Distributed = {{ node_id = {} }} }}\n\n[sequences]\n",
        serde_json::to_string(node_id)?
    );
    let mut entries: Vec<_> = state.sequences.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (prefix, counter) in entries {
        // JSON strings and TOML basic strings share this printable ASCII
        // subset. Refuse other legacy key encodings rather than normalize.
        anyhow::ensure!(
            prefix.is_ascii() && !prefix.chars().any(char::is_control),
            "unsupported candidate counter key encoding"
        );
        text.push_str(&format!("{} = {counter}\n", serde_json::to_string(prefix)?));
    }
    let decoded: DispenserState = toml::from_str(&text)?;
    anyhow::ensure!(
        decoded.mode == state.mode && decoded.sequences == state.sequences,
        "candidate counter serialization changed its state"
    );
    Ok(text.into_bytes())
}
impl Drop for Candidate<'_> {
    fn drop(&mut self) {
        self.detached.active.store(false, Ordering::SeqCst);
    }
}

impl Sealed<'_> {
    pub(crate) fn admit_rows(
        &self,
        incoming: &[crate::Requirement],
        existing: &[crate::Requirement],
    ) -> Result<()> {
        self.candidate.admit(incoming, existing)
    }

    pub(crate) fn admit_metadata_ids(
        &self,
        incoming: &[(uuid::Uuid, Option<String>)],
        existing: &[(uuid::Uuid, Option<String>)],
    ) -> Result<()> {
        self.candidate.detached.check(false)?;
        crate::allocation_authority::admit_candidate_meta(self.candidate.node(), incoming, existing)
    }
    pub(crate) fn private_artifacts(&self) -> impl Iterator<Item = (&'static str, &[u8])> {
        [
            (COUNTER_FILE, self.counter_bytes.as_deref()),
            (BINDING_FILE, self.binding_bytes.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, bytes)| bytes.map(|bytes| (name, bytes)))
    }

    /// The complete prepared mutation is the only production caller. This
    /// consumes this exact sealed snapshot; no live allocator is reopened.
    pub(crate) fn apply(self) -> Result<()> {
        self.revalidate()?;
        if let Some(bytes) = self.counter_bytes {
            publish_bytes(&counter_path(self.candidate.root)?, &bytes)
                .map_err(|_| anyhow::anyhow!("Admitted WORK counter persistence could not be confirmed; inspect local state before retry"))?;
        }
        if let Some(bytes) = self.binding_bytes {
            publish_bytes(&crate::allocation_authority::common_dir(self.candidate.root)?.join(BINDING_FILE), &bytes)
                .map_err(|_| anyhow::anyhow!("Admitted WORK counter may be persisted; binding persistence could not be confirmed"))?;
        }
        Ok(())
    }

    /// Revalidation is read-only and must complete before ANY planned effect.
    pub(crate) fn revalidate(&self) -> Result<()> {
        let c = self.candidate;
        c.detached.check(false)?;
        anyhow::ensure!(
            !crate::allocation_authority::resolve(c.root)?.authoritative,
            "allocation authority changed during candidate preparation"
        );
        let binding = load(c.root)?;
        if c.publish_binding {
            anyhow::ensure!(binding.is_none(), "WORK binding changed during preparation");
        } else {
            anyhow::ensure!(
                binding
                    .as_ref()
                    .is_some_and(|b| b.node_id == c.binding.node_id),
                "WORK binding changed during preparation"
            );
        }
        let current: Option<DispenserState> = read(&counter_path(c.root)?)?;
        match (&current, &c.initial_counter) {
            (Some(current), Some(initial))
                if current.mode == initial.mode && current.sequences == initial.sequences => {}
            (None, None) => {}
            _ => bail!("WORK counter changed during preparation"),
        }
        Ok(())
    }
}

// Unlike generic object writes, allocator state must remain owner-only even
// under a permissive inherited umask. Existing admissible legacy modes are
// read by the unchanged allocator reader; the admitted replacement is 0600.
fn publish_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension(format!("prepared-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(
            path.parent()
                .context("invalid allocation state destination")?,
        )?
        .sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// The callback runs synchronously while the original locks remain held. The
/// candidate itself cannot escape the borrow or move to another thread. Any
/// cloned detached provider also becomes unusable as soon as this scope ends.
pub(crate) fn with_unspent<T>(
    root: &Path,
    f: impl FnOnce(&Candidate<'_>) -> Result<T>,
) -> Result<T> {
    anyhow::ensure!(
        !crate::node::dispense_lock_held(),
        "nested WORK allocation candidates are unsupported; refused before lock inversion"
    );
    anyhow::ensure!(
        !crate::allocation_authority::resolve(root)?.authoritative,
        "prepared WORK allocation is unsupported for an authoritative provider"
    );
    crate::db::with_store_write_lock(root, || {
        let counter = counter_path(root)?;
        crate::BlockRegistry::with_dispense_lock(&counter, || {
            let binding = load(root)?;
            if let Some(binding) = binding {
                let state: DispenserState =
                    read(&counter)?.context("WORK counter missing; explicit recovery required")?;
                anyhow::ensure!(
                    state.mode
                        == IdMode::Distributed {
                            node_id: binding.node_id.clone()
                        },
                    "WORK counter binding mismatch"
                );
                let candidate = Candidate {
                    root,
                    binding,
                    original: state.clone(),
                    initial_counter: Some(state.clone()),
                    publish_binding: false,
                    detached: Arc::new(Detached {
                        state: Mutex::new(state),
                        owner: std::thread::current().id(),
                        active: AtomicBool::new(true),
                        sealed: AtomicBool::new(false),
                    }),
                    _scope: std::marker::PhantomData,
                };
                return f(&candidate);
            }
            let legacy = root.join(".aida/dispenser.toml");
            crate::BlockRegistry::with_dispense_lock(&legacy, || {
                let initial_counter: Option<DispenserState> = read(&counter)?;
                let mut state: DispenserState = initial_counter.clone().unwrap_or_else(|| {
                    DispenserState::new(IdMode::Distributed {
                        node_id: uuid::Uuid::new_v4().simple().to_string(),
                    })
                });
                let binding = match &state.mode {
                    IdMode::Distributed { node_id } => WorkBinding {
                        version: 1,
                        node_id: node_id.clone(),
                    },
                    _ => bail!("invalid WORK counter mode"),
                };
                binding.validate()?;
                merge_legacy_state(root, &legacy, &mut state)?;
                let candidate = Candidate {
                    root,
                    binding,
                    original: state.clone(),
                    initial_counter,
                    publish_binding: true,
                    detached: Arc::new(Detached {
                        state: Mutex::new(state),
                        owner: std::thread::current().id(),
                        active: AtomicBool::new(true),
                        sealed: AtomicBool::new(false),
                    }),
                    _scope: std::marker::PhantomData,
                };
                f(&candidate)
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_candidate_preserves_high_water_without_spending_or_migrating() {
        let temp = tempfile::tempdir().unwrap();
        crate::git_ops::init(temp.path()).unwrap();
        std::fs::create_dir(temp.path().join(".aida")).unwrap();
        std::fs::write(
            temp.path().join(".aida/node.toml"),
            "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n",
        )
        .unwrap();
        let legacy = b"[mode.Distributed]\nnode_id='7'\n[sequences]\nBUG=91\n'$USER'=3\n";
        std::fs::write(temp.path().join(".aida/dispenser.toml"), legacy).unwrap();
        with_unspent(temp.path(), |candidate| {
            assert_eq!(candidate.dispenser().next("bug")?, 92);
            let sealed = candidate.seal()?;
            let bytes = sealed.counter_bytes.as_deref().unwrap();
            let state: DispenserState = toml::from_str(std::str::from_utf8(bytes)?)?;
            assert_eq!(state.sequences.get("BUG"), Some(&92));
            assert_eq!(state.sequences.get("$USER"), Some(&3));
            let policy = crate::transport_sanitization::Policy {
                version: 1,
                reviewed: true,
                deny_tokens: vec!["private-marker".into()],
                git_name: "AIDA Generic".into(),
                git_identity: "aida".into(),
            };
            policy.document("toml", bytes).unwrap();
            sealed.revalidate()
        })
        .unwrap();
        assert_eq!(
            std::fs::read(temp.path().join(".aida/dispenser.toml")).unwrap(),
            legacy
        );
        assert!(!counter_path(temp.path()).unwrap().exists());
        assert!(load(temp.path()).unwrap().is_none());
    }

    #[test]
    fn read_only_candidate_does_not_plan_bootstrap() {
        let temp = tempfile::tempdir().unwrap();
        crate::git_ops::init(temp.path()).unwrap();
        with_unspent(temp.path(), |candidate| {
            assert!(with_unspent(temp.path(), |_| Ok(())).is_err());
            let sealed = candidate.seal()?;
            assert!(sealed.counter_bytes.is_none());
            assert!(sealed.binding_bytes.is_none());
            sealed.revalidate()
        })
        .unwrap();
        assert!(!counter_path(temp.path()).unwrap().exists());
        assert!(load(temp.path()).unwrap().is_none());
    }

    #[test]
    fn aborted_candidate_never_publishes_binding_or_spends_counters() {
        let temp = tempfile::tempdir().unwrap();
        crate::git_ops::init(temp.path()).unwrap();
        let result: Result<()> = with_unspent(temp.path(), |candidate| {
            let provider = candidate.dispenser();
            assert!(provider
                .next_id("BUG")?
                .starts_with(&format!("BUG-{}-", candidate.node())));
            let plan = candidate.seal()?;
            assert!(plan.counter_bytes.is_some());
            assert!(plan.binding_bytes.is_some());
            plan.revalidate()?;
            bail!("synthetic refusal before admission")
        });
        assert!(result.is_err());
        assert!(!counter_path(temp.path()).unwrap().exists());
        assert!(load(temp.path()).unwrap().is_none());
    }

    #[test]
    fn detached_provider_is_thread_bound_sealed_and_invalid_after_scope() {
        let temp = tempfile::tempdir().unwrap();
        crate::git_ops::init(temp.path()).unwrap();
        let escaped = with_unspent(temp.path(), |candidate| {
            let provider = candidate.dispenser();
            let other = provider.clone();
            assert!(std::thread::spawn(move || other.next("BUG"))
                .join()
                .unwrap()
                .is_err());
            assert_eq!(provider.next("BUG")?, 1);
            let _sealed = candidate.seal()?;
            assert!(provider.next("BUG").is_err());
            Ok(provider)
        })
        .unwrap();
        assert!(escaped.next("BUG").is_err());
        assert!(escaped.state().is_err());
        assert!(!counter_path(temp.path()).unwrap().exists());
    }
}
