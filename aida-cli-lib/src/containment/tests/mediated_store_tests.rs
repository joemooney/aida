//! Shared boundary fixtures, without enabling staged production mutation verbs.
// trace:TASK-3223 | ai:codex
use super::*;
use crate::contained_boundary::{contained_store_write_paths_for, StoreWriteProfile};

#[test]
fn mediated_profile_admission_is_closed_before_any_launch_effect() {
    let mut child = Command::new("unused-vendor");
    assert_eq!(
        selected_store_profile(&child).unwrap(),
        StoreWriteProfile::Compatibility
    );
    child.env("AIDA_STORE_MUTATION_REQUIRED", "1");
    assert_eq!(
        selected_store_profile(&child).unwrap(),
        StoreWriteProfile::Compatibility
    );
    for value in ["mediated", "", "unknown"] {
        child.env("AIDA_STORE_MUTATION_PROFILE", value);
        let error = PreparedLaunch::prepare(&child, Path::new("/missing-root"), None)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains(if value == "mediated" {
                "gated"
            } else {
                "invalid explicit"
            }),
            "{error}"
        );
        assert!(require_store_service_surface(&child, false).is_err());
    }
    assert!(
        contained_store_write_paths_for(Path::new("/main"), StoreWriteProfile::Mediated).is_empty()
    );
    assert_eq!(
        contained_store_write_paths_for(Path::new("/main"), StoreWriteProfile::Compatibility).len(),
        2
    );
}

struct Fixture {
    _tmp: tempfile::TempDir,
    main: PathBuf,
    lane: PathBuf,
    state: PathBuf,
    protected: Vec<PathBuf>,
}
impl Fixture {
    fn new(linked: bool) -> Self {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let lane = if linked {
            tmp.path().join("lane")
        } else {
            main.clone()
        };
        let state = tmp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let mut protected = Vec::new();
        for base in [&main, &lane] {
            for name in [".aida-store", ".aida", ".aida/cache-db", ".git"] {
                let path = base.join(name);
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join("sentinel"), "unchanged").unwrap();
                if !protected.contains(&path) {
                    protected.push(path);
                }
            }
        }
        if linked {
            // All three targets lie beneath a deliberately granted ancestor.
            // Protecting only lexical names would fail these cases.
            for name in [".aida-store", ".aida/cache-db"] {
                let path = lane.join(name);
                std::fs::remove_dir_all(&path).unwrap();
                symlink(main.join(name), &path).unwrap();
            }
            let alias = lane.join("store-alias");
            symlink(main.join(".aida-store"), &alias).unwrap();
            protected.push(alias);
            // Authority and cache may each resolve outside their lexical
            // checkout. Cover both, not only an alias to the store itself.
            let cache = main.join(".aida/cache-db");
            let relocated = tmp.path().join("shared-cache");
            std::fs::rename(&cache, &relocated).unwrap();
            symlink(&relocated, &cache).unwrap();
            protected.push(relocated);
            std::fs::remove_dir_all(lane.join(".aida")).unwrap();
            symlink(main.join(".aida"), lane.join(".aida")).unwrap();
        }
        Self {
            _tmp: tmp,
            main,
            lane,
            state,
            protected,
        }
    }
    fn profile(&self) -> (WriteProfile, PathBuf) {
        let mut profile = WriteProfile::new();
        // Deliberately broader than ordinary production grants. Final sealing
        // must survive both ancestor grants and legacy cache/store grants.
        profile.add(self._tmp.path(), true).unwrap();
        for path in crate::contained_boundary::contained_store_write_paths(&self.main) {
            // The compatibility launch resolves these before pinning too.
            profile
                .add(&std::fs::canonicalize(path).unwrap(), true)
                .unwrap();
        }
        let cache = profile
            .seal_mediated_store(&self.lane, &self.main, &self.state)
            .unwrap();
        (profile, cache)
    }
}

#[test]
fn mediated_profile_requires_provisioned_protection_and_disjoint_cache() {
    let f = Fixture::new(false);
    let mut profile = WriteProfile::new();
    assert!(profile
        .seal_mediated_store(&f.lane, &f.main, &f.main.join(".aida"))
        .is_err());
    std::fs::remove_dir_all(f.main.join(".aida-store")).unwrap();
    assert!(profile
        .seal_mediated_store(&f.lane, &f.main, &f.state)
        .is_err());
}

