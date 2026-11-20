//! STORY-2884: the project's default for a spec whose `execution_mode` is
//! unset — the `[gate.unset_mode]` section of `.aida/config.toml`.
//!
//! ```toml
//! [gate.unset_mode]
//! default = "supervised"         # the default: an unset mode holds the merge
//! drain_types = ["bug", "task"]  # with default = "drain": the types it covers
//! ```
//!
//! Only `drain` licenses an unattended merge (BUG-727), and an unset mode is
//! supervised. This setting lets a project treat an unset mode as `drain` for
//! the listed spec types. It never overrides a mode a spec (or its carrier)
//! sets, never applies to a sensitive spec (the classes a mandate excludes or
//! treats as always-human), and is withdrawn for a change that touches a
//! protected gate path ([`withdraw_defaults_for_change`]). Every other
//! merge gate still applies.
//!
//! The setting relaxes what a merge waits for, so — like `[landing]` — it is
//! read only from the config on the trusted default branch, never from a
//! feature branch's own copy. A missing, unreadable or malformed value keeps
//! the supervised default; it never fails open.
// trace:STORY-2884 | ai:claude
// mandate-gate

use aida_core::{ExecutionMode, Requirement, RequirementType};
use std::path::Path;

/// What an unset `execution_mode` means for the merge gates.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum UnsetDefault {
    /// An unset mode holds the merge for a human (BUG-727). The default.
    #[default]
    Supervised,
    /// An unset mode on a listed, non-sensitive spec type gates as `drain`.
    Drain,
}

impl UnsetDefault {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Supervised => "supervised",
            Self::Drain => "drain",
        }
    }
}

/// The resolved `[gate.unset_mode]` policy.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct UnsetModePolicy {
    pub(crate) default: UnsetDefault,
    /// The spec types the drain default applies to. Empty: none.
    pub(crate) drain_types: Vec<RequirementType>,
    /// Values that were ignored, each with its reason.
    pub(crate) warnings: Vec<String>,
}

/// A type name with case, spaces, hyphens and underscores removed, so
/// `change-request`, `Change Request` and `changerequest` all match.
fn type_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn type_named(name: &str) -> Option<RequirementType> {
    let key = type_key(name);
    if key.is_empty() {
        return None;
    }
    RequirementType::ALL
        .into_iter()
        .find(|t| type_key(&t.to_string()) == key)
}

/// The lower-case display name `aida show` uses for a type.
fn type_word(t: &RequirementType) -> String {
    format!("{t:?}").to_ascii_lowercase()
}

