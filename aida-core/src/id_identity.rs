//! Numeric spec-ID identity (BUG-HOME-046).
//!
//! A spec ID is `PREFIX-SEQ` where PREFIX is one or more `-`-separated
//! segments (`TASK`, `TASK-<node>`, `FR-1`, `$USER`) and SEQ is the trailing
//! run of digits. Two IDs are the SAME ID when their prefixes match
//! case-insensitively and their sequences are numerically equal: leading zeros
//! never matter, so `TASK-0042`, `TASK-42` and `TASK-00042` name one spec.
//!
//! Stored IDs and object filenames keep whatever padding they were minted
//! with; identity is computed, never written. Every comparison between two
//! spec IDs (lookups, dedup, collision scans) goes through [`same_id`] or
//! [`id_key`], and every path that MINTS an ID goes through [`allocate_id`]
//! (dispensed numbers) or [`admit_new_id`] (caller-supplied IDs) — the single
//! allocator-facing seam STORY-HOME-105's block dispenser reuses.
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude

use anyhow::{bail, Result};
use uuid::Uuid;

/// The numeric identity of a spec ID: upper-cased prefix plus the sequence
/// with leading zeros stripped (kept as text, so any width compares).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IdKey {
    /// Everything before the last `-`, upper-cased (`TASK`, `TASK-<NODE>`).
    pub prefix: String,
    /// The sequence digits without leading zeros (`"0"` for an all-zero run).
    pub seq: String,
}

impl IdKey {
    /// Parse `TYPE-N`, `TYPE-NODE-N`, `FEAT-TYPE-N` or `$META-N` with any
    /// number of leading zeros. `None` for anything that is not ID-shaped
    /// (no `-`, empty prefix, a non-digit or empty sequence).
    pub fn parse(id: &str) -> Option<IdKey> {
        let id = id.trim();
        let (prefix, seq) = id.rsplit_once('-')?;
        let bare = prefix.strip_prefix('$').unwrap_or(prefix);
        if bare.is_empty()
            || !bare.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            || prefix.split('-').any(str::is_empty)
            || !prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '$' || c == '_')
        {
            return None;
        }
        if seq.is_empty() || !seq.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let stripped = seq.trim_start_matches('0');
        Some(IdKey {
            prefix: prefix.to_ascii_uppercase(),
            seq: if stripped.is_empty() { "0" } else { stripped }.to_string(),
        })
    }

    /// Whether the prefix is node-local (`TYPE-<32-hex node>`).
    pub fn is_node_local(&self) -> bool {
        self.prefix.split_once('-').is_some_and(|(_, node)| {
            node.len() == 32 && node.bytes().all(|b| b.is_ascii_hexdigit())
        })
    }

    /// Render with the sequence zero-padded to at least `width` digits
    /// (0 = no padding). Width is a minimum: a longer sequence prints whole.
    pub fn render(&self, width: usize) -> String {
        format!("{}-{:0>width$}", self.prefix, self.seq, width = width)
    }
}

impl std::fmt::Display for IdKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.prefix, self.seq)
    }
}

/// The canonical identity text of `id` (`TASK-42` for `task-0042`), usable
/// as a map key. A non-ID string keys as itself, trimmed and upper-cased, so
/// callers can key arbitrary handles without a separate branch.
pub fn id_key(id: &str) -> String {
    match IdKey::parse(id) {
        Some(key) => key.to_string(),
        None => id.trim().to_ascii_uppercase(),
    }
}

/// Whether `a` and `b` name the same spec: numeric identity for ID-shaped
/// input, case-insensitive text equality otherwise.
pub fn same_id(a: &str, b: &str) -> bool {
    match (IdKey::parse(a), IdKey::parse(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

/// Refusal raised when a new ID's number is already held by another spec
/// under any padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdTakenError {
    pub candidate: String,
    pub owner: Uuid,
}

impl std::fmt::Display for IdTakenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "spec ID {} is already used by requirement {} (IDs differing only by leading zeros are the same ID); nothing was written",
            self.candidate.trim().to_ascii_uppercase(),
            self.owner
        )
    }
}

impl std::error::Error for IdTakenError {}

