use super::*;

// BUG-1945: digest ordering is by instant, not by timestamp string.
// trace:BUG-1945 | ai:claude
fn digest_row(spec_id: &str, ts: &str) -> DigestRow {
    DigestRow {
        spec_id: spec_id.to_string(),
        req_type: "BUG".to_string(),
        status: "Approved".to_string(),
        title: String::new(),
        last_ts_iso: ts.to_string(),
        last_author: String::new(),
        had_add: false,
        had_delete: false,
    }
}

fn digest_ids(rows: &[DigestRow]) -> Vec<&str> {
    rows.iter().map(|r| r.spec_id.as_str()).collect()
}

// trace:BUG-1945 | ai:claude
#[test]
fn digest_orders_by_instant_not_string() {
    // 06:00-07:00 is 13:00Z: later than 12:00Z, though it sorts first as
    // a string.
    let mut rows = vec![
        digest_row("BUG-1", "2025-03-04T12:00:00Z"),
        digest_row("BUG-2", "2025-03-04T06:00:00-07:00"),
    ];
    sort_digest_rows(&mut rows);
    assert_eq!(digest_ids(&rows), vec!["BUG-2", "BUG-1"]);
    // Rendering is untouched: the original strings survive the sort.
    assert_eq!(rows[0].last_ts_iso, "2025-03-04T06:00:00-07:00");
    assert_eq!(rows[1].last_ts_iso, "2025-03-04T12:00:00Z");
}

// trace:BUG-1945 | ai:claude
#[test]
fn digest_tie_breaks_by_spec_id() {
    // A live spec (YAML `...Z`) and a deleted spec (git `+00:00` from
    // older git, `Z` from newer) at the same instant order by spec id,
    // whatever the input order and whichever form git printed.
    for deleted_ts in ["2025-03-04T12:00:00+00:00", "2025-03-04T12:00:00Z"] {
        for flip in [false, true] {
            let live = digest_row("BUG-20", "2025-03-04T12:00:00Z");
            let deleted = digest_row("BUG-10", deleted_ts);
            let mut rows = if flip {
                vec![deleted, live]
            } else {
                vec![live, deleted]
            };
            sort_digest_rows(&mut rows);
            assert_eq!(
                digest_ids(&rows),
                vec!["BUG-10", "BUG-20"],
                "deleted_ts={deleted_ts} flip={flip}"
            );
        }
    }
}

// trace:BUG-1945 | ai:claude
#[test]
fn digest_unparseable_timestamp_sorts_last() {
    let mut rows = vec![
        digest_row("BUG-3", "not-a-timestamp"),
        digest_row("BUG-1", "2025-03-04T12:00:00Z"),
        digest_row("BUG-4", ""),
        digest_row("BUG-2", "2025-03-05T12:00:00+00:00"),
    ];
    sort_digest_rows(&mut rows);
    assert_eq!(digest_ids(&rows), vec!["BUG-2", "BUG-1", "BUG-3", "BUG-4"]);
}

/// TASK-1055: a path-scoped `--id` walk only visits the spec's own commits,
/// not the whole orphan-branch history. Builds a tiny store with three
/// commits — one touching only TASK-1, one touching only STORY-1, and a
/// bulk commit touching BOTH — then asserts (a) git's pathspec sees just
/// the two commits that touched TASK-1's YAML (not all three), and (b) the
/// filtered events come back as TASK-1's only.
// trace:TASK-1055
#[test]
fn history_id_filter_path_scopes_log_walk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);

    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    };
    let task_path = "objects/TASK/000/TASK-1.yaml";
    let story_path = "objects/STORY/000/STORY-1.yaml";

    // Commit 1: TASK-1 only.
    write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add TASK-1"]);

    // Commit 2: STORY-1 only (a different spec — must NOT be walked).
    write(story_path, "spec_id: STORY-1\ntitle: s\nstatus: Draft\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add STORY-1"]);

    // Commit 3: a bulk commit touching BOTH specs.
    write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Approved\n");
    write(story_path, "spec_id: STORY-1\ntitle: s\nstatus: Approved\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "chore: update requirements store"]);

    // (a) git's pathspec sees only the two TASK-1 commits, not all three.
    let scoped = run_git(
        root,
        &[
            "log".into(),
            "--oneline".into(),
            "--".into(),
            task_path.into(),
        ],
    )
    .unwrap();
    let full = run_git(root, &["log".into(), "--oneline".into()]).unwrap();
    assert_eq!(
        scoped.lines().count(),
        2,
        "pathspec must scope the walk to TASK-1's 2 commits"
    );
    assert_eq!(
        full.lines().count(),
        3,
        "the full history has all 3 commits — proving the pathspec narrows it"
    );

    // (b) the filtered events are TASK-1's only.
    let opts = HistoryOpts {
        id_filter: Some("TASK-1".to_string()),
        ..base_opts()
    };
    let (events, _, _) = collect_filtered_events(root, &opts).unwrap();
    assert!(!events.is_empty(), "expected at least one TASK-1 event");
    assert!(
        events.iter().all(|e| e.spec_id == "TASK-1"),
        "every event must be for the path-scoped spec, got: {:?}",
        events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
    );
}

/// STORY-737 (delight #4): the default `aida history` view hides the
/// stateless internal META prompt-template rows (`exclude_meta`), but they
/// stay reachable. This drives the real orphan-store git log and asserts the
/// META spec's events drop out by default and return when `exclude_meta` is
/// off (the `--include-meta` / `--type meta` path).
// trace:STORY-737
#[test]
fn history_excludes_meta_rows_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);

    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    };
    let meta_path = "objects/META/000/META-1.yaml";
    let task_path = "objects/TASK/000/TASK-1.yaml";

    // Commit 1: seed both specs as Draft.
    write(meta_path, "spec_id: META-1\ntitle: m\nstatus: Draft\n");
    write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "seed"]);

    // Commit 2: flip both to Approved — one status-change event each.
    write(meta_path, "spec_id: META-1\ntitle: m\nstatus: Approved\n");
    write(task_path, "spec_id: TASK-1\ntitle: t\nstatus: Approved\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "approve both"]);

    // Default view (exclude_meta = true): META-1 is hidden, TASK-1 shows.
    let hidden = HistoryOpts {
        exclude_meta: true,
        ..base_opts()
    };
    let (events, _, _) = collect_filtered_events(root, &hidden).unwrap();
    assert!(
        events.iter().any(|e| e.spec_id == "TASK-1"),
        "the real spec's events must still show, got: {:?}",
        events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
    );
    assert!(
        !events.iter().any(|e| e.spec_id == "META-1"),
        "META rows must be hidden by default, got: {:?}",
        events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
    );

    // `--include-meta` / `--type meta` (exclude_meta = false): META returns.
    let shown = HistoryOpts {
        exclude_meta: false,
        ..base_opts()
    };
    let (events, _, _) = collect_filtered_events(root, &shown).unwrap();
    assert!(
        events.iter().any(|e| e.spec_id == "META-1"),
        "META rows must be visible when not excluded, got: {:?}",
        events.iter().map(|e| &e.spec_id).collect::<Vec<_>>()
    );
}

