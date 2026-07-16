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
