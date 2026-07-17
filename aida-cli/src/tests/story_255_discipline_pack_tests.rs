//! Starter discipline pack — `docs/aida/discipline/` + the opt-in
//! memory pack scaffolded by `aida init --with-memories`.
//! trace:STORY-255 | STORY-443 | ai:claude
use super::*;

/// Verifies the embedded discipline pack scaffolds the full set of canonical
/// files into a new project's docs/aida/discipline/ directory.
///
/// TASK-465: STRUCTURAL, not count-based. The expected file set is derived
/// from the SAME single source `ensure_discipline_pack_scaffold` writes from
/// — the `docs/aida/discipline/` keys in `EMBEDDED_TEMPLATES` — so adding a
/// doc to the master template can never break this test (the prior hardcoded
/// `assert_eq!(written, 16)` + hand-maintained file list broke on every pack
// addition, e.g. PR-193). trace:TASK-465 | ai:claude
#[test]
fn discipline_pack_scaffolds_full_set() {
    use aida_core::templates::EMBEDDED_TEMPLATES;
    // Single source of truth: every embedded `docs/aida/discipline/<file>`.
    let expected: Vec<String> = EMBEDDED_TEMPLATES
        .keys()
        .filter_map(|k| k.strip_prefix("docs/aida/discipline/"))
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .collect();
    assert!(
        expected.len() >= 10,
        "sanity: expected a populated discipline pack, found {}",
        expected.len()
    );

    let root = tempfile::tempdir().unwrap();
    let written = ensure_discipline_pack_scaffold(root.path(), false).unwrap();
    assert_eq!(
        written,
        expected.len(),
        "scaffolded count must match the embedded discipline pack"
    );

    let dir = root.path().join("docs/aida/discipline");
    for f in &expected {
        assert!(dir.join(f).is_file(), "missing discipline doc: {f}");
    }

    // Idempotent: a second call without --force writes nothing.
    assert_eq!(
        ensure_discipline_pack_scaffold(root.path(), false).unwrap(),
        0
    );
    // --force re-writes them all.
    assert_eq!(
        ensure_discipline_pack_scaffold(root.path(), true).unwrap(),
        expected.len()
    );
}

#[test]
fn ecosystem_watch_scaffold_writes_with_todays_date_and_is_idempotent() {
    // trace:TASK-126 — fresh init must plant docs/competitive-analysis/
    // ecosystem-watch.md with today's local date in the `Last updated`
    // line so `scripts/release.sh`'s ecosystem-watch verification has a
    // file to read on a fresh project's first major/minor cut.
    let root = tempfile::tempdir().unwrap();
    let written = ensure_ecosystem_watch_scaffold(root.path(), false).unwrap();
    assert!(written, "expected the starter file to be written");

    let dest = root
        .path()
        .join("docs/competitive-analysis/ecosystem-watch.md");
    assert!(dest.is_file(), "missing ecosystem-watch.md");
    let body = std::fs::read_to_string(&dest).unwrap();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert!(
        body.contains(&format!("**Last updated**: {today}")),
        "expected today's date in Last updated line; got:\n{body}"
    );
    assert!(
        !body.contains("{{LAST_UPDATED}}"),
        "placeholder must be substituted"
    );

    // Idempotent: a second call without --force leaves the file alone.
    assert!(
        !ensure_ecosystem_watch_scaffold(root.path(), false).unwrap(),
        "second call without --force must not overwrite"
    );

    // --force re-writes.
    assert!(
        ensure_ecosystem_watch_scaffold(root.path(), true).unwrap(),
        "--force must re-write"
    );
}

#[test]
fn discipline_pack_lands_under_docs_aida_namespace() {
    // trace:STORY-443 — pack must land at docs/aida/discipline/, not the
    // historical docs/aida-discipline/. Guards against accidental
    // fallback during the namespace reshape.
    let root = tempfile::tempdir().unwrap();
    ensure_discipline_pack_scaffold(root.path(), false).unwrap();
    assert!(
        root.path().join("docs/aida/discipline/README.md").is_file(),
        "discipline pack must land at docs/aida/discipline/"
    );
    assert!(
        !root.path().join("docs/aida-discipline").exists(),
        "historical docs/aida-discipline/ path must not be created"
    );
}

