// trace:FR-0227 | ai:claude:high
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse the protos with protox (pure Rust) instead of shelling out to the
    // external `protoc` binary; tonic-build still generates the Rust code from
    // the resulting FileDescriptorSet via compile_fds. trace:FR-0227 | ai:claude
    println!("cargo:rerun-if-changed=../proto/aida.proto");
    let fds = protox::compile(["../proto/aida.proto"], ["../proto"])?;
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/generated")
        // Enable serde for JSON serialization (REST API support)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute(".", "#[serde(rename_all = \"camelCase\")]")
        .compile_fds(fds)?;
    Ok(())
}
