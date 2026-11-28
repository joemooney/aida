//! Shadow adversarial review trial: off unless `[review] shadow_adversarial`
//! is on. A shadow run never records a verdict, never touches a merge hold,
//! never posts to the forge and never blocks the review it shadows; its only
//! output is one row in a local results file. Any failure is logged and
//! ignored.
// trace:STORY-3183 | ai:claude
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// At most this many shadow runs at once.
pub(crate) const MAX_SHADOW: usize = 2;
/// Finding text can quote code; keep at most this many characters of it.
const TEXT_CAP: usize = 500;
/// The trial pairs the default review vendor (claude) with itself until a
/// second vendor is available again: a labelled same-vendor run.
const FINDER: &str = crate::vendors::review_runner::DEFAULT_REVIEW_VENDOR.program;
const CHALLENGER: &str = FINDER;
const RESULTS: &str = ".aida/review-shadow/runs.jsonl";

fn parse(body: Option<&str>) -> Result<bool> {
    #[derive(Default, serde::Deserialize)]
    struct Review {
        #[serde(default)]
        shadow_adversarial: bool,
    }
    #[derive(Default, serde::Deserialize)]
    struct Config {
        #[serde(default)]
        review: Review,
    }
    Ok(match body {
        Some(body) => {
            toml::from_str::<Config>(body)
                .context("parsing [review] shadow_adversarial")?
                .review
                .shadow_adversarial
        }
        None => false,
    })
}

/// Shadow runs in flight, counted against the reviewer budget.
pub(crate) struct Budget(AtomicUsize);
static RUNNING: Budget = Budget(AtomicUsize::new(0));

struct Slot(&'static Budget);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Budget {
    /// A slot only from spare capacity: shadows never take a slot a real
    /// review is using, and real admission never waits for a shadow.
    fn acquire(&'static self, real_active: usize, max: usize) -> Option<Slot> {
        let mut n = self.0.load(Ordering::SeqCst);
        loop {
            if n >= MAX_SHADOW || real_active + n >= max {
                return None;
            }
            match self
                .0
                .compare_exchange(n, n + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Some(Slot(self)),
                Err(now) => n = now,
            }
        }
    }
}

/// Where the shadow launches from: the watcher's own command builder (the
/// S3a runner path, with review env scrubbed) and its `aida` executable.
pub(crate) struct Launch {
    pub root: PathBuf,
    pub command: std::process::Command,
    pub pr: u64,
    pub head: String,
}

/// Start a shadow run beside a dispatched review when the knob is on and
/// capacity is spare. Never fails: a refusal is logged.
pub(crate) fn launch(
    launch: Launch,
    real_active: usize,
    max: u8,
) -> Option<std::thread::JoinHandle<()>> {
    match parse(crate::trusted_config::read_trusted_config_toml_local(&launch.root).as_deref()) {
        Ok(true) => start(&RUNNING, launch, real_active, usize::from(max)),
        Ok(false) => None,
        Err(error) => {
            eprintln!("shadow review skipped: {error:#}");
            None
        }
    }
}

fn start(
    budget: &'static Budget,
    mut launch: Launch,
    real_active: usize,
    max: usize,
) -> Option<std::thread::JoinHandle<()>> {
    let Some(slot) = budget.acquire(real_active, max) else {
        eprintln!(
            "PR #{}: shadow review skipped: no spare reviewer capacity",
            launch.pr
        );
        return None;
    };
    let pr = launch.pr.to_string();
    // Never `--post-comment`: a shadow never writes to the forge.
    launch.command.args([
        "review",
        "prepare",
        "--pr",
        &pr,
        "--run-review",
        "--adversarial",
        "--agent",
        FINDER,
        "--challenger-agent",
        CHALLENGER,
        "--json",
    ]);
    std::thread::Builder::new()
        .name(format!("shadow-review-{pr}"))
        .spawn(move || {
            let _slot = slot;
            let row = run(&mut launch);
            if let Err(error) = append(&launch.root, &row) {
                eprintln!(
                    "PR #{}: shadow review row not written: {error:#}",
                    launch.pr
                );
            }
        })
        .map_err(|error| eprintln!("shadow review thread refused: {error}"))
        .ok()
}

fn cap(text: &str) -> String {
    text.chars().take(TEXT_CAP).collect()
}