/// BUG-1596: the digest must resolve a requirement's *current* YAML
/// under its real shard, not a hard-coded `000`. Builds a store with one
/// spec in shard 000 (BUG-1) and one whose sequence number lands in
/// shard 001 (BUG-1001, sequence 1001 — the first id past the 1000-per-
/// shard boundary), commits both, and asserts the digest reports each
/// one's real status/title rather than "(deleted)". A genuinely deleted
/// spec (never written) must still render "(deleted)".
// trace:BUG-1596 | ai:claude
#[test]
fn digest_resolves_current_yaml_across_shards() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);

    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    };

    // Shard 000: sequence 1.
    let shard0_path = "objects/BUG/000/BUG-1.yaml";
    write(
        shard0_path,
        "spec_id: BUG-1\ntitle: shard zero bug\nstatus: Completed\nmodified_at: \"2026-01-01T00:00:00Z\"\n",
    );
    // Shard 001: sequence 1001 (first id past the 1000-per-shard
    // boundary — object_path()/shard_number() puts seq 1001 in shard
    // 001). This is the path the hard-coded "000" join used to miss.
    let shard1_path = "objects/BUG/001/BUG-1001.yaml";
    write(
        shard1_path,
        "spec_id: BUG-1001\ntitle: shard one bug\nstatus: Done\nmodified_at: \"2026-01-02T00:00:00Z\"\n",
    );
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add BUG-1 and BUG-1001"]);

    let opts = HistoryOpts {
        events_mode: false,
        ..base_opts()
    };
    let (rows, _, _) = build_digest_rows(root, &opts).unwrap();

    let by_id = |id: &str| rows.iter().find(|r| r.spec_id == id);

    let shard0 = by_id("BUG-1").expect("BUG-1 (shard 000) must appear");
    assert_eq!(shard0.status, "Completed");
    assert_eq!(shard0.title, "shard zero bug");

    let shard1 = by_id("BUG-1001").expect("BUG-1001 (shard 001) must appear");
    assert_eq!(
        shard1.status, "Done",
        "shard-001 spec must resolve its real status, not fall through to (deleted)"
    );
    assert_eq!(shard1.title, "shard one bug");

    // A genuinely deleted / never-written id still renders "(deleted)".
    let (status, title, modified_at) = read_current(&root.join("objects/BUG/000/BUG-999.yaml"));
    assert_eq!(status, "(deleted)");
    assert_eq!(title, "");
    assert_eq!(modified_at, None);
}

// TASK-1502: `resolve_history_window` — every unit, mixed relative +
// absolute bounds, bad units, reversed order, and unchanged absolute
// forms. All deterministic via an injected `now`. trace:TASK-1502 | ai:claude

fn fixed_now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-09-25T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

/// A fixed, non-UTC offset (UTC-7) so `resolve_history_window` tests are
/// deterministic regardless of the test machine's real timezone, and so
/// the local-midnight-vs-UTC-midnight distinction is actually exercised
/// (on UTC+0 the two coincide and the bug wouldn't show up).
// trace:TASK-1502 | ai:claude
fn fixed_local_offset() -> chrono::FixedOffset {
    chrono::FixedOffset::west_opt(7 * 3600).unwrap()
}

#[test]
fn resolve_history_window_every_relative_unit() {
    let now = fixed_now();
    let offset = fixed_local_offset();
    let cases: &[(&str, chrono::Duration)] = &[
        ("30m", chrono::Duration::minutes(30)),
        ("5h", chrono::Duration::hours(5)),
        ("7d", chrono::Duration::days(7)),
        ("2w", chrono::Duration::weeks(2)),
    ];
    for (raw, delta) in cases {
        let opts = HistoryOpts {
            since: Some((*raw).to_string()),
            ..base_opts()
        };
        let (resolved, since_at, until_at) = resolve_history_window(&opts, now, &offset).unwrap();
        assert_eq!(since_at, Some(now - *delta), "unit `{raw}`");
        assert_eq!(until_at, None);
        // The rewritten opts carry an unambiguous RFC3339 string, not
        // the original compact form, so every downstream `git log`
        // call gets a value git's approxidate parser won't mangle.
        assert_eq!(resolved.since, Some((now - *delta).to_rfc3339()));
    }
}

#[test]
fn resolve_history_window_mixed_relative_and_absolute_bounds() {
    // --since as a compact relative duration, --until as an absolute
    // RFC3339 timestamp — both forms must compose.
    let now = fixed_now();
    let offset = fixed_local_offset();
    let opts = HistoryOpts {
        since: Some("7d".to_string()),
        until: Some("2026-09-24T00:00:00Z".to_string()),
        ..base_opts()
    };
    let (_, since_at, until_at) = resolve_history_window(&opts, now, &offset).unwrap();
    assert_eq!(since_at, Some(now - chrono::Duration::days(7)));
    assert_eq!(
        until_at,
        Some(
            chrono::DateTime::parse_from_rfc3339("2026-09-24T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc)
        )
    );

    // And the reverse pairing: --since absolute (bare date, LOCAL
    // midnight at UTC-7 → 07:00 UTC), --until relative.
    let opts = HistoryOpts {
        since: Some("2026-09-01".to_string()),
        until: Some("5h".to_string()),
        ..base_opts()
    };
    let (_, since_at, until_at) = resolve_history_window(&opts, now, &offset).unwrap();
    assert_eq!(since_at.unwrap().to_rfc3339(), "2026-09-01T07:00:00+00:00");
    assert_eq!(until_at, Some(now - chrono::Duration::hours(5)));
}

#[test]
fn resolve_history_window_unchanged_absolute_forms_still_work() {
    // RFC3339 — the pre-TASK-1502 documented form — must keep resolving
    // exactly as before, zone honored exactly regardless of the
    // machine's local offset.
    let now = fixed_now();
    let offset = fixed_local_offset();
    let opts = HistoryOpts {
        since: Some("2026-05-01T00:00:00Z".to_string()),
        until: Some("2026-06-01T00:00:00+02:00".to_string()),
        ..base_opts()
    };
    let (_, since_at, until_at) = resolve_history_window(&opts, now, &offset).unwrap();
    assert_eq!(
        since_at.unwrap().format("%Y-%m-%d").to_string(),
        "2026-05-01"
    );
    assert_eq!(until_at.unwrap().to_rfc3339(), "2026-05-31T22:00:00+00:00");
}

#[test]
fn resolve_history_window_bare_date_uses_local_offset_not_utc() {
    // A bare ISO date resolves to LOCAL midnight, not UTC midnight; the
    // two differ by exactly `offset` away from UTC+0.
    let now = fixed_now();
    let offset = fixed_local_offset(); // UTC-7
    let opts = HistoryOpts {
        since: Some("2026-05-01".to_string()),
        ..base_opts()
    };
    let (resolved, since_at, _) = resolve_history_window(&opts, now, &offset).unwrap();
    assert_eq!(
        since_at.unwrap().to_rfc3339(),
        "2026-05-01T07:00:00+00:00",
        "bare date must resolve to local midnight (UTC-7 → 07:00 UTC), not UTC midnight"
    );
    assert_eq!(
        resolved.since,
        Some("2026-05-01T07:00:00+00:00".to_string())
    );
}

#[test]
fn resolve_history_window_rejects_bad_unit() {
    let now = fixed_now();
    let offset = fixed_local_offset();
    let opts = HistoryOpts {
        since: Some("5y".to_string()),
        ..base_opts()
    };
    let err = resolve_history_window(&opts, now, &offset)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--since") && err.contains("5y"),
        "error should name the flag and the bad value, got: {err}"
    );

    let opts = HistoryOpts {
        until: Some("3q".to_string()),
        ..base_opts()
    };
    let err = resolve_history_window(&opts, now, &offset)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--until") && !err.starts_with("invalid --since"),
        "a bad --until must be labeled --until, not --since, got: {err}"
    );
}

