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
fn git_push(dir: &Path, args: &[&str]) -> Result<std::result::Result<(), String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("push")
        .args(args)
        .output()
        .context("could not launch `git push`")?;
    if out.status.success() {
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
    let stderr = match git_push(dir, &args)? {
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
    match git_push(dir, &args)? {
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

/// The `git ls-remote --heads <url>` probe. `GIT_TERMINAL_PROMPT=0` makes an
/// HTTPS URL without stored credentials fail instead of blocking on a
/// username prompt.
// trace:BUG-1972 | ai:claude
fn ls_remote_probe(dir: &Path, url: &str) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["ls-remote", "--heads", url])
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// `aida remote mirror --url`: probe a GitLab URL with `git ls-remote`; when
/// the project is missing (the refusal text), create it. Returns true when a
/// project was created. Non-GitLab URLs and non-refusal probe errors are left
/// alone (the mirror is still wired; the first push reports the real error).
// trace:STORY-1656 | ai:claude
pub(crate) fn ensure_gitlab_project_exists(dir: &Path, url: &str, public: bool) -> Result<bool> {
    if crate::forge::detect_forge_kind(url) != crate::forge::ForgeKind::GitLab {
        return Ok(false);
    }
    let out = ls_remote_probe(dir, url)
        .output()
        .context("could not launch `git ls-remote`")?;
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
fn remote_tip(dir: &Path, remote: &str, branch: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-remote", remote, &format!("refs/heads/{branch}")])
        .output()
        .ok()?;
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
