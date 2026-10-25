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

impl Policy {
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

    fn json_document(&self, text: &str, nodes: &mut usize) -> ScanResult {
        // First enforce JSON syntax. Value alone is insufficient for scanning:
        // its map decoder overwrites duplicate keys. The YAML mapping decoder
        // rejects decoded duplicate keys, including escaped spellings, before
        // we traverse its scalar-preserving representation of this JSON.
        let _: serde_json::Value = serde_json::from_str(text).map_err(|_| Refusal::Encoding)?;
        let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|_| Refusal::Encoding)?;
        self.yaml(&value, 0, nodes)
    }

    fn explicit_host(&self, key: &str, value: Option<&str>) -> ScanResult {
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
                    self.explicit_host(key, v.as_str())?;
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
                    self.explicit_host(k, v.as_str())?;
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
}
