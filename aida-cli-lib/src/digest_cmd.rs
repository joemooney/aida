//! `aida digest` command handler (STORY-252 / STORY-541 / TASK-381).
//!
//! The advisor's narrative work digest — `--since <window>` selects the
//! reporting window, `--audience customer|team|self|operator` selects the
//! lens, and `--copy` / `--out` / `--reset` control delivery. This handler is
//! the thin I/O + delivery boundary; the report assembly, audience-tone
//! formatting, and since-window parsing live in the `digest` module.
//! Extracted verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::{Context, Result};
use colored::Colorize;

use aida_core::RequirementsStore;

use crate::{copy_to_clipboard, digest, find_project_root};

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_digest_command(
    since_raw: Option<&str>,
    audience: digest::DigestAudience,
    format: digest::DigestFormat,
    include_next: Option<bool>,
    include_process: Option<bool>,
    out: Option<std::path::PathBuf>,
    copy: bool,
    reset: bool,
    store: &RequirementsStore,
) -> Result<()> {
    let project_root = find_project_root()?;
    let until = chrono::Utc::now();
    let since = digest::parse_digest_since(since_raw, &project_root)?;
    if since > until {
        anyhow::bail!(
            "--since resolves to {} which is after now; nothing to digest",
            since.to_rfc3339()
        );
    }
    let include_next = include_next.unwrap_or(true);
    // operator is a CLI-surface lens (not a work narrative), so process/memory
    // artifacts are off by default just like customer. trace:STORY-541
    let include_process = include_process.unwrap_or(!matches!(
        audience,
        digest::DigestAudience::Customer | digest::DigestAudience::Operator
    ));
    let opts = digest::DigestOptions {
        since,
        until,
        audience,
        format,
        include_next,
        include_process,
        out,
    };
    // TASK-381: when --copy is set, render to a string and try the
    // system clipboard; fall through to stdout if no clipboard tool
    // is found. --copy is a no-op on --reset (which clears the marker
    // without rendering anything). Composes with --out: writes the
    // file AND copies. trace:TASK-381 | ai:claude
    if copy {
        if reset {
            digest::run(opts, &project_root, store, reset)?;
            println!(
                "{} --copy was a no-op (--reset cleared the cadence marker; nothing rendered)",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).yellow()
            );
            return Ok(());
        }
        let rendered = digest::render_string(&opts, &project_root, store)?;
        // Also honor --out if supplied — write file before copying.
        if let Some(path) = &opts.out {
            aida_core::write_atomic(path, &rendered)
                .with_context(|| format!("Failed to write {}", path.display()))?;
            eprintln!("Wrote digest to {}", path.display());
        }
        if copy_to_clipboard(&rendered) {
            println!(
                "{} copied digest to clipboard ({} chars)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                rendered.chars().count(),
            );
        } else {
            println!(
                "{} no clipboard tool found (wl-copy/xclip/xsel/pbcopy/clip) — printing instead",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            print!("{rendered}");
            if !rendered.ends_with('\n') {
                println!();
            }
        }
        return Ok(());
    }
    digest::run(opts, &project_root, store, reset)
}

/// Generate a curated narrative work digest — Released / Major progress /
/// Strategic direction / Next iteration / Process artifacts — for a time
/// window. Editorial logic is mechanical: drop typo/chore/style commits,
/// collapse cluster-PRs to one theme line, keep rejected specs only when
/// they carry a supersedes/pivoted-from link, strip SPEC-IDs in customer
/// mode. Default audience is `customer`, default window is
/// `.aida/last-digest.toml` marker → 24h.
// trace:STORY-252
#[derive(clap::Parser, Debug)]
#[clap(name = "digest")]
pub(crate) struct DigestArgs {
    /// Window start: a relative duration (`7d`, `12h`, `2w`, `30m`, or
    /// `24 hours ago`), an ISO date (`2026-05-01`, local midnight), a
    /// zone-less ISO datetime (local time), RFC3339, or a git tag/ref.
    /// Absent: marker's window_end, else last 24h.
    // trace:TASK-1509 | ai:claude
    #[clap(long, value_name = "WINDOW")]
    pub since: Option<String>,
    /// Tailor framing + SPEC-ID visibility for the reader.
    #[clap(long, value_enum, default_value_t = crate::digest::DigestAudience::Customer)]
    pub audience: crate::digest::DigestAudience,
    /// Digest rendering format (markdown / plain / json / brief). Named
    /// `--digest-format` because `--format` is the global output-format flag
    /// (OutputFormat), and a same-named subcommand arg of a different type
    /// makes clap panic on parse.
    // trace:BUG-1141 | ai:claude
    #[clap(long = "digest-format", value_enum, default_value_t = crate::digest::DigestFormat::Markdown)]
    pub digest_format: crate::digest::DigestFormat,
    /// Include forward-looking "Next iteration" section. Default on; pass
    /// `--include-next=false` to suppress.
    #[clap(
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true",
        require_equals = true
    )]
    pub include_next: Option<bool>,
    /// Include "Process artifacts" memory-pack section. Default: on for
    /// team/self, off for customer. Pass `--include-process=false` to
    /// suppress, bare `--include-process` to force on.
    #[clap(
        long,
        value_name = "BOOL",
        num_args = 0..=1,
        default_missing_value = "true",
        require_equals = true
    )]
    pub include_process: Option<bool>,
    /// Write digest to this file instead of stdout.
    #[clap(long, value_name = "PATH")]
    pub out: Option<std::path::PathBuf>,
    /// Copy the rendered digest to the system clipboard. Tries
    /// wl-copy / xclip / xsel / pbcopy / clip in turn; falls back
    /// to a warning and stdout if no clipboard tool is found.
    /// Composes with --out (writes both); composes with --format.
    // trace:TASK-381 | ai:claude
    #[clap(long)]
    pub copy: bool,
    /// Clear the cadence marker and exit without rendering. Use when the
    /// next digest should not auto-resume from the current window_end.
    #[clap(long)]
    pub reset: bool,
}

fn run_digest(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <DigestArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                DigestArgs {
                    since,
                    audience,
                    digest_format,
                    include_next,
                    include_process,
                    out,
                    copy,
                    reset,
                } => {
                    let store = backend.load()?;
                    return crate::command_registry::returned(self::handle_digest_command(
                        since.as_deref(),
                        *audience,
                        *digest_format,
                        *include_next,
                        *include_process,
                        out.clone(),
                        *copy,
                        *reset,
                        &store,
                    ));
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                DigestArgs { .. } => {
                    anyhow::bail!(
                    "`aida digest` requires a git-canonical store. Run `aida init` (or upgrade from \
                     the deprecated centralized backend with `aida db export-git`)."
                );
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static DIGEST: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "digest",
        help_after: "field-study",
        command: <DigestArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_digest),
    };
