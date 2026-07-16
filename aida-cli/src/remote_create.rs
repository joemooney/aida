//! Guided origin bootstrap — `aida remote create` / `aida remote attach`.
//!
//! When a project has no git `origin`, both legs of `aida push` silently skip
//! ("no origin — will skip"), leaving the operator to manually create the repo
//! on GitHub/GitLab + `git remote add origin` + push. This module smooths that:
//! a guided flow that offers GitHub (via `gh repo create`), personal-GitLab
//! push-to-create over SSH (token-free, no `glab` required), an
//! attempt-then-degrade path for corporate GitLab, and attach-existing.
//!
//! Design split (deliberate, for testability): the *pure* pieces — command
//! construction, URL building, repo-name derivation, host memory, and the
//! non-interactive manual recipe — live as free functions with unit tests.
//! The interactive prompting + process execution is the thin shell around
//! them. The non-interactive path (no TTY) prints the manual recipe and exits
//! cleanly rather than guessing.
//!
//! Honors the EPIC-35 forge abstraction for WHICH forge a remembered host maps
//! to; repo creation is "step 0" before the forge lifecycle takes over.
//!
//! trace:STORY-537 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A GitLab host AIDA remembers per machine so "your personal GitLab" is a
/// one-key choice rather than a re-typed host. The working SSH route is
/// remembered alongside (personal GitLab is SSH :2222 locally, for example).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownHost {
    /// The forge host, e.g. `gitlab.joemooney.com`.
    pub host: String,
    /// A friendly label for the menu, e.g. `personal GitLab`.
    pub label: Option<String>,
    /// Preferred SSH port for push-to-create (None → 22).
    pub ssh_port: Option<u16>,
}

// ───────────────────────────── pure helpers ─────────────────────────────

/// Derive a default repo name from the project directory's basename.
/// Falls back to `"repo"` for a root/empty path. Pure.
pub fn default_repo_name(project_root: &Path) -> String {
    project_root
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("repo")
        .to_string()
}

/// Build the `gh repo create` argv (after the `gh` binary itself) for creating
/// a repo from the current directory, wiring `origin` and pushing in one shot.
/// `ns_name` may be bare (`myrepo`) or namespaced (`owner/myrepo`). Pure so the
/// exact command shape is unit-testable without invoking `gh`.
/// trace:STORY-537 | ai:claude
pub fn gh_repo_create_args(ns_name: &str, private: bool) -> Vec<String> {
    vec![
        "repo".to_string(),
        "create".to_string(),
        ns_name.to_string(),
        if private { "--private" } else { "--public" }.to_string(),
        "--source".to_string(),
        ".".to_string(),
        "--remote".to_string(),
        "origin".to_string(),
        "--push".to_string(),
    ]
}

/// Build a GitLab SSH push-to-create origin URL. Uses the `ssh://` scheme form
/// when a non-default port is given (scp-like syntax can't carry a port),
/// otherwise the compact scp-like `git@host:ns/name.git`. Pure.
/// trace:STORY-537 | ai:claude
pub fn gitlab_ssh_origin_url(host: &str, port: Option<u16>, namespace: &str, name: &str) -> String {
    let repo = name.trim_end_matches(".git");
    let path = if namespace.is_empty() {
        format!("{repo}.git")
    } else {
        format!("{}/{repo}.git", namespace.trim_matches('/'))
    };
    match port {
        Some(p) if p != 22 => format!("ssh://git@{host}:{p}/{path}"),
        _ => format!("git@{host}:{path}"),
    }
}

/// The manual recipe AIDA prints in a non-interactive context (no TTY) when it
/// finds no origin. It can't prompt, so it hands the operator the exact steps
/// for each forge and exits cleanly (exit 0 — this is guidance, not an error).
/// Pure → snapshot-testable. trace:STORY-537 | ai:claude
pub fn manual_recipe(repo_name: &str, branch: &str) -> String {
    let mut s = String::new();
    s.push_str("No `origin` remote — create or attach one, then push:\n\n");
    s.push_str("  GitHub (gh):\n");
    s.push_str(&format!(
        "    gh repo create {repo_name} --private --source . --remote origin --push\n\n"
    ));
    s.push_str("  GitLab push-to-create (SSH, no glab/token needed):\n");
    s.push_str(&format!(
        "    git remote add origin git@<gitlab-host>:<namespace>/{repo_name}.git\n"
    ));
    s.push_str(&format!("    git push -u origin {branch}\n\n"));
    s.push_str("  Attach an existing repo:\n");
    s.push_str("    aida remote attach <url>\n\n");
    s.push_str("Then `aida push` syncs both the code and the aida-store legs.");
    s
}

/// The clear UI-step + attach instruction printed when an auto-create attempt
/// fails on a forge AIDA can't guarantee (corporate GitLab: push-to-create off,
/// API namespace perms). Never leaves the operator stuck. Pure.
/// trace:STORY-537 | ai:claude
pub fn attach_fallback_hint(host: &str, repo_name: &str) -> String {
    format!(
        "Couldn't auto-create the repo on {host} (push-to-create may be disabled, \
         or you may lack namespace permissions).\n\
         Create it in the GitLab UI:\n  \
         1. Open https://{host}/projects/new and create an empty project named \"{repo_name}\"\n  \
         2. Copy its clone URL, then run:  aida remote attach <url>\n\
         AIDA will wire origin + push both legs."
    )
}

