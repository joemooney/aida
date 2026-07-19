//! `aida dev` developer-workflow command cluster — the pyenv-style in-repo
//! build activation (activate / deactivate / status / serve / shell-init) plus
//! the release passthrough and the pure auto-select binary picker.
//!
//! Extracted verbatim from main.rs (SPIKE-78 — pure movement, no behavior
//! change). Shared SHA-classification helpers (`classify_sha_match`,
//! `parse_embedded_sha`, `pull_binary_is_stale`, `warn_if_pulled_binary_stale`,
//! `current_branch_head_sha`, `binary_embedded_sha`) and `eval_subcommand_hint`
//! stay in main.rs and are reached via `crate::`.

use crate::*;
use anyhow::{Context, Result};
use colored::Colorize;

pub(crate) fn handle_dev_command(cmd: &DevCommand) -> Result<()> {
    match cmd {
        DevCommand::Activate {
            repo,
            profile,
            debug,
            release,
            auto,
        } => handle_dev_activate(repo.as_deref(), profile.as_deref(), *debug, *release, *auto),
        DevCommand::Deactivate => handle_dev_deactivate(),
        DevCommand::Status => handle_dev_status(),
        DevCommand::Ps1 => handle_dev_ps1(),
        DevCommand::ShellInit { install } => handle_dev_shell_init(*install),
        DevCommand::Serve {
            rest_port,
            grpc_port,
            web_port,
            no_web,
        } => handle_dev_serve(*rest_port, *grpc_port, *web_port, *no_web),
        DevCommand::Release { bump } => handle_dev_release(bump),
        DevCommand::Patch => handle_dev_release("patch"),
    }
}

/// Locate an AIDA repo: prefer `--repo` arg, then $AIDA_DEV_REPO, then CWD
/// if it looks like one. Returns absolute path.
fn resolve_aida_repo(repo_arg: Option<&str>) -> Result<std::path::PathBuf> {
    // Track WHICH source we used so the error message can be specific about
    // what to fix.
    let (candidate, source): (std::path::PathBuf, &str) = if let Some(p) = repo_arg {
        (std::path::PathBuf::from(p), "--repo")
    } else if let Ok(p) = std::env::var("AIDA_DEV_REPO") {
        (std::path::PathBuf::from(p), "$AIDA_DEV_REPO")
    } else {
        (std::env::current_dir()?, "PWD")
    };

    let canonical = candidate.canonicalize().with_context(|| {
        format!(
            "Cannot resolve AIDA repo path ({}): {}",
            source,
            candidate.display()
        )
    })?;

    if !is_aida_repo(&canonical) {
        // Build a context-specific error. PWD-based failure is most often a
        // shell that hasn't picked up the AIDA_DEV_REPO export yet (the
        // `aida dev shell-init --install` flow writes it to .bashrc but
        // doesn't reload the current shell). Surface that fix prominently.
        let in_bashrc = dirs::home_dir()
            .map(|h| h.join(".bashrc"))
            .filter(|p| p.exists())
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .map(|s| s.contains("AIDA_DEV_REPO"))
            .unwrap_or(false);

        let mut msg = format!(
            "Cannot locate the aida repo for activation:\n  \
             - {} ({}) is not a joemooney/aida checkout",
            source,
            canonical.display()
        );
        if source == "PWD" {
            msg.push_str("\n  - $AIDA_DEV_REPO is not set in this shell");
            if in_bashrc {
                msg.push_str(
                    "\n\n\
                     Your ~/.bashrc has the export, but this shell hasn't picked it up yet:\n  \
                       exec bash      (restart bash in place)\n  \
                       source ~/.bashrc",
                );
            } else {
                msg.push_str(
                    "\n\n\
                     One-time setup (from inside the aida repo):\n  \
                       aida dev shell-init --install\n  \
                       exec bash    (or: source ~/.bashrc)",
                );
            }
            msg.push_str(
                "\n\nOr pass it directly:\n  \
                 aida dev activate --repo /path/to/aida",
            );
        } else {
            msg.push_str(&format!(
                "\n\n\
                 Check that {} points at a real aida checkout (must contain a Cargo.toml \
                 with `repository = \"https://github.com/joemooney/aida\"`).",
                source
            ));
        }
        anyhow::bail!("{}", msg);
    }
    Ok(canonical)
}

/// TASK-221: how the binary was picked, for the activate-time banner.
/// Surfaced verbatim so `aida dev status` can show "why this binary."
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BinarySelectionReason {
    /// User passed `--release` / `--debug` / a positional / the env pin.
    Explicit,
    /// Binary's embedded SHA matches current branch HEAD exactly.
    ShaExactMatch,
    /// Binary's embedded SHA is an ancestor of current branch HEAD
    /// (current is ahead of build — likely fine; would be more so if rebuilt).
    ShaAncestorMatch,
    /// No SHA-matching binary found; fell back to most-recently-built
    /// with a warning printed at activate time.
    RecencyFallback,
    /// Only one binary exists (no choice to make).
    OnlyOne,
}

/// Pick the build directory + profile name for activation, honoring an
/// explicit profile request (debug / release) when given, else preferring
/// the binary whose embedded git SHA matches (or is an ancestor of)
/// current branch HEAD (TASK-221). Recency is the fallback when neither
/// binary's SHA is recognizable on the current branch. Errors when the
/// requested profile isn't built, or when neither exists at all.
fn pick_dev_binary_dir(
    repo: &std::path::Path,
    requested: Option<&str>,
) -> Result<(std::path::PathBuf, &'static str, BinarySelectionReason)> {
    let release = repo.join("target/release/aida");
    let debug = repo.join("target/debug/aida");
    let release_mtime = std::fs::metadata(&release).and_then(|m| m.modified()).ok();
    let debug_mtime = std::fs::metadata(&debug).and_then(|m| m.modified()).ok();

    if let Some(req) = requested {
        return match req {
            "debug" => {
                if debug_mtime.is_none() {
                    anyhow::bail!(
                        "No debug build at {}.\nRun `cargo build` (debug) first.",
                        debug.display()
                    );
                }
                Ok((
                    repo.join("target/debug"),
                    "debug",
                    BinarySelectionReason::Explicit,
                ))
            }
            "release" => {
                if release_mtime.is_none() {
                    anyhow::bail!(
                        "No release build at {}.\nRun `cargo build --release` first.",
                        release.display()
                    );
                }
                Ok((
                    repo.join("target/release"),
                    "release",
                    BinarySelectionReason::Explicit,
                ))
            }
            other => anyhow::bail!("unknown profile '{}': expected debug or release", other),
        };
    }

    // BUG-643: auto mode (pin == auto) must RE-PICK the freshest SHA-matched
    // binary on EVERY activate — never stay sticky on whichever build is
    // already active. Probe both candidates ({mtime, sha-verdict}) and run the
    // pure selector so a just-rebuilt debug flips in over an older release.
    let (release_cand, debug_cand) = dev_build_candidates(repo);
    match auto_select_dev_profile(release_cand, debug_cand) {
        Some((DevProfile::Release, reason)) => Ok((repo.join("target/release"), "release", reason)),
        Some((DevProfile::Debug, reason)) => Ok((repo.join("target/debug"), "debug", reason)),
        None => anyhow::bail!(
            "No aida binary found at {} or {}.\n\
             Run `cargo build --release` (or just `cargo build`) first.",
            release.display(),
            debug.display()
        ),
    }
}
/// True when the inactive-side build at `<repo>/target/<other>/aida` is
/// newer than the active-side build at `<repo>/target/<active>/aida`.
/// Used for the stale-build warning + PS1 marker.
// trace:FR-1-068 | ai:claude
fn alternate_build_is_newer(repo: &std::path::Path, active: &str) -> bool {
    let other = if active == "debug" {
        "release"
    } else {
        "debug"
    };
    let active_mtime = std::fs::metadata(repo.join(format!("target/{}/aida", active)))
        .and_then(|m| m.modified())
        .ok();
    let other_mtime = std::fs::metadata(repo.join(format!("target/{}/aida", other)))
        .and_then(|m| m.modified())
        .ok();
    matches!((active_mtime, other_mtime), (Some(a), Some(o)) if o > a)
}

