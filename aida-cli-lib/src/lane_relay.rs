//! `aida lane relay`: hand an advisor's brief to an implementer lane on a
//! lane host, as a rework of an open change or as fresh work.
//!
//! The relay posts the brief on the spec as its authoritative round brief
//! (marking earlier ones superseded), waits until no lane for the spec runs
//! on the host and the host is above its memory floor, re-checks the change
//! against the head seen at invocation, then delivers the brief through the
//! vendor's brief mailbox and starts the lane with that brief named
//! explicitly. It then watches the lane's admission receipt. Every stop is
//! typed, written to the spec and mailed to the advisor.
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

/// The brief plus its frontmatter must fit a pickup's limit (64 KiB).
const BRIEF_LIMIT: usize = 60 * 1024;
/// Consecutive "no lane process and no receipt" observations that mean the
/// launcher exited without recording an attempt.
const VANISHED_POLLS: u32 = 3;
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
    HeadMoved { pr: u64, from: String, to: String },
    PrNotOpen { pr: u64, state: String },
    PrUnreadable { pr: u64, why: String },
    LaneStillRunning { secs: u64 },
    HostCapacity { secs: u64, why: String },
    HostUnreachable { secs: u64, why: String },
    AdmissionRefused { reason: String, detail: String },
    AdmissionUnconfirmed { secs: u64 },
    LaunchVanished { log_tail: String },
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
            Stop::AdmissionRefused { .. } => "admission-refused",
            Stop::AdmissionUnconfirmed { .. } => "admission-unconfirmed",
            Stop::LaunchVanished { .. } => "launch-vanished",
        }
    }

    /// Whether the lane was started before the stop. Before that, nothing
    /// ran and the brief was not delivered.
    pub(crate) fn launched(&self) -> bool {
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
            _ => "SKIPPED",
        }
    }

    fn explain(&self) -> String {
        match self {
            Stop::HeadMoved { pr, from, to } => format!(
                "PR #{pr} head moved {} -> {} while waiting; the brief targeted the earlier round, so nothing was launched. Re-route needed.",
                short(from),
                short(to)
            ),
            Stop::PrNotOpen { pr, state } => format!(
                "PR #{pr} is {state}, not OPEN; the rework brief has no round left to run on, so nothing was launched."
            ),
            Stop::PrUnreadable { pr, why } => format!(
                "PR #{pr} could not be read before the launch ({why}); its head is unverified, so nothing was launched."
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

/// The relay's side effects, so tests drive the real flow against a fake
/// forge, store and clock and a lane host rooted in a temp directory.
pub(crate) trait World {
    fn pr(&mut self, pr: u64) -> Result<PrView>;
    /// `(comment id, content)` for each top-level comment on the spec.
    fn comments(&mut self, spec: &str) -> Result<Vec<(String, String)>>;
    fn comment_add(&mut self, spec: &str, body: &str) -> Result<()>;
    fn comment_edit(&mut self, spec: &str, id: &str, body: &str) -> Result<()>;
    fn store_push(&mut self) -> Result<()>;
    fn notify_advisor(&mut self, subject: &str, body: &str) -> Result<()>;
    /// Run one lane-host operation and return its JSON.
    fn host(&mut self, op: &RelayHostCommand) -> Result<serde_json::Value>;
    fn sleep(&mut self, duration: Duration);
    fn log(&mut self, line: &str);
}

fn read_brief(path: &Path, footer: Option<&str>) -> Result<String> {
    let mut body = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read brief {}", path.display()))?;
    if let Some(footer) = footer.map(str::trim).filter(|f| !f.is_empty()) {
        if !body.contains(footer) {
            body = format!("{}\n\n{footer}\n", body.trim_end());
        }
    }
    anyhow::ensure!(
        !body.trim().is_empty() && !body.contains('\0') && body.len() <= BRIEF_LIMIT,
        "brief {} must be nonempty text of at most {BRIEF_LIMIT} bytes",
        path.display()
    );
    Ok(body)
}

fn label_for(args: &RelayArgs, mode: &Mode) -> Result<String> {
    let label = match &args.label {
        Some(label) => label.trim().to_ascii_uppercase(),
        None => match mode {
            Mode::Rework { .. } => "REWORK".into(),
            Mode::Fresh => "FRESH".into(),
        },
    };
    anyhow::ensure!(
        !label.is_empty()
            && label.len() <= 40
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')),
        "invalid --label `{label}`: use letters, digits, spaces, `-` and `_`"
    );
    Ok(label)
}

/// Whether a spec comment is a relayed round brief still in force.
pub(crate) fn is_authoritative_brief(content: &str) -> bool {
    crate::rework_direction::is_relayed_brief_lead(&crate::rework_direction::lead_line(content))
}

/// Post the round brief and its pointer comment, marking earlier round
/// briefs superseded. Returns the new brief comment's id when it can be found.
fn post_brief(
    world: &mut dyn World,
    spec: &str,
    label: &str,
    plan: &Plan,
    mode: &Mode,
    body: &str,
) -> Result<Option<String>> {
    let stamp = Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
    let mut superseded = 0;
    for (id, content) in world.comments(spec)? {
        if is_authoritative_brief(&content) {
            world.comment_edit(
                spec,
                &id,
                &format!(
                    "SUPERSEDED by the {label} brief relayed {stamp}. Do not act on this brief.\n\n{content}"
                ),
            )?;
            superseded += 1;
        }
    }
    let target = match mode {
        Mode::Rework { pr, head, .. } => format!("; PR #{pr} at head {}", short(head)),
        Mode::Fresh => String::new(),
    };
    let heading = format!(
        "AUTHORITATIVE {label} BRIEF (relayed {stamp}; {} lane on {}{target}):",
        plan.vendor,
        plan.host_name()
    );
    let brief_comment = format!("{heading}\n{}", body.trim_end());
    world.comment_add(spec, &brief_comment)?;
    let branch = match mode {
        Mode::Rework { branch, .. } => format!(", branch {branch}"),
        Mode::Fresh => ", fresh work on a new branch".into(),
    };
    world.comment_add(
        spec,
        &format!(
            "Lane relay {stamp}: {label} lane queued on {} ({}{branch}). Scope = the AUTHORITATIVE {label} BRIEF in the previous comment{}.",
            plan.host_name(),
            plan.vendor,
            match superseded {
                0 => String::new(),
                n => format!("; {n} earlier round brief(s) marked SUPERSEDED"),
            }
        ),
    )?;
    Ok(world
        .comments(spec)
        .ok()
        .and_then(|all| {
            all.into_iter()
                .rev()
                .find(|(_, content)| content.trim() == brief_comment.trim())
        })
        .map(|(id, _)| id))
}

/// Record a stop where a person will see it: the relay log, the spec and the
/// advisor's mailbox. A brief that never launched is marked so no later lane
/// treats it as the round in force. None of this can fail the relay.
fn report(
    world: &mut dyn World,
    spec: &str,
    label: &str,
    brief_comment: Option<&str>,
    stop: &Stop,
) {
    let line = stop.line(spec, label);
    world.log(&line);
    if !stop.launched() {
        if let Some(id) = brief_comment {
            let marked = world.comments(spec).ok().and_then(|all| {
                let (_, content) = all.into_iter().find(|(cid, _)| cid == id)?;
                world
                    .comment_edit(
                        spec,
                        id,
                        &format!(
                            "SUPERSEDED: not launched ({}). Do not act on this brief.\n\n{content}",
                            stop.code()
                        ),
                    )
                    .ok()
            });
            if marked.is_none() {
                world.log("warning: could not mark the unlaunched brief comment superseded");
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

/// Wait, bounded, until no lane for the spec runs on the host and the host
/// is above its memory floor.
fn wait_for_host(world: &mut dyn World, args: &RelayArgs, spec: &str) -> Option<Stop> {
    let mut waited = 0;
    let mut last = String::new();
    loop {
        let blocked = match world.host(&RelayHostCommand::Probe { spec: spec.into() }) {
            Ok(probe) => {
                // A reply without the field is not evidence the host is idle.
                if probe["running"].as_bool() != Some(false) {
                    Stop::LaneStillRunning { secs: waited }
                } else if let Some(why) = probe["shortfall"].as_str() {
                    Stop::HostCapacity {
                        secs: waited,
                        why: why.into(),
                    }
                } else {
                    return None;
                }
            }
            Err(err) => Stop::HostUnreachable {
                secs: waited,
                why: format!("{err:#}"),
            },
        };
        if waited >= args.wait_timeout {
            return Some(blocked);
        }
        if blocked.code() != last {
            last = blocked.code().into();
            world.log(&format!(
                "waiting ({last}) before launching {spec}; checking every {}s for up to {}s",
                args.poll, args.wait_timeout
            ));
        }
        world.sleep(Duration::from_secs(args.poll));
        waited += args.poll;
    }
}

/// The head gate: the change must still be open at the head seen at
/// invocation. An unreadable change is not a pass.
fn head_gate(world: &mut dyn World, pr: u64, head: &str) -> Option<Stop> {
    let mut error = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            world.sleep(Duration::from_secs(5));
        }
        match world.pr(pr) {
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

fn text(value: &serde_json::Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("the reply has no `{key}`: {value}"))
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
    let (mut waited, mut vanished, mut announced) = (0, 0, false);
    loop {
        match world.host(&op) {
            Ok(reply) => match reply["state"].as_str().unwrap_or("pending") {
                "started" | "finished" => return None,
                "refused" => {
                    return Some(Stop::AdmissionRefused {
                        reason: reply["reason"].as_str().unwrap_or("unknown").into(),
                        detail: reply["detail"].as_str().unwrap_or("").into(),
                    })
                }
                "vanished" => {
                    vanished += 1;
                    if vanished >= VANISHED_POLLS {
                        return Some(Stop::LaunchVanished {
                            log_tail: reply["log_tail"].as_str().unwrap_or("").into(),
                        });
                    }
                }
                _ => vanished = 0,
            },
            Err(err) => world.log(&format!("warning: admission check failed: {err:#}")),
        }
        if waited >= args.admission_timeout {
            return Some(Stop::AdmissionUnconfirmed { secs: waited });
        }
        if !announced {
            announced = true;
            world.log(&format!(
                "launched {spec}; watching its admission for up to {}s",
                args.admission_timeout
            ));
        }
        world.sleep(Duration::from_secs(args.poll));
        waited += args.poll;
    }
}

/// One relay run, start to recorded outcome.
pub(crate) fn relay(args: &RelayArgs, plan: &Plan, world: &mut dyn World) -> Result<Outcome> {
    let spec = args.spec.as_str();
    let body = read_brief(&args.brief, plan.footer.as_deref())?;
    let mode = match args.pr {
        None => Mode::Fresh,
        Some(pr) => {
            let view = world
                .pr(pr)
                .with_context(|| format!("cannot read PR #{pr}, so its head cannot be recorded"))?;
            if view.state.eq_ignore_ascii_case("OPEN") {
                plain_value("PR branch", &view.branch)?;
                Mode::Rework {
                    pr,
                    branch: view.branch,
                    head: view.head,
                }
            } else {
                // A finished change has no round to rework: routing a lane
                // onto its branch ends in a no-change exit.
                world.log(&format!(
                    "PR #{pr} is {}: launching fresh work for {spec}, not a rework of that PR",
                    view.state
                ));
                Mode::Fresh
            }
        }
    };
    let label = label_for(args, &mode)?;
    let brief_comment = post_brief(world, spec, &label, plan, &mode, &body)?;
    if let Err(err) = world.store_push() {
        world.log(&format!("warning: store push failed: {err:#}"));
    }
    world.log(&format!(
        "queued {spec} ({label}, {} on {})",
        plan.vendor,
        plan.host_name()
    ));

    let mut stop = wait_for_host(world, args, spec);
    if let (None, Mode::Rework { pr, head, .. }) = (&stop, &mode) {
        stop = head_gate(world, *pr, head);
    }
    if let Some(stop) = stop {
        report(world, spec, &label, brief_comment.as_deref(), &stop);
        return Ok(Outcome::Stopped(stop));
    }

    let placed = world
        .host(&RelayHostCommand::PlaceBrief {
            spec: spec.into(),
            vendor: plan.vendor.clone(),
            label: label.clone(),
            body_b64: base64::engine::general_purpose::STANDARD.encode(&body),
        })
        .context("could not deliver the brief to the lane host; nothing was launched")?;
    let brief = text(&placed, "path")?;
    let launched = world
        .host(&RelayHostCommand::Launch {
            spec: spec.into(),
            vendor: plan.vendor.clone(),
            branch: match &mode {
                Mode::Rework { branch, .. } => Some(branch.clone()),
                Mode::Fresh => None,
            },
            model: plan.model.clone(),
            brief: PathBuf::from(&brief),
        })
        .with_context(|| {
            format!("could not start the lane; the brief stays pending on the host at {brief}")
        })?;
    let (since, log) = (text(&launched, "since")?, text(&launched, "log")?);
    if let Some(stop) = watch_admission(world, args, spec, &since, &log) {
        report(world, spec, &label, brief_comment.as_deref(), &stop);
        return Ok(Outcome::Stopped(stop));
    }
    world.log(&format!(
        "admitted: {spec} ({label}) is running on {}; brief {brief}; log {log}",
        plan.host_name()
    ));
    Ok(Outcome::Admitted { mode, brief, log })
}

// ---------------------------------------------------------------------------
// The lane-host side.

/// What the host operations need from the machine.
pub(crate) trait HostSys {
    /// Whether a lane launcher for `spec` is running.
    fn lane_running(&self, spec: &str) -> bool;
    /// Why the host is below its memory floor, if it is.
    fn shortfall(&self) -> Option<String>;
    /// Start `argv` detached in `cwd`, its output appended to `log`.
    fn spawn(&mut self, argv: &[String], cwd: &Path, log: &Path) -> Result<()>;
    fn now(&self) -> DateTime<Utc>;
    /// The home directory holding this machine's relay settings.
    fn home(&self) -> Option<PathBuf>;
}

/// Whether a process's arguments are a lane launcher working `spec`: an
/// `aida queue work <spec> …`, or a `lane-run.sh <spec> …` wrapper that has
/// not reached it yet.
pub(crate) fn cmdline_runs_lane(argv: &[String], spec: &str) -> bool {
    let same =
        |arg: &String| aida_core::id_identity::same_id(arg, spec) || arg.eq_ignore_ascii_case(spec);
    argv.windows(3)
        .any(|w| w[0] == "queue" && w[1] == "work" && same(&w[2]))
        || argv.windows(2).any(|w| {
            Path::new(&w[0])
                .file_name()
                .is_some_and(|name| name == "lane-run.sh")
                && same(&w[1])
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

fn file_stamp(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%dT%H%M%SZ").to_string()
}

fn mailbox(root: &Path, vendor: &str) -> Result<PathBuf> {
    Ok(root.join(".aida/agent-briefs").join(vendor_name(vendor)?))
}

/// Store `body` as the spec's one pending brief for `vendor`.
fn place_brief(
    root: &Path,
    spec: &str,
    vendor: &str,
    label: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<serde_json::Value> {
    let stem = spec_file_stem(spec)?;
    anyhow::ensure!(
        !body.trim().is_empty() && !body.contains('\0') && body.len() <= BRIEF_LIMIT,
        "the brief must be nonempty text of at most {BRIEF_LIMIT} bytes"
    );
    let dir = mailbox(root, vendor)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let superseded = crate::brief::supersede_pending_briefs(root, spec)?;
    let framed = format!(
        "---\nspec_id: {}\nagent: {vendor}\ngenerated_at: {}\ngenerated_by: {}\nnote: |-\n{}---\n",
        crate::brief::yaml_scalar(spec),
        file_stamp(now),
        crate::brief::yaml_scalar(&format!("aida lane relay ({label})")),
        crate::brief::indent_block(body.trim_end(), "  "),
    );
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

fn launch(
    root: &Path,
    spec: &str,
    vendor: &str,
    branch: Option<&str>,
    model: Option<&str>,
    brief: &Path,
    sys: &mut dyn HostSys,
) -> Result<serde_json::Value> {
    let stem = spec_file_stem(spec)?;
    // Only a brief this host stored for this vendor may be handed to a lane.
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
    let mut argv: Vec<String> = launcher(sys.home().as_deref())?
        .iter()
        .map(|arg| arg.replace("{spec}", spec).replace("{vendor}", vendor))
        .collect();
    if let Some(branch) = branch {
        plain_value("branch", branch)?;
        argv.extend(["--branch".into(), branch.into()]);
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
    sys.spawn(&argv, root, &log)?;
    Ok(serde_json::json!({
        "since": since.to_rfc3339(),
        "log": log,
        "argv": argv,
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
    spec: &str,
    since: &str,
    log: &Path,
    sys: &dyn HostSys,
) -> Result<serde_json::Value> {
    use crate::lane_result::Kind;
    let since = DateTime::parse_from_rfc3339(since)
        .with_context(|| format!("invalid launch time `{since}`"))?
        .with_timezone(&Utc);
    let record = crate::lane_result::admission_since(root, spec, since);
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
        _ if sys.lane_running(spec) => serde_json::json!({ "state": "pending" }),
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
        RelayHostCommand::Probe { spec } => Ok(serde_json::json!({
            "running": sys.lane_running(spec),
            "shortfall": sys.shortfall(),
        })),
        RelayHostCommand::PlaceBrief {
            spec,
            vendor,
            label,
            body_b64,
        } => {
            let body = base64::engine::general_purpose::STANDARD
                .decode(body_b64)
                .context("the brief body is not base64")?;
            let body = String::from_utf8(body).context("the brief body is not UTF-8")?;
            place_brief(root, spec, vendor, label, &body, sys.now())
        }
        RelayHostCommand::Launch {
            spec,
            vendor,
            branch,
            model,
            brief,
        } => launch(
            root,
            spec,
            vendor,
            branch.as_deref(),
            model.as_deref(),
            brief,
            sys,
        ),
        RelayHostCommand::Admission {
            spec,
            launched_at,
            log,
        } => admission(root, spec, launched_at, log, sys),
    }
}

/// The arguments that run `op` as `aida lane relay-host …`.
pub(crate) fn host_argv(op: &RelayHostCommand) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut argv: Vec<String> = vec!["lane".into(), "relay-host".into()];
    match op {
        RelayHostCommand::Probe { spec } => argv.extend(["probe".into(), spec.clone()]),
        RelayHostCommand::PlaceBrief {
            spec,
            vendor,
            label,
            body_b64,
        } => argv.extend(
            [
                "place-brief",
                spec,
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
            vendor,
            branch,
            model,
            brief,
        } => {
            argv.extend(["launch", spec, "--vendor", vendor].map(String::from));
            if let Some(branch) = branch {
                argv.extend(["--branch".into(), branch.clone()]);
            }
            if let Some(model) = model {
                argv.extend(["--model".into(), model.clone()]);
            }
            argv.extend(["--brief".into(), path(brief)]);
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

/// The real machine.
struct Machine;

impl HostSys for Machine {
    fn lane_running(&self, spec: &str) -> bool {
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
            cmdline_runs_lane(&argv, spec)
        })
    }

    fn shortfall(&self) -> Option<String> {
        match crate::lane_admission::memory_floor_shortfall() {
            Ok(shortfall) => shortfall,
            Err(err) => Some(format!("cannot read the host's lane limits: {err:#}")),
        }
    }

    fn spawn(&mut self, argv: &[String], cwd: &Path, log: &Path) -> Result<()> {
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
        aida_core::home::home_dir()
    }
}

/// `aida lane relay-host <op>`: run in the lane host's primary checkout.
pub(crate) fn run_host(op: &RelayHostCommand) -> Result<()> {
    let root = crate::main_worktree_root_from(&std::env::current_dir()?);
    println!("{}", host_op(&root, op, &mut Machine)?);
    Ok(())
}

/// The real forge, store, mailbox and lane host.
struct Live {
    root: PathBuf,
    host: Option<(String, String)>,
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

impl Live {
    fn aida(&self, args: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new(crate::resolve_aida_exe());
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
        use crate::forge::ChangeState;
        let change =
            crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(30), || {
                crate::forge::forge_for(&self.root)
                    .change_metadata(pr, &mut crate::network_retry::NoopSink)
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
        })
    }

    fn comments(&mut self, spec: &str) -> Result<Vec<(String, String)>> {
        let out = bounded(
            self.aida(&["show", spec, "--format", "json"]),
            60,
            "`aida show`",
        )?;
        let json: serde_json::Value =
            serde_json::from_str(&out).context("`aida show --format json` returned non-JSON")?;
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

    fn comment_add(&mut self, spec: &str, body: &str) -> Result<()> {
        self.with_body(body, |file| {
            bounded(
                self.aida(&["comment", "add", spec, "--body-file", file]),
                60,
                "`aida comment add`",
            )
        })
        .map(drop)
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
            return host_op(&self.root, op, &mut Machine);
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
        let out = bounded(cmd, 90, &format!("the lane host command on {host}"))?;
        let line = out
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        serde_json::from_str(line)
            .with_context(|| format!("{host} returned no JSON result: {}", out.trim()))
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
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
    let mut world = Live { root, host };
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
