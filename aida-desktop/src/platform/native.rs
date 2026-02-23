// trace:FR-0273 | ai:claude:high
//! Native platform implementation for desktop targets
//!
//! Uses: rfd (file dialogs), arboard (clipboard), dirs (directories), open (external apps)

use std::path::PathBuf;

use super::{FileFilter, PlatformServices};

/// Native platform services implementation
pub struct NativePlatform;

impl PlatformServices for NativePlatform {
    // ============ File Operations ============

    fn pick_file(&self, title: &str, filters: &[FileFilter]) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        for (name, exts) in filters {
            dialog = dialog.add_filter(*name, exts);
        }
        dialog.pick_file()
    }

    fn pick_files(&self, title: &str, filters: &[FileFilter]) -> Vec<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        for (name, exts) in filters {
            dialog = dialog.add_filter(*name, exts);
        }
        dialog.pick_files().unwrap_or_default()
    }

    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).pick_folder()
    }

    fn save_file(&self, suggested_name: &str, data: &[u8]) -> Result<Option<PathBuf>, String> {
        let path = rfd::FileDialog::new()
            .set_file_name(suggested_name)
            .save_file();

        if let Some(ref p) = path {
            std::fs::write(p, data).map_err(|e| e.to_string())?;
        }

        Ok(path)
    }

    fn read_file(&self, path: &PathBuf) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))
    }

    fn write_file(&self, path: &PathBuf, data: &[u8]) -> Result<(), String> {
        std::fs::write(path, data).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
    }

    fn path_exists(&self, path: &PathBuf) -> bool {
        path.exists()
    }

    fn create_dir_all(&self, path: &PathBuf) -> Result<(), String> {
        std::fs::create_dir_all(path)
            .map_err(|e| format!("Failed to create directory {}: {}", path.display(), e))
    }

    fn list_dir(&self, path: &PathBuf) -> Result<Vec<PathBuf>, String> {
        let entries = std::fs::read_dir(path)
            .map_err(|e| format!("Failed to read directory {}: {}", path.display(), e))?;

        let mut paths = Vec::new();
        for entry in entries {
            if let Ok(entry) = entry {
                paths.push(entry.path());
            }
        }
        Ok(paths)
    }

    // ============ Clipboard Operations ============

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(text.to_string()))
            .map_err(|e| format!("Clipboard error: {}", e))
    }

    fn get_clipboard(&self) -> Option<String> {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.get_text())
            .ok()
    }

    fn copy_to_primary(&self, text: &str) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use arboard::SetExtLinux;
            arboard::Clipboard::new()
                .and_then(|mut cb| {
                    cb.set().clipboard(arboard::LinuxClipboardKind::Primary).text(text.to_string())
                })
                .map_err(|e| format!("Primary selection error: {}", e))
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Primary selection only exists on X11/Linux
            let _ = text;
            Ok(())
        }
    }

    // ============ Settings Storage ============

    fn get_config_dir(&self) -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("aida"))
    }

    fn get_home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    fn load_setting(&self, key: &str) -> Option<String> {
        let config_dir = self.get_config_dir()?;
        let settings_file = config_dir.join("settings.yaml");

        if !settings_file.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&settings_file).ok()?;
        let settings: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;

        settings.get(key)?.as_str().map(|s| s.to_string())
    }

    fn save_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let config_dir = self.get_config_dir()
            .ok_or_else(|| "No config directory available".to_string())?;

        self.create_dir_all(&config_dir)?;

        let settings_file = config_dir.join("settings.yaml");

        // Load existing settings or create new
        let mut settings: serde_yaml::Mapping = if settings_file.exists() {
            let content = std::fs::read_to_string(&settings_file)
                .map_err(|e| format!("Failed to read settings: {}", e))?;
            serde_yaml::from_str(&content).unwrap_or_default()
        } else {
            serde_yaml::Mapping::new()
        };

        // Update the setting
        settings.insert(
            serde_yaml::Value::String(key.to_string()),
            serde_yaml::Value::String(value.to_string()),
        );

        // Write back
        let content = serde_yaml::to_string(&settings)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;
        std::fs::write(&settings_file, content)
            .map_err(|e| format!("Failed to write settings: {}", e))
    }

    fn delete_setting(&self, key: &str) -> Result<(), String> {
        let config_dir = self.get_config_dir()
            .ok_or_else(|| "No config directory available".to_string())?;

        let settings_file = config_dir.join("settings.yaml");

        if !settings_file.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&settings_file)
            .map_err(|e| format!("Failed to read settings: {}", e))?;

        let mut settings: serde_yaml::Mapping = serde_yaml::from_str(&content).unwrap_or_default();
        settings.remove(&serde_yaml::Value::String(key.to_string()));

        let content = serde_yaml::to_string(&settings)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;
        std::fs::write(&settings_file, content)
            .map_err(|e| format!("Failed to write settings: {}", e))
    }

    // ============ External Actions ============

    fn open_url(&self, url: &str) -> Result<(), String> {
        open::that(url).map_err(|e| format!("Failed to open URL: {}", e))
    }

    fn open_file_external(&self, path: &PathBuf) -> Result<(), String> {
        open::that(path).map_err(|e| format!("Failed to open file: {}", e))
    }

    // ============ System Information ============

    fn get_hostname(&self) -> String {
        hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    }

    fn is_web(&self) -> bool {
        false
    }

    // ============ Threading/Async ============

    fn spawn_background<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        std::thread::spawn(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_not_web() {
        let platform = NativePlatform;
        assert!(!platform.is_web());
    }

    #[test]
    fn test_get_hostname() {
        let platform = NativePlatform;
        let hostname = platform.get_hostname();
        assert!(!hostname.is_empty());
    }

    #[test]
    fn test_config_dir_exists() {
        let platform = NativePlatform;
        let config_dir = platform.get_config_dir();
        assert!(config_dir.is_some());
    }
}
