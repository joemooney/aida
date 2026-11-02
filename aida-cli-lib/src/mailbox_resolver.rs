//! Live delivery resolution shared by explicit mail and comment mentions.
//! Historical mailbox inspection deliberately does not require a live reader.
// trace:STORY-1843 | ai:codex
use crate::agent_registry::{self, AgentRegistryEntry};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub(crate) struct RecipientDirectory {
    project: std::path::PathBuf,
    agents: Vec<AgentRegistryEntry>,
    roles: BTreeMap<String, String>,
    people: HashMap<String, String>,
    operator: Option<String>,
    live_ids: BTreeSet<String>,
    unknown_ids: BTreeSet<String>,
    // trace:STORY-1843 | ai:claude
    relay_runs: Vec<crate::relay_mail::AcceptingRun>,
    warnings: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct ResolvedRecipient {
    pub(crate) address: String,
    pub(crate) reader_notice: Option<String>,
    /// For a role address: the send-time reader snapshot (slice D1).
    pub(crate) cohort: Option<CohortReaders>,
    /// For a relay label: the open run that owns it (slice D2b).
    // trace:STORY-1843 | ai:claude
    pub(crate) relay_run: Option<aida_core::mailbox_delivery::RecipientUuid>,
}

/// The live and unverified readers of a role at send, split by whether
/// each has a recipient UUID. `queued`: zero readers and the sender asked
/// for `--queue-for-role`.
// trace:STORY-1843 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct CohortReaders {
    pub(crate) role: String,
    pub(crate) readers: Vec<aida_core::mailbox_delivery::RecipientUuid>,
    pub(crate) uuidless: Vec<String>,
    pub(crate) queued: bool,
}

/// The delivery sidecar for `resolved` (slice D1) and the warnings to
/// print. A role cohort gets one record per reader UUID; readers with no
/// UUID are left out and named. A send queued for the role (`queued`: the
/// role had no reader with a UUID and the sender asked to queue) writes one
/// pending record that the first eligible reader with a UUID takes. Without
/// that request a cohort left with no reader refuses, as a zero-reader send
/// does; `cli_hint` names the CLI's `--queue-for-role` flag in that refusal
/// (MCP callers cannot pass it, so their refusal never names it). Other
/// addresses get no sidecar.
// trace:STORY-1843 | ai:claude
pub(crate) fn delivery_sidecar(
    resolved: &ResolvedRecipient,
    message_id: &str,
    now_ms: i64,
    cli_hint: bool,
) -> Result<(
    Option<aida_core::mailbox_delivery::DeliverySidecar>,
    Vec<String>,
)> {
    use aida_core::mailbox_delivery::DeliverySidecar;
    let Some(cohort) = &resolved.cohort else {
        // Mail to a relay label belongs to the run that owns the label.
        // trace:STORY-1843 | ai:claude
        return Ok((
            resolved
                .relay_run
                .as_ref()
                .map(|run| DeliverySidecar::relay(message_id, run)),
            Vec::new(),
        ));
    };
    let uuidless = |verb: &str| -> Vec<String> {
        cohort
            .uuidless
            .iter()
            .map(|label| {
                format!("warning: {label} has no recipient UUID and will not {verb} this; address it by handle")
            })
            .collect()
    };
    if cohort.queued {
        let mut warnings = vec![format!(
            "queued for '{}': the first eligible reader with a recipient UUID to check its mail takes it",
            cohort.role
        )];
        warnings.extend(uuidless("take"));
        return Ok((
            Some(DeliverySidecar::queued(message_id, &cohort.role, now_ms)),
            warnings,
        ));
    }
    if cohort.readers.is_empty() {
        let queue = if cli_hint {
            ", or pass --queue-for-role"
        } else {
            ""
        };
        bail!(
            "send refused: '{}' has 0 readers with a recipient UUID ({} has none); address a reader by handle{queue}",
            cohort.role,
            cohort.uuidless.join(", ")
        );
    }
    Ok((
        Some(DeliverySidecar::cohort(
            message_id,
            &cohort.role,
            now_ms,
            &cohort.readers,
        )),
        uuidless("receive"),
    ))
}

// Unknown liveness is usable only while its heartbeat remains recent.
// trace:STORY-1843 | ai:codex
const UNKNOWN_READER_TTL_SECONDS: i64 = 24 * 60 * 60;
const MAX_NOTICE_READERS: usize = 5;

// Legacy entries have no namespace stamp. A non-host observer cannot prove
// those PIDs dead, even if its private /proc exposes only one NSpid level.
// This is delivery information only, never lifecycle/lease authority.
// trace:STORY-1843 | ai:codex
fn reader_liveness(
    project: &Path,
    entry: &AgentRegistryEntry,
    namespace: Option<u64>,
) -> Option<bool> {
    if entry.source == "role-enter" {
        return Some(false);
    }
    agent_registry::observed_liveness(project, entry, namespace).or_else(|| {
        let age = chrono::Utc::now().signed_duration_since(entry.last_active_at);
        let fresh = age >= chrono::Duration::zero()
            && age <= chrono::Duration::seconds(UNKNOWN_READER_TTL_SECONDS);
        (!fresh).then_some(false)
    })
}

// The operator is AIDA_USER when it names a known person, else the one person
// holding an explicit `operator` seat in the team roster. Catalog order never
// picks one: no unique answer means the hint names nobody.
// trace:STORY-1843 | ai:claude
fn select_operator(
    aida_user: Option<&str>,
    people: &HashMap<String, String>,
    roster: &[(String, Vec<String>)],
) -> Option<String> {
    let usable = |person: &String| {
        !person.chars().any(char::is_whitespace)
            && aida_core::mailbox::recipient_id_rejection(person).is_none()
    };
    if let Some(person) = aida_user
        .and_then(|name| people.get(&name.trim().to_lowercase()))
        .filter(|p| usable(p))
    {
        return Some(person.clone());
    }
    let mut seated = roster
        .iter()
        .filter(|(_, seats)| {
            seats
                .iter()
                .any(|s| s.trim().eq_ignore_ascii_case("operator"))
        })
        .filter_map(|(user, _)| people.get(&user.trim().to_lowercase()))
        .filter(|p| usable(p))
        .collect::<BTreeSet<_>>()
        .into_iter();
    match (seated.next(), seated.next()) {
        (Some(only), None) => Some(only.clone()),
        _ => None,
    }
}

impl RecipientDirectory {
    /// [`Self::load`] for a send. Send is a relay reconciliation point: a
    /// run whose controller died is ended (and its end notices queued)
    /// before it is resolved. Every other caller, including read-only
    /// previews, uses [`Self::load`], which never writes.
    // trace:STORY-1843 | ai:claude
    pub(crate) fn load_for_send(project: &Path, store: &Path) -> Result<Self> {
        let reconciled = crate::relay_mail::reconcile_relay_runs(project);
        let mut directory = Self::load(project, store)?;
        if let Err(e) = reconciled {
            directory
                .warnings
                .push(format!("warning: relay run reconciliation skipped: {e:#}"));
            directory.warnings.sort();
        }
        Ok(directory)
    }

