//! `aida lane relay`: the relay flow driven end to end against a fake forge,
//! spec store and clock, with the real lane-host operations running in a temp
//! checkout (real brief mailbox, real pickup reader, real admission receipts).
// trace:TASK-3826 | ai:claude

use super::*;
use crate::lane_result::{Attempt, Kind};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

const SPEC: &str = "TASK-9";
const H0: &str = "aaaaaaaa00000000000000000000000000000000";
const H1: &str = "bbbbbbbb11111111111111111111111111111111";

#[derive(Default)]
struct HostState {
    running: Cell<bool>,
    shortfall: RefCell<Option<String>>,
    spawned: RefCell<Vec<Vec<String>>>,
    /// The environment each launcher was started with.
    spawn_env: RefCell<Vec<Vec<(String, String)>>>,
    /// Other ids the host's store knows the spec by, canonical first.
    aliases: RefCell<Vec<String>>,
    /// What the forge tells the started launcher, in order; the last repeats.
    launcher_prs: RefCell<VecDeque<PrView>>,
    /// The lane host operator's home directory.
    home: PathBuf,
    /// What the started launcher does on the host.
    on_spawn: RefCell<Option<Box<dyn Fn(&Path, &HostState)>>>,
    /// Store pulls and spec lookups, in the order the host made them.
    store_calls: RefCell<Vec<&'static str>>,
    /// Why the host's store pull fails, when it does.
    pull_error: RefCell<Option<String>>,
}

impl HostState {
    fn owner(&self) -> RelayOwner {
        let env = self.spawn_env.borrow();
        let value = &env
            .last()
            .unwrap()
            .iter()
            .find(|(k, _)| k == OWNER_ENV)
            .unwrap()
            .1;
        serde_json::from_str(value).unwrap()
    }

    /// The gate the last launcher was started under.
    fn gate(&self) -> LaunchGate {
        let env = self.spawn_env.borrow();
        let (_, raw) = env
            .last()
            .and_then(|env| env.iter().find(|(key, _)| key == GATE_ENV))
            .expect("every relayed launch carries its gate");
        LaunchGate::parse(raw).unwrap()
    }
}

/// The forge as the launcher on the host sees it.
impl HeadSource for &HostState {
    fn pr(&mut self, _pr: u64) -> Result<PrView> {
        let mut prs = self.launcher_prs.borrow_mut();
        Ok(if prs.len() > 1 {
            prs.pop_front().unwrap()
        } else {
            prs.front().cloned().expect("a scripted launcher PR reply")
        })
    }
    fn pause(&mut self, _duration: Duration) {}
}

struct FakeSys(Rc<HostState>);

impl HostSys for FakeSys {
    fn refresh_store(&self, _root: &Path) -> Result<()> {
        self.0.store_calls.borrow_mut().push("pull");
        match self.0.pull_error.borrow().as_ref() {
            Some(why) => anyhow::bail!("{why}"),
            None => Ok(()),
        }
    }
    fn spec_ids(&self, _root: &Path, spec: &str) -> Result<Vec<String>> {
        self.0.store_calls.borrow_mut().push("ids");
        let mut ids = self.0.aliases.borrow().clone();
        ids.push(spec.to_string());
        Ok(ids)
    }
    fn lane_running(&self, _root: &Path, _ids: &[String]) -> bool {
        self.0.running.get()
    }
    fn shortfall(&self) -> Option<String> {
        self.0.shortfall.borrow().clone()
    }
    fn spawn(
        &mut self,
        argv: &[String],
        env: &[(String, String)],
        cwd: &Path,
        log: &Path,
    ) -> Result<()> {
        std::fs::write(log, "lane-run: started\n")?;
        self.0.spawned.borrow_mut().push(argv.to_vec());
        self.0.spawn_env.borrow_mut().push(env.to_vec());
        if let Some(hook) = self.0.on_spawn.borrow().as_ref() {
            hook(cwd, &self.0);
        }
        Ok(())
    }
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn home(&self) -> Option<PathBuf> {
        Some(self.0.home.clone())
    }
}

struct FakeWorld {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    host: Rc<HostState>,
    /// Forge replies in order; the last one repeats.
    prs: VecDeque<Result<PrView, String>>,
    pr_reads: usize,
    comments: Vec<(String, String)>,
    /// How many comments were ever added.
    added: usize,
    /// The store keeps a comment's text in its own form, not byte for byte.
    store_reflows_text: bool,
    /// Reading the spec's comments back fails.
    comments_unreadable: bool,
    /// A comment someone else posts while the host starts the launcher.
    posted_during_launch: Option<String>,
    mails: Vec<(String, String)>,
    logs: Vec<String>,
    pushes: usize,
    slept: u64,
    /// Seconds each lane-host operation takes.
    host_cost: u64,
    /// Wall seconds so far: sleeps plus host operations.
    clock: u64,
    /// Runs after each sleep with the total seconds slept.
    on_sleep: Box<dyn FnMut(u64, &HostState)>,
    /// Alters a lane-host reply on its way back.
    on_reply: Box<dyn Fn(&RelayHostCommand, &mut serde_json::Value)>,
}

impl FakeWorld {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        Self {
            _tmp: tmp,
            root: root.clone(),
            host: Rc::new(HostState {
                home: root.join("home"),
                ..Default::default()
            }),
            prs: VecDeque::new(),
            pr_reads: 0,
            comments: Vec::new(),
            added: 0,
            store_reflows_text: false,
            comments_unreadable: false,
            posted_during_launch: None,
            mails: Vec::new(),
            logs: Vec::new(),
            pushes: 0,
            slept: 0,
            host_cost: 0,
            clock: 0,
            on_sleep: Box::new(|_, _| {}),
            on_reply: Box::new(|_, _| {}),
        }
    }

    /// The started launcher records an attempt and `then` says how it went.
    fn launcher_records(&self, then: impl Fn(&Attempt) + 'static) {
        *self.host.on_spawn.borrow_mut() = Some(Box::new(move |root, host| {
            then(&Attempt::begin_with_id(root, SPEC, None, host.owner().attempt).unwrap());
        }));
    }

    fn brief_file(&self, body: &str) -> PathBuf {
        let path = self.root.join("brief-input.md");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn args(&self, pr: Option<u64>, body: &str) -> RelayArgs {
        RelayArgs {
            spec: SPEC.into(),
            pr,
            brief: self.brief_file(body),
            vendor: None,
            model: None,
            host: None,
            host_dir: None,
            label: None,
            wait_timeout: 600,
            admission_timeout: 600,
            poll: 60,
        }
    }

    fn pending(&self, vendor: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> =
            std::fs::read_dir(self.root.join(".aida/agent-briefs").join(vendor))
                .map(|dir| dir.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
        found.retain(|p| p.extension().is_some_and(|e| e == "md"));
        found
    }

    /// The prompt text a lane for `vendor` is handed from the mailbox.
    fn pickup_prompt(&self, vendor: &str) -> String {
        let briefs = crate::brief::pickup_briefs(&self.root, vendor, SPEC, SPEC, None).unwrap();
        let mut prompt = String::new();
        crate::brief::append_pickup_briefs(&mut prompt, &briefs);
        prompt
    }

    fn comment_starting(&self, lead: &str) -> Vec<&str> {
        self.comments
            .iter()
            .map(|(_, c)| c.as_str())
            .filter(|c| c.starts_with(lead))
            .collect()
    }
}

impl World for FakeWorld {
    fn pr(&mut self, _pr: u64) -> Result<PrView> {
        self.pr_reads += 1;
        let reply = if self.prs.len() > 1 {
            self.prs.pop_front().unwrap()
        } else {
            self.prs.front().cloned().expect("a scripted PR reply")
        };
        reply.map_err(|why| anyhow::anyhow!(why))
    }
    fn comments(&mut self, _spec: &str) -> Result<Vec<(String, String)>> {
        anyhow::ensure!(!self.comments_unreadable, "store read timed out");
        Ok(self.comments.clone())
    }
    fn spec_ids(&mut self, spec: &str) -> Result<Vec<String>> {
        let mut ids = self.host.aliases.borrow().clone();
        ids.push(spec.to_string());
        Ok(ids)
    }
    fn comment_add(&mut self, _spec: &str, body: &str) -> Result<String> {
        self.added += 1;
        let id = format!("added-{}", self.added);
        let stored = if self.store_reflows_text {
            body.replace('\n', "\r\n")
        } else {
            body.to_string()
        };
        self.comments.push((id.clone(), stored));
        Ok(id)
    }
    fn comment_edit(&mut self, _spec: &str, id: &str, body: &str) -> Result<()> {
        let comment = self
            .comments
            .iter_mut()
            .find(|(cid, _)| cid == id)
            .context("no such comment")?;
        comment.1 = body.to_string();
        Ok(())
    }
    fn store_push(&mut self) -> Result<()> {
        self.pushes += 1;
        Ok(())
    }
    fn notify_advisor(&mut self, subject: &str, body: &str) -> Result<()> {
        self.mails.push((subject.to_string(), body.to_string()));
        Ok(())
    }
    fn host(&mut self, op: &RelayHostCommand) -> Result<serde_json::Value> {
        // Through the same argument encoding the SSH hop uses.
        let parsed = <crate::lane_cmd::LaneArgs as clap::Parser>::try_parse_from(
            std::iter::once("lane".to_string()).chain(host_argv(op).into_iter().skip(1)),
        )?;
        let crate::cli::LaneCommand::RelayHost(parsed) = parsed.command else {
            panic!("host_argv must build a relay-host command")
        };
        assert_eq!(&parsed, op);
        self.clock += self.host_cost;
        let mut reply = host_op(&self.root, &parsed, &mut FakeSys(self.host.clone()))?;
        if matches!(op, RelayHostCommand::Launch { .. }) {
            if let Some(body) = self.posted_during_launch.take() {
                self.comments.push(("by-hand".into(), body));
            }
        }
        (self.on_reply)(op, &mut reply);
        Ok(reply)
    }
    fn sleep(&mut self, duration: Duration) {
        self.slept += duration.as_secs();
        self.clock += duration.as_secs();
        (self.on_sleep)(self.slept, &self.host);
    }
    fn elapsed(&mut self) -> Duration {
        Duration::from_secs(self.clock)
    }
    fn log(&mut self, line: &str) {
        self.logs.push(line.to_string());
    }
}

fn plan_for(vendor: &str) -> Plan {
    Plan {
        vendor: vendor.into(),
        model: None,
        host: Some("lanehost".into()),
        host_dir: None,
        footer: None,
    }
}

fn open(head: &str) -> Result<PrView, String> {
    Ok(PrView {
        state: "OPEN".into(),
        head: head.into(),
        branch: "task-9-branch".into(),
        title: "fix the gate (Refs TASK-9)".into(),
    })
}

fn not_open(state: &str) -> Result<PrView, String> {
    Ok(PrView {
        state: state.into(),
        head: H0.into(),
        branch: "task-9-branch".into(),
        title: "fix the gate (Refs TASK-9)".into(),
    })
}

fn flag<'a>(argv: &'a [String], name: &str) -> Option<&'a str> {
    argv.iter()
        .position(|arg| arg == name)
        .and_then(|at| argv.get(at + 1))
        .map(String::as_str)
}

