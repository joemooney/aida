//! Train service using the same binding and supervision as review-watch.
// trace:STORY-2691 | ai:codex
use super::service::HostAuthority;
const TRAIN: bool = true;
include!("advisor_loop_service.rs");

// trace:BUG-2665 | ai:codex
fn update_starvation_alert(
    alert: &mut Option<Notice>,
    carried: &mut Vec<Notice>,
    reason: Option<String>,
) {
    if let Some(reason) = reason {
        if alert.is_none() {
            let mut message = notice::message(
                format!("advisor-loop-{NAME}-starvation-{}", uuid::Uuid::new_v4()),
                "advisor",
                format!("{NAME}: train starvation"),
                format!("{reason}. Inspect aida advisor loops train-status and .aida/{NAME}/service.log."),
            );
            message.from = NAME.into();
            *alert = Some(Notice {
                message,
                sent: false,
            });
        }
    } else if let Some(old) = alert.take().filter(|notice| !notice.sent) {
        carried.push(old);
    }
}

pub(super) fn record_starvation(
    root: &Path,
    contention: bool,
    pass: crate::pr_train::TrainPass,
) -> Result<()> {
    let config = match std::fs::read_to_string(root.join(".aida/config.toml")) {
        Ok(text) => toml::from_str::<toml::Value>(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(Default::default())
        }
        Err(error) => return Err(error.into()),
    };
    let section = config
        .get("auto_train")
        .map(|value| value.as_table().context("auto_train must be a TOML table"))
        .transpose()?;
    let setting = |key: &str, default| {
        section
            .and_then(|table| table.get(key))
            .map(|value| {
                value
                    .as_integer()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .with_context(|| format!("auto_train.{key} must be a positive integer"))
            })
            .unwrap_or(Ok(default))
    };
    let skip_limit = setting("starvation_contention_skips", 3)?;
    let wait_limit = setting("starvation_wait_minutes", 30)?;

    let dir = directory(root, true)?.context("service directory missing")?;
    let _lock = dir.lock("failures.lock")?;
    let mut state = failure_state(root)?;
    let starvation = state.starvation.get_or_insert_with(Starvation::default);
    starvation.consecutive_admission_skips = if contention {
        starvation.consecutive_admission_skips.saturating_add(1)
    } else {
        0
    };
    update_starvation_alert(
        &mut starvation.contention_alert,
        &mut state.carried,
        (starvation.consecutive_admission_skips >= skip_limit).then(|| {
            format!(
                "{} consecutive passes skipped on admission contention",
                starvation.consecutive_admission_skips
            )
        }),
    );

    if let Some(snapshot) = pass.eligible {
        let now = snapshot.at;
        starvation.last_eligibility_observation = Some(now);
        let mut previous = std::mem::take(&mut starvation.waiting);
        starvation.waiting = snapshot
            .members
            .into_iter()
            .map(|(pr, head)| {
                let since = previous
                    .remove(&pr)
                    .filter(|old| old.head == head && old.since <= now)
                    .map_or(now, |old| old.since);
                (pr, EligibleWait { head, since })
            })
            .collect();

        let limit = chrono::Duration::minutes(i64::from(wait_limit));
        let waiting_alert = starvation
            .waiting
            .iter()
            .find(|(_, wait)| now - wait.since > limit)
            .map(|(pr, _)| {
                format!(
                    "Eligible PR #{pr} has waited longer than {wait_limit} minutes for train admission"
                )
            });
        update_starvation_alert(
            &mut starvation.wait_alert,
            &mut state.carried,
            waiting_alert,
        );
        for pr in pass.admitted {
            starvation.waiting.remove(&pr);
        }
    }
    dir.write("failures.json", &serde_json::to_vec_pretty(&state)?)?;
    deliver_failure_notices(root, &dir, &mut state)
}

#[cfg(test)]
mod starvation_tests {
    use super::*;

