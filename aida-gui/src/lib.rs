// trace:FR-0273 | ai:claude:high
//! AIDA GUI Library
//!
//! This library provides the core GUI components for AIDA requirements management.
//! It can be compiled for both native desktop and WebAssembly (browser) targets.
//!
//! ## Feature Flags
//! - `native`: Full native desktop GUI with local storage (default)
//! - `web`: Web/WASM support with gRPC-Web storage client
//!
//! ## Modules
//! - `storage`: gRPC client that works on both native and WASM
//! - `ui`: Shared UI components for rendering requirements (works on both platforms)

// The app module requires many native-only types from aida-core
#[cfg(feature = "native")]
mod app;

// Platform module for native/web abstractions
#[cfg(feature = "native")]
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
// Requires both native types (from aida-core) and web features
// This is NOT used when aida-web imports aida-gui for just the storage module
#[cfg(all(target_arch = "wasm32", feature = "native"))]
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
