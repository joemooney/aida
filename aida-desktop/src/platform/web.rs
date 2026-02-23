// trace:FR-0273 | ai:claude:high
//! Web platform implementation for WASM browser targets
//!
//! Uses web-sys for browser APIs: localStorage, clipboard, file download, etc.

use std::path::PathBuf;

use super::{FileFilter, PlatformServices};

/// Web platform services implementation
pub struct WebPlatform {
    // Could store references to web APIs if needed
}

impl WebPlatform {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for WebPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformServices for WebPlatform {
    // ============ File Operations ============

    fn pick_file(&self, _title: &str, _filters: &[FileFilter]) -> Option<PathBuf> {
        // File picking in browser requires async callback via <input type="file">
        // Return None - actual file picking will be handled separately via JS interop
        log::warn!("pick_file: Not directly supported in web - use file input element");
        None
    }

    fn pick_files(&self, _title: &str, _filters: &[FileFilter]) -> Vec<PathBuf> {
        log::warn!("pick_files: Not directly supported in web - use file input element");
        Vec::new()
    }

    fn pick_folder(&self, _title: &str) -> Option<PathBuf> {
        log::warn!("pick_folder: Not supported in web browsers");
        None
    }

    fn save_file(&self, suggested_name: &str, data: &[u8]) -> Result<Option<PathBuf>, String> {
        // Trigger browser download
        use wasm_bindgen::JsCast;

        let window = web_sys::window().ok_or("No window")?;
        let document = window.document().ok_or("No document")?;

        // Create blob from data
        let uint8_array = js_sys::Uint8Array::from(data);
        let blob_parts = js_sys::Array::new();
        blob_parts.push(&uint8_array);

        let blob = web_sys::Blob::new_with_u8_array_sequence(&blob_parts)
            .map_err(|_| "Failed to create blob")?;

        // Create object URL
        let url = web_sys::Url::create_object_url_with_blob(&blob)
            .map_err(|_| "Failed to create object URL")?;

        // Create and click download link
        let link = document
            .create_element("a")
            .map_err(|_| "Failed to create element")?
            .dyn_into::<web_sys::HtmlAnchorElement>()
            .map_err(|_| "Failed to cast to anchor")?;

        link.set_href(&url);
        link.set_download(suggested_name);
        link.click();

        // Cleanup
        let _ = web_sys::Url::revoke_object_url(&url);

        // Return None since we don't know where the file was saved
        Ok(None)
    }

    fn read_file(&self, path: &PathBuf) -> Result<Vec<u8>, String> {
        // Reading arbitrary files not supported in browser
        // Files must be picked via file input and read via FileReader API
        Err(format!(
            "Cannot read file {} in browser - use file input",
            path.display()
        ))
    }

    fn write_file(&self, path: &PathBuf, _data: &[u8]) -> Result<(), String> {
        Err(format!(
            "Cannot write to {} in browser - use save_file for downloads",
            path.display()
        ))
    }

    fn path_exists(&self, _path: &PathBuf) -> bool {
        // No filesystem access in browser
        false
    }

    fn create_dir_all(&self, _path: &PathBuf) -> Result<(), String> {
        // No filesystem in browser
        Ok(()) // No-op
    }

    fn list_dir(&self, _path: &PathBuf) -> Result<Vec<PathBuf>, String> {
        Ok(Vec::new()) // No filesystem
    }

    // ============ Clipboard Operations ============

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        let text = text.to_string();

        wasm_bindgen_futures::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let promise = clipboard.write_text(&text);
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            }
        });

        Ok(())
    }

    fn get_clipboard(&self) -> Option<String> {
        // Clipboard read requires async and user permission
        // Cannot be done synchronously
        log::warn!("get_clipboard: Requires async - use clipboard paste event");
        None
    }

    fn copy_to_primary(&self, _text: &str) -> Result<(), String> {
        // Primary selection doesn't exist in browsers
        Ok(())
    }

    // ============ Settings Storage ============

    fn get_config_dir(&self) -> Option<PathBuf> {
        // No filesystem in browser
        None
    }

    fn get_home_dir(&self) -> Option<PathBuf> {
        None
    }

    fn load_setting(&self, key: &str) -> Option<String> {
        let window = web_sys::window()?;
        let storage = window.local_storage().ok()??;
        storage.get_item(key).ok()?
    }

    fn save_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let window = web_sys::window().ok_or("No window")?;
        let storage = window
            .local_storage()
            .map_err(|_| "localStorage not available")?
            .ok_or("localStorage not available")?;

        storage
            .set_item(key, value)
            .map_err(|_| "Failed to save to localStorage".to_string())
    }

    fn delete_setting(&self, key: &str) -> Result<(), String> {
        let window = web_sys::window().ok_or("No window")?;
        let storage = window
            .local_storage()
            .map_err(|_| "localStorage not available")?
            .ok_or("localStorage not available")?;

        storage
            .remove_item(key)
            .map_err(|_| "Failed to delete from localStorage".to_string())
    }

    // ============ External Actions ============

    fn open_url(&self, url: &str) -> Result<(), String> {
        let window = web_sys::window().ok_or("No window")?;
        window
            .open_with_url_and_target(url, "_blank")
            .map_err(|_| "Failed to open URL")?;
        Ok(())
    }

    fn open_file_external(&self, _path: &PathBuf) -> Result<(), String> {
        Err("Cannot open local files in browser".to_string())
    }

    // ============ System Information ============

    fn get_hostname(&self) -> String {
        web_sys::window()
            .and_then(|w| w.location().hostname().ok())
            .unwrap_or_else(|| "web-client".to_string())
    }

    fn is_web(&self) -> bool {
        true
    }

    // ============ Threading/Async ============

    fn spawn_background<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        // In WASM, we can't use threads directly
        // For simple fire-and-forget tasks, just run synchronously
        // For async work, use wasm_bindgen_futures::spawn_local
        task();
    }
}

/// Helper to get the current window location origin
pub fn get_origin() -> Option<String> {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
}

/// Helper to get a query parameter from the URL
pub fn get_query_param(name: &str) -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require wasm-pack test --headless --chrome
    // They won't run in regular cargo test

    #[test]
    fn test_is_web() {
        let platform = WebPlatform::new();
        assert!(platform.is_web());
    }
}
