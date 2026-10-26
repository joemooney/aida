//! Capture the actual effective Git identities before prepared WORK effects.
//! No synthetic author, signature suppression or hook bypass is supplied.
// trace:STORY-1742 | ai:codex
use super::*;
use crate::transport_sanitization::Policy;
use anyhow::bail;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::time::Instant;

#[derive(PartialEq, Eq)]
struct Identity {
    name: String,
    email: String,
    date: String,
}

fn run(
    root: &Path,
    args: &[&str],
    redact: &[String],
) -> Result<(std::process::ExitStatus, Vec<u8>)> {
    let mut cmd = host_git_command(root);
    cmd.args(redact).args(args);
    let (status, bytes) =
        crate::transport_policy::bounded(cmd, 1_048_576, Instant::now() + Duration::from_secs(10))
            .map_err(|_| {
                anyhow::anyhow!("WORK Git preparation unavailable or exceeded its bound")
            })?;
    Ok((status, bytes))
}

fn required(root: &Path, args: &[&str], redact: &[String]) -> Result<Vec<u8>> {
    let (status, bytes) = run(root, args, redact)?;
    anyhow::ensure!(status.success(), "WORK Git preparation refused");
    Ok(bytes)
}

fn identity(root: &Path, kind: &str, redact: &[String], policy: &Policy) -> Result<Identity> {
    let bytes = required(root, &["var", kind], redact)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("Unsupported Git identity encoding"))?;
    let (person, date) = text
        .trim_end_matches('\n')
        .rsplit_once("> ")
        .context("Unsupported Git identity representation")?;
    let (name, email) = person
        .rsplit_once(" <")
        .context("Unsupported Git identity representation")?;
    anyhow::ensure!(
        name == policy.git_name && email == policy.git_identity,
        "Effective Git identity differs from reviewed WORK policy"
    );
    policy
        .text(name)
        .and_then(|_| policy.text(email))
        .map_err(|_| anyhow::anyhow!("Effective Git identity refused by WORK policy"))?;
    let mut fields = date.split(' ');
    let seconds = fields.next().unwrap_or("");
    let offset = fields.next().unwrap_or("");
    anyhow::ensure!(
        seconds.parse::<i64>().is_ok()
            && offset.len() == 5
            && matches!(offset.as_bytes()[0], b'+' | b'-')
            && offset[1..].bytes().all(|b| b.is_ascii_digit())
            && fields.next().is_none(),
        "Unsupported Git identity timestamp"
    );
    Ok(Identity {
        name: name.into(),
        email: email.into(),
        date: date.into(),
    })
}

fn routing_environment() -> Result<()> {
    // The actual storage root owns this operation. Identity/config overrides
    // remain observable through Git; alternate repository/object/index roots
    // are unsupported rather than allowed to redirect the prepared effect.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
        "GIT_REPLACE_REF_BASE",
    ] {
        anyhow::ensure!(
            std::env::var_os(key).is_none(),
            "Unsupported Git routing override for WORK intake"
        );
    }
    Ok(())
}

pub(crate) struct PreparedCommit {
    head: Vec<u8>,
    reference: Vec<u8>,
    git_dir: Vec<u8>,
    config_hash: Vec<u8>,
    public_email: Option<String>,
    author: Identity,
    committer: Identity,
    date_environment: [Option<OsString>; 2],
    message: String,
}

