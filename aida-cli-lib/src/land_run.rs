//! The acting half of the landing reconciler: `aida land run` runs the same
//! pass as `aida land status` and then carries out each row's one action.
//!
//! Reviewers, rework lanes and the train are child processes of this one, so
//! a reviewer's verdict binds through its process ancestry and liveness is a
//! fact about a process this one started. The only things written between
//! passes are those processes' identities and two small counters keyed by a
//! change's head; every one is checked against what is alive, and against
//! the open changes, each time it is read.
// trace:STORY-2768 | ai:claude

use super::{one_line, specs_of, Action, Pr, Row, State, Track, INTEGRATION_BRANCH};
use aida_core::proc_identity::{self, ProcIdentity};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The record of the children a run started.
const BOOK: &str = ".aida/cache/land-children.json";
/// Held by the one `aida land run` a clone may have.
const RUN_LOCK: &str = ".aida/cache/land-run.lock";
const LOG_DIR: &str = ".aida/land/logs";
const TRAIN_DIR: &str = ".aida/train";
/// The most changes one train takes.
const TRAIN_MAX: usize = 10;
/// A train older than this is reported. It is never stopped: stopping it
/// could interrupt a merge or cancel its CI.
const TRAIN_WARN_SECS: i64 = 90 * 60;
/// Reviewers of one head that may be stopped for running too long before
/// the change goes to the advisor: the first is relaunched once.
pub(super) const MAX_REVIEW_TIMEOUTS: u32 = 2;
/// A lane's refusal that a relaunch cannot fix: its seat holds commits
/// that exist nowhere else.
const SEAT_REFUSAL: &str = "that no remote has";

const DEFAULT_MAX_REVIEWS: usize = 2;
const DEFAULT_MAX_LANES: usize = 2;
const DEFAULT_REVIEW_TIMEOUT_SECS: i64 = 45 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Kind {
    Review,
    Lane,
    Train,
}

/// One child process a run started.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Child {
    pub kind: Kind,
    /// The change a reviewer or lane works; every member of a train.
    pub prs: Vec<u64>,
    /// The head a reviewer was launched against, or a lane was launched to
    /// rework. Empty for a train.
    pub head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    /// The tree of the clean merge into the base that the reviewer's launch
    /// on a possibly stale base relied on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_tree: Option<String>,
    pub identity: ProcIdentity,
    pub started_at: String,
    pub log: String,
}

impl Child {
    /// How the child reads in the table.
    pub(super) fn describe(&self) -> String {
        let what = match self.kind {
            Kind::Review => "reviewer",
            Kind::Lane => "rework lane",
            Kind::Train => "train",
        };
        format!(
            "land run's {what} (pid {}), started {}",
            self.identity.pid, self.started_at
        )
    }

    fn age_secs(&self, now: DateTime<Utc>) -> i64 {
        DateTime::parse_from_rfc3339(&self.started_at)
            .map(|at| (now - at.with_timezone(&Utc)).num_seconds())
            .unwrap_or(0)
    }
}

/// Something a run recorded about one head of one change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Mark {
    pub pr: u64,
    pub head: String,
    pub count: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Book {
    #[serde(default)]
    pub children: Vec<Child>,
    /// Reviewers stopped for running too long, counted per head.
    #[serde(default)]
    pub timeouts: Vec<Mark>,
    /// Heads whose rework lane refused in a way a relaunch cannot fix.
    #[serde(default)]
    pub stops: Vec<Mark>,
}

