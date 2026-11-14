//! Review after merge on the integration track (STORY-2772).
//!
//! A low-risk change may merge into `integration` before its review. The
//! review is not skipped: a promotion to `main` is refused while any change
//! it contains lacks an approval at exactly the head that was merged.
//!
//! Everything that decides is here and pure: the `[integration]` table, the
//! one eligibility function, the merge admission and the promotion check.
//! `integration_cmd.rs` supplies the forge, the gate ledger and git.
//!
//! The table is read from the default branch's `.aida/config.toml`, never
//! from the worktree copy: it turns a review requirement off and names shell
//! commands, so a branch must not be able to set it for itself.
// trace:STORY-2772 | ai:claude

use crate::effort_calibration::{effort_from_tags, EffortBucket, EffortTouchpoint};
use crate::gate::mandate_gates::{touches_gates, ProtectedPaths};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The tag on a spec whose change merged into integration before review.
/// Shown in the open list; the promotion check never relies on it.
pub(crate) const MERGED_BEFORE_REVIEW_TAG: &str = "merged-before-review";

/// `[integration] review_after_merge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Off,
    LowRisk,
}

/// The `[integration]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub mode: Mode,
    /// The largest opening effort estimate a spec may carry.
    pub max_effort: EffortBucket,
    /// A spec carrying any of these tags is never eligible.
    pub excluded_tags: Vec<String>,
    /// Shell commands that must each exit 0 on the merge result.
    pub checks: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            max_effort: EffortBucket::FourHours,
            excluded_tags: ["architecture", "security", "gate", "supervised"]
                .map(String::from)
                .to_vec(),
            checks: vec!["make check-ci-fast".to_string()],
        }
    }
}

impl Config {
    /// Parse a config body. No body, or no `[integration]` table, is the
    /// default (off). A value that cannot be read is an error, never "off":
    /// a typo must not quietly change what merges unreviewed.
    pub(crate) fn parse(body: Option<&str>) -> Result<Self, String> {
        let mut config = Self::default();
        let Some(body) = body else {
            return Ok(config);
        };
        let root: toml::Value = toml::from_str(body)
            .map_err(|error| format!("the default branch's .aida/config.toml: {error}"))?;
        let Some(table) = root.get("integration") else {
            return Ok(config);
        };
        let strings = |key: &str| -> Result<Option<Vec<String>>, String> {
            let Some(value) = table.get(key) else {
                return Ok(None);
            };
            value
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().map(String::from))
                        .collect::<Option<Vec<String>>>()
                })
                .map(Some)
                .ok_or_else(|| format!("[integration] {key} must be a list of strings"))
        };
        if let Some(value) = table.get("review_after_merge") {
            config.mode = match value.as_str() {
                Some("off") => Mode::Off,
                Some("low-risk") => Mode::LowRisk,
                _ => return Err(format!(
                    "[integration] review_after_merge must be \"off\" or \"low-risk\", not {value}"
                )),
            };
        }
        if let Some(value) = table.get("max_effort") {
            config.max_effort = value
                .as_str()
                .and_then(EffortBucket::parse_str)
                .ok_or_else(|| {
                    format!(
                        "[integration] max_effort must be one of 15m, 1h, 4h, 1d, 1w, not {value}"
                    )
                })?;
        }
        if let Some(tags) = strings("excluded_tags")? {
            config.excluded_tags = tags;
        }
        if let Some(checks) = strings("checks")? {
            config.checks = checks;
        }
        Ok(config)
    }

    /// The table as the default branch has it, from local refs.
    pub(crate) fn load(root: &Path) -> Result<Self, String> {
        Self::parse(crate::trusted_config::read_trusted_config_toml_local(root).as_deref())
    }
}

/// What eligibility reads of one spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecFacts {
    pub tags: Vec<String>,
    pub mode: Option<aida_core::ExecutionMode>,
}

/// Whether a change may merge into integration before its review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Eligibility {
    /// Why it qualifies.
    Eligible(String),
    /// The first rule it fails.
    Ineligible(String),
}

