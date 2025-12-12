// trace:FR-0227 | ai:claude:high
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/generated")
        // Enable serde for JSON serialization (REST API support)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute(".", "#[serde(rename_all = \"camelCase\")]")
        .compile_protos(&["../proto/aida.proto"], &["../proto"])?;
    Ok(())
}