#[test]
fn memory_pack_writes_marked_files_with_marker_and_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory");
    let report = scaffold_memory_pack_into(&mem, false, None).unwrap();

    assert!(
        report.written >= 10,
        "expected the marker-driven pack (>= 10), got {}",
        report.written
    );
    assert_eq!(report.unchanged, 0);

    // Every memory file carries the scaffold marker + a checksum.
    let mut md_count = 0;
    for entry in std::fs::read_dir(&mem).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == "MEMORY.md" {
            continue;
        }
        md_count += 1;
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("originSessionId: aida-scaffold"),
            "{path:?} missing scaffold marker"
        );
        assert!(
            body.contains("scaffoldChecksum: "),
            "{path:?} missing checksum"
        );
    }
    assert_eq!(md_count, report.written);

    // A known generic memory is present.
    assert!(mem
        .join("feedback_run_help_before_suggesting_flags.md")
        .is_file());

    // MEMORY.md carries the generated index block.
    let index = std::fs::read_to_string(mem.join("MEMORY.md")).unwrap();
    assert!(index.contains("<!-- aida:scaffold-pack:start -->"));
    assert!(index.contains("<!-- aida:scaffold-pack:end -->"));
    assert!(index.contains("](feedback_run_help_before_suggesting_flags.md)"));
}

/// STORY-362: the `--focus <subsystem>` loading filter. A memory tagged
/// `subsystem: X` loads only under `--focus X`; an untagged memory is
/// universal and loads regardless of focus (or its absence).
// trace:STORY-362 | ai:claude
#[test]
fn memory_focus_filter_loads_universal_plus_matching_subsystem() {
    // Hand-built templates — fully isolated, no embedded pack, no $HOME,
    // no filesystem.
    let universal = "---\nname: universal_mem\ntype: feedback\n---\nBody.\n";
    let orchestrator = concat!(
        "---\nname: orch_mem\nsubsystem: orchestrator\ntype: feedback\n---\n",
        "Body.\n"
    );
    let storage = "---\nname: store_mem\nsubsystem: storage\ntype: feedback\n---\nBody.\n";

    // No focus → everything loads (full pack).
    assert!(memory_matches_focus(universal, None));
    assert!(memory_matches_focus(orchestrator, None));
    assert!(memory_matches_focus(storage, None));

    // --focus orchestrator → universal + the orchestrator memory load;
    // the storage-tagged memory does not.
    assert!(
        memory_matches_focus(universal, Some("orchestrator")),
        "untagged memory must be universal under any focus"
    );
    assert!(
        memory_matches_focus(orchestrator, Some("orchestrator")),
        "subsystem-matching memory must load under its focus"
    );
    assert!(
        !memory_matches_focus(storage, Some("orchestrator")),
        "non-matching subsystem memory must NOT load under a different focus"
    );

    // Match is case-insensitive on the subsystem value.
    assert!(memory_matches_focus(orchestrator, Some("Orchestrator")));

    // Malformed / frontmatter-less content is treated as universal.
    assert!(memory_matches_focus("no frontmatter here", Some("storage")));
}

/// STORY-362: end-to-end through the scaffold writer — only universal +
/// matching-subsystem memories land on disk under `--focus`.
// trace:STORY-362 | ai:claude
#[test]
fn memory_pack_focus_scopes_written_files() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory");
    // The embedded pack is currently all-universal (untagged), so a focus
    // on any subsystem still writes the full universal set — and crucially
    // no fewer than the unfocused run for the universal members. Compare a
    // focused run against an unfocused baseline: every file the focused run
    // wrote must be one the baseline also wrote (subset), and every member
    // it kept is universal.
    let focused = scaffold_memory_pack_into(&mem, false, Some("nonexistent-subsystem")).unwrap();
    // All embedded members are untagged today → all universal → all written.
    assert!(
        focused.written >= 10,
        "untagged (universal) memories must always load under any focus, got {}",
        focused.written
    );
    // Every written file is genuinely universal (no subsystem tag).
    for entry in std::fs::read_dir(&mem).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == "MEMORY.md" {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        let (fm, _) = split_md_frontmatter(&body).unwrap();
        assert!(
            frontmatter_field(fm, "subsystem").is_none(),
            "{path:?} carries a subsystem tag but was written under a non-matching focus"
        );
    }
}

