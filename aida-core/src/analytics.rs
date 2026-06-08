// trace:ARCH-analytics | ai:claude
//! Analytics engine — deeper metrics for requirements management.
//!
//! Computes velocity trends, requirement churn, AI contribution metrics,
//! quality score trends, cycle time distributions, and more from the
//! existing requirement data (history entries, AI evaluations, timestamps).

#[cfg(test)]
use chrono::Duration;
use chrono::{DateTime, Datelike, Utc};
use serde::Serialize;
use std::collections::HashMap;

use crate::models::{Requirement, RequirementStatus};

/// Full analytics report.
#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsReport {
    /// When this report was generated
    pub generated_at: DateTime<Utc>,
    /// Total requirements
    pub total_requirements: usize,
    /// Requirements by status
    pub status_distribution: HashMap<String, usize>,
    /// Requirements by type
    pub type_distribution: HashMap<String, usize>,
    /// Velocity trend (requirements completed per week)
    pub velocity_trend: Vec<TimeBucket>,
    /// Creation trend (requirements created per week)
    pub creation_trend: Vec<TimeBucket>,
    /// Churn metrics
    pub churn: ChurnMetrics,
    /// Cycle time stats
    pub cycle_time: CycleTimeStats,
    /// AI contribution metrics
    pub ai_metrics: AiMetrics,
    /// Quality score trends
    pub quality_trend: Vec<QualityPoint>,
    /// Top contributors by requirement changes
    pub top_contributors: Vec<(String, usize)>,
    /// Traceability coverage
    pub traceability: TraceabilityMetrics,
}

/// A time bucket with a count value.
#[derive(Debug, Clone, Serialize)]
pub struct TimeBucket {
    pub period: String, // "2026-W12" or "2026-03"
    pub count: usize,
}

/// Churn metrics — how much requirements change after creation.
#[derive(Debug, Clone, Serialize)]
pub struct ChurnMetrics {
    /// Total field changes across all requirements
    pub total_changes: usize,
    /// Requirements that have never been modified after creation
    pub stable_count: usize,
    /// Requirements modified more than 5 times
    pub high_churn_count: usize,
    /// Average changes per requirement
    pub avg_changes_per_req: f64,
    /// Most churned requirements (spec_id, change_count)
    pub most_churned: Vec<(String, usize)>,
}

/// Cycle time statistics.
#[derive(Debug, Clone, Serialize)]
pub struct CycleTimeStats {
    /// Average hours from creation to completion
    pub avg_hours: Option<f64>,
    /// Median hours
    pub median_hours: Option<f64>,
    /// 90th percentile hours
    pub p90_hours: Option<f64>,
    /// Fastest completion (hours)
    pub min_hours: Option<f64>,
    /// Slowest completion (hours)
    pub max_hours: Option<f64>,
    /// Number of completed requirements with timing data
    pub sample_count: usize,
}

/// AI contribution metrics.
#[derive(Debug, Clone, Serialize)]
pub struct AiMetrics {
    /// Requirements with trace comments (ai:claude or similar)
    pub ai_traced_count: usize,
    /// Requirements with AI evaluations
    pub ai_evaluated_count: usize,
    /// Average AI quality score (0-100)
    pub avg_quality_score: Option<f64>,
    /// Quality score distribution (buckets of 10)
    pub score_distribution: HashMap<String, usize>,
    /// Requirements with stale evaluations (content changed since eval)
    pub stale_evaluations: usize,
}

/// Quality score over time.
#[derive(Debug, Clone, Serialize)]
pub struct QualityPoint {
    pub period: String,
    pub avg_score: f64,
    pub count: usize,
}

/// Traceability coverage metrics.
#[derive(Debug, Clone, Serialize)]
pub struct TraceabilityMetrics {
    /// Total requirements
    pub total: usize,
    /// Requirements with at least one trace link
    pub with_trace_links: usize,
    /// Coverage percentage
    pub coverage_pct: f64,
    /// Requirements with commit references in comments
    pub with_commit_refs: usize,
}

