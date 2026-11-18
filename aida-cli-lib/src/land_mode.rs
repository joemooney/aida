//! `aida land mode status|set|reconcile`: the landing mode and the forge's
//! required checks, kept in step. `set` and `reconcile` change the default
//! branch's protection, so they need the operator at a terminal or the
//! advisor seat under a mandate that includes protected paths
//! ([`crate::gate::ship::landing_mode_change_refusal`]). Every change is
//! read back from the forge, restored on a mismatch, and appended to
//! `.aida/land/landing-audit.jsonl`.
// trace:STORY-2897 | ai:claude

use crate::landing_policy::{LandingMode, LandingPolicy};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const AUDIT: &str = ".aida/land/landing-audit.jsonl";

/// The forge's required status checks on the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Required {
    pub contexts: Vec<String>,
    pub strict: bool,
}

/// Read and replace the required status checks of the default branch. Only
/// the check list is replaced; `strict` is written back as it was read.
pub(crate) trait Protection {
    fn read(&mut self) -> Result<Required>;
    fn write(&mut self, contexts: &[String], strict: bool) -> Result<()>;
}

/// GitHub's branch protection, through `gh api`.
struct GhProtection {
    root: PathBuf,
    endpoint: String,
}

impl GhProtection {
    fn new(root: &Path) -> Result<Self> {
        let branch = crate::gate::review_carryover::protected_base(root)
            .map_err(|e| anyhow::anyhow!("{}", e.reason()))
            .context("could not resolve the default branch")?;
        Ok(Self {
            root: root.to_path_buf(),
            endpoint: format!(
                "repos/{{owner}}/{{repo}}/branches/{}/protection/required_status_checks",
                crate::pr_emergency_land::encode(&branch)
            ),
        })
    }

    fn gh(&self, args: &[&str], input: Option<&str>) -> Result<String> {
        crate::forge::gh_api(&self.root, args, input)
    }
}

impl Protection for GhProtection {
    fn read(&mut self) -> Result<Required> {
        let body = self.gh(&[&self.endpoint], None)?;
        parse_required(&body)
    }

    fn write(&mut self, contexts: &[String], strict: bool) -> Result<()> {
        let body = serde_json::json!({ "strict": strict, "contexts": contexts }).to_string();
        self.gh(
            &["-X", "PATCH", &self.endpoint, "--input", "-"],
            Some(&body),
        )
        .map(|_| ())
    }
}

/// PURE: the `required_status_checks` object GitHub returns.
pub(crate) fn parse_required(body: &str) -> Result<Required> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("the forge's required checks are not JSON")?;
    let contexts = value
        .get("contexts")
        .and_then(|c| c.as_array())
        .context("the forge's required checks carry no `contexts` list")?
        .iter()
        .filter_map(|c| c.as_str().map(str::to_string))
        .collect();
    Ok(Required {
        contexts,
        strict: value
            .get("strict")
            .and_then(|s| s.as_bool())
            .unwrap_or(false),
    })
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<&String> = a.iter().collect();
    let mut b: Vec<&String> = b.iter().collect();
    a.sort();
    a.dedup();
    b.sort();
    b.dedup();
    a == b
}

/// Whether a restore of the previous checks took, by its own read-back.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Restore {
    /// Written and read back equal to the previous list.
    Verified,
    /// Written, but its read-back failed or shows another list.
    Unverified(String),
    /// The restore write itself failed.
    Failed(String),
}

/// What a reconcile did to the forge.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The forge already required the wanted list; nothing was written.
    Unchanged,
    /// Written and read back equal to the wanted list.
    Applied,
    /// The change did not take, and a read-back shows the previous list.
    NotApplied { reason: String },
    /// The change did not take and the forge was left changed, so the
    /// previous list was written back; `restore` is that write's verdict.
    RolledBack { reason: String, restore: Restore },
}

/// What a reconcile did.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reconciled {
    pub before: Required,
    /// The last list read back from the forge, if the last read succeeded.
    pub after: Option<Required>,
    pub outcome: Outcome,
}

impl Reconciled {
    /// The forge requires the wanted list.
    pub(crate) fn in_step(&self) -> bool {
        matches!(self.outcome, Outcome::Unchanged | Outcome::Applied)
    }