impl UnsetModePolicy {
    /// Parse `[gate.unset_mode]` from a config body. Never fails: a bad value
    /// keeps the supervised default (or drops the bad list entry) and records
    /// a warning.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn from_config(config: Option<&toml::Value>) -> Self {
        let mut policy = Self::default();
        let Some(section) = config
            .and_then(|c| c.get("gate"))
            .and_then(|g| g.get("unset_mode"))
        else {
            return policy;
        };
        if !section.is_table() {
            policy.warnings.push(
                "`[gate.unset_mode]` must be a table; an unset execution mode stays supervised"
                    .into(),
            );
            return policy;
        }
        let default = match section.get("default") {
            None => UnsetDefault::Supervised,
            Some(v) => match v.as_str().map(str::trim) {
                Some("supervised") => UnsetDefault::Supervised,
                Some("drain") => UnsetDefault::Drain,
                Some(other) => {
                    policy.warnings.push(format!(
                        "`[gate.unset_mode] default` is set to `{other}`, which is not a known \
                         value (allowed: supervised, drain); an unset execution mode stays \
                         supervised"
                    ));
                    return policy;
                }
                None => {
                    policy.warnings.push(
                        "`[gate.unset_mode] default` must be a string; an unset execution mode \
                         stays supervised"
                            .into(),
                    );
                    return policy;
                }
            },
        };
        let mut types = Vec::new();
        match section.get("drain_types") {
            None => {}
            Some(toml::Value::Array(items)) => {
                for item in items {
                    match item.as_str().and_then(type_named) {
                        Some(t) if !types.contains(&t) => types.push(t),
                        Some(_) => {}
                        None => policy.warnings.push(format!(
                            "`[gate.unset_mode] drain_types` entry {item} is not a spec type; \
                             ignored"
                        )),
                    }
                }
            }
            Some(_) => policy.warnings.push(
                "`[gate.unset_mode] drain_types` must be a list of spec types; no type gets \
                 the drain default"
                    .into(),
            ),
        }
        if default == UnsetDefault::Drain && types.is_empty() {
            policy.warnings.push(
                "`[gate.unset_mode] default = \"drain\"` lists no `drain_types`; an unset \
                 execution mode stays supervised"
                    .into(),
            );
        }
        policy.default = default;
        policy.drain_types = types;
        policy
    }

    /// The policy of a trusted config body (`None`: no config, the default).
    // trace:STORY-2884 | ai:claude
    pub(crate) fn from_body(body: Option<&str>) -> Self {
        match body.map(str::parse::<toml::Value>) {
            Some(Ok(value)) => Self::from_config(Some(&value)),
            Some(Err(e)) => Self {
                warnings: vec![format!(
                    "the default branch's .aida/config.toml does not parse ({e}); an unset \
                     execution mode stays supervised"
                )],
                ..Self::default()
            },
            None => Self::default(),
        }
    }

    /// The policy of the project at `root`, from the default branch's config
    /// (local refs only, never the worktree copy).
    // trace:STORY-2884 | ai:claude
    pub(crate) fn load(root: &Path) -> Self {
        Self::from_body(crate::trusted_config::read_trusted_config_toml_local(root).as_deref())
    }

    /// Whether the project sets the drain default at all (any listed type).
    pub(crate) fn drains_any(&self) -> bool {
        self.default == UnsetDefault::Drain && !self.drain_types.is_empty()
    }

    /// The effective gating mode of `req`, given `gating` — the mode the
    /// merge gates already read for it (its own, else its carrier's,
    /// BUG-2413). A set mode is returned unchanged; an unset one gets the
    /// project default only for a listed, non-sensitive type.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn resolve(
        &self,
        gating: Option<ExecutionMode>,
        req: &Requirement,
    ) -> EffectiveMode {
        if gating.is_some() {
            return EffectiveMode::from(gating);
        }
        if !self.drains_any() {
            return EffectiveMode::from(None);
        }
        let word = type_word(&req.req_type);
        let withheld = if !self.drain_types.contains(&req.req_type) {
            Some(format!(
                "the project's drain default does not cover type {word}"
            ))
        } else {
            sensitive_class(req).map(|why| format!("not defaulted, {why}"))
        };
        match withheld {
            Some(why) => EffectiveMode {
                mode: None,
                source: ModeSource::Unset {
                    withheld: Some(why),
                },
            },
            None => EffectiveMode {
                mode: Some(ExecutionMode::Drain),
                source: ModeSource::ProjectDefault { req_type: word },
            },
        }
    }
}

/// STORY-2884: the effective-mode line `aida show` prints for a spec with no
/// mode of its own, when the project sets a drain default; `None` otherwise
/// (a set mode prints as before, and without the setting nothing changes).
// trace:STORY-2884 | ai:claude
pub(crate) fn show_label(root: &Path, req: &Requirement) -> Option<String> {
    if req.execution_mode.is_some() {
        return None;
    }
    let policy = UnsetModePolicy::load(root);
    policy
        .drains_any()
        .then(|| policy.resolve(None, req).label())
}

/// Why a spec is sensitive and never takes the project default: the classes
/// a mandate excludes unless minted with `--include-supervised`
/// (keystone, security, authority, containment, ...) or treats as
/// always-human (`operator-only`).
// trace:STORY-2884 | ai:claude
fn sensitive_class(req: &Requirement) -> Option<String> {
    crate::gate::mandate::always_human_class(req)
        .map(|class| format!("{class} spec"))
        .or_else(|| crate::gate::mandate::excluded_class(req))
}

/// Where an effective gating mode came from.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModeSource {
    /// The spec's own mode, or its carrier's.
    Spec,
    /// No mode set; the project's `[gate.unset_mode]` drain default applies.
    ProjectDefault { req_type: String },
    /// No mode set and no default applies. `withheld` says why when the
    /// project has a drain default that did not apply.
    Unset { withheld: Option<String> },
}

/// The mode a merge gate decides on, with its provenance.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectiveMode {
    pub(crate) mode: Option<ExecutionMode>,
    pub(crate) source: ModeSource,
}

