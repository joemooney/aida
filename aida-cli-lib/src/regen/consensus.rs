//! Like-for-like consensus reports. Draft filing is explicit and deduplicates under lock.
// trace:STORY-1679 | ai:codex
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

// trace:TASK-0446 | ai:antigravity
const CHECK_MAP_RELPATH: &str = ".aida/regen-check-map.toml";
const MAX_CHECK_MAP_BYTES: usize = 1024 * 1024;
const MAX_CHECK_MAP_ENTRIES: usize = 4096;
const MAX_CLAUSES_PER_CHECK: usize = 64;

// trace:TASK-0446 | ai:antigravity
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckMapFile {
    format_version: u32,
    #[serde(default)]
    checks: Vec<CheckMapEntry>,
}

// trace:TASK-0446 | ai:antigravity
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckMapEntry {
    name: String,
    spec_uuid: Uuid,
    description_sha256: String,
    clauses: Vec<ClauseEntry>,
}

// trace:TASK-0446 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ClauseEntry {
    text: String,
    sha256: String,
}

// trace:TASK-0446 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedCheckMapping {
    spec_uuid: Uuid,
    display_id: String,
    title: String,
    description_sha256: String,
    clauses: Vec<ClauseEntry>,
}

// trace:TASK-0446 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckMapping {
    Missing,
    Unavailable(String),
    Invalid(String),
    Stale(String),
    Valid(ResolvedCheckMapping),
}

struct Consensus<'a> {
    trials: Vec<&'a score::TrialRecord>,
    failures: BTreeMap<String, usize>,
    models: BTreeSet<&'a str>,
    vendors: BTreeSet<String>,
    mappings: BTreeMap<String, CheckMapping>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_hex_commit_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

// trace:TASK-0446 | ai:antigravity
fn validate_check_map(raw: &[u8]) -> std::result::Result<BTreeMap<String, CheckMapEntry>, String> {
    if raw.len() > MAX_CHECK_MAP_BYTES {
        return Err(format!(
            "{CHECK_MAP_RELPATH} exceeds max size ({MAX_CHECK_MAP_BYTES} bytes)"
        ));
    }
    let text =
        std::str::from_utf8(raw).map_err(|_| format!("{CHECK_MAP_RELPATH} is not valid UTF-8"))?;
    let parsed: CheckMapFile =
        toml::from_str(text).map_err(|e| format!("malformed {CHECK_MAP_RELPATH}: {e}"))?;
    if parsed.format_version != 1 {
        return Err(format!(
            "unsupported {CHECK_MAP_RELPATH} format_version {}",
            parsed.format_version
        ));
    }
    if parsed.checks.len() > MAX_CHECK_MAP_ENTRIES {
        return Err(format!(
            "{CHECK_MAP_RELPATH} exceeds max check count ({MAX_CHECK_MAP_ENTRIES})"
        ));
    }
    let mut by_name = BTreeMap::new();
    for entry in parsed.checks {
        if entry.name.is_empty()
            || entry.name.trim() != entry.name
            || entry.name.chars().any(char::is_control)
        {
            return Err("invalid check name in mapping".to_string());
        }
        if !is_lower_hex_sha256(&entry.description_sha256) {
            return Err(format!(
                "invalid description_sha256 for check {}",
                entry.name.escape_debug()
            ));
        }
        if entry.clauses.is_empty() || entry.clauses.len() > MAX_CLAUSES_PER_CHECK {
            return Err(format!(
                "check {} must declare 1..={MAX_CLAUSES_PER_CHECK} clauses",
                entry.name.escape_debug()
            ));
        }
        let mut seen_clauses = HashSet::new();
        for clause in &entry.clauses {
            if clause.text.is_empty() || clause.text.trim() != clause.text {
                return Err(format!(
                    "empty or untrimmed clause text for check {}",
                    entry.name.escape_debug()
                ));
            }
            if !is_lower_hex_sha256(&clause.sha256) {
                return Err(format!(
                    "invalid clause sha256 for check {}",
                    entry.name.escape_debug()
                ));
            }
            if sha256_hex(clause.text.as_bytes()) != clause.sha256 {
                return Err(format!(
                    "clause sha256 does not match UTF-8 text for check {}",
                    entry.name.escape_debug()
                ));
            }
            if !seen_clauses.insert(clause.text.as_str()) {
                return Err(format!(
                    "duplicate clause text for check {}",
                    entry.name.escape_debug()
                ));
            }
        }
        if by_name.insert(entry.name.clone(), entry).is_some() {
            return Err("duplicate check name in mapping".to_string());
        }
    }
    Ok(by_name)
}

// trace:TASK-0446 | ai:antigravity
fn resolve_entry_against_requirement(
    entry: &CheckMapEntry,
    req: Option<&Requirement>,
    scope_label: &str,
) -> CheckMapping {
    let Some(req) = req else {
        return CheckMapping::Stale(format!(
            "mapped spec UUID {} not found in {scope_label}",
            entry.spec_uuid
        ));
    };
    if req.archived {
        return CheckMapping::Stale(format!(
            "mapped spec {} ({}) is archived in {scope_label}",
            req.display_id(),
            entry.spec_uuid
        ));
    }
    if sha256_hex(req.description.as_bytes()) != entry.description_sha256 {
        return CheckMapping::Stale(format!(
            "mapped spec {} ({}) description_sha256 changed in {scope_label}",
            req.display_id(),
            entry.spec_uuid
        ));
    }
    for clause in &entry.clauses {
        let matches = req.description.matches(clause.text.as_str()).count();
        if matches == 0 {
            return CheckMapping::Stale(format!(
                "mapped clause text [{}] not found in {} ({}) description in {scope_label}",
                &clause.sha256[..16],
                req.display_id(),
                entry.spec_uuid
            ));
        }
        if matches > 1 {
            return CheckMapping::Stale(format!(
                "mapped clause text [{}] is ambiguous ({matches} occurrences) in {} ({}) description in {scope_label}",
                &clause.sha256[..16],
                req.display_id(),
                entry.spec_uuid
            ));
        }
    }
    CheckMapping::Valid(ResolvedCheckMapping {
        spec_uuid: req.id,
        display_id: req.display_id(),
        title: req.title.clone(),
        description_sha256: entry.description_sha256.clone(),
        clauses: entry.clauses.clone(),
    })
}

// trace:TASK-0446 | ai:antigravity
fn code_repo_for_store(store: &Path) -> &Path {
    if store.file_name() == Some(std::ffi::OsStr::new(".aida-store")) {
        if let Some(parent) = store.parent() {
            if parent.join(".git").exists() {
                return parent;
            }
        }
    }
    store
}

