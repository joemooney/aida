//! Signed constraint schema and no-execution boundaries.
//! trace:STORY-1673 | ai:codex
use super::*;
use crate::{DatabaseBackend, GitBackend, RequirementsStore, SqliteBackend, Storage};
use serde_json::json;

fn block(probe: &str, args: Value, op: &str, value: Value) -> Value {
    json!({"version": 1, "probe": probe, "args": args, "expect": {"op": op, "value": value}})
}
fn row(raw: Value) -> Requirement {
    let mut req = Requirement::new("host fact".into(), "fixture".into());
    req.spec_id = Some("CNST-1".into());
    req.req_type = RequirementType::Constraint;
    req.verify = Some(raw);
    req
}
fn host(root: &Path) -> LinuxHost {
    LinuxHost {
        proc_root: root.join("proc"),
        sys_root: root.join("sys"),
        path: Some(root.join("bin").into_os_string()),
    }
}
fn status(host: &LinuxHost, raw: Value) -> ConstraintResult {
    report_with(&[row(raw)], host).pop().unwrap()
}

#[test]
fn story1673_closed_schema_validates_all_six_probes_and_exact_types() {
    for (probe, args, value) in [
        ("disk-rotational", json!({"path": "/dev/sda"}), json!(true)),
        ("cpu-count", json!({}), json!(6)),
        ("mem-total-bytes", json!({}), json!(4294967296u64)),
        ("psi-available", json!({}), json!(true)),
        ("path-exists", json!({"path": "/a path/file"}), json!(false)),
        (
            "command-on-path",
            json!({"name": "aida-cli_1.0"}),
            json!(true),
        ),
    ] {
        assert!(
            Verify::parse(&block(probe, args.clone(), "eq", value.clone())).is_ok(),
            "{probe}"
        );
        let numeric = matches!(probe, "cpu-count" | "mem-total-bytes");
        for op in ["ge", "le", "exists"] {
            assert_eq!(
                Verify::parse(&block(probe, args.clone(), op, value.clone())).is_ok(),
                if op == "exists" { !numeric } else { numeric },
                "{probe} {op}"
            );
        }
        for bad in [
            Value::Null,
            json!("true"),
            json!("4 GiB"),
            json!([]),
            json!({}),
            json!(1.0),
        ] {
            assert!(Verify::parse(&block(probe, args.clone(), "eq", bad)).is_err());
        }
        let wrong_type = if numeric { json!(true) } else { json!(1) };
        assert!(Verify::parse(&block(probe, args, "eq", wrong_type)).is_err());
    }
    let mut raw = block("cpu-count", json!({}), "eq", json!(6));
    for version in [json!(0), json!(2), json!("1"), json!(1.0), json!(-1)] {
        raw["version"] = version;
        assert!(Verify::parse(&raw).is_err());
    }
    for raw in [
        Value::Null,
        json!("echo bad"),
        json!([]),
        block("shell", json!({"command": "echo bad"}), "eq", json!(true)),
        block("cpu-count", json!({"extra": 1}), "eq", json!(1)),
        block("path-exists", json!({}), "eq", json!(true)),
        block("path-exists", json!({"path": 1}), "eq", json!(true)),
        block(
            "path-exists",
            json!({"path": "/tmp", "extra": false}),
            "eq",
            json!(true),
        ),
        block("cpu-count", json!({}), "ne", json!(1)),
        block("cpu-count", json!({}), "eq", json!(true)),
    ] {
        assert!(Verify::parse(&raw).is_err(), "{raw}");
    }
    let good = block("cpu-count", json!({}), "eq", json!(1));
    for key in ["version", "probe", "args", "expect"] {
        let mut raw = good.clone();
        raw.as_object_mut().unwrap().remove(key);
        assert!(Verify::parse(&raw).is_err());
    }
    let mut raw = good.clone();
    raw["command"] = json!("echo bad");
    assert!(Verify::parse(&raw).is_err());
    let mut raw = good;
    raw["expect"]["extra"] = json!(1);
    assert!(Verify::parse(&raw).is_err());
}