/// A prior lane that exits after two polls.
fn prior_lane_exits_after_two_polls(world: &mut FakeWorld) {
    world.host.running.set(true);
    world.on_sleep = Box::new(|slept, host| {
        if slept >= 120 {
            host.running.set(false);
        }
    });
}

#[test]
fn head_moved_while_waiting_is_skipped_and_reported() {
    let mut world = FakeWorld::new();
    world.prs = [open(H0), open(H1)].into();
    prior_lane_exits_after_two_polls(&mut world);
    let args = world.args(Some(7), "round two: fix the gate");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    let Outcome::Stopped(stop) = outcome else {
        panic!("a moved head must not launch")
    };
    assert_eq!(stop.code(), "head-moved");
    assert!(world.host.spawned.borrow().is_empty(), "nothing launched");
    assert!(
        world.pending("claude").is_empty(),
        "the brief was never delivered"
    );
    let skipped = world.comment_starting("Lane relay SKIPPED");
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].contains("head-moved") && skipped[0].contains("aaaaaaaa -> bbbbbbbb"));
    assert_eq!(world.mails.len(), 1, "the advisor is mailed");
    assert!(world.mails[0].0.contains("SKIPPED") && world.mails[0].0.contains("head-moved"));
    assert!(world.mails[0].1.contains("aaaaaaaa -> bbbbbbbb"));
    // The brief that never ran is no longer the round in force.
    assert_eq!(
        world
            .comment_starting("SUPERSEDED: not launched (head-moved)")
            .len(),
        1
    );
    assert!(!world
        .comments
        .iter()
        .any(|(_, c)| is_authoritative_brief(c)));
}

#[test]
fn pr_closed_or_merged_while_waiting_is_skipped() {
    for (state, code) in [("CLOSED", "pr-closed"), ("MERGED", "pr-merged")] {
        let mut world = FakeWorld::new();
        world.prs = [open(H0), not_open(state)].into();
        prior_lane_exits_after_two_polls(&mut world);
        let args = world.args(Some(7), "round two");

        let outcome = relay(&args, &plan_for("codex"), &mut world).unwrap();

        let Outcome::Stopped(stop) = outcome else {
            panic!("{state} must not launch")
        };
        assert_eq!(stop.code(), code);
        assert!(world.host.spawned.borrow().is_empty());
        assert!(world.pending("codex").is_empty());
        assert!(world.comment_starting("Lane relay SKIPPED")[0].contains(state));
        assert_eq!(world.mails.len(), 1);
    }
}

#[test]
fn an_unreadable_pr_before_launch_fails_closed() {
    let mut world = FakeWorld::new();
    world.prs = [open(H0), Err("gh: HTTP 502".into())].into();
    let args = world.args(Some(7), "round two");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    assert!(matches!(
        outcome,
        Outcome::Stopped(Stop::PrUnreadable { pr: 7, .. })
    ));
    assert!(world.host.spawned.borrow().is_empty());
    assert_eq!(world.mails.len(), 1);
}

#[test]
fn a_live_prior_lane_is_waited_for_then_the_rework_fires() {
    let mut world = FakeWorld::new();
    world.prs = [open(H0)].into();
    prior_lane_exits_after_two_polls(&mut world);
    world.launcher_records(|attempt| attempt.started(4242));
    let args = world.args(Some(7), "round two: fix the gate");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    let Outcome::Admitted { mode, brief, .. } = outcome else {
        panic!("expected an admitted lane, got {outcome:?}")
    };
    assert_eq!(
        mode,
        Mode::Rework {
            pr: 7,
            branch: "task-9-branch".into(),
            head: H0.into()
        }
    );
    assert_eq!(world.slept, 120, "two polls while the prior lane ran");
    let spawned = world.host.spawned.borrow();
    assert_eq!(spawned.len(), 1);
    assert_eq!(flag(&spawned[0], "--branch"), Some("task-9-branch"));
    assert!(spawned[0].contains(&"--force-claim".to_string()));
    assert_eq!(flag(&spawned[0], "--brief"), Some(brief.as_str()));
    assert!(
        world.mails.is_empty(),
        "an admitted launch needs no advisor"
    );
    assert_eq!(
        world.comment_starting("AUTHORITATIVE REWORK BRIEF (").len(),
        1
    );
    assert!(world.comment_starting("Lane relay ")[0].contains("branch task-9-branch"));
    assert!(world.pushes >= 1);
}

#[test]
fn the_brief_reaches_each_vendor_through_its_own_mailbox_and_an_explicit_path() {
    for vendor in ["claude", "codex"] {
        let mut world = FakeWorld::new();
        world.launcher_records(|attempt| attempt.started(1));
        let args = world.args(None, "do the work\nwith care");

        let outcome = relay(&args, &plan_for(vendor), &mut world).unwrap();

        let Outcome::Admitted { mode, brief, .. } = outcome else {
            panic!("{vendor}: expected an admitted lane")
        };
        assert_eq!(mode, Mode::Fresh);
        let brief = PathBuf::from(brief);
        assert_eq!(
            brief.parent().unwrap(),
            world.root.join(".aida/agent-briefs").join(vendor)
        );
        assert_eq!(world.pending(vendor), vec![brief.clone()]);
        let spawned = world.host.spawned.borrow();
        assert_eq!(flag(&spawned[0], "--vendor"), Some(vendor));
        assert_eq!(flag(&spawned[0], "--brief"), brief.to_str());
        assert_eq!(
            flag(&spawned[0], "--branch"),
            None,
            "fresh work names no branch"
        );
        // What `aida queue work --brief <path>` reads and hands the agent.
        let explicit = crate::brief::PickupBrief::read_explicit(&brief, SPEC, SPEC).unwrap();
        let briefs =
            crate::brief::pickup_briefs(&world.root, vendor, SPEC, SPEC, Some(&explicit)).unwrap();
        let mut prompt = String::new();
        crate::brief::append_pickup_briefs(&mut prompt, &briefs);
        assert!(prompt.contains("  do the work\n  with care"), "{prompt}");
        assert!(prompt.contains(&format!("agent: {vendor}")));
        // Delivery acknowledges it, so no later lane is handed it again.
        crate::brief::consume_pickup_briefs(&briefs);
        assert!(world.pending(vendor).is_empty());
    }
}

#[test]
fn an_admission_refusal_is_recorded_on_the_spec_and_mailed() {
    let mut world = FakeWorld::new();
    world.launcher_records(|attempt| {
        attempt
            .finish(
                Kind::RefusedBeforeStart,
                "holder-state-unknown",
                "retained seat is dirty; holder state Unknown",
                None,
                None,
                true,
            )
            .unwrap()
    });
    let args = world.args(None, "do the work");

    let outcome = relay(&args, &plan_for("codex"), &mut world).unwrap();

    let Outcome::Stopped(Stop::AdmissionRefused { reason, detail }) = outcome else {
        panic!("expected a refusal, got {outcome:?}")
    };
    assert_eq!(reason, "holder-state-unknown");
    assert!(detail.contains("holder state Unknown"));
    let refused = world.comment_starting("Lane relay REFUSED");
    assert_eq!(refused.len(), 1);
    assert!(
        refused[0].contains("admission-refused") && refused[0].contains("holder-state-unknown")
    );
    assert_eq!(world.mails.len(), 1);
    assert!(world.mails[0].1.contains("retained seat is dirty"));
    // The brief was delivered; it stays the round in force for the retry.
    assert!(world
        .comments
        .iter()
        .any(|(_, c)| is_authoritative_brief(c)));
}

#[test]
fn a_launcher_that_dies_without_an_attempt_is_reported_with_its_log() {
    let mut world = FakeWorld::new();
    let args = world.args(None, "do the work");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    let Outcome::Stopped(Stop::LaunchVanished { log_tail }) = outcome else {
        panic!("expected a vanished launch, got {outcome:?}")
    };
    assert!(log_tail.contains("lane-run: started"));
    assert_eq!(world.mails.len(), 1);
}

#[test]
fn a_lane_still_queued_for_a_host_slot_is_reported_after_the_bound() {
    let mut world = FakeWorld::new();
    // The launcher runs but has recorded nothing yet.
    *world.host.on_spawn.borrow_mut() = Some(Box::new(|_, host| host.running.set(true)));
    let args = world.args(None, "do the work");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    assert_eq!(
        outcome,
        Outcome::Stopped(Stop::AdmissionUnconfirmed { secs: 600 })
    );
    assert_eq!(world.comment_starting("Lane relay UNCONFIRMED").len(), 1);
    assert_eq!(world.mails.len(), 1);
}

