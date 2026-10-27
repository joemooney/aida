//! Refusal-only WORK content grammar shared by transport and prepared intake.
//! No filesystem, identity discovery, mutation, or permission is provided here.
// trace:STORY-1742 | ai:codex

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::LazyLock;

pub const SCANNER_VERSION: u32 = 1;
pub const MAX_BYTES: usize = 1_048_576;
/// The native oplog is append-only and grows with history, so it is bounded
/// per record (each line, and each operation's node budget, keeps the
/// ordinary limits) under this larger whole-file ceiling.
// trace:STORY-1742 | ai:claude
pub const MAX_OPLOG_BYTES: usize = 64 * MAX_BYTES;
const MAX_OPLOG_OPERATIONS: usize = 1_000_000;
const MAX_NODES: usize = 50_000;
const MAX_DEPTH: usize = 64;

#[path = "transport_yaml.rs"]
mod yaml_grammar;
pub use yaml_grammar::{plain as yaml_plain, Tags as YamlTags};

/// Values never appear in errors or the public status projection.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub reviewed: bool,
    pub deny_tokens: Vec<String>,
    pub git_name: String,
    pub git_identity: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Policy,
    Limit,
    Encoding,
    Unsupported,
    Email,
    Address,
    Host,
    Url,
    DenyToken,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Policy => "policy",
            Self::Limit => "limit",
            Self::Encoding => "encoding",
            Self::Unsupported => "unsupported grammar",
            Self::Email => "email",
            Self::Address => "network address",
            Self::Host => "hostname",
            Self::Url => "host-bearing URL",
            Self::DenyToken => "reviewed deny token",
        })
    }
}