#[test]
fn story1673_shell_looking_arguments_are_invalid_before_any_host_read() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path()); // no procfs files: a lookup would be unknown
    for name in [
        "sh -c touch",
        "a/b",
        "$(touch sentinel)",
        "x;touch",
        "`id`",
        "",
        "a\nb",
        "é",
    ] {
        assert_eq!(
            status(
                &host,
                block("command-on-path", json!({"name": name}), "eq", json!(true))
            )
            .status,
            Status::Invalid
        );
    }
    for path in [
        "relative",
        "/tmp/../etc",
        "/tmp/$(touch sentinel)",
        "/tmp/x;touch sentinel",
        "/tmp/`id`",
        "/tmp/a|b",
        "/tmp/a\nb",
    ] {
        for probe in ["path-exists", "disk-rotational"] {
            assert_eq!(
                status(
                    &host,
                    block(probe, json!({"path": path}), "eq", json!(true))
                )
                .status,
                Status::Invalid
            );
        }
    }
    assert_eq!(
        status(
            &host,
            block(
                "cpu-count",
                json!({"command": "touch sentinel"}),
                "eq",
                json!(1)
            )
        )
        .status,
        Status::Invalid
    );
    assert!(!dir.path().join("sentinel").exists());
}

#[test]
fn story1673_proc_fixtures_match_drift_unknown_and_integer_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    std::fs::create_dir_all(&host.proc_root).unwrap();
    assert_eq!(
        status(&host, block("cpu-count", json!({}), "eq", json!(6))).status,
        Status::Unknown
    );
    std::fs::write(
        host.proc_root.join("stat"),
        "cpu 9 8 7\ncpu0 1 2 3\ncpu1 4 5 6\nintr 4\ncpuBogus 1\n",
    )
    .unwrap();
    for (op, expected, outcome) in [
        ("eq", 2, Status::Match),
        ("eq", 6, Status::Drift),
        ("ge", 1, Status::Match),
        ("ge", 3, Status::Drift),
        ("le", 3, Status::Match),
        ("le", 1, Status::Drift),
        ("ge", -1, Status::Match),
    ] {
        let result = status(&host, block("cpu-count", json!({}), op, json!(expected)));
        assert_eq!(result.status, outcome);
        assert_eq!(result.observed, Some(json!(2)));
    }
    std::fs::write(
        host.proc_root.join("meminfo"),
        "MemTotal: 4194304 kB\nMemFree: 32 kB\n",
    )
    .unwrap();
    assert_eq!(
        status(
            &host,
            block("mem-total-bytes", json!({}), "eq", json!(4294967296u64))
        )
        .status,
        Status::Match
    );
    for text in [
        "MemTotal: 4 GB\n",
        "MemTotal: 18446744073709551615 kB\n",
        "MemFree: 1 kB\n",
        "MemTotal: 1 kB\nMemTotal: 2 kB\n",
    ] {
        std::fs::write(host.proc_root.join("meminfo"), text).unwrap();
        assert_eq!(
            status(&host, block("mem-total-bytes", json!({}), "eq", json!(1))).status,
            Status::Unknown
        );
    }
    let verify =
        Verify::parse(&block("mem-total-bytes", json!({}), "ge", json!(u64::MAX))).unwrap();
    assert!(verify.matches(&json!(u64::MAX)));
    assert!(!verify.matches(&json!(i64::MAX)));
    let verify = Verify::parse(&block("cpu-count", json!({}), "le", json!(-1))).unwrap();
    assert!(!verify.matches(&json!(0)));
    assert_eq!(
        status(
            &host,
            block(
                "disk-rotational",
                json!({"path": "/dev/null"}),
                "eq",
                json!(true)
            )
        )
        .status,
        Status::Unknown
    );
}

#[test]
fn story1673_path_psi_and_executable_lookup_never_execute() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    let fixture = dir.path().join("present");
    std::fs::write(&fixture, "data").unwrap();
    for (path, expected) in [(&fixture, true), (&dir.path().join("absent"), false)] {
        assert_eq!(
            status(
                &host,
                block(
                    "path-exists",
                    json!({"path": path}),
                    "exists",
                    json!(expected)
                )
            )
            .status,
            Status::Match
        );
    }
    assert_eq!(
        status(&host, block("psi-available", json!({}), "eq", json!(false))).status,
        Status::Match
    );
    std::fs::create_dir_all(host.proc_root.join("pressure")).unwrap();
    for name in ["cpu", "memory", "io"] {
        std::fs::write(
            host.proc_root.join("pressure").join(name),
            "some avg10=0.00\n",
        )
        .unwrap();
    }
    assert_eq!(
        status(&host, block("psi-available", json!({}), "eq", json!(true))).status,
        Status::Match
    );
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let sentinel = dir.path().join("EXECUTED");
    let executable = bin.join("sentinel");
    std::fs::write(
        &executable,
        format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        status(
            &host,
            block(
                "command-on-path",
                json!({"name": "sentinel"}),
                "eq",
                json!(true)
            )
        )
        .status,
        Status::Match
    );
    assert!(!sentinel.exists(), "PATH probe executed fixture");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        status(
            &host,
            block(
                "command-on-path",
                json!({"name": "sentinel"}),
                "eq",
                json!(false)
            )
        )
        .status,
        Status::Match
    );
    let no_path = LinuxHost { path: None, ..host };
    assert_eq!(
        status(
            &no_path,
            block(
                "command-on-path",
                json!({"name": "sentinel"}),
                "eq",
                json!(false)
            )
        )
        .status,
        Status::Unknown
    );
}

