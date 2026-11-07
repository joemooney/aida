//! `aida own`: ownership records (STORY-2325).
// trace:STORY-2325 | ai:claude

#[path = "ownership_cmd_args.rs"]
pub(crate) mod ownership_cmd_args;
pub(crate) use ownership_cmd_args::*;

/// Ownership records: one per piece of work (a spec, a pull request, a
/// review, a merge train), naming who holds it and the evidence that the
/// holder is still running.
// trace:STORY-2325 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "own")]
pub(crate) struct OwnArgs {
    #[clap(subcommand)]
    pub inner: OwnCommand,
}

// `aida own` reads and writes `.aida/ownership/` on the main worktree and
// self-loads the requirement store; no shared storage handle.
// trace:STORY-2325 | ai:claude
fn run_own(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <OwnArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let start = crate::find_project_root()?;
    let main = crate::seat_occupancy::store::main_worktree_root(&start)?;
    match &args.inner {
        OwnCommand::Show { key, compare, json } => {
            crate::ownership::compare::run(&main, key.as_deref(), *compare, *json)
        }
        OwnCommand::Release { key } => {
            crate::gate::seat_authority::require_direct_human()?;
            anyhow::ensure!(
                !crate::session::process_contained(),
                "release requires the host shell"
            );
            anyhow::ensure!(
                crate::ownership::dual::WriterFacts::here().pid_ns
                    == Some(aida_core::process_probe::INITIAL_PID_NAMESPACE_INODE),
                "release requires the host pid namespace"
            );
            let store = crate::ownership::store::OwnershipStore::for_project(&main)?
                .ok_or_else(|| anyhow::anyhow!("ownership store unavailable"))?;
            operator_release(&store, key)?;
            println!("Released {key}; leases and trees retained for guarded session end.");
            Ok(())
        }
        OwnCommand::Migrate { execute, json, .. } => {
            crate::ownership::migrate::run(&main, *execute, *json)
        }
    }
}

// trace:STORY-2325 | ai:claude
pub(crate) static OWN: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "own",
    help_after: "orchestrator",
    command: <OwnArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(run_own),
};

/// Exact keys avoid a substring accidentally releasing unrelated work. The
/// caller enforces the existing direct-human TTY gate before opening the store.
// trace:STORY-2325 | ai:codex
fn operator_release(
    store: &crate::ownership::store::OwnershipStore,
    key: &str,
) -> anyhow::Result<()> {
    use crate::ownership::{
        store::{file_name, Expect, Step},
        Phase, WorkKey,
    };
    let (records, errors) = store.list();
    anyhow::ensure!(
        errors.is_empty(),
        "ownership evidence unreadable: {errors:?}"
    );
    let record = records
        .iter()
        .find(|r| {
            let (kind, name) = file_name(&r.key);
            format!("{kind}/{name}") == key
        })
        .ok_or_else(|| {
            anyhow::anyhow!("ownership key {key} missing; use kind/file-stem from .aida/ownership")
        })?;
    let mut affected = vec![record];
    // Releasing a train also releases its rider reservations, even when a
    // failed begin-train left the train without a holder.
    if let WorkKey::Train { train } = &record.key {
        affected.extend(records.iter().filter(|r| {
            matches!(&r.phase, Phase::Training { train: t } if t == train) && r.key != record.key
        }));
    }
    let steps = affected
        .into_iter()
        .map(|r| Step {
            key: r.key.clone(),
            op: "operator-release".into(),
            expect: Expect::Generation(r.generation),
            transition: Box::new(|cur| {
                let mut next = cur?.clone();
                next.holder = None;
                next.phase = Phase::Closed {
                    why: "operator release at a human terminal".into(),
                };
                Some((
                    next,
                    Some("explicit human override; tree retirement requires session end".into()),
                ))
            }),
        })
        .collect();
    store.update_all(steps)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ownership::{
        dual,
        store::{Expect, OwnershipStore},
        Ownership, Phase, WorkKey,
    };

    // trace:STORY-2325 | ai:codex
    #[test]
    fn operator_release_recovers_orphan_riders_and_journals_override() {
        let dir = tempfile::tempdir().unwrap();
        let store = OwnershipStore::at(dir.path().join("ownership"));
        let key = WorkKey::Change {
            change: dual::change(9),
        };
        store
            .update(&key, "seed", Expect::Absent, |_| {
                Some((
                    Ownership {
                        key: key.clone(),
                        generation: 0,
                        holder: None,
                        phase: Phase::Training {
                            train: dual::change(12),
                        },
                    },
                    None,
                ))
            })
            .unwrap();
        assert!(operator_release(&store, "change/origin-9-extra").is_err());
        operator_release(&store, "change/origin-9").unwrap();
        assert!(matches!(
            store.load(&key).unwrap().unwrap().phase,
            Phase::Closed { .. }
        ));
        assert_eq!(store.journal().last().unwrap().op, "operator-release");
    }

    // trace:STORY-2325 | ai:codex
    #[test]
    fn operator_release_train_releases_riders_but_preserves_other_trains() {
        let dir = tempfile::tempdir().unwrap();
        let store = OwnershipStore::at(dir.path().join("ownership"));
        for key in [
            WorkKey::Train {
                train: dual::change(12),
            },
            WorkKey::Change {
                change: dual::change(9),
            },
            WorkKey::Change {
                change: dual::change(10),
            },
        ] {
            let train = if matches!(&key, WorkKey::Change { change } if change.n == 10) {
                13
            } else {
                12
            };
            store
                .update(&key, "seed", Expect::Absent, |_| {
                    Some((
                        Ownership {
                            key: key.clone(),
                            generation: 0,
                            holder: None,
                            phase: Phase::Training {
                                train: dual::change(train),
                            },
                        },
                        None,
                    ))
                })
                .unwrap();
        }
        operator_release(&store, "train/origin-12").unwrap();
        let (records, errors) = store.list();
        assert!(errors.is_empty());
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r.phase, Phase::Closed { .. }))
                .count(),
            2
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r.phase, Phase::Training { .. }))
                .count(),
            1
        );
    }
    // trace:STORY-2325 | ai:codex
    #[test]
    fn prc_release_command_refuses_agents_and_non_tty_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(p)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        for agent in [None, Some("codex")] {
            let _ambient = crate::test_env::AmbientGuard::hermetic_and(
                p,
                None,
                &[("AIDA_AGENT_NAME", agent), ("AIDA_AGENT_TYPE", None)],
            );
            let matches = <OwnArgs as clap::CommandFactory>::command()
                .try_get_matches_from(["own", "release", "train/origin-12"])
                .unwrap();
            let ctx = crate::command_registry::DispatchCtx {
                globals: None,
                store: None,
            };
            let error = run_own(&matches, &ctx).unwrap_err();
            let text = format!("{error:#}");
            assert!(
                text.contains(if agent.is_some() {
                    "agent sessions"
                } else {
                    "interactive TTY"
                }),
                "{text}"
            );
            assert!(!p.join(".aida/ownership").exists());
        }
    }
}