#[test]
fn a_host_below_its_memory_floor_is_waited_for_within_the_bound() {
    let mut world = FakeWorld::new();
    *world.host.shortfall.borrow_mut() = Some("host memory headroom is 2.00 GiB".into());
    let args = world.args(None, "do the work");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    let Outcome::Stopped(Stop::HostCapacity { secs, why }) = outcome else {
        panic!("expected a capacity stop, got {outcome:?}")
    };
    assert_eq!((secs, world.slept), (600, 600));
    assert!(why.contains("2.00 GiB"));
    assert!(world.host.spawned.borrow().is_empty());
    assert_eq!(world.mails.len(), 1);

    // Headroom that returns in time lets the launch through.
    let mut world = FakeWorld::new();
    *world.host.shortfall.borrow_mut() = Some("low".into());
    world.on_sleep = Box::new(|_, host| *host.shortfall.borrow_mut() = None);
    world.launcher_records(|attempt| attempt.started(1));
    let args = world.args(None, "do the work");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Admitted { .. }));
}

#[test]
fn a_newer_brief_supersedes_an_old_stored_round_brief() {
    let mut world = FakeWorld::new();
    for vendor in ["claude", "codex"] {
        let dir = world.root.join(".aida/agent-briefs").join(vendor);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{SPEC}-r1-2026-11-19T010101Z.md")),
            format!("---\nspec_id: {SPEC}\nagent: {vendor}\nnote: |-\n  OLD ROUND ONE TASK\n---\n"),
        )
        .unwrap();
        // Another spec's brief is left alone.
        std::fs::write(
            dir.join("TASK-90-2026-11-19T010101Z.md"),
            "---\nspec_id: TASK-90\nnote: |-\n  other spec\n---\n",
        )
        .unwrap();
    }
    assert!(world.pickup_prompt("claude").contains("OLD ROUND ONE TASK"));
    world.comments.push((
        "old".into(),
        "AUTHORITATIVE REWORK BRIEF (advisor, relayed 09:12):\nOLD ROUND ONE TASK".into(),
    ));
    world.launcher_records(|attempt| attempt.started(1));
    let args = world.args(None, "NEW ROUND TWO TASK");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Admitted { .. }));

    // The lane's mailbox pickup now yields the newer brief alone.
    let prompt = world.pickup_prompt("claude");
    assert!(prompt.contains("NEW ROUND TWO TASK"), "{prompt}");
    assert!(!prompt.contains("OLD ROUND ONE TASK"), "{prompt}");
    assert!(
        world.pickup_prompt("codex").is_empty(),
        "the other vendor's stale copy is retired"
    );
    for vendor in ["claude", "codex"] {
        let dir = world.root.join(".aida/agent-briefs").join(vendor);
        assert!(dir
            .join(format!("{SPEC}-r1-2026-11-19T010101Z.md.superseded"))
            .exists());
        assert!(dir.join("TASK-90-2026-11-19T010101Z.md").exists());
    }

    // On the spec, the old round brief is marked and only the new one leads.
    let old = &world.comments[0].1;
    assert!(
        old.starts_with("SUPERSEDED by the FRESH brief relayed"),
        "{old}"
    );
    assert!(old.contains("OLD ROUND ONE TASK"), "the text is kept");
    assert!(!crate::rework_direction::is_brief_comment(old));
    let in_force: Vec<_> = world
        .comments
        .iter()
        .filter(|(_, c)| crate::rework_direction::is_brief_comment(c))
        .collect();
    assert_eq!(in_force.len(), 1);
    assert!(in_force[0].1.contains("NEW ROUND TWO TASK"));
    assert!(!world.comment_starting("Lane relay ")[0].contains("SUPERSEDED"));
    let admitted = world.comment_starting("Lane relay: FRESH lane admitted on lanehost");
    assert_eq!(admitted.len(), 1);
    assert!(admitted[0].contains("1 earlier round brief(s) marked SUPERSEDED"));
}

const OLD_BRIEF: &str = "AUTHORITATIVE R1 BRIEF (advisor, relayed 09:12):\nOLD ROUND ONE TASK";

fn with_old_brief() -> FakeWorld {
    let mut world = FakeWorld::new();
    world.comments.push(("old".into(), OLD_BRIEF.into()));
    world
}

/// The earlier round's brief stays the one in force through every stop, on
/// this machine or on the host: only a started lane retires it.
#[test]
fn earlier_briefs_stay_in_force_until_the_new_lane_starts() {
    let unmarked = |world: &FakeWorld, case: &str| {
        assert_eq!(world.comments[0].1, OLD_BRIEF, "{case}");
        assert!(
            !world
                .comments
                .iter()
                .any(|(_, c)| c.contains("SUPERSEDED by")),
            "{case}"
        );
    };

    // The head moved before the launch.
    let mut world = with_old_brief();
    world.prs = [open(H0), open(H1)].into();
    let args = world.args(Some(7), "round two");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Stopped(Stop::HeadMoved { .. })));
    unmarked(&world, "head-moved");

    // The host stops the launch itself: a lane for the spec started between
    // the relay's probe and its launch step.
    let mut world = with_old_brief();
    let host = world.host.clone();
    world.on_reply = Box::new(move |op, _| {
        if matches!(op, RelayHostCommand::PlaceBrief { .. }) {
            host.running.set(true);
        }
    });
    let args = world.args(None, "round two");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert_eq!(
        outcome,
        Outcome::Stopped(Stop::LaneStillRunning { secs: 0 })
    );
    assert!(world.host.spawned.borrow().is_empty());
    unmarked(&world, "lane-still-running at launch");

    // A later relay took the round on the host.
    let mut world = with_old_brief();
    let file = round_file(&world, SPEC);
    world.on_reply = Box::new(move |op, _| {
        if matches!(op, RelayHostCommand::Begin { .. }) {
            std::fs::write(&file, "7\n").unwrap();
        }
    });
    let args = world.args(None, "round two");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert_eq!(outcome, Outcome::Stopped(Stop::Superseded));
    unmarked(&world, "relay-superseded");

    // The newer relay takes a round but stops before delivering anything.
    let mut world = with_old_brief();
    let file = round_file(&world, SPEC);
    world.on_reply = Box::new(move |op, _| {
        if matches!(op, RelayHostCommand::PlaceBrief { .. }) {
            std::fs::write(&file, "2").unwrap();
        }
    });
    let args = world.args(None, "round two");
    assert_eq!(
        relay(&args, &plan_for("claude"), &mut world).unwrap(),
        Outcome::Stopped(Stop::Superseded)
    );
    assert!(world.pending("claude").is_empty());

    // The launcher's own gate stops it after host admission.
    let mut world = with_old_brief();
    launcher_applies_its_gate(&world, Some((4373, "task-9-branch")));
    let args = world.args(None, "round two");
    let outcome = relay(&args, &plan_for("codex"), &mut world).unwrap();
    assert!(matches!(
        outcome,
        Outcome::Stopped(Stop::GateRefused { .. })
    ));
    unmarked(&world, "fresh-blocked");

    // The host refuses admission, or never confirms it: the new brief stays
    // for the retry, and so does the old one.
    let mut world = with_old_brief();
    world.launcher_records(|attempt| {
        attempt
            .finish(
                Kind::RefusedBeforeStart,
                "holder-state-unknown",
                "dirty",
                None,
                None,
                true,
            )
            .unwrap()
    });
    let args = world.args(None, "round two");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(
        outcome,
        Outcome::Stopped(Stop::AdmissionRefused { .. })
    ));
    unmarked(&world, "admission-refused");
    let mut world = with_old_brief();
    let args = world.args(None, "round two");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(
        outcome,
        Outcome::Stopped(Stop::LaunchVanished { .. })
    ));
    unmarked(&world, "launch-vanished");

    // The lane starts: the old brief is marked, and a brief someone posted
    // after this relay's own is newer direction and is left alone.
    let mut world = with_old_brief();
    world
        .comments
        .push(("note".into(), "an unrelated note".into()));
    world.posted_during_launch = Some("AUTHORITATIVE R3 BRIEF (by hand):\nNEWER STILL".into());
    world.launcher_records(|attempt| attempt.started(1));
    let args = world.args(None, "round two");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Admitted { .. }), "{outcome:?}");
    assert!(world.comments[0]
        .1
        .starts_with("SUPERSEDED by the FRESH brief relayed"));
    assert_eq!(world.comments[1].1, "an unrelated note");
    let in_force: Vec<&str> = world
        .comments
        .iter()
        .filter(|(_, c)| is_authoritative_brief(c))
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(in_force, ["added-1", "by-hand"]);
}

/// The relay finds its own brief by the id the store returned for it, so a
/// store that keeps the text in another form, or cannot be read back, still
/// gets exactly that comment marked, and the brief is posted once.
#[test]
fn the_relays_own_brief_is_found_by_its_comment_id() {
    let briefs_posted = |world: &FakeWorld| {
        world
            .comments
            .iter()
            .filter(|(_, c)| c.contains("round two: fix the gate"))
            .count()
    };

    // Stored text differs from what was sent.
    let mut world = with_old_brief();
    world.store_reflows_text = true;
    world.prs = [open(H0), open(H1)].into();
    let args = world.args(Some(7), "round two: fix the gate\nand its test");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Stopped(Stop::HeadMoved { .. })));
    let own = &world
        .comments
        .iter()
        .find(|(id, _)| id == "added-1")
        .unwrap()
        .1;
    assert!(
        own.starts_with("SUPERSEDED: not launched (head-moved)"),
        "{own}"
    );
    assert!(own.contains("\r\n"), "the stored form is what gets marked");
    assert_eq!(briefs_posted(&world), 1);
    assert_eq!(world.comments[0].1, OLD_BRIEF);

    // The spec cannot be read back after the brief was added: the stop is
    // still recorded against that comment, and nothing is posted twice.
    let mut world = with_old_brief();
    world.comments_unreadable = true;
    world.prs = [open(H0), open(H1)].into();
    let args = world.args(Some(7), "round two: fix the gate");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Stopped(Stop::HeadMoved { .. })));
    let own = &world
        .comments
        .iter()
        .find(|(id, _)| id == "added-1")
        .unwrap()
        .1;
    assert!(
        own.starts_with("SUPERSEDED: not launched (head-moved)"),
        "{own}"
    );
    assert_eq!(briefs_posted(&world), 1);
    assert_eq!(world.mails.len(), 1);

    // The same failure once the lane has started: the launch stands, the
    // unmarked earlier briefs are reported, and nothing is reposted.
    let mut world = with_old_brief();
    world.comments_unreadable = true;
    world.launcher_records(|attempt| attempt.started(1));
    let args = world.args(None, "round two: fix the gate");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Admitted { .. }), "{outcome:?}");
    assert!(world.logs.iter().any(
        |l| l.contains("could not be marked superseded") && l.contains("store read timed out")
    ));
    assert_eq!(briefs_posted(&world), 1);
    assert_eq!(world.comments[0].1, OLD_BRIEF);
    assert_eq!(world.host.spawned.borrow().len(), 1);
}