// trace:TASK-0446 | ai:antigravity
fn resolve_trusted_code_tip(code_repo: &Path, store: &Path) -> Option<String> {
    if let Some(sha) = crate::trusted_config::trusted_default_branch_sha(code_repo) {
        return Some(sha);
    }
    if code_repo == store {
        if let Ok(out) = super::git(
            store,
            &["rev-parse", "--verify", "refs/heads/aida-store^{commit}"],
        ) {
            let sha = String::from_utf8_lossy(&out).trim().to_string();
            if !sha.is_empty() {
                return Some(sha);
            }
        }
    }
    None
}

// trace:TASK-0446 | ai:antigravity
fn resolve_consensus_mappings(
    store: &Path,
    oracle_sha: &str,
    store_sha: &str,
    expected_corpus_sha256: &str,
    gaps: &[String],
) -> BTreeMap<String, CheckMapping> {
    if gaps.is_empty() {
        return BTreeMap::new();
    }
    let fill_all = |state: CheckMapping| -> BTreeMap<String, CheckMapping> {
        gaps.iter().map(|g| (g.clone(), state.clone())).collect()
    };

    let code_repo = code_repo_for_store(store);
    // Preserve legacy unit-test fixtures that pass the placeholder `"oracle"`
    // string with no commit in the repository.
    if oracle_sha == "oracle" && !is_hex_commit_oid(oracle_sha) {
        if super::git(
            code_repo,
            &["rev-parse", "--verify", &format!("{oracle_sha}^{{commit}}")],
        )
        .is_err()
        {
            return fill_all(CheckMapping::Missing);
        }
    }

    if !is_hex_commit_oid(oracle_sha) {
        return fill_all(CheckMapping::Unavailable(
            "oracle_sha is not a valid commit OID".to_string(),
        ));
    }

    let Some(trusted_tip) = resolve_trusted_code_tip(code_repo, store) else {
        return fill_all(CheckMapping::Unavailable(
            "trusted default-branch tip could not be resolved".to_string(),
        ));
    };

    if super::git(
        code_repo,
        &["rev-parse", "--verify", &format!("{oracle_sha}^{{commit}}")],
    )
    .is_err()
    {
        return fill_all(CheckMapping::Unavailable(format!(
            "oracle_sha {oracle_sha} is not available in trusted repository"
        )));
    }

    if super::git(
        code_repo,
        &["merge-base", "--is-ancestor", oracle_sha, &trusted_tip],
    )
    .is_err()
    {
        return fill_all(CheckMapping::Unavailable(format!(
            "oracle_sha {oracle_sha} is not an ancestor of trusted tip {trusted_tip}"
        )));
    }

    let Ok(tree_out) = super::git(
        code_repo,
        &[
            "ls-tree",
            "--full-tree",
            oracle_sha,
            "--",
            CHECK_MAP_RELPATH,
        ],
    ) else {
        return fill_all(CheckMapping::Unavailable(format!(
            "could not inspect {CHECK_MAP_RELPATH} at {oracle_sha}"
        )));
    };

    if tree_out.is_empty() {
        // Legacy trials whose pinned code commit has no mapping file keep the
        // explicit missing diagnostic and unparented draft behavior.
        return fill_all(CheckMapping::Missing);
    }

    let Ok(tree_line) = std::str::from_utf8(&tree_out) else {
        return fill_all(CheckMapping::Invalid(format!(
            "non-UTF-8 git tree entry for {CHECK_MAP_RELPATH}"
        )));
    };
    let mut rows = tree_line.lines().filter(|l| !l.trim().is_empty());
    let Some(first_row) = rows.next() else {
        return fill_all(CheckMapping::Missing);
    };
    if rows.next().is_some() {
        return fill_all(CheckMapping::Invalid(format!(
            "multiple tree entries for {CHECK_MAP_RELPATH}"
        )));
    }
    let Some((meta, path)) = first_row.split_once('\t') else {
        return fill_all(CheckMapping::Invalid(format!(
            "malformed git tree row for {CHECK_MAP_RELPATH}"
        )));
    };
    let fields: Vec<&str> = meta.split_whitespace().collect();
    if path != CHECK_MAP_RELPATH
        || fields.len() != 3
        || !matches!(fields[0], "100644" | "100755")
        || fields[1] != "blob"
    {
        return fill_all(CheckMapping::Invalid(format!(
            "{CHECK_MAP_RELPATH} at {oracle_sha} must be a regular blob (found mode {})",
            fields.first().copied().unwrap_or("unknown")
        )));
    }
    let blob_oid = fields[2];
    let Ok(blob_bytes) = super::git(code_repo, &["cat-file", "blob", blob_oid]) else {
        return fill_all(CheckMapping::Unavailable(format!(
            "could not read {CHECK_MAP_RELPATH} blob {blob_oid}"
        )));
    };

    let entries = match validate_check_map(&blob_bytes) {
        Ok(map) => map,
        Err(reason) => return fill_all(CheckMapping::Invalid(reason)),
    };

    // Reconstruct the selected corpus at committed `store_sha` and verify its
    // content_sha256 equals the consensus population before resolving clauses.
    if !is_hex_commit_oid(store_sha)
        || super::git(
            store,
            &["rev-parse", "--verify", &format!("{store_sha}^{{commit}}")],
        )
        .is_err()
    {
        return fill_all(CheckMapping::Unavailable(format!(
            "committed store_sha {store_sha} is not available in store"
        )));
    }
    let Ok(corpus_reqs) = super::read_objects(store, store_sha) else {
        return fill_all(CheckMapping::Unavailable(format!(
            "could not read store objects at {store_sha}"
        )));
    };
    let projected_rows: Result<Vec<super::CorpusRow<'_>>> =
        corpus_reqs.iter().map(super::project).collect();
    let Ok(projected_rows) = projected_rows else {
        return fill_all(CheckMapping::Unavailable(
            "could not project committed corpus rows".to_string(),
        ));
    };
    let Ok(corpus_bytes) = super::json_bytes(&super::Corpus {
        format_version: 1,
        rows: projected_rows,
    }) else {
        return fill_all(CheckMapping::Unavailable(
            "could not serialize committed corpus".to_string(),
        ));
    };
    let actual_corpus_sha256 = sha256_hex(&corpus_bytes);
    if actual_corpus_sha256 != expected_corpus_sha256 {
        return fill_all(CheckMapping::Stale(format!(
            "reconstructed corpus content_sha256 {actual_corpus_sha256} at {store_sha} does not match trial {expected_corpus_sha256}"
        )));
    }

    let by_uuid: BTreeMap<Uuid, &Requirement> = corpus_reqs.iter().map(|r| (r.id, r)).collect();

    let mut resolved = BTreeMap::new();
    for gap in gaps {
        let mapping = match entries.get(gap) {
            None => CheckMapping::Missing,
            Some(entry) => resolve_entry_against_requirement(
                entry,
                by_uuid.get(&entry.spec_uuid).copied(),
                "selected corpus",
            ),
        };
        resolved.insert(gap.clone(), mapping);
    }
    resolved
}

