//! Untrusted review returns are data only, never review authority.
// trace:STORY-2493 | ai:codex
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Kind {
    Finding,
    ReviewVerdict,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Finding {
    pub severity: Severity,
    pub blocking: bool,
    pub title: String,
    pub body: String,
    pub file: String,
    pub line: u32,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Severity {
    Major,
    Minor,
    Cosmetic,
    Observation,
    Note,
}
impl Severity {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Cosmetic => "cosmetic",
            Self::Observation => "observation",
            Self::Note => "note",
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Decision {
    Approved,
    RequestChanges,
    Rejected,
    Waiting,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Verdict {
    pr: u64,
    head_sha: String,
    decision: Decision,
    blocking_findings: Vec<String>,
    pub reviewer_node: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Payload {
    Finding(Finding),
    Verdict(Verdict),
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Envelope {
    version: u32,
    kind: Kind,
    pub payload: Payload,
    pub checksum: String,
}
#[derive(Serialize)]
struct HashInput<'a> {
    version: u32,
    kind: Kind,
    payload: &'a Payload,
}
fn hex(value: &str, n: usize) -> bool {
    value.len() == n
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Envelope {
    fn digest(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&HashInput {
                version: self.version,
                kind: self.kind,
                payload: &self.payload
            })?)
        ))
    }
    pub fn folder(&self) -> &'static str {
        match self.kind {
            Kind::Finding => "findings",
            Kind::ReviewVerdict => "review-verdicts",
        }
    }
    fn validate(&self, patterns: &[String]) -> Result<()> {
        if self.version != 1 || !hex(&self.checksum, 64) || self.checksum != self.digest()? {
            bail!("Invalid review envelope version or checksum");
        }
        match (&self.kind, &self.payload) {
            (Kind::Finding, Payload::Finding(f)) => {
                if f.title.is_empty()
                    || f.title.len() > 512
                    || f.body.len() > 65536
                    || f.file.is_empty()
                    || f.file.len() > 4096
                    || f.line == 0
                    || f.file
                        .split('/')
                        .any(|p| p.is_empty() || p == "." || p == "..")
                    || f.file.contains(['\\', ':'])
                    || f.file.chars().any(char::is_control)
                {
                    bail!("Invalid finding fields");
                }
            }
            (Kind::ReviewVerdict, Payload::Verdict(v)) => {
                // Opaque node identity is data, not an authenticated principal.
                if v.pr == 0
                    || !hex(&v.head_sha, 40)
                    || !hex(&v.reviewer_node, 32)
                    || v.blocking_findings.len() > 256
                    || v.blocking_findings.iter().any(|d| !hex(d, 64))
                    || v.blocking_findings
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != v.blocking_findings.len()
                {
                    bail!("Invalid verdict fields");
                }
            }
            _ => bail!("Review kind and payload disagree"),
        }
        // Check decoded strings, including enums and fixed fields, not escaped JSON.
        fn scan(value: &serde_json::Value, patterns: &[String]) -> Result<()> {
            match value {
                serde_json::Value::String(s) => {
                    // trace:STORY-2493 | ai:codex
                    // The paired-field helper must only see actual field values.
                    crate::observation_envelope::check_patterns(s, s, patterns)?
                }
                serde_json::Value::Array(a) => {
                    for v in a {
                        scan(v, patterns)?;
                    }
                }
                serde_json::Value::Object(o) => {
                    for v in o.values() {
                        scan(v, patterns)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        scan(&serde_json::to_value(self)?, patterns)
    }
}
pub(super) fn parse(bytes: &[u8], patterns: &[String]) -> Result<Envelope> {
    if bytes.len() > crate::observation_envelope::ENVELOPE_LIMIT {
        bail!("Review envelope exceeds limit");
    }
    let value: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Invalid review envelope schema or JSON"))?;
    value.validate(patterns)?;
    Ok(value)
}
/// Input has the same closed schema, with checksum empty; output freezes its digest.
pub(super) fn generate(bytes: &[u8], patterns: &[String]) -> Result<Vec<u8>> {
    let mut value: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Invalid review envelope schema or JSON"))?;
    if !value.checksum.is_empty() {
        bail!("Generation requires an empty checksum");
    }
    value.checksum = value.digest()?;
    let bytes = serde_json::to_vec(&value)?;
    parse(&bytes, patterns)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn input() -> Value {
        json!({"version":1,"kind":"finding","payload":{"severity":"minor","blocking":true,"title":"A finding","body":"untrusted data","file":"src/lib.rs","line":1},"checksum":""})
    }
    #[test]
    fn canonical_hash_closed_schema_bounds_and_every_string_policy() {
        let bytes = generate(&serde_json::to_vec(&input()).unwrap(), &[]).unwrap();
        let parsed = parse(&bytes, &[]).unwrap();
        let reordered: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            parse(&serde_json::to_vec_pretty(&reordered).unwrap(), &[])
                .unwrap()
                .checksum,
            parsed.checksum
        );
        for field in ["title", "body", "file", "severity"] {
            let value = &reordered["payload"][field];
            let pattern = regex::escape(value.as_str().unwrap());
            assert!(
                parse(&bytes, &[pattern]).is_err(),
                "unscanned field {field}"
            );
        }
        assert!(parse(&bytes, &["finding".into()]).is_err());
        for path in [
            "/etc/passwd",
            "../x",
            "src/../x",
            "src//x",
            "./x",
            "C:\\x",
            "src\nx",
        ] {
            let mut value = input();
            value["payload"]["file"] = json!(path);
            assert!(generate(&serde_json::to_vec(&value).unwrap(), &[]).is_err());
        }
        for mutation in [
            json!({"extra":true}),
            json!({"line":0}),
            json!({"severity":"unknown"}),
            json!({"title":"x".repeat(513)}),
            json!({"body":"x".repeat(65537)}),
        ] {
            let mut value = input();
            for (key, val) in mutation.as_object().unwrap() {
                value["payload"][key] = val.clone();
            }
            assert!(generate(&serde_json::to_vec(&value).unwrap(), &[]).is_err());
        }
        let mut extra = reordered.clone();
        extra["unexpected"] = json!(1);
        assert!(parse(&serde_json::to_vec(&extra).unwrap(), &[]).is_err());
        let mut tamper = reordered;
        tamper["payload"]["blocking"] = json!(false);
        assert!(parse(&serde_json::to_vec(&tamper).unwrap(), &[]).is_err());
        assert!(parse(&vec![b' '; 131073], &[]).is_err());
    }
    #[test]
    fn empty_string_policy_checks_actual_fields_only() {
        let patterns = ["^$".to_string()];
        let finding = input();
        let verdict = json!({"version":1,"kind":"review-verdict","payload":{"pr":7,"head_sha":"a".repeat(40),"decision":"approved","blocking_findings":["b".repeat(64)],"reviewer_node":"c".repeat(32)},"checksum":""});
        for value in [&finding, &verdict] {
            let bytes = generate(&serde_json::to_vec(value).unwrap(), &patterns).unwrap();
            parse(&bytes, &patterns).unwrap();
            assert!(parse(&bytes, &["[".into()]).is_err());
        }
        let mut empty_body = finding;
        empty_body["payload"]["body"] = json!("");
        let input_bytes = serde_json::to_vec(&empty_body).unwrap();
        assert!(generate(&input_bytes, &patterns).is_err());
        let bytes = generate(&input_bytes, &[]).unwrap();
        assert!(parse(&bytes, &patterns).is_err());
    }
    #[test]
    fn verdicts_are_fixed_closed_and_digest_sets_unique() {
        let mut value = json!({"version":1,"kind":"review-verdict","payload":{"pr":7,"head_sha":"a".repeat(40),"decision":"approved","blocking_findings":["b".repeat(64)],"reviewer_node":"c".repeat(32)},"checksum":""});
        let bytes = generate(&serde_json::to_vec(&value).unwrap(), &[]).unwrap();
        assert!(parse(&bytes, &["c{32}".into()]).is_err());
        value["payload"]["blocking_findings"] = json!(["b".repeat(64), "b".repeat(64)]);
        assert!(generate(&serde_json::to_vec(&value).unwrap(), &[]).is_err());
        value["payload"]["blocking_findings"] = json!([]);
        value["payload"]["summary"] = json!("free text");
        assert!(generate(&serde_json::to_vec(&value).unwrap(), &[]).is_err());
    }
}