/// Compute the full analytics report from a set of requirements.
pub fn compute_analytics(requirements: &[Requirement]) -> AnalyticsReport {
    AnalyticsReport {
        generated_at: Utc::now(),
        total_requirements: requirements.len(),
        status_distribution: compute_status_distribution(requirements),
        type_distribution: compute_type_distribution(requirements),
        velocity_trend: compute_velocity_trend(requirements),
        creation_trend: compute_creation_trend(requirements),
        churn: compute_churn(requirements),
        cycle_time: compute_cycle_time(requirements),
        ai_metrics: compute_ai_metrics(requirements),
        quality_trend: compute_quality_trend(requirements),
        top_contributors: compute_top_contributors(requirements),
        traceability: compute_traceability(requirements),
    }
}

fn compute_status_distribution(reqs: &[Requirement]) -> HashMap<String, usize> {
    let mut dist = HashMap::new();
    for req in reqs {
        *dist.entry(req.effective_status()).or_insert(0) += 1;
    }
    dist
}

fn compute_type_distribution(reqs: &[Requirement]) -> HashMap<String, usize> {
    let mut dist = HashMap::new();
    for req in reqs {
        *dist.entry(format!("{:?}", req.req_type)).or_insert(0) += 1;
    }
    dist
}

fn compute_velocity_trend(reqs: &[Requirement]) -> Vec<TimeBucket> {
    // Count requirements completed per week (based on modified_at for completed items)
    let mut weekly: HashMap<String, usize> = HashMap::new();

    for req in reqs {
        if matches!(req.status, RequirementStatus::Completed) {
            let week = format!(
                "{}-W{:02}",
                req.modified_at.year(),
                req.modified_at.iso_week().week()
            );
            *weekly.entry(week).or_insert(0) += 1;
        }
    }

    let mut trend: Vec<TimeBucket> = weekly
        .into_iter()
        .map(|(period, count)| TimeBucket { period, count })
        .collect();
    trend.sort_by(|a, b| a.period.cmp(&b.period));

    // Keep last 12 weeks
    if trend.len() > 12 {
        trend = trend.split_off(trend.len() - 12);
    }
    trend
}

fn compute_creation_trend(reqs: &[Requirement]) -> Vec<TimeBucket> {
    let mut weekly: HashMap<String, usize> = HashMap::new();

    for req in reqs {
        let week = format!(
            "{}-W{:02}",
            req.created_at.year(),
            req.created_at.iso_week().week()
        );
        *weekly.entry(week).or_insert(0) += 1;
    }

    let mut trend: Vec<TimeBucket> = weekly
        .into_iter()
        .map(|(period, count)| TimeBucket { period, count })
        .collect();
    trend.sort_by(|a, b| a.period.cmp(&b.period));

    if trend.len() > 12 {
        trend = trend.split_off(trend.len() - 12);
    }
    trend
}

fn compute_churn(reqs: &[Requirement]) -> ChurnMetrics {
    let mut total_changes = 0;
    let mut stable_count = 0;
    let mut high_churn_count = 0;
    let mut per_req_changes: Vec<(String, usize)> = Vec::new();

    for req in reqs {
        let changes = req.history.len();
        total_changes += changes;

        if changes == 0 {
            stable_count += 1;
        }
        if changes > 5 {
            high_churn_count += 1;
        }

        per_req_changes.push((
            req.spec_id.clone().unwrap_or_else(|| req.id.to_string()),
            changes,
        ));
    }

    per_req_changes.sort_by(|a, b| b.1.cmp(&a.1));
    let most_churned = per_req_changes.into_iter().take(10).collect();

    let avg = if reqs.is_empty() {
        0.0
    } else {
        total_changes as f64 / reqs.len() as f64
    };

    ChurnMetrics {
        total_changes,
        stable_count,
        high_churn_count,
        avg_changes_per_req: avg,
        most_churned,
    }
}

