//! Refusal-only WORK content grammar shared by transport and prepared intake.
//! No filesystem, identity discovery, mutation, or permission is provided here.
// trace:STORY-1742 | ai:codex

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::LazyLock;

pub const SCANNER_VERSION: u32 = 1;
pub const MAX_BYTES: usize = 1_048_576;
const MAX_NODES: usize = 50_000;
const MAX_DEPTH: usize = 64;

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

impl Policy {
    /// The same closed path grammar is used for prospective writes and Git
    /// history. Structural names never exempt user-controlled ID components.
    pub fn path_format<'a>(&self, path: &'a str) -> Result<&'a str, Refusal> {
        if path.contains('\\') || path.len() > 4096 || path.split('/').count() > 64 {
            return Err(Refusal::Unsupported);
        }
        let id = path
            .rsplit('/')
            .next()
            .and_then(|n| n.strip_suffix(".yaml"));
        let canonical = id.is_some_and(|id| {
            crate::object_store::relative_object_path(id)
                .ok()
                .as_deref()
                == Some(path)
        });
        for component in path.split('/') {
            if component.is_empty() || matches!(component, "." | "..") {
                return Err(Refusal::Unsupported);
            }
            let fixed = !path.contains('/')
                && matches!(
                    component,
                    "metadata.yaml"
                        | "config.yaml"
                        | "config.toml"
                        | "oplog.yaml"
                        | "oplog.jsonl"
                        | "README.md"
                        | ".gitignore"
                );
            let uuid_object = path.starts_with("objects/")
                && path.split('/').count() == 2
                && component.strip_suffix(".yaml").is_some_and(|stem| {
                    uuid::Uuid::parse_str(stem).is_ok_and(|id| id.to_string() == stem)
                });
            if canonical && component.ends_with(".yaml") {
                self.text(component.strip_suffix(".yaml").unwrap())?;
            } else if !fixed && !uuid_object {
                self.text(component)?;
            }
        }
        let lower = path.to_ascii_lowercase();
        if self
            .deny_tokens
            .iter()
            .any(|t| lower.contains(&t.to_ascii_lowercase()))
        {
            return Err(Refusal::DenyToken);
        }
        let format = if path == "oplog.yaml" {
            "aida-oplog"
        } else {
            path.rsplit_once('.')
                .map(|(_, extension)| extension)
                .unwrap_or("")
        };
        if !matches!(
            format,
            "aida-oplog" | "yaml" | "yml" | "json" | "jsonl" | "toml" | "txt" | "md"
        ) {
            return Err(Refusal::Unsupported);
        }
        Ok(format)
    }

    pub fn artifact(&self, path: &str, bytes: &[u8]) -> ScanResult {
        self.document(self.path_format(path)?, bytes)
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
        if PERCENT_ESCAPE.is_match(value) {
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
        if bytes.len() > MAX_BYTES {
            return Err(Refusal::Limit);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Refusal::Encoding)?;
        self.text(text)?;
        // Conservative pre-parse bound, including quoted delimiters. False
        // refusals are preferable to parser recursion on hostile documents.
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
        let mut nodes = MAX_NODES;
        match extension {
            "aida-oplog" => self.oplog_document(text, &mut nodes),
            "json" => self.json_document(text, &mut nodes),
            "jsonl" => {
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    self.json_document(line, &mut nodes)?;
                }
                Ok(())
            }
            "yaml" | "yml" => {
                // serde_yaml expands aliases. Forbid them before parsing so a
                // small source cannot cause unbounded expanded allocations.
                if text.contains(['&', '*', '!']) {
                    return Err(Refusal::Unsupported);
                }
                let value: serde_yaml::Value =
                    serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
                self.yaml(&value, 0, &mut nodes)
            }
            "toml" => {
                let value: toml::Value = toml::from_str(text).map_err(|_| Refusal::Encoding)?;
                self.toml(&value, 0, &mut nodes)
            }
            "txt" | "md" => {
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
    fn oplog_document(&self, text: &str, nodes: &mut usize) -> ScanResult {
        if text.contains(['&', '*']) {
            return Err(Refusal::Unsupported);
        }
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
        for operation in operations {
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
        self.yaml(&value, 0, nodes)
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
        assert!(policy
            .artifact(
                "oplog.yaml",
                serialized
                    .replace("Generic failure", "private-marker")
                    .as_bytes()
            )
            .is_err());
        assert!(policy
            .artifact(
                "oplog.yaml",
                serialized.replace("!SetTitle", "!Unknown").as_bytes()
            )
            .is_err());
        assert!(policy
            .artifact(
                "oplog.yaml",
                serialized
                    .replace(
                        "title: Generic failure",
                        "title: Generic failure\n    encoding: base64"
                    )
                    .as_bytes()
            )
            .is_err());
        assert!(policy
            .artifact(
                "oplog.yaml",
                b"operations: []\nclock: 0\nnode_id: safe\nextra: !SetTitle {title: safe}\n"
            )
            .is_err());
        assert!(policy.path_format("oplog-private-marker.yaml").is_err());
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
