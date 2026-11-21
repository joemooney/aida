//! `aida lane relay`: hand an advisor's brief to an implementer lane on a
//! lane host, as a rework of an open change or as fresh work.
//!
//! The relay takes the spec's next relay round on the host (which first
//! pulls its store), posts the brief on the spec as its authoritative round
//! brief, waits until no lane for the spec runs on the host and the
//! host is above its memory floor, re-checks the change against the head
//! seen at invocation, then delivers the brief through the vendor's brief
//! mailbox and starts the lane with that brief named explicitly. It then
//! watches the lane's admission receipt; once the lane has started, earlier
//! round briefs on the spec are marked superseded, and not before. Every
//! stop is typed, written to the spec and mailed to the advisor.
//!
//! Two things keep a stale round from running. A later relay for the same
//! spec takes a higher round, and every host step of an earlier relay then
//! stops without launching. The launch itself carries a gate
//! ([`LaunchGate`], in [`GATE_ENV`]): the launcher re-checks the change's
//! head after the host admitted it, which can be long after the relay's own
//! check, and fresh work refuses to continue an open change or an existing
//! branch.
//!
//! The lane-host half (`aida lane relay-host …`) runs in the host's primary
//! checkout, over SSH or in-process when the lane runs on this machine. The
//! host's cold-start cap and memory floor are the ones a lane launch is
//! admitted under ([`crate::lane_admission`]); the relay adds no limits.
// trace:TASK-3826 | ai:claude

use anyhow::{Context, Result};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::{RelayArgs, RelayHostCommand};

/// Consecutive "no lane process and no receipt" observations that mean the
/// launcher exited without recording an attempt.
const VANISHED_POLLS: u32 = 3;
/// Carries a relayed launch's [`LaunchGate`] from the lane host's launch
/// step to the launcher it starts. It only adds refusals.
pub(crate) const GATE_ENV: &str = "AIDA_LANE_RELAY_GATE";
const DEFAULT_LAUNCHER: [&str; 8] = [
    "bash",
    "scripts/lane-run.sh",
    "{spec}",
    "--role",
    "implementer",
    "--no-human",
    "--vendor",
    "{vendor}",
];

/// `[lane_relay]` in the project's `.aida/config.toml`: policy only. What
/// names a machine or a program lives in [`MachineConfig`], outside the
/// checkout.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RelayConfig {
    /// Default `--vendor`.
    vendor: Option<String>,
    /// Text appended to every relayed brief that does not already contain it.
    brief_footer: Option<String>,
}

pub(crate) fn load_config(root: &Path) -> Result<RelayConfig> {
    let path = root.join(".aida/config.toml");
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };
    let config: toml::Value =
        toml::from_str(&body).with_context(|| format!("invalid TOML in {}", path.display()))?;
    match config.get("lane_relay") {
        Some(section) => section
            .clone()
            .try_into()
            .with_context(|| format!("invalid [lane_relay] in {}", path.display())),
        None => Ok(Default::default()),
    }
}

/// `~/.aida/lane-relay.toml`: this machine's relay settings. They choose an
/// SSH destination and the program a launch runs, so they are read from the
/// operator's home, never from a checkout a branch can change.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MachineConfig {
    /// On the relaying machine: default `--host`.
    host: Option<String>,
    /// On the relaying machine: default `--host-dir`.
    host_dir: Option<String>,
    /// On the lane host: the launcher's leading arguments, with `{spec}` and
    /// `{vendor}` placeholders. The relay appends `--branch`,
    /// `--force-claim`, `--model` and `--brief`.
    launcher: Option<Vec<String>>,
}

pub(crate) fn load_machine_config(home: Option<&Path>) -> Result<MachineConfig> {
    let Some(path) = home.map(|home| home.join(".aida/lane-relay.toml")) else {
        return Ok(Default::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(body) => {
            toml::from_str(&body).with_context(|| format!("invalid settings in {}", path.display()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Everything a relay run resolved from its flags and project config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) vendor: String,
    pub(crate) model: Option<String>,
    /// The lane host's SSH alias; `None` runs the lane on this machine.
    pub(crate) host: Option<String>,
    pub(crate) host_dir: Option<String>,
    pub(crate) footer: Option<String>,
}

impl Plan {
    fn host_name(&self) -> &str {
        self.host.as_deref().unwrap_or("this host")
    }
}

fn valid_host(host: &str) -> bool {
    host.len() <= 253
        && host.starts_with(|c: char| c.is_ascii_alphanumeric())
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// An opaque value that becomes one launcher argument: never an option.
fn plain_value(what: &str, value: &str) -> Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && !value.starts_with('-')
            && value.len() <= 200
            && !value.chars().any(|c| c.is_control() || c.is_whitespace()),
        "invalid {what} `{value}`"
    );
    Ok(())
}

fn vendor_name(raw: &str) -> Result<String> {
    crate::vendors::launch_vendor_name(raw).map_err(|unknown| anyhow::anyhow!(unknown))
}

/// Resolve flags over the project's and this machine's settings. No vendor,
/// model or host is built in.
pub(crate) fn plan(args: &RelayArgs, root: &Path, home: Option<&Path>) -> Result<Plan> {
    let config = load_config(root)?;
    let machine = load_machine_config(home)?;
    let vendor = args
        .vendor
        .clone()
        .or(config.vendor)
        .or_else(|| aida_core::agents_config::resolve_default_vendor(root))
        .context("no lane agent chosen: pass --vendor or set `[lane_relay] vendor`")?;
    let vendor = vendor_name(&vendor)?;
    let model = args.model.clone().or_else(|| {
        aida_core::agents_config::resolve_agent_tuning(
            root,
            &vendor,
            aida_core::agents_config::AgentSeat::Implementer,
        )
        .model
    });
    if let Some(model) = &model {
        plain_value("model", model)?;
    }
    let host = args.host.clone().or(machine.host);
    if let Some(host) = &host {
        anyhow::ensure!(valid_host(host), "invalid lane host `{host}`");
    }
    Ok(Plan {
        vendor,
        model,
        host,
        host_dir: args.host_dir.clone().or(machine.host_dir),
        footer: config.brief_footer,
    })
}

/// A change as the forge reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrView {
    /// `OPEN`, `MERGED` or `CLOSED`.
    pub(crate) state: String,
    pub(crate) head: String,
    pub(crate) branch: String,
    pub(crate) title: String,
}

/// What the relay starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A rework of open change `pr`, valid only while it stays at `head`.
    Rework {
        pr: u64,
        branch: String,
        head: String,
    },
    /// New work on a new branch.
    Fresh,
}

/// Why a relay did not end in an admitted lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stop {
    HeadMoved {
        pr: u64,
        from: String,
        to: String,
    },
    PrNotOpen {
        pr: u64,
        state: String,
    },
    PrUnreadable {
        pr: u64,
        why: String,
    },
    LaneStillRunning {
        secs: u64,
    },
    HostCapacity {
        secs: u64,
        why: String,
    },
    HostUnreachable {
        secs: u64,
        why: String,
    },
    /// A later relay for the spec took over on the host.
    Superseded,
    /// The launcher's own gate stopped the launch after host admission.
    GateRefused {
        code: &'static str,
        detail: String,
    },
    /// A relay step failed; `code` names the step.
    Failed {
        code: &'static str,
        why: String,
    },
    AdmissionRefused {
        reason: String,
        detail: String,
    },
    AdmissionUnconfirmed {
        secs: u64,
    },
    LaunchVanished {
        log_tail: String,
    },
}

