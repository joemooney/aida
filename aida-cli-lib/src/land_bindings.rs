//! The trusted record of required-check app bindings that `aida land mode`
//! restores from. A mode change can remove a required check (`Build` when
//! the mode goes review-gated); when it comes back it must be required from
//! the same GitHub App it was bound to, or any app could satisfy it.
//!
//! The workspace audit is not an authority source: any seat can edit it.
//! This record lives under the operator's AIDA home, keyed by the project
//! and the protected branch. It is written, before every protection PATCH,
//! from the forge's own read-back (so a crash after the PATCH still leaves
//! the bindings it removed). It is read only when it is a private regular
//! file (never a symlink, owned by this user, not group- or world-writable)
//! in private directories, and names this project and branch. Anything else
//! refuses: the caller never falls back to letting GitHub pick an app.
// trace:STORY-2897 | ai:claude

use crate::land_mode::{AppBinding, Check};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const VERSION: u64 = 1;
const MAX_BYTES: u64 = 64 * 1024;

/// The binding record for one project's protected branch.
// trace:STORY-2897 | ai:claude
pub(crate) struct BindingStore {
    dir: PathBuf,
    path: PathBuf,
    project: String,
    branch: String,
    user: String,
    grant_id: Option<String>,
}

impl BindingStore {
    /// The record for `branch` of the project at `root`, written as `user`
    /// under `grant_id`.
    pub(crate) fn for_target(
        root: &Path,
        branch: &str,
        user: &str,
        grant_id: Option<&str>,
    ) -> Result<Self> {
        let home = crate::kernel::aida_home_dir()
            .context("no home directory for the trusted check-binding record")?;
        let project = crate::codex_bg_receipt::project_key(root);
        let dir = home.join(".aida").join("land-bindings").join(&project);
        let file = branch
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        Ok(Self {
            path: dir.join(format!("{file}.json")),
            dir,
            project,
            branch: branch.to_string(),
            user: user.to_string(),
            grant_id: grant_id.map(str::to_string),
        })
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The recorded bindings; none when no record exists yet. A record that
    /// fails any provenance check is an error, never "no bindings".
    pub(crate) fn load(&self) -> Result<BTreeMap<String, AppBinding>> {
        for dir in [self.dir.parent().unwrap_or(&self.dir), &self.dir] {
            match std::fs::symlink_metadata(dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
                Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
                Ok(meta) => private(&meta, dir, true)?,
            }
        }
        let meta = match std::fs::symlink_metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.path.display())),
            Ok(meta) => meta,
        };
        private(&meta, &self.path, false)?;
        if meta.len() > MAX_BYTES {
            bail!("{} is larger than a binding record", self.path.display());
        }
        let mut file = open_nofollow(&self.path, false)?;
        let mut body = String::new();
        std::io::Read::read_to_string(&mut file, &mut body)
            .with_context(|| format!("reading {}", self.path.display()))?;
        self.parse(&body)
            .with_context(|| format!("{} is not a valid binding record", self.path.display()))
    }

    fn parse(&self, body: &str) -> Result<BTreeMap<String, AppBinding>> {
        let value: serde_json::Value = serde_json::from_str(body)?;
        if value.get("version").and_then(|v| v.as_u64()) != Some(VERSION) {
            bail!("unknown version");
        }
        if value.get("project").and_then(|v| v.as_str()) != Some(self.project.as_str()) {
            bail!("it names another project");
        }
        if value.get("branch").and_then(|v| v.as_str()) != Some(self.branch.as_str()) {
            bail!("it names another branch");
        }
        let bindings = value
            .get("bindings")
            .and_then(|v| v.as_object())
            .context("no bindings")?;
        bindings
            .iter()
            .map(|(context, entry)| {
                // `app_id` must be present: null (any app) is not absent.
                let app = match entry.get("app_id") {
                    Some(serde_json::Value::Null) => AppBinding::Any,
                    Some(id) => match id.as_i64() {
                        Some(id) if id > 0 => AppBinding::App(id),
                        _ => bail!("`{context}` has an invalid app_id"),
                    },
                    None => bail!("`{context}` has no app_id"),
                };
                Ok((context.clone(), app))
            })
            .collect()
    }

    /// Record `observed` (the forge's checks as read back) over the
    /// existing record, atomically and durably. Called before a PATCH.
    pub(crate) fn record(&self, observed: &[Check]) -> Result<()> {
        let mut bindings = self.load()?;
        for check in observed {
            bindings.insert(check.context.clone(), check.app);
        }
        let json: serde_json::Map<String, serde_json::Value> = bindings
            .iter()
            .map(|(context, app)| {
                let id = match app {
                    AppBinding::App(id) => serde_json::json!(id),
                    AppBinding::Any => serde_json::Value::Null,
                };
                (context.clone(), serde_json::json!({ "app_id": id }))
            })
            .collect();
        let body = serde_json::json!({
            "version": VERSION,
            "project": self.project,
            "branch": self.branch,
            "bindings": json,
            "observed_at": chrono::Utc::now().to_rfc3339(),
            "user": self.user,
            "grant_id": self.grant_id,
        })
        .to_string();
        private_dirs(&self.dir)?;
        let tmp = self.dir.join(format!(
            ".{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        {
            use std::io::Write;
            let mut file = open_nofollow(&tmp, true)?;
            file.write_all(body.as_bytes())
                .and_then(|()| file.sync_all())
                .with_context(|| format!("writing {}", tmp.display()))?;
        }
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("installing {}", self.path.display()))?;
        if let Ok(dir) = std::fs::File::open(&self.dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

/// A private entry: not a symlink, of the expected kind, owned by this user,
/// and not group- or world-writable.
fn private(meta: &std::fs::Metadata, path: &Path, dir: bool) -> Result<()> {
    let kind_ok = if dir {
        meta.file_type().is_dir()
    } else {
        meta.file_type().is_file()
    };
    if !kind_ok {
        bail!(
            "{} is not a private {} (a symlink or another kind of entry)",
            path.display(),
            if dir { "directory" } else { "file" }
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if meta.uid() != me || meta.mode() & 0o022 != 0 {
            bail!(
                "{} is not private (owned by another user, or group- or world-writable)",
                path.display()
            );
        }
    }
    Ok(())
}

fn private_dirs(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    for d in [dir.parent().unwrap_or(dir), dir] {
        private(&std::fs::symlink_metadata(d)?, d, true)?;
    }
    Ok(())
}

fn open_nofollow(path: &Path, create: bool) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    if create {
        options.write(true).create_new(true);
    } else {
        options.read(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}