// ───────────────────────────── host memory ─────────────────────────────

/// Path to the machine-global remote-hosts memory file (`~/.aida/remotes.toml`).
/// Returns None when the home dir can't be resolved.
pub fn known_hosts_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida").join("remotes.toml"))
}

/// Parse the `[[gitlab_host]]` array-of-tables from a `remotes.toml` body.
/// Hand-rolled (mirrors the project's other section parsers) to avoid a serde
/// round-trip for a tiny file. Pure over its `&str` input → unit-testable.
/// trace:STORY-537 | ai:claude
pub fn parse_known_hosts(toml_body: &str) -> Vec<KnownHost> {
    let mut hosts = Vec::new();
    let mut cur: Option<KnownHost> = None;
    let flush = |cur: &mut Option<KnownHost>, hosts: &mut Vec<KnownHost>| {
        if let Some(h) = cur.take() {
            if !h.host.is_empty() {
                hosts.push(h);
            }
        }
    };
    for raw in toml_body.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[gitlab_host]]" {
            flush(&mut cur, &mut hosts);
            cur = Some(KnownHost {
                host: String::new(),
                label: None,
                ssh_port: None,
            });
            continue;
        }
        if line.starts_with('[') {
            // Some other table — stop accumulating into the current host.
            flush(&mut cur, &mut hosts);
            continue;
        }
        if let (Some(entry), Some((key, val))) = (cur.as_mut(), line.split_once('=')) {
            let key = key.trim();
            let val = val.trim().trim_matches('"');
            match key {
                "host" => entry.host = val.to_string(),
                "label" => entry.label = Some(val.to_string()),
                "ssh_port" => entry.ssh_port = val.parse::<u16>().ok(),
                _ => {}
            }
        }
    }
    flush(&mut cur, &mut hosts);
    hosts
}

/// Serialize known hosts back to a `remotes.toml` body. Pure → round-trippable.
/// trace:STORY-537 | ai:claude
pub fn serialize_known_hosts(hosts: &[KnownHost]) -> String {
    let mut s = String::from(
        "# AIDA remembered forge hosts (machine-global).\n\
         # `aida remote create` offers these as one-key choices.\n",
    );
    for h in hosts {
        s.push_str("\n[[gitlab_host]]\n");
        s.push_str(&format!("host = \"{}\"\n", h.host));
        if let Some(label) = &h.label {
            s.push_str(&format!("label = \"{label}\"\n"));
        }
        if let Some(port) = h.ssh_port {
            s.push_str(&format!("ssh_port = {port}\n"));
        }
    }
    s
}

/// Read remembered GitLab hosts from `~/.aida/remotes.toml`. Empty on any error
/// (missing file / unreadable / no home dir) — host memory is a convenience, so
/// absence degrades to "ask the host".
pub fn load_known_hosts() -> Vec<KnownHost> {
    let Some(path) = known_hosts_path() else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_known_hosts(&body)
}

/// Insert-or-update a host in the remembered set (dedup by host), then persist.
/// Idempotent on a repeat bootstrap of the same host. Returns the path written.
/// trace:STORY-537 | ai:claude
pub fn remember_host(new: KnownHost) -> Result<PathBuf> {
    let path = known_hosts_path().context("cannot resolve home dir for remotes.toml")?;
    let mut hosts = load_known_hosts();
    if let Some(existing) = hosts.iter_mut().find(|h| h.host == new.host) {
        // Update label/port when the new bootstrap learned a working route.
        if new.label.is_some() {
            existing.label = new.label.clone();
        }
        if new.ssh_port.is_some() {
            existing.ssh_port = new.ssh_port;
        }
    } else {
        hosts.push(new);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, serialize_known_hosts(&hosts))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

// ───────────────────────────── git plumbing ─────────────────────────────

/// Run `git -C <root> remote add origin <url>`, then `git -C <root> push -u
/// origin <branch>`. Returns Ok(()) only when both succeed.
/// trace:STORY-537 | ai:claude
fn add_origin_and_push(project_root: &Path, url: &str, branch: &str) -> Result<()> {
    let add = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["remote", "add", "origin", url])
        .status()
        .context("invoking git remote add origin")?;
    if !add.success() {
        anyhow::bail!("`git remote add origin {url}` failed");
    }
    let push = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["push", "-u", "origin", branch])
        .status()
        .context("invoking git push -u origin")?;
    if !push.success() {
        anyhow::bail!("`git push -u origin {branch}` failed");
    }
    Ok(())
}

/// True when AIDA is interactive (both stdin and stdout are a TTY). The flow
/// only prompts when interactive; otherwise it prints the manual recipe.
fn is_interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn is_on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn current_branch(project_root: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .filter(|b| !b.is_empty() && b != "HEAD")
        .unwrap_or_else(|| "main".to_string())
}

