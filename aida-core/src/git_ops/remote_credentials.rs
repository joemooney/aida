//! The one credential gate for host git calls that contact a remote.
//!
//! An operator may store an HTTP(S) remote as `https://oauth2:<token>@host/…`.
//! Handing git that remote by name makes git pass the stored URL, token and
//! all, to `git-remote-http(s)` in `argv`, where `ps` and
//! `/proc/<pid>/cmdline` show it to every local user. Every fetch, pull, push
//! and ls-remote against a named remote goes through
//! [`configure_remote_credentials`] (or [`host_remote_git_command`]), which
//! rewrites the URL to its credential-free form for that one invocation and
//! supplies the token through an environment-backed credential helper.
// trace:BUG-2119 | ai:agy
// trace:BUG-2181 | ai:claude

use std::path::Path;
use std::process::Command;

/// Credential helper shell snippet configured via `GIT_CONFIG_KEY_*` /
/// `GIT_CONFIG_VALUE_*`. It reads the username and token from environment
/// variables (`AIDA_GIT_CREDENTIAL_USER` / `AIDA_GIT_CREDENTIAL_TOKEN`), so
/// neither `git`, `git-remote-http`, nor `/bin/sh` ever carries the token in
/// `argv` (`ps` / `/proc/<pid>/cmdline`).
// trace:BUG-2119 | ai:agy
pub const GIT_CREDENTIAL_HELPER_SCRIPT: &str =
    "!f() { test \"$1\" = get && printf 'username=%s\\npassword=%s\\n' \"${AIDA_GIT_CREDENTIAL_USER:-oauth2}\" \"$AIDA_GIT_CREDENTIAL_TOKEN\"; }; f";

/// A credential source consulted only when neither the stored URL nor the
/// URL hint carries a token. Called with the repository and the clean remote
/// URL; returns `(username, token)`.
// trace:BUG-2181 | ai:claude
pub type FallbackCredentials<'a> = &'a dyn Fn(&Path, &str) -> Option<(String, String)>;

/// [`super::host_git_command`] for a call that contacts `remote`, with the
/// remote's credentials kept out of `argv` ([`configure_remote_credentials`]).
// trace:BUG-2181 | ai:claude
pub fn host_remote_git_command(cwd: impl AsRef<Path>, remote: &str) -> Command {
    let cwd = cwd.as_ref();
    let mut cmd = super::host_git_command(cwd);
    configure_remote_credentials(&mut cmd, cwd, remote);
    cmd
}

/// Chainable [`configure_remote_credentials`] for a `Command` builder:
/// `Command::new("git").args(["fetch", "origin"]).remote_credentials(dir, "origin")`.
// trace:BUG-2181 | ai:claude
pub trait RemoteCredentialsExt {
    /// Keep `remote`'s stored credentials out of every child's `argv`.
    fn remote_credentials(&mut self, dir: &Path, remote: &str) -> &mut Self;
}

impl RemoteCredentialsExt for Command {
    fn remote_credentials(&mut self, dir: &Path, remote: &str) -> &mut Self {
        configure_remote_credentials(self, dir, remote);
        self
    }
}

/// Configure `cmd`, a git invocation in `dir` that contacts `remote`, so
/// `git-remote-http` receives a credential-free URL in `argv`.
///
/// For each stored `remote.<remote>.url` / `pushurl` carrying `user:token@`,
/// this one invocation gets a `url.<clean>.insteadOf=<credentialed>` override
/// (through `GIT_CONFIG_*`, never argv), and the embedded token feeds a
/// helper scoped to that host. The persisted configuration is never
/// modified, so the credential the operator stored keeps working for later
/// aida and plain `git` runs. A clean remote, an SSH remote or a URL
/// argument leaves `cmd` unchanged.
// trace:BUG-2181 | ai:claude
pub fn configure_remote_credentials(cmd: &mut Command, dir: &Path, remote: &str) {
    configure_remote_credentials_with(cmd, dir, remote, None, None);
}

