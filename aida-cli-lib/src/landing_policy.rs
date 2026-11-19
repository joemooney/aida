//! The project's landing policy: the `[landing]` section of
//! `.aida/config.toml`. Every landing choice is a setting; the defaults are
//! the CI-gated behaviour AIDA had before the setting existed.
//!
//! The policy relaxes or tightens what a merge waits for, so it is read only
//! from the config on the trusted default branch, never from a feature
//! branch's own copy. A missing, unreadable or malformed value falls back to
//! its default (the stricter behaviour) with a warning; it never fails open.
// trace:STORY-2897 | ai:claude

use std::path::Path;

/// The check every required-check set must contain (advisor condition C1).
pub(crate) const MERGE_HOLD_GATE: &str = crate::ci_gate::HOLD_GATE_CHECK;

/// What a merge into the default branch waits for.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LandingMode {
    /// A merge waits for CI on the head (and full CI where the repo tiers
    /// it). The default.
    #[default]
    CiGated,
    /// A merge waits for an independent approval at the exact head and no
    /// hold; CI runs after the merge.
    ReviewGated,
}

/// Who performs the merge.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Merger {
    /// Anyone who passes the gates. The default. In `review-gated` mode the
    /// merging session must also have authored none of the change (as it
    /// must under `Advisor`).
    #[default]
    AnyNonAuthor,
    /// Only the advisor seat (or the operator at a terminal) merges; other
    /// seats report a ready change.
    Advisor,
}

/// Which default-branch commits the live binary is promoted from.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PromoteFrom {
    /// A clean default-branch checkout equal to its remote. The default.
    #[default]
    CleanMain,
    /// Additionally, every required CI check is green on that exact commit.
    GreenMain,
    /// Every test failing at the commit also fails at the promoted commit,
    /// and the smoke test passes (`promotion_failures`). Missing CI evidence
    /// refuses; it never weakens to `clean-main`.
    // trace:STORY-2912 | ai:claude
    NoNewFailures,
}

macro_rules! setting_enum {
    ($ty:ident, $key:literal, [$($variant:ident => $word:literal),+ $(,)?]) => {
        impl $ty {
            /// The accepted setting values, default first.
            pub(crate) const ALLOWED: &'static [&'static str] = &[$($word),+];

            pub(crate) fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word),+
                }
            }

            /// Parse a configured value; an unknown one names the allowed values.
            pub(crate) fn parse(value: &str) -> Result<Self, String> {
                match value.trim() {
                    $($word => Ok(Self::$variant),)+
                    other => Err(format!(
                        "`[landing] {}` is set to `{other}`, which is not a known value; \
                         allowed values: {}",
                        $key,
                        Self::ALLOWED.join(", ")
                    )),
                }
            }
        }
    };
}

setting_enum!(LandingMode, "mode", [CiGated => "ci-gated", ReviewGated => "review-gated"]);
setting_enum!(Merger, "merger", [AnyNonAuthor => "any-non-author", Advisor => "advisor"]);
setting_enum!(PromoteFrom, "promote_live_binary_from", [CleanMain => "clean-main", GreenMain => "green-main", NoNewFailures => "no-new-failures"]);

/// The default required checks of a CI-gated default branch.
pub(crate) const DEFAULT_CI_GATED_CHECKS: &[&str] = &[MERGE_HOLD_GATE, "Build (ubuntu-latest)"];
/// The default required checks of a review-gated default branch.
pub(crate) const DEFAULT_REVIEW_GATED_CHECKS: &[&str] = &[MERGE_HOLD_GATE];