/// THE mint-time admission check: refuse `candidate` when a DIFFERENT
/// requirement already holds its [`IdKey`] (as `spec_id` or `agreed_id`,
/// under any padding). `taken` answers "which UUID holds this key?";
/// `owner` is the requirement being written (re-saving its own ID is fine).
pub fn admit_new_id(
    candidate: &str,
    taken: &dyn Fn(&str) -> Result<Option<Uuid>>,
    owner: Uuid,
) -> Result<()> {
    if let Some(holder) = taken(candidate)? {
        if holder != owner {
            return Err(IdTakenError {
                candidate: candidate.to_string(),
                owner: holder,
            }
            .into());
        }
    }
    Ok(())
}

/// How many dispensed numbers [`allocate_id`] skips before giving up.
pub const MAX_ALLOCATION_SKIPS: usize = 64;

/// THE mint seam for dispensed IDs: draw from `mint` until a candidate passes
/// [`admit_new_id`], skipping numbers already used under any padding (the
/// dispenser has advanced past them; they are never handed out). Errors after
/// [`MAX_ALLOCATION_SKIPS`] consecutive taken numbers — a counter that far
/// behind the store needs repair, not a silent scan.
pub fn allocate_id(
    mut mint: impl FnMut() -> Result<String>,
    taken: &dyn Fn(&str) -> Result<Option<Uuid>>,
    owner: Uuid,
) -> Result<String> {
    let mut skipped = Vec::new();
    for _ in 0..=MAX_ALLOCATION_SKIPS {
        let candidate = mint()?;
        match admit_new_id(&candidate, taken, owner) {
            Ok(()) => {
                if !skipped.is_empty() {
                    eprintln!(
                        "note: skipped spec ID(s) already in use under another padding: {}",
                        skipped.join(", ")
                    );
                }
                return Ok(candidate);
            }
            Err(e) if e.downcast_ref::<IdTakenError>().is_some() => skipped.push(candidate),
            Err(e) => return Err(e),
        }
    }
    bail!(
        "ID allocation skipped {} numbers already in use ({}); the counter is behind the store — run `aida doctor`",
        skipped.len(),
        skipped.join(", ")
    )
}

// ---------------------------------------------------------------------------
// Width settings ([ids] in .aida/config.toml)
// ---------------------------------------------------------------------------

/// `[ids]` widths: `seq_width` for legacy/global IDs and `node_seq_width`
/// for node-local IDs. Each governs BOTH minting and rendering:
///
/// - **Mint:** a new ID's sequence is zero-padded to the width; an unset
///   width mints at [`DEFAULT_MINT_WIDTH`] (3, today's `FR-001` look).
/// - **Render:** a configured width re-pads every displayed ID; an UNSET
///   width renders the stored ID unchanged, so a project that never sets
///   `[ids]` keeps today's look and its displayed IDs match the filenames.
///
/// Stored IDs and filenames are never re-padded. This replaced the legacy
/// `id_config.digits` knob (metadata.yaml), which no longer drives minting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IdWidths {
    pub seq_width: Option<usize>,
    pub node_seq_width: Option<usize>,
}

/// Mint width when `[ids]` leaves a width unset (legacy and node-local).
pub const DEFAULT_MINT_WIDTH: usize = 3;

/// Upper bound for a configured width (keeps a typo from rendering 1000 zeros).
pub const MAX_SEQ_WIDTH: usize = 12;

impl IdWidths {
    /// Read `[ids] seq_width` / `node_seq_width` from a project's
    /// `.aida/config.toml` text. Missing keys stay unset; values are clamped
    /// to [`MAX_SEQ_WIDTH`].
    pub fn from_config_toml(text: &str) -> IdWidths {
        let mut out = IdWidths::default();
        let Ok(value) = text.parse::<toml::Table>() else {
            return out;
        };
        let Some(ids) = value.get("ids").and_then(|v| v.as_table()) else {
            return out;
        };
        let read = |k: &str| {
            ids.get(k)
                .and_then(|v| v.as_integer())
                .filter(|n| *n >= 0)
                .map(|n| (n as usize).min(MAX_SEQ_WIDTH))
        };
        out.seq_width = read("seq_width");
        out.node_seq_width = read("node_seq_width");
        out
    }