/// [`configure_remote_credentials`] with a `url_hint` and a `fallback`.
/// The hint is the URL the caller is wiring: its userinfo supplies the token
/// when the stored URL is clean, and its clean form is the scope when
/// `remote` has no stored URL. The fallback is consulted only when neither
/// URL carries a token.
// trace:BUG-2119 | ai:agy
// trace:BUG-2119 trace:BUG-2181 | ai:claude
pub fn configure_remote_credentials_with(
    cmd: &mut Command,
    dir: &Path,
    remote: &str,
    url_hint: Option<&str>,
    fallback: Option<FallbackCredentials<'_>>,
) {
    let is_named_remote = !remote.is_empty() && !remote.contains("://") && !remote.contains('@');
    let stored_urls = if is_named_remote {
        configured_remote_urls(dir, remote)
    } else {
        Vec::new()
    };
    // TASK-1924: each stored token is paired with its OWN URL's host scope,
    // so a url on host A and a credentialed pushurl on host B never offer
    // B's token to A, and B keeps its token. The first token per host wins.
    // trace:TASK-1952 | trace:TASK-1924 | ai:claude
    let mut clean_stored: Option<String> = None;
    let mut scoped: Vec<(String, (String, String))> = Vec::new();
    for stored in &stored_urls {
        let (clean, creds) = split_http_url_userinfo(stored);
        if let Some(creds) = creds {
            // Only a credentialed URL is rewritten: a clean stored URL needs
            // no override, and appending one would duplicate the push target.
            append_git_config_env(cmd, &format!("url.{clean}.insteadOf"), stored);
            if let Some(scope) = http_credential_scope(&clean) {
                if !scoped.iter().any(|(s, _)| *s == scope) {
                    scoped.push((scope, creds));
                }
            }
        }
        clean_stored.get_or_insert(clean);
    }
    let (clean_hint, hint_creds) = url_hint
        .map(split_http_url_userinfo)
        .unwrap_or_else(|| (String::new(), None));
    let clean_url = clean_stored.unwrap_or(clean_hint);
    let primary = http_credential_scope(&clean_url);

    // A token from the URL is the one git used before argv was cleaned, so
    // it keeps precedence: the empty value resets helpers for that host so
    // ours answers first, as the URL credential did. The hint's token
    // belongs to the primary (first stored, else hinted) URL's host.
    let mut helpers: Vec<(String, (String, String), bool)> = Vec::new();
    if let Some(scope) = &primary {
        let from_url = hint_creds.or_else(|| {
            scoped
                .iter()
                .find(|(s, _)| s == scope)
                .map(|(_, c)| c.clone())
        });
        match from_url {
            Some(creds) => helpers.push((scope.clone(), creds, true)),
            None => {
                if let Some(creds) = fallback.and_then(|f| f(dir, &clean_url)) {
                    helpers.push((scope.clone(), creds, false));
                }
            }
        }
    }
    for (scope, creds) in scoped {
        if !helpers.iter().any(|(s, _, _)| *s == scope) {
            helpers.push((scope, creds, true));
        }
    }
    for (index, (scope, (user, token), reset_operator_helpers)) in helpers.into_iter().enumerate() {
        // Each host reads its own pair of variables; the first keeps the
        // unsuffixed names.
        let suffix = if index == 0 {
            String::new()
        } else {
            format!("_{index}")
        };
        cmd.env(format!("AIDA_GIT_CREDENTIAL_USER{suffix}"), user);
        cmd.env(format!("AIDA_GIT_CREDENTIAL_TOKEN{suffix}"), token);
        // Scoped to this host only: a redirect or any other credential
        // request in the same git run never sees the token.
        let key = format!("credential.{scope}.helper");
        if reset_operator_helpers {
            append_git_config_env(cmd, &key, "");
        }
        append_git_config_env(cmd, &key, &credential_helper_script(&suffix));
    }
}

/// [`GIT_CREDENTIAL_HELPER_SCRIPT`] reading the variables named with
/// `suffix` (`""` is the unsuffixed pair).
// trace:TASK-1952 | trace:TASK-1924 | ai:claude
fn credential_helper_script(suffix: &str) -> String {
    if suffix.is_empty() {
        return GIT_CREDENTIAL_HELPER_SCRIPT.to_string();
    }
    GIT_CREDENTIAL_HELPER_SCRIPT
        .replace(
            "AIDA_GIT_CREDENTIAL_USER:",
            &format!("AIDA_GIT_CREDENTIAL_USER{suffix}:"),
        )
        .replace(
            "$AIDA_GIT_CREDENTIAL_TOKEN\"",
            &format!("$AIDA_GIT_CREDENTIAL_TOKEN{suffix}\""),
        )
}

