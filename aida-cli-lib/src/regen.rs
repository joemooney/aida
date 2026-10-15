//! Versioned regeneration corpus. Only committed spec content crosses this boundary.
// trace:TASK-1729 | ai:codex

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use aida_core::models::{
    Comment, RelationshipType, Requirement, RequirementPriority, RequirementStatus, RequirementType,
};
use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::cli::RegenCommand;

#[derive(Debug, Serialize)]
pub(crate) struct CorpusManifest {
    pub(crate) format_version: u32,
    pub(crate) store_sha: String,
    pub(crate) corpus_file: &'static str,
    pub(crate) content_sha256: String,
    pub(crate) row_count: usize,
    /// Present only for the explicit dirty-store override; bytes still come from HEAD.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) dirty: bool,
}

#[derive(Serialize)]
struct Corpus<'a> {
    format_version: u32,
    rows: Vec<CorpusRow<'a>>,
}

/// Explicit allow-list: serializing Requirement directly would leak file linkage,
/// processing/history metadata and any future model fields into the blind trial.
#[derive(Serialize)]
struct CorpusRow<'a> {
    id: Uuid,
    spec_id: &'a Option<String>,
    agreed_id: &'a Option<String>,
    title: &'a str,
    description: &'a str,
    req_type: &'a RequirementType,
    status: &'a RequirementStatus,
    priority: &'a RequirementPriority,
    feature: &'a str,
    archived: bool,
    tags: Vec<&'a str>,
    dependencies: Vec<Uuid>,
    relationships: Vec<CorpusRelationship<'a>>,
    comments: Vec<CorpusComment<'a>>,
}

#[derive(Serialize)]
struct CorpusRelationship<'a> {
    rel_type: &'a RelationshipType,
    target_id: Uuid,
}

#[derive(Serialize)]
struct CorpusComment<'a> {
    id: Uuid,
    author: &'a str,
    content: &'a str,
    parent_id: Option<Uuid>,
    replies: Vec<CorpusComment<'a>>,
}

fn comments(rows: &[Comment]) -> Vec<CorpusComment<'_>> {
    rows.iter()
        .map(|c| CorpusComment {
            id: c.id,
            author: &c.author,
            content: &c.content,
            parent_id: c.parent_id,
            replies: comments(&c.replies),
        })
        .collect()
}

