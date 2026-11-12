//! Multi-forge publishing for `aida init` and `aida remote mirror`: the
//! GitLab hub's create-if-missing fallback and the "every hub agrees" check.
//!
//! GitLab's SSH push-to-create works in a personal namespace but a group can
//! refuse it with "The project you were looking for could not be found". When
//! (and ONLY when) a push or probe is refused that way, AIDA creates the
//! project through `glab api -X POST projects` in the requested namespace,
//! private unless asked otherwise, and retries. Any other push failure is
//! surfaced unchanged: a wrong host, a bad key or a rejected ref must never
//! make AIDA create a project on someone's forge.
//!
//! The pure pieces (target parsing, refusal classification, the glab argv
//! builders, namespace-id parsing) are free functions with unit tests; the
//! thin process shell around them is exercised with fake `glab`/`ssh`.
// trace:STORY-1656 | ai:claude

use crate::process_retry::RetryEtxtbsy;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// The remote name a second (GitLab) hub gets when GitHub is `origin`.
// trace:STORY-1656 | ai:claude
pub(crate) const GITLAB_HUB_REMOTE: &str = "gitlab";

/// A GitLab hub requested on the command line (`--gitlab HOST[/GROUP]`) or
/// by a forge profile: where the project lives and how to reach it.
// trace:STORY-1656 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitLabHub {
    /// The forge host, e.g. `gitlab.joemooney.com`.
    pub host: String,
    /// The group/user namespace, e.g. `ai` or `ai/tools`. Empty means the
    /// authenticated user's personal namespace.
    pub namespace: String,
    /// Use an HTTPS clone URL instead of SSH (profiles with protocol=https).
    pub https: bool,
}

impl GitLabHub {
    /// Parse `HOST[/GROUP[/SUBGROUP...]]`, tolerating a leading scheme and a
    /// trailing slash. Pure.
    pub(crate) fn parse(spec: &str) -> Result<GitLabHub> {
        let trimmed = spec.trim();
        let without_scheme = trimmed
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(trimmed)
            .trim_matches('/');
        let (host, namespace) = match without_scheme.split_once('/') {
            Some((h, ns)) => (h, ns.trim_matches('/')),
            None => (without_scheme, ""),
        };
        if host.is_empty() || host.contains(char::is_whitespace) || host.contains('@') {
            bail!("--gitlab expects HOST or HOST/GROUP (for example gitlab.example.com/team), got `{spec}`");
        }
        Ok(GitLabHub {
            host: host.to_ascii_lowercase(),
            namespace: namespace.to_string(),
            https: false,
        })
    }

    /// The clone URL for `repo` on this hub. Pure.
    pub(crate) fn remote_url(&self, repo: &str) -> String {
        let path = if self.namespace.is_empty() {
            format!("{repo}.git")
        } else {
            format!("{}/{repo}.git", self.namespace)
        };
        if self.https {
            format!("https://{}/{path}", self.host)
        } else {
            format!("git@{}:{path}", self.host)
        }
    }
}

/// True when git's stderr is GitLab refusing a push or fetch because the
/// project does not exist (push-to-create disabled, or a group namespace that
/// does not allow it). Deliberately narrow: auth failures, network errors and
/// rejected refs are NOT refusals, so they never trigger project creation.
// trace:STORY-1656 | ai:claude
pub(crate) fn is_push_to_create_refusal(stderr: &str) -> bool {
    // external-prose-classifier: gitlab_project_create::is_push_to_create_refusal
    let s = stderr.to_ascii_lowercase();
    s.contains("project you were looking for could not be found")
        || s.contains("project could not be found")
}

/// Percent-encode a namespace path for a GitLab API path segment
/// (`ai/tools` -> `ai%2Ftools`). Pure.
fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `glab` argv (after the binary) that resolves a group/user namespace to its
/// numeric id. Pure.
// trace:STORY-1656 | ai:claude
pub(crate) fn namespace_lookup_args(host: &str, namespace: &str) -> Vec<String> {
    vec![
        "api".to_string(),
        "--hostname".to_string(),
        host.to_string(),
        format!("namespaces/{}", encode_path(namespace)),
    ]
}