/// The resolved `[landing]` policy.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LandingPolicy {
    pub(crate) mode: LandingMode,
    pub(crate) merger: Merger,
    pub(crate) promote_from: PromoteFrom,
    /// `required_checks.ci_gated`: the forge's required checks in CI-gated
    /// mode, and the full CI set that green-main promotion reads.
    pub(crate) ci_gated_checks: Vec<String>,
    /// `required_checks.review_gated`.
    pub(crate) review_gated_checks: Vec<String>,
    /// Values that fell back to their default, each with its reason.
    pub(crate) warnings: Vec<String>,
    /// Keys the config sets validly (`required_checks.<mode>` for lists).
    pub(crate) explicit: Vec<&'static str>,
    /// Keys the config sets but that fell back to their default.
    pub(crate) invalid: Vec<&'static str>,
}

/// Where an effective `[landing]` value came from.
// trace:STORY-2897 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Default,
    Explicit,
    Invalid,
}

impl Default for LandingPolicy {
    fn default() -> Self {
        Self {
            mode: LandingMode::default(),
            merger: Merger::default(),
            promote_from: PromoteFrom::default(),
            ci_gated_checks: owned(DEFAULT_CI_GATED_CHECKS),
            review_gated_checks: owned(DEFAULT_REVIEW_GATED_CHECKS),
            warnings: Vec::new(),
            explicit: Vec::new(),
            invalid: Vec::new(),
        }
    }
}

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

impl LandingPolicy {
    /// Parse the `[landing]` section of a config body. Never fails: a bad
    /// value keeps its default and records a warning.
    // trace:STORY-2897 | ai:claude
    pub(crate) fn from_config(config: Option<&toml::Value>) -> Self {
        let mut policy = Self::default();
        let Some(section) = config.and_then(|c| c.get("landing")) else {
            return policy;
        };
        if !section.is_table() {
            policy
                .warnings
                .push("`[landing]` must be a table; landing uses its defaults".to_string());
            return policy;
        }
        macro_rules! read {
            ($field:ident, $ty:ident, $key:literal) => {
                if let Some(value) = section.get($key) {
                    match value.as_str().map($ty::parse) {
                        Some(Ok(v)) => {
                            policy.$field = v;
                            policy.explicit.push($key);
                        }
                        Some(Err(e)) => {
                            policy.invalid.push($key);
                            policy
                                .warnings
                                .push(format!("{e}; using `{}`", $ty::default().as_str()));
                        }
                        None => {
                            policy.invalid.push($key);
                            policy.warnings.push(format!(
                                "`[landing] {}` must be a string; using `{}`",
                                $key,
                                $ty::default().as_str()
                            ));
                        }
                    }
                }
            };
        }
        read!(mode, LandingMode, "mode");
        // Train routing is not part of `[landing]` yet: say so rather than
        // show a knob that changes nothing.
        if section.get("use_train").is_some() {
            policy.invalid.push("use_train");
            policy.warnings.push(
                "`[landing] use_train` is not yet supported (S1t); train routing is unchanged"
                    .to_string(),
            );
        }
        read!(merger, Merger, "merger");
        read!(promote_from, PromoteFrom, "promote_live_binary_from");
        if let Some(checks) = section.get("required_checks") {
            if !checks.is_table() {
                policy.invalid.push("required_checks");
                policy.warnings.push(
                    "`[landing] required_checks` must be a table of check lists; using the \
                     default lists"
                        .to_string(),
                );
                return policy;
            }
            for (key, label, default) in [
                (
                    "ci_gated",
                    "required_checks.ci_gated",
                    DEFAULT_CI_GATED_CHECKS,
                ),
                (
                    "review_gated",
                    "required_checks.review_gated",
                    DEFAULT_REVIEW_GATED_CHECKS,
                ),
            ] {
                let Some(value) = checks.get(key) else {
                    continue;
                };
                let field = if key == "ci_gated" {
                    &mut policy.ci_gated_checks
                } else {
                    &mut policy.review_gated_checks
                };
                match check_list(value) {
                    Ok(list) => {
                        *field = list;
                        policy.explicit.push(label);
                    }
                    Err(why) => {
                        *field = owned(default);
                        policy.invalid.push(label);
                        policy.warnings.push(format!(
                            "`[landing] {label}` {why}; using {}",
                            default.join(", ")
                        ));
                    }
                }
            }
        }
        policy
    }