#[test]
fn a_pr_for_another_spec_is_refused_before_anything_is_posted() {
    const CANONICAL: &str = "TASK-b6b1278890774ef993d2ebc327862470-9";
    let view = |state: &str, title: &str, branch: &str| -> Result<PrView, String> {
        Ok(PrView {
            state: state.into(),
            head: H0.into(),
            branch: branch.into(),
            title: title.into(),
        })
    };
    for (state, title, branch, named) in [
        (
            "OPEN",
            "fix the parser (TASK-90)",
            "task-90-parser",
            "TASK-90",
        ),
        // A finished change for another spec is as much a mistyped --pr.
        (
            "MERGED",
            "fix the parser (TASK-90)",
            "task-90-parser",
            "TASK-90",
        ),
        // A change that names no spec cannot be confirmed as this spec's.
        ("OPEN", "tidy the docs", "docs-tidy", "no spec"),
        // An id that only starts like the spec's is another spec.
        ("OPEN", "fix (TASK-91)", "wip", "TASK-91"),
    ] {
        let mut world = FakeWorld::new();
        world.prs = [view(state, title, branch)].into();
        let args = world.args(Some(7), "round two");
        let error = relay(&args, &plan_for("claude"), &mut world).unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("PR #7") && error.contains(named) && error.contains("check --pr"),
            "{title}: {error}"
        );
        assert!(
            world.comments.is_empty() && world.mails.is_empty(),
            "{title}"
        );
        assert!(
            !round_file(&world, SPEC).exists(),
            "{title}: host not asked"
        );
        assert!(world.host.spawned.borrow().is_empty());
    }
    // The change may name the spec by either id, in its title or its branch.
    for (title, branch) in [
        ("fix the gate (Refs TASK-9)", "wip"),
        ("fix the gate", "task-9-gate"),
        ("fix the gate", "task-b6b1278890774ef993d2ebc327862470-9"),
        ("fix (refs task-9)", "wip"),
    ] {
        let mut world = FakeWorld::new();
        *world.host.aliases.borrow_mut() = vec![CANONICAL.into()];
        world.prs = [view("OPEN", title, branch)].into();
        world.launcher_records(|attempt| attempt.started(1));
        let args = world.args(Some(7), "round two");
        let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
        assert!(
            matches!(outcome, Outcome::Admitted { .. }),
            "{title} / {branch}: {outcome:?}"
        );
    }
}

#[test]
fn a_label_is_never_an_option() {
    for bad in ["-x", "--help", "- r2", "", "r2;rm"] {
        let mut world = FakeWorld::new();
        let mut args = world.args(None, "do the work");
        args.label = Some(bad.into());
        let error = relay(&args, &plan_for("claude"), &mut world).unwrap_err();
        assert!(
            format!("{error:#}").contains("invalid --label"),
            "{bad}: {error:#}"
        );
        assert!(world.comments.is_empty(), "{bad}");
        assert!(!round_file(&world, SPEC).exists(), "{bad}: host not asked");
    }
    for good in ["r2", "R2-ci", "round 3", "_x"] {
        let mut world = FakeWorld::new();
        world.launcher_records(|attempt| attempt.started(1));
        let mut args = world.args(None, "do the work");
        args.label = Some(good.into());
        let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
        assert!(matches!(outcome, Outcome::Admitted { .. }), "{good}");
    }
}

#[test]
fn the_host_pulls_its_store_before_it_resolves_the_spec() {
    let mut world = FakeWorld::new();
    world.launcher_records(|attempt| attempt.started(1));
    let args = world.args(None, "do the work");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Admitted { .. }));
    let calls = world.host.store_calls.borrow().clone();
    assert_eq!(calls[..2], ["pull", "ids"], "{calls:?}");
    assert_eq!(
        calls.iter().filter(|call| **call == "pull").count(),
        1,
        "only the first host step pulls"
    );

    // A pull that fails is reported; the host resolves against what it has.
    let mut world = FakeWorld::new();
    *world.host.pull_error.borrow_mut() = Some("store remote unreachable".into());
    world.launcher_records(|attempt| attempt.started(1));
    let args = world.args(None, "do the work");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Admitted { .. }));
    assert!(world
        .logs
        .iter()
        .any(|l| l.starts_with("warning:") && l.contains("store remote unreachable")));
}

#[test]
fn a_merged_or_closed_last_pr_launches_fresh_work_not_a_rework() {
    for state in ["MERGED", "CLOSED"] {
        let mut world = FakeWorld::new();
        world.prs = [not_open(state)].into();
        world.launcher_records(|attempt| attempt.started(1));
        let args = world.args(Some(4309), "the directory split");

        let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

        let Outcome::Admitted { mode, .. } = outcome else {
            panic!("{state}: expected a fresh launch, got {outcome:?}")
        };
        assert_eq!(mode, Mode::Fresh);
        let spawned = world.host.spawned.borrow();
        assert_eq!(spawned.len(), 1);
        assert_eq!(
            flag(&spawned[0], "--branch"),
            None,
            "the finished PR's branch must not be handed to the lane"
        );
        assert_eq!(world.pr_reads, 1, "no head gate on a finished PR");
        assert_eq!(
            world.comment_starting("AUTHORITATIVE FRESH BRIEF (").len(),
            1
        );
        assert!(world
            .logs
            .iter()
            .any(|l| l.contains(&format!("PR #4309 is {state}")) && l.contains("fresh work")));
    }
}

#[test]
fn vendor_model_and_launcher_come_from_config_and_flags() {
    let mut world = FakeWorld::new();
    std::fs::create_dir_all(world.root.join(".aida")).unwrap();
    std::fs::write(
        world.root.join(".aida/config.toml"),
        r#"
[agents.claude.seats]
implementer = { model = "claude-configured" }
[agents.codex.seats]
implementer = { model = "codex-configured" }

[lane_relay]
vendor = "codex" # the project's usual lane vendor
brief_footer = "Before pushing, run the pre-push gate."
"#,
    )
    .unwrap();
    // The SSH destination and the launcher program are machine settings.
    let home = world.host.home.clone();
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::write(
        home.join(".aida/lane-relay.toml"),
        "host = \"lanehost\"\nhost_dir = \"src/project\"\nlauncher = [\"/opt/lane-run.sh\", \"{spec}\", \"{vendor}\"]\n",
    )
    .unwrap();
    let root = world.root.clone();
    let plan = |args: &RelayArgs| plan(args, &root, Some(&home));
    let mut args = world.args(None, "do the work");

    let from_config = plan(&args).unwrap();
    assert_eq!(from_config.vendor, "codex");
    assert_eq!(from_config.model.as_deref(), Some("codex-configured"));
    assert_eq!(from_config.host.as_deref(), Some("lanehost"));
    assert_eq!(
        remote_dir(&from_config, &world.root).unwrap(),
        "src/project"
    );

    args.vendor = Some("claude".into());
    assert_eq!(
        plan(&args).unwrap().model.as_deref(),
        Some("claude-configured"),
        "each vendor reads its own implementer model"
    );
    args.model = Some("one-off".into());
    let overridden = plan(&args).unwrap();
    assert_eq!(overridden.model.as_deref(), Some("one-off"));

    world.launcher_records(|attempt| attempt.started(1));
    relay(&args, &overridden, &mut world).unwrap();
    let spawned = world.host.spawned.borrow();
    assert_eq!(spawned[0][..3], ["/opt/lane-run.sh", SPEC, "claude"]);
    assert_eq!(flag(&spawned[0], "--model"), Some("one-off"));
    let brief = std::fs::read_to_string(flag(&spawned[0], "--brief").unwrap()).unwrap();
    assert!(brief.contains("  Before pushing, run the pre-push gate."));
    drop(spawned);

    args.model = Some("--dangerous".into());
    assert!(plan(&args).is_err(), "a model is never an option");
    args.model = None;
    args.vendor = Some("not-an-agent".into());
    assert!(plan(&args).is_err(), "only a registered agent runs a lane");

    // The tracked project file cannot choose a host or a program.
    for key in [
        "launcher = [\"/opt/x\"]",
        "host = \"elsewhere\"",
        "vendr = \"codex\"",
    ] {
        std::fs::write(
            world.root.join(".aida/config.toml"),
            format!("[lane_relay]\n{key}\n"),
        )
        .unwrap();
        assert!(load_config(&world.root).is_err(), "{key} is refused");
    }
}

#[test]
fn the_host_refuses_briefs_and_ids_outside_its_mailbox() {
    let world = FakeWorld::new();
    let mut sys = FakeSys(world.host.clone());
    let place = |spec: &str, sys: &mut FakeSys| {
        host_op(
            &world.root,
            &RelayHostCommand::PlaceBrief {
                spec: spec.into(),
                round: 0,
                vendor: "claude".into(),
                label: "FRESH".into(),
                body_b64: base64::engine::general_purpose::STANDARD.encode("body"),
            },
            sys,
        )
    };
    assert!(place("../../etc/x", &mut sys).is_err());
    assert!(place("", &mut sys).is_err());
    assert!(place("--help", &mut sys).is_err());
    place(SPEC, &mut sys).unwrap();
    let outside = world.brief_file("not a mailbox brief");
    let launch = host_op(
        &world.root,
        &RelayHostCommand::Launch {
            spec: SPEC.into(),
            round: 0,
            vendor: "claude".into(),
            branch: None,
            pr: None,
            head: None,
            model: None,
            brief: outside,
        },
        &mut sys,
    );
    assert!(launch.is_err());
    assert!(world.host.spawned.borrow().is_empty());
}