impl From<Option<ExecutionMode>> for EffectiveMode {
    /// A mode as set on the spec (unset stays unset, supervised).
    fn from(mode: Option<ExecutionMode>) -> Self {
        let source = match mode {
            Some(_) => ModeSource::Spec,
            None => ModeSource::Unset { withheld: None },
        };
        Self { mode, source }
    }
}

/// Evidence records keep their old shape: the effective mode alone.
impl serde::Serialize for EffectiveMode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.mode.serialize(s)
    }
}

impl EffectiveMode {
    /// The label the refusal text and `aida show` print, e.g. `drive`,
    /// `unset (supervised)`, or `drain (project default for unset bug)`.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn label(&self) -> String {
        match (&self.source, self.mode) {
            (ModeSource::ProjectDefault { req_type }, _) => {
                format!("drain (project default for unset {req_type})")
            }
            (
                ModeSource::Unset {
                    withheld: Some(why),
                },
                _,
            ) => {
                format!("unset (supervised: {why})")
            }
            (_, mode) => crate::gate::ship::supervision_mode_label(mode),
        }
    }

    /// True when this mode holds an unattended merge for a human.
    pub(crate) fn requires_supervision(&self) -> bool {
        crate::gate::ship::merge_requires_supervision(self.mode)
    }

    /// True when the drain comes from the project default, not the spec.
    pub(crate) fn is_project_default(&self) -> bool {
        matches!(self.source, ModeSource::ProjectDefault { .. })
    }

    /// Withdraw a project-default drain (back to unset, supervised) with the
    /// reason. A mode the spec sets is never touched.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn withdraw(&mut self, why: String) {
        if self.is_project_default() {
            self.mode = None;
            self.source = ModeSource::Unset {
                withheld: Some(why),
            };
        }
    }
}

/// Why a change's paths withdraw the project default: they touch a protected
/// gate path, or they could not be read. `None`: the default stands.
// trace:STORY-2884 | ai:claude
pub(crate) fn protected_path_reason(
    paths: Option<&[String]>,
    protected: &crate::gate::mandate_gates::ProtectedPaths,
) -> Option<String> {
    match paths {
        None => Some("not defaulted, the change's files could not be read".into()),
        Some(paths) => crate::gate::mandate_gates::touches_gates(paths, protected)
            .map(|hit| format!("not defaulted, the change touches {}", hit.describe())),
    }
}

/// Withdraw every project-default drain in `modes` when the change touches a
/// protected gate path or its files cannot be read. `paths` (the change's
/// files; `None` when unknown) is called only when some mode is a project
/// default, so a project without the setting pays no git read.
// trace:STORY-2884 | ai:claude
pub(crate) fn withdraw_defaults_for_change<'a>(
    modes: impl IntoIterator<Item = &'a mut EffectiveMode>,
    root: &Path,
    paths: impl FnOnce() -> Option<Vec<String>>,
) {
    let mut defaulted: Vec<&mut EffectiveMode> = modes
        .into_iter()
        .filter(|m| m.is_project_default())
        .collect();
    if defaulted.is_empty() {
        return;
    }
    let paths = paths();
    let protected = protected_paths(root);
    if let Some(why) = protected_path_reason(paths.as_deref(), &protected) {
        for mode in &mut defaulted {
            mode.withdraw(why.clone());
        }
    }
}

/// The protected gate paths of the project at `root` (the floor when the
/// gate config is unreadable — fail closed).
// trace:STORY-2884 | ai:claude
pub(crate) fn protected_paths(root: &Path) -> crate::gate::mandate_gates::ProtectedPaths {
    crate::gate::mandate_gates::ProtectedPaths::of(&crate::gate::mandate_gates::load_human_gates(
        root,
    ))
}

/// Files `head` changes against its merge base with `base` (a ref such as
/// `origin/main`). `None` when git cannot say — the caller withdraws the
/// project default.
// trace:STORY-2884 | ai:claude
pub(crate) fn changed_paths(root: &Path, base: &str, head: &str) -> Option<Vec<String>> {
    let (base, head) = (base.trim(), head.trim());
    if base.is_empty() || head.is_empty() || base.starts_with('-') || head.starts_with('-') {
        return None;
    }
    let out = aida_core::git_ops::host_git_command(root)
        .args(["diff", "--name-only", "-z", "--no-renames"])
        .arg(format!("{base}...{head}"))
        .arg("--")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        out.stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect(),
    )
}

#[cfg(test)]
#[path = "tests/story_2884_unset_mode_tests.rs"]
mod tests;