/// The one eligibility decision. `specs` are the specs the change names,
/// each with its stored facts (`None` when the store has no such spec);
/// `changed_paths` is the change's own diff against its base.
// trace:STORY-2772 | ai:claude
pub(crate) fn review_after_merge_eligible(
    config: &Config,
    specs: &[(String, Option<SpecFacts>)],
    changed_paths: &[String],
    protected: &ProtectedPaths,
) -> Eligibility {
    let no = |why: String| Eligibility::Ineligible(why);
    if config.mode == Mode::Off {
        return no("review after merge is off ([integration] review_after_merge)".into());
    }
    if specs.is_empty() {
        return no("the change names no spec".into());
    }
    let mut efforts = Vec::new();
    for (id, facts) in specs {
        let Some(facts) = facts else {
            return no(format!("{id} is not a spec in the store"));
        };
        if let Some(tag) = facts
            .tags
            .iter()
            .find(|tag| config.excluded_tags.iter().any(|excluded| excluded == *tag))
        {
            return no(format!("{id} is tagged `{tag}`"));
        }
        use aida_core::ExecutionMode as M;
        if let Some(mode @ (M::Guided | M::Operator | M::Decide)) = facts.mode {
            return no(format!(
                "{id} is supervised (execution mode {})",
                format!("{mode:?}").to_lowercase()
            ));
        }
        let Some(effort) = effort_from_tags(&facts.tags, EffortTouchpoint::Open) else {
            return no(format!("{id} has no effort estimate"));
        };
        if effort.minutes() > config.max_effort.minutes() {
            return no(format!(
                "{id} is estimated at {}, above the {} limit",
                effort.as_str(),
                config.max_effort.as_str()
            ));
        }
        efforts.push(format!("{id} {}", effort.as_str()));
    }
    if let Some(hit) = touches_gates(changed_paths, protected) {
        return no(format!("it changes {}", hit.describe()));
    }
    Eligibility::Eligible(format!(
        "low-risk ({}; no excluded tag, no protected path)",
        efforts.join(", ")
    ))
}

/// Reads what eligibility needs, once per pass: the config, the store and
/// the protected paths.
pub(crate) struct Reader {
    root: PathBuf,
    config: std::cell::OnceCell<Result<Config, String>>,
    store: std::cell::OnceCell<Result<aida_core::RequirementsStore, String>>,
}

impl Reader {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            config: Default::default(),
            store: Default::default(),
        }
    }

    pub(crate) fn config(&self) -> Result<&Config, String> {
        self.config
            .get_or_init(|| Config::load(&self.root))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn store(&self) -> Result<&aida_core::RequirementsStore, String> {
        self.store
            .get_or_init(|| super::load_store(&self.root).map_err(|error| format!("{error:#}")))
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The stored spec each of `ids` names, with the id as shown.
    pub(crate) fn resolve(
        &self,
        ids: &[String],
    ) -> Result<Vec<(String, Option<&aida_core::Requirement>)>, String> {
        let store = self.store()?;
        Ok(ids
            .iter()
            .map(|id| {
                let spec = super::resolve_trailer(store, id)
                    .and_then(|uuid| store.requirements.iter().find(|r| r.id == uuid));
                (aida_core::node_alias::display_spec_id(id), spec)
            })
            .collect())
    }

    /// Eligibility of a change naming `ids` whose own diff is
    /// `changed_paths`. `Err` when a fact could not be read.
    pub(crate) fn eligibility(
        &self,
        ids: &[String],
        changed_paths: &[String],
    ) -> Result<Eligibility, String> {
        let config = self.config()?;
        if config.mode == Mode::Off {
            return Ok(review_after_merge_eligible(
                config,
                &[],
                &[],
                ProtectedPaths::floor_ref(),
            ));
        }
        let specs: Vec<(String, Option<SpecFacts>)> = self
            .resolve(ids)?
            .into_iter()
            .map(|(id, spec)| {
                let facts = spec.map(|spec| SpecFacts {
                    tags: spec.tags.iter().cloned().collect(),
                    mode: spec.execution_mode,
                });
                (id, facts)
            })
            .collect();
        // An unreadable gate file still checks the floor.
        let protected =
            ProtectedPaths::of(&crate::gate::mandate_gates::load_human_gates(&self.root));
        Ok(review_after_merge_eligible(
            config,
            &specs,
            changed_paths,
            &protected,
        ))
    }
}

/// How a change was admitted to integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Approved at its head, with no hold.
    Approved(String),
    /// Not yet approved; eligible to merge before review.
    ReviewAfterMerge(String),
}

/// Whether the change in `row` may merge into integration now. `hold` is
/// the armed merge hold, if any; `eligibility` is only asked for a change
/// that is not approved.
// trace:STORY-2772 | ai:claude
pub(crate) fn admit(
    row: &crate::land::Row,
    hold: Result<Option<String>, String>,
    eligibility: impl FnOnce() -> Result<Eligibility, String>,
) -> Result<Admission, String> {
    use crate::land::{State, Track};
    if row.track != Track::Integration {
        return Err("it does not target `integration`".into());
    }
    match hold {
        Err(why) => return Err(format!("its merge hold could not be read ({why})")),
        Ok(Some(hold)) => return Err(format!("a merge hold is armed: {hold}")),
        Ok(None) => {}
    }
    match row.state {
        State::Ready => Ok(Admission::Approved(row.reason.clone())),
        State::NeedsReview | State::Reviewing => match eligibility() {
            Ok(Eligibility::Eligible(why)) => Ok(Admission::ReviewAfterMerge(why)),
            Ok(Eligibility::Ineligible(why)) => Err(format!(
                "it is not approved ({}) and may not merge before review: {why}",
                row.reason
            )),
            Err(why) => Err(format!(
                "it is not approved and its eligibility could not be read ({why})"
            )),
        },
        _ => Err(format!("it is {}: {}", row.state.label(), row.reason)),
    }
}