fn has_origin(project_root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["remote", "get-url", "origin"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn origin_url(project_root: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn prompt_line(prompt: &str) -> Result<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

// ───────────────────────────── command entry points ─────────────────────

/// `aida remote attach <url>` — wire an existing repo's URL as origin and push
/// the code leg. The clean-degradation target for corporate GitLab. The store
/// leg is synced by a follow-up `aida push`; we only own the code leg + origin
/// wiring here so the module stays store-agnostic. trace:STORY-537 | ai:claude
pub fn handle_remote_attach(project_root: &Path, url: &str) -> Result<()> {
    if has_origin(project_root) {
        anyhow::bail!(
            "origin already set — remove it first with `git remote remove origin` if you mean to re-point it"
        );
    }
    let branch = current_branch(project_root);
    add_origin_and_push(project_root, url, &branch)?;
    println!("Attached origin {url} and pushed {branch}.");
    println!("Run `aida push` to sync the aida-store leg too.");
    Ok(())
}

/// `aida remote create` — the guided bootstrap. Non-interactive prints the
/// recipe and exits cleanly; interactive walks the forge menu. The
/// `--attach <url>` / `--github` / `--gitlab <host>` flags pre-select a route
/// so the flow stays scriptable. trace:STORY-537 | ai:claude
pub fn handle_remote_create(
    project_root: &Path,
    attach: Option<&str>,
    github: bool,
    gitlab_host: Option<&str>,
    private: bool,
) -> Result<()> {
    if has_origin(project_root) {
        let url = origin_url(project_root);
        println!("origin already set ({url}). Nothing to do — `aida push` will sync it.");
        return Ok(());
    }

    let repo_name = default_repo_name(project_root);
    let branch = current_branch(project_root);

    // Pre-selected route via flag — works with or without a TTY (scriptable).
    if let Some(url) = attach {
        return handle_remote_attach(project_root, url);
    }
    if github {
        return create_via_github(project_root, &repo_name, private);
    }
    if let Some(host) = gitlab_host {
        return create_via_gitlab_ssh(project_root, host, &repo_name, &branch, private);
    }

    // No pre-selected route and no TTY → print the recipe and exit 0.
    if !is_interactive() {
        println!("{}", manual_recipe(&repo_name, &branch));
        return Ok(());
    }

    interactive_menu(project_root, &repo_name, &branch, private)
}

fn create_via_github(project_root: &Path, repo_name: &str, private: bool) -> Result<()> {
    if !is_on_path("gh") {
        anyhow::bail!(
            "GitHub CLI (`gh`) is not on PATH — install it (https://cli.github.com) or use \
             `aida remote attach <url>` after creating the repo in the GitHub UI."
        );
    }
    let args = gh_repo_create_args(repo_name, private);
    // `gh repo create … --source .` operates on the repo at the current dir.
    let status = Command::new("gh")
        .args(&args)
        .current_dir(project_root)
        .status()
        .context("invoking gh repo create")?;
    if !status.success() {
        anyhow::bail!("`gh repo create` failed — check `gh auth status` and try again");
    }
    println!("Created GitHub repo and pushed code. Run `aida push` to sync the aida-store leg.");
    Ok(())
}

fn create_via_gitlab_ssh(
    project_root: &Path,
    host: &str,
    repo_name: &str,
    branch: &str,
    _private: bool,
) -> Result<()> {
    // Use a remembered SSH port for this host if we have one.
    let known = load_known_hosts();
    let port = known
        .iter()
        .find(|h| h.host == host)
        .and_then(|h| h.ssh_port);

    let namespace = if is_interactive() {
        prompt_line(&format!(
            "GitLab namespace (user/group) on {host} [press Enter for your default]: "
        ))?
    } else {
        String::new()
    };

    let url = gitlab_ssh_origin_url(host, port, &namespace, repo_name);
    println!("Push-to-create on {host} via SSH:");
    println!("  origin = {url}");
    match add_origin_and_push(project_root, &url, branch) {
        Ok(()) => {
            // Remember the host (+ port) for next time.
            let _ = remember_host(KnownHost {
                host: host.to_string(),
                label: None,
                ssh_port: port,
            });
            println!("Wired origin + pushed {branch}. Run `aida push` to sync the aida-store leg.");
            Ok(())
        }
        Err(e) => {
            // Degrade cleanly — clean up the half-wired origin, show the
            // attach-existing fallback (corporate case).
            let _ = Command::new("git")
                .arg("-C")
                .arg(project_root)
                .args(["remote", "remove", "origin"])
                .status();
            eprintln!("{}", attach_fallback_hint(host, repo_name));
            Err(e.context("push-to-create over SSH failed"))
        }
    }
}

fn interactive_menu(
    project_root: &Path,
    repo_name: &str,
    branch: &str,
    private: bool,
) -> Result<()> {
    println!("No `origin` remote. Where should I create one?\n");
    let known = load_known_hosts();
    let mut idx = 1;
    println!("  {idx}) GitHub (via gh)");
    let github_choice = idx;
    idx += 1;
    let mut host_choices: Vec<(usize, KnownHost)> = Vec::new();
    for h in &known {
        let label = h.label.clone().unwrap_or_else(|| h.host.clone());
        println!("  {idx}) {label} ({})", h.host);
        host_choices.push((idx, h.clone()));
        idx += 1;
    }
    let other_gitlab_choice = idx;
    println!("  {idx}) Other GitLab host (push-to-create over SSH)");
    idx += 1;
    let attach_choice = idx;
    println!("  {idx}) Attach an existing URL (created elsewhere)");

    let answer = prompt_line("\nChoice: ")?;
    let choice: usize = answer.parse().unwrap_or(0);

    if choice == github_choice {
        return create_via_github(project_root, repo_name, private);
    }
    if let Some((_, h)) = host_choices.iter().find(|(n, _)| *n == choice) {
        return create_via_gitlab_ssh(project_root, &h.host, repo_name, branch, private);
    }
    if choice == other_gitlab_choice {
        let host = prompt_line("GitLab host (e.g. gitlab.example.com): ")?;
        if host.is_empty() {
            anyhow::bail!("no host given");
        }
        let port_in = prompt_line("SSH port [Enter for 22]: ")?;
        if !port_in.is_empty() {
            if let Ok(p) = port_in.parse::<u16>() {
                let _ = remember_host(KnownHost {
                    host: host.clone(),
                    label: None,
                    ssh_port: Some(p),
                });
            }
        }
        return create_via_gitlab_ssh(project_root, &host, repo_name, branch, private);
    }
    if choice == attach_choice {
        let url = prompt_line("Existing repo URL: ")?;
        if url.is_empty() {
            anyhow::bail!("no URL given");
        }
        return handle_remote_attach(project_root, &url);
    }
    anyhow::bail!("no valid choice — aborting");
}

// ───────────────────────────── remote drift status ──────────────────────

/// Branches that must stay byte-identical across every hub: the code trunk and
/// the orphan requirement store. If these diverge across remotes, the shared
/// substrate has forked.
// trace:TASK-1095 | ai:claude
const DRIFT_BRANCHES: [&str; 2] = ["main", "aida-store"];

/// One remote's standing for a branch: its current head (via `ls-remote`, no
/// object transfer) and how far the local branch is ahead/behind it.
struct RemoteBranchStanding {
    remote: String,
    head: Option<String>,
    ahead_behind: Option<(u32, u32)>,
}

/// `aida remote status` — read-only drift readout across all configured
/// remotes. Best-effort refresh, then compare each remote's tip for the shared
/// branches; exit 2 when any branch's tips disagree so the command can gate a
/// hook or CI. Never writes to a remote.
// trace:TASK-1095 | ai:claude
pub fn handle_remote_status(project_root: &Path, json: bool, no_fetch: bool) -> Result<()> {
    // Compare only real remotes. Skip the `all` fan-out pseudo-remote (a single
    // name with multiple push URLs) — comparing it against its own members is
    // noise, not drift.
    let remotes: Vec<String> = aida_core::git_ops::list_remotes(project_root)
        .into_iter()
        .filter(|r| r != "all")
        .collect();

    if remotes.len() < 2 {
        let msg = format!(
            "Only {} remote(s) configured — nothing to drift against.",
            remotes.len()
        );
        if json {
            println!(
                "{}",
                serde_json::json!({ "remotes": remotes, "branches": [], "diverged": false, "note": msg })
            );
        } else {
            println!("{msg}");
        }
        return Ok(());
    }

    // Best-effort refresh so ahead/behind reflects reality. A fetch failure
    // (offline, corporate-blocked remote) is a warning, not a hard error — we
    // fall back to whatever tracking refs exist. Read-only: fetch never writes
    // to the remote.
    if !no_fetch {
        for r in &remotes {
            for b in &DRIFT_BRANCHES {
                let ok = Command::new("git")
                    .arg("-C")
                    .arg(project_root)
                    .args(["fetch", "--quiet", r, b])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if !ok && !json {
                    eprintln!(
                        "  {} could not fetch {r}/{b} — comparing against stale refs",
                        crate::glyph(crate::glyphs::Glyph::Warning)
                    );
                }
            }
        }
    }

    let mut any_diverged = false;
    // (branch, standings, diverged) per branch.
    let mut branch_reports: Vec<(String, Vec<RemoteBranchStanding>, bool)> = Vec::new();

    for b in &DRIFT_BRANCHES {
        let mut standings = Vec::new();
        for r in &remotes {
            let head = aida_core::git_ops::remote_branch_head_sha(project_root, r, b);
            let ahead_behind =
                aida_core::git_ops::ahead_behind(project_root, b, &format!("{r}/{b}"));
            standings.push(RemoteBranchStanding {
                remote: r.clone(),
                head,
                ahead_behind,
            });
        }
        // Diverged when the present tips disagree, OR some remotes have the
        // branch and others don't (mixed presence is drift too).
        let present: Vec<&String> = standings.iter().filter_map(|s| s.head.as_ref()).collect();
        let absent = standings.len() - present.len();
        let distinct = {
            let mut v: Vec<&String> = present.clone();
            v.sort();
            v.dedup();
            v.len()
        };
        let diverged = distinct > 1 || (distinct >= 1 && absent > 0);
        if diverged {
            any_diverged = true;
        }
        branch_reports.push((b.to_string(), standings, diverged));
    }

    if json {
        let branches: Vec<serde_json::Value> = branch_reports
            .iter()
            .map(|(branch, standings, diverged)| {
                let rows: Vec<serde_json::Value> = standings
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "remote": s.remote,
                            "head": s.head,
                            "local_ahead": s.ahead_behind.map(|(a, _)| a),
                            "local_behind": s.ahead_behind.map(|(_, b)| b),
                        })
                    })
                    .collect();
                serde_json::json!({ "branch": branch, "diverged": diverged, "remotes": rows })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "remotes": remotes, "branches": branches, "diverged": any_diverged })
        );
    } else {
        println!("Remote sync status  ({})", remotes.join(", "));
        for (branch, standings, diverged) in &branch_reports {
            let verdict = if *diverged {
                format!("{} DIVERGED", crate::glyph(crate::glyphs::Glyph::Cross))
            } else {
                format!("{} in sync", crate::glyph(crate::glyphs::Glyph::Check))
            };
            println!();
            println!("{branch:<16} {verdict}");
            for s in standings {
                // `+ahead / -behind` of local vs this remote (ASCII: no down-arrow
                // glyph in the theme, and +/- reads naturally to git users).
                let ab = match s.ahead_behind {
                    Some((a, b)) => format!("+{a}/-{b}"),
                    None => "?".to_string(),
                };
                let head = s
                    .head
                    .as_deref()
                    .map(|h| h.chars().take(12).collect::<String>())
                    .unwrap_or_else(|| "absent".to_string());
                println!("  {:<12} {:<10} {}", s.remote, ab, head);
            }
        }
        println!();
        if any_diverged {
            println!(
                "{} remotes disagree on a shared branch. For `aida-store`, run `aida remote reconcile`",
                crate::glyph(crate::glyphs::Glyph::Cross)
            );
            println!("  (dry-run plan; add --execute to union-merge and push every hub). For code");
            println!("  branches, merge the divergent tips and push to every remote — never");
            println!("  force-push a shared branch to resolve.");
        } else {
            println!(
                "{} all remotes agree on every shared branch.",
                crate::glyph(crate::glyphs::Glyph::Check)
            );
        }
    }

    if any_diverged {
        // Non-zero so a pre-push hook / CI step can gate on this.
        std::process::exit(2);
    }
    Ok(())
}

