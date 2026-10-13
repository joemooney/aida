//! BUG-1928: the contained posture's WRITE boundary, as a list of absolute
//! allowed roots.
//!
//! A contained headless session runs `claude -p --permission-mode dontAsk` with
//! an inline `--settings` document. Under `dontAsk` every tool call that no
//! `permissions.allow` rule pre-approves is denied, so the allow list IS the
//! write boundary. The pre-BUG-1928 list was the single rule `Edit(/**)`. In
//! Claude Code's Read/Edit rule grammar a single leading `/` is anchored to the
//! project/settings root, not the filesystem root; the absolute form is `//`.
//! Probed against claude 2.1.296 with the session's cwd at the worktree root:
//! `Edit(/**)` denied a Write to `<worktree>/inside.txt`, while
//! `Edit(//<worktree>/**)` + `Write(//<worktree>/**)` allowed it and denied
//! `<worktree>/../outside.txt`. Every contained drain therefore shipped nothing.
//!
//! The boundary is a LIST of absolute roots so a later slice (BUG-1929: the
//! main checkout's `.aida-store` and cache) extends this one list instead of
//! adding a second rule source. Each root becomes one absolute rule per
//! file-writing tool. A root that cannot be expressed exactly (relative, the
//! filesystem root, `..`, non-UTF-8, or containing glob/rule metacharacters)
//! emits NO rule: the boundary fails closed, never wider.
//!
//! The roots come from the worktree the session actually runs in (the git root
//! of the spawning process's cwd, which `queue work` sets to the lease's
//! worktree, pool or named), never from a requested path the warm pool may have
//! replaced. trace:BUG-1928 | ai:claude

use std::path::{Component, Path, PathBuf};

/// The built-in Claude Code tools that write files. All use the Read/Edit path
/// rule grammar. Claude Code applies `Edit` rules to every file-editing tool;
/// the others are emitted explicitly too, so the boundary does not depend on
/// that aliasing.
// trace:BUG-1928 | ai:claude
pub(crate) const WRITE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Characters that would change the meaning of a rule path (glob syntax, rule
/// delimiters, escapes). A root containing any of them is refused rather than
/// escaped.
// trace:BUG-1928 | ai:claude
const RULE_METACHARS: [char; 10] = ['*', '?', '[', ']', '{', '}', '(', ')', '\\', '!'];

/// The absolute roots a contained session may write under, derived from the
/// worktree it runs in: the path as given when it is absolute, plus its
/// canonical form when that differs (a symlinked parent), so the rule matches
/// whichever spelling Claude Code resolves a tool path to. Both spell the SAME
/// directory, so the list is never broader than the worktree.
// trace:BUG-1928 | ai:claude
pub(crate) fn contained_write_roots(worktree: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if worktree.is_absolute() {
        roots.push(worktree.to_path_buf());
    }
    if let Ok(canonical) = std::fs::canonicalize(worktree) {
        if !roots.contains(&canonical) {
            roots.push(canonical);
        }
    }
    roots
}

/// The rule-path spelling of `root` (`/a/b`, no trailing slash), or `None` when
/// the root cannot be expressed as an exact absolute directory.
// trace:BUG-1928 | ai:claude
pub(crate) fn root_rule_path(root: &Path) -> Option<String> {
    if !root.is_absolute() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for component in root.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(segment) => parts.push(segment.to_str()?),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        // `/` itself: a rule over the whole filesystem is never a worktree.
        return None;
    }
    let unsafe_segment = parts.iter().any(|part| {
        part.chars()
            .any(|ch| ch.is_control() || RULE_METACHARS.contains(&ch))
    });
    if unsafe_segment {
        return None;
    }
    Some(format!("/{}", parts.join("/")))
}

