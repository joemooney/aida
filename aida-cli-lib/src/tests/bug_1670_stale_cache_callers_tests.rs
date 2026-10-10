//! Callers that prune, file, gate or resolve for a write must not act on a
//! stale cache snapshot. A tolerant cache read serves the last committed
//! snapshot whenever it loses the single-flight refresh and its bounded read
//! budget expires, and that snapshot can be arbitrarily old while other
//! processes keep writing.
//!
//! Every fixture is a temporary git-canonical store. The stale snapshot is
//! produced the way it happens in the field: the cache is current, another
//! writer commits straight to the store, and another process holds the refresh
//! flock, so a tolerant read serves the old rows. Each fixture asserts the
//! tolerant read really serves the old value, so a test cannot pass vacuously.
//! trace:TASK-1526 | ai:claude
// trace:BUG-1670 | ai:claude
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aida_core::{
    DatabaseBackend, Relationship, RelationshipType, Requirement, RequirementStatus,
    RequirementType,
};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    store_root: PathBuf,
    cache_path: PathBuf,
    // Holds the refresh flock for as long as the fixture lives, once a test
    // asks for a stale snapshot. trace:TASK-1526 | ai:claude
    foreign: std::cell::RefCell<Option<crate::stale_cache_fixture::ForeignRefresh>>,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let store_root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store_root).unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_root);
        assert!(
            cache_path.starts_with(tmp.path()),
            "fixture: the cache must live inside the temp dir: {}",
            cache_path.display()
        );
        let fx = Fixture {
            tmp,
            store_root,
            cache_path,
            foreign: std::cell::RefCell::new(None),
        };
        fx.write_config("");
        fx
    }

    /// `.aida/config.toml` naming the fixture store, plus `extra`.
    fn write_config(&self, extra: &str) {
        std::fs::write(
            self.root().join(".aida").join("config.toml"),
            format!("store_path = \".aida-store\"\n{extra}"),
        )
        .unwrap();
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn backend(&self) -> aida_core::CachedGitBackend {
        aida_core::CachedGitBackend::open(&self.store_root, &self.cache_path).unwrap()
    }

    fn new_req(spec_id: &str, status: RequirementStatus) -> Requirement {
        let mut req = Requirement::new(format!("spec {spec_id}"), "body".into());
        req.spec_id = Some(spec_id.to_string());
        req.status = status;
        req
    }

    fn add(&self, spec_id: &str, status: RequirementStatus) -> Requirement {
        self.backend()
            .add_requirement(Self::new_req(spec_id, status))
            .unwrap()
    }

    /// Bring the cache to the store HEAD, so later external commits leave it
    /// behind.
    fn freshen_cache(&self) {
        self.backend()
            .list_summaries(&aida_core::ListFilter::default())
            .unwrap();
    }

    fn external(&self) -> aida_core::GitBackend {
        aida_core::GitBackend::new(&self.store_root).unwrap()
    }

    /// Another writer commits an edit straight to the store, bypassing this
    /// cache.
    fn external_edit(&self, spec_id: &str, edit: impl FnOnce(&mut Requirement)) {
        let external = self.external();
        let mut req = external
            .get_requirement_by_spec_id(spec_id)
            .unwrap()
            .unwrap();
        edit(&mut req);
        external.update_requirement(&req).unwrap();
    }

    /// Another writer commits a new spec straight to the store.
    fn external_add(&self, req: Requirement) -> Requirement {
        self.external().add_requirement(req).unwrap()
    }

    /// Make every subsequent read on this thread serve the last committed
    /// (stale) snapshot: another thread takes the refresh flock, so this thread
    /// loses the single flight and falls back to the committed rows once its
    /// bounded read budget expires.
    ///
    /// Before TASK-1526 this planted a `.lock-info` sidecar for a live foreign
    /// PID. That shortcut is gone — the sidecar is a diagnostic and never
    /// authorizes stale serving — so the fixture now holds the refresh flock,
    /// which is the mechanism the shipped read path actually loses to.
    // trace:TASK-1526 | ai:claude
    /// Calling this twice re-arms the same condition, so the previous holder is
    /// released before the new one takes the flock; acquiring first would make
    /// the fixture contend with itself.
    fn hold_foreign_writer(&self) {
        let mut slot = self.foreign.borrow_mut();
        *slot = None;
        *slot = Some(crate::stale_cache_fixture::hold_foreign_refresh(
            &self.cache_path,
        ));
    }

    /// The status the tolerant (stale) cache read serves for `spec_id`.
    fn cached_status(&self, spec_id: &str) -> Option<String> {
        self.backend()
            .list_summaries(&aida_core::ListFilter {
                archive: aida_core::ArchiveFilter::Both,
                defer: aida_core::DeferFilter::Both,
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .find(|s| s.spec_id.as_deref() == Some(spec_id))
            .map(|s| s.status)
    }
}

// ---------------------------------------------------------------- worker gc

fn write_worker_cmd(fx: &Fixture, body: &str) -> PathBuf {
    let path = crate::worker::worker_cmd_path(fx.root());
    std::fs::write(&path, body).unwrap();
    path
}

// trace:BUG-1670 | ai:claude
#[test]
fn worker_gc_keeps_directive_for_reopened_spec() {
    let fx = Fixture::new();
    fx.add("BUG-1", RequirementStatus::Completed);
    fx.add("BUG-2", RequirementStatus::Completed);
    fx.freshen_cache();
    fx.external_edit("BUG-1", |r| r.status = RequirementStatus::Approved);
    fx.hold_foreign_writer();
    assert_eq!(
        fx.cached_status("BUG-1").as_deref(),
        Some("Completed"),
        "fixture: the cache must serve the stale pre-reopen status"
    );
    let path = write_worker_cmd(&fx, "drain BUG-1\ndrain BUG-2\n");

    crate::run_worker_gc(fx.root(), false).unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    assert!(
        body.contains("drain BUG-1"),
        "the reopened spec's directive must survive: {body:?}"
    );
    assert!(
        !body.contains("drain BUG-2"),
        "a truly finished spec's directive is still pruned: {body:?}"
    );
}

// Worker gc re-reads every prune candidate from its stored object even when
// the cache is not behind HEAD: here the object file was rewritten in place
// (no commit), so only the per-target re-read can see the spec is live.
// trace:BUG-1670 | ai:claude
#[test]
fn worker_gc_revalidates_each_pruned_target_on_the_stored_object() {
    let fx = Fixture::new();
    fx.add("BUG-1", RequirementStatus::Completed);
    fx.freshen_cache();
    let objects_root = fx.store_root.join("objects");
    let file = aida_core::object_store::object_path(&objects_root, "BUG-1").unwrap();
    let yaml = std::fs::read_to_string(&file).unwrap();
    let edited = yaml.replace("status: Completed", "status: Approved");
    assert_ne!(yaml, edited, "fixture: the object must carry the status");
    std::fs::write(&file, edited).unwrap();
    assert_eq!(
        fx.cached_status("BUG-1").as_deref(),
        Some("Completed"),
        "fixture: the cache row still says Completed"
    );
    let path = write_worker_cmd(&fx, "drain BUG-1\n");

    crate::run_worker_gc(fx.root(), false).unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    assert!(
        body.contains("drain BUG-1"),
        "a directive whose stored spec is live must survive: {body:?}"
    );
}

// ------------------------------------------------------- completion findings

// trace:BUG-1670 | ai:claude
#[test]
fn completion_findings_not_duplicated_on_stale_snapshot() {
    let fx = Fixture::new();
    fx.add("BUG-1", RequirementStatus::Completed);
    fx.freshen_cache();

    let finding = "Consider a shorter error message";
    let hash_tag = format!(
        "finding-hash:{}",
        &crate::fnv1a_hex(finding.as_bytes())[..8]
    );
    // Another process already carried the finding forward, after this
    // cache's snapshot.
    let mut successor = Fixture::new_req("TASK-9", RequirementStatus::Draft);
    successor.req_type = RequirementType::Task;
    successor.tags.insert("carried-from:BUG-1".to_string());
    successor.tags.insert(hash_tag);
    fx.external_add(successor);
    fx.hold_foreign_writer();
    assert_eq!(
        fx.cached_status("TASK-9"),
        None,
        "fixture: the stale cache must not yet see the filed successor"
    );

    let verdict = crate::review_verdict::verdict_path(fx.root(), "BUG-1");
    std::fs::create_dir_all(verdict.parent().unwrap()).unwrap();
    std::fs::write(
        &verdict,
        serde_json::json!({ "verdict": "APPROVED", "findings": [finding] }).to_string(),
    )
    .unwrap();

    let filed = crate::try_emit_nonblocking_findings_on_completion(
        fx.root(),
        &fx.store_root,
        "BUG-1",
        "",
        None,
    )
    .unwrap();

    assert_eq!(filed, 0, "an already-filed finding must not be filed again");
    let carried = fx
        .external()
        .load()
        .unwrap()
        .requirements
        .into_iter()
        .filter(|r| r.tags.contains("carried-from:BUG-1"))
        .count();
    assert_eq!(carried, 1, "exactly one successor on the store");
}

// ------------------------------------------------------------- focus guard

// trace:BUG-1670 | ai:claude
#[test]
fn focus_guard_uses_fresh_subtree() {
    let fx = Fixture::new();
    fx.write_config("[focus]\nout_of_scope = \"block\"\n");
    let mut epic = Fixture::new_req("EPIC-1", RequirementStatus::Approved);
    epic.req_type = RequirementType::Epic;
    let epic = fx.backend().add_requirement(epic).unwrap();
    let task = fx.add("TASK-2", RequirementStatus::Approved);
    fx.freshen_cache();
    // Another writer parents the task under the epic (parent edges point
    // down at the child).
    fx.external_edit("EPIC-1", |r| {
        r.relationships.push(Relationship {
            rel_type: RelationshipType::Parent,
            target_id: task.id,
            created_at: None,
            created_by: None,
        })
    });
    fx.hold_foreign_writer();
    assert!(
        !fx.backend()
            .descendant_ids(&epic.id)
            .unwrap()
            .contains(&task.id),
        "fixture: the stale cache must not yet see the new child"
    );
    crate::focus::write_focus_marker(fx.root(), "EPIC-1").unwrap();

    let backend = fx.backend();
    let target = backend.get_requirement(&task.id).unwrap().unwrap();
    crate::focus_scope_guard(fx.root(), &backend, &target, false)
        .expect("a child added by another writer is inside the focus");

    // And the guard still blocks a genuinely out-of-scope start.
    let outside = fx.add("TASK-3", RequirementStatus::Approved);
    assert!(crate::focus_scope_guard(fx.root(), &fx.backend(), &outside, false).is_err());
}

// ------------------------------------------------ write-path id resolution

// trace:BUG-1670 | ai:claude
#[test]
fn unambiguous_write_resolution_sees_external_collision_under_foreign_lock() {
    let fx = Fixture::new();
    fx.add("BUG-34", RequirementStatus::Approved);
    fx.freshen_cache();
    // Another writer commits a spec whose agreed id collides with BUG-34.
    let mut colliding = Fixture::new_req("BUG-2-081", RequirementStatus::Approved);
    colliding.agreed_id = Some("BUG-34".into());
    fx.external_add(colliding);
    fx.hold_foreign_writer();

    // The read-only resolver stays tolerant: it answers from the stale
    // snapshot (one candidate). This also proves the fixture is stale.
    let read = fx
        .backend()
        .get_requirement_unambiguous_for_read("BUG-34")
        .expect("the read-only resolver serves the stale snapshot");
    assert!(read.is_some());

    // The write-path resolver refreshes first and refuses the ambiguous id.
    let err = fx
        .backend()
        .get_requirement_unambiguous("BUG-34")
        .expect_err("a write must not pick one of two colliding specs");
    assert!(err
        .downcast_ref::<aida_core::id_collisions::AmbiguousIdError>()
        .is_some());
    // The trait entry point every write path uses routes to the same check.
    fx.hold_foreign_writer();
    assert!(DatabaseBackend::get_requirement_unambiguous(&fx.backend(), "BUG-34").is_err());
}

// --------------------------------------------------------------- rules sync

// trace:BUG-1670 | ai:claude
#[test]
fn rules_sync_keeps_rule_for_spec_approved_after_snapshot() {
    let fx = Fixture::new();
    fx.add("BUG-1", RequirementStatus::Approved);
    fx.freshen_cache();
    // After the snapshot another writer moves the spec into active work, and
    // another checkout already emitted its rule and review fragment.
    fx.external_edit("BUG-1", |r| r.status = RequirementStatus::InProgress);
    let rule = fx
        .root()
        .join(".claude")
        .join("rules")
        .join("aida-specs")
        .join("BUG-1.md");
    let fragment = fx.root().join(".aida").join("review").join("BUG-1.md");
    for f in [&rule, &fragment] {
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, "previously emitted\n").unwrap();
    }
    fx.hold_foreign_writer();
    assert_eq!(
        fx.cached_status("BUG-1").as_deref(),
        Some("Approved"),
        "fixture: the cache must serve the stale pre-start status"
    );

    let mut graph: HashMap<String, Vec<crate::rules_sync::TracedFile>> = HashMap::new();
    graph.insert(
        "BUG-1".to_string(),
        vec![crate::rules_sync::TracedFile {
            path: "src/lib.rs".to_string(),
            symbol: None,
        }],
    );

    let report = crate::rules_sync::sync(fx.root(), &fx.backend(), &graph, false).unwrap();
    assert!(report.removed.is_empty(), "no rule removed: {report:?}");
    assert!(rule.exists(), "the active spec's rule must be kept");

    fx.hold_foreign_writer();
    let review =
        crate::rules_sync::sync_review_md(fx.root(), &fx.backend(), &graph, false).unwrap();
    assert_eq!(review.specs_included, vec!["BUG-1".to_string()]);
    assert!(
        fragment.exists(),
        "the active spec's review fragment must be kept"
    );
}

// ------------------------------------------------------------ session reap

// Session reap decides "spec finished" from the stored object, not from the
// cache rows, so a spec reopened behind a stale cache never marks its lease
// reapable.
// trace:BUG-1670 | ai:claude
#[test]
fn reap_skips_lease_whose_spec_reopened() {
    let fx = Fixture::new();
    fx.add("BUG-1", RequirementStatus::Completed);
    fx.add("BUG-2", RequirementStatus::Completed);
    fx.freshen_cache();
    fx.external_edit("BUG-1", |r| r.status = RequirementStatus::Approved);
    fx.hold_foreign_writer();
    assert_eq!(
        fx.cached_status("BUG-1").as_deref(),
        Some("Completed"),
        "fixture: the cache must serve the stale pre-reopen status"
    );

    let finished = crate::session_reap::finished_scopes(
        fx.root(),
        &["BUG-1".to_string(), "BUG-2".to_string()],
    );
    assert!(
        !finished.contains("BUG-1"),
        "a reopened spec's lease is not finished: {finished:?}"
    );
    assert!(finished.contains("BUG-2"), "{finished:?}");
}

// ---------------------------------------------------------------- lint

/// Tolerant cache reads that may serve a stale snapshot.
const TOLERANT_METHODS: &[&str] = &["list_summaries", "descendant_ids", "id_candidates"];

/// Marker for an explicit allowance. The comment must name the YAML
/// re-validation that makes the tolerant read safe.
const ALLOW_MARKER: &str = "cache-tolerant-read:";

fn lint_name_matches(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["gc", "sweep", "prune", "reap", "emit_findings"]
        .iter()
        .any(|p| n.contains(p))
        || (n.contains("emit") && n.contains("findings"))
}

/// Blank out comments and string/char literal contents (newlines kept), so
/// brace matching and call detection see only code.
fn mask_code(src: &str) -> Vec<char> {
    let c: Vec<char> = src.chars().collect();
    let mut out = c.clone();
    let blank = |out: &mut Vec<char>, from: usize, to: usize| {
        let to = to.min(out.len());
        let from = from.min(to);
        for ch in &mut out[from..to] {
            if *ch != '\n' {
                *ch = ' ';
            }
        }
    };
    let is_ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut i = 0;
    while i < c.len() {
        if c[i] == '/' && c.get(i + 1) == Some(&'/') {
            let end = c[i..]
                .iter()
                .position(|&x| x == '\n')
                .map_or(c.len(), |p| i + p);
            blank(&mut out, i, end);
            i = end;
        } else if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let (mut depth, mut j) = (1, i + 2);
            while j < c.len() && depth > 0 {
                if c[j] == '/' && c.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if c[j] == '*' && c.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank(&mut out, i, j);
            i = j;
        } else if c[i] == 'r'
            && (i == 0
                || !is_ident(c[i - 1])
                || (c[i - 1] == 'b' && (i < 2 || !is_ident(c[i - 2]))))
            && matches!(c.get(i + 1), Some('"') | Some('#'))
        {
            let mut j = i + 1;
            let mut hashes = 0;
            while c.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if c.get(j) != Some(&'"') {
                i += 1;
                continue;
            }
            let start = j + 1;
            let mut k = start;
            'scan: while k < c.len() {
                if c[k] == '"' && (0..hashes).all(|h| c.get(k + 1 + h) == Some(&'#')) {
                    break 'scan;
                }
                k += 1;
            }
            blank(&mut out, start, k);
            i = k + 1 + hashes;
        } else if c[i] == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' {
                if c[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            blank(&mut out, i + 1, j);
            i = j + 1;
        } else if c[i] == '\'' {
            if c.get(i + 1) == Some(&'\\') {
                let end = c[i + 2..]
                    .iter()
                    .position(|&x| x == '\'')
                    .map_or(c.len(), |p| i + 2 + p);
                blank(&mut out, i + 1, end);
                i = end + 1;
            } else if c.get(i + 2) == Some(&'\'') {
                blank(&mut out, i + 1, i + 2);
                i += 3;
            } else {
                i += 1; // a lifetime
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Every `(fn name, body text)` in `src`, bodies taken from the original
/// text (so comments are visible to the allow check).
fn functions(src: &str) -> Vec<(String, String, String)> {
    let orig: Vec<char> = src.chars().collect();
    let masked = mask_code(src);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < masked.len() {
        let is_fn = masked[i] == 'f'
            && masked[i + 1] == 'n'
            && masked[i + 2].is_whitespace()
            && (i == 0 || !(masked[i - 1].is_alphanumeric() || masked[i - 1] == '_'));
        if !is_fn {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        while j < masked.len() && masked[j].is_whitespace() {
            j += 1;
        }
        let name_start = j;
        while j < masked.len() && (masked[j].is_alphanumeric() || masked[j] == '_') {
            j += 1;
        }
        let name: String = masked[name_start..j].iter().collect();
        // Find the body's `{` (or a `;` for a bodiless declaration).
        let mut k = j;
        while k < masked.len() && masked[k] != '{' && masked[k] != ';' {
            k += 1;
        }
        if k >= masked.len() || masked[k] == ';' || name.is_empty() {
            i = j;
            continue;
        }
        let open = k;
        let mut depth = 0i32;
        while k < masked.len() {
            match masked[k] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        let body_orig: String = orig[open..k.min(orig.len())].iter().collect();
        let body_code: String = masked[open..k.min(masked.len())].iter().collect();
        out.push((name, body_orig, body_code));
        // Continue scanning inside the body too, so nested fns are seen.
        i = j;
    }
    out
}

fn tolerant_calls(body_code: &str) -> Vec<&'static str> {
    TOLERANT_METHODS
        .iter()
        .copied()
        .filter(|m| body_code.contains(&format!(".{m}(")))
        .collect()
}

fn allowed(body_orig: &str) -> bool {
    body_orig
        .lines()
        .any(|l| l.contains(ALLOW_MARKER) && l.contains("YAML"))
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Test fixtures read the cache on purpose.
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Offending `(file, fn, method)` triples in `src`.
fn lint_source(file: &str, src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (name, body_orig, body_code) in functions(src) {
        if !lint_name_matches(&name) {
            continue;
        }
        if allowed(&body_orig) {
            continue;
        }
        for m in tolerant_calls(&body_code) {
            hits.push(format!("{file}: fn {name} calls tolerant `{m}`"));
        }
    }
    hits
}

// A sweep/gc/prune/reap/findings-emit function must read the cache strictly
// (a `*_strict` method) or carry an allow comment naming the YAML
// re-validation that makes a tolerant read safe.
// trace:BUG-1670 | ai:claude
#[test]
fn destructive_callers_do_not_use_tolerant_cache_reads() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src_root, &mut files);
    assert!(files.len() > 10, "lint must see the crate sources");
    // The scanner must see real bodies: worker gc's strict read is found.
    let mut lib = std::fs::read_to_string(src_root.join("lib.rs")).unwrap();
    for i in 1..=6 { if let Ok(s) = std::fs::read_to_string(src_root.join(format!("lib_part{i}.rs"))) { lib.push_str(&s); } }
    assert!(
        functions(&lib)
            .iter()
            .any(|(n, _, code)| n == "run_worker_gc" && code.contains(".list_summaries_strict(")),
        "lint scanner failed to parse lib.rs"
    );
    let mut hits = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let rel = f.strip_prefix(&src_root).unwrap().display().to_string();
        hits.extend(lint_source(&rel, &src));
    }
    assert!(
        hits.is_empty(),
        "tolerant cache reads in destructive callers (use a *_strict read or \
         add `// {ALLOW_MARKER} <YAML re-validation>`):\n{}",
        hits.join("\n")
    );
}

// The lint itself: it flags a tolerant read, accepts the strict variant and
// an allow comment naming YAML, and ignores strings and comments.
// trace:BUG-1670 | ai:claude
#[test]
fn lint_flags_tolerant_reads_and_honours_allowances() {
    let bad = "fn run_worker_gc(b: &B) { let s = b.list_summaries(&f); }";
    assert_eq!(lint_source("x.rs", bad).len(), 1);
    let emit = "fn try_emit_nonblocking_findings(b: &B) { b.list_summaries(&f); }";
    assert_eq!(lint_source("x.rs", emit).len(), 1);
    let strict = "fn run_worker_gc(b: &B) { let s = b.list_summaries_strict(&f); }";
    assert!(lint_source("x.rs", strict).is_empty());
    let allow = "fn prune_x(b: &B) {\n    // cache-tolerant-read: each hit re-read from YAML\n    b.descendant_ids(&r);\n}";
    assert!(lint_source("x.rs", allow).is_empty());
    let allow_no_yaml =
        "fn prune_x(b: &B) {\n    // cache-tolerant-read: fine\n    b.descendant_ids(&r);\n}";
    assert_eq!(lint_source("x.rs", allow_no_yaml).len(), 1);
    let in_text =
        "fn reap_x() { let _ = \"b.id_candidates(x)\"; // b.id_candidates(y)\n let c = '{'; }";
    assert!(lint_source("x.rs", in_text).is_empty());
    let unrelated = "fn show(b: &B) { b.list_summaries(&f); }";
    assert!(lint_source("x.rs", unrelated).is_empty());
}