#[test]
fn a_lane_process_is_recognised_by_its_spec() {
    let argv = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
    let ids = ["TASK-node1-9".to_string(), "TASK-9".to_string()];
    for running in [
        "aida queue work TASK-node1-9 --role implementer",
        "aida queue work TASK-9 --role implementer --no-human --vendor claude",
        "/home/u/.local/bin/aida queue work task-9 --branch b",
        // The spec after the flags.
        "aida queue work --role implementer --no-human TASK-9",
        "aida --format json queue work --vendor codex TASK-node1-9 --force-claim",
        // A rework that launches.
        "aida queue rework TASK-9 --work",
        "aida queue rework TASK-9 --for implementer --work --no-human",
        "aida queue rework --work task-9",
        "/home/u/.local/bin/aida queue rework --status in-progress TASK-node1-9 --work",
        // The wrapper, before it reaches `queue work`.
        "/bin/bash /home/u/lane-run.sh TASK-9 codex --branch b",
        "bash scripts/lane-run.sh TASK-9 --role implementer",
        "bash scripts/lane-run.sh --role implementer TASK-9",
    ] {
        assert!(cmdline_runs_lane(&argv(running), &ids), "{running}");
    }
    for other in [
        "aida queue work TASK-90 --role implementer",
        "aida queue work --role implementer TASK-90",
        "aida queue list TASK-9",
        // A rework that only routes the spec starts no lane.
        "aida queue rework TASK-9",
        "aida queue rework TASK-9 --for reviewer",
        "aida queue rework TASK-90 --work",
        "aida queue add TASK-9 --for implementer",
        // The spec before the subcommand names no lane.
        "aida show TASK-9 queue work",
        "aida queue work --brief /briefs/TASK-9-2026.md TASK-90",
        "aida lane relay-host probe TASK-9",
        "aida lane relay TASK-9 --brief b.md",
        "bash scripts/lane-run.sh TASK-91",
        "cat TASK-9 scripts/lane-run.sh",
        "queue",
        "",
    ] {
        assert!(!cmdline_runs_lane(&argv(other), &ids), "{other}");
    }
}

#[test]
fn the_command_parses_its_documented_form() {
    let parsed = <crate::lane_cmd::LaneArgs as clap::Parser>::try_parse_from([
        "lane", "relay", "TASK-9", "--pr", "7", "--brief", "b.md", "--vendor", "codex", "--model",
        "m", "--host", "h", "--label", "R2",
    ])
    .unwrap();
    let crate::cli::LaneCommand::Relay(args) = parsed.command else {
        panic!("expected relay")
    };
    assert_eq!((args.pr, args.vendor.as_deref()), (Some(7), Some("codex")));
    assert_eq!((args.wait_timeout, args.poll), (21_600, 60));
    for bad in [
        vec!["lane", "relay", "TASK-9"],
        vec![
            "lane", "relay", "TASK-9", "--brief", "b.md", "--vendor", "gemini",
        ],
        vec![
            "lane", "relay", "TASK-9", "--brief", "b.md", "--label", "-x",
        ],
        vec!["lane", "relay", "TASK-9", "--brief", "b.md", "--label=-x"],
    ] {
        assert!(<crate::lane_cmd::LaneArgs as clap::Parser>::try_parse_from(bad).is_err());
    }
}

#[test]
fn only_a_relayed_heading_counts_as_a_round_brief() {
    for brief in [
        "AUTHORITATIVE REWORK BRIEF (relayed 2026-11-20 16:00 UTC; codex lane on h):\nbody",
        "AUTHORITATIVE R2 BRIEF (x):",
        "**AUTHORITATIVE FRESH BRIEF (x)**",
    ] {
        assert!(is_authoritative_brief(brief), "{brief}");
        assert!(crate::rework_direction::is_brief_comment(brief), "{brief}");
    }
    for other in [
        "SUPERSEDED by the R2 brief relayed now.\n\nAUTHORITATIVE REWORK BRIEF (x):",
        "Scope = the AUTHORITATIVE REWORK BRIEF in the previous comment.",
        "Authoritative source for briefs is the store",
        "",
    ] {
        assert!(!is_authoritative_brief(other), "{other}");
    }
}

/// The launcher on the host: it records an attempt, is admitted, and then
/// applies the gate it was started under, as a real launcher does.
fn launcher_applies_its_gate(world: &FakeWorld, open_change: Option<(u64, &'static str)>) {
    *world.host.on_spawn.borrow_mut() = Some(Box::new(move |root, mut host| {
        let attempt = Attempt::begin_with_id(root, SPEC, None, host.owner().attempt).unwrap();
        let gate = host.gate();
        let gated = gate.route(open_change).and_then(|()| gate.admit(&mut host));
        match gated {
            Ok(()) => attempt.started(1),
            Err(refusal) => attempt
                .finish_result(&Err(crate::lane_admission::refused_before_spawn(
                    refusal.into(),
                )))
                .unwrap(),
        }
    }));
}

fn launcher_sees(world: &FakeWorld, head: &str) {
    *world.host.launcher_prs.borrow_mut() = [open(head).unwrap()].into();
}

#[test]
fn a_head_that_moves_after_host_admission_stops_the_launch() {
    let mut world = FakeWorld::new();
    // The relay's own two reads see the recorded head; the push lands while
    // the launcher waits for a start slot.
    world.prs = [open(H0)].into();
    launcher_sees(&world, H1);
    launcher_applies_its_gate(&world, Some((7, "task-9-branch")));
    let args = world.args(Some(7), "round two: fix the gate");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    let Outcome::Stopped(Stop::GateRefused { code, detail }) = outcome else {
        panic!("a head moved after admission must stop the launch, got {outcome:?}")
    };
    assert_eq!(code, "head-moved");
    assert!(detail.contains("aaaaaaaa -> bbbbbbbb"), "{detail}");
    assert_eq!(
        world.host.gate(),
        LaunchGate::Rework {
            pr: 7,
            head: H0.into()
        },
        "the launch carried the recorded PR and head"
    );
    // The launcher recorded a refusal before any agent started.
    let record =
        crate::lane_result::admission_for(&world.root, world.host.owner().attempt, &|id| {
            id == SPEC
        })
        .unwrap();
    assert!(!record.started);
    assert_eq!(record.reason_code.as_deref(), Some("head-moved"));
    // The stale brief is withdrawn from the host and marked on the spec.
    assert!(world.pending("claude").is_empty());
    assert_eq!(
        world
            .comment_starting("SUPERSEDED: not launched (head-moved)")
            .len(),
        1
    );
    let skipped = world.comment_starting("Lane relay SKIPPED");
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].contains("head-moved"));
    assert_eq!(world.mails.len(), 1);

    // A head that held through admission starts the lane.
    let mut world = FakeWorld::new();
    world.prs = [open(H0)].into();
    launcher_sees(&world, H0);
    launcher_applies_its_gate(&world, Some((7, "task-9-branch")));
    let args = world.args(Some(7), "round two");
    let outcome = relay(&args, &plan_for("codex"), &mut world).unwrap();
    assert!(matches!(outcome, Outcome::Admitted { .. }), "{outcome:?}");
}

#[test]
fn a_change_closed_or_unreadable_after_admission_stops_the_launch() {
    let gate = LaunchGate::Rework {
        pr: 7,
        head: H0.into(),
    };
    for (state, code) in [("MERGED", "pr-merged"), ("CLOSED", "pr-closed")] {
        let host = HostState::default();
        *host.launcher_prs.borrow_mut() = [not_open(state).unwrap()].into();
        assert_eq!(gate.admit(&mut &host).unwrap_err().code, code);
    }
    struct Down;
    impl HeadSource for Down {
        fn pr(&mut self, _pr: u64) -> Result<PrView> {
            anyhow::bail!("HTTP 502")
        }
        fn pause(&mut self, _duration: Duration) {}
    }
    assert_eq!(gate.admit(&mut Down).unwrap_err().code, "pr-unreadable");
    // Fresh work has no head to check.
    assert!(LaunchGate::Fresh.admit(&mut Down).is_ok());
}

#[test]
fn fresh_work_never_continues_an_open_change() {
    // The launcher's routing: an open change for the spec refuses fresh work.
    let refusal = LaunchGate::Fresh
        .route(Some((4373, "task-9-branch")))
        .unwrap_err();
    assert_eq!(refusal.code, "fresh-blocked");
    assert!(refusal.detail.contains("--pr 4373"), "{}", refusal.detail);
    assert!(LaunchGate::Fresh.route(None).is_ok());
    let rework = LaunchGate::Rework {
        pr: 7,
        head: H0.into(),
    };
    assert!(rework.route(Some((7, "b"))).is_ok());
    assert_eq!(
        rework.route(Some((8, "b"))).unwrap_err().code,
        "rework-mismatch"
    );

    // The gate survives its trip through the launcher's environment, and a
    // value that is set but unreadable refuses.
    for gate in [LaunchGate::Fresh, rework] {
        assert_eq!(LaunchGate::parse(&gate.encode()).unwrap(), gate);
    }
    for bad in [
        "",
        "rework",
        "rework:7",
        "rework:x:abcdef12",
        "rework:7:zz",
        "fresh:1",
    ] {
        assert_eq!(
            LaunchGate::parse(bad).unwrap_err().code,
            "relay-gate-invalid",
            "{bad}"
        );
    }

    // End to end: the relay starts fresh work under the fresh gate, and the
    // launcher's refusal comes back typed, with the brief withdrawn.
    let mut world = FakeWorld::new();
    launcher_applies_its_gate(&world, Some((4373, "task-9-branch")));
    let args = world.args(None, "start over");
    let outcome = relay(&args, &plan_for("codex"), &mut world).unwrap();
    assert_eq!(world.host.gate(), LaunchGate::Fresh);
    let Outcome::Stopped(Stop::GateRefused { code, .. }) = outcome else {
        panic!("expected the fresh gate to stop the launch, got {outcome:?}")
    };
    assert_eq!(code, "fresh-blocked");
    assert!(world.pending("codex").is_empty());
    assert!(!world
        .comments
        .iter()
        .any(|(_, c)| is_authoritative_brief(c)));
    assert_eq!(world.mails.len(), 1);
}

