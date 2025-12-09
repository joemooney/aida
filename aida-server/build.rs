// trace:FR-0227 | ai:claude:high
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/generated")
        .compile_protos(&["../proto/aida.proto"], &["../proto"])?;
    Ok(())
}
