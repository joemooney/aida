// trace:TASK-3780 | ai:codex
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::os::unix::fs::PermissionsExt;

#[test]
fn bug_2486_doctor_reports_lifetime_without_credential_material() {
    let home = tempfile::tempdir().unwrap();
    let exp = chrono::Utc::now().timestamp() + 36000;
    let access = format!(
        "e30.{}.secret-signature",
        URL_SAFE_NO_PAD.encode(format!("{{\"exp\":{exp}}}"))
    );
    let value = serde_json::json!({"auth_mode":"chatgpt", "tokens":{
            "access_token":access,"refresh_token":"secret-refresh"}});
    let path = home.path().join("auth.json");
    std::fs::write(&path, value.to_string()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let env = aida_core::exec_context::Environment::from_pairs([(
        "CODEX_HOME".into(),
        home.path().as_os_str().into(),
    )]);
    assert!(codex_token_lifetime(&env, Some("git-hooks"))
        .unwrap()
        .is_none());
    assert!(codex_token_lifetime(&env, Some("permission-posture"))
        .unwrap()
        .is_some());
    let lifetime = codex_token_lifetime(&env, None).unwrap().unwrap();
    assert!(lifetime.contains("remaining") && lifetime.contains(&exp.to_string()));
    let mut report = DoctorReport::default();
    report.codex_token_lifetime = Some(lifetime);
    for rendered in [
        serde_json::to_string(&report).unwrap(),
        render_doctor_report_toon(&report),
    ] {
        assert!(rendered.contains("codex_token_lifetime"));
        assert!(!rendered.contains("secret-") && !rendered.contains(&access));
    }
    let expired = serde_json::json!({"auth_mode":"chatgpt", "tokens":{"access_token":format!("e30.{}.secret-signature",
            URL_SAFE_NO_PAD.encode("{\"exp\":1}"))}});
    std::fs::write(&path, expired.to_string()).unwrap();
    let expired = codex_token_lifetime(&env, None).unwrap().unwrap();
    assert!(expired.contains("expired"));
    assert!(!expired.contains("remaining"));
    assert!(!expired.contains("secret-"));
    std::fs::write(&path, "secret-malformed-credential").unwrap();
    let invalid = codex_token_lifetime(&env, None).unwrap().unwrap();
    assert!(invalid.contains("unavailable") && invalid.contains("codex login"));
    assert!(!invalid.contains("secret-"));
}
