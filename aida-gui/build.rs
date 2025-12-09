// trace:FR-0227 | ai:claude:high
//! Build script for aida-gui - generates gRPC client code when remote feature is enabled

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "remote")]
    {
        // Compile proto for client
        tonic_build::configure()
            .build_server(false)
            .build_client(true)
            .out_dir("src/generated")
            .compile_protos(&["../proto/aida.proto"], &["../proto"])?;
    }
    Ok(())
}