impl PreparedCommit {
    pub(crate) fn prepare(root: &Path, message: &str, policy: &Policy) -> Result<Self> {
        routing_environment()?;
        anyhow::ensure!(
            is_store_tree(root),
            "WORK prepared commit requires the actual canonical store"
        );
        ensure_store_write_safe(root)?;
        let (_, public_email) = read_public_identity();
        let redact = commit_redaction_args(public_email.as_deref());
        let version = required(root, &["--version"], &redact)?;
        let version = std::str::from_utf8(&version)
            .unwrap_or("")
            .trim()
            .strip_prefix("git version ")
            .unwrap_or("");
        let mut parts = version.split('.');
        let major = parts
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let minor = parts
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        anyhow::ensure!(
            major > 2 || (major == 2 && minor >= 40),
            "WORK prepared commit requires Git attribute-source isolation support"
        );
        let reference = required(root, &["symbolic-ref", "--quiet", "HEAD"], &redact)?;
        anyhow::ensure!(
            reference == b"refs/heads/aida-store\n",
            "WORK intake requires its attached canonical store branch"
        );
        let git_dir = required(
            root,
            &["rev-parse", "--path-format=absolute", "--git-dir"],
            &redact,
        )?;
        let head = required(root, &["rev-parse", "--verify", "HEAD"], &redact)?;
        let head_text = std::str::from_utf8(&head).unwrap_or("").trim();
        anyhow::ensure!(
            matches!(head_text.len(), 40 | 64) && head_text.bytes().all(|b| b.is_ascii_hexdigit()),
            "Unsupported WORK store HEAD"
        );
        for key in ["commit.gpgsign", "i18n.commitEncoding"] {
            let (status, bytes) = run(root, &["config", "--get", key], &redact)?;
            let present = status.success();
            anyhow::ensure!(
                present || status.code() == Some(1),
                "Cannot establish effective WORK commit configuration"
            );
            let value = std::str::from_utf8(&bytes)
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if present
                && !(if key == "commit.gpgsign" {
                    matches!(value.as_str(), "false" | "no" | "off" | "0")
                } else {
                    matches!(value.as_str(), "utf-8" | "utf8")
                })
            {
                bail!("Unsupported required commit signature or encoding for WORK intake");
            }
            if !present && !bytes.is_empty() {
                bail!("Cannot establish effective WORK commit configuration");
            }
        }
        let config = required(
            root,
            &["config", "--null", "--list", "--show-origin"],
            &redact,
        )?;
        let message = format!("{}\n\nAida-Store: {}\n", message.trim_end(), head_text);
        policy
            .text(&message)
            .map_err(|_| anyhow::anyhow!("WORK commit message refused"))?;
        let author = identity(root, "GIT_AUTHOR_IDENT", &redact, policy)?;
        let committer = identity(root, "GIT_COMMITTER_IDENT", &redact, policy)?;
        Ok(Self {
            head,
            reference,
            git_dir,
            config_hash: Sha256::digest(config).to_vec(),
            public_email,
            author,
            committer,
            date_environment: [
                std::env::var_os("GIT_AUTHOR_DATE"),
                std::env::var_os("GIT_COMMITTER_DATE"),
            ],
            message,
        })
    }

    pub(crate) fn revalidate(&self, root: &Path, policy: &Policy) -> Result<()> {
        routing_environment()?;
        anyhow::ensure!(
            read_public_identity().1 == self.public_email
                && self.date_environment
                    == [
                        std::env::var_os("GIT_AUTHOR_DATE"),
                        std::env::var_os("GIT_COMMITTER_DATE")
                    ],
            "Effective WORK commit context changed"
        );
        let redact = commit_redaction_args(self.public_email.as_deref());
        anyhow::ensure!(
            required(root, &["symbolic-ref", "--quiet", "HEAD"], &redact)? == self.reference
                && required(
                    root,
                    &["rev-parse", "--path-format=absolute", "--git-dir"],
                    &redact
                )? == self.git_dir,
            "WORK store attachment changed during preparation"
        );
        anyhow::ensure!(
            required(root, &["rev-parse", "--verify", "HEAD"], &redact)? == self.head,
            "WORK store HEAD changed during preparation"
        );
        let config = required(
            root,
            &["config", "--null", "--list", "--show-origin"],
            &redact,
        )?;
        anyhow::ensure!(
            Sha256::digest(config).as_slice() == self.config_hash,
            "WORK Git configuration changed during preparation"
        );
        for (kind, original) in [
            ("GIT_AUTHOR_IDENT", &self.author),
            ("GIT_COMMITTER_IDENT", &self.committer),
        ] {
            let current = identity(root, kind, &redact, policy)?;
            anyhow::ensure!(
                current.name == original.name && current.email == original.email,
                "Effective WORK commit identity changed"
            );
        }
        Ok(())
    }

    /// Called only after whole-plan admission. Errors honestly leave the
    /// already admitted counter/files as partial persistence, never rollback.
    pub(crate) fn commit(self, root: &Path, paths: &[&str]) -> Result<()> {
        let mut command = host_git_command(root);
        command.args(commit_redaction_args(self.public_email.as_deref()));
        command.args(["commit", "--cleanup=verbatim", "-m", &self.message, "--"]);
        command.args(paths);
        for (prefix, ident) in [("AUTHOR", &self.author), ("COMMITTER", &self.committer)] {
            command
                .env(format!("GIT_{prefix}_NAME"), &ident.name)
                .env(format!("GIT_{prefix}_EMAIL"), &ident.email)
                .env(format!("GIT_{prefix}_DATE"), &ident.date);
        }
        let (status, _) = crate::transport_policy::bounded(
            command,
            65536,
            Instant::now() + Duration::from_secs(30),
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "Admitted WORK files may be persisted; commit outcome could not be confirmed"
            )
        })?;
        anyhow::ensure!(
            status.success(),
            "Admitted WORK files persisted but commit failed; inspect local state before retry"
        );
        Ok(())
    }
}
