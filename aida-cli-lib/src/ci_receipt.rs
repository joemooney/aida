//! Closed CI return channel. A digest establishes integrity, never admission.
// trace:STORY-2480 | ai:codex
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const LIMIT: usize = 128 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Status {
    Success,
    Failed,
    Canceled,
    Skipped,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Job {
    pub name: String,
    pub status: Status,
    pub duration_seconds: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub version: u32,
    pub kind: String,
    pub sha: String,
    pub attempt: u32,
    pub status: Status,
    pub jobs: Vec<Job>,
    pub failing_tests: Vec<String>,
    pub digest: String,
}
// Order is the approved compact-JSON hash contract. Never hash a JSON map.
#[derive(Serialize)]
struct Payload<'a> {
    version: u32,
    kind: &'a str,
    sha: &'a str,
    attempt: u32,
    status: Status,
    jobs: &'a [Job],
    failing_tests: &'a [String],
}
#[derive(Debug, Clone)]
pub(super) struct Inventory {
    pub jobs: BTreeSet<String>,
    pub tests: BTreeSet<String>,
}
pub(super) fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn attempt(value: &str) -> Result<u32> {
    let n: u32 = value
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid CI attempt"))?;
    if n == 0 || n.to_string() != value {
        bail!("CI attempts require canonical positive u32 decimal spelling");
    }
    Ok(n)
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
impl Receipt {
    fn checksum(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&Payload {
                version: self.version,
                kind: &self.kind,
                sha: &self.sha,
                attempt: self.attempt,
                status: self.status,
                jobs: &self.jobs,
                failing_tests: &self.failing_tests,
            })?)
        ))
    }
    pub fn seal(&mut self) -> Result<Vec<u8>> {
        self.jobs.sort_by(|a, b| a.name.cmp(&b.name));
        self.failing_tests.sort();
        self.digest = self.checksum()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > LIMIT {
            bail!("CI receipt exceeds size limit");
        }
        Ok(bytes)
    }
    pub fn path(&self) -> String {
        format!("ci-results/{}/{}.json", self.sha, self.attempt)
    }
    pub fn parse(bytes: &[u8], inventory: &Inventory, sha: &str, attempt: u32) -> Result<Self> {
        if bytes.len() > LIMIT {
            bail!("CI receipt exceeds size limit");
        }
        let r: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid CI receipt schema or JSON"))?;
        if r.version != 1
            || r.kind != "ci-result"
            || !oid(&r.sha)
            || r.sha != sha
            || r.attempt == 0
            || r.attempt != attempt
            || r.digest != r.checksum()?
            || r.jobs.is_empty()
            || r.jobs.len() > 256
            || r.failing_tests.len() > 2048
            || r.jobs.windows(2).any(|w| w[0].name >= w[1].name)
            || r.failing_tests.windows(2).any(|w| w[0] >= w[1])
            || r.jobs
                .iter()
                .any(|j| !identifier(&j.name) || j.duration_seconds > 86400)
            || r.failing_tests
                .iter()
                .any(|t| !identifier(t) || !inventory.tests.contains(t))
            || r.jobs
                .iter()
                .map(|j| j.name.clone())
                .collect::<BTreeSet<_>>()
                != inventory.jobs
        {
            bail!("Invalid CI receipt identity, digest, inventory or limits");
        }
        let expected =
            if r.jobs.iter().any(|j| j.status == Status::Failed) || !r.failing_tests.is_empty() {
                Status::Failed
            } else if r.jobs.iter().any(|j| j.status == Status::Canceled) {
                Status::Canceled
            } else if r.jobs.iter().any(|j| j.status == Status::Skipped) {
                Status::Skipped
            } else {
                Status::Success
            };
        if r.status != expected {
            bail!("Inconsistent CI receipt status");
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Receipt, Inventory) {
        (
            Receipt {
                version: 1,
                kind: "ci-result".into(),
                sha: "a".repeat(40),
                attempt: 7,
                status: Status::Success,
                jobs: vec![Job {
                    name: "build".into(),
                    status: Status::Success,
                    duration_seconds: 2,
                }],
                failing_tests: vec![],
                digest: String::new(),
            },
            Inventory {
                jobs: ["build".into()].into(),
                tests: ["test_ok".into()].into(),
            },
        )
    }
    #[test]
    fn strict_wire_and_admission() {
        let (mut r, inventory) = fixture();
        let bytes = r.seal().unwrap();
        assert!(Receipt::parse(&bytes, &inventory, &r.sha, 7).is_ok());
        assert!(Receipt::parse(&bytes, &inventory, &r.sha, 8).is_err());
        for extra in [r#", "unknown": 1}"#, r#", "attempt": 7}"#] {
            let mut bad = bytes[..bytes.len() - 1].to_vec();
            bad.extend(extra.as_bytes());
            assert!(Receipt::parse(&bad, &inventory, &r.sha, 7).is_err());
        }
        for n in ["0", "01", "+1", "4294967296", "-1"] {
            assert!(attempt(n).is_err());
        }
        r.jobs[0].name = "remote-secret".into();
        let bytes = r.seal().unwrap(); // Recomputed digest grants no allow-list authority.
        assert!(Receipt::parse(&bytes, &inventory, &r.sha, 7).is_err());
    }
    #[test]
    fn hostile_nested_types_and_bounds_refuse() {
        let (mut r, inventory) = fixture();
        let bytes = r.seal().unwrap();
        for mutated in [
            String::from_utf8(bytes.clone())
                .unwrap()
                .replace("\"duration_seconds\":2", "\"duration_seconds\":true"),
            String::from_utf8(bytes.clone())
                .unwrap()
                .replace("\"duration_seconds\":2", "\"duration_seconds\":2.0"),
            String::from_utf8(bytes.clone())
                .unwrap()
                .replace("\"duration_seconds\":2", "\"duration_seconds\":-1"),
            String::from_utf8(bytes.clone()).unwrap().replace(
                "\"duration_seconds\":2",
                "\"duration_seconds\":2,\"unknown\":0",
            ),
            format!("{} null", String::from_utf8(bytes.clone()).unwrap()),
        ] {
            assert!(Receipt::parse(mutated.as_bytes(), &inventory, &r.sha, 7).is_err());
        }
        r.jobs[0].duration_seconds = 86401;
        assert!(Receipt::parse(&r.seal().unwrap(), &inventory, &r.sha, 7).is_err());
        r.jobs[0].duration_seconds = 2;
        r.jobs.push(r.jobs[0].clone());
        assert!(Receipt::parse(&r.seal().unwrap(), &inventory, &r.sha, 7).is_err());
        assert!(Receipt::parse(&vec![b' '; LIMIT + 1], &inventory, &r.sha, 7).is_err());
    }
    #[test]
    fn complete_inventory_and_consistent_status() {
        let (mut r, mut inventory) = fixture();
        inventory.jobs.insert("test".into());
        assert!(Receipt::parse(&r.seal().unwrap(), &inventory, &r.sha, 7).is_err());
        inventory.jobs.remove("test");
        r.jobs[0].status = Status::Skipped;
        assert!(Receipt::parse(&r.seal().unwrap(), &inventory, &r.sha, 7).is_err());
        r.status = Status::Skipped;
        assert!(Receipt::parse(&r.seal().unwrap(), &inventory, &r.sha, 7).is_ok());
        r.failing_tests.push("unknown".into());
        r.status = Status::Failed;
        assert!(Receipt::parse(&r.seal().unwrap(), &inventory, &r.sha, 7).is_err());
    }
}
