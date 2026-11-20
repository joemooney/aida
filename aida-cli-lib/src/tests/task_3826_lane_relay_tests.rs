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
    /// The lane host operator's home directory.
    home: PathBuf,
    /// What the started launcher does on the host.
    on_spawn: RefCell<Option<Box<dyn Fn(&Path, &HostState)>>>,
}

struct FakeSys(Rc<HostState>);

impl HostSys for FakeSys {
    fn lane_running(&self, _spec: &str) -> bool {
        self.0.running.get()
    }
    fn shortfall(&self) -> Option<String> {
        self.0.shortfall.borrow().clone()
    }
    fn spawn(&mut self, argv: &[String], cwd: &Path, log: &Path) -> Result<()> {
        std::fs::write(log, "lane-run: started\n")?;
        self.0.spawned.borrow_mut().push(argv.to_vec());
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
    mails: Vec<(String, String)>,
    logs: Vec<String>,
    pushes: usize,
    slept: u64,
    /// Runs after each sleep with the total seconds slept.
    on_sleep: Box<dyn FnMut(u64, &HostState)>,
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
            mails: Vec::new(),
            logs: Vec::new(),
            pushes: 0,
            slept: 0,
            on_sleep: Box::new(|_, _| {}),
        }
    }

    /// The started launcher records an attempt and `then` says how it went.
    fn launcher_records(&self, then: impl Fn(&Attempt) + 'static) {
        *self.host.on_spawn.borrow_mut() = Some(Box::new(move |root, _| {
            then(&Attempt::begin(root, SPEC, None).unwrap());
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
        Ok(self.comments.clone())
    }
    fn comment_add(&mut self, _spec: &str, body: &str) -> Result<()> {
        self.comments
            .push((format!("c{}", self.comments.len()), body.to_string()));
        Ok(())
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
        host_op(&self.root, &parsed, &mut FakeSys(self.host.clone()))
    }
    fn sleep(&mut self, duration: Duration) {
        self.slept += duration.as_secs();
        (self.on_sleep)(self.slept, &self.host);
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
    })
}

fn not_open(state: &str) -> Result<PrView, String> {
    Ok(PrView {
        state: state.into(),
        head: H0.into(),
        branch: "task-9-branch".into(),
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
    assert!(world.comment_starting("Lane relay ")[0]
        .contains("1 earlier round brief(s) marked SUPERSEDED"));
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
        "launcher = [\"/tmp/x\"]",
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
            vendor: "claude".into(),
            branch: None,
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
    for running in [
        "aida queue work TASK-9 --role implementer --no-human --vendor claude",
        "/home/u/.local/bin/aida queue work task-9 --branch b",
        "/bin/bash /home/u/lane-run.sh TASK-9 codex --branch b",
        "bash scripts/lane-run.sh TASK-9 --role implementer",
    ] {
        assert!(cmdline_runs_lane(&argv(running), "TASK-9"), "{running}");
    }
    for other in [
        "aida queue work TASK-90 --role implementer",
        "aida queue list TASK-9",
        "aida lane relay-host probe TASK-9",
        "aida lane relay TASK-9 --brief b.md",
        "bash scripts/lane-run.sh TASK-91",
        "",
    ] {
        assert!(!cmdline_runs_lane(&argv(other), "TASK-9"), "{other}");
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
