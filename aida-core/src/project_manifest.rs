//! The AIDA project manifest — `.aida/project.toml`.
//!
//! A checked-in, human-edited statement of what a project IS: description, why
//! it exists, whether it is alive, where it is in its lifecycle, who owns it.
//! These are precisely the facts no scanner can derive, and until now they were
//! either absent or duplicated ad-hoc into whichever consumer needed them.
//!
//! # Why AIDA owns this, and consumers only read it
//!
//! If a consumer defined the format, every project would have to adopt that
//! consumer in order to be well-described. `aida init` is a command these
//! repositories already run, so the standard rides in on adoption that is
//! already happening, and any future reader — a catalogue, a TUI, a phone
//! client, a `curl` one-liner — reads the same file.
//!
//! This mirrors the stance downstream consumers already take toward
//! `~/.ports`: the port manager writes it, everyone else reads it.
//!
//! # Design constraints, all load-bearing
//!
//! - **Absence is not a defect.** A project without a manifest behaves exactly
//!   as it did before. No warning, no error, no lowered score. [`load`] returns
//!   [`ManifestState::Absent`], which callers treat as "nothing to say".
//! - **A malformed manifest never breaks a command.** [`load`] does not return
//!   `Result`; a parse failure becomes [`ManifestState::Malformed`] carrying the
//!   message. Making it unrepresentable is how the guarantee stays true, rather
//!   than depending on every caller remembering to catch.
//! - **Readable without reverse engineering.** Flat `[project]` table, plain
//!   TOML, every field optional, documented in `docs/project-manifest.md`.
//! - **Never a blank form.** The scaffolder pre-fills from what is already
//!   knowable. An empty form is exactly how a metadata standard goes stale in a
//!   week, which is the failure this exists to prevent.
//!
//! trace:STORY-781 | ai:claude

use serde::{Deserialize, Serialize};

/// Path of the manifest relative to the project root.
pub const MANIFEST_REL_PATH: &str = ".aida/project.toml";

/// Schema version written by this build.
///
/// Present so a future reader can branch on the shape rather than sniffing
/// fields. Bump only on a breaking change; adding an optional field is not one.
pub const SCHEMA_VERSION: u32 = 1;

/// Whether a project is still being worked on.
///
/// Deliberately coarse — the question is "should I care about this?", not a
/// lifecycle model. Absent means "not stated", which is different from every
/// stated value and must not be rendered as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// Actively worked on, or intended to be.
    Alive,
    /// Deliberately paused. Coming back to it is plausible.
    Parked,
    /// Done with. Kept for reference, not for work.
    Abandoned,
    /// A value this build does not know, PRESERVED rather than rejected.
    ///
    /// A closed enum here would mean one typo — or one value added by a newer
    /// AIDA — makes the whole manifest unreadable, losing `description`, `why`
    /// and everything else with it. That is "reject" where the contract for
    /// every reader of this format is "degrade": keep what you understood, say
    /// what you did not. `aida doctor` reports these so the author finds out.
    Unrecognised(String),
}

impl Liveness {
    pub fn as_str(&self) -> &str {
        match self {
            Liveness::Alive => "alive",
            Liveness::Parked => "parked",
            Liveness::Abandoned => "abandoned",
            Liveness::Unrecognised(s) => s,
        }
    }

    pub fn is_recognised(&self) -> bool {
        !matches!(self, Liveness::Unrecognised(_))
    }

    /// Every value this build understands, for docs and messages.
    pub const ALL: &'static [&'static str] = &["alive", "parked", "abandoned"];
}

impl std::str::FromStr for Liveness {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "alive" => Liveness::Alive,
            "parked" => Liveness::Parked,
            "abandoned" => Liveness::Abandoned,
            _ => Liveness::Unrecognised(s.to_string()),
        })
    }
}

impl Serialize for Liveness {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Liveness {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(s.parse().unwrap_or(Liveness::Unrecognised(s)))
    }
}

