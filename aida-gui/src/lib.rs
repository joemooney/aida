// trace:FR-0273 | ai:claude:high
//! AIDA GUI Library
//!
//! This library provides the core GUI components for AIDA requirements management.
//! It can be compiled for both native desktop and WebAssembly (browser) targets.

mod app;
pub mod platform;
#[cfg(feature = "remote")]
mod remote;
mod storage;

pub use app::RequirementsApp;

// Web entry point for WASM builds
#[cfg(target_arch = "wasm32")]
mod web_entry {
    use super::*;
    use wasm_bindgen::prelude::*;

    /// Initialize logging and panic hooks for web
    fn init_web() {
        // Set up panic hook for better error messages
        #[cfg(feature = "web")]
        console_error_panic_hook::set_once();

        // Initialize logging
        #[cfg(feature = "web")]
        {
            use log::Level;
            console_log::init_with_level(Level::Debug).ok();
        }
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
            let start_result = eframe::WebRunner::new()
                .start(
                    "aida_canvas", // Canvas element ID
                    web_options,
                    Box::new(move |cc| {
                        Ok(Box::new(RequirementsApp::new_with_config(
                            cc,
                            None, // No file path in web mode
                            server_address.clone(),
                        )))
                    }),
                )
                .await;

            if let Err(e) = start_result {
                log::error!("Failed to start eframe: {:?}", e);
            }
        });

        Ok(())
    }
}
