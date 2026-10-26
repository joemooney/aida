//! Bounded, local-only scan of immutable Git objects, through every parent.
// trace:STORY-1742 | ai:codex
use super::*;
use aida_core::transport_sanitization::{Policy, MAX_BYTES, SCANNER_VERSION};
use std::collections::HashSet;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const POLICY_FILE: &str = "aida-transport-policy.json";
const RECEIPT_FILE: &str = "aida-transport-receipt.json";
const MAX_OBJECTS: usize = 10000;

pub(super) struct Scan {
    snapshot: aida_core::transport_policy::Snapshot,
}
impl std::ops::Deref for Scan {
    type Target = aida_core::transport_policy::Snapshot;
    fn deref(&self) -> &Self::Target {
        &self.snapshot
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    scanner: u32,
    endpoint: String,
    reference: String,
    trust_root: String,
    accepted_tip: String,
    policy: String,
}

impl Scan {
    pub(super) fn load(private: &private::Private) -> Result<Self> {
        Ok(Self {
            snapshot: aida_core::transport_policy::Snapshot::load(private)?,
        })
    }

    pub(super) fn revalidate(&self, private: &private::Private) -> Result<()> {
        self.snapshot.revalidate(private)
    }

    pub(super) fn history(&self, store: &Path, profile: &Profile, tip: &str) -> Result<()> {
        let root = profile
            .baseline
            .as_deref()
            .context("Explicit reviewed trust root is missing; push refused")?;
        let reader = Reader {
            store,
            deadline: Instant::now() + Duration::from_secs(60),
        };
        reader.supported()?;
        // Always scan from the original root. A receipt is evidence of a
        // successful attempt, never a result cache or a new trust root.
        if let Some(bytes) =
            private::Private::open(&common_dir(store)?.context("Missing common directory")?)?
                .read(RECEIPT_FILE)?
        {
            let receipt: Receipt = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid transport receipt"))?;
            if receipt.version != 1
                || receipt.scanner != SCANNER_VERSION
                || receipt.endpoint != profile.origin_sha256
                || receipt.reference != STORE_REF
                || receipt.trust_root != root
                || !is_hex_id(&receipt.accepted_tip, &[40, 64])
                || !is_hex_id(&receipt.policy, &[64])
            {
                bail!("Transport receipt does not match enrollment; refused");
            }
        }
        reader.run(&["merge-base", "--is-ancestor", root, tip], 0)?;
        let closure = reader.run(
            &["rev-list", "--objects", "--no-object-names", root, "--"],
            4 * MAX_BYTES,
        )?;
        let closure = std::str::from_utf8(&closure)
            .map_err(|_| anyhow::anyhow!("Invalid trusted graph encoding"))?;
        let trusted: HashSet<_> = closure.lines().collect();
        if trusted.len() > MAX_OBJECTS || trusted.iter().any(|oid| !is_hex_id(oid, &[40, 64])) {
            bail!("Trusted graph is incomplete or exceeds limits");
        }
        let range = format!("^{root}");
        let commits = reader.run(&["rev-list", "--topo-order", tip, &range, "--"], MAX_BYTES)?;
        let commits =
            std::str::from_utf8(&commits).map_err(|_| anyhow::anyhow!("Invalid graph encoding"))?;
        let commits: Vec<_> = commits.lines().collect();
        if commits.len() > MAX_OBJECTS {
            bail!("Transport graph limit exceeded");
        }
        let mut objects = HashSet::new();
        for (ordinal, oid) in commits.iter().enumerate() {
            if !is_hex_id(oid, &[40, 64]) {
                bail!("Invalid graph object identity");
            }
            let bytes = reader.object("commit", oid)?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("Transport scan refused: commit encoding"))?;
            if text.contains('\\') {
                bail!("Unsupported commit escape encoding");
            }
            self.policy.text(text).map_err(|category| {
                anyhow::anyhow!("Transport scan refused: commit {ordinal}, {category}")
            })?;
            let (headers, _) = text
                .split_once("\n\n")
                .context("Unsupported commit structure")?;
            let mut tree = None;
            let mut identities = 0;
            for line in headers.lines() {
                let (key, value) = line.split_once(' ').context("Unsupported commit header")?;
                match key {
                    "tree" if tree.is_none() && is_hex_id(value, &[40, 64]) => tree = Some(value),
                    "parent" if is_hex_id(value, &[40, 64]) => {}
                    "author" | "committer" => {
                        // Metadata is safe only if it is the explicitly
                        // enrolled pseudonym; the shipped hook still runs.
                        let prefix =
                            format!("{} <{}> ", self.policy.git_name, self.policy.git_identity);
                        let tail = value
                            .strip_prefix(&prefix)
                            .context("Commit identity differs from reviewed transport identity")?;
                        let parts: Vec<_> = tail.split(' ').collect();
                        if parts.len() != 2
                            || parts[0].parse::<i64>().is_err()
                            || parts[1].len() != 5
                            || !parts[1].starts_with(['+', '-'])
                            || !parts[1][1..].bytes().all(|b| b.is_ascii_digit())
                        {
                            bail!("Unsupported commit identity encoding");
                        }
                        identities |= if key == "author" { 1 } else { 2 };
                    }
                    // Signatures and extension headers are opaque encodings;
                    // do not call them inspected merely after a text regex.
                    _ => bail!("Unsupported commit metadata; transport refused"),
                }
            }
            if identities != 3 {
                bail!("Incomplete commit identities");
            }
            let tree = tree.context("Missing commit tree")?;
            reader.object("tree", tree)?;
            let listing = reader.run(
                &["ls-tree", "-r", "-t", "-z", "--full-tree", tree],
                4 * MAX_BYTES,
            )?;
            let mut count = 0;
            for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
                count += 1;
                if count > MAX_OBJECTS {
                    bail!("Transport tree limit exceeded");
                }
                let split = entry
                    .iter()
                    .position(|b| *b == b'\t')
                    .context("Invalid tree entry")?;
                let meta = std::str::from_utf8(&entry[..split])
                    .map_err(|_| anyhow::anyhow!("Invalid tree metadata"))?;
                let path = std::str::from_utf8(&entry[split + 1..])
                    .map_err(|_| anyhow::anyhow!("Transport scan refused: path encoding"))?;
                let parts: Vec<_> = meta.split(' ').collect();
                if parts.len() != 3 || !is_hex_id(parts[2], &[40, 64]) {
                    bail!("Invalid tree object");
                }
                let extension = scan_path(&self.policy, path)?;
                match (parts[0], parts[1]) {
                    ("040000", "tree") => {
                        reader.object("tree", parts[2])?;
                    }
                    ("100644" | "100755", "blob") => {
                        // Inspect path on EVERY traversal, even for a reused
                        // baseline blob or a previously visited object.
                        if !trusted.contains(parts[2])
                            && objects.insert((parts[2].to_owned(), extension.to_owned()))
                        {
                            if objects.len() > MAX_OBJECTS {
                                bail!("Transport object limit exceeded");
                            }
                            let bytes = reader.object("blob", parts[2])?;
                            self.policy
                                .document(extension, &bytes)
                                .map_err(|category| {
                                    anyhow::anyhow!(
                                        "Transport scan refused: object {}, {category}",
                                        objects.len()
                                    )
                                })?;
                        }
                    }
                    _ => bail!("Unsupported tree mode; transport refused"),
                }
            }
        }
        Ok(())
    }

    pub(super) fn receipt(
        &self,
        private: &private::Private,
        profile: &Profile,
        tip: &str,
    ) -> Result<()> {
        self.revalidate(private)?;
        let receipt = Receipt {
            version: 1,
            scanner: SCANNER_VERSION,
            endpoint: profile.origin_sha256.clone(),
            reference: STORE_REF.into(),
            trust_root: profile.baseline.clone().context("Missing trust root")?,
            accepted_tip: tip.into(),
            policy: self.hash.clone(),
        };
        private.write(RECEIPT_FILE, &serde_json::to_vec(&receipt)?)
    }
}