impl Book {
    /// The record on disk; none is an empty record. One that cannot be
    /// parsed is an error: reading it as empty would launch a second
    /// reviewer beside a live one.
    pub(super) fn read(root: &Path) -> Result<Self> {
        let path = root.join(BOOK);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| {
                format!(
                    "{} is not a valid record of land run's children",
                    path.display()
                )
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn write(&self, root: &Path) -> Result<()> {
        let path = root.join(BOOK);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let scratch = path.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&scratch, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", scratch.display()))?;
        std::fs::rename(&scratch, &path).with_context(|| format!("replacing {}", path.display()))
    }

    /// Bring the record in line with the facts: children that are no longer
    /// alive leave it, a lane that died refusing over unpushed seat work
    /// leaves a stop for its head, and anything recorded about a head that
    /// is no longer an open change's head is dropped.
    pub(super) fn settle(
        &mut self,
        changes: &[Pr],
        alive: &dyn Fn(&Child) -> bool,
        log_tail: &dyn Fn(&Child) -> String,
    ) -> Vec<String> {
        let mut notes = Vec::new();
        let mut live = Vec::new();
        for child in std::mem::take(&mut self.children) {
            if alive(&child) {
                live.push(child);
                continue;
            }
            if child.kind != Kind::Lane {
                continue;
            }
            let tail = log_tail(&child);
            let Some(refusal) = tail.lines().rev().find(|line| line.contains(SEAT_REFUSAL)) else {
                continue;
            };
            for pr in &child.prs {
                let reason = format!("{} (log: {})", one_line(refusal, 240), child.log);
                notes.push(format!("#{pr}: the rework lane refused: {reason}"));
                self.stops.retain(|stop| stop.pr != *pr);
                self.stops.push(Mark {
                    pr: *pr,
                    head: child.head.clone(),
                    count: 1,
                    reason,
                });
            }
        }
        self.children = live;
        let current = |mark: &Mark| {
            changes
                .iter()
                .any(|pr| pr.id == mark.pr && pr.head == mark.head)
        };
        self.timeouts.retain(current);
        self.stops.retain(current);
        notes
    }

    pub(super) fn reviewer(&self, pr: &Pr) -> Option<&Child> {
        self.children.iter().find(|child| {
            child.kind == Kind::Review && child.prs.contains(&pr.id) && child.head == pr.head
        })
    }

    /// A lane moves the head, so it is found by its change alone.
    pub(super) fn lane(&self, pr: &Pr) -> Option<&Child> {
        self.children
            .iter()
            .find(|child| child.kind == Kind::Lane && child.prs.contains(&pr.id))
    }

    fn train(&self) -> Option<&Child> {
        self.children.iter().find(|child| child.kind == Kind::Train)
    }

    pub(super) fn timeouts(&self, pr: &Pr) -> u32 {
        self.timeouts
            .iter()
            .find(|mark| mark.pr == pr.id && mark.head == pr.head)
            .map_or(0, |mark| mark.count)
    }

    pub(super) fn stop(&self, pr: &Pr) -> Option<String> {
        self.stops
            .iter()
            .find(|mark| mark.pr == pr.id && mark.head == pr.head)
            .map(|mark| mark.reason.clone())
    }

    fn count_timeout(&mut self, pr: u64, head: &str) {
        match self
            .timeouts
            .iter_mut()
            .find(|mark| mark.pr == pr && mark.head == head)
        {
            Some(mark) => mark.count += 1,
            None => self.timeouts.push(Mark {
                pr,
                head: head.to_string(),
                count: 1,
                reason: "reviewer stopped for running too long".into(),
            }),
        }
    }
}

/// Whether a recorded child is still running. An identity that cannot be
/// judged counts as running: a second reviewer is never launched on a guess.
pub(super) fn alive(child: &Child) -> bool {
    !matches!(
        proc_identity::observe(&child.identity, true),
        proc_identity::Verdict::Dead(_)
    )
}

/// The end of a child's log.
pub(super) fn log_tail(child: &Child) -> String {
    let text = std::fs::read(&child.log).unwrap_or_default();
    let from = text.len().saturating_sub(16 * 1024);
    String::from_utf8_lossy(&text[from..]).into_owned()
}

pub(crate) struct Limits {
    pub max_reviews: usize,
    pub max_lanes: usize,
    pub review_timeout_secs: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_reviews: DEFAULT_MAX_REVIEWS,
            max_lanes: DEFAULT_MAX_LANES,
            review_timeout_secs: DEFAULT_REVIEW_TIMEOUT_SECS,
        }
    }
}

