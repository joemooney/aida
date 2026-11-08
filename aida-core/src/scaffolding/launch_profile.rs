//! Portable declarations, never vendor permission settings.
// trace:STORY-1486 | ai:codex
pub const PROFILE_PATH: &str = ".aida/launch-profile.toml";

/// Symbols are resolved afresh by the launcher, not expanded during scaffold.
/// Both development binary locations remain covered when the build changes.
pub const RECIPE: &str = include_str!("../../templates/launch-profile.toml");

#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-1486.generated_profile_preserves_approval_prompts | ai:codex
    #[test]
    fn generated_profile_preserves_approval_prompts() {
        let value: toml::Value = toml::from_str(RECIPE).unwrap();
        assert_eq!(value["approvals"].as_str(), Some("preserve"));
        for forbidden in ["allow", "no-ask", "bypass", "never", "/home/", "/tmp/"] {
            assert!(!RECIPE.contains(forbidden), "{forbidden}");
        }
        assert!(RECIPE.contains("target/debug/aida"));
        assert!(RECIPE.contains("target/release/aida"));
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
}
