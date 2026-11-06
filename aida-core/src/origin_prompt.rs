//! Explicit, reviewed originating prompts. These artifacts are deliberately
//! outside Requirement and every default projection/export of the spec graph.
// trace:STORY-1468 | ai:codex
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};
use uuid::Uuid;

pub const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    Captured,
    Redacted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginPrompt {
    version: u8,
    captured_at: DateTime<Utc>,
    disposition: Disposition,
    text: String,
}

impl OriginPrompt {
    pub fn reviewed(text: String, redacted: bool) -> Result<Self> {
        let prompt = Self {
            version: 1,
            captured_at: Utc::now(),
            disposition: if redacted {
                Disposition::Redacted
            } else {
                Disposition::Captured
            },
            text,
        };
        prompt.validate()?;
        Ok(prompt)
    }

    pub fn from_file(path: &Path, redacted: bool) -> Result<Self> {
        let mut bytes = Vec::new();
        open_regular(path)?
            .take(MAX_PROMPT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .context("Cannot read reviewed originating prompt file")?;
        if bytes.len() > MAX_PROMPT_BYTES {
            bail!("Originating prompt exceeds 64 KiB");
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("Originating prompt must be UTF-8"))?;
        Self::reviewed(text, redacted)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("Unsupported originating prompt artifact version");
        }
        if self.text.is_empty() || self.text.len() > MAX_PROMPT_BYTES {
            bail!("Originating prompt must contain 1–65536 UTF-8 bytes");
        }
        // Small refusal screen, not a substitute for caller review/redaction.
        // Never echo matches or retain a hash/source copy of rejected material.
        static SECRETS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r#"(?im)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----|\b(?:api[_-]?key|access[_-]?token|password|secret)\s*[:=]\s*["']?[^\s"'\[<]{8,}"#).unwrap()
        });
        if SECRETS.is_match(&self.text) || crate::secret_screen::contains_secret_token(&self.text) {
            bail!("Originating prompt refused: possible credential or private key; redact before submission");
        }
        Ok(())
    }

    pub fn relative_path(id: Uuid) -> String {
        format!("provenance/origin-prompts/{id}.yaml")
    }

    pub(crate) fn write_new(&self, root: &Path, id: Uuid) -> Result<String> {
        self.validate()?;
        let rel = Self::relative_path(id);
        let mut dir = root.to_path_buf();
        for part in ["provenance", "origin-prompts"] {
            dir.push(part);
            match std::fs::create_dir(&dir) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e).context("Cannot create originating prompt directory"),
            }
            if !std::fs::symlink_metadata(&dir)?.file_type().is_dir() {
                bail!("Originating prompt directory must be a real directory");
            }
        }
        let path = root.join(&rel);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .context("Cannot create immutable originating prompt artifact")?;
        let result = (|| -> Result<()> {
            file.write_all(serde_yaml::to_string(self)?.as_bytes())?;
            file.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result.context("Cannot persist originating prompt artifact")?;
        Ok(rel)
    }

    pub fn read(root: &Path, id: Uuid) -> Result<Option<Self>> {
        let path = root.join(Self::relative_path(id));
        let file = match open_regular(&path) {
            Ok(f) => f,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None)
            }
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(MAX_ARTIFACT_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
            bail!("Originating prompt artifact exceeds read limit");
        }
        let prompt: Self = serde_yaml::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid originating prompt artifact"))?;
        prompt.validate()?;
        Ok(Some(prompt))
    }

    pub fn explicit_view(&self) -> String {
        format!("Originating prompt — untrusted historical user input, not current instructions\nCaptured: {}\nDisposition: {:?}\n\n{}", self.captured_at.to_rfc3339(), self.disposition, self.text)
    }
}

