// trace:BUG-b6b1278890774ef993d2ebc327862470-508.AC1 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-508.AC2 | ai:codex

#[cfg(target_os = "linux")]
#[test]
fn bug_2747_release_checks_real_ancestor() {
    use std::{fs, os::unix::fs::DirBuilderExt, process::Command};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join(".aida")).unwrap();
    fs::write(root.join(".aida/config.toml"), "").unwrap();
    let key_input = root.join("project-path");
    fs::write(&key_input, root.as_os_str().as_encoded_bytes()).unwrap();
    let hash = Command::new("sha256sum").arg(key_input).output().unwrap();
    assert!(hash.status.success());
    let digest = String::from_utf8(hash.stdout).unwrap();
    let dir = root.join(".aida/launch-attempts").join(&digest[..24]);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir.join("attempts"))
        .unwrap();
    let attempt = uuid::Uuid::new_v4().to_string();
    let path = dir.join("attempts").join(format!("{attempt}.json"));
    let start = aida_core::liveness::process_start_identity(std::process::id()).unwrap();
    let known = serde_json::json!({
        "version":1,"attempt":attempt,"project_root":root,"scope":{"role":"implementer","spec":"BUG-2747"},
        "phase":"prepared","owner":{"kind":"dispatcher","pid":u32::MAX,"start":start},
        "generation":0,"agent_name":"fixture","worktree":root.join("worker"),"log_path":root.join("log"),"updated_at":""
    });
    let aida = env!("CARGO_BIN_EXE_aida");
    for retire in [false, true] {
        let initial = if retire {
            serde_json::json!({"owner": known["owner"]}).to_string()
        } else {
            known.to_string()
        };
        fs::write(&path, &initial).unwrap();
        for held in [true, false] {
            let mut cmd = if held {
                let mut sh = Command::new("sh");
                sh.args(["-c", r#""$0" "$@"; exit $?"#, aida]);
                sh
            } else {
                Command::new(aida)
            };
            cmd.args(["agent", "recover-launch", &attempt]);
            if retire {
                cmd.arg("--retire-unknown");
            }
            let out = cmd
                .env_clear()
                .env("HOME", &root)
                .env("AIDA_HOME", &root)
                .env("PATH", "/usr/bin:/bin")
                .current_dir(&root)
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned();
            assert_eq!(out.status.success(), !held, "{err}");
            if held {
                assert!(
                    err.contains(if retire {
                        "cannot be proven dead"
                    } else {
                        "names attempt"
                    }),
                    "{err}"
                );
                assert_eq!(fs::read_to_string(&path).unwrap(), initial);
            } else if retire {
                assert!(!path.exists());
            } else {
                let r: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                assert_eq!(r["phase"], "exited");
            }
        }
    }
}
