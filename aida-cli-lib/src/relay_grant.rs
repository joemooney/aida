//! The grant a rotating relay relaunches under (BUG-2585).
//!
//! `aida agent new --rotate` mints each generation's seat grant from the
//! grant its own shell exports. When that grant expires the next relaunch
//! is refused, and a relay that had already ended its session lost the seat.
//! This module gives the relay and its seat one view of that grant's life:
//! the banner/statusline/doctor text, the mail sent ahead of expiry, the
//! hold that keeps the working session while the grant is expired, and the
//! adoption of a grant a human renewed for this relay.
//!
//! Authority never comes from here. A renewed grant is used only after the
//! ordinary grant verdict (lifetime, subject, roster, process binding)
//! accepts it for this process; the scan only finds its selector.
// trace:BUG-2585 | ai:claude

use crate::gate::seat_authority::{self as authority, GrantCheck, GrantRejection, SeatGrant};
use aida_core::exec_context::Environment;
use chrono::{DateTime, Duration, Utc};
use std::path::Path;

/// The pid a renewed grant must be bound to (`aida role enter --bind-pid`)
/// for the relay to use it, as the relay tells its seat.
pub(crate) const BIND_PID_ENV: &str = "AIDA_ROTATION_RENEW_PID";

/// Warn this long before the grant expires.
pub(crate) const WARN_WITHIN: Duration = Duration::hours(2);

fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// PURE: the operator command that renews `parent` so a relay bound to
/// `bind_pid` can use it. It keeps every seat the grant already delegates.
/// Without a pid the renewed grant belongs to the operator's shell only.
pub(crate) fn renewal_command(
    parent: &SeatGrant,
    requested_seat: &str,
    bind_pid: Option<u32>,
) -> String {
    let mut command =
        authority::widen_delegation_command(&parent.seat, &parent.delegable_seats, requested_seat);
    if let Some(pid) = bind_pid {
        command.push_str(&format!(" --bind-pid {pid}"));
    }
    format!("eval \"$({command})\"")
}

/// PURE: direct grants in `dir` that renew `old` for a relay launching
/// `requested_seat`: same principal and seat, issued by `aida role enter`
/// after `old`, live at `now`, delegating the seat. Newest expiry first.
/// Metadata only; the caller still runs the grant verdict on each.
pub(crate) fn successors_in(
    dir: &Path,
    old: &SeatGrant,
    requested_seat: &str,
    now: DateTime<Utc>,
) -> Vec<SeatGrant> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let canonical = |seat: &str| crate::gate::dispatch_deps::canonical_role_name(seat);
    let mut out: Vec<SeatGrant> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
        })
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|bytes| serde_json::from_slice::<SeatGrant>(&bytes).ok())
        .filter(|g| {
            g.id != old.id
                && g.parent_grant_id.is_none()
                && g.origin.is_none()
                && g.principal == old.principal
                && canonical(&g.seat) == canonical(&old.seat)
                && g.revoked_at.is_none()
                && g.expires_at > now
                && g.issued_at >= old.issued_at
                && g.delegable_seats.iter().any(|s| s == requested_seat)
        })
        .collect();
    out.sort_by(|a, b| b.expires_at.cmp(&a.expires_at));
    out
}

/// The pid `--bind-pid` should name so this process can present the renewed
/// grant: the process the current grant is bound to that this process runs
/// under (the shell that entered the role), else this process's parent.
pub(crate) fn bind_pid(parent: Option<&SeatGrant>) -> Option<u32> {
    let presenter = authority::presenter_tokens();
    parent
        .and_then(|grant| {
            authority::matched_binding(grant, &presenter)
                .iter()
                .filter(|token| token.starts_with("proc:"))
                .filter_map(|token| authority::binding_process(token).map(|(pid, _, _)| pid))
                .find(|pid| *pid != std::process::id())
        })
        .or_else(|| Some(std::os::unix::process::parent_id()).filter(|pid| *pid > 1))
}

