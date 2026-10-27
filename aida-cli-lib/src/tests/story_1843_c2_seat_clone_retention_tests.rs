//! STORY-1843 C2 (rework F4, F5 f): a seat clone's mail is retired into its
//! main checkout's anchor through the verified host binding. A refusal takes
//! the same recovery-ownership ending as a worktree, a clone without a
//! verified binding is never destroyed, and `destroy_retained` reopens a
//! retirement whenever it keeps the clone.
//! trace:STORY-1843 | ai:claude

use super::s4_host_git_tests::leased_clone;
use super::tests::fixture;
use super::*;

fn put_mail(clone: &Path) {
    let mailbox = clone.join(".aida").join("mailbox");
    std::fs::create_dir_all(&mailbox).unwrap();
    std::fs::write(mailbox.join("m.json"), "{}").unwrap();
}

fn has_mail(clone: &Path) -> bool {
    clone.join(".aida").join("mailbox").join("m.json").is_file()
}

/// Record an activation under `main`'s anchor, as an operator would.
fn activate(main: &Path) {
    let anchor = main.join(".git").join("aida").join("mail-lifecycle");
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
        .create(&anchor)
        .unwrap();
    // Only the source build that recorded an activation honours it: this
    // test binary's, named as `main_entry` names it.
    crate::upgrade_release::register_source_build();
    let build = aida_core::mailbox_lifecycle::source_build().expect("a git build");
    std::fs::write(
        anchor.join("activation.json"),
        format!(
            r#"{{"schema":1,"all_writer_hosts_admitting":true,"recorded_at":"2026-10-27T00:00:00Z","source_build":"{build}"}}"#
        ),
    )
    .unwrap();
}

#[test]
fn a_refused_clone_cleanup_takes_the_ending_retained_exit_and_keeps_the_clone() {
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let fx = fixture();
    let clone = leased_clone(&fx, "c2-kept", "c0ffee184301");
    put_mail(&clone);
    let lease = crate::list_leases(&fx.main)
        .into_iter()
        .find(|l| l.id == "c0ffee184301")
        .unwrap();

    assert!(!crate::leases::force_cleanup_lease(&fx.main, &lease));
    let after = crate::list_leases(&fx.main)
        .into_iter()
        .find(|l| l.id == "c0ffee184301")
        .expect("the lease is kept");
    assert!(crate::leases::is_ending_retained(&after), "F4: same ending");
    assert!(clone.exists() && has_mail(&clone));
}

#[test]
fn a_clone_without_a_verified_binding_is_never_destroyed() {
    let fx = fixture();
    let clone = leased_clone(&fx, "c2-unbound", "c0ffee184302");
    std::fs::create_dir_all(clone.join(".aida").join("mailbox")).unwrap();
    std::fs::remove_file(crate::lease_path(&fx.main, "c0ffee184302")).unwrap();
    // Empty mailbox, no binding: still refused (C SKETCH section 5, F3).
    let refusal = crate::mail_retention::retire_tree(&fx.main, &clone, true, "test").unwrap_err();
    assert!(
        refusal.reason.contains("verified host binding"),
        "{refusal}"
    );
}

#[test]
fn destroy_retained_reopens_a_retirement_when_it_keeps_the_clone() {
    let fx = fixture();
    let clone = leased_clone(&fx, "c2-reopen", "c0ffee184303");
    activate(&fx.main);
    put_mail(&clone);
    let retirement = crate::mail_retention::retire_tree(&fx.main, &clone, true, "test")
        .unwrap()
        .expect("the bound clone retires into main's anchor");
    assert!(!has_mail(&clone), "the mail moved to the anchor");
    assert!(retirement
        .retained_dir()
        .starts_with(fx.main.join(".git").join("aida")));

    // A HEAD that does not match keeps the clone (or the probe cannot run in
    // this sandbox); either way nothing was removed, so the retirement must
    // be reopened and the mail copied back.
    let outcome = destroy_retained(
        &fx.main,
        &clone,
        DestroyMode::KeepUnshipped,
        Some("0000000000000000000000000000000000000000"),
        &[],
        Some(retirement),
    );
    assert!(
        !matches!(outcome, Ok(Destroyed::Removed { .. })),
        "{outcome:?}"
    );
    assert!(clone.exists());
    assert!(has_mail(&clone), "the kept clone's mail was reopened");
}