/// The numeric `id` from a GitLab namespace JSON body. Pure.
// trace:STORY-1656 | ai:claude
pub(crate) fn parse_namespace_id(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json.get("id").and_then(|v| v.as_u64())
}

/// `glab` argv (after the binary) that creates `name` in `namespace_id`
/// (None = the caller's personal namespace). Private unless `public`. Pure.
// trace:STORY-1656 | ai:claude
pub(crate) fn create_project_args(
    host: &str,
    namespace_id: Option<u64>,
    name: &str,
    public: bool,
) -> Vec<String> {
    let mut args = vec![
        "api".to_string(),
        "--hostname".to_string(),
        host.to_string(),
        "-X".to_string(),
        "POST".to_string(),
        "projects".to_string(),
        "-f".to_string(),
        format!("name={name}"),
        "-f".to_string(),
        format!("path={name}"),
    ];
    if let Some(id) = namespace_id {
        args.push("-f".to_string());
        args.push(format!("namespace_id={id}"));
    }
    args.push("-f".to_string());
    args.push(format!(
        "visibility={}",
        if public { "public" } else { "private" }
    ));
    args
}

/// Split a GitLab clone URL into `(host, namespace, name)`. Pure.
// trace:STORY-1656 | ai:claude
pub(crate) fn gitlab_target_of_url(url: &str) -> Option<(String, String, String)> {
    let host = crate::forge::forge_host_of(url)?;
    let path = crate::forge::project_path_of(url)?;
    let (namespace, name) = match path.rsplit_once('/') {
        Some((ns, n)) => (ns.to_string(), n.to_string()),
        None => (String::new(), path.clone()),
    };
    if name.is_empty() {
        return None;
    }
    Some((host, namespace, name))
}

fn glab_binary() -> std::path::PathBuf {
    crate::resolve_glab_binary().unwrap_or_else(|| std::path::PathBuf::from("glab"))
}