fn compute_cycle_time(reqs: &[Requirement]) -> CycleTimeStats {
    let mut cycle_times: Vec<f64> = Vec::new();

    for req in reqs {
        if matches!(req.status, RequirementStatus::Completed) {
            let hours = (req.modified_at - req.created_at).num_hours() as f64;
            if hours >= 0.0 {
                cycle_times.push(hours);
            }
        }
    }

    if cycle_times.is_empty() {
        return CycleTimeStats {
            avg_hours: None,
            median_hours: None,
            p90_hours: None,
            min_hours: None,
            max_hours: None,
            sample_count: 0,
        };
    }

    cycle_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let sum: f64 = cycle_times.iter().sum();
    let avg = sum / cycle_times.len() as f64;
    let median = cycle_times[cycle_times.len() / 2];
    let p90_idx = (cycle_times.len() as f64 * 0.9) as usize;
    let p90 = cycle_times[p90_idx.min(cycle_times.len() - 1)];

    CycleTimeStats {
        avg_hours: Some(avg),
        median_hours: Some(median),
        p90_hours: Some(p90),
        min_hours: cycle_times.first().copied(),
        max_hours: cycle_times.last().copied(),
        sample_count: cycle_times.len(),
    }
}

fn compute_ai_metrics(reqs: &[Requirement]) -> AiMetrics {
    let mut ai_traced = 0;
    let mut ai_evaluated = 0;
    let mut scores: Vec<f64> = Vec::new();
    let mut score_dist: HashMap<String, usize> = HashMap::new();
    let mut stale = 0;

    for req in reqs {
        // Check for AI trace links
        if req
            .trace_links
            .iter()
            .any(|t| t.notes.as_deref().unwrap_or("").contains("ai:"))
        {
            ai_traced += 1;
        }

        // Check AI evaluation
        if let Some(ref eval) = req.ai_evaluation {
            ai_evaluated += 1;
            let score = eval.evaluation.quality_score as f64;
            scores.push(score);

            // Bucket: 0-10, 10-20, ..., 90-100
            let bucket = format!(
                "{}-{}",
                (score as u32 / 10) * 10,
                ((score as u32 / 10) + 1) * 10
            );
            *score_dist.entry(bucket).or_insert(0) += 1;

            // Check staleness
            if eval.content_hash.is_empty() {
                stale += 1;
            }
        }
    }

    let avg_score = if scores.is_empty() {
        None
    } else {
        Some(scores.iter().sum::<f64>() / scores.len() as f64)
    };

    AiMetrics {
        ai_traced_count: ai_traced,
        ai_evaluated_count: ai_evaluated,
        avg_quality_score: avg_score,
        score_distribution: score_dist,
        stale_evaluations: stale,
    }
}

fn compute_quality_trend(reqs: &[Requirement]) -> Vec<QualityPoint> {
    // Group AI evaluation scores by month
    let mut monthly: HashMap<String, (f64, usize)> = HashMap::new();

    for req in reqs {
        if let Some(ref eval) = req.ai_evaluation {
            let month = format!(
                "{}-{:02}",
                eval.evaluated_at.year(),
                eval.evaluated_at.month()
            );
            let entry = monthly.entry(month).or_insert((0.0, 0));
            entry.0 += eval.evaluation.quality_score as f64;
            entry.1 += 1;
        }
    }

    let mut trend: Vec<QualityPoint> = monthly
        .into_iter()
        .map(|(period, (sum, count))| QualityPoint {
            period,
            avg_score: sum / count as f64,
            count,
        })
        .collect();
    trend.sort_by(|a, b| a.period.cmp(&b.period));
    trend
}