/// Which build profile auto-selection chose.
// trace:BUG-643 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DevProfile {
    Release,
    Debug,
}

impl DevProfile {
    fn as_str(self) -> &'static str {
        match self {
            DevProfile::Release => "release",
            DevProfile::Debug => "debug",
        }
    }
}

/// A built binary candidate for auto-selection: its file mtime plus how its
/// embedded SHA relates to current HEAD.
// trace:BUG-643 | ai:claude
#[derive(Debug, Clone, Copy)]
pub(crate) struct DevBuildCandidate {
    pub mtime: std::time::SystemTime,
    pub sha: ShaMatch,
}

/// Rank a SHA verdict for auto-selection. An exact match to HEAD is the
/// strongest signal the binary reflects the source on disk, an ancestor is
/// weaker, and anything else (diverged / unknown) is recency-only.
// trace:BUG-643 | ai:claude
fn sha_rank(m: ShaMatch) -> u8 {
    match m {
        ShaMatch::Exact => 2,
        ShaMatch::Ancestor => 1,
        ShaMatch::Unrelated | ShaMatch::Unknown => 0,
    }
}

/// Pure auto-mode (`pin == auto`) build picker (BUG-643). Given each profile's
/// `{mtime, sha-verdict}`, RE-PICK the freshest SHA-matched binary on every
/// call so a fresh `aida dev activate` flips to a newer build instead of
/// staying sticky on whichever build happens to be active.
///
/// Ordering:
///   1. higher SHA rank wins (exact > ancestor > diverged/unknown) — the
///      TASK-221 invariant that a HEAD-matching binary beats a stale one;
///   2. within the same rank, the freshest (highest mtime) wins — this is
///      what makes a just-rebuilt debug flip in over an older release;
///   3. an exact mtime tie falls to release (the conventional default).
///
/// The selection reason is derived from the WINNER's own SHA verdict so the
/// `dev activate` chip stays truthful. Returns `None` only when neither build
/// exists.
// trace:BUG-643 | ai:claude
pub(crate) fn auto_select_dev_profile(
    release: Option<DevBuildCandidate>,
    debug: Option<DevBuildCandidate>,
) -> Option<(DevProfile, BinarySelectionReason)> {
    match (release, debug) {
        (Some(r), Some(d)) => {
            let (rr, dr) = (sha_rank(r.sha), sha_rank(d.sha));
            let pick = if rr != dr {
                // Stronger SHA match wins outright.
                if rr > dr {
                    DevProfile::Release
                } else {
                    DevProfile::Debug
                }
            } else if r.mtime != d.mtime {
                // Same SHA class → freshest mtime wins (the sticky-bug fix).
                if r.mtime > d.mtime {
                    DevProfile::Release
                } else {
                    DevProfile::Debug
                }
            } else {
                // Exact tie → release, the stable conventional default.
                DevProfile::Release
            };
            let winner_sha = match pick {
                DevProfile::Release => r.sha,
                DevProfile::Debug => d.sha,
            };
            let reason = match winner_sha {
                ShaMatch::Exact => BinarySelectionReason::ShaExactMatch,
                ShaMatch::Ancestor => BinarySelectionReason::ShaAncestorMatch,
                ShaMatch::Unrelated | ShaMatch::Unknown => BinarySelectionReason::RecencyFallback,
            };
            Some((pick, reason))
        }
        (Some(_), None) => Some((DevProfile::Release, BinarySelectionReason::OnlyOne)),
        (None, Some(_)) => Some((DevProfile::Debug, BinarySelectionReason::OnlyOne)),
        (None, None) => None,
    }
}

/// Probe `<repo>/target/{release,debug}/aida`: file mtime + embedded-SHA
/// verdict vs current HEAD, packaged for [`auto_select_dev_profile`]. Shared
/// by `pick_dev_binary_dir`'s auto branch and `dev status`'s flip advice so
/// the two always agree on what auto would pick.
// trace:BUG-643 | ai:claude
fn dev_build_candidates(
    repo: &std::path::Path,
) -> (Option<DevBuildCandidate>, Option<DevBuildCandidate>) {
    let release = repo.join("target/release/aida");
    let debug = repo.join("target/debug/aida");
    let release_mtime = std::fs::metadata(&release).and_then(|m| m.modified()).ok();
    let debug_mtime = std::fs::metadata(&debug).and_then(|m| m.modified()).ok();
    let head_sha = current_branch_head_sha(repo);
    let verdict = |mtime: Option<std::time::SystemTime>, bin: &std::path::Path| {
        mtime.map(|m| {
            let sha = binary_embedded_sha(bin);
            let verdict = head_sha
                .as_ref()
                .zip(sha.as_ref())
                .map(|(h, s)| classify_sha_match(repo, s, h))
                .unwrap_or(ShaMatch::Unknown);
            DevBuildCandidate {
                mtime: m,
                sha: verdict,
            }
        })
    };
    (
        verdict(release_mtime, &release),
        verdict(debug_mtime, &debug),
    )
}

/// What `aida dev activate` (auto / unpinned) would select for `repo` right
/// now — used by `aida dev status` to give accurate flip advice instead of
/// promising a re-run will flip when it would not. Returns the profile name
/// ("debug"/"release"), or None if no build exists.
// trace:BUG-643 | ai:claude
fn auto_pick_profile_name(repo: &std::path::Path) -> Option<&'static str> {
    let (release_cand, debug_cand) = dev_build_candidates(repo);
    auto_select_dev_profile(release_cand, debug_cand).map(|(p, _)| p.as_str())
}

fn sha_prefix_match(a: &str, b: &str) -> bool {
    if a == "unknown" || b == "unknown" || a.is_empty() || b.is_empty() {
        return false;
    }
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    a.starts_with(&b) || b.starts_with(&a)
}