    /// Where a knob's effective value came from: `Explicit` when the
    /// default branch's config sets it validly, `Invalid` when it is set but
    /// fell back to its default, `Default` when it is not set.
    // trace:STORY-2897 | ai:claude
    pub(crate) fn origin(&self, key: &str) -> Origin {
        if self.invalid.contains(&key) {
            Origin::Invalid
        } else if self.explicit.contains(&key) {
            Origin::Explicit
        } else {
            Origin::Default
        }
    }

    /// The required-check set the forge should enforce in the current mode.
    pub(crate) fn required_checks(&self) -> &[String] {
        match self.mode {
            LandingMode::CiGated => &self.ci_gated_checks,
            LandingMode::ReviewGated => &self.review_gated_checks,
        }
    }

    /// Whether `aida pr ship` waits for CI on the head before merging.
    pub(crate) fn waits_for_ci(&self) -> bool {
        self.mode == LandingMode::CiGated
    }

    /// Whether only the advisor seat (or the operator at a terminal) may
    /// merge: the internal merger setting, and always in review-gated mode
    /// (operator direction 2026-11-18 10:37: the merger is the advisor).
    // trace:STORY-2903 | ai:claude
    pub(crate) fn advisor_merges(&self) -> bool {
        self.merger == Merger::Advisor || self.mode == LandingMode::ReviewGated
    }
}

/// A required-check list: non-empty strings, no duplicates, and always
/// containing `merge-hold-gate` (advisor condition C1).
fn check_list(value: &toml::Value) -> Result<Vec<String>, String> {
    let items = value.as_array().ok_or("must be a list of check names")?;
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let name = item
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("must contain only non-empty check names")?;
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    if !out.iter().any(|n| n == MERGE_HOLD_GATE) {
        return Err(format!("must contain `{MERGE_HOLD_GATE}`"));
    }
    Ok(out)
}

/// The landing policy of the project at `root`, from the default branch's
/// config (local refs only, never the worktree copy). No trusted config
/// means the defaults.
// trace:STORY-2897 | ai:claude
pub(crate) fn load(root: &Path) -> LandingPolicy {
    from_body(crate::trusted_config::read_trusted_config_toml_local(root).as_deref())
}

/// The policy of a trusted config body (`None`: no config, the defaults).
// trace:STORY-2897 | ai:claude
pub(crate) fn from_body(body: Option<&str>) -> LandingPolicy {
    let parsed = body.map(str::parse::<toml::Value>);
    match parsed {
        Some(Ok(value)) => LandingPolicy::from_config(Some(&value)),
        Some(Err(e)) => LandingPolicy {
            warnings: vec![format!(
                "the default branch's .aida/config.toml does not parse ({e}); landing uses its defaults"
            )],
            ..LandingPolicy::default()
        },
        None => LandingPolicy::default(),
    }
}