/// Where a project sits in its life, independent of whether anyone is
/// currently working on it.
///
/// `Alpha` and `Parked`, for instance, are orthogonal: a paused project still
/// has a maturity.
/// See [`Liveness::Unrecognised`] for why this is not a closed enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Not built yet — a repository holding a thought.
    Idea,
    /// Exploring whether it works at all.
    Prototype,
    /// Works, interface still moving.
    Alpha,
    /// Interface mostly settled, in use.
    Beta,
    /// Settled and relied upon.
    Stable,
    /// Complete; changes are fixes, not features.
    Maintenance,
    /// On the way out; do not build on it.
    Sunset,
    /// A value this build does not know, preserved rather than rejected.
    Unrecognised(String),
}

impl Stage {
    pub fn as_str(&self) -> &str {
        match self {
            Stage::Idea => "idea",
            Stage::Prototype => "prototype",
            Stage::Alpha => "alpha",
            Stage::Beta => "beta",
            Stage::Stable => "stable",
            Stage::Maintenance => "maintenance",
            Stage::Sunset => "sunset",
            Stage::Unrecognised(s) => s,
        }
    }

    pub fn is_recognised(&self) -> bool {
        !matches!(self, Stage::Unrecognised(_))
    }

    pub const ALL: &'static [&'static str] = &[
        "idea",
        "prototype",
        "alpha",
        "beta",
        "stable",
        "maintenance",
        "sunset",
    ];
}

impl std::str::FromStr for Stage {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "idea" => Stage::Idea,
            "prototype" => Stage::Prototype,
            "alpha" => Stage::Alpha,
            "beta" => Stage::Beta,
            "stable" => Stage::Stable,
            "maintenance" => Stage::Maintenance,
            "sunset" => Stage::Sunset,
            _ => Stage::Unrecognised(s.to_string()),
        })
    }
}

impl Serialize for Stage {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Stage {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(s.parse().unwrap_or(Stage::Unrecognised(s)))
    }
}

/// The `[project]` table.
///
/// EVERY FIELD IS OPTIONAL. A manifest carrying only a description is valid and
/// useful; requiring fields would turn this into a form, and forms go stale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSection {
    /// Display name. Defaults to the directory name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// One line: what this is. Derivable from a README, so the scaffolder
    /// pre-fills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Why this exists. THE field a scanner can never derive, and the reason
    /// the manifest is worth checking in at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness: Option<Liveness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
    /// Who to ask. Free text — a name, a handle, a team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// An author-asserted category. Free text ON PURPOSE: AIDA does not own a
    /// taxonomy here, and baking one consumer's vocabulary into the standard
    /// would make every other consumer wrong. Readers interpret it, or ignore
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// Canonical remote. Pre-filled from `origin`; lets a reader notice the
    /// repository moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// A parsed manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectManifest {
    /// Schema version. Absent is treated as 1 — the first published shape had
    /// it, but a hand-written file may reasonably omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    #[serde(default)]
    pub project: ProjectSection,
}

impl ProjectManifest {
    pub fn schema_version(&self) -> u32 {
        self.schema.unwrap_or(1)
    }