fn run_glab(token: &str, args: &[String]) -> Result<String> {
    let out = Command::new(glab_binary())
        .args(args)
        .env("GITLAB_TOKEN", token)
        .output_retrying_etxtbsy()
        .context("could not launch `glab` — install it from https://gitlab.com/gitlab-org/cli")?;
    if !out.status.success() {
        bail!(
            "`glab {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Create `namespace/name` on `host` through the GitLab API (via `glab`).
/// Without a GitLab token the error carries the manual recipe
/// ([`crate::remote_create::attach_fallback_hint`]) and nothing is created.
// trace:STORY-1656 | ai:claude
pub(crate) fn create_gitlab_project(
    host: &str,
    namespace: &str,
    name: &str,
    public: bool,
) -> Result<()> {
    let Some((token, source)) = crate::remote_create::gitlab_token_for_host(host) else {
        bail!(
            "{host} refused push-to-create and no GitLab token is available \
             (run `glab auth login --hostname {host}`, or set GITLAB_TOKEN).\n{}",
            crate::remote_create::attach_fallback_hint(host, name)
        );
    };
    // An env token names no host; say it is being sent to this one.
    // trace:BUG-1972 | ai:claude
    if let crate::remote_create::GitlabTokenSource::Env(var) = source {
        println!(
            "  {} no `glab auth login` token for {host}; using ${var}, which is not tied \
             to a host (run `glab auth login --hostname {host}` to scope one)",
            crate::glyph(crate::glyphs::Glyph::Warning)
        );
    }
    let namespace_id = if namespace.is_empty() {
        None
    } else {
        let body = run_glab(&token, &namespace_lookup_args(host, namespace))?;
        Some(parse_namespace_id(&body).with_context(|| {
            format!("could not read the id of GitLab namespace `{namespace}` on {host}")
        })?)
    };
    run_glab(
        &token,
        &create_project_args(host, namespace_id, name, public),
    )?;
    let shown = if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}/{name}")
    };
    println!(
        "  {} created {} GitLab project {host}/{shown} through the API",
        crate::glyph(crate::glyphs::Glyph::Check),
        if public { "public" } else { "private" },
    );
    Ok(())
}

/// Run `git push` with `args` in `dir`, returning stderr on failure.
// trace:BUG-2119 | ai:agy
fn git_push(
    dir: &Path,
    remote: &str,
    url: &str,
    args: &[&str],
) -> Result<std::result::Result<(), String>> {
    // trace:TASK-2270 | ai:claude
    let upstream_branch = args.contains(&"-u").then(|| args.last().copied()).flatten();
    if upstream_branch.is_some() {
        aida_core::git_ops::clear_sandbox_config_lock(dir);
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .arg("push")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    configure_git_remote_credentials(&mut cmd, dir, remote, Some(url));
    let out = cmd.output().context("could not launch `git push`")?;
    if out.status.success() {
        if let Some(branch) = upstream_branch {
            aida_core::git_ops::repair_push_upstream(dir, remote, Some(branch), branch);
        }
        Ok(Ok(()))
    } else {
        Ok(Err(String::from_utf8_lossy(&out.stderr).to_string()))
    }
}

/// Push `branch` to the GitLab hub `remote` (whose URL is `url`). When the
/// forge refuses because the project does not exist, create it through the
/// API and push again. Any other failure is returned as-is.
// trace:STORY-1656 | ai:claude
pub(crate) fn push_with_create_fallback(
    dir: &Path,
    remote: &str,
    url: &str,
    branch: &str,
    set_upstream: bool,
    public: bool,
) -> Result<()> {
    let mut args: Vec<&str> = Vec::new();
    if set_upstream {
        args.push("-u");
    }
    args.push(remote);
    args.push(branch);
    let stderr = match git_push(dir, remote, url, &args)? {
        Ok(()) => return Ok(()),
        Err(stderr) => stderr,
    };
    // A remote URL may embed a token; never echo it. trace:BUG-1972 | ai:claude
    if !is_push_to_create_refusal(&stderr) {
        bail!(
            "`git push {}` failed: {}",
            args.join(" "),
            redact_url_userinfo(stderr.trim())
        );
    }
    let Some((host, namespace, name)) = gitlab_target_of_url(url) else {
        bail!(
            "`git push {}` was refused and `{}` is not a GitLab project URL AIDA can create: {}",
            args.join(" "),
            redact_url_userinfo(url),
            redact_url_userinfo(stderr.trim())
        );
    };
    println!(
        "  {} {host} refused push-to-create — creating the project through the API",
        crate::glyph(crate::glyphs::Glyph::Warning)
    );
    create_gitlab_project(&host, &namespace, &name, public)?;
    match git_push(dir, remote, url, &args)? {
        Ok(()) => Ok(()),
        Err(stderr) => bail!(
            "`git push {}` still failed after creating the project: {}",
            args.join(" "),
            redact_url_userinfo(stderr.trim())
        ),
    }
}

/// `text` with the userinfo of every `scheme://user:secret@host` URL replaced
/// by `***`, so a remote URL carrying an embedded token is never printed.
// trace:BUG-1972 | ai:claude
pub(crate) fn redact_url_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let authority_end = tail
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(tail.len());
        match tail[..authority_end].rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&tail[at..authority_end]);
            }
            None => out.push_str(&tail[..authority_end]),
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

// The credential gate lives in `aida_core::git_ops` so every host remote
// call shares it; these names stay here for existing callers.
// trace:BUG-2181 | ai:claude
pub(crate) use aida_core::git_ops::{percent_decode, split_http_url_userinfo};

