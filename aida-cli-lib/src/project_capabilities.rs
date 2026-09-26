//! Project capability classification at `aida init` (STORY-1467).
//!
//! AIDA's local graph and pure-git workflow need no forge, no forge CLI and no
//! CI. The PR/MR + CI lifecycle does. Init therefore classifies what the
//! project actually has — which forge (if any) backs `origin`, whether that
//! forge's CLI is installed and authenticated, and whether CI configuration is
//! present — records the repo facts (class, CI) under `[project.capabilities]`
//! in the tracked `.aida/config.toml`, the per-machine facts (origin host,
//! forge CLI/auth) in the gitignored `.aida/capabilities.local.toml`, and
//! prints both, so a forge-dependent feature degrades explicitly instead of
//! failing later.
//!
//! Detection is local and cheap: `git remote get-url origin`, a `PATH` scan,
//! and a few file-existence checks. The only process that may touch the
//! network is `gh auth status`, bounded by [`AUTH_PROBE_TIMEOUT`]. `glab` is
//! never probed, so GitLab auth is recorded as `unknown` — unknown means
//! unknown, never assumed (PRIN-5).
// trace:STORY-1467 | ai:claude

use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

use crate::forge::{self, ForgeKind};
use aida_core::toml_quote::toml_string;

/// Upper bound on the one network-capable probe (`gh auth status`).
// trace:STORY-1467 | ai:claude
pub(crate) const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Which kind of remote collaboration surface backs the project.
// trace:STORY-1467 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForgeClass {
    /// `origin` is hosted on GitHub.
    GitHub,
    /// `origin` is hosted on GitLab (gitlab.com or a gitlab-named host).
    GitLab,
    /// `origin` exists but its host is not a recognized forge. Never guessed
    /// to be GitHub.
    PureGit,
    /// No `origin` remote at all.
    LocalOnly,
}

impl ForgeClass {
    pub(crate) fn token(self) -> &'static str {
        match self {
            ForgeClass::GitHub => "github",
            ForgeClass::GitLab => "gitlab",
            ForgeClass::PureGit => "pure-git",
            ForgeClass::LocalOnly => "local-only",
        }
    }

    fn forge_kind(self) -> ForgeKind {
        match self {
            ForgeClass::GitHub => ForgeKind::GitHub,
            ForgeClass::GitLab => ForgeKind::GitLab,
            ForgeClass::PureGit | ForgeClass::LocalOnly => ForgeKind::None,
        }
    }
}

/// One capability's state. `Unknown` is a real answer: it is recorded when
/// detection could not decide, and nothing downstream may read it as either
/// available or unavailable.
// trace:STORY-1467 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapState {
    Available,
    Unavailable,
    NotApplicable,
    Unknown,
}

impl CapState {
    pub(crate) fn token(self) -> &'static str {
        match self {
            CapState::Available => "available",
            CapState::Unavailable => "unavailable",
            CapState::NotApplicable => "not-applicable",
            CapState::Unknown => "unknown",
        }
    }
}

/// The classified capabilities of one project.
// trace:STORY-1467 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectCapabilities {
    pub class: ForgeClass,
    /// Host parsed from `origin`, when there is one.
    pub origin_host: Option<String>,
    /// Is the forge's CLI (`gh` / `glab`) on `PATH`?
    pub forge_cli: CapState,
    /// Is that CLI authenticated?
    pub forge_auth: CapState,
    /// Can the PR/MR lifecycle run (CLI present AND authenticated)?
    pub forge_lifecycle: CapState,
    /// Is CI configured for the forge that hosts `origin`?
    pub ci: CapState,
    /// Every CI configuration found in the tree, regardless of forge.
    pub ci_config: Vec<&'static str>,
}

/// The side-effecting probes, injectable so each class is testable with
/// fixtures and no real `gh`.
// trace:STORY-1467 | ai:claude
pub(crate) struct Probes<'a> {
    /// Is this binary on `PATH`?
    pub cli_on_path: &'a dyn Fn(&str) -> bool,
    /// Is the GitHub CLI authenticated for this host? `None` = could not tell.
    pub gh_authed: &'a dyn Fn(&str) -> Option<bool>,
}