#[test]
fn story1673_absent_null_valid_invalid_survive_yaml_json_and_canonical_store() {
    let dir = tempfile::tempdir().unwrap();
    for (index, raw) in [
        None,
        Some(Value::Null),
        Some(json!("malformed")),
        Some(block("cpu-count", json!({}), "ge", json!(1))),
    ]
    .into_iter()
    .enumerate()
    {
        let mut req = row(Value::Null);
        req.verify = raw.clone();
        let yaml = serde_yaml::to_string(&req).unwrap();
        let decoded: Requirement = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(decoded.verify, raw);
        let json = serde_json::to_string(&req).unwrap();
        let decoded: Requirement = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.verify, raw);
        let mut store = RequirementsStore::new();
        store.requirements.push(req.clone());
        let storage = Storage::new(dir.path().join(format!("{index}.yaml")));
        storage.save(&store).unwrap();
        assert_eq!(storage.load().unwrap().requirements[0].verify, raw);
        let backend = GitBackend::new(&dir.path().join(format!("git-{index}"))).unwrap();
        backend.save(&store).unwrap();
        assert_eq!(backend.load().unwrap().requirements[0].verify, raw);
        let before = serde_yaml::to_string(&backend.load().unwrap().requirements[0]).unwrap();
        let rows = verify_constraints(&backend.load().unwrap().requirements);
        assert_eq!(rows.len(), usize::from(raw.is_some()));
        let after = serde_yaml::to_string(&backend.load().unwrap().requirements[0]).unwrap();
        assert_eq!(before, after, "verification mutated row");
    }
}

#[test]
fn story1673_only_configured_constraints_are_selected() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    let mut ordinary = row(Value::Null);
    ordinary.req_type = RequirementType::Task;
    let mut legacy = row(Value::Null);
    legacy.verify = None;
    let invalid = row(Value::Null);
    let rows = report_with(&[ordinary, legacy, invalid], &host);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, Status::Invalid);
}

#[test]
fn story1673_archived_sqlite_refuses_loss_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let backend = SqliteBackend::new(dir.path().join("legacy.db")).unwrap();
    let mut store = RequirementsStore::new();
    let mut req = row(Value::Null);
    req.verify = None;
    store.requirements.push(req.clone());
    backend.save(&store).unwrap();
    req.verify = Some(Value::Null);
    req.title = "must not overwrite".into();
    assert!(backend
        .update_requirement(&req)
        .unwrap_err()
        .to_string()
        .contains("canonical/YAML"));
    store.requirements[0] = req;
    assert!(backend.save(&store).is_err());
    assert_eq!(backend.load().unwrap().requirements[0].title, "host fact");
    assert!(backend
        .update_atomically(|s| s.requirements[0].verify = Some(Value::Null))
        .is_err());
    assert_eq!(backend.load().unwrap().requirements[0].title, "host fact");
    let clean = backend.load().unwrap();
    backend.save(&clean).unwrap();
}

