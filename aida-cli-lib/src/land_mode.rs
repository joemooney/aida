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

/// Which GitHub App must provide a required check. A check bound to an app
/// is satisfied only by that app's status; dropping the binding would let
/// any app satisfy it, so a binding is carried through every write.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AppBinding {
    /// Only this app (`app_id`).
    App(i64),
    /// Any app (`app_id` null, or `-1` when written).
    Any,
}

/// One required check and the app bound to it.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub context: String,
    pub app: AppBinding,
}

impl Check {
    pub(crate) fn new(context: &str, app: AppBinding) -> Self {
        Self {
            context: context.to_string(),
            app,
        }
    }

    fn json(&self) -> serde_json::Value {
        match self.app {
            AppBinding::App(id) => serde_json::json!({ "context": self.context, "app_id": id }),
            AppBinding::Any => serde_json::json!({ "context": self.context, "app_id": -1 }),
        }
    }

    fn label(&self) -> String {
        match self.app {
            AppBinding::App(id) => format!("{} (app {id})", self.context),
            _ => self.context.clone(),
        }
    }
}

/// The audit form of a check list: `{context, app_id}` (`app_id` null for
/// any app).
fn audit_checks(checks: &[Check]) -> serde_json::Value {
    serde_json::Value::Array(
        checks
            .iter()
            .map(|c| match c.app {
                AppBinding::App(id) => serde_json::json!({ "context": c.context, "app_id": id }),
                _ => serde_json::json!({ "context": c.context, "app_id": null }),
            })
            .collect(),
    )
}

/// The forge's required status checks on the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Required {
    pub checks: Vec<Check>,
    pub strict: bool,
}

impl Required {
    /// The required check names.
    pub(crate) fn contexts(&self) -> Vec<String> {
        self.checks.iter().map(|c| c.context.clone()).collect()
    }

    /// The checks for display, each with its app when bound.
    pub(crate) fn shown(&self) -> String {
        show(&self.checks)
    }
}

