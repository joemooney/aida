// trace:ARCH-observability | ai:claude
//! Telemetry — track AIDA usage for measuring tool effectiveness.
//!
//! Records events like requirement creation, lookups, status changes,
//! skill invocations, and AI interactions. Stored in the requirements
//! database itself (dogfooding) so the data is always available for
//! analysis alongside the requirements it describes.
//!
//! This is NOT analytics sent to a server. All data stays local.
//! The purpose is to answer: "Is this tool actually being used, and how?"
//!
//! Key metrics this enables:
//! - Requirements created per day/week
//! - Requirements referenced in commits (traceability coverage)
//! - Skill invocations (which skills are used most?)
//! - Time from creation to completion (cycle time)
//! - AI evaluation scores over time (quality trends)
//! - Active users (who is contributing?)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A telemetry event — a single recorded action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    /// Event ID
    pub id: Uuid,
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// Who triggered it (user handle or "system")
    pub actor: String,
    /// What happened
    pub kind: EventKind,
    /// Optional requirement ID this event relates to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<String>,
}

/// Categories of tracked events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventKind {
    // Requirement lifecycle
    RequirementCreated,
    RequirementUpdated {
        fields: Vec<String>,
    },
    RequirementStatusChanged {
        from: String,
        to: String,
    },
    RequirementDeleted,
    RequirementViewed,

    // Traceability
    TraceCommentAdded {
        file: String,
    },
    CommitLinked {
        commit_sha: String,
    },

    // Skills
    SkillInvoked {
        skill: String,
    },

    // AI
    AiEvaluationRun {
        score: Option<f32>,
    },
    AiChatQuery,

    // Sync
    GitSyncPush,
    GitSyncPull,
    GitHubPush {
        issue_number: u64,
    },
    GitHubPull {
        count: u32,
    },

    // Search
    SearchPerformed {
        query: String,
        results: u32,
    },

    // Session
    SessionStart,
    SessionEnd {
        duration_secs: u64,
    },

    // Reviews
    ReviewCompleted {
        total_issues: usize,
        critical: usize,
        important: usize,
        minor: usize,
    },
}

/// A telemetry store — append-only event log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetryStore {
    pub events: Vec<TelemetryEvent>,
}

impl TelemetryStore {
    /// Record a new event.
    pub fn record(&mut self, actor: &str, kind: EventKind, requirement_id: Option<&str>) {
        self.events.push(TelemetryEvent {
            id: Uuid::now_v7(),
            timestamp: Utc::now(),
            actor: actor.to_string(),
            kind,
            requirement_id: requirement_id.map(String::from),
        });
    }

    /// Get events in a time range.
    pub fn events_between(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<&TelemetryEvent> {
        self.events
            .iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .collect()
    }

    /// Count events by kind in a time range.
    pub fn count_by_kind(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> std::collections::HashMap<String, u32> {
        let mut counts = std::collections::HashMap::new();
        for event in self.events_between(start, end) {
            let kind_name = match &event.kind {
                EventKind::RequirementCreated => "created",
                EventKind::RequirementUpdated { .. } => "updated",
                EventKind::RequirementStatusChanged { .. } => "status_changed",
                EventKind::RequirementDeleted => "deleted",
                EventKind::RequirementViewed => "viewed",
                EventKind::TraceCommentAdded { .. } => "trace_added",
                EventKind::CommitLinked { .. } => "commit_linked",
                EventKind::SkillInvoked { .. } => "skill_invoked",
                EventKind::AiEvaluationRun { .. } => "ai_evaluation",
                EventKind::AiChatQuery => "ai_chat",
                EventKind::GitSyncPush => "git_push",
                EventKind::GitSyncPull => "git_pull",
                EventKind::GitHubPush { .. } => "github_push",
                EventKind::GitHubPull { .. } => "github_pull",
                EventKind::SearchPerformed { .. } => "search",
                EventKind::SessionStart => "session_start",
                EventKind::SessionEnd { .. } => "session_end",
                EventKind::ReviewCompleted { .. } => "review_completed",
            };
            *counts.entry(kind_name.to_string()).or_insert(0) += 1;
        }
        counts
    }

    /// Get the most invoked skills.
    pub fn top_skills(&self, limit: usize) -> Vec<(String, u32)> {
        let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for event in &self.events {
            if let EventKind::SkillInvoked { skill } = &event.kind {
                *counts.entry(skill.clone()).or_insert(0) += 1;
            }
        }
        let mut sorted: Vec<_> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.truncate(limit);
        sorted
    }

    /// Calculate average cycle time (creation → completion) in hours.
    pub fn avg_cycle_time_hours(&self) -> Option<f64> {
        let mut creation_times: std::collections::HashMap<String, DateTime<Utc>> =
            std::collections::HashMap::new();
        let mut cycle_times = Vec::new();

        for event in &self.events {
            if let Some(ref req_id) = event.requirement_id {
                match &event.kind {
                    EventKind::RequirementCreated => {
                        creation_times.insert(req_id.clone(), event.timestamp);
                    }
                    EventKind::RequirementStatusChanged { to, .. }
                        if to == "Completed" || to == "completed" =>
                    {
                        if let Some(created) = creation_times.get(req_id) {
                            let duration = event.timestamp - *created;
                            cycle_times.push(duration.num_hours() as f64);
                        }
                    }
                    _ => {}
                }
            }
        }

        if cycle_times.is_empty() {
            None
        } else {
            let sum: f64 = cycle_times.iter().sum();
            Some(sum / cycle_times.len() as f64)
        }
    }

    /// Generate a usage summary report.
    pub fn summary(&self) -> UsageSummary {
        let total_events = self.events.len();
        let unique_actors: std::collections::HashSet<&str> =
            self.events.iter().map(|e| e.actor.as_str()).collect();
        let unique_requirements: std::collections::HashSet<&str> = self
            .events
            .iter()
            .filter_map(|e| e.requirement_id.as_deref())
            .collect();

        UsageSummary {
            total_events,
            unique_actors: unique_actors.len(),
            unique_requirements: unique_requirements.len(),
            top_skills: self.top_skills(5),
            avg_cycle_time_hours: self.avg_cycle_time_hours(),
            first_event: self.events.first().map(|e| e.timestamp),
            last_event: self.events.last().map(|e| e.timestamp),
        }
    }

    /// Save to a YAML file.
    #[cfg(feature = "native")]
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let content = serde_yaml::to_string(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load from a YAML file.
    #[cfg(feature = "native")]
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&content)?)
    }
}

