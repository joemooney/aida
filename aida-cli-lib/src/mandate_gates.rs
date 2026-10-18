//! STORY-1700 (ADR-77): the human gates a drive mandate never acts past.
//!
//! `[mandate.human_gates] ops = [...]` lists the actions that stay human
//! even under a mandate; it is empty by default (gates are opt-in). Agents
//! must not be able to loosen them (R1), so the layers are:
//!
//! - the COMMITTED layer, read from the `origin/main` blob of
//!   `.aida/config.toml` — never the working tree, so an edit on a branch or
//!   an uncommitted edit in the main checkout changes nothing;
//! - the OPERATOR layer, `~/.aida/config.toml`.
//!
//! The result is their union: a layer can only add gates. An unreadable
//! committed layer is an `Err`, which every caller treats as "every op
//! gated" (fail closed). [`GATE_PROTECTED_PATHS`] names the files a PR may
//! not touch and still ship under a mandate.
// trace:STORY-1700 | ai:claude

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// An action a human gate can keep human.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum GateOp {
    Ship,
    SketchSignoff,
    Dispatch,
    /// Listed so an operator can state it; drive never clears holds.
    HoldClear,
}

impl GateOp {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ship => "ship",
            Self::SketchSignoff => "sketch-signoff",
            Self::Dispatch => "dispatch",
            Self::HoldClear => "hold-clear",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        [
            Self::Ship,
            Self::SketchSignoff,
            Self::Dispatch,
            Self::HoldClear,
        ]
        .into_iter()
        .find(|op| op.as_str() == raw.trim())
    }
}

/// The union of both layers.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HumanGates {
    pub ops: BTreeSet<GateOp>,
    /// `[mandate.drive] max_actions_per_tick`, honoured from the operator
    /// layer only.
    pub max_actions_per_tick: Option<usize>,
}

impl HumanGates {
    pub(crate) fn gates(&self, op: GateOp) -> bool {
        self.ops.contains(&op)
    }

