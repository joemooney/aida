// trace:FR-0227 | ai:claude:high
//! Build script for aida-cli — compiles gRPC client code when the remote
//! feature is enabled, and stamps every build with build time + git SHA so
//! `aida --version` can show "0.4.0 (built 2026-05-03T01:23:45Z, sha abc1234)".
//! Lets `aida upgrade` distinguish two binaries at the same version number.

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

    // Git SHA, dirty flag and their rerun triggers (shared with aida-server).
    emit_git_stamps();

    Ok(())
}

include!("build_git_stamp.rs");