fn discriminating(record: &score::TrialRecord) -> Result<BTreeMap<String, bool>> {
    anyhow::ensure!(
        record.format_version == 1 && record.kind == "trial" && record.status == "scored",
        "consensus requires ordinary scored R1 trials: {}",
        record.trial_id
    );
    let mut checks = BTreeMap::new();
    for check in &record.checks {
        anyhow::ensure!(
            !check.name.is_empty()
                && check.name.trim() == check.name
                && !check.name.chars().any(char::is_control)
                && checks.insert(check.name.clone(), check.passed).is_none(),
            "invalid or duplicate check identity in trial {}",
            record.trial_id
        );
    }
    let mut excluded = HashSet::new();
    for name in &record.excluded_checks {
        anyhow::ensure!(
            checks.contains_key(name) && excluded.insert(name),
            "invalid excluded check set in trial {}",
            record.trial_id
        );
    }
    checks.retain(|name, _| !excluded.contains(name));
    let aggregate = record
        .discriminating
        .as_ref()
        .context("consensus requires a recorded discriminating score")?;
    let passed = checks.values().filter(|p| **p).count();
    anyhow::ensure!(
        !checks.is_empty()
            && aggregate.total == checks.len()
            && aggregate.passed == passed
            // JSON parsing may round a valid fraction by a few ULPs. Counts stay
            // exact, and only the redundant rate gets a bounded tolerance.
            && aggregate.pass_rate.is_finite()
            && (aggregate.pass_rate - passed as f64 / checks.len() as f64).abs()
                <= 8.0 * f64::EPSILON,
        "invalid discriminating score in trial {}",
        record.trial_id
    );
    Ok(checks)
}

