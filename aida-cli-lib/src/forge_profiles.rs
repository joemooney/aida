//! Machine-global forge profiles for `aida init <DIR> --forge <name>`.
//!
//! Profiles deliberately live outside project config: they describe this
//! machine's preferred hub/namespace conventions, not repository state.
//! trace:STORY-823 | ai:codex

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ForgeProfiles {
    #[serde(default)]
    pub(crate) profile: Vec<ForgeProfile>,
    #[serde(default)]
    pub(crate) defaults: ForgeDefaults,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ForgeDefaults {
    pub(crate) profile: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct ForgeProfile {
    pub(crate) name: String,
    pub(crate) kind: ForgeProfileKind,
    pub(crate) host: Option<String>,
    pub(crate) namespace: String,
    #[serde(default = "default_protocol")]
    pub(crate) protocol: ForgeProtocol,
    #[serde(default = "default_visibility")]
    pub(crate) visibility: ForgeVisibility,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ForgeProfileKind {
    GitHub,
    GitLab,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ForgeProtocol {
    Ssh,
    Https,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ForgeVisibility {
    Private,
    Public,
}

fn default_protocol() -> ForgeProtocol {
    ForgeProtocol::Ssh
}

fn default_visibility() -> ForgeVisibility {
    ForgeVisibility::Private
}

pub(crate) fn profiles_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("HOME not set; cannot locate ~/.aida/forges.toml")?;
    Ok(home.join(".aida").join("forges.toml"))
}

pub(crate) fn load_profiles() -> Result<Option<ForgeProfiles>> {
    let path = profiles_path()?;
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let profiles: ForgeProfiles =
        toml::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(profiles))
}

#[cfg(test)]
pub(crate) fn parse_profiles(body: &str) -> Result<ForgeProfiles> {
    toml::from_str(body).context("parsing forge profiles")
}

impl ForgeProfiles {
    pub(crate) fn find(&self, name: &str) -> Option<&ForgeProfile> {
        self.profile.iter().find(|p| p.name == name)
    }

    pub(crate) fn names(&self) -> Vec<String> {
        self.profile.iter().map(|p| p.name.clone()).collect()
    }

    pub(crate) fn require(&self, name: &str) -> Result<&ForgeProfile> {
        self.find(name).ok_or_else(|| {
            let names = self.names();
            let valid = if names.is_empty() {
                "none configured".to_string()
            } else {
                names.join(", ")
            };
            anyhow::anyhow!("unknown --forge `{name}` — configured profiles: {valid}")
        })
    }
}

impl ForgeProfile {
    pub(crate) fn github_repo_name(&self, repo: &str) -> String {
        format!("{}/{}", self.namespace, repo)
    }

    pub(crate) fn gitlab_remote_url(&self, repo: &str) -> Result<String> {
        if self.kind != ForgeProfileKind::GitLab {
            bail!("profile `{}` is not a gitlab profile", self.name);
        }
        let host = self
            .host
            .as_deref()
            .filter(|h| !h.trim().is_empty())
            .unwrap_or("gitlab.com");
        match self.protocol {
            ForgeProtocol::Ssh => Ok(format!("git@{host}:{}/{repo}.git", self.namespace)),
            ForgeProtocol::Https => Ok(format!("https://{host}/{}/{repo}.git", self.namespace)),
        }
    }

    pub(crate) fn is_public(&self) -> bool {
        self.visibility == ForgeVisibility::Public
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profiles_and_default() {
        let profiles = parse_profiles(
            r#"
            [[profile]]
            name = "home-gitlab"
            kind = "gitlab"
            host = "gitlab.joemooney.com"
            namespace = "joe"
            protocol = "ssh"

            [defaults]
            profile = "home-gitlab"
            "#,
        )
        .unwrap();

        assert_eq!(profiles.defaults.profile.as_deref(), Some("home-gitlab"));
        assert_eq!(profiles.profile[0].kind, ForgeProfileKind::GitLab);
    }

    #[test]
    fn templates_gitlab_urls() {
        let ssh = parse_profiles(
            r#"
            [[profile]]
            name = "work"
            kind = "gitlab"
            host = "gitlab.example.test"
            namespace = "platform"
            protocol = "ssh"
            "#,
        )
        .unwrap();
        assert_eq!(
            ssh.require("work")
                .unwrap()
                .gitlab_remote_url("app")
                .unwrap(),
            "git@gitlab.example.test:platform/app.git"
        );

        let https = parse_profiles(
            r#"
            [[profile]]
            name = "work"
            kind = "gitlab"
            host = "gitlab.example.test"
            namespace = "platform"
            protocol = "https"
            "#,
        )
        .unwrap();
        assert_eq!(
            https
                .require("work")
                .unwrap()
                .gitlab_remote_url("app")
                .unwrap(),
            "https://gitlab.example.test/platform/app.git"
        );
    }

    #[test]
    fn unknown_profile_error_lists_valid_names() {
        let profiles = parse_profiles(
            r#"
            [[profile]]
            name = "home"
            kind = "github"
            namespace = "joe"
            "#,
        )
        .unwrap();
        let err = profiles.require("work").unwrap_err().to_string();
        assert!(err.contains("unknown --forge `work`"));
        assert!(err.contains("home"));
    }
}
