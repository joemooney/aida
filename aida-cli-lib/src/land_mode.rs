//! `aida land mode status|set|reconcile`: the landing mode and the forge's
//! required checks, kept in step. `set` and `reconcile` change the default
//! branch's protection, so they need the operator at a terminal or the
//! advisor seat under a mandate that includes protected paths
//! ([`crate::gate::ship::landing_mode_change_refusal`]). Every change is
//! read back from the forge, restored on a mismatch, and appended to
//! `.aida/land/landing-audit.jsonl`.
// trace:STORY-2897 | ai:claude

use crate::landing_policy::{LandingMode, LandingPolicy};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const AUDIT: &str = ".aida/land/landing-audit.jsonl";

/// The forge's required status checks on the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Required {
    pub contexts: Vec<String>,
    pub strict: bool,
}

/// Read and replace the required status checks of the default branch. Only
/// the check list is replaced; `strict` is written back as it was read.
pub(crate) trait Protection {
    fn read(&mut self) -> Result<Required>;
    fn write(&mut self, contexts: &[String], strict: bool) -> Result<()>;
}

/// GitHub's branch protection, through `gh api`.
struct GhProtection {
    root: PathBuf,
    endpoint: String,
}

impl GhProtection {
    fn new(root: &Path) -> Result<Self> {
        let branch = crate::gate::review_carryover::protected_base(root)
            .map_err(|e| anyhow::anyhow!("{}", e.reason()))
            .context("could not resolve the default branch")?;
        Ok(Self {
            root: root.to_path_buf(),
            endpoint: format!(
                "repos/{{owner}}/{{repo}}/branches/{}/protection/required_status_checks",
                crate::pr_emergency_land::encode(&branch)
            ),
        })
    }

    fn gh(&self, args: &[&str], input: Option<&str>) -> Result<String> {
        crate::forge::gh_api(&self.root, args, input)
    }
}

impl Protection for GhProtection {
    fn read(&mut self) -> Result<Required> {
        let body = self.gh(&[&self.endpoint], None)?;
        parse_required(&body)
    }

    fn write(&mut self, contexts: &[String], strict: bool) -> Result<()> {
        let body = serde_json::json!({ "strict": strict, "contexts": contexts }).to_string();
        self.gh(
            &["-X", "PATCH", &self.endpoint, "--input", "-"],
            Some(&body),
        )
        .map(|_| ())
    }
}

/// PURE: the `required_status_checks` object GitHub returns.
pub(crate) fn parse_required(body: &str) -> Result<Required> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("the forge's required checks are not JSON")?;
    let contexts = value
        .get("contexts")
        .and_then(|c| c.as_array())
        .context("the forge's required checks carry no `contexts` list")?
        .iter()
        .filter_map(|c| c.as_str().map(str::to_string))
        .collect();
    Ok(Required {
        contexts,
        strict: value
            .get("strict")
            .and_then(|s| s.as_bool())
            .unwrap_or(false),
    })
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<&String> = a.iter().collect();
    let mut b: Vec<&String> = b.iter().collect();
    a.sort();
    a.dedup();
    b.sort();
    b.dedup();
    a == b
}

/// What a reconcile did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reconciled {
    pub before: Required,
    pub after: Required,
    pub changed: bool,
}

/// Make the forge require exactly `want`, then read it back. A read-back
/// that differs restores the previous list and fails.
pub(crate) fn reconcile(forge: &mut dyn Protection, want: &[String]) -> Result<Reconciled> {
    let before = forge.read()?;
    if same_set(&before.contexts, want) {
        return Ok(Reconciled {
            after: before.clone(),
            before,
            changed: false,
        });
    }
    forge.write(want, before.strict)?;
    let after = forge.read()?;
    if !same_set(&after.contexts, want) {
        let restored = forge.write(&before.contexts, before.strict);
        bail!(
            "the forge requires [{}] after it was asked for [{}]; {}",
            after.contexts.join(", "),
            want.join(", "),
            match restored {
                Ok(()) => format!("restored [{}]", before.contexts.join(", ")),
                Err(e) => format!(
                    "restoring [{}] also failed ({e:#}); fix branch protection by hand",
                    before.contexts.join(", ")
                ),
            }
        );
    }
    Ok(Reconciled {
        before,
        after,
        changed: true,
    })
}

/// PURE: how the forge differs from what the effective mode requires.
pub(crate) fn drift(policy: &LandingPolicy, forge: &Required) -> Vec<String> {
    let want = policy.required_checks();
    if same_set(want, &forge.contexts) {
        return Vec::new();
    }
    let missing: Vec<&str> = want
        .iter()
        .filter(|w| !forge.contexts.contains(w))
        .map(String::as_str)
        .collect();
    let extra: Vec<&str> = forge
        .contexts
        .iter()
        .filter(|c| !want.contains(c))
        .map(String::as_str)
        .collect();
    let mut out = Vec::new();
    if !missing.is_empty() {
        out.push(format!(
            "the forge does not require {} ({} mode does)",
            missing.join(", "),
            policy.mode.as_str()
        ));
    }
    if !extra.is_empty() {
        out.push(format!(
            "the forge also requires {} ({} mode does not)",
            extra.join(", "),
            policy.mode.as_str()
        ));
    }
    out
}

