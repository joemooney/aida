//! Operator emergency merge; protection restoration precedes all success reporting.
// trace:STORY-2732 | ai:codex
use crate::forge::{emergency_land::Client, ForgeKind};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(clap::Args, Debug)]
pub struct EmergencyLandArgs {
    /// Open PR or MR to merge into the default branch.
    pub n: u64,
    /// Maximum added plus deleted lines.
    #[arg(long, default_value_t = 200)]
    pub max_lines: u64,
    /// Permit more than the line limit or ten files.
    #[arg(long)]
    pub allow_large: bool,
    /// Print the plan and precondition verdict without writes.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip the bounded post-merge CI watch.
    #[arg(long)]
    pub no_watch: bool,
}

pub(crate) trait Forge {
    fn api(&mut self, method: &str, endpoint: &str) -> Result<Value>;
    fn needs_lift(&self, backup: &Value) -> Result<bool>;
    fn lift(&mut self, scope: &str) -> Result<()>;
    fn restore_protection(&mut self, scope: &str) -> Result<()>;
    fn verify_protection(&mut self, scope: &str, backup: &Value) -> Result<()>;
    fn recovery_command(&self, scope: &str, backup: &Value) -> String;
    fn merge_sha(&mut self, scope: &str, n: u64) -> Result<String>;
    fn merge(&mut self, n: u64, title: &str, head: &str) -> Result<()>;
}

// The shared ship floor includes the operator's script-deputy PTY and its
// existing attribution policy; a piped stdout is not an authority decision.
fn operator_allowed(operator_present: bool, headless: bool, lane: bool, role: &str) -> bool {
    operator_present && !headless && !lane && !matches!(role, "implementer" | "reviewer")
}

fn preconditions(args: &EmergencyLandArgs, human: bool, pr: &Value, base: &str) -> Result<()> {
    ensure!(human, "emergency-land requires an operator terminal or attributed script-deputy job; lanes, headless and implementer/reviewer sessions are refused");
    ensure!(pr["state"] == "OPEN", "PR must be open");
    ensure!(
        pr["baseRefName"] == base,
        "PR must target the default branch"
    );
    ensure!(
        pr["mergeable"] == "MERGEABLE" && pr["mergeStateStatus"] != "DIRTY",
        "PR mergeability must be known and non-conflicting"
    );
    ensure!(pr["isDraft"] == false, "PR must not be a draft");
    let lines = pr["additions"]
        .as_u64()
        .context("missing additions")?
        .checked_add(pr["deletions"].as_u64().context("missing deletions")?)
        .context("diff size overflow")?;
    let files = pr["changedFiles"].as_u64().context("missing file count")?;
    ensure!(
        args.allow_large || (lines <= args.max_lines && files <= 10),
        "PR too large ({lines} lines, {files} files); inspect it and use --allow-large explicitly"
    );
    ensure!(
        pr["headRefOid"].as_str().is_some_and(|s| !s.is_empty()),
        "missing PR head"
    );
    ensure!(
        pr["title"].as_str().is_some_and(|s| !s.is_empty()),
        "missing PR title"
    );
    Ok(())
}

pub(crate) fn without_urls(mut value: Value) -> Value {
    match &mut value {
        Value::Object(map) => {
            map.retain(|key, _| key != "url" && !key.ends_with("_url"));
            for v in map.values_mut() {
                *v = without_urls(v.take());
            }
        }
        Value::Array(list) => {
            for v in list {
                *v = without_urls(v.take());
            }
        }
        _ => (),
    }
    value
}

struct Restore<'a, F: Forge> {
    forge: &'a mut F,
    endpoint: &'a str,
    backup: &'a Value,
    armed: bool,
}
impl<F: Forge> Restore<'_, F> {
    fn restore(&mut self) -> Result<()> {
        if self.armed {
            self.forge.restore_protection(self.endpoint)?;
            self.armed = false;
        }
        Ok(())
    }
    fn command(&self) -> String {
        self.forge.recovery_command(self.endpoint, self.backup)
    }
}
impl<F: Forge> Drop for Restore<'_, F> {
    fn drop(&mut self) {
        if self.armed {
            // Also runs on unwind. Never replace an in-flight panic with another panic.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.restore()));
            if !matches!(result, Ok(Ok(()))) {
                eprintln!("ERROR: protection restore failed; run: {}", self.command());
            }
        }
    }
}

/// The finally boundary includes ambiguous policy-write failures and panics, not just merge errors.
fn land<F: Forge>(
    forge: &mut F,
    args: &EmergencyLandArgs,
    pr: &Value,
    endpoint: &str,
    backup: &Value,
    audit: &Path,
    operator: &str,
) -> Result<String> {
    if args.dry_run {
        return Ok(String::new());
    }
    let enabled = forge.needs_lift(backup)?;
    let mut guard = Restore {
        forge,
        endpoint,
        backup,
        armed: enabled,
    };
    let merged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        if enabled {
            guard.forge.lift(endpoint)?;
        }
        guard.forge.merge(
            args.n,
            pr["title"].as_str().unwrap(),
            pr["headRefOid"].as_str().unwrap(),
        )
    }));
    let restore = guard.restore();
    let verified = guard.forge.verify_protection(endpoint, backup);
    let restoration = restore.and(verified);
    let merge_result = match merged {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("emergency merge panicked")),
    };
    // Preserve actual merge evidence even after an ambiguous merge response.
    let sha = guard.forge.merge_sha(endpoint, args.n).unwrap_or_default();
    let record = json!({"ts": chrono::Utc::now().to_rfc3339(), "operator": operator,
        "pr": args.n, "head_sha": pr["headRefOid"], "merge_sha": sha,
        "skipped_required_checks": pr["required_checks"],
        "merge_result": merge_result.as_ref().err().map(ToString::to_string),
        "restore_result": restoration.as_ref().err().map(ToString::to_string).unwrap_or_else(|| "verified".into())});
    let audit_result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(audit)?;
        writeln!(file, "{record}")?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = restoration {
        bail!(
            "ERROR: protection restoration could not be verified: {error:#}; run: {}",
            guard.command()
        );
    }
    audit_result.context("emergency-land audit write failed")?;
    merge_result?;
    ensure!(
        !sha.is_empty(),
        "merge returned success but squash SHA is unavailable; inspect PR {}",
        args.n
    );
    Ok(sha)
}