/// Cheap HEAD resolver for prompt-time use. Avoids spawning `git` by reading
/// `.git/HEAD` and the referenced loose ref directly. Linked worktree gitfiles
/// are supported; packed refs intentionally fall back to `None` so the prompt
/// goes quiet rather than doing expensive work.
// trace:TASK-1157 | ai:codex
pub(crate) fn current_branch_head_sha_direct(repo: &std::path::Path) -> Option<String> {
    let git_dot = repo.join(".git");
    let git_dir = if git_dot.is_dir() {
        git_dot
    } else {
        let gitfile = std::fs::read_to_string(&git_dot).ok()?;
        let raw = gitfile.trim().strip_prefix("gitdir:")?.trim();
        let path = std::path::PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            repo.join(path)
        }
    };
    let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|s| {
            let path = std::path::PathBuf::from(s.trim());
            if path.is_absolute() {
                path
            } else {
                git_dir.join(path)
            }
        })
        .unwrap_or_else(|| git_dir.clone());
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        let reference = reference.trim();
        if !reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
        {
            return None;
        }
        return std::fs::read_to_string(git_dir.join(reference))
            .or_else(|_| std::fs::read_to_string(common_dir.join(reference)))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    Some(head.to_string()).filter(|s| !s.is_empty())
}

/// Compact prompt token:
/// - empty: active binary matches HEAD
/// - ⇄: another built profile matches HEAD, so `aida dev activate` can flip
/// - ↻: no known build matches HEAD, so rebuild
// trace:TASK-1157 | ai:codex
pub(crate) fn ps1_staleness_token(
    active_sha: &str,
    head_sha: Option<&str>,
    other_sha: Option<&str>,
) -> &'static str {
    let Some(head_sha) = head_sha else {
        return "";
    };
    if sha_prefix_match(active_sha, head_sha) {
        ""
    } else if other_sha
        .map(|s| sha_prefix_match(s, head_sha))
        .unwrap_or(false)
    {
        "⇄"
    } else {
        "↻"
    }
}

fn handle_dev_ps1() -> Result<()> {
    let repo = match std::env::var("AIDA_DEV_REPO") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => return Ok(()),
    };
    let active_profile = std::env::var("AIDA_DEV_PROFILE").unwrap_or_default();
    let other_profile = match active_profile.as_str() {
        "debug" => Some("release"),
        "release" => Some("debug"),
        _ => None,
    };
    let head = current_branch_head_sha_direct(&repo);
    if ps1_staleness_token(build_git_sha(), head.as_deref(), None).is_empty() {
        return Ok(());
    }
    let other_sha = other_profile
        .map(|p| repo.join(format!("target/{}/aida", p)))
        .filter(|p| p.exists())
        .and_then(|p| binary_embedded_sha(&p));

    print!(
        "{}",
        ps1_staleness_token(build_git_sha(), head.as_deref(), other_sha.as_deref())
    );
    Ok(())
}

fn handle_dev_activate(
    repo_arg: Option<&str>,
    profile_pos: Option<&str>,
    debug_flag: bool,
    release_flag: bool,
    auto_flag: bool,
) -> Result<()> {
    let repo = resolve_aida_repo(repo_arg)?;

    // Resolve the explicit-profile request from any of: positional `profile`,
    // --debug / --release / --auto flags, or an existing AIDA_DEV_PROFILE_PIN.
    // Precedence: explicit CLI request beats the env-var pin; --auto clears.
    // trace:FR-1-068 | ai:claude
    let cli_request: Option<&str> = match (profile_pos, debug_flag, release_flag, auto_flag) {
        (Some("debug"), _, _, _) => Some("debug"),
        (Some("release"), _, _, _) => Some("release"),
        (Some("auto"), _, _, _) => None, // positional 'auto' also clears
        (_, true, _, _) => Some("debug"),
        (_, _, true, _) => Some("release"),
        _ => None,
    };
    let clear_pin = auto_flag || profile_pos == Some("auto");
    let env_pin = std::env::var("AIDA_DEV_PROFILE_PIN").ok();
    let effective_request: Option<&str> = if clear_pin {
        None
    } else if cli_request.is_some() {
        cli_request
    } else {
        env_pin.as_deref().filter(|s| !s.is_empty())
    };

    let (bin_dir, profile, reason) = pick_dev_binary_dir(&repo, effective_request)?;
    let stale = alternate_build_is_newer(&repo, profile);
    let ps1_marker = if stale { "*" } else { "" };

    // TASK-221: when the chosen binary doesn't match current HEAD by SHA,
    // surface a warning so the user knows in advance that aida commands
    // may behave as the binary's compile-time source, not the source on
    // disk. Print to stderr so the eval-friendly stdout shell code is
    // untouched. trace:TASK-221 | ai:claude
    if reason == BinarySelectionReason::RecencyFallback {
        let head = current_branch_head_sha(&repo);
        let bin_sha = binary_embedded_sha(&bin_dir.join("aida"));
        eprintln!(
            "{} active aida binary SHA {} does not match current HEAD {}; \
             expect parse failures or stale behavior if source diverged.",
            "Warning:".yellow().bold(),
            bin_sha.as_deref().unwrap_or("?"),
            head.as_deref().unwrap_or("?"),
        );
        eprintln!("  Recommended: `cargo build --release` (or `cargo build`) to refresh.",);
    }

    // Quote-safety: paths shouldn't contain double-quotes in practice;
    // single-quote everything we emit so shell evaluation is safe.
    let reason_chip = match reason {
        BinarySelectionReason::Explicit => " (explicit)",
        BinarySelectionReason::ShaExactMatch => " (SHA matches HEAD)",
        BinarySelectionReason::ShaAncestorMatch => " (SHA is HEAD ancestor)",
        BinarySelectionReason::RecencyFallback => " (recency fallback)",
        BinarySelectionReason::OnlyOne => "",
    };
    println!(
        "# aida dev activate — using {} build at {}{}{}",
        profile,
        bin_dir.display(),
        if stale {
            "  (alternate build is newer)"
        } else {
            ""
        },
        reason_chip,
    );
    println!("export AIDA_DEV_REPO='{}'", repo.display());
    println!("export AIDA_DEV_BIN='{}'", bin_dir.display());
    println!("export AIDA_DEV_PROFILE='{}'", profile);
    println!("export AIDA_DEV_ACTIVE=1");

    // Persist the pin across re-activations. Three cases:
    //   - explicit CLI request → set the pin to that profile
    //   - --auto / 'auto' positional → clear the pin
    //   - neither → leave the existing pin alone (sticky)
    if let Some(pin) = cli_request {
        println!("export AIDA_DEV_PROFILE_PIN='{}'", pin);
    } else if clear_pin {
        println!("unset AIDA_DEV_PROFILE_PIN");
    }

    println!("if [ -z \"${{AIDA_DEV_PREV_PATH+x}}\" ]; then");
    println!("    export AIDA_DEV_PREV_PATH=\"$PATH\"");
    println!("fi");
    println!("export PATH='{}':\"$PATH\"", bin_dir.display());
    // TASK-19: splice-in semantics for PS1 instead of save/restore. We
    // record the literal prefix we're prepending in AIDA_DEV_PS1_PREFIX
    // so deactivate can strip exactly the same string regardless of what
    // else (e.g., `aida role enter`) has touched PS1 in between.
    // trace:TASK-19 | ai:claude
    //
    // BUG-70: strip ALL existing `(aida-PROFILE) ` prefixes before
    // prepending the new one. Repeated `aida dev activate` (or switching
    // debug ↔ release without a deactivate in between) would otherwise
    // stack multiple prefixes like "(aida-debug) (aida-debug) joe@…".
    // Mirrors BUG-60's loop for role-enter PS1 hygiene. The strip is
    // pattern-based (any `(aida-WORD) ` token, with optional trailing
    // `*` for the stale-marker variant) so stale prefixes from prior
    // sessions / lost env vars are cleaned up too. trace:BUG-70 | ai:claude
    let ps1_prefix = format!("(aida-{}{}) ", profile, ps1_marker);
    println!("if [ -n \"${{PS1+x}}\" ]; then");
    println!("    while case \"$PS1\" in *'(aida-'*') '*) true;; *) false;; esac; do");
    println!("        _aida_old_ps1=\"$PS1\"");
    println!("        _aida_after=\"${{PS1#*'(aida-'}}\"");
    println!("        _aida_tag=\"${{_aida_after%%') '*}}\"");
    println!("        PS1=\"${{PS1//'(aida-'$_aida_tag') '/}}\"");
    println!("        [ \"$PS1\" = \"$_aida_old_ps1\" ] && break");
    println!("    done");
    println!("    unset _aida_old_ps1 _aida_after _aida_tag");
    println!("    export AIDA_DEV_PS1_PREFIX='{}'", ps1_prefix);
    println!("    export PS1=\"$AIDA_DEV_PS1_PREFIX$PS1\"");
    println!("fi");

    let pin_note = match cli_request {
        Some(p) => format!(", pinned to {}", p),
        None if clear_pin => ", pin cleared".to_string(),
        None => String::new(),
    };
    let stale_note = if stale {
        format!(
            " {} alternate build is newer — run `aida dev status` for details",
            crate::glyph(crate::glyphs::Glyph::Warning)
        )
    } else {
        String::new()
    };
    println!(
        "echo '{} aida dev activated ({} build at {}{}){}'",
        crate::glyph(crate::glyphs::Glyph::Check),
        profile,
        bin_dir.display(),
        pin_note,
        stale_note
    );
    Ok(())
}

