// trace:TASK-3816 | ai:codex
use super::*;

#[test]
fn task_3816_inventory_chain_rejections_order_and_parity() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = crate::gate::test_support::AmbientGuard::hermetic_with_seat_and(
        root,
        "advisor",
        &["reviewer"],
        &[("AIDA_USER", Some("delegated-reader"))],
    );
    let principal = crate::gate::test_support::HERMETIC_TEST_USER;
    let reader_id = current_grant_id().unwrap();
    let mut reader = read_grant(&reader_id).unwrap();
    reader.expires_at = Utc::now() - Duration::seconds(1);
    write_grant(&reader).unwrap();
    let parent = test_support::mint_grant_for(root, principal, "advisor", &["reviewer"]);
    let foreign = test_support::mint_grant_for(root, "another-operator", "advisor", &[]);
    let inline = test_support::mint_grant_for(root, principal, "advisor", &[]);
    let mut inline_record = read_grant(&inline).unwrap();
    inline_record.origin = Some(INLINE_ORIGIN.into());
    write_grant(&inline_record).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n[seat]\nmax_grant_hours = 12\n",
    )
    .unwrap();
    let now = Utc::now();
    let mut ids = Vec::new();
    for case in 0..4 {
        let id = test_support::mint_grant_for(root, principal, "reviewer", &[]);
        let mut g = read_grant(&id).unwrap();
        g.parent_grant_id = Some(parent.clone());
        g.expires_at = now + Duration::minutes(case + 1);
        match case {
            1 => g.parent_grant_id = Some(Uuid::new_v4().to_string()),
            2 => g.revoked_at = Some(now),
            3 => g.expires_at = now - Duration::seconds(1),
            _ => (),
        }
        write_grant(&g).unwrap();
        ids.push(id);
    }
    let before = std::fs::read(grant_path(&ids[0]).unwrap()).unwrap();
    let report = grant_inventory(root, principal, now);
    assert!(!report
        .grants
        .iter()
        .any(|g| g.id == foreign || g.id == inline));
    assert!(report
        .grants
        .iter()
        .all(|g| g.effective_cap_hours == 12 && g.config_origin.contains("project")));
    let find = |id: &str| report.grants.iter().find(|g| g.id == id).unwrap();
    assert!(find(&ids[0]).valid);
    assert!(find(&ids[1])
        .rejection
        .as_deref()
        .unwrap()
        .contains("delegated from"));
    assert!(find(&ids[2])
        .rejection
        .as_deref()
        .unwrap()
        .contains("revoked"));
    assert!(find(&ids[3])
        .rejection
        .as_deref()
        .unwrap()
        .contains("expired"));
    assert!(report
        .grants
        .windows(2)
        .all(|w| w[0].expires_at <= w[1].expires_at));
    let json = serde_json::to_value(&report).unwrap();
    let text = report.text();
    for (g, j) in report.grants.iter().zip(json["grants"].as_array().unwrap()) {
        assert_eq!(j["id"], g.id);
        assert_eq!(j["role"], g.role);
        assert_eq!(j["subject"], g.subject);
        assert_eq!(j["valid"], g.valid);
        assert_eq!(j["effective_cap_hours"], g.effective_cap_hours);
        assert_eq!(j["holder_pids"], serde_json::json!(g.holder_pids));
        assert_eq!(j["parent"], serde_json::json!(g.parent));
        assert_eq!(j["rejection"], serde_json::json!(g.rejection));
        assert!(text.contains(&g.expires_at.to_rfc3339()));
        assert!(text.contains(&format!("holder_pids={:?}", g.holder_pids)));
        assert!(text.contains(g.parent.as_deref().unwrap_or("none")));
        for value in [&g.id, &g.role, &g.subject, &g.config_origin] {
            assert!(text.contains(value));
        }
        assert!(text.contains(&format!("cap={}h", g.effective_cap_hours)));
        if let Some(reason) = &g.rejection {
            assert!(text.contains(reason));
        }
    }
    assert_eq!(before, std::fs::read(grant_path(&ids[0]).unwrap()).unwrap());
    let mut parent_record = read_grant(&parent).unwrap();
    parent_record.revoked_at = Some(now);
    write_grant(&parent_record).unwrap();
    let report = grant_inventory(root, principal, now);
    let child = report.grants.iter().find(|g| g.id == ids[0]).unwrap();
    assert!(!child.valid);
    assert!(child.rejection.as_deref().unwrap().contains("revoked"));
    parent_record.revoked_at = None;
    parent_record.expires_at = now;
    write_grant(&parent_record).unwrap();
    let report = grant_inventory(root, principal, now);
    let child = report.grants.iter().find(|g| g.id == ids[0]).unwrap();
    assert!(!child.valid);
    assert!(child.rejection.as_deref().unwrap().contains("expired"));
}