fn project(req: &Requirement) -> Result<CorpusRow<'_>> {
    let mut tags: Vec<_> = req.tags.iter().map(String::as_str).collect();
    tags.sort_unstable();
    let mut dependencies = req.dependencies.clone();
    dependencies.sort_unstable();
    let mut relationships = req
        .relationships
        .iter()
        .map(|r| {
            Ok((
                serde_json::to_string(&r.rel_type)?,
                r.target_id,
                &r.rel_type,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    relationships.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    Ok(CorpusRow {
        id: req.id,
        spec_id: &req.spec_id,
        agreed_id: &req.agreed_id,
        title: &req.title,
        description: &req.description,
        req_type: &req.req_type,
        status: &req.status,
        priority: &req.priority,
        feature: &req.feature,
        archived: req.archived,
        tags,
        dependencies,
        relationships: relationships
            .into_iter()
            .map(|(_, target_id, rel_type)| CorpusRelationship {
                rel_type,
                target_id,
            })
            .collect(),
        comments: comments(&req.comments),
    })
}

fn git(store: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(store)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}

fn head(store: &Path) -> Result<String> {
    Ok(
        String::from_utf8(git(store, &["rev-parse", "--verify", "HEAD^{commit}"])?)?
            .trim()
            .to_owned(),
    )
}

fn dirty(store: &Path) -> Result<bool> {
    Ok(!git(
        store,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?
    .is_empty())
}

/// Batch immutable blob reads to keep a large store export O(1) git processes.
/// A writer thread drains stdin concurrently with stdout (large stores exceed pipe capacity).
fn read_objects(store: &Path, sha: &str) -> Result<Vec<Requirement>> {
    let tree = git(store, &["ls-tree", "-r", "-z", sha, "--", "objects/"])?;
    let mut objects = Vec::new();
    for entry in tree.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let (header, path) = split_byte(entry, b'\t').context("invalid git tree entry")?;
        if !path.ends_with(b".yaml") {
            continue;
        }
        let header = std::str::from_utf8(header)?;
        let parts: Vec<_> = header.split_whitespace().collect();
        anyhow::ensure!(
            parts.len() == 3 && matches!(parts[0], "100644" | "100755") && parts[1] == "blob",
            "corpus object must be a regular YAML blob: {}",
            String::from_utf8_lossy(path)
        );
        objects.push((parts[2].to_owned(), String::from_utf8(path.to_vec())?));
    }
    if objects.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = objects.iter().map(|(oid, _)| format!("{oid}\n")).collect();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(store)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("git batch stdin unavailable")?;
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output();
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("git batch input writer panicked"))??;
    let output = output?;
    anyhow::ensure!(
        output.status.success(),
        "git cat-file failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut remaining = output.stdout.as_slice();
    let mut reqs = Vec::new();
    let mut ids = HashSet::new();
    for (oid, path) in objects {
        let (header, data) = split_byte(remaining, b'\n').context("missing git blob header")?;
        let fields: Vec<_> = std::str::from_utf8(header)?.split_whitespace().collect();
        anyhow::ensure!(
            fields.len() == 3 && fields[0] == oid && fields[1] == "blob",
            "invalid git blob header for {path}"
        );
        let size: usize = fields[2].parse()?;
        anyhow::ensure!(
            size < data.len() && data[size] == b'\n',
            "truncated git blob for {path}"
        );
        let req: Requirement = serde_yaml::from_slice(&data[..size])
            .with_context(|| format!("parse committed object {path}"))?;
        anyhow::ensure!(
            ids.insert(req.id),
            "duplicate corpus UUID {} in {path}",
            req.id
        );
        reqs.push(req);
        remaining = &data[size + 1..];
    }
    anyhow::ensure!(remaining.is_empty(), "unexpected trailing git batch output");
    reqs.sort_by_key(|r| r.id);
    Ok(reqs)
}

fn split_byte(data: &[u8], byte: u8) -> Option<(&[u8], &[u8])> {
    let pos = data.iter().position(|b| *b == byte)?;
    Some((&data[..pos], &data[pos + 1..]))
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn export_corpus(store: &Path, dir: &Path, allow_dirty: bool) -> Result<CorpusManifest> {
    // Validate output before any writes; never mix a corpus with preexisting artifacts.
    if dir.try_exists()? {
        anyhow::ensure!(
            dir.is_dir() && std::fs::read_dir(dir)?.next().transpose()?.is_none(),
            "corpus output must be a new or empty directory: {}",
            dir.display()
        );
    }
    anyhow::ensure!(
        store.join(".git").exists(),
        "regeneration requires a git-canonical store"
    );
    let sha = head(store)?;
    let mut is_dirty = dirty(store)?;
    anyhow::ensure!(allow_dirty || !is_dirty, "store has uncommitted changes; commit them or pass --allow-dirty to export committed HEAD with dirty: true");
    let reqs = read_objects(store, &sha)?;
    // Catch concurrent ordinary store writers before publishing; all blob reads use pinned SHA.
    is_dirty |= dirty(store)?;
    anyhow::ensure!(
        head(store)? == sha,
        "store HEAD changed during corpus export; retry"
    );
    anyhow::ensure!(
        allow_dirty || !is_dirty,
        "store became dirty during corpus export; retry or pass --allow-dirty"
    );
    let bytes = json_bytes(&Corpus {
        format_version: 1,
        rows: reqs.iter().map(project).collect::<Result<_>>()?,
    })?;
    let manifest = CorpusManifest {
        format_version: 1,
        store_sha: sha,
        corpus_file: "corpus.json",
        content_sha256: format!("{:x}", Sha256::digest(&bytes)),
        row_count: reqs.len(),
        dirty: is_dirty,
    };
    let manifest_bytes = json_bytes(&manifest)?;
    std::fs::create_dir_all(dir)?;
    // create_new also refuses concurrent writers and preexisting output symlinks.
    for (name, content) in [("corpus.json", bytes), ("manifest.json", manifest_bytes)] {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(name))?
            .write_all(&content)?;
    }
    Ok(manifest)
}

pub(crate) fn handle_command(store: &Path, cmd: &RegenCommand) -> Result<()> {
    match cmd {
        RegenCommand::Corpus { dir, allow_dirty } => {
            let manifest = export_corpus(store, dir, *allow_dirty)?;
            println!(
                "Exported {} rows to {} (store {}, content {})",
                manifest.row_count,
                dir.display(),
                manifest.store_sha,
                manifest.content_sha256
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::models::{Relationship, RequirementType};
    use tempfile::TempDir;

    fn commit(store: &Path) {
        git(store, &["add", "."]).unwrap();
        git(
            store,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
    }

    fn fixture() -> TempDir {
        let tmp = TempDir::new().unwrap();
        git(tmp.path(), &["init"]).unwrap();
        let mut req = Requirement::new(
            "knowledge α".into(),
            "Keep full prose\n```rust\nexample()\n```".into(),
        );
        req.id = Uuid::from_u128(2);
        req.spec_id = Some("DOC-1".into());
        req.req_type = RequirementType::Doc;
        req.archived = true;
        req.tags = ["zeta".into(), "alpha".into()].into();
        req.dependencies = vec![Uuid::from_u128(3), Uuid::from_u128(1)];
        req.relationships = vec![Relationship {
            rel_type: RelationshipType::References,
            target_id: Uuid::from_u128(1),
            created_at: None,
            created_by: Some("RELATIONSHIP_METADATA_SECRET".into()),
        }];
        req.implementation_summary = Some("IMPLEMENTATION_SECRET".into());
        let mut comment = Comment::new("author".into(), "full comment".into());
        comment.replies.push(Comment::new_reply(
            "replier".into(),
            "full reply".into(),
            comment.id,
        ));
        comment.session_id = Some("SESSION_METADATA_SECRET".into());
        req.comments.push(comment);
        let path = tmp.path().join("objects/DOC/000/DOC-1.yaml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_yaml::to_string(&req).unwrap()).unwrap();
        let mut first = Requirement::new("first UUID".into(), "first".into());
        first.id = Uuid::from_u128(1);
        std::fs::write(
            tmp.path().join("objects/DOC/000/DOC-2.yaml"),
            serde_yaml::to_string(&first).unwrap(),
        )
        .unwrap();
        for path in [
            "src/code.rs",
            "tests/oracle.sh",
            "docs/design.md",
            "attachments/sample.yaml",
            "objects/DOC/000/attachment.png",
            "regen/trials/trial.json",
        ] {
            let path = tmp.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "ARTIFACT_SECRET").unwrap();
        }
        commit(tmp.path());
        tmp
    }

    #[test]
    fn corpus_is_deterministic_across_checkout_paths_and_excludes_artifacts() {
        let store = fixture();
        let out = TempDir::new().unwrap();
        let first = export_corpus(store.path(), &out.path().join("a"), false).unwrap();
        export_corpus(store.path(), &out.path().join("b"), false).unwrap();
        let clone = out.path().join("different-checkout");
        git(
            out.path(),
            &[
                "clone",
                store.path().to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        )
        .unwrap();
        export_corpus(&clone, &out.path().join("c"), false).unwrap();
        for file in ["corpus.json", "manifest.json"] {
            let bytes = std::fs::read(out.path().join("a").join(file)).unwrap();
            assert_eq!(
                bytes,
                std::fs::read(out.path().join("b").join(file)).unwrap()
            );
            assert_eq!(
                bytes,
                std::fs::read(out.path().join("c").join(file)).unwrap()
            );
        }
        assert_eq!(first.store_sha, head(store.path()).unwrap());
        let bytes = std::fs::read(out.path().join("a/corpus.json")).unwrap();
        assert_eq!(
            first.content_sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert_eq!(first.row_count, 2);
        let text = std::str::from_utf8(&bytes).unwrap();
        for excluded in [
            "ARTIFACT_SECRET",
            "IMPLEMENTATION_SECRET",
            "RELATIONSHIP_METADATA_SECRET",
            "SESSION_METADATA_SECRET",
            "created_at",
            "modified_at",
            "filed_at",
            "history",
            ".git",
            store.path().to_str().unwrap(),
        ] {
            assert!(!text.contains(excluded), "leaked {excluded}");
        }
        let corpus: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let row = &corpus["rows"][1];
        assert_eq!(corpus["rows"][0]["title"], "first UUID");
        assert_eq!(row["req_type"], "Doc");
        assert_eq!(row["archived"], true);
        assert_eq!(row["tags"], serde_json::json!(["alpha", "zeta"]));
        assert_eq!(row["dependencies"][0], Uuid::from_u128(1).to_string());
        assert_eq!(row["comments"][0]["content"], "full comment");
        assert_eq!(row["comments"][0]["replies"][0]["content"], "full reply");
        assert!(row["description"].as_str().unwrap().contains("example()"));
        assert_eq!(std::fs::read_dir(out.path().join("a")).unwrap().count(), 2);
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.path().join("a/manifest.json")).unwrap())
                .unwrap();
        assert!(manifest.get("dirty").is_none());
    }

    #[test]
    fn corpus_refuses_dirty_store_unless_explicit_and_still_reads_committed_blobs() {
        let store = fixture();
        let out = TempDir::new().unwrap();
        export_corpus(store.path(), &out.path().join("clean"), false).unwrap();
        // Tracked edits and new malformed objects must never become corpus rows.
        std::fs::write(
            store.path().join("objects/DOC/000/DOC-1.yaml"),
            "invalid dirty YAML",
        )
        .unwrap();
        std::fs::write(
            store.path().join("objects/DOC/000/NEW.yaml"),
            "invalid untracked YAML",
        )
        .unwrap();
        assert!(
            export_corpus(store.path(), &out.path().join("refused"), false)
                .unwrap_err()
                .to_string()
                .contains("uncommitted")
        );
        assert!(!out.path().join("refused").exists());
        let manifest = export_corpus(store.path(), &out.path().join("override"), true).unwrap();
        assert!(manifest.dirty);
        assert_eq!(
            std::fs::read(out.path().join("clean/corpus.json")).unwrap(),
            std::fs::read(out.path().join("override/corpus.json")).unwrap()
        );
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.path().join("override/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(json["dirty"], true);
    }

    #[test]
    fn corpus_refuses_nonempty_output_and_malformed_committed_objects() {
        let store = fixture();
        let out = TempDir::new().unwrap();
        std::fs::write(out.path().join("existing-code"), "keep me").unwrap();
        assert!(export_corpus(store.path(), out.path(), false)
            .unwrap_err()
            .to_string()
            .contains("empty directory"));
        assert_eq!(
            std::fs::read_to_string(out.path().join("existing-code")).unwrap(),
            "keep me"
        );
        std::fs::write(
            store.path().join("objects/DOC/000/DOC-1.yaml"),
            "invalid committed YAML",
        )
        .unwrap();
        commit(store.path());
        assert!(export_corpus(store.path(), &out.path().join("bad"), false)
            .unwrap_err()
            .to_string()
            .contains("parse committed object"));
        assert!(!out.path().join("bad").exists());
    }

    #[test]
    fn corpus_dispatch_reads_git_without_cache_and_handles_a_pipe_sized_batch() {
        let store = TempDir::new().unwrap();
        git(store.path(), &["init"]).unwrap();
        let objects = store.path().join("objects/DOC/000");
        std::fs::create_dir_all(&objects).unwrap();
        let mut req = Requirement::new("batch fixture".into(), "content".into());
        // More than 64 KiB of cat-file requests: a synchronous stdin write
        // before draining stdout would deadlock on common Linux pipe limits.
        for n in 1..=1700 {
            req.id = Uuid::from_u128(n);
            std::fs::write(
                objects.join(format!("DOC-{n}.yaml")),
                serde_yaml::to_string(&req).unwrap(),
            )
            .unwrap();
        }
        commit(store.path());
        let out = TempDir::new().unwrap();
        crate::store_sync::run_on_distributed_store(
            &crate::cli::Command::Regen(RegenCommand::Corpus {
                dir: out.path().to_owned(),
                allow_dirty: false,
            }),
            store.path(),
            None,
        )
        .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.path().join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["row_count"], 1700);
        assert!(!store.path().join(".aida").exists());
        assert!(!store.path().join("cache.db").exists());
    }

    #[test]
    fn corpus_cli_parses_override() {
        use clap::Parser;
        let cli =
            crate::cli::Cli::try_parse_from(["aida", "regen", "corpus", "output", "--allow-dirty"])
                .unwrap();
        assert!(matches!(
            cli.command,
            crate::cli::Command::Regen(RegenCommand::Corpus {
                allow_dirty: true,
                ..
            })
        ));
    }
}