#[test]
fn memory_pack_refresh_overlays_pristine_skips_edited() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory");
    let first = scaffold_memory_pack_into(&mem, false, None).unwrap();
    assert!(first.written >= 10);

    // (a) User-edit one file's body — must be kept on refresh.
    let edited = mem.join("feedback_verify_before_filing.md");
    let mut edited_content = std::fs::read_to_string(&edited).unwrap();
    edited_content.push_str("\n\nUSER EDIT — do not clobber.\n");
    std::fs::write(&edited, &edited_content).unwrap();

    // (b) Replace one file with a user-owned file (no scaffold marker).
    let user_owned = mem.join("feedback_goal_prompt_phrasing.md");
    std::fs::write(&user_owned, "---\nname: mine\n---\n\nmy own memory\n").unwrap();

    // (c) Make one pristine file look like an older scaffold version: a
    //     benign extra frontmatter line, body untouched → still Pristine
    //     but != the current scaffold, so refresh overlays it.
    let stale = mem.join("feedback_pause_for_design_input.md");
    let stale_v1 =
        std::fs::read_to_string(&stale)
            .unwrap()
            .replacen("---\n", "---\nstaleField: x\n", 1);
    std::fs::write(&stale, &stale_v1).unwrap();

    let report = scaffold_memory_pack_into(&mem, true, None).unwrap();
    assert_eq!(report.written, 0, "all files already exist on refresh");
    assert_eq!(report.kept_edited, 1, "the body-edited file is kept");
    assert_eq!(report.kept_user, 1, "the unmarked user file is kept");
    assert_eq!(
        report.refreshed, 1,
        "the stale-but-pristine file is overlaid"
    );

    // The user edit survived.
    assert!(std::fs::read_to_string(&edited)
        .unwrap()
        .contains("USER EDIT"));
    // The user-owned file survived.
    assert!(std::fs::read_to_string(&user_owned)
        .unwrap()
        .contains("my own memory"));
    // The stale file was overlaid — the benign line is gone.
    assert!(!std::fs::read_to_string(&stale)
        .unwrap()
        .contains("staleField"));
}

#[test]
fn memory_disposition_classifies_pristine_edited_and_user() {
    let template = "---\nname: t\ndescription: d\nmetadata:\n  type: feedback\n---\n\nbody text\n";
    let scaffolded = build_scaffolded_memory(template).unwrap();
    assert!(scaffolded.contains("originSessionId: aida-scaffold"));
    assert_eq!(
        memory_refresh_disposition(&scaffolded),
        MemoryDisposition::Pristine
    );

    // A user file with no scaffold marker.
    assert_eq!(
        memory_refresh_disposition("---\nname: x\n---\n\nmine\n"),
        MemoryDisposition::UserOwned
    );

    // Scaffolded but the body was edited → checksum mismatch.
    let edited = scaffolded.replace("body text", "body text — changed");
    assert_eq!(
        memory_refresh_disposition(&edited),
        MemoryDisposition::Edited
    );
}

#[test]
fn parser_is_line_ending_agnostic_crlf_and_cr() {
    // The embedded memory templates are checked out as CRLF on a Windows
    // box with git autocrlf, then embedded verbatim by build.rs. The
    // frontmatter parser must not assume LF — and the scaffolded output
    // (hence its checksum) must be byte-identical across platforms.
    // Regression for BUG-244. trace:BUG-244 | ai:claude
    let lf = "---\nname: t\ndescription: d\nmetadata:\n  type: feedback\n---\n\nbody text\n";
    let crlf = lf.replace('\n', "\r\n");
    let cr = lf.replace('\n', "\r");

    let from_lf = build_scaffolded_memory(lf).expect("LF template parses");
    let from_crlf = build_scaffolded_memory(&crlf).expect("CRLF template parses");
    let from_cr = build_scaffolded_memory(&cr).expect("bare-CR template parses");

    assert_eq!(
        from_lf, from_crlf,
        "CRLF input must scaffold identically to LF"
    );
    assert_eq!(
        from_lf, from_cr,
        "bare-CR input must scaffold identically to LF"
    );
    assert!(
        !from_crlf.contains('\r'),
        "scaffolded output must be LF-only regardless of input line endings"
    );
    assert!(from_crlf.contains("scaffoldChecksum: "));

    // A CRLF-on-disk copy of a pristine scaffold still classifies as
    // Pristine — the refresh check is line-ending-agnostic too.
    let crlf_scaffolded = from_lf.replace('\n', "\r\n");
    assert_eq!(
        memory_refresh_disposition(&crlf_scaffolded),
        MemoryDisposition::Pristine,
        "a CRLF copy of a pristine scaffold must still be Pristine"
    );
}

// ── STORY-410: existing-project substrate-drift discovery ──────────────