/// The `permissions.allow` rules for `roots`: `<Tool>(//<root>/**)` for every
/// tool in [`WRITE_TOOLS`]. The `//` prefix is Claude Code's absolute-path
/// anchor (`/<root>` would be read relative to the project root).
// trace:BUG-1928 | ai:claude
pub(crate) fn write_allow_rules(roots: &[PathBuf]) -> Vec<String> {
    let mut rules: Vec<String> = Vec::new();
    for root in roots {
        let Some(abs) = root_rule_path(root) else {
            continue;
        };
        for tool in WRITE_TOOLS {
            let rule = format!("{tool}(/{abs}/**)");
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
    rules
}

/// The label for the workspace line a launching `queue work` child prints. The
/// orchestrator's phase-1 child carries `AIDA_AUTO_COMPLETE_PHASE=1`.
// trace:BUG-1928 | ai:claude
pub(crate) fn launched_workspace_label(phase_env: Option<&str>) -> &'static str {
    if phase_env.map(str::trim) == Some("1") {
        "phase 1 workspace"
    } else {
        "workspace"
    }
}

/// `<label>: <worktree> [<branch>]` — the workspace announcement, printed from
/// the lease's worktree once it exists, so it names the tree the session runs
/// in (a warm-pool tree included), not the path the orchestrator requested.
// trace:BUG-1928 | ai:claude
pub(crate) fn workspace_announcement(label: &str, worktree: &Path, branch: &str) -> String {
    format!("{label}: {} [{branch}]", worktree.display())
}

/// The `write boundary` line for a contained session launched outside any git
/// repository: there is no worktree, so there are no write roots.
// trace:BUG-1971 | ai:claude
pub(crate) const OUTSIDE_GIT_BOUNDARY_ANNOUNCEMENT: &str =
    "write boundary: none (the session is not inside a git repository, so there is no \
     worktree and every file edit will be denied)";

/// BUG-1971: the launch lines `queue work` prints once the lease exists: the
/// workspace line from the lease's worktree and, for a contained claude
/// launch, the write-boundary line from the SAME roots the launch's
/// `--settings` rules use (the git root of the process cwd), never from
/// `lease_worktree` as given.
// trace:BUG-1971 | ai:claude
pub(crate) fn queue_work_launch_lines(
    phase_env: Option<&str>,
    lease_worktree: &Path,
    branch: &str,
    contained_claude: bool,
) -> Vec<String> {
    let mut lines = vec![workspace_announcement(
        launched_workspace_label(phase_env),
        lease_worktree,
        branch,
    )];
    if contained_claude {
        lines.push(crate::session::contained_write_boundary_announcement());
    }
    lines
}

/// `write boundary: //<root>/**, …` — the operator-facing view of the allow
/// rules, built from the same roots.
// trace:BUG-1928 | ai:claude
pub(crate) fn write_boundary_announcement(roots: &[PathBuf]) -> String {
    let patterns: Vec<String> = roots
        .iter()
        .filter_map(|root| root_rule_path(root.as_path()))
        .map(|abs| format!("/{abs}/**"))
        .collect();
    if patterns.is_empty() {
        "write boundary: none (the worktree path cannot be expressed as an exact rule, so \
         every file edit will be denied)"
            .to_string()
    } else {
        format!("write boundary: {}", patterns.join(", "))
    }
}

/// Test-only evaluator for Claude Code's documented Read/Edit permission rule
/// grammar (gitignore-style), so the generated settings can be checked against
/// REAL filesystem paths rather than by string presence:
///
/// - `//p` — absolute path from the filesystem root
/// - `~/p` — relative to the home directory
/// - `/p`  — relative to the project/settings root
/// - `**` matches any number of path segments (a trailing `/**` at least one);
///   `*` and `?` match within one segment
/// - an `Edit` rule applies to every file-editing tool
/// - under `dontAsk`, a matching `deny` wins, then a matching `allow` permits,
///   and anything else is denied
///
/// Target paths are normalised lexically (`..` is resolved) before matching,
/// the way a tool's `file_path` is resolved before the check.
// trace:BUG-1928 | ai:claude
#[cfg(test)]
pub(crate) mod evaluator {
    use std::path::{Component, Path, PathBuf};

    pub(crate) fn normalize(path: &Path) -> Option<Vec<String>> {
        if !path.is_absolute() {
            return None;
        }
        let mut out: Vec<String> = Vec::new();
        for component in path.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                Component::Normal(segment) => out.push(segment.to_str()?.to_string()),
                Component::Prefix(_) => return None,
            }
        }
        Some(out)
    }

    /// `Tool(spec)` → `(tool, spec)`.
    pub(crate) fn parse_rule(rule: &str) -> Option<(&str, &str)> {
        let open = rule.find('(')?;
        let inner = rule[open + 1..].strip_suffix(')')?;
        Some((&rule[..open], inner))
    }

    /// The rule's path pattern as absolute segments, or `None` for a form AIDA
    /// never emits (cwd-relative / unanchored).
    pub(crate) fn anchor(spec: &str, settings_root: &Path, home: &Path) -> Option<Vec<String>> {
        let (base, rest): (PathBuf, &str) = if let Some(rest) = spec.strip_prefix("//") {
            (PathBuf::from("/"), rest)
        } else if let Some(rest) = spec.strip_prefix("~/") {
            (home.to_path_buf(), rest)
        } else if let Some(rest) = spec.strip_prefix('/') {
            (settings_root.to_path_buf(), rest)
        } else {
            return None;
        };
        let mut segments = normalize(&base)?;
        segments.extend(
            rest.split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        );
        Some(segments)
    }

    fn segment_matches(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some((&'*', rest)) => (0..=text.len()).any(|i| segment_matches(rest, &text[i..])),
            Some((&'?', rest)) => !text.is_empty() && segment_matches(rest, &text[1..]),
            Some((c, rest)) => text.first() == Some(c) && segment_matches(rest, &text[1..]),
        }
    }

    pub(crate) fn path_matches(pattern: &[String], path: &[String]) -> bool {
        match pattern.split_first() {
            None => path.is_empty(),
            Some((head, rest)) if head == "**" => {
                if rest.is_empty() {
                    // `dir/**` covers everything INSIDE dir, not dir itself.
                    !path.is_empty()
                } else {
                    (0..=path.len()).any(|i| path_matches(rest, &path[i..]))
                }
            }
            Some((head, rest)) => match path.split_first() {
                Some((segment, path_rest)) => {
                    let p: Vec<char> = head.chars().collect();
                    let t: Vec<char> = segment.chars().collect();
                    segment_matches(&p, &t) && path_matches(rest, path_rest)
                }
                None => false,
            },
        }
    }

    fn rule_applies(rule_tool: &str, tool: &str) -> bool {
        rule_tool == tool || (rule_tool == "Edit" && super::WRITE_TOOLS.contains(&tool))
    }

    fn any_rule_matches(
        settings: &serde_json::Value,
        key: &str,
        tool: &str,
        target: &[String],
        settings_root: &Path,
        home: &Path,
    ) -> bool {
        settings
            .pointer(&format!("/permissions/{key}"))
            .and_then(|v| v.as_array())
            .map(|rules| {
                rules.iter().filter_map(|r| r.as_str()).any(|rule| {
                    let Some((rule_tool, spec)) = parse_rule(rule) else {
                        return false;
                    };
                    rule_applies(rule_tool, tool)
                        && anchor(spec, settings_root, home)
                            .map(|pattern| path_matches(&pattern, target))
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// Whether a `dontAsk` session with `settings` may run `tool` on `path`.
    pub(crate) fn allowed(
        settings: &serde_json::Value,
        tool: &str,
        path: &Path,
        settings_root: &Path,
        home: &Path,
    ) -> bool {
        let Some(target) = normalize(path) else {
            return false;
        };
        !any_rule_matches(settings, "deny", tool, &target, settings_root, home)
            && any_rule_matches(settings, "allow", tool, &target, settings_root, home)
    }
}

#[cfg(test)]
#[path = "tests/bug_1928_contained_boundary_tests.rs"]
mod tests;
