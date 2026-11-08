//! Portable declarations, never vendor permission settings.
// trace:STORY-1486 | ai:codex
pub const PROFILE_PATH: &str = ".aida/launch-profile.toml";

/// Symbols are resolved afresh by the launcher, not expanded during scaffold.
/// Both development binary locations remain covered when the build changes.
pub const RECIPE: &str = include_str!("../../templates/launch-profile.toml");

#[cfg(test)]
mod tests {
    use super::*;

    // These negative path checks use Linux spellings; they do not create paths.
    // trace:STORY-1486.generated_profile_preserves_approval_prompts | ai:codex
    #[cfg(target_os = "linux")]
    #[test]
    fn generated_profile_preserves_approval_prompts() {
        use crate::scaffolding::{ScaffoldConfig, Scaffolder};
        let root = tempfile::tempdir().unwrap();
        let preview = Scaffolder::new(root.path().into(), ScaffoldConfig::default())
            .preview(&crate::RequirementsStore::new());
        let content = &preview
            .artifacts
            .iter()
            .find(|artifact| artifact.path == std::path::Path::new(PROFILE_PATH))
            .unwrap()
            .content;
        let value: toml::Value = toml::from_str(content).unwrap();
        assert_eq!(value["approvals"].as_str(), Some("preserve"));
        for forbidden in ["allow", "no-ask", "bypass", "never", "/home/", "/tmp/"] {
            assert!(!content.contains(forbidden), "{forbidden}");
        }
        assert!(content.contains("target/debug/aida"));
        assert!(content.contains("target/release/aida"));
    }
    // trace:STORY-1486.acc62716 | ai:codex
    #[test]
    fn scaffold_recipe_is_portable_across_project_locations() {
        use crate::scaffolding::{ScaffoldConfig, Scaffolder};
        let store = crate::RequirementsStore::new();
        let mut contents = Vec::new();
        for name in ["consumer", "relocated"] {
            let root = tempfile::tempdir().unwrap();
            let project = root.path().join(name);
            std::fs::create_dir_all(&project).unwrap();
            let preview =
                Scaffolder::new(project.clone(), ScaffoldConfig::default()).preview(&store);
            let artifact = preview
                .artifacts
                .iter()
                .find(|a| a.path == std::path::Path::new(PROFILE_PATH))
                .unwrap();
            assert!(!artifact.content.contains(project.to_str().unwrap()));
            let parsed: toml::Value = toml::from_str(&artifact.content).unwrap();
            assert_eq!(parsed, toml::from_str::<toml::Value>(RECIPE).unwrap());
            contents.push(artifact.content.clone());
        }
        assert_eq!(contents[0], contents[1]);
    }
    // trace:STORY-1486 | ai:codex
    #[test]
    fn launch_profile_preview_classifies_existing_files() {
        use crate::scaffolding::{ScaffoldConfig, Scaffolder};
        let root = tempfile::tempdir().unwrap();
        let store = crate::RequirementsStore::new();
        let path = std::path::PathBuf::from(PROFILE_PATH);
        let mut scaffolder = Scaffolder::new(root.path().into(), ScaffoldConfig::default());
        let initial = scaffolder.preview(&store);
        assert!(initial.new_files.contains(&path));
        let generated = initial
            .artifacts
            .iter()
            .find(|a| a.path == path)
            .unwrap()
            .content
            .clone();
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        for (content, expected) in [
            (generated.clone(), "unmodified"),
            (RECIPE.to_string(), "modified"),
            (
                generated.replacen("checksum:", "checksum:deadbeef", 1),
                "modified",
            ),
            (
                generated.replacen(&format!("v{}", super::super::SCAFFOLD_VERSION), "v0.0.0", 1),
                "older",
            ),
        ] {
            std::fs::write(root.path().join(&path), content).unwrap();
            let preview = scaffolder.preview(&store);
            assert_eq!(preview.overwrites.contains(&path), expected == "unmodified");
            assert_eq!(
                preview.modified_files.contains(&path),
                expected == "modified"
            );
            assert_eq!(
                preview.upgradeable_files.contains(&path),
                expected == "older"
            );
            assert!(!preview.new_files.contains(&path));
        }
    }
}