    /// The short outcome name the audit and the event record.
    pub(crate) fn outcome_name(&self) -> &'static str {
        match &self.outcome {
            Outcome::Unchanged => "unchanged",
            Outcome::Applied => "applied",
            Outcome::NotApplied { .. } => "not-applied",
            Outcome::RolledBack {
                restore: Restore::Verified,
                ..
            } => "rolled-back",
            Outcome::RolledBack { .. } => "unresolved",
        }
    }

    /// Why a reconcile that is not in step failed, with the forge's state.
    pub(crate) fn failure(&self) -> Option<String> {
        let before = self.before.contexts.join(", ");
        match &self.outcome {
            Outcome::Unchanged | Outcome::Applied => None,
            Outcome::NotApplied { reason } => Some(format!(
                "{reason}; the forge still requires [{before}], unchanged"
            )),
            Outcome::RolledBack { reason, restore } => Some(match restore {
                Restore::Verified => format!("{reason}; restored [{before}] (read back)"),
                Restore::Unverified(why) => format!(
                    "{reason}; restoring [{before}] could not be confirmed ({why}). Branch \
                     protection is UNRESOLVED: check it with `aida land mode status` and set the \
                     required checks by hand"
                ),
                Restore::Failed(why) => format!(
                    "{reason}; restoring [{before}] also failed ({why}). Branch protection is \
                     UNRESOLVED: check it with `aida land mode status` and set the required \
                     checks by hand"
                ),
            }),
        }
    }
}

/// Make the forge require exactly `want`, then read it back. Fails only when
/// the forge's checks cannot be read at all (nothing is written then). A
/// write the forge refuses, an unreadable read-back, or one that differs
/// from `want` is a failed verification: unless a read-back shows the
/// previous list, that list is written back and read back again.
// trace:STORY-2897 | ai:claude
pub(crate) fn reconcile(forge: &mut dyn Protection, want: &[String]) -> Result<Reconciled> {
    let before = forge.read()?;
    if same_set(&before.contexts, want) {
        return Ok(Reconciled {
            after: Some(before.clone()),
            before,
            outcome: Outcome::Unchanged,
        });
    }
    let wrote = forge.write(want, before.strict);
    let read = forge.read();
    let reason = match (&wrote, &read) {
        (Ok(()), Ok(after)) if same_set(&after.contexts, want) => {
            return Ok(Reconciled {
                after: Some(after.clone()),
                before,
                outcome: Outcome::Applied,
            })
        }
        (Err(e), _) => format!("the forge refused [{}] ({e:#})", want.join(", ")),
        (Ok(()), Err(e)) => format!(
            "the forge accepted [{}] but could not be read back ({e:#})",
            want.join(", ")
        ),
        (Ok(()), Ok(after)) => format!(
            "the forge requires [{}] after it was asked for [{}]",
            after.contexts.join(", "),
            want.join(", ")
        ),
    };
    if let Ok(after) = &read {
        if same_set(&after.contexts, &before.contexts) {
            return Ok(Reconciled {
                after: Some(after.clone()),
                before,
                outcome: Outcome::NotApplied { reason },
            });
        }
    }
    let (after, restore) = match forge.write(&before.contexts, before.strict) {
        Err(e) => (read.ok(), Restore::Failed(format!("{e:#}"))),
        Ok(()) => match forge.read() {
            Ok(now) if same_set(&now.contexts, &before.contexts) => (Some(now), Restore::Verified),
            Ok(now) => {
                let why = format!("the forge requires [{}]", now.contexts.join(", "));
                (Some(now), Restore::Unverified(why))
            }
            Err(e) => (
                None,
                Restore::Unverified(format!("its read-back failed: {e:#}")),
            ),
        },
    };
    Ok(Reconciled {
        before,
        after,
        outcome: Outcome::RolledBack { reason, restore },
    })
}

