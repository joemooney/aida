//! Derived local display cache. Control and reporting gates never consume it.
// trace:TASK-1734 | ai:codex
use super::*;
use std::io::{BufRead, BufReader, Write as IoWrite};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    v: u32,
    head: String,
    build: String,
    latest: Option<RegenerationSummary>,
}
fn build() -> &'static str {
    concat!(
        env!("CARGO_PKG_VERSION"),
        ":",
        env!("AIDA_BUILD_GIT_SHA"),
        ":",
        env!("AIDA_BUILD_UNIX_TIME")
    )
}
fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

// HEAD and refs are Git's local files, including linked worktree commondir and
// packed refs. Unsupported/unreadable layouts omit this optional display.
fn store_head(store: &Path) -> Option<String> {
    let dot = store.join(".git");
    let git_dir = if dot.is_dir() {
        dot
    } else {
        let body = std::fs::read_to_string(dot).ok()?;
        let raw = body.trim().strip_prefix("gitdir:")?.trim();
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            store.join(path)
        }
    };
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(raw) => {
            let path = PathBuf::from(raw.trim());
            if path.is_absolute() {
                path
            } else {
                git_dir.join(path)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
        Err(_) => return None,
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if valid_sha(head) {
        return Some(head.into());
    }
    let reference = head.strip_prefix("ref: ")?.trim();
    if !reference.starts_with("refs/")
        || !Path::new(reference)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    for root in [&git_dir, &common] {
        match std::fs::read_to_string(root.join(reference)) {
            Ok(body) => return valid_sha(body.trim()).then(|| body.trim().to_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    std::fs::read_to_string(common.join("packed-refs"))
        .ok()?
        .lines()
        .find_map(|line| {
            let (sha, name) = line.split_once(' ')?;
            (name == reference && valid_sha(sha)).then(|| sha.to_owned())
        })
}

// This intentionally narrow, file-only lookup belongs ONLY to optional
// display. Do not change the canonical resolver used by mail or authority.
// Unusual Git discovery environments/layouts omit the segment.
pub(super) fn display_store(start: &Path) -> Option<PathBuf> {
    for key in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    ] {
        if std::env::var_os(key).is_some() {
            return None;
        }
    }
    let mut root = start;
    loop {
        if aida_core::store_locate::is_system_temp_dir(root) {
            return None;
        }
        if let Ok(body) = std::fs::read_to_string(root.join(".aida/config.toml")) {
            for relative in aida_core::store_locate::store_path_candidates(&body) {
                let direct = root.join(&relative);
                if direct.is_dir() {
                    return Some(direct);
                }
                // A linked checkout points directly at its worktree metadata;
                // no generic ancestor Git discovery or subprocess fallback.
                let dot = std::fs::read_to_string(root.join(".git")).ok()?;
                let path = PathBuf::from(dot.trim().strip_prefix("gitdir:")?.trim());
                let git_dir = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                let raw = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
                let common = PathBuf::from(raw.trim());
                let common = if common.is_absolute() {
                    common
                } else {
                    git_dir.join(common)
                };
                let common = common.canonicalize().ok()?;
                let store = common.parent()?.join(&relative);
                if store.is_dir() {
                    return Some(store);
                }
            }
        }
        root = root.parent()?;
    }
}

fn valid(candidate: &RegenerationSummary) -> bool {
    valid_rate(candidate.passed, candidate.total, candidate.pass_rate)
}
fn aged(mut candidate: RegenerationSummary, until: DateTime<Utc>) -> RegenerationSummary {
    candidate.pass_rate = candidate.passed as f64 / candidate.total as f64;
    candidate.age_seconds = until
        .signed_duration_since(candidate.completed_at)
        .num_seconds()
        .max(0);
    candidate
}
fn load(path: &Path, head: &str, until: DateTime<Utc>) -> Option<Option<RegenerationSummary>> {
    let mut input = BufReader::new(std::fs::File::open(path).ok()?);
    let mut line = String::new();
    input.read_line(&mut line).ok()?;
    let header: Header = serde_json::from_str(&line).ok()?;
    if header.v != 1 || header.head != head || header.build != build() {
        return None;
    }
    let Some(latest) = header.latest else {
        return Some(None);
    };
    if !valid(&latest) {
        return None;
    }
    // Current statusline/digest reads only this constant-sized first line.
    if latest.completed_at <= until {
        return Some(Some(aged(latest, until)));
    }
    let candidates: Vec<RegenerationSummary> = serde_json::from_reader(input).ok()?;
    if !candidates.iter().all(valid) {
        return None;
    }
    Some(
        candidates
            .into_iter()
            .filter(|r| r.completed_at <= until)
            .max_by_key(|r| (r.completed_at, r.trial_id))
            .map(|r| aged(r, until)),
    )
}
fn save(path: &Path, head: &str, candidates: &[RegenerationSummary]) -> Result<()> {
    let directory = path.parent().context("display cache parent missing")?;
    std::fs::create_dir_all(directory)?;
    let header = Header {
        v: 1,
        head: head.into(),
        build: build().into(),
        latest: candidates
            .iter()
            .max_by_key(|r| (r.completed_at, r.trial_id))
            .cloned(),
    };
    let mut output = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut output, &header)?;
    output.write_all(b"\n")?;
    serde_json::to_writer(&mut output, candidates)?;
    output.write_all(b"\n")?;
    output.persist(path)?;
    Ok(())
}

pub(super) fn read(store: &Path, path: &Path, until: DateTime<Utc>) -> Option<RegenerationSummary> {
    let head = store_head(store)?;
    if let Some(cached) = load(path, &head, until) {
        return cached;
    }
    // Never persist a failed read: transient Git/I/O failures must recover at
    // the same HEAD. Only a successfully read (possibly empty) set is cached.
    let records = controls::read_records_at(store, &head).ok()?;
    let candidates: Vec<_> = records
        .iter()
        .filter_map(|row| summary(std::slice::from_ref(row), row.completed_at))
        .collect();
    let _ = save(path, &head, &candidates);
    candidates
        .into_iter()
        .filter(|r| r.completed_at <= until)
        .max_by_key(|r| (r.completed_at, r.trial_id))
        .map(|r| aged(r, until))
}

pub(super) fn path(store: &Path) -> Option<PathBuf> {
    let store = store.canonicalize().ok()?;
    let mut hash = Sha256::new();
    hash.update(store.as_os_str().as_encoded_bytes());
    hash.update(build());
    Some(
        dirs::cache_dir()?
            .join("aida/regen-display")
            .join(format!("{:x}.jsonl", hash.finalize())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trial(id: u128, time: i64) -> TrialRecord {
        super::super::tests::trial(id, time)
    }

    #[test]
    fn warm_statusline_and_historical_digest_use_no_git_and_misses_batch_once() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let git = |root: &Path, args: &[&str]| super::super::super::super::git(root, args).unwrap();
        git(project.path(), &["init", "-b", "main"]);
        git(project.path(), &["config", "user.name", "Test"]);
        git(
            project.path(),
            &["config", "user.email", "test@example.com"],
        );
        std::fs::create_dir(project.path().join(".aida")).unwrap();
        std::fs::write(
            project.path().join(".aida/config.toml"),
            "[deployment]\nmode='distributed'\nstore_path='.aida-store'\n",
        )
        .unwrap();
        std::fs::write(project.path().join(".gitignore"), ".aida-store/\n").unwrap();
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "project"]);
        let linked = tempfile::tempdir().unwrap();
        git(
            project.path(),
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                linked.path().to_str().unwrap(),
            ],
        );
        let store = project.path().join(".aida-store");
        std::fs::create_dir(&store).unwrap();
        git(&store, &["init", "-b", "aida-store"]);
        git(&store, &["config", "user.name", "Test"]);
        git(&store, &["config", "user.email", "test@example.com"]);
        std::fs::create_dir_all(store.join("regen/trials")).unwrap();
        let mut rows = vec![trial(1, 100), trial(2, 150), trial(3, 250)];
        rows[2].discriminating = Some(Aggregate {
            passed: 1,
            total: 11,
            pass_rate: 1.0 / 11.0,
        });
        for id in 100..228 {
            let mut control = trial(id, 190);
            control.kind = "control-reference".into();
            rows.push(control);
        }
        for row in &rows {
            std::fs::write(
                store.join(format!("regen/trials/{}.json", row.trial_id)),
                serde_json::to_vec(row).unwrap(),
            )
            .unwrap();
        }
        git(&store, &["add", "."]);
        git(&store, &["commit", "-m", "trial batch"]);
        let bin = tempfile::tempdir_in("/var/tmp").unwrap();
        let log = bin.path().join("calls");
        let wrapper = bin.path().join("git");
        let fail = bin.path().join("fail");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in *'cat-file --batch'*) if [ -f '{}' ]; then exit 1; fi;; esac\nexec /usr/bin/git \"$@\"\n",
                log.display(), fail.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(bin.path().to_path_buf()).chain(std::env::split_paths(&old_path)),
        )
        .unwrap();
        let _env = crate::test_env::EnvVarGuard::set("PATH", path.to_str().unwrap());
        let now = DateTime::from_timestamp(200, 0).unwrap();
        let cache = super::path(&store).unwrap();
        let _ = std::fs::remove_file(&cache);
        // A failed cold read cannot poison this HEAD's cache. Recover after
        // restoring Git with neither a commit nor a build change.
        std::fs::write(&fail, "").unwrap();
        assert!(read_summary_for_project(linked.path(), now).is_none());
        assert!(!cache.exists());
        std::fs::remove_file(&fail).unwrap();
        std::fs::write(&log, "").unwrap();
        let selected = read_summary_for_project(linked.path(), now).unwrap();
        assert_eq!(selected.trial_id, Uuid::from_u128(2));
        let calls = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.contains("cat-file --batch"))
                .count(),
            1,
            "{calls}"
        );
        assert!(
            !calls.lines().any(|line| line.contains("cat-file blob")),
            "{calls}"
        );
        std::fs::write(&log, "").unwrap();
        assert_eq!(
            crate::statusline_cmd::regeneration_statusline_segment(linked.path(), now).as_deref(),
            Some("regen 75.0% age 50s")
        );
        let options = crate::digest::DigestOptions {
            since: DateTime::from_timestamp(0, 0).unwrap(),
            until: DateTime::from_timestamp(120, 0).unwrap(),
            audience: crate::digest::DigestAudience::Operator,
            format: crate::digest::DigestFormat::Json,
            include_next: false,
            include_process: false,
            out: None,
        };
        // Digest's other git history reads are unrelated; assert its selection,
        // then separately assert the shared optional reader has zero git calls.
        let output = crate::digest::render_string(
            &options,
            linked.path(),
            &aida_core::RequirementsStore::default(),
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            output["regeneration"]["trial_id"],
            Uuid::from_u128(1).to_string()
        );
        assert_eq!(output["regeneration"]["age_seconds"], 20);
        std::fs::write(&log, "").unwrap();
        assert_eq!(
            read_summary_for_project(linked.path(), options.until)
                .unwrap()
                .trial_id,
            Uuid::from_u128(1)
        );
        assert!(
            read_summary_for_project(linked.path(), DateTime::from_timestamp(90, 0).unwrap())
                .is_none()
        );
        assert_eq!(
            crate::statusline_cmd::regeneration_statusline_segment(linked.path(), now).as_deref(),
            Some("regen 75.0% age 50s")
        );
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "",
            "warm linked-worktree reader spawned git"
        );
        assert_eq!(
            read_summary_for_project(linked.path(), DateTime::from_timestamp(300, 0).unwrap())
                .unwrap()
                .trial_id,
            Uuid::from_u128(3)
        );
        let fractional =
            read_summary_for_project(linked.path(), DateTime::from_timestamp(300, 0).unwrap())
                .unwrap();
        assert_eq!((fractional.passed, fractional.total), (1, 11));
        assert_eq!(fractional.pass_rate, 1.0 / 11.0);
        assert_eq!(fractional.text(), "regen 9.1% age 50s");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
        // Optional display omits unusual discovery contexts; canonical resolver
        // semantics are unaffected because its source is unchanged.
        {
            let _ceiling = crate::test_env::EnvVarGuard::set(
                "GIT_CEILING_DIRECTORIES",
                project.path().to_str().unwrap(),
            );
            assert!(read_summary_for_project(linked.path(), now).is_none());
        }
        // Packing refs must preserve the file-only HEAD key and cache hit.
        git(&store, &["pack-refs", "--all"]);
        std::fs::write(&log, "").unwrap();
        assert_eq!(
            read_summary_for_project(linked.path(), now)
                .unwrap()
                .trial_id,
            Uuid::from_u128(2)
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
        std::fs::remove_file(cache).unwrap();
    }
}

#[cfg(test)]
#[test]
fn display_rate_tolerates_roundtrip_but_rejects_wrong_counts() {
    let parsed: f64 = serde_json::from_str(&serde_json::to_string(&(1.0 / 11.0)).unwrap()).unwrap();
    assert!(valid_rate(1, 11, parsed));
    assert!(!valid_rate(1, 11, 0.1));
    assert!(!valid_rate(2, 1, 2.0));
    assert!(!valid_rate(0, 0, 0.0));
    assert!(!valid_rate(1, 11, f64::NAN));
}
