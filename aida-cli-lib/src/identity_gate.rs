//! TASK-1330 — identity hygiene gate.
//!
//! 2026-10-05: an employer-identifying work email reached public repo history
//! (438 author/committer entries across refs plus Co-authored-by trailers in
//! squash-merge bodies) because a work machine's git config used the employer
//! address and nothing refused it. This module is the recurrence guard:
//!
//! 1. a project allowlist of author/committer emails
//!    (`[identity] allowed_emails` in `.aida/config.toml`);
//! 2. `aida identity check-push` — the fail-closed pre-push gate that refuses
//!    a push introducing a non-allowlisted email in any commit's author,
//!    committer, or `Co-authored-by:` trailers (wired into the pre-push hook
//!    by `aida identity install-hook`, and embedded in the mirror fan-out
//!    hook so the two never need separate files);
//! 3. an `aida commit` enforcement point (`enforce_at_commit`) mirroring the
//!    advisor-code / locking gate pattern;
//! 4. the `aida pr ship` squash-body sanitizer
//!    (`squash_coauthor_body_override`) that strips/replaces non-allowlisted
//!    co-author trailers instead of letting the forge auto-derive them.
//!
//! No allowlist configured = every check is silent (the gate is opt-in per
//! project). Once configured, push-time checks are fail-closed: an error
//! inspecting the commits refuses the push rather than letting it through.
//!
//! trace:TASK-1330 | ai:claude

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::Command;

/// First comment line of the standalone identity pre-push hook — the
/// installer recognizes its own generated hook by this banner (same pattern
/// as `remote_create::MIRROR_HOOK_HEADER`).
pub(crate) const IDENTITY_HOOK_HEADER: &str =
    "# Identity hygiene pre-push hook — installed by `aida identity install-hook`.";

/// The line a custom pre-push hook must carry for the gate to run. Doctor's
/// `identity` scan looks for this marker, so a hand-rolled hook that pipes
/// the ref lines through `aida identity check-push` also counts as covered.
pub(crate) const IDENTITY_HOOK_MARKER: &str = "aida identity check-push";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Read `[identity] allowed_emails` from the project's `.aida/config.toml`.
///
/// Returns an empty vec when the file, section, or key is absent (gate
/// disabled). A bare string is tolerated as single-entry shorthand, matching
/// the `[store.sync] mirror_remotes` convention. A config file that exists
/// but does not parse is an `Err` — push-time callers treat that as refuse
/// (fail-closed), because a mangled config must not silently disable the gate.
// trace:TASK-1330 | ai:claude
pub(crate) fn read_allowed_emails(project_root: &Path) -> Result<Vec<String>> {
    let path = crate::config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let value: toml::Value = toml::from_str(&body)
        .map_err(|err| anyhow::anyhow!(crate::config_parse_error_message(&path, &body, &err)))?;
    let Some(identity) = value.get("identity") else {
        return Ok(Vec::new());
    };
    let emails = match identity.get("allowed_emails") {
        Some(toml::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(toml::Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        _ => Vec::new(),
    };
    Ok(emails)
}

/// Case-insensitive allowlist membership (emails are case-insensitive in
/// practice and git preserves whatever the author typed).
pub(crate) fn email_allowed(email: &str, allowlist: &[String]) -> bool {
    let email = email.trim();
    allowlist
        .iter()
        .any(|a| a.trim().eq_ignore_ascii_case(email))
}

// ---------------------------------------------------------------------------
// Commit identity extraction
// ---------------------------------------------------------------------------

/// One commit's identity-bearing fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitIdentity {
    pub(crate) sha: String,
    pub(crate) author_email: String,
    pub(crate) committer_email: String,
    /// Full commit message (subject + body), for `Co-authored-by:` trailers.
    pub(crate) message: String,
}

/// One non-allowlisted email found in a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdentityViolation {
    pub(crate) sha: String,
    /// "author" | "committer" | "Co-authored-by"
    pub(crate) role: &'static str,
    pub(crate) email: String,
}

/// Extract the email addresses from `Co-authored-by:` trailer lines
/// (case-insensitive, `Co-Authored-By` included). Lines without an
/// `<email>` part are skipped.
pub(crate) fn coauthor_trailer_emails(message: &str) -> Vec<String> {
    message
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let lower = trimmed.to_ascii_lowercase();
            if !lower.starts_with("co-authored-by:") {
                return None;
            }
            let open = trimmed.rfind('<')?;
            let close = trimmed.rfind('>')?;
            if close <= open + 1 {
                return None;
            }
            Some(trimmed[open + 1..close].trim().to_string())
        })
        .filter(|e| !e.is_empty())
        .collect()
}

