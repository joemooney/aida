//! `aida upgrade` self-update command cluster — check the latest GitHub
//! release, then download + install (or `cargo install`) the new binary over
//! the running one, a `--target` path, or stale sibling installs found from a
//! developer build.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement; no behavior
//! change). Shared helpers stay in `main.rs` and are reached via `crate::`:
//! the `InstallMethod` enum + `detect_install_method` / `current_version` /
//! `build_banner`, the `release_target` / `strip_v` / `confirm` /
//! `find_aida_repo_above` helpers (also used by `aida release` / `aida dev`),
//! and the `git_describe_latest_tag` / `git_commits_since_tag` repo probes.

use crate::*;
use anyhow::{Context, Result};
use colored::Colorize;

/// Query GitHub for the latest release tag. Uses curl; no extra dep needed.
fn fetch_latest_release_tag() -> Result<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            "-H",
            "Accept: application/vnd.github+json",
            "https://api.github.com/repos/joemooney/aida/releases/latest",
        ])
        .output()
        .context("Failed to invoke curl — is it installed?")?;
    if !out.status.success() {
        anyhow::bail!("curl failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let body = String::from_utf8(out.stdout).context("GitHub API response not UTF-8")?;
    // Tiny parser — avoids a serde_json dep here and keeps the code simple.
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"tag_name\":") {
            let v = rest.trim().trim_start_matches('"');
            if let Some(end) = v.find('"') {
                return Ok(v[..end].to_string());
            }
        }
    }
    anyhow::bail!("Could not parse latest release tag from GitHub response");
}

pub(crate) fn handle_upgrade_command(
    check: bool,
    version: Option<&str>,
    yes: bool,
    target: Option<&str>,
    diff: bool,
) -> Result<()> {
    // --target path: upgrade a specific binary, regardless of what's running.
    if let Some(target) = target {
        return upgrade_specific_binary(std::path::Path::new(target), check, version, yes);
    }

    let install = detect_install_method()?;

    // Developer build: don't try to upgrade ourselves; instead scan for
    // sibling installs and offer to upgrade them.
    if let InstallMethod::DeveloperBuild(_) = &install {
        return upgrade_dev_mode_sibling_scan(check, version, yes, diff);
    }

    upgrade_running_binary(install, check, version, yes)
}

/// The original "upgrade the binary I'm running" flow. Used for cargo and
/// pre-built binary installs.
fn upgrade_running_binary(
    install: InstallMethod,
    check: bool,
    version: Option<&str>,
    yes: bool,
) -> Result<()> {
    let current = current_version();
    let install_label = match &install {
        InstallMethod::Cargo(p) => format!("cargo install ({})", p.display()),
        InstallMethod::Binary(p) => format!("pre-built binary ({})", p.display()),
        InstallMethod::DeveloperBuild(p) => format!("developer build ({})", p.display()),
    };
    println!("Current version: {}", build_banner());
    println!("Installed via:   {}", install_label);
    let _ = current; // version is included in build_banner()

    let target_tag = resolve_target_tag(version)?;
    let target_version = strip_v(&target_tag);
    println!("Target version:  {}", target_tag);

    if target_version == current {
        println!("\n{}: already on {}.", "OK".green(), target_tag);
        return Ok(());
    }

    if check {
        println!(
            "\n{}: an upgrade is available. Run `aida upgrade` (without --check) to install.",
            "INFO".blue()
        );
        return Ok(());
    }

    if !yes
        && !confirm(&format!(
            "\nUpgrade from v{} to {}? [y/N]: ",
            current, target_tag
        ))
    {
        println!("Cancelled.");
        return Ok(());
    }

    match install {
        InstallMethod::Cargo(_) => upgrade_via_cargo(&target_tag),
        InstallMethod::Binary(p) => upgrade_via_release_tarball(&p, &target_tag),
        InstallMethod::DeveloperBuild(_) => unreachable!(),
    }
}