impl Limits {
    /// `[land] max_reviews`, `max_lanes` and `review_timeout_secs`. They
    /// bound counts and a wait; none selects what runs.
    fn load(root: &Path) -> Self {
        let config = std::fs::read_to_string(root.join(".aida").join("config.toml"))
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
        let number = |key: &str, low: i64, high: i64| {
            config
                .as_ref()
                .and_then(|config| config.get("land")?.get(key)?.as_integer())
                .filter(|value| (low..=high).contains(value))
        };
        let defaults = Self::default();
        Self {
            max_reviews: number("max_reviews", 1, 8)
                .map_or(defaults.max_reviews, |value| value as usize),
            max_lanes: number("max_lanes", 1, 8).map_or(defaults.max_lanes, |value| value as usize),
            review_timeout_secs: number("review_timeout_secs", 300, 4 * 3600)
                .unwrap_or(defaults.review_timeout_secs),
        }
    }
}

/// Every side effect a pass can have, behind one seam.
pub(crate) trait Acts {
    /// Start a reviewer of `pr`'s head. `merge_tree` is the clean merge of
    /// the head into its base that this pass computed.
    fn launch_review(&mut self, pr: &Pr, merge_tree: Option<&str>) -> Result<Child>;
    /// Start a lane to rework `pr` under `spec`.
    fn dispatch_rework(&mut self, pr: &Pr, spec: &str) -> Result<Child>;
    /// Release `pr`'s hold. The release decides for itself whether it may.
    fn release_hold(&mut self, pr: &Pr) -> Result<String>;
    fn train(&mut self, prs: &[u64]) -> Result<Child>;
    /// Why no train may start now.
    fn train_blocker(&mut self) -> Option<String>;
    /// Whether a verdict is recorded for `pr` at `head`.
    fn verdict_at(&mut self, pr: u64, head: &str) -> std::result::Result<bool, String>;
    /// Stop a child and everything it started.
    fn stop(&mut self, child: &Child);
}

/// Look after the children already running: a reviewer whose head moved or
/// whose change closed is stopped, and one past the time limit is stopped
/// unless its verdict cannot be read. A stop past the limit counts against
/// the head only when no verdict was recorded there.
pub(crate) fn supervise(
    changes: &[Pr],
    book: &mut Book,
    limits: &Limits,
    now: DateTime<Utc>,
    acts: &mut dyn Acts,
    dry_run: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    let mut kept = Vec::new();
    for child in std::mem::take(&mut book.children) {
        let age = child.age_secs(now);
        let pr = child.prs.first().copied().unwrap_or_default();
        let stop = |why: String, notes: &mut Vec<String>, acts: &mut dyn Acts| {
            if dry_run {
                notes.push(format!("#{pr}: would stop the reviewer: {why}"));
                false
            } else {
                acts.stop(&child);
                notes.push(format!("#{pr}: stopped the reviewer: {why}"));
                true
            }
        };
        let stopped = match child.kind {
            Kind::Review => {
                let current = changes
                    .iter()
                    .any(|change| change.id == pr && change.head == child.head);
                if !current {
                    stop(
                        "the head moved or the change closed".into(),
                        &mut notes,
                        acts,
                    )
                } else if age > limits.review_timeout_secs {
                    // A reviewer that recorded its verdict and is only slow
                    // to exit has done its work.
                    match acts.verdict_at(pr, &child.head) {
                        Ok(true) => stop(
                            "its verdict is recorded and it had not exited".into(),
                            &mut notes,
                            acts,
                        ),
                        Ok(false) => {
                            let stopped = stop(
                                format!("no verdict after {} min", age / 60),
                                &mut notes,
                                acts,
                            );
                            if stopped {
                                book.count_timeout(pr, &child.head);
                            }
                            stopped
                        }
                        Err(why) => {
                            notes.push(format!(
                                "#{pr}: the reviewer is past its time limit and stays: whether it recorded a verdict could not be read ({why})"
                            ));
                            false
                        }
                    }
                } else {
                    false
                }
            }
            Kind::Train => {
                if age > TRAIN_WARN_SECS {
                    notes.push(format!(
                        "the train of {} has run {} min; it is left running (log: {})",
                        numbers(&child.prs),
                        age / 60,
                        child.log
                    ));
                }
                false
            }
            Kind::Lane => false,
        };
        if !stopped {
            kept.push(child);
        }
    }
    book.children = kept;
    notes
}

