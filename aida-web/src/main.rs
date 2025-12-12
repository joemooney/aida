// trace:FR-0273 | ai:claude:high
//! AIDA Web - WASM entry point

use wasm_bindgen::JsCast;

/// WASM entry point
#[cfg(target_arch = "wasm32")]
fn main() {
    // Set up panic hook for better error messages
    console_error_panic_hook::set_once();

    // Initialize logging to browser console
    console_log::init_with_level(log::Level::Debug).expect("Failed to init logging");

    log::info!("AIDA Web starting...");

    // Configure eframe for web
    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        // Get or create canvas element
        let document = web_sys::window()
            .expect("No window")
            .document()
            .expect("No document");

        // Create canvas element
        let canvas = document
            .create_element("canvas")
            .expect("Failed to create canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("Failed to cast to HtmlCanvasElement");

        canvas.set_id("the_canvas_id");
        document
            .body()
            .expect("No body")
            .append_child(&canvas)
            .expect("Failed to append canvas");

        let start_result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| Ok(Box::new(aida_web::app::AidaWebApp::new(cc)))),
            )
            .await;

        // Hide the loading indicator
        if let Some(loading) = document.get_element_by_id("loading") {
            loading.set_attribute("style", "display: none").ok();
        }

        if let Err(e) = start_result {
            log::error!("Failed to start eframe: {:?}", e);
        }
    });
}

/// For non-WASM builds (development/testing)
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("This binary is intended for WASM target only.");
    eprintln!("Build with: trunk build");
    eprintln!("Or run with: trunk serve");
    std::process::exit(1);
}