/// `--target PATH` flow: upgrade a specific binary path regardless of what's
/// currently running. Lets a dev-build session refresh `~/.local/bin/aida`.
fn upgrade_specific_binary(
    target_path: &std::path::Path,
    check: bool,
    version: Option<&str>,
    yes: bool,
) -> Result<()> {
    let install = classify_install_path(target_path);

    let probed = query_binary_version(target_path);
    let current_version = probed.as_ref().map(|(v, _)| v.clone());
    let current_banner = probed
        .as_ref()
        .map(|(_, b)| b.clone())
        .unwrap_or_else(|| "(not installed)".into());
    let target_tag = resolve_target_tag(version)?;

    println!("Target binary: {}", target_path.display());
    println!("Install type:  {}", install_method_label(&install));
    println!("Current:       {}", current_banner);
    println!("Target:        {}", target_tag);

    if let InstallMethod::DeveloperBuild(p) = &install {
        anyhow::bail!(
            "Refusing to upgrade a developer build at {}.\n\
             Pass --target pointing at a real install (e.g. ~/.local/bin/aida).",
            p.display()
        );
    }

    if current_version.as_deref() == Some(strip_v(&target_tag)) {
        println!(
            "\n{}: {} already on {}.",
            "OK".green(),
            target_path.display(),
            target_tag
        );
        return Ok(());
    }

    if check {
        println!(
            "\n{}: upgrade available for {}. Re-run without --check to install.",
            "INFO".blue(),
            target_path.display()
        );
        return Ok(());
    }

    if !yes
        && !confirm(&format!(
            "\nUpgrade {} to {}? [y/N]: ",
            target_path.display(),
            target_tag
        ))
    {
        println!("Cancelled.");
        return Ok(());
    }

    match install {
        InstallMethod::Cargo(_) => upgrade_via_cargo(&target_tag),
        InstallMethod::Binary(p) => upgrade_via_release_tarball(&p, &target_tag),
        InstallMethod::DeveloperBuild(_) => unreachable!(),
    }
}

/// From a developer build, scan known install locations and report on
/// sibling aida installs, offering to upgrade any that are stale.
pub(crate) fn upgrade_dev_mode_sibling_scan(
    check: bool,
    version: Option<&str>,
    yes: bool,
    diff: bool,
) -> Result<()> {
    let exe = std::env::current_exe()?;
    println!("Current version: {}", build_banner());
    println!("Installed via:   developer build ({})", exe.display());
    println!("Note: developer build doesn't need upgrading. Looking for other installs...");
    println!();

    let target_tag = resolve_target_tag(version)?;
    let target_version = strip_v(&target_tag);

    let candidates = sibling_install_candidates();
    let mut found: Vec<(std::path::PathBuf, Option<(String, String)>)> = Vec::new();
    for path in candidates {
        if path.exists() && path != exe {
            let probed = query_binary_version(&path);
            found.push((path, probed));
        }
    }

    if found.is_empty() {
        println!("(no other aida installs found at common locations)");
        println!("  Searched: ~/.local/bin/, ~/.cargo/bin/, /usr/local/bin/, /opt/aida/bin/");
        return Ok(());
    }

    println!("Found:");
    let mut stale: Vec<std::path::PathBuf> = Vec::new();
    for (path, probed) in &found {
        let mtime = file_mtime_short(path);
        let (label, is_stale) = match probed {
            Some((v, banner)) if v == target_version => (
                format!("{}  · mtime {}  {}", banner, mtime, "up to date".green()),
                false,
            ),
            Some((_, banner)) => (
                format!(
                    "{}  · mtime {}  ({}, latest is {})",
                    banner,
                    mtime,
                    "stale".yellow(),
                    target_tag
                ),
                true,
            ),
            None => (
                format!("(could not detect version) · mtime {}", mtime),
                false,
            ),
        };
        println!("  {:<36}  {}", path.display(), label);
        if is_stale {
            stale.push(path.clone());
        }
    }
    println!();

    if check {
        if stale.is_empty() {
            println!(
                "{}: all sibling installs are at {}.",
                "OK".green(),
                target_tag
            );
            print_unreleased_dev_hint(&exe, &target_tag, diff);
        } else {
            println!(
                "{}: {} sibling install(s) are stale. Re-run without --check to upgrade.",
                "INFO".blue(),
                stale.len()
            );
        }
        return Ok(());
    }

    if stale.is_empty() {
        println!(
            "{}: nothing to do — all sibling installs are at {}.",
            "OK".green(),
            target_tag
        );
        print_unreleased_dev_hint(&exe, &target_tag, diff);
        return Ok(());
    }

    for path in stale {
        println!();
        if !yes
            && !confirm(&format!(
                "Upgrade {} to {}? [y/N]: ",
                path.display(),
                target_tag
            ))
        {
            println!("  skipped {}", path.display());
            continue;
        }
        let install = classify_install_path(&path);
        let result = match install {
            InstallMethod::Cargo(_) => upgrade_via_cargo(&target_tag),
            InstallMethod::Binary(_) => upgrade_via_release_tarball(&path, &target_tag),
            InstallMethod::DeveloperBuild(_) => {
                eprintln!(
                    "  {} {} is itself a developer build, skipping",
                    "warning:".yellow(),
                    path.display()
                );
                Ok(())
            }
        };
        if let Err(e) = result {
            eprintln!("  {} {}: {}", "error:".red(), path.display(), e);
        }
    }

    Ok(())
}

