//! The store's declared hubs: the git servers claims are ordered on.
//!
//! A hub is declared in the tracked `.aida/config.toml` under
//! `[store.hubs]`, identified by URL (remote names are local to each clone):
//!
//! ```toml
//! [store.hubs]
//! github = { urls = ["github.com/joemooney/aida"] }
//! ```
//!
//! One declared hub is single-hub mode: the hub's `aida-store` ref orders
//! every block claim by compare-and-swap push, so no root clone is needed.
//! A second declared hub requires an enrolled root (`aida node enroll-root`).
//! Remotes listed in `[store.sync] mirror_remotes` are never hubs.
//!
//! The list counts only once it is on the shared code default branch (this
//! clone's remote-tracking copy of it): two clones each guessing "my
//! `origin` is the hub", or each committing a different hub locally, would
//! mint on two refs.
//! The list grants no authority by itself; authority is still push
//! permission on the hub ref.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Prefix of the pool id stamped on every block claimed in single-hub mode:
/// `hub:<hub>:<TYPE>`.
pub const HUB_POOL_PREFIX: &str = "hub:";

/// One declared hub: its name and the normalized URLs that reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hub {
    pub name: String,
    pub urls: Vec<String>,
}

/// The declared hubs plus the configured mirror remote names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HubSet {
    pub hubs: Vec<Hub>,
    pub mirror_remotes: Vec<String>,
}