// Fixed storage shapes only. An arbitrary suffix never grants an exemption.
fn scan_path<'a>(policy: &Policy, path: &'a str) -> Result<&'a str> {
    policy
        .path_format(path)
        .map_err(|category| anyhow::anyhow!("Transport scan refused: path {category}"))
}

pub(super) use aida_core::transport_policy::bounded;

struct Reader<'a> {
    store: &'a Path,
    deadline: Instant,
}
impl Reader<'_> {
    fn run(&self, args: &[&str], cap: usize) -> Result<Vec<u8>> {
        if Instant::now() >= self.deadline {
            bail!("Transport scan deadline exceeded");
        }
        // raw-git-ok(STORY-1742): closed local object/graph inspection only;
        // no hook, filter, lazy fetch, remote helper or maintenance execution.
        let mut cmd = Command::new("git");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(self.store)
            .args([
                "--no-replace-objects",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "gc.auto=0",
                "-c",
                "maintenance.auto=false",
            ])
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let (status, bytes) = bounded(
            cmd,
            cap,
            self.deadline.min(Instant::now() + Duration::from_secs(10)),
        )?;
        if !status.success() {
            bail!("Transport graph incomplete or unsupported; refused");
        }
        Ok(bytes)
    }

    fn object(&self, kind: &str, oid: &str) -> Result<Vec<u8>> {
        let size = self.run(&["cat-file", "-s", oid], 32)?;
        let size: usize = std::str::from_utf8(&size)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .context("Invalid graph object size")?;
        if size > MAX_BYTES {
            bail!("Transport object exceeds size limit");
        }
        let bytes = self.run(&["cat-file", kind, oid], MAX_BYTES)?;
        if bytes.len() != size {
            bail!("Transport object length changed");
        }
        Ok(bytes)
    }

    fn supported(&self) -> Result<()> {
        if self.run(&["rev-parse", "--is-shallow-repository"], 32)? != b"false\n" {
            bail!("Incomplete transport graph; refused");
        }
        if !self
            .run(
                &["for-each-ref", "--format=%(refname)", "refs/replace/"],
                MAX_BYTES,
            )?
            .is_empty()
        {
            bail!("Replacement graph unsupported");
        }
        for key in [
            "info/grafts",
            "objects/info/alternates",
            "objects/info/http-alternates",
        ] {
            let path = self.run(
                &["rev-parse", "--path-format=absolute", "--git-path", key],
                4096,
            )?;
            let path = std::str::from_utf8(&path)
                .map_err(|_| anyhow::anyhow!("Invalid graph metadata"))?
                .trim();
            match fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => bail!("Alternate or uncertain graph unsupported"),
            }
        }
        self.run(
            &["fsck", "--strict", "--no-reflogs", "--no-dangling"],
            MAX_BYTES,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_1742_exact_store_paths_scan_ids_without_a_suffix_exemption() {
        let policy = Policy {
            version: 1,
            reviewed: true,
            deny_tokens: vec!["private-marker".into()],
            git_name: "Generic".into(),
            git_identity: "aida".into(),
        };
        let safe = aida_core::object_store::relative_object_path("BUG-1").unwrap();
        assert_eq!(scan_path(&policy, &safe).unwrap(), "yaml");
        let unsafe_id =
            aida_core::object_store::relative_object_path("BUG-PRIVATE-MARKER-1").unwrap();
        assert!(scan_path(&policy, &unsafe_id).is_err());
        assert!(scan_path(&policy, "objects/BUG/000/arbitrary.yaml").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn story_1742_exited_parent_cannot_leave_a_stdout_holding_helper() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("survived");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "(sleep 1; printf survived > \"$1\") &",
                "transport-child",
            ])
            .arg(&marker);
        assert!(bounded(command, 1024, Instant::now() + Duration::from_secs(3)).is_err());
        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "helper survived an incomplete transport read"
        );
    }
}