fn install_method_label(install: &InstallMethod) -> String {
    match install {
        InstallMethod::Cargo(_) => "cargo install".to_string(),
        InstallMethod::Binary(_) => "pre-built binary".to_string(),
        InstallMethod::DeveloperBuild(_) => "developer build".to_string(),
    }
}

fn resolve_target_tag(version: Option<&str>) -> Result<String> {
    match version {
        Some(v) => Ok(format!("v{}", v.strip_prefix('v').unwrap_or(v))),
        None => {
            print!("Querying github.com/joemooney/aida for latest release... ");
            std::io::Write::flush(&mut std::io::stdout()).ok();
            let tag = fetch_latest_release_tag()?;
            println!("{}", tag);
            Ok(tag)
        }
    }
}

/// Run `<path> --version` and parse out (version, full_banner). The banner
/// is everything after the program-name prefix and may include a build-time
/// stamp ("0.4.0 (built 2026-05-03T01:30:00Z, sha 866b050)") for binaries
/// built post-EPIC-1-001 — older binaries just have "0.4.0". Returns None
/// if the binary doesn't run or output doesn't look like a version.
fn query_binary_version(path: &std::path::Path) -> Option<(String, String)> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Strip the leading program name ("aida ", "aida-cli ", "aida-server ").
    let banner = ["aida-cli ", "aida-server ", "aida "]
        .iter()
        .find_map(|p| s.strip_prefix(*p))
        .map(String::from)
        .unwrap_or(s);
    // Pluck the first whitespace-separated token as the bare version.
    let version = banner
        .split_whitespace()
        .next()
        .filter(|v| {
            v.chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        })?
        .to_string();
    Some((version, banner))
}

