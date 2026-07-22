// TASK-1175: the burndown ready set is ordered by QUEUE-INSERTION time — the
// `added_at` stamp on each spec's queue entry — not by how its spec id happens
// to spell. The pure sort itself is covered in `burndown::tests`; these tests
// pin the two impure halves that feed it:
//
//   1. `all_queued_added_at` — the filesystem join that reads every user's
//      queue YAML and yields requirement UUID → earliest `added_at`;
//   2. `resolved_burndown_ready_order` — the resolution ladder, with the
//      `AIDA_BURNDOWN_ORDER` env tier (what `--order` exports) on top.
//
// trace:TASK-1175 | ai:claude

use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::burndown::ReadyOrder;
use crate::{all_queued_added_at, resolved_burndown_ready_order};

/// `AIDA_BURNDOWN_ORDER` is process-global, so the env-touching tests in this
/// file serialize on one lock and restore the prior value on the way out.
fn env_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

struct OrderEnv(Option<String>);

impl OrderEnv {
    fn set(value: Option<&str>) -> Self {
        let prior = std::env::var("AIDA_BURNDOWN_ORDER").ok();
        match value {
            Some(v) => std::env::set_var("AIDA_BURNDOWN_ORDER", v),
            None => std::env::remove_var("AIDA_BURNDOWN_ORDER"),
        }
        Self(prior)
    }
}

impl Drop for OrderEnv {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var("AIDA_BURNDOWN_ORDER", v),
            None => std::env::remove_var("AIDA_BURNDOWN_ORDER"),
        }
    }
}

fn write_queue(root: &std::path::Path, user: &str, body: &str) {
    let dir = root.join(".aida-store/registry/queues");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{user}.yaml")), body).unwrap();
}

fn entry(user: &str, id: &str, position: u32, added_at: &str) -> String {
    format!(
        "- user_id: {user}\n  requirement_id: {id}\n  position: {position}\n  added_by: {user}\n  added_at: {added_at}\n"
    )
}

const A: &str = "019ed372-b246-7a33-b116-fad59c031498";
const B: &str = "019ed47e-d744-7502-bbce-8153858edd50";
const C: &str = "019f7c66-7892-7e91-80be-67e96bfdb4b7";

#[test]
fn added_at_is_read_per_requirement_from_every_users_queue_file() {
    let tmp = tempfile::tempdir().unwrap();
    write_queue(
        tmp.path(),
        "joe",
        &format!(
            "{}{}",
            entry("joe", A, 1000, "2026-06-17T14:46:48.658820117Z"),
            entry("joe", B, 2000, "2026-06-18T09:00:00Z"),
        ),
    );
    write_queue(
        tmp.path(),
        "codex-advisor-1",
        &entry("codex-advisor-1", C, 1000, "2026-07-19T22:02:13.789623429Z"),
    );

    let map = all_queued_added_at(tmp.path());
    assert_eq!(map.len(), 3, "every user's queue file contributes");
    let a = map[&A.parse().unwrap()];
    let b = map[&B.parse().unwrap()];
    let c = map[&C.parse().unwrap()];
    assert!(a < b && b < c, "stamps must order oldest-queued first");
}

#[test]
fn a_spec_in_two_queues_carries_the_earlier_stamp() {
    // Re-routing work to a second role must not silently push it to the back of
    // the wave: the earliest entry is the one that dates the work.
    let tmp = tempfile::tempdir().unwrap();
    write_queue(
        tmp.path(),
        "joe",
        &entry("joe", A, 1000, "2026-07-01T00:00:00Z"),
    );
    write_queue(
        tmp.path(),
        "reviewer",
        &entry("reviewer", A, 1000, "2026-06-01T00:00:00Z"),
    );

    let map = all_queued_added_at(tmp.path());
    assert_eq!(map.len(), 1);
    assert_eq!(
        map[&A.parse().unwrap()],
        "2026-06-01T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
    );
}

#[test]
fn a_missing_or_unparseable_queue_dir_yields_an_empty_map_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(all_queued_added_at(tmp.path()).is_empty(), "no queue dir");

    // A corrupt file is skipped rather than poisoning the whole read.
    write_queue(tmp.path(), "broken", "this: is: not: a queue [\n");
    write_queue(
        tmp.path(),
        "joe",
        &entry("joe", A, 1000, "2026-06-17T00:00:00Z"),
    );
    let map = all_queued_added_at(tmp.path());
    assert_eq!(map.len(), 1);
    assert!(map.contains_key(&A.parse().unwrap()));
}

#[test]
fn ready_order_ladder_puts_the_env_override_above_project_config() {
    let _lock = env_guard();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[burndown]\norder = \"queue\"\n",
    )
    .unwrap();

    // Config alone.
    {
        let _env = OrderEnv::set(None);
        assert_eq!(resolved_burndown_ready_order(tmp.path()), ReadyOrder::Queue);
    }
    // `--order priority` (exported as the env var) wins over the config.
    {
        let _env = OrderEnv::set(Some("priority"));
        assert_eq!(
            resolved_burndown_ready_order(tmp.path()),
            ReadyOrder::Priority
        );
    }
    // A typo in the env tier falls THROUGH to the config rather than failing.
    {
        let _env = OrderEnv::set(Some("prioritise"));
        assert_eq!(resolved_burndown_ready_order(tmp.path()), ReadyOrder::Queue);
    }
}

#[test]
fn ready_order_defaults_to_priority_with_no_config_and_no_env() {
    let _lock = env_guard();
    let _env = OrderEnv::set(None);
    let tmp = tempfile::tempdir().unwrap();
    // No project config, and the machine-global one is rooted at $HOME — point
    // it at the empty temp dir so the read can only reach the built-in default.
    let prior_home = std::env::var("AIDA_HOME").ok();
    std::env::set_var("AIDA_HOME", tmp.path());
    let resolved = resolved_burndown_ready_order(tmp.path());
    match prior_home {
        Some(h) => std::env::set_var("AIDA_HOME", h),
        None => std::env::remove_var("AIDA_HOME"),
    }
    assert_eq!(resolved, ReadyOrder::Priority);
}
