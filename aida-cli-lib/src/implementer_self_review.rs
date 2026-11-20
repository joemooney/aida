//! Optional implementer self-review; informational evidence, never a merge gate.
// trace:TASK-3815 | ai:codex
use anyhow::{Context, Result};
use std::path::Path;

pub(crate) const MARKER: &str = "## Implementer self-review";

fn parse(body: Option<&str>) -> Result<bool> {
    #[derive(Default, serde::Deserialize)]
    struct Review {
        #[serde(default)]
        implementer_self_review: bool,
    }
    #[derive(Default, serde::Deserialize)]
    struct Config {
        #[serde(default)]
        review: Review,
    }
    Ok(match body {
        Some(body) => {
            toml::from_str::<Config>(body)
                .context("parsing implementer self-review setting")?
                .review
                .implementer_self_review
        }
        None => false,
    })
}

pub(crate) fn enabled(root: &Path) -> Result<bool> {
    parse(crate::trusted_config::read_trusted_config_toml(root).as_deref())
}

fn rubric() -> &'static str {
    let template = include_str!("../../aida-core/templates/skills/aida-review.md");
    let start = template
        .find("### 3. Walk each spec")
        .expect("review acceptance walk");
    let end = template[start..]
        .find("### 5. Fix-forward")
        .expect("review rubric end")
        + start;
    template[start..end].trim_end()
}

fn render(enabled: bool) -> String {
    if !enabled {
        return String::new();
    }
    format!(
        "\n\nImplementer self-review enabled by project setting. Before pushing, apply the reviewer rubric below to `git diff origin/main...HEAD`, fix actionable defects and repeat affected checks. In a native lane also review `git diff HEAD` and untracked additions before declaring host publication. Record a spec comment using `aida comment add <SPEC> \"{MARKER}\nReviewed HEAD: <sha>; acceptance evidence, probe findings, fixes, residual risks\"`. Adapt worksheet instructions to this comment; do not record a reviewer verdict. Normal implementer validation remains required; the reviewer-only cargo policy below does not replace it. Missing comments are reported, never blocking.\n\n{}",
        rubric()
    )
}

pub(crate) fn prompt(root: &Path) -> String {
    match enabled(root) {
        Ok(value) => render(value),
        Err(error) => format!("\n\nImplementer self-review setting unavailable: {error:#}"),
    }
}

pub(crate) fn has_comment(
    comments: &[aida_core::Comment],
    since: chrono::DateTime<chrono::Utc>,
) -> bool {
    comments.iter().any(|comment| {
        comment.created_at >= since
            && comment.content.lines().next() == Some(MARKER)
            && !comment.content[MARKER.len()..].trim().is_empty()
    })
}

pub(crate) fn observed(
    root: &Path,
    spec: &str,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<bool> {
    let path = match crate::detect_distributed_store_from(root) {
        Some(path) => path,
        None => aida_core::resolve_requirements_path_in(root, None, None, Path::new(""))?,
    };
    let store = aida_core::Storage::new(path).load()?;
    let req = store
        .get_requirement_unambiguous(spec)?
        .context("self-review spec missing")?;
    Ok(has_comment(&req.comments, since))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_3815_opt_in_embeds_single_source_reviewer_rubric() {
        assert!(!parse(None).unwrap());
        assert!(!parse(Some("[agents]\nvendor = 'codex'")).unwrap());
        assert!(!parse(Some("[review]\nimplementer_self_review = false")).unwrap());
        assert!(parse(Some("[review]\nimplementer_self_review = true")).unwrap());
        assert!(parse(Some("[review]\nimplementer_self_review = 'yes'")).is_err());
        assert!(render(false).is_empty());
        let brief = render(true);
        assert!(brief.contains(rubric()));
        assert!(brief.contains("git diff origin/main...HEAD"));
        assert!(brief.contains(MARKER));
        assert!(rubric().contains("**Probe 5"));
        for skill in [
            include_str!("../../aida-core/templates/skills/aida-implement.md"),
            include_str!("../../aida-core/templates/skills/aida-pickup.md"),
        ] {
            assert!(skill.contains("steps 3 and 4"));
            assert!(skill.contains(MARKER));
        }
    }

    #[test]
    fn task_3815_comment_must_be_current_and_have_result() {
        let now = chrono::Utc::now();
        let mut comment =
            aida_core::Comment::new("implementer".into(), format!("{MARKER}\nPASS: evidence"));
        assert!(has_comment(&[comment.clone()], now));
        comment.created_at = now - chrono::Duration::seconds(1);
        assert!(!has_comment(&[comment], now));
        for content in [
            MARKER,
            "unrelated comment",
            "## Implementer self-review extra\nPASS",
        ] {
            assert!(!has_comment(
                &[aida_core::Comment::new(
                    "implementer".into(),
                    content.into()
                )],
                now
            ));
        }
    }
}