fn handle_dev_deactivate() -> Result<()> {
    println!("# aida dev deactivate — restoring PATH and splicing dev prefix out of PS1");
    println!("if [ -n \"${{AIDA_DEV_PREV_PATH+x}}\" ]; then");
    println!("    export PATH=\"$AIDA_DEV_PREV_PATH\"");
    println!("    unset AIDA_DEV_PREV_PATH");
    println!("fi");
    // TASK-19: splice-out semantics. Strip exactly the prefix we recorded
    // at activate time so any other PS1 modifiers added in between (role
    // prefix, virtualenv name, etc.) are preserved on deactivate.
    // trace:TASK-19 | ai:claude
    println!("if [ -n \"${{AIDA_DEV_PS1_PREFIX+x}}\" ] && [ -n \"${{PS1+x}}\" ]; then");
    println!("    PS1=\"${{PS1/$AIDA_DEV_PS1_PREFIX/}}\"");
    println!("    unset AIDA_DEV_PS1_PREFIX");
    println!("fi");
    // Clean up the legacy save/restore env var if any prior session set it.
    println!("unset AIDA_DEV_PREV_PS1");
    println!(
        "unset AIDA_DEV_REPO AIDA_DEV_BIN AIDA_DEV_PROFILE AIDA_DEV_ACTIVE AIDA_DEV_PROFILE_PIN"
    );
    println!(
        "echo '{} aida dev deactivated'",
        crate::glyph(crate::glyphs::Glyph::Check)
    );
    Ok(())
}

fn handle_dev_status() -> Result<()> {
    let active = std::env::var("AIDA_DEV_ACTIVE").is_ok();
    println!(
        "Activation:   {}",
        if active {
            "ACTIVE".green().to_string()
        } else {
            // trace:TASK-667 — wrapper-correct activate form.
            format!(
                "(not active — `{}` to enable)",
                eval_subcommand_hint("dev activate")
            )
            .yellow()
            .to_string()
        }
    );
    if active {
        if let Ok(p) = std::env::var("AIDA_DEV_REPO") {
            println!("Repo:         {}", p);
        }
        if let Ok(b) = std::env::var("AIDA_DEV_BIN") {
            println!("Binary dir:   {}", b);
            let aida_path = std::path::PathBuf::from(&b).join("aida");
            if let Ok(meta) = std::fs::metadata(&aida_path) {
                if let Ok(modified) = meta.modified() {
                    if let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH) {
                        let dt =
                            chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                                .map(|t| t.to_rfc3339())
                                .unwrap_or_else(|| "?".into());
                        println!("Built at:     {}", dt);
                    }
                }
            }
        }
        if let Ok(p) = std::env::var("AIDA_DEV_PROFILE") {
            println!("Build profile: {}", p);
        }
        // TASK-221: compare active binary's embedded SHA to current HEAD.
        // Surfaces the same classification `dev activate` used; reads as
        // "is this binary in sync with the source tree right now?"
        // trace:TASK-221 | ai:claude
        if let (Ok(bin_dir), Ok(repo)) = (
            std::env::var("AIDA_DEV_BIN"),
            std::env::var("AIDA_DEV_REPO"),
        ) {
            let aida_path = std::path::PathBuf::from(&bin_dir).join("aida");
            let repo_path = std::path::PathBuf::from(&repo);
            let head = current_branch_head_sha(&repo_path);
            let bin_sha = binary_embedded_sha(&aida_path);
            if let (Some(h), Some(b)) = (head.as_deref(), bin_sha.as_deref()) {
                let kind = classify_sha_match(&repo_path, b, h);
                let label = match kind {
                    ShaMatch::Exact => "exact match".green().to_string(),
                    ShaMatch::Ancestor => "ancestor of HEAD".cyan().to_string(),
                    ShaMatch::Unrelated => "DIVERGED from HEAD".red().bold().to_string(),
                    ShaMatch::Unknown => "(git unavailable)".dimmed().to_string(),
                };
                let bin_short = b.get(..b.len().min(8)).unwrap_or(b);
                let head_short = h.get(..h.len().min(8)).unwrap_or(h);
                println!(
                    "Binary SHA:   {} → HEAD {}  [{}]",
                    bin_short, head_short, label
                );
                if matches!(kind, ShaMatch::Unrelated) {
                    println!(
                        "      {}: rebuild with `cargo build --release` (or `cargo build`)",
                        "Recommended".yellow().bold()
                    );
                }
            }
        }
        match std::env::var("AIDA_DEV_PROFILE_PIN") {
            Ok(pin) if !pin.is_empty() => {
                println!("Pin:          {} (sticky across re-activations)", pin);
            }
            _ => {
                println!(
                    "Pin:          {} (freshest of debug/release wins on `aida dev activate`)",
                    "auto".dimmed()
                );
            }
        }
    }

    // Stale-build warning: when we know the active repo + profile, compare
    // the inactive-side build's mtime. If newer, surface — re-running
    // `aida dev activate` would silently flip you to the alternate.
    // trace:FR-1-068 | ai:claude
    if active {
        let repo = std::env::var("AIDA_DEV_REPO").ok();
        let profile = std::env::var("AIDA_DEV_PROFILE").ok();
        if let (Some(repo), Some(profile)) = (repo, profile) {
            let repo_path = std::path::PathBuf::from(&repo);
            if alternate_build_is_newer(&repo_path, &profile) {
                let other = if profile == "debug" {
                    "release"
                } else {
                    "debug"
                };
                println!();
                println!(
                    "{}: the {} build is newer than the active {} build.",
                    "WARN".yellow().bold(),
                    other.bold(),
                    profile.bold()
                );
                let pinned = std::env::var("AIDA_DEV_PROFILE_PIN")
                    .map(|p| !p.is_empty())
                    .unwrap_or(false);
                if pinned {
                    println!(
                        "      Pin keeps you on {}. Run `aida dev activate --auto` to clear",
                        profile
                    );
                    println!(
                        "      and pick the freshest, or `aida dev activate {}` to switch.",
                        other
                    );
                } else {
                    // BUG-643: only promise a plain re-run flips when auto-select
                    // ACTUALLY would. A newer-by-mtime alternate that is a weaker
                    // SHA match (e.g. ancestor vs the active exact match) is NOT
                    // auto-picked, so the old "Re-run to flip" advice was false —
                    // point at the explicit override in that case instead.
                    match auto_pick_profile_name(&repo_path) {
                        Some(pick) if pick != profile => {
                            println!(
                                "      Re-run `aida dev activate` to flip to {}, or pin with",
                                pick
                            );
                            println!(
                                "      `aida dev activate {}` to keep working on {}.",
                                profile, profile
                            );
                        }
                        _ => {
                            println!(
                                "      Auto-select keeps {} (it matches HEAD more closely).",
                                profile
                            );
                            println!("      To switch anyway, run `aida dev activate {}`.", other);
                        }
                    }
                }
            }
        }
    }
    println!(
        "PS1 marker:   {} current, {} activate matching other build, {} rebuild",
        "empty".dimmed(),
        "⇄".cyan(),
        "↻".yellow()
    );

    // Also report which `aida` actually wins on PATH right now.
    if let Ok(out) = std::process::Command::new("which").arg("aida").output() {
        let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !resolved.is_empty() {
            println!("`which aida`: {}", resolved);
        }
    }
    Ok(())
}
/// Shell helpers emitted by `aida dev shell-init`. A single `aida()` wrapper
/// function — pyenv/rbenv style. For most subcommands it just delegates to
/// the binary. For the handful of eval-only subcommands (dev activate, dev
/// deactivate, role enter, role end, role add, worktree enter — those that
/// need to mutate the calling shell, e.g. cd into a new worktree), it wraps
/// them in `eval "$(command aida ...)"` so they actually take effect in the
/// user's shell instead of getting lost in the subprocess.
///
/// Use `command aida ...` to bypass the wrapper and invoke the binary
/// directly (e.g., for scripting where you want raw stdout).
const SHELL_HELPERS: &str = r#"# AIDA shell wrapper.
#
# Most `aida` subcommands run as plain commands. The few that need to
# modify the calling shell (set env vars, prepend PATH, change PS1) get
# automatically eval'd so they take effect here, not in the subprocess.
#
# Bypass the wrapper with `command aida ...` if you need raw stdout.