    // trace:BUG-2665 | ai:codex
    #[test]
    fn bug2665_malformed_starvation_settings_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        directory(root, true).unwrap();
        for (toml, diagnostic) in [
            ("auto_train = 3", "auto_train must be a TOML table"),
            (
                "[auto_train]\nstarvation_contention_skips = 0",
                "auto_train.starvation_contention_skips must be a positive integer",
            ),
            (
                "[auto_train]\nstarvation_wait_minutes = '30'",
                "auto_train.starvation_wait_minutes must be a positive integer",
            ),
        ] {
            std::fs::write(root.join(".aida/config.toml"), toml).unwrap();
            let error = record_starvation(root, true, Default::default()).unwrap_err();
            assert_eq!(error.to_string(), diagnostic);
        }
        assert!(failure_state(root).unwrap().starvation.is_none());
    }

    // trace:BUG-2665 | ai:codex
    #[test]
    fn bug2665_contention_threshold_is_quiet_then_notifies_once_per_streak() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dir = directory(root, true).unwrap().unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[auto_train]\nstarvation_contention_skips = 2 # test threshold\n",
        )
        .unwrap();
        let skip = |contention| record_starvation(root, contention, Default::default()).unwrap();
        skip(true);
        let saved = failure_state(root).unwrap();
        assert_eq!(saved.count, 0);
        let saved = saved.starvation.unwrap();
        assert_eq!(saved.consecutive_admission_skips, 1);
        assert!(saved.contention_alert.is_none());
        skip(true);
        let first = failure_state(root)
            .unwrap()
            .starvation
            .unwrap()
            .contention_alert
            .unwrap();
        assert!(first.sent);
        assert!(root
            .join(".aida/mailbox")
            .join(format!("{}.json", first.message.id))
            .is_file());
        skip(true);
        let saved = failure_state(root).unwrap().starvation.unwrap();
        assert_eq!(saved.consecutive_admission_skips, 3);
        assert_eq!(saved.contention_alert.unwrap().message.id, first.message.id);

        skip(false);
        record_failure(root, None).unwrap();
        let saved = failure_state(root).unwrap().starvation.unwrap();
        assert_eq!(saved.consecutive_admission_skips, 0);
        assert!(saved.contention_alert.is_none());
        skip(true);
        skip(true);
        let saved = failure_state(root).unwrap().starvation.unwrap();
        assert_ne!(saved.contention_alert.unwrap().message.id, first.message.id);
        assert!(dir.read("failures.json").unwrap().is_some());
    }

    // trace:BUG-2665 | ai:codex
    #[test]
    fn bug2665_wait_tracks_pr_and_head_from_complete_observations() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        directory(root, true).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[auto_train]\nstarvation_wait_minutes = 5\n",
        )
        .unwrap();
        let start = Utc::now();
        let observe = |seconds, rows: &[(u64, &str)], admitted: Vec<u64>| {
            record_starvation(
                root,
                false,
                crate::pr_train::TrainPass {
                    eligible: Some(crate::pr_train::EligibleSnapshot {
                        at: start + chrono::Duration::seconds(seconds),
                        members: rows
                            .iter()
                            .map(|(pr, head)| (*pr, (*head).into()))
                            .collect(),
                    }),
                    admitted,
                    ..Default::default()
                },
            )
            .unwrap();
            failure_state(root).unwrap().starvation.unwrap()
        };
        assert_eq!(observe(0, &[(12, "a"), (13, "b")], vec![]).waiting.len(), 2);
        assert!(observe(300, &[(12, "a"), (13, "c")], vec![])
            .wait_alert
            .is_none());
        record_starvation(root, false, Default::default()).unwrap();
        let saved = observe(301, &[(12, "a"), (13, "c")], vec![12]);
        assert!(!saved.waiting.contains_key(&12));
        assert_eq!(
            saved.waiting[&13].since,
            start + chrono::Duration::seconds(300)
        );
        let notice = saved.wait_alert.unwrap();
        assert!(notice.sent);
        assert!(notice.message.body.contains("PR #12"));
        assert!(root
            .join(".aida/mailbox")
            .join(format!("{}.json", notice.message.id))
            .is_file());
        let saved = observe(302, &[(12, "a"), (13, "c")], vec![]);
        assert!(saved.wait_alert.is_none());
        assert_eq!(
            saved.waiting[&12].since,
            start + chrono::Duration::seconds(302)
        );
        assert!(observe(303, &[], vec![]).waiting.is_empty());
    }
}