fn short(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

impl Stop {
    /// The typed reason recorded with the stop.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Stop::HeadMoved { .. } => "head-moved",
            Stop::PrNotOpen { state, .. } if state.eq_ignore_ascii_case("MERGED") => "pr-merged",
            Stop::PrNotOpen { .. } => "pr-closed",
            Stop::PrUnreadable { .. } => "pr-unreadable",
            Stop::LaneStillRunning { .. } => "lane-still-running",
            Stop::HostCapacity { .. } => "host-capacity",
            Stop::HostUnreachable { .. } => "host-unreachable",
            Stop::Superseded => "relay-superseded",
            Stop::GateRefused { code, .. } | Stop::Failed { code, .. } => *code,
            Stop::AdmissionRefused { .. } => "admission-refused",
            Stop::AdmissionUnconfirmed { .. } => "admission-unconfirmed",
            Stop::LaunchVanished { .. } => "launch-vanished",
        }
    }

    /// Whether the relayed brief is still the round in force after the stop:
    /// only when a launcher took it and may yet run it, or was refused for a
    /// reason a retry of the same round can clear. Every other stop voids
    /// the brief.
    pub(crate) fn brief_stays(&self) -> bool {
        matches!(
            self,
            Stop::AdmissionRefused { .. }
                | Stop::AdmissionUnconfirmed { .. }
                | Stop::LaunchVanished { .. }
        )
    }

    fn verdict(&self) -> &'static str {
        match self {
            Stop::AdmissionRefused { .. } => "REFUSED",
            Stop::AdmissionUnconfirmed { .. } | Stop::LaunchVanished { .. } => "UNCONFIRMED",
            Stop::Failed { .. } => "FAILED",
            _ => "SKIPPED",
        }
    }

    fn explain(&self) -> String {
        match self {
            Stop::HeadMoved { pr, from, to } => format!(
                "PR #{pr} head moved {} -> {}; the brief targeted the earlier round, so nothing was launched. Re-route needed.",
                short(from),
                short(to)
            ),
            Stop::PrNotOpen { pr, state } => format!(
                "PR #{pr} is {state}, not OPEN; the rework brief has no round left to run on, so nothing was launched."
            ),
            Stop::PrUnreadable { pr, why } => format!(
                "PR #{pr} could not be read ({why}); its head is unverified, so nothing was launched."
            ),
            Stop::LaneStillRunning { secs } => format!(
                "a lane for this spec was still running on the host after {secs}s; nothing was launched."
            ),
            Stop::HostCapacity { secs, why } => format!(
                "the lane host stayed below its limits for {secs}s ({why}); nothing was launched."
            ),
            Stop::HostUnreachable { secs, why } => format!(
                "the lane host could not be probed for {secs}s ({why}); nothing was launched."
            ),
            Stop::Superseded => "a later relay for this spec took over on the lane host; this one launched nothing.".into(),
            Stop::GateRefused { detail, .. } => format!(
                "the launcher stopped after the host admitted it, before any agent started: {detail}"
            ),
            Stop::Failed { why, .. } => format!(
                "{why}. The brief is withdrawn; if the lane host shows a lane for this spec, the launch got through before the failure."
            ),
            Stop::AdmissionRefused { reason, detail } => format!(
                "the lane host refused the launch before the agent started: {reason}: {detail}"
            ),
            Stop::AdmissionUnconfirmed { secs } => format!(
                "the lane was started but had not been admitted after {secs}s (still queued for a host start slot, or still in its pre-launch steps); check the host."
            ),
            Stop::LaunchVanished { log_tail } => format!(
                "the launcher exited without recording an attempt. Launch log tail:\n{log_tail}"
            ),
        }
    }

    pub(crate) fn line(&self, spec: &str, label: &str) -> String {
        format!(
            "Lane relay {} {spec} ({label}; {}): {}",
            self.verdict(),
            self.code(),
            self.explain()
        )
    }
}

/// How a relay run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Admitted {
        mode: Mode,
        /// The delivered brief's path on the lane host.
        brief: String,
        /// The launch log's path on the lane host.
        log: String,
    },
    Stopped(Stop),
}

/// Where a head check reads the change and waits between tries.
pub(crate) trait HeadSource {
    fn pr(&mut self, pr: u64) -> Result<PrView>;
    fn pause(&mut self, duration: Duration);
}

/// The head gate: the change must still be open at the head seen at
/// invocation. An unreadable change is not a pass.
fn head_gate(source: &mut dyn HeadSource, pr: u64, head: &str) -> Option<Stop> {
    let mut error = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            source.pause(Duration::from_secs(5));
        }
        match source.pr(pr) {
            Ok(now) if !now.state.eq_ignore_ascii_case("OPEN") => {
                return Some(Stop::PrNotOpen {
                    pr,
                    state: now.state,
                })
            }
            Ok(now) if now.head != head => {
                return Some(Stop::HeadMoved {
                    pr,
                    from: head.into(),
                    to: now.head,
                })
            }
            Ok(_) => return None,
            Err(err) => error = format!("{err:#}"),
        }
    }
    Some(Stop::PrUnreadable { pr, why: error })
}

/// A launch the relay's gate stopped, in the launcher, before any agent
/// started. `code` is recorded as the attempt's reason.
#[derive(Debug)]
pub(crate) struct GateRefusal {
    pub(crate) code: &'static str,
    pub(crate) detail: String,
}

impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "relayed launch stopped ({}): {}", self.code, self.detail)
    }
}

impl std::error::Error for GateRefusal {}

/// The reason code of a [`GateRefusal`] anywhere under `error`.
pub(crate) fn gate_refusal_code(error: &anyhow::Error) -> Option<&'static str> {
    error.downcast_ref::<GateRefusal>().map(|r| r.code)
}

/// Reason codes a launcher records for a [`GateRefusal`].
const GATE_CODES: [&str; 7] = [
    "head-moved",
    "pr-merged",
    "pr-closed",
    "pr-unreadable",
    "fresh-blocked",
    "rework-mismatch",
    "relay-gate-invalid",
];

/// What a relayed launch is for, carried to the launcher in [`GATE_ENV`] so
/// it is enforced where the lane actually starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchGate {
    /// Rework change `pr`, and only while it is open at `head`.
    Rework { pr: u64, head: String },
    /// New work: never a continuation of an open change or existing branch.
    Fresh,
}

impl LaunchGate {
    fn of(mode: &Mode) -> Self {
        match mode {
            Mode::Rework { pr, head, .. } => LaunchGate::Rework {
                pr: *pr,
                head: head.clone(),
            },
            Mode::Fresh => LaunchGate::Fresh,
        }
    }

    pub(crate) fn encode(&self) -> String {
        match self {
            LaunchGate::Rework { pr, head } => format!("rework:{pr}:{head}"),
            LaunchGate::Fresh => "fresh".into(),
        }
    }

    /// A value that is set but unreadable refuses the launch.
    pub(crate) fn parse(raw: &str) -> Result<Self, GateRefusal> {
        let parsed = match raw.trim().split(':').collect::<Vec<_>>()[..] {
            ["fresh"] => Some(LaunchGate::Fresh),
            ["rework", pr, head] => pr
                .parse()
                .ok()
                .filter(|_| {
                    (7..=64).contains(&head.len()) && head.bytes().all(|b| b.is_ascii_hexdigit())
                })
                .map(|pr| LaunchGate::Rework {
                    pr,
                    head: head.into(),
                }),
            _ => None,
        };
        parsed.ok_or_else(|| GateRefusal {
            code: "relay-gate-invalid",
            detail: format!("{GATE_ENV} is set to `{raw}`, which names no relayed launch"),
        })
    }

    /// The gate this process was launched under, if any.
    pub(crate) fn from_env() -> Result<Option<Self>, GateRefusal> {
        aida_core::exec_context::Environment::capture()
            .get(GATE_ENV)
            .map(|raw| Self::parse(&raw.to_string_lossy()))
            .transpose()
    }

    /// The routing half, applied once the launcher knows which open change,
    /// if any, the pickup would continue.
    pub(crate) fn route(&self, open_change: Option<(u64, &str)>) -> Result<(), GateRefusal> {
        match (self, open_change) {
            (LaunchGate::Fresh, Some((pr, branch))) => Err(GateRefusal {
                code: "fresh-blocked",
                detail: format!(
                    "the brief is for fresh work, but the spec has open change #{pr} on `{branch}`; relay it with `--pr {pr}` to rework that change, or close the change first"
                ),
            }),
            (LaunchGate::Rework { pr, .. }, Some((open, branch))) if open != *pr => {
                Err(GateRefusal {
                    code: "rework-mismatch",
                    detail: format!(
                        "the brief reworks PR #{pr}, but the pickup would continue #{open} on `{branch}`"
                    ),
                })
            }
            _ => Ok(()),
        }
    }

