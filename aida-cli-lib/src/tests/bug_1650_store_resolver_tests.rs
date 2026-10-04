//! BUG-1650: `detect_store_path` tries every `store_path` candidate, so a
//! pre-BUG-1649 config with raw backslashes still finds its store.
// trace:BUG-1650 | ai:claude

use super::detect_store_path;
use std::path::Path;

fn write_config(dir: &Path, body: &str) {
    std::fs::create_dir_all(dir.join(".aida")).unwrap();
    std::fs::write(dir.join(".aida/config.toml"), body).unwrap();
}

fn git_init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(out.status.success(), "git init {}", dir.display());
}

/// `..\team-store` parses as valid TOML with `\t` applied as an escape; only
/// the raw legacy candidate names the real store, and it must win.
#[test]
fn bug_1650_detect_store_path_uses_the_legacy_backslash_candidate() {
    for raw in ["..\\team-store", "stores\\new"] {
        let tmp = tempfile::tempdir().unwrap();
        // An enclosing project with its own store: never the answer here.
        let outer = tmp.path().join("outer");
        write_config(&outer, "[deployment]\nstore_path = \".outer\"\n");
        git_init(&outer.join(".outer"));

        let repo = outer.join("proj");
        // Deliberately unescaped, as a pre-BUG-1649 writer produced it.
        let body = ["[deployment]\nstore_path = \"", raw, "\"\n"].concat();
        assert!(
            toml::from_str::<toml::Table>(&body).is_ok(),
            "{body:?} parses"
        );
        write_config(&repo, &body);
        // On Unix one file name containing `\`; on Windows a relative path.
        let legacy_store = repo.join(raw);
        git_init(&legacy_store);

        assert_eq!(detect_store_path(&repo), Some(legacy_store), "{raw:?}");
    }
}