/// A fresh dir that never scaffolded the pack reports every master member
/// as Missing — `behind()` equals the full master count and nothing is
// up-to-date / edited / user-owned. trace:STORY-410 | ai:claude
#[test]
fn drift_empty_dir_reports_all_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory"); // does not exist
    let report = compute_memory_drift_into(&mem).unwrap();

    assert!(
        report.rows.len() >= 10,
        "sanity: a populated master pack, got {}",
        report.rows.len()
    );
    assert_eq!(report.missing(), report.rows.len());
    assert_eq!(report.behind(), report.rows.len());
    assert_eq!(report.up_to_date(), 0);
    assert_eq!(report.stale(), 0);
    assert_eq!(report.edited(), 0);
    assert_eq!(report.user_owned(), 0);
}

/// A freshly-scaffolded dir is fully up to date: zero drift, every member
// matches the master byte-for-byte. trace:STORY-410 | ai:claude
#[test]
fn drift_freshly_scaffolded_dir_is_current() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory");
    let written = scaffold_memory_pack_into(&mem, false, None).unwrap();
    assert!(written.written >= 10);

    let report = compute_memory_drift_into(&mem).unwrap();
    assert_eq!(report.behind(), 0, "a fresh scaffold must not be behind");
    assert_eq!(report.missing(), 0);
    assert_eq!(report.stale(), 0);
    assert_eq!(
        report.up_to_date(),
        report.rows.len(),
        "every member matches master"
    );
}

/// The three drift dispositions are classified correctly: a deleted file
/// → Missing, a body-edited file → Edited (kept by refresh), an unmarked
/// user file → UserOwned, a benign-frontmatter-mutated pristine file →
// Stale (refresh would overlay it). trace:STORY-410 | ai:claude
#[test]
fn drift_classifies_missing_stale_edited_and_user_owned() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory");
    scaffold_memory_pack_into(&mem, false, None).unwrap();

    // (a) Missing: delete one scaffolded file.
    let missing = mem.join("feedback_run_help_before_suggesting_flags.md");
    std::fs::remove_file(&missing).unwrap();

    // (b) Edited: change a scaffolded file's body → checksum mismatch.
    let edited = mem.join("feedback_verify_before_filing.md");
    let mut body = std::fs::read_to_string(&edited).unwrap();
    body.push_str("\n\nUSER EDIT.\n");
    std::fs::write(&edited, &body).unwrap();

    // (c) UserOwned: replace a file with an unmarked one.
    let user_owned = mem.join("feedback_goal_prompt_phrasing.md");
    std::fs::write(&user_owned, "---\nname: mine\n---\n\nmine\n").unwrap();

    // (d) Stale: pristine body, but a benign extra frontmatter line makes
    //     the on-disk bytes differ from the current master scaffold.
    let stale = mem.join("feedback_pause_for_design_input.md");
    let stale_v1 =
        std::fs::read_to_string(&stale)
            .unwrap()
            .replacen("---\n", "---\nstaleField: x\n", 1);
    std::fs::write(&stale, &stale_v1).unwrap();

    let report = compute_memory_drift_into(&mem).unwrap();

    let find = |name: &str| {
        report
            .rows
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("row {name} present"))
            .state
    };
    assert_eq!(
        find("feedback_run_help_before_suggesting_flags.md"),
        MemoryDriftState::Missing
    );
    assert_eq!(
        find("feedback_verify_before_filing.md"),
        MemoryDriftState::Edited
    );
    assert_eq!(
        find("feedback_goal_prompt_phrasing.md"),
        MemoryDriftState::UserOwned
    );
    assert_eq!(
        find("feedback_pause_for_design_input.md"),
        MemoryDriftState::Stale
    );

    // behind() counts only missing + stale; edited/user-owned are kept.
    assert_eq!(report.missing(), 1);
    assert_eq!(report.stale(), 1);
    assert_eq!(report.edited(), 1);
    assert_eq!(report.user_owned(), 1);
    assert_eq!(report.behind(), 2);
}

/// Every drift row carries a label + (usually) a description sourced from
/// the master template's frontmatter, so the report is human-readable.
// trace:STORY-410 | ai:claude
#[test]
fn drift_rows_carry_labels_and_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    let mem = dir.path().join("memory"); // empty → all missing
    let report = compute_memory_drift_into(&mem).unwrap();

    // Labels are never empty (fall back to filename).
    assert!(report.rows.iter().all(|r| !r.label.is_empty()));
    // The bundled memories carry descriptions, so most rows have one.
    let with_desc = report
        .rows
        .iter()
        .filter(|r| !r.description.is_empty())
        .count();
    assert!(
        with_desc >= report.rows.len() / 2,
        "expected most rows to carry a description, {with_desc}/{}",
        report.rows.len()
    );
}