    /// The head half, applied after the host admitted the launch.
    pub(crate) fn admit(&self, source: &mut dyn HeadSource) -> Result<(), GateRefusal> {
        let LaunchGate::Rework { pr, head } = self else {
            return Ok(());
        };
        match head_gate(source, *pr, head) {
            None => Ok(()),
            Some(stop) => Err(GateRefusal {
                code: stop.code(),
                detail: stop.explain(),
            }),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// What the forge answers a launcher's own head check in a test.
    pub(crate) static TEST_LAUNCH_PR: std::cell::RefCell<Option<Result<PrView, String>>> =
        const { std::cell::RefCell::new(None) };
}

fn forge_pr(root: &Path, pr: u64) -> Result<PrView> {
    use crate::forge::ChangeState;
    #[cfg(test)]
    if let Some(reply) = TEST_LAUNCH_PR.with(|p| p.borrow().clone()) {
        return reply.map_err(|why| anyhow::anyhow!(why));
    }
    let change = crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(30), || {
        crate::forge::forge_for(root).change_metadata(pr, &mut crate::network_retry::NoopSink)
    })?;
    Ok(PrView {
        state: match change.state {
            ChangeState::Open => "OPEN",
            ChangeState::Merged => "MERGED",
            ChangeState::Closed => "CLOSED",
        }
        .into(),
        head: change.head_sha,
        branch: change.head_ref,
        title: change.title,
    })
}

struct ForgeHead<'a>(&'a Path);

impl HeadSource for ForgeHead<'_> {
    fn pr(&mut self, pr: u64) -> Result<PrView> {
        forge_pr(self.0, pr)
    }
    fn pause(&mut self, duration: Duration) {
        #[cfg(test)]
        if TEST_LAUNCH_PR.with(|p| p.borrow().is_some()) {
            return;
        }
        std::thread::sleep(duration);
    }
}

/// In a launcher, right after the host admitted the launch (which may have
/// waited for a start slot): a relayed rework goes on only if its change is
/// still open at the relayed head. Launches the relay did not start pass.
pub(crate) fn admit_launch(root: &Path) -> Result<()> {
    if let Some(gate) = LaunchGate::from_env()? {
        gate.admit(&mut ForgeHead(root))?;
    }
    Ok(())
}

/// In a launcher, once the pickup's open change is known: whether a relayed
/// launch is fresh work, which must also not resume the spec's own branch.
pub(crate) fn route_launch(open_change: Option<(u64, &str)>) -> Result<bool> {
    let gate = LaunchGate::from_env()?;
    if let Some(gate) = &gate {
        gate.route(open_change)?;
    }
    Ok(gate == Some(LaunchGate::Fresh))
}

/// The relay's side effects, so tests drive the real flow against a fake
/// forge, store and clock and a lane host rooted in a temp directory.
pub(crate) trait World {
    fn pr(&mut self, pr: u64) -> Result<PrView>;
    /// `(comment id, content)` for each top-level comment on the spec.
    fn comments(&mut self, spec: &str) -> Result<Vec<(String, String)>>;
    /// Every id that names `spec` in this machine's store.
    fn spec_ids(&mut self, spec: &str) -> Result<Vec<String>>;
    /// Add a top-level comment and return the id the store gave it.
    fn comment_add(&mut self, spec: &str, body: &str) -> Result<String>;
    fn comment_edit(&mut self, spec: &str, id: &str, body: &str) -> Result<()>;
    fn store_push(&mut self) -> Result<()>;
    fn notify_advisor(&mut self, subject: &str, body: &str) -> Result<()>;
    /// Run one lane-host operation and return its JSON.
    fn host(&mut self, op: &RelayHostCommand) -> Result<serde_json::Value>;
    fn sleep(&mut self, duration: Duration);
    /// Wall time since the run began, host operations included.
    fn elapsed(&mut self) -> Duration;
    fn log(&mut self, line: &str);
}

struct WorldHead<'a>(&'a mut dyn World);

impl HeadSource for WorldHead<'_> {
    fn pr(&mut self, pr: u64) -> Result<PrView> {
        self.0.pr(pr)
    }
    fn pause(&mut self, duration: Duration) {
        self.0.sleep(duration)
    }
}

/// A bounded wait measured in wall time.
struct Deadline {
    start: Duration,
    limit: Duration,
}

impl Deadline {
    fn new(world: &mut dyn World, secs: u64) -> Self {
        Self {
            start: world.elapsed(),
            limit: Duration::from_secs(secs),
        }
    }

    fn spent(&self, world: &mut dyn World) -> Duration {
        world.elapsed().saturating_sub(self.start)
    }

    /// Sleep one poll, or less when less is left. `false` once time is up.
    fn nap(&self, world: &mut dyn World, poll: u64) -> bool {
        let left = self.limit.saturating_sub(self.spent(world));
        if left.is_zero() {
            return false;
        }
        world.sleep(left.min(Duration::from_secs(poll)));
        true
    }
}

fn read_brief(path: &Path, footer: Option<&str>) -> Result<String> {
    let mut body = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read brief {}", path.display()))?;
    if let Some(footer) = footer.map(str::trim).filter(|f| !f.is_empty()) {
        if !body.contains(footer) {
            body = format!("{}\n\n{footer}\n", body.trim_end());
        }
    }
    Ok(body)
}

fn file_stamp(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%dT%H%M%SZ").to_string()
}

