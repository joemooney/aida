//! Hypothesis evidence boundary around AIDA-owned store synchronization.
//!
//! A clean textual rebase or merge can join an offline description rewrite
//! with a newly appended `hypothesis_lock`, or bring a hypothesis and its first
//! completed result together from separate clones, without any structured
//! resolver running. Every store rebase, union merge and reconcile
//! fast-forward therefore pins its inputs, strictly reads each input graph and
//! refuses incompatible evidence before Git runs, then validates the resulting
//! graph and commits any lock the combined graph first establishes before it
//! reports success. The post-Git check is a success gate, not a rollback: on a
//! failure there it returns an error naming the pinned and resulting commits
//! and never resets, force-pushes or repairs.
// trace:BUG-2162 | ai:claude
use super::{
    commit_paths, git, head_sha, host_git_command, is_store_tree, merge_in_progress,
    rebase_in_progress, store_worktree_issue,
};
use crate::hypothesis::{finalize_sync, plan_sync, EvidenceRefusal};
use crate::models::Requirement;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use uuid::Uuid;

fn refusal(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(EvidenceRefusal(message.into()))
}

/// Run one synchronization under the store write lock, re-entering it when
/// the caller already holds it. Non-store repositories run unchanged.
pub(super) fn with_sync_lock<T>(repo: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    if is_store_tree(repo) {
        crate::db::with_store_write_lock(repo, f)
    } else {
        f()
    }
}

#[cfg(test)]
thread_local! {
    /// Test seam: 1 fails the candidate read, 2 the lock write, 3 the commit.
    pub(super) static FAIL_FINALIZE_AT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

fn injected(step: u8) -> Result<()> {
    #[cfg(test)]
    if FAIL_FINALIZE_AT.with(|f| f.get()) == step {
        anyhow::bail!("injected finalization failure at step {step}");
    }
    let _ = step;
    Ok(())
}

/// Bytes from one plumbing command; the text wrapper trims and decodes lossily.
fn git_bytes(repo: &Path, args: &[&str], input: Option<Vec<u8>>) -> Result<Vec<u8>> {
    let mut child = host_git_command(repo)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to run: git {}", args.join(" ")))?;
    let writer = input.map(|bytes| {
        let mut stdin = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || stdin.write_all(&bytes))
    });
    let output = child.wait_with_output()?;
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("git {} input writer panicked", args.join(" ")))??;
    }
    anyhow::ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

/// One pinned commit's canonical object inventory: `(path, blob)` pairs.
struct Listing {
    commit: String,
    entries: Vec<(String, String)>,
}

fn mentions_hypothesis(text: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN
        .get_or_init(|| regex::Regex::new("(?i)hypothesis").expect("valid pattern"))
        .is_match(text)
}

/// Strict reader of the canonical requirement objects in pinned commits.
#[derive(Default)]
struct SnapshotReader {
    text: HashMap<String, String>,
    parsed: HashMap<String, Requirement>,
    /// Per (blob, file name): must the blob be parsed on its text alone?
    textual: HashMap<(String, String), bool>,
}