/// CI configuration present under `root`. Pure file checks.
// trace:STORY-1467 | ai:claude
pub(crate) fn detect_ci_config(root: &Path) -> Vec<&'static str> {
    let mut found = Vec::new();
    let workflows = root.join(".github").join("workflows");
    let has_workflow = std::fs::read_dir(&workflows)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                e.path().is_file() && (name.ends_with(".yml") || name.ends_with(".yaml"))
            })
        })
        .unwrap_or(false);
    if has_workflow {
        found.push("github-actions");
    }
    if root.join(".gitlab-ci.yml").is_file() || root.join(".gitlab-ci.yaml").is_file() {
        found.push("gitlab-ci");
    }
    found
}

/// Classify a project from its `origin` URL (if any) and working tree.
// trace:STORY-1467 | ai:claude
pub(crate) fn classify(
    root: &Path,
    origin_url: Option<&str>,
    probes: &Probes<'_>,
) -> ProjectCapabilities {
    let origin_url = origin_url.map(str::trim).filter(|u| !u.is_empty());
    let origin_host = origin_url.and_then(forge::forge_host_of);
    let class = match origin_url {
        None => ForgeClass::LocalOnly,
        Some(url) => match forge::detect_forge_kind(url) {
            ForgeKind::GitHub => ForgeClass::GitHub,
            ForgeKind::GitLab => ForgeClass::GitLab,
            ForgeKind::None => ForgeClass::PureGit,
        },
    };

    let (forge_cli, forge_auth) = match class {
        ForgeClass::PureGit | ForgeClass::LocalOnly => {
            (CapState::NotApplicable, CapState::NotApplicable)
        }
        ForgeClass::GitHub | ForgeClass::GitLab => {
            let cli = class.forge_kind().cli_name();
            if !(probes.cli_on_path)(cli) {
                (CapState::Unavailable, CapState::Unavailable)
            } else if class == ForgeClass::GitHub {
                let host = origin_host.as_deref().unwrap_or("github.com");
                let auth = match (probes.gh_authed)(host) {
                    Some(true) => CapState::Available,
                    Some(false) => CapState::Unavailable,
                    None => CapState::Unknown,
                };
                (CapState::Available, auth)
            } else {
                // glab is never probed (network, no bounded status verb we
                // rely on) — record what we know, not a guess.
                (CapState::Available, CapState::Unknown)
            }
        }
    };

    let forge_lifecycle = match (forge_cli, forge_auth) {
        (CapState::NotApplicable, _) => CapState::NotApplicable,
        (CapState::Unavailable, _) | (_, CapState::Unavailable) => CapState::Unavailable,
        (CapState::Available, CapState::Available) => CapState::Available,
        _ => CapState::Unknown,
    };

    let ci_config = detect_ci_config(root);
    let ci = match class {
        ForgeClass::GitHub if ci_config.contains(&"github-actions") => CapState::Available,
        ForgeClass::GitLab if ci_config.contains(&"gitlab-ci") => CapState::Available,
        ForgeClass::GitHub | ForgeClass::GitLab => CapState::Unavailable,
        // An unrecognized host (Gitea, Forgejo, a bare server) may or may not
        // run the config it finds — do not guess.
        ForgeClass::PureGit if !ci_config.is_empty() => CapState::Unknown,
        ForgeClass::PureGit => CapState::Unavailable,
        // No remote → nothing hosted runs CI.
        ForgeClass::LocalOnly => CapState::NotApplicable,
    };

    ProjectCapabilities {
        class,
        origin_host,
        forge_cli,
        forge_auth,
        forge_lifecycle,
        ci,
        ci_config,
    }
}

/// Real probe: run `gh auth status --hostname <host>` under a timeout.
/// `Some(true)` on success, `Some(false)` only when gh says it is not logged
/// in, `None` for anything else (timeout, network error, spawn failure).
// trace:STORY-1467 | ai:claude
fn gh_auth_status(host: &str) -> Option<bool> {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["auth", "status", "--hostname", host])
        .stdin(std::process::Stdio::null())
        .env("GH_PROMPT_DISABLED", "1");
    let out = crate::command_output_with_timeout(cmd, AUTH_PROBE_TIMEOUT)?;
    if out.status.success() {
        return Some(true);
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_ascii_lowercase();
    // gh exposes no typed "logged out" exit code (1 covers network errors
    // too), so its documented login hint is the only discriminator; anything
    // else stays unknown.
    // external-prose-classifier: project_capabilities::gh_auth_status
    if text.contains("not logged in") || text.contains("gh auth login") {
        Some(false)
    } else {
        None
    }
}

fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(name).is_file() || (cfg!(windows) && dir.join(format!("{name}.exe")).is_file())
    })
}

/// Classify with the real probes.
// trace:STORY-1467 | ai:claude
pub(crate) fn detect(root: &Path) -> ProjectCapabilities {
    let origin = forge::origin_url(root);
    classify(
        root,
        origin.as_deref(),
        &Probes {
            cli_on_path: &binary_on_path,
            gh_authed: &gh_auth_status,
        },
    )
}

const SECTION_MARKER: &str = "# aida:project-capabilities (STORY-1467)";
const SECTION_HEADER: &str = "[project.capabilities]";

/// Render the `[project.capabilities]` block, marker comment first.
// trace:STORY-1467 | ai:claude
fn render_section(caps: &ProjectCapabilities) -> String {
    let mut s = String::new();
    s.push_str(SECTION_MARKER);
    s.push_str(
        "\n# Repo facts detected at `aida init` (re-run init to refresh).\n\
         # class: github | gitlab | pure-git | local-only.\n\
         # ci: available | unavailable | not-applicable | unknown; `unknown`\n\
         # means detection could not tell and is never assumed either way.\n\
         # Per-machine forge CLI/auth state lives in the gitignored\n\
         # .aida/capabilities.local.toml, never here.\n",
    );
    s.push_str(SECTION_HEADER);
    s.push('\n');
    // Repo facts only: no host (a self-hosted or internal hostname must not
    // be committed to a possibly-public repo) and no per-machine CLI/auth.
    // trace:BUG-1650 | ai:claude
    s.push_str(&format!("class = {}\n", toml_string(caps.class.token())));
    s.push_str(&format!("ci = {}\n", toml_string(caps.ci.token())));
    let list: Vec<String> = caps.ci_config.iter().map(|c| toml_string(c)).collect();
    s.push_str(&format!("ci_config = [{}]\n", list.join(", ")));
    s
}

/// Line span `[first, last)` of an existing capabilities block: the marker
/// comment run in front of the header (when present), the header, and its key
/// lines up to the next blank line or table header.
// trace:STORY-1467 | ai:claude
fn section_span(lines: &[&str]) -> Option<(usize, usize)> {
    let header = lines.iter().position(|l| l.trim() == SECTION_HEADER)?;
    let mut first = header;
    let mut i = header;
    while i > 0 && lines[i - 1].trim_start().starts_with('#') {
        i -= 1;
        if lines[i].trim() == SECTION_MARKER {
            first = i;
            break;
        }
    }
    let mut last = header + 1;
    while last < lines.len() {
        let t = lines[last].trim();
        if t.is_empty() || t.starts_with('[') {
            break;
        }
        last += 1;
    }
    Some((first, last))
}

/// `content` with its capabilities block replaced where it stands, or a fresh
/// block appended when there is none. Pure; replacing in place (not moving
/// the block to the end) is what keeps a refresh byte-identical after other
/// init steps append their own sections. Every other byte is preserved.
// trace:STORY-1467 | ai:claude
fn upsert_section(content: &str, caps: &ProjectCapabilities) -> String {
    let block = render_section(caps);
    let lines: Vec<&str> = content.lines().collect();
    if let Some((first, last)) = section_span(&lines) {
        let mut out: Vec<&str> = lines[..first].to_vec();
        out.extend(block.lines());
        out.extend_from_slice(&lines[last..]);
        let mut joined = out.join("\n");
        if content.ends_with('\n') || last == lines.len() {
            joined.push('\n');
        }
        return joined;
    }
    let mut after = content.to_string();
    if !after.is_empty() && !after.ends_with('\n') {
        after.push('\n');
    }
    if !after.is_empty() {
        after.push('\n');
    }
    after.push_str(&block);
    after
}