// trace:TASK-3788 | ai:codex
pub(crate) fn run(args: &EmergencyLandArgs) -> Result<()> {
    let env = aida_core::exec_context::Environment::capture();
    let role = crate::role::effective_role();
    let human = operator_allowed(
        crate::pr_cmd::ship_interactive_tty(),
        env.var("AIDA_HEADLESS").unwrap_or("") == "1",
        env.var("AIDA_CONTAINMENT_POSTURE").unwrap_or("") == "native",
        &role,
    );
    ensure!(human, "emergency-land requires a human terminal or operator-attributed script deputy; never run directly from a lane, headless process or implementer/reviewer session");
    let root = crate::find_project_root()?;
    ensure!(
        aida_core::seat_clone_detect::seat_clone_root(&root).is_none(),
        "emergency-land is never callable from a lane clone; run from the operator checkout"
    );
    let kind = crate::forge::resolve_forge_kind(&root);
    crate::forge::emergency_land::supported(kind)?;
    let mut forge = Client::new(root.clone(), kind)?;
    let (pr, endpoint, backup, base) = if kind == ForgeKind::GitLab {
        let (pr, endpoint, backup, base) = forge.prepare_gitlab(args.n)?;
        preconditions(args, human, &pr, &base)?;
        (pr, endpoint, backup, base)
    } else {
        let (pr, endpoint, backup) = prepare_github(&mut forge, args, human)?;
        let base = pr["baseRefName"].as_str().unwrap().to_owned();
        (pr, endpoint, backup, base)
    };
    println!("Preconditions passed for change {}. Plan: save policy backup; {}; ALWAYS restore and verify; append audit and spec comment; {}.", args.n,
        if kind == ForgeKind::GitLab { "PUT pipeline requirement off only if enabled; GitLab merge API with squash and auto-merge disabled" } else { "DELETE enforce_admins only if enabled; gh pr merge --squash --admin" },
        if args.no_watch { "skip watch" } else { "watch merge-commit CI for up to 30 minutes" });
    if args.dry_run {
        println!("Dry run: no API or local writes.");
        return Ok(());
    }
    let operator = forge.api("GET", "user")?[if kind == ForgeKind::GitLab {
        "username"
    } else {
        "login"
    }]
    .as_str()
    .context("missing operator login")?
    .to_owned();
    let dir = root.join(".aida/emergency-land");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "{}-PR-{}.protection.json",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
        args.n
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(&serde_json::to_vec_pretty(&backup)?)?;
    file.sync_all()?;
    println!("Protection backup: {}", path.display());
    let mut signals = crate::forwarded_signals::Forwarding::install()?;
    let landed = land(
        &mut forge,
        args,
        &pr,
        &endpoint,
        &backup,
        &dir.join("audit.jsonl"),
        &operator,
    );
    let outcome = match &landed {
        Ok(sha) => format!("merged at {sha}; protection restored and verified"),
        Err(error) => format!("failed: {error:#}"),
    };
    let mut comment_failed = false;
    for spec in
        crate::pr_ship::extract_authorship_spec_ids_from_subject(pr["title"].as_str().unwrap())
    {
        let status = std::process::Command::new(crate::resolve_aida_exe())
            .current_dir(&root)
            .args([
                "comment",
                "add",
                &spec,
                &format!(
                    "Operator {operator} emergency-land PR {}: {outcome}. Audit: {}",
                    args.n,
                    dir.join("audit.jsonl").display()
                ),
            ])
            .status();
        if !matches!(status, Ok(status) if status.success()) {
            eprintln!("ERROR: emergency-land spec audit comment failed for {spec}");
            comment_failed = true;
        }
    }
    let sha = landed?;
    ensure!(
        !comment_failed,
        "merged and restored, but spec audit comment failed"
    );
    println!(
        "Merged PR {} at {sha}; protection restored and verified.",
        args.n
    );
    if signals.take().is_some() {
        bail!("interrupted; protection restored; inspect merge {sha}");
    }
    if !args.no_watch {
        watch(&forge, &endpoint, &base, &sha, &mut signals)?;
    }
    Ok(())
}

fn prepare_github(
    forge: &mut Client,
    args: &EmergencyLandArgs,
    human: bool,
) -> Result<(Value, String, Value)> {
    let repo = forge.json(&["repo", "view", "--json", "nameWithOwner,defaultBranchRef"])?;
    let name = repo["nameWithOwner"]
        .as_str()
        .context("missing repository")?;
    let base = repo["defaultBranchRef"]["name"]
        .as_str()
        .context("missing default branch")?;
    let mut pr = forge.json(&["pr", "view", &args.n.to_string(), "--json", "state,baseRefName,mergeable,mergeStateStatus,isDraft,additions,deletions,changedFiles,headRefOid,title"])?;
    preconditions(args, human, &pr, base)?;
    let checks = forge.command(&[
        "pr",
        "checks",
        &args.n.to_string(),
        "--required",
        "--json",
        "name,state",
    ]);
    // `gh pr checks` exits 8 for pending and 1 for failed checks. Use the API
    // protection contexts + head check runs instead of dropping that evidence.
    let endpoint = format!("repos/{name}/branches/{}/protection", encode(base));
    let backup = forge.api("GET", &endpoint)?;
    ensure!(
        backup["enforce_admins"]["enabled"].is_boolean(),
        "missing admin enforcement evidence"
    );
    pr["required_checks"] = match checks {
        Ok(text) => serde_json::from_str(&text)?,
        Err(_) => required_checks(forge, name, &pr, &backup)?,
    };
    Ok((pr, endpoint, backup))
}