/// Why the relay does not rotate now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hold {
    /// The expired grant's id.
    pub id: String,
    /// The seat of the grant to renew.
    pub seat: String,
    pub expired_at: DateTime<Utc>,
    /// The operator command that renews it.
    pub command: String,
    /// Whether the relay picks the renewed grant up by itself.
    pub resumes: bool,
}

impl Hold {
    /// The lines the relay prints. `kept` says what happens to the session:
    /// `true` while it still runs, `false` when it had already exited.
    pub(crate) fn message(&self, handle: &str, kept: bool) -> String {
        let at = stamp(self.expired_at);
        let state = if kept {
            format!("not ending {handle}: its session keeps running")
        } else {
            format!("not relaunching {handle} yet: its handle and lease are kept")
        };
        let after = if self.resumes {
            format!("The relay continues with {handle} once the renewed grant is valid.")
        } else {
            format!(
                "This relay holds a delegated grant, which is not renewed in place: after renewing, start {handle} again from its launcher."
            )
        };
        format!(
            "rotation: {state}. The {} seat grant {} this relay launches under expired at {at}. Renew it from any terminal:\n  {}\n{after}",
            self.seat,
            short(&self.id),
            self.command
        )
    }

    /// The mail body for the advisor and product seats.
    fn mail(&self, handle: &str) -> String {
        format!(
            "The {} seat grant {} expired at {}. Relay {handle} keeps its current session and cannot rotate until a human renews the grant from any terminal: {}",
            self.seat,
            short(&self.id),
            stamp(self.expired_at),
            self.command
        )
    }
}

fn expired_at(reason: &GrantRejection) -> Option<DateTime<Utc>> {
    match reason {
        GrantRejection::Expired { at } => Some(*at),
        GrantRejection::Parent(inner) => match inner.as_ref() {
            GrantRejection::Expired { at } => Some(*at),
            _ => None,
        },
        _ => None,
    }
}

/// The top of `grant`'s delegation chain (itself when it is direct).
fn chain_root(grant: SeatGrant) -> SeatGrant {
    let mut current = grant;
    for _ in 0..64 {
        let Some(parent) = current
            .parent_grant_id
            .as_deref()
            .and_then(|id| authority::read_grant(id).ok())
        else {
            break;
        };
        current = parent;
    }
    current
}

/// The renewed grant this process may launch `requested_seat` under, by the
/// full grant verdict. Only a relay holding a direct grant has one: a
/// delegated grant may have been narrowed, and a renewed root would widen it.
fn renewed(
    project_root: &Path,
    old: &SeatGrant,
    requested_seat: &str,
    now: DateTime<Utc>,
) -> Option<SeatGrant> {
    if old.parent_grant_id.is_some() {
        return None;
    }
    let dir = authority::grant_dir().ok()?;
    successors_in(&dir, old, requested_seat, now)
        .into_iter()
        .find_map(
            |next| match authority::grant_check_id(project_root, &next.id) {
                GrantCheck::Valid(grant) => Some(grant),
                _ => None,
            },
        )
}

/// Whether this relay must hold instead of rotating: `Some` only when the
/// grant it exports is refused because it (or an ancestor) expired and no
/// renewed grant is usable. Any other state is left to the launch itself.
/// With `adopt`, a usable renewed grant becomes this process's grant.
pub(crate) fn hold(project_root: &Path, requested_seat: &str, adopt: bool) -> Option<Hold> {
    let now = Utc::now();
    let GrantCheck::Rejected { id, reason, .. } = authority::grant_check_at(project_root, now)
    else {
        return None;
    };
    let expired_at = expired_at(&reason)?;
    let own = authority::read_grant(&id).ok()?;
    if let Some(next) = renewed(project_root, &own, requested_seat, now) {
        if adopt {
            // The selector only; the verdict above is the authority.
            crate::agent_launch::adopt_launcher_grant(&next);
        }
        return None;
    }
    let resumes = own.parent_grant_id.is_none();
    let pid = resumes.then(|| bind_pid(Some(&own))).flatten();
    let root = chain_root(own);
    Some(Hold {
        command: renewal_command(&root, requested_seat, pid),
        id: root.id,
        seat: root.seat,
        expired_at,
        resumes,
    })
}

