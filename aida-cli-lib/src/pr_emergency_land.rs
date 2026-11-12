//! Operator emergency merge; protection restoration precedes all success reporting.
// trace:STORY-2732 | ai:codex
use crate::forge::emergency_land::GitHub;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(clap::Args, Debug)]
pub struct EmergencyLandArgs {
    /// Open GitHub PR to merge into the default branch.
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
    fn recovery_command(&self, scope: &str) -> String;
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
        self.forge.recovery_command(self.endpoint)
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

/// The finally boundary includes ambiguous DELETE failures and panics, not just merge errors.
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
    ensure!(
        crate::forge::resolve_forge_kind(&root) == crate::forge::ForgeKind::GitHub,
        "emergency-land supports GitHub only"
    );
    let mut forge = GitHub::new(root.clone())?;
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
        Err(_) => required_checks(&mut forge, name, &pr, &backup)?,
    };
    println!("Preconditions passed for PR {}. Plan: GET protection; save backup; DELETE enforce_admins only if enabled; gh pr merge --squash --admin; ALWAYS restore; GET and compare protection; append audit and spec comment; {}.", args.n, if args.no_watch { "skip watch" } else { "watch main squash CI for up to 30 minutes" });
    if args.dry_run {
        println!("Dry run: no API or local writes.");
        return Ok(());
    }
    let operator = forge.api("GET", "user")?["login"]
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
        watch(&forge, &sha, &mut signals)?;
    }
    Ok(())
}

fn encode(s: &str) -> String {
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

fn watch(
    forge: &GitHub,
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
        let runs: Value = serde_json::from_str(&forge.command_with_budget(
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
        )?)?;
        let runs = runs.as_array().context("invalid CI run list")?;
        if runs.iter().any(|r| {
            matches!(
                r["conclusion"].as_str(),
                Some("failure" | "timed_out" | "cancelled" | "action_required")
            )
        }) {
            bail!("Post-merge CI is red. Revert through a new PR: git fetch origin && git switch -c revert-emergency-{} origin/HEAD && git revert {} && git push -u origin HEAD && gh pr create --fill; then aida pr emergency-land <REVERT-PR>. No automatic revert was made.", &sha[..sha.len().min(12)], sha);
        }
        if !runs.is_empty() && runs.iter().all(|r| r["status"] == "completed") {
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

#[cfg(test)]
mod tests {
    use super::*;
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
        fn recovery_command(&self, scope: &str) -> String {
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