/// The landing policy for a decision that must not default under
/// uncertainty (the auto-train's choice to run, STORY-2905): no trusted
/// default branch, a config that exists but cannot be read, config that
/// does not parse, or an invalid `mode` is an error, never the ci-gated
/// default. A default branch with no `.aida/config.toml` is the defaults.
// trace:STORY-2905 | ai:claude
pub(crate) fn load_checked(root: &Path) -> Result<LandingPolicy, String> {
    let sha = crate::trusted_config::local_trusted_default_branch_sha(root)
        .ok_or("the default branch cannot be resolved, so its landing policy is unknown")?;
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| format!("git could not run: {e}"))
    };
    let listed = git(&["ls-tree", "--name-only", &sha, "--", ".aida/config.toml"])?;
    if !listed.status.success() {
        return Err(format!(
            "the default branch's tree cannot be read ({})",
            String::from_utf8_lossy(&listed.stderr).trim()
        ));
    }
    if listed.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(LandingPolicy::default());
    }
    let shown = git(&["show", &format!("{sha}:.aida/config.toml")])?;
    if !shown.status.success() {
        return Err(format!(
            "the default branch's .aida/config.toml cannot be read ({})",
            String::from_utf8_lossy(&shown.stderr).trim()
        ));
    }
    let body = String::from_utf8(shown.stdout)
        .map_err(|_| "the default branch's .aida/config.toml is not UTF-8".to_string())?;
    let value = body
        .parse::<toml::Value>()
        .map_err(|e| format!("the default branch's .aida/config.toml does not parse ({e})"))?;
    // A `landing` key that is not a table is invalid, not absent.
    if value.get("landing").is_some_and(|l| !l.is_table()) {
        return Err("the default branch's `landing` is not a table".to_string());
    }
    let policy = LandingPolicy::from_config(Some(&value));
    if policy.origin("mode") == Origin::Invalid {
        return Err(format!(
            "the default branch's `[landing] mode` is not valid: {}",
            policy.warnings.join("; ")
        ));
    }
    Ok(policy)
}

/// `load`, printing any fallback warnings once to stderr.
pub(crate) fn load_reporting(root: &Path) -> LandingPolicy {
    let policy = load(root);
    for w in &policy.warnings {
        eprintln!("  warning: {w}");
    }
    policy
}

/// `Some(refusal)` when the landing policy that decides a merge changed
/// between the start of a ship and its merge (another merge can land a new
/// `[landing]` on the default branch while this one waits).
// trace:STORY-2897 | ai:claude
pub(crate) fn policy_change_refusal(
    before: &LandingPolicy,
    after: &LandingPolicy,
    subject: &str,
) -> Option<String> {
    let mut changed = Vec::new();
    if before.mode != after.mode {
        changed.push(format!(
            "mode {} -> {}",
            before.mode.as_str(),
            after.mode.as_str()
        ));
    }
    if before.merger != after.merger {
        changed.push(format!(
            "merger {} -> {}",
            before.merger.as_str(),
            after.merger.as_str()
        ));
    }
    (!changed.is_empty()).then(|| {
        format!(
            "{subject} is not merged here: the landing policy changed while it waited ({}); \
             run the ship again so it is decided under the policy now in force",
            changed.join(", ")
        )
    })
}

#[cfg(test)]
mod story2897_landing_policy_tests {
    use super::*;

    fn parse(body: &str) -> LandingPolicy {
        LandingPolicy::from_config(Some(&body.parse::<toml::Value>().unwrap()))
    }

    #[test]
    fn no_section_is_the_ci_gated_default() {
        let p = parse("[land]\nwip_max = 10\n");
        assert_eq!(p, LandingPolicy::default());
        assert!(p.waits_for_ci());
        assert_eq!(
            p.required_checks(),
            ["merge-hold-gate", "Build (ubuntu-latest)"]
        );
    }

    #[test]
    fn every_knob_parses_its_values() {
        let p = parse(
            "[landing]\nmode = \"review-gated\"\nmerger = \"advisor\"\n\
             promote_live_binary_from = \"green-main\"\n\
             [landing.required_checks]\nreview_gated = [\"merge-hold-gate\", \"lint\"]\n",
        );
        assert_eq!(p.mode, LandingMode::ReviewGated);
        assert_eq!(p.merger, Merger::Advisor);
        assert_eq!(p.promote_from, PromoteFrom::GreenMain);
        assert!(!p.waits_for_ci());
        assert_eq!(p.required_checks(), ["merge-hold-gate", "lint"]);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let p = parse("[landing]\npromote_live_binary_from = \"no-new-failures\"\n");
        assert_eq!(p.promote_from, PromoteFrom::NoNewFailures);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let p = parse("[landing]\nuse_train = \"never\"\n");
        assert_eq!(p.origin("use_train"), Origin::Invalid);
        assert!(
            p.warnings[0].contains("not yet supported (S1t)"),
            "{:?}",
            p.warnings
        );
    }