/// The brief as the lane's pickup reads it: frontmatter plus the indented
/// body. The pickup's size limit applies to this text, so it is checked
/// here, on both sides of the relay.
fn frame_brief(
    spec: &str,
    vendor: &str,
    label: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<String> {
    anyhow::ensure!(
        !body.trim().is_empty() && !body.contains('\0'),
        "the brief must be nonempty text"
    );
    let framed = format!(
        "---\nspec_id: {}\nagent: {vendor}\ngenerated_at: {}\ngenerated_by: {}\nnote: |-\n{}---\n",
        crate::brief::yaml_scalar(spec),
        file_stamp(now),
        crate::brief::yaml_scalar(&format!("aida lane relay ({label})")),
        crate::brief::indent_block(body.trim_end(), "  "),
    );
    let limit = crate::brief::PICKUP_BRIEF_LIMIT;
    anyhow::ensure!(
        framed.len() <= limit,
        "the brief is {} bytes once framed for the lane's pickup; the limit is {limit}",
        framed.len()
    );
    Ok(framed)
}

fn label_for(args: &RelayArgs, rework: bool) -> Result<String> {
    let label = match &args.label {
        Some(label) => label.trim().to_ascii_uppercase(),
        None if rework => "REWORK".into(),
        None => "FRESH".into(),
    };
    anyhow::ensure!(
        !label.is_empty()
            && !label.starts_with('-')
            && label.len() <= 40
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')),
        "invalid --label `{label}`: use letters, digits, spaces, `-` and `_`, starting with a letter, digit or `_`"
    );
    Ok(label)
}

/// Whether a spec comment is a relayed round brief still in force.
pub(crate) fn is_authoritative_brief(content: &str) -> bool {
    crate::rework_direction::is_relayed_brief_lead(&crate::rework_direction::lead_line(content))
}

/// What one relay run has put where, so a stop can take it back.
struct Round {
    label: String,
    /// This relay's round on the lane host.
    number: u64,
    /// When the brief was posted, as its heading says.
    stamp: String,
    /// The brief comment this run posted: the id the store returned for it,
    /// and its text.
    posted: Option<(String, String)>,
    /// Whether delivery to the lane host was attempted.
    delivering: bool,
    /// The brief's path on the lane host, once delivered.
    placed: Option<String>,
}

/// Post the round brief and its pointer comment. Earlier round briefs stay
/// in force until this round's lane actually starts ([`supersede_earlier`]).
fn post_brief(
    world: &mut dyn World,
    spec: &str,
    plan: &Plan,
    mode: &Mode,
    body: &str,
    round: &mut Round,
) -> Result<()> {
    let label = round.label.clone();
    round.stamp = Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
    let stamp = &round.stamp;
    let target = match mode {
        Mode::Rework { pr, head, .. } => format!("; PR #{pr} at head {}", short(head)),
        Mode::Fresh => String::new(),
    };
    let heading = format!(
        "AUTHORITATIVE {label} BRIEF (relayed {stamp}; {} lane on {}; relay {}#{}{target}):",
        plan.vendor,
        plan.host_name(),
        plan.host_name(),
        round.number
    );
    let brief_comment = format!("{heading}\n{}", body.trim_end());
    // The store's own id for the comment is how this run finds it again.
    let id = world.comment_add(spec, &brief_comment)?;
    round.posted = Some((id, brief_comment));
    let branch = match mode {
        Mode::Rework { branch, .. } => format!(", branch {branch}"),
        Mode::Fresh => ", fresh work on a new branch".into(),
    };
    world
        .comment_add(
            spec,
            &format!(
                "Lane relay {stamp}: {label} lane queued on {} ({}{branch}). Scope = the AUTHORITATIVE {label} BRIEF in the previous comment.",
                plan.host_name(),
                plan.vendor,
            ),
        )
        .map(drop)
}

/// Once this round's lane has started: mark every round brief posted before
/// this run's as superseded. A brief posted after it is newer direction and
/// is left alone. Returns how many were marked.
fn supersede_earlier(world: &mut dyn World, spec: &str, round: &Round) -> Result<usize> {
    let (own, _) = round.posted.as_ref().context("this run posted no brief")?;
    let all = world.comments(spec)?;
    let at = all
        .iter()
        .position(|(id, _)| id == own)
        .with_context(|| format!("this run's brief comment {own} is no longer on {spec}"))?;
    let mut marked = 0;
    for (id, content) in &all[..at] {
        if !is_authoritative_brief(content) {
            continue;
        }
        world.comment_edit(
            spec,
            id,
            &format!(
                "SUPERSEDED by the {} brief relayed {}. Do not act on this brief.\n\n{content}",
                round.label, round.stamp
            ),
        )?;
        marked += 1;
    }
    Ok(marked)
}

/// Record a stop where a person will see it: the relay log, the spec and the
/// advisor's mailbox. A brief the stop voids is marked on the spec and
/// withdrawn from the lane host, so no later lane treats it as the round in
/// force. None of this can fail the relay.
fn report(world: &mut dyn World, spec: &str, vendor: &str, round: &Round, stop: &Stop) {
    let line = stop.line(spec, &round.label);
    world.log(&line);
    if !stop.brief_stays() {
        if let Some((id, posted)) = &round.posted {
            // The comment as it stands now; when the spec cannot be re-read,
            // the text this run posted under that id.
            let content = world
                .comments(spec)
                .ok()
                .and_then(|all| all.into_iter().find(|(cid, _)| cid == id))
                .map_or_else(|| posted.clone(), |(_, content)| content);
            // A later relay's launch may already have marked it.
            if is_authoritative_brief(&content) {
                if let Err(err) = world.comment_edit(
                    spec,
                    id,
                    &format!(
                        "SUPERSEDED: not launched ({}). Do not act on this brief.\n\n{content}",
                        stop.code()
                    ),
                ) {
                    world.log(&format!(
                        "warning: could not mark the unlaunched brief comment {id} superseded: {err:#}"
                    ));
                }
            }
        }
        // A delivery whose reply was lost may still have stored the brief.
        if round.delivering && *stop != Stop::Superseded {
            let retract = RelayHostCommand::Retract {
                spec: spec.into(),
                round: round.number,
                vendor: vendor.into(),
                brief: round.placed.as_ref().map(PathBuf::from),
            };
            if let Err(err) = world.host(&retract) {
                world.log(&format!(
                    "warning: could not withdraw the brief on the lane host: {err:#}"
                ));
            }
        }
    }
    if let Err(err) = world.comment_add(spec, &line) {
        world.log(&format!(
            "warning: could not record the stop on {spec}: {err:#}"
        ));
    }
    if let Err(err) = world.store_push() {
        world.log(&format!("warning: store push failed: {err:#}"));
    }
    if let Err(err) = world.notify_advisor(
        &format!("Lane relay {}: {spec} ({})", stop.verdict(), stop.code()),
        &line,
    ) {
        world.log(&format!("warning: could not notify the advisor: {err:#}"));
    }
}

fn malformed(what: &str, reply: &serde_json::Value) -> Stop {
    Stop::Failed {
        code: "host-reply-malformed",
        why: format!("the lane host's {what} reply was not understood: {reply}"),
    }
}

/// A stop the lane host itself declared in a reply.
fn host_stop(what: &str, reply: &serde_json::Value) -> Option<Stop> {
    match reply.get("stop").map(|stop| stop.as_str()) {
        None => None,
        Some(Some("relay-superseded")) => Some(Stop::Superseded),
        Some(Some("lane-still-running")) => Some(Stop::LaneStillRunning { secs: 0 }),
        Some(_) => Some(malformed(what, reply)),
    }
}

/// Run a host step whose failure ends the relay with `code`.
fn host_step(
    world: &mut dyn World,
    op: &RelayHostCommand,
    what: &str,
    code: &'static str,
) -> Result<serde_json::Value, Stop> {
    let reply = world.host(op).map_err(|err| Stop::Failed {
        code,
        why: format!("the lane host's {what} step failed: {err:#}"),
    })?;
    match host_stop(what, &reply) {
        Some(stop) => Err(stop),
        None => Ok(reply),
    }
}

fn text(reply: &serde_json::Value, what: &str, key: &str) -> Result<String, Stop> {
    reply[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| malformed(what, reply))
}

/// Wait, bounded, until no lane for the spec runs on the host and the host
/// is above its memory floor. A relay that is no longer the spec's newest
/// stops at once.
fn wait_for_host(world: &mut dyn World, args: &RelayArgs, spec: &str, round: u64) -> Option<Stop> {
    let clock = Deadline::new(world, args.wait_timeout);
    let probe = RelayHostCommand::Probe {
        spec: spec.into(),
        round,
    };
    let mut last = "";
    loop {
        let reply = world.host(&probe);
        let secs = clock.spent(world).as_secs();
        let blocked = match reply {
            Ok(reply) => {
                if let Some(stop) = host_stop("probe", &reply) {
                    return Some(stop);
                }
                // A reply without a field is not evidence about the host.
                match (reply["current"].as_bool(), reply["running"].as_bool()) {
                    (Some(false), _) => return Some(Stop::Superseded),
                    (Some(true), Some(false)) => match reply["shortfall"].as_str() {
                        Some(why) => Stop::HostCapacity {
                            secs,
                            why: why.into(),
                        },
                        None => return None,
                    },
                    (Some(true), Some(true)) => Stop::LaneStillRunning { secs },
                    _ => Stop::HostUnreachable {
                        secs,
                        why: format!("unreadable probe reply: {reply}"),
                    },
                }
            }
            Err(err) => Stop::HostUnreachable {
                secs,
                why: format!("{err:#}"),
            },
        };
        if blocked.code() != last {
            last = blocked.code();
            world.log(&format!(
                "waiting ({last}) before launching {spec}; checking every {}s for up to {}s",
                args.poll, args.wait_timeout
            ));
        }
        if !clock.nap(world, args.poll) {
            return Some(blocked);
        }
    }
}

/// Watch, bounded, for the started lane's admission result.
fn watch_admission(
    world: &mut dyn World,
    args: &RelayArgs,
    spec: &str,
    since: &str,
    log: &str,
) -> Option<Stop> {
    let op = RelayHostCommand::Admission {
        spec: spec.into(),
        launched_at: since.into(),
        log: log.into(),
    };
    let clock = Deadline::new(world, args.admission_timeout);
    let (mut vanished, mut announced) = (0, false);
    loop {
        match world.host(&op) {
            Ok(reply) => match reply["state"].as_str() {
                Some("started" | "finished") => return None,
                Some("refused") => {
                    let reason = reply["reason"].as_str().unwrap_or("unknown");
                    let detail = reply["detail"].as_str().unwrap_or("").to_string();
                    return Some(match GATE_CODES.iter().find(|code| **code == reason) {
                        Some(code) => Stop::GateRefused {
                            code: *code,
                            detail,
                        },
                        None => Stop::AdmissionRefused {
                            reason: reason.into(),
                            detail,
                        },
                    });
                }
                Some("vanished") => {
                    vanished += 1;
                    if vanished >= VANISHED_POLLS {
                        return Some(Stop::LaunchVanished {
                            log_tail: reply["log_tail"].as_str().unwrap_or("").into(),
                        });
                    }
                }
                Some("pending") => vanished = 0,
                _ => world.log(&format!("warning: unreadable admission reply: {reply}")),
            },
            Err(err) => world.log(&format!("warning: admission check failed: {err:#}")),
        }
        if !announced {
            announced = true;
            world.log(&format!(
                "launched {spec}; watching its admission for up to {}s",
                args.admission_timeout
            ));
        }
        if !clock.nap(world, args.poll) {
            return Some(Stop::AdmissionUnconfirmed {
                secs: clock.spent(world).as_secs(),
            });
        }
    }
}

/// Everything after the round is taken: any error is a typed stop.
fn deliver(
    args: &RelayArgs,
    plan: &Plan,
    world: &mut dyn World,
    mode: &Mode,
    body: &str,
    round: &mut Round,
) -> Result<Outcome, Stop> {
    let spec = args.spec.as_str();
    post_brief(world, spec, plan, mode, body, round).map_err(|err| Stop::Failed {
        code: "brief-record-failed",
        why: format!("the brief could not be recorded on {spec}: {err:#}"),
    })?;
    if let Err(err) = world.store_push() {
        world.log(&format!("warning: store push failed: {err:#}"));
    }
    world.log(&format!(
        "queued {spec} ({}, {} on {}, relay round {})",
        round.label,
        plan.vendor,
        plan.host_name(),
        round.number
    ));

    if let Some(stop) = wait_for_host(world, args, spec, round.number) {
        return Err(stop);
    }
    if let Mode::Rework { pr, head, .. } = mode {
        if let Some(stop) = head_gate(&mut WorldHead(world), *pr, head) {
            return Err(stop);
        }
    }

    round.delivering = true;
    let placed = host_step(
        world,
        &RelayHostCommand::PlaceBrief {
            spec: spec.into(),
            round: round.number,
            vendor: plan.vendor.clone(),
            label: round.label.clone(),
            body_b64: base64::engine::general_purpose::STANDARD.encode(body),
        },
        "brief delivery",
        "brief-delivery-failed",
    )?;
    let brief = text(&placed, "brief delivery", "path")?;
    round.placed = Some(brief.clone());
    let (branch, pr, head) = match mode {
        Mode::Rework { pr, branch, head } => (Some(branch.clone()), Some(*pr), Some(head.clone())),
        Mode::Fresh => (None, None, None),
    };
    let launched = host_step(
        world,
        &RelayHostCommand::Launch {
            spec: spec.into(),
            round: round.number,
            vendor: plan.vendor.clone(),
            branch,
            pr,
            head,
            model: plan.model.clone(),
            brief: PathBuf::from(&brief),
        },
        "launch",
        "launch-failed",
    )?;
    let since = text(&launched, "launch", "since")?;
    let log = text(&launched, "launch", "log")?;
    if let Some(stop) = watch_admission(world, args, spec, &since, &log) {
        return Err(stop);
    }
    world.log(&format!(
        "admitted: {spec} ({}) is running on {}; brief {brief}; log {log}",
        round.label,
        plan.host_name()
    ));
    // Only now is this round the one in force.
    match supersede_earlier(world, spec, round) {
        Ok(0) => {}
        Ok(marked) => {
            let note = format!(
                "Lane relay: {} lane admitted on {}; {marked} earlier round brief(s) marked SUPERSEDED.",
                round.label,
                plan.host_name()
            );
            world.log(&note);
            let recorded = world.comment_add(spec, &note).map(drop);
            if let Err(err) = recorded.and_then(|()| world.store_push()) {
                world.log(&format!(
                    "warning: could not record the superseded briefs on {spec}: {err:#}"
                ));
            }
        }
        Err(err) => world.log(&format!(
            "warning: the lane is running, but earlier round briefs on {spec} could not be marked superseded: {err:#}"
        )),
    }
    Ok(Outcome::Admitted {
        mode: mode.clone(),
        brief,
        log,
    })
}

/// Whether the change's title or branch names `spec`, by any id this
/// machine's store knows it by. A change that names no spec at all does not.
fn pr_names_spec(world: &mut dyn World, spec: &str, view: &PrView) -> Result<bool> {
    let mut ids = world
        .spec_ids(spec)
        .with_context(|| format!("cannot resolve {spec} to check --pr against it"))?;
    if !ids.iter().any(|id| id == spec) {
        ids.push(spec.to_string());
    }
    Ok(
        crate::pr_ship::extract_pr_spec_ids(&view.branch, &view.title)
            .iter()
            .any(|named| names_spec(&ids, named)),
    )
}

/// One relay run, start to recorded outcome. An `Err` is a usage error
/// found before anything was posted or delivered.
pub(crate) fn relay(args: &RelayArgs, plan: &Plan, world: &mut dyn World) -> Result<Outcome> {
    let spec = args.spec.as_str();
    let body = read_brief(&args.brief, plan.footer.as_deref())?;
    // Both default labels and any explicit one are checked before the forge
    // or the host is asked anything.
    let mut round = Round {
        label: label_for(args, args.pr.is_some())?,
        number: 0,
        stamp: String::new(),
        posted: None,
        delivering: false,
        placed: None,
    };
    frame_brief(spec, &plan.vendor, &round.label, &body, Utc::now())?;

    let mode = match args.pr {
        None => Ok(Mode::Fresh),
        Some(pr) => match world.pr(pr) {
            Err(err) => Err(Stop::PrUnreadable {
                pr,
                why: format!("{err:#}"),
            }),
            // A change for another spec is a mistyped `--pr`, whatever its
            // state: refused before anything is posted.
            Ok(view) if !pr_names_spec(world, spec, &view)? => anyhow::bail!(
                "PR #{pr} (`{}`, branch `{}`) names {}, not {spec}; check --pr",
                view.title,
                view.branch,
                match crate::pr_ship::extract_pr_spec_ids(&view.branch, &view.title) {
                    ids if ids.is_empty() => "no spec".to_string(),
                    ids => ids.join(", "),
                }
            ),
            Ok(view) if view.state.eq_ignore_ascii_case("OPEN") => {
                match plain_value("PR branch", &view.branch) {
                    Ok(()) => Ok(Mode::Rework {
                        pr,
                        branch: view.branch,
                        head: view.head,
                    }),
                    Err(err) => Err(Stop::PrUnreadable {
                        pr,
                        why: format!("{err:#}"),
                    }),
                }
            }
            Ok(view) => {
                // A finished change has no round to rework: routing a lane
                // onto its branch ends in a no-change exit.
                world.log(&format!(
                    "PR #{pr} is {}: launching fresh work for {spec}, not a rework of that PR",
                    view.state
                ));
                Ok(Mode::Fresh)
            }
        },
    };
    let stopped = mode.and_then(|mode| {
        if mode == Mode::Fresh && args.label.is_none() {
            round.label = "FRESH".into();
        }
        // Nothing is posted until the host has given this run its round.
        let begun = host_step(
            world,
            &RelayHostCommand::Begin { spec: spec.into() },
            "round",
            "host-unreachable",
        )
        .map_err(|stop| match stop {
            Stop::Failed { why, .. } => Stop::HostUnreachable { secs: 0, why },
            other => other,
        })?;
        if let Some(why) = begun["store_pull"].as_str() {
            world.log(&format!(
                "warning: the lane host could not refresh its store before resolving {spec}: {why}"
            ));
        }
        round.number = begun["round"]
            .as_u64()
            .filter(|number| *number > 0)
            .ok_or_else(|| malformed("round", &begun))?;
        deliver(args, plan, world, &mode, &body, &mut round)
    });
    match stopped {
        Ok(admitted) => Ok(admitted),
        Err(stop) => {
            report(world, spec, &plan.vendor, &round, &stop);
            Ok(Outcome::Stopped(stop))
        }
    }
}

// ---------------------------------------------------------------------------
// The lane-host side.

/// What the host operations need from the machine.
pub(crate) trait HostSys {
    /// Bring the host's store up to date with its remote, bounded, so the
    /// spec is resolved against current data.
    fn refresh_store(&self, root: &Path) -> Result<()>;
    /// Every id that names `spec` in the host's store (its canonical id, its
    /// display id and the one given), the canonical one first. An error
    /// means the host cannot tell, which no step treats as "no match".
    fn spec_ids(&self, root: &Path, spec: &str) -> Result<Vec<String>>;
    /// Whether a lane launcher for the spec named by `ids` is running.
    fn lane_running(&self, ids: &[String]) -> bool;
    /// Why the host is below its memory floor, if it is.
    fn shortfall(&self) -> Option<String>;
    /// Start `argv` detached in `cwd` with `env` added, its output appended
    /// to `log`.
    fn spawn(
        &mut self,
        argv: &[String],
        env: &[(String, String)],
        cwd: &Path,
        log: &Path,
    ) -> Result<()>;
    fn now(&self) -> DateTime<Utc>;
    /// The home directory holding this machine's relay settings.
    fn home(&self) -> Option<PathBuf>;
}

/// Whether `candidate` names the spec `ids` names. Every place the relay
/// compares a spec id (lane processes, admission receipts) uses this.
pub(crate) fn names_spec(ids: &[String], candidate: &str) -> bool {
    let candidate = aida_core::node_alias::expand_or_keep(candidate.trim());
    ids.iter().any(|id| {
        aida_core::id_identity::same_id(id, &candidate) || id.eq_ignore_ascii_case(&candidate)
    })
}

/// Whether a process's arguments are a lane launcher working the spec:
/// `aida queue work … <spec> …`, `aida queue rework … <spec> … --work`, or a
/// `lane-run.sh … <spec> …` wrapper that has not reached `queue work` yet.
/// The spec may stand anywhere after the subcommand. An argument that merely
/// equals the spec's id (a flag's value) also counts: a wrong "running" only
/// makes a relay wait, a wrong "idle" starts a second lane.
pub(crate) fn cmdline_runs_lane(argv: &[String], ids: &[String]) -> bool {
    let names = |rest: &[String]| rest.iter().any(|arg| names_spec(ids, arg));
    (0..argv.len()).any(|at| {
        let rest = &argv[at + 1..];
        if argv[at] == "queue" {
            return match rest.first().map(String::as_str) {
                Some("work") => names(&rest[1..]),
                Some("rework") => names(&rest[1..]) && rest[1..].iter().any(|arg| arg == "--work"),
                _ => false,
            };
        }
        Path::new(&argv[at])
            .file_name()
            .is_some_and(|name| name == "lane-run.sh")
            && names(rest)
    })
}

fn spec_file_stem(spec: &str) -> Result<&str> {
    anyhow::ensure!(
        spec.starts_with(|c: char| c.is_ascii_alphanumeric())
            && spec.len() <= 120
            && spec
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
        "invalid spec id `{spec}`"
    );
    Ok(spec)
}

/// The ids naming `spec` on this host, canonical first, the given one
/// always among them.
fn ids_for(root: &Path, spec: &str, sys: &dyn HostSys) -> Result<Vec<String>> {
    spec_file_stem(spec)?;
    let mut ids = sys.spec_ids(root, spec)?;
    ids.retain(|id| spec_file_stem(id).is_ok());
    if !ids.iter().any(|id| id == spec) {
        ids.push(spec.to_string());
    }
    Ok(ids)
}

/// The spec's relay round on this host, held under an exclusive lock: the
/// number the newest relay took. A relay whose number is lower has been
/// superseded and may place or start nothing.
struct Fence {
    file: std::fs::File,
    path: PathBuf,
}

impl Fence {
    fn hold(root: &Path, ids: &[String]) -> Result<Self> {
        let dir = root.join(".aida/lane-relay");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        // One file per spec whichever id the relay was given.
        let path = dir.join(format!("{}.round", ids[0].to_ascii_uppercase()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        fs2::FileExt::lock_exclusive(&file)
            .with_context(|| format!("cannot lock {}", path.display()))?;
        Ok(Self { file, path })
    }

    fn current(&mut self) -> Result<u64> {
        use std::io::{Read, Seek};
        let mut text = String::new();
        self.file.rewind()?;
        self.file.read_to_string(&mut text)?;
        match text.trim() {
            "" => Ok(0),
            number => number
                .parse()
                .with_context(|| format!("{} holds no round number", self.path.display())),
        }
    }

    fn advance(&mut self) -> Result<u64> {
        use std::io::{Seek, Write};
        let next = self.current()? + 1;
        self.file.rewind()?;
        self.file.set_len(0)?;
        writeln!(self.file, "{next}")?;
        self.file.sync_all()?;
        Ok(next)
    }

    /// The reply that stops a relay which is no longer the newest.
    fn superseded(&mut self, round: u64) -> Result<Option<serde_json::Value>> {
        let current = self.current()?;
        Ok((current != round).then(|| {
            serde_json::json!({
                "stop": "relay-superseded",
                "current_round": current,
            })
        }))
    }
}

fn mailbox(root: &Path, vendor: &str) -> Result<PathBuf> {
    Ok(root.join(".aida/agent-briefs").join(vendor_name(vendor)?))
}

/// Only a brief this host stored for this vendor is handed to a lane or
/// withdrawn.
fn mailbox_brief(root: &Path, vendor: &str, brief: &Path) -> Result<PathBuf> {
    let dir = mailbox(root, vendor)?.canonicalize()?;
    let brief = brief
        .canonicalize()
        .with_context(|| format!("no brief at {}", brief.display()))?;
    anyhow::ensure!(
        brief.parent() == Some(dir.as_path()),
        "brief {} is not in {}",
        brief.display(),
        dir.display()
    );
    Ok(brief)
}

/// Store `body` as the spec's one pending brief for `vendor`.
fn place_brief(
    root: &Path,
    ids: &[String],
    spec: &str,
    vendor: &str,
    label: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<serde_json::Value> {
    let stem = spec_file_stem(spec)?;
    let framed = frame_brief(spec, vendor, label, body, now)?;
    let dir = mailbox(root, vendor)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let superseded = crate::brief::supersede_pending_briefs(root, ids)?;
    let path = dir.join(format!("{stem}-{}.md", file_stamp(now)));
    let staged = dir.join(format!(
        ".{stem}-{}.relay-{}",
        file_stamp(now),
        std::process::id()
    ));
    std::fs::write(&staged, framed)
        .with_context(|| format!("cannot write {}", staged.display()))?;
    if let Err(err) = aida_core::mailbox_lifecycle::rename_noreplace(&staged, &path) {
        let _ = std::fs::remove_file(&staged);
        return Err(err).with_context(|| format!("cannot publish brief {}", path.display()));
    }
    Ok(serde_json::json!({
        "path": path,
        "superseded": superseded,
    }))
}

fn launcher(home: Option<&Path>) -> Result<Vec<String>> {
    let launcher = load_machine_config(home)?
        .launcher
        .unwrap_or_else(|| DEFAULT_LAUNCHER.iter().map(|s| s.to_string()).collect());
    anyhow::ensure!(
        launcher.first().is_some_and(|program| !program.is_empty()),
        "`launcher` in ~/.aida/lane-relay.toml must name a program"
    );
    Ok(launcher)
}

#[allow(clippy::too_many_arguments)]
fn launch(
    root: &Path,
    spec: &str,
    vendor: &str,
    branch: Option<&str>,
    gate: &LaunchGate,
    model: Option<&str>,
    brief: &Path,
    sys: &mut dyn HostSys,
) -> Result<serde_json::Value> {
    let stem = spec_file_stem(spec)?;
    let brief = mailbox_brief(root, vendor, brief)?;
    let mut argv: Vec<String> = launcher(sys.home().as_deref())?
        .iter()
        .map(|arg| arg.replace("{spec}", spec).replace("{vendor}", vendor))
        .collect();
    match (gate, branch) {
        (LaunchGate::Rework { .. }, Some(branch)) => {
            plain_value("branch", branch)?;
            argv.extend(["--branch".into(), branch.into()]);
        }
        (LaunchGate::Fresh, None) => {}
        (LaunchGate::Rework { .. }, None) => anyhow::bail!("a rework launch names its branch"),
        (LaunchGate::Fresh, Some(_)) => anyhow::bail!("a fresh launch names no branch"),
    }
    argv.push("--force-claim".into());
    if let Some(model) = model {
        plain_value("model", model)?;
        argv.extend(["--model".into(), model.into()]);
    }
    argv.extend(["--brief".into(), brief.to_string_lossy().into_owned()]);
    let since = sys.now();
    let logs = root.join(".aida/lane-relay");
    std::fs::create_dir_all(&logs).with_context(|| format!("cannot create {}", logs.display()))?;
    let log = logs.join(format!("{stem}-{}.log", file_stamp(since)));
    let gate = gate.encode();
    sys.spawn(&argv, &[(GATE_ENV.to_string(), gate.clone())], root, &log)?;
    Ok(serde_json::json!({
        "since": since.to_rfc3339(),
        "log": log,
        "argv": argv,
        "gate": gate,
    }))
}

fn log_tail(root: &Path, log: &Path) -> String {
    let inside = log
        .canonicalize()
        .ok()
        .zip(root.join(".aida/lane-relay").canonicalize().ok())
        .filter(|(log, dir)| log.parent() == Some(dir.as_path()));
    let Some((log, _)) = inside else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&std::fs::read(log).unwrap_or_default()).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(12)..].join("\n");
    let start = tail
        .char_indices()
        .rev()
        .nth(1999)
        .map_or(0, |(index, _)| index);
    tail[start..].to_string()
}

fn admission(
    root: &Path,
    ids: &[String],
    since: &str,
    log: &Path,
    sys: &dyn HostSys,
) -> Result<serde_json::Value> {
    use crate::lane_result::Kind;
    let since = DateTime::parse_from_rfc3339(since)
        .with_context(|| format!("invalid launch time `{since}`"))?
        .with_timezone(&Utc);
    let record =
        crate::lane_result::admission_since(root, since, &|recorded| names_spec(ids, recorded));
    let reply = match record {
        Some(r) if r.result == Some(Kind::RefusedBeforeStart) => serde_json::json!({
            "state": "refused",
            "reason": r.reason_code.unwrap_or_else(|| "unknown".into()),
            "detail": r.detail.unwrap_or_default(),
        }),
        Some(r) if r.result.is_some() => serde_json::json!({
            "state": "finished",
            "result": r.result,
            "reason": r.reason_code,
        }),
        Some(r) if r.started => serde_json::json!({ "state": "started" }),
        _ if sys.lane_running(ids) => serde_json::json!({ "state": "pending" }),
        _ => serde_json::json!({ "state": "vanished", "log_tail": log_tail(root, log) }),
    };
    Ok(reply)
}

/// Run one lane-host operation against the checkout at `root`.
pub(crate) fn host_op(
    root: &Path,
    op: &RelayHostCommand,
    sys: &mut dyn HostSys,
) -> Result<serde_json::Value> {
    match op {
        RelayHostCommand::Begin { spec } => {
            spec_file_stem(spec)?;
            // A spec filed moments ago on the relaying machine is only in
            // this host's store after a pull. A failed pull is reported and
            // the spec is resolved against what the host has.
            let pull = sys.refresh_store(root).err().map(|err| format!("{err:#}"));
            let ids = ids_for(root, spec, sys)?;
            let round = Fence::hold(root, &ids)?.advance()?;
            let mut reply = serde_json::json!({ "round": round });
            if let Some(why) = pull {
                reply["store_pull"] = why.into();
            }
            Ok(reply)
        }
        RelayHostCommand::Probe { spec, round } => {
            let ids = ids_for(root, spec, sys)?;
            let current = Fence::hold(root, &ids)?.current()?;
            Ok(serde_json::json!({
                "current": current == *round,
                "running": sys.lane_running(&ids),
                "shortfall": sys.shortfall(),
            }))
        }
        RelayHostCommand::PlaceBrief {
            spec,
            round,
            vendor,
            label,
            body_b64,
        } => {
            let body = base64::engine::general_purpose::STANDARD
                .decode(body_b64)
                .context("the brief body is not base64")?;
            let body = String::from_utf8(body).context("the brief body is not UTF-8")?;
            let ids = ids_for(root, spec, sys)?;
            let mut fence = Fence::hold(root, &ids)?;
            if let Some(stop) = fence.superseded(*round)? {
                return Ok(stop);
            }
            place_brief(root, &ids, spec, vendor, label, &body, sys.now())
        }
        RelayHostCommand::Launch {
            spec,
            round,
            vendor,
            branch,
            pr,
            head,
            model,
            brief,
        } => {
            let gate = match (pr, head) {
                (Some(pr), Some(head)) => LaunchGate::parse(
                    &LaunchGate::Rework {
                        pr: *pr,
                        head: head.clone(),
                    }
                    .encode(),
                )?,
                (None, None) => LaunchGate::Fresh,
                _ => anyhow::bail!("a rework launch names both its PR and the expected head"),
            };
            let ids = ids_for(root, spec, sys)?;
            // Held until the launcher is started, so two relays cannot both
            // pass the checks below.
            let mut fence = Fence::hold(root, &ids)?;
            if let Some(stop) = fence.superseded(*round)? {
                return Ok(stop);
            }
            if sys.lane_running(&ids) {
                return Ok(serde_json::json!({ "stop": "lane-still-running" }));
            }
            launch(
                root,
                spec,
                vendor,
                branch.as_deref(),
                &gate,
                model.as_deref(),
                brief,
                sys,
            )
        }
        RelayHostCommand::Retract {
            spec,
            round,
            vendor,
            brief,
        } => {
            let ids = ids_for(root, spec, sys)?;
            let mut fence = Fence::hold(root, &ids)?;
            // A later round's delivery already retired this one's brief, and
            // what is pending now is that round's.
            if fence.current()? != *round {
                return Ok(serde_json::json!({ "retired": [] }));
            }
            let retired = match brief {
                None => crate::brief::supersede_pending_briefs(root, &ids)?,
                Some(brief) => match mailbox_brief(root, vendor, brief) {
                    Ok(brief) => crate::brief::retire_brief(&brief)?.into_iter().collect(),
                    // Already taken by a lane, or already withdrawn.
                    Err(_) if !brief.exists() => Vec::new(),
                    Err(err) => return Err(err),
                },
            };
            Ok(serde_json::json!({ "retired": retired }))
        }
        RelayHostCommand::Admission {
            spec,
            launched_at,
            log,
        } => {
            let ids = ids_for(root, spec, sys)?;
            admission(root, &ids, launched_at, log, sys)
        }
    }
}

/// The arguments that run `op` as `aida lane relay-host …`.
pub(crate) fn host_argv(op: &RelayHostCommand) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut argv: Vec<String> = vec!["lane".into(), "relay-host".into()];
    match op {
        RelayHostCommand::Begin { spec } => argv.extend(["begin".into(), spec.clone()]),
        RelayHostCommand::Probe { spec, round } => argv.extend([
            "probe".into(),
            spec.clone(),
            "--round".into(),
            round.to_string(),
        ]),
        RelayHostCommand::PlaceBrief {
            spec,
            round,
            vendor,
            label,
            body_b64,
        } => argv.extend(
            [
                "place-brief",
                spec,
                "--round",
                &round.to_string(),
                "--vendor",
                vendor,
                "--label",
                label,
                "--body-b64",
                body_b64,
            ]
            .map(String::from),
        ),
        RelayHostCommand::Launch {
            spec,
            round,
            vendor,
            branch,
            pr,
            head,
            model,
            brief,
        } => {
            argv.extend(
                [
                    "launch",
                    spec,
                    "--round",
                    &round.to_string(),
                    "--vendor",
                    vendor,
                ]
                .map(String::from),
            );
            if let Some(branch) = branch {
                argv.extend(["--branch".into(), branch.clone()]);
            }
            if let Some(pr) = pr {
                argv.extend(["--pr".into(), pr.to_string()]);
            }
            if let Some(head) = head {
                argv.extend(["--head".into(), head.clone()]);
            }
            if let Some(model) = model {
                argv.extend(["--model".into(), model.clone()]);
            }
            argv.extend(["--brief".into(), path(brief)]);
        }
        RelayHostCommand::Retract {
            spec,
            round,
            vendor,
            brief,
        } => {
            argv.extend(
                [
                    "retract",
                    spec,
                    "--round",
                    &round.to_string(),
                    "--vendor",
                    vendor,
                ]
                .map(String::from),
            );
            if let Some(brief) = brief {
                argv.extend(["--brief".into(), path(brief)]);
            }
        }
        RelayHostCommand::Admission {
            spec,
            launched_at,
            log,
        } => argv.extend([
            "admission".into(),
            spec.clone(),
            "--launched-at".into(),
            launched_at.clone(),
            "--log".into(),
            path(log),
        ]),
    }
    argv
}

fn bounded(mut cmd: std::process::Command, secs: u64, what: &str) -> Result<String> {
    cmd.stdin(std::process::Stdio::null());
    match crate::command_output_with_timeout_detail(cmd, Duration::from_secs(secs)) {
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
            "{what} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        crate::BoundedCommandOutput::SpawnFailed => anyhow::bail!("{what} could not be started"),
        crate::BoundedCommandOutput::TimedOut => anyhow::bail!("{what} timed out after {secs}s"),
    }
}

/// `aida show <spec> --format json`, run with `aida` in `root`.
fn show_spec(aida: &Path, root: &Path, spec: &str) -> Result<serde_json::Value> {
    let mut cmd = std::process::Command::new(aida);
    cmd.current_dir(root)
        .args(["show", spec, "--format", "json"]);
    let out = bounded(cmd, 60, "`aida show`")?;
    serde_json::from_str(&out).context("`aida show --format json` returned non-JSON")
}

/// The ids `aida show --format json` reports for a spec, canonical first.
fn shown_ids(shown: &serde_json::Value) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for key in ["spec_id", "agreed_id", "display_id"] {
        if let Some(id) = shown[key].as_str().filter(|id| !id.is_empty()) {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_string());
            }
        }
    }
    anyhow::ensure!(!ids.is_empty(), "the store reported no id");
    Ok(ids)
}