/// Who is acting, for the audit.
struct Actor {
    user: String,
    floor: bool,
    seat: Option<String>,
    mandate: Option<String>,
}

fn authorize(root: &Path) -> Result<Actor> {
    let floor = crate::gate::role_authority::has_integrity_floor_authority();
    let seat = crate::gate::seat_authority::current_seat(root);
    let mandate = match crate::gate::mandate::current_state() {
        crate::gate::mandate::MandateState::Active(m) if m.include_gate_paths => Some(m.id),
        _ => None,
    };
    if let Some(why) =
        crate::gate::ship::landing_mode_change_refusal(floor, mandate.is_some(), seat.as_deref())
    {
        bail!("{why}");
    }
    Ok(Actor {
        user: crate::current_user_id(None),
        floor,
        seat,
        mandate,
    })
}

fn require_github(root: &Path) -> Result<()> {
    if crate::forge::resolve_forge_kind(root) != crate::forge::ForgeKind::GitHub {
        bail!(
            "`aida land mode` reconciles GitHub branch protection only; this project's forge is \
             not GitHub, so its required checks are left as they are"
        );
    }
    Ok(())
}

fn audit(
    root: &Path,
    action: &str,
    actor: &Actor,
    from: Option<LandingMode>,
    to: LandingMode,
    done: &Reconciled,
) -> Result<()> {
    use std::io::Write;
    let path = root.join(AUDIT);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let line = serde_json::json!({
        "at": chrono::Utc::now().to_rfc3339(),
        "action": action,
        "from": from.map(LandingMode::as_str),
        "to": to.as_str(),
        "user": actor.user,
        "operator_at_terminal": actor.floor,
        "seat": actor.seat,
        "mandate": actor.mandate,
        "forge_before": done.before.contexts,
        "forge_after": done.after.contexts,
        "changed": done.changed,
        "read_back_ok": true,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn print_change(done: &Reconciled) {
    if done.changed {
        println!(
            "  forge required checks: [{}] -> [{}] (read back)",
            done.before.contexts.join(", "),
            done.after.contexts.join(", ")
        );
    } else {
        println!(
            "  forge required checks already [{}]; nothing changed",
            done.before.contexts.join(", ")
        );
    }
}

/// `aida land mode set <mode>`.
pub(crate) fn set(root: &Path, mode: LandingMode) -> Result<()> {
    let actor = authorize(root)?;
    require_github(root)?;
    let policy = crate::landing_policy::load_reporting(root);
    let from = policy.mode;
    let want = match mode {
        LandingMode::CiGated => policy.ci_gated_checks.clone(),
        LandingMode::ReviewGated => policy.review_gated_checks.clone(),
    };
    let done = reconcile(&mut GhProtection::new(root)?, &want)?;
    let path = root.join(".aida").join("config.toml");
    crate::config_edit::set_kvs(
        &path,
        "landing",
        &[
            ("mode", toml_edit::Value::from(mode.as_str())),
            ("mode_set_by", toml_edit::Value::from(actor.user.as_str())),
            (
                "mode_set_at",
                toml_edit::Value::from(
                    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                ),
            ),
        ],
    )
    .with_context(|| format!("could not write {}", path.display()))?;
    audit(root, "set", &actor, Some(from), mode, &done)?;
    println!("landing mode: {} -> {}", from.as_str(), mode.as_str());
    print_change(&done);
    println!(
        "  .aida/config.toml now says `[landing] mode = {}`; landing reads the default \
         branch's copy, so land this edit in a reviewed PR for it to take effect.",
        aida_core::toml_quote::toml_string(mode.as_str())
    );
    println!("  to undo: aida land mode set {}", from.as_str());
    Ok(())
}

/// `aida land mode reconcile`: re-apply the effective mode's check list.
pub(crate) fn reconcile_effective(root: &Path) -> Result<()> {
    let actor = authorize(root)?;
    require_github(root)?;
    let policy = crate::landing_policy::load_reporting(root);
    let done = reconcile(&mut GhProtection::new(root)?, policy.required_checks())?;
    audit(root, "reconcile", &actor, None, policy.mode, &done)?;
    println!("landing mode: {} (default branch)", policy.mode.as_str());
    print_change(&done);
    Ok(())
}

/// `aida land mode status`.
pub(crate) fn status(root: &Path) -> Result<()> {
    let policy = crate::landing_policy::load_reporting(root);
    let declared = crate::land::run::project_config(root)
        .and_then(|c| c.get("landing")?.get("mode")?.as_str().map(str::to_string));
    println!("landing mode: {} (default branch)", policy.mode.as_str());
    match declared {
        Some(local) if local != policy.mode.as_str() => println!(
            "  this checkout's .aida/config.toml says `{local}`; it takes effect once it is on \
             the default branch"
        ),
        _ => {}
    }
    println!(
        "  {} mode requires: {}",
        policy.mode.as_str(),
        policy.required_checks().join(", ")
    );
    let forge = if crate::forge::resolve_forge_kind(root) == crate::forge::ForgeKind::GitHub {
        GhProtection::new(root).and_then(|mut p| p.read())
    } else {
        Err(anyhow::anyhow!("not a GitHub project"))
    };
    match forge {
        Ok(required) => {
            println!("  the forge requires: {}", required.contexts.join(", "));
            let drift = drift(&policy, &required);
            if drift.is_empty() {
                println!("  in step");
            } else {
                for line in drift {
                    println!("  DRIFT: {line}; run `aida land mode reconcile`");
                }
            }
        }
        Err(e) => println!("  the forge's required checks could not be read: {e:#}"),
    }
    if let Ok(text) = std::fs::read_to_string(root.join(AUDIT)) {
        let lines: Vec<&str> = text.lines().collect();
        if !lines.is_empty() {
            println!("  recent changes ({AUDIT}):");
            for line in lines.iter().rev().take(5).rev() {
                println!("    {line}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod story2897_land_mode_tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        now: Vec<String>,
        strict: bool,
        writes: Vec<Vec<String>>,
        /// Ignore the next write (the forge accepts it and changes nothing).
        swallow: bool,
    }
    impl Protection for Fake {
        fn read(&mut self) -> Result<Required> {
            Ok(Required {
                contexts: self.now.clone(),
                strict: self.strict,
            })
        }
        fn write(&mut self, contexts: &[String], strict: bool) -> Result<()> {
            assert_eq!(strict, self.strict, "strict is written back as read");
            self.writes.push(contexts.to_vec());
            if self.swallow {
                self.swallow = false;
            } else {
                self.now = contexts.to_vec();
            }
            Ok(())
        }
    }
    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reconcile_sets_reads_back_and_reports_before_and_after() {
        let mut forge = Fake {
            now: names(&["merge-hold-gate", "Build (ubuntu-latest)"]),
            strict: true,
            ..Fake::default()
        };
        let done = reconcile(&mut forge, &names(&["merge-hold-gate"])).unwrap();
        assert!(done.changed);
        assert_eq!(
            done.before.contexts,
            ["merge-hold-gate", "Build (ubuntu-latest)"]
        );
        assert_eq!(done.after.contexts, ["merge-hold-gate"]);
        assert_eq!(forge.writes.len(), 1);
    }

    #[test]
    fn reconcile_leaves_a_matching_forge_untouched() {
        let mut forge = Fake {
            now: names(&["Build (ubuntu-latest)", "merge-hold-gate"]),
            ..Fake::default()
        };
        let done = reconcile(
            &mut forge,
            &names(&["merge-hold-gate", "Build (ubuntu-latest)"]),
        )
        .unwrap();
        assert!(!done.changed);
        assert!(forge.writes.is_empty());
    }

    #[test]
    fn a_read_back_mismatch_restores_the_previous_checks_and_fails() {
        let mut forge = Fake {
            now: names(&["merge-hold-gate", "Build (ubuntu-latest)"]),
            swallow: true,
            ..Fake::default()
        };
        // The swallowed write leaves the old list; read-back sees it unchanged
        // only if it differs from the request, so ask for something new.
        let err = reconcile(&mut forge, &names(&["merge-hold-gate"])).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("after it was asked for [merge-hold-gate]"),
            "{text}"
        );
        assert!(
            text.contains("restored [merge-hold-gate, Build (ubuntu-latest)]"),
            "{text}"
        );
        assert_eq!(forge.writes.len(), 2, "one attempt, one restore");
        assert_eq!(forge.now, ["merge-hold-gate", "Build (ubuntu-latest)"]);
    }

    #[test]
    fn drift_names_missing_and_extra_checks() {
        let policy = LandingPolicy {
            mode: LandingMode::ReviewGated,
            ..LandingPolicy::default()
        };
        assert!(drift(
            &policy,
            &Required {
                contexts: names(&["merge-hold-gate"]),
                strict: false
            }
        )
        .is_empty());
        let lines = drift(
            &policy,
            &Required {
                contexts: names(&["Build (ubuntu-latest)"]),
                strict: false,
            },
        );
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("does not require merge-hold-gate"));
        assert!(lines[1].contains("also requires Build (ubuntu-latest)"));
    }

    #[test]
    fn the_forge_response_parses() {
        let r = parse_required(r#"{"strict":true,"contexts":["merge-hold-gate"],"checks":[]}"#)
            .unwrap();
        assert_eq!(r.contexts, ["merge-hold-gate"]);
        assert!(r.strict);
        assert!(parse_required(r#"{"strict":true}"#).is_err());
    }
}