/// PURE: how the forge differs from what the effective mode requires.
pub(crate) fn drift(policy: &LandingPolicy, forge: &Required) -> Vec<String> {
    let want = policy.required_checks();
    if same_set(want, &forge.contexts) {
        return Vec::new();
    }
    let missing: Vec<&str> = want
        .iter()
        .filter(|w| !forge.contexts.contains(w))
        .map(String::as_str)
        .collect();
    let extra: Vec<&str> = forge
        .contexts
        .iter()
        .filter(|c| !want.contains(c))
        .map(String::as_str)
        .collect();
    let mut out = Vec::new();
    if !missing.is_empty() {
        out.push(format!(
            "the forge does not require {} ({} mode does)",
            missing.join(", "),
            policy.mode.as_str()
        ));
    }
    if !extra.is_empty() {
        out.push(format!(
            "the forge also requires {} ({} mode does not)",
            extra.join(", "),
            policy.mode.as_str()
        ));
    }
    out
}

/// Who is acting, for the audit: the validated grant and the mandate that
/// covered the change, or the operator at a terminal.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct Actor {
    pub user: String,
    pub floor: bool,
    pub seat: Option<String>,
    pub grant_id: Option<String>,
    pub principal: Option<String>,
    pub mandate: Option<String>,
}

/// The caller, if it may change the landing mode: the operator at a
/// terminal, or the advisor seat whose validated grant belongs to the
/// principal who minted the active mandate that includes protected paths.
// trace:STORY-2897 | ai:claude
fn acting_caller(root: &Path) -> Result<Actor> {
    let floor = crate::gate::role_authority::has_integrity_floor_authority();
    let grant = crate::gate::seat_authority::current_grant(root);
    let now = chrono::Utc::now();
    let mandate = match crate::gate::mandate::current_state() {
        crate::gate::mandate::MandateState::Active(m)
            if m.include_gate_paths && m.revoked_at.is_none() && now < m.expires_at =>
        {
            Some(m)
        }
        _ => None,
    };
    if let Some(why) = crate::gate::ship::landing_mode_change_refusal(
        floor,
        mandate.as_ref().map(|m| m.principal.as_str()),
        grant
            .as_ref()
            .map(|g| (g.seat.as_str(), g.principal.as_str())),
    ) {
        bail!("{why}");
    }
    Ok(Actor {
        user: crate::current_user_id(None),
        floor,
        seat: grant.as_ref().map(|g| g.seat.clone()),
        grant_id: grant.as_ref().map(|g| g.id.clone()),
        principal: grant.map(|g| g.principal),
        mandate: mandate.map(|m| m.id),
    })
}

fn require_github(root: &Path) -> Result<()> {
    if crate::forge::resolve_forge_kind(root) != crate::forge::ForgeKind::GitHub {
        bail!(
            "`aida land mode` reconciles GitHub branch protection only; this project's forge is \
             not GitHub, so its required checks are left as they are"
        );
    }
    Ok(())
}

/// The audit journal, opened before any forge write so a change is never
/// made that cannot be recorded.
// trace:STORY-2897 | ai:claude
struct Journal {
    path: PathBuf,
    file: std::fs::File,
}

impl Journal {
    fn open(root: &Path) -> Result<Self> {
        let path = root.join(AUDIT);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("could not open the audit {}", path.display()))?;
        Ok(Self { path, file })
    }

    fn record(&mut self, line: &serde_json::Value) -> Result<()> {
        use std::io::Write;
        writeln!(self.file, "{line}")
            .and_then(|()| self.file.flush())
            .with_context(|| format!("could not append to {}", self.path.display()))
    }
}

/// The `[landing] mode` edit `set` makes, checked before the forge is
/// touched: the file parses, may be written here (not a projected copy),
/// and its directory takes the atomic write's staging file.
// trace:STORY-2897 | ai:claude
fn preflight_config(path: &Path) -> Result<()> {
    crate::config_edit::load_doc(path)?;
    #[cfg(unix)]
    aida_core::adoption_projection::refuse_owned_write(path)
        .with_context(|| format!("{} may not be written here", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("{} is not writable", dir.display()))?;
    if path.exists() {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .with_context(|| format!("{} is not writable", path.display()))?;
    }
    Ok(())
}

/// One mode change: what it is asked to do.
// trace:STORY-2897 | ai:claude
pub(crate) struct Change<'a> {
    pub action: &'static str,
    pub from: Option<LandingMode>,
    pub to: LandingMode,
    pub want: Vec<String>,
    /// `set` writes `[landing] mode` here once the forge is in step.
    pub config: Option<&'a Path>,
}

