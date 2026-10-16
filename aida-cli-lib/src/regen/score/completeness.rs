//! Completeness evidence is separate from behavior and uses trusted component mappings.
// trace:STORY-1677 | ai:codex
use super::*;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;

const PLACEHOLDERS: [&[u8]; 5] = [
    b"todo: implement",
    b"notimplemented",
    b"notimplementederror",
    b"unimplemented!",
    b"placeholder",
];
pub(super) fn default_min_bytes() -> usize {
    64
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ComponentConfig {
    #[serde(default)]
    checks: Vec<String>,
}

fn check_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.chars().any(char::is_control)
}

pub(super) fn validate_config(config: &RegenConfig) -> Result<()> {
    anyhow::ensure!(config.min_bytes > 0, "regen min_bytes must be positive");
    let mut mapped = HashSet::new();
    for (name, component) in &config.components {
        anyhow::ensure!(
            config.required_tools.contains(name),
            "component must name a required tool: {name}"
        );
        for check in &component.checks {
            anyhow::ensure!(
                check_name(check),
                "component checks must be exact nonempty check names"
            );
            anyhow::ensure!(mapped.insert(check), "check mapped more than once: {check}");
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Component {
    name: String,
    complete: bool,
    reasons: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    format_version: u32,
    min_bytes: usize,
    components: Vec<Component>,
    check_components: BTreeMap<String, String>,
    restricted_score: Option<Aggregate>,
    unmapped_checks: Vec<String>,
    #[serde(skip)]
    fingerprints: BTreeMap<String, String>,
}

// Installer survivors cannot see these post-quarantine paths in their original
// namespaces. Refuse links outside the staged system/prefix; never read host files.
fn inspect_with_hash(
    path: &Path,
    dir: &Path,
    prefix: &Path,
    minimum: usize,
) -> Result<(Vec<String>, String)> {
    let resolved = path
        .canonicalize()
        .context("missing or unresolvable tool")?;
    anyhow::ensure!(
        resolved.starts_with(dir) || resolved.starts_with(prefix),
        "tool resolves outside the private system/prefix"
    );
    let mut hash = Sha256::new();
    hash.update(resolved.as_os_str().as_encoded_bytes());
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(resolved)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "tool is not a regular file");
    hash.update(metadata.permissions().mode().to_le_bytes());
    let mut reasons = Vec::new();
    if metadata.permissions().mode() & 0o111 == 0 {
        reasons.push("not executable".into());
    }
    let mut count = 0usize;
    let mut carry = Vec::new();
    let mut placeholder = false;
    let mut buffer = [0u8; 8192];
    loop {
        let len = file.read(&mut buffer)?;
        if len == 0 {
            break;
        }
        hash.update(&buffer[..len]);
        count = count.saturating_add(
            buffer[..len]
                .iter()
                .filter(|b| !b.is_ascii_whitespace())
                .count(),
        );
        carry.extend(buffer[..len].iter().map(u8::to_ascii_lowercase));
        placeholder |= PLACEHOLDERS
            .iter()
            .any(|p| carry.windows(p.len()).any(|w| w == *p));
        let retain = carry.len().saturating_sub(32);
        carry.drain(..retain);
    }
    if count < minimum {
        reasons.push(format!(
            "too small: {count} non-whitespace bytes, minimum {minimum}"
        ));
    }
    if placeholder {
        reasons.push("placeholder marker".into());
    }
    Ok((reasons, format!("{:x}", hash.finalize())))
}

pub(super) fn check(
    config: &RegenConfig,
    dir: &Path,
    env: &ScoreEnv,
) -> Result<(Snapshot, usize, Vec<roots::Quarantine>)> {
    // All host reads occur at quarantine boundaries, before generated code
    // can address the current replacement roots. Keep prior roots through scoring.
    let mut fingerprints = BTreeMap::new();
    let components: Vec<_> = config
        .required_tools
        .iter()
        .map(|name| {
            let reasons = match inspect_with_hash(
                &env.prefix.join("bin").join(name),
                dir,
                &env.prefix,
                config.min_bytes,
            ) {
                Ok((reasons, hash)) => {
                    fingerprints.insert(name.clone(), hash);
                    reasons
                }
                Err(_) => vec!["missing, unreadable or unsafe tool".into()],
            };
            Component {
                name: name.clone(),
                complete: reasons.is_empty(),
                reasons,
            }
        })
        .collect();
    let mut snapshot = Snapshot {
        format_version: 1,
        min_bytes: config.min_bytes,
        components,
        fingerprints,
        check_components: config
            .components
            .iter()
            .flat_map(|(name, c)| {
                c.checks
                    .iter()
                    .map(move |check| (check.clone(), name.clone()))
            })
            .collect(),
        restricted_score: None,
        unmapped_checks: Vec::new(),
    };
    let mut survivors = 0;
    let mut quarantines = Vec::new();
    for index in 0..snapshot.components.len() {
        if !snapshot.components[index].complete {
            continue;
        }
        let result = env.run(
            dir,
            &[
                env.prefix
                    .join("bin")
                    .join(&snapshot.components[index].name)
                    .to_string_lossy()
                    .into_owned(),
                "--help".into(),
            ],
        );
        if !result
            .is_ok_and(|o| o.status.success() && (!o.stdout.is_empty() || !o.stderr.is_empty()))
        {
            snapshot.components[index].complete = false;
            snapshot.components[index]
                .reasons
                .push("--help failed or produced no output".into());
        }
        survivors += roots::survivors(
            &[dir.to_path_buf(), env.prefix.clone(), env.root.clone()],
            Some(env.guard_channel.fd()),
            true,
        )?;
        quarantines.push(roots::Quarantine::snapshot(&[
            dir.to_path_buf(),
            env.prefix.clone(),
        ])?);
        // Prevent one helper from substituting the executable tested next,
        // including a surrogate that would restore its original after running.
        recheck(&mut snapshot, dir, env);
    }
    Ok((snapshot, survivors, quarantines))
}

// --help is generated code and can mutate installed artifacts. After the caller
// quarantines helper-writable roots, attest the bytes that the oracle will see.
pub(super) fn recheck(snapshot: &mut Snapshot, dir: &Path, env: &ScoreEnv) {
    for component in &mut snapshot.components {
        let reasons = match inspect_with_hash(
            &env.prefix.join("bin").join(&component.name),
            dir,
            &env.prefix,
            snapshot.min_bytes,
        ) {
            Ok((mut reasons, hash)) => {
                if snapshot.fingerprints.get(&component.name) != Some(&hash) {
                    reasons.push("installed tool changed during --help".into());
                }
                reasons
            }
            Err(_) => vec!["missing, unreadable or unsafe tool".into()],
        };
        for reason in reasons {
            if !component.reasons.contains(&reason) {
                component.reasons.push(reason);
            }
        }
        component.complete = component.reasons.is_empty();
    }
}

fn snapshot(record: &TrialRecord) -> Option<Snapshot> {
    let value = record.completeness.as_ref()?;
    let mut snapshot: Snapshot = serde_json::from_value(value.clone()).ok()?;
    if snapshot.format_version != 1 || snapshot.min_bytes == 0 {
        return None;
    }
    let mut names = HashSet::new();
    for component in &snapshot.components {
        if !check_name(&component.name)
            || !names.insert(&component.name)
            || component.complete != component.reasons.is_empty()
        {
            return None;
        }
    }
    if !snapshot
        .check_components
        .iter()
        .all(|(c, n)| check_name(c) && names.contains(n))
    {
        return None;
    }
    let complete: HashSet<_> = snapshot
        .components
        .iter()
        .filter(|c| c.complete)
        .map(|c| &c.name)
        .collect();
    let selected: Vec<_> = record
        .checks
        .iter()
        .filter(|c| {
            snapshot
                .check_components
                .get(&c.name)
                .is_some_and(|name| complete.contains(name))
        })
        .collect();
    snapshot.unmapped_checks = record
        .checks
        .iter()
        .filter(|c| !snapshot.check_components.contains_key(&c.name))
        .map(|c| c.name.clone())
        .collect();
    let total = selected.len();
    let passed = selected.iter().filter(|c| c.passed).count();
    snapshot.restricted_score = (total > 0).then_some(Aggregate {
        passed,
        total,
        pass_rate: passed as f64 / total as f64,
    });
    Some(snapshot)
}

pub(super) fn eligible(record: &TrialRecord) -> bool {
    record.completeness.is_none()
        || snapshot(record).is_some_and(|s| s.components.iter().all(|c| c.complete))
}

pub(super) fn finish(record: &mut TrialRecord) {
    if let Some(data) = snapshot(record) {
        if data.components.iter().any(|c| !c.complete) && record.status == "scored" {
            record.status = "incomplete".into();
            record.clean = false;
            record.flags.push("incomplete".into());
        }
        record.completeness = serde_json::to_value(data).ok();
    }
    if let Some(adapter) = &mut record.adapter {
        finish(&mut adapter.r2);
        record.clean = record.clean && adapter.r2.clean;
    }
}

pub(super) fn render(out: &mut String, record: &TrialRecord) {
    use std::fmt::Write;
    let Some(data) = snapshot(record) else {
        if record.completeness.is_some() {
            out.push_str("\nCompleteness: invalid metadata; restricted score unavailable");
        }
        return;
    };
    let incomplete: Vec<_> = data.components.iter().filter(|c| !c.complete).collect();
    let _ = write!(
        out,
        "\nCompleteness: {} ({}/{} components complete)",
        if incomplete.is_empty() {
            "complete"
        } else {
            "incomplete"
        },
        data.components.len() - incomplete.len(),
        data.components.len()
    );
    for component in incomplete {
        let _ = write!(
            out,
            "\n  {}: {}",
            component.name.escape_debug(),
            component
                .reasons
                .iter()
                .map(|r| r.escape_debug().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if matches!(record.status.as_str(), "scored" | "incomplete") {
        if let Some(a) = record.aggregate.as_ref().filter(|a| {
            a.total > 0 && a.passed <= a.total && a.pass_rate == a.passed as f64 / a.total as f64
        }) {
            let _ = write!(
                out,
                "\nOverall oracle pass rate (diagnostic): {:.1}% ({}/{})",
                a.pass_rate * 100.0,
                a.passed,
                a.total
            );
        }
        match data.restricted_score {
            Some(a) => { let _ = write!(out, "\nComplete-component oracle pass rate: {:.1}% ({}/{})", a.pass_rate*100.0, a.passed, a.total); }
            None if data.check_components.is_empty() => out.push_str("\nComplete-component oracle pass rate: unavailable (no check mapping)"),
            None => out.push_str("\nComplete-component oracle pass rate: unavailable (no observed checks mapped to complete components)"),
        }
    }
    if !data.unmapped_checks.is_empty() {
        let _ = write!(
            out,
            "\nUnmapped checks: {}",
            data.unmapped_checks
                .iter()
                .map(|c| c.escape_debug().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

#[cfg(test)]
fn inspect(path: &Path, dir: &Path, prefix: &Path, minimum: usize) -> Result<Vec<String>> {
    inspect_with_hash(path, dir, prefix, minimum).map(|(reasons, _)| reasons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture(mapping: bool, bad: &str) -> TrialRecord {
        fixture_with_good(mapping, bad, None)
    }

    fn fixture_with_good(mapping: bool, bad: &str, good_override: Option<&str>) -> TrialRecord {
        let source = TempDir::new().unwrap();
        let config = format!("[regen]\noracle_command=['/bin/sh','-c',\"printf 'ok   - good check\\\\nFAIL - bad check\\\\nok   - unmapped\\\\n'\"]\noracle_paths=['oracle.sh']\nrequired_tools=['good','bad']\n{}", if mapping {
            "[regen.components.good]\nchecks=['good check']\n[regen.components.bad]\nchecks=['bad check']\n"
        } else { "" });
        let mut config = RegenConfig::parse(&config).unwrap();
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\"'\"'"));
        let good = "#!/bin/sh\n# This is a complete executable component with a documented help interface.\nif test \"${1-}\" = --help; then printf 'Usage: good\\n'; else printf 'working\\n'; fi\n";
        let good = good_override.unwrap_or(good);
        config.install_command = Some(vec!["/bin/sh".into(), "-c".into(), format!("printf %s {} > \"$PREFIX/bin/good\"; printf %s {} > \"$PREFIX/bin/bad\"; chmod +x \"$PREFIX/bin/good\" \"$PREFIX/bin/bad\"", quote(good), quote(bad))]);
        let mut record = run_trial(
            &config,
            source.path(),
            super::super::tests::manifest(),
            "m",
            "v",
            "h",
            true,
        )
        .unwrap();
        assert_eq!(record.status, "scored", "{:?}", record.error);
        finish(&mut record);
        record
    }

    const COMPLETE: &str = "#!/bin/sh\n# This complete component has enough substantive bytes and a working help interface.\nif test \"${1-}\" = --help; then echo Usage: bad; else echo working; fi\n";

    #[test]
    fn placeholder_is_incomplete_and_split_scores_only_count_complete_mapped_components() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let record = fixture(true, "#!/bin/sh\n# TODO: implement a substantive tool here instead of returning canned usage\necho Usage: bad\n");
        assert_eq!(record.status, "incomplete");
        assert!(!eligible(&record));
        let data = snapshot(&record).unwrap();
        assert!(data.components[0].complete);
        assert!(!data.components[1].complete);
        assert!(data.components[1]
            .reasons
            .iter()
            .any(|r| r == "placeholder marker"));
        let restricted = data.restricted_score.unwrap();
        assert_eq!(
            (restricted.passed, restricted.total, restricted.pass_rate),
            (1, 1, 1.0)
        );
        let overall = record.aggregate.as_ref().unwrap();
        assert_eq!((overall.passed, overall.total), (2, 3));
        assert_eq!(data.unmapped_checks, ["unmapped"]);
        let mut out = String::new();
        render(&mut out, &record);
        assert!(out.contains("Overall oracle pass rate (diagnostic): 66.7% (2/3)"));
        assert!(out.contains("Complete-component oracle pass rate: 100.0% (1/1)"));
        assert!(
            out.contains("bad: placeholder marker") && out.contains("Unmapped checks: unmapped")
        );
    }

    #[test]
    fn complete_fixture_passes_and_missing_mapping_never_fabricates_restricted_score() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let complete = fixture(true, COMPLETE);
        assert_eq!(complete.status, "scored");
        assert!(eligible(&complete));
        let data = snapshot(&complete).unwrap();
        let a = data.restricted_score.unwrap();
        assert_eq!((a.passed, a.total), (1, 2));
        let unmapped = fixture(false, COMPLETE);
        assert_eq!(unmapped.status, "scored");
        assert!(snapshot(&unmapped).unwrap().restricted_score.is_none());
        let mut out = String::new();
        render(&mut out, &unmapped);
        assert!(out.contains("unavailable (no check mapping)"));
        for invalid in [
            "[regen.components.unknown]\nchecks=['x']\n",
            "min_bytes=0\n",
            "[regen.components.good]\nchecks=['duplicate','duplicate']\n",
            "[regen.components.good]\nchecks=['same']\n[regen.components.bad]\nchecks=['same']\n",
        ] {
            let config = format!("[regen]\noracle_command=['oracle.sh']\noracle_paths=['oracle.sh']\nrequired_tools=['good','bad']\n{invalid}");
            assert!(RegenConfig::parse(&config).is_err(), "{config}");
        }
    }

    #[test]
    fn file_inspection_counts_content_and_refuses_stubs_missing_permissions_and_escaping_links() {
        let root = TempDir::new().unwrap();
        let prefix = root.path().join("prefix");
        std::fs::create_dir(&prefix).unwrap();
        let path = prefix.join("tool");
        for marker in [
            "tOdO: ImPlEmEnT",
            "NotImplemented",
            "NotImplementedError",
            "unimplemented!",
            "PLACEHOLDER",
        ] {
            std::fs::write(&path, format!("#!/bin/sh\n{}{}", " ".repeat(8190), marker)).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(inspect(&path, root.path(), &prefix, 1)
                .unwrap()
                .iter()
                .any(|r| r == "placeholder marker"));
        }
        std::fs::write(&path, " \n\t".repeat(100)).unwrap();
        assert!(inspect(&path, root.path(), &prefix, 64)
            .unwrap()
            .iter()
            .any(|r| r.starts_with("too small:")));
        std::fs::write(&path, COMPLETE).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(inspect(&path, root.path(), &prefix, 64)
            .unwrap()
            .iter()
            .any(|r| r == "not executable"));
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/bin/sh", &path).unwrap();
        assert!(inspect(&path, root.path(), &prefix, 64).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(inspect(&path, root.path(), &prefix, 64).is_err());
    }

    #[test]
    fn help_cannot_replace_a_previously_inspected_tool_with_a_stub() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let malicious = "#!/bin/sh\n# This component prints help but replaces another component with a stub.\nprintf '#!/bin/sh\\n# %s%s\\necho usage\\n' place holder > \"$PREFIX/bin/good\"\necho Usage\n";
        let record = fixture(true, malicious);
        assert_eq!(record.status, "incomplete");
        let data = snapshot(&record).unwrap();
        assert!(!data.components[0].complete);
        assert!(data.components[0]
            .reasons
            .iter()
            .any(|r| r == "placeholder marker"));
    }

    #[test]
    fn successful_help_cannot_replace_itself_with_a_full_sized_help_failure() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let malicious = "#!/bin/sh\n# This component replaces itself with a full sized executable that fails help.\nprintf '#!/bin/sh\\n# This full sized replacement contains enough ordinary bytes but fails its help interface.\\nexit 2\\n' > \"$PREFIX/bin/bad\"\necho Usage\n";
        let record = fixture(true, malicious);
        assert_eq!(record.status, "incomplete");
        let data = snapshot(&record).unwrap();
        assert!(data.components[1]
            .reasons
            .iter()
            .any(|r| r == "installed tool changed during --help"));
    }

    #[test]
    fn one_helper_cannot_substitute_a_surrogate_for_the_next_help_check() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let original_bad = "#!/bin/sh\n# Original full sized component fails its help and must remain incomplete.\nexit 2\n";
        let good = r#"#!/bin/sh
# Good temporarily substitutes the next executable with a successful surrogate.
cat > "$PREFIX/bin/bad" <<'PROXY'
#!/bin/sh
# This surrogate would restore the original failing component after help.
printf '#!/bin/sh\n# Original full sized component fails its help and must remain incomplete.\nexit 2\n' > "$PREFIX/bin/bad"
echo Usage
PROXY
chmod +x "$PREFIX/bin/bad"
echo Usage
"#;
        let record = fixture_with_good(true, original_bad, Some(good));
        assert_eq!(record.status, "incomplete");
        let data = snapshot(&record).unwrap();
        assert!(data.components[1]
            .reasons
            .iter()
            .any(|r| r == "installed tool changed during --help"));
    }

    #[test]
    fn help_must_succeed_with_output_and_metadata_cannot_promote_incomplete_trials() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        for script in [
            "#!/bin/sh\n# This executable component is long enough but gives no help whatsoever.\nexit 0\n",
            "#!/bin/sh\n# This executable component prints help and fails, so it is incomplete.\necho Usage; exit 2\n",
        ] {
            let record = fixture(true, script);
            assert_eq!(record.status, "incomplete");
            assert!(snapshot(&record).unwrap().components[1].reasons.iter().any(|r| r == "--help failed or produced no output"));
        }
        let mut corrupt = fixture(true, COMPLETE);
        corrupt.completeness = Some(serde_json::json!({"format_version":999}));
        assert!(!eligible(&corrupt));
    }
}