/// [`aida_core::git_ops::configure_remote_credentials_with`] plus the GitLab
/// fallback: with no token in the stored URL or `url_hint`, a GitLab host
/// falls back to the glab / env token only when the operator has no
/// credential helper for it. Their helper (glab, a store, a keychain) keeps
/// answering and is never overridden.
// trace:BUG-2119 | ai:agy
// trace:BUG-2119 trace:BUG-2181 | ai:claude
pub(crate) fn configure_git_remote_credentials(
    cmd: &mut Command,
    dir: &Path,
    remote: &str,
    url_hint: Option<&str>,
) {
    let gitlab_token = |dir: &Path, clean_url: &str| {
        if crate::forge::detect_forge_kind(clean_url) != crate::forge::ForgeKind::GitLab
            || aida_core::git_ops::operator_credential_helper_configured(dir, clean_url)
        {
            return None;
        }
        let host = crate::forge::forge_host_of(clean_url)?;
        let (token, _) = crate::remote_create::gitlab_token_for_host(&host)?;
        Some(("oauth2".to_string(), token))
    };
    aida_core::git_ops::configure_remote_credentials_with(
        cmd,
        dir,
        remote,
        url_hint,
        Some(&gitlab_token),
    );
}

/// The `git ls-remote --heads <remote>` probe. `GIT_TERMINAL_PROMPT=0` makes an
/// HTTPS URL without stored credentials fail instead of blocking on a
/// username prompt.
// trace:BUG-1972 | ai:claude
// trace:BUG-2004 trace:BUG-2119 | ai:antigravity
pub(crate) fn ls_remote_probe(dir: &Path, remote: &str) -> Command {
    ls_remote_probe_with_url(dir, remote, None)
}

// trace:BUG-2119 | ai:agy
fn ls_remote_probe_with_url(dir: &Path, remote: &str, url_hint: Option<&str>) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["ls-remote", "--heads", remote])
        .env("GIT_TERMINAL_PROMPT", "0");
    configure_git_remote_credentials(&mut cmd, dir, remote, url_hint);
    cmd
}

/// Construct the `git ls-remote` probe command for a GitLab project existence check.
/// Returns None for non-GitLab URLs.
// trace:BUG-2004 trace:BUG-2119 | ai:antigravity
pub(crate) fn mirror_probe_command(dir: &Path, remote_name: &str, url: &str) -> Option<Command> {
    if crate::forge::detect_forge_kind(url) != crate::forge::ForgeKind::GitLab {
        return None;
    }
    Some(ls_remote_probe_with_url(dir, remote_name, Some(url)))
}

/// `aida remote mirror --url`: probe a GitLab URL with `git ls-remote`; when
/// the project is missing (the refusal text), create it. Returns true when a
/// project was created. Non-GitLab URLs and non-refusal probe errors are left
/// alone (the mirror is still wired; the first push reports the real error).
// trace:STORY-1656 | ai:claude
// trace:BUG-2004 trace:BUG-2119 | ai:antigravity
pub(crate) fn ensure_gitlab_project_exists(
    dir: &Path,
    remote_name: &str,
    url: &str,
    public: bool,
) -> Result<bool> {
    let Some(mut cmd) = mirror_probe_command(dir, remote_name, url) else {
        return Ok(false);
    };
    let out = cmd.output().context("could not launch `git ls-remote`")?;
    if out.status.success() {
        return Ok(false);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !is_push_to_create_refusal(&stderr) {
        eprintln!(
            "  {} could not reach {} ({}) — not creating anything",
            crate::glyph(crate::glyphs::Glyph::Warning),
            redact_url_userinfo(url),
            redact_url_userinfo(stderr.trim())
        );
        return Ok(false);
    }
    let Some((host, namespace, name)) = gitlab_target_of_url(url) else {
        return Ok(false);
    };
    create_gitlab_project(&host, &namespace, &name, public)?;
    Ok(true)
}

/// The tip of `branch` on `remote` as the remote itself reports it, or None.
// trace:BUG-2119 | ai:agy
fn remote_tip(dir: &Path, remote: &str, branch: &str) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["ls-remote", remote, &format!("refs/heads/{branch}")])
        .env("GIT_TERMINAL_PROMPT", "0");
    configure_git_remote_credentials(&mut cmd, dir, remote, None);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

