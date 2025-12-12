// trace:FR-0273 | ai:claude:high
//! AIDA GUI Library
//!
//! This library provides the core GUI components for AIDA requirements management.
//! It can be compiled for both native desktop and WebAssembly (browser) targets.
//!
//! ## Feature Flags
//! - `native`: Full native desktop GUI with local storage (default)
//! - `web`: Web/WASM support with gRPC-Web storage client
//! - `native-types`: Just aida-core types without native storage (for WASM)
//!
//! ## Modules
//! - `storage`: gRPC client that works on both native and WASM
//! - `ui`: Shared UI components for rendering requirements (works on both platforms)

// The app module requires aida-core types
// Available on native builds or WASM builds with native-types feature
#[cfg(any(feature = "native", feature = "native-types"))]
mod app;

// Platform module for native/web abstractions
#[cfg(any(feature = "native", feature = "native-types"))]
pub mod platform;

#[cfg(feature = "remote")]
mod remote;

// Storage module is available for both native and web builds
// It provides the GrpcStorageClient that works on both platforms
pub mod storage;

// Shared UI components - available for both native and web builds
// These are pure egui rendering functions that work with proto types
pub mod ui;

#[cfg(feature = "native")]
pub use app::RequirementsApp;

// Web entry point for WASM builds of aida-gui itself (full GUI)
// Requires native-types feature (for app/aida-core types) and web feature
// This is activated when building with: --features "native-types,web" for wasm32 target
#[cfg(all(target_arch = "wasm32", feature = "native-types", feature = "web"))]
mod web_entry {
    use super::*;
    use app::RequirementsApp;
    use wasm_bindgen::prelude::*;

    /// Initialize logging and panic hooks for web
    fn init_web() {
        // Set up panic hook for better error messages
        console_error_panic_hook::set_once();

        // Initialize logging
        {
            use log::Level;
            console_log::init_with_level(Level::Debug).ok();
        }
    }

    /// Get the canvas element by ID
    fn get_canvas_by_id(canvas_id: &str) -> Option<web_sys::HtmlCanvasElement> {
        use wasm_bindgen::JsCast;
        let document = web_sys::window()?.document()?;
        let canvas = document.get_element_by_id(canvas_id)?;
        canvas.dyn_into::<web_sys::HtmlCanvasElement>().ok()
    }

    /// WASM entry point - called when the module loads
    #[wasm_bindgen(start)]
    pub fn start() -> Result<(), JsValue> {
        init_web();

        // Get server address from URL query params
        let server_address = platform::web::get_query_param("server");

        // Start the eframe web runner
        let web_options = eframe::WebOptions::default();

        wasm_bindgen_futures::spawn_local(async move {
            // Get the canvas element
            let canvas = match get_canvas_by_id("aida_canvas") {
                Some(c) => c,
                None => {
                    log::error!("Canvas element 'aida_canvas' not found");
                    return;
                }
            };

            let start_result = eframe::WebRunner::new()
                .start(
                    canvas,
                    web_options,
                    Box::new(move |cc| Ok(Box::new(RequirementsApp::new_wasm(cc)))),
                )
                .await;

            if let Err(e) = start_result {
                log::error!("Failed to start eframe: {:?}", e);
            }
        });

        Ok(())
    }
}