/// Apply `change` to `forge` and journal it. The audit and the config edit
/// are checked first, so a refusal there changes nothing. The attempt is
/// journaled before the forge is written and its observed outcome after,
/// failures and rollbacks included; a `LandModeChanged` event follows any
/// forge or config change. A partial success (forge changed, config not
/// written) fails with the recovery steps.
// trace:STORY-2897 | ai:claude
pub(crate) fn apply_change(
    root: &Path,
    forge: &mut dyn Protection,
    actor: &Actor,
    change: &Change<'_>,
) -> Result<Reconciled> {
    let mut journal = Journal::open(root)?;
    if let Some(path) = change.config {
        preflight_config(path)?;
    }
    let base = serde_json::json!({
        "action": change.action,
        "from": change.from.map(LandingMode::as_str),
        "to": change.to.as_str(),
        "want": change.want,
        "user": actor.user,
        "operator_at_terminal": actor.floor,
        "seat": actor.seat,
        "grant_id": actor.grant_id,
        "principal": actor.principal,
        "mandate": actor.mandate,
    });
    let entry = |phase: &str, extra: serde_json::Value| {
        let mut line = base.clone();
        line["at"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
        line["phase"] = serde_json::json!(phase);
        if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
            line.extend(extra.clone());
        }
        line
    };
    journal.record(&entry("attempt", serde_json::json!({})))?;
    let done = match reconcile(forge, &change.want) {
        Ok(done) => done,
        Err(e) => {
            let _ = journal.record(&entry(
                "outcome",
                serde_json::json!({
                    "outcome": "unread",
                    "error": format!("{e:#}"),
                    "read_back_ok": false,
                }),
            ));
            return Err(e.context("the forge's required checks could not be read; nothing changed"));
        }
    };
    let config_written = match (change.config, done.in_step()) {
        (Some(path), true) => Some(
            crate::config_edit::set_kvs(
                path,
                "landing",
                &[
                    ("mode", toml_edit::Value::from(change.to.as_str())),
                    ("mode_set_by", toml_edit::Value::from(actor.user.as_str())),
                    (
                        "mode_set_at",
                        toml_edit::Value::from(
                            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                        ),
                    ),
                ],
            )
            .with_context(|| format!("could not write {}", path.display())),
        ),
        _ => None,
    };
    let outcome_line = entry(
        "outcome",
        serde_json::json!({
            "outcome": done.outcome_name(),
            "forge_before": done.before.contexts,
            "forge_after": done.after.as_ref().map(|a| &a.contexts),
            "read_back_ok": done.in_step(),
            "error": done.failure(),
            "config_written": config_written.as_ref().map(Result::is_ok),
            "config_error": config_written
                .as_ref()
                .and_then(|r| r.as_ref().err())
                .map(|e| format!("{e:#}")),
        }),
    );
    let journaled = journal.record(&outcome_line);
    let touched = !matches!(
        done.outcome,
        Outcome::Unchanged | Outcome::NotApplied { .. }
    ) || matches!(config_written, Some(Ok(())));
    if touched {
        let mut event = crate::events::Event::new(
            None,
            "",
            crate::events::EventKind::LandModeChanged {
                action: change.action.to_string(),
                from: change.from.map(|m| m.as_str().to_string()),
                to: change.to.as_str().to_string(),
                outcome: done.outcome_name().to_string(),
                forge_before: done.before.contexts.clone(),
                forge_after: done.after.as_ref().map(|a| a.contexts.clone()),
                grant_id: actor.grant_id.clone(),
                mandate: actor.mandate.clone(),
            },
        );
        event.seat = crate::events::active_seat();
        crate::events::emit(root, &event);
    }
    if let Err(e) = journaled {
        // Never silent: the record goes to stderr when the journal fails.
        eprintln!("the audit record could not be written ({e:#}); it was:\n{outcome_line}");
    }
    if let Some(why) = done.failure() {
        bail!("{why}");
    }
    if let Some(Err(e)) = config_written {
        bail!(
            "the forge now requires [{}], but `[landing] mode` was not written: {e:#}. The forge \
             change is in the audit ({AUDIT}). To return the forge to the default branch's mode, \
             run `aida land mode reconcile`; or fix the file and run `aida land mode set {}` again.",
            done.after
                .as_ref()
                .map_or_else(|| change.want.join(", "), |a| a.contexts.join(", ")),
            change.to.as_str()
        );
    }
    Ok(done)
}

