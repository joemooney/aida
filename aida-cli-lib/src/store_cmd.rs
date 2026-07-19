//! `aida store` command cluster (EPIC-21) — code↔store commit pairing and
//! orphan-store maintenance. `handle_store_command` dispatches the `status`
//! pairing report (`store_status`), the prepare-commit-msg hook installer
//! (`store_install_hook`), the `compact`/`gc`/`--squash` repack path
//! (`store_compact`, `store_compact_squash`), and the identity-redaction
//! preview (`store_scrub_preview`).
//!
//! Extracted verbatim from `main.rs` (SPIKE-78; pure movement, no behavior
//! change). The drift-comparison helpers that `aida status` also drives —
//! `short_sha`, `store_sha_is_ancestor`, `store_drift_verdict`,
//! `paired_store_sha_for_head`, `current_store_head_sha`, and
//! `print_status_store_drift_section` — stay in `main.rs` (STORY-707); this
//! module reaches them via `crate::`.

use anyhow::Result;
use colored::Colorize;

use crate::*;

pub(crate) fn handle_store_command(cmd: &cli::StoreCommand) -> Result<()> {
    match cmd {
        cli::StoreCommand::Status => store_status(),
        cli::StoreCommand::InstallHook { force } => store_install_hook(*force),
        cli::StoreCommand::Compact { squash, yes } => store_compact(*squash, *yes),
        cli::StoreCommand::ScrubPreview => store_scrub_preview(),
    }
}