// ───────────────────────── multi-hub store reconcile ─────────────────────

/// The orphan store branch every hub must agree on.
const STORE_BRANCH: &str = "aida-store";

/// Run git in `repo`, returning stdout on success, None otherwise.
fn git_out(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

/// One hub's standing for the store branch during a reconcile.
struct HubStanding {
    remote: String,
    /// The hub's `aida-store` tip (ls-remote), None when absent/unreachable.
    sha: Option<String>,
    /// Human classification relative to the local store tip.
    state: &'static str,
}

/// Extract email-shaped tokens from a text blob. Pure — used to surface
/// identity-bearing registry content a reconcile would newly publish, so the
/// operator consents explicitly (--yes) instead of a hub-only email silently
/// reaching every hub.
// trace:BUG-714 | ai:claude
pub fn emails_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || "\"'`,;()<>[]{}".contains(c)) {
        let token = raw.trim_matches('.');
        let Some(at) = token.find('@') else { continue };
        let (local, rest) = token.split_at(at);
        let domain = &rest[1..];
        let domain_ok = domain.contains('.')
            && domain
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if !local.is_empty() && domain_ok {
            out.push(token.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `aida remote reconcile` — heal a multi-hub diverged `aida-store` (the
/// pull-and-union counterpart of the mirror push fan-out). Fetches every
/// hub's store tip, union-merges the diverged frontier in the store worktree
/// (a merge commit both tips parent, so every hub fast-forwards — never a
/// rebase of published history, never a force-push), verifies the union
/// (registry ranges disjoint, no spec lost), gates it behind the identity
/// scrub, and pushes it to every hub. Dry-run by default: prints the plan and
/// exits 2 when the hubs disagree; `--execute` performs it.
// trace:BUG-714 | ai:claude
pub fn handle_remote_reconcile(
    project_root: &Path,
    execute: bool,
    json: bool,
    yes: bool,
) -> Result<()> {
    use aida_core::git_ops;

    let store_dir = project_root.join(".aida-store");
    if !git_ops::is_git_repo(&store_dir) {
        anyhow::bail!(
            "no store worktree at {} — run a store-reading command (e.g. `aida list`) to attach it, then retry",
            store_dir.display()
        );
    }

    let remotes: Vec<String> = git_ops::list_remotes(project_root)
        .into_iter()
        .filter(|r| r != "all")
        .collect();
    if remotes.is_empty() {
        anyhow::bail!("no git remotes configured — nothing to reconcile");
    }

    // Fetch every hub's store tip (refs via ls-remote for truth, objects via
    // fetch so ancestry checks and the merge can see them). A hub that is
    // unreachable or lacks the branch reports sha=None.
    let local = git_ops::head_sha(&store_dir)?;
    let mut hubs: Vec<HubStanding> = Vec::new();
    for r in &remotes {
        let sha = git_ops::remote_branch_head_sha(project_root, r, STORE_BRANCH);
        if sha.is_some() {
            let _ = Command::new("git")
                .arg("-C")
                .arg(&store_dir)
                .args(["fetch", "--quiet", r, &format!("refs/heads/{STORE_BRANCH}")])
                .status();
        }
        let state = match &sha {
            None => "absent",
            Some(s) if *s == local => "in sync with local",
            Some(s) if git_ops::is_ancestor(&store_dir, s, &local).unwrap_or(false) => {
                "behind local"
            }
            Some(s) if git_ops::is_ancestor(&store_dir, &local, s).unwrap_or(false) => {
                "ahead of local"
            }
            Some(_) => "diverged from local",
        };
        hubs.push(HubStanding {
            remote: r.clone(),
            sha,
            state,
        });
    }

    // The frontier: tips not contained in any other tip. One frontier tip →
    // no merge needed (only fast-forwards / pushes); more → union merge.
    let mut tips: Vec<String> = vec![local.clone()];
    for h in &hubs {
        if let Some(s) = &h.sha {
            if !tips.contains(s) {
                tips.push(s.clone());
            }
        }
    }
    let frontier: Vec<String> = tips
        .iter()
        .filter(|t| {
            !tips.iter().any(|other| {
                *t != other && git_ops::is_ancestor(&store_dir, t, other).unwrap_or(false)
            })
        })
        .cloned()
        .collect();

    let needs_merge = frontier.len() > 1;
    let union_candidate = if needs_merge {
        None
    } else {
        frontier.first().cloned()
    };
    let hubs_needing_push: Vec<&HubStanding> = hubs
        .iter()
        .filter(|h| match (&h.sha, &union_candidate) {
            (Some(s), Some(u)) => s != u,
            // Diverged frontier: every hub gets the (future) union.
            (Some(_), None) => true,
            // Absent branch on the hub: push creates it.
            (None, _) => true,
        })
        .collect();
    let local_needs_ff = union_candidate.as_ref().is_some_and(|u| *u != local);
    let needs_action = needs_merge || !hubs_needing_push.is_empty() || local_needs_ff;

    let short = |s: &str| s.chars().take(12).collect::<String>();

    if !execute {
        if json {
            let hub_rows: Vec<serde_json::Value> = hubs
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "remote": h.remote,
                        "sha": h.sha,
                        "state": h.state,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "branch": STORE_BRANCH,
                    "local": local,
                    "hubs": hub_rows,
                    "diverged": needs_merge,
                    "needs_action": needs_action,
                    "plan": {
                        "union_merge": needs_merge,
                        "push_to": hubs_needing_push.iter().map(|h| h.remote.clone()).collect::<Vec<_>>(),
                        "fast_forward_local": local_needs_ff,
                    },
                    "executed": false,
                })
            );
        } else {
            println!("Store reconcile plan  ({STORE_BRANCH})");
            println!();
            println!("  local        {}", short(&local));
            for h in &hubs {
                let sha = h
                    .sha
                    .as_deref()
                    .map(short)
                    .unwrap_or_else(|| "absent".into());
                println!("  {:<12} {:<14} {}", h.remote, sha, h.state);
            }
            println!();
            if !needs_action {
                println!(
                    "{} every hub already agrees on {STORE_BRANCH} — nothing to reconcile.",
                    crate::glyph(crate::glyphs::Glyph::Check)
                );
                return Ok(());
            }
            if needs_merge {
                println!(
                    "  1. union-merge {} diverged tip(s) in the store worktree",
                    frontier.len()
                );
                println!(
                    "     (spec objects, oplog, and id-block/node registries merge structurally)"
                );
            } else if local_needs_ff {
                println!(
                    "  1. fast-forward the local store to {}",
                    short(union_candidate.as_deref().unwrap_or(""))
                );
            }
            if !hubs_needing_push.is_empty() {
                println!(
                    "  {}. push the result to: {}",
                    if needs_merge || local_needs_ff { 2 } else { 1 },
                    hubs_needing_push
                        .iter()
                        .map(|h| h.remote.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            println!();
            println!("Dry run — nothing changed. Re-run with --execute to perform it.");
        }
        if needs_action {
            // Non-zero so scripts (and the mirror-push warning path) can gate.
            std::process::exit(2);
        }
        return Ok(());
    }

    // ── execute ──
    if !needs_action {
        println!(
            "{} every hub already agrees on {STORE_BRANCH} — nothing to reconcile.",
            crate::glyph(crate::glyphs::Glyph::Check)
        );
        return Ok(());
    }
    if git_ops::worktree_is_dirty(&store_dir) {
        anyhow::bail!(
            "the store worktree at {} has uncommitted changes — commit or clean them first",
            store_dir.display()
        );
    }

    let pre = local.clone();
    let reset_to_pre = || {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&store_dir)
            .args(["reset", "--hard", &pre])
            .status();
    };

    // Merge every frontier tip not already contained in HEAD. Fast-forward
    // when possible; otherwise the structural union merge.
    let mut merge_notes: Vec<String> = Vec::new();
    for tip in &frontier {
        let head = git_ops::head_sha(&store_dir)?;
        if *tip == head || git_ops::is_ancestor(&store_dir, tip, &head).unwrap_or(false) {
            continue;
        }
        if git_ops::is_ancestor(&store_dir, &head, tip).unwrap_or(false) {
            if git_out(&store_dir, &["merge", "--ff-only", tip]).is_none() {
                reset_to_pre();
                anyhow::bail!("fast-forward to {} failed", short(tip));
            }
            continue;
        }
        match git_ops::merge_union_auto(
            &store_dir,
            tip,
            "reconcile multi-hub aida-store (union merge)",
        ) {
            Ok(aida_core::git_ops::StorePullOutcome::Clean) => {}
            Ok(aida_core::git_ops::StorePullOutcome::AutoMerged { notes }) => {
                merge_notes.extend(notes)
            }
            Err(e) => {
                reset_to_pre();
                return Err(e.context(format!(
                    "union merge of {} failed — nothing was pushed; the local store is unchanged",
                    short(tip)
                )));
            }
        }
    }
    let union = git_ops::head_sha(&store_dir)?;

    // Verify: no spec present on any tip may be lost by the union.
    let ls_objects = |rev: &str| -> std::collections::BTreeSet<String> {
        git_out(
            &store_dir,
            &["ls-tree", "-r", "--name-only", rev, "--", "objects/"],
        )
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
    };
    let union_objects = ls_objects(&union);
    for tip in &tips {
        let lost: Vec<String> = ls_objects(tip)
            .difference(&union_objects)
            .cloned()
            .collect();
        if !lost.is_empty() {
            reset_to_pre();
            anyhow::bail!(
                "union would lose {} spec file(s) present on {} (e.g. {}) — refusing; reconcile manually",
                lost.len(),
                short(tip),
                lost[0]
            );
        }
    }

    // Verify: the unioned block registry must stay collision-free even when
    // the merge itself was textually clean.
    let blocks_path = store_dir.join("registry").join("blocks.yaml");
    if blocks_path.exists() {
        let registry = aida_core::node::BlockRegistry::load(&blocks_path)?;
        let collisions = registry.overlapping_ranges();
        if !collisions.is_empty() {
            reset_to_pre();
            anyhow::bail!(
                "unioned block registry has range collision(s): {} — refusing to publish; resolve the allocation conflict first",
                collisions.join("; ")
            );
        }
    }

    // Scrub guard (same contract as the canonical store write path): never
    // publish hub-only content that the identity redaction would have caught.
    // (1) this machine's raw identity must not appear in anything newly
    // published; (2) email-bearing registry lines new to some hub need
    // explicit consent (--yes).
    let mut newly_published = String::new();
    let mut newly_published_registry = String::new();
    for h in &hubs {
        match &h.sha {
            Some(s) if *s == union => {}
            Some(s) => {
                if let Some(diff) = git_out(&store_dir, &["diff", &format!("{s}..{union}")]) {
                    for line in diff.lines() {
                        if line.starts_with('+') && !line.starts_with("+++") {
                            newly_published.push_str(line);
                            newly_published.push('\n');
                        }
                    }
                }
                if let Some(diff) = git_out(
                    &store_dir,
                    &["diff", &format!("{s}..{union}"), "--", "registry/"],
                ) {
                    for line in diff.lines() {
                        if line.starts_with('+') && !line.starts_with("+++") {
                            newly_published_registry.push_str(line);
                            newly_published_registry.push('\n');
                        }
                    }
                }
            }
            None => {
                // A hub without the branch receives everything — treat the
                // identity-bearing registry files as newly published.
                for f in ["registry/nodes.toml", "registry/blocks.yaml", "oplog.yaml"] {
                    if let Ok(text) = std::fs::read_to_string(store_dir.join(f)) {
                        newly_published.push_str(&text);
                        newly_published_registry.push_str(&text);
                    }
                }
            }
        }
    }
    let (pub_host, pub_email) = git_ops::public_identity();
    let raw_host = crate::hostname();
    let raw_email = crate::git_config_value(project_root, "user.email");
    let leaks = git_ops::detect_identity_leaks(
        &newly_published,
        (!raw_host.is_empty()).then_some(raw_host.as_str()),
        raw_email.as_deref(),
        pub_host.as_deref(),
        pub_email.as_deref(),
    );
    if !leaks.is_empty() {
        reset_to_pre();
        anyhow::bail!(
            "reconcile would publish this machine's raw identity to another hub — {} — scrub it first (`aida store scrub-preview`, docs/security/); nothing was pushed",
            leaks.join("; ")
        );
    }
    let suspicious: Vec<String> = emails_in(&newly_published_registry)
        .into_iter()
        .filter(|e| e != aida_core::git_ops::REDACTED_EMAIL_PLACEHOLDER)
        .filter(|e| pub_email.as_deref() != Some(e.as_str()))
        .collect();
    if !suspicious.is_empty() && !yes {
        reset_to_pre();
        anyhow::bail!(
            "reconcile would publish registry content carrying email(s) currently on one hub only: {} — re-run with --yes to consent, or scrub first (docs/security/); nothing was pushed",
            suspicious.join(", ")
        );
    }

    // Push the union everywhere. Every hub's old tip is an ancestor of the
    // union, so each push fast-forwards — no force needed, ever.
    let warn = crate::glyph(crate::glyphs::Glyph::Warning);
    let mut push_failures: Vec<String> = Vec::new();
    let mut pushed: Vec<String> = Vec::new();
    for h in &hubs {
        if h.sha.as_deref() == Some(union.as_str()) {
            continue;
        }
        match git_ops::push(&store_dir, &h.remote, STORE_BRANCH) {
            Ok(true) => pushed.push(h.remote.clone()),
            Ok(false) => {
                push_failures.push(format!(
                    "{}: rejected (raced by a concurrent push — re-run reconcile)",
                    h.remote
                ));
            }
            Err(e) => push_failures.push(format!("{}: {e}", h.remote)),
        }
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "branch": STORE_BRANCH,
                "union": union,
                "merged_tips": frontier,
                "merge_notes": merge_notes,
                "pushed": pushed,
                "push_failures": push_failures,
                "executed": true,
            })
        );
    } else {
        println!(
            "{} reconciled {STORE_BRANCH}: union {}",
            crate::glyph(crate::glyphs::Glyph::Check),
            short(&union)
        );
        for note in &merge_notes {
            println!("    {note}");
        }
        for r in &pushed {
            println!("  pushed → {r}");
        }
        for f in &push_failures {
            eprintln!("  {warn} {f}");
        }
    }
    if !push_failures.is_empty() {
        anyhow::bail!(
            "the union is committed locally but {} hub(s) did not receive it — re-run `aida remote reconcile --execute` once they are reachable",
            push_failures.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_repo_name_uses_basename() {
        assert_eq!(
            default_repo_name(Path::new("/home/joe/ai/aida")),
            "aida".to_string()
        );
        assert_eq!(
            default_repo_name(Path::new("/tmp/my-project")),
            "my-project".to_string()
        );
    }

    #[test]
    fn default_repo_name_falls_back_for_root() {
        assert_eq!(default_repo_name(Path::new("/")), "repo".to_string());
    }

    #[test]
    fn gh_create_args_private_and_public() {
        let priv_args = gh_repo_create_args("owner/repo", true);
        assert_eq!(
            priv_args,
            vec![
                "repo",
                "create",
                "owner/repo",
                "--private",
                "--source",
                ".",
                "--remote",
                "origin",
                "--push"
            ]
        );
        let pub_args = gh_repo_create_args("repo", false);
        assert!(pub_args.contains(&"--public".to_string()));
        assert!(!pub_args.contains(&"--private".to_string()));
        // Always wires origin + pushes in one shot.
        assert!(pub_args.contains(&"--remote".to_string()));
        assert!(pub_args.contains(&"origin".to_string()));
        assert!(pub_args.contains(&"--push".to_string()));
    }

    #[test]
    fn gitlab_url_default_port_is_scp_like() {
        assert_eq!(
            gitlab_ssh_origin_url("gitlab.com", None, "joe", "aida"),
            "git@gitlab.com:joe/aida.git"
        );
        assert_eq!(
            gitlab_ssh_origin_url("gitlab.com", Some(22), "joe", "aida"),
            "git@gitlab.com:joe/aida.git"
        );
    }

    #[test]
    fn gitlab_url_nonstandard_port_uses_ssh_scheme() {
        assert_eq!(
            gitlab_ssh_origin_url("gitlab.joemooney.com", Some(2222), "joe", "aida"),
            "ssh://git@gitlab.joemooney.com:2222/joe/aida.git"
        );
    }

    #[test]
    fn gitlab_url_strips_redundant_dot_git_and_namespace_slashes() {
        assert_eq!(
            gitlab_ssh_origin_url("h", None, "/grp/", "repo.git"),
            "git@h:grp/repo.git"
        );
    }

    #[test]
    fn gitlab_url_empty_namespace_omits_path_segment() {
        assert_eq!(
            gitlab_ssh_origin_url("h", None, "", "repo"),
            "git@h:repo.git"
        );
    }

    #[test]
    fn manual_recipe_names_all_three_routes() {
        let r = manual_recipe("myrepo", "main");
        assert!(r.contains("gh repo create myrepo"));
        assert!(r.contains("git@<gitlab-host>"));
        assert!(r.contains("aida remote attach"));
        assert!(r.contains("git push -u origin main"));
    }

    #[test]
    fn attach_fallback_hint_points_at_ui_and_attach() {
        let h = attach_fallback_hint("gitlab.corp.com", "myrepo");
        assert!(h.contains("gitlab.corp.com"));
        assert!(h.contains("aida remote attach"));
        assert!(h.contains("projects/new"));
    }

    #[test]
    fn known_hosts_round_trip() {
        let hosts = vec![
            KnownHost {
                host: "gitlab.joemooney.com".to_string(),
                label: Some("personal GitLab".to_string()),
                ssh_port: Some(2222),
            },
            KnownHost {
                host: "gitlab.corp.com".to_string(),
                label: None,
                ssh_port: None,
            },
        ];
        let body = serialize_known_hosts(&hosts);
        let parsed = parse_known_hosts(&body);
        assert_eq!(parsed, hosts);
    }

    #[test]
    fn parse_known_hosts_ignores_other_tables_and_comments() {
        let body = "\
# a comment
[[gitlab_host]]
host = \"gitlab.example.com\"  # inline comment
ssh_port = 2222

[other_table]
host = \"should.not.count\"
";
        let parsed = parse_known_hosts(body);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].host, "gitlab.example.com");
        assert_eq!(parsed[0].ssh_port, Some(2222));
        assert_eq!(parsed[0].label, None);
    }

    #[test]
    fn parse_known_hosts_empty_body_is_empty() {
        assert!(parse_known_hosts("").is_empty());
        assert!(parse_known_hosts("# just a comment\n").is_empty());
    }

    // trace:BUG-714 | ai:claude
    #[test]
    fn emails_in_finds_registry_style_emails() {
        let text = "+email = \"jane.doe@corp.example.com\"\n+owner = \"joe\"\n";
        assert_eq!(emails_in(text), vec!["jane.doe@corp.example.com"]);
    }

    // trace:BUG-714 | ai:claude
    #[test]
    fn emails_in_dedupes_and_ignores_non_emails() {
        let text = "a@b.example a@b.example not-an-email @nodomain host@ trailing.dot@x.y.";
        assert_eq!(emails_in(text), vec!["a@b.example", "trailing.dot@x.y"]);
    }

    // trace:BUG-714 | ai:claude
    #[test]
    fn emails_in_empty_text_is_empty() {
        assert!(emails_in("").is_empty());
        assert!(emails_in("registry:\n  blocks: []\n").is_empty());
    }
}