pub(crate) fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn required_checks(
    forge: &mut impl Forge,
    repo: &str,
    pr: &Value,
    backup: &Value,
) -> Result<Value> {
    let head = pr["headRefOid"].as_str().context("missing head")?;
    let runs = forge.api(
        "GET",
        &format!("repos/{repo}/commits/{head}/check-runs?per_page=100"),
    )?;
    let statuses = forge.api(
        "GET",
        &format!("repos/{repo}/commits/{head}/status?per_page=100"),
    )?;
    let mut rows = Vec::new();
    if let Some(contexts) = backup["required_status_checks"]["contexts"].as_array() {
        for context in contexts {
            let name = context.as_str().context("invalid required check context")?;
            let run = runs["check_runs"]
                .as_array()
                .and_then(|rs| rs.iter().find(|r| r["name"] == name));
            let status = statuses["statuses"]
                .as_array()
                .and_then(|rs| rs.iter().find(|r| r["context"] == name));
            rows.push(json!({"name": name, "state": run.map(|r| json!({"status":r["status"],"conclusion":r["conclusion"]})).or_else(|| status.map(|r| r["state"].clone())).unwrap_or(json!("unknown"))}));
        }
    }
    Ok(json!(rows))
}

// trace:TASK-3788 | ai:codex
pub(crate) fn gitlab_revert_command(commit: &Value, sha: &str) -> Result<String> {
    ensure!(commit["id"] == sha, "commit does not match merge SHA");
    let parents = commit["parent_ids"]
        .as_array()
        .context("missing commit parents")?;
    ensure!(
        !parents.is_empty()
            && parents
                .iter()
                .all(|p| p.as_str().is_some_and(|s| !s.is_empty())),
        "invalid commit parents"
    );
    Ok(format!(
        "git revert {}{sha}",
        if parents.len() > 1 { "-m 1 " } else { "" }
    ))
}