#[test]
fn resolve_history_window_rejects_since_later_than_until() {
    let now = fixed_now();
    let offset = fixed_local_offset();
    // --since 5h (5 hours ago) is LATER than --until 7d (7 days ago) —
    // the window is empty and must be refused with a clear error.
    let opts = HistoryOpts {
        since: Some("5h".to_string()),
        until: Some("7d".to_string()),
        ..base_opts()
    };
    let err = resolve_history_window(&opts, now, &offset)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--since") && err.contains("--until"),
        "reversed-order error should name both flags, got: {err}"
    );
}

#[test]
fn format_resolved_window_variants() {
    let now = fixed_now();
    let offset = fixed_local_offset();
    assert_eq!(format_resolved_window(None, None, &offset), None);
    assert!(format_resolved_window(Some(now), None, &offset)
        .unwrap()
        .contains("since"));
    assert!(format_resolved_window(None, Some(now), &offset)
        .unwrap()
        .contains("until"));
    let both =
        format_resolved_window(Some(now - chrono::Duration::days(1)), Some(now), &offset).unwrap();
    // Rendered in the injected LOCAL offset (UTC-7): `now`
    // (2026-09-25T12:00:00Z) is 2026-09-25 05:00 -0700 locally, and
    // `now - 1 day` is 2026-09-24 05:00 -0700 — same calendar dates as
    // UTC here (no midnight boundary crossed at this hour), so this
    // also proves the explicit `-0700` zone marker is present.
    // trace:TASK-1502 | ai:claude
    assert!(
        both.contains("-0700"),
        "Window line must state the timezone, got: {both}"
    );
    assert!(both.contains("2026-09-24") && both.contains("2026-09-25"));
}

/// A test-only zone that observes US-Pacific-style DST in 2026: UTC-8
/// outside, UTC-7 between 2026-03-08T10:00Z (02:00 PST) and
/// 2026-11-01T09:00Z (02:00 PDT). Stands in for a real DST zone so the
/// per-date offset lookup is exercised without adding chrono-tz.
// trace:TASK-1502 | ai:claude
#[derive(Clone, Copy, Debug)]
struct TestPacific;

impl TestPacific {
    fn std() -> chrono::FixedOffset {
        chrono::FixedOffset::west_opt(8 * 3600).unwrap()
    }
    fn dst() -> chrono::FixedOffset {
        chrono::FixedOffset::west_opt(7 * 3600).unwrap()
    }
}

impl chrono::TimeZone for TestPacific {
    type Offset = chrono::FixedOffset;

    fn from_offset(_: &chrono::FixedOffset) -> Self {
        TestPacific
    }

    fn offset_from_local_date(
        &self,
        local: &chrono::NaiveDate,
    ) -> chrono::MappedLocalTime<chrono::FixedOffset> {
        self.offset_from_local_datetime(&local.and_time(chrono::NaiveTime::MIN))
    }

    fn offset_from_local_datetime(
        &self,
        local: &chrono::NaiveDateTime,
    ) -> chrono::MappedLocalTime<chrono::FixedOffset> {
        // Every offset whose implied UTC instant maps back to itself.
        let valid: Vec<chrono::FixedOffset> = [Self::std(), Self::dst()]
            .into_iter()
            .filter(|off| {
                let utc = *local - chrono::Duration::seconds(off.local_minus_utc().into());
                self.offset_from_utc_datetime(&utc) == *off
            })
            .collect();
        match valid.as_slice() {
            [] => chrono::MappedLocalTime::None,
            [one] => chrono::MappedLocalTime::Single(*one),
            [a, b] => chrono::MappedLocalTime::Ambiguous(*a, *b),
            _ => unreachable!(),
        }
    }

    fn offset_from_utc_date(&self, utc: &chrono::NaiveDate) -> chrono::FixedOffset {
        self.offset_from_utc_datetime(&utc.and_time(chrono::NaiveTime::MIN))
    }

    fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> chrono::FixedOffset {
        let start = chrono::NaiveDate::from_ymd_opt(2026, 3, 8)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let end = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        if *utc >= start && *utc < end {
            Self::dst()
        } else {
            Self::std()
        }
    }
}

#[test]
fn resolve_history_window_bare_dates_use_that_dates_dst_offset() {
    // `now` is in September (PDT, UTC-7), but a January date must still
    // resolve against January's offset (PST, UTC-8), not today's.
    let now = fixed_now();
    let opts = HistoryOpts {
        since: Some("2026-01-15".to_string()),
        until: Some("2026-07-15".to_string()),
        ..base_opts()
    };
    let (_, since_at, until_at) = resolve_history_window(&opts, now, &TestPacific).unwrap();
    assert_eq!(since_at.unwrap().to_rfc3339(), "2026-01-15T08:00:00+00:00");
    assert_eq!(until_at.unwrap().to_rfc3339(), "2026-07-15T07:00:00+00:00");
    // And the Window line shows each instant's own offset.
    let line = format_resolved_window(since_at, until_at, &TestPacific).unwrap();
    assert!(
        line.contains("2026-01-15 00:00 -0800") && line.contains("2026-07-15 00:00 -0700"),
        "each bound must render with its own offset, got: {line}"
    );
}