fn compute_top_contributors(reqs: &[Requirement]) -> Vec<(String, usize)> {
    let mut contributions: HashMap<String, usize> = HashMap::new();

    for req in reqs {
        // Count by owner
        if !req.owner.is_empty() {
            *contributions.entry(req.owner.clone()).or_insert(0) += 1;
        }
        // Count by history authors
        for entry in &req.history {
            if !entry.author.is_empty() {
                *contributions.entry(entry.author.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut sorted: Vec<_> = contributions.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(10);
    sorted
}

fn compute_traceability(reqs: &[Requirement]) -> TraceabilityMetrics {
    let total = reqs.len();
    let with_trace = reqs.iter().filter(|r| !r.trace_links.is_empty()).count();
    let with_commits = reqs
        .iter()
        .filter(|r| {
            r.comments
                .iter()
                .any(|c| c.content.contains("Committed in") || c.content.contains("commit"))
        })
        .count();

    TraceabilityMetrics {
        total,
        with_trace_links: with_trace,
        coverage_pct: if total > 0 {
            (with_trace as f64 / total as f64) * 100.0
        } else {
            0.0
        },
        with_commit_refs: with_commits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(title: &str, status: RequirementStatus, days_ago_created: i64) -> Requirement {
        let mut req = Requirement::new(title.into(), "desc".into());
        req.status = status;
        req.created_at = Utc::now() - Duration::days(days_ago_created);
        req.modified_at = Utc::now();
        req.spec_id = Some(format!("FR-{:03}", days_ago_created));
        req
    }

    #[test]
    fn test_status_distribution() {
        let reqs = vec![
            make_req("A", RequirementStatus::Draft, 10),
            make_req("B", RequirementStatus::Draft, 9),
            make_req("C", RequirementStatus::Completed, 8),
            make_req("D", RequirementStatus::Approved, 7),
        ];
        let dist = compute_status_distribution(&reqs);
        assert_eq!(dist.get("Draft"), Some(&2));
        assert_eq!(dist.get("Completed"), Some(&1));
        assert_eq!(dist.get("Approved"), Some(&1));
    }

    #[test]
    fn test_cycle_time() {
        let reqs = vec![
            make_req("Fast", RequirementStatus::Completed, 1),
            make_req("Slow", RequirementStatus::Completed, 30),
            make_req("Draft", RequirementStatus::Draft, 5),
        ];
        let ct = compute_cycle_time(&reqs);
        assert_eq!(ct.sample_count, 2);
        assert!(ct.avg_hours.is_some());
        assert!(ct.min_hours.unwrap() < ct.max_hours.unwrap());
    }

    #[test]
    fn test_churn() {
        let mut req = make_req("Churny", RequirementStatus::Draft, 5);
        for i in 0..10 {
            req.history.push(crate::models::HistoryEntry {
                id: uuid::Uuid::now_v7(),
                timestamp: Utc::now(),
                author: "joe".into(),
                changes: vec![crate::models::FieldChange {
                    field_name: "title".into(),
                    old_value: format!("v{}", i),
                    new_value: format!("v{}", i + 1),
                }],
            });
        }

        let reqs = vec![req, make_req("Stable", RequirementStatus::Draft, 3)];
        let churn = compute_churn(&reqs);
        assert_eq!(churn.total_changes, 10);
        assert_eq!(churn.stable_count, 1);
        assert_eq!(churn.high_churn_count, 1);
    }

    #[test]
    fn test_full_report() {
        let reqs = vec![
            make_req("A", RequirementStatus::Completed, 10),
            make_req("B", RequirementStatus::Draft, 5),
            make_req("C", RequirementStatus::Approved, 3),
        ];
        let report = compute_analytics(&reqs);
        assert_eq!(report.total_requirements, 3);
        assert!(report.cycle_time.sample_count >= 1);
    }

    #[test]
    fn test_traceability() {
        let mut req = make_req("Traced", RequirementStatus::Completed, 5);
        req.trace_links.push(crate::models::TraceLink {
            id: uuid::Uuid::now_v7(),
            artifact_type: crate::models::ArtifactType::SourceCode,
            file_path: "src/main.rs".into(),
            symbol: Some("validate".into()),
            line_start: Some(10),
            line_end: Some(20),
            notes: Some("ai:claude".into()),
            created_at: Utc::now(),
            created_by: Some("test".into()),
            commit_hash: None,
        });

        let reqs = vec![req, make_req("Untraced", RequirementStatus::Draft, 3)];
        let trace = compute_traceability(&reqs);
        assert_eq!(trace.total, 2);
        assert_eq!(trace.with_trace_links, 1);
        assert!((trace.coverage_pct - 50.0).abs() < 0.1);
    }
}