fn round_file(world: &FakeWorld, id: &str) -> PathBuf {
    world.root.join(format!(".aida/lane-relay/{id}.round"))
}

#[test]
fn a_relay_superseded_by_a_later_one_launches_nothing() {
    let mut world = FakeWorld::new();
    world.host.running.set(true);
    // While this relay waits for the prior lane, a second relay for the
    // spec takes the next round on the host.
    let file = round_file(&world, SPEC);
    world.on_sleep = Box::new(move |_, host| {
        std::fs::write(&file, "2\n").unwrap();
        host.running.set(false);
    });
    let args = world.args(None, "the older brief");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    assert_eq!(outcome, Outcome::Stopped(Stop::Superseded));
    assert!(world.host.spawned.borrow().is_empty());
    assert!(world.pending("claude").is_empty());
    assert_eq!(
        world
            .comment_starting("SUPERSEDED: not launched (relay-superseded)")
            .len(),
        1
    );
    assert_eq!(world.mails.len(), 1);

    // Each host step refuses a superseded round on its own, under the lock.
    let world = FakeWorld::new();
    let mut sys = FakeSys(world.host.clone());
    let begin = |sys: &mut FakeSys| {
        host_op(
            &world.root,
            &RelayHostCommand::Begin { spec: SPEC.into() },
            sys,
        )
        .unwrap()["round"]
            .as_u64()
            .unwrap()
    };
    let place = |round: u64, sys: &mut FakeSys| {
        host_op(
            &world.root,
            &RelayHostCommand::PlaceBrief {
                spec: SPEC.into(),
                round,
                vendor: "claude".into(),
                label: "R2".into(),
                body_b64: base64::engine::general_purpose::STANDARD.encode("body"),
            },
            sys,
        )
        .unwrap()
    };
    let launch = |round: u64, brief: &str, sys: &mut FakeSys| {
        host_op(
            &world.root,
            &RelayHostCommand::Launch {
                spec: SPEC.into(),
                round,
                vendor: "claude".into(),
                branch: None,
                pr: None,
                head: None,
                model: None,
                brief: brief.into(),
            },
            sys,
        )
        .unwrap()
    };
    let first = begin(&mut sys);
    let brief = place(first, &mut sys)["path"].as_str().unwrap().to_string();
    let second = begin(&mut sys);
    assert_eq!((first, second), (1, 2));
    assert_eq!(place(first, &mut sys)["stop"], "relay-superseded");
    assert_eq!(launch(first, &brief, &mut sys)["stop"], "relay-superseded");
    assert!(world.host.spawned.borrow().is_empty());
    let old = PathBuf::from(&brief);
    let retract = RelayHostCommand::Retract {
        spec: SPEC.into(),
        round: first,
        vendor: "claude".into(),
        brief: None,
    };
    host_op(&world.root, &retract, &mut sys).unwrap();
    assert!(!old.exists());
    // The newest round is refused too while a lane for the spec runs.
    let brief = place(second, &mut sys)["path"]
        .as_str()
        .unwrap()
        .to_string();
    host_op(&world.root, &retract, &mut sys).unwrap();
    assert!(Path::new(&brief).exists());
    world.host.running.set(true);
    assert_eq!(
        launch(second, &brief, &mut sys)["stop"],
        "lane-still-running"
    );
    world.host.running.set(false);
    assert!(launch(second, &brief, &mut sys)["since"].is_string());
    assert_eq!(world.host.spawned.borrow().len(), 1);
}

#[test]
fn the_spec_is_matched_by_any_of_its_ids_on_the_host() {
    const CANONICAL: &str = "TASK-b6b1278890774ef993d2ebc327862470-9";
    let mut world = FakeWorld::new();
    *world.host.aliases.borrow_mut() = vec![CANONICAL.into()];
    // An older round's brief was stored under the canonical id.
    let dir = world.root.join(".aida/agent-briefs/claude");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{CANONICAL}-2026-11-19T010101Z.md")),
        format!("---\nspec_id: {CANONICAL}\nnote: |-\n  OLD ROUND\n---\n"),
    )
    .unwrap();
    // The launcher records its attempt under the canonical id, although the
    // relay was given the display id.
    *world.host.on_spawn.borrow_mut() = Some(Box::new(|root, host| {
        Attempt::begin_with_id(root, CANONICAL, None, host.owner().attempt)
            .unwrap()
            .started(1);
    }));
    let args = world.args(None, "do the work");

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    assert!(matches!(outcome, Outcome::Admitted { .. }), "{outcome:?}");
    assert!(dir
        .join(format!("{CANONICAL}-2026-11-19T010101Z.md.superseded"))
        .exists());
    // One fence per spec, whichever id a relay is given.
    assert!(round_file(&world, &CANONICAL.to_ascii_uppercase()).exists());
    assert!(!round_file(&world, SPEC).exists());

    // A receipt for another spec is never this spec's admission.
    let ids = [CANONICAL.to_string(), SPEC.to_string()];
    assert!(names_spec(&ids, "task-9") && names_spec(&ids, &CANONICAL.to_ascii_uppercase()));
    assert!(!names_spec(&ids, "TASK-90") && !names_spec(&ids, "queue-head"));
}

#[test]
fn the_pickup_size_limit_applies_to_the_framed_brief() {
    let limit = crate::brief::PICKUP_BRIEF_LIMIT;
    // Short lines: under the limit raw, over it once every line is indented.
    let body = "x\n".repeat(limit / 2 - 100);
    assert!(body.len() < limit);
    let mut world = FakeWorld::new();
    let args = world.args(None, &body);

    let error = relay(&args, &plan_for("claude"), &mut world).unwrap_err();

    assert!(format!("{error:#}").contains("once framed"), "{error:#}");
    assert!(world.comments.is_empty(), "nothing was posted");
    assert!(!round_file(&world, SPEC).exists(), "the host was not asked");

    // The host applies the same rule to what it is handed.
    let mut sys = FakeSys(world.host.clone());
    let place = |body: &str, sys: &mut FakeSys| {
        host_op(
            &world.root,
            &RelayHostCommand::PlaceBrief {
                spec: SPEC.into(),
                round: 0,
                vendor: "claude".into(),
                label: "FRESH".into(),
                body_b64: base64::engine::general_purpose::STANDARD.encode(body),
            },
            sys,
        )
    };
    assert!(place(&body, &mut sys).is_err());
    assert!(world.pending("claude").is_empty());
    // A brief that fits framed is stored at a size the pickup accepts.
    let fits = "x\n".repeat(limit / 4 - 100);
    let stored = place(&fits, &mut sys).unwrap();
    let stored = PathBuf::from(stored["path"].as_str().unwrap());
    assert!(std::fs::metadata(&stored).unwrap().len() as usize <= limit);
    crate::brief::PickupBrief::read_explicit(&stored, SPEC, SPEC).unwrap();
}

#[test]
fn operational_failures_are_typed_stops_that_withdraw_the_brief() {
    // The change cannot be read at invocation: reported, nothing posted.
    let mut world = FakeWorld::new();
    world.prs = [Err("gh: HTTP 502".into())].into();
    let args = world.args(Some(7), "round two");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(
        outcome,
        Outcome::Stopped(Stop::PrUnreadable { pr: 7, .. })
    ));
    assert_eq!(world.mails.len(), 1);
    assert_eq!(world.comment_starting("Lane relay SKIPPED").len(), 1);
    assert_eq!(world.comments.len(), 1, "no brief was posted");

    // The host cannot store the brief.
    let mut world = FakeWorld::new();
    std::fs::create_dir_all(world.root.join(".aida")).unwrap();
    std::fs::write(world.root.join(".aida/agent-briefs"), "not a directory").unwrap();
    let args = world.args(None, "do the work");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    let Outcome::Stopped(stop) = outcome else {
        panic!("expected a stop, got {outcome:?}")
    };
    assert_eq!(stop.code(), "brief-delivery-failed");
    assert!(world.host.spawned.borrow().is_empty());
    assert_eq!(world.comment_starting("Lane relay FAILED").len(), 1);
    assert_eq!(
        world
            .comment_starting("SUPERSEDED: not launched (brief-delivery-failed)")
            .len(),
        1
    );
    assert_eq!(world.mails.len(), 1);

    // The host cannot start the launcher: the delivered brief is withdrawn.
    let mut world = FakeWorld::new();
    std::fs::create_dir_all(world.host.home.join(".aida")).unwrap();
    std::fs::write(
        world.host.home.join(".aida/lane-relay.toml"),
        "launcher = [\"\"]\n",
    )
    .unwrap();
    let args = world.args(None, "do the work");
    let outcome = relay(&args, &plan_for("codex"), &mut world).unwrap();
    let Outcome::Stopped(stop) = outcome else {
        panic!("expected a stop, got {outcome:?}")
    };
    assert_eq!(stop.code(), "launch-failed");
    assert!(world.pending("codex").is_empty(), "the brief is withdrawn");
    assert!(!world
        .comments
        .iter()
        .any(|(_, c)| is_authoritative_brief(c)));
    assert_eq!(world.mails.len(), 1);

    // A host reply that lacks what the relay needs.
    for (broken, key) in [("place", "path"), ("launch", "log")] {
        let mut world = FakeWorld::new();
        world.launcher_records(|attempt| attempt.started(1));
        world.on_reply = Box::new(move |op, reply| {
            let hit = match op {
                RelayHostCommand::PlaceBrief { .. } => broken == "place",
                RelayHostCommand::Launch { .. } => broken == "launch",
                _ => false,
            };
            if hit {
                reply.as_object_mut().unwrap().remove(key);
            }
        });
        let args = world.args(None, "do the work");
        let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
        let Outcome::Stopped(stop) = outcome else {
            panic!("{broken}: expected a stop, got {outcome:?}")
        };
        assert_eq!(stop.code(), "host-reply-malformed", "{broken}");
        assert_eq!(world.mails.len(), 1, "{broken}");
        assert!(
            !world
                .comments
                .iter()
                .any(|(_, c)| is_authoritative_brief(c)),
            "{broken}: no brief is left in force"
        );
    }

    // A probe reply without its fields is never read as an idle host.
    let mut world = FakeWorld::new();
    world.on_reply = Box::new(|op, reply| {
        if matches!(op, RelayHostCommand::Probe { .. }) {
            reply.as_object_mut().unwrap().remove("current");
        }
    });
    let args = world.args(None, "do the work");
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    assert!(matches!(
        outcome,
        Outcome::Stopped(Stop::HostUnreachable { secs: 600, .. })
    ));
    assert!(world.host.spawned.borrow().is_empty());
}