# trace:TASK-667 — signal the wrapper's presence so the binary tailors its
# auto-eval hints. When this is set, the `aida()` function below auto-evals
# the shell-modifying subcommands, so the binary prints the BARE form
# (`aida role enter <role>`); printing `eval "$(...)"` would double-eval and
# lose the effect. The value lists the auto-evaled verb groups for any future
# wrapper-aware decisions.
export AIDA_SHELL_WRAPPER='role,session,dev,worktree'

aida() {
    # Take the first two positional words verbatim — that's enough to
    # disambiguate every eval-required subcommand we have.
    local _aida_cmd="${1:-} ${2:-}"
    case "$_aida_cmd" in
        "dev activate"|"dev deactivate"|"role enter"|"role end"|"role add"|"session start"|"session end"|"worktree enter")
            # session start/end split output: stderr for human messages
            # (status, prompts), stdout for the shell-modifying lines
            # (`export AIDA_SESSION_ID=...` / `unset AIDA_SESSION_ID`).
            # `eval "$(...)"` captures stdout only — stderr passes through
            # to the user, stdin still reaches the binary for prompts.
            eval "$(command aida "$@")"
            ;;
        "tui"|"tui "*)
            # STORY-681: `aida tui` is now self-sufficient — it dispatches
            # the launcher's chosen command IN-PROCESS and re-enters in a
            # loop, with no fd-3 pipe and no shell wrapper. So this case just
            # passes through. (BUG-612 used to route `aida tui` through the
            # `aida-tui` function to set up fd 3; that detour is no longer
            # needed. The `aida-tui` function below stays as an opt-in
            # power-user/legacy hook over the STORY-244 fd-3 protocol.)
            command aida "$@"
            ;;
        *)
            command aida "$@"
            ;;
    esac
}

_aida_dev_prompt_marker() {
    [ -n "${PS1+x}" ] || return 0
    [ -n "${AIDA_DEV_ACTIVE+x}" ] || return 0
    [ -n "${AIDA_DEV_PROFILE:-}" ] || return 0

    local _aida_marker
    _aida_marker="$(command aida dev ps1 2>/dev/null || true)"

    while case "$PS1" in *'(aida-'*') '*) true;; *) false;; esac; do
        _aida_old_ps1="$PS1"
        _aida_after="${PS1#*'(aida-'}"
        _aida_tag="${_aida_after%%') '*}"
        PS1="${PS1//'(aida-'$_aida_tag') '/}"
        [ "$PS1" = "$_aida_old_ps1" ] && break
    done
    unset _aida_old_ps1 _aida_after _aida_tag

    export AIDA_DEV_PS1_PREFIX="(aida-${AIDA_DEV_PROFILE}${_aida_marker}) "
    export PS1="$AIDA_DEV_PS1_PREFIX$PS1"
}

if [ -n "${ZSH_VERSION:-}" ]; then
    if ! command -v add-zsh-hook >/dev/null 2>&1; then
        autoload -Uz add-zsh-hook 2>/dev/null || true
    fi
    if command -v add-zsh-hook >/dev/null 2>&1; then
        add-zsh-hook precmd _aida_dev_prompt_marker 2>/dev/null || true
    else
        case " ${precmd_functions[*]-} " in
            *" _aida_dev_prompt_marker "*) ;;
            *) precmd_functions+=(_aida_dev_prompt_marker) ;;
        esac
    fi
else
    case ";${PROMPT_COMMAND:-};" in
        *";_aida_dev_prompt_marker;"*) ;;
        *) PROMPT_COMMAND="_aida_dev_prompt_marker${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
    esac
fi