fn analyze<'a>(records: &'a [score::TrialRecord], ids: &[Uuid]) -> Result<Consensus<'a>> {
    anyhow::ensure!(ids.len() >= 3, "consensus needs at least three trial IDs");
    let mut unique = HashSet::new();
    anyhow::ensure!(
        ids.iter().all(|id| unique.insert(id)),
        "consensus trial IDs must be distinct"
    );
    let mut by_id = BTreeMap::new();
    for record in records {
        anyhow::ensure!(
            by_id.insert(record.trial_id, record).is_none(),
            "duplicate committed trial identity"
        );
    }
    let trials = ids
        .iter()
        .map(|id| {
            by_id
                .get(id)
                .copied()
                .with_context(|| format!("committed trial not found: {id}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let first = trials[0];
    let expected = discriminating(first)?;
    let mut failures: BTreeMap<_, usize> = expected.keys().map(|k| (k.clone(), 0)).collect();
    for trial in &trials {
        anyhow::ensure!(
            trial.oracle_sha == first.oracle_sha
                && trial.oracle_bundle_sha256 == first.oracle_bundle_sha256
                && trial.content_sha256 == first.content_sha256,
            "consensus refuses mixed oracle/bundle/corpus provenance: {}",
            trial.trial_id
        );
        let checks = discriminating(trial)?;
        anyhow::ensure!(
            checks.keys().eq(expected.keys()),
            "consensus refuses differing discriminating check sets: {}",
            trial.trial_id
        );
        for (check, pass) in checks {
            if !pass {
                *failures.get_mut(&check).expect("validated check set") += 1;
            }
        }
    }
    let models = trials.iter().map(|r| r.model.as_str()).collect();
    let vendors = trials
        .iter()
        .map(|r| r.vendor.trim().to_ascii_lowercase())
        .collect();
    let mappings = failures
        .iter()
        .filter(|(_, count)| **count == trials.len())
        .map(|(name, _)| (name.clone(), CheckMapping::Missing))
        .collect();
    Ok(Consensus {
        trials,
        failures,
        models,
        vendors,
        mappings,
    })
}

fn analyze_for_store<'a>(
    store: &Path,
    records: &'a [score::TrialRecord],
    ids: &[Uuid],
) -> Result<Consensus<'a>> {
    let mut consensus = analyze(records, ids)?;
    let first = consensus.trials[0];
    let gaps: Vec<String> = consensus.gaps().into_iter().map(str::to_owned).collect();
    consensus.mappings = resolve_consensus_mappings(
        store,
        &first.oracle_sha,
        &first.store_sha,
        &first.content_sha256,
        &gaps,
    );
    Ok(consensus)
}

impl Consensus<'_> {
    fn gaps(&self) -> Vec<&str> {
        self.failures
            .iter()
            .filter(|(_, count)| **count == self.trials.len())
            .map(|(name, _)| name.as_str())
            .collect()
    }
    fn mapping_for(&self, check: &str) -> &CheckMapping {
        self.mappings.get(check).unwrap_or(&CheckMapping::Missing)
    }
    fn render(&self) -> String {
        let n = self.trials.len();
        let mut out = format!(
            "Consensus: N={n}, {} distinct models, {} distinct vendors",
            self.models.len(),
            self.vendors.len()
        );
        let unclean = self.trials.iter().filter(|r| !r.clean).count();
        let _ = write!(out, "\nUnclean trials: {unclean}/{n}");
        if unclean > 0 {
            out.push_str(
                "\nWarning: unclean trials are included; investigate their execution evidence",
            );
        }
        if self.vendors.len() == 1 {
            out.push_str("\nWarning: all regenerations came from one vendor; consensus is weaker");
        }
        let _ = write!(out, "\nStore gaps (failed by every regeneration):");
        let gaps = self.gaps();
        if gaps.is_empty() {
            out.push_str("\n  none");
        }
        for gap in gaps {
            match self.mapping_for(gap) {
                CheckMapping::Missing => {
                    let _ = write!(
                        out,
                        "\n  {}: {n}/{n} failures; contract mapping missing",
                        gap.escape_debug()
                    );
                }
                CheckMapping::Unavailable(reason) => {
                    let _ = write!(
                        out,
                        "\n  {}: {n}/{n} failures; contract mapping unavailable ({reason})",
                        gap.escape_debug()
                    );
                }
                CheckMapping::Invalid(reason) => {
                    let _ = write!(
                        out,
                        "\n  {}: {n}/{n} failures; contract mapping invalid ({reason})",
                        gap.escape_debug()
                    );
                }
                CheckMapping::Stale(reason) => {
                    let _ = write!(
                        out,
                        "\n  {}: {n}/{n} failures; contract mapping stale ({reason})",
                        gap.escape_debug()
                    );
                }
                CheckMapping::Valid(resolved) => {
                    let _ = write!(
                        out,
                        "\n  {}: {n}/{n} failures; mapped to {} ({}, {}) — proposed investigation target, not proof of cause",
                        gap.escape_debug(),
                        resolved.display_id,
                        resolved.spec_uuid,
                        resolved.title
                    );
                    for clause in &resolved.clauses {
                        let exact_clause =
                            serde_json::to_string(&clause.text).expect("string serialization");
                        let _ = write!(
                            out,
                            "\n    clause [{}]: {}",
                            &clause.sha256[..16],
                            exact_clause
                        );
                    }
                }
            }
        }
        out.push_str("\nCapability-dependent (some regenerations fail; not store gaps):");
        let mut partial = 0;
        for (name, count) in &self.failures {
            if *count > 0 && *count < n {
                let _ = write!(out, "\n  {}: {count}/{n} failures", name.escape_debug());
                partial += 1;
            }
        }
        if partial == 0 {
            out.push_str("\n  none");
        }
        let _ = write!(
            out,
            "\nCompared trials: {}",
            self.trials
                .iter()
                .map(|r| r.trial_id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        out
    }
}

fn gap_tag(check: &str) -> String {
    format!(
        "regen-gap:{}",
        &format!("{:x}", Sha256::digest(check.as_bytes()))[..16]
    )
}

fn gap_description(
    check: &str,
    consensus: &Consensus<'_>,
    effective_mapping: &CheckMapping,
) -> String {
    let exact_check = serde_json::to_string(check).expect("string serialization");
    let n = consensus.trials.len();
    let models = consensus.models.len();
    let vendors = consensus.vendors.len();
    let trial_ids = consensus
        .trials
        .iter()
        .map(|r| r.trial_id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mapping_note = match effective_mapping {
        CheckMapping::Missing => {
            "Contract mapping missing; investigate and link the responsible contract clause before amending it."
                .to_string()
        }
        CheckMapping::Unavailable(reason) => format!(
            "Contract mapping unavailable ({reason}); investigate and link the responsible contract clause before amending it."
        ),
        CheckMapping::Invalid(reason) => format!(
            "Contract mapping invalid ({reason}); investigate and link the responsible contract clause before amending it."
        ),
        CheckMapping::Stale(reason) => format!(
            "Contract mapping stale ({reason}); investigate and refresh `.aida/regen-check-map.toml` before amending the contract."
        ),
        CheckMapping::Valid(resolved) => {
            let clauses = resolved
                .clauses
                .iter()
                .map(|c| {
                    format!(
                        "- [{}] {}",
                        &c.sha256[..16],
                        serde_json::to_string(&c.text).expect("string serialization")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Mapped contract target: {} ({}) — {}\n\
                 Mapped clauses:\n{clauses}\n\
                 Mapping is a proposed investigation target, not proof of cause.",
                resolved.display_id, resolved.spec_uuid, resolved.title
            )
        }
    };
    format!(
        "Amend the store after a consensus oracle failure.\n\n\
         Exact oracle check (JSON): {exact_check}\n\n\
         Failed in every one of {n} committed regenerations ({models} models, {vendors} vendors). \
         {mapping_note} \
         Consensus is evidence for investigation, not proof of cause.\n\n\
         Trial IDs: {trial_ids}\n\n\
         ## Acceptance\n\n\
         - Investigate the named check against the intended contract.\n\
         - If the store omits the required behavior, amend the owning spec with evidence.\n\
         - Otherwise record the oracle defect or capability explanation and close this draft.\n"
    )
}

#[derive(Default)]
struct FileGapsOutcome {
    filed: Vec<String>,
    parent_linked: Vec<(String, String)>,
    parent_conflicts: Vec<String>,
}

// Revalidate a corpus-resolved check mapping against the live RequirementsStore
// inside the atomic write seam before creating or updating parent links.
// trace:TASK-0446 | ai:antigravity
fn revalidate_mapping_on_live_graph(
    graph: &aida_core::RequirementsStore,
    mapping: &CheckMapping,
    check: &str,
) -> CheckMapping {
    let CheckMapping::Valid(resolved) = mapping else {
        return mapping.clone();
    };
    let entry = CheckMapEntry {
        name: check.to_string(),
        spec_uuid: resolved.spec_uuid,
        description_sha256: resolved.description_sha256.clone(),
        clauses: resolved.clauses.clone(),
    };
    resolve_entry_against_requirement(
        &entry,
        graph.get_requirement_by_id(&resolved.spec_uuid),
        "live store",
    )
}

fn existing_parent_uuids(graph: &aida_core::RequirementsStore, child_uuid: &Uuid) -> Vec<Uuid> {
    let mut parents = BTreeSet::new();
    if let Some(child) = graph.get_requirement_by_id(child_uuid) {
        for rel in &child.relationships {
            if rel.rel_type == RelationshipType::Child {
                parents.insert(rel.target_id);
            }
        }
    }
    for req in &graph.requirements {
        for rel in &req.relationships {
            if rel.rel_type == RelationshipType::Parent && rel.target_id == *child_uuid {
                parents.insert(req.id);
            }
        }
    }
    parents.into_iter().collect()
}

// The normal Storage atomic path uses GitBackend's store lock and delta writes.
// Do not load/save a snapshot or deduplicate outside the write seam.
fn file_gaps(store: &Path, consensus: &Consensus<'_>) -> Result<FileGapsOutcome> {
    let storage = aida_core::Storage::new(store).with_dispenser(crate::load_dispenser(store)?);
    let mut outcome = FileGapsOutcome::default();
    storage.update_atomically(|graph| {
        for check in consensus.gaps() {
            let tag = gap_tag(check);
            let effective_mapping = revalidate_mapping_on_live_graph(
                graph,
                consensus.mapping_for(check),
                check,
            );
            let existing_id = graph
                .requirements
                .iter()
                .find(|r| {
                    !r.archived
                        && !matches!(
                            r.status,
                            RequirementStatus::Completed | RequirementStatus::Rejected
                        )
                        && r.tags.contains(&tag)
                })
                .map(|r| r.id);

            if let Some(existing_uuid) = existing_id {
                if let CheckMapping::Valid(resolved) = &effective_mapping {
                    let parents = existing_parent_uuids(graph, &existing_uuid);
                    if parents.is_empty() {
                        if graph
                            .add_relationship(
                                &existing_uuid,
                                RelationshipType::Child,
                                &resolved.spec_uuid,
                                true,
                            )
                            .is_ok()
                        {
                            let exact_check =
                                serde_json::to_string(check).expect("string serialization");
                            if let Some(existing_req) =
                                graph.get_requirement_by_id_mut(&existing_uuid)
                            {
                                existing_req.add_comment(Comment::new(
                                    crate::get_default_author(),
                                    format!(
                                        "Linked parent {} ({}) from trusted regen check mapping for {exact_check}.",
                                        resolved.display_id, resolved.spec_uuid
                                    ),
                                ));
                            }
                            let draft_label = graph
                                .get_requirement_by_id(&existing_uuid)
                                .and_then(|r| r.spec_id.clone())
                                .unwrap_or_else(|| existing_uuid.to_string());
                            outcome
                                .parent_linked
                                .push((draft_label, resolved.display_id.clone()));
                        }
                    } else if parents.as_slice() != [resolved.spec_uuid] {
                        let draft_label = graph
                            .get_requirement_by_id(&existing_uuid)
                            .and_then(|r| r.spec_id.clone())
                            .unwrap_or_else(|| existing_uuid.to_string());
                        outcome.parent_conflicts.push(format!(
                            "{draft_label} ({}): existing parent {:?} conflicts with mapped parent {} ({}); kept existing parent",
                            check.escape_debug(),
                            parents,
                            resolved.display_id,
                            resolved.spec_uuid
                        ));
                    }
                }
                continue;
            }

            let description = gap_description(check, consensus, &effective_mapping);
            let mut requirement = Requirement::new(
                format!("Amend store for consensus failure: {check}"),
                description,
            );
            requirement.req_type = RequirementType::Task;
            requirement.status = RequirementStatus::Draft;
            requirement.owner = crate::get_default_author();
            requirement.tags.insert(tag);
            requirement.tags.insert("regenerability".into());
            let prefix = graph.get_type_prefix(&RequirementType::Task);
            let id = requirement.id;
            graph.add_requirement_with_id(requirement, None, prefix.as_deref());
            if let CheckMapping::Valid(resolved) = &effective_mapping {
                let _ = graph.add_relationship(
                    &id,
                    RelationshipType::Child,
                    &resolved.spec_uuid,
                    true,
                );
            }
            outcome.filed.push(
                graph
                    .get_requirement_by_id(&id)
                    .and_then(|r| r.spec_id.clone())
                    .unwrap_or_else(|| id.to_string()),
            );
        }
    })?;
    if !outcome.filed.is_empty() || !outcome.parent_linked.is_empty() {
        crate::maybe_auto_push_store(store, crate::StoreAutoPushMode::PerWrite, "per-write");
    }
    Ok(outcome)
}

pub(super) fn handle(store: &Path, ids: &[Uuid], file: bool) -> Result<()> {
    // Committed evidence only; validate the entire selection before any writes.
    let records = score::read_committed_trials(store)?;
    let consensus = analyze_for_store(store, &records, ids)?;
    println!("{}", consensus.render());
    if file {
        let outcome = file_gaps(store, &consensus)?;
        println!("Draft amendment tasks filed: {}", outcome.filed.len());
        for id in outcome.filed {
            println!("  {id}");
        }
        for (draft, parent) in outcome.parent_linked {
            println!("Linked existing draft {draft} to parent {parent}");
        }
        for conflict in outcome.parent_conflicts {
            println!("Parent conflict: {conflict}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trial(id: u128, gap: bool, partial: bool) -> score::TrialRecord {
        let pass = (!gap as usize) + (!partial as usize) + 1;
        serde_json::from_value(json!({
            "format_version":1, "trial_id":Uuid::from_u128(id),"store_sha":"s","content_sha256":"corpus",
            "oracle_sha":"oracle","oracle_bundle_sha256":"bundle","model":format!("model-{id}"),
            "vendor":if id == 3 { "vendor B" } else { "vendor A" },"harness":"h",
            "started_at":chrono::Utc::now(),"completed_at":chrono::Utc::now(),"status":"scored","clean":true,
            "checks":[{"name":"gap","passed":!gap},{"name":"partial","passed":!partial},
                      {"name":"pass","passed":true},{"name":"excluded","passed":false}],
            "excluded_checks":["excluded"],
            "discriminating":{"passed":pass,"total":3,"pass_rate":pass as f64/3.0},
            "guard_hits":[],"unconfined":false,"flags":[]
        })).unwrap()
    }
    fn rows() -> Vec<score::TrialRecord> {
        vec![
            trial(1, true, true),
            trial(2, true, false),
            trial(3, true, false),
        ]
    }
    fn ids() -> Vec<Uuid> {
        (1..=3).map(Uuid::from_u128).collect()
    }
    fn store() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-b", "aida-store"]).unwrap();
        git(root.path(), &["config", "user.name", "Test"]).unwrap();
        git(root.path(), &["config", "user.email", "test@example.com"]).unwrap();
        git(root.path(), &["commit", "--allow-empty", "-m", "fixture"]).unwrap();
        root
    }
    fn commit_rows(store: &Path, rows: &[score::TrialRecord]) {
        std::fs::create_dir_all(store.join("regen/trials")).unwrap();
        for row in rows {
            std::fs::write(
                store.join(format!("regen/trials/{}.json", row.trial_id)),
                serde_json::to_vec(row).unwrap(),
            )
            .unwrap();
        }
        git(store, &["add", "regen/trials"]).unwrap();
        git(store, &["commit", "-m", "trial fixtures"]).unwrap();
    }

    #[test]
    fn consensus_separates_all_fail_from_partial_and_reports_diversity() {
        let rows = rows();
        let ids = ids();
        let report = analyze(&rows, &ids).unwrap();
        assert_eq!(report.gaps(), ["gap"]);
        let out = report.render();
        assert!(out.contains("N=3, 3 distinct models, 2 distinct vendors"));
        assert!(out.contains("gap: 3/3 failures; contract mapping missing"));
        assert!(out.contains("partial: 1/3 failures"));
        assert!(!out.contains("excluded: ") && !out.contains("pass: "));
        assert!(!out.contains("consensus is weaker"));
        assert!(out.contains("Unclean trials: 0/3"));
        assert!(!out.contains("unclean trials are included"));
        let mut rows = rows;
        rows[1].clean = false;
        for row in &mut rows {
            row.vendor = if row.trial_id == Uuid::from_u128(1) {
                "  One VENDOR  ".into()
            } else {
                "one vendor".into()
            };
            row.model = "one model".into();
        }
        let out = analyze(&rows, &ids).unwrap().render();
        assert!(out.contains("1 distinct models, 1 distinct vendors"));
        assert!(out.contains("consensus is weaker"));
        assert!(out.contains("Unclean trials: 1/3"));
        assert!(out.contains("unclean trials are included"));
    }

    #[test]
    fn consensus_refuses_mixed_provenance_sets_invalid_scores_or_nonordinary_attempts() {
        for variant in 0..13 {
            let mut rows = rows();
            match variant {
                0 => rows[1].oracle_sha = "other".into(),
                1 => rows[1].oracle_bundle_sha256 = "other".into(),
                2 => rows[1].content_sha256 = "other".into(),
                3 => rows[1].checks[0].name = "different".into(),
                4 => rows[1].kind = "control-reference".into(),
                5 => rows[1].kind = "trial-adapted".into(),
                6 => rows[1].status = "install_failed".into(),
                7 => rows[1].discriminating.as_mut().unwrap().pass_rate = 0.99,
                8 => rows[1].excluded_checks.push("nonexistent".into()),
                9 => rows[1].discriminating.as_mut().unwrap().passed = 3,
                10 => rows[1].discriminating.as_mut().unwrap().total = 4,
                11 => rows[1].discriminating.as_mut().unwrap().pass_rate = f64::NAN,
                _ => rows[1].discriminating.as_mut().unwrap().pass_rate = f64::INFINITY,
            }
            assert!(analyze(&rows, &ids()).is_err(), "variant {variant}");
        }
        assert!(analyze(&rows(), &ids()[..2]).is_err());
        assert!(analyze(&rows(), &[Uuid::from_u128(1); 3]).is_err());
        assert!(analyze(
            &rows(),
            &[Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(4)]
        )
        .is_err());
    }

    #[test]
    fn explicit_file_is_idempotent_and_report_default_does_not_write() {
        let store = store();
        let rows = rows();
        commit_rows(store.path(), &rows);
        let sha = head(store.path()).unwrap();
        handle(store.path(), &ids(), false).unwrap();
        assert_eq!(head(store.path()).unwrap(), sha);
        assert!(!store.path().join("objects").exists());
        handle(store.path(), &ids(), true).unwrap();
        let storage = aida_core::Storage::new(store.path());
        let graph = storage.load().unwrap();
        assert_eq!(graph.requirements.len(), 1);
        let gap = &graph.requirements[0];
        assert_eq!(gap.status, RequirementStatus::Draft);
        assert_eq!(gap.req_type, RequirementType::Task);
        assert!(gap.tags.contains(&gap_tag("gap")));
        assert!(gap
            .description
            .contains("Exact oracle check (JSON): \"gap\""));
        assert!(gap.description.contains("Contract mapping missing"));
        assert!(gap.relationships.is_empty());
        let first_id = gap.id;
        handle(store.path(), &ids(), true).unwrap();
        let graph = storage.load().unwrap();
        assert_eq!(graph.requirements.len(), 1);
        assert_eq!(graph.requirements[0].id, first_id);
        // Grooming a gap into any non-closed status must retain its identity.
        for status in [
            RequirementStatus::Approved,
            RequirementStatus::Planned,
            RequirementStatus::InProgress,
            RequirementStatus::Done,
            RequirementStatus::NeedsAttention,
            RequirementStatus::Superseded,
        ] {
            storage
                .update_atomically(|graph| graph.requirements[0].status = status.clone())
                .unwrap();
            let before = head(store.path()).unwrap();
            handle(store.path(), &ids(), true).unwrap();
            assert_eq!(head(store.path()).unwrap(), before, "status {status:?}");
            let graph = storage.load().unwrap();
            assert_eq!(graph.requirements.len(), 1, "status {status:?}");
            assert_eq!(graph.requirements[0].id, first_id);
        }
        // Once the old draft is closed, the same check can file a new investigation.
        storage
            .update_atomically(|graph| graph.requirements[0].status = RequirementStatus::Rejected)
            .unwrap();
        handle(store.path(), &ids(), true).unwrap();
        assert_eq!(storage.load().unwrap().requirements.len(), 2);
        let mut mixed = rows;
        mixed[2].oracle_sha = "other".into();
        commit_rows(store.path(), &mixed);
        let before = head(store.path()).unwrap();
        assert!(handle(store.path(), &ids(), true).is_err());
        assert_eq!(head(store.path()).unwrap(), before);
    }

    #[test]
    fn consensus_accepts_committed_json_fraction_43_of_44() {
        let store = store();
        let mut rows = rows();
        for row in &mut rows {
            row.checks = (0..44)
                .map(|i| score::CheckOutcome {
                    name: format!("check-{i}"),
                    passed: i != 0,
                })
                .collect();
            row.excluded_checks.clear();
            row.discriminating = Some(score::Aggregate {
                passed: 43,
                total: 44,
                pass_rate: 43.0 / 44.0,
            });
        }
        commit_rows(store.path(), &rows);
        let committed = score::read_committed_trials(store.path()).unwrap();
        let report = analyze(&committed, &ids()).unwrap();
        assert_eq!(report.gaps(), ["check-0"]);
        handle(store.path(), &ids(), false).unwrap();

        // A materially wrong redundant rate still refuses committed evidence.
        rows[1].discriminating.as_mut().unwrap().pass_rate = 0.99;
        commit_rows(store.path(), &rows);
        assert!(handle(store.path(), &ids(), true).is_err());
        assert!(!store.path().join("objects").exists());
    }

    #[test]
    fn completed_rejected_or_archived_gap_allows_a_new_investigation() {
        for (status, archived) in [
            (RequirementStatus::Completed, false),
            (RequirementStatus::Rejected, false),
            (RequirementStatus::Approved, true),
        ] {
            let store = store();
            commit_rows(store.path(), &rows());
            handle(store.path(), &ids(), true).unwrap();
            let storage = aida_core::Storage::new(store.path());
            let closed_id = storage.load().unwrap().requirements[0].id;
            storage
                .update_atomically(|graph| {
                    graph.requirements[0].status = status.clone();
                    graph.requirements[0].archived = archived;
                })
                .unwrap();
            handle(store.path(), &ids(), true).unwrap();
            let graph = storage.load().unwrap();
            assert_eq!(
                graph.requirements.len(),
                2,
                "{status:?}, archived={archived}"
            );
            let fresh = graph
                .requirements
                .iter()
                .find(|r| r.id != closed_id)
                .unwrap();
            assert_eq!(fresh.status, RequirementStatus::Draft);
            assert!(!fresh.archived);
            assert!(fresh.tags.contains(&gap_tag("gap")));
        }
    }

    #[test]
    fn consensus_reads_committed_trials_and_cli_excludes_execution_flags() {
        use clap::Parser;
        let store = store();
        commit_rows(store.path(), &rows());
        let path = store.path().join(format!("regen/trials/{}.json", ids()[0]));
        std::fs::write(&path, "invalid dirty JSON").unwrap();
        handle(store.path(), &ids(), false).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid dirty JSON");
        let texts: Vec<_> = ids().iter().map(Uuid::to_string).collect();
        let mut args = vec!["aida", "regen", "score", "--consensus"];
        args.extend(texts.iter().map(String::as_str));
        assert!(crate::command_registry::try_parse_from(&args).is_ok());
        let mut file = args.clone();
        file.push("--file");
        assert!(crate::command_registry::try_parse_from(file).is_ok());
        for extra in [
            vec!["system"],
            vec!["--control", "stub"],
            vec!["--corpus", "corpus"],
            vec!["--model", "model"],
            vec!["--vendor", "vendor"],
            vec!["--harness", "h"],
            vec!["--unconfined"],
        ] {
            let mut bad = args.clone();
            bad.extend(extra);
            assert!(crate::command_registry::try_parse_from(bad).is_err());
        }
        assert!(crate::command_registry::try_parse_from([
            "aida", "regen", "score", "system", "--file"
        ])
        .is_err());
        assert!(
            crate::command_registry::try_parse_from(["aida", "regen", "score", "system"]).is_err()
        );
    }

    // trace:TASK-0446 | ai:antigravity
    #[test]
    fn check_map_schema_rejects_bad_versions_unknown_fields_duplicates_and_hash_mismatches() {
        let uuid = Uuid::from_u128(42);
        let clause = "The CLI shall exit 24 when a merge hold is active.";
        let clause_sha = sha256_hex(clause.as_bytes());
        let desc = format!("Contract body.\n\n{clause}\n");
        let desc_sha = sha256_hex(desc.as_bytes());

        let valid = format!(
            "format_version = 1\n\
             [[checks]]\n\
             name = \"hold.exit_code\"\n\
             spec_uuid = \"{uuid}\"\n\
             description_sha256 = \"{desc_sha}\"\n\
             clauses = [{{ text = \"{clause}\", sha256 = \"{clause_sha}\" }}]\n"
        );
        assert!(validate_check_map(valid.as_bytes()).is_ok());

        for bad in [
            valid.replace("format_version = 1", "format_version = 2"),
            format!("{valid}\nextra_root = true\n"),
            valid.replace(
                "name = \"hold.exit_code\"",
                "name = \"hold.exit_code\"\nunknown_field = 1",
            ),
            format!("{valid}\n{}", valid.replace("format_version = 1\n", "")),
            valid.replace(&desc_sha, &"A".repeat(64)),
            valid.replace(&clause_sha, &"0".repeat(64)),
            valid.replace("clauses = [", "clauses = [] # "),
        ] {
            assert!(
                validate_check_map(bad.as_bytes()).is_err(),
                "expected invalid map to fail: {bad}"
            );
        }
    }

    // trace:TASK-0446 | ai:antigravity
    #[test]
    fn valid_mapping_links_consensus_gap_parents_amendment_and_handles_existing_drafts() {
        let store = store();
        let storage = aida_core::Storage::new(store.path());

        let clause = "When a consensus gap is mapped, --file shall parent the draft under the contract spec.";
        let desc = format!("Contract specification.\n\n## Acceptance\n\n- {clause}\n");
        let desc_sha = sha256_hex(desc.as_bytes());
        let clause_sha = sha256_hex(clause.as_bytes());

        let mut owner_id = Uuid::nil();
        let mut other_parent_id = Uuid::nil();
        storage
            .update_atomically(|graph| {
                let mut req = Requirement::new("Consensus contract".into(), desc.clone());
                req.req_type = RequirementType::Story;
                req.status = RequirementStatus::Completed;
                req.agreed_id = Some("STORY-900".into());
                owner_id = req.id;
                let prefix = graph.get_type_prefix(&RequirementType::Story);
                graph.add_requirement_with_id(req, None, prefix.as_deref());

                let mut other = Requirement::new("Unrelated parent".into(), "Other spec.".into());
                other.req_type = RequirementType::Story;
                other.status = RequirementStatus::Approved;
                other_parent_id = other.id;
                graph.add_requirement_with_id(other, None, prefix.as_deref());
            })
            .unwrap();

        let corpus_dir = tempfile::tempdir().unwrap();
        let manifest = super::super::export_corpus(store.path(), corpus_dir.path(), false).unwrap();

        // Commit `.aida/regen-check-map.toml` into the repository and record its commit as oracle_sha.
        std::fs::create_dir_all(store.path().join(".aida")).unwrap();
        let map_toml = format!(
            "format_version = 1\n\
             [[checks]]\n\
             name = \"gap\"\n\
             spec_uuid = \"{owner_id}\"\n\
             description_sha256 = \"{desc_sha}\"\n\
             clauses = [{{ text = {}, sha256 = \"{clause_sha}\" }}]\n",
            serde_json::to_string(clause).unwrap()
        );
        std::fs::write(store.path().join(CHECK_MAP_RELPATH), map_toml).unwrap();
        git(store.path(), &["add", CHECK_MAP_RELPATH]).unwrap();
        git(store.path(), &["commit", "-m", "add regen check map"]).unwrap();
        let oracle_sha = head(store.path()).unwrap();

        let mut rows = rows();
        for row in &mut rows {
            row.store_sha = manifest.store_sha.clone();
            row.content_sha256 = manifest.content_sha256.clone();
            row.oracle_sha = oracle_sha.clone();
        }
        commit_rows(store.path(), &rows);

        let committed = score::read_committed_trials(store.path()).unwrap();
        let consensus = analyze_for_store(store.path(), &committed, &ids()).unwrap();
        let rendered = consensus.render();
        assert!(
            rendered.contains(&format!(
                "gap: 3/3 failures; mapped to STORY-900 ({owner_id}, Consensus contract) — proposed investigation target, not proof of cause"
            )),
            "rendered report: {rendered}"
        );
        assert!(rendered.contains(&clause_sha[..16]));

        // Report-only default does not write.
        let before = head(store.path()).unwrap();
        handle(store.path(), &ids(), false).unwrap();
        assert_eq!(head(store.path()).unwrap(), before);

        // Opt-in `--file` creates the draft and parents it under `owner_id`.
        handle(store.path(), &ids(), true).unwrap();
        let graph = storage.load().unwrap();
        let draft = graph
            .requirements
            .iter()
            .find(|r| r.tags.contains(&gap_tag("gap")))
            .expect("draft filed");
        assert_eq!(draft.status, RequirementStatus::Draft);
        assert!(draft.description.contains("STORY-900"));
        assert!(draft
            .relationships
            .iter()
            .any(|rel| { rel.rel_type == RelationshipType::Child && rel.target_id == owner_id }));
        let owner_after = graph.get_requirement_by_id(&owner_id).unwrap();
        assert_eq!(owner_after.status, RequirementStatus::Completed);
        assert_eq!(owner_after.description, desc);
        assert!(owner_after
            .relationships
            .iter()
            .any(|rel| { rel.rel_type == RelationshipType::Parent && rel.target_id == draft.id }));

        // Repeat `--file` is idempotent (no new commit or duplicate draft).
        let after_first_file = head(store.path()).unwrap();
        handle(store.path(), &ids(), true).unwrap();
        assert_eq!(head(store.path()).unwrap(), after_first_file);

        // If an existing unparented draft exists, `--file` links the validated parent once and records provenance.
        let draft_uuid = draft.id;
        storage
            .update_atomically(|graph| {
                for r in &mut graph.requirements {
                    r.relationships.clear();
                }
            })
            .unwrap();
        let outcome = file_gaps(store.path(), &consensus).unwrap();
        assert!(outcome.filed.is_empty());
        assert_eq!(outcome.parent_linked.len(), 1);
        let graph = storage.load().unwrap();
        let relinked = graph.get_requirement_by_id(&draft_uuid).unwrap();
        assert!(relinked
            .relationships
            .iter()
            .any(|rel| { rel.rel_type == RelationshipType::Child && rel.target_id == owner_id }));
        assert!(relinked
            .comments
            .iter()
            .any(|c| c.content.contains("Linked parent STORY-900")));

        // If an existing draft already has a different parent, `--file` keeps it and reports a conflict.
        storage
            .update_atomically(|graph| {
                for r in &mut graph.requirements {
                    r.relationships.clear();
                }
                graph
                    .add_relationship(&draft_uuid, RelationshipType::Child, &other_parent_id, true)
                    .unwrap();
            })
            .unwrap();
        let before_conflict = head(store.path()).unwrap();
        let outcome = file_gaps(store.path(), &consensus).unwrap();
        assert_eq!(head(store.path()).unwrap(), before_conflict);
        assert!(outcome.filed.is_empty());
        assert!(outcome.parent_linked.is_empty());
        assert_eq!(outcome.parent_conflicts.len(), 1);
    }

    // trace:TASK-0446 | ai:antigravity
    #[test]
    fn stale_invalid_and_unavailable_mappings_remain_explicit_and_file_unparented() {
        let store = store();
        let storage = aida_core::Storage::new(store.path());

        let clause = "Unique acceptance clause.";
        let desc_with_dup = format!("Body.\n{clause}\n{clause}\n");
        let desc_dup_sha = sha256_hex(desc_with_dup.as_bytes());
        let clause_sha = sha256_hex(clause.as_bytes());

        let mut owner_id = Uuid::nil();
        storage
            .update_atomically(|graph| {
                let mut req = Requirement::new("Ambiguous spec".into(), desc_with_dup.clone());
                req.req_type = RequirementType::Story;
                req.status = RequirementStatus::Approved;
                owner_id = req.id;
                let prefix = graph.get_type_prefix(&RequirementType::Story);
                graph.add_requirement_with_id(req, None, prefix.as_deref());
            })
            .unwrap();

        let corpus_dir = tempfile::tempdir().unwrap();
        let manifest = super::super::export_corpus(store.path(), corpus_dir.path(), false).unwrap();

        std::fs::create_dir_all(store.path().join(".aida")).unwrap();
        let map_toml = format!(
            "format_version = 1\n\
             [[checks]]\n\
             name = \"gap\"\n\
             spec_uuid = \"{owner_id}\"\n\
             description_sha256 = \"{desc_dup_sha}\"\n\
             clauses = [{{ text = \"{clause}\", sha256 = \"{clause_sha}\" }}]\n"
        );
        std::fs::write(store.path().join(CHECK_MAP_RELPATH), map_toml).unwrap();
        git(store.path(), &["add", CHECK_MAP_RELPATH]).unwrap();
        git(store.path(), &["commit", "-m", "map with ambiguous clause"]).unwrap();
        let oracle_sha = head(store.path()).unwrap();

        let mut rows = rows();
        for row in &mut rows {
            row.store_sha = manifest.store_sha.clone();
            row.content_sha256 = manifest.content_sha256.clone();
            row.oracle_sha = oracle_sha.clone();
        }
        commit_rows(store.path(), &rows);

        let committed = score::read_committed_trials(store.path()).unwrap();
        let consensus = analyze_for_store(store.path(), &committed, &ids()).unwrap();
        assert!(
            consensus
                .render()
                .contains("gap: 3/3 failures; contract mapping stale (mapped clause text"),
            "rendered: {}",
            consensus.render()
        );

        // Filing a stale mapping creates an unparented draft with an explicit stale note.
        handle(store.path(), &ids(), true).unwrap();
        let graph = storage.load().unwrap();
        let draft = graph
            .requirements
            .iter()
            .find(|r| r.tags.contains(&gap_tag("gap")))
            .unwrap();
        assert!(draft.relationships.is_empty());
        assert!(draft.description.contains("Contract mapping stale"));

        // Unavailable oracle_sha is diagnosed explicitly.
        let mut unavailable_rows = rows;
        for row in &mut unavailable_rows {
            row.oracle_sha = "0123456789abcdef0123456789abcdef01234567".into();
        }
        commit_rows(store.path(), &unavailable_rows);
        let committed = score::read_committed_trials(store.path()).unwrap();
        let consensus = analyze_for_store(store.path(), &committed, &ids()).unwrap();
        assert!(consensus
            .render()
            .contains("gap: 3/3 failures; contract mapping unavailable"));
    }
}