/// When `aida upgrade` runs from a developer build and finds all sibling
/// installs already at the latest released tag, surface the fact that the
/// dev build itself is ahead of that tag — otherwise the user might think
/// "everything's up to date" when really there are unreleased commits
/// sitting in their repo. Pure hint; doesn't trigger any action. When
/// `with_diff` is true, also prints `git log --stat <tag>..HEAD` so the
/// user can vet what `aida dev patch` would ship.
fn print_unreleased_dev_hint(exe: &std::path::Path, target_tag: &str, with_diff: bool) {
    let repo = match find_aida_repo_above(exe) {
        Some(r) => r,
        None => return,
    };
    let latest = match git_describe_latest_tag(&repo) {
        Some(t) => t,
        None => return,
    };
    if latest != target_tag {
        // Latest tag locally doesn't match the latest published release —
        // probably a fetch lag. Don't speculate; just bail quietly.
        return;
    }
    let ahead = git_commits_since_tag(&repo, &latest).unwrap_or(0);
    if ahead == 0 {
        return;
    }
    println!();
    println!(
        "{}: this dev build is {} commit{} ahead of {}.",
        "Note".blue(),
        ahead,
        if ahead == 1 { "" } else { "s" },
        latest
    );

    if with_diff {
        println!();
        println!(
            "{}",
            format!("Unreleased commits ({}..HEAD):", latest).bold()
        );
        println!("{}", "─".repeat(72));
        let log = std::process::Command::new("git")
            .args([
                "log",
                "--stat",
                "--no-decorate",
                "--pretty=format:%C(yellow)%h%Creset %s%n  %C(dim)%an, %ar%Creset",
                &format!("{}..HEAD", latest),
            ])
            .current_dir(&repo)
            .output();
        match log {
            Ok(out) if out.status.success() => {
                print!("{}", String::from_utf8_lossy(&out.stdout));
                if !out.stdout.ends_with(b"\n") {
                    println!();
                }
            }
            _ => {
                println!("  (could not read git log)");
            }
        }
        println!();
    } else {
        println!(
            "      Pass {} to see the unreleased commits before shipping.",
            "--diff".cyan()
        );
    }

    println!("      To ship those changes AND refresh your sibling installs in one shot:");
    println!("        {}", "aida dev patch".cyan());
    println!("      (or `aida dev release {{minor|major|<version>}}` for a different bump)");
    println!();
    println!(
        "      Or do it manually: `cd {} && ./scripts/release.sh patch`,",
        repo.display()
    );
    println!("      then re-run `aida upgrade`.");
}

/// File mtime as `YYYY-MM-DD` for display next to a binary's version. Useful
/// as a universal "when was this binary placed here" indicator — works even
/// for binaries built before the build-banner stamps existed (pre-EPIC-1-001).
fn file_mtime_short(path: &std::path::Path) -> String {
    let modified = match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(m) => m,
        Err(_) => return "(?)".to_string(),
    };
    let secs = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return "(?)".to_string(),
    };
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "(?)".to_string())
}

fn sibling_install_candidates() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local/bin/aida"));
        paths.push(home.join(".cargo/bin/aida"));
    }
    paths.push(std::path::PathBuf::from("/usr/local/bin/aida"));
    paths.push(std::path::PathBuf::from("/opt/aida/bin/aida"));
    paths
}

/// Like `detect_install_method` but classifies an arbitrary path rather
/// than the running binary.
fn classify_install_path(path: &std::path::Path) -> InstallMethod {
    let path_str = path.to_string_lossy();
    if path_str.contains("/target/debug/") || path_str.contains("/target/release/") {
        return InstallMethod::DeveloperBuild(path.to_path_buf());
    }
    let cargo_home = std::env::var("CARGO_HOME").ok();
    let cargo_bin = cargo_home
        .map(|h| std::path::PathBuf::from(h).join("bin"))
        .or_else(|| dirs::home_dir().map(|h| h.join(".cargo/bin")));
    if let Some(bin) = cargo_bin {
        if path.starts_with(&bin) {
            return InstallMethod::Cargo(path.to_path_buf());
        }
    }
    InstallMethod::Binary(path.to_path_buf())
}

/// Re-run `cargo install --git ...` to refresh the binary. Pins to the
/// requested tag so the install matches what the user asked for.
fn upgrade_via_cargo(tag: &str) -> Result<()> {
    println!(
        "\nRunning: cargo install --git https://github.com/joemooney/aida.git --tag {} --force aida-cli",
        tag
    );
    let status = std::process::Command::new("cargo")
        .args([
            "install",
            "--git",
            "https://github.com/joemooney/aida.git",
            "--tag",
            tag,
            "--force",
            "aida-cli",
        ])
        .status()
        .context("Failed to invoke cargo")?;
    if !status.success() {
        anyhow::bail!("cargo install failed");
    }
    println!("\n{}: upgraded to {}.", "OK".green(), tag);
    Ok(())
}