/// Summary of usage metrics.
#[derive(Debug, Clone, Serialize)]
pub struct UsageSummary {
    pub total_events: usize,
    pub unique_actors: usize,
    pub unique_requirements: usize,
    pub top_skills: Vec<(String, u32)>,
    pub avg_cycle_time_hours: Option<f64>,
    pub first_event: Option<DateTime<Utc>>,
    pub last_event: Option<DateTime<Utc>>,
}

impl std::fmt::Display for UsageSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "AIDA Usage Summary")?;
        writeln!(f, "─────────────────────────────────")?;
        writeln!(f, "Total events:        {}", self.total_events)?;
        writeln!(f, "Unique actors:       {}", self.unique_actors)?;
        writeln!(f, "Requirements touched: {}", self.unique_requirements)?;
        if let Some(first) = self.first_event {
            writeln!(f, "First event:         {}", first.format("%Y-%m-%d"))?;
        }
        if let Some(last) = self.last_event {
            writeln!(f, "Last event:          {}", last.format("%Y-%m-%d"))?;
        }
        if let Some(cycle) = self.avg_cycle_time_hours {
            writeln!(f, "Avg cycle time:      {:.1} hours", cycle)?;
        }
        if !self.top_skills.is_empty() {
            writeln!(f, "Top skills:")?;
            for (skill, count) in &self.top_skills {
                writeln!(f, "  {:20} {}", skill, count)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_count() {
        let mut store = TelemetryStore::default();

        store.record("joe", EventKind::RequirementCreated, Some("FR-001"));
        store.record("joe", EventKind::RequirementCreated, Some("FR-002"));
        store.record(
            "joe",
            EventKind::SkillInvoked {
                skill: "aida-req".into(),
            },
            None,
        );
        store.record("alice", EventKind::RequirementViewed, Some("FR-001"));

        assert_eq!(store.events.len(), 4);

        let summary = store.summary();
        assert_eq!(summary.total_events, 4);
        assert_eq!(summary.unique_actors, 2);
        assert_eq!(summary.unique_requirements, 2);
    }

    #[test]
    fn test_top_skills() {
        let mut store = TelemetryStore::default();

        for _ in 0..5 {
            store.record(
                "joe",
                EventKind::SkillInvoked {
                    skill: "aida-req".into(),
                },
                None,
            );
        }
        for _ in 0..3 {
            store.record(
                "joe",
                EventKind::SkillInvoked {
                    skill: "aida-commit".into(),
                },
                None,
            );
        }
        store.record(
            "joe",
            EventKind::SkillInvoked {
                skill: "aida-grill".into(),
            },
            None,
        );

        let top = store.top_skills(2);
        assert_eq!(top[0].0, "aida-req");
        assert_eq!(top[0].1, 5);
        assert_eq!(top[1].0, "aida-commit");
        assert_eq!(top[1].1, 3);
    }

    #[test]
    fn test_cycle_time() {
        let mut store = TelemetryStore::default();

        // Simulate: create at T, complete at T+2h
        store.record("joe", EventKind::RequirementCreated, Some("FR-001"));

        // Manually adjust timestamp for completion
        let completion = TelemetryEvent {
            id: Uuid::now_v7(),
            timestamp: Utc::now() + chrono::Duration::hours(2),
            actor: "joe".into(),
            kind: EventKind::RequirementStatusChanged {
                from: "Draft".into(),
                to: "Completed".into(),
            },
            requirement_id: Some("FR-001".into()),
        };
        store.events.push(completion);

        let cycle = store.avg_cycle_time_hours();
        assert!(cycle.is_some());
        assert!(cycle.unwrap() >= 1.0); // at least 1 hour (close to 2)
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telemetry.yaml");

        let mut store = TelemetryStore::default();
        store.record("joe", EventKind::RequirementCreated, Some("FR-001"));
        store.record(
            "joe",
            EventKind::SkillInvoked {
                skill: "aida-req".into(),
            },
            None,
        );
        store.save(&path).unwrap();

        let loaded = TelemetryStore::load(&path).unwrap();
        assert_eq!(loaded.events.len(), 2);
    }
}