#[test]
fn resolve_history_window_rejects_dst_gap_and_overlap() {
    let now = fixed_now();
    // 02:30 on the spring-forward day never happens locally.
    let opts = HistoryOpts {
        since: Some("2026-03-08T02:30".to_string()),
        ..base_opts()
    };
    let err = resolve_history_window(&opts, now, &TestPacific)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--since") && err.contains("ambiguous or doesn't exist"),
        "a DST-gap time must be refused with the DST reason, got: {err}"
    );
    // 01:30 on the fall-back day happens twice.
    let opts = HistoryOpts {
        until: Some("2026-11-01 01:30".to_string()),
        ..base_opts()
    };
    let err = resolve_history_window(&opts, now, &TestPacific)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--until") && err.contains("ambiguous or doesn't exist"),
        "a DST-overlap time must be refused with the DST reason, got: {err}"
    );
}

#[test]
fn resolve_history_window_accepts_legacy_forms() {
    // Zone-less ISO datetimes (local time) and git-style `N units ago`
    // phrases were accepted before and must keep working.
    let now = fixed_now();
    let offset = fixed_local_offset(); // UTC-7
    for raw in [
        "2026-05-01T10:00",
        "2026-05-01 10:00",
        "2026-05-01T10:00:00",
    ] {
        let opts = HistoryOpts {
            since: Some(raw.to_string()),
            ..base_opts()
        };
        let (_, since_at, _) = resolve_history_window(&opts, now, &offset).unwrap();
        assert_eq!(
            since_at.unwrap().to_rfc3339(),
            "2026-05-01T17:00:00+00:00",
            "`{raw}` must be read as local time"
        );
    }
    let cases: &[(&str, chrono::Duration)] = &[
        ("24 hours ago", chrono::Duration::hours(24)),
        ("1 hour ago", chrono::Duration::hours(1)),
        ("90 minutes ago", chrono::Duration::minutes(90)),
        ("3 days ago", chrono::Duration::days(3)),
        ("1 week ago", chrono::Duration::weeks(1)),
        ("2 Weeks Ago", chrono::Duration::weeks(2)),
    ];
    for (raw, delta) in cases {
        let opts = HistoryOpts {
            since: Some((*raw).to_string()),
            ..base_opts()
        };
        let (_, since_at, _) = resolve_history_window(&opts, now, &offset).unwrap();
        assert_eq!(since_at, Some(now - *delta), "phrase `{raw}`");
    }
    for bad in ["3 fortnights ago", "3 days", "ago 3 days"] {
        let opts = HistoryOpts {
            since: Some(bad.to_string()),
            ..base_opts()
        };
        assert!(
            resolve_history_window(&opts, now, &offset).is_err(),
            "`{bad}` must be rejected"
        );
    }
}

/// TASK-1502: end-to-end through `collect_filtered_events` — a compact
/// relative duration like `1h` must bound the actual `git log --since=`
/// walk correctly, not just parse cleanly. This is the regression the
/// feature exists to fix: git's own approxidate parser does not
/// understand compact `30m`/`1h` forms (`git log --since=30m` is read as
/// a date on the 30th and matches nothing, with no error), so history
/// resolves the bound itself and hands git an unambiguous RFC3339
/// string instead.
#[test]
fn relative_since_bounds_the_actual_git_log_walk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let git = |args: &[&str], env: &[(&str, &str)]| {
        let mut cmd = crate::test_git::git_command();
        cmd.arg("-C").arg(root).args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"], &[]);
    git(&["config", "user.email", "t@example.com"], &[]);
    git(&["config", "user.name", "t"], &[]);

    let write = |rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    };

    let now = fixed_now();
    let old_ts = (now - chrono::Duration::hours(2)).to_rfc3339();
    let recent_ts = (now - chrono::Duration::minutes(10)).to_rfc3339();

    // Commit A: 2 hours before `now` — outside a 1h window.
    write(
        "objects/TASK/000/TASK-1.yaml",
        "spec_id: TASK-1\ntitle: old\nstatus: Draft\n",
    );
    git(&["add", "-A"], &[]);
    git(
        &["commit", "-q", "-m", "old commit"],
        &[
            ("GIT_AUTHOR_DATE", old_ts.as_str()),
            ("GIT_COMMITTER_DATE", old_ts.as_str()),
        ],
    );

    // Commit B: 10 minutes before `now` — inside a 1h window.
    write(
        "objects/TASK/000/TASK-2.yaml",
        "spec_id: TASK-2\ntitle: recent\nstatus: Draft\n",
    );
    git(&["add", "-A"], &[]);
    git(
        &["commit", "-q", "-m", "recent commit"],
        &[
            ("GIT_AUTHOR_DATE", recent_ts.as_str()),
            ("GIT_COMMITTER_DATE", recent_ts.as_str()),
        ],
    );

    let opts = HistoryOpts {
        since: Some("1h".to_string()),
        ..base_opts()
    };
    let (resolved, _, _) = resolve_history_window(&opts, now, &fixed_local_offset()).unwrap();
    let (events, _, _) = collect_filtered_events(root, &resolved).unwrap();
    let ids: std::collections::BTreeSet<&str> = events.iter().map(|e| e.spec_id.as_str()).collect();
    assert!(
        ids.contains("TASK-2"),
        "recent commit must be in a 1h window, got {ids:?}"
    );
    assert!(
        !ids.contains("TASK-1"),
        "commit from 2h ago must be excluded from a 1h window, got {ids:?}"
    );
}