/// The real machine.
struct Machine {
    /// The `aida` that reads the host's store.
    aida: PathBuf,
    /// The home directory holding this machine's relay settings.
    home: Option<PathBuf>,
}

impl Machine {
    fn new(aida: PathBuf) -> Self {
        Self {
            aida,
            home: aida_core::home::home_dir(),
        }
    }
}

impl HostSys for Machine {
    fn refresh_store(&self, root: &Path) -> Result<()> {
        let aida = &self.aida;
        let mut cmd = std::process::Command::new(aida);
        cmd.current_dir(root).args(["db", "sync", "--pull"]);
        bounded(cmd, 45, "`aida db sync --pull`").map(drop)
    }

    fn spec_ids(&self, root: &Path, spec: &str) -> Result<Vec<String>> {
        let shown = show_spec(&self.aida, root, spec)
            .with_context(|| format!("cannot resolve {spec} in this host's store"))?;
        shown_ids(&shown).with_context(|| format!("{spec} has no id in this host's store"))
    }

    fn lane_running(&self, ids: &[String]) -> bool {
        // An unreadable process table is not evidence that no lane runs.
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return true;
        };
        let me = std::process::id().to_string();
        processes.flatten().any(|entry| {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            else {
                return false;
            };
            if pid == me {
                return false;
            }
            let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
                return false;
            };
            let argv: Vec<String> = raw
                .split(|byte| *byte == 0)
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect();
            cmdline_runs_lane(&argv, ids)
        })
    }

    fn shortfall(&self) -> Option<String> {
        match crate::lane_admission::memory_floor_shortfall() {
            Ok(shortfall) => shortfall,
            Err(err) => Some(format!("cannot read the host's lane limits: {err:#}")),
        }
    }

    fn spawn(
        &mut self,
        argv: &[String],
        env: &[(String, String)],
        cwd: &Path,
        log: &Path,
    ) -> Result<()> {
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .with_context(|| format!("cannot open launch log {}", log.display()))?;
        // `setsid --fork` returns at once and leaves the launcher in its own
        // session, so it outlives this command and the SSH connection.
        let status = std::process::Command::new("setsid")
            .arg("--fork")
            .args(argv)
            .current_dir(cwd)
            .env_remove("AIDA_SESSION_PURPOSE")
            .envs(env.iter().map(|(key, value)| (key, value)))
            .stdin(std::process::Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out)
            .status()
            .context("cannot run `setsid` to detach the lane launcher")?;
        anyhow::ensure!(status.success(), "`setsid --fork` exited {status}");
        Ok(())
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn home(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// `aida lane relay-host <op>`: run in the lane host's primary checkout.
pub(crate) fn run_host(op: &RelayHostCommand) -> Result<()> {
    let root = crate::main_worktree_root_from(&std::env::current_dir()?);
    let mut machine = Machine::new(crate::resolve_aida_exe());
    println!("{}", host_op(&root, op, &mut machine)?);
    Ok(())
}

/// The real forge, store, mailbox and lane host.
struct Live {
    root: PathBuf,
    /// The lane host's SSH alias and the project directory there.
    host: Option<(String, String)>,
    /// The `aida` that reads and writes the spec.
    aida: PathBuf,
    /// Added to the SSH client's environment; empty outside tests.
    ssh_env: Vec<(String, String)>,
    started: std::time::Instant,
}

impl Live {
    fn aida(&self, args: &[&str]) -> std::process::Command {
        // The spec-store `aida`, never a vendor program.
        let aida = &self.aida;
        let mut cmd = std::process::Command::new(aida);
        cmd.current_dir(&self.root).args(args);
        cmd
    }

    fn with_body<T>(&self, body: &str, run: impl FnOnce(&str) -> Result<T>) -> Result<T> {
        let mut file = tempfile::NamedTempFile::new()?;
        std::io::Write::write_all(&mut file, body.as_bytes())?;
        run(&file.path().to_string_lossy())
    }
}

impl World for Live {
    fn pr(&mut self, pr: u64) -> Result<PrView> {
        forge_pr(&self.root, pr)
    }

    fn comments(&mut self, spec: &str) -> Result<Vec<(String, String)>> {
        let json = show_spec(&self.aida, &self.root, spec)?;
        Ok(json["comments"]
            .as_array()
            .map(|comments| {
                comments
                    .iter()
                    .filter_map(|c| {
                        Some((
                            c["id"].as_str()?.to_owned(),
                            c["content"].as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn spec_ids(&mut self, spec: &str) -> Result<Vec<String>> {
        shown_ids(&show_spec(&self.aida, &self.root, spec)?)
    }

    fn comment_add(&mut self, spec: &str, body: &str) -> Result<String> {
        let out = self.with_body(body, |file| {
            bounded(
                self.aida(&["comment", "add", spec, "--body-file", file, "--print-id"]),
                60,
                "`aida comment add`",
            )
        })?;
        out.lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix(crate::comment_cmd::PRINT_ID_PREFIX)
            })
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .with_context(|| {
                format!("`aida comment add` posted on {spec} but did not report the comment's id")
            })
    }

    fn comment_edit(&mut self, spec: &str, id: &str, body: &str) -> Result<()> {
        self.with_body(body, |file| {
            bounded(
                self.aida(&[
                    "comment",
                    "edit",
                    "--req-id",
                    spec,
                    "--comment-id",
                    id,
                    "--body-file",
                    file,
                ]),
                60,
                "`aida comment edit`",
            )
        })
        .map(drop)
    }

    fn store_push(&mut self) -> Result<()> {
        let push = || {
            bounded(
                self.aida(&["db", "sync", "--push"]),
                120,
                "`aida db sync --push`",
            )
        };
        push().or_else(|_| push()).map(drop)
    }

    fn notify_advisor(&mut self, subject: &str, body: &str) -> Result<()> {
        crate::ship_from_seat::advisor_request(&self.root, subject, body)
    }

    fn host(&mut self, op: &RelayHostCommand) -> Result<serde_json::Value> {
        let Some((host, dir)) = &self.host else {
            return host_op(&self.root, op, &mut Machine::new(self.aida.clone()));
        };
        let remote = format!(
            r#"export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; cd -- {} && exec aida {}"#,
            aida_core::work_binding::shell_quote(dir),
            host_argv(op)
                .iter()
                .map(|arg| aida_core::work_binding::shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let mut cmd = std::process::Command::new("ssh");
        cmd.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "--",
            host,
            &remote,
        ]);
        cmd.envs(self.ssh_env.iter().map(|(key, value)| (key, value)));
        // Longer than the host's own bounds: a store pull, then `aida show`.
        let out = bounded(cmd, 150, &format!("the lane host command on {host}"))?;
        let line = out
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let reply: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("{host} returned no JSON result: {}", out.trim()))?;
        anyhow::ensure!(
            reply.is_object(),
            "{host} returned a result that is not a JSON object: {line}"
        );
        Ok(reply)
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn elapsed(&mut self) -> Duration {
        self.started.elapsed()
    }

    fn log(&mut self, line: &str) {
        use std::io::Write;
        println!("{line}");
        let _ = std::io::stdout().flush();
        let dir = self.root.join(".aida/lane-relay");
        let logged = std::fs::create_dir_all(&dir).and_then(|()| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("relay.log"))?;
            writeln!(file, "{} {line}", Utc::now().format("%Y-%m-%dT%H:%M:%SZ"))
        });
        if let Err(err) = logged {
            eprintln!("warning: could not write the relay log: {err}");
        }
    }
}

/// Where the project lives on the lane host: the flag or config value, else
/// this checkout's path relative to the local home directory.
fn remote_dir(plan: &Plan, root: &Path) -> Result<String> {
    if let Some(dir) = &plan.host_dir {
        return Ok(dir.clone());
    }
    aida_core::home::home_dir()
        .and_then(|home| root.strip_prefix(home).ok().map(Path::to_path_buf))
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.to_string_lossy().into_owned())
        .context(
            "cannot tell where the project is on the lane host: pass --host-dir or set `host_dir` in ~/.aida/lane-relay.toml",
        )
}

/// `aida lane relay`.
pub(crate) fn run(args: &RelayArgs) -> Result<()> {
    let root = crate::main_worktree_root_from(&std::env::current_dir()?);
    let plan = plan(args, &root, aida_core::home::home_dir().as_deref())?;
    let host = match &plan.host {
        Some(host) => Some((host.clone(), remote_dir(&plan, &root)?)),
        None => None,
    };
    let mut world = Live {
        root,
        host,
        aida: crate::resolve_aida_exe(),
        ssh_env: Vec::new(),
        started: std::time::Instant::now(),
    };
    match relay(args, &plan, &mut world)? {
        Outcome::Admitted { .. } => Ok(()),
        Outcome::Stopped(_) => {
            use std::io::Write;
            let _ = std::io::stdout().flush();
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
#[path = "tests/task_3826_lane_relay_tests.rs"]
mod tests;
