// trace:FR-0273 | ai:claude:high
//! Platform abstraction layer for cross-platform GUI support
//!
//! This module provides a unified interface for platform-specific operations,
//! allowing the GUI to compile for both native desktop and WebAssembly targets.

use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::NativePlatform;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::WebPlatform;

/// File filter for file dialogs (name, extensions)
pub type FileFilter<'a> = (&'a str, &'a [&'a str]);

/// Platform-specific services abstraction
///
/// This trait abstracts operations that differ between native and web platforms:
/// - File dialogs and file system access
/// - Clipboard operations
/// - Settings/preferences storage
/// - External application launching
/// - System information
pub trait PlatformServices {
    // ============ File Operations ============

    /// Open a file picker dialog
    /// Returns the selected file path, or None if cancelled
    fn pick_file(&self, title: &str, filters: &[FileFilter]) -> Option<PathBuf>;

    /// Open a file picker dialog for multiple files
    fn pick_files(&self, title: &str, filters: &[FileFilter]) -> Vec<PathBuf>;

    /// Open a folder picker dialog
    fn pick_folder(&self, title: &str) -> Option<PathBuf>;

    /// Trigger a file save/download
    /// On native: opens save dialog
    /// On web: triggers browser download
    fn save_file(&self, suggested_name: &str, data: &[u8]) -> Result<Option<PathBuf>, String>;

    /// Read a file's contents
    /// On native: reads from filesystem
    /// On web: only works for files previously picked via pick_file
    fn read_file(&self, path: &PathBuf) -> Result<Vec<u8>, String>;

    /// Write data to a file
    /// On native: writes to filesystem
    /// On web: not supported (use save_file for downloads)
    fn write_file(&self, path: &PathBuf, data: &[u8]) -> Result<(), String>;

    /// Check if a path exists
    fn path_exists(&self, path: &PathBuf) -> bool;

    /// Create a directory (and parents if needed)
    fn create_dir_all(&self, path: &PathBuf) -> Result<(), String>;

    /// List files in a directory
    fn list_dir(&self, path: &PathBuf) -> Result<Vec<PathBuf>, String>;

    // ============ Clipboard Operations ============

    /// Copy text to the system clipboard
    fn copy_to_clipboard(&self, text: &str) -> Result<(), String>;

    /// Get text from the system clipboard
    fn get_clipboard(&self) -> Option<String>;

    /// Copy to primary selection (X11 middle-click paste)
    /// No-op on platforms without primary selection
    fn copy_to_primary(&self, text: &str) -> Result<(), String>;

    // ============ Settings Storage ============

    /// Get the platform-specific config directory
    /// Native: ~/.config/aida/ or platform equivalent
    /// Web: None (use load_setting/save_setting instead)
    fn get_config_dir(&self) -> Option<PathBuf>;

    /// Get the user's home directory
    /// Web: None
    fn get_home_dir(&self) -> Option<PathBuf>;

    /// Load a setting by key
    /// Native: reads from config file
    /// Web: reads from localStorage
    fn load_setting(&self, key: &str) -> Option<String>;

    /// Save a setting by key
    /// Native: writes to config file
    /// Web: writes to localStorage
    fn save_setting(&self, key: &str, value: &str) -> Result<(), String>;

    /// Delete a setting
    fn delete_setting(&self, key: &str) -> Result<(), String>;

    // ============ External Actions ============

    /// Open a URL in the default browser
    fn open_url(&self, url: &str) -> Result<(), String>;

    /// Open a file with its default application
    /// Web: not supported, returns error
    fn open_file_external(&self, path: &PathBuf) -> Result<(), String>;

    // ============ System Information ============

    /// Get the hostname of the machine
    /// Web: returns "web-client"
    fn get_hostname(&self) -> String;

    /// Check if running in a web browser
    fn is_web(&self) -> bool;

    // ============ Threading/Async ============

    /// Spawn a background task
    /// Native: spawns a thread
    /// Web: spawns via wasm-bindgen-futures
    fn spawn_background<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static;
}

/// Get the platform services instance for the current platform
#[allow(dead_code)] // Will be used in Phase 2 when app.rs is refactored
#[cfg(not(target_arch = "wasm32"))]
pub fn platform() -> &'static NativePlatform {
    static INSTANCE: NativePlatform = NativePlatform;
    &INSTANCE
}

#[cfg(target_arch = "wasm32")]
pub fn platform() -> &'static WebPlatform {
    static INSTANCE: std::sync::OnceLock<WebPlatform> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(WebPlatform::new)
}
