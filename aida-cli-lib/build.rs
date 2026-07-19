// trace:FR-0227 | ai:claude:high
//! Build script for aida-cli — compiles gRPC client code when the remote
//! feature is enabled, and stamps every build with build time + git SHA so
//! `aida --version` can show "0.4.0 (built 2026-05-03T01:23:45Z, sha abc1234)".
//! Lets `aida upgrade` distinguish two binaries at the same version number.

use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "remote")]
    {
        // Compile proto for client with protox (pure Rust) — no external `protoc`
        // binary; tonic-build generates the client from the descriptor set.
        // trace:FR-0227 | ai:claude
        println!("cargo:rerun-if-changed=../proto/aida.proto");
        let fds = protox::compile(["../proto/aida.proto"], ["../proto"])?;
        tonic_build::configure()
            .build_server(false)
            .build_client(true)
            .out_dir("src/generated")
            .compile_fds(fds)?;
    }

    // ---- Build-time stamps (EPIC-1-001) -------------------------------------

    // Unix epoch seconds at build time. Formatted at runtime to keep the
    // build-script deps minimal (no chrono in build.rs).
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=AIDA_BUILD_UNIX_TIME={}", now_secs);

    // Short git SHA, or "unknown" if we can't run git or aren't in a repo.
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AIDA_BUILD_GIT_SHA={}", sha);

    // Mark dirty if working tree has uncommitted changes (best-effort).
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    println!(
        "cargo:rustc-env=AIDA_BUILD_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );

    // Re-run the build script when the git HEAD or the index changes so the
    // SHA and dirty flag stay accurate. (Pure timestamp updates won't trigger
    // a rebuild on their own, which is fine — `cargo build` after a code
    // change will pick up the new timestamp; pristine builds get cached.)
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");

    Ok(())
}