fn numbers(prs: &[u64]) -> String {
    prs.iter()
        .map(|pr| format!("#{pr}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Carry out each row's one action. A failed action is reported and left
/// for the next pass, which derives the row again from the facts.
pub(crate) fn act(
    changes: &[Pr],
    rows: &[Row],
    merge_trees: &BTreeMap<u64, String>,
    book: &mut Book,
    limits: &Limits,
    acts: &mut dyn Acts,
    dry_run: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    // Every review in progress counts against the cap, whoever started it.
    let mut reviews = rows
        .iter()
        .filter(|row| row.state == State::Reviewing)
        .count();
    let mut lanes = book
        .children
        .iter()
        .filter(|child| child.kind == Kind::Lane)
        .count();
    let mut ready: Vec<u64> = Vec::new();
    for row in rows {
        let Some(pr) = changes.iter().find(|pr| pr.id == row.pr) else {
            continue;
        };
        let id = pr.id;
        match row.action {
            Action::Wait | Action::ReportToIntegration | Action::AdvisorRuling => {}
            Action::LaunchReview => {
                if reviews >= limits.max_reviews {
                    notes.push(format!(
                        "#{id}: review waits: {reviews} of {} reviews are running",
                        limits.max_reviews
                    ));
                } else if dry_run {
                    reviews += 1;
                    notes.push(format!("#{id}: would launch a reviewer"));
                } else {
                    match acts.launch_review(pr, merge_trees.get(&id).map(String::as_str)) {
                        Ok(child) => {
                            reviews += 1;
                            notes.push(format!("#{id}: launched a reviewer (log: {})", child.log));
                            book.children.push(child);
                        }
                        Err(error) => notes.push(format!(
                            "#{id}: the reviewer did not launch, retried next pass: {}",
                            one_line(&format!("{error:#}"), 300)
                        )),
                    }
                }
            }
            Action::DispatchRework => {
                let Some(spec) = specs_of(pr).into_iter().next() else {
                    notes.push(format!(
                        "#{id}: no rework lane: the change's title names no spec"
                    ));
                    continue;
                };
                if lanes >= limits.max_lanes {
                    notes.push(format!(
                        "#{id}: rework waits: {lanes} of {} lanes are running",
                        limits.max_lanes
                    ));
                } else if dry_run {
                    lanes += 1;
                    notes.push(format!("#{id}: would launch a rework lane for {spec}"));
                } else {
                    match acts.dispatch_rework(pr, &spec) {
                        Ok(child) => {
                            lanes += 1;
                            notes.push(format!(
                                "#{id}: launched a rework lane for {spec} (log: {})",
                                child.log
                            ));
                            book.children.push(child);
                        }
                        Err(error) => notes.push(format!(
                            "#{id}: the rework lane did not launch, retried next pass: {}",
                            one_line(&format!("{error:#}"), 300)
                        )),
                    }
                }
            }
            // The release is its own decision. Nothing else happens to the
            // change in this pass: the next one derives it from the facts.
            Action::ClearHoldAndLand => {
                if dry_run {
                    notes.push(format!("#{id}: would ask the hold's release to decide"));
                } else {
                    match acts.release_hold(pr) {
                        Ok(line) => notes.push(line),
                        Err(error) => notes.push(format!(
                            "#{id}: the hold stays: {}",
                            one_line(&format!("{error:#}"), 300)
                        )),
                    }
                }
            }
            Action::Train => {
                if row.state != State::Ready || row.track != Track::Main {
                    continue;
                }
                // The train's CI is the gate on the merged result; a member
                // still needs its own checks green at this head to board.
                if row.ci.as_deref() == Some("green") {
                    ready.push(id);
                } else {
                    notes.push(format!(
                        "#{id}: ready, not boarded: CI on its head is {} (a cancelled run counts as pending; rerun it)",
                        row.ci.as_deref().unwrap_or("unread")
                    ));
                }
            }
        }
    }
    if ready.is_empty() {
        return notes;
    }
    ready.truncate(TRAIN_MAX);
    if let Some(train) = book.train() {
        notes.push(format!(
            "{} wait: the train of {} is running",
            numbers(&ready),
            numbers(&train.prs)
        ));
    } else if let Some(blocker) = acts.train_blocker() {
        notes.push(format!("{} wait: {blocker}", numbers(&ready)));
    } else if dry_run {
        notes.push(format!("would start one train of {}", numbers(&ready)));
    } else {
        match acts.train(&ready) {
            Ok(child) => {
                notes.push(format!(
                    "started one train of {} (log: {})",
                    numbers(&ready),
                    child.log
                ));
                book.children.push(child);
            }
            Err(error) => notes.push(format!(
                "the train of {} did not start, retried next pass: {}",
                numbers(&ready),
                one_line(&format!("{error:#}"), 300)
            )),
        }
    }
    notes
}

// ---- live acts -------------------------------------------------------------

struct Live {
    root: PathBuf,
    repo: crate::review_watch::forge::Repository,
    program: PathBuf,
    vendor: Option<String>,
    gitlab: bool,
    /// The children of this process, kept so each is reaped when it exits.
    handles: Vec<std::process::Child>,
}

impl Live {
    /// An `aida` command in this repository, without the review and gate
    /// context this process may have inherited, so the child takes its
    /// ordinary attested path.
    fn command(&self, args: &[String]) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command
            .current_dir(&self.root)
            .args(args)
            .stdin(std::process::Stdio::null());
        for (key, _) in std::env::vars_os() {
            if key.to_str().is_some_and(|key| {
                key.starts_with("AIDA_REVIEW_")
                    || key.starts_with("AIDA_FROM_PR_")
                    || key.starts_with("AIDA_AGENT_GATE_")
            }) {
                command.env_remove(key);
            }
        }
        command
            .env_remove("AIDA_GRADED_REVIEW_CONTEXT")
            .env_remove("AIDA_SESSION_PURPOSE")
            .env_remove("AIDA_CARGO_NO_SLOT");
        self.repo.pin_command(&mut command);
        command
    }

    /// Run a short `aida` command to its end.
    fn finish(&self, args: &[String], timeout_secs: u64) -> Result<()> {
        let shown = args.join(" ");
        let out = crate::command_output_with_timeout(
            self.command(args),
            std::time::Duration::from_secs(timeout_secs),
        )
        .with_context(|| format!("`aida {shown}` did not finish in {timeout_secs}s"))?;
        anyhow::ensure!(
            out.status.success(),
            "`aida {shown}` failed: {}",
            one_line(&String::from_utf8_lossy(&out.stderr), 300)
        );
        Ok(())
    }

    /// Start a long-running child of this process in its own process group
    /// (so all of it can be stopped at once), logging to a file. It stays a
    /// descendant of this process: that is what lets a reviewer's verdict
    /// bind.
    fn start(
        &mut self,
        kind: Kind,
        name: &str,
        prs: &[u64],
        head: &str,
        args: &[String],
    ) -> Result<Child> {
        use std::os::unix::process::CommandExt as _;
        let now = Utc::now();
        let dir = self.root.join(LOG_DIR);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let log = dir.join(format!("{name}-{}.log", now.format("%Y%m%dT%H%M%SZ")));
        let out =
            std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
        let err = out.try_clone()?;
        let mut command = self.command(args);
        command.stdout(out).stderr(err).process_group(0);
        let mut child = command
            .spawn()
            .with_context(|| format!("starting `aida {}`", args.join(" ")))?;
        let identity = match ProcIdentity::capture_child(&child) {
            Ok(identity) => identity,
            Err(why) => {
                // A child that cannot be identified cannot be looked after.
                crate::bounded_process::kill_process_group(child.id());
                let _ = child.wait();
                anyhow::bail!("the child's process identity could not be read: {why}");
            }
        };
        self.handles.push(child);
        Ok(Child {
            kind,
            prs: prs.to_vec(),
            head: head.to_string(),
            spec: None,
            merge_tree: None,
            identity,
            started_at: now.to_rfc3339(),
            log: log.display().to_string(),
        })
    }

    /// Collect the children that have exited, so a finished one reads as
    /// gone and not as a process still there.
    fn reap(&mut self) {
        self.handles
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

impl Acts for Live {
    fn launch_review(&mut self, pr: &Pr, merge_tree: Option<&str>) -> Result<Child> {
        // A change into the integration branch gets its review story first.
        if pr.base == INTEGRATION_BRANCH {
            if let Some(branch) = pr.branch.as_deref() {
                if let Err(error) = self.finish(
                    &strings(&["pr", "auto-queue-review", "--branch", branch]),
                    120,
                ) {
                    eprintln!("#{}: {error:#}", pr.id);
                }
            }
        }
        let key = format!("{}-{}", if self.gitlab { "MR" } else { "PR" }, pr.id);
        let mut args = strings(&[
            "queue",
            "work",
            &key,
            "--role",
            "reviewer",
            "--no-human",
            "--fresh",
        ]);
        // This pass merged the head into its base cleanly, so a base that
        // moved does not stop the review. The approval still has to carry
        // over to the head that lands.
        if merge_tree.is_some() {
            args.push("--allow-stale-base".into());
        }
        if let Some(vendor) = &self.vendor {
            args.extend(["--vendor".into(), vendor.clone()]);
        }
        let mut child = self.start(
            Kind::Review,
            &format!("review-{}", pr.id),
            &[pr.id],
            &pr.head,
            &args,
        )?;
        child.merge_tree = merge_tree.map(str::to_string);
        Ok(child)
    }

    fn dispatch_rework(&mut self, pr: &Pr, spec: &str) -> Result<Child> {
        // Routing the spec back to an implementer is best effort: the lane
        // below claims it either way.
        if let Err(error) = self.finish(
            &strings(&["queue", "rework", spec, "--for", "implementer"]),
            60,
        ) {
            eprintln!("#{}: {error:#}", pr.id);
        }
        // No vendor is named: the lane keeps the one its change already has.
        let mut child = self.start(
            Kind::Lane,
            &format!("rework-{}", pr.id),
            &[pr.id],
            &pr.head,
            &strings(&[
                "queue",
                "work",
                spec,
                "--user",
                "role:implementer",
                "--role",
                "implementer",
                "--no-human",
                "--fresh",
                "--force-claim",
            ]),
        )?;
        child.spec = Some(spec.to_string());
        Ok(child)
    }

    fn release_hold(&mut self, pr: &Pr) -> Result<String> {
        crate::gate::rework_hold_release::auto_release(&self.root, pr.id)
            .map(|outcome| outcome.line(pr.id))
    }

    fn train(&mut self, prs: &[u64]) -> Result<Child> {
        let mut args = strings(&["pr", "train"]);
        args.extend(prs.iter().map(u64::to_string));
        self.start(Kind::Train, "train", prs, "", &args)
    }

    fn train_blocker(&mut self) -> Option<String> {
        use fs2::FileExt as _;
        let dir = self.root.join(TRAIN_DIR);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => return Some(format!("{} could not be read: {error}", dir.display())),
        };
        // A journal left behind is a train that did not finish; a new one
        // must not start beside it.
        let mut journals: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .collect();
        journals.sort();
        if let Some(journal) = journals.first() {
            return Some(format!(
                "the train journal {journal} remains; finish or abandon that train first"
            ));
        }
        let lock = std::fs::File::open(dir.join("run.lock")).ok()?;
        match lock.try_lock_exclusive() {
            Ok(()) => None,
            Err(_) => Some("another train is running in this repository".into()),
        }
    }

    fn verdict_at(&mut self, pr: u64, head: &str) -> std::result::Result<bool, String> {
        let change = crate::gate::ledger::ChangeKey {
            forge: if self.gitlab {
                crate::gate::ledger::ChangeForge::Mr
            } else {
                crate::gate::ledger::ChangeForge::Pr
            },
            number: pr,
        };
        Ok(
            crate::gate::ledger::project::change_verdict_records_checked(&self.root, change)?
                .iter()
                .any(|record| {
                    crate::gate::review_verdict::recording_key(record)
                        .0
                        .as_deref()
                        == Some(head)
                }),
        )
    }

    fn stop(&mut self, child: &Child) {
        // Only a process proven to be the one recorded is signalled.
        if matches!(
            proc_identity::observe(&child.identity, true),
            proc_identity::Verdict::Alive
        ) {
            crate::bounded_process::kill_process_group(child.identity.pid);
        }
        self.reap();
    }
}

/// `aida land run`.
pub(super) fn run(start: &Path, args: &super::land_args::RunArgs) -> Result<()> {
    use fs2::FileExt as _;
    let root = crate::review_watch::forge::main_root(start)?;
    let _lock = if args.dry_run {
        None
    } else {
        // Reviewers are this process's children; without a grant that
        // delegates the reviewer seat none of their verdicts could bind.
        crate::gate::seat_authority::validate_child_delegation(&root, "reviewer").context(
            "`aida land run` launches reviewers, so it needs a seat grant that delegates reviewer",
        )?;
        let path = root.join(RUN_LOCK);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        lock.try_lock_exclusive()
            .context("another `aida land run` is already running in this repository")?;
        eprintln!(
            "aida land run: autoland and the review watcher should be stopped while this runs; it launches reviewers, rework lanes and trains itself."
        );
        Some(lock)
    };
    let limits = Limits::load(&root);
    let mut acts = Live {
        repo: crate::review_watch::forge::Repository::load(&root)?,
        program: std::env::current_exe().context("resolving the running aida binary")?,
        vendor: args.vendor.clone(),
        gitlab: false,
        handles: Vec::new(),
        root: root.clone(),
    };
    let mut last: Vec<Pr> = Vec::new();
    loop {
        acts.reap();
        let started = std::time::Instant::now();
        match super::live_pass(&root) {
            Ok(mut pass) => {
                acts.gitlab = pass.gitlab;
                let mut notes = std::mem::take(&mut pass.notes);
                notes.extend(supervise(
                    &pass.changes,
                    &mut pass.book,
                    &limits,
                    Utc::now(),
                    &mut acts,
                    args.dry_run,
                ));
                notes.extend(act(
                    &pass.changes,
                    &pass.rows,
                    &pass.merge_trees,
                    &mut pass.book,
                    &limits,
                    &mut acts,
                    args.dry_run,
                ));
                if !args.dry_run {
                    pass.book.write(&root)?;
                }
                let snapshot = crate::now::build(&pass, started.elapsed());
                if let Err(error) = crate::now::write(&pass.root, &snapshot) {
                    eprintln!("the `aida now` snapshot was not written: {error:#}");
                }
                print!("{}", super::render(&pass.rows));
                for note in &notes {
                    println!("  {note}");
                }
                last = pass.changes;
            }
            // A pass that could not run loses nothing: the next derives
            // everything again.
            Err(error) if !args.once && !args.dry_run => {
                eprintln!("aida land run: this pass did not run: {error:#}");
            }
            Err(error) => return Err(error),
        }
        if args.once || args.dry_run {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(args.interval));
    }
    // A child whose launcher has exited loses the ancestry its verdict binds
    // through, so one pass waits for what it started.
    while !args.dry_run {
        acts.reap();
        if acts.handles.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(15));
        acts.reap();
        let mut book = Book::read(&root)?;
        book.settle(&last, &alive, &log_tail);
        for note in supervise(&last, &mut book, &limits, Utc::now(), &mut acts, false) {
            println!("  {note}");
        }
        book.write(&root)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_2768_land_run_tests.rs"]
mod tests;