impl SnapshotReader {
    /// List every `objects/<TYPE>/<SHARD>/*.yaml` object of `commit`, the
    /// inventory `object_store::list_objects_strict` reads from a working
    /// tree. Symlinks and submodules refuse.
    fn list(&mut self, repo: &Path, commit: &str) -> Result<Listing> {
        let listing = git_bytes(
            repo,
            &[
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                commit,
                "--",
                "objects/",
            ],
            None,
        )
        .map_err(|e| refusal(format!("cannot list the store objects of {commit}: {e:#}")))?;
        let mut entries = Vec::new();
        for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let entry = std::str::from_utf8(entry)
                .map_err(|_| refusal(format!("{commit} has a non-UTF-8 object path")))?;
            let unreadable = || refusal(format!("unreadable tree entry in {commit}: {entry}"));
            let (meta, path) = entry.split_once('\t').ok_or_else(unreadable)?;
            let mut meta = meta.split(' ');
            let (Some(mode), Some(_kind), Some(oid)) = (meta.next(), meta.next(), meta.next())
            else {
                return Err(unreadable());
            };
            let depth = path.split('/').count();
            if depth > 4 {
                continue;
            }
            if mode == "120000" || mode == "160000" {
                return Err(refusal(format!(
                    "{commit} holds a symlink or submodule at {path} in the object store"
                )));
            }
            if depth == 4 && path.ends_with(".yaml") {
                if mode != "100644" && mode != "100755" {
                    return Err(refusal(format!(
                        "{commit} holds a non-file object at {path}"
                    )));
                }
                entries.push((path.to_string(), oid.to_string()));
            }
        }
        Ok(Listing {
            commit: commit.to_string(),
            entries,
        })
    }

    /// Load the text of every blob in `wanted`. A blob that is missing,
    /// truncated or not UTF-8 refuses.
    fn load(&mut self, repo: &Path, wanted: Vec<String>, commit: &str) -> Result<()> {
        if !wanted.is_empty() {
            let mut input = wanted.join("\n").into_bytes();
            input.push(b'\n');
            let batch = git_bytes(repo, &["cat-file", "--batch"], Some(input))
                .map_err(|e| refusal(format!("cannot read store objects of {commit}: {e:#}")))?;
            let mut rest = batch.as_slice();
            let truncated = || refusal(format!("truncated object read for {commit}"));
            for oid in wanted {
                let line_end = rest
                    .iter()
                    .position(|b| *b == b'\n')
                    .ok_or_else(truncated)?;
                let header = String::from_utf8_lossy(&rest[..line_end]).into_owned();
                rest = &rest[line_end + 1..];
                let mut fields = header.split(' ');
                let size = match (fields.next(), fields.next(), fields.next()) {
                    (Some(got), Some("blob"), Some(size)) if got == oid => {
                        size.parse::<usize>().ok()
                    }
                    _ => None,
                }
                .ok_or_else(|| {
                    refusal(format!(
                        "store object {oid} of {commit} is missing: {header}"
                    ))
                })?;
                if rest.len() <= size {
                    return Err(truncated());
                }
                let text = String::from_utf8(rest[..size].to_vec())
                    .map_err(|_| refusal(format!("store object {oid} of {commit} is not UTF-8")))?;
                self.text.insert(oid, text);
                rest = &rest[size + 1..];
            }
        }
        Ok(())
    }

    fn parse(&mut self, oid: &str, path: &str, commit: &str) -> Result<()> {
        if self.parsed.contains_key(oid) {
            return Ok(());
        }
        let row: Requirement = serde_yaml::from_str(&self.text[oid])
            .map_err(|e| refusal(format!("cannot parse {path} in {commit}: {e}")))?;
        self.parsed.insert(oid.to_string(), row);
        Ok(())
    }

    /// The graphs of `listings`, read strictly wherever evidence can live.
    ///
    /// A blob is parsed when its path differs between any two listings, when
    /// it mentions a hypothesis at all (every tag and lock does), when it does
    /// not plainly name its own file's ID, or when it contains the UUID of a
    /// parsed row or of a parsed row's edge target. Only a row byte-identical
    /// in every listing, which the synchronization cannot change and which can
    /// neither carry evidence nor join an edge to a parsed row, is left out.
    ///
    /// `worktree` is a listed commit the clean working tree matches; such a
    /// row's text is read from its file rather than decompressed by Git.
    fn graphs(
        &mut self,
        repo: &Path,
        listings: &[&Listing],
        worktree: Option<&str>,
    ) -> Result<Vec<Vec<Requirement>>> {
        let mut at: HashMap<&str, (usize, HashSet<&str>)> = HashMap::new();
        for listing in listings {
            for (path, oid) in &listing.entries {
                let slot = at.entry(path.as_str()).or_default();
                slot.0 += 1;
                slot.1.insert(oid.as_str());
            }
        }
        let changed: HashSet<&str> = at
            .values()
            .filter(|(seen, oids)| *seen != listings.len() || oids.len() != 1)
            .flat_map(|(_, oids)| oids.iter().copied())
            .collect();
        let on_disk = worktree.is_some_and(|c| listings.iter().any(|l| l.commit == c));
        let mut wanted = HashSet::new();
        for listing in listings {
            for (path, oid) in &listing.entries {
                if self.text.contains_key(oid) {
                    continue;
                }
                if on_disk && !changed.contains(oid.as_str()) {
                    if let Ok(text) = std::fs::read_to_string(repo.join(path)) {
                        self.text.insert(oid.clone(), text);
                        continue;
                    }
                }
                wanted.insert(oid.clone());
            }
        }
        let label = listings
            .iter()
            .map(|l| l.commit.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        self.load(repo, wanted.into_iter().collect(), &label)?;
        let mut later = Vec::new();
        let mut primary = HashSet::new();
        for listing in listings {
            for (path, oid) in &listing.entries {
                let stem = path.rsplit('/').next().unwrap_or_default();
                let key = (oid.clone(), stem.to_string());
                let textual = match self.textual.get(&key) {
                    Some(hit) => *hit,
                    None => {
                        let text = &self.text[oid];
                        let named = format!("\nspec_id: {}\n", stem.trim_end_matches(".yaml"));
                        let hit = mentions_hypothesis(text) || !text.contains(&named);
                        self.textual.insert(key, hit);
                        hit
                    }
                };
                if textual || changed.contains(oid.as_str()) {
                    self.parse(oid, path, &listing.commit)?;
                    primary.insert(oid.as_str());
                } else {
                    later.push((oid, path, &listing.commit));
                }
            }
        }
        // Only an edge with a hypothesis endpoint can lock anything, and every
        // hypothesis or stamp is a primary row. Its neighbours are the rows
        // naming its UUID (incoming edges, duplicates) or one of its targets.
        let primary: Vec<&Requirement> = primary.iter().map(|oid| &self.parsed[*oid]).collect();
        let evidence: HashSet<Uuid> = primary
            .iter()
            .filter(|r| crate::hypothesis::holds_evidence(std::slice::from_ref(*r)))
            .map(|r| r.id)
            .collect();
        let related: HashSet<String> = primary
            .iter()
            .filter(|r| evidence.contains(&r.id))
            .flat_map(|r| std::iter::once(r.id).chain(r.relationships.iter().map(|e| e.target_id)))
            .map(|u| regex::escape(&u.to_string()))
            .collect();
        if !related.is_empty() {
            let pattern = format!("(?i){}", related.into_iter().collect::<Vec<_>>().join("|"));
            let neighbour = regex::Regex::new(&pattern).context("build the UUID matcher")?;
            for (oid, path, commit) in later {
                if !self.parsed.contains_key(oid.as_str())
                    && neighbour.is_match(&self.text[oid.as_str()])
                {
                    self.parse(oid, path, commit)?;
                }
            }
        }
        listings
            .iter()
            .map(|listing| {
                listing
                    .entries
                    .iter()
                    .filter_map(|(path, oid)| Some((path, self.parsed.get(oid)?)))
                    .map(|(path, row)| {
                        let stem = path.rsplit('/').next().unwrap_or_default();
                        let named = row.spec_id.as_deref().is_some_and(|sid| {
                            format!("{}.yaml", crate::object_store::canonical_spec_id(sid)) == stem
                        });
                        if !named {
                            return Err(refusal(format!(
                                "{path} in {} holds {}, not the row its name says",
                                listing.commit,
                                row.display_id()
                            )));
                        }
                        Ok(row.clone())
                    })
                    .collect()
            })
            .collect()
    }
}