impl HubSet {
    /// Parse `[store.hubs]` and `[store.sync] mirror_remotes` from the text
    /// of a `.aida/config.toml`. A hub value is `{ urls = [..] }`,
    /// `{ url = ".." }`, or a bare URL string.
    pub fn parse(text: &str) -> Result<Self> {
        let table: toml::Table = toml::from_str(text).context("invalid .aida/config.toml")?;
        let store = table.get("store").and_then(toml::Value::as_table);
        let mut hubs = Vec::new();
        if let Some(declared) = store
            .and_then(|s| s.get("hubs"))
            .and_then(toml::Value::as_table)
        {
            for (name, value) in declared {
                anyhow::ensure!(
                    valid_hub_name(name),
                    "[store.hubs] `{name}` is not a valid hub name (letters, digits, `-`, `_`)"
                );
                let raw: Vec<String> = match value {
                    toml::Value::String(url) => vec![url.clone()],
                    toml::Value::Table(t) => {
                        let mut urls: Vec<String> = t
                            .get("urls")
                            .and_then(toml::Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if let Some(url) = t.get("url").and_then(toml::Value::as_str) {
                            urls.push(url.to_string());
                        }
                        urls
                    }
                    _ => anyhow::bail!("[store.hubs] `{name}` must be a URL or {{ urls = [..] }}"),
                };
                let urls: Vec<String> = raw
                    .iter()
                    .map(|u| normalize_url(u))
                    .filter(|u| !u.is_empty())
                    .collect();
                anyhow::ensure!(!urls.is_empty(), "[store.hubs] `{name}` names no URL");
                hubs.push(Hub {
                    name: name.clone(),
                    urls,
                });
            }
        }
        let mirror_remotes = store
            .and_then(|s| s.get("sync"))
            .and_then(|s| s.get("mirror_remotes"))
            .map(|v| match v {
                toml::Value::String(s) => vec![s.clone()],
                toml::Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        Ok(Self {
            hubs,
            mirror_remotes,
        })
    }

    pub fn get(&self, name: &str) -> Option<&Hub> {
        self.hubs.iter().find(|h| h.name == name)
    }

    /// The hub whose URLs include `url` (normalized first).
    pub fn hub_for_url(&self, url: &str) -> Option<&Hub> {
        let url = normalize_url(url);
        self.hubs.iter().find(|h| h.urls.contains(&url))
    }
}

/// A hub name usable inside a pool id: plain, bounded, no `:`.
pub fn valid_hub_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Reduce a git URL to `host/path` so one server reached over ssh, https,
/// or the scp form counts as one hub: the scheme, user, port, a trailing
/// `/` and `.git` are dropped and the host is lowercased. A local path
/// keeps its path (with `file://` and `.git` dropped).
pub fn normalize_url(url: &str) -> String {
    let url = url.trim();
    let (authority_and_path, had_scheme) = match url.split_once("://") {
        Some((_, rest)) => (rest, true),
        None => (url, false),
    };
    let strip = |s: &str| -> String {
        let s = s.trim_end_matches('/');
        let s = s.strip_suffix(".git").unwrap_or(s);
        s.trim_end_matches('/').to_string()
    };
    if had_scheme {
        // file:///path, or host[:port]/path with optional user@.
        if authority_and_path.starts_with('/') {
            return strip(authority_and_path);
        }
        let (authority, path) = authority_and_path
            .split_once('/')
            .unwrap_or((authority_and_path, ""));
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = match host.rsplit_once(':') {
            Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
            _ => host,
        };
        return strip(&format!("{}/{}", host.to_ascii_lowercase(), path));
    }
    if url.starts_with('/') || url.starts_with('.') {
        return strip(url);
    }
    // scp form: [user@]host:path
    if let Some((authority, path)) = url.split_once(':') {
        if !authority.contains('/') {
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            return strip(&format!(
                "{}/{}",
                host.to_ascii_lowercase(),
                path.trim_start_matches('/')
            ));
        }
    }
    strip(url)
}

/// `hub:<hub>:<TYPE>`: the pool a single-hub claim is ordered against.
pub fn hub_pool_id(hub: &str, type_prefix: &str) -> String {
    format!(
        "{HUB_POOL_PREFIX}{hub}:{}",
        type_prefix.to_ascii_uppercase()
    )
}

/// `(hub, TYPE)` of a `hub:<hub>:<TYPE>` pool id.
pub fn parse_hub_pool_id(pool_id: &str) -> Option<(&str, &str)> {
    let rest = pool_id.strip_prefix(HUB_POOL_PREFIX)?;
    let (hub, typ) = rest.split_once(':')?;
    (valid_hub_name(hub) && !typ.is_empty() && !typ.contains(':')).then_some((hub, typ))
}

/// The directory whose git repository holds the code branches for
/// `store_path`: the project root next to a `.aida-store` worktree, else
/// the store itself (a store worktree shares its repository).
fn code_repo_for(store_path: &Path) -> PathBuf {
    store_path
        .parent()
        .filter(|p| p.join(".aida/config.toml").exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| store_path.to_path_buf())
}

/// The hubs declared in the `.aida/config.toml` of the shared code default
/// branch: this clone's remote-tracking copy of it (the branch's configured
/// upstream, else `origin/<default>`). A declaration committed only on a
/// local branch (never pushed, never reviewed) does not count: two clones
/// could each declare a different hub locally and mint on two refs. Empty
/// when no remote-tracking default branch or file can be read.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
pub fn committed(store_path: &Path) -> Result<HubSet> {
    let repo = code_repo_for(store_path);
    let branch = crate::git_ops::default_branch_name(&repo);
    let mut revs = Vec::new();
    if let Ok(out) = crate::git_ops::host_git_command(&repo)
        .args([
            "rev-parse",
            "--symbolic-full-name",
            &format!("refs/heads/{branch}@{{upstream}}"),
        ])
        .output()
    {
        let upstream = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && upstream.starts_with("refs/remotes/") {
            revs.push(upstream);
        }
    }
    revs.push(format!("refs/remotes/origin/{branch}"));
    for rev in revs {
        let out = crate::git_ops::host_git_command(&repo)
            .args(["show", &format!("{rev}:.aida/config.toml")])
            .output();
        if let Ok(out) = out {
            if out.status.success() {
                return HubSet::parse(&String::from_utf8_lossy(&out.stdout));
            }
        }
    }
    Ok(HubSet::default())
}

/// The hubs declared in the working copy of `.aida/config.toml` (which may
/// not be committed yet).
pub fn working(store_path: &Path) -> Result<HubSet> {
    let path = code_repo_for(store_path).join(".aida/config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => HubSet::parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HubSet::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Every URL `remote` fetches from or pushes to, as configured in `repo`.
pub fn remote_urls(repo: &Path, remote: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for args in [
        &["remote", "get-url", "--all", "--end-of-options", remote][..],
        &[
            "remote",
            "get-url",
            "--push",
            "--all",
            "--end-of-options",
            remote,
        ][..],
    ] {
        if let Ok(out) = crate::git_ops::host_git_command(repo).args(args).output() {
            if out.status.success() {
                urls.extend(
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string),
                );
            }
        }
    }
    urls
}

/// The first non-mirror remote of `repo` that reaches `hub` (by any fetch
/// or push URL).
pub fn remote_reaching(repo: &Path, hubs: &HubSet, hub: &Hub) -> Option<String> {
    crate::git_ops::list_remotes(repo)
        .into_iter()
        .filter(|r| !hubs.mirror_remotes.contains(r))
        .find(|r| {
            remote_urls(repo, r)
                .iter()
                .any(|u| hub.urls.contains(&normalize_url(u)))
        })
}

/// Config errors a clone can see: a remote listed as a store mirror whose
/// URL is also a declared hub (a remote cannot be both).
pub fn config_conflicts(repo: &Path, hubs: &HubSet) -> Vec<String> {
    let mut out = Vec::new();
    for mirror in &hubs.mirror_remotes {
        for url in remote_urls(repo, mirror) {
            if let Some(hub) = hubs.hub_for_url(&url) {
                let line = format!(
                    "remote `{mirror}` is listed in [store.sync] mirror_remotes but its URL {url} is declared hub `{}` in [store.hubs]; a remote cannot be both a mirror and a hub",
                    hub.name
                );
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/store_hubs_tests.rs"]
mod tests;
