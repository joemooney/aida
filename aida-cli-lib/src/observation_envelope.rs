//! Store-independent observation data. Imported prose is never an instruction.
// trace:STORY-2479 trace:TASK-3077 | ai:codex
use anyhow::{bail, Result};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const TITLE_LIMIT: usize = 512;
const BODY_LIMIT: usize = 64 * 1024;
pub(super) const ENVELOPE_LIMIT: usize = 128 * 1024;
const LIMITATION: &str =
    "Validation does not certify sanitization. Review all text before sharing.";

// Closed per-version kinds: a future ci-result requires explicit version handling.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Observation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Category {
    Observation,
    Question,
    Request,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Envelope {
    version: u32,
    kind: Kind,
    category: Category,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) checksum: String,
}

// Struct field order is the wire hash contract; never hash a JSON map.
#[derive(Serialize)]
struct Payload<'a> {
    version: u32,
    kind: Kind,
    category: Category,
    title: &'a str,
    body: &'a str,
}

impl Envelope {
    fn digest(&self) -> Result<String> {
        let bytes = serde_json::to_vec(&Payload {
            version: self.version,
            kind: self.kind,
            category: self.category,
            title: &self.title,
            body: &self.body,
        })?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    fn validate(&self, patterns: &[String]) -> Result<()> {
        match self.version {
            // Exhaustive per-version matching makes adding a kind require an
            // explicit decision about which versions admit it.
            1 => match self.kind {
                Kind::Observation => {}
            },
            _ => bail!("Unsupported envelope version"),
        }
        if self.title.is_empty() || self.title.len() > TITLE_LIMIT || self.body.len() > BODY_LIMIT {
            bail!("Envelope text exceeds limits: nonempty title <= 512 bytes; body <= 64 KiB");
        }
        if self.checksum.len() != 64
            || !self
                .checksum
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.checksum != self.digest()?
        {
            bail!("Envelope checksum mismatch or invalid full digest");
        }
        if serde_json::to_vec(self)?.len() > ENVELOPE_LIMIT {
            bail!("Serialized envelope exceeds 128 KiB");
        }
        check_patterns(&self.title, &self.body, patterns)
    }

    pub(super) fn label(&self) -> String {
        format!("INTENT-{}", &self.checksum[..12])
    }
}

pub(super) fn check_patterns(title: &str, body: &str, patterns: &[String]) -> Result<()> {
    if patterns.len() > 256 {
        bail!("At most 256 deny patterns are supported");
    }
    for (index, pattern) in patterns.iter().enumerate() {
        let rule = index + 1;
        if pattern.is_empty() || pattern.len() > 256 {
            bail!("Deny pattern rule {rule} must contain 1 to 256 bytes");
        }
        let regex = regex::RegexBuilder::new(pattern)
            .size_limit(1024 * 1024)
            .build()
            .map_err(|_| anyhow::anyhow!("Invalid deny pattern rule {rule}"))?;
        if regex.is_match(title) || regex.is_match(body) {
            bail!("Sensitive content refused by deny pattern rule {rule}");
        }
    }
    Ok(())
}

// Open first, validate the handle, then read at most limit+1. NONBLOCK keeps
// malicious FIFOs/devices from blocking; only regular, non-symlink files pass.
pub(super) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            // Keep imported paths out of diagnostics. trace:TASK-3112 | ai:codex
            anyhow::anyhow!(match e.kind() {
                std::io::ErrorKind::NotFound => "Input file not found",
                std::io::ErrorKind::PermissionDenied => "Permission denied opening input file",
                _ if e.raw_os_error() == Some(libc::ELOOP) => "Input must not be a symlink",
                _ => "Cannot open input file",
            })
        })?;
    if !file.metadata()?.is_file() {
        bail!("Input must be a regular file");
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bail!("Input exceeds {limit} byte limit");
    }
    Ok(bytes)
}

