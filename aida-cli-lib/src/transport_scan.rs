//! Bounded, local-only scan of immutable Git objects, through every parent.
// trace:STORY-1742 | ai:codex
use super::*;
use aida_core::transport_sanitization::{Policy, MAX_BYTES, SCANNER_VERSION};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub(super) const POLICY_FILE: &str = "aida-transport-policy.json";
const RECEIPT_FILE: &str = "aida-transport-receipt.json";

// Separate, explicit budgets (trace:STORY-1742 | ai:claude). The trusted root
// closure is never enumerated: Git's own `tip ^root` exclusion names every
// object outside Reach(root), so the cost scales with the new delta, while
// every path of every new commit is still walked.
/// New commits in Reach(tip) − Reach(root).
const MAX_NEW_COMMITS: usize = 100_000;
/// New commits, trees and blobs in that delta.
const MAX_NEW_OBJECTS: usize = 2_000_000;
/// Uncompressed bytes of the new delta that integrity verification covers.
const MAX_NEW_BYTES: u64 = 2 << 30;
/// Bytes read for inspection: commits, walked trees, new blobs.
const MAX_READ_BYTES: u64 = 4 << 30;
/// Entries in a single tree object.
const MAX_TREE_ENTRIES: usize = 100_000;
/// Distinct (directory path, tree) pairs walked across all new commits.
const MAX_TREE_VISITS: usize = 2_000_000;
/// Whole scan, including integrity verification. A tighten-only override
/// exists so the deadline refusal itself is testable; it cannot lengthen it.
const SCAN_DEADLINE: Duration = Duration::from_secs(1800);

fn scan_deadline() -> Instant {
    let tightened = std::env::var("AIDA_TRANSPORT_SCAN_DEADLINE_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
        .map(Duration::from_millis);
    Instant::now() + tightened.map_or(SCAN_DEADLINE, |d| d.min(SCAN_DEADLINE))
}

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
        let common = common_dir(store)?.context("Missing common directory")?;
        let reader = Reader {
            store,
            deadline: scan_deadline(),
        };
        reader.supported()?;
        // Always scan from the original root. A receipt is evidence of a
        // successful attempt, never a result cache or a new trust root.
        if let Some(bytes) = private::Private::open(&common)?.read(RECEIPT_FILE)? {
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
        reader.run(&["merge-base", "--is-ancestor", root, tip], &[], 0)?;
        let range = format!("^{root}");
        let commits = reader.run(
            &["rev-list", "--topo-order", tip, &range, "--"],
            &[],
            MAX_NEW_COMMITS * 65,
        )?;
        let commits =
            std::str::from_utf8(&commits).map_err(|_| anyhow::anyhow!("Invalid graph encoding"))?;
        let commits: Vec<_> = commits.lines().collect();
        if commits.len() > MAX_NEW_COMMITS {
            bail!("Transport new-commit limit exceeded; refused");
        }
        if commits.iter().any(|oid| !is_hex_id(oid, &[40, 64])) {
            bail!("Invalid graph object identity");
        }
        // Every object outside Reach(root). Exclusion is sound: Git marks an
        // object uninteresting only when an excluded commit reaches it.
        let fresh = reader.run(
            &[
                "rev-list",
                "--objects",
                "--no-object-names",
                tip,
                &range,
                "--",
            ],
            &[],
            MAX_NEW_OBJECTS * 65,
        )?;
        let fresh =
            std::str::from_utf8(&fresh).map_err(|_| anyhow::anyhow!("Invalid graph encoding"))?;
        let fresh: HashSet<&str> = fresh.lines().collect();
        if fresh.len() > MAX_NEW_OBJECTS || fresh.iter().any(|oid| !is_hex_id(oid, &[40, 64])) {
            bail!("Transport new-object limit exceeded or graph invalid; refused");
        }
        if !commits.is_empty() {
            reader.verify(&common, tip, &range, &fresh)?;
        }
        let mut batch = Batch::open(&reader)?;
        let mut walked = HashSet::new();
        let mut scanned = HashSet::new();
        for (ordinal, oid) in commits.iter().enumerate() {
            let tree = self.commit(&mut batch, ordinal, oid)?;
            // Inspect paths on EVERY new commit, even where a whole subtree
            // or blob is reused from the trusted root. Memoizing an exact
            // (directory path, tree) pair repeats no distinct path verdict.
            let mut stack = vec![(String::new(), tree)];
            while let Some((prefix, tree)) = stack.pop() {
                if !walked.insert((prefix.clone(), tree.clone())) {
                    continue;
                }
                if walked.len() > MAX_TREE_VISITS {
                    bail!("Transport tree-walk limit exceeded; refused");
                }
                let bytes = batch.get(&tree, "tree")?;
                for (mode, name, child) in tree_entries(&bytes, tree.len() / 2)? {
                    let path = if prefix.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{prefix}/{name}")
                    };
                    let refusal =
                        |category| anyhow::anyhow!("Transport scan refused: path {category}");
                    match mode {
                        "40000" => {
                            self.policy.directory(&path).map_err(refusal)?;
                            stack.push((path, child));
                        }
                        "100644" | "100755" | "120000" => {
                            let format = self.policy.path_format(&path).map_err(refusal)?;
                            if (mode == "120000") != (format == "symlink") {
                                bail!("Unsupported tree mode; transport refused");
                            }
                            if fresh.contains(child.as_str())
                                && scanned.insert((child.clone(), format))
                            {
                                let bytes = batch.get(&child, "blob")?;
                                self.policy.document(format, &bytes).map_err(|category| {
                                    anyhow::anyhow!(
                                        "Transport scan refused: object {}, {category}",
                                        scanned.len()
                                    )
                                })?;
                            }
                        }
                        _ => bail!("Unsupported tree mode; transport refused"),
                    }
                }
            }
        }
        Ok(())
    }

    /// Commit metadata is safe only as the explicitly enrolled pseudonym;
    /// the shipped identity hook still runs at push time.
    fn commit(&self, batch: &mut Batch, ordinal: usize, oid: &str) -> Result<String> {
        let bytes = batch.get(oid, "commit")?;
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
                "tree" if tree.is_none() && is_hex_id(value, &[oid.len()]) => tree = Some(value),
                "parent" if is_hex_id(value, &[oid.len()]) => {}
                "author" | "committer" => {
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
                    let bit = if key == "author" { 1 } else { 2 };
                    if identities & bit != 0 {
                        bail!("Duplicate commit identity");
                    }
                    identities |= bit;
                }
                // Signatures and extension headers are opaque encodings;
                // do not call them inspected merely after a text regex.
                _ => bail!("Unsupported commit metadata; transport refused"),
            }
        }
        if identities != 3 {
            bail!("Incomplete commit identities");
        }
        Ok(tree.context("Missing commit tree")?.to_owned())
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