/// BUG-1616: a commit that deletes one spec and adds another with
/// near-identical content is paired by git's default rename detection
/// into a single `R<score>` line. The event walk and the digest must
/// still report the delete and the add as separate events.
// trace:BUG-1616 | ai:claude
#[test]
fn history_reports_delete_and_add_that_git_pairs_as_rename() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let git = |args: &[&str]| -> String {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);

    let a_path = "objects/BUG/000/BUG-389.yaml";
    let b_path = "objects/BUG/000/BUG-390.yaml";
    let body = |id: &str| {
        format!(
            "spec_id: {id}\ntitle: a long shared title so git scores the pair as a rename\n\
                 status: Draft\npriority: high\nreq_type: bug\n\
                 description: the same long description body in both files so the \
                 similarity index stays well above the rename threshold\n"
        )
    };

    std::fs::create_dir_all(root.join("objects/BUG/000")).unwrap();
    std::fs::write(root.join(a_path), body("BUG-389")).unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add BUG-389"]);

    // One commit: delete BUG-389, add BUG-390 with near-identical content.
    std::fs::remove_file(root.join(a_path)).unwrap();
    std::fs::write(root.join(b_path), body("BUG-390")).unwrap();
    git(&["add", "-A"]);
    git(&[
        "commit",
        "-q",
        "-m",
        "chore: update 1 requirements, delete 1",
    ]);

    // Precondition: git's default detection does pair these as a rename,
    // so the fixture really exercises the bug.
    let default_status = git(&["show", "--name-status", "--format=", "-M", "HEAD"]);
    assert!(
        default_status.lines().any(|l| l.starts_with('R')),
        "fixture must be a git rename pair, got: {default_status}"
    );
    let head = git(&["rev-parse", "HEAD"]).trim().to_string();

    // Event walk: both the delete and the add from the rename commit.
    let (events, _, _) = collect_filtered_events(root, &base_opts()).unwrap();
    let in_head: Vec<&Event> = events.iter().filter(|e| e.sha == head).collect();
    assert!(
        in_head
            .iter()
            .any(|e| e.spec_id == "BUG-389" && matches!(e.kind, EventKind::Deleted { .. })),
        "missing Deleted BUG-389, got: {:?}",
        in_head
            .iter()
            .map(|e| (&e.spec_id, &e.kind))
            .collect::<Vec<_>>()
    );
    assert!(
        in_head
            .iter()
            .any(|e| e.spec_id == "BUG-390" && matches!(e.kind, EventKind::Added { .. })),
        "missing Added BUG-390, got: {:?}",
        in_head
            .iter()
            .map(|e| (&e.spec_id, &e.kind))
            .collect::<Vec<_>>()
    );

    // Digest: BUG-389 marked deleted, BUG-390 marked added.
    let digest_opts = HistoryOpts {
        events_mode: false,
        ..base_opts()
    };
    let (rows, _, _) = build_digest_rows(root, &digest_opts).unwrap();
    let a = rows
        .iter()
        .find(|r| r.spec_id == "BUG-389")
        .expect("BUG-389 row");
    let b = rows
        .iter()
        .find(|r| r.spec_id == "BUG-390")
        .expect("BUG-390 row");
    assert!(a.had_delete, "digest must mark BUG-389 as deleted");
    assert!(b.had_add, "digest must mark BUG-390 as added");
}

/// A `HistoryOpts` with every filter off — tests override the one field
/// they exercise.
// trace:TASK-1055
fn base_opts() -> HistoryOpts {
    HistoryOpts {
        limit: 1000,
        max_commits: 1000,
        max_commits_explicit: false,
        events_mode: true,
        id_filter: None,
        type_filter: None,
        author_filter: None,
        since: None,
        until: None,
        status_changes_only: false,
        shipped_only: false,
        to_status: None,
        from_status: None,
        opened_only: false,
        comments_only: false,
        oneline: false,
        archived_specs: std::collections::HashSet::new(),
        archived_only_specs: None,
        deferred_specs: std::collections::HashSet::new(),
        deferred_only_specs: None,
        exclude_meta: false,
    }
}

/// BUG-1617: builds a git-canonical fixture with one spec whose status
/// flips back and forth `commit_count` times after a seed commit — each
/// flip is exactly one `StatusChange` event, so a test can dial in a
/// precise commit/event count without needing hundreds of real commits
/// to exercise a small `max_commits` window.
// trace:BUG-1617 | ai:claude
fn write_status_flip_fixture(root: &Path, commit_count: usize) {
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);

    let rel = "objects/BUG/000/BUG-1.yaml";
    let full = root.join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();

    std::fs::write(&full, "spec_id: BUG-1\ntitle: t\nstatus: Draft\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "seed"]);

    for i in 0..commit_count {
        let status = if i % 2 == 0 { "Approved" } else { "Draft" };
        std::fs::write(
            &full,
            format!("spec_id: BUG-1\ntitle: t\nstatus: {status}\n"),
        )
        .unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", &format!("flip {i}")]);
    }
}

/// BUG-1617: the commit walk gets capped at 3 commits, but 8 events were
/// requested and 10 exist — the DEFAULT window ran out first, so
/// `window_exhausted` must be true.
// trace:BUG-1617 | ai:claude
#[test]
fn collect_filtered_events_reports_window_exhausted_when_default_window_runs_out() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_status_flip_fixture(root, 10);

    let opts = HistoryOpts {
        limit: 8,
        max_commits: 3,
        max_commits_explicit: false,
        ..base_opts()
    };
    let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
    assert!(
        events.len() < 8,
        "expected fewer than the requested limit, got {}",
        events.len()
    );
    assert!(
        window_exhausted,
        "the 3-commit cap was hit before the 8-event limit — expected window_exhausted=true"
    );
}

/// BUG-1617: same fixture, but the window and the limit line up exactly
/// (3 commits, 3 requested) — the limit was MET, so even though the
/// commit walk was also capped at the same point, this is not
/// "exhausted": nothing was silently left out of the requested count.
// trace:BUG-1617 | ai:claude
#[test]
fn collect_filtered_events_no_window_exhausted_when_limit_met() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_status_flip_fixture(root, 10);

    let opts = HistoryOpts {
        limit: 3,
        max_commits: 3,
        max_commits_explicit: false,
        ..base_opts()
    };
    let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
    assert_eq!(events.len(), 3);
    assert!(
        !window_exhausted,
        "the limit was met exactly at the window edge — must not be flagged as exhausted"
    );
}

/// BUG-1617: the window is far larger than the real history (4 commits
/// total vs. a 100-commit cap) — the walk legitimately ran out of
/// commits to look at, not window capacity. Must not be flagged as
/// exhausted even though fewer than `limit` events came back.
// trace:BUG-1617 | ai:claude
#[test]
fn collect_filtered_events_not_exhausted_when_real_history_ends_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_status_flip_fixture(root, 3);

    let opts = HistoryOpts {
        limit: 10,
        max_commits: 100,
        max_commits_explicit: false,
        ..base_opts()
    };
    let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
    // 3 flip commits (StatusChange events) plus the seed commit itself
    // (an Added event — the default opts don't filter kinds) = 4.
    assert_eq!(events.len(), 4);
    assert!(
        !window_exhausted,
        "real history ran out, not the window — must not be flagged as exhausted"
    );
}

/// BUG-1617 review fix: the exact-boundary case the naive
/// `commits.len() >= max_commits` check got wrong — history that is
/// *exactly* `max_commits` commits long is a coincidence, not
/// exhaustion. 4 flips + 1 seed = 5 commits total, `max_commits: 5`, and
/// a `--limit` nowhere near met (10). Before the over-fetch-by-one fix
/// this reported `window_exhausted: true`; it must report `false` —
/// there is nothing beyond the cap to widen toward.
// trace:BUG-1617 | ai:claude
#[test]
fn collect_filtered_events_not_exhausted_when_history_is_exactly_max_commits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_status_flip_fixture(root, 4);

    let opts = HistoryOpts {
        limit: 10,
        max_commits: 5,
        max_commits_explicit: false,
        ..base_opts()
    };
    let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
    // 4 status-change flips + 1 Added (the seed) = 5, all of history.
    assert_eq!(events.len(), 5);
    assert!(
        !window_exhausted,
        "history is exactly max_commits long — that IS everything, not exhaustion"
    );
}