/// The remote the current branch pulls from (`branch.<name>.remote`), for a
/// `git pull` / `git fetch` that names no remote; `origin` when HEAD is
/// detached or the branch has no upstream, which is what git itself uses.
// trace:BUG-2181 | ai:claude
pub fn upstream_remote(dir: &Path) -> String {
    let stdout = |args: &[&str]| {
        super::host_git_command(dir)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    stdout(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .and_then(|branch| stdout(&["config", "--get", &format!("branch.{branch}.remote")]))
        .filter(|remote| remote != ".")
        .unwrap_or_else(|| "origin".to_string())
}

/// True when the operator's git config already names a credential helper for
/// `url` (`credential.helper` or a matching `credential.<url>.helper`, after
/// any empty-value reset).
// trace:BUG-2119 | ai:claude
pub fn operator_credential_helper_configured(dir: &Path, url: &str) -> bool {
    let mut cmd = if dir.is_dir() {
        super::host_git_command(dir)
    } else {
        Command::new("git")
    };
    cmd.args(["config", "--get-urlmatch", "credential.helper", url])
        .output()
        .is_ok_and(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
}

/// Decode `%XX` escapes in URL userinfo. A malformed escape (anything but two
/// ASCII hex digits, so `%+1` too) is kept verbatim.
// trace:BUG-2119 | ai:claude
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Strip an embedded `user:secret@` from an `http://` or `https://` URL,
/// returning `(clean_url, Option<(username, password)>)` with both parts
/// percent-decoded. Only a non-empty password counts as a credential: a
/// username-only userinfo (`joe@`, `joe:@`) carries no secret, so the URL is
/// returned unchanged with `None`. Non-HTTP(S) URLs (such as
/// `ssh://git@host/...` or `git@host:...`) are also returned unchanged.
// trace:BUG-2119 | ai:agy
// trace:BUG-2119 | ai:claude
pub fn split_http_url_userinfo(url: &str) -> (String, Option<(String, String)>) {
    let trimmed = url.trim();
    let Some((scheme, tail)) = trimmed.split_once("://") else {
        return (url.to_string(), None);
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return (url.to_string(), None);
    }
    let authority_end = tail
        .find(|c: char| c == '/' || c.is_whitespace())
        .unwrap_or(tail.len());
    let Some(at) = tail[..authority_end].rfind('@') else {
        return (url.to_string(), None);
    };
    let Some((user, pass)) = tail[..at].split_once(':').filter(|(_, p)| !p.is_empty()) else {
        return (url.to_string(), None);
    };
    let user = if user.is_empty() {
        "oauth2".to_string()
    } else {
        percent_decode(user)
    };
    let clean_url = format!("{scheme}://{}", &tail[at + 1..]);
    (clean_url, Some((user, percent_decode(pass))))
}

/// `scheme://host[:port]` of an HTTP(S) URL, any userinfo dropped: the
/// `credential.<url>.*` scope that matches this host and no other.
// trace:BUG-2119 | ai:claude
fn http_credential_scope(url: &str) -> Option<String> {
    let (scheme, tail) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = &tail[..tail.find('/').unwrap_or(tail.len())];
    let host = authority.rsplit('@').next().unwrap_or(authority);
    (!host.is_empty()).then(|| format!("{}://{host}", scheme.to_ascii_lowercase()))
}

/// Append one `GIT_CONFIG_KEY_n` / `GIT_CONFIG_VALUE_n` pair after any pairs
/// already set on `cmd` or, when `cmd` sets none, inherited from this process
/// (a CI `safe.directory`, say), so neither is overwritten.
// trace:BUG-2119 | ai:agy
// trace:BUG-2119 | ai:claude
fn append_git_config_env(cmd: &mut Command, key: &str, value: &str) {
    let parse = |s: &str| s.trim().parse::<usize>().ok();
    let explicit = cmd
        .get_envs()
        .find(|(k, _)| *k == "GIT_CONFIG_COUNT")
        .map(|(_, v)| v.and_then(|s| s.to_str()).and_then(parse));
    let current_count = match explicit {
        Some(count) => count.unwrap_or(0),
        None => std::env::var("GIT_CONFIG_COUNT")
            .ok()
            .as_deref()
            .and_then(parse)
            .unwrap_or(0),
    };
    cmd.env(format!("GIT_CONFIG_KEY_{current_count}"), key);
    cmd.env(format!("GIT_CONFIG_VALUE_{current_count}"), value);
    cmd.env("GIT_CONFIG_COUNT", (current_count + 1).to_string());
}

/// Every `remote.<name>.url` and `remote.<name>.pushurl` value exactly as
/// configured (before any `insteadOf` rewrite), in git's order.
// trace:BUG-2119 | ai:claude
fn configured_remote_urls(dir: &Path, remote: &str) -> Vec<String> {
    let out = super::host_git_command(dir)
        .args([
            "config",
            "--get-regexp",
            &format!("^remote\\.{}\\.(url|pushurl)$", regex_escape(remote)),
        ])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
        .collect()
}

/// Escape POSIX ERE metacharacters so a remote name matches literally.
// trace:BUG-2119 | ai:claude
fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let special = "\\^$.|?*+()[]{}".contains(c);
            special
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/remote_credentials_tests.rs"]
mod remote_credentials_tests;