fn local_tip(dir: &Path, branch: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Every `(hub, branch)` whose tip on the hub differs from the local tip, as
/// a readable line. Empty means all hubs agree on every branch.
// trace:STORY-1656 | ai:claude
pub(crate) fn hub_disagreements(dir: &Path, hubs: &[String], branches: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for branch in branches {
        let local = local_tip(dir, branch);
        for hub in hubs {
            let remote = remote_tip(dir, hub, branch);
            if remote.is_none() || remote != local {
                out.push(format!(
                    "{hub}/{branch} is {} but local {branch} is {}",
                    remote.as_deref().map(short).unwrap_or("absent"),
                    local.as_deref().map(short).unwrap_or("absent"),
                ));
            }
        }
    }
    out
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

#[cfg(test)]
#[path = "tests/story_1656_multi_forge_init_tests.rs"]
mod story_1656_multi_forge_init_tests;

#[cfg(test)]
#[path = "tests/bug_2004_token_leak_argv_tests.rs"]
mod bug_2004_token_leak_argv_tests;

// trace:BUG-2181 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2181_host_remote_credentials_tests.rs"]
mod bug_2181_host_remote_credentials_tests;

// trace:TASK-2306 | ai:claude
#[cfg(all(test, unix))]
#[path = "tests/task_2270_push_upstream_site_tests.rs"]
mod task_2270_push_upstream_site_tests;

/// Preserve user pipelines, including dangling symlinks, and atomically create only
/// a missing file. JSON strings are also valid YAML scalars for runner tags.
// trace:STORY-2701 | ai:codex
pub(crate) fn scaffold_ci(
    dir: &Path,
    lang: Option<crate::init_bootstrap::Lang>,
    tags: &[String],
) -> Result<()> {
    use crate::init_bootstrap::Lang;
    use std::io::Write;
    let path = dir.join(".gitlab-ci.yml");
    if path.symlink_metadata().is_ok() {
        println!("  GitLab pipeline already exists — preserving it");
        return Ok(());
    }
    let detected = lang.or_else(|| {
        if dir.join("Cargo.toml").is_file() {
            Some(Lang::Rust)
        } else if dir.join("pyproject.toml").is_file() || dir.join("requirements.txt").is_file() {
            Some(Lang::Python)
        } else if dir.join("package.json").is_file() {
            Some(Lang::Node)
        } else {
            None
        }
    });
    // Resolve the full Python constraint rather than guessing a minor version.
    // uv also handles upper bounds, exclusions and future Python releases.
    let mut python_commands = String::new();
    if matches!(detected, Some(Lang::Python)) && dir.join("pyproject.toml").is_file() {
        let metadata: toml::Value =
            toml::from_str(&std::fs::read_to_string(dir.join("pyproject.toml"))?)
                .context("cannot read Python compatibility from pyproject.toml")?;
        if let Some(requirement) = metadata
            .get("project")
            .and_then(|project| project.get("requires-python"))
            .and_then(toml::Value::as_str)
        {
            let quoted = requirement.replace('\'', "'\"'\"'");
            python_commands = format!(
                "    - python -m pip install uv\n    - uv venv --seed --managed-python --python '{quoted}' .aida-ci-venv\n    - export PATH=\"$PWD/.aida-ci-venv/bin:$PATH\"\n"
            );
        }
    }
    let (image, commands) = match detected {
        Some(Lang::Rust) => ("rust:latest", "    - cargo test --locked\n"),
        Some(Lang::Python) => ("python:3.12", "    - python -m pip install pytest\n    - if [ -f requirements.txt ]; then python -m pip install -r requirements.txt; fi\n    - if [ -f pyproject.toml ]; then python -m pip install -e .; fi\n    - python -m pytest\n"),
        Some(Lang::Node) => ("node:22", "    - if [ -f package-lock.json ]; then npm ci; else npm install; fi\n    - npm test\n"),
        None => {
            println!("  GitLab pipeline not generated: language unknown; add .gitlab-ci.yml manually or select --lang");
            return Ok(());
        }
    };
    let commands = format!("{python_commands}{commands}");
    let content = format!("# Generated by aida init; customize for your project.\nimage: {image}\ndefault:\n  tags: {}\nstages: [test]\ntest:\n  stage: test\n  script:\n{commands}", serde_json::to_string(tags)?);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(content.as_bytes())?;
    println!(
        "  Created .gitlab-ci.yml; review dependencies and test commands before relying on CI"
    );
    Ok(())
}

// trace:STORY-2701 | ai:codex
fn ci_enabled(project: &serde_json::Value) -> Result<bool> {
    match project["builds_access_level"].as_str() {
        Some("disabled") => Ok(false),
        Some("enabled" | "private") => Ok(true),
        _ => bail!(
            "project response has missing or unknown builds_access_level; CI/CD state is unknown"
        ),
    }
}

// trace:STORY-2701 | ai:codex
fn runner_matches(runner: &serde_json::Value, tags: &[String]) -> bool {
    runner["status"] == "online"
        && runner["paused"] == false
        // Protected-only runners cannot establish general branch availability.
        && runner["access_level"] == "not_protected"
        && if tags.is_empty() {
            runner["run_untagged"] == true
        } else {
            runner["tag_list"].as_array().is_some_and(|available| {
                tags.iter().all(|tag| available.iter().any(|v| v.as_str() == Some(tag.as_str())))
            })
        }
}

/// Read project state using host-scoped credentials. Only explicit flag consent
/// or an affirmative interactive answer authorizes the project-level PUT.
// trace:STORY-2701 | ai:codex
pub(crate) fn check_project_ci(url: &str, consent: bool, tags: &[String]) -> Result<()> {
    use std::io::{IsTerminal, Write};
    let (host, namespace, name) =
        gitlab_target_of_url(url).context("cannot resolve GitLab project")?;
    let (token, _) = crate::remote_create::gitlab_token_for_host(&host)
        .context("CI/CD state unknown: authenticate glab for this host")?;
    let project = if namespace.is_empty() {
        name
    } else {
        format!("{namespace}/{name}")
    };
    let endpoint = format!("projects/{}", encode_path(&project));
    let api = |endpoint: &str, extra: &[&str]| -> Result<serde_json::Value> {
        let mut args = vec![
            "api".into(),
            "--hostname".into(),
            host.clone(),
            endpoint.into(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        let mut command = Command::new(glab_binary());
        command.args(&args).env("GITLAB_TOKEN", &token);
        let output =
            crate::command_output_with_timeout(command, std::time::Duration::from_secs(15))
                .context("GitLab CI/CD probe failed to launch or timed out")?;
        if !output.status.success() {
            bail!(
                "GitLab CI/CD API refused {}: {}",
                endpoint,
                redact_url_userinfo(String::from_utf8_lossy(&output.stderr).trim())
            );
        }
        serde_json::from_slice(&output.stdout).context("malformed GitLab CI/CD response")
    };
    let state = api(&endpoint, &[])?;
    if !ci_enabled(&state)? {
        println!("  GitLab project CI/CD is disabled");
        let mut approved = consent;
        if !approved
            && !crate::autopilot::current_headless()
            && std::io::stdin().is_terminal()
            && std::io::stdout().is_terminal()
        {
            print!("Enable CI/CD for {host}/{project}? [y/N] ");
            std::io::stdout().flush()?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            approved = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
        }
        if approved {
            api(
                &endpoint,
                &["--method", "PUT", "--field", "builds_access_level=enabled"],
            )?;
            if !ci_enabled(&api(&endpoint, &[])?)? {
                bail!("GitLab retained disabled CI/CD after the requested change");
            }
            println!("  Enabled project CI/CD; start a pipeline for the pushed branch in GitLab");
        } else {
            println!("  CI/CD remains disabled; re-run init with --gitlab-enable-ci to consent, or ask an administrator");
        }
    } else {
        println!("  GitLab project CI/CD is enabled");
    }
    // Fetch runner details: the project list alone omits scheduling attributes.
    let runners = api(&format!("{endpoint}/runners?per_page=100"), &[])?;
    let rows = runners
        .as_array()
        .context("malformed project runner list")?;
    let mut unknown = Vec::new();
    for row in rows {
        let Some(id) = row["id"].as_u64() else {
            unknown.push("runner entry missing id".to_string());
            continue;
        };
        let detail = match api(&format!("runners/{id}"), &[]) {
            Ok(detail) => detail,
            Err(error) => {
                let diagnostic = format!("runner {id} availability unknown: {error:#}");
                eprintln!("  {diagnostic}");
                unknown.push(diagnostic);
                continue;
            }
        };
        if runner_matches(&detail, tags) {
            println!("  Verified an online, unpaused GitLab runner matching tags {tags:?}; pipeline execution remains unverified");
            return Ok(());
        }
    }
    if !unknown.is_empty() {
        bail!(
            "no compatible runner verified; runner availability remains unknown: {}",
            unknown.join("; ")
        );
    }
    bail!("no compatible runner verified among the first 100 project runners for tags {tags:?}; check runner assignment, tags, protection and group/instance restrictions")
}

#[cfg(test)]
mod story_2701_tests {
    use super::*;

    #[test]
    fn gitlab_ci_api_requires_consent_and_preserves_policy_refusals() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("glab");
        let log = dir.path().join("calls");
        let state = dir.path().join("enabled");
        std::fs::write(&script, r#"#!/bin/sh
printf '%s\n' "$*" >> "$CI_TEST_LOG"
case "$*" in
  *PUT*)
    if [ -f "$CI_TEST_STATE.refuse" ]; then echo '403 group policy forbids CI' >&2; exit 1; fi
    touch "$CI_TEST_STATE"; echo '{}';;
  *'runners/6'*) echo '403 inherited runner details forbidden' >&2; exit 1 ;;
  *'runners/7'*) if [ -f "$CI_TEST_STATE.unknown" ]; then echo '503 runner unavailable' >&2; exit 1; fi; echo '{"status":"online","paused":false,"access_level":"not_protected","run_untagged":true,"tag_list":[]}' ;;
  *'/runners?'*) echo '[{"id":6},{"id":7}]' ;;
  *) if [ -f "$CI_TEST_STATE" ]; then echo '{"builds_access_level":"enabled"}'; else echo '{"builds_access_level":"disabled"}'; fi ;;
esac
"#).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_TEST_GLAB_BINARY", script.to_str().unwrap()),
            ("GITLAB_TOKEN", "fake-ci-test-token"),
            ("HOME", dir.path().to_str().unwrap()),
            ("AIDA_TEST_HOME", dir.path().to_str().unwrap()),
            ("AIDA_HEADLESS", "1"),
            ("CI_TEST_LOG", log.to_str().unwrap()),
            ("CI_TEST_STATE", state.to_str().unwrap()),
        ]);
        let url = "git@gitlab.example.test:group/demo.git";
        check_project_ci(url, false, &[]).unwrap();
        assert!(!std::fs::read_to_string(&log).unwrap().contains("PUT"));
        check_project_ci(url, true, &[]).unwrap();
        assert!(state.exists());
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("--hostname gitlab.example.test projects/group%2Fdemo"));
        assert_eq!(calls.matches("PUT").count(), 1);
        check_project_ci(url, true, &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log)
                .unwrap()
                .matches("PUT")
                .count(),
            1
        );
        std::fs::write(state.with_extension("unknown"), "").unwrap();
        let error = check_project_ci(url, false, &[]).unwrap_err().to_string();
        assert!(error.contains("runner 6 availability unknown"), "{error}");
        assert!(error.contains("403 inherited runner"), "{error}");
        assert!(error.contains("runner 7 availability unknown"), "{error}");
        std::fs::remove_file(&state).unwrap();
        std::fs::write(state.with_extension("refuse"), "").unwrap();
        let error = check_project_ci(url, true, &[]).unwrap_err().to_string();
        assert!(error.contains("403 group policy"), "{error}");
        assert!(!state.exists());
    }

    #[test]
    fn gitlab_ci_state_and_runner_require_positive_evidence() {
        assert!(!ci_enabled(&serde_json::json!({"builds_access_level":"disabled"})).unwrap());
        assert!(ci_enabled(&serde_json::json!({"builds_access_level":"private"})).unwrap());
        assert!(ci_enabled(&serde_json::json!({})).is_err());
        let mut runner = serde_json::json!({"status":"online", "paused":false,
            "access_level":"not_protected", "run_untagged":false, "tag_list":["linux", "docker"]});
        assert!(runner_matches(&runner, &["docker".into()]));
        assert!(!runner_matches(&runner, &[]));
        assert!(!runner_matches(&runner, &["gpu".into()]));
        runner["paused"] = true.into();
        assert!(!runner_matches(&runner, &["docker".into()]));
        assert!(!runner_matches(&serde_json::json!({}), &[]));
    }

    #[test]
    fn gitlab_ci_flags_require_explicit_gitlab_target() {
        assert!(crate::TEST_CLI
            .try_parse_from([
                "aida",
                "init",
                "demo",
                "--gitlab",
                "gitlab.example.test/group",
                "--gitlab-enable-ci",
                "--gitlab-runner-tags",
                "linux,docker"
            ])
            .is_ok());
        assert!(crate::TEST_CLI
            .try_parse_from(["aida", "init", "demo", "--gitlab-enable-ci"])
            .is_err());
    }

    #[test]
    fn gitlab_ci_scaffold_supports_rust_node_and_preserves_dangling_link() {
        for (manifest, expected) in [
            ("Cargo.toml", "cargo test --locked"),
            ("package.json", "npm test"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(manifest), "").unwrap();
            scaffold_ci(dir.path(), None, &[]).unwrap();
            assert!(std::fs::read_to_string(dir.path().join(".gitlab-ci.yml"))
                .unwrap()
                .contains(expected));
        }
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("missing");
        std::os::unix::fs::symlink(&target, dir.path().join(".gitlab-ci.yml")).unwrap();
        scaffold_ci(dir.path(), Some(crate::init_bootstrap::Lang::Python), &[]).unwrap();
        assert!(!target.exists());
    }

    // trace:STORY-2701 | ai:codex
    #[test]
    fn gitlab_ci_scaffold_resolves_newer_python_requirements() {
        for requirement in [">=3.13", ">=3.14,<3.15", "==3.13.*"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("pyproject.toml"),
                format!("[project]\nrequires-python = {requirement:?}\n"),
            )
            .unwrap();
            scaffold_ci(dir.path(), None, &[]).unwrap();
            let content = std::fs::read_to_string(dir.path().join(".gitlab-ci.yml")).unwrap();
            assert!(content.contains(&format!("--managed-python --python '{requirement}'")));
            assert!(
                content.find("export PATH=").unwrap() < content.find("pip install -e .").unwrap()
            );
        }
    }

    #[test]
    fn gitlab_ci_scaffold_detects_python_quotes_tags_and_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        scaffold_ci(dir.path(), None, &["linux: docker".into()]).unwrap();
        let path = dir.path().join(".gitlab-ci.yml");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("python -m pytest"));
        assert!(content.contains("[\"linux: docker\"]"));
        std::fs::write(&path, "custom pipeline").unwrap();
        scaffold_ci(dir.path(), None, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "custom pipeline");
    }
}
