//! Live delivery resolution shared by explicit mail and comment mentions.
//! Historical mailbox inspection deliberately does not require a live reader.
// trace:STORY-1843 | ai:codex
use crate::agent_registry::{self, AgentRegistryEntry};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub(crate) struct RecipientDirectory {
    agents: Vec<AgentRegistryEntry>,
    roles: BTreeMap<String, String>,
    people: HashMap<String, String>,
    live_ids: BTreeSet<String>,
    warnings: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct ResolvedRecipient {
    pub(crate) address: String,
    pub(crate) reader_notice: Option<String>,
}

impl RecipientDirectory {
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
        let live_ids = agents
            .iter()
            .filter(|a| {
                a.source != "role-enter" && agent_registry::registry_entry_is_live(project, a)
            })
            .map(|a| a.id.clone())
            .collect();
        let mut roles: BTreeMap<_, _> = crate::AGENT_ROLES
            .iter()
            .map(|r| (r.to_lowercase(), r.to_string()))
            .collect();
        for role in crate::list_roles(project)? {
            roles.insert(role.name.to_lowercase(), role.name);
        }
        warnings.sort();
        Ok(Self {
            agents,
            roles,
            people: crate::known_mailbox_people(store),
            live_ids,
            warnings,
        })
    }

    // These diagnostics are distinct from a group's reader notice: a skipped
    // record never establishes a recipient kind or contributes a live reader.
    // trace:STORY-1843 | ai:codex
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub(crate) fn resolve(&self, target: &str) -> Result<ResolvedRecipient> {
        let target = target.trim();
        if let Some(why) = aida_core::mailbox::recipient_id_rejection(target) {
            bail!("send refused: recipient {why}");
        }
        let key = target.to_lowercase();
        let role = self.roles.get(&key);
        let vendor = match key.as_str() {
            "claude" | "codex" | "antigravity" | "web" => Some(key.as_str()),
            "agy" => Some("antigravity"),
            _ => None,
        };
        let person = self.people.get(&key);
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
            + named.iter().any(|a| self.live_ids.contains(&a.id)) as u8)
            > 1
        {
            bail!("send refused: ambiguous recipient '{target}' names more than one recipient kind; use an unambiguous live agent name");
        }
        if let Some(person) = person {
            return Ok(ResolvedRecipient {
                address: person.clone(),
                reader_notice: None,
            });
        }
        if role.is_some() || vendor.is_some() {
            let readers: BTreeSet<_> = self
                .agents
                .iter()
                .filter(|a| self.live_ids.contains(&a.id))
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
                    a.name
                        .as_deref()
                        .map(|name| format!("{name} ({})", a.id))
                        .unwrap_or_else(|| a.id.clone())
                })
                .collect();
            let address = role.cloned().unwrap_or_else(|| vendor.unwrap().to_string());
            if readers.is_empty() {
                bail!("send refused: '{address}' has 0 live readers; name a live agent handle");
            }
            let warning = if readers.len() > 1 { "warning: " } else { "" };
            return Ok(ResolvedRecipient {
                address,
                reader_notice: Some(format!(
                    "{warning}{} live reader(s): {}",
                    readers.len(),
                    readers.into_iter().collect::<Vec<_>>().join(", ")
                )),
            });
        }
        let live: Vec<_> = named
            .iter()
            .filter(|a| self.live_ids.contains(&a.id))
            .collect();
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
                let owners = self.agents.iter().filter(|a| self.live_ids.contains(&a.id))
                    .filter(|a| a.name.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(name))).count();
                if owners != 1 || self.roles.contains_key(&name.to_lowercase()) || self.people.contains_key(&name.to_lowercase())
                    || matches!(name.to_ascii_lowercase().as_str(), "claude" | "codex" | "antigravity" | "agy" | "web") {
                    bail!("send refused: ambiguous inbox '{name}' for '{target}'; choose a unique private name");
                }
                Ok(ResolvedRecipient { address: name.to_string(), reader_notice: None })
            },
            [] if !named.is_empty() => bail!("send refused: '{target}' has no live agent; choose a live agent handle"),
            [] => bail!("send refused: unknown recipient '{target}'; choose a live agent handle or a known person"),
            _ => bail!("send refused: ambiguous live recipient '{target}'; choose a unique private name"),
        }
    }
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
    }

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
            assert!(d.resolve(name).is_err(), "{name} must refuse");
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
        assert!(note.starts_with("1 live reader"), "{note}");
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
        assert!(group.reader_notice.unwrap().starts_with("1 live reader"));
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
}
