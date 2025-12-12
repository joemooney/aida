// trace:FR-0227 | ai:claude:high
//! Build script for aida-gui - generates gRPC client code when remote feature is enabled

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "remote")]
    {
        // Compile proto for client
        // build_transport(false) ensures generated code doesn't require tonic::transport
        // which is not available in WASM builds
        tonic_build::configure()
            .build_server(false)
            .build_client(true)
            .build_transport(false)
            .out_dir("src/generated")
            .compile_protos(&["../proto/aida.proto"], &["../proto"])?;
    }
    Ok(())
}