/// Whether tracked store files differ from HEAD. Untracked files take no part
/// in a synchronization and do not count.
fn tracked_changes(repo: &Path) -> Result<bool> {
    let status = git(
        repo,
        &[
            "status",
            "--porcelain",
            "--untracked-files=no",
            "--ignore-submodules=all",
        ],
    )?;
    anyhow::ensure!(status.success, "git status failed: {}", status.stderr);
    Ok(!status.stdout.is_empty())
}

/// HEAD's commit, or None on a proven unborn branch. A HEAD that cannot be
/// read is an error, never an empty graph.
fn local_tip(repo: &Path) -> Result<Option<String>> {
    let head = git(repo, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    if head.success {
        return Ok(Some(head.stdout));
    }
    let symref = git(repo, &["symbolic-ref", "--quiet", "HEAD"])?;
    if symref.success {
        let branch = git(repo, &["rev-parse", "--verify", "--quiet", &symref.stdout])?;
        if !branch.success && branch.stderr.is_empty() {
            return Ok(None);
        }
    }
    Err(refusal(format!(
        "cannot read the store HEAD of {}: {}",
        repo.display(),
        head.stderr
    )))
}

/// The evidence one synchronization must preserve, computed before Git runs.
pub(super) struct EvidenceGate {
    local: Option<String>,
    target: String,
    bases: Vec<Listing>,
    tips: Vec<Listing>,
    reader: SnapshotReader,
    now: chrono::DateTime<chrono::Utc>,
}

/// Input graphs (bases, tips) and, when given, the candidate's graph.
type Graphs = (
    Vec<Vec<Requirement>>,
    Vec<Vec<Requirement>>,
    Vec<Requirement>,
);

fn refs(graphs: &[Vec<Requirement>]) -> Vec<&[Requirement]> {
    graphs.iter().map(Vec::as_slice).collect()
}

/// Pin `target` and the local HEAD, read them and all their merge bases, and
/// refuse incompatible evidence while the branch and index are unchanged.
/// None for a repository that is not an AIDA store.
pub(super) fn preflight(repo: &Path, target: &str) -> Result<Option<EvidenceGate>> {
    if !is_store_tree(repo) {
        return Ok(None);
    }
    let resolved = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{target}^{{commit}}"),
        ],
    )?;
    anyhow::ensure!(
        resolved.success,
        "cannot resolve store sync target {target}: {}",
        resolved.stderr
    );
    let target = resolved.stdout;
    let local = local_tip(repo)?;
    let mut reader = SnapshotReader::default();
    let mut tips = Vec::new();
    let mut bases = Vec::new();
    if let Some(local) = &local {
        tips.push(reader.list(repo, local)?);
        let found = git(repo, &["merge-base", "--all", local, &target])?;
        if found.success {
            for base in found.stdout.lines().filter(|l| !l.trim().is_empty()) {
                bases.push(reader.list(repo, base.trim())?);
            }
        } else if !(found.stdout.is_empty() && found.stderr.is_empty()) {
            return Err(refusal(format!(
                "cannot find the merge base of {local} and {target}: {}",
                found.stderr
            )));
        }
    }
    tips.push(reader.list(repo, &target)?);
    let mut gate = EvidenceGate {
        local,
        target,
        bases,
        tips,
        reader,
        now: chrono::Utc::now(),
    };
    let (bases, tips, _) = gate.graphs(repo, None)?;
    let plan = plan_sync(&refs(&bases), &refs(&tips), gate.now).map_err(refusal)?;
    // Without a base a deletion cannot be told from a row the other side never
    // had, so unrelated histories may only join while neither holds evidence.
    if gate.local.is_some()
        && bases.is_empty()
        && (!plan.is_empty() || tips.iter().any(|g| crate::hypothesis::holds_evidence(g)))
    {
        return Err(refusal(format!(
            "the local store and {} share no history and hold hypothesis evidence; refusing to join them",
            gate.target
        )));
    }
    Ok(Some(gate))
}

