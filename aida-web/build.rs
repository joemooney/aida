// trace:FR-0273 | ai:claude:high
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false) // Client only for WASM
        .build_client(true)
        .build_transport(false) // No default transport - using tonic-web-wasm-client
        .out_dir("src/generated")
        .compile_protos(&["../proto/aida.proto"], &["../proto"])?;
    Ok(())
}