# STORY-244 launcher wrapper. LEGACY / power-user hook: as of STORY-681
# bare `aida tui` dispatches IN-PROCESS and re-enters on its own, so this
# function is no longer required for `aida tui` to work. It is kept for
# scripts/users that want the fd-3 wire protocol — it explicitly passes
# `--intent-fd 3` to opt into the single-shot emit mode, captures the
# intent line on fd 3, dispatches it, and re-enters. One line per run:
#
#   quit                     stop the loop
#   launch:<command>         eval <command> (typically `aida queue work …`)
#   resume:<session-id>      claude --resume <session-id>
#   shell:<command>          eval <command> (e.g. `gh pr view 42`)
#
# Pass `--once` to disable the re-entry loop (debug / scripting). All
# other args are forwarded to `aida tui --launcher`.
aida-tui() {
    local _once=0
    local -a _args=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --once) _once=1; shift ;;
            *) _args+=("$1"); shift ;;
        esac
    done
    local _intent _cmd _rc
    while true; do
        # The launcher paints the real terminal (stdout/stderr) and writes
        # its single intent line to fd 3 — the redirection here moves fd 3
        # into our captured pipe and points the launcher's stdout/stderr
        # at /dev/tty so it can draw without contaminating the capture.
        _intent="$(command aida tui --launcher --intent-fd 3 "${_args[@]}" 3>&1 1>/dev/tty 2>/dev/tty)"
        case "$_intent" in
            ""|quit)
                break
                ;;
            launch:*)
                _cmd="${_intent#launch:}"
                eval "$_cmd"
                _rc=$?
                # Defensive terminal reset between Claude and the next
                # launcher entry — if the dispatched command crashed and
                # left raw mode on, this prevents the new launcher from
                # painting over garbage. trace:STORY-244 risk #3
                [ $_rc -ne 0 ] && command -v tput >/dev/null && tput reset 2>/dev/null
                ;;
            resume:*)
                claude --resume "${_intent#resume:}"
                _rc=$?
                [ $_rc -ne 0 ] && command -v tput >/dev/null && tput reset 2>/dev/null
                ;;
            shell:*)
                eval "${_intent#shell:}"
                ;;
            *)
                printf 'aida-tui: unrecognized intent line: %s\n' "$_intent" >&2
                break
                ;;
        esac
        [ $_once -eq 1 ] && break
    done
}
"#;

/// TASK-294: the `aida-worker` autonomous-drain loop, emitted alongside the
/// `aida()` wrapper by `aida dev shell-init`. The function reads directives
/// from `.aida/worker.cmd` (a FIFO — one directive per line) and drives a
/// loop:
///
/// - bare/absent file or a `drain` line → run the queue head through a full
///   `aida queue work --auto-complete` lifecycle.
/// - `drain <args>` → run `aida queue work <args> --auto-complete`; the line
///   is consumed (popped) on success so a user can write an overnight plan
///   as a heredoc.
/// - `pause` → log and sleep 30s; line persists.
/// - `exit` → return 0.
/// - unknown verb → defensively treated as `pause`.
///
/// Each drain is wrapped in `timeout` (knob: `AIDA_WORKER_SPEC_TIMEOUT`,
/// default 1800s) so a hung session can't block the loop. The 2026-05-16
/// watchdog comment explicitly asked for this — it applies even today's
/// interactive Claude, where a walked-away user's Ctrl+D-blocked session
/// would otherwise wedge.
///
/// Empty-queue handling is *separate* from failure handling: the
/// canonical `aida queue work` error message contains the literal substring
/// `nothing to drive` for both an empty queue and an all-in-flight queue.
/// The worker greps for that and sleeps 30s (the queue may fill from another
/// session) — it does *not* auto-pause, which would wedge a productive
/// drain the moment it drained everything.
///
/// MUST call `command aida …` (not bare `aida`) so it bypasses the `aida()`
/// wrapper above and gets raw stdout/exit codes.
// trace:TASK-294 | ai:claude
const WORKER_FUNCTION: &str = r#"
# AIDA autonomous-drain worker (TASK-294).
#
# Loop: read .aida/worker.cmd, act on the head directive, re-read.
#
# Directives (one per line, blank/`#` lines skipped):
#   drain                  pick queue head; run `aida queue work --auto-complete`
#   drain <args>           run `aida queue work <args> --auto-complete`; line is
#                          consumed on success (FIFO)
#   pause                  sleep 30s; line persists (worker stays paused until edited)
#   exit                   return 0
#   <anything else>        defensively treated as `pause`
#
# An overnight plan is just a heredoc:
#   printf 'drain batch:autonomy-modes --zen\ndrain batch:cleanup\nexit\n' \
#       > .aida/worker.cmd
#   aida-worker
#
# Knob: AIDA_WORKER_SPEC_TIMEOUT (default 1800s) — per-spec watchdog.
# Exit 124 from `timeout` → log "TIMED OUT" + auto-pause; other non-zero →
# log "halted" + auto-pause; "nothing to drive" output → sleep 30s only.
aida-worker() {
    local project_root cmd_file head verb rest args raw output rc
    local sleep_short=30
    project_root="$(pwd)"
    while [ ! -d "$project_root/.aida" ] && [ "$project_root" != "/" ]; do
        project_root="$(dirname "$project_root")"
    done
    if [ ! -d "$project_root/.aida" ]; then
        echo "aida-worker: no .aida/ directory found from $(pwd); refusing to run" >&2
        return 1
    fi
    cmd_file="$project_root/.aida/worker.cmd"
    echo "aida-worker: starting; directive file = $cmd_file"
    while true; do
        head=""
        if [ -f "$cmd_file" ]; then
            # Read the first non-blank, non-comment line, normalised by
            # stripping leading/trailing whitespace so an indented directive
            # like `\tdrain batch:x` matches the trimmed form that
            # `parse_directives_from_str` produces and that
            # `_aida_worker_pop_head` compares against. trace:TASK-364 | ai:claude
            head=$(awk 'NF && $1 !~ /^#/ {sub(/^[[:space:]]+/, ""); sub(/[[:space:]]+$/, ""); print; exit}' "$cmd_file" 2>/dev/null || true)
        fi
        if [ -z "$head" ]; then
            verb="drain"
            rest=""
            raw=""
        else
            verb=$(printf '%s\n' "$head" | awk '{print $1}')
            rest=$(printf '%s\n' "$head" | awk '{$1=""; sub(/^ /, ""); print}')
            raw="$head"
        fi
        case "$verb" in
            exit)
                echo "aida-worker: exit directive — stopping (rc=0)"
                return 0
                ;;
            pause)
                echo "aida-worker: pause directive — sleeping ${sleep_short}s (edit $cmd_file to resume)"
                sleep "$sleep_short"
                continue
                ;;
            drain)
                if [ -n "$rest" ]; then
                    echo "aida-worker: drain $rest"
                else
                    echo "aida-worker: drain (head pickup)"
                fi
                # Build argv. Word-splitting `$rest` is the point — directives
                # like `drain batch:x --zen` forward each word as a separate
                # arg to `aida queue work`. Note: NO `command` prefix here —
                # `timeout` invokes its child via execvp(), bypassing shell
                # function lookup entirely, and `command` is a bash builtin
                # that execvp() cannot resolve (it would fail with rc 127).
                #
                # `--kill-after=5s` is defense in depth: if the child ignores
                # the initial SIGTERM, a SIGKILL follows 5s later.
                # (Verified: GNU `timeout` reaches normal forking children
                # and exec-chains via its process-group SIGTERM. A child that
                # `setsid()`s out of the group survives — that is the residual
                # the heartbeat followup addresses.)
                # shellcheck disable=SC2086
                output=$(timeout --kill-after=5s "${AIDA_WORKER_SPEC_TIMEOUT:-1800}" aida queue work $rest --auto-complete 2>&1)
                rc=$?
                printf '%s\n' "$output"
                case "$rc" in
                    0)
                        echo "aida-worker: drain succeeded"
                        # Pop the consumed line on a scoped drain (one
                        # whose directive was a literal line in the file).
                        # A bare/absent-file `drain` has nothing to pop.
                        if [ -n "$raw" ] && [ -f "$cmd_file" ]; then
                            _aida_worker_pop_head "$cmd_file" "$raw"
                        fi
                        ;;
                    124)
                        echo "aida-worker: TIMED OUT after ${AIDA_WORKER_SPEC_TIMEOUT:-1800}s; auto-pausing"
                        printf 'pause\n' > "$cmd_file"
                        ;;
                    *)
                        if printf '%s' "$output" | grep -q 'nothing to drive'; then
                            echo "aida-worker: queue empty — sleeping ${sleep_short}s"
                            sleep "$sleep_short"
                        else
                            echo "aida-worker: halted (exit $rc); auto-pausing"
                            printf 'pause\n' > "$cmd_file"
                        fi
                        ;;
                esac
                ;;
            *)
                echo "aida-worker: unknown directive '$verb' — treating as pause (sleeping ${sleep_short}s)"
                sleep "$sleep_short"
                ;;
        esac
    done
}