impl EvidenceGate {
    fn graphs(&mut self, repo: &Path, candidate: Option<&Listing>) -> Result<Graphs> {
        let mut listings: Vec<&Listing> = self.bases.iter().chain(self.tips.iter()).collect();
        listings.extend(candidate);
        let worktree = match tracked_changes(repo) {
            Ok(false) => head_sha(repo).ok(),
            _ => None,
        };
        let mut graphs = self.reader.graphs(repo, &listings, worktree.as_deref())?;
        let candidate = if candidate.is_some() {
            graphs.pop().unwrap_or_default()
        } else {
            Vec::new()
        };
        let tips = graphs.split_off(self.bases.len());
        Ok((graphs, tips, candidate))
    }

    /// Validate the graph at `commit` and return the rows needing a lock.
    fn check(&mut self, repo: &Path, commit: &str) -> Result<Vec<Requirement>> {
        let listing = self.reader.list(repo, commit)?;
        let (bases, tips, candidate) = self.graphs(repo, Some(&listing))?;
        finalize_sync(&refs(&bases), &refs(&tips), &candidate, self.now).map_err(refusal)
    }

    /// The success gate after Git finished: validate the resulting graph,
    /// commit any lock the combined graph established, and re-verify.
    pub(super) fn finalize(mut self, repo: &Path) -> Result<()> {
        let head = head_sha(repo).unwrap_or_else(|_| "<unreadable>".to_string());
        self.finalize_at(repo, &head).map_err(|e| {
            let now_head = head_sha(repo).unwrap_or_else(|_| "<unreadable>".to_string());
            let worktree = match tracked_changes(repo) {
                Ok(true) => "has uncommitted changes",
                Ok(false) => "is clean",
                Err(_) => "could not be inspected",
            };
            let state = store_worktree_issue(repo).unwrap_or("on its branch");
            refusal(format!(
                "store sync stopped after Git finished: {e:#}. Local was {}, incoming {}, the store is now at {now_head} ({state}) and its worktree {worktree}. Nothing was reset or pushed; inspect the store history and repair it explicitly before retrying",
                self.local.as_deref().unwrap_or("<unborn>"),
                self.target
            ))
        })
    }

    fn finalize_at(&mut self, repo: &Path, head: &str) -> Result<()> {
        if let Some(issue) = store_worktree_issue(repo) {
            anyhow::bail!("the store is left in an unsafe state: {issue}");
        }
        anyhow::ensure!(
            !rebase_in_progress(repo) && !merge_in_progress(repo),
            "a rebase or merge is still in progress"
        );
        injected(1)?;
        let writes = self.check(repo, head)?;
        if writes.is_empty() {
            return Ok(());
        }
        let objects = repo.join("objects");
        let mut paths = Vec::new();
        for row in &writes {
            injected(2)?;
            crate::object_store::write_object_if_changed(&objects, row)?;
            let sid = row.spec_id.as_deref().unwrap_or_default();
            paths.push(crate::object_store::relative_object_path(sid)?);
        }
        injected(3)?;
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        commit_paths(
            repo,
            "aida: lock hypothesis evidence after store sync",
            &paths,
        )?;
        let locked = head_sha(repo)?;
        anyhow::ensure!(
            super::first_parent_sha(repo, &locked).as_deref() == Some(head),
            "the lock commit {locked} is not a child of {head}"
        );
        anyhow::ensure!(
            !tracked_changes(repo)?,
            "the lock commit left uncommitted store changes"
        );
        anyhow::ensure!(
            self.check(repo, &locked)?.is_empty(),
            "the lock commit did not persist every hypothesis lock"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;