#[test]
fn waits_are_bounded_in_wall_time_including_host_operations() {
    let mut world = FakeWorld::new();
    world.host.running.set(true);
    world.host_cost = 50;
    let mut args = world.args(None, "do the work");
    (args.wait_timeout, args.poll) = (100, 60);

    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();

    // Round 50s, probe 50s, a 0s remainder: no sleep can fit, and a 60s poll
    // is never slept in full past the deadline.
    let Outcome::Stopped(Stop::LaneStillRunning { secs }) = outcome else {
        panic!("expected a bounded wait, got {outcome:?}")
    };
    assert!((50..=150).contains(&secs), "{secs}");
    assert!(world.slept <= 50, "slept {}s of a 100s bound", world.slept);

    // The same for the admission watch.
    let mut world = FakeWorld::new();
    *world.host.on_spawn.borrow_mut() = Some(Box::new(|_, host| host.running.set(true)));
    world.host_cost = 10;
    let mut args = world.args(None, "do the work");
    (args.admission_timeout, args.poll) = (90, 60);
    let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
    let Outcome::Stopped(Stop::AdmissionUnconfirmed { secs }) = outcome else {
        panic!("expected an unconfirmed admission, got {outcome:?}")
    };
    assert!((90..=110).contains(&secs), "{secs}");
    // 10s check, 60s sleep, 10s check: 10s are left, so the last sleep is
    // 10s, not a full poll.
    assert_eq!(world.slept, 60 + 10);
}

// ---------------------------------------------------------------------------
// The production boundary: the real `Machine` and `Live`, with stub programs.

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::test_exec::write_executable(path, body);
}

#[test]
fn the_real_machine_starts_the_launcher_and_sees_it_running() {
    // A spec id no other test or lane on this machine uses.
    let spec = format!("TASK-{}", 900_000_000 + std::process::id() as u64);
    let canonical = format!("TASK-relaytest-{}", std::process::id());
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    // The host's `aida`, as far as the relay uses it: the store pull and
    // the spec's ids. It records each call.
    let aida = root.join("bin/aida");
    let aida_calls = root.join("aida-calls");
    script(
        &aida,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$PWD $*\" >> '{}'\nif [ \"$1\" = db ]; then exit 0; fi\nprintf '%s\\n' '{{\"spec_id\":\"{canonical}\",\"agreed_id\":\"{spec}\",\"display_id\":\"{spec}\"}}'\n",
            aida_calls.display()
        ),
    );
    // The launcher: a wrapper named as the real one, which records how it
    // was started and stays alive like a lane waiting for a start slot.
    let launcher = root.join("scripts/lane-run.sh");
    let record = root.join("launcher-record");
    script(
        &launcher,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" \"$AIDA_LANE_RELAY_GATE\" \"$PWD\" \"$@\" > '{0}.tmp'\nmv '{0}.tmp' '{0}'\nsleep 60\n",
            record.display()
        ),
    );
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::write(
        home.join(".aida/lane-relay.toml"),
        format!(
            "launcher = [\"{}\", \"{{spec}}\", \"--vendor\", \"{{vendor}}\"]\n",
            launcher.display()
        ),
    )
    .unwrap();
    let mut machine = Machine {
        aida,
        home: Some(home),
    };
    let mut op = |op: RelayHostCommand| host_op(&root, &op, &mut machine).unwrap();

    let round = op(RelayHostCommand::Begin { spec: spec.clone() })["round"]
        .as_u64()
        .unwrap();
    assert!(root
        .join(format!(
            ".aida/lane-relay/{}.round",
            canonical.to_ascii_uppercase()
        ))
        .exists());
    // Taking the round pulled the host's store first, then resolved the spec.
    assert_eq!(
        std::fs::read_to_string(&aida_calls).unwrap(),
        format!(
            "{root} db sync --pull\n{root} show {spec} --format json\n",
            root = root.display()
        )
    );
    let probe = op(RelayHostCommand::Probe {
        spec: spec.clone(),
        round,
    });
    assert!(
        probe["current"] == true && probe["running"] == false,
        "{probe}"
    );
    let placed = op(RelayHostCommand::PlaceBrief {
        spec: spec.clone(),
        round,
        vendor: "codex".into(),
        label: "R2".into(),
        body_b64: base64::engine::general_purpose::STANDARD.encode("fix the gate"),
    });
    let brief = PathBuf::from(placed["path"].as_str().unwrap());
    let launched = op(RelayHostCommand::Launch {
        spec: spec.clone(),
        round,
        vendor: "codex".into(),
        branch: Some("task-branch".into()),
        pr: Some(7),
        head: Some(H0.into()),
        model: Some("some-model".into()),
        brief: brief.clone(),
    });

    // The detached launcher got the argv, directory and gate for real.
    let started = std::time::Instant::now();
    while !record.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "launcher never ran"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let recorded = std::fs::read_to_string(&record).unwrap();
    let recorded: Vec<&str> = recorded.lines().collect();
    let pid = recorded[0];
    assert_eq!(recorded[1], format!("rework:7:{H0}"));
    assert_eq!(Path::new(recorded[2]), root);
    assert_eq!(
        recorded[3..],
        [
            spec.as_str(),
            "--vendor",
            "codex",
            "--branch",
            "task-branch",
            "--force-claim",
            "--model",
            "some-model",
            "--brief",
            brief.to_str().unwrap(),
        ]
    );
    // The process table shows it as this spec's lane, by either id, and a
    // second launch for the spec is refused while it runs.
    let probe = op(RelayHostCommand::Probe {
        spec: canonical.clone(),
        round,
    });
    assert_eq!(probe["running"], true);
    let again = op(RelayHostCommand::Launch {
        spec: spec.clone(),
        round,
        vendor: "codex".into(),
        branch: None,
        pr: None,
        head: None,
        model: None,
        brief: brief.clone(),
    });
    assert_eq!(again["stop"], "lane-still-running");
    let admission = |machine_op: &mut dyn FnMut(RelayHostCommand) -> serde_json::Value| {
        machine_op(RelayHostCommand::Admission {
            spec: spec.clone(),
            launched_at: launched["since"].as_str().unwrap().into(),
            log: launched["log"].as_str().unwrap().into(),
        })
    };
    assert_eq!(admission(&mut op)["state"], "pending");

    // Once the launcher and its session are gone, so is the lane.
    let killed = std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .status()
        .unwrap();
    assert!(killed.success());
    let started = std::time::Instant::now();
    loop {
        let probe = op(RelayHostCommand::Probe {
            spec: spec.clone(),
            round,
        });
        if probe["running"] == false {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "lane never exited"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(admission(&mut op)["state"], "vanished");
    let retired = op(RelayHostCommand::Retract {
        spec,
        round,
        vendor: "codex".into(),
        brief: Some(brief.clone()),
    });
    assert!(retired["retired"][0].is_string() && !brief.exists());
}

#[test]
fn the_live_relay_reaches_the_lane_host_through_ssh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    let checkout = root.join("host checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let calls = root.join("aida-calls");
    let comments = root.join("comments");
    let ssh_calls = root.join("ssh-calls");
    let own = root.join("own-comment");
    let edits = root.join("comment-edits");
    // `aida` for both sides: it records each call (directory, then its
    // arguments, tab-separated) and answers as the real one would.
    let aida = home.join(".local/bin/aida");
    script(
        &aida,
        &format!(
            r#"#!/bin/sh
tab=$(printf '\t')
line=$PWD
for arg in "$@"; do line="$line$tab$arg"; done
printf '%s\n' "$line" >> '{calls}'
case "$1 $2 $3" in
  "show "*)
    printf '{{"spec_id":"TASK-9","comments":[{{"id":"old-1","content":"AUTHORITATIVE R1 BRIEF (x):\\nOLD ROUND"}},{{"content":"a comment without an id"}}'
    if [ -f '{own}' ]; then printf ',{{"id":"%s","content":"as the store holds it"}}' "$(cat '{own}')"; fi
    printf ',{{"id":"later-1","content":"AUTHORITATIVE R9 BRIEF (y):\\nLATER"}}]}}\n' ;;
  "comment add "*)
    cat "$5" >> '{comments}'; printf '\n==\n' >> '{comments}'
    id="new-$(grep -c '' '{calls}')"
    [ -f '{own}' ] || printf '%s' "$id" > '{own}'
    echo "Comment added to $3"; echo "comment-id: $id" ;;
  "comment edit "*) printf '%s\n' "$4" "$6" >> '{edits}'; cat "$8" >> '{edits}' ;;
  "lane relay-host begin") echo 'noise before the result'; echo '{{"round":3}}' ;;
  "lane relay-host probe") echo '{{"current":true,"running":false,"shortfall":null}}' ;;
  "lane relay-host place-brief") echo '{{"path":"/host/briefs/b.md","superseded":[]}}' ;;
  "lane relay-host launch") echo '{{"since":"2026-11-20T00:00:00+00:00","log":"/host/launch.log"}}' ;;
  "lane relay-host admission") echo '{{"state":"started"}}' ;;