/// One commit on integration's first-parent line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FirstParent {
    pub sha: String,
    pub parents: Vec<String>,
    pub tree: String,
    pub subject: String,
}

/// The commits of `commits` (newest first) that no promotion has carried to
/// main. A promotion squashes an integration head into main, so main then
/// has a commit with that head's tree: everything from the newest such head
/// back is promoted.
pub(crate) fn unpromoted<'a>(
    commits: &'a [FirstParent],
    main_trees: &HashSet<String>,
) -> &'a [FirstParent] {
    let end = commits
        .iter()
        .position(|commit| main_trees.contains(&commit.tree))
        .unwrap_or(commits.len());
    &commits[..end]
}

/// The change a merge subject names: the number after `merge PR #`.
pub(crate) fn merged_pr(subject: &str) -> Option<u64> {
    let at = subject.find("merge PR #")? + "merge PR #".len();
    let digits: String = subject[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The newest recorded verdict for a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub approves: bool,
    pub label: String,
    pub sha: Option<String>,
}

/// A change merged into integration: its merge commit and the head merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Constituent {
    pub pr: u64,
    pub merge: String,
    pub head: String,
    pub subject: String,
}

/// Why a promotion is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Finding {
    /// A merged change with no approval at the head that was merged.
    Pending {
        constituent: Constituent,
        why: String,
    },
    /// A commit that is no merge of a reviewed change.
    Unreviewed { sha: String, subject: String },
}

impl Finding {
    pub(crate) fn describe(&self) -> String {
        let short = |sha: &str| sha.chars().take(11).collect::<String>();
        match self {
            Finding::Pending { constituent, why } => format!(
                "#{} merged at {} (head {}): {why}. Review it, or take it out with `git revert -m 1 {}`",
                constituent.pr,
                short(&constituent.merge),
                short(&constituent.head),
                short(&constituent.merge),
            ),
            Finding::Unreviewed { sha, subject } => format!(
                "{} is not a merge of a reviewed change: {subject}",
                short(sha)
            ),
        }
    }
}

/// Check every unpromoted commit. A merge whose second parent is on main is
/// a sync of main and is skipped; a merge naming a change is a constituent
/// and needs that change's newest verdict to approve exactly the merged
/// head; anything else is refused. Returns the approved constituents and
/// the findings, newest first.
// trace:STORY-2772 | ai:claude
pub(crate) fn promotion_check(
    commits: &[FirstParent],
    on_main: &dyn Fn(&str) -> bool,
    verdict: &dyn Fn(u64) -> Option<Verdict>,
) -> (Vec<Constituent>, Vec<Finding>) {
    let mut approved = Vec::new();
    let mut findings = Vec::new();
    for commit in commits {
        let merged = match commit.parents.as_slice() {
            [_, second] => Some(second),
            _ => None,
        };
        if merged.is_some_and(|second| on_main(second)) {
            continue;
        }
        let (Some(head), Some(pr)) = (merged, merged_pr(&commit.subject)) else {
            findings.push(Finding::Unreviewed {
                sha: commit.sha.clone(),
                subject: commit.subject.clone(),
            });
            continue;
        };
        let constituent = Constituent {
            pr,
            merge: commit.sha.clone(),
            head: head.clone(),
            subject: commit.subject.clone(),
        };
        let why = match verdict(pr) {
            None => Some("no review verdict is recorded".to_string()),
            Some(verdict) if !verdict.approves => {
                Some(format!("its newest verdict is {}", verdict.label))
            }
            Some(verdict) => match verdict.sha.as_deref() {
                // The full merged head must begin with what was reviewed.
                Some(sha) if sha.len() >= 7 && head.starts_with(&sha.to_ascii_lowercase()) => None,
                Some(sha) => Some(format!(
                    "it is approved at {sha}, not at the head that was merged"
                )),
                None => Some("its approval records no commit".to_string()),
            },
        };
        match why {
            None => approved.push(constituent),
            Some(why) => findings.push(Finding::Pending { constituent, why }),
        }
    }
    (approved, findings)
}

/// The merge commit's subject for change `pr` titled `title`: the title's
/// own summary and spec trailer, without its `[AI:tool] type(scope):` lead.
pub(crate) fn merge_subject(pr: u64, title: &str) -> String {
    let title = title.trim();
    let summary = match title.split_once("): ") {
        Some((lead, rest)) if !lead.contains(' ') || lead.starts_with('[') => rest,
        _ => title,
    };
    format!("chore(integration): merge PR #{pr} {summary}")
}

#[cfg(test)]
#[path = "tests/story_2772_review_after_merge_tests.rs"]
mod tests;