/// BUG-1617 review fix: the sibling of the exactly-at-cap case above —
/// history is one commit LONGER than `max_commits` (5 flips + 1 seed = 6
/// commits, `max_commits: 5`), so there genuinely is more beyond the
/// window. Only the newest `max_commits` commits should be decoded
/// (the oldest — the seed's Added event — must NOT appear), and
/// `window_exhausted` must be true.
// trace:BUG-1617 | ai:claude
#[test]
fn collect_filtered_events_exhausted_when_history_is_one_more_than_max_commits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_status_flip_fixture(root, 5);

    let opts = HistoryOpts {
        limit: 10,
        max_commits: 5,
        max_commits_explicit: false,
        ..base_opts()
    };
    let (events, _, window_exhausted) = collect_filtered_events(root, &opts).unwrap();
    // Only the 5 newest commits are decoded — all 5 are StatusChange
    // flips; the oldest (seed/Added) commit falls outside the window.
    assert_eq!(events.len(), 5);
    assert!(
        events
            .iter()
            .all(|e| matches!(e.kind, EventKind::StatusChange { .. })),
        "the oldest (seed/Added) commit must fall outside the window"
    );
    assert!(
        window_exhausted,
        "history continues one commit past the cap — expected window_exhausted=true"
    );
}

/// BUG-1617: the notice text a human sees — present, and mentions both
/// ways to widen the walk (--max-commits and a time bound), exactly when
/// the window was exhausted and the caller did NOT pin --max-commits
/// themselves.
// trace:BUG-1617 | ai:claude
#[test]
fn window_exhausted_notice_text_present_for_default_window() {
    let opts = HistoryOpts {
        limit: 8,
        max_commits: 3,
        max_commits_explicit: false,
        ..base_opts()
    };
    let msg = window_exhausted_notice_text(&opts, true, 3).expect("expected a notice");
    assert!(
        msg.contains("--max-commits"),
        "notice should mention --max-commits, got: {msg}"
    );
    assert!(
        msg.contains("--since") || msg.contains("--until"),
        "notice should mention a time-bound escape hatch, got: {msg}"
    );
}

/// BUG-1617 acceptance: no notice when the limit was met.
#[test]
fn window_exhausted_notice_text_absent_when_limit_met() {
    let opts = HistoryOpts {
        limit: 3,
        max_commits: 3,
        max_commits_explicit: false,
        ..base_opts()
    };
    assert!(window_exhausted_notice_text(&opts, false, 3).is_none());
}

/// BUG-1617 acceptance: no notice when the caller passed an explicit
/// --max-commits — they already know they narrowed the walk.
#[test]
fn window_exhausted_notice_text_absent_when_max_commits_explicit() {
    let opts = HistoryOpts {
        limit: 8,
        max_commits: 3,
        max_commits_explicit: true,
        ..base_opts()
    };
    assert!(window_exhausted_notice_text(&opts, true, 3).is_none());
}

/// BUG-424: a multibyte char straddling the truncation point must not panic
/// (raw byte-slicing did). Titles carry emoji/unicode glyphs.
#[test]
fn shorten_is_char_boundary_safe_on_emoji_title() {
    let s = "Review PR-75: apply ⇒/⏸ glyph set to skill templates (BUG-116)";
    // byte index ~59 lands inside the ⏸ codepoint — the old panic point.
    for max in [40usize, 58, 59, 60, 1] {
        let out = shorten(s, max); // must not panic at any boundary
        assert!(out.chars().count() <= max.max(1));
    }
    assert!(shorten(s, 30).ends_with('…'));
    // No truncation when it fits (by char count).
    assert_eq!(shorten("short ⏸ title", 100), "short ⏸ title");
}

#[test]
fn spec_id_from_path_extracts_stem() {
    assert_eq!(
        spec_id_from_path("objects/FR/000/FR-1-011.yaml"),
        "FR-1-011"
    );
    assert_eq!(
        spec_id_from_path("objects/EPIC/000/EPIC-1-005.yaml"),
        "EPIC-1-005"
    );
}

#[test]
fn req_type_from_path_pulls_first_segment() {
    assert_eq!(req_type_from_path("objects/FR/000/FR-1-011.yaml"), "FR");
    assert_eq!(req_type_from_path("objects/TASK/000/TASK-1.yaml"), "TASK");
}

#[test]
fn human_timestamp_converts_utc_to_local() {
    // The input is UTC; the output is in the user's local zone. We
    // can't assert a specific value here without controlling TZ, but
    // we CAN assert the parse path round-trips: parsing then
    // formatting produces the same instant regardless of zone.
    use chrono::{DateTime, FixedOffset, Local};
    let iso = "2026-05-04T18:02:38.123456Z";
    let formatted = human_timestamp(iso);
    // Re-parse our formatted local string + assert it matches the
    // expected local representation of the original UTC instant.
    let utc: DateTime<FixedOffset> = iso.parse().unwrap();
    let expected = utc
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    assert_eq!(formatted, expected);
    // Sanity: the bug we're guarding against is "shows raw UTC HH:MM
    // regardless of local zone" — so on west-of-UTC machines the
    // output should NOT contain "18:02" verbatim.
    if Local::now().offset().local_minus_utc() < 0 {
        assert!(
            !formatted.ends_with("18:02"),
            "output is still in UTC: {}",
            formatted
        );
    }
}

#[test]
fn effective_falls_back_to_custom() {
    let v: Value = serde_yaml::from_str("status: Draft\ncustom_status: Awaiting Review").unwrap();
    assert_eq!(
        effective(&v, "status", "custom_status").as_deref(),
        Some("Awaiting Review")
    );
}

/// The human (colored) line shows the authority clause after the status
/// transition, and omits it for an ungated flip.
// trace:TASK-1953 | ai:claude — human-rendering coverage for TASK-1737.
#[test]
fn format_event_body_renders_authority_clause() {
    let mk = |authority: Option<EventAuthority>| Event {
        sha: "abc".into(),
        timestamp: "2026-05-04 12:00".into(),
        author: "joe".into(),
        spec_id: "FR-1".into(),
        req_type: "FR".into(),
        kind: EventKind::StatusChange {
            from: "Approved".into(),
            to: "Planned".into(),
            authority,
        },
    };
    let gated = mk(Some(EventAuthority {
        kind: "grant".into(),
        seat: Some("advisor".into()),
        grant: Some("g-1".into()),
    }));
    assert_eq!(
        crate::pr_lookup::strip_ansi_color(&format_event_body(&gated)),
        "status: Approved → Planned (authority: grant (advisor, g-1))"
    );
    assert_eq!(
        crate::pr_lookup::strip_ansi_color(&format_event_body(&mk(None))),
        "status: Approved → Planned"
    );
}