/// Download the release tarball matching this platform, extract, and install
/// over the existing binary. Uses sudo if the destination is not writable by
/// the current user.
fn upgrade_via_release_tarball(current_exe: &std::path::Path, tag: &str) -> Result<()> {
    let target = release_target()
        .ok_or_else(|| anyhow::anyhow!("Unsupported platform — no release artifact available. Use `cargo install --git` instead."))?;
    let url = format!(
        "https://github.com/joemooney/aida/releases/download/{}/aida-{}.tar.gz",
        tag, target
    );

    let temp_dir = std::env::temp_dir().join(format!("aida-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;

    println!("\nDownloading {} ...", url);
    let status = std::process::Command::new("curl")
        .args(["-fSL", "-o"])
        .arg(temp_dir.join("aida.tar.gz"))
        .arg(&url)
        .status()
        .context("Failed to invoke curl")?;
    if !status.success() {
        anyhow::bail!(
            "Download failed. Verify {} exists and you have network access.",
            url
        );
    }

    println!("Extracting...");
    let status = std::process::Command::new("tar")
        .args(["xzf"])
        .arg(temp_dir.join("aida.tar.gz"))
        .arg("-C")
        .arg(&temp_dir)
        .status()
        .context("Failed to invoke tar")?;
    if !status.success() {
        anyhow::bail!("tar extraction failed");
    }

    let dest_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not determine install directory"))?;
    let needs_sudo = !dest_writable(dest_dir);

    let install_from = |src: &std::path::Path, dst_name: &str| -> Result<()> {
        let dst = dest_dir.join(dst_name);
        let mut cmd = if needs_sudo {
            let mut c = std::process::Command::new("sudo");
            c.arg("install");
            c
        } else {
            std::process::Command::new("install")
        };
        cmd.args(["-m", "755"]).arg(src).arg(&dst);
        let s = cmd
            .status()
            .with_context(|| format!("Failed to install {}", dst.display()))?;
        if !s.success() {
            anyhow::bail!("install failed for {}", dst.display());
        }
        println!("  {} {}", "Installed".green(), dst.display());
        Ok(())
    };

    // Find binaries in the extracted tarball. The release workflow has
    // shipped two layouts at different times:
    //   v0.4.0 era: a single file named `aida-${target}` (the renamed
    //               aida binary; no aida-server).
    //   future:     two files `aida` and `aida-server` at top level.
    // Handle both.
    let mut installed_any = false;

    let single = temp_dir.join(format!("aida-{}", target));
    if single.is_file() {
        install_from(&single, "aida")?;
        installed_any = true;
    }

    let aida_top = temp_dir.join("aida");
    if aida_top.is_file() {
        install_from(&aida_top, "aida")?;
        installed_any = true;
    }

    let server_top = temp_dir.join("aida-server");
    if server_top.is_file() {
        install_from(&server_top, "aida-server")?;
        installed_any = true;
    }

    if !installed_any {
        // Surface what WAS in the tarball so the user can debug, instead
        // of the previous silent "OK: upgraded" lie.
        let mut entries: Vec<String> = std::fs::read_dir(&temp_dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        entries.sort();
        anyhow::bail!(
            "Extracted tarball at {} contains no aida binary I recognize.\n\
             Expected one of: aida-{}, aida.\n\
             Tarball contents: {:?}\n\
             This is an aida bug — please report.",
            temp_dir.display(),
            target,
            entries
        );
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
    println!("\n{}: upgraded to {}.", "OK".green(), tag);
    Ok(())
}

fn dest_writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".aida-upgrade-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