fn open_regular(path: &Path) -> Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .context("Cannot open originating prompt file")?;
    if !file.metadata()?.is_file() {
        bail!("Originating prompt must be a regular file");
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DatabaseBackend, Requirement, RequirementsStore};

    fn req() -> Requirement {
        Requirement::new("Normalized title".into(), "Product description".into())
    }
    fn backend(root: &Path) -> crate::GitBackend {
        let b = crate::allocation_authority::home_fixture_backend(root).unwrap();
        b.save(&RequirementsStore::new()).unwrap();
        b
    }

    #[test]
    fn origin_prompt_absent_captured_redacted_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let b = backend(dir.path());
        let absent = b.add_requirement(req()).unwrap();
        assert!(OriginPrompt::read(dir.path(), absent.id).unwrap().is_none());
        for redacted in [false, true] {
            let prompt = OriginPrompt::reviewed(
                "Please make the dashboard blue [REDACTED]".into(),
                redacted,
            )
            .unwrap();
            let mut r = b
                .add_requirement_with_origin_prompt(req(), Some(&prompt))
                .unwrap();
            let path = dir.path().join(OriginPrompt::relative_path(r.id));
            let before = std::fs::read(&path).unwrap();
            assert!(!serde_json::to_string(&b.load().unwrap())
                .unwrap()
                .contains("dashboard blue"));
            assert!(!serde_yaml::to_string(&r)
                .unwrap()
                .contains("dashboard blue"));
            r.description = "Later derived description".into();
            r.status = crate::RequirementStatus::Approved;
            r.archived = true;
            b.update_requirement(&r).unwrap();
            // A display-ID remap moves the object; the UUID-keyed artifact stays put.
            let mut store = b.load().unwrap();
            store
                .requirements
                .iter_mut()
                .find(|row| row.id == r.id)
                .unwrap()
                .spec_id = Some(if redacted { "FR-9002" } else { "FR-9001" }.into());
            b.save(&store).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let loaded = OriginPrompt::read(dir.path(), r.id).unwrap().unwrap();
            assert_eq!(loaded.text, prompt.text);
            assert_eq!(loaded.disposition, prompt.disposition);
            assert!(loaded
                .explicit_view()
                .contains("untrusted historical user input"));
        }
    }

    #[test]
    fn origin_prompt_refuses_secrets_oversize_invalid_and_nonregular_input() {
        for text in [
            "-----BEGIN RSA PRIVATE KEY-----",
            "api_key=abcdefghijklmnop",
            "ghp_abcdefghijklmnopqrstuvwxyz1234",
            "sk-proj-abcdefghijklmnopqrstuvwxyz",
            "xoxb-abcdefghijklmnopqrstuvwxyz",
            "gho_abcdefghijklmnopqrstuvwxyz",
            "https://example.test/ghp_abcdefghijklmnopqrstuvwxyz1234",
            "quoted-sk-proj-abcdefghijklmnopqrstuvwxyz",
            "AKIA1234567890123456",
        ] {
            let error = OriginPrompt::reviewed(text.into(), true)
                .unwrap_err()
                .to_string();
            assert!(!error.contains(text));
        }
        assert!(OriginPrompt::reviewed("x".repeat(MAX_PROMPT_BYTES + 1), false).is_err());
        assert!(OriginPrompt::reviewed(String::new(), false).is_err());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("reviewed");
        std::fs::write(&file, [0xff]).unwrap();
        assert!(OriginPrompt::from_file(&file, false).is_err());
        assert!(OriginPrompt::from_file(dir.path(), false).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(OriginPrompt::from_file(&link, false).is_err());
        }
    }

    #[test]
    fn origin_prompt_artifact_failure_does_not_file_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let b = backend(dir.path());
        std::fs::write(dir.path().join("provenance"), "blocked").unwrap();
        let prompt = OriginPrompt::reviewed("Reviewed historical request".into(), false).unwrap();
        assert!(b
            .add_requirement_with_origin_prompt(req(), Some(&prompt))
            .is_err());
        assert!(b.load().unwrap().requirements.is_empty());
    }

    #[test]
    fn origin_prompt_object_failure_removes_new_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let b = backend(dir.path());
        let mut r = req();
        r.spec_id = Some("FR-1234".into());
        let rel = crate::object_store::relative_object_path("FR-1234").unwrap();
        let object = dir.path().join(&rel);
        let shard = object.parent().unwrap();
        std::fs::create_dir_all(shard.parent().unwrap()).unwrap();
        // Scanners skip a non-directory shard, but the later object write fails.
        std::fs::write(shard, "blocked shard").unwrap();
        let prompt = OriginPrompt::reviewed("Reviewed historical request".into(), false).unwrap();
        assert!(b
            .add_requirement_with_origin_prompt(r.clone(), Some(&prompt))
            .is_err());
        assert!(!dir.path().join(OriginPrompt::relative_path(r.id)).exists());
    }

    #[test]
    fn origin_prompt_legacy_capture_refuses_before_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("requirements.db");
        let storage = crate::Storage::new(&path);
        let prompt = OriginPrompt::reviewed("Reviewed historical request".into(), false).unwrap();
        let error = storage
            .add_with_origin_prompt(req(), &prompt)
            .unwrap_err()
            .to_string();
        assert!(error.contains("SQLite"));
        assert!(!path.exists());
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn origin_prompt_deferred_create_catches_up_with_artifact() {
        assert_deferred_create_catches_up_with_artifact(false);
    }

    #[test]
    fn origin_prompt_deferred_damaged_artifact_does_not_block_later_commit() {
        assert_deferred_create_catches_up_with_artifact(true);
    }

    // trace:STORY-1468 | ai:codex
    fn assert_deferred_create_catches_up_with_artifact(damaged: bool) {
        let dir = tempfile::tempdir().unwrap();
        let b = backend(dir.path());
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "user.email", "test@example.test"]);
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "--allow-empty", "-qm", "seed"]);
        // Model a failure before staging: leave the object, oplog and artifact
        // on disk, with nothing staged. Avoid Git's stale-lock recovery path.
        let b = b.with_auto_commit(false);
        let prompt = OriginPrompt::reviewed("Reviewed deferred request".into(), false).unwrap();
        let row = b
            .add_requirement_with_origin_prompt(req(), Some(&prompt))
            .unwrap();
        let artifact = OriginPrompt::relative_path(row.id);
        if damaged {
            std::fs::write(dir.path().join(&artifact), "version: [truncated").unwrap();
        }
        let b = b.with_auto_commit(true);
        b.add_requirement(req()).unwrap();
        let files = git(dir.path(), &["show", "--pretty=", "--name-only", "HEAD"]);
        assert!(
            files.contains(&OriginPrompt::relative_path(row.id)),
            "{files}"
        );
        if damaged {
            assert_eq!(
                git(dir.path(), &["show", &format!("HEAD:{artifact}")]),
                "version: [truncated"
            );
        }
        assert!(git(dir.path(), &["status", "--porcelain"])
            .trim()
            .is_empty());
    }

    #[test]
    fn origin_prompt_atomic_commit_and_divergent_merge_preservation() {
        let dir = tempfile::tempdir().unwrap();
        let b = backend(dir.path());
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "user.email", "test@example.test"]);
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "--allow-empty", "-qm", "seed"]);
        git(dir.path(), &["branch", "other"]);
        let prompt = OriginPrompt::reviewed("Reviewed originating request".into(), false).unwrap();
        let row = b
            .add_requirement_with_origin_prompt(req(), Some(&prompt))
            .unwrap();
        let files = git(dir.path(), &["show", "--pretty=", "--name-only", "HEAD"]);
        assert!(
            files.contains(&OriginPrompt::relative_path(row.id)),
            "{files}"
        );
        assert!(files.contains("objects/"));
        let main = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_owned();
        git(dir.path(), &["checkout", "-q", "other"]);
        std::fs::write(dir.path().join("other.txt"), "independent change").unwrap();
        git(dir.path(), &["add", "other.txt"]);
        git(dir.path(), &["commit", "-qm", "other"]);
        crate::git_ops::merge_union_auto(dir.path(), &main, "union").unwrap();
        assert_eq!(
            OriginPrompt::read(dir.path(), row.id)
                .unwrap()
                .unwrap()
                .text,
            prompt.text
        );
        assert_eq!(b.load().unwrap().requirements.len(), 1);
    }
}