#[test]
fn diff_status_emits_one_event() {
    let before: Value = serde_yaml::from_str("status: Draft\nspec_id: FR-1\ntitle: t").unwrap();
    let after: Value = serde_yaml::from_str("status: Approved\nspec_id: FR-1\ntitle: t").unwrap();
    let commit = CommitMeta {
        sha: "abc".into(),
        iso_timestamp: "2026-05-04T12:00:00Z".into(),
        git_author: "joe@example.com".into(),
        authority: None,
    };
    let mut out = Vec::new();
    let mk = |kind: EventKind| Event {
        sha: commit.sha.clone(),
        timestamp: human_timestamp(&commit.iso_timestamp),
        author: "joe".into(),
        spec_id: "FR-1".into(),
        req_type: "FR".into(),
        kind,
    };
    diff_modified(&before, &after, &mk, &mut out);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].kind, EventKind::StatusChange { .. }));
}

/// TASK-507: `--shipped` keeps only transitions into Completed, not other
/// status flips. BUG-1636: from any prior status, not only Done.
// trace:BUG-1636 | ai:claude
#[test]
fn is_ship_event_only_transitions_into_completed() {
    assert!(is_ship_event(&EventKind::StatusChange {
        from: "InProgress".into(),
        to: "Completed".into(),
        authority: None,
    }));
    assert!(!is_ship_event(&EventKind::StatusChange {
        from: "Completed".into(),
        to: "Completed".into(),
        authority: None,
    }));
    let ship = EventKind::StatusChange {
        from: "Done".into(),
        to: "Completed".into(),
        authority: None,
    };
    assert!(is_ship_event(&ship));
    // case-insensitive
    assert!(is_ship_event(&EventKind::StatusChange {
        from: "done".into(),
        to: "completed".into(),
        authority: None,
    }));
    // other transitions are not ships
    assert!(!is_ship_event(&EventKind::StatusChange {
        from: "Approved".into(),
        to: "Done".into(),
        authority: None,
    }));
    assert!(!is_ship_event(&EventKind::StatusChange {
        from: "InProgress".into(),
        to: "Done".into(),
        authority: None,
    }));
    assert!(!is_ship_event(&EventKind::PriorityChange {
        from: "Low".into(),
        to: "High".into(),
    }));
}

#[test]
fn event_record_exposes_structured_status_change() {
    let event = Event {
        sha: "abcdef123456".into(),
        timestamp: "2026-05-24 10:00".into(),
        author: "codex".into(),
        spec_id: "TASK-538".into(),
        req_type: "TASK".into(),
        kind: EventKind::StatusChange {
            from: "Approved".into(),
            to: "Completed".into(),
            authority: None,
        },
    };

    let record = event_record(&event);
    assert_eq!(record.kind, "status_change");
    assert_eq!(record.spec_id, "TASK-538");
    assert_eq!(record.detail["from"], "Approved");
    assert_eq!(record.detail["to"], "Completed");
    assert!(record.summary.contains("Approved -> Completed"));
}

#[test]
fn diff_tags_set_semantics() {
    let before: Value = serde_yaml::from_str("tags: [a, b]").unwrap();
    let after: Value = serde_yaml::from_str("tags: [b, c]").unwrap();
    let mut out = Vec::new();
    let commit = CommitMeta {
        sha: "abc".into(),
        iso_timestamp: "2026-05-04T12:00:00Z".into(),
        git_author: "joe@example.com".into(),
        authority: None,
    };
    let mk = |kind: EventKind| Event {
        sha: commit.sha.clone(),
        timestamp: "x".into(),
        author: "joe".into(),
        spec_id: "FR-1".into(),
        req_type: "FR".into(),
        kind,
    };
    diff_modified(&before, &after, &mk, &mut out);
    assert_eq!(out.len(), 1);
    match &out[0].kind {
        EventKind::TagsChange { added, removed } => {
            assert_eq!(added, &vec!["c".to_string()]);
            assert_eq!(removed, &vec!["a".to_string()]);
        }
        _ => panic!("expected TagsChange"),
    }
}

#[test]
fn comment_added_uses_last_author() {
    let before: Value = serde_yaml::from_str("comments: []").unwrap();
    let after: Value =
        serde_yaml::from_str("comments:\n  - author: alice\n    content: hi").unwrap();
    let mut out = Vec::new();
    let commit = CommitMeta {
        sha: "abc".into(),
        iso_timestamp: "2026-05-04T12:00:00Z".into(),
        git_author: "joe@example.com".into(),
        authority: None,
    };
    let mk = |kind: EventKind| Event {
        sha: commit.sha.clone(),
        timestamp: "x".into(),
        author: "joe".into(),
        spec_id: "FR-1".into(),
        req_type: "FR".into(),
        kind,
    };
    diff_modified(&before, &after, &mk, &mut out);
    assert_eq!(out.len(), 1);
    match &out[0].kind {
        EventKind::CommentsAdded { count, author } => {
            assert_eq!(*count, 1);
            assert_eq!(author.as_deref(), Some("alice"));
        }
        _ => panic!("expected CommentsAdded"),
    }
}

/// `--status-changes` and `--comments` each narrow to one event kind;
/// applied together they used to AND (impossible — always empty).
/// TASK-1480 combines them with OR: either kind passes.
// trace:TASK-1480 | ai:claude
#[test]
fn event_kind_allowed_combines_status_and_comments_with_or() {
    let status = EventKind::StatusChange {
        from: "Draft".into(),
        to: "Approved".into(),
        authority: None,
    };
    let comment = EventKind::CommentsAdded {
        count: 1,
        author: Some("joe".into()),
    };
    let other = EventKind::TagsChange {
        added: vec!["x".into()],
        removed: vec![],
    };

    let neither = HistoryOpts { ..base_opts() };
    assert!(event_kind_allowed(&status, &neither));
    assert!(event_kind_allowed(&comment, &neither));
    assert!(event_kind_allowed(&other, &neither));

    let status_only = HistoryOpts {
        status_changes_only: true,
        ..base_opts()
    };
    assert!(event_kind_allowed(&status, &status_only));
    assert!(!event_kind_allowed(&comment, &status_only));
    assert!(!event_kind_allowed(&other, &status_only));

    let comments_only = HistoryOpts {
        comments_only: true,
        ..base_opts()
    };
    assert!(!event_kind_allowed(&status, &comments_only));
    assert!(event_kind_allowed(&comment, &comments_only));

    // The regression case: both set at once must OR, not AND.
    let both = HistoryOpts {
        status_changes_only: true,
        comments_only: true,
        ..base_opts()
    };
    assert!(event_kind_allowed(&status, &both));
    assert!(event_kind_allowed(&comment, &both));
    assert!(!event_kind_allowed(&other, &both));
}