#[test]
fn mediated_profile_pins_linked_gitdir_and_refuses_unreadable_binding() {
    let f = Fixture::new(true);
    let gitfile = f.lane.join(".git");
    std::fs::remove_dir_all(&gitfile).unwrap();
    let gitdir = f._tmp.path().join("linked-gitdir");
    std::fs::create_dir(&gitdir).unwrap();
    std::fs::write(&gitfile, format!("gitdir: {}\n", gitdir.display())).unwrap();
    let (profile, _) = f.profile();
    for path in [&gitfile, &gitdir] {
        assert!(
            !profile
                .mounts
                .iter()
                .rev()
                .find(|m| m.path == *path)
                .unwrap()
                .writable
        );
    }
    std::fs::write(&gitfile, "invalid binding").unwrap();
    assert!(WriteProfile::new()
        .seal_mediated_store(&f.lane, &f.main, &f.state)
        .unwrap_err()
        .to_string()
        .contains("metadata target unavailable"));
}

fn run_in_profile(profile: &WriteProfile, cwd: &Path, script: &str) -> std::process::Output {
    let mut command = Command::new("/usr/bin/bwrap");
    command
        .args(profile.args(cwd))
        .args(["--", "/usr/bin/python3", "-I", "-S", "-c", script]);
    let fds: Vec<_> = profile.mounts.iter().map(|m| m.file.as_raw_fd()).collect();
    unsafe {
        command.pre_exec(move || {
            for fd in &fds {
                libc::fcntl(*fd, libc::F_SETFD, 0);
            }
            Ok(())
        });
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Fixture-only transport carries closed operations from this filesystem
/// profile to the existing transaction primitive; the host reads the result.
/// This grants no live-broker/CLI/MCP/publication or grant-binding coverage.
fn host_primitive_positives(f: &Fixture, profile: &WriteProfile) {
    use aida_core::db::{canonical_mutation::CanonicalOperation, GitBackend};
    let init = Command::new("git")
        .arg("-C")
        .arg(&f.main)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(init.status.success());
    // Explicit temporary HOME authority fixture, never operator enrollment.
    std::fs::write(
        f.main.join(".git/aida-allocation.toml"),
        "version=1\nauthoritative=true\n",
    )
    .unwrap();
    let store = f.main.join(".aida-store");
    let mut row = aida_core::Requirement::new("fixture".into(), "before".into());
    row.spec_id = Some("TASK-3223".into());
    let object = aida_core::object_store::write_object(&store.join("objects"), &row).unwrap();
    let backend = GitBackend::new(&store).unwrap();
    let epoch = uuid::Uuid::new_v4();
    for (op, fields) in [
        (
            "comment_add",
            serde_json::json!({"content":"first\n\tsecond"}),
        ),
        (
            "spec_edit",
            serde_json::json!({"fields":{"description":"edited\n\tdescription"}}),
        ),
    ] {
        let fields = fields.to_string();
        let script = format!(
            r#"
import hashlib,json
from pathlib import Path
data=json.loads({fields:?})
data.update(spec_id='TASK-3223', expected_revision=hashlib.sha256(Path({object:?}).read_bytes()).hexdigest())
print(json.dumps(dict(op={op:?},data=data)))
"#,
            object = object.to_str().unwrap()
        );
        let out = run_in_profile(profile, &f.lane, &script);
        let operation: CanonicalOperation = serde_json::from_slice(&out.stdout).unwrap();
        backend
            .mutate_canonical(
                "filesystem-fixture",
                epoch,
                uuid::Uuid::new_v4(),
                "bound fixture author",
                &operation,
                || Ok(()),
            )
            .unwrap();
    }
    let installed =
        aida_core::object_store::read_object(&store.join("objects"), "TASK-3223").unwrap();
    assert_eq!(installed.comments.len(), 1);
    assert_eq!(installed.comments[0].content, "first\n\tsecond");
    assert_eq!(installed.comments[0].author, "bound fixture author");
    assert_eq!(installed.description, "edited\n\tdescription");
}

#[test]
fn mediated_profile_pins_readonly_targets_and_fresh_private_cache() {
    for linked in [false, true] {
        let f = Fixture::new(linked);
        let (profile, cache) = f.profile();
        assert!(cache.starts_with(&f.state));
        assert!(!cache.exists());
        assert!(std::fs::read_dir(cache.parent().unwrap())
            .unwrap()
            .next()
            .is_none());
        for path in &f.protected {
            let canonical = std::fs::canonicalize(path).unwrap();
            let mount = profile
                .mounts
                .iter()
                .rev()
                .find(|m| m.path == canonical)
                .unwrap();
            assert!(!mount.writable, "{}", path.display());
        }
        // The returned path can be passed straight to the existing cache API;
        // no new ambient selector or shared-cache trust is introduced.
        let db = aida_core::db::Cache::open(&cache).unwrap();
        drop(db);
        assert!(cache.is_file());
        for path in &f.protected {
            assert_eq!(
                std::fs::read_to_string(path.join("sentinel")).unwrap(),
                "unchanged"
            );
        }
    }
}

#[test]
fn mediated_profile_live_raw_create_append_rename_denied_and_private_cache_writable() {
    // This exercises filesystem mounts, independent of the egress broker's
    // seccomp/Landlock ABI admission. Required-live mode still forbids skips.
    if !Path::new("/usr/bin/bwrap").exists() {
        super::tests::skip("mediated store boundary requires bwrap");
        return;
    }
    let probe = Command::new("/usr/bin/bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--", "/bin/true"])
        .output()
        .unwrap();
    if !probe.status.success() {
        super::tests::skip(&format!(
            "mediated store namespaces unavailable: {}",
            String::from_utf8_lossy(&probe.stderr)
        ));
        return;
    }
    for linked in [false, true] {
        let f = Fixture::new(linked);
        let (profile, cache) = f.profile();
        let paths = serde_json::to_string(&f.protected).unwrap();
        let script = format!(
            r#"
import errno, json, os, sqlite3
from pathlib import Path
paths=json.loads({paths:?})
source=Path({lane:?})/'rename-source'
source.write_text('source')
for directory in paths:
    p=Path(directory)
    operations = [lambda: (p/'created').write_text('bad'),
                      lambda: (p/'sentinel').open('a').write('bad'),
                      lambda: os.replace(source,p/'sentinel'),
                      lambda: os.replace(source,p/'created')]
    # Renaming a lane-owned symlink changes only its spelling. The resolved
    # directory mountpoint itself must resist ancestor rename/removal.
    if not p.is_symlink(): operations.append(lambda: os.rename(p, str(p)+'-moved'))
    for operation in operations:
        try:
            operation()
        except OSError as e:
            assert e.errno in (errno.EROFS,errno.EACCES,errno.EXDEV,errno.EBUSY),e
        else: raise AssertionError('raw write allowed: '+str(p))
    assert (p/'sentinel').read_text()=='unchanged'
db=sqlite3.connect({cache:?})
db.execute('pragma journal_mode=wal')
db.execute('create table derived(value text)')
db.execute("insert into derived values ('private')")
db.commit()
assert db.execute('select value from derived').fetchone()==('private',)
db.close()
print('raw writes denied; private cache writable')
"#,
            lane = f.lane.to_str().unwrap(),
            cache = cache.to_str().unwrap()
        );
        let out = run_in_profile(&profile, &f.lane, &script);
        assert!(String::from_utf8_lossy(&out.stdout).contains("raw writes denied"));
        host_primitive_positives(&f, &profile);
        // Parent observations, not assertions made solely by the lane.
        for path in &f.protected {
            assert_eq!(std::fs::read(path.join("sentinel")).unwrap(), b"unchanged");
            assert!(!path.join("created").exists());
            assert!(!PathBuf::from(format!("{}-moved", path.display())).exists());
        }
        assert!(cache.is_file());
        assert_eq!(
            std::fs::read(f.lane.join("rename-source")).unwrap(),
            b"source"
        );
    }
}