fn print_change(done: &Reconciled) {
    match (&done.outcome, &done.after) {
        (Outcome::Applied, Some(after)) => println!(
            "  forge required checks: [{}] -> [{}] (read back)",
            done.before.contexts.join(", "),
            after.contexts.join(", ")
        ),
        _ => println!(
            "  forge required checks already [{}]; nothing changed",
            done.before.contexts.join(", ")
        ),
    }
}

/// `aida land mode set <mode>`.
pub(crate) fn set(root: &Path, mode: LandingMode) -> Result<()> {
    let actor = acting_caller(root)?;
    require_github(root)?;
    let policy = crate::landing_policy::load_reporting(root);
    let from = policy.mode;
    let want = match mode {
        LandingMode::CiGated => policy.ci_gated_checks.clone(),
        LandingMode::ReviewGated => policy.review_gated_checks.clone(),
    };
    let path = root.join(".aida").join("config.toml");
    let done = apply_change(
        root,
        &mut GhProtection::new(root)?,
        &actor,
        &Change {
            action: "set",
            from: Some(from),
            to: mode,
            want,
            config: Some(&path),
        },
    )?;
    println!("landing mode: {} -> {}", from.as_str(), mode.as_str());
    print_change(&done);
    println!(
        "  .aida/config.toml now says `[landing] mode = {}`; landing reads the default \
         branch's copy, so land this edit in a reviewed PR for it to take effect.",
        aida_core::toml_quote::toml_string(mode.as_str())
    );
    println!("  to undo: aida land mode set {}", from.as_str());
    Ok(())
}

/// `aida land mode reconcile`: re-apply the effective mode's check list.
pub(crate) fn reconcile_effective(root: &Path) -> Result<()> {
    let actor = acting_caller(root)?;
    require_github(root)?;
    let policy = crate::landing_policy::load_reporting(root);
    let done = apply_change(
        root,
        &mut GhProtection::new(root)?,
        &actor,
        &Change {
            action: "reconcile",
            from: None,
            to: policy.mode,
            want: policy.required_checks().to_vec(),
            config: None,
        },
    )?;
    println!("landing mode: {} (default branch)", policy.mode.as_str());
    print_change(&done);
    Ok(())
}

/// `aida land mode status`.
pub(crate) fn status(root: &Path) -> Result<()> {
    let policy = crate::landing_policy::load_reporting(root);
    let declared = crate::land::run::project_config(root)
        .and_then(|c| c.get("landing")?.get("mode")?.as_str().map(str::to_string));
    println!("landing mode: {} (default branch)", policy.mode.as_str());
    match declared {
        Some(local) if local != policy.mode.as_str() => println!(
            "  this checkout's .aida/config.toml says `{local}`; it takes effect once it is on \
             the default branch"
        ),
        _ => {}
    }
    println!(
        "  {} mode requires: {}",
        policy.mode.as_str(),
        policy.required_checks().join(", ")
    );
    let forge = if crate::forge::resolve_forge_kind(root) == crate::forge::ForgeKind::GitHub {
        GhProtection::new(root).and_then(|mut p| p.read())
    } else {
        Err(anyhow::anyhow!("not a GitHub project"))
    };
    match forge {
        Ok(required) => {
            println!("  the forge requires: {}", required.contexts.join(", "));
            let drift = drift(&policy, &required);
            if drift.is_empty() {
                println!("  in step");
            } else {
                for line in drift {
                    println!("  DRIFT: {line}; run `aida land mode reconcile`");
                }
            }
        }
        Err(e) => println!("  the forge's required checks could not be read: {e:#}"),
    }
    if let Ok(text) = std::fs::read_to_string(root.join(AUDIT)) {
        let lines: Vec<&str> = text.lines().collect();
        if !lines.is_empty() {
            println!("  recent changes ({AUDIT}):");
            for line in lines.iter().rev().take(5).rev() {
                println!("    {line}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_2897_land_mode_tests.rs"]
mod story_2897_land_mode_tests;
