// trace:TASK-3781 | ai:codex

use super::*;

/// `aida doctor --fix-sandbox` — guided, copy-pasteable bring-up of the OS
/// sandbox (bubblewrap write-confinement) on the current host. A PRINTER, not a
/// silent sudo-runner: it detects the current state, prints the exact ordered
/// sequence (install / persist-sysctl / opt-in / verify) with sudo steps marked
/// "run this yourself", and runs the NON-sudo availability re-probe as a smoke
/// check. Honest + safe — it never escalates privileges on the user's behalf.
// trace:STORY-665 | ai:claude
pub(crate) fn doctor_fix_sandbox() -> Result<()> {
    use crate::session::{
        bwrap_availability, BwrapAvailability, BWRAP_INSTALL_DEBIAN, BWRAP_USERNS_SYSCTL_PERSIST,
        BWRAP_USERNS_SYSCTL_RUNTIME,
    };

    println!(
        "{}",
        "Guided OS-sandbox setup (bubblewrap write-confinement)"
            .bold()
            .cyan()
    );
    println!(
        "{}",
        "AIDA can run the agent it launches under an OS-level write-confinement \n\
         sandbox (bwrap). It is opt-in and off by default. This walks you through \n\
         bringing it up on THIS host. sudo steps are yours to run."
            .dimmed()
    );
    println!();

    let avail = bwrap_availability();

    // Step 1 — detected state.
    println!("{}", "1. Detected state".bold());
    match &avail {
        BwrapAvailability::Ok => println!(
            "   {} bwrap is installed and the unprivileged-userns self-test passes.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        BwrapAvailability::NotInstalled => println!(
            "   {} bwrap is not installed on this host.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
        BwrapAvailability::UsernsBlocked { .. } => println!(
            "   {} bwrap is installed, but the kernel is blocking the unprivileged \n      user namespace it needs (Ubuntu 23.10+/24.04 AppArmor default).",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
    }
    println!();

    let mut step = 2;

    // Step 2 — install (only if missing).
    if avail == BwrapAvailability::NotInstalled {
        println!("{}", format!("{step}. Install bubblewrap").bold());
        println!("   {}", "Run this yourself (sudo):".dimmed());
        println!("      {}", BWRAP_INSTALL_DEBIAN.cyan());
        println!(
            "   {}",
            "(non-Debian: use your package manager's `bubblewrap` package)".dimmed()
        );
        println!();
        step += 1;
    }

    // Step (conditional) — lift + persist the userns sysctl.
    if matches!(avail, BwrapAvailability::UsernsBlocked { .. })
        || avail == BwrapAvailability::NotInstalled
    {
        println!(
            "{}",
            format!("{step}. Permit unprivileged user namespaces").bold()
        );
        println!(
            "   {}",
            "Run these yourself (sudo). First applies it now, second persists across reboots:"
                .dimmed()
        );
        println!(
            "      {}  {}",
            BWRAP_USERNS_SYSCTL_RUNTIME.cyan(),
            "# this boot".dimmed()
        );
        println!(
            "      {}  {}",
            BWRAP_USERNS_SYSCTL_PERSIST.cyan(),
            "# persist".dimmed()
        );
        println!();
        step += 1;
    }

    // Step — opt in (the knob is off by default regardless of host state).
    println!("{}", format!("{step}. Enable the sandbox (opt-in)").bold());
    println!(
        "   {}",
        "Set the knob in .aida/config.toml (off by default):".dimmed()
    );
    println!("      {}", "[contained]".cyan());
    println!("      {}", "os_wrap = true".cyan());
    println!(
        "   {}",
        "Recommended: enable per-host with `export AIDA_OS_WRAP=1` (no shared-config change). The config knob above enables it repo-wide.".dimmed()
    );
    println!();
    step += 1;

    // Step — verify.
    println!("{}", format!("{step}. Verify").bold());
    println!(
        "   {}",
        "Re-run this command (or `aida doctor`) — step 1 should report a passing self-test:"
            .dimmed()
    );
    println!("      {}", "aida doctor --fix-sandbox".cyan());
    println!(
        "   {}",
        "Once enabled, `aida config show` renders the resolved [contained] posture.".dimmed()
    );
    println!();

    // Non-sudo smoke: re-probe availability and report the live verdict so the
    // operator sees whether the host is already there without touching sudo.
    println!("{}", "Live smoke check (non-sudo re-probe)".bold());
    match bwrap_availability() {
        BwrapAvailability::Ok => println!(
            "   {} Confinement self-test PASSES — this host is ready; just set os_wrap = true.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        BwrapAvailability::NotInstalled => println!(
            "   {} bwrap still not on PATH — run the install step above.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
        BwrapAvailability::UsernsBlocked { .. } => println!(
            "   {} Self-test still failing — run the sudo sysctl step above, then re-check.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
    }
    println!();
    println!(
        "   {}",
        "Full reference: docs/internal/agents/claude-bubblewrap-sandbox.md".dimmed()
    );

    Ok(())
}

// trace:STORY-1128 | ai:codex
pub(super) fn heal_doctor_permission_posture(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let result = crate::config_cmd::apply_permission_posture(
        project_root,
        cli::ConfigPermissionTier::Contained,
        crate::config_cmd::PermissionPostureScope::Local,
    )?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "applied contained permission posture".to_string(),
        status: "healed".to_string(),
        detail: Some(format!(
            "wrote {}; backups {}",
            result
                .edited_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            result
                .backup_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    })
}

/// Remove the sandbox-left `config.lock`. Re-resolves and re-checks the
/// lock from `project_root` rather than trusting the finding's path, so a
/// lock that became a real git lock since the scan is kept.
// trace:BUG-2211 | ai:claude
pub(crate) fn heal_doctor_sandbox_config_lock(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let removed = aida_core::git_ops::clear_sandbox_config_lock(project_root);
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed empty read-only git config lock".to_string(),
        status: if removed.is_some() {
            "healed"
        } else {
            "skipped"
        }
        .to_string(),
        detail: removed
            .is_none()
            .then(|| "lock is gone or no longer empty and read-only; left in place".to_string()),
    })
}