/// Wait at the relaunch gate until the relay may mint the next generation's
/// grant. Returns `false` when `stop` was raised (Ctrl-C) first. `tick`
/// waits one second.
pub(crate) fn await_parent(
    project_root: &Path,
    handle: &str,
    requested_seat: &str,
    stop: &std::sync::atomic::AtomicBool,
    tick: &mut dyn FnMut(),
    out: &mut dyn std::io::Write,
) -> bool {
    use std::sync::atomic::Ordering;
    let mut announced = false;
    loop {
        let Some(hold) = hold(project_root, requested_seat, true) else {
            if announced {
                let _ = writeln!(
                    out,
                    "rotation: the grant was renewed; continuing with {handle}"
                );
            }
            return true;
        };
        if !announced {
            let _ = writeln!(out, "{}", hold.message(handle, false));
            announced = true;
        }
        // Every held poll: the notifier's markers make it once per grant,
        // and a delivery that failed is retried. trace:BUG-2585 | ai:claude
        notify_hold(project_root, handle, &hold);
        for _ in 0..5 {
            if stop.load(Ordering::SeqCst) {
                return false;
            }
            tick();
        }
    }
}

/// The relay's whole relaunch gate: hold while the grant is expired, run
/// `countdown`, then check the grant again, so one that expired during the
/// countdown holds instead of failing the next child grant's issuance.
/// Returns `false` when `stop` was raised first.
// trace:BUG-2585 | ai:claude
pub(crate) fn relaunch_gate(
    project_root: &Path,
    handle: &str,
    requested_seat: &str,
    stop: &std::sync::atomic::AtomicBool,
    tick: &mut dyn FnMut(),
    out: &mut dyn std::io::Write,
    countdown: &mut dyn FnMut(
        &std::sync::atomic::AtomicBool,
        &mut dyn FnMut(),
        &mut dyn std::io::Write,
    ) -> bool,
) -> bool {
    loop {
        if !await_parent(project_root, handle, requested_seat, stop, tick, out) {
            return false;
        }
        if !countdown(stop, tick, out) {
            return false;
        }
        if hold(project_root, requested_seat, true).is_none() {
            return true;
        }
    }
}

/// A grant's life as a relay or its seat shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParentLife {
    pub id: String,
    pub seat: String,
    pub expires_at: DateTime<Utc>,
    pub command: String,
}

impl ParentLife {
    /// PURE: `PARENT GRANT advisor · 5h40m left · expires …`, with the
    /// renewal command once inside [`WARN_WITHIN`].
    pub(crate) fn line(&self, now: DateTime<Utc>) -> String {
        let left = self.expires_at - now;
        let at = stamp(self.expires_at);
        if left <= Duration::zero() {
            format!(
                "PARENT GRANT {} · expired {at} · the relay keeps this session and cannot rotate · renew from any terminal: {}",
                self.seat, self.command
            )
        } else if left < WARN_WITHIN {
            format!(
                "PARENT GRANT {} · {} left · expires {at} · the relay cannot rotate after that · renew from any terminal: {}",
                self.seat,
                authority::format_remaining(left),
                self.command
            )
        } else {
            format!(
                "PARENT GRANT {} · {} left · expires {at}",
                self.seat,
                authority::format_remaining(left)
            )
        }
    }

    /// PURE: the statusline suffix inside [`WARN_WITHIN`].
    pub(crate) fn suffix(&self, now: DateTime<Utc>) -> Option<String> {
        let left = self.expires_at - now;
        if left >= WARN_WITHIN {
            None
        } else if left <= Duration::zero() {
            Some("parent grant expired".to_string())
        } else {
            Some(format!(
                "parent grant expires in {}",
                authority::format_remaining(left)
            ))
        }
    }

    fn mail(&self, handle: &str, now: DateTime<Utc>) -> String {
        format!(
            "The {} seat grant {} expires at {} (in {}). Relay {handle} and every relay started under it cannot rotate after that; each keeps its current session until a human renews the grant from any terminal: {}",
            self.seat,
            short(&self.id),
            stamp(self.expires_at),
            authority::format_remaining(self.expires_at - now),
            self.command
        )
    }
}