fn run(launch: &mut Launch) -> serde_json::Value {
    let started = Instant::now();
    let at = chrono::Utc::now().to_rfc3339();
    let output = launch.command.stdin(std::process::Stdio::null()).output();
    let duration_ms = started.elapsed().as_millis() as u64;
    let (code, stdout, error) = match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
            let error = (!out.status.success()).then(|| cap(last.unwrap_or("no stderr")));
            (out.status.code(), out.stdout, error)
        }
        Err(e) => (None, Vec::new(), Some(cap(&format!("spawn failed: {e}")))),
    };
    let bundle: serde_json::Value = serde_json::from_slice(&stdout).unwrap_or_default();
    let adv = &bundle["adversarial"];
    let challenger = adv["challenger_agent"].as_str().unwrap_or(CHALLENGER);
    let mut findings = Vec::new();
    let mut push = |f: &serde_json::Value, refuted: bool, reason: Option<&str>| {
        let line = f["resolved_span"]["start_line"]
            .as_u64()
            .or_else(|| f["start_line"].as_u64());
        findings.push(serde_json::json!({
            "severity": f["severity"].as_str().map(cap),
            "path": f["path"].as_str().map(cap),
            "line": line,
            "message": f["message"].as_str().map(cap),
            "refuted": refuted,
            "reason": reason.map(cap),
        }));
    };
    let list = |v: &serde_json::Value| v.as_array().cloned().unwrap_or_default();
    for f in list(&bundle["anchored_findings"]) {
        push(&f, false, None);
    }
    for f in list(&adv["unchallenged"]) {
        push(&f, false, Some("unchallenged: challenger unavailable"));
    }
    for r in list(&adv["refuted"]) {
        push(&r["finding"], true, r["reason"].as_str());
    }
    serde_json::json!({
        "at": at,
        "pr": launch.pr,
        "head": launch.head,
        "reviewed_head": bundle["head_ref"].as_str(),
        "finder": FINDER,
        "challenger": challenger,
        "same_vendor": challenger == FINDER,
        "vendor_pair": if challenger == FINDER { "same-vendor" } else { "cross-vendor" },
        "exit_code": code,
        "partial": code == Some(4),
        "duration_ms": duration_ms,
        "error": error,
        "findings": findings,
    })
}