    pub(crate) fn describe(&self) -> String {
        if self.ops.is_empty() {
            "none".into()
        } else {
            self.ops
                .iter()
                .map(|op| op.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

/// One parsed layer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GateLayer {
    pub ops: BTreeSet<GateOp>,
    pub max_actions_per_tick: Option<usize>,
}

/// Parse the `[mandate]` table of one config layer. An unknown op, an
/// unknown key under `[mandate]`, `[mandate.human_gates]` or `[mandate.drive]`,
/// or a mistyped value is an error.
// trace:STORY-1700 | ai:claude
pub(crate) fn parse_layer(text: &str, source: &str) -> Result<GateLayer> {
    let value: toml::Value =
        toml::from_str(text).with_context(|| format!("{source} is not valid TOML"))?;
    let mut layer = GateLayer::default();
    let Some(mandate) = value.get("mandate") else {
        return Ok(layer);
    };
    let mandate = mandate
        .as_table()
        .with_context(|| format!("{source}: [mandate] must be a table"))?;
    for (key, section) in mandate {
        let table = section
            .as_table()
            .with_context(|| format!("{source}: [mandate.{key}] must be a table"))?;
        match key.as_str() {
            "human_gates" => {
                for (k, v) in table {
                    match k.as_str() {
                        "ops" => {
                            let ops = v.as_array().with_context(|| {
                                format!("{source}: [mandate.human_gates] ops must be a list")
                            })?;
                            for op in ops {
                                let raw = op.as_str().with_context(|| {
                                    format!(
                                        "{source}: [mandate.human_gates] ops holds a non-string"
                                    )
                                })?;
                                let Some(op) = GateOp::parse(raw) else {
                                    bail!(
                                        "{source}: [mandate.human_gates] unknown op `{raw}` (known: ship, sketch-signoff, dispatch, hold-clear)"
                                    );
                                };
                                layer.ops.insert(op);
                            }
                        }
                        other => bail!("{source}: [mandate.human_gates] unknown key `{other}`"),
                    }
                }
            }
            "drive" => {
                for (k, v) in table {
                    match k.as_str() {
                        "max_actions_per_tick" => {
                            let n = v.as_integer().filter(|n| *n > 0).with_context(|| {
                                format!(
                                    "{source}: [mandate.drive] max_actions_per_tick must be a positive integer"
                                )
                            })?;
                            layer.max_actions_per_tick = Some(n as usize);
                        }
                        other => bail!("{source}: [mandate.drive] unknown key `{other}`"),
                    }
                }
            }
            other => bail!("{source}: unknown table [mandate.{other}]"),
        }
    }
    Ok(layer)
}

/// The union of the two layers. Only the operator layer sets the budget.
// trace:STORY-1700 | ai:claude
pub(crate) fn union(committed: GateLayer, operator: GateLayer) -> HumanGates {
    let mut ops = committed.ops;
    ops.extend(operator.ops);
    HumanGates {
        ops,
        max_actions_per_tick: operator.max_actions_per_tick,
    }
}

const COMMITTED_REF: &str = "origin/main";
const CONFIG_PATH: &str = ".aida/config.toml";

/// The committed layer: `origin/main:.aida/config.toml`. `Err` when the ref
/// cannot be read; a ref whose tree has no config file is an empty layer.
// trace:STORY-1700 | ai:claude
pub(crate) fn committed_layer(project_root: &Path) -> Result<GateLayer> {
    let has_ref = aida_core::git_ops::host_git_command(project_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{COMMITTED_REF}^{{commit}}"))
        .output()
        .context("git failed to run")?;
    if !has_ref.status.success() {
        bail!("no {COMMITTED_REF} ref to read the committed gate layer from");
    }
    let listed = aida_core::git_ops::host_git_command(project_root)
        .args(["ls-tree", "--name-only", COMMITTED_REF, "--", CONFIG_PATH])
        .output()
        .context("git failed to run")?;
    if !listed.status.success() {
        bail!(
            "cannot list {COMMITTED_REF}:{CONFIG_PATH}: {}",
            String::from_utf8_lossy(&listed.stderr).trim()
        );
    }
    if String::from_utf8_lossy(&listed.stdout).trim().is_empty() {
        return Ok(GateLayer::default());
    }
    let text = crate::history::git_show_blob(project_root, COMMITTED_REF, CONFIG_PATH)?;
    parse_layer(&text, &format!("{COMMITTED_REF}:{CONFIG_PATH}"))
}

/// The operator layer: `~/.aida/config.toml`; absent = empty.
// trace:STORY-1700 | ai:claude
pub(crate) fn operator_layer() -> Result<GateLayer> {
    let home = crate::aida_home_dir().context("cannot locate the user's home directory")?;
    let path = home.join(".aida").join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_layer(&text, &path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GateLayer::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Both layers, unioned. `Err` = treat every op as gated.
// trace:STORY-1700 | ai:claude
pub(crate) fn load_human_gates(project_root: &Path) -> Result<HumanGates> {
    Ok(union(committed_layer(project_root)?, operator_layer()?))
}

/// Why `op` is gated, if it is. `Err` gates everything.
// trace:STORY-1700 | ai:claude
pub(crate) fn gate_reason(
    gates: &std::result::Result<HumanGates, String>,
    op: GateOp,
) -> Option<String> {
    match gates {
        Err(why) => Some(format!("gate config unreadable: {why}")),
        Ok(g) if g.gates(op) => Some(format!(
            "`{}` is listed in [mandate.human_gates]",
            op.as_str()
        )),
        Ok(_) => None,
    }
}

/// For the ship path: why `ship` is gated right now in the main checkout
/// at `main_worktree`, if it is.
// trace:STORY-1700 | ai:claude
pub(crate) fn ship_gate(main_worktree: &Path) -> Option<String> {
    let gates = load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    gate_reason(&gates, GateOp::Ship)
}

/// The human gates in force, for display.
// trace:STORY-1700 | ai:claude
pub(crate) fn describe(main_worktree: Option<&Path>) -> String {
    match main_worktree.map(load_human_gates) {
        Some(Ok(g)) => g.describe(),
        Some(Err(_)) | None => "unreadable (every action gated)".into(),
    }
}

/// Files a PR may not change and still ship under a mandate (R1, condition
/// C1). `.aida/config.toml` counts whole: the gate table lives in it and a
/// path filter cannot see which table a hunk touches.
// trace:STORY-1700 | ai:claude
pub(crate) const GATE_PROTECTED_PATHS: &[&str] = &[
    ".aida/config.toml",
    "aida-cli-lib/src/mandate.rs",
    "aida-cli-lib/src/mandate_gates.rs",
    "aida-cli-lib/src/mandate_drive.rs",
    "aida-cli-lib/src/mandate_ship.rs",
    "aida-cli-lib/src/fleet_lanes.rs",
    "aida-cli-lib/src/seat_authority.rs",
    "aida-cli-lib/src/pr_ship.rs",
    "aida-cli-lib/src/merge_hold.rs",
];

/// The first protected path in `paths`, if any.
// trace:STORY-1700 | ai:claude
pub(crate) fn touches_gates(paths: &[String]) -> Option<String> {
    paths
        .iter()
        .map(|p| p.trim().trim_start_matches("./"))
        .find(|p| GATE_PROTECTED_PATHS.contains(p))
        .map(str::to_string)
}