    /// Load from `<project>/.aida/config.toml` (unset when absent).
    #[cfg(feature = "native")]
    pub fn load(project_root: &std::path::Path) -> IdWidths {
        std::fs::read_to_string(project_root.join(".aida/config.toml"))
            .map(|t| IdWidths::from_config_toml(&t))
            .unwrap_or_default()
    }

    /// Mint width for legacy/global IDs.
    pub fn mint_width(&self) -> usize {
        self.seq_width.unwrap_or(DEFAULT_MINT_WIDTH)
    }

    /// Mint width for node-local IDs.
    pub fn node_mint_width(&self) -> usize {
        self.node_seq_width.unwrap_or(DEFAULT_MINT_WIDTH)
    }

    /// The configured render width for `key` (`None` = render the stored
    /// text unchanged).
    pub fn width_for(&self, key: &IdKey) -> Option<usize> {
        if key.is_node_local() {
            self.node_seq_width
        } else {
            self.seq_width
        }
    }

    /// `id` rendered at the configured width. A non-ID string, or an ID whose
    /// width is unset, is returned unchanged; the prefix keeps the input's
    /// text (case included) so display labels survive.
    pub fn render(&self, id: &str) -> String {
        let Some(key) = IdKey::parse(id) else {
            return id.to_string();
        };
        let Some(width) = self.width_for(&key) else {
            return id.to_string();
        };
        let trimmed = id.trim();
        let prefix = trimmed.rsplit_once('-').map_or(trimmed, |(p, _)| p);
        format!("{prefix}-{:0>width$}", key.seq)
    }
}

fn widths_slot() -> &'static std::sync::RwLock<Option<IdWidths>> {
    static SLOT: std::sync::OnceLock<std::sync::RwLock<Option<IdWidths>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::RwLock::new(None))
}

thread_local! {
    static THREAD_WIDTHS: std::cell::Cell<Option<IdWidths>> = const { std::cell::Cell::new(None) };
}

/// Install the project's widths for this process (the CLI/MCP do this next
/// to the node-alias map).
pub fn install_widths(widths: IdWidths) {
    if let Ok(mut g) = widths_slot().write() {
        *g = Some(widths);
    }
}

/// Install widths for the current thread only (tests); returns the previous.
pub fn install_widths_for_thread(widths: Option<IdWidths>) -> Option<IdWidths> {
    THREAD_WIDTHS.with(|t| t.replace(widths))
}

/// The installed widths (thread-scoped first), or the defaults.
pub fn installed_widths() -> IdWidths {
    if let Some(w) = THREAD_WIDTHS.with(|t| t.get()) {
        return w;
    }
    widths_slot()
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_default()
}