/// `aida store compact` (alias `aida store gc`). The bare command runs a deep,
/// non-destructive `git gc --aggressive` on the orphan store. `--squash` is the
/// DESTRUCTIVE history-rewrite path — gated behind `--yes`, prints the plan
/// otherwise, and never runs automatically.
// trace:STORY-733 | ai:claude
/// TASK-1122 (part 3): print the identity this machine will WRITE to the shared
/// store and how the configured redaction rewrites it — so a user can verify,
/// before their first push, that no raw corporate identity will land in a
/// public-mirrored store. Read-only.
// trace:TASK-1122 | ai:claude
fn store_scrub_preview() -> Result<()> {
    let project_root = find_project_root()?;
    let raw_host = hostname();
    let raw_email = git_config_value(&project_root, "user.email");
    let (pub_host, pub_email) = aida_core::git_ops::public_identity();
    let (out_host, out_email) =
        aida_core::git_ops::redacted_identity(&raw_host, raw_email.as_deref());

    println!("Store identity preview — what this machine writes to the shared store:");
    println!();
    if pub_host.is_some() || pub_email.is_some() {
        println!(
            "  {} redaction ACTIVE  ([node] public_* in ~/.aida/config.toml)",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        println!(
            "  {} redaction off — the raw system identity is written verbatim.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        println!(
            "     Set [node] public_email / public_hostname in ~/.aida/config.toml to redact."
        );
    }
    println!();
    let arrow = |raw: &str, out: &str| {
        if raw == out {
            format!("{raw}  (unchanged)")
        } else {
            format!("{raw}  →  {out}")
        }
    };
    println!("  hostname:  {}", arrow(&raw_host, &out_host));
    match (raw_email.as_deref(), out_email.as_deref()) {
        (Some(r), Some(o)) => println!("  email:     {}", arrow(r, o)),
        (Some(r), None) => println!("  email:     {r}  (unchanged)"),
        (None, _) => println!("  email:     (git user.email unset)"),
    }
    Ok(())
}

fn store_compact(squash: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    if !store_path.exists() {
        anyhow::bail!(
            "no .aida-store/ worktree found at {} — run an `aida` store command first to attach it",
            store_path.display()
        );
    }

    if squash {
        return store_compact_squash(&store_path, yes);
    }

    // ── Safe path: aggressive repack ──
    let before = aida_core::git_ops::count_objects(&store_path);
    if let Some(b) = before {
        println!(
            "{} {} packs, {} loose objects before compaction",
            "Store:".bold(),
            b.packs.to_string().cyan(),
            b.loose.to_string().cyan()
        );
    }
    println!("Running an aggressive repack (deep gc — this can take a moment)...");
    let res = aida_core::git_ops::gc_aggressive(&store_path)?;
    if !res.success {
        anyhow::bail!("git gc --aggressive failed: {}", res.stderr);
    }
    let after = aida_core::git_ops::count_objects(&store_path);
    match (before, after) {
        (Some(b), Some(a)) => println!(
            "{} {} -> {} packs, {} -> {} loose objects",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            b.packs,
            a.packs,
            b.loose,
            a.loose
        ),
        _ => println!(
            "{} aggressive repack complete",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
    }
    println!(
        "{}",
        "History is unchanged — `aida history` and the orphan log are intact.".dimmed()
    );
    Ok(())
}

/// The DESTRUCTIVE `--squash` path. Without `--yes` it prints exactly what it
/// would do and exits without touching the store. With `--yes` it records a
/// backup ref BEFORE any rewrite, collapses the orphan history to a single
/// snapshot commit, and prints (never runs) the coordinated force-push.
// trace:STORY-733 | ai:claude
fn store_compact_squash(store_path: &std::path::Path, yes: bool) -> Result<()> {
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let commit_count = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "?".to_string());
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_ref = format!("aida-store-pre-squash-{ts}");

    println!(
        "{}  {}",
        crate::glyph(crate::glyphs::Glyph::Warning).red().bold(),
        "DESTRUCTIVE: aida store compact --squash".red().bold()
    );
    println!();
    let bullet = crate::glyph(crate::glyphs::Glyph::Bullet);
    println!("This will REWRITE the orphan `aida-store` branch:");
    println!(
        "  {bullet} collapse {} commits into a single snapshot commit",
        commit_count.yellow()
    );
    println!(
        "  {bullet} record a backup branch {} pointing at the current tip {}",
        backup_ref.cyan(),
        head.as_deref()
            .map(short_sha)
            .unwrap_or_else(|| "(unknown)".to_string())
            .cyan()
    );
    println!("  {bullet} leave every YAML / the cache byte-identical (only history collapses)");
    println!();
    println!("{}", "Consequences:".bold());
    println!(
        "  {bullet} {} loses its full timeline — the pre-squash horizon survives ONLY",
        "aida history --events".yellow()
    );
    println!("    at the backup branch; point history tooling there to inspect old events.");
    println!(
        "  {bullet} a coordinated {} is required; every other clone must re-sync",
        "force-push".yellow()
    );
    println!("    (`git fetch` + hard reset of their `.aida-store/`).");
    println!("  {bullet} this command NEVER pushes — you run the force-push after review.");
    println!();

    if !yes {
        println!(
            "{} no changes made. Re-run with {} to perform the rewrite.",
            "Plan only —".dimmed(),
            "--yes".cyan()
        );
        return Ok(());
    }

    let message = format!(
        "snapshot: compacted aida-store @ {} (squashed {} commits; backup: {})",
        chrono::Local::now().format("%Y-%m-%d"),
        commit_count,
        backup_ref
    );
    let outcome = aida_core::git_ops::squash_orphan_to_snapshot(store_path, &message, &backup_ref)?;

    // Reclaim the now-unreferenced object space (the old commits are still held
    // by the backup ref, so this won't drop them until that ref is deleted).
    let _ = aida_core::git_ops::gc_aggressive(store_path);

    println!(
        "{} squashed to snapshot {}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        short_sha(&outcome.snapshot_sha).cyan()
    );
    println!("  backup branch: {}", outcome.backup_ref.cyan());
    println!("  pre-squash tip: {}", short_sha(&outcome.old_head).cyan());
    println!();
    println!("{}", "Next (manual — review first):".bold());
    println!(
        "  {}",
        "git -C .aida-store push --force-with-lease origin aida-store".cyan()
    );
    println!(
        "  {}",
        "Then every other clone must re-sync their .aida-store/ (fetch + hard reset).".dimmed()
    );
    println!(
        "  {}",
        format!(
            "Recovery if needed: git -C .aida-store reset --hard {}",
            outcome.backup_ref
        )
        .dimmed()
    );
    Ok(())
}

/// Print the alignment between this commit's paired store SHA (from the
/// `Aida-Store:` trailer) and the current orphan store HEAD. Reports
/// "aligned" when they match, "drift" with commit count otherwise.
// trace:EPIC-21 | ai:claude
fn store_status() -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");

    // Current code HEAD.
    let code_head = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    let code_head = match code_head {
        Some(s) if !s.is_empty() => s,
        _ => anyhow::bail!("not in a git repo or no commits yet"),
    };

    // Paired store SHA — read the Aida-Store trailer from HEAD's message.
    let head_msg = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["log", "-1", "--format=%B"])
        .output()?;
    let head_msg = String::from_utf8_lossy(&head_msg.stdout).to_string();
    let trailers = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["interpret-trailers", "--parse"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    {
        use std::io::Write;
        if let Some(mut stdin) = trailers.stdin.as_ref() {
            let _ = stdin.write_all(head_msg.as_bytes());
        }
    }
    let trailer_output = trailers.wait_with_output()?;
    let trailer_text = String::from_utf8_lossy(&trailer_output.stdout).to_string();
    let paired_store_sha: Option<String> = trailer_text
        .lines()
        .find_map(|l| l.strip_prefix("Aida-Store:").map(|s| s.trim().to_string()));

    // Current orphan-store HEAD.
    let store_head: Option<String> = if store_path.exists() {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&store_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
    } else {
        None
    };

    println!("{}", "Code ↔ store pairing".bold());
    println!("  code HEAD:        {}", short_sha(&code_head).cyan());
    match &paired_store_sha {
        Some(p) => println!("  paired store SHA: {}", short_sha(p).cyan()),
        None => {
            println!(
                "  paired store SHA: {} {}",
                "(none)".dimmed(),
                "— commit was made before the prepare-commit-msg hook was installed".dimmed()
            );
        }
    }
    match &store_head {
        Some(s) => println!("  current store:    {}", short_sha(s).cyan()),
        None => println!(
            "  current store:    {} (no .aida-store/)",
            "(missing)".yellow()
        ),
    }
    println!();

    match (paired_store_sha.as_deref(), store_head.as_deref()) {
        (Some(p), Some(c)) if p == c => {
            println!(
                "{} aligned — code commit was paired with the current store HEAD.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        (Some(p), Some(c)) => {
            // Compute the commit counts: how many commits is store HEAD
            // ahead of / behind the paired SHA.
            let drift = std::process::Command::new("git")
                .arg("-C")
                .arg(&store_path)
                .args([
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{}...{}", p, c),
                ])
                .output()
                .ok();
            let (behind, ahead) = match drift {
                Some(o) if o.status.success() => {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    let parts: Vec<&str> = s.split_whitespace().collect();
                    let b: i32 = parts.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                    let a: i32 = parts.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
                    (b, a)
                }
                _ => (-1, -1),
            };
            // The store legitimately advances ahead of the pin on every
            // normal spec write — that is the healthy state, not drift. Only
            // a genuine divergence (paired SHA not an ancestor of the current
            // store HEAD) is worth flagging. trace:BUG-584
            let is_ancestor = store_sha_is_ancestor(&store_path, p, c);
            if behind <= 0 && is_ancestor {
                // Fast-forward: store simply moved forward N commits.
                if ahead > 0 {
                    println!(
                        "{} store ahead — {} normal spec-write commit(s) since this code commit was paired (healthy: the store moves forward as you file specs).",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        ahead
                    );
                } else {
                    // Ancestor with zero ahead-count shouldn't happen (that's
                    // the aligned arm), but stay quiet-and-clean if it does.
                    println!(
                        "{} store ahead — code commit was paired with an ancestor of the current store HEAD (healthy).",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                    );
                }
            } else if behind < 0 {
                println!(
                    "{} drift — store diverged since this commit was made (could not count commits, paired SHA may not exist locally).",
                    "·".yellow()
                );
                println!(
                    "  v2 will offer: {}",
                    "aida store checkout HEAD   # rewind .aida-store/ to the paired SHA".dimmed()
                );
            } else {
                println!(
                    "{} drift — store diverged from the paired SHA (possible store-side history rewrite).",
                    "·".yellow()
                );
                println!(
                    "  {} commit(s) on the paired SHA are not on the current store HEAD; {} commit(s) on the current store HEAD are not on the paired SHA.",
                    behind, ahead
                );
                println!(
                    "  v2 will offer: {}",
                    "aida store checkout HEAD   # rewind .aida-store/ to the paired SHA".dimmed()
                );
            }
        }
        (None, _) => {
            println!(
                "{} no Aida-Store trailer on this commit — install the hook with: {}",
                "·".dimmed(),
                "aida store install-hook".cyan()
            );
        }
        (Some(_), None) => {
            println!(
                "{} commit has a paired SHA but no .aida-store/ in the worktree to compare against.",
                "·".yellow()
            );
        }
    }
    Ok(())
}

/// Install the prepare-commit-msg hook from EMBEDDED_TEMPLATES into
/// `.git/hooks/prepare-commit-msg`. Idempotent.
// trace:EPIC-21 | ai:claude
fn store_install_hook(force: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let hooks_dir = project_root.join(".git").join("hooks");
    if !hooks_dir.exists() {
        anyhow::bail!(
            "{} doesn't exist — is this a git repo?",
            hooks_dir.display()
        );
    }
    let target = hooks_dir.join("prepare-commit-msg");
    if target.exists() && !force {
        anyhow::bail!(
            "{} already exists — pass --force to overwrite, or inspect with `cat {}`",
            target.display(),
            target.display()
        );
    }

    let body = aida_core::templates::EMBEDDED_TEMPLATES
        .get("hooks/aida-store-pair.sh")
        .copied()
        .ok_or_else(|| anyhow::anyhow!("aida-store-pair.sh missing from embedded templates"))?;

    std::fs::write(&target, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&target)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&target, perms)?;
    }

    println!(
        "{} installed prepare-commit-msg hook at {}",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        target.display().to_string().cyan()
    );
    println!();
    println!("Every future code commit will get an `Aida-Store: <sha>` trailer pinning the");
    println!(
        "orphan-store HEAD at commit time. Inspect alignment with: {}",
        "aida store status".cyan()
    );
    Ok(())
}