fn show(checks: &[Check]) -> String {
    checks
        .iter()
        .map(Check::label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read and replace the required status checks of the default branch. Only
/// the check list is replaced, each check with its app binding; `strict` is
/// written back as it was read.
pub(crate) trait Protection {
    fn read(&mut self) -> Result<Required>;
    fn write(&mut self, checks: &[Check], strict: bool) -> Result<()>;
}

/// GitHub's branch protection, through `gh api`.
struct GhProtection {
    root: PathBuf,
    endpoint: String,
}

impl GhProtection {
    fn new(root: &Path) -> Result<Self> {
        Ok(Self::for_branch(root, &live_default_branch(root)?))
    }

    /// The protection of `branch`, the default branch the caller resolved
    /// (and pinned its policy to).
    fn for_branch(root: &Path, branch: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            endpoint: format!(
                "repos/{{owner}}/{{repo}}/branches/{}/protection/required_status_checks",
                crate::pr_emergency_land::encode(branch)
            ),
        }
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

    fn write(&mut self, checks: &[Check], strict: bool) -> Result<()> {
        // Explicit `checks`, never the deprecated `contexts`: a `contexts`
        // write drops every check's app binding. trace:STORY-2897 | ai:claude
        let checks: Vec<serde_json::Value> = checks.iter().map(Check::json).collect();
        let body = serde_json::json!({ "strict": strict, "checks": checks }).to_string();
        self.gh(
            &["-X", "PATCH", &self.endpoint, "--input", "-"],
            Some(&body),
        )
        .map(|_| ())
    }
}

/// The remote's default branch as the forge has it now (`ls-remote`).
fn live_default_branch(root: &Path) -> Result<String> {
    crate::gate::review_carryover::protected_base(root)
        .map_err(|e| anyhow::anyhow!("{}", e.reason()))
        .context("could not resolve the default branch")
}

/// The landing policy a mutation is decided under, bound to the protection
/// it changes: `branch` (the live default branch) is fetched from origin and
/// the policy read from that exact fetched commit, never from a stale local
/// ref or another branch. Anything that cannot be established refuses.
// trace:STORY-2897 | ai:claude
pub(crate) fn bound_policy(root: &Path, branch: &str) -> Result<LandingPolicy> {
    crate::pr_cmd::fetch_ship_refs(root, branch, branch).map_err(|e| {
        anyhow::anyhow!(
            "the default branch `{branch}` could not be fetched ({e}), so its landing policy \
             cannot be established; nothing was changed"
        )
    })?;
    let sha =
        crate::trusted_config::rev_parse_commit(root, &format!("refs/remotes/origin/{branch}"))
            .with_context(|| {
                format!(
                "origin/{branch} does not resolve after the fetch, so its landing policy cannot \
                 be established; nothing was changed"
            )
            })?;
    let policy = crate::landing_policy::from_body(
        crate::trusted_config::read_config_at_sha(root, &sha).as_deref(),
    );
    for w in &policy.warnings {
        eprintln!("  warning: {w}");
    }
    Ok(policy)
}

/// PURE: the `required_status_checks` object GitHub returns. Its `checks`
/// carry each check's app binding (`app_id` null: any app); a response with
/// only the legacy `contexts` list binds none.
pub(crate) fn parse_required(body: &str) -> Result<Required> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("the forge's required checks are not JSON")?;
    let checks = match value.get("checks").and_then(|c| c.as_array()) {
        Some(items) => items
            .iter()
            .map(|item| {
                let context = item
                    .get("context")
                    .and_then(|c| c.as_str())
                    .context("a required check carries no `context`")?;
                let app = match item.get("app_id").and_then(|a| a.as_i64()) {
                    Some(id) if id > 0 => AppBinding::App(id),
                    _ => AppBinding::Any,
                };
                Ok(Check::new(context, app))
            })
            .collect::<Result<Vec<_>>>()?,
        None => value
            .get("contexts")
            .and_then(|c| c.as_array())
            .context("the forge's required checks carry no `checks` or `contexts` list")?
            .iter()
            .filter_map(|c| c.as_str().map(|c| Check::new(c, AppBinding::Any)))
            .collect(),
    };
    Ok(Required {
        checks,
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

/// Whether `got` requires exactly the checks of `want`, each from the app
/// `want` binds it to.
// trace:STORY-2897 | ai:claude
pub(crate) fn same_checks(want: &[Check], got: &[Check]) -> bool {
    let names = |c: &[Check]| c.iter().map(|c| c.context.clone()).collect::<Vec<_>>();
    same_set(&names(want), &names(got))
        && want.iter().all(|w| {
            got.iter()
                .filter(|g| g.context == w.context)
                .all(|g| g.app == w.app)
        })
}

/// The checks to require for the names `want`: a check the forge already
/// requires keeps its binding; one it no longer requires takes the binding
/// the trusted record holds for it (`trusted`). A name with neither is
/// returned as missing: GitHub is never left to pick its app.
// trace:STORY-2897 | ai:claude
pub(crate) fn target_checks(
    want: &[String],
    before: &Required,
    trusted: &std::collections::BTreeMap<String, AppBinding>,
) -> std::result::Result<Vec<Check>, Vec<String>> {
    let mut missing = Vec::new();
    let checks = want
        .iter()
        .filter_map(|name| {
            let app = before
                .checks
                .iter()
                .find(|c| &c.context == name)
                .map(|c| c.app)
                .or_else(|| trusted.get(name).copied());
            match app {
                Some(app) => Some(Check::new(name, app)),
                None => {
                    missing.push(name.clone());
                    None
                }
            }
        })
        .collect();
    if missing.is_empty() {
        Ok(checks)
    } else {
        Err(missing)
    }
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
        let before = self.before.shown();
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

/// Make the forge require exactly `want`, then read it back. `authority`
/// runs immediately before the write (after the forge's checks are read,
/// which can take a while): a lapse refuses with nothing written. Fails only
/// then, or when the forge's checks cannot be read at all. A
/// write the forge refuses, an unreadable read-back, or one that differs
/// from `want` is a failed verification: unless a read-back shows the
/// previous list, that list is written back and read back again.
// trace:STORY-2897 | ai:claude
pub(crate) fn reconcile(
    forge: &mut dyn Protection,
    want: &[String],
    store: &crate::land_bindings::BindingStore,
    authority: &mut dyn FnMut() -> Result<()>,
) -> Result<Reconciled> {
    let before = forge.read()?;
    let refuse = |why: String| {
        Err(crate::gate::ship::ShipRefusal::Review.error(format!(
            "{why}. Branch protection is UNRESOLVED for this change and was left unchanged \
             (it still requires [{}])",
            before.shown()
        )))
    };
    let trusted = match store.load() {
        Ok(trusted) => trusted,
        Err(e) => {
            return refuse(format!(
                "the trusted check-binding record is unusable ({e:#})"
            ))
        }
    };
    let want = match target_checks(want, &before, &trusted) {
        Ok(want) => want,
        Err(missing) => {
            return refuse(format!(
                "no trusted app binding is on record for {}, so it cannot be required again \
                 from the app it was bound to",
                missing.join(", ")
            ))
        }
    };
    let want = want.as_slice();
    if same_checks(want, &before.checks) {
        return Ok(Reconciled {
            after: Some(before.clone()),
            before,
            outcome: Outcome::Unchanged,
        });
    }
    authority()?;
    // The bindings this PATCH may remove are recorded first, durably: a
    // crash after the PATCH still leaves them to restore from.
    if let Err(e) = store.record(&before.checks) {
        return refuse(format!(
            "the check bindings could not be recorded before the change ({e:#})"
        ));
    }
    let wrote = forge.write(want, before.strict);
    let read = forge.read();
    let reason = match (&wrote, &read) {
        (Ok(()), Ok(after)) if same_checks(want, &after.checks) => {
            return Ok(Reconciled {
                after: Some(after.clone()),
                before,
                outcome: Outcome::Applied,
            })
        }
        (Err(e), _) => format!("the forge refused [{}] ({e:#})", show(want)),
        (Ok(()), Err(e)) => format!(
            "the forge accepted [{}] but could not be read back ({e:#})",
            show(want)
        ),
        (Ok(()), Ok(after)) => format!(
            "the forge requires [{}] after it was asked for [{}]",
            after.shown(),
            show(want)
        ),
    };
    if let Ok(after) = &read {
        if same_checks(&before.checks, &after.checks) {
            return Ok(Reconciled {
                after: Some(after.clone()),
                before,
                outcome: Outcome::NotApplied { reason },
            });
        }
    }
    let (after, restore) = restore(forge, &before, read.ok());
    Ok(Reconciled {
        before,
        after,
        outcome: Outcome::RolledBack { reason, restore },
    })
}

/// Write `before` back and read it back. A compensating restore is not
/// gated on authority: it returns the forge to the state the caller found
/// it in, which every refusal must leave. `last` is the last read-back.
// trace:STORY-2897 | ai:claude
fn restore(
    forge: &mut dyn Protection,
    before: &Required,
    last: Option<Required>,
) -> (Option<Required>, Restore) {
    match forge.write(&before.checks, before.strict) {
        Err(e) => (last, Restore::Failed(format!("{e:#}"))),
        Ok(()) => match forge.read() {
            Ok(now) if same_checks(&before.checks, &now.checks) => (Some(now), Restore::Verified),
            Ok(now) => {
                let why = format!("the forge requires [{}]", now.shown());
                (Some(now), Restore::Unverified(why))
            }
            Err(e) => (
                None,
                Restore::Unverified(format!("its read-back failed: {e:#}")),
            ),
        },
    }
}

/// PURE: how the forge differs from what the effective mode requires.
pub(crate) fn drift(policy: &LandingPolicy, forge: &Required) -> Vec<String> {
    let want = policy.required_checks();
    let have = forge.contexts();
    if same_set(want, &have) {
        return Vec::new();
    }
    let missing: Vec<&str> = want
        .iter()
        .filter(|w| !have.contains(w))
        .map(String::as_str)
        .collect();
    let extra: Vec<&str> = have
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

/// The authority re-check run at each mutation boundary: the same caller,
/// validated afresh, still holds the authority it began with (the same
/// operator floor, or the same grant and the same mandate). A lapse,
/// revocation or replacement refuses with the review category (exit 23).
// trace:STORY-2897 | ai:claude
pub(crate) fn recheck_caller(now: Result<Actor>, entry: &Actor) -> Result<()> {
    let refuse = |why: String| {
        Err(crate::gate::ship::ShipRefusal::Review.error(format!(
            "the authority this landing-mode change started under {why}; nothing more was \
             changed"
        )))
    };
    match now {
        Err(e) => refuse(format!("has lapsed ({e:#})")),
        Ok(now)
            if now.floor != entry.floor
                || now.grant_id != entry.grant_id
                || now.mandate != entry.mandate =>
        {
            refuse("was replaced while the change was in progress".to_string())
        }
        Ok(_) => Ok(()),
    }
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
/// written, or the audit outcome could not be appended) fails with recovery details.
// trace:STORY-2897 | ai:claude
pub(crate) fn apply_change(
    root: &Path,
    forge: &mut dyn Protection,
    actor: &Actor,
    change: &Change<'_>,
    store: &crate::land_bindings::BindingStore,
    authority: &mut dyn FnMut() -> Result<()>,
) -> Result<Reconciled> {
    let mut journal = Journal::open(root)?;
    apply_change_with_audit(root, forge, actor, change, store, authority, &mut |line| {
        journal.record(line)
    })
}

// trace:BUG-2691 | ai:codex
fn apply_change_with_audit(
    root: &Path,
    forge: &mut dyn Protection,
    actor: &Actor,
    change: &Change<'_>,
    store: &crate::land_bindings::BindingStore,
    authority: &mut dyn FnMut() -> Result<()>,
    record: &mut dyn FnMut(&serde_json::Value) -> Result<()>,
) -> Result<Reconciled> {
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
    record(&entry("attempt", serde_json::json!({})))?;
    let mut done = match reconcile(forge, &change.want, store, authority) {
        Ok(done) => done,
        Err(e) => {
            let refused = e.downcast_ref::<crate::gate::ship::ShipFailure>().is_some();
            let _ = record(&entry(
                "outcome",
                serde_json::json!({
                    "outcome": if refused { "refused" } else { "unread" },
                    "error": format!("{e:#}"),
                    "read_back_ok": false,
                }),
            ));
            if refused {
                return Err(e);
            }
            return Err(e.context("the forge's required checks could not be read; nothing changed"));
        }
    };
    // The config edit is a second mutation: authority is re-read before it.
    // A lapse here undoes the forge change (a compensating restore, not
    // gated on authority) and writes no config.
    let mut lapsed = None;
    if change.config.is_some() && done.in_step() {
        if let Err(e) = authority() {
            if done.outcome == Outcome::Applied {
                let (after, verdict) = restore(forge, &done.before, done.after.clone());
                done.after = after;
                done.outcome = Outcome::RolledBack {
                    reason: format!("{e:#}"),
                    restore: verdict,
                };
            }
            lapsed = Some(e);
        }
    }
    let config_written = match (change.config, done.in_step() && lapsed.is_none()) {
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
            "outcome": if lapsed.is_some() && done.in_step() { "refused" } else { done.outcome_name() },
            "forge_before": audit_checks(&done.before.checks),
            "forge_after": done.after.as_ref().map(|a| audit_checks(&a.checks)),
            "read_back_ok": done.in_step(),
            "error": done.failure(),
            "config_written": config_written.as_ref().map(Result::is_ok),
            "config_error": config_written
                .as_ref()
                .and_then(|r| r.as_ref().err())
                .map(|e| format!("{e:#}")),
        }),
    );
    let journaled = record(&outcome_line);
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
                forge_before: done.before.contexts(),
                forge_after: done.after.as_ref().map(Required::contexts),
                grant_id: actor.grant_id.clone(),
                mandate: actor.mandate.clone(),
            },
        );
        event.seat = crate::events::active_seat();
        crate::events::emit(root, &event);
    }
    let finish = || {
        if let Some(e) = lapsed {
            return Err(match done.failure() {
                // The forge change was undone (or could not be): say so.
                Some(why) => crate::gate::ship::ShipRefusal::Review.error(why),
                None => e,
            });
        }
        if let Some(why) = done.failure() {
            bail!("{why}");
        }
        if let Some(Err(e)) = config_written {
            let checks = done
                .after
                .as_ref()
                .map_or_else(|| change.want.join(", "), Required::shown);
            let mode = change.to.as_str();
            bail!(
                "the forge now requires [{checks}], but `[landing] mode` was not written: {e:#}. \
                 To return the forge to the default branch's mode, run `aida land mode reconcile`. \
                 Or fix the file and run `aida land mode set {mode}` again."
            );
        }
        Ok(done)
    };
    match journaled {
        Ok(()) => finish(),
        Err(e) => {
            let audit = format!(
                "the audit outcome could not be appended to {AUDIT} ({e:#}). Preserve this \
                 outcome for recovery and check the recorded state before retrying:\n{outcome_line}"
            );
            match finish() {
                Ok(_) if touched => bail!("landing mode change partially succeeded: {audit}"),
                Ok(_) => bail!("{audit}"),
                Err(original) => Err(original.context(audit)),
            }
        }
    }
}

fn print_change(done: &Reconciled) {
    match (&done.outcome, &done.after) {
        (Outcome::Applied, Some(after)) => println!(
            "  forge required checks: [{}] -> [{}] (read back)",
            done.before.shown(),
            after.shown()
        ),
        _ => println!(
            "  forge required checks already [{}]; nothing changed",
            done.before.shown()
        ),
    }
}

/// `aida land mode set <mode>`.
pub(crate) fn set(root: &Path, mode: LandingMode) -> Result<()> {
    let actor = acting_caller(root)?;
    require_github(root)?;
    let branch = live_default_branch(root)?;
    let policy = bound_policy(root, &branch)?;
    let from = policy.mode;
    let want = match mode {
        LandingMode::CiGated => policy.ci_gated_checks.clone(),
        LandingMode::ReviewGated => policy.review_gated_checks.clone(),
    };
    let path = root.join(".aida").join("config.toml");
    let done = apply_change(
        root,
        &mut GhProtection::for_branch(root, &branch),
        &actor,
        &Change {
            action: "set",
            from: Some(from),
            to: mode,
            want,
            config: Some(&path),
        },
        &crate::land_bindings::BindingStore::for_target(
            root,
            &branch,
            &actor.user,
            actor.grant_id.as_deref(),
        )?,
        &mut || recheck_caller(acting_caller(root), &actor),
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
    let branch = live_default_branch(root)?;
    let policy = bound_policy(root, &branch)?;
    let done = apply_change(
        root,
        &mut GhProtection::for_branch(root, &branch),
        &actor,
        &Change {
            action: "reconcile",
            from: None,
            to: policy.mode,
            want: policy.required_checks().to_vec(),
            config: None,
        },
        &crate::land_bindings::BindingStore::for_target(
            root,
            &branch,
            &actor.user,
            actor.grant_id.as_deref(),
        )?,
        &mut || recheck_caller(acting_caller(root), &actor),
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
            println!("  the forge requires: {}", required.shown());
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