/// `id` rendered at the installed width (see [`IdWidths::render`]).
pub fn render_id(id: &str) -> String {
    installed_widths().render(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_zeros_are_insignificant() {
        let a = IdKey::parse("TASK-0418").unwrap();
        assert_eq!(a, IdKey::parse("TASK-418").unwrap());
        assert_eq!(a, IdKey::parse("task-00418").unwrap());
        assert_eq!(a.to_string(), "TASK-418");
        assert!(same_id("TASK-0418", "task-418"));
        assert!(!same_id("TASK-418", "TASK-4180"));
        assert!(!same_id("TASK-418", "BUG-418"));
    }

    #[test]
    fn node_local_and_meta_ids_parse() {
        let node = "0123456789abcdef0123456789abcdef";
        let k = IdKey::parse(&format!("BUG-{node}-036")).unwrap();
        assert!(k.is_node_local());
        assert!(same_id(
            &format!("BUG-{node}-36"),
            &format!("bug-{node}-0036")
        ));
        assert!(same_id("$USER-001", "$user-1"));
        assert!(same_id("FR-1-042", "FR-1-42"));
        assert!(!same_id("FR-1-042", "FR-2-042"));
    }

    #[test]
    fn non_ids_are_rejected_and_compare_as_text() {
        for s in ["", "TASK", "TASK-", "-12", "TASK-12a", "12-34", "TASK--3"] {
            assert!(IdKey::parse(s).is_none(), "{s:?} must not parse");
        }
        assert!(same_id("hello", "HELLO"));
        assert!(!same_id("hello", "TASK-1"));
        assert_eq!(id_key("task-007"), "TASK-7");
        assert_eq!(id_key(" free-form "), "FREE-FORM");
        assert_eq!(IdKey::parse("TASK-000").unwrap().seq, "0");
    }

    #[test]
    fn render_pads_to_a_minimum_width() {
        let k = IdKey::parse("TASK-0042").unwrap();
        assert_eq!(k.render(0), "TASK-42");
        assert_eq!(k.render(3), "TASK-042");
        assert_eq!(k.render(4), "TASK-0042");
        assert_eq!(IdKey::parse("TASK-2357").unwrap().render(3), "TASK-2357");
    }

    #[test]
    fn widths_render_legacy_and_node_local_separately() {
        let node = "0123456789ABCDEF0123456789ABCDEF";
        let w = IdWidths::from_config_toml("[ids]\nseq_width = 0\nnode_seq_width = 3\n");
        assert_eq!(w.render("TASK-0418"), "TASK-418");
        assert_eq!(
            w.render(&format!("BUG-{node}-36")),
            format!("BUG-{node}-036")
        );
        let w = IdWidths::from_config_toml("[ids]\nseq_width = 4\nnode_seq_width = 0\n");
        assert_eq!(w.render("TASK-418"), "TASK-0418");
        assert_eq!(
            w.render(&format!("BUG-{node}-036")),
            format!("BUG-{node}-36")
        );
        assert_eq!(w.render("not an id"), "not an id");
        assert_eq!(IdWidths::from_config_toml("garbage ="), IdWidths::default());
        assert_eq!(
            IdWidths::from_config_toml("[ids]\nseq_width = 999\n").seq_width,
            Some(MAX_SEQ_WIDTH)
        );
    }

    /// With no `[ids]` table the stored text renders unchanged (FR-001,
    /// FR-1-042 and TASK-0042 keep their look and match their files), and
    /// minting keeps the historical 3-digit width.
    #[test]
    fn unset_widths_preserve_the_stored_look() {
        let w = IdWidths::default();
        let node = "BUG-0123456789ABCDEF0123456789ABCDEF-036";
        for id in ["FR-001", "FR-1-042", "TASK-0418", "TASK-418", node] {
            assert_eq!(w.render(id), id);
        }
        assert_eq!((w.mint_width(), w.node_mint_width()), (3, 3));
        let w = IdWidths::from_config_toml("[ids]\nseq_width = 0\n");
        assert_eq!((w.mint_width(), w.node_mint_width()), (0, 3));
        assert_eq!(w.render("FR-1-042"), "FR-1-42");
        assert_eq!(w.render(node), node);
    }

    fn taken_by(held: Vec<(&'static str, Uuid)>) -> impl Fn(&str) -> Result<Option<Uuid>> {
        move |id: &str| Ok(held.iter().find(|(h, _)| same_id(h, id)).map(|(_, u)| *u))
    }

    /// AC2 regression: a store holding `TASK-0005` and a dispenser that hands
    /// out 5 mints `TASK-6`, never a padded twin.
    #[test]
    fn allocate_skips_a_number_used_under_other_padding() {
        let other = Uuid::new_v4();
        let taken = taken_by(vec![("TASK-0005", other)]);
        let mut next = 5;
        let minted = allocate_id(
            || {
                let id = format!("TASK-{next}");
                next += 1;
                Ok(id)
            },
            &taken,
            Uuid::new_v4(),
        )
        .unwrap();
        assert_eq!(minted, "TASK-6");
    }

    #[test]
    fn explicit_id_under_other_padding_is_refused() {
        let other = Uuid::new_v4();
        let taken = taken_by(vec![("TASK-0005", other)]);
        let err = admit_new_id("TASK-5", &taken, Uuid::new_v4()).unwrap_err();
        assert!(err.downcast_ref::<IdTakenError>().is_some());
        assert!(err.to_string().contains("TASK-5"), "{err}");
        // Re-saving the holder's own id is not a collision.
        admit_new_id("TASK-05", &taken, other).unwrap();
    }

    #[test]
    fn allocate_gives_up_after_the_skip_budget() {
        let taken = |_: &str| Ok(Some(Uuid::nil()));
        let mut n = 0;
        let err = allocate_id(
            || {
                n += 1;
                Ok(format!("TASK-{n}"))
            },
            &taken,
            Uuid::new_v4(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("counter is behind"), "{err}");
    }
}