/// Raw tree object entries: `<mode> <name>\0<binary id>`.
fn tree_entries(bytes: &[u8], id_len: usize) -> Result<Vec<(&str, &str, String)>> {
    let mut entries = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        if entries.len() >= MAX_TREE_ENTRIES {
            bail!("Transport tree-entry limit exceeded; refused");
        }
        let space = rest
            .iter()
            .position(|b| *b == b' ')
            .context("Invalid tree object")?;
        let nul = rest
            .iter()
            .position(|b| *b == 0)
            .filter(|nul| *nul > space + 1 && rest.len() >= nul + 1 + id_len)
            .context("Invalid tree object")?;
        let mode = std::str::from_utf8(&rest[..space])
            .map_err(|_| anyhow::anyhow!("Invalid tree object"))?;
        let name = std::str::from_utf8(&rest[space + 1..nul])
            .map_err(|_| anyhow::anyhow!("Transport scan refused: path encoding"))?;
        if name.contains('/') {
            bail!("Invalid tree object");
        }
        let id = rest[nul + 1..nul + 1 + id_len]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        entries.push((mode, name, id));
        rest = &rest[nul + 1 + id_len..];
    }
    Ok(entries)
}

pub(super) use aida_core::transport_policy::{bounded, drain, Group};

/// Run the network push. Unlike the local readers it has no wall-clock kill
/// (a large first push must not be cut mid-transfer). It runs in its own
/// session: with no controlling terminal a credential or host-key prompt
/// fails at once instead of stopping on SIGTTIN, and Git's own prompts are
/// disabled. Agent and helper credential routing is unchanged. Ctrl+C and
/// SIGTERM are forwarded to the still-unreaped push group only, and helpers
/// that intentionally outlive the push (credential caches) are left alone.
// trace:STORY-1742 | ai:claude
pub(super) fn publish(mut cmd: Command, cap: usize) -> Result<(ExitStatus, Vec<u8>, bool)> {
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let pending = Arc::new(AtomicUsize::new(0));
    let mut hooks = Vec::new();
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        hooks.push(
            signal_hook::flag::register_usize(signal, Arc::clone(&pending), signal as usize)
                .map_err(|_| anyhow::anyhow!("Could not install push interrupt forwarding"))?,
        );
    }
    let result = (|| {
        let mut group = Group::spawn(cmd)?;
        let rx = drain(group.child.stdout.take().unwrap(), cap);
        let mut interrupted = false;
        while !group.exited() {
            let signal = pending.swap(0, Ordering::SeqCst);
            if signal != 0 {
                interrupted = true;
                group.signal(signal as i32);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = group.reap()?;
        // A helper still holding stdout means the porcelain is incomplete.
        let stdout = rx
            .recv_timeout(Duration::from_secs(5))
            .ok()
            .and_then(Result::ok)
            .filter(|bytes| bytes.len() <= cap)
            .unwrap_or_default();
        Ok((status, stdout, interrupted))
    })();
    for hook in hooks {
        signal_hook::low_level::unregister(hook);
    }
    result
}

struct Reader<'a> {
    store: &'a Path,
    deadline: Instant,
}
impl Reader<'_> {
    fn git(&self, args: &[&str]) -> Command {
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
            .args(args);
        cmd
    }

    fn run(&self, args: &[&str], input: &[u8], cap: usize) -> Result<Vec<u8>> {
        if Instant::now() >= self.deadline {
            bail!("Transport scan deadline exceeded");
        }
        let (status, bytes) = bounded(self.git(args), input, cap, self.deadline)?;
        if !status.success() {
            bail!("Transport graph incomplete or unsupported; refused");
        }
        Ok(bytes)
    }

    /// Full integrity for exactly the new delta: pack it privately and let
    /// `index-pack --strict` rehash every object and apply fsck's strict
    /// object and link checks, as a receiver with fsckObjects would. This
    /// replaces a whole-ODB fsck (which also walked unrelated code history)
    /// without dropping to connectivity-only. Trusted-root objects were
    /// accepted at enrollment; nothing here writes the repository's ODB.
    fn verify(&self, common: &Path, tip: &str, range: &str, fresh: &HashSet<&str>) -> Result<()> {
        let mut request = String::new();
        for oid in fresh {
            request.push_str(oid);
            request.push('\n');
        }
        let sizes = self.run(
            &["cat-file", "--batch-check=%(objectsize)"],
            request.as_bytes(),
            fresh.len() * 24,
        )?;
        let mut total = 0u64;
        for line in std::str::from_utf8(&sizes)
            .map_err(|_| anyhow::anyhow!("Invalid graph metadata"))?
            .lines()
        {
            let size: u64 = line.parse().context("Transport graph object is missing")?;
            if size > MAX_BYTES as u64 {
                bail!("Transport object exceeds size limit");
            }
            total += size;
        }
        if total > MAX_NEW_BYTES {
            bail!("Transport new-delta byte limit exceeded; refused");
        }
        let temp = tempfile::Builder::new()
            .prefix(".aida-transport-verify-")
            .tempdir_in(common)
            .map_err(|_| anyhow::anyhow!("Transport integrity workspace unavailable"))?;
        let pack = temp.path().join("delta.pack");
        let file = std::fs::File::create(&pack)
            .map_err(|_| anyhow::anyhow!("Transport integrity workspace unavailable"))?;
        let mut cmd = self.git(&["pack-objects", "--revs", "--stdout", "-q"]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::from(file))
            .stderr(Stdio::null())
            .process_group(0);
        let mut group = Group::spawn(cmd)?;
        if let Some(mut stdin) = group.child.stdin.take() {
            stdin
                .write_all(format!("{tip}\n{range}\n").as_bytes())
                .map_err(|_| anyhow::anyhow!("Transport integrity verification failed"))?;
        }
        while !group.exited() {
            let written = std::fs::metadata(&pack).map(|m| m.len()).unwrap_or(0);
            if Instant::now() >= self.deadline || written > MAX_NEW_BYTES + (64 << 20) {
                bail!("Transport integrity verification limit or deadline exceeded");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        group.signal(libc::SIGKILL);
        if !group.reap()?.success() {
            bail!("Transport graph incomplete or unsupported; refused");
        }
        let index = temp.path().join("delta.idx");
        self.run(
            &[
                "index-pack",
                "--strict",
                "-o",
                index.to_str().context("Invalid integrity workspace")?,
                pack.to_str().context("Invalid integrity workspace")?,
            ],
            &[],
            4096,
        )
        .map_err(|_| anyhow::anyhow!("Transport integrity verification failed; refused"))?;
        Ok(())
    }

    fn supported(&self) -> Result<()> {
        if self.run(&["rev-parse", "--is-shallow-repository"], &[], 32)? != b"false\n" {
            bail!("Incomplete transport graph; refused");
        }
        if !self
            .run(
                &["for-each-ref", "--format=%(refname)", "refs/replace/"],
                &[],
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
                &[],
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
        Ok(())
    }
}

/// One long-lived `cat-file --batch` reader, bounded by the scan deadline
/// and the read budget instead of two process spawns per object.
struct Batch {
    group: Group,
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<Result<(String, Vec<u8>)>>,
    deadline: Instant,
    read: u64,
}

impl Batch {
    fn open(reader: &Reader<'_>) -> Result<Self> {
        let mut cmd = reader.git(&["cat-file", "--batch"]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let mut group = Group::spawn(cmd)?;
        let stdin = group.child.stdin.take().unwrap();
        let mut stdout = BufReader::new(group.child.stdout.take().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut header = Vec::new();
            let response = (|| -> Result<(String, Vec<u8>)> {
                // Fixed, value-free diagnostics only; `get` passes them on.
                let incomplete =
                    || anyhow::anyhow!("Transport graph incomplete or unsupported; refused");
                (&mut stdout)
                    .take(256)
                    .read_until(b'\n', &mut header)
                    .map_err(|_| incomplete())?;
                let header = std::str::from_utf8(&header).map_err(|_| incomplete())?;
                let mut fields = header.trim_end().split(' ').skip(1);
                let (Some(kind), Some(size), None) = (fields.next(), fields.next(), fields.next())
                else {
                    bail!("Transport graph object is missing");
                };
                let size: usize = size.parse().map_err(|_| incomplete())?;
                if size > MAX_BYTES {
                    bail!("Transport object exceeds size limit");
                }
                let mut bytes = vec![0; size + 1];
                stdout.read_exact(&mut bytes).map_err(|_| incomplete())?;
                if bytes.pop() != Some(b'\n') {
                    bail!("Invalid graph object framing");
                }
                Ok((kind.to_owned(), bytes))
            })();
            let failed = response.is_err();
            if tx.send(response).is_err() || failed {
                return;
            }
        });
        Ok(Self {
            group,
            stdin,
            rx,
            deadline: reader.deadline,
            read: 0,
        })
    }

    fn get(&mut self, oid: &str, kind: &str) -> Result<Vec<u8>> {
        writeln!(self.stdin, "{oid}")
            .and_then(|()| self.stdin.flush())
            .map_err(|_| anyhow::anyhow!("Transport graph reader failed"))?;
        let wait = self.deadline.saturating_duration_since(Instant::now());
        let (actual, bytes) = match self.rx.recv_timeout(wait) {
            Ok(response) => response?,
            Err(_) => {
                self.group.signal(libc::SIGKILL);
                bail!("Transport scan deadline exceeded");
            }
        };
        if actual != kind {
            bail!("Transport graph object has an unexpected type");
        }
        self.read += bytes.len() as u64;
        if self.read > MAX_READ_BYTES {
            bail!("Transport scan read limit exceeded; refused");
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_1742_tree_entries_parse_closed_binary_framing() {
        // trace:STORY-1742 | ai:claude
        let mut tree = b"100644 a.yaml\0".to_vec();
        tree.extend([0xab; 20]);
        tree.extend(b"40000 objects\0");
        tree.extend([0x01; 20]);
        let entries = tree_entries(&tree, 20).unwrap();
        assert_eq!(entries[0].0, "100644");
        assert_eq!(entries[0].2, "ab".repeat(20));
        assert_eq!(entries[1].1, "objects");
        assert!(tree_entries(&tree[..tree.len() - 1], 20).is_err());
        assert!(tree_entries(b"100644 a/b\0aaaaaaaaaaaaaaaaaaaa", 20).is_err());
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
        assert!(bounded(command, &[], 1024, Instant::now() + Duration::from_secs(3)).is_err());
        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "helper survived an incomplete transport read"
        );
    }

    #[test]
    #[cfg(unix)]
    fn story_1742_push_runner_disables_prompts_and_forwards_interrupts() {
        // trace:STORY-1742 | ai:claude
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf '%s' \"$GIT_TERMINAL_PROMPT\"; if [ -r /dev/tty ] && (exec </dev/tty) 2>/dev/null; then printf tty; fi",
        ]);
        let (status, stdout, interrupted) = publish(command, 1024).unwrap();
        assert!(status.success());
        assert_eq!(stdout, b"0", "push must have no controlling terminal");
        assert!(!interrupted);

        let mut command = Command::new("sh");
        command.args([
            "-c",
            "trap 'printf forwarded; exit 7' INT; printf ready; while :; do sleep 0.05; done",
        ]);
        let raise = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(400));
            unsafe {
                libc::raise(libc::SIGINT);
            }
        });
        let (status, stdout, interrupted) = publish(command, 1024).unwrap();
        raise.join().unwrap();
        assert!(interrupted);
        assert_eq!(status.code(), Some(7));
        assert_eq!(stdout, b"readyforwarded");
    }
}