# Pop the first non-blank, non-comment line whose text matches $2 from $1.
# Re-reads the file fresh so directives the user *appended* during a multi-
# minute drain survive (overwrites mid-drain are documented as racey).
_aida_worker_pop_head() {
    local file="$1"
    local target="$2"
    local tmp
    tmp=$(mktemp "${file}.XXXXXX") || return 0
    # Trim both sides of `target` and of each candidate line so an indented
    # directive matches its own trimmed form — head extraction also trims,
    # but trimming `target` here keeps the pop defensive against any caller
    # passing a raw line. trace:TASK-364 | ai:claude
    awk -v target="$target" '
        BEGIN {
            sub(/^[[:space:]]+/, "", target)
            sub(/[[:space:]]+$/, "", target)
            popped = 0
        }
        {
            if (!popped) {
                line = $0
                t = line
                sub(/^[[:space:]]+/, "", t)
                sub(/[[:space:]]+$/, "", t)
                if (length(t) == 0 || t ~ /^#/) {
                    print line
                    next
                }
                if (t == target) {
                    popped = 1
                    next
                }
            }
            print
        }
    ' "$file" > "$tmp" 2>/dev/null && mv "$tmp" "$file" || rm -f "$tmp"
}
"#;

const HELPERS_BEGIN_MARKER: &str = "# >>> aida shell helpers >>>";
const HELPERS_END_MARKER: &str = "# <<< aida shell helpers <<<";

/// Old marker pair from before the helpers were split out into a separate
/// file. Detected during --install so we can migrate the user's rc cleanly.
const LEGACY_BEGIN_MARKER: &str = "# >>> aida dev workflow helpers >>>";
const LEGACY_END_MARKER: &str = "# <<< aida dev workflow helpers <<<";
const INTERIM_STALENESS_BEGIN_MARKER: &str = "# >>> aida-staleness-marker >>>";
const INTERIM_STALENESS_END_MARKER: &str = "# <<< aida-staleness-marker <<<";

fn handle_dev_shell_init(install: bool) -> Result<()> {
    // If we're inside the aida repo, capture its absolute path so we can
    // bake an `export AIDA_DEV_REPO=...` line into the helpers file. That
    // lets `aida dev activate` find the in-repo build from any directory
    // (e.g. while working in ~/ai/paradox), not only from inside or under
    // the aida checkout.
    let repo = std::env::current_dir()
        .ok()
        .and_then(|cwd| find_aida_repo_above(&cwd));
    let env_export = match &repo {
        Some(r) => format!("export AIDA_DEV_REPO='{}'\n\n", r.display()),
        None => String::new(),
    };
    let helpers_body = format!(
        "{}{}{}{}",
        "# AIDA shell helpers — generated by `aida dev shell-init --install`.\n\
         # Re-run that command to regenerate this file (e.g. after upgrading aida).\n\n",
        env_export,
        SHELL_HELPERS,
        WORKER_FUNCTION
    );

    if !install {
        // Preview mode — show what would land in the helpers file, marker-wrapped
        // so the user can also paste it directly into a shell rc if they prefer.
        print!(
            "{}\n{}{}\n",
            HELPERS_BEGIN_MARKER, helpers_body, HELPERS_END_MARKER
        );
        return Ok(());
    }

    let shell = std::env::var("SHELL").unwrap_or_default();
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let rc_path = if shell.ends_with("/zsh") || shell.ends_with("zsh") {
        home.join(".zshrc")
    } else {
        home.join(".bashrc")
    };

    // The helpers file lives at ~/.aida/shell-init.sh — the rc only gets a
    // one-line `[ -f ... ] && source ...` stub. Lets us update helpers on
    // every `--install` without growing the rc.
    let helpers_path = home.join(".aida").join("shell-init.sh");
    if let Some(parent) = helpers_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&helpers_path, &helpers_body)
        .with_context(|| format!("Failed to write {}", helpers_path.display()))?;

    // Now ensure the rc has the source-stub block. Three input states:
    //   (a) rc already has the new marker pair → replace stub in place
    //   (b) rc has the old (pre-split) marker pair → migrate: drop old fat
    //       block, append the new stub
    //   (c) rc has no aida block → append the new stub
    let stub_body = format!(
        "# Auto-generated by `aida dev shell-init --install`. The actual helpers\n\
         # live at the path below; re-run that command to update them.\n\
         [ -f \"{path}\" ] && source \"{path}\"\n",
        path = helpers_path.display()
    );
    let new_block = format!(
        "{}\n{}{}\n",
        HELPERS_BEGIN_MARKER, stub_body, HELPERS_END_MARKER
    );

    let existing = std::fs::read_to_string(&rc_path).unwrap_or_default();
    let mut migration_note: Option<String> = None;

    let mut new_content = if let Some(start) = existing.find(HELPERS_BEGIN_MARKER) {
        // (a) Replace existing stub.
        let end_after = existing[start..]
            .find(HELPERS_END_MARKER)
            .map(|e| start + e + HELPERS_END_MARKER.len())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "found begin marker but no end marker in {} — please clean up manually",
                    rc_path.display()
                )
            })?;
        let end_after = if existing.as_bytes().get(end_after) == Some(&b'\n') {
            end_after + 1
        } else {
            end_after
        };
        let mut s = existing[..start].to_string();
        s.push_str(&new_block);
        s.push_str(&existing[end_after..]);
        s
    } else if let Some(start) = existing.find(LEGACY_BEGIN_MARKER) {
        // (b) Migrate from the previous fat block (helpers inlined into rc).
        let end_after = existing[start..]
            .find(LEGACY_END_MARKER)
            .map(|e| start + e + LEGACY_END_MARKER.len())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "found legacy begin marker but no legacy end marker in {} — please clean up manually",
                    rc_path.display()
                )
            })?;
        let removed_lines = existing[start..end_after].lines().count();
        let end_after = if existing.as_bytes().get(end_after) == Some(&b'\n') {
            end_after + 1
        } else {
            end_after
        };
        let mut s = existing[..start].to_string();
        s.push_str(&new_block);
        s.push_str(&existing[end_after..]);
        migration_note = Some(format!(
            "  (migrated: {} lines of inline helpers replaced with a 3-line source stub)",
            removed_lines
        ));
        s
    } else if existing.contains("# AIDA dev workflow helpers") {
        // Markerless legacy form (very early — pre-marker). Same drop-by-line
        // approach as before, then append the new stub.
        let mut kept: Vec<&str> = Vec::new();
        let mut skipping = false;
        let mut skipped = 0usize;
        for line in existing.lines() {
            if line.starts_with("# AIDA dev workflow helpers") {
                skipping = true;
                skipped += 1;
                continue;
            }
            if skipping {
                if line.starts_with("aida-on") || line.starts_with("aida-off") {
                    skipped += 1;
                    continue;
                }
                skipping = false;
            }
            kept.push(line);
        }
        migration_note = Some(format!(
            "  (migrated: removed {} lines of pre-marker legacy helpers)",
            skipped
        ));
        let mut s = kept.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
        s.push_str(&new_block);
        s
    } else {
        // (c) Fresh install.
        let mut s = existing;
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
        s.push_str(&new_block);
        s
    };

    if let Some(start) = new_content.find(INTERIM_STALENESS_BEGIN_MARKER) {
        let end_after = new_content[start..]
            .find(INTERIM_STALENESS_END_MARKER)
            .map(|e| start + e + INTERIM_STALENESS_END_MARKER.len())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "found interim staleness begin marker but no end marker in {} — please clean up manually",
                    rc_path.display()
                )
            })?;
        let end_after = if new_content.as_bytes().get(end_after) == Some(&b'\n') {
            end_after + 1
        } else {
            end_after
        };
        let removed_lines = new_content[start..end_after].lines().count();
        let mut s = new_content[..start].to_string();
        s.push_str(&new_content[end_after..]);
        new_content = s;
        migration_note = Some(format!(
            "  (migrated: removed {} lines of interim aida-staleness-marker hook)",
            removed_lines
        ));
    }

    std::fs::write(&rc_path, new_content)
        .with_context(|| format!("Failed to write {}", rc_path.display()))?;

    if let Some(note) = migration_note {
        eprintln!("{}", note);
    }
    eprintln!("{}: helpers installed.", "OK".green());
    eprintln!(
        "  Helpers file: {} ({} lines)",
        helpers_path.display(),
        helpers_body.lines().count()
    );
    eprintln!(
        "  Source stub:  {} (3 lines, sourced on shell startup)",
        rc_path.display()
    );
    match &repo {
        Some(r) => eprintln!(
            "  AIDA_DEV_REPO={} (baked into the helpers file)",
            r.display()
        ),
        None => {
            eprintln!(
                "  {}: not run from inside the aida repo, so AIDA_DEV_REPO was NOT set.",
                "Note".yellow()
            );
            eprintln!(
                "         `aida dev activate` will only find the dev binary when you cd into the repo."
            );
            eprintln!(
                "         To make it work everywhere, re-run from the aida repo or add manually:"
            );
            eprintln!("           export AIDA_DEV_REPO=/path/to/aida");
        }
    }
    eprintln!("  Reload: source {}", rc_path.display());
    eprintln!(
        "  Then any of: {}, {}, {}",
        "aida dev activate".cyan(),
        "aida role list".cyan(),
        "aida role enter <name>".cyan()
    );
    eprintln!("  All eval-required commands now Just Work — the wrapper handles the eval.");
    Ok(())
}