    pub(crate) fn load(project: &Path, store: &Path) -> Result<Self> {
        let mut agents = Vec::new();
        let mut warnings = Vec::new();
        // Refuse a failed directory scan, but a damaged individual record is
        // not a reader and must not disable unrelated human or agent inboxes.
        let entries = match std::fs::read_dir(agent_registry::agents_dir(project)) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("reading mailbox recipient registry"),
        };
        if let Some(entries) = entries {
            for entry in entries {
                let path = match entry {
                    Ok(entry) => entry.path(),
                    Err(error) => {
                        warnings.push(format!(
                            "warning: skipping unreadable agent registry entry: {error}"
                        ));
                        continue;
                    }
                };
                if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                let body = match std::fs::read_to_string(&path) {
                    Ok(body) => body,
                    Err(error) => {
                        warnings.push(format!(
                            "warning: skipping unreadable agent registry record {}: {error}",
                            path.display()
                        ));
                        continue;
                    }
                };
                let record: AgentRegistryEntry = match toml::from_str(&body) {
                    Ok(record) => record,
                    Err(_) => {
                        warnings.push(format!(
                            "warning: skipping invalid agent registry record {}",
                            path.display()
                        ));
                        continue;
                    }
                };
                agents.push(record);
            }
        }
        let namespace = agent_registry::current_pid_namespace();
        let mut live_ids = BTreeSet::new();
        let mut unknown_ids = BTreeSet::new();
        for agent in &agents {
            match reader_liveness(project, agent, namespace) {
                Some(true) => {
                    live_ids.insert(agent.id.clone());
                }
                None => {
                    unknown_ids.insert(agent.id.clone());
                }
                Some(false) => {}
            }
        }
        let mut roles: BTreeMap<_, _> = crate::AGENT_ROLES
            .iter()
            .map(|r| (r.to_lowercase(), r.to_string()))
            .collect();
        for role in crate::list_roles(project)? {
            roles.insert(role.name.to_lowercase(), role.name);
        }
        let (relay_runs, relay_warnings) = crate::relay_mail::accepting_runs(project);
        warnings.extend(relay_warnings);
        warnings.sort();
        let people = crate::known_mailbox_people(store);
        let operator = select_operator(
            std::env::var("AIDA_USER").ok().as_deref(),
            &people,
            &aida_core::team::TeamRoster::load(store).entries(),
        );
        Ok(Self {
            project: project.to_path_buf(),
            agents,
            roles,
            people,
            operator,
            live_ids,
            unknown_ids,
            relay_runs,
            warnings,
        })
    }

    // These diagnostics are distinct from a group's reader notice: a skipped
    // record never establishes a recipient kind or contributes a live reader.
    // trace:STORY-1843 | ai:codex
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    // trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
    /// Inspection is read-only and stricter than send's fresh-but-unverified
    /// cohort. A failed observation remains diagnostic unknown, never authority.
    pub(crate) fn inspection_readers(
        &self,
    ) -> Vec<(
        &AgentRegistryEntry,
        Result<aida_core::mailbox_identity::InspectionSubject>,
    )> {
        self.agents
            .iter()
            .filter(|e| {
                e.source != "role-enter"
                    && agent_registry::observed_liveness(
                        &self.project,
                        e,
                        agent_registry::current_pid_namespace(),
                    ) != Some(false)
            })
            .map(|entry| {
                let subject = if agent_registry::observed_liveness(
                    &self.project,
                    entry,
                    agent_registry::current_pid_namespace(),
                )
                .is_none()
                {
                    Err(anyhow::anyhow!("reader liveness unknown"))
                } else {
                    aida_core::mailbox_identity::InspectionSubject::observe(
                        &agent_registry::agents_dir(&self.project)
                            .join(format!("{}.toml", entry.id)),
                    )
                    .map_err(|_| anyhow::anyhow!("reader identity/reservation unknown"))
                };
                (entry, subject)
            })
            .collect()
    }

    fn reader_may_exist(&self, entry: &AgentRegistryEntry) -> bool {
        self.live_ids.contains(&entry.id) || self.unknown_ids.contains(&entry.id)
    }

    // Qualifying the person keeps the escape resolvable even when their name
    // also names a role. Environment text alone never invents a recipient.
    // trace:STORY-1843 | ai:codex
    fn refusal_hint(&self, mention: bool) -> String {
        let hint = match &self.operator {
            Some(person) => format!("mail the operator (--to person:{person})"),
            None => "contact the operator".to_string(),
        };
        if mention {
            hint
        } else {
            format!("{hint} or use aida comment add <SPEC>")
        }
    }

    pub(crate) fn resolve(&self, target: &str) -> Result<ResolvedRecipient> {
        self.resolve_with_context(target, false, false, false)
    }

    /// [`Self::resolve`] for a CLI send, which may queue for a role with no
    /// reader (`--queue-for-role`). Queuing applies only to a role, and only
    /// this surface's refusals name the flag (MCP cannot pass it).
    // trace:STORY-1843 | ai:claude
    pub(crate) fn resolve_send(
        &self,
        target: &str,
        queue_for_role: bool,
    ) -> Result<ResolvedRecipient> {
        self.resolve_with_context(target, false, queue_for_role, true)
    }

    pub(crate) fn resolve_mention(&self, target: &str) -> Result<ResolvedRecipient> {
        self.resolve_with_context(target, true, false, false)
    }

    /// The recipient UUID a reader's handle reservation gives its row.
    fn reader_uuid(
        &self,
        entry: &AgentRegistryEntry,
    ) -> Option<aida_core::mailbox_delivery::RecipientUuid> {
        let handle = entry.name.as_deref()?.trim();
        let sessions: Vec<&str> = [&entry.claude_session_id, &entry.native_session_id]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        aida_core::mailbox_identity::reservation_recipient(
            &agent_registry::agents_dir(&self.project),
            handle,
            &entry.id,
            &sessions,
        )
    }

    fn resolve_with_context(
        &self,
        target: &str,
        mention: bool,
        queue_for_role: bool,
        cli_hint: bool,
    ) -> Result<ResolvedRecipient> {
        let mut resolved = self.resolve_address(target, mention, queue_for_role, cli_hint)?;
        // A relay label resolves to the run that owns it, whether a session
        // of the run is live or the run is between sessions. A live session
        // under the label after the run ended is not the run.
        // trace:STORY-1843 | ai:claude
        if resolved.cohort.is_none() {
            resolved.relay_run = self
                .relay_runs
                .iter()
                .find(|r| r.label.eq_ignore_ascii_case(&resolved.address))
                .and_then(|r| aida_core::mailbox_delivery::RecipientUuid::parse(&r.run_id));
        }
        Ok(resolved)
    }

    fn resolve_address(
        &self,
        target: &str,
        mention: bool,
        queue_for_role: bool,
        cli_hint: bool,
    ) -> Result<ResolvedRecipient> {
        let escape = self.refusal_hint(mention);
        let target = target.trim();
        if let Some(why) = aida_core::mailbox::recipient_id_rejection(target) {
            bail!("send refused: recipient {why}");
        }
        let key = target.to_lowercase();
        if let Some(name) = key.strip_prefix("person:") {
            return self
                .people
                .get(name)
                .map(|person| ResolvedRecipient {
                    relay_run: None,
                    address: person.clone(),
                    reader_notice: None,
                    cohort: None,
                })
                .ok_or_else(|| anyhow::anyhow!("send refused: unknown person '{name}'; {escape}"));
        }
        let role = self.roles.get(&key);
        let vendor = match key.as_str() {
            "claude" | "codex" | "antigravity" | "web" => Some(key.as_str()),
            "agy" => Some("antigravity"),
            _ => None,
        };
        let person = self
            .people
            .get(&key)
            .filter(|_| role.is_none() && vendor.is_none());
        // Existing launches may borrow a reserved alias until PR2 reserves
        // names. Such a record must not poison that catalog address. Looking
        // it up by ID still goes through the private-inbox collision guard.
        // trace:STORY-1843 | ai:codex
        let catalog_address = role.is_some() || vendor.is_some() || person.is_some();
        let named: Vec<_> = self
            .agents
            .iter()
            .filter(|a| {
                !catalog_address
                    && (a.id.eq_ignore_ascii_case(target)
                        || a.name
                            .as_deref()
                            .is_some_and(|n| n.trim().eq_ignore_ascii_case(target)))
            })
            .collect();
        if (role.is_some() as u8
            + vendor.is_some() as u8
            + person.is_some() as u8
            + named.iter().any(|a| self.reader_may_exist(a)) as u8)
            > 1
        {
            bail!("send refused: ambiguous recipient '{target}' names more than one recipient kind; {escape}");
        }
        if let Some(person) = person {
            if queue_for_role {
                bail!("send refused: --queue-for-role applies only to a role address");
            }
            return Ok(ResolvedRecipient {
                relay_run: None,
                address: person.clone(),
                reader_notice: None,
                cohort: None,
            });
        }
        if queue_for_role && role.is_none() {
            bail!("send refused: --queue-for-role applies only to a role address, not '{target}'");
        }
        if role.is_some() || vendor.is_some() {
            // Live readers sort ahead of unverified ones so the five-name cap
            // never hides a live reader. trace:STORY-1843 | ai:claude
            let readers: BTreeMap<_, _> = self
                .agents
                .iter()
                .filter(|a| self.reader_may_exist(a))
                .filter(|a| {
                    if let Some(role) = role {
                        a.role
                            .as_deref()
                            .is_some_and(|r| r.eq_ignore_ascii_case(role))
                    } else {
                        a.agent_type == vendor.unwrap()
                    }
                })
                .map(|a| {
                    let label = a
                        .name
                        .as_deref()
                        .map(|name| format!("{name} ({})", a.id))
                        .unwrap_or_else(|| a.id.clone());
                    (
                        (self.unknown_ids.contains(&a.id), label),
                        !self.unknown_ids.contains(&a.id),
                    )
                })
                .collect();
            let address = role.cloned().unwrap_or_else(|| vendor.unwrap().to_string());
            if readers.is_empty() && queue_for_role {
                return Ok(ResolvedRecipient {
                    relay_run: None,
                    address: address.clone(),
                    reader_notice: None,
                    cohort: Some(CohortReaders {
                        role: address,
                        queued: true,
                        ..Default::default()
                    }),
                });
            }
            if readers.is_empty() {
                let queue = if cli_hint && role.is_some() {
                    ", or pass --queue-for-role"
                } else {
                    ""
                };
                bail!("send refused: '{address}' has 0 live readers; {escape}{queue}");
            }
            // The send-time cohort of a role: the same readers the notice
            // names, split by recipient UUID. trace:STORY-1843 | ai:claude
            let cohort = role.map(|role| {
                let mut cohort = CohortReaders {
                    role: role.clone(),
                    ..Default::default()
                };
                for a in self.agents.iter().filter(|a| {
                    self.reader_may_exist(a)
                        && a.role
                            .as_deref()
                            .is_some_and(|r| r.eq_ignore_ascii_case(role))
                }) {
                    match self.reader_uuid(a) {
                        Some(uuid) => cohort.readers.push(uuid),
                        None => cohort.uuidless.push(
                            a.name
                                .as_deref()
                                .map(|name| format!("{name} ({})", a.id))
                                .unwrap_or_else(|| a.id.clone()),
                        ),
                    }
                }
                cohort.readers.sort();
                cohort.readers.dedup();
                // Every live reader is UUID-less: an explicit queue request
                // queues for the first reader with a UUID (advisor ruling,
                // PR-3042 round 1). trace:STORY-1843 | ai:claude
                // A pool role (several generic implementers) never fans out:
                // its mail queues so exactly one member takes it — also when
                // every live member is UUID-less (the first with a UUID takes it).
                // trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude
                cohort.queued = if agent_registry::is_pool_role(role) {
                    !cohort.readers.is_empty() || !cohort.uuidless.is_empty() || queue_for_role
                } else {
                    queue_for_role && cohort.readers.is_empty()
                };
                cohort
            });
            let live_count = readers.values().filter(|live| **live).count();
            let unknown = readers.len() - live_count;
            let mut live_labels = Vec::new();
            let mut unverified_labels = Vec::new();
            for ((_, label), live) in readers.iter().take(MAX_NOTICE_READERS) {
                if *live {
                    live_labels.push(label.as_str());
                } else {
                    unverified_labels.push(label.as_str());
                }
            }
            let mut lists = Vec::new();
            if !live_labels.is_empty() {
                lists.push(format!("live: {}", live_labels.join(", ")));
            }
            if !unverified_labels.is_empty() {
                lists.push(format!("unverified: {}", unverified_labels.join(", ")));
            }
            if readers.len() > MAX_NOTICE_READERS {
                lists.push(format!("and {} more", readers.len() - MAX_NOTICE_READERS));
            }
            if unknown > 0 {
                lists.push(
                    "unverified readers have unknown liveness; delivering to durable inbox"
                        .to_string(),
                );
            }
            let warning = if readers.len() > 1 || unknown > 0 {
                "warning: "
            } else {
                ""
            };
            return Ok(ResolvedRecipient {
                relay_run: None,
                address,
                cohort,
                reader_notice: Some(format!(
                    "{warning}{live_count} live, {unknown} unverified: {}",
                    lists.join("; ")
                )),
            });
        }
        let live: Vec<_> = named.iter().filter(|a| self.reader_may_exist(a)).collect();
        // Between a relay's sessions its label has no live reader, but the
        // run still owns it: the next session reads the mail.
        // trace:STORY-1843 | ai:claude
        if live.is_empty() && !catalog_address {
            if let Some(run) = self
                .relay_runs
                .iter()
                .find(|r| r.label.eq_ignore_ascii_case(target))
            {
                let notice = match run.liveness {
                    crate::relay_mail::ControllerLiveness::Live => format!(
                        "relay {} is between sessions; its next session reads this",
                        run.label
                    ),
                    _ => "warning: relay controller liveness unknown (PID namespace unobservable); delivering to durable inbox".to_string(),
                };
                return Ok(ResolvedRecipient {
                    relay_run: None,
                    address: run.label.clone(),
                    reader_notice: Some(notice),
                    cohort: None,
                });
            }
        }
        match live.as_slice() {
            [agent] => {
                let Some(name) = agent.name.as_deref().filter(|n| !n.trim().is_empty()) else {
                    bail!("send refused: '{target}' has no private inbox name; address its role or vendor group explicitly, or launch it with a name");
                };
                // Resolving an ID must not bypass ambiguity in the actual inbox spelling.
                let name = name.trim();
                if let Some(why) = aida_core::mailbox::recipient_id_rejection(name) {
                    bail!("send refused: inbox name for '{target}' {why}; register a valid private name");
                }
                let owners = self
                    .agents
                    .iter()
                    .filter(|a| self.reader_may_exist(a))
                    .filter(|a| {
                        a.name
                            .as_deref()
                            .is_some_and(|n| n.trim().eq_ignore_ascii_case(name))
                    })
                    .count();
                if owners != 1
                    || self.roles.contains_key(&name.to_lowercase())
                    || self.people.contains_key(&name.to_lowercase())
                    || matches!(
                        name.to_ascii_lowercase().as_str(),
                        "claude" | "codex" | "antigravity" | "agy" | "web"
                    )
                {
                    bail!("send refused: ambiguous inbox '{name}' for '{target}'; {escape}");
                }
                Ok(ResolvedRecipient { relay_run: None, address: name.to_string(), cohort: None, reader_notice: self.unknown_ids.contains(&agent.id).then(||
                    "warning: reader liveness unknown (PID namespace unobservable); delivering to durable inbox".to_string()) })
            }
            [] if !named.is_empty() => {
                bail!("send refused: '{target}' has no live agent; {escape}")
            }
            [] => bail!("send refused: unknown recipient '{target}'; {escape}"),
            _ => bail!("send refused: ambiguous live recipient '{target}'; {escape}"),
        }
    }
}