/// A single spec (`--id` or the positional SPEC-ID alias) renders the
/// status-progression view unless `--full`/`events` asked for the
/// complete trail. BUG-1635: the choice no longer depends on whether a
/// human or a script is reading; agent/piped callers used to get a
/// one-row digest instead of the transitions.
// trace:TASK-1480 | ai:claude
// trace:BUG-1635 | ai:claude
#[test]
fn single_spec_view_selection_ignores_the_reader() {
    let opts = HistoryOpts {
        id_filter: Some("TASK-1".to_string()),
        events_mode: false,
        ..base_opts()
    };
    assert!(single_spec_uses_progress_view(&opts));

    // `--full` (or the `events` subcommand) always wins.
    let full = HistoryOpts {
        events_mode: true,
        ..opts.clone()
    };
    assert!(!single_spec_uses_progress_view(&full));

    // No id at all never takes the single-spec branch.
    let no_id = HistoryOpts {
        id_filter: None,
        ..opts
    };
    assert!(!single_spec_uses_progress_view(&no_id));
}

/// TASK-1480: build a small history for one spec — several status
/// changes plus a comment on the same commit as one of the transitions
/// — and drive it through `collect_filtered_events` the same way
/// `aida history <SPEC-ID>` (status-progression, the default single-spec
/// view) does: `status_changes_only = true`. Asserts the status changes
/// come back (and only them — the comment is excluded), and that
/// reversing the git-log order (as `run_single_spec_progress` does)
/// produces the true chronological (oldest → newest) progression:
/// Draft → Approved → In Progress → Done.
#[test]
fn status_progression_filters_to_status_changes_in_chronological_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?} failed", args);
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    let write = |body: &str| {
        let path = root.join("objects/TASK/000/TASK-1.yaml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    };

    write("spec_id: TASK-1\ntitle: t\nstatus: Draft\ncomments: []\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add TASK-1"]);

    write("spec_id: TASK-1\ntitle: t\nstatus: Approved\ncomments: []\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "approve TASK-1"]);

    // Same commit carries a status change AND a comment.
    write(
        "spec_id: TASK-1\ntitle: t\nstatus: In Progress\ncomments:\n  - author: joe\n    content: starting\n",
    );
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "start TASK-1"]);

    write(
        "spec_id: TASK-1\ntitle: t\nstatus: Done\ncomments:\n  - author: joe\n    content: starting\n",
    );
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "finish TASK-1"]);

    let opts = HistoryOpts {
        id_filter: Some("TASK-1".to_string()),
        status_changes_only: true,
        ..base_opts()
    };
    let (mut filtered, _, _) = collect_filtered_events(root, &opts).unwrap();
    assert_eq!(
        filtered.len(),
        3,
        "expected exactly the 3 status changes, got: {:?}",
        filtered.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );
    assert!(
        filtered
            .iter()
            .all(|e| matches!(e.kind, EventKind::StatusChange { .. })),
        "the comment event must be filtered out"
    );

    // git-log order is newest-first; the progression view reverses it.
    filtered.reverse();
    let transitions: Vec<(String, String)> = filtered
        .iter()
        .map(|e| match &e.kind {
            EventKind::StatusChange { from, to, .. } => (from.clone(), to.clone()),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        transitions,
        vec![
            ("Draft".to_string(), "Approved".to_string()),
            ("Approved".to_string(), "In Progress".to_string()),
            ("In Progress".to_string(), "Done".to_string()),
        ]
    );

    // The comment-only view picks up exactly the one comment event
    // instead.
    let comments_opts = HistoryOpts {
        id_filter: Some("TASK-1".to_string()),
        comments_only: true,
        ..base_opts()
    };
    let (comment_events, _, _) = collect_filtered_events(root, &comments_opts).unwrap();
    assert_eq!(comment_events.len(), 1);
    assert!(matches!(
        comment_events[0].kind,
        EventKind::CommentsAdded { .. }
    ));

    // Both flags together (the OR-fix): status changes AND the comment.
    let both_opts = HistoryOpts {
        id_filter: Some("TASK-1".to_string()),
        status_changes_only: true,
        comments_only: true,
        ..base_opts()
    };
    let (both_events, _, _) = collect_filtered_events(root, &both_opts).unwrap();
    assert_eq!(both_events.len(), 4);
}

/// TASK-1480: `aida history <SPEC-ID>` on a real spec that simply
/// hasn't changed status yet must NOT error — it's a friendly empty
/// view, not an invalid id. Calls `run_single_spec_progress` directly
/// with an explicit output rather than the public `run()` dispatcher,
/// whose output choice depends on the ambient `agent_output_mode()`
/// (real TTY/env state).
// trace:TASK-1480 | ai:claude
#[test]
fn run_single_spec_with_no_status_changes_yet_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?} failed", args);
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    let path = root.join("objects/TASK/000/TASK-2.yaml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "spec_id: TASK-2\ntitle: quiet spec\nstatus: Draft\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add TASK-2"]);

    let opts = HistoryOpts {
        id_filter: Some("TASK-2".to_string()),
        events_mode: false,
        status_changes_only: false,
        ..base_opts()
    };
    assert!(
        run_single_spec_progress(root, &opts, HistoryOutput::Human).is_ok(),
        "a real, quiet spec must not error"
    );
}

/// TASK-1480: an id that never appears anywhere in the orphan branch's
/// history (well-formed shape, but nothing was ever committed under it)
/// gets a clear error from the single-spec view, not a silent
/// "(no recent activity)." Calls `run_single_spec_progress` directly —
/// see the doc comment on the previous test for why.
// trace:TASK-1480 | ai:claude
#[test]
fn run_single_spec_with_no_history_at_all_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?} failed", args);
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    // Some unrelated history must exist so the branch isn't itself
    // empty (an empty orphan branch is a different, store-level case).
    let path = root.join("objects/TASK/000/TASK-1.yaml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "spec_id: TASK-1\ntitle: t\nstatus: Draft\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "add TASK-1"]);

    let opts = HistoryOpts {
        id_filter: Some("TASK-999999".to_string()),
        events_mode: false,
        ..base_opts()
    };
    let err = run_single_spec_progress(root, &opts, HistoryOutput::Human).unwrap_err();
    assert!(
        format!("{err:#}").to_lowercase().contains("not found"),
        "expected a not-found error, got: {err:#}"
    );
}