/// Pure core of the gate: every non-allowlisted email in `commits`, across
/// author, committer, and `Co-authored-by:` trailers.
// trace:TASK-1330 | ai:claude
pub(crate) fn commit_identity_violations(
    commits: &[CommitIdentity],
    allowlist: &[String],
) -> Vec<IdentityViolation> {
    let mut out = Vec::new();
    for c in commits {
        if !email_allowed(&c.author_email, allowlist) {
            out.push(IdentityViolation {
                sha: c.sha.clone(),
                role: "author",
                email: c.author_email.clone(),
            });
        }
        if !email_allowed(&c.committer_email, allowlist) {
            out.push(IdentityViolation {
                sha: c.sha.clone(),
                role: "committer",
                email: c.committer_email.clone(),
            });
        }
        for email in coauthor_trailer_emails(&c.message) {
            if !email_allowed(&email, allowlist) {
                out.push(IdentityViolation {
                    sha: c.sha.clone(),
                    role: "Co-authored-by",
                    email,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pre-push ref parsing + git plumbing
// ---------------------------------------------------------------------------

/// One updated ref from the lines git feeds a pre-push hook on stdin
/// (`<local ref> <local sha> <remote ref> <remote sha>` per line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushedRef {
    pub(crate) local_ref: String,
    pub(crate) local_sha: String,
    /// `None` when the remote ref does not exist yet (all-zero sha).
    pub(crate) remote_sha: Option<String>,
}

fn is_zero_sha(sha: &str) -> bool {
    !sha.is_empty() && sha.chars().all(|c| c == '0')
}

/// Parse pre-push stdin into the refs that introduce commits. Deletions
/// (all-zero local sha) and malformed lines carry no new commits and are
/// skipped. Unlike the mirror fan-out, the store branch is NOT skipped —
/// the 2026-10-05 incident put 378 offending entries on the public store
/// branch, so it is gated like any other ref.
// trace:TASK-1330 | ai:claude
pub(crate) fn parse_pre_push_ref_lines(input: &str) -> Vec<PushedRef> {
    input
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let local_ref = it.next()?;
            let local_sha = it.next()?;
            let _remote_ref = it.next()?;
            let remote_sha = it.next()?;
            if is_zero_sha(local_sha) {
                return None;
            }
            Some(PushedRef {
                local_ref: local_ref.to_string(),
                local_sha: local_sha.to_string(),
                remote_sha: (!is_zero_sha(remote_sha)).then(|| remote_sha.to_string()),
            })
        })
        .collect()
}

/// `git log` record/field separators: one `\x1e`-terminated record per
/// commit, `\x1f` between fields, `%B` last so the full message (which may
/// contain anything but these control bytes) parses unambiguously.
const LOG_FORMAT: &str = "%H%x1f%ae%x1f%ce%x1f%B%x1e";

fn parse_log_records(stdout: &str) -> Vec<CommitIdentity> {
    stdout
        .split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            let mut fields = record.splitn(4, '\u{1f}');
            let sha = fields.next()?.trim().to_string();
            if sha.is_empty() {
                return None;
            }
            Some(CommitIdentity {
                sha,
                author_email: fields.next()?.trim().to_string(),
                committer_email: fields.next()?.trim().to_string(),
                message: fields.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

fn git_log_identities(repo: &Path, range_args: &[&str]) -> Result<Vec<CommitIdentity>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", &format!("--format={LOG_FORMAT}")])
        .args(range_args)
        .arg("--")
        .output()
        .context("failed to run git log")?;
    if !out.status.success() {
        bail!(
            "git log {} failed: {}",
            range_args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_log_records(&String::from_utf8_lossy(&out.stdout)))
}

/// The commits a pushed ref would introduce on the remote. Known remote tip →
/// `<remote-sha>..<local-sha>`; new ref (or a remote tip this clone does not
/// have) → everything reachable from the local sha that no ref of `remote`
/// already holds.
// trace:TASK-1330 | ai:claude
pub(crate) fn commits_for_pushed_ref(
    repo: &Path,
    remote: &str,
    pushed: &PushedRef,
) -> Result<Vec<CommitIdentity>> {
    if let Some(remote_sha) = &pushed.remote_sha {
        let range = format!("{remote_sha}..{}", pushed.local_sha);
        if let Ok(commits) = git_log_identities(repo, &[&range]) {
            return Ok(commits);
        }
        // The remote tip may be absent locally (e.g. a stale remote-tracking
        // state); fall through to the exclusion form rather than failing on
        // a range git cannot walk.
    }
    let not_remotes = format!("--remotes={remote}");
    git_log_identities(repo, &[&pushed.local_sha, "--not", &not_remotes])
}

// ---------------------------------------------------------------------------
// Hook scripts
// ---------------------------------------------------------------------------

/// The POSIX-sh gate block shared by the standalone identity hook and the
/// mirror fan-out hook, so the two generators cannot drift. Expects the hook
/// to have captured its stdin in `$refs` first. Fail-closed: a configured
/// allowlist with no working `aida identity check-push` refuses the push.
// trace:TASK-1330 | ai:claude
pub(crate) fn identity_gate_hook_lines() -> String {
    "# trace:TASK-1330 | ai:claude\n\
     # Identity hygiene gate (refuses a push that introduces a non-allowlisted\n\
     # author/committer/Co-authored-by email when [identity] allowed_emails is\n\
     # configured in .aida/config.toml). Fail-closed; bypass deliberately with\n\
     # `git push --no-verify`.\n\
     if command -v aida >/dev/null 2>&1 && aida identity check-push --help >/dev/null 2>&1; then\n\
     \u{20} printf '%s\\n' \"$refs\" | aida identity check-push \"$1\" || exit 1\n\
     elif [ -f .aida/config.toml ] && grep -q allowed_emails .aida/config.toml 2>/dev/null; then\n\
     \u{20} echo \"identity gate: [identity] allowed_emails is configured but 'aida identity check-push' is unavailable - refusing the push (fail-closed). Install/upgrade aida, or bypass deliberately with git push --no-verify.\" >&2\n\
     \u{20} exit 1\n\
     fi\n"
        .to_string()
}

/// The standalone pre-push hook `aida identity install-hook` writes when no
/// AIDA hook exists yet. Repos with mirror fan-out get the gate embedded in
/// the mirror hook instead (one file, git only runs one pre-push hook).
// trace:TASK-1330 | ai:claude
pub(crate) fn identity_pre_push_hook_script() -> String {
    format!(
        "#!/bin/sh\n\
         {IDENTITY_HOOK_HEADER}\n\
         # Refuses a push introducing commits whose author, committer, or\n\
         # Co-authored-by trailer email is not in [identity] allowed_emails\n\
         # (.aida/config.toml). No allowlist configured = no gate. Safe to\n\
         # delete; reinstall with `aida identity install-hook`.\n\
         unset GIT_DIR GIT_WORK_TREE\n\
         refs=$(cat)\n\
         {gate}\
         exit 0\n",
        gate = identity_gate_hook_lines()
    )
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

fn effective_email(repo: &Path, var: &str) -> Option<String> {
    // `git var GIT_AUTHOR_IDENT` / `GIT_COMMITTER_IDENT` honor config AND the
    // GIT_*_EMAIL environment overrides — the identities a commit made right
    // now would actually carry.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["var", var])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let ident = String::from_utf8_lossy(&out.stdout);
    let open = ident.rfind('<')?;
    let close = ident.rfind('>')?;
    if close <= open + 1 {
        return None;
    }
    Some(ident[open + 1..close].trim().to_string())
}

/// `aida identity check` — report the identities a commit made here would
/// carry against the project allowlist. Exits non-zero on a violation so it
/// can gate scripts; a project with no allowlist passes with a hint.
// trace:TASK-1330 | ai:claude
pub(crate) fn handle_identity_check(project_root: &Path) -> Result<()> {
    let check = crate::glyph(crate::glyphs::Glyph::Check);
    let allowlist = read_allowed_emails(project_root)?;
    if allowlist.is_empty() {
        println!(
            "identity gate not configured — add [identity] allowed_emails = [\"you@example.org\"] \
             to .aida/config.toml to refuse commits/pushes carrying any other email"
        );
        return Ok(());
    }
    let mut bad = Vec::new();
    for (label, var) in [
        ("author", "GIT_AUTHOR_IDENT"),
        ("committer", "GIT_COMMITTER_IDENT"),
    ] {
        match effective_email(project_root, var) {
            Some(email) if email_allowed(&email, &allowlist) => {
                println!("{check} {label} email {email} is allowlisted");
            }
            Some(email) => bad.push((label, email)),
            None => bad.push((label, "<unset>".to_string())),
        }
    }
    if !bad.is_empty() {
        let lines: Vec<String> = bad
            .iter()
            .map(|(label, email)| format!("  {label}: {email}"))
            .collect();
        bail!(
            "identity gate: effective git identity is not in [identity] allowed_emails \
             ({}):\n{}\n  Fix: git config user.email <allowed-address>",
            allowlist.join(", "),
            lines.join("\n")
        );
    }
    Ok(())
}

/// `aida identity check-push <remote>` — the pre-push hook plumbing. Reads
/// the ref lines git feeds the hook on stdin, inspects every commit the push
/// would introduce, and exits non-zero when any carries a non-allowlisted
/// author, committer, or Co-authored-by email. Fail-closed: a config or git
/// failure refuses the push rather than waving it through.
// trace:TASK-1330 | ai:claude
pub(crate) fn handle_identity_check_push(project_root: &Path, pushed_remote: &str) -> Result<()> {
    let allowlist = read_allowed_emails(project_root)
        .context("identity gate: could not read [identity] allowed_emails — refusing the push")?;
    if allowlist.is_empty() {
        return Ok(());
    }
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("identity gate: could not read the pre-push ref lines from stdin")?;
    let mut violations = Vec::new();
    for pushed in parse_pre_push_ref_lines(&input) {
        let commits =
            commits_for_pushed_ref(project_root, pushed_remote, &pushed).with_context(|| {
                format!(
                    "identity gate: could not inspect the commits {} would push — \
                     refusing the push",
                    pushed.local_ref
                )
            })?;
        for v in commit_identity_violations(&commits, &allowlist) {
            violations.push((pushed.local_ref.clone(), v));
        }
    }
    if violations.is_empty() {
        return Ok(());
    }
    let mut lines = Vec::new();
    for (local_ref, v) in &violations {
        let short = &v.sha[..v.sha.len().min(10)];
        lines.push(format!(
            "  {short} {role} {email} ({local_ref})",
            role = v.role,
            email = v.email
        ));
    }
    lines.sort();
    lines.dedup();
    bail!(
        "identity gate: this push introduces commits carrying emails outside \
         [identity] allowed_emails ({allow}):\n{list}\n  \
         Fix the identity (git config user.email), rewrite the offending commits \
         (git commit --amend --reset-author / git rebase -i), or bypass deliberately \
         with `git push --no-verify`.",
        allow = allowlist.join(", "),
        list = lines.join("\n")
    );
}

/// `aida identity install-hook` — wire the gate into the repo's pre-push
/// hook. Idempotent. Repos running the AIDA mirror fan-out hook get the
/// current mirror script (which embeds the gate) refreshed in place; a
/// custom pre-push hook is never clobbered (the one line to add is printed).
// trace:TASK-1330 | ai:claude
pub(crate) fn handle_identity_install_hook(project_root: &Path) -> Result<()> {
    let check = crate::glyph(crate::glyphs::Glyph::Check);
    let warn = crate::glyph(crate::glyphs::Glyph::Warning);
    if read_allowed_emails(project_root)?.is_empty() {
        eprintln!(
            "{warn} no [identity] allowed_emails configured in .aida/config.toml — \
             installing the hook anyway; it stays inert until the allowlist is set"
        );
    }
    let hooks_dir = crate::remote_create::repo_hooks_dir(project_root);
    std::fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("failed to create {}", hooks_dir.display()))?;
    let target = hooks_dir.join("pre-push");
    if target.exists() {
        let existing = std::fs::read_to_string(&target).unwrap_or_default();
        if existing.contains(crate::remote_create::MIRROR_HOOK_HEADER) {
            // The mirror fan-out hook embeds the identity gate; refresh it to
            // the current generator so an older (gate-less) install gains it.
            let script = crate::remote_create::mirror_pre_push_hook_script();
            if !aida_core::scaffolding::generated_text_matches(&existing, &script) {
                write_hook(&target, &script)?;
            }
            println!(
                "{check} identity gate active via the mirror fan-out pre-push hook at {}",
                target.display()
            );
            return Ok(());
        }
        if existing.contains(IDENTITY_HOOK_HEADER) {
            let script = identity_pre_push_hook_script();
            if !aida_core::scaffolding::generated_text_matches(&existing, &script) {
                write_hook(&target, &script)?;
            }
            println!("{check} identity pre-push hook already installed");
            return Ok(());
        }
        if existing.contains(IDENTITY_HOOK_MARKER) {
            println!(
                "{check} custom pre-push hook at {} already runs `{IDENTITY_HOOK_MARKER}`",
                target.display()
            );
            return Ok(());
        }
        eprintln!(
            "{warn} a custom pre-push hook already exists at {} — not overwriting.",
            target.display()
        );
        eprintln!("  To enable the identity gate, add these lines to it:");
        eprintln!("    refs=$(cat)");
        eprintln!("    printf '%s\\n' \"$refs\" | aida identity check-push \"$1\" || exit 1");
        return Ok(());
    }
    write_hook(&target, &identity_pre_push_hook_script())?;
    println!(
        "{check} installed identity pre-push hook at {}",
        target.display()
    );
    Ok(())
}

fn write_hook(target: &Path, script: &str) -> Result<()> {
    std::fs::write(target, script)
        .with_context(|| format!("failed to write {}", target.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(target)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(target, perms)?;
    }
    Ok(())
}

/// `aida commit` enforcement point — the same two-enforcement-point pattern
/// as the advisor-code and locking gates: refuse assembling a commit whose
/// effective author/committer email is outside the allowlist. Silent when no
/// allowlist is configured.
// trace:TASK-1330 | ai:claude
pub(crate) fn enforce_at_commit(project_root: &Path) -> Result<()> {
    let allowlist = read_allowed_emails(project_root)?;
    if allowlist.is_empty() {
        return Ok(());
    }
    for (label, var) in [
        ("author", "GIT_AUTHOR_IDENT"),
        ("committer", "GIT_COMMITTER_IDENT"),
    ] {
        if let Some(email) = effective_email(project_root, var) {
            if !email_allowed(&email, &allowlist) {
                bail!(
                    "identity gate: {label} email {email} is not in [identity] allowed_emails \
                     ({}).\n  Fix: git config user.email <allowed-address>",
                    allowlist.join(", ")
                );
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `aida pr ship` squash-body sanitizer
// ---------------------------------------------------------------------------

/// Strip non-allowlisted `Co-authored-by:` trailer lines from one commit
/// message. Allowlisted trailers (and everything else) are preserved.
pub(crate) fn strip_disallowed_coauthor_lines(message: &str, allowlist: &[String]) -> String {
    let kept: Vec<&str> = message
        .lines()
        .filter(|line| {
            let emails = coauthor_trailer_emails(line);
            emails.is_empty() || emails.iter().all(|e| email_allowed(e, allowlist))
        })
        .collect();
    kept.join("\n")
}

/// When the squash merge of `base..head` would carry a non-allowlisted email
/// into the squash body — an explicit non-allowlisted `Co-authored-by:`
/// trailer, or a non-allowlisted commit author/committer the forge would
/// auto-derive a trailer from — return an explicit replacement body: the
/// branch's commit messages, oldest first, each as a `* subject` bullet, with
/// the offending trailers stripped. Passing this body to the forge suppresses
/// its auto-derived co-author trailers, so the offending email never reaches
/// the squash commit. `Ok(None)` = nothing to sanitize, let the forge build
/// its default body.
// trace:TASK-1330 | ai:claude
pub(crate) fn squash_coauthor_body_override(
    repo: &Path,
    base_branch: &str,
    head: &str,
    allowlist: &[String],
) -> Result<Option<(String, Vec<IdentityViolation>)>> {
    if allowlist.is_empty() {
        return Ok(None);
    }
    let range = format!("origin/{base_branch}..{head}");
    let commits = match git_log_identities(repo, &[&range]) {
        Ok(c) => c,
        // No origin/<base> locally — fall back to the plain base name.
        Err(_) => {
            git_log_identities(repo, &[&format!("{base_branch}..{head}")]).with_context(|| {
                format!(
                    "identity gate: could not inspect commits {base_branch}..{head} to \
                     sanitize the squash body — fetch the base branch and retry"
                )
            })?
        }
    };
    let violations = commit_identity_violations(&commits, allowlist);
    if violations.is_empty() {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for c in commits.iter().rev() {
        let sanitized = strip_disallowed_coauthor_lines(c.message.trim_end(), allowlist);
        let mut lines = sanitized.lines();
        let subject = lines.next().unwrap_or("").trim();
        if subject.is_empty() {
            continue;
        }
        let rest: Vec<&str> = lines.collect();
        let rest = rest.join("\n");
        let rest = rest.trim();
        if rest.is_empty() {
            entries.push(format!("* {subject}"));
        } else {
            entries.push(format!("* {subject}\n\n{rest}"));
        }
    }
    Ok(Some((entries.join("\n\n"), violations)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod task_1330_identity_gate_tests {
    use super::*;

    fn allow(emails: &[&str]) -> Vec<String> {
        emails.iter().map(|s| s.to_string()).collect()
    }

    fn commit(sha: &str, author: &str, committer: &str, message: &str) -> CommitIdentity {
        CommitIdentity {
            sha: sha.to_string(),
            author_email: author.to_string(),
            committer_email: committer.to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn allowlist_membership_is_case_insensitive_and_trimmed() {
        let list = allow(&["Joe.Mooney@gmail.com"]);
        assert!(email_allowed("joe.mooney@gmail.com", &list));
        assert!(email_allowed("  JOE.MOONEY@GMAIL.COM  ", &list));
        assert!(!email_allowed("joe@work.example.com", &list));
    }

    #[test]
    fn coauthor_trailers_parse_case_insensitively() {
        let msg = "feat: x\n\nBody.\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n\
                   co-authored-by: Someone <some.one@work.example.com>\n\
                   Co-authored-by: broken-no-email\n";
        assert_eq!(
            coauthor_trailer_emails(msg),
            vec!["noreply@anthropic.com", "some.one@work.example.com"]
        );
    }

    #[test]
    fn violations_cover_author_committer_and_trailers() {
        let list = allow(&["good@example.org"]);
        let commits = vec![
            commit("a1", "good@example.org", "good@example.org", "ok: clean"),
            commit(
                "b2",
                "bad@work.example.com",
                "good@example.org",
                "feat: y\n\nCo-authored-by: Bad <also.bad@work.example.com>",
            ),
        ];
        let v = commit_identity_violations(&commits, &list);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].role, "author");
        assert_eq!(v[0].email, "bad@work.example.com");
        assert_eq!(v[1].role, "Co-authored-by");
        assert_eq!(v[1].email, "also.bad@work.example.com");
    }

    #[test]
    fn pre_push_parsing_skips_deletions_and_garbage() {
        const ZERO: &str = "0000000000000000000000000000000000000000";
        let input = format!(
            "refs/heads/main aaa111 refs/heads/main bbb222\n\
             refs/heads/gone {ZERO} refs/heads/gone ccc333\n\
             refs/heads/new ddd444 refs/heads/new {ZERO}\n\
             garbage\n"
        );
        let refs = parse_pre_push_ref_lines(&input);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].local_sha, "aaa111");
        assert_eq!(refs[0].remote_sha.as_deref(), Some("bbb222"));
        assert_eq!(refs[1].local_ref, "refs/heads/new");
        assert_eq!(refs[1].remote_sha, None);
    }

    #[test]
    fn log_records_parse_multiline_messages() {
        let stdout = "sha1\u{1f}a@x.org\u{1f}c@x.org\u{1f}feat: one\n\nbody line\n\u{1e}\n\
                      sha2\u{1f}b@x.org\u{1f}d@x.org\u{1f}fix: two\n\u{1e}\n";
        let commits = parse_log_records(stdout);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "sha1");
        assert_eq!(commits[0].committer_email, "c@x.org");
        assert!(commits[0].message.contains("body line"));
        assert_eq!(commits[1].author_email, "b@x.org");
    }

    #[test]
    fn read_allowed_emails_handles_absent_section_and_shorthand() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No config file at all.
        assert!(read_allowed_emails(root).unwrap().is_empty());
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        // Config without the section.
        std::fs::write(
            root.join(".aida/config.toml"),
            "[hints]\nworkflow_hints = true\n",
        )
        .unwrap();
        assert!(read_allowed_emails(root).unwrap().is_empty());
        // Array form.
        std::fs::write(
            root.join(".aida/config.toml"),
            "[identity]\nallowed_emails = [\"a@x.org\", \" b@y.org \", \"\"]\n",
        )
        .unwrap();
        assert_eq!(
            read_allowed_emails(root).unwrap(),
            vec!["a@x.org", "b@y.org"]
        );
        // Bare-string shorthand.
        std::fs::write(
            root.join(".aida/config.toml"),
            "[identity]\nallowed_emails = \"solo@x.org\"\n",
        )
        .unwrap();
        assert_eq!(read_allowed_emails(root).unwrap(), vec!["solo@x.org"]);
    }

    #[test]
    fn read_allowed_emails_fails_closed_on_unparseable_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(tmp.path().join(".aida/config.toml"), "[identity\nbroken").unwrap();
        assert!(read_allowed_emails(tmp.path()).is_err());
    }

    #[test]
    fn standalone_hook_script_is_posix_and_fail_closed() {
        let s = identity_pre_push_hook_script();
        assert!(s.starts_with("#!/bin/sh\n"), "hook must run under /bin/sh");
        assert!(s.contains(IDENTITY_HOOK_HEADER));
        assert!(
            s.contains(IDENTITY_HOOK_MARKER),
            "installer must recognize its own hook"
        );
        assert!(
            s.contains("|| exit 1"),
            "a gate violation must refuse the push"
        );
        assert!(
            s.contains("refusing the push (fail-closed)"),
            "a configured allowlist without a working gate must refuse"
        );
        assert!(s.trim_end().ends_with("exit 0"));
    }

    #[test]
    fn strip_keeps_allowlisted_trailers_and_everything_else() {
        let list = allow(&["ok@x.org"]);
        let msg = "feat: z (TASK-1)\n\nReal body.\n\n\
                   Co-authored-by: Ok <ok@x.org>\n\
                   Co-authored-by: Bad <bad@work.example.com>";
        let out = strip_disallowed_coauthor_lines(msg, &list);
        assert!(out.contains("Real body."));
        assert!(out.contains("ok@x.org"));
        assert!(!out.contains("bad@work.example.com"));
    }

    // Integration: a real repo, a commit with an offending trailer, and the
    // squash-body override that strips it.
    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&st.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.name", "Good"]);
        run(&["config", "user.email", "good@example.org"]);
        run(&["commit", "-q", "--allow-empty", "-m", "chore: root"]);
    }

    #[test]
    fn squash_body_override_strips_offending_trailers_from_branch_commits() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&st.stderr)
            );
        };
        run(&["checkout", "-q", "-b", "feature"]);
        run(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: slice one (TASK-1)\n\nBody.\n\nCo-authored-by: Bad <bad@work.example.com>",
        ]);
        run(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fix: slice two (TASK-1)",
        ]);
        let list = allow(&["good@example.org"]);
        let (body, violations) =
            squash_coauthor_body_override(tmp.path(), "main", "feature", &list)
                .unwrap()
                .expect("offending trailer must trigger an override");
        assert!(!body.contains("bad@work.example.com"));
        assert!(body.contains("* feat: slice one (TASK-1)"));
        assert!(body.contains("* fix: slice two (TASK-1)"));
        // Oldest first, like the forge's own squash body.
        assert!(body.find("slice one").unwrap() < body.find("slice two").unwrap());
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].role, "Co-authored-by");

        // A clean branch produces no override.
        let clean = squash_coauthor_body_override(
            tmp.path(),
            "main",
            "feature",
            &allow(&["good@example.org", "bad@work.example.com"]),
        )
        .unwrap();
        assert!(clean.is_none());
    }

    #[test]
    fn commits_for_pushed_ref_walks_only_new_commits() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let sha = |rev: &str| {
            let out = Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(["rev-parse", rev])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let base = sha("HEAD");
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(st.status.success());
        };
        run(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: new work",
            "--author",
            "Bad <bad@work.example.com>",
        ]);
        let head = sha("HEAD");
        let pushed = PushedRef {
            local_ref: "refs/heads/main".to_string(),
            local_sha: head.clone(),
            remote_sha: Some(base),
        };
        let commits = commits_for_pushed_ref(tmp.path(), "origin", &pushed).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].author_email, "bad@work.example.com");
        let v = commit_identity_violations(&commits, &allow(&["good@example.org"]));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].role, "author");
    }
}