pub(super) fn parse(bytes: &[u8], patterns: &[String]) -> Result<Envelope> {
    if bytes.len() > ENVELOPE_LIMIT {
        bail!("Envelope exceeds 128 KiB");
    }
    // Do not echo unknown keys, variants or imported text in diagnostics.
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Invalid envelope schema or JSON"))?;
    envelope.validate(patterns)?;
    Ok(envelope)
}

#[derive(Debug, Subcommand)]
pub(crate) enum LocalCommand {
    /// Generate a finding or pending verdict from explicit closed JSON input (empty checksum).
    ReviewReturn {
        /// Explicit JSON input, at most 128 KiB; checksum must be empty.
        #[clap(long)]
        file: PathBuf,
        /// Refuse matching decoded string fields (regex, repeatable).
        #[clap(long)]
        deny_pattern: Vec<String>,
    },
    /// Configure or use the independently enrolled intake branch.
    #[clap(flatten)]
    Intake(crate::intent_intake::IntakeCommand),
    /// Generate an observation envelope on stdout, without a spec or store.
    Note {
        #[clap(long, value_enum)]
        category: Category,
        /// Explicit observation title, 1 to 512 UTF-8 bytes.
        #[clap(long)]
        title: String,
        /// Explicit UTF-8 body file, at most 64 KiB.
        #[clap(long)]
        file: PathBuf,
        /// Refuse matching title/body text (regex, repeatable; no automatic policy discovery).
        #[clap(long)]
        deny_pattern: Vec<String>,
    },
    /// Validate an envelope without creating a requirement. Does not certify sanitization.
    Validate {
        /// JSON envelope file, at most 128 KiB.
        #[clap(long)]
        file: PathBuf,
        /// Refuse matching title/body text (regex, repeatable).
        #[clap(long)]
        deny_pattern: Vec<String>,
        /// Report validity, label, full digest and sanitization limitation as JSON.
        #[clap(long)]
        json: bool,
    },
    /// Validate and emit a self-contained JSON attachment on stdout for manual email.
    Export {
        /// JSON envelope file, at most 128 KiB.
        #[clap(long)]
        file: PathBuf,
        /// Refuse matching title/body text (regex, repeatable).
        #[clap(long)]
        deny_pattern: Vec<String>,
    },
}

fn emit(envelope: &Envelope) -> Result<()> {
    let bytes = serde_json::to_vec(envelope)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&bytes)?;
    // No newline: even an envelope exactly at the bound must round-trip.
    stdout.flush()?;
    eprintln!("{}: {LIMITATION}", envelope.label());
    Ok(())
}

pub(crate) fn run(command: &LocalCommand, json_format: bool) -> Result<()> {
    match command {
        LocalCommand::ReviewReturn { file, deny_pattern } => {
            let bytes = read_bounded(file, ENVELOPE_LIMIT)?;
            let bytes = crate::review_envelope::generate(&bytes, deny_pattern)?;
            std::io::stdout().lock().write_all(&bytes)?;
            Ok(())
        }
        LocalCommand::Intake(command) => crate::intent_intake::run(command, json_format),
        LocalCommand::Note {
            category,
            title,
            file,
            deny_pattern,
        } => {
            let body = String::from_utf8(read_bounded(file, BODY_LIMIT)?)
                .map_err(|_| anyhow::anyhow!("Body must be UTF-8"))?;
            let mut envelope = Envelope {
                version: 1,
                kind: Kind::Observation,
                category: *category,
                title: title.clone(),
                body,
                checksum: String::new(),
            };
            envelope.checksum = envelope.digest()?;
            envelope.validate(deny_pattern)?;
            emit(&envelope)
        }
        LocalCommand::Validate {
            file,
            deny_pattern,
            json,
        } => {
            let bytes = read_bounded(file, ENVELOPE_LIMIT)?;
            let (_, checksum) = crate::intent_intake::content_path(&bytes, deny_pattern)?;
            let label = format!("INTENT-{}", &checksum[..12]);
            if *json || json_format {
                println!(
                    "{}",
                    serde_json::json!({
                        "valid": true, "label": label, "checksum": checksum,
                        "sanitization_certified": false, "limitation": LIMITATION,
                    })
                );
            } else {
                println!("{} valid\nSHA-256: {}\n{LIMITATION}", label, checksum);
            }
            Ok(())
        }
        LocalCommand::Export { file, deny_pattern } => {
            let bytes = read_bounded(file, ENVELOPE_LIMIT)?;
            let (folder, checksum) = crate::intent_intake::content_path(&bytes, deny_pattern)?;
            if folder == "observations" {
                return emit(&parse(&bytes, deny_pattern)?);
            }
            let value = crate::review_envelope::parse(&bytes, deny_pattern)?;
            std::io::stdout()
                .lock()
                .write_all(&serde_json::to_vec(&value)?)?;
            eprintln!("INTENT-{}: {LIMITATION}", &checksum[..12]);
            Ok(())
        }
    }
}