/// Append the repo-fact capabilities block to the config `aida init` is about
/// to write, so the scaffold commit already contains it and the end-of-init
/// refresh is a no-op instead of leaving `.aida/config.toml` dirty. Repo facts
/// only, so no CLI or auth probe runs here.
// trace:STORY-1467 | ai:claude
pub(crate) fn with_init_config_section(root: &Path, content: String) -> String {
    let caps = classify(
        root,
        forge::origin_url(root).as_deref(),
        &Probes {
            cli_on_path: &|_| false,
            gh_authed: &|_| None,
        },
    );
    upsert_section(&content, &caps)
}

/// Upsert `[project.capabilities]` into `<root>/.aida/config.toml`, appended
/// at the end so it never splits another section's trailing comments; every
/// other byte is preserved. Returns false (writes nothing) when there is no
/// config. Refuses to write a result that no longer parses as TOML.
// trace:STORY-1467 | ai:claude
pub(crate) fn write_capabilities(root: &Path, caps: &ProjectCapabilities) -> Result<bool> {
    let path = root.join(".aida").join("config.toml");
    let before = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let after = upsert_section(&before, caps);
    toml::from_str::<toml::Value>(&after)
        .with_context(|| format!("{} would not stay valid TOML", path.display()))?;
    if after != before {
        std::fs::write(&path, after).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(true)
}

/// Gitignored per-machine runtime state (covered by the `.aida/*`
/// deny-by-default ignore block): which host `origin` points at and whether
/// this machine's forge CLI is installed and logged in.
// trace:STORY-1467 | ai:claude
pub(crate) const LOCAL_STATE_REL_PATH: &str = ".aida/capabilities.local.toml";

/// Render the per-machine runtime state file.
// trace:STORY-1467 | ai:claude
fn render_local_state(caps: &ProjectCapabilities) -> String {
    let mut s = String::from(
        "# Per-machine forge state detected by `aida init` (STORY-1467).\n\
         # Runtime state: gitignored, rewritten on every init.\n\
         # Each value: available | unavailable | not-applicable | unknown.\n",
    );
    // trace:BUG-1650 | ai:claude
    s.push_str(&format!("class = {}\n", toml_string(caps.class.token())));
    if let Some(host) = &caps.origin_host {
        // trace:BUG-1649 | ai:claude
        s.push_str(&format!("origin_host = {}\n", toml_string(host)));
    }
    s.push_str(&format!(
        "forge_cli = {}\n",
        toml_string(caps.forge_cli.token())
    ));
    s.push_str(&format!(
        "forge_auth = {}\n",
        toml_string(caps.forge_auth.token())
    ));
    s.push_str(&format!(
        "forge_lifecycle = {}\n",
        toml_string(caps.forge_lifecycle.token())
    ));
    s
}

/// Write `.aida/capabilities.local.toml`. Returns false when `.aida/` is
/// absent (nothing initialized to attach it to).
// trace:STORY-1467 | ai:claude
pub(crate) fn write_local_state(root: &Path, caps: &ProjectCapabilities) -> Result<bool> {
    if !root.join(".aida").is_dir() {
        return Ok(false);
    }
    let path = root.join(LOCAL_STATE_REL_PATH);
    let body = render_local_state(caps);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(true)
}

/// Human-facing summary lines: the class, then one line per capability, each
/// unavailable/unknown state carrying what to do about it.
// trace:STORY-1467 | ai:claude
pub(crate) fn summary_lines(caps: &ProjectCapabilities) -> Vec<String> {
    let mut lines = Vec::new();
    let class_line = match caps.class {
        ForgeClass::GitHub => "GitHub (origin host)".to_string(),
        ForgeClass::GitLab => "GitLab (origin host)".to_string(),
        ForgeClass::PureGit => format!(
            "pure git (origin host {} is not a recognized forge)",
            caps.origin_host.as_deref().unwrap_or("unknown")
        ),
        ForgeClass::LocalOnly => "local only (no origin remote)".to_string(),
    };
    lines.push(format!("Project capabilities: {class_line}"));

    let kind = caps.class.forge_kind();
    let cli = kind.cli_name();
    match caps.class {
        ForgeClass::PureGit | ForgeClass::LocalOnly => {
            lines.push(
                "  PR/MR lifecycle: not applicable - specs, traces and merge-by-ancestry \
                 work without a forge"
                    .to_string(),
            );
            if caps.class == ForgeClass::LocalOnly {
                lines.push(
                    "  add a remote later with `aida remote create` or `git remote add origin <url>`"
                        .to_string(),
                );
            }
        }
        ForgeClass::GitHub | ForgeClass::GitLab => {
            let cli_line = match caps.forge_cli {
                CapState::Available => format!("  forge CLI: {cli} available"),
                _ => {
                    let url = kind.cli_install_hint().map(|(_, u)| u).unwrap_or("");
                    format!("  forge CLI: {cli} unavailable - install it: {url}")
                }
            };
            lines.push(cli_line);
            let auth_line = match caps.forge_auth {
                CapState::Available => format!("  forge auth: {cli} authenticated"),
                CapState::Unavailable if caps.forge_cli == CapState::Available => {
                    format!("  forge auth: unavailable - run `{cli} auth login`")
                }
                CapState::Unavailable => "  forge auth: unavailable (no CLI)".to_string(),
                _ => format!("  forge auth: unknown - check with `{cli} auth status`"),
            };
            lines.push(auth_line);
            let lifecycle = match caps.forge_lifecycle {
                CapState::Available => "available",
                CapState::Unavailable => {
                    "unavailable - PR/MR commands will refuse until the CLI is set up"
                }
                _ => "unknown - PR/MR commands may fail until the CLI is confirmed",
            };
            lines.push(format!("  PR/MR lifecycle: {lifecycle}"));
        }
    }

    let found = if caps.ci_config.is_empty() {
        String::new()
    } else {
        format!(" (found: {})", caps.ci_config.join(", "))
    };
    let ci_line = match caps.ci {
        CapState::Available => format!("  CI: available{found}"),
        CapState::Unavailable => match caps.class {
            ForgeClass::GitHub => format!(
                "  CI: unavailable{found} - no .github/workflows/; CI-wait phases are skipped"
            ),
            ForgeClass::GitLab => {
                format!("  CI: unavailable{found} - no .gitlab-ci.yml; CI-wait phases are skipped")
            }
            _ => "  CI: unavailable - no CI configuration found".to_string(),
        },
        CapState::NotApplicable => format!("  CI: not applicable{found} (no remote runs it)"),
        CapState::Unknown => {
            format!("  CI: unknown{found} - whether this host runs it could not be determined")
        }
    };
    lines.push(ci_line);
    lines
}

/// Detect, record, and print. Best-effort: a failure here never fails init.
// trace:STORY-1467 | ai:claude
pub(crate) fn record_and_report(root: &Path) {
    if !root.join(".aida").join("config.toml").is_file() {
        return;
    }
    let caps = detect(root);
    if let Err(e) = write_capabilities(root, &caps) {
        eprintln!("  note: could not record project capabilities: {e}");
    }
    if let Err(e) = write_local_state(root, &caps) {
        eprintln!("  note: could not record local forge state: {e}");
    }
    println!();
    for line in summary_lines(&caps) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes<'a>(
        on_path: &'a dyn Fn(&str) -> bool,
        authed: &'a dyn Fn(&str) -> Option<bool>,
    ) -> Probes<'a> {
        Probes {
            cli_on_path: on_path,
            gh_authed: authed,
        }
    }

    fn all_cli(_: &str) -> bool {
        true
    }
    fn no_cli(_: &str) -> bool {
        false
    }
    fn authed(_: &str) -> Option<bool> {
        Some(true)
    }
    fn not_authed(_: &str) -> Option<bool> {
        Some(false)
    }
    fn auth_unknown(_: &str) -> Option<bool> {
        None
    }
    fn must_not_probe(_: &str) -> Option<bool> {
        panic!("auth must not be probed for this class")
    }

    fn fixture() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn add_github_workflow(root: &Path) {
        let dir = root.join(".github").join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ci.yml"), "on: push\n").unwrap();
    }

    #[test]
    fn github_with_authed_cli_and_workflows_is_fully_available() {
        let d = fixture();
        add_github_workflow(d.path());
        let caps = classify(
            d.path(),
            Some("git@github.com:o/r.git"),
            &probes(&all_cli, &authed),
        );
        assert_eq!(caps.class, ForgeClass::GitHub);
        assert_eq!(caps.origin_host.as_deref(), Some("github.com"));
        assert_eq!(caps.forge_cli, CapState::Available);
        assert_eq!(caps.forge_auth, CapState::Available);
        assert_eq!(caps.forge_lifecycle, CapState::Available);
        assert_eq!(caps.ci, CapState::Available);
        assert_eq!(caps.ci_config, vec!["github-actions"]);
    }

    #[test]
    fn github_without_gh_is_unavailable_with_install_guidance() {
        let d = fixture();
        let caps = classify(
            d.path(),
            Some("https://github.com/o/r.git"),
            &probes(&no_cli, &must_not_probe),
        );
        assert_eq!(caps.class, ForgeClass::GitHub);
        assert_eq!(caps.forge_cli, CapState::Unavailable);
        assert_eq!(caps.forge_lifecycle, CapState::Unavailable);
        assert_eq!(caps.ci, CapState::Unavailable);
        let text = summary_lines(&caps).join("\n");
        assert!(text.contains("https://cli.github.com"), "{text}");
        assert!(text.contains("no .github/workflows/"), "{text}");
    }

    #[test]
    fn github_not_logged_in_says_auth_login() {
        let d = fixture();
        let caps = classify(
            d.path(),
            Some("https://github.com/o/r.git"),
            &probes(&all_cli, &not_authed),
        );
        assert_eq!(caps.forge_auth, CapState::Unavailable);
        assert_eq!(caps.forge_lifecycle, CapState::Unavailable);
        assert!(summary_lines(&caps).join("\n").contains("gh auth login"));
    }

    #[test]
    fn github_auth_probe_inconclusive_stays_unknown() {
        let d = fixture();
        let caps = classify(
            d.path(),
            Some("https://github.com/o/r.git"),
            &probes(&all_cli, &auth_unknown),
        );
        assert_eq!(caps.forge_auth, CapState::Unknown);
        assert_eq!(caps.forge_lifecycle, CapState::Unknown);
    }

    #[test]
    fn gitlab_with_ci_config_never_probes_auth() {
        let d = fixture();
        std::fs::write(d.path().join(".gitlab-ci.yml"), "stages: []\n").unwrap();
        let caps = classify(
            d.path(),
            Some("git@gitlab.example.com:g/p.git"),
            &probes(&all_cli, &must_not_probe),
        );
        assert_eq!(caps.class, ForgeClass::GitLab);
        assert_eq!(caps.forge_cli, CapState::Available);
        assert_eq!(caps.forge_auth, CapState::Unknown);
        assert_eq!(caps.forge_lifecycle, CapState::Unknown);
        assert_eq!(caps.ci, CapState::Available);
        assert_eq!(caps.ci_config, vec!["gitlab-ci"]);
    }

    #[test]
    fn unknown_host_is_pure_git_not_github() {
        let d = fixture();
        let caps = classify(
            d.path(),
            Some("ssh://git@git.internal.example:2222/team/repo.git"),
            &probes(&all_cli, &must_not_probe),
        );
        assert_eq!(caps.class, ForgeClass::PureGit);
        assert_eq!(caps.origin_host.as_deref(), Some("git.internal.example"));
        assert_eq!(caps.forge_cli, CapState::NotApplicable);
        assert_eq!(caps.forge_lifecycle, CapState::NotApplicable);
        assert_eq!(caps.ci, CapState::Unavailable);
    }

    #[test]
    fn pure_git_with_workflows_reports_ci_unknown() {
        let d = fixture();
        add_github_workflow(d.path());
        let caps = classify(
            d.path(),
            Some("https://codeberg.org/o/r.git"),
            &probes(&all_cli, &must_not_probe),
        );
        assert_eq!(caps.class, ForgeClass::PureGit);
        assert_eq!(caps.ci, CapState::Unknown);
    }

    #[test]
    fn no_remote_is_local_only() {
        let d = fixture();
        for origin in [None, Some(""), Some("  ")] {
            let caps = classify(d.path(), origin, &probes(&all_cli, &must_not_probe));
            assert_eq!(caps.class, ForgeClass::LocalOnly);
            assert_eq!(caps.origin_host, None);
            assert_eq!(caps.forge_cli, CapState::NotApplicable);
            assert_eq!(caps.forge_auth, CapState::NotApplicable);
            assert_eq!(caps.ci, CapState::NotApplicable);
        }
    }

    #[test]
    fn workflow_dir_without_yaml_is_not_ci() {
        let d = fixture();
        let dir = d.path().join(".github").join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("README.md"), "x").unwrap();
        assert!(detect_ci_config(d.path()).is_empty());
    }

    #[test]
    fn write_upserts_section_and_preserves_config() {
        let d = fixture();
        let aida = d.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        let original = "# top\n[forge]\nprovider = \"github\"\n";
        std::fs::write(aida.join("config.toml"), original).unwrap();

        let caps = classify(
            d.path(),
            Some("git@github.com:o/r.git"),
            &probes(&all_cli, &authed),
        );
        assert!(write_capabilities(d.path(), &caps).unwrap());
        let local = classify(d.path(), None, &probes(&all_cli, &must_not_probe));
        assert!(write_capabilities(d.path(), &local).unwrap());

        let text = std::fs::read_to_string(aida.join("config.toml")).unwrap();
        assert!(text.starts_with(original), "{text}");
        assert!(text.ends_with("ci_config = []\n"), "{text}");
        assert!(write_capabilities(d.path(), &local).unwrap());
        assert_eq!(
            std::fs::read_to_string(aida.join("config.toml")).unwrap(),
            text,
            "rewrite must be idempotent"
        );
        assert_eq!(text.matches("[project.capabilities]").count(), 1, "{text}");
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        let t = &parsed["project"]["capabilities"];
        assert_eq!(t["class"].as_str(), Some("local-only"));
        assert_eq!(t["ci"].as_str(), Some("not-applicable"));
        assert!(t.get("origin_host").is_none());
        for key in ["forge_cli", "forge_auth", "forge_lifecycle"] {
            assert!(t.get(key).is_none(), "{key} is per-machine: {text}");
        }
        assert_eq!(parsed["forge"]["provider"].as_str(), Some("github"));
    }

    #[test]
    fn rewrite_keeps_content_that_follows_an_old_block() {
        let d = fixture();
        let aida = d.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        let caps = classify(d.path(), None, &probes(&all_cli, &must_not_probe));
        let seeded = format!(
            "[forge]\nprovider = \"pure-git\"\n\n{}\n# tail\n[x]\ny = 1\n",
            render_section(&caps)
        );
        std::fs::write(aida.join("config.toml"), &seeded).unwrap();
        assert!(write_capabilities(d.path(), &caps).unwrap());
        let text = std::fs::read_to_string(aida.join("config.toml")).unwrap();
        assert_eq!(text.matches(SECTION_HEADER).count(), 1, "{text}");
        assert_eq!(text.matches(SECTION_MARKER).count(), 1, "{text}");
        assert!(text.contains("# tail\n[x]\ny = 1\n"), "{text}");
        assert!(text.contains("provider = \"pure-git\""), "{text}");
    }

    #[test]
    fn write_refuses_to_break_a_scalar_project_key() {
        let d = fixture();
        let aida = d.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(aida.join("config.toml"), "project = \"x\"\n").unwrap();
        let caps = classify(d.path(), None, &probes(&all_cli, &must_not_probe));
        assert!(write_capabilities(d.path(), &caps).is_err());
        assert_eq!(
            std::fs::read_to_string(aida.join("config.toml")).unwrap(),
            "project = \"x\"\n"
        );
    }

    #[test]
    fn tracked_config_never_carries_the_origin_host() {
        let d = fixture();
        let aida = d.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(
            aida.join("config.toml"),
            "[forge]\nprovider = \"pure-git\"\n",
        )
        .unwrap();
        let caps = classify(
            d.path(),
            Some("git@git.internal.example:team/repo.git"),
            &probes(&all_cli, &must_not_probe),
        );
        assert!(write_capabilities(d.path(), &caps).unwrap());
        let text = std::fs::read_to_string(aida.join("config.toml")).unwrap();
        assert!(!text.contains("git.internal.example"), "{text}");
        assert!(!text.contains("origin_host"), "{text}");
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed["project"]["capabilities"]["class"].as_str(),
            Some("pure-git")
        );
    }

    #[test]
    fn local_state_holds_host_and_per_machine_forge_state() {
        let d = fixture();
        std::fs::create_dir_all(d.path().join(".aida")).unwrap();
        let caps = classify(
            d.path(),
            Some("https://github.com/o/r.git"),
            &probes(&all_cli, &not_authed),
        );
        assert!(write_local_state(d.path(), &caps).unwrap());
        let first = std::fs::read_to_string(d.path().join(LOCAL_STATE_REL_PATH)).unwrap();
        let parsed: toml::Value = toml::from_str(&first).unwrap();
        assert_eq!(parsed["origin_host"].as_str(), Some("github.com"));
        assert_eq!(parsed["forge_cli"].as_str(), Some("available"));
        assert_eq!(parsed["forge_auth"].as_str(), Some("unavailable"));
        assert_eq!(parsed["forge_lifecycle"].as_str(), Some("unavailable"));
        assert!(write_local_state(d.path(), &caps).unwrap());
        assert_eq!(
            std::fs::read_to_string(d.path().join(LOCAL_STATE_REL_PATH)).unwrap(),
            first,
            "rewrite must be idempotent"
        );
    }

    // trace:BUG-1649 | ai:claude
    #[test]
    fn bug_1649_local_state_origin_host_round_trips_hostile_values() {
        let d = fixture();
        let mut caps = classify(
            d.path(),
            Some("https://github.com/o/r.git"),
            &probes(&all_cli, &not_authed),
        );
        for host in ["C:\\Users\\RUNNER~1\\x", "host\"with\"quotes", "tab\there"] {
            caps.origin_host = Some(host.to_string());
            let body = render_local_state(&caps);
            let parsed: toml::Value = toml::from_str(&body).expect("valid TOML");
            assert_eq!(parsed["origin_host"].as_str(), Some(host));
        }
    }

    #[test]
    fn local_state_needs_an_initialized_aida_dir() {
        let d = fixture();
        let caps = classify(d.path(), None, &probes(&all_cli, &must_not_probe));
        assert!(!write_local_state(d.path(), &caps).unwrap());
        assert!(!d.path().join(".aida").exists());
    }

    #[test]
    fn init_section_then_refresh_is_byte_identical() {
        let d = fixture();
        let aida = d.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        add_github_workflow(d.path());
        let seeded = with_init_config_section(d.path(), "[forge]\nprovider = \"x\"\n".into());
        std::fs::write(aida.join("config.toml"), &seeded).unwrap();
        let caps = classify(d.path(), None, &probes(&all_cli, &must_not_probe));
        assert!(write_capabilities(d.path(), &caps).unwrap());
        assert_eq!(
            std::fs::read_to_string(aida.join("config.toml")).unwrap(),
            seeded
        );
    }

    #[test]
    fn refresh_replaces_the_block_where_it_stands() {
        let d = fixture();
        let aida = d.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        let seeded = with_init_config_section(d.path(), "[forge]\nprovider = \"x\"\n".into());
        // Later init steps append their own sections after the block.
        let seeded = format!("{seeded}\n[scaffold]\nfootprint = \"full\"\n");
        std::fs::write(aida.join("config.toml"), &seeded).unwrap();
        let same = classify(d.path(), None, &probes(&all_cli, &must_not_probe));
        assert!(write_capabilities(d.path(), &same).unwrap());
        assert_eq!(
            std::fs::read_to_string(aida.join("config.toml")).unwrap(),
            seeded
        );

        add_github_workflow(d.path());
        let changed = classify(
            d.path(),
            Some("https://github.com/o/r.git"),
            &probes(&all_cli, &authed),
        );
        assert!(write_capabilities(d.path(), &changed).unwrap());
        let text = std::fs::read_to_string(aida.join("config.toml")).unwrap();
        assert!(
            text.ends_with("[scaffold]\nfootprint = \"full\"\n"),
            "{text}"
        );
        assert!(text.contains("class = \"github\""), "{text}");
        assert_eq!(text.matches(SECTION_HEADER).count(), 1, "{text}");
    }

    #[test]
    fn write_without_config_is_a_noop() {
        let d = fixture();
        let caps = classify(d.path(), None, &probes(&all_cli, &must_not_probe));
        assert!(!write_capabilities(d.path(), &caps).unwrap());
        assert!(!d.path().join(".aida").exists());
    }
}