fn handle_dev_serve(
    rest_port: Option<u16>,
    grpc_port: Option<u16>,
    web_port: Option<u16>,
    no_web: bool,
) -> Result<()> {
    use std::process::Stdio;
    use tokio::process::Command as TokioCommand;
    use tokio::sync::mpsc;

    let cwd = std::env::current_dir()?;
    let repo_for_web = if !no_web && cwd.join("aida-web-react").is_dir() {
        Some(cwd.clone())
    } else {
        None
    };

    // Locate the aida-server binary: prefer the in-repo build (since dev
    // workflow), fall back to PATH.
    let server_bin = locate_aida_server_binary(&cwd)?;

    let rest = rest_port.unwrap_or(8080);
    let grpc = grpc_port.unwrap_or(50051);
    let web = web_port.unwrap_or(5173);

    let store_path = detect_distributed_store().unwrap_or_else(|| cwd.clone());

    println!("{}", "─── aida dev serve ───".bold());
    println!("  REST/HTTP:  http://localhost:{}", rest);
    println!("  gRPC:       localhost:{}", grpc);
    if repo_for_web.is_some() {
        println!("  React dev:  http://localhost:{}", web);
    } else if no_web {
        println!("  React dev:  skipped (--no-web)");
    } else {
        println!("  React dev:  skipped (no aida-web-react/ in cwd)");
    }
    println!("  Store:      {}", store_path.display());
    println!("  Press Ctrl+C to stop");
    println!();

    // Run inside a tokio runtime so we can supervise children + signals.
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();

        // Start aida-server.
        let mut server_child = TokioCommand::new(&server_bin)
            .args([
                "--host",
                "0.0.0.0",
                "--port",
                &grpc.to_string(),
                "--rest-port",
                &rest.to_string(),
                "--database",
            ])
            .arg(&store_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn aida-server at {}", server_bin.display()))?;

        spawn_log_pump("server", server_child.stdout.take(), tx.clone());
        spawn_log_pump("server", server_child.stderr.take(), tx.clone());

        // Optionally start vite dev server.
        let mut web_child = if let Some(repo) = repo_for_web {
            let cwd = repo.join("aida-web-react");
            if !cwd.join("node_modules").is_dir() {
                eprintln!(
                    "[web] note: aida-web-react/node_modules not found — run `npm install` first."
                );
            }
            let child = TokioCommand::new("npm")
                .args(["run", "dev", "--", "--port", &web.to_string()])
                .current_dir(&cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("Failed to spawn `npm run dev` for aida-web-react")?;
            Some(child)
        } else {
            None
        };

        if let Some(ref mut w) = web_child {
            spawn_log_pump("web", w.stdout.take(), tx.clone());
            spawn_log_pump("web", w.stderr.take(), tx.clone());
        }

        // Helper future: wait for a child to exit naturally.
        async fn wait_child(
            child: &mut tokio::process::Child,
        ) -> std::io::Result<std::process::ExitStatus> {
            child.wait().await
        }

        // Race Ctrl+C against either child exiting.
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\n[dev serve] Ctrl+C — stopping children...");
            }
            r = wait_child(&mut server_child) => {
                eprintln!("\n[dev serve] aida-server exited unexpectedly: {:?}", r);
            }
            r = async {
                if let Some(ref mut w) = web_child {
                    wait_child(w).await
                } else {
                    std::future::pending().await
                }
            } => {
                eprintln!("\n[dev serve] vite dev server exited unexpectedly: {:?}", r);
            }
        }

        // Send SIGTERM (kill_on_drop fires SIGKILL on drop, but we want
        // a chance for clean shutdown first).
        let _ = server_child.start_kill();
        if let Some(ref mut w) = web_child {
            let _ = w.start_kill();
        }
        let _ = server_child.wait().await;
        if let Some(mut w) = web_child {
            let _ = w.wait().await;
        }

        // Drop the sender so log-pump tasks can exit cleanly.
        drop(tx);
        while rx.recv().await.is_some() {}

        Ok::<_, anyhow::Error>(())
    })?;

    eprintln!("[dev serve] stopped.");
    Ok(())
}