    /// True when the manifest exists but nobody has said anything a scan could
    /// not already work out.
    ///
    /// This is the "scaffolded and never filled in" state — the blank form the
    /// whole design is trying to avoid — and it is what `aida doctor` reports
    /// as stale.
    pub fn is_unfilled(&self) -> bool {
        let p = &self.project;
        blank(&p.why)
            && p.liveness.is_none()
            && p.stage.is_none()
            && blank(&p.owner)
            && blank(&p.classification)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

fn blank(v: &Option<String>) -> bool {
    v.as_deref()
        .map(str::trim)
        .map(|s| s.is_empty() || s.eq_ignore_ascii_case("NOT RECORDED"))
        .unwrap_or(true)
}

/// The result of looking for a manifest.
///
/// Note there is no error variant: every outcome is a state a caller can render.
/// See the module docs for why that is structural rather than stylistic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestState {
    /// No manifest. Entirely normal; not a defect.
    Absent,
    Present(Box<ProjectManifest>),
    /// Present but unreadable. Carries a message fit to show a human.
    Malformed(String),
}

impl ManifestState {
    pub fn manifest(&self) -> Option<&ProjectManifest> {
        match self {
            ManifestState::Present(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_absent(&self) -> bool {
        matches!(self, ManifestState::Absent)
    }
}

/// Interpret manifest text. Split from the filesystem so it is testable
/// without one.
pub fn parse_state(text: &str) -> ManifestState {
    match ProjectManifest::parse(text) {
        Ok(m) => ManifestState::Present(Box::new(m)),
        Err(e) => ManifestState::Malformed(e),
    }
}

/// Load the manifest for a project root. Never fails.
///
/// An unreadable file (permissions, a directory where a file should be) is
/// reported as [`ManifestState::Malformed`] rather than swallowed: the caller
/// should be able to tell "there is nothing here" from "there is something here
/// I could not read".
pub fn load(root: &std::path::Path) -> ManifestState {
    let path = root.join(MANIFEST_REL_PATH);
    if !path.exists() {
        return ManifestState::Absent;
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_state(&text),
        Err(e) => ManifestState::Malformed(format!("cannot read {}: {e}", path.display())),
    }
}

/// Facts already knowable about a project, used to pre-fill a new manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DerivedFacts {
    pub name: Option<String>,
    pub description: Option<String>,
    pub repository: Option<String>,
}

/// Render a manifest file, pre-filled from `facts`.
///
/// The commentary is part of the artifact, not decoration: the file has to be
/// editable by someone who has never read AIDA's docs, and every field a
/// scanner cannot derive needs to say what it is for at the point of editing.
/// Fields with no derived value are written COMMENTED OUT with their accepted
/// values, so the file is a filled-in example rather than a blank form.
pub fn render(facts: &DerivedFacts) -> String {
    let mut s = String::new();
    s.push_str(
        "# AIDA project manifest — what this project IS.\n\
         #\n\
         # Checked in on purpose: these are the facts no scan can derive, so they\n\
         # travel with the repository instead of living on one machine. Tools read\n\
         # this file; AIDA is the only thing that writes it, and only to create it.\n\
         # Your edits are never overwritten — `aida init` leaves an existing\n\
         # manifest alone.\n\
         #\n\
         # Every field is optional. Schema: docs/project-manifest.md\n\n",
    );
    s.push_str(&format!("schema = {SCHEMA_VERSION}\n\n[project]\n"));

    match &facts.name {
        Some(v) => s.push_str(&format!("name = {}\n", toml_str(v))),
        None => s.push_str("# name = \"\"\n"),
    }

    s.push_str("\n# One line: what this is.\n");
    match &facts.description {
        Some(v) => s.push_str(&format!("description = {}\n", toml_str(v))),
        None => s.push_str("description = \"\"\n"),
    }

    s.push_str(
        "\n# WHY it exists — the one thing no tool can work out for you, and the\n\
         # reason this file is worth keeping. What was the itch?\n\
         # NOT RECORDED means this item was skipped at init, not answered.\n\
         why = \"NOT RECORDED\"\n",
    );

    s.push_str(&format!(
        "\n# Is anyone working on this? One of: {}\n\
         # liveness = \"alive\"\n",
        Liveness::ALL.join(", ")
    ));

    s.push_str(&format!(
        "\n# How far along, independent of whether it is active. One of:\n\
         # {}\n\
         # stage = \"prototype\"\n",
        Stage::ALL.join(", ")
    ));

    s.push_str("\n# Who to ask.\n# owner = \"\"\n");

    s.push_str(
        "\n# A category you assert about this project. Free text — consumers\n\
         # interpret it; AIDA does not validate it.\n\
         # classification = \"\"\n",
    );

    match &facts.repository {
        Some(v) => s.push_str(&format!("\nrepository = {}\n", toml_str(v))),
        None => s.push_str("\n# repository = \"\"\n"),
    }

    s.push_str("# homepage = \"\"\n# tags = []\n");
    s
}

/// Pull the opening prose out of README text.
///
/// Skips the chrome most READMEs open with — the title heading, badge rows,
/// raw-HTML logo blocks, blockquotes, rules, code fences, tables — and returns
/// the first run of real sentences, flattened to one line.
///
/// Pure so the skipping rules are testable without a filesystem.
pub fn readme_lead(text: &str) -> Option<String> {
    let mut para: Vec<&str> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if is_chrome(line) {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        para.push(line);
    }
    let joined = strip_inline_markup(&para.join(" "));
    let cut = truncate_on_word(&joined, 200);
    Some(cut).filter(|s| !s.is_empty())
}

fn is_chrome(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with('<')
        || line.starts_with('>')
        || line.starts_with("---")
        || line.starts_with("===")
        || line.starts_with("```")
        || line.starts_with('|')
        || line.starts_with("[!")
        || line.starts_with("![")
        || is_mostly_punctuation(line)
}

/// A line that is mostly punctuation — ASCII art, banners, rules. Real prose is
/// overwhelmingly letters; a logo drawn in slashes is not a description.
fn is_mostly_punctuation(line: &str) -> bool {
    let total = line.chars().filter(|c| !c.is_whitespace()).count();
    if total < 4 {
        return false;
    }
    let letters = line.chars().filter(|c| c.is_alphanumeric()).count();
    letters * 100 < total * 55
}

/// Remove the markdown that reads badly once flattened onto one line.
fn strip_inline_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // [text](url) -> text
            '[' => {
                let mut text = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    text.push(c);
                }
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == ')' {
                            break;
                        }
                    }
                }
                out.push_str(&text);
            }
            '*' | '_' | '`' => {}
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_on_word(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(i) if i > max / 2 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

/// Gather what is already knowable about a project, for pre-filling a manifest.
///
/// Everything here is best-effort: a project with no README, no remote and an
/// unreadable directory name still produces a valid (emptier) manifest. Nothing
/// in this path may fail, because failing to derive a nicety must never stop a
/// project being initialized.
pub fn derive_facts(root: &std::path::Path) -> DerivedFacts {
    let name = root
        .canonicalize()
        .ok()
        .as_deref()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .or_else(|| root.file_name().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty());

    let description = [
        "README.md",
        "README.rst",
        "README.txt",
        "README",
        "readme.md",
    ]
    .iter()
    .map(|f| root.join(f))
    .find(|p| p.is_file())
    .and_then(|p| read_head(&p, 8 * 1024))
    .and_then(|t| readme_lead(&t));

    let repository = crate::git_ops::remote_url(root, "origin");

    DerivedFacts {
        name,
        description,
        repository,
    }
}

/// Read at most `limit` bytes. A README can be very long and we only want its
/// opening; nothing in scaffolding should be slow.
fn read_head(path: &std::path::Path, limit: usize) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; limit];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Quote a string for TOML. Enough for the values we generate (paths, URLs,
/// README leads); rejects nothing, escapes what matters.
fn toml_str(v: &str) -> String {
    let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
    // A README lead can contain newlines after flattening failures; keep the
    // output a single valid basic string rather than emitting a broken file.
    let escaped = escaped.replace('\n', " ").replace('\r', "");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_is_a_state_not_an_error() {
        let dir = std::env::temp_dir().join(format!("aida-pm-absent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir), ManifestState::Absent);
        assert!(load(&dir).is_absent());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_minimal_manifest_parses() {
        let m = match parse_state("[project]\ndescription = \"a thing\"\n") {
            ManifestState::Present(m) => m,
            other => panic!("expected Present, got {other:?}"),
        };
        assert_eq!(m.project.description.as_deref(), Some("a thing"));
        assert_eq!(m.schema_version(), 1, "absent schema defaults to 1");
    }

    #[test]
    fn an_empty_file_is_valid_not_malformed() {
        // TOML's empty document is legal, and a manifest with nothing in it is
        // a legitimate (if useless) state — not a parse failure.
        assert!(matches!(parse_state(""), ManifestState::Present(_)));
    }

    #[test]
    fn malformed_toml_is_reported_never_panics() {
        let st = parse_state("[project\ndescription = ");
        match st {
            ManifestState::Malformed(msg) => assert!(!msg.is_empty(), "must carry a message"),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn an_unrecognised_liveness_value_degrades_rather_than_rejecting_the_file() {
        // A closed enum would make one typo lose `description`, `why` and
        // everything else in the file. Keep what parsed; preserve what didn't.
        let m = match parse_state(
            "[project]\ndescription = \"still here\"\nliveness = \"sort-of\"\n",
        ) {
            ManifestState::Present(m) => m,
            other => panic!("must not reject the whole file: {other:?}"),
        };
        assert_eq!(m.project.description.as_deref(), Some("still here"));
        assert_eq!(
            m.project.liveness,
            Some(Liveness::Unrecognised("sort-of".into())),
            "the value must be preserved, not silently dropped"
        );
        assert!(!m.project.liveness.as_ref().unwrap().is_recognised());
    }

    #[test]
    fn an_unrecognised_stage_value_degrades_too() {
        let m = match parse_state("[project]\nstage = \"deprecated\"\n") {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            m.project.stage,
            Some(Stage::Unrecognised("deprecated".into()))
        );
    }

    #[test]
    fn a_value_a_newer_aida_adds_does_not_break_an_older_reader() {
        // The forward-compatibility this whole design promises: schema 2 may
        // add a stage value, and a schema-1 reader must still read the file.
        let text = "schema = 2\n[project]\nwhy = \"x\"\nstage = \"incubating\"\n";
        assert!(matches!(parse_state(text), ManifestState::Present(_)));
    }

    #[test]
    fn known_values_survive_a_serialize_round_trip() {
        let m = match parse_state("[project]\nliveness = \"parked\"\nstage = \"alpha\"\n") {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        let text = toml::to_string(&*m).expect("serialize");
        let back = match parse_state(&text) {
            ManifestState::Present(b) => b,
            other => panic!("{other:?}"),
        };
        assert_eq!(back.project.liveness, Some(Liveness::Parked));
        assert_eq!(back.project.stage, Some(Stage::Alpha));
    }

    #[test]
    fn an_unrecognised_value_survives_a_round_trip_unchanged() {
        // Rewriting a manifest must not quietly delete a value it did not
        // understand — that would lose the author's words.
        let m = match parse_state("[project]\nliveness = \"dormant\"\n") {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        let text = toml::to_string(&*m).expect("serialize");
        assert!(text.contains("dormant"), "got: {text}");
    }

    #[test]
    fn liveness_parsing_is_case_and_whitespace_tolerant() {
        for raw in ["Alive", "  alive  ", "ALIVE"] {
            let m = match parse_state(&format!("[project]\nliveness = \"{raw}\"\n")) {
                ManifestState::Present(m) => m,
                other => panic!("{raw}: {other:?}"),
            };
            assert_eq!(m.project.liveness, Some(Liveness::Alive), "raw {raw}");
        }
    }

    #[test]
    fn every_documented_liveness_and_stage_value_parses() {
        for v in Liveness::ALL {
            assert!(
                matches!(
                    parse_state(&format!("[project]\nliveness = \"{v}\"\n")),
                    ManifestState::Present(_)
                ),
                "liveness {v} must parse — it is documented"
            );
        }
        for v in Stage::ALL {
            assert!(
                matches!(
                    parse_state(&format!("[project]\nstage = \"{v}\"\n")),
                    ManifestState::Present(_)
                ),
                "stage {v} must parse — it is documented"
            );
        }
    }

    #[test]
    fn unknown_fields_do_not_break_a_reader() {
        // A newer AIDA adding a field must not make the file unreadable to an
        // older one, or the standard cannot evolve.
        let st = parse_state("[project]\ndescription = \"x\"\nfuture_field = 42\n");
        assert!(matches!(st, ManifestState::Present(_)), "got {st:?}");
    }

    #[test]
    fn a_scaffolded_manifest_round_trips() {
        let facts = DerivedFacts {
            name: Some("aida-hub".into()),
            description: Some("A catalogue of my projects.".into()),
            repository: Some("https://github.com/joemooney/aida-hub.git".into()),
        };
        let text = render(&facts);
        let m = match parse_state(&text) {
            ManifestState::Present(m) => m,
            other => panic!("scaffold must parse, got {other:?}"),
        };
        assert_eq!(m.project.name.as_deref(), Some("aida-hub"));
        assert_eq!(
            m.project.description.as_deref(),
            Some("A catalogue of my projects.")
        );
        assert_eq!(
            m.project.repository.as_deref(),
            Some("https://github.com/joemooney/aida-hub.git")
        );
        assert_eq!(m.schema_version(), SCHEMA_VERSION);
    }

    #[test]
    fn a_scaffold_with_no_derivable_facts_still_parses() {
        let text = render(&DerivedFacts::default());
        assert!(
            matches!(parse_state(&text), ManifestState::Present(_)),
            "{text}"
        );
    }

    #[test]
    fn the_scaffold_documents_every_accepted_value() {
        // The file has to be editable without opening the docs.
        let text = render(&DerivedFacts::default());
        for v in Liveness::ALL {
            assert!(text.contains(v), "scaffold must list liveness `{v}`");
        }
        for v in Stage::ALL {
            assert!(text.contains(v), "scaffold must list stage `{v}`");
        }
    }

    #[test]
    fn a_freshly_scaffolded_manifest_reads_as_unfilled() {
        let m = match parse_state(&render(&DerivedFacts {
            name: Some("x".into()),
            description: Some("derived from the README".into()),
            repository: Some("git@example.com:x/y.git".into()),
        })) {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        // Pre-filled derivable facts do NOT count as filled in: the point is
        // whether a human added anything a scan could not.
        assert!(m.is_unfilled());
    }

    #[test]
    fn saying_why_makes_it_filled_in() {
        let m = match parse_state("[project]\nwhy = \"because I kept forgetting\"\n") {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        assert!(!m.is_unfilled());
    }

    #[test]
    fn whitespace_is_not_content() {
        let m = match parse_state("[project]\nwhy = \"   \"\n") {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        assert!(m.is_unfilled(), "a field of spaces is still unfilled");
    }

    #[test]
    fn not_recorded_is_a_skipped_marker_not_content() {
        let m = match parse_state("[project]\nwhy = \"NOT RECORDED\"\n") {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        assert!(m.is_unfilled(), "skipped init answers are not content");
        assert!(
            render(&DerivedFacts::default()).contains("why = \"NOT RECORDED\""),
            "the scaffold should record an honest skipped state"
        );
    }

    #[test]
    fn any_stated_field_counts_as_filled_in() {
        for line in [
            "liveness = \"parked\"",
            "stage = \"alpha\"",
            "owner = \"joe\"",
            "classification = \"tool\"",
        ] {
            let m = match parse_state(&format!("[project]\n{line}\n")) {
                ManifestState::Present(m) => m,
                other => panic!("{line}: {other:?}"),
            };
            assert!(!m.is_unfilled(), "{line} should count as filled in");
        }
    }

    #[test]
    fn readme_lead_skips_the_title_and_returns_the_first_prose() {
        let md = "# aida-hub\n\nA catalogue of my projects.\nSo I stop forgetting them.\n\nMore.\n";
        assert_eq!(
            readme_lead(md).as_deref(),
            Some("A catalogue of my projects. So I stop forgetting them.")
        );
    }

    #[test]
    fn readme_lead_skips_badges_and_html_logo_blocks() {
        let badges = "# p\n\n![build](https://img.shields.io/x)\n\nThe real description.\n";
        assert_eq!(
            readme_lead(badges).as_deref(),
            Some("The real description.")
        );
        // This is what the leptos start-axum template opens with.
        let html = "<picture>\n<source srcset=\"a.svg\">\n</picture>\n\nActual text.\n";
        assert_eq!(readme_lead(html).as_deref(), Some("Actual text."));
    }

    #[test]
    fn readme_lead_rejects_ascii_art() {
        // A logo drawn in slashes is not a description; lifting it into the
        // manifest would put visual noise where a sentence belongs.
        let md = "# p\n\n/ \\ / / / \\ / / / / .\\,/ \\/ \\, / \\/ //\n\nA real description.\n";
        assert_eq!(readme_lead(md).as_deref(), Some("A real description."));
    }

    #[test]
    fn readme_lead_flattens_links_and_emphasis() {
        assert_eq!(
            readme_lead("Built with [Leptos](https://leptos.dev) and **axum**.\n").as_deref(),
            Some("Built with Leptos and axum.")
        );
    }

    #[test]
    fn readme_lead_stops_at_a_code_fence() {
        assert_eq!(
            readme_lead("Run it like this.\n```bash\ncargo run\n```\n").as_deref(),
            Some("Run it like this.")
        );
    }

    #[test]
    fn a_readme_of_pure_chrome_yields_nothing_rather_than_junk() {
        assert_eq!(readme_lead("# Title\n\n![b](x)\n\n## Another\n"), None);
        assert_eq!(readme_lead(""), None);
    }

    #[test]
    fn a_long_lead_is_cut_on_a_word_boundary() {
        let lead = readme_lead(&"word ".repeat(200)).unwrap();
        assert!(lead.chars().count() <= 201, "got {}", lead.chars().count());
        assert!(lead.ends_with('…'));
        assert!(!lead.contains("wor…"), "must not cut mid-word: {lead}");
    }

    #[test]
    fn derive_facts_never_fails_on_a_bare_directory() {
        // No README, no git, nothing. Must still produce a usable manifest.
        let dir = std::env::temp_dir().join(format!("aida-pm-bare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = derive_facts(&dir);
        assert!(f.description.is_none());
        assert!(f.repository.is_none());
        assert!(f.name.is_some(), "the directory name is always knowable");
        assert!(matches!(
            parse_state(&render(&f)),
            ManifestState::Present(_)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn derive_facts_picks_up_a_readme() {
        let dir = std::env::temp_dir().join(format!("aida-pm-readme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("README.md"), "# thing\n\nDoes a thing well.\n").unwrap();
        let f = derive_facts(&dir);
        assert_eq!(f.description.as_deref(), Some("Does a thing well."));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quotes_and_backslashes_in_a_derived_description_do_not_break_the_file() {
        let facts = DerivedFacts {
            description: Some(r#"He said "hi" \ then left"#.into()),
            ..Default::default()
        };
        let m = match parse_state(&render(&facts)) {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            m.project.description.as_deref(),
            Some(r#"He said "hi" \ then left"#)
        );
    }

    #[test]
    fn a_multiline_derived_description_is_flattened_rather_than_emitting_broken_toml() {
        let facts = DerivedFacts {
            description: Some("line one\nline two".into()),
            ..Default::default()
        };
        let m = match parse_state(&render(&facts)) {
            ManifestState::Present(m) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(m.project.description.as_deref(), Some("line one line two"));
    }
}