/// Give a test registry row a handle reservation that names it, so the
/// row's reader has a recipient UUID (slice D1). Test-only.
// trace:STORY-1843 | ai:claude
#[cfg(test)]
pub(crate) fn reserve_test_reader(project: &Path, entry: &AgentRegistryEntry) {
    let handle = entry.name.as_deref().expect("a test reader has a handle");
    let dir = agent_registry::agents_dir(project).join("handles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{handle}.toml")),
        format!(
            "handle = '{handle}'\nrecipient_id = '{}'\nregistry_id = '{}'\n",
            uuid::Uuid::new_v4(),
            entry.id
        ),
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(project: &Path, id: &str, name: &str, role: &str, ended: bool) {
        let dir = agent_registry::agents_dir(project);
        std::fs::create_dir_all(&dir).unwrap();
        let end = if ended {
            "ended_at = '2026-10-20T00:00:00Z'\n"
        } else {
            ""
        };
        std::fs::write(dir.join(format!("{id}.toml")), format!(
            "id = '{id}'\nagent_type = 'codex'\npid = {}\nname = '{name}'\nrole = '{role}'\nstarted_at = '2026-10-20T00:00:00Z'\nlast_active_at = '2026-10-20T00:00:00Z'\nworktree_path = '{}'\nsource = 'agent-launcher'\n{end}", std::process::id(), project.display())).unwrap();
        if let Some(namespace) = agent_registry::current_pid_namespace() {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(dir.join(format!("{id}.toml")))
                .unwrap();
            writeln!(file, "pid_namespace = {namespace}").unwrap();
        }
    }

    // Build the same directory classification a foreign PID namespace observes.
    // trace:STORY-1843 | ai:codex
    fn observed_from(project: &Path, namespace: u64) -> RecipientDirectory {
        let mut directory =
            RecipientDirectory::load(project, &project.join(".aida-store")).unwrap();
        directory.live_ids.clear();
        directory.unknown_ids.clear();
        for entry in &directory.agents {
            match reader_liveness(project, entry, Some(namespace)) {
                Some(true) => {
                    directory.live_ids.insert(entry.id.clone());
                }
                None => {
                    directory.unknown_ids.insert(entry.id.clone());
                }
                Some(false) => {}
            }
        }
        directory
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn round2_stale_unknown_readers_refuse_role_and_named_delivery_without_rewriting() {
        for namespace in [None, Some(42)] {
            let tmp = tempfile::tempdir().unwrap();
            registry(tmp.path(), "legacy", "legacy-worker", "advisor", false);
            let mut entry = agent_registry::load_entries(tmp.path()).remove(0).1;
            entry.pid = u32::MAX;
            entry.pid_namespace = namespace;
            entry.last_active_at = chrono::Utc::now() - chrono::Duration::hours(25);
            agent_registry::write_entry(tmp.path(), &entry).unwrap();
            let path = agent_registry::agents_dir(tmp.path()).join("legacy.toml");
            let before = std::fs::read(&path).unwrap();
            let directory = observed_from(tmp.path(), 43);
            for target in ["advisor", "legacy-worker", "legacy"] {
                assert!(
                    directory.resolve(target).is_err(),
                    "stale {target} must refuse"
                );
            }
            assert_eq!(std::fs::read(path).unwrap(), before);
        }
    }

    // TASK-2177: from a sandbox every row is foreign, so a finished session
    // that ran within 24h used to count as an unverified reader. Once a host
    // observer's gc ends it, the sandboxed role send refuses.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn sandboxed_role_send_refuses_after_host_gc_ends_dead_resumable_reader() {
        let Some(host) = agent_registry::current_pid_namespace() else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "finished", "finished-worker", "reviewer", false);
        let mut entry = agent_registry::load_entries(tmp.path()).remove(0).1;
        entry.pid = u32::MAX;
        entry.pid_namespace = Some(host);
        entry.native_session_id = Some("native-1".to_string());
        entry.last_active_at = chrono::Utc::now() - chrono::Duration::hours(1);
        agent_registry::write_entry(tmp.path(), &entry).unwrap();
        let sandbox = host ^ 1;

        let before = observed_from(tmp.path(), sandbox);
        assert!(
            before.resolve("reviewer").is_ok(),
            "fresh foreign row is unverified"
        );

        let report = agent_registry::gc_dead_agents(tmp.path(), false, None).unwrap();
        assert_eq!(report.marked_ended, 1);

        let after = observed_from(tmp.path(), sandbox);
        let error = after.resolve("reviewer").unwrap_err().to_string();
        assert!(error.contains("0 live readers"), "{error}");
        assert!(after.resolve("finished-worker").is_err());
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn round2_fresh_unknown_readers_deliver_and_are_labeled_unverified() {
        for namespace in [None, Some(42)] {
            let tmp = tempfile::tempdir().unwrap();
            registry(tmp.path(), "foreign", "foreign-worker", "advisor", false);
            let mut entry = agent_registry::load_entries(tmp.path()).remove(0).1;
            entry.pid = u32::MAX;
            entry.pid_namespace = namespace;
            entry.last_active_at = chrono::Utc::now();
            agent_registry::write_entry(tmp.path(), &entry).unwrap();
            let directory = observed_from(tmp.path(), 43);
            let role = directory.resolve("advisor").unwrap();
            assert_eq!(role.address, "advisor");
            let notice = role.reader_notice.unwrap();
            assert!(notice.contains("0 live, 1 unverified:"), "{notice}");
            assert!(
                notice.contains("unverified: foreign-worker (foreign)"),
                "{notice}"
            );
            assert!(!notice.contains("live: foreign-worker"), "{notice}");
            assert!(notice.contains("unknown liveness"), "{notice}");
            assert_eq!(
                directory.resolve("foreign-worker").unwrap().address,
                "foreign-worker"
            );
        }
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn round2_missing_heartbeat_is_not_a_reader_and_record_is_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "missing", "missing-worker", "advisor", false);
        let path = agent_registry::agents_dir(tmp.path()).join("missing.toml");
        let body = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("last_active_at") && !l.starts_with("pid_namespace"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &body).unwrap();
        let directory = observed_from(tmp.path(), 43);
        assert!(directory.resolve("advisor").is_err());
        assert!(directory.resolve("missing-worker").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), body);
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn round2_group_notice_caps_twenty_readers_at_five() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..20 {
            registry(
                tmp.path(),
                &format!("id-{i:02}"),
                &format!("worker-{i:02}"),
                "advisor",
                false,
            );
        }
        let directory =
            RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        let notice = directory.resolve("advisor").unwrap().reader_notice.unwrap();
        assert!(notice.contains("20 live, 0 unverified:"), "{notice}");
        assert!(notice.contains("and 15 more"), "{notice}");
        for i in 0..20 {
            assert_eq!(
                notice.contains(&format!("worker-{i:02}")),
                i < 5,
                "{notice}"
            );
        }
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn round2_mixed_notice_keeps_unverified_names_out_of_live_list() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..20 {
            let id = format!("id-{i:02}");
            registry(tmp.path(), &id, &format!("worker-{i:02}"), "advisor", false);
            if i % 2 == 1 {
                let path = agent_registry::agents_dir(tmp.path()).join(format!("{id}.toml"));
                let mut entry: AgentRegistryEntry =
                    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                entry.pid_namespace = Some(agent_registry::current_pid_namespace().unwrap() ^ 1);
                entry.last_active_at = chrono::Utc::now();
                agent_registry::write_entry(tmp.path(), &entry).unwrap();
            }
        }
        let directory =
            RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        let notice = directory.resolve("advisor").unwrap().reader_notice.unwrap();
        assert!(notice.contains("10 live, 10 unverified:"), "{notice}");
        let live_list = notice
            .split(": live: ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        // Live readers (even indices) fill the cap before any unverified one.
        // trace:STORY-1843 | ai:claude
        assert!(!notice.contains("; unverified: "), "{notice}");
        for i in 0..20 {
            let name = format!("worker-{i:02}");
            let shown = i % 2 == 0 && i < 10;
            assert_eq!(notice.contains(&name), shown, "{notice}");
            assert_eq!(live_list.contains(&name), shown, "{notice}");
        }
        assert!(notice.contains("and 15 more"), "{notice}");
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn round2_escape_hint_names_only_a_resolvable_person_or_no_recipient() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("registry")).unwrap();
        let mut directory = RecipientDirectory::load(tmp.path(), &store).unwrap();
        directory.operator = None;
        let error = directory.resolve("missing").unwrap_err().to_string();
        assert!(!error.contains("--to "), "{error}");
        std::fs::write(
            store.join("registry/team.toml"),
            "[members]\nZara = []\nAdvisor = [\"operator\"]\n",
        )
        .unwrap();
        let mut directory = RecipientDirectory::load(tmp.path(), &store).unwrap();
        directory.operator = select_operator(
            None,
            &directory.people,
            &aida_core::team::TeamRoster::load(&store).entries(),
        );
        let error = directory.resolve("missing").unwrap_err().to_string();
        let hint = error
            .split("--to ")
            .nth(1)
            .expect("operator seat supplies an escape");
        let target = hint.split(')').next().unwrap();
        assert_eq!(target, "person:Advisor", "{error}");
        assert!(directory.resolve(target).is_ok(), "{error}");
    }

    // TASK-2179: never the first catalog person; only AIDA_USER or a unique
    // explicit operator seat. trace:STORY-1843 | ai:claude
    #[test]
    fn operator_comes_only_from_aida_user_or_a_unique_operator_seat() {
        let people: HashMap<String, String> = ["aaron", "joe", "zara"]
            .iter()
            .map(|p| (p.to_string(), p.to_string()))
            .collect();
        let seats = |rows: &[(&str, &[&str])]| -> Vec<(String, Vec<String>)> {
            rows.iter()
                .map(|(u, s)| (u.to_string(), s.iter().map(|x| x.to_string()).collect()))
                .collect()
        };
        let plain = seats(&[("aaron", &["advisor"]), ("zara", &["reviewer"])]);
        assert_eq!(select_operator(None, &people, &plain), None);
        assert_eq!(select_operator(Some("ghost"), &people, &plain), None);
        assert_eq!(
            select_operator(Some(" JOE "), &people, &plain).as_deref(),
            Some("joe")
        );
        let one = seats(&[("aaron", &["advisor"]), ("zara", &["Operator"])]);
        assert_eq!(
            select_operator(None, &people, &one).as_deref(),
            Some("zara")
        );
        assert_eq!(
            select_operator(Some("joe"), &people, &one).as_deref(),
            Some("joe"),
            "a resolvable AIDA_USER wins over the roster"
        );
        let two = seats(&[("aaron", &["operator"]), ("zara", &["operator"])]);
        assert_eq!(select_operator(None, &people, &two), None);
        let unknown = seats(&[("nobody", &["operator"])]);
        assert_eq!(select_operator(None, &people, &unknown), None);
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn round2_mention_drop_hint_never_suggests_comment_add() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        let (recipients, warnings) =
            crate::punt_done::resolve_comment_mentions(&store, "author", "@missing-worker");
        assert!(recipients.is_empty());
        let warning = warnings
            .iter()
            .find(|w| w.contains("mention dropped"))
            .unwrap();
        assert!(!warning.contains("aida comment add"), "{warning}");
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn namespace_mismatch_is_unknown_even_when_pid_probe_says_dead() {
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "foreign", "foreign-reader", "advisor", false);
        let mut entry = agent_registry::load_entries(tmp.path()).remove(0).1;
        entry.pid = u32::MAX;
        entry.pid_namespace = Some(42);
        entry.last_active_at = chrono::Utc::now();
        assert!(!agent_registry::registry_entry_is_live(tmp.path(), &entry));
        assert_eq!(reader_liveness(tmp.path(), &entry, Some(43)), None);
        assert_eq!(reader_liveness(tmp.path(), &entry, None), None);
        assert_eq!(reader_liveness(tmp.path(), &entry, Some(42)), Some(false));
        entry.pid_namespace = None;
        assert_eq!(reader_liveness(tmp.path(), &entry, Some(43)), None);
        assert_eq!(
            reader_liveness(tmp.path(), &entry, Some(0xEFFF_FFFC)),
            Some(false)
        );
        entry.ended_at = Some(chrono::Utc::now());
        assert_eq!(reader_liveness(tmp.path(), &entry, Some(43)), Some(false));
    }

    // trace:STORY-1843.acfeb946 | ai:claude
    #[test]
    fn unknown_dead_and_zero_reader_recipients_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "codex-old", "old-worker", "implementer", true);
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        for name in [
            "typo-worker",
            "old-worker",
            "codex-old",
            "implementer",
            "codex",
            "--include",
        ] {
            let error = d.resolve(name).unwrap_err().to_string();
            if name != "--include" {
                assert!(!error.contains("--to "), "{error}");
                assert!(error.contains("aida comment add <SPEC>"), "{error}");
                assert!(!error.contains("handle"), "{error}");
            }
        }
        assert!(!tmp.path().join(".aida/mailbox").exists());
    }

    #[test]
    fn name_and_id_resolve_to_existing_inbox_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        registry(
            tmp.path(),
            "codex-worker",
            "Worker-One",
            "implementer",
            false,
        );
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        assert_eq!(d.resolve(" worker-one ").unwrap().address, "Worker-One");
        assert_eq!(d.resolve("codex-worker").unwrap().address, "Worker-One");
        let group = d.resolve("IMPLEMENTER").unwrap();
        assert_eq!(group.address, "implementer");
        assert!(group.reader_notice.unwrap().contains("Worker-One"));
    }

    #[test]
    fn multiple_readers_are_named_and_duplicate_names_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "codex-one", "worker-one", "implementer", false);
        registry(tmp.path(), "codex-two", "worker-two", "implementer", false);
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        let group = d.resolve("implementer").unwrap().reader_notice.unwrap();
        assert!(
            group.contains("warning:")
                && group.contains("worker-one (codex-one), worker-two (codex-two)"),
            "{group}"
        );
        registry(tmp.path(), "codex-three", "worker-one", "reviewer", false);
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        assert!(d
            .resolve("worker-one")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
    }

    #[test]
    fn registry_id_cannot_bypass_duplicate_inbox_names() {
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "codex-one", "same-name", "implementer", false);
        registry(tmp.path(), "codex-two", "same-name", "reviewer", false);
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        assert!(d
            .resolve("codex-one")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
    }

    // trace:STORY-1843.acfeb946 | ai:claude
    #[test]
    fn cli_send_refuses_without_write_then_delivers_to_canonical_name() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        let command = |target: &str| crate::cli::MailboxCommand::Send {
            to: Some(target.into()),
            broadcast: false,
            body: Some("routing probe".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: None,
            from: Some("sender".into()),
            urgent: false,
            intent: "fyi".into(),
            relayed_from: None,
            allow_second_person: false,
            queue_for_role: false,
        };
        assert!(
            crate::mailbox_cmd::handle_mailbox_command(&command("typo-worker"), &store).is_err()
        );
        assert!(crate::mailbox_store::read_local_messages(tmp.path())
            .unwrap()
            .is_empty());
        registry(tmp.path(), "codex-one", "Case-Worker", "implementer", false);
        crate::mailbox_cmd::handle_mailbox_command(&command(" case-worker "), &store).unwrap();
        let messages = crate::mailbox_store::read_local_messages(tmp.path()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].to,
            aida_core::mailbox::Recipient::Agent("Case-Worker".into())
        );
    }

    // An old-layout send against a migrated clone exits nonzero and leaves
    // the manifest and its directory exactly as they were.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn cli_send_refuses_at_a_layout_manifest_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("registry")).unwrap();
        std::fs::write(store.join("registry/team.toml"), "[members]\njoe = []\n").unwrap();
        let aida = tmp.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        let manifest = crate::mailbox_store::mailbox_dir(tmp.path());
        let body = format!(
            "{}\nlayout = 2\n",
            aida_core::mailbox::MAILBOX_LAYOUT_MANIFEST_MARKER
        );
        std::fs::write(&manifest, &body).unwrap();
        let listing = || {
            let mut names: Vec<_> = std::fs::read_dir(&aida)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            names
        };
        let before = listing();
        let send = crate::cli::MailboxCommand::Send {
            to: Some("person:joe".into()),
            broadcast: false,
            body: Some("layout probe".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: None,
            from: Some("sender".into()),
            urgent: false,
            intent: "fyi".into(),
            relayed_from: None,
            allow_second_person: false,
            queue_for_role: false,
        };
        let error = crate::mailbox_cmd::handle_mailbox_command(&send, &store).unwrap_err();
        let chain = format!("{error:#}");
        assert!(chain.contains("migrated to layout 2"), "{chain}");
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), body);
        assert_eq!(listing(), before);
    }

    #[test]
    fn role_shell_rows_are_not_mailbox_readers() {
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let child = Child(
            std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap(),
        );
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "shell-one", "role:advisor", "advisor", false);
        let path = agent_registry::agents_dir(tmp.path()).join("shell-one.toml");
        let body = std::fs::read_to_string(&path)
            .unwrap()
            .replace("source = 'agent-launcher'", "source = 'role-enter'");
        std::fs::write(path, body).unwrap();
        crate::agent_registry::register_existing_agent(
            tmp.path(),
            "codex",
            child.0.id(),
            "advisor".into(),
            None,
            tmp.path().to_path_buf(),
            Some("real-reader".into()),
            None,
        )
        .unwrap();
        let shell: AgentRegistryEntry = toml::from_str(
            &std::fs::read_to_string(agent_registry::agents_dir(tmp.path()).join("shell-one.toml"))
                .unwrap(),
        )
        .unwrap();
        assert!(
            agent_registry::registry_entry_is_live(tmp.path(), &shell),
            "fixture must reproduce a live role shell"
        );
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        assert!(d.resolve("shell-one").is_err());
        let note = d.resolve("advisor").unwrap().reader_notice.unwrap();
        assert!(note.starts_with("1 live, 0 unverified:"), "{note}");
        assert!(note.contains("real-reader") && !note.contains("role:advisor"));
    }

    #[test]
    fn dead_aliases_do_not_shadow_people_or_active_roles() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("registry")).unwrap();
        std::fs::write(
            store.join("registry/team.toml"),
            "[members]\nJoe = ['advisor']\n",
        )
        .unwrap();
        registry(tmp.path(), "codex-old-human", "joe", "reviewer", true);
        registry(
            tmp.path(),
            "codex-old-role",
            "implementer",
            "reviewer",
            true,
        );
        registry(
            tmp.path(),
            "codex-current",
            "current-worker",
            "implementer",
            false,
        );
        let d = RecipientDirectory::load(tmp.path(), &store).unwrap();
        assert_eq!(d.resolve("joe").unwrap().address, "Joe");
        assert_eq!(d.resolve("implementer").unwrap().address, "implementer");
    }

    #[test]
    fn supported_web_vendor_keeps_live_group_routing() {
        let tmp = tempfile::tempdir().unwrap();
        crate::agent_registry::register_existing_agent(
            tmp.path(),
            "web",
            std::process::id(),
            "implementer".into(),
            None,
            tmp.path().to_path_buf(),
            Some("web-one".into()),
            None,
        )
        .unwrap();
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        let result = d.resolve("web").unwrap();
        assert_eq!(result.address, "web");
        assert!(result.reader_notice.unwrap().contains("web-one"));
    }

    #[test]
    fn registry_id_cannot_bypass_invalid_inbox_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        registry(tmp.path(), "codex-one", "-include", "implementer", false);
        let d = RecipientDirectory::load(tmp.path(), &tmp.path().join(".aida-store")).unwrap();
        assert!(d.resolve("codex-one").is_err());
    }

    #[test]
    fn historical_catalog_preserves_precedence_while_live_catalog_addresses_win() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("registry")).unwrap();
        std::fs::write(
            store.join("registry/team.toml"),
            "[members]\nJoe = ['advisor']\nAdvisor = []\n",
        )
        .unwrap();
        registry(tmp.path(), "codex-one", "joe", "implementer", false);
        registry(
            tmp.path(),
            "codex-advisor",
            "advisor-reader",
            "advisor",
            false,
        );
        let historical = crate::known_mailbox_recipients(&store);
        assert_eq!(
            historical["joe"], "joe",
            "registry spelling keeps its original precedence"
        );
        assert_eq!(
            historical["advisor"], "advisor",
            "role spelling keeps its original precedence"
        );
        let live = RecipientDirectory::load(tmp.path(), &store).unwrap();
        assert_eq!(live.resolve("joe").unwrap().address, "Joe");
        let role = live.resolve("advisor").unwrap();
        assert_eq!(role.address, "advisor");
        assert!(role.reader_notice.unwrap().contains("advisor-reader"));
        let person = live.resolve("person:Advisor").unwrap();
        assert_eq!(person.address, "Advisor");
        assert!(person.reader_notice.is_none());
        assert!(live.resolve("person:missing").is_err());
        assert!(live.resolve("codex-one").is_err());
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn bad_records_warn_without_disabling_humans_or_valid_readers() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        std::fs::create_dir_all(store.join("registry")).unwrap();
        std::fs::write(
            store.join("registry/team.toml"),
            "[members]\nJoe = ['advisor']\n",
        )
        .unwrap();
        let d = RecipientDirectory::load(tmp.path(), &store).unwrap();
        assert_eq!(d.resolve(" joe ").unwrap().address, "Joe");
        let dir = agent_registry::agents_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.toml"), "invalid toml [").unwrap();
        // A directory cannot be read as a registry file, even under a root test runner.
        std::fs::create_dir(dir.join("unreadable.toml")).unwrap();
        registry(
            tmp.path(),
            "codex-worker",
            "worker-one",
            "implementer",
            false,
        );
        let directory = RecipientDirectory::load(tmp.path(), &store).unwrap();
        assert_eq!(directory.warnings().len(), 2);
        assert!(directory
            .warnings()
            .iter()
            .any(|w| w.contains("broken.toml")));
        assert!(directory
            .warnings()
            .iter()
            .any(|w| w.contains("unreadable.toml")));
        let human = directory.resolve("joe").unwrap();
        assert_eq!(human.address, "Joe");
        assert!(human.reader_notice.is_none());
        assert_eq!(
            directory.resolve("worker-one").unwrap().address,
            "worker-one"
        );
        let group = directory.resolve("implementer").unwrap();
        assert!(group
            .reader_notice
            .unwrap()
            .starts_with("1 live, 0 unverified:"));
        assert!(directory.resolve("unknown").is_err());
        assert!(!tmp.path().join(".aida/mailbox").exists());
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn reserved_catalog_addresses_are_not_poisoned_by_agent_names() {
        for (borrowed, canonical, group) in [
            ("advisor", "advisor", true),
            ("codex", "codex", true),
            ("joe", "Joe", false),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let store = tmp.path().join(".aida-store");
            std::fs::create_dir_all(store.join("registry")).unwrap();
            std::fs::write(store.join("registry/team.toml"), "[members]\nJoe = []\n").unwrap();
            registry(tmp.path(), "codex-reserved", borrowed, "advisor", false);
            let directory = RecipientDirectory::load(tmp.path(), &store).unwrap();
            let resolved = directory.resolve(borrowed).unwrap();
            assert_eq!(resolved.address, canonical);
            assert_eq!(resolved.reader_notice.is_some(), group);
            // The ID must not bypass the same reserved-inbox protection.
            assert!(directory.resolve("codex-reserved").is_err());
        }
    }

    // STORY-1843 D1: a role send records one delivery per reader UUID,
    // names a UUID-less reader, and queues only on request.
    fn reserve(project: &Path, handle: &str, registry_id: &str, uuid: &str) {
        let dir = agent_registry::agents_dir(project).join("handles");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{handle}.toml")),
            format!(
                "handle = '{handle}'\nrecipient_id = '{uuid}'\nagent_type = 'codex'\nreserved_at = '2026-10-28T00:00:00Z'\nregistry_id = '{registry_id}'\n"
            ),
        )
        .unwrap();
    }

    fn role_send(target: &str, queue_for_role: bool) -> crate::cli::MailboxCommand {
        crate::cli::MailboxCommand::Send {
            to: Some(target.into()),
            broadcast: false,
            body: Some("cohort probe".into()),
            subject: None,
            body_file: None,
            stdin: false,
            thread: None,
            in_reply_to: None,
            from: Some("sender".into()),
            urgent: false,
            intent: "fyi".into(),
            relayed_from: None,
            allow_second_person: false,
            queue_for_role,
        }
    }

    fn sidecars(project: &Path) -> Vec<aida_core::mailbox_delivery::DeliverySidecar> {
        let dir = crate::mailbox_store::mailbox_dir(project).join(".deliveries");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .map(|p| {
                let stem = p.file_stem().unwrap().to_str().unwrap().to_string();
                aida_core::mailbox_delivery::DeliverySidecar::parse(
                    &std::fs::read(&p).unwrap(),
                    &stem,
                )
                .unwrap()
            })
            .collect()
    }

    // trace:STORY-1843 | ai:claude
    #[test]
    fn story1843_d1_role_send_records_the_uuid_cohort_and_names_uuidless_readers() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        let a = "00000000-0000-4000-8000-00000000000a";
        let b = "00000000-0000-4000-8000-00000000000b";
        registry(tmp.path(), "row-a", "rev-a", "reviewer", false);
        registry(tmp.path(), "row-b", "rev-b", "reviewer", false);
        registry(tmp.path(), "row-old", "rev-old", "reviewer", false);
        reserve(tmp.path(), "rev-a", "row-a", a);
        reserve(tmp.path(), "rev-b", "row-b", b);
        // A reservation naming another row confers no UUID.
        reserve(
            tmp.path(),
            "rev-old",
            "row-elsewhere",
            "00000000-0000-4000-8000-00000000000c",
        );

        let d = RecipientDirectory::load(tmp.path(), &store).unwrap();
        let resolved = d.resolve("reviewer").unwrap();
        let cohort = resolved.cohort.as_ref().unwrap();
        let uuids: Vec<&str> = cohort.readers.iter().map(|u| u.as_str()).collect();
        assert_eq!(uuids, [a, b]);
        assert_eq!(cohort.uuidless, ["rev-old (row-old)"]);
        let (sidecar, warnings) = delivery_sidecar(&resolved, "m1", 7, true).unwrap();
        let sidecar = sidecar.unwrap();
        assert_eq!(sidecar.deliveries.len(), 2);
        assert!(sidecar
            .deliveries
            .iter()
            .all(|r| !r.queued_for_role && r.cohort.as_ref().unwrap().reader_uuids.len() == 2));
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("rev-old (row-old) has no recipient UUID"),
            "{warnings:?}"
        );
        // A handle send and a vendor send carry no cohort.
        assert!(d.resolve("rev-a").unwrap().cohort.is_none());
        assert!(d.resolve("codex").unwrap().cohort.is_none());

        crate::mailbox_cmd::handle_mailbox_command(&role_send("reviewer", false), &store).unwrap();
        let written = sidecars(tmp.path());
        assert_eq!(written.len(), 1);
        let messages = crate::mailbox_store::read_local_messages(tmp.path()).unwrap();
        assert_eq!(messages.len(), 1, "the sidecar is not read as a message");
        assert_eq!(written[0].message_id, messages[0].id);
    }

    // TASK-121: mail to a pool role is taken by exactly one member — it
    // queues instead of fanning out to every live implementer; a handle send
    // still reaches only that member.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude
    #[test]
    fn pool_role_mail_queues_for_one_taker_instead_of_fanning_out() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        let a = "00000000-0000-4000-8000-00000000000a";
        let b = "00000000-0000-4000-8000-00000000000b";
        registry(tmp.path(), "row-a", "impl-a", "implementer", false);
        registry(tmp.path(), "row-b", "impl-b", "implementer", false);
        reserve(tmp.path(), "impl-a", "row-a", a);
        reserve(tmp.path(), "impl-b", "row-b", b);

        let d = RecipientDirectory::load(tmp.path(), &store).unwrap();
        let resolved = d.resolve("implementer").unwrap();
        assert!(resolved.cohort.as_ref().unwrap().queued);
        let (sidecar, _) = delivery_sidecar(&resolved, "m1", 7, true).unwrap();
        let sidecar = sidecar.unwrap();
        assert_eq!(sidecar.deliveries.len(), 1, "one delivery, one taker");
        assert!(sidecar.deliveries[0].queued_for_role);
        assert!(sidecar.deliveries[0].recipient_uuid.is_none());

        // A handle send is not a role send: it reaches only that member.
        assert!(d.resolve("impl-a").unwrap().cohort.is_none());

        // Every live member UUID-less, no --queue-for-role: still queued for
        // one taker, never refused like a non-pool role.
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        registry(tmp.path(), "row-old", "impl-old", "implementer", false);
        let d = RecipientDirectory::load(tmp.path(), &store).unwrap();
        let resolved = d.resolve("implementer").unwrap();
        let cohort = resolved.cohort.as_ref().unwrap();
        assert!(cohort.readers.is_empty() && !cohort.uuidless.is_empty());
        assert!(cohort.queued);
        let (sidecar, warnings) = delivery_sidecar(&resolved, "m2", 7, true).unwrap();
        let sidecar = sidecar.unwrap();
        assert_eq!(sidecar.deliveries.len(), 1);
        assert!(sidecar.deliveries[0].queued_for_role);
        assert!(
            warnings.iter().any(|w| w.contains("impl-old")),
            "{warnings:?}"
        );
    }

    // trace:STORY-1843 | ai:claude
    #[test]
    fn story1843_d1_empty_cohort_refuses_unless_queued_for_a_role() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".aida-store");
        // Only a UUID-less reader: the cohort is empty, so the send refuses.
        registry(tmp.path(), "row-old", "rev-old", "reviewer", false);
        let err = crate::mailbox_cmd::handle_mailbox_command(&role_send("reviewer", false), &store)
            .unwrap_err()
            .to_string();
        assert!(err.contains("0 readers with a recipient UUID"), "{err}");
        assert!(
            err.contains("--queue-for-role"),
            "the CLI names its flag: {err}"
        );
        assert!(crate::mailbox_store::read_local_messages(tmp.path())
            .unwrap()
            .is_empty());
        // MCP callers cannot pass the flag, so their refusal never names it.
        let d = RecipientDirectory::load(tmp.path(), &store).unwrap();
        let mcp = delivery_sidecar(&d.resolve("reviewer").unwrap(), "m0", 1, false)
            .unwrap_err()
            .to_string();
        assert!(mcp.contains("0 readers with a recipient UUID"), "{mcp}");
        assert!(!mcp.contains("--queue-for-role"), "{mcp}");

        // Queued, an all-UUID-less role queues for the first reader with a
        // UUID, and the warning names the readers that will not take it.
        let queued = d.resolve_send("reviewer", true).unwrap();
        let (sidecar, warnings) = delivery_sidecar(&queued, "m0", 1, true).unwrap();
        let sidecar = sidecar.unwrap();
        assert!(sidecar.deliveries[0].queued_for_role);
        assert!(
            warnings[0].starts_with("queued for 'reviewer'"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("rev-old (row-old) has no recipient UUID and will not take this"),
            "{warnings:?}"
        );
        crate::mailbox_cmd::handle_mailbox_command(&role_send("reviewer", true), &store).unwrap();
        let written = sidecars(tmp.path());
        assert_eq!(written.len(), 1);
        assert!(written[0].deliveries[0].queued_for_role);
        std::fs::remove_dir_all(crate::mailbox_store::mailbox_dir(tmp.path())).unwrap();

        // No reader at all: refused by default, one pending record when queued.
        let err =
            crate::mailbox_cmd::handle_mailbox_command(&role_send("implementer", false), &store)
                .unwrap_err()
                .to_string();
        assert!(err.contains("0 live readers"), "{err}");
        assert!(
            err.contains("--queue-for-role"),
            "the CLI names its flag: {err}"
        );
        let mcp = d.resolve("implementer").unwrap_err().to_string();
        assert!(mcp.contains("0 live readers"), "{mcp}");
        assert!(!mcp.contains("--queue-for-role"), "{mcp}");
        crate::mailbox_cmd::handle_mailbox_command(&role_send("implementer", true), &store)
            .unwrap();
        let written = sidecars(tmp.path());
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].deliveries.len(), 1);
        assert!(written[0].deliveries[0].queued_for_role);
        assert!(written[0].deliveries[0].recipient_uuid.is_none());

        // Queuing applies only to a role.
        registry(tmp.path(), "row-h", "handle-x", "implementer", false);
        assert!(
            crate::mailbox_cmd::handle_mailbox_command(&role_send("handle-x", true), &store)
                .unwrap_err()
                .to_string()
                .contains("applies only to a role")
        );
    }
}