esac
"#,
            calls = calls.display(),
            comments = comments.display(),
            own = own.display(),
            edits = edits.display(),
        ),
    );
    // `ssh`, found on PATH: it records its arguments and runs the remote
    // command through a shell, as the lane host's login shell would.
    script(
        &root.join("stubs/ssh"),
        &format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; last=$arg; done\nexec sh -c \"$last\"\n",
            ssh_calls.display()
        ),
    );
    let path = format!(
        "{}:{}",
        root.join("stubs").display(),
        aida_core::exec_context::Environment::capture()
            .get("PATH")
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let mut live = Live {
        root: root.clone(),
        host: Some(("lanehost".into(), checkout.to_string_lossy().into_owned())),
        aida,
        ssh_env: vec![
            ("PATH".into(), path),
            ("HOME".into(), home.to_string_lossy().into_owned()),
        ],
        started: std::time::Instant::now(),
    };
    let brief = root.join("brief.md");
    let body = "fix the gate; don't `rm -rf $HOME`\n  \"quoted\" & 'single'\n";
    std::fs::write(&brief, body).unwrap();
    let args = RelayArgs {
        spec: SPEC.into(),
        pr: None,
        brief,
        vendor: None,
        model: None,
        host: None,
        host_dir: None,
        label: Some("r 2".into()),
        wait_timeout: 5,
        admission_timeout: 5,
        poll: 1,
    };
    let mut plan = plan_for("codex");
    plan.model = Some("some-model".into());

    let outcome = relay(&args, &plan, &mut live).unwrap();

    assert_eq!(
        outcome,
        Outcome::Admitted {
            mode: Mode::Fresh,
            brief: "/host/briefs/b.md".into(),
            log: "/host/launch.log".into(),
        }
    );
    // Every lane-host step arrived in the host checkout as the command the
    // relay built, intact through SSH's single string and the remote shell.
    let calls = std::fs::read_to_string(&calls).unwrap();
    let host_ops: Vec<RelayHostCommand> = calls
        .lines()
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|call| call[1..3] == ["lane", "relay-host"])
        .map(|call| {
            assert_eq!(Path::new(call[0]), checkout, "run in the host checkout");
            let parsed = <crate::lane_cmd::LaneArgs as clap::Parser>::try_parse_from(
                std::iter::once("lane").chain(call[2..].iter().copied()),
            )
            .unwrap();
            let crate::cli::LaneCommand::RelayHost(op) = parsed.command else {
                panic!("expected a relay-host command")
            };
            op
        })
        .collect();
    assert_eq!(
        host_ops,
        [
            RelayHostCommand::Begin { spec: SPEC.into() },
            RelayHostCommand::Probe {
                spec: SPEC.into(),
                round: 3
            },
            RelayHostCommand::PlaceBrief {
                spec: SPEC.into(),
                round: 3,
                vendor: "codex".into(),
                label: "R 2".into(),
                body_b64: base64::engine::general_purpose::STANDARD.encode(body),
            },
            RelayHostCommand::Launch {
                spec: SPEC.into(),
                round: 3,
                vendor: "codex".into(),
                branch: None,
                pr: None,
                head: None,
                model: Some("some-model".into()),
                brief: "/host/briefs/b.md".into(),
            },
            RelayHostCommand::Admission {
                spec: SPEC.into(),
                launched_at: "2026-11-20T00:00:00+00:00".into(),
                log: "/host/launch.log".into(),
            },
        ]
    );
    let ssh = std::fs::read_to_string(&ssh_calls).unwrap();
    assert!(ssh.starts_with("-T\n-o\nBatchMode=yes\n-o\nConnectTimeout=10\n--\nlanehost\n"));
    // The spec side ran locally, in this checkout, through `aida`.
    assert!(calls.lines().any(|line| line.starts_with(&format!(
        "{}\tcomment\tadd\t{SPEC}\t--body-file\t",
        root.display()
    ))));
    let posted = std::fs::read_to_string(&comments).unwrap();
    assert!(
        posted.starts_with("AUTHORITATIVE R 2 BRIEF (relayed "),
        "{posted}"
    );
    assert!(posted.contains("; relay lanehost#3)") && posted.contains(body.trim_end()));
    // The brief was added with a request for its id, and that id is what the
    // relay then looked for when it read the spec back through `aida show`.
    let adds: Vec<&str> = calls
        .lines()
        .filter(|line| line.contains("\tcomment\tadd\t"))
        .collect();
    assert!(adds.len() >= 2 && adds.iter().all(|add| add.ends_with("\t--print-id")));
    assert!(std::fs::read_to_string(&own).unwrap().starts_with("new-"));
    assert!(calls
        .lines()
        .any(|line| line == format!("{}\tshow\t{SPEC}\t--format\tjson", root.display())));
    // Once the lane had started, the one round brief posted before this
    // relay's own was edited through `aida comment edit`, by its id; the
    // brief posted after it was not.
    let edit_calls: Vec<&str> = calls
        .lines()
        .filter(|line| line.contains("\tcomment\tedit\t"))
        .collect();
    assert_eq!(edit_calls.len(), 1, "{edit_calls:?}");
    assert!(edit_calls[0].starts_with(&format!(
        "{}\tcomment\tedit\t--req-id\t{SPEC}\t--comment-id\told-1\t--body-file\t",
        root.display()
    )));
    let edited = std::fs::read_to_string(&edits).unwrap();
    assert!(
        edited.starts_with(&format!(
            "{SPEC}\nold-1\nSUPERSEDED by the R 2 brief relayed "
        )),
        "{edited}"
    );
    assert!(
        edited.ends_with("AUTHORITATIVE R1 BRIEF (x):\nOLD ROUND"),
        "{edited}"
    );
    assert!(posted.contains(
        "Lane relay: R 2 lane admitted on lanehost; 1 earlier round brief(s) marked SUPERSEDED."
    ));
    assert!(
        std::fs::read_to_string(root.join(".aida/lane-relay/relay.log"))
            .unwrap()
            .contains("admitted: TASK-9 (R 2) is running on lanehost")
    );
}

#[test]
fn a_new_begin_invalidates_the_owner_sent_to_the_launcher() {
    let mut world = FakeWorld::new();
    world.prs.push_back(open(H0));
    world.launcher_records(|a| a.started(1));
    let args = world.args(Some(7), "rework");
    relay(&args, &plan_for("claude"), &mut world).unwrap();
    let owner = world.host.owner();
    drop(owner.lock_current(&world.root).unwrap());
    host_op(
        &world.root,
        &RelayHostCommand::Begin { spec: SPEC.into() },
        &mut FakeSys(world.host.clone()),
    )
    .unwrap();
    assert!(
        matches!(owner.lock_current(&world.root), Err(e) if gate_refusal_code(&e) == Some("relay-superseded"))
    );
}

#[test]
fn another_pickup_receipt_cannot_supply_admission() {
    for relay_starts in [true, false] {
        let mut world = FakeWorld::new();
        world.launcher_records(move |attempt| {
            if relay_starts {
                attempt.started(1);
            } else {
                attempt
                    .finish(
                        Kind::RefusedBeforeStart,
                        "relay-refused",
                        "fixture refusal",
                        None,
                        None,
                        true,
                    )
                    .unwrap();
            }
        });
        let root = world.root.clone();
        world.on_reply = Box::new(move |op, _| {
            if matches!(op, RelayHostCommand::Launch { .. }) {
                let other = Attempt::begin(&root, SPEC, None).unwrap();
                if relay_starts {
                    other
                        .finish(
                            Kind::RefusedBeforeStart,
                            "other-refused",
                            "fixture refusal",
                            None,
                            None,
                            true,
                        )
                        .unwrap();
                } else {
                    other.started(2);
                }
            }
        });
        let args = world.args(None, "this round");
        let outcome = relay(&args, &plan_for("claude"), &mut world).unwrap();
        if relay_starts {
            assert!(matches!(outcome, Outcome::Admitted { .. }));
        } else {
            assert!(
                matches!(outcome, Outcome::Stopped(Stop::AdmissionRefused {reason, ..}) if reason == "relay-refused")
            );
        }
    }
}

#[test]
fn symlinked_mailbox_refuses_placement() {
    let world = FakeWorld::new();
    let dir = world.root.join(".aida/agent-briefs");
    std::fs::create_dir_all(&dir).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.join("claude")).unwrap();
    let result = place_brief(
        &world.root,
        &[SPEC.into()],
        SPEC,
        1,
        "claude",
        "R1",
        "body",
        Utc::now(),
    );
    assert!(result.is_err());
    assert_eq!(outside.path().read_dir().unwrap().count(), 0);
}

#[test]
fn queue_head_pickup_with_a_resolved_live_lease_blocks_the_relay() {
    let root = tempfile::tempdir().unwrap();
    let id = "TASK-91919191";
    let ids = vec![id.to_owned()];
    let machine = Machine::new("aida".into());
    assert!(!cmdline_runs_lane(
        &[
            "aida".into(),
            "queue".into(),
            "work".into(),
            "--headless".into()
        ],
        &ids
    ));
    assert!(!machine.lane_running(root.path(), &ids));
    let dir = crate::leases::leases_dir(root.path());
    std::fs::create_dir_all(&dir).unwrap();
    let lease: crate::leases::SessionLease = serde_json::from_value(serde_json::json!({
        "id":"queue-head-worker", "scope":id, "slug":"queue-head", "owner":"operator",
        "worktree_path":root.path(), "branch":"work", "started_at":Utc::now(),
        "hostname":"host", "active_pid":std::process::id()
    }))
    .unwrap();
    std::fs::write(
        dir.join("queue-head-worker.toml"),
        toml::to_string(&lease).unwrap(),
    )
    .unwrap();
    assert!(machine.lane_running(root.path(), &ids));
}