static APPEND: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn append(root: &Path, row: &serde_json::Value) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let path = root.join(RESULTS);
    let dir = path.parent().context("results directory")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let _guard = APPEND.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let mut line = serde_json::to_string(row)?;
    line.push('\n');
    file.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn budget() -> &'static Budget {
        Box::leak(Box::new(Budget(AtomicUsize::new(0))))
    }

    fn tree(root: &Path) -> std::collections::BTreeSet<PathBuf> {
        let mut out = std::collections::BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.insert(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
        out
    }

    /// A stub `aida`: records its arguments, prints a bundle, exits `code`.
    fn stub(dir: &Path, bundle: &str, code: i32) -> PathBuf {
        let path = dir.join("aida-stub");
        let body = format!(
            "#!/bin/sh\necho \"$@\" >>\"{}\"\ncat <<'EOF'\n{bundle}\nEOF\necho 'partial: challenger unavailable' >&2\nexit {code}\n",
            dir.join("args").display()
        );
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn launch(root: &Path, program: &Path, pr: u64) -> Launch {
        let mut command = std::process::Command::new(program);
        command.current_dir(root);
        Launch {
            root: root.to_path_buf(),
            command,
            pr,
            head: "abc123".into(),
        }
    }

    fn rows(root: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(root.join(RESULTS))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn story_3183_knob_defaults_off_and_is_bool_only() {
        assert!(!parse(None).unwrap());
        assert!(!parse(Some("[review]\nimplementer_self_review = true")).unwrap());
        assert!(parse(Some("[review]\nshadow_adversarial = true")).unwrap());
        assert!(parse(Some("[review]\nshadow_adversarial = 'yes'")).is_err());
    }

    #[test]
    fn story_3183_knob_off_launches_no_shadow() {
        let project = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let program = stub(bin.path(), "{}", 0);
        // No trusted config enables it, so nothing is spawned or written.
        assert!(super::launch(launch(project.path(), &program, 7), 0, 8).is_none());
        assert!(!bin.path().join("args").exists());
        assert!(tree(project.path()).is_empty());
    }

    #[test]
    fn story_3183_shadow_writes_only_its_results_row() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".aida/gate-ledger")).unwrap();
        let before = tree(project.path());
        let bin = tempfile::tempdir().unwrap();
        let long = "x".repeat(900);
        let bundle = serde_json::json!({
            "head_ref": "abc123",
            "inventory": {"files": [{"path": "src/a.rs", "content": "SECRET FILE BODY"}]},
            "anchored_findings": [{"path": "src/a.rs", "severity": "high", "category": "bug",
                "message": long, "existing_code": "SECRET SNIPPET", "resolved_span": {"start_line": 12, "end_line": 13, "match_kind": "exact"}}],
            "adversarial": {"challenger_agent": "claude", "ai_authors": [],
                "refuted": [{"finding": {"path": "src/b.rs", "severity": "low", "category": "style",
                    "message": "nit", "start_line": 3}, "reason": "not reachable"}]}
        });
        let program = stub(bin.path(), &bundle.to_string(), 0);
        start(budget(), launch(project.path(), &program, 42), 0, 4)
            .unwrap()
            .join()
            .unwrap();
        let args = std::fs::read_to_string(bin.path().join("args")).unwrap();
        assert_eq!(
            args.trim(),
            "review prepare --pr 42 --run-review --adversarial --agent claude --challenger-agent claude --json"
        );
        // Nothing outward-facing: no --post-comment, and no verdict command.
        assert!(!args.contains("post-comment") && !args.contains("record"));
        // The only new paths: the results directory and its file. The gate
        // ledger stays empty; no store, hold or receipt file appears.
        let added: Vec<_> = tree(project.path()).difference(&before).cloned().collect();
        assert_eq!(
            added,
            [PathBuf::from(".aida/review-shadow"), PathBuf::from(RESULTS)]
        );
        assert!(std::fs::read_dir(project.path().join(".aida/gate-ledger"))
            .unwrap()
            .next()
            .is_none());
        let mode = std::fs::metadata(project.path().join(RESULTS))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let text = std::fs::read_to_string(project.path().join(RESULTS)).unwrap();
        assert!(!text.contains("SECRET"), "no file contents: {text}");
        let row = &rows(project.path())[0];
        assert_eq!(row["pr"], 42);
        assert_eq!(row["head"], "abc123");
        assert_eq!(row["finder"], "claude");
        assert_eq!(row["challenger"], "claude");
        assert_eq!(row["same_vendor"], true);
        assert_eq!(row["vendor_pair"], "same-vendor");
        assert_eq!(row["exit_code"], 0);
        assert!(row["duration_ms"].is_u64());
        let f = &row["findings"];
        assert_eq!(f[0]["severity"], "high");
        assert_eq!(f[0]["path"], "src/a.rs");
        assert_eq!(f[0]["line"], 12);
        assert_eq!(f[0]["refuted"], false);
        assert_eq!(f[0]["message"].as_str().unwrap().chars().count(), TEXT_CAP);
        assert_eq!(f[1]["line"], 3);
        assert_eq!(f[1]["refuted"], true);
        assert_eq!(f[1]["reason"], "not reachable");
    }

    #[test]
    fn story_3183_failure_and_partial_are_logged_and_ignored() {
        let project = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let program = stub(bin.path(), "not json", 4);
        start(budget(), launch(project.path(), &program, 5), 0, 4)
            .unwrap()
            .join()
            .unwrap();
        let missing = project.path().join("no-such-aida");
        start(budget(), launch(project.path(), &missing, 6), 0, 4)
            .unwrap()
            .join()
            .unwrap();
        let rows = rows(project.path());
        assert_eq!(rows[0]["exit_code"], 4);
        assert_eq!(rows[0]["partial"], true);
        assert_eq!(rows[0]["error"], "partial: challenger unavailable");
        assert_eq!(rows[0]["findings"], serde_json::json!([]));
        assert!(rows[1]["exit_code"].is_null());
        assert!(rows[1]["error"]
            .as_str()
            .unwrap()
            .starts_with("spawn failed"));
    }

    #[test]
    fn story_3183_at_most_two_shadows_and_only_spare_capacity() {
        let budget = budget();
        let a = budget.acquire(0, 8).unwrap();
        let b = budget.acquire(0, 8).unwrap();
        assert!(budget.acquire(0, 8).is_none(), "cap of two");
        drop(a);
        // Real reviews hold 3 of 4 slots and one shadow runs: none spare.
        assert!(budget.acquire(3, 4).is_none());
        assert!(budget.acquire(2, 4).is_some());
        drop(b);
        // Real reviews hold the whole budget: no shadow at all.
        assert!(budget.acquire(4, 4).is_none());
    }
}