    #[test]
    fn a_bad_value_falls_back_to_the_stricter_default() {
        let p = parse(
            "[landing]\nmode = \"yolo\"\nmerger = \"anyone\"\n\
             promote_live_binary_from = \"\"\n",
        );
        assert_eq!(p.mode, LandingMode::CiGated);
        assert_eq!(p.merger, Merger::AnyNonAuthor);
        assert_eq!(p.promote_from, PromoteFrom::CleanMain);
        assert_eq!(p.warnings.len(), 3, "{:?}", p.warnings);
        assert!(p.warnings[0].contains("allowed values: ci-gated, review-gated"));
    }

    /// Advisor condition C1.
    #[test]
    fn a_required_check_set_without_the_hold_gate_is_refused() {
        let p = parse(
            "[landing]\nmode = \"review-gated\"\n[landing.required_checks]\n\
             review_gated = [\"lint\"]\nci_gated = []\n",
        );
        assert_eq!(p.review_gated_checks, ["merge-hold-gate"]);
        assert_eq!(
            p.ci_gated_checks,
            ["merge-hold-gate", "Build (ubuntu-latest)"]
        );
        assert_eq!(p.warnings.len(), 2, "{:?}", p.warnings);
        assert!(p
            .warnings
            .iter()
            .all(|w| w.contains("must contain `merge-hold-gate`")));
        let p = parse("[landing.required_checks]\nci_gated = [\"merge-hold-gate\", 7]\n");
        assert_eq!(
            p.ci_gated_checks,
            ["merge-hold-gate", "Build (ubuntu-latest)"]
        );
    }

    #[test]
    fn provenance_tells_explicit_defaults_from_absent_and_invalid() {
        let p = parse(
            "[landing]\nmode = \"ci-gated\"\nmerger = \"anyone\"\n\
             [landing.required_checks]\nreview_gated = [\"merge-hold-gate\"]\n",
        );
        assert_eq!(p.origin("mode"), Origin::Explicit, "explicit default value");
        assert_eq!(p.origin("merger"), Origin::Invalid);
        assert_eq!(p.origin("promote_live_binary_from"), Origin::Default);
        assert_eq!(p.origin("required_checks.review_gated"), Origin::Explicit);
        assert_eq!(p.origin("required_checks.ci_gated"), Origin::Default);
    }

    #[test]
    fn a_malformed_section_or_check_table_warns() {
        let p = parse("landing = 3\n");
        assert_eq!(p.mode, LandingMode::CiGated);
        assert!(
            p.warnings[0].contains("must be a table"),
            "{:?}",
            p.warnings
        );
        let p = parse("[landing]\nrequired_checks = [\"merge-hold-gate\"]\n");
        assert_eq!(p.origin("required_checks"), Origin::Invalid);
        assert!(
            p.warnings[0].contains("must be a table of check lists"),
            "{:?}",
            p.warnings
        );
    }

    #[test]
    fn a_policy_change_during_the_wait_refuses_the_merge() {
        let before = LandingPolicy {
            mode: LandingMode::ReviewGated,
            ..LandingPolicy::default()
        };
        assert_eq!(
            policy_change_refusal(&before, &before.clone(), "PR-7"),
            None
        );
        let after = LandingPolicy {
            merger: Merger::Advisor,
            ..LandingPolicy::default()
        };
        let why = policy_change_refusal(&before, &after, "PR-7").expect("refused");
        assert!(
            why.contains("mode review-gated -> ci-gated")
                && why.contains("merger any-non-author -> advisor"),
            "{why}"
        );
    }

    #[test]
    fn a_project_without_a_trusted_config_uses_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), LandingPolicy::default());
    }
}