pub type ScanResult = Result<(), Refusal>;

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9_-]+(?:\.[a-z0-9_-]+)*").unwrap()
});
static HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]{0,62}").unwrap()
});
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[a-z][a-z0-9+.-]*://[^\s/<>\x22]+|//[a-z0-9\[]").unwrap());
static IPV4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:[0-9]+\.){3}[0-9]+").unwrap());
static PERCENT_ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%[0-9a-fA-F]{2}").unwrap());
static OPAQUE_URI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bdata:[^\s]").unwrap());
// HTML/XML character references: numeric, hex, and named, with or without
// the closing semicolon (legacy named references decode without one). Any
// entity-shaped `&` refuses rather than claiming a decoding we do not do.
// trace:STORY-1742 | ai:claude
static CHARACTER_REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&(?:#|[A-Za-z])").unwrap());

/// Fixed store-root files and their formats. Only names an owning store
/// writer actually creates; anything else in the tree is unsupported.
const ROOT_FILES: &[(&str, &str)] = &[
    ("metadata.yaml", "yaml"),
    ("oplog.yaml", "aida-oplog"),
    ("README.md", "md"),
    (".gitignore", "txt"),
    (".aida-store", "symlink"),
];
const FIXED_FILES: &[(&str, &str)] = &[
    ("objects/.gitkeep", "empty"),
    ("coordination/drain.lock.toml", "toml"),
    ("coordination/solo.lock.toml", "toml"),
    ("registry/team.toml", "toml"),
    ("registry/nodes.toml", "toml"),
    ("registry/agreed_counters.toml", "toml"),
    ("registry/blocks.yaml", "yaml"),
    ("registry/blocks.lock", "empty"),
    (".aida/.advisor-seat-hint-shown", "txt"),
    (".aida/.format-hint-shown", "txt"),
];
/// Directories holding one user-named file per entry: (dir, extension, format).
const NAMED_FILES: &[(&str, &str, &str)] = &[
    ("coordination/leases", ".toml", "toml"),
    ("registry/queues", ".yaml", "yaml"),
    ("schedule", ".yaml", "yaml"),
    (".aida/agents", ".toml", "toml"),
    (".aida/burndown", ".jsonl", "jsonl"),
];
const FIXED_DIRECTORIES: &[&str] = &[
    "objects",
    "timings",
    "mailbox",
    "coordination",
    "coordination/leases",
    "registry",
    "registry/queues",
    "schedule",
    ".aida",
    ".aida/agents",
    ".aida/burndown",
];

/// The record list of a native oplog as written (`operations:` in block
/// style, then the `clock`/`node_id` trailer), ending at a line boundary:
/// the bytes an append carries unchanged. `None` for any other layout.
// trace:STORY-1742 | ai:claude
pub fn oplog_prefix(bytes: &[u8]) -> Option<&[u8]> {
    if !bytes.starts_with(b"operations:\n- ") {
        return None;
    }
    let trailer = bytes
        .windows(b"\nclock: ".len())
        .rposition(|window| window == b"\nclock: ")?;
    Some(&bytes[..trailer + 1])
}

fn canonical_uuid(stem: &str) -> bool {
    uuid::Uuid::parse_str(stem).is_ok_and(|id| id.to_string() == stem)
}

/// Queue filenames escape exactly `%`, `:`, `/` and `\` (BUG-1021). Decode
/// only that closed, canonical spelling; a separator decoded back into the
/// logical name is not a portable queue identity.
fn queue_identity(stem: &str) -> Result<String, Refusal> {
    let mut out = String::new();
    let mut rest = stem;
    while let Some(index) = rest.find('%') {
        out.push_str(&rest[..index]);
        let escape = rest.get(index..index + 3).ok_or(Refusal::Unsupported)?;
        out.push(match escape {
            "%25" => '%',
            "%3A" => ':',
            _ => return Err(Refusal::Unsupported),
        });
        rest = &rest[index + 3..];
    }
    out.push_str(rest);
    Ok(out)
}

impl Policy {
    /// Closed structural grammar for the real store layout. Fixed names come
    /// from the owning writers; every user-controlled component (spec ID,
    /// queue identity, lease/schedule/agent/burndown name, type directory) is
    /// decoded where the writer encodes it and scanned. Only the validated
    /// structural extension is removed. Unknown shapes are unsupported.
    // trace:STORY-1742 | ai:claude
    pub fn path_format(&self, path: &str) -> Result<&'static str, Refusal> {
        let parts = self.path_parts(path)?;
        if let Some((_, format)) = ROOT_FILES.iter().find(|(name, _)| *name == path) {
            return Ok(format);
        }
        if let Some((_, format)) = FIXED_FILES.iter().find(|(name, _)| *name == path) {
            return Ok(format);
        }
        let (dir, name) = path.rsplit_once('/').ok_or(Refusal::Unsupported)?;
        if let Some((_, extension, format)) = NAMED_FILES.iter().find(|(d, _, _)| *d == dir) {
            let stem = name.strip_suffix(extension).ok_or(Refusal::Unsupported)?;
            if stem.is_empty() {
                return Err(Refusal::Unsupported);
            }
            if dir == "registry/queues" {
                self.text(&queue_identity(stem)?)?;
            } else {
                self.text(stem)?;
            }
            return Ok(format);
        }
        match parts.as_slice() {
            ["objects", name] => {
                let stem = name.strip_suffix(".yaml").ok_or(Refusal::Unsupported)?;
                canonical_uuid(stem)
                    .then_some("yaml")
                    .ok_or(Refusal::Unsupported)
            }
            ["mailbox", name] => {
                let stem = name.strip_suffix(".json").ok_or(Refusal::Unsupported)?;
                let review = stem.strip_prefix("review-watch-").is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                });
                (canonical_uuid(stem) || review)
                    .then_some("json")
                    .ok_or(Refusal::Unsupported)
            }
            ["objects", _, _, name] | ["timings", _, _, name] => {
                let (stem, format) = match name.rsplit_once('.') {
                    Some((stem, "yaml")) if parts[0] == "objects" => (stem, "yaml"),
                    Some((stem, "json")) if parts[0] == "timings" => (stem, "json"),
                    _ => return Err(Refusal::Unsupported),
                };
                let canonical = if parts[0] == "objects" {
                    crate::object_store::relative_object_path(stem)
                } else {
                    crate::object_store::relative_timing_record_path(stem)
                };
                if canonical.ok().as_deref() != Some(path) {
                    return Err(Refusal::Unsupported);
                }
                self.text(stem)?;
                Ok(format)
            }
            _ => Err(Refusal::Unsupported),
        }
    }

    /// Tree directories must be a prefix of the closed file grammar.
    // trace:STORY-1742 | ai:claude
    pub fn directory(&self, path: &str) -> ScanResult {
        let parts = self.path_parts(path)?;
        if FIXED_DIRECTORIES.contains(&path) {
            return Ok(());
        }
        match parts.as_slice() {
            ["objects" | "timings", kind] | ["objects" | "timings", kind, _]
                if !kind.is_empty()
                    && kind.len() <= 32
                    && kind.starts_with(|c: char| c.is_ascii_uppercase())
                    && kind
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') =>
            {
                if let [_, _, shard] = parts.as_slice() {
                    if !(3..=6).contains(&shard.len()) || !shard.bytes().all(|b| b.is_ascii_digit())
                    {
                        return Err(Refusal::Unsupported);
                    }
                }
                self.text(kind)
            }
            _ => Err(Refusal::Unsupported),
        }
    }

    fn path_parts<'a>(&self, path: &'a str) -> Result<Vec<&'a str>, Refusal> {
        if path.len() > 4096 || path.split('/').count() > 64 {
            return Err(Refusal::Limit);
        }
        if path.contains('\\') || !path.is_ascii() || path.bytes().any(|b| b < 32 || b == 127) {
            return Err(Refusal::Encoding);
        }
        let parts: Vec<_> = path.split('/').collect();
        if parts
            .iter()
            .any(|part| part.is_empty() || matches!(*part, "." | ".."))
        {
            return Err(Refusal::Unsupported);
        }
        // Even fixed structural names are subject to reviewed literal tokens.
        let lower = path.to_ascii_lowercase();
        if self
            .deny_tokens
            .iter()
            .any(|token| lower.contains(&token.to_ascii_lowercase()))
        {
            return Err(Refusal::DenyToken);
        }
        Ok(parts)
    }

    /// Scan one tracked file: its closed path grammar selects its format.
    pub fn artifact(&self, path: &str, bytes: &[u8]) -> ScanResult {
        self.document(self.path_format(path)?, bytes)
    }

    /// The one rule for what a new file version scans, shared by WORK intake
    /// and the push gate so the two never disagree. `oplog.yaml` and
    /// `metadata.yaml` scan only their delta against `before` (the parent
    /// version); every other file is scanned whole. Policy covers what a
    /// change adds, never history.
    // trace:STORY-1742 | ai:claude
    pub fn change(&self, path: &str, before: Option<&[u8]>, after: &[u8]) -> ScanResult {
        match path {
            "oplog.yaml" => self.oplog_append(before, after),
            "metadata.yaml" => self.yaml_change(before, after),
            path => self.artifact(path, after),
        }
    }

    /// Whether [`Self::change`] reads a parent version for `path`.
    // trace:STORY-1742 | ai:claude
    pub fn scans_delta(path: &str) -> bool {
        matches!(path, "oplog.yaml" | "metadata.yaml")
    }

    pub fn validate(&self) -> ScanResult {
        if self.version != 1 || !self.reviewed || self.deny_tokens.len() > 256 {
            return Err(Refusal::Policy);
        }
        for token in &self.deny_tokens {
            // Matching is ASCII case-insensitive, literal and substring-based.
            // Refuse normalization ambiguity rather than pretend Unicode case
            // folding or invisible characters have a supported interpretation.
            if token.len() < 2
                || token.len() > 256
                || !token.is_ascii()
                || token.chars().any(char::is_control)
                || token.trim() != token
            {
                return Err(Refusal::Policy);
            }
        }
        for value in [&self.git_name, &self.git_identity] {
            if value.is_empty()
                || value.len() > 128
                || value.trim() != value
                || value.contains(['<', '>', '\n', '\r'])
            {
                return Err(Refusal::Policy);
            }
            self.text(value)?;
        }
        if self.git_identity.chars().any(char::is_whitespace) {
            return Err(Refusal::Policy);
        }
        Ok(())
    }

    /// Closed lexical recognition, plus operator-reviewed short-host tokens.
    /// This is not a proof that arbitrary prose contains no private facts.
    pub fn text(&self, value: &str) -> ScanResult {
        if value.len() > MAX_BYTES {
            return Err(Refusal::Limit);
        }
        if !value.is_ascii()
            || value
                .bytes()
                .any(|b| b < 32 && !matches!(b, b'\n' | b'\r' | b'\t'))
        {
            return Err(Refusal::Encoding);
        }
        let lower = value.to_ascii_lowercase();
        if self
            .deny_tokens
            .iter()
            .any(|token| lower.contains(&token.to_ascii_lowercase()))
        {
            return Err(Refusal::DenyToken);
        }
        if lower.contains("-----begin pgp")
            || lower.contains("-----begin ssh signature")
            || lower.contains("-----begin certificate")
            || OPAQUE_URI.is_match(value)
        {
            return Err(Refusal::Unsupported);
        }
        if EMAIL.is_match(value) {
            return Err(Refusal::Email);
        }
        if URL.is_match(value) {
            return Err(Refusal::Url);
        }
        if IPV4.is_match(value) {
            return Err(Refusal::Address);
        }
        if PERCENT_ESCAPE.is_match(value) || CHARACTER_REFERENCE.is_match(value) {
            return Err(Refusal::Unsupported);
        }
        // Include malformed address-shaped candidates as uncertain, rather
        // than accepting a parser failure as evidence of safety.
        for token in value
            .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '%' | '-')))
        {
            let token = token.trim_matches('.');
            if token.matches(':').count() >= 2 {
                if token.len() > 256 || token.matches(':').count() > 16 {
                    return Err(Refusal::Limit);
                }
                let address = token.split('%').next().unwrap_or(token);
                for (offset, _) in address.char_indices().filter(|(_, c)| *c == ':') {
                    if address[offset + 1..].parse::<IpAddr>().is_ok() {
                        return Err(Refusal::Address);
                    }
                }
            }
            if token
                .split('%')
                .next()
                .unwrap_or(token)
                .parse::<IpAddr>()
                .is_ok()
                || (token.matches('.').count() == 3
                    && token
                        .split('.')
                        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
                || (token.matches(':').count() >= 2
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.' | b'%')))
            {
                return Err(Refusal::Address);
            }
        }
        if HOST.is_match(value) {
            return Err(Refusal::Host);
        }
        Ok(())
    }

    /// Formats are explicit. YAML tags and aliases, compressed/opaque content,
    /// and unknown formats refuse. Both source bytes and decoded keys/scalars
    /// are inspected; no whole-section or schema-suffix exemptions exist.
    pub fn document(&self, extension: &str, bytes: &[u8]) -> ScanResult {
        let oplog = extension == "aida-oplog";
        if bytes.len() > if oplog { MAX_OPLOG_BYTES } else { MAX_BYTES } {
            return Err(Refusal::Limit);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Refusal::Encoding)?;
        if oplog {
            // Every source byte is still scanned, in whole-line chunks; a
            // single line beyond the ordinary bound refuses.
            let mut chunk = 0..0;
            for line in text.split_inclusive('\n') {
                if line.len() > MAX_BYTES {
                    return Err(Refusal::Limit);
                }
                if chunk.len() + line.len() > MAX_BYTES {
                    self.text(&text[chunk.clone()])?;
                    chunk = chunk.end..chunk.end;
                }
                chunk.end += line.len();
            }
            self.text(&text[chunk])?;
        } else {
            self.text(text)?;
        }
        Self::shape(bytes, text)?;
        let mut nodes = MAX_NODES;
        match extension {
            "empty" if bytes.is_empty() => Ok(()),
            "aida-oplog" => self.oplog_document(text, &mut nodes),
            "json" => self.json_document(text, &mut nodes),
            "jsonl" => {
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    self.json_document(line, &mut nodes)?;
                }
                Ok(())
            }
            "yaml" | "yml" => {
                // serde_yaml expands aliases. Forbid them (and tags) by event
                // before parsing so a small source cannot cause unbounded
                // expanded allocations. Prose bytes are not structure.
                yaml_plain(bytes, YamlTags::Core)?;
                let value: serde_yaml::Value =
                    serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
                self.yaml(&value, 0, &mut nodes)
            }
            "toml" => {
                let value: toml::Value = toml::from_str(text).map_err(|_| Refusal::Encoding)?;
                self.toml(&value, 0, &mut nodes)
            }
            "txt" | "md" | "symlink" => {
                // Encoded payloads in prose are unsupported: do not claim
                // to have decoded arbitrary escape/container conventions.
                if text.contains('\\') {
                    return Err(Refusal::Unsupported);
                }
                Ok(())
            }
            _ => Err(Refusal::Unsupported),
        }
    }

    /// Conservative pre-parse bound, including quoted delimiters. False
    /// refusals are preferable to parser recursion on hostile documents.
    fn shape(bytes: &[u8], text: &str) -> ScanResult {
        let mut nesting = 0usize;
        for b in bytes {
            if matches!(b, b'[' | b'{') {
                nesting += 1;
            }
            if nesting > MAX_DEPTH {
                return Err(Refusal::Limit);
            }
            if matches!(b, b']' | b'}') {
                nesting = nesting.saturating_sub(1);
            }
        }
        if text.lines().any(|line| {
            line.bytes()
                .take_while(|b| matches!(b, b' ' | b'\t'))
                .count()
                > 128
        }) {
            return Err(Refusal::Limit);
        }
        Ok(())
    }

    /// Scan an oplog write. Policy applies to what the write adds, never to
    /// history: when `after` carries `before`'s record list byte-for-byte,
    /// only the appended records and trailer are content-scanned, while the
    /// grammar (size, shape, anchors/aliases/tags, typed parse) covers the
    /// whole file and the carried records must keep their exact meaning.
    /// Anything else is scanned whole.
    // trace:STORY-1742 | ai:claude
    pub fn oplog_append(&self, before: Option<&[u8]>, after: &[u8]) -> ScanResult {
        let Some((before, prefix)) = before
            .and_then(|before| oplog_prefix(before).map(|prefix| (before, prefix)))
            .filter(|(_, prefix)| after.starts_with(prefix))
        else {
            return self.document("aida-oplog", after);
        };
        if after.len() > MAX_OPLOG_BYTES {
            return Err(Refusal::Limit);
        }
        let text = std::str::from_utf8(after).map_err(|_| Refusal::Encoding)?;
        Self::shape(after, text)?;
        yaml_plain(after, YamlTags::Variants)?;
        let _: crate::oplog::OpLog = serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
        // An unchanged record list (a trailer-only change, or a file a later
        // commit carries as is) adds no records. trace:STORY-1742 | ai:claude
        let rest = &text[prefix.len()..];
        let added = if rest.starts_with("- ") {
            format!("operations:\n{rest}")
        } else {
            format!("operations: []\n{rest}")
        };
        self.document("aida-oplog", added.as_bytes())?;
        let records = |bytes: &[u8]| -> Result<Vec<serde_yaml::Value>, Refusal> {
            let value: serde_yaml::Value =
                serde_yaml::from_slice(bytes).map_err(|_| Refusal::Encoding)?;
            value
                .get("operations")
                .and_then(serde_yaml::Value::as_sequence)
                .cloned()
                .ok_or(Refusal::Unsupported)
        };
        let (whole, old, new) = (
            records(after)?,
            records(before)?,
            records(added.as_bytes())?,
        );
        if whole.len() > MAX_OPLOG_OPERATIONS
            || whole.len() != old.len() + new.len()
            || whole[..old.len()] != old[..]
            || whole[old.len()..] != new[..]
        {
            return Err(Refusal::Unsupported);
        }
        Ok(())
    }

    /// Scan a YAML rewrite (store metadata) by what it adds: source lines
    /// absent from `before` and decoded values that are new or changed. The
    /// grammar still covers the whole file. Without a readable `before` the
    /// file is scanned whole.
    // trace:STORY-1742 | ai:claude
    pub fn yaml_change(&self, before: Option<&[u8]>, after: &[u8]) -> ScanResult {
        let Some((before, old)) = before.and_then(|before| {
            serde_yaml::from_slice::<serde_yaml::Value>(before)
                .ok()
                .map(|old| (before, old))
        }) else {
            return self.document("yaml", after);
        };
        if after.len() > MAX_BYTES {
            return Err(Refusal::Limit);
        }
        let text = std::str::from_utf8(after).map_err(|_| Refusal::Encoding)?;
        Self::shape(after, text)?;
        yaml_plain(after, YamlTags::Core)?;
        let carried: std::collections::HashSet<&[u8]> = before.split(|b| *b == b'\n').collect();
        let added: Vec<&str> = text
            .split('\n')
            .filter(|line| !carried.contains(line.as_bytes()))
            .collect();
        self.text(&added.join("\n"))?;
        let new: serde_yaml::Value = serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
        let mut nodes = MAX_NODES;
        self.yaml_added(&old, &new, 0, &mut nodes)
    }

    /// Scan the parts of `new` that `old` does not already hold unchanged.
    fn yaml_added(
        &self,
        old: &serde_yaml::Value,
        new: &serde_yaml::Value,
        depth: usize,
        nodes: &mut usize,
    ) -> ScanResult {
        use serde_yaml::Value;
        if old == new {
            return Ok(());
        }
        match (old, new) {
            (Value::Mapping(old), Value::Mapping(new)) => {
                Self::budget(depth, nodes)?;
                for (key, value) in new {
                    match old.get(key) {
                        Some(prior) => self.yaml_added(prior, value, depth + 1, nodes)?,
                        None => {
                            let mut entry = serde_yaml::Mapping::new();
                            entry.insert(key.clone(), value.clone());
                            self.yaml(&Value::Mapping(entry), depth, nodes)?;
                        }
                    }
                }
                Ok(())
            }
            (Value::Sequence(old), Value::Sequence(new)) => {
                Self::budget(depth, nodes)?;
                for value in new.iter().filter(|value| !old.contains(value)) {
                    self.yaml(value, depth + 1, nodes)?;
                }
                Ok(())
            }
            _ => self.yaml(new, depth, nodes),
        }
    }

    fn budget(depth: usize, nodes: &mut usize) -> ScanResult {
        if depth > MAX_DEPTH || *nodes == 0 {
            return Err(Refusal::Limit);
        }
        *nodes -= 1;
        Ok(())
    }

    /// Native oplog enum tags are supported only at the closed operation-kind
    /// position. Every source byte, tag, key and decoded scalar is still
    /// inspected; arbitrary YAML tags and aliases remain unsupported.
    // trace:STORY-1742 | ai:codex
    fn oplog_document(&self, text: &str, nodes: &mut usize) -> ScanResult {
        yaml_plain(text.as_bytes(), YamlTags::Variants)?;
        let _: crate::oplog::OpLog = serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
        let mut value: serde_yaml::Value =
            serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
        let root = value.as_mapping_mut().ok_or(Refusal::Unsupported)?;
        if root
            .keys()
            .any(|key| !matches!(key.as_str(), Some("operations" | "clock" | "node_id")))
        {
            return Err(Refusal::Unsupported);
        }
        let operations = root
            .get_mut(serde_yaml::Value::String("operations".into()))
            .and_then(serde_yaml::Value::as_sequence_mut)
            .ok_or(Refusal::Unsupported)?;
        if operations.len() > MAX_OPLOG_OPERATIONS {
            return Err(Refusal::Limit);
        }
        for operation in operations.iter_mut() {
            let operation = operation.as_mapping_mut().ok_or(Refusal::Unsupported)?;
            if operation.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    Some(
                        "id" | "target_id"
                            | "author"
                            | "node_id"
                            | "lamport"
                            | "timestamp"
                            | "kind"
                    )
                )
            }) {
                return Err(Refusal::Unsupported);
            }
            let kind = operation
                .get_mut(serde_yaml::Value::String("kind".into()))
                .ok_or(Refusal::Unsupported)?;
            if let serde_yaml::Value::Tagged(tagged) = kind {
                let name = tagged.tag.to_string();
                let name = name.strip_prefix('!').unwrap_or(&name);
                let allowed: &[&str] = match name {
                    "Create" => &["title", "description", "req_type", "status", "priority"],
                    "SetTitle" => &["title"],
                    "SetDescription" => &["description"],
                    "SetStatus" => &["status"],
                    "SetPriority" => &["priority"],
                    "SetOwner" => &["owner"],
                    "AddTag" | "RemoveTag" => &["tag"],
                    "AddComment" => &["comment_id", "content", "author"],
                    "AddRelationship" => &["target_id", "rel_type"],
                    "RemoveRelationship" => &["target_id"],
                    _ => return Err(Refusal::Unsupported),
                };
                self.text(name)?;
                let fields = tagged.value.as_mapping().ok_or(Refusal::Unsupported)?;
                if fields
                    .keys()
                    .any(|key| key.as_str().is_none_or(|k| !allowed.contains(&k)))
                {
                    return Err(Refusal::Unsupported);
                }
                let mut plain = serde_yaml::Mapping::new();
                plain.insert(serde_yaml::Value::String(name.into()), tagged.value.clone());
                *kind = serde_yaml::Value::Mapping(plain);
            } else if !matches!(kind.as_str(), Some("Archive" | "Unarchive")) {
                return Err(Refusal::Unsupported);
            }
        }
        // Node budgets are per operation record; the header shares `nodes`.
        for (key, value) in value.as_mapping().ok_or(Refusal::Unsupported)? {
            let key = key.as_str().ok_or(Refusal::Unsupported)?;
            self.text(key)?;
            self.explicit_field(key, value.as_str())?;
            match value.as_sequence() {
                Some(records) if key == "operations" => {
                    Self::budget(1, nodes)?;
                    for record in records {
                        let mut budget = MAX_NODES;
                        self.yaml(record, 2, &mut budget)?;
                    }
                }
                _ => self.yaml(value, 1, nodes)?,
            }
        }
        Ok(())
    }

    fn json_document(&self, text: &str, nodes: &mut usize) -> ScanResult {
        // First enforce JSON syntax. Value alone is insufficient for scanning:
        // its map decoder overwrites duplicate keys. The YAML mapping decoder
        // rejects decoded duplicate keys, including escaped spellings, before
        // we traverse its scalar-preserving representation of this JSON.
        let _: serde_json::Value = serde_json::from_str(text).map_err(|_| Refusal::Encoding)?;
        let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
        self.yaml(&value, 0, nodes)
    }

    fn explicit_field(&self, key: &str, value: Option<&str>) -> ScanResult {
        let field = key.to_ascii_lowercase();
        let normalized = value.unwrap_or("").to_ascii_lowercase();
        match field.as_str() {
            "encoding"
            | "content_encoding"
            | "content-encoding"
            | "transfer_encoding"
            | "content_transfer_encoding"
            | "content-transfer-encoding" => {
                if value.is_none()
                    || !matches!(
                        normalized.as_str(),
                        "" | "utf8"
                            | "utf-8"
                            | "ascii"
                            | "us-ascii"
                            | "identity"
                            | "plain"
                            | "text"
                    )
                {
                    return Err(Refusal::Unsupported);
                }
            }
            "compression" | "compression_format" | "compressed" => {
                if value.is_none() || !matches!(normalized.as_str(), "" | "none" | "identity") {
                    return Err(Refusal::Unsupported);
                }
            }
            "signature" | "gpgsig" | "gpg_signature" | "ssh_signature" | "signed_payload"
                if value != Some("") =>
            {
                return Err(Refusal::Unsupported)
            }
            _ => {}
        }
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "hostname" | "host" | "fqdn"
        ) && value != Some("")
        {
            return Err(Refusal::Host);
        }
        Ok(())
    }

    fn yaml(&self, value: &serde_yaml::Value, depth: usize, nodes: &mut usize) -> ScanResult {
        Self::budget(depth, nodes)?;
        match value {
            serde_yaml::Value::String(s) => self.text(s),
            serde_yaml::Value::Sequence(a) => {
                for v in a {
                    self.yaml(v, depth + 1, nodes)?;
                }
                Ok(())
            }
            serde_yaml::Value::Mapping(o) => {
                for (k, v) in o {
                    let key = k.as_str().ok_or(Refusal::Unsupported)?;
                    self.text(key)?;
                    self.explicit_field(key, v.as_str())?;
                    self.yaml(v, depth + 1, nodes)?;
                }
                Ok(())
            }
            serde_yaml::Value::Tagged(_) => Err(Refusal::Unsupported),
            _ => Ok(()),
        }
    }

    fn toml(&self, value: &toml::Value, depth: usize, nodes: &mut usize) -> ScanResult {
        Self::budget(depth, nodes)?;
        match value {
            toml::Value::String(s) => self.text(s),
            toml::Value::Array(a) => {
                for v in a {
                    self.toml(v, depth + 1, nodes)?;
                }
                Ok(())
            }
            toml::Value::Table(o) => {
                for (k, v) in o {
                    self.text(k)?;
                    self.explicit_field(k, v.as_str())?;
                    self.toml(v, depth + 1, nodes)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> Policy {
        Policy {
            version: 1,
            reviewed: true,
            deny_tokens: vec!["private-marker".into()],
            git_name: "AIDA Generic".into(),
            git_identity: "aida".into(),
        }
    }

    #[test]
    fn story_1742_closed_path_grammar_matches_the_real_store_layout() {
        // trace:STORY-1742 | ai:claude
        let p = policy();
        for (path, format) in [
            ("oplog.yaml", "aida-oplog"),
            ("metadata.yaml", "yaml"),
            (".gitignore", "txt"),
            (".aida-store", "symlink"),
            ("objects/.gitkeep", "empty"),
            ("objects/TASK/002/TASK-2347.yaml", "yaml"),
            ("objects/FR/000/FR-1-042.yaml", "yaml"),
            ("timings/TASK/002/TASK-2347.json", "json"),
            ("mailbox/019deaa4-dcd0-7541-95f0-ae88f28fc0e6.json", "json"),
            (
                &format!("mailbox/review-watch-{}.json", "ab12".repeat(16)),
                "json",
            ),
            ("coordination/leases/pr-2929-019f7dda.toml", "toml"),
            ("coordination/drain.lock.toml", "toml"),
            ("registry/queues/role%3Aadvisor.yaml", "yaml"),
            ("registry/blocks.lock", "empty"),
            ("registry/agreed_counters.toml", "toml"),
            ("schedule/night-shift.yaml", "yaml"),
            (".aida/agents/generic-1154604.toml", "toml"),
            (".aida/burndown/20260720T044813Z-019f7dda.jsonl", "jsonl"),
            (".aida/.format-hint-shown", "txt"),
        ] {
            assert_eq!(p.path_format(path), Ok(format), "{path}");
        }
        for dir in [
            "objects",
            "objects/TASK",
            "objects/TASK/002",
            "timings/FR/000",
            ".aida/agents",
        ] {
            p.directory(dir).unwrap();
        }
        for path in [
            // Never a blanket suffix, section or email exemption.
            "oplog.jsonl",
            "config.toml",
            "notes/arbitrary.yaml",
            "objects/TASK/000/TASK-2347.yaml",
            "objects/TASK/002/task-2347.yaml",
            "objects/TASK/002/TASK-2347.json",
            "timings/TASK/002/TASK-2347.yaml",
            "objects/019DEAA4-DCD0-7541-95F0-AE88F28FC0E6.yaml",
            "mailbox/private-marker.json",
            "coordination/leases/private-marker.toml",
            "coordination/leases/host.invalid.toml",
            "schedule/name@host.invalid.yaml",
            "registry/queues/role%2Fadvisor.yaml",
            "registry/queues/role%3aadvisor.yaml",
            "registry/queues/role%3Ahost.invalid.yaml",
            "registry/queues/name@host.invalid.yaml",
            ".aida/agents/.toml",
            "objects/BUG/000/BUG-PRIVATE-MARKER-1.yaml",
            "objects//BUG-1.yaml",
            "objects/../oplog.yaml",
        ] {
            assert!(p.path_format(path).is_err(), "{path}");
        }
        for dir in [
            "notes",
            "objects/task",
            "objects/TASK/x",
            "objects/PRIVATE-MARKER",
            "registry/other",
        ] {
            assert!(p.directory(dir).is_err(), "{dir}");
        }
        // Type directories are user-controlled and still deny-scanned.
        let mut strict = policy();
        strict.deny_tokens = vec!["SECRET".into()];
        assert_eq!(
            strict.directory("objects/SECRET_X"),
            Err(Refusal::DenyToken)
        );
        assert!(p.artifact("registry/blocks.lock", b"x").is_err());
        p.artifact("registry/blocks.lock", b"").unwrap();
    }

    #[test]
    fn story_1742_html_character_references_refuse_in_every_format() {
        // trace:STORY-1742 | ai:claude
        let p = policy();
        for value in [
            "private&#45;marker",
            "private&#x2d;marker",
            "private&#X2D;marker",
            "private&#45marker",
            "private&hyphen;marker",
            "name&commat;example",
            "a &amp b",
        ] {
            assert_eq!(p.text(value), Err(Refusal::Unsupported), "{value}");
        }
        for (extension, document) in [
            ("md", "Generic private&#45;marker\n"),
            ("txt", "private&#x2d;marker"),
            ("json", r#"{"title":"private&#45;marker"}"#),
            ("toml", "title = \"private&#45;marker\"\n"),
            ("jsonl", "{\"title\":\"private&hyphen;marker\"}\n"),
        ] {
            assert!(
                p.document(extension, document.as_bytes()).is_err(),
                "{document}"
            );
        }
        p.text("Generic & safe && plain").unwrap();
    }

    #[test]
    fn story_1742_native_oplog_tags_are_closed_and_all_values_are_scanned() {
        let policy = policy();
        let mut log = crate::oplog::OpLog::new("a".repeat(32));
        log.append(
            uuid::Uuid::new_v4(),
            "aida".into(),
            crate::oplog::OpKind::SetTitle {
                title: "Generic failure".into(),
            },
        );
        let serialized = serde_yaml::to_string(&log).unwrap();
        policy
            .artifact("oplog.yaml", serialized.as_bytes())
            .unwrap();
        assert!(policy.document("yaml", serialized.as_bytes()).is_err());
        for unsafe_log in [
            serialized.replace("Generic failure", "private-marker"),
            serialized.replace("!SetTitle", "!Unknown"),
            serialized.replace(
                "title: Generic failure",
                "title: Generic failure\n    encoding: base64",
            ),
            "operations: []\nclock: 0\nnode_id: safe\nextra: !SetTitle {title: safe}\n".into(),
        ] {
            assert!(policy
                .artifact("oplog.yaml", unsafe_log.as_bytes())
                .is_err());
        }
        assert!(policy.path_format("oplog-private-marker.yaml").is_err());
    }

    #[test]
    fn story_1742_oplog_and_metadata_scan_only_what_a_write_adds() {
        // trace:STORY-1742 | ai:claude
        let policy = policy();
        let mut log = crate::oplog::OpLog::new("1".into());
        let target = uuid::Uuid::new_v4();
        log.append(
            target,
            "aida".into(),
            crate::oplog::OpKind::SetTitle {
                title: "R&D private-marker".into(),
            },
        );
        let before = serde_yaml::to_string(&log).unwrap();
        let prefix = oplog_prefix(before.as_bytes()).unwrap();
        let append = |title: &str| {
            let mut next = log.clone();
            next.append(
                target,
                "aida".into(),
                crate::oplog::OpKind::SetTitle {
                    title: title.into(),
                },
            );
            let mut bytes = prefix.to_vec();
            bytes.extend(
                serde_yaml::to_string(&next.operations[1..])
                    .unwrap()
                    .as_bytes(),
            );
            bytes.extend(format!("clock: {}\nnode_id: '1'\n", next.clock.0).as_bytes());
            bytes
        };
        let admitted = append("Generic failure");
        policy
            .oplog_append(Some(before.as_bytes()), &admitted)
            .unwrap();
        assert!(policy
            .oplog_append(Some(before.as_bytes()), &append("R&D failure"))
            .is_err());
        // Without a carried prefix the whole file is new content.
        assert!(policy.oplog_append(None, &admitted).is_err());
        // Grammar covers the whole file: an alias in the tail refuses.
        let mut aliased = admitted.clone();
        aliased.extend(b"extra: *x\n");
        assert!(policy
            .oplog_append(Some(before.as_bytes()), &aliased)
            .is_err());

        let metadata = b"title: Generic\ndescription: R&D backlog\n";
        policy
            .yaml_change(
                Some(metadata),
                b"title: Clarified\ndescription: R&D backlog\n",
            )
            .unwrap();
        for added in [
            &b"title: R&D\ndescription: R&D backlog\n"[..],
            b"title: Generic\ndescription: R&D backlog\nnote: \"R\\x26D\"\n",
            b"title: Generic # private-marker\ndescription: R&D backlog\n",
            b"title: &a Generic\ndescription: *a\n",
        ] {
            assert!(
                policy.yaml_change(Some(metadata), added).is_err(),
                "{}",
                String::from_utf8_lossy(added)
            );
        }
        assert!(policy.yaml_change(None, metadata).is_err());

        // `change` is the one rule intake and push share: an unchanged or
        // trailer-only oplog adds nothing, other files scan whole.
        policy
            .change("oplog.yaml", Some(before.as_bytes()), before.as_bytes())
            .unwrap();
        let ticked = before.replace("clock: 1\n", "clock: 2\n");
        assert_ne!(ticked, before);
        policy
            .change("oplog.yaml", Some(before.as_bytes()), ticked.as_bytes())
            .unwrap();
        policy
            .change("metadata.yaml", Some(metadata), metadata)
            .unwrap();
        assert!(policy
            .change("README.md", Some(b"R&D\n"), b"R&D\n")
            .is_err());
    }

    #[test]
    fn story_1742_categories_and_decoding_refuse_without_values() {
        let p = policy();
        p.validate().unwrap();
        for value in [
            "name@node.invalid",
            "192.0.2.4",
            "::1",
            "2001:db8::1",
            "::ffff:192.0.2.4",
            "192.0.2.4:443",
            "Address:2001:db8::1",
            "IPv6:2001:db8::1",
            "(//short/path)",
            "endpoint=//short/path",
            "fe80::1%en0",
            "host.invalid",
            "ssh://short/path",
            "PRIVATE-MARKER",
        ] {
            assert!(p.text(value).is_err(), "{value}");
        }
        for (extension, document) in [
            ("json", r#"{"title":"private\u002dmarker"}"#),
            ("yaml", r#"title: "private\x2dmarker""#),
            ("toml", r#"title = "private\u002dmarker""#),
            ("json", r#"{"hostname":"short"}"#),
            ("json", r#"{"x":"private\u002dmarker","x":"safe"}"#),
            ("jsonl", r#"{"x":"private\u002dmarker","\u0078":"safe"}"#),
        ] {
            assert!(p.document(extension, document.as_bytes()).is_err());
        }
        p.document(
            "yaml",
            b"title: Generic failure\ndescription: Synthetic reproduction\n",
        )
        .unwrap();
    }

    #[test]
    fn story_1742_unknown_and_ambiguous_encodings_refuse() {
        let p = policy();
        for (ext, bytes) in [
            ("gz", &b"payload"[..]),
            ("txt", &b"\\x41"[..]),
            ("yaml", &b"a: &x [1]\nb: *x"[..]),
            ("json", &b"\xff"[..]),
        ] {
            assert!(p.document(ext, bytes).is_err());
        }
        assert_eq!(p.text("caf\u{e9}"), Err(Refusal::Encoding));
        assert!(p.text(&"a".repeat(MAX_BYTES + 1)).is_err());
    }

    #[test]
    fn story_1742_declared_opaque_payloads_are_not_plain_decoded_text() {
        let p = policy();
        for (extension, document) in [
            ("yaml", "encoding: base64\ndata: cHJpdmF0ZS1tYXJrZXI=\n"),
            (
                "json",
                r#"{"en\u0063oding":"base64","data":"cHJpdmF0ZS1tYXJrZXI="}"#,
            ),
            ("toml", "compression = \"gzip\"\ndata = \"opaque\"\n"),
            ("yaml", "signature: opaque\n"),
            ("json", r#"{"encoding":["base64"],"data":"opaque"}"#),
        ] {
            assert!(p.document(extension, document.as_bytes()).is_err());
        }
        assert!(p
            .text("data:text/plain;base64,cHJpdmF0ZS1tYXJrZXI=")
            .is_err());
        assert!(p.text("-----BEGIN PGP SIGNATURE-----").is_err());
        p.document("yaml", b"encoding: utf-8\ndata: Generic reproduction\n")
            .unwrap();
    }
}