fn watch(
    forge: &Client,
    scope: &str,
    base: &str,
    sha: &str,
    signals: &mut crate::forwarded_signals::Forwarding,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30 * 60);
    loop {
        if signals.take().is_some() {
            bail!("CI watch interrupted; protection already restored");
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "CI watch timed out after 30 minutes; inspect merge {sha}"
        );
        let runs: Value = if forge.kind == ForgeKind::GitLab {
            // glab ci status is branch-scoped. Query pipelines by exact merge SHA
            // so a newer default-branch pipeline cannot substitute its evidence.
            let rows: Value = serde_json::from_str(&forge.command_with_budget(
                &[
                    "api",
                    "-X",
                    "GET",
                    &format!(
                        "{scope}/pipelines?sha={}&ref={}&per_page=100",
                        encode(sha),
                        encode(base)
                    ),
                ],
                remaining,
            )?)?;
            gitlab_ci_runs(&rows, sha)?
        } else {
            serde_json::from_str(&forge.command_with_budget(
                &[
                    "run",
                    "list",
                    "--commit",
                    sha,
                    "--limit",
                    "100",
                    "--json",
                    "status,conclusion",
                ],
                remaining,
            )?)?
        };
        let runs = runs.as_array().context("invalid CI run list")?;
        let state = classify_ci_runs(runs);
        if state == CiState::Red {
            let revert = forge.revert_command(scope, sha)
                .context("Post-merge CI is red; cannot determine revert topology; inspect the merge commit parents")?;
            bail!("Post-merge CI is red. Revert through a new PR: git fetch origin && git switch -c revert-emergency-{} origin/HEAD && {} && git push -u origin HEAD && {}; then aida pr emergency-land <REVERT-PR>. No automatic revert was made.", &sha[..sha.len().min(12)], revert, if forge.kind == ForgeKind::GitLab { "glab mr create --fill" } else { "gh pr create --fill" });
        }
        if state == CiState::Green {
            println!("Merge-commit CI completed successfully.");
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("CI watch timed out after 30 minutes; inspect merge {sha}");
        }
        println!(
            "Watching merge {sha}: {} CI runs, waiting (30-minute bound).",
            runs.len()
        );
        std::thread::sleep(
            Duration::from_secs(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

// trace:TASK-3788 | ai:codex
fn gitlab_ci_runs(rows: &Value, sha: &str) -> Result<Value> {
    let mut runs = Vec::new();
    for row in rows.as_array().context("invalid GitLab pipeline list")? {
        ensure!(row["sha"] == sha, "pipeline does not match merge commit");
        let status = row["status"].as_str().context("missing pipeline status")?;
        let (state, conclusion) = match status {
            "success" => ("completed", "success"),
            "failed" | "canceled" | "skipped" | "manual" => ("completed", "failure"),
            "created"
            | "waiting_for_resource"
            | "preparing"
            | "pending"
            | "running"
            | "scheduled" => ("in_progress", ""),
            _ => bail!("unknown GitLab pipeline status: {status}"),
        };
        runs.push(json!({"status":state, "conclusion":conclusion}));
    }
    Ok(json!(runs))
}

#[derive(Debug, PartialEq, Eq)]
enum CiState {
    Red,
    Green,
    Pending,
}

// Only an explicit success-like conclusion counts as green. A completed run
// with any other conclusion (startup_failure, stale, unknown, null) is red, so
// the skill never reports success without the revert guidance.
// trace:STORY-2732 | ai:claude
fn classify_ci_runs(runs: &[Value]) -> CiState {
    let completed = |r: &&Value| r["status"] == "completed";
    let ok = |r: &&Value| {
        matches!(
            r["conclusion"].as_str(),
            Some("success" | "skipped" | "neutral")
        )
    };
    if runs.iter().filter(completed).any(|r| !ok(&r)) {
        CiState::Red
    } else if !runs.is_empty() && runs.iter().all(|r| completed(&r)) {
        CiState::Green
    } else {
        CiState::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:STORY-2732 | ai:claude
    #[test]
    fn pr_emergency_land_ci_watch_classifies_conclusions() {
        let run = |status: &str, conclusion: Value| serde_json::json!({"status": status, "conclusion": conclusion});
        let done = |c: &str| run("completed", Value::from(c));
        assert_eq!(classify_ci_runs(&[]), CiState::Pending);
        assert_eq!(
            classify_ci_runs(&[done("success"), done("skipped"), done("neutral")]),
            CiState::Green
        );
        assert_eq!(
            classify_ci_runs(&[done("success"), run("in_progress", Value::Null)]),
            CiState::Pending
        );
        for bad in [
            "failure",
            "timed_out",
            "cancelled",
            "action_required",
            "startup_failure",
            "stale",
            "something_new",
        ] {
            assert_eq!(
                classify_ci_runs(&[done("success"), done(bad)]),
                CiState::Red,
                "{bad}"
            );
        }
        assert_eq!(
            classify_ci_runs(&[run("completed", Value::Null)]),
            CiState::Red
        );
        assert_eq!(
            classify_ci_runs(&[run("completed", Value::Null), run("queued", Value::Null)]),
            CiState::Red
        );
    }
    // Exercise the production GitLab adapter through a fake command boundary.
    // trace:TASK-3788 | ai:codex
    #[derive(Default)]
    struct GitLabState {
        calls: Vec<Vec<String>>,
        enabled: bool,
        nullable_skipped: bool,
        armed_pipeline: Option<&'static str>,
        merged: bool,
        fail_merge: bool,
        panic_merge: bool,
        fail_lift: bool,
        fail_restore: bool,
        mismatch: bool,
        fail_verify: bool,
    }
    fn gitlab_fake(state: GitLabState) -> (Client, std::rc::Rc<std::cell::RefCell<GitLabState>>) {
        let state = std::rc::Rc::new(std::cell::RefCell::new(state));
        let shared = state.clone();
        let client = Client::fake(ForgeKind::GitLab, move |args| {
            let mut s = shared.borrow_mut();
            s.calls.push(args.iter().map(|s| s.to_string()).collect());
            let value = match args {
                ["api", "-X", "PUT", "projects/group%2Frepo", "-f", field] => {
                    if *field == "only_allow_merge_if_pipeline_succeeds=false" {
                        s.enabled = false;
                        ensure!(!s.fail_lift, "lost PUT response");
                    } else {
                        assert_eq!(*field, "only_allow_merge_if_pipeline_succeeds=true");
                        ensure!(!s.fail_restore, "restore failed");
                        s.enabled = true;
                    }
                    Value::Null
                }
                ["api", "-X", "GET", "projects/:id"] => {
                    json!({"id":42, "path_with_namespace":"group/repo", "default_branch":"main",
                        "only_allow_merge_if_pipeline_succeeds":s.enabled,
                        "allow_merge_on_skipped_pipeline":if s.nullable_skipped { Value::Null } else { json!(false) },
                        "only_allow_merge_if_all_discussions_are_resolved":true})
                }
                ["api", "-X", "GET", "projects/group%2Frepo/merge_requests/7/changes"] => {
                    json!({"sha":"head", "changes_count":"1", "overflow":false,
                        "changes":[{"diff":"@@ -1 +1 @@\n-old\n+new\n"}]})
                }
                ["api", "-X", "GET", "projects/group%2Frepo"] => {
                    ensure!(!s.fail_verify, "verify unavailable");
                    json!({"only_allow_merge_if_pipeline_succeeds":s.enabled,
                        "allow_merge_on_skipped_pipeline":if s.nullable_skipped { Value::Null } else { json!(false) },
                        "only_allow_merge_if_all_discussions_are_resolved":!s.mismatch})
                }
                ["api", "-X", "GET", "projects/group%2Frepo/merge_requests/7"] => {
                    json!({"state":if s.merged { "merged" } else { "opened" },
                        "merge_commit_sha":if s.merged { json!("merge123") } else { Value::Null },
                        "squash_commit_sha":if s.merged { json!("squash123") } else { Value::Null },
                        "merge_when_pipeline_succeeds":s.armed_pipeline.is_some(),
                        "head_pipeline":{"status":s.armed_pipeline},
                        "target_project_id":42,"target_branch":"main","changes_count":"1",
                        "detailed_merge_status":if s.armed_pipeline == Some("running") { "ci_still_running" } else { "ci_must_pass" },
                        "has_conflicts":false,"draft":false,"sha":"head","diff_refs":{"head_sha":"head"},
                        "title":pr()["title"]})
                }
                ["api", "-X", "PUT", "projects/:id/merge_requests/7/merge", "-f", "squash=true", "-f", "auto_merge=false", "-f", "merge_when_pipeline_succeeds=false", "-f", "sha=head", "-f", message] =>
                {
                    assert_eq!(
                        *message,
                        format!("squash_commit_message={}", pr()["title"].as_str().unwrap())
                    );
                    assert!(
                        !s.enabled,
                        "pipeline-success policy must be off during merge"
                    );
                    assert!(!s.panic_merge, "merge panic");
                    ensure!(!s.fail_merge, "GitLab: approval required");
                    s.merged = true;
                    s.armed_pipeline = None;
                    Value::Null
                }
                _ => panic!("unexpected glab command: {args:?}"),
            };
            Ok(value.to_string())
        });
        (client, state)
    }
    fn gitlab_exercise(
        state: GitLabState,
        args: &EmergencyLandArgs,
        human: bool,
        pr: &Value,
    ) -> (Result<String>, std::rc::Rc<std::cell::RefCell<GitLabState>>) {
        let backup = json!({"only_allow_merge_if_pipeline_succeeds":state.enabled,
            "allow_merge_on_skipped_pipeline":if state.nullable_skipped { Value::Null } else { json!(false) },
            "only_allow_merge_if_all_discussions_are_resolved":true});
        let armed = state.armed_pipeline.is_some();
        let (mut client, shared) = gitlab_fake(state);
        // Already-armed cases go through the actual adapter preparation too,
        // with the server reporting an opened MR behind running or failed CI.
        let prepared;
        let pr = if armed {
            let (change, scope, saved, base) = client.prepare_gitlab(args.n).unwrap();
            assert_eq!(scope, "projects/group%2Frepo");
            assert_eq!(saved, backup);
            assert_eq!(base, "main");
            prepared = change;
            &prepared
        } else {
            pr
        };
        let dir = tempfile::tempdir().unwrap();
        let result = preconditions(args, human, pr, "main").and_then(|()| {
            land(
                &mut client,
                args,
                pr,
                "projects/group%2Frepo",
                &backup,
                &dir.path().join("audit.jsonl"),
                "operator",
            )
        });
        if !shared.borrow().calls.is_empty() {
            let audit: Value = serde_json::from_str(
                std::fs::read_to_string(dir.path().join("audit.jsonl"))
                    .unwrap()
                    .trim(),
            )
            .unwrap();
            assert_eq!(
                audit["merge_sha"],
                if shared.borrow().merged {
                    "merge123"
                } else {
                    ""
                }
            );
            assert_eq!(audit["skipped_required_checks"], pr["required_checks"]);
        } else {
            assert!(!dir.path().join("audit.jsonl").exists());
        }
        (result, shared)
    }
    #[test]
    fn pr_emergency_land_gitlab_happy_and_already_off() {
        for enabled in [true, false] {
            let (result, state) = gitlab_exercise(
                GitLabState {
                    enabled,
                    ..Default::default()
                },
                &args(),
                true,
                &pr(),
            );
            assert_eq!(result.unwrap(), "merge123");
            let state = state.borrow();
            assert_eq!(state.enabled, enabled);
            let writes: Vec<_> = state
                .calls
                .iter()
                .filter(|c| c.get(2).is_some_and(|a| a == "PUT") && c[3] == "projects/group%2Frepo")
                .collect();
            assert_eq!(writes.len(), if enabled { 2 } else { 0 });
            if enabled {
                assert_eq!(
                    state.calls[0][5],
                    "only_allow_merge_if_pipeline_succeeds=false"
                );
                assert_eq!(
                    state.calls[2][5],
                    "only_allow_merge_if_pipeline_succeeds=true"
                );
            }
        }
    }
    // trace:TASK-3788 | ai:codex
    #[test]
    fn pr_emergency_land_gitlab_nullable_policy_and_armed_pipeline() {
        for enabled in [true, false] {
            for pipeline in ["running", "failed"] {
                for fail_merge in [false, true] {
                    let (result, shared) = gitlab_exercise(
                        GitLabState {
                            enabled,
                            nullable_skipped: true,
                            armed_pipeline: Some(pipeline),
                            fail_merge,
                            ..Default::default()
                        },
                        &args(),
                        true,
                        &pr(),
                    );
                    assert_eq!(result.is_ok(), !fail_merge);
                    let state = shared.borrow();
                    assert_eq!(state.enabled, enabled);
                    assert_eq!(
                        state.armed_pipeline,
                        if fail_merge { Some(pipeline) } else { None }
                    );
                    let writes: Vec<_> = state
                        .calls
                        .iter()
                        .filter(|c| c[2] == "PUT" && c[3] == "projects/group%2Frepo")
                        .collect();
                    assert_eq!(writes.len(), if enabled { 2 } else { 0 });
                    assert!(state.calls.iter().any(|c| c[3].ends_with("/merge")));
                }
            }
        }
    }

    #[test]
    fn pr_emergency_land_gitlab_policy_rejects_missing_invalid_and_changed_null() {
        use crate::forge::emergency_land::policy;
        let valid = json!({"only_allow_merge_if_pipeline_succeeds":false,
            "allow_merge_on_skipped_pipeline":null,
            "only_allow_merge_if_all_discussions_are_resolved":true});
        assert_eq!(policy(&valid).unwrap(), valid);
        for field in valid.as_object().unwrap().keys() {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(policy(&missing).is_err());
            for value in [json!("false"), json!(0), json!([])] {
                let mut invalid = valid.clone();
                invalid[field] = value;
                assert!(policy(&invalid).is_err());
            }
            if field != "allow_merge_on_skipped_pipeline" {
                let mut invalid = valid.clone();
                invalid[field] = Value::Null;
                assert!(policy(&invalid).is_err());
            }
        }
        let (mut client, _) = gitlab_fake(GitLabState::default());
        assert!(client
            .verify_protection("projects/group%2Frepo", &valid)
            .is_err());
    }

    #[test]
    fn pr_emergency_land_gitlab_revert_uses_actual_commit_parents() {
        for (parents, expected) in [
            (json!(["base", "squash"]), "git revert -m 1 abc123"),
            (json!(["base"]), "git revert abc123"),
        ] {
            let client = Client::fake(ForgeKind::GitLab, move |args| {
                assert_eq!(
                    args,
                    [
                        "api",
                        "-X",
                        "GET",
                        "projects/group%2Frepo/repository/commits/abc123"
                    ]
                );
                Ok(json!({"id":"abc123", "parent_ids":parents}).to_string())
            });
            assert_eq!(
                client
                    .revert_command("projects/group%2Frepo", "abc123")
                    .unwrap(),
                expected
            );
        }
        for commit in [
            json!({"id":"abc123"}),
            json!({"id":"other", "parent_ids":["base"]}),
            json!({"id":"abc123", "parent_ids":[]}),
            json!({"id":"abc123", "parent_ids":[null]}),
        ] {
            assert!(gitlab_revert_command(&commit, "abc123").is_err());
        }
    }

    #[test]
    fn pr_emergency_land_gitlab_merge_failure_panic_and_ambiguous_lift_restore() {
        for mode in 0..3 {
            let (result, state) = gitlab_exercise(
                GitLabState {
                    enabled: true,
                    fail_merge: mode == 0,
                    panic_merge: mode == 1,
                    fail_lift: mode == 2,
                    ..Default::default()
                },
                &args(),
                true,
                &pr(),
            );
            assert!(result.is_err());
            if mode == 0 {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("approval required"));
            }
            let state = state.borrow();
            assert!(state.enabled);
            assert!(state.calls.iter().any(|c| c
                .last()
                .is_some_and(|s| s == "only_allow_merge_if_pipeline_succeeds=true")));
            if mode == 2 {
                assert!(!state.calls.iter().any(|c| c[3].ends_with("/merge")));
            }
        }
    }
    #[test]
    fn pr_emergency_land_gitlab_restore_and_verify_failures_show_recovery() {
        for mode in 0..3 {
            let (result, _) = gitlab_exercise(
                GitLabState {
                    enabled: true,
                    fail_restore: mode == 0,
                    mismatch: mode == 1,
                    fail_verify: mode == 2,
                    ..Default::default()
                },
                &args(),
                true,
                &pr(),
            );
            assert!(result.unwrap_err().to_string().contains(
                "glab api -X PUT projects/group%2Frepo -f only_allow_merge_if_pipeline_succeeds=true"));
        }
    }
    #[test]
    fn pr_emergency_land_gitlab_disabled_mismatch_recovery_preserves_original_setting() {
        let (result, state) = gitlab_exercise(
            GitLabState {
                mismatch: true,
                ..Default::default()
            },
            &args(),
            true,
            &pr(),
        );
        assert!(result.unwrap_err().to_string().contains(
            "glab api -X PUT projects/group%2Frepo -f only_allow_merge_if_pipeline_succeeds=false"
        ));
        assert!(!state
            .borrow()
            .calls
            .iter()
            .any(|c| c.get(2).is_some_and(|s| s == "PUT") && c[3] == "projects/group%2Frepo"));
    }
    #[test]
    fn pr_emergency_land_gitlab_refusals_and_dry_run_make_no_calls() {
        let mut cases = vec![(false, pr())];
        for (field, value) in [
            ("state", json!("CLOSED")),
            ("baseRefName", json!("other")),
            ("mergeable", json!("UNKNOWN")),
            ("isDraft", json!(true)),
            ("additions", json!(201)),
            ("changedFiles", json!(11)),
        ] {
            let mut p = pr();
            p[field] = value;
            cases.push((true, p));
        }
        for (human, p) in cases {
            let (result, state) = gitlab_exercise(GitLabState::default(), &args(), human, &p);
            assert!(result.is_err());
            assert!(state.borrow().calls.is_empty());
        }
        let mut a = args();
        a.dry_run = true;
        let (result, state) = gitlab_exercise(
            GitLabState {
                enabled: true,
                ..Default::default()
            },
            &a,
            true,
            &pr(),
        );
        assert!(result.is_ok());
        assert!(state.borrow().calls.is_empty());
        assert!(crate::forge::emergency_land::supported(ForgeKind::None)
            .unwrap_err()
            .to_string()
            .contains("no forge-side protection to lift; push the fix normally"));
    }
    #[test]
    fn pr_emergency_land_gitlab_normalization_and_ci_are_fail_closed() {
        use crate::forge::emergency_land::{gitlab_change, policy};
        let mr = json!({"overflow":false,"changes_count":"1",
            "changes":[{"diff":"@@ -1 +1 @@\n-old\n+new\n"}],
            "state":"opened","target_branch":"main","detailed_merge_status":"ci_must_pass",
            "has_conflicts":false,"draft":false,"sha":"head","diff_refs":{"head_sha":"head"},"title":"fix","head_pipeline":{"status":"running"}});
        // The full preparation uses only read APIs, encodes nested project paths
        // and retains only the three policy fields.
        for enabled in [true, false] {
            for skipped in [json!(false), Value::Null] {
                let expected_skipped = skipped.clone();
                let observed = mr.clone();
                let mut client = Client::fake(ForgeKind::GitLab, move |args| {
                    Ok(match args {
                ["api", "-X", "GET", "projects/:id"] => json!({
                    "id":42, "path_with_namespace":"group/sub/repo", "default_branch":"main",
                    "only_allow_merge_if_pipeline_succeeds":enabled,
                    "allow_merge_on_skipped_pipeline":skipped,
                    "only_allow_merge_if_all_discussions_are_resolved":true, "unrelated":"ignored"}),
                ["api", "-X", "GET", "projects/group%2Fsub%2Frepo/merge_requests/7"]
                | ["api", "-X", "GET", "projects/group%2Fsub%2Frepo/merge_requests/7/changes"] => {
                    let mut response = observed.clone(); response["target_project_id"] = json!(42); response
                }
                _ => panic!("unexpected preparation command: {args:?}"),
            }.to_string())
                });
                let (prepared, scope, backup, base) = client.prepare_gitlab(7).unwrap();
                assert_eq!(scope, "projects/group%2Fsub%2Frepo");
                assert_eq!(base, "main");
                assert_eq!(backup.as_object().unwrap().len(), 3);
                assert_eq!(prepared, gitlab_change(&mr).unwrap());
                assert_eq!(backup["allow_merge_on_skipped_pipeline"], expected_skipped);
                assert_eq!(client.needs_lift(&backup).unwrap(), enabled);
            }
        }
        let normalized = gitlab_change(&mr).unwrap();
        preconditions(&args(), true, &normalized, "main").unwrap();
        assert_eq!(normalized["additions"], 1);
        assert_eq!(normalized["deletions"], 1);
        for (key, value) in [
            ("overflow", json!(true)),
            ("overflow", Value::Null),
            ("changes_count", json!("1000+")),
            ("changes_count", json!("2")),
        ] {
            let mut bad = mr.clone();
            bad[key] = value;
            assert!(gitlab_change(&bad).is_err());
        }
        for (key, value) in [
            ("has_conflicts", json!(true)),
            ("detailed_merge_status", json!("checking")),
            ("detailed_merge_status", json!("future_state")),
        ] {
            let mut bad = mr.clone();
            bad[key] = value;
            assert!(preconditions(&args(), true, &gitlab_change(&bad).unwrap(), "main").is_err());
        }
        let mut stale = mr.clone();
        stale["diff_refs"]["head_sha"] = json!("old");
        assert!(gitlab_change(&stale).is_err());
        assert!(policy(&json!({"only_allow_merge_if_pipeline_succeeds":true})).is_err());
        assert_eq!(
            classify_ci_runs(
                gitlab_ci_runs(&json!([]), "abc123")
                    .unwrap()
                    .as_array()
                    .unwrap()
            ),
            CiState::Pending
        );
        for (status, expected) in [
            ("success", CiState::Green),
            ("running", CiState::Pending),
            ("failed", CiState::Red),
            ("canceled", CiState::Red),
            ("manual", CiState::Red),
        ] {
            let runs =
                gitlab_ci_runs(&json!([{"sha":"abc123","status":status}]), "abc123").unwrap();
            assert_eq!(classify_ci_runs(runs.as_array().unwrap()), expected);
        }
        assert!(gitlab_ci_runs(&json!([{"sha":"other","status":"success"}]), "abc123").is_err());
        assert!(gitlab_ci_runs(&json!([{"sha":"abc123","status":"future"}]), "abc123").is_err());
    }

    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        enabled: bool,
        fail_merge: bool,
        fail_restore: bool,
        mismatch: bool,
        panic_merge: bool,
        fail_lift: bool,
    }
    impl Forge for Fake {
        fn api(&mut self, method: &str, endpoint: &str) -> Result<Value> {
            self.calls.push(format!("{method} {endpoint}"));
            if endpoint.ends_with("/pulls/7") {
                return Ok(json!({"merged":true,"merge_commit_sha":"abc123"}));
            }
            match method {
                "DELETE" => {
                    self.enabled = false;
                    ensure!(!self.fail_lift, "lost lift response");
                }
                "POST" => {
                    ensure!(!self.fail_restore, "restore failed");
                    self.enabled = true;
                }
                _ => (),
            }
            Ok(
                json!({"enforce_admins":{"enabled":self.enabled}, "extra":self.mismatch, "url":"different"}),
            )
        }
        fn needs_lift(&self, backup: &Value) -> Result<bool> {
            backup["enforce_admins"]["enabled"]
                .as_bool()
                .context("missing enabled")
        }
        fn lift(&mut self, scope: &str) -> Result<()> {
            self.api("DELETE", &format!("{scope}/enforce_admins"))?;
            Ok(())
        }
        fn restore_protection(&mut self, scope: &str) -> Result<()> {
            self.api("POST", &format!("{scope}/enforce_admins"))?;
            Ok(())
        }
        fn verify_protection(&mut self, scope: &str, backup: &Value) -> Result<()> {
            let after = self.api("GET", scope)?;
            ensure!(
                without_urls(after) == without_urls(backup.clone()),
                "protection differs from backup"
            );
            Ok(())
        }
        fn recovery_command(&self, scope: &str, _: &Value) -> String {
            format!("gh api -X POST {scope}/enforce_admins")
        }
        fn merge_sha(&mut self, _: &str, n: u64) -> Result<String> {
            Ok(
                self.api("GET", &format!("repos/o/r/pulls/{n}"))?["merge_commit_sha"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            )
        }
        fn merge(&mut self, _: u64, _: &str, _: &str) -> Result<()> {
            self.calls.push("MERGE".into());
            assert!(!self.panic_merge, "merge panic");
            ensure!(!self.fail_merge, "merge failed");
            Ok(())
        }
    }
    fn args() -> EmergencyLandArgs {
        EmergencyLandArgs {
            n: 7,
            max_lines: 200,
            allow_large: false,
            dry_run: false,
            no_watch: true,
        }
    }
    fn pr() -> Value {
        json!({"state":"OPEN","baseRefName":"main","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","isDraft":false,"additions":1,"deletions":1,"changedFiles":1,"headRefOid":"head","title":"fix (STORY-2732)","required_checks":[{"name":"build","state":"pending"}]})
    }
    fn exercise(
        fake: &mut Fake,
        args: &EmergencyLandArgs,
        human: bool,
        pr: &Value,
    ) -> Result<String> {
        preconditions(args, human, pr, "main")?;
        let dir = tempfile::tempdir()?;
        let backup =
            json!({"enforce_admins":{"enabled":fake.enabled},"extra":false,"url":"original"});
        let result = land(
            fake,
            args,
            pr,
            "repos/o/r/branches/main/protection",
            &backup,
            &dir.path().join("audit.jsonl"),
            "operator",
        );
        if args.dry_run {
            assert!(!dir.path().join("audit.jsonl").exists());
            return result;
        }
        let audit: Value =
            serde_json::from_str(std::fs::read_to_string(dir.path().join("audit.jsonl"))?.trim())?;
        assert_eq!(audit["skipped_required_checks"], pr["required_checks"]);
        result
    }
    #[test]
    fn pr_emergency_land_operator_gate() {
        assert!(operator_allowed(true, false, false, "operator"));
        for role in ["implementer", "reviewer"] {
            assert!(!operator_allowed(true, false, false, role));
        }
        for facts in [
            (false, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            assert!(!operator_allowed(facts.0, facts.1, facts.2, "advisor"));
        }
    }
    #[test]
    fn pr_emergency_land_deputy_admission_keeps_lane_and_headless_refusals() {
        // `ship_interactive_tty` supplies this attested floor for script-deputy
        // jobs even though stdout is captured by the deputy transcript.
        let deputy_floor = true;
        let mut f = Fake {
            enabled: true,
            ..Default::default()
        };
        exercise(
            &mut f,
            &args(),
            operator_allowed(deputy_floor, false, false, "advisor"),
            &pr(),
        )
        .unwrap();
        assert!(f.enabled);
        for (headless, lane) in [(true, false), (false, true)] {
            let mut f = Fake::default();
            assert!(exercise(
                &mut f,
                &args(),
                operator_allowed(deputy_floor, headless, lane, "advisor"),
                &pr()
            )
            .is_err());
            assert!(f.calls.is_empty());
        }
    }
    #[test]
    fn pr_emergency_land_skill_reaches_claude_and_portable_inventory() {
        use aida_core::scaffolding::{inventory::portable_skill_inventory, ScaffoldConfig};
        let inventory = portable_skill_inventory(&ScaffoldConfig::default());
        let skill = &inventory["aida-emergency-fix"];
        assert_eq!(
            skill.skill_md_source(),
            Some("skills-portable/aida-emergency-fix.md")
        );
        assert!(aida_core::templates::EMBEDDED_TEMPLATES
            .contains_key("skills/aida-emergency-fix/SKILL.md"));
    }
    #[test]
    fn pr_emergency_land_happy_path() {
        let mut f = Fake {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(exercise(&mut f, &args(), true, &pr()).unwrap(), "abc123");
        assert_eq!(
            f.calls
                .iter()
                .map(|s| s.split_whitespace().next().unwrap())
                .collect::<Vec<_>>(),
            ["DELETE", "MERGE", "POST", "GET", "GET"]
        );
        assert!(f.enabled);
    }
    #[test]
    fn pr_emergency_land_merge_failure_and_panic_restore() {
        for panic in [false, true] {
            let mut f = Fake {
                enabled: true,
                fail_merge: !panic,
                panic_merge: panic,
                ..Default::default()
            };
            assert!(exercise(&mut f, &args(), true, &pr()).is_err());
            assert!(f.enabled);
        }
        let mut f = Fake {
            enabled: true,
            fail_lift: true,
            ..Default::default()
        };
        assert!(exercise(&mut f, &args(), true, &pr()).is_err());
        assert!(f.enabled);
        assert!(!f.calls.iter().any(|c| c == "MERGE"));
    }
    #[test]
    fn pr_emergency_land_already_disabled_never_posts() {
        let mut f = Fake::default();
        exercise(&mut f, &args(), true, &pr()).unwrap();
        assert!(!f
            .calls
            .iter()
            .any(|c| c.starts_with("POST") || c.starts_with("DELETE")));
        assert!(!f.enabled);
    }
    #[test]
    fn pr_emergency_land_restore_failure_and_mismatch_print_recovery() {
        for mismatch in [false, true] {
            let mut f = Fake {
                enabled: true,
                mismatch,
                fail_restore: !mismatch,
                ..Default::default()
            };
            let err = exercise(&mut f, &args(), true, &pr())
                .unwrap_err()
                .to_string();
            assert!(err.contains("ERROR"));
            assert!(
                err.contains("gh api -X POST repos/o/r/branches/main/protection/enforce_admins")
            );
        }
    }
    #[test]
    fn pr_emergency_land_refusals_and_dry_run_do_not_write() {
        let mut cases = Vec::new();
        cases.push((false, pr())); // all production non-human/headless/role evidence maps to this input
        for (field, value) in [
            ("state", json!("CLOSED")),
            ("baseRefName", json!("other")),
            ("mergeable", json!("CONFLICTING")),
            ("mergeable", json!("UNKNOWN")),
            ("additions", json!(201)),
            ("changedFiles", json!(11)),
        ] {
            let mut p = pr();
            p[field] = value;
            cases.push((true, p));
        }
        for (human, p) in cases {
            let mut f = Fake::default();
            assert!(exercise(&mut f, &args(), human, &p).is_err());
            assert!(f.calls.is_empty());
        }
        let mut a = args();
        a.dry_run = true;
        let mut f = Fake::default();
        exercise(&mut f, &a, true, &pr()).unwrap();
        assert!(f.calls.is_empty());
        a.allow_large = true;
        let mut p = pr();
        p["changedFiles"] = json!(20);
        p["additions"] = json!(1000);
        exercise(&mut f, &a, true, &p).unwrap();
        assert!(f.calls.is_empty());
    }
}