/// The earliest expiry on `grant`'s delegation chain.
fn chain_expiry(grant: &SeatGrant) -> DateTime<Utc> {
    let mut at = grant.expires_at;
    let mut current = grant.clone();
    for _ in 0..64 {
        let Some(parent) = current
            .parent_grant_id
            .as_deref()
            .and_then(|id| authority::read_grant(id).ok())
        else {
            break;
        };
        at = at.min(parent.expires_at);
        current = parent;
    }
    at
}

/// The relay's own view: the grant this launcher exports, for a child it
/// launches as `requested_seat`. Informational; includes an expired record.
pub(crate) fn launcher_life(requested_seat: &str) -> Option<ParentLife> {
    let own = authority::read_grant(&authority::current_grant_id()?).ok()?;
    if own.revoked_at.is_some() {
        return None;
    }
    let pid = own
        .parent_grant_id
        .is_none()
        .then(|| bind_pid(Some(&own)))
        .flatten();
    let expires_at = chain_expiry(&own);
    let root = chain_root(own);
    Some(ParentLife {
        command: renewal_command(&root, requested_seat, pid),
        id: root.id,
        seat: root.seat,
        expires_at,
    })
}

/// The renewed grant that covers the relay holding `old`, as far as its
/// seat can tell: a successor that passes the grant verdict for its
/// principal (lifetime, roster) and is bound to the live process the relay
/// named in [`BIND_PID_ENV`], which the relay runs under. Any other later
/// grant for the same seat (another terminal's, another relay's) leaves the
/// relay held, so it must not quiet the seat's warning.
fn covering(
    project_root: &Path,
    old: &SeatGrant,
    requested_seat: &str,
    relay_pid: u32,
    now: DateTime<Utc>,
) -> Option<SeatGrant> {
    let dir = authority::grant_dir().ok()?;
    successors_in(&dir, old, requested_seat, now)
        .into_iter()
        .find(|next| {
            authority::bound_to_process(next, relay_pid)
                && matches!(
                    authority::grant_check_for_principal(project_root, &next.id, &next.principal),
                    GrantCheck::Valid(_)
                )
        })
}

/// The seat's view: the grant its relay launched it under, read from the
/// seat's `env` snapshot. `None` outside a supervised (`--rotate`) session.
/// A grant renewed for this relay reports the renewed expiry.
pub(crate) fn seat_life(
    project_root: &Path,
    env: &Environment,
    now: DateTime<Utc>,
) -> Option<ParentLife> {
    env.get(crate::agent_rotation::GENERATION_ENV)?;
    let own_id = env
        .var(authority::GRANT_ENV)
        .ok()
        .filter(|id| !id.trim().is_empty())?;
    let own = authority::read_grant(own_id).ok()?;
    let parent = authority::read_grant(own.parent_grant_id.as_deref()?).ok()?;
    if parent.revoked_at.is_some() {
        return None;
    }
    let direct = parent.parent_grant_id.is_none();
    let pid = env
        .var(BIND_PID_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|_| direct);
    let mut expires_at = chain_expiry(&parent);
    if expires_at - now < WARN_WITHIN {
        if let Some(next) = pid.and_then(|pid| covering(project_root, &parent, &own.seat, pid, now))
        {
            expires_at = next.expires_at;
        }
    }
    let root = chain_root(parent);
    Some(ParentLife {
        command: renewal_command(&root, &own.seat, pid),
        id: root.id,
        seat: root.seat,
        expires_at,
    })
}

/// `aida doctor`'s finding for a supervised seat whose relay grant is inside
/// [`WARN_WITHIN`] or expired (the `seat-grant` category).
pub(crate) fn doctor_scan(
    project_root: &Path,
    env: &Environment,
    now: DateTime<Utc>,
) -> Vec<crate::DoctorFinding> {
    let Some(life) = seat_life(project_root, env, now) else {
        return Vec::new();
    };
    if life.suffix(now).is_none() {
        return Vec::new();
    }
    vec![crate::DoctorFinding {
        category: authority::DOCTOR_CATEGORY.into(),
        id: format!(
            "{}:relay-parent:{}",
            authority::DOCTOR_CATEGORY,
            short(&life.id)
        ),
        summary: life.line(now),
        action: format!("a human runs `{}` in any terminal", life.command),
        safe_heal: false,
    }]
}

