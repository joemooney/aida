mod app;
mod platform;
#[cfg(feature = "remote")]
mod remote;

use eframe::egui;
use std::env;

/// Command line arguments
struct CliArgs {
    /// Path to the requirements file (for local storage)
    file_path: Option<String>,
    /// Server address (for remote storage, e.g., "localhost:50051")
    server: Option<String>,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = env::args().collect();
    let mut file_path: Option<String> = None;
    let mut server: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--file" if i + 1 < args.len() => {
                file_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--server" | "-s" if i + 1 < args.len() => {
                server = Some(args[i + 1].clone());
                i += 2;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("aida-gui {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => {
                i += 1;
            }
        }
    }

    // Check for AIDA_SERVER environment variable if --server not specified
    if server.is_none() {
        server = env::var("AIDA_SERVER").ok();
    }

    CliArgs { file_path, server }
}

fn print_help() {
    println!("AIDA GUI - Requirements Management System");
    println!();
    println!("Usage: aida-gui [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --file <PATH>       Path to the requirements file (local storage)");
    println!("  --server <ADDR>     Connect to AIDA gRPC server (e.g., localhost:50051)");
    println!("  -s <ADDR>           Short form of --server");
    println!("  --help, -h          Print this help message");
    println!("  --version, -V       Print version");
    println!();
    println!("Environment Variables:");
    println!("  AIDA_SERVER         Server address (used if --server not specified)");
    println!();
    println!("Examples:");
    println!("  aida-gui                          # Open with default local file");
    println!("  aida-gui --file project.yaml      # Open specific file");
    println!("  aida-gui --server localhost:50051 # Connect to remote server");
    println!("  AIDA_SERVER=localhost:50051 aida-gui  # Via environment variable");
    #[cfg(not(feature = "remote"))]
    println!();
    #[cfg(not(feature = "remote"))]
    println!("Note: Remote server support requires building with --features remote");
}

fn main() -> Result<(), eframe::Error> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    let cli = parse_args();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    // Determine window title based on connection type
    let title = if cli.server.is_some() {
        format!("AIDA - Requirements Manager (Remote: {})", cli.server.as_ref().unwrap())
    } else {
        "AIDA - Requirements Manager".to_string()
    };

    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::RequirementsApp::new_with_config(
                cc,
                cli.file_path.clone(),
                cli.server.clone(),
            )))
        }),
    )
}
