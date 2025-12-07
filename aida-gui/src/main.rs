mod app;

use eframe::egui;
use std::env;

fn main() -> Result<(), eframe::Error> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    // Parse command line arguments for --file <path>
    let args: Vec<String> = env::args().collect();
    let mut file_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--file" && i + 1 < args.len() {
            file_path = Some(args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "AIDA - Requirements Manager",
        options,
        Box::new(move |cc| Ok(Box::new(app::RequirementsApp::new_with_file(cc, file_path.clone())))),
    )
}