// Clap's args_conflicts_with_subcommands also prevents interleaved global
// flags from reaching a subcommand. Refuse only legacy comprehension arguments.
pub(crate) fn run_local(
    matches: &clap::ArgMatches,
    command: &LocalCommand,
    json_format: bool,
) -> Result<()> {
    for name in ["id", "audience", "refresh", "json"] {
        if matches.value_source(name) == Some(clap::parser::ValueSource::CommandLine) {
            bail!(
                "Spec comprehension arguments cannot be combined with local observation commands"
            );
        }
    }
    run(command, json_format)
}

/// Intercept only the new subcommands, before identity, telemetry, project/store
/// discovery or role prompts. Parse with the real registry, including globals.
pub(crate) fn local_entry() -> Option<Result<()>> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    // Global options may appear between `intent` and its subcommand. This
    // conservative prefilter only decides whether to parse; the actual command
    // tree below decides what runs, never an argv substring match.
    if !argv.iter().any(|arg| arg == "intent")
        || !argv.iter().any(|arg| {
            matches!(
                arg.to_str(),
                Some(
                    "review-return"
                        | "ci"
                        | "note"
                        | "validate"
                        | "export"
                        | "enroll"
                        | "publish"
                        | "pull"
                        | "list"
                        | "process"
                )
            )
        })
    {
        return None;
    }
    // Raw argv may belong to an extension or an alias. Let normal dispatch
    // own rewriting and clap diagnostics when this speculative parse fails.
    let cli = crate::command_registry::try_parse_from(argv).ok()?;
    if cli.command.name() != "intent" {
        return None;
    }
    let crate::cli::Command::Registered(invocation) = &cli.command;
    let args = match <crate::plan_intent::IntentArgs as clap::FromArgMatches>::from_arg_matches(
        &invocation.matches,
    ) {
        Ok(args) => args,
        Err(_) => return Some(Err(anyhow::anyhow!("Invalid local observation arguments"))),
    };
    args.local.as_ref().map(|command| {
        run_local(
            &invocation.matches,
            command,
            cli.globals.format == Some(crate::cli::OutputFormat::Json)
                || (cli.globals.format.is_none()
                    && std::env::var("AIDA_OUTPUT_FORMAT").as_deref() == Ok("json")),
        )
    })
}

#[cfg(test)]
mod task3112_tests {
    use super::*;
    // trace:TASK-3112 | ai:codex
    #[test]
    fn task3112_open_errors_distinguish_missing_and_symlink_without_paths() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("private-name");
        let error = read_bounded(&missing, 10).unwrap_err().to_string();
        assert_eq!(error, "Input file not found");
        let link = dir.path().join("private-link");
        std::os::unix::fs::symlink(&missing, &link).unwrap();
        assert_eq!(
            read_bounded(&link, 10).unwrap_err().to_string(),
            "Input must not be a symlink"
        );
    }
}
