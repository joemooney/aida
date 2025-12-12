// trace:FR-0273 | ai:claude:high
//! Text formatters for proto types
//!
//! These functions convert proto enum values to human-readable strings.

use crate::storage::proto::{RequirementPriority, RequirementStatus, RequirementType, Timestamp};

/// Format a requirement status as a human-readable string
pub fn format_status(status: i32) -> &'static str {
    match RequirementStatus::try_from(status) {
        Ok(RequirementStatus::Draft) => "Draft",
        Ok(RequirementStatus::Approved) => "Approved",
        Ok(RequirementStatus::Planned) => "Planned",
        Ok(RequirementStatus::InProgress) => "In Progress",
        Ok(RequirementStatus::Completed) => "Completed",
        Ok(RequirementStatus::Rejected) => "Rejected",
        _ => "Unknown",
    }
}

/// Format a requirement priority as a human-readable string
pub fn format_priority(priority: i32) -> &'static str {
    match RequirementPriority::try_from(priority) {
        Ok(RequirementPriority::High) => "High",
        Ok(RequirementPriority::Medium) => "Medium",
        Ok(RequirementPriority::Low) => "Low",
        _ => "—",
    }
}

/// Format a requirement type as a human-readable string
pub fn format_type(req_type: i32) -> &'static str {
    match RequirementType::try_from(req_type) {
        Ok(RequirementType::Functional) => "Functional",
        Ok(RequirementType::NonFunctional) => "Non-Functional",
        Ok(RequirementType::System) => "System",
        Ok(RequirementType::User) => "User",
        Ok(RequirementType::ChangeRequest) => "Change Request",
        Ok(RequirementType::Bug) => "Bug",
        Ok(RequirementType::Epic) => "Epic",
        Ok(RequirementType::Story) => "Story",
        Ok(RequirementType::Task) => "Task",
        Ok(RequirementType::Spike) => "Spike",
        Ok(RequirementType::Sprint) => "Sprint",
        Ok(RequirementType::Folder) => "Folder",
        _ => "—",
    }
}

/// Format a timestamp for display
///
/// Returns a human-readable date/time string, or "—" if the timestamp is invalid/empty.
pub fn format_timestamp(ts: &Timestamp) -> String {
    let secs = ts.seconds;
    if secs > 0 {
        // Convert to date components using chrono if available, otherwise simple calculation
        #[cfg(feature = "chrono")]
        {
            use chrono::{TimeZone, Utc};
            if let Some(dt) = Utc.timestamp_opt(secs, ts.nanos as u32).single() {
                return dt.format("%Y-%m-%d %H:%M").to_string();
            }
        }

        // Fallback: simple date calculation
        let days_since_epoch = secs / 86400;
        let years = days_since_epoch / 365 + 1970;
        let remaining_days = days_since_epoch % 365;
        let month = remaining_days / 30 + 1;
        let day = remaining_days % 30 + 1;
        format!("{}-{:02}-{:02}", years, month.min(12), day.min(31))
    } else {
        "—".to_string()
    }
}

/// Format a timestamp as relative time (e.g., "2 hours ago", "yesterday")
pub fn format_relative_time(ts: &Timestamp) -> String {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let diff_secs = now_secs - ts.seconds;

    if diff_secs < 0 {
        return "in the future".to_string();
    }

    if diff_secs < 60 {
        return "just now".to_string();
    }

    let diff_mins = diff_secs / 60;
    if diff_mins < 60 {
        return if diff_mins == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{} minutes ago", diff_mins)
        };
    }

    let diff_hours = diff_mins / 60;
    if diff_hours < 24 {
        return if diff_hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{} hours ago", diff_hours)
        };
    }

    let diff_days = diff_hours / 24;
    if diff_days < 7 {
        return if diff_days == 1 {
            "yesterday".to_string()
        } else {
            format!("{} days ago", diff_days)
        };
    }

    let diff_weeks = diff_days / 7;
    if diff_weeks < 4 {
        return if diff_weeks == 1 {
            "1 week ago".to_string()
        } else {
            format!("{} weeks ago", diff_weeks)
        };
    }

    // Fall back to date format for older timestamps
    format_timestamp(ts)
}

/// Truncate text to a maximum length with ellipsis
pub fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else if max_len <= 3 {
        "...".to_string()
    } else {
        format!("{}...", &text.chars().take(max_len - 3).collect::<String>())
    }
}

/// Get a short status abbreviation (for compact displays)
pub fn status_abbrev(status: i32) -> &'static str {
    match RequirementStatus::try_from(status) {
        Ok(RequirementStatus::Draft) => "DFT",
        Ok(RequirementStatus::Approved) => "APR",
        Ok(RequirementStatus::Planned) => "PLN",
        Ok(RequirementStatus::InProgress) => "WIP",
        Ok(RequirementStatus::Completed) => "DONE",
        Ok(RequirementStatus::Rejected) => "REJ",
        _ => "???",
    }
}

/// Get a short priority abbreviation
pub fn priority_abbrev(priority: i32) -> &'static str {
    match RequirementPriority::try_from(priority) {
        Ok(RequirementPriority::High) => "H",
        Ok(RequirementPriority::Medium) => "M",
        Ok(RequirementPriority::Low) => "L",
        _ => "?",
    }
}

/// Get a short type abbreviation
pub fn type_abbrev(req_type: i32) -> &'static str {
    match RequirementType::try_from(req_type) {
        Ok(RequirementType::Functional) => "FUNC",
        Ok(RequirementType::NonFunctional) => "NF",
        Ok(RequirementType::System) => "SYS",
        Ok(RequirementType::User) => "USR",
        Ok(RequirementType::ChangeRequest) => "CR",
        Ok(RequirementType::Bug) => "BUG",
        Ok(RequirementType::Epic) => "EPIC",
        Ok(RequirementType::Story) => "STOR",
        Ok(RequirementType::Task) => "TASK",
        Ok(RequirementType::Spike) => "SPK",
        Ok(RequirementType::Sprint) => "SPRT",
        Ok(RequirementType::Folder) => "FLD",
        _ => "?",
    }
}