#[test]
fn story1673_sync_merge_preserves_one_sided_add_edit_remove_and_converges() {
    let first = block("cpu-count", json!({}), "eq", json!(1));
    let second = block("cpu-count", json!({}), "eq", json!(2));
    for (before, after) in [
        (None, Some(first.clone())),
        (Some(first.clone()), Some(second.clone())),
        (Some(first.clone()), None),
        (Some(Value::Null), None),
    ] {
        let mut base = row(Value::Null);
        base.verify = before;
        let mut ours = base.clone();
        ours.verify = after.clone();
        let mut theirs = base.clone();
        theirs.title = "concurrent title".into();
        theirs.modified_at = base.modified_at + chrono::Duration::seconds(2);
        for merged in [
            crate::conflict::merge_spec_three_way(&base, &ours, &theirs),
            crate::conflict::merge_spec_three_way(&base, &theirs, &ours),
        ] {
            assert_eq!(merged.verify, after);
            assert_eq!(merged.title, "concurrent title");
        }
    }
    let base = row(Value::Null);
    let mut ours = base.clone();
    ours.verify = Some(first);
    let mut theirs = base.clone();
    theirs.verify = Some(second.clone());
    theirs.modified_at += chrono::Duration::seconds(2);
    assert_eq!(
        crate::conflict::merge_spec_three_way(&base, &ours, &theirs).verify,
        Some(second.clone())
    );
    assert_eq!(
        crate::conflict::merge_spec_three_way(&base, &theirs, &ours).verify,
        Some(second)
    );
}

#[test]
fn story1673_command_lookup_checks_effective_user_access_not_any_execute_bit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let executable = bin.join("owner_denied");
    std::fs::write(&executable, "#!/bin/sh\nexit 99\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o001)).unwrap();
    let root = unsafe { libc::geteuid() } == 0;
    assert_eq!(
        status(
            &host,
            block(
                "command-on-path",
                json!({"name": "owner_denied"}),
                "eq",
                json!(root)
            )
        )
        .status,
        Status::Match
    );
}

#[test]
fn story1673_sysfs_disk_partition_rotational_states_and_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(dir.path());
    let disk = dir.path().join("sys/devices/block/sda");
    let partition = disk.join("sda1");
    std::fs::create_dir_all(disk.join("queue")).unwrap();
    std::fs::create_dir_all(&partition).unwrap();
    std::fs::write(partition.join("partition"), "1\n").unwrap();
    for (text, expected) in [("0\n", false), ("1\n", true)] {
        std::fs::write(disk.join("queue/rotational"), text).unwrap();
        assert_eq!(host.rotational_from_sysfs(&disk).unwrap(), expected);
        assert_eq!(host.rotational_from_sysfs(&partition).unwrap(), expected);
    }
    std::fs::write(disk.join("queue/rotational"), "unknown\n").unwrap();
    assert!(host.rotational_from_sysfs(&disk).is_err());
    std::fs::remove_file(disk.join("queue/rotational")).unwrap();
    assert!(host.rotational_from_sysfs(&disk).is_err());
}

#[test]
fn story1673_tree_export_import_preserves_valid_and_invalid_blocks() {
    for raw in [
        Value::Null,
        json!("bad block"),
        block("cpu-count", json!({}), "ge", json!(1)),
    ] {
        let mut source = RequirementsStore::new();
        let req = row(raw.clone());
        source.requirements.push(req);
        let exported = crate::export::export_tree(&source, "CNST-1").unwrap();
        let serialized = serde_json::to_string(&exported).unwrap();
        let exported = serde_json::from_str(&serialized).unwrap();
        let mut destination = RequirementsStore::new();
        crate::export::import_tree(
            &mut destination,
            exported,
            crate::export::TreeImportOptions::default(),
        )
        .unwrap();
        assert_eq!(destination.requirements[0].verify, Some(raw));
    }
}

#[test]
fn story1673_bad_yaml_args_are_invalid_data_instead_of_dropped_constraints() {
    for bad in ["{version: 1, probe: path-exists, args: {path: !custom foo}, expect: {op: eq, value: true}}", "{version: 1, probe: path-exists, args: {? [a,b]: foo}, expect: {op: eq, value: true}}", "{version: 1, probe: cpu-count, args: {}, expect: {op: eq, value: .inf}}"] {
        let mut yaml = serde_yaml::to_value(row(Value::Null)).unwrap();
        yaml["verify"] = serde_yaml::from_str(bad).unwrap();
        let req: Requirement = serde_yaml::from_value(yaml).unwrap();
        assert!(req.verify.as_ref().unwrap().get("invalid_yaml").is_some());
        assert_eq!(verify_constraints(&[req.clone()])[0].status, Status::Invalid);
        let retained: Requirement = serde_yaml::from_str(&serde_yaml::to_string(&req).unwrap()).unwrap();
        assert_eq!(retained.verify, req.verify);
        serde_json::to_string(&retained).unwrap();
    }
}