/// Mail the advisor and product seats once per grant, expiry and stage.
/// Best-effort: a failed delivery is retried on the next call.
fn notify(root: &Path, grant_id: &str, expires_at: DateTime<Utc>, stage: &str, body: &str) {
    let Ok(dir) = crate::grant_expiry_notice::notice_leaf(grant_id) else {
        return;
    };
    let prefix = format!(
        "relay-grant-{}-{}-{stage}",
        short(grant_id),
        expires_at.timestamp()
    );
    let recipients = ["advisor", "product"];
    if (0..recipients.len()).all(|i| dir.join(format!("{prefix}-{i}")).exists()) {
        return;
    }
    let store = root.join(".aida-store");
    let Ok(directory) = crate::mailbox_resolver::RecipientDirectory::load(root, &store) else {
        return;
    };
    let now = Utc::now().timestamp_millis();
    for (index, recipient) in recipients.iter().enumerate() {
        let id = format!("{prefix}-{index}");
        let marker = dir.join(&id);
        if marker.exists() {
            continue;
        }
        let Ok(resolved) = directory.resolve_send(recipient, true) else {
            // A project without that role has nobody to tell: settle it, so
            // the relay does not reload the directory every minute.
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::write(&marker, b"unresolved");
            }
            continue;
        };
        let Ok((deliveries, _)) =
            crate::mailbox_resolver::delivery_sidecar(&resolved, &id, now, false)
        else {
            continue;
        };
        let message = aida_core::mailbox::Message {
            id: id.clone(),
            thread_id: id,
            from: "aida-relay".into(),
            to: aida_core::mailbox::Recipient::Agent(resolved.address),
            timestamp: now,
            subject: Some("Relay seat grant renewal due".into()),
            body: body.to_string(),
            in_reply_to: None,
            urgent: false,
            intent: aida_core::mailbox::Intent::Request,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
            from_seat: None,
            relayed_from: None,
        };
        if crate::mailbox_store::write_message_with_deliveries(root, &message, deliveries.as_ref())
            .is_ok()
            && std::fs::create_dir_all(&dir).is_ok()
        {
            let _ = std::fs::write(&marker, b"delivered");
        }
    }
}

fn notify_hold(root: &Path, handle: &str, hold: &Hold) {
    notify(
        root,
        &hold.id,
        hold.expired_at,
        "expired",
        &hold.mail(handle),
    );
}

/// The relay's periodic check while a generation runs: mail the advisor and
/// product seats once the grant is inside [`WARN_WITHIN`].
pub(crate) fn warn_ahead(root: &Path, handle: &str, requested_seat: &str, now: DateTime<Utc>) {
    let Some(life) = launcher_life(requested_seat) else {
        return;
    };
    let left = life.expires_at - now;
    if left <= Duration::zero() || left >= WARN_WITHIN {
        return;
    }
    notify(
        root,
        &life.id,
        life.expires_at,
        "soon",
        &life.mail(handle, now),
    );
}

/// The relay's check before it ends a session at a turn boundary. `Some`
/// (announced once per `announced`, mailed until delivered) means: keep the session.
pub(crate) fn hold_before_end(
    root: &Path,
    handle: &str,
    requested_seat: &str,
    announced: &mut bool,
) -> bool {
    let Some(hold) = hold(root, requested_seat, false) else {
        *announced = false;
        return false;
    };
    if !*announced {
        eprintln!("{}", hold.message(handle, true));
        *announced = true;
    }
    // Independent of the stderr announcement, so a failed first delivery is
    // retried at the next turn boundary. trace:BUG-2585 | ai:claude
    notify_hold(root, handle, &hold);
    true
}

#[cfg(test)]
#[path = "tests/bug_2585_relay_grant_tests.rs"]
mod tests;
