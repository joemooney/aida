//! Data-only inventory of one retained commit and its relay push pipeline.
// trace:TASK-3232 | ai:codex
use crate::ci_receipt::{self, Inventory};
use crate::intent_intake;
use aida_core::transport_policy as transport_process;
use anyhow::{bail, Context, Result};
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

const SOURCE_LIMIT: usize = 64 * 1024 * 1024;
const FILE_LIMIT: usize = 2 * 1024 * 1024;
const CHUNK: usize = 8 * 1024 * 1024;

fn key<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    v.as_mapping()?.get(Value::String(name.into()))
}
fn static_name(s: &str) -> Result<String> {
    if s.is_empty() || s.len() > 512 || s.contains('$') || s.chars().any(char::is_control) {
        bail!("Unsupported dynamic CI identifier");
    }
    Ok(s.into())
}
fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        bail!("CI source inventory deadline exceeded");
    }
    Ok(())
}
// Parse literal test declarations and inline modules, but skip opaque item
// bodies. Constructing expression ASTs for 48+ MiB is unnecessary inventory
// work. Tokens must still be lexically valid; CI remains the compiler.
fn rust_tests(source: &str, tests: &mut BTreeSet<String>) -> Result<()> {
    use syn::parse::discouraged::Speculative;
    use syn::parse::{ParseStream, Parser};
    fn skip(input: ParseStream) -> syn::Result<()> {
        input.step(|cursor| {
            cursor
                .token_tree()
                .map(|(_, next)| ((), next))
                .ok_or_else(|| cursor.error("Missing Rust token"))
        })
    }
    fn scan(input: ParseStream, tests: &mut BTreeSet<String>, depth: usize) -> syn::Result<()> {
        if depth > 128 {
            return Err(input.error("Rust module depth limit"));
        }
        syn::Attribute::parse_inner(input)?;
        while !input.is_empty() {
            let attrs = syn::Attribute::parse_outer(input)?;
            if attrs
                .iter()
                .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "test"))
            {
                input.parse::<syn::Visibility>()?;
                let sig = input.parse::<syn::Signature>()?;
                if !sig.inputs.is_empty() || !input.peek(syn::token::Brace) {
                    return Err(input.error("Unsupported Rust test declaration"));
                }
                let name = static_name(&sig.ident.to_string())
                    .map_err(|_| input.error("Unsupported Rust test identifier"))?;
                tests.insert(name);
                skip(input)?;
            } else {
                let fork = input.fork();
                fork.parse::<syn::Visibility>()?;
                if fork.peek(syn::Token![mod]) {
                    fork.parse::<syn::Token![mod]>()?;
                    fork.parse::<syn::Ident>()?;
                    if fork.peek(syn::Token![;]) {
                        fork.parse::<syn::Token![;]>()?;
                    } else {
                        let content;
                        syn::braced!(content in fork);
                        scan(&content, tests, depth + 1)?;
                    }
                    input.advance_to(&fork);
                } else {
                    skip(input)?;
                }
            }
        }
        Ok(())
    }
    let source = source.trim_start_matches('\u{feff}');
    let source = if source.starts_with("#!") && !source[2..].trim_start().starts_with('[') {
        source.split_once('\n').map(|(_, body)| body).unwrap_or("")
    } else {
        source
    };
    (|input: ParseStream| scan(input, tests, 0))
        .parse_str(source)
        .map_err(|_| anyhow::anyhow!("Unsupported Rust test inventory syntax"))
}

struct Object {
    path: String,
    oid: String,
    size: usize,
}

pub(super) fn inventory(root: &Path, sha: &str) -> Result<Inventory> {
    if !ci_receipt::oid(sha) {
        bail!("Invalid CI source commit");
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let tree = intent_intake::git(root, &["ls-tree", "-rlz", "--full-tree", sha], &[])?;
    let mut objects = Vec::new();
    let mut total = 0;
    let mut rust_files = 0;
    // Preflight only Rust sources and CI entry points. Keep other YAML tree
    // metadata for lazy include lookup, without inspecting unrelated blobs.
    let mut includes = BTreeMap::new();
    for entry in tree.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let line = std::str::from_utf8(entry).context("Invalid CI source tree")?;
        let (meta, path) = line.split_once('\t').context("Invalid CI source entry")?;
        if !(path.ends_with(".rs") || path.ends_with(".yml") || path.ends_with(".yaml")) {
            continue;
        }
        if !path.ends_with(".rs")
            && path != ".gitlab-ci.yml"
            && !path.starts_with(".github/workflows/")
        {
            includes.insert(path, meta);
            continue;
        }
        let fields: Vec<_> = meta.split_whitespace().collect();
        if fields.len() != 4 || fields[0] != "100644" || fields[1] != "blob" {
            bail!("CI inventory requires regular committed files");
        }
        let size: usize = fields[3].parse()?;
        total += size;
        if path.ends_with(".rs") {
            rust_files += 1;
        }
        if size > FILE_LIMIT || total > SOURCE_LIMIT || rust_files > 4096 || objects.len() >= 8192 {
            bail!("CI source inventory exceeds size or file limits");
        }
        objects.push(Object {
            path: path.into(),
            oid: fields[2].into(),
            size,
        });
    }
    let mut tests = BTreeSet::new();
    let mut yaml = BTreeMap::new();
    let mut offset = 0;
    while offset < objects.len() {
        check_deadline(deadline)?;
        let start = offset;
        let mut size = 0;
        while offset < objects.len() && size + objects[offset].size <= CHUNK {
            size += objects[offset].size;
            offset += 1;
        }
        let chunk = &objects[start..offset];
        let input: String = chunk.iter().map(|o| format!("{}\n", o.oid)).collect();
        let mut command = intent_intake::git_command(root);
        command.args(["cat-file", "--batch"]);
        let (status, bytes) = transport_process::bounded(
            command,
            input.as_bytes(),
            size + chunk.len() * 128,
            deadline.min(Instant::now() + Duration::from_secs(30)),
        )?;
        if !status.success() {
            bail!("CI source batch read refused");
        }
        let mut rest = bytes.as_slice();
        for object in chunk {
            check_deadline(deadline)?;
            let end = rest
                .iter()
                .position(|b| *b == b'\n')
                .context("Invalid CI blob header")?;
            let header = std::str::from_utf8(&rest[..end])?;
            if header != format!("{} blob {}", object.oid, object.size) {
                bail!("CI blob identity mismatch");
            }
            rest = &rest[end + 1..];
            if rest.len() <= object.size || rest[object.size] != b'\n' {
                bail!("Truncated CI blob");
            }
            let body = &rest[..object.size];
            rest = &rest[object.size + 1..];
            if object.path.ends_with(".rs") {
                rust_tests(std::str::from_utf8(body)?, &mut tests)?;
            } else {
                yaml.insert(object.path.clone(), body.to_vec());
            }
        }
        if !rest.is_empty() {
            bail!("Trailing CI blob data");
        }
    }
    let mut file_count = objects.len();
    let jobs = if yaml.contains_key(".gitlab-ci.yml") {
        gitlab_jobs_with(|path| {
            check_deadline(deadline)?;
            if let Some(bytes) = yaml.get(path) {
                return Ok(bytes.clone());
            }
            let meta = includes
                .get(path)
                .context("CI local include missing from pinned commit")?;
            let fields: Vec<_> = meta.split_whitespace().collect();
            if fields.len() != 4 || fields[0] != "100644" || fields[1] != "blob" {
                bail!("CI inventory requires regular committed files");
            }
            let size: usize = fields[3].parse()?;
            total += size;
            file_count += 1;
            if size > FILE_LIMIT || total > SOURCE_LIMIT || file_count > 8192 {
                bail!("CI source inventory exceeds size or file limits");
            }
            let mut command = intent_intake::git_command(root);
            command.args(["cat-file", "blob", fields[2]]);
            let (status, bytes) = transport_process::bounded(
                command,
                &[],
                size,
                deadline.min(Instant::now() + Duration::from_secs(30)),
            )?;
            if !status.success() || bytes.len() != size {
                bail!("CI include blob read refused");
            }
            yaml.insert(path.into(), bytes.clone());
            Ok(bytes)
        })?
    } else {
        github_jobs(&yaml)?
    };
    check_deadline(deadline)?;
    if jobs.is_empty() || jobs.len() > 256 || tests.len() > 100_000 {
        bail!("CI inventory exceeds limits or selects no jobs");
    }
    Ok(Inventory { jobs, tests })
}

fn parse(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > 1024 * 1024 {
        bail!("CI YAML exceeds size limit");
    }
    serde_yaml::from_slice(bytes).map_err(|_| anyhow::anyhow!("Invalid committed CI YAML"))
}
fn merge(base: &mut Value, other: Value) {
    if let (Some(base), Some(other)) = (base.as_mapping_mut(), other.as_mapping()) {
        for (k, v) in other {
            merge(base.entry(k.clone()).or_insert(Value::Null), v.clone());
        }
    } else {
        *base = other;
    }
}
fn local_path(path: &str) -> Result<()> {
    if path.len() > 512
        || !(path.ends_with(".yml") || path.ends_with(".yaml"))
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
    {
        bail!("CI include requires a canonical local YAML path");
    }
    Ok(())
}
struct Includes<F> {
    read: F,
    stack: BTreeSet<String>,
    count: usize,
    bytes: usize,
}
impl<F: FnMut(&str) -> Result<Vec<u8>>> Includes<F> {
    fn load(&mut self, path: &str) -> Result<Value> {
        local_path(path)?;
        self.count += 1;
        if self.count > 64 || self.stack.len() >= 16 || !self.stack.insert(path.into()) {
            bail!("CI include cycle or expansion limit");
        }
        let bytes = (self.read)(path)?;
        self.bytes += bytes.len();
        if self.bytes > 4 * 1024 * 1024 {
            bail!("CI include bytes exceed limit");
        }
        let mut own = parse(&bytes)?;
        let mapping = own
            .as_mapping_mut()
            .context("CI config must be a mapping")?;
        let mut merged = Value::Mapping(Mapping::new());
        if let Some(includes) = mapping.remove(Value::String("include".into())) {
            let includes = match includes {
                Value::Sequence(v) => v,
                v => vec![v],
            };
            for include in includes {
                let path = match &include {
                    Value::String(s) => s,
                    Value::Mapping(m) if m.len() == 1 => key(&include, "local")
                        .and_then(Value::as_str)
                        .context("Only committed local CI includes are supported")?,
                    _ => bail!("Only committed local CI includes are supported"),
                };
                merge(&mut merged, self.load(path)?);
            }
        }
        merge(&mut merged, own);
        self.stack.remove(path);
        Ok(merged)
    }
}
fn expanded(
    name: &str,
    config: &Value,
    stack: &mut BTreeSet<String>,
    budget: &mut usize,
) -> Result<Value> {
    *budget += 1;
    if *budget > 1024 {
        bail!("CI template expansion limit");
    }
    if stack.len() >= 11 || !stack.insert(name.into()) {
        bail!("CI extends cycle or depth limit");
    }
    let mut own = key(config, name)
        .context("Missing CI extends template")?
        .clone();
    let mapping = own.as_mapping_mut().context("CI job must be a mapping")?;
    let parents = mapping.remove(Value::String("extends".into()));
    let mut result = Value::Mapping(Mapping::new());
    if let Some(parents) = parents {
        let parents = match parents {
            Value::Sequence(v) => v,
            v => vec![v],
        };
        if parents.len() > 16 {
            bail!("CI extends parent limit");
        }
        for parent in parents {
            let parent = parent.as_str().context("Invalid CI extends template")?;
            merge(&mut result, expanded(parent, config, stack, budget)?);
        }
    }
    merge(&mut result, own);
    stack.remove(name);
    Ok(result)
}

// Closed push context: no user variables, branch-default assumptions, filesystem
// probes, regex engine, or evaluation of returned work data. Unknown syntax
// refuses even in a job that would otherwise be excluded.
fn condition(expr: &str) -> Result<bool> {
    if expr.len() > 1024 {
        bail!("CI rule expression exceeds limit");
    }
    let mut result = Some(false);
    for disjunction in expr.split(" || ") {
        let mut conjunction = Some(true);
        for atom in disjunction.split(" && ") {
            let value = match atom.trim() {
                "$CI_PIPELINE_SOURCE == \"push\"" => Some(true),
                "$CI_PIPELINE_SOURCE == \"schedule\""
                | "$CI_PIPELINE_SOURCE == \"web\""
                | "$CI_PIPELINE_SOURCE == \"merge_request_event\""
                | "$CI_MERGE_REQUEST_IID"
                | "$CI_COMMIT_TAG"
                | "$CI_COMMIT_TAG =~ /^v[0-9]+/" => Some(false),
                "$CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH" | "$PUBLISH_PAGES == \"true\"" => None,
                "$CI_COMMIT_BRANCH =~ /^ci\\/[1-9][0-9]*$/" => Some(true),
                _ => bail!("Unsupported CI rule expression"),
            };
            conjunction = match (conjunction, value) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
        }
        result = match (result, conjunction) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        };
    }
    result.context("CI rule depends on unknown deployment variables")
}
fn selected(rules: Option<&Value>) -> Result<bool> {
    let Some(rules) = rules else {
        return Ok(true);
    };
    let rules = rules.as_sequence().context("CI rules must be an array")?;
    if rules.is_empty() || rules.len() > 64 {
        bail!("CI rules exceed limits");
    }
    let mut first = None;
    for rule in rules {
        let m = rule.as_mapping().context("Invalid CI rule")?;
        if m.keys().any(|k| !matches!(k.as_str(), Some("if" | "when"))) {
            bail!("Unsupported dynamic CI rule");
        }
        let matches = match key(rule, "if") {
            Some(v) => condition(v.as_str().context("Invalid CI condition")?)?,
            None => true,
        };
        let when = key(rule, "when")
            .map(|v| v.as_str().context("Invalid CI rule when"))
            .transpose()?
            .unwrap_or("on_success");
        if !matches!(when, "always" | "on_success" | "never") {
            bail!("Unsupported CI rule when");
        }
        if first.is_none() && matches {
            first = Some(when != "never");
        }
    }
    Ok(first.unwrap_or(false))
}
fn variables(v: Option<&Value>) -> Result<()> {
    if let Some(v) = v {
        let m = v.as_mapping().context("Invalid CI variables")?;
        for k in m.keys() {
            let k = k.as_str().context("Invalid CI variable name")?;
            if k.starts_with("CI_") || k == "PUBLISH_PAGES" {
                bail!("CI selection variables cannot be overridden");
            }
        }
    }
    Ok(())
}
#[cfg(test)]
fn gitlab_jobs(files: &BTreeMap<String, Vec<u8>>) -> Result<BTreeSet<String>> {
    gitlab_jobs_with(|path| {
        files
            .get(path)
            .cloned()
            .context("CI local include missing from pinned commit")
    })
}
fn gitlab_jobs_with(read: impl FnMut(&str) -> Result<Vec<u8>>) -> Result<BTreeSet<String>> {
    let config = Includes {
        read,
        stack: BTreeSet::new(),
        count: 0,
        bytes: 0,
    }
    .load(".gitlab-ci.yml")?;
    let mapping = config.as_mapping().context("Invalid CI mapping")?;
    variables(key(&config, "variables"))?;
    if let Some(workflow) = key(&config, "workflow") {
        let m = workflow.as_mapping().context("Invalid CI workflow")?;
        if m.keys()
            .any(|k| !matches!(k.as_str(), Some("rules" | "auto_cancel")))
        {
            bail!("Unsupported CI workflow");
        }
        if !selected(key(workflow, "rules"))? {
            bail!("CI workflow excludes relay pushes");
        }
    }
    if let Some(default) = key(&config, "default") {
        let m = default.as_mapping().context("Invalid CI defaults")?;
        if m.keys().any(|k| {
            !matches!(
                k.as_str(),
                Some(
                    "image"
                        | "services"
                        | "before_script"
                        | "after_script"
                        | "cache"
                        | "artifacts"
                        | "interruptible"
                        | "retry"
                        | "timeout"
                        | "tags"
                )
            )
        }) {
            bail!("Unsupported CI defaults");
        }
    }
    let mut jobs = BTreeSet::new();
    let mut expansion_budget = 0;
    for name in mapping.keys() {
        let name = name.as_str().context("CI key must be a string")?;
        if matches!(
            name,
            "stages"
                | "variables"
                | "image"
                | "services"
                | "before_script"
                | "after_script"
                | "cache"
                | "workflow"
                | "default"
        ) {
            continue;
        }
        if name == "spec" {
            bail!("Unsupported CI spec inputs");
        }
        let job = expanded(name, &config, &mut BTreeSet::new(), &mut expansion_budget)?;
        let fields = job.as_mapping().context("Invalid CI job")?;
        if fields.keys().any(|k| {
            !matches!(
                k.as_str(),
                Some(
                    "script"
                        | "stage"
                        | "image"
                        | "tags"
                        | "before_script"
                        | "after_script"
                        | "variables"
                        | "services"
                        | "cache"
                        | "artifacts"
                        | "interruptible"
                        | "retry"
                        | "timeout"
                        | "needs"
                        | "dependencies"
                        | "rules"
                        | "release"
                )
            )
        }) {
            bail!("Unsupported conditional, matrix or dynamic GitLab job");
        }
        variables(key(&job, "variables"))?;
        let include = selected(key(&job, "rules"))?;
        if !name.starts_with('.') && include {
            if key(&job, "script").is_none() {
                bail!("CI job requires a script");
            }
            jobs.insert(static_name(name)?);
        }
    }
    Ok(jobs)
}
fn github_jobs(files: &BTreeMap<String, Vec<u8>>) -> Result<BTreeSet<String>> {
    let mut jobs = BTreeSet::new();
    let mut count = 0;
    for (path, bytes) in files {
        if !path.starts_with(".github/workflows/") {
            continue;
        }
        count += 1;
        if count > 64 {
            bail!("CI workflow limit");
        }
        let yaml = parse(bytes)?;
        let mapping = key(&yaml, "jobs")
            .and_then(Value::as_mapping)
            .context("Missing GitHub jobs")?;
        for (id, job) in mapping {
            let fields = job.as_mapping().context("Invalid GitHub job")?;
            if key(job, "steps").is_none()
                || fields.keys().any(|k| {
                    !matches!(
                        k.as_str(),
                        Some(
                            "name"
                                | "steps"
                                | "runs-on"
                                | "needs"
                                | "permissions"
                                | "env"
                                | "defaults"
                                | "timeout-minutes"
                                | "services"
                                | "container"
                        )
                    )
                })
            {
                bail!("Unsupported conditional, matrix or reusable GitHub job");
            }
            let name = key(job, "name")
                .unwrap_or(id)
                .as_str()
                .context("Invalid GitHub job name")?;
            if !jobs.insert(static_name(name)?) {
                bail!("Duplicate CI job name");
            }
        }
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn files(yaml: &str) -> BTreeMap<String, Vec<u8>> {
        [(".gitlab-ci.yml".into(), yaml.as_bytes().to_vec())].into()
    }
    #[test]
    fn literal_test_scan_skips_bodies_macros_and_impls() {
        let source = r##"#!/usr/bin/env rust-script
            #![allow(dead_code)]
            // #[test] fn comment_is_not_a_test() {}
            const TEXT: &str = "#[test] fn string_is_not_a_test() {}";
            fn ordinary() { #[test] fn nested_function_is_not_a_top_level_test() {} }
            macro_rules! generated { () => { #[test] fn not_literal() {} }; }
            impl Thing { #[test] fn method_is_not_a_free_test() {} }
            #[cfg(test)] pub(crate) mod outer {
                //! inner documentation
                #[tokio::test(flavor = "current_thread")]
                async fn known_async() { let _ = async { 1 }; }
                mod external;
                pub mod inner { #[test] pub fn known_nested() {} }
            }
            #[test] fn known_root() {}
        "##;
        let mut tests = BTreeSet::new();
        rust_tests(source, &mut tests).unwrap();
        assert_eq!(
            tests,
            [
                "known_async".into(),
                "known_nested".into(),
                "known_root".into()
            ]
            .into()
        );
        for unsupported in [
            "#[test] fn parameterized(x: u32) {}",
            "#[test] fn missing_body();",
            "mod broken {",
            "#[test] const NOT_A_FUNCTION: u32 = 0;",
        ] {
            assert!(rust_tests(unsupported, &mut BTreeSet::new()).is_err());
        }
    }
    #[test]
    fn bulk_inventory_uses_pinned_blobs_and_refuses_nonregular_and_oversized_sources() {
        let repo = tempfile::tempdir().unwrap();
        // trace:TASK-3232 | ai:codex
        let git = |args: &[&str]| {
            let mut guarded = aida_core::git_ops::NO_AUTO_MAINTENANCE.to_vec();
            guarded.extend_from_slice(args);
            intent_intake::git(repo.path(), &guarded, &[]).unwrap()
        };
        git(&["init", "-q", "--template=", "-b", "main"]);
        std::fs::write(repo.path().join(".gitlab-ci.yml"), "include: local.yml\n").unwrap();
        std::fs::write(repo.path().join("local.yml"), "job: {script: cargo test}\n").unwrap();
        std::fs::write(repo.path().join("lib.rs"), "#[test] fn pinned() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "pinned inventory"]);
        let sha = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        let sha = sha.trim();
        std::fs::write(repo.path().join("local.yml"), "evil: {script: injected}\n").unwrap();
        std::fs::write(repo.path().join("lib.rs"), "#[test] fn dirty() {}\n").unwrap();
        let inv = inventory(repo.path(), sha).unwrap();
        assert_eq!(inv.jobs, ["job".into()].into());
        assert_eq!(inv.tests, ["pinned".into()].into());
        std::fs::remove_file(repo.path().join("local.yml")).unwrap();
        std::os::unix::fs::symlink("lib.rs", repo.path().join("local.yml")).unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "symlink refused"]);
        let symlink_sha = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        assert!(inventory(repo.path(), symlink_sha.trim()).is_err());
        std::fs::remove_file(repo.path().join("local.yml")).unwrap();
        std::fs::write(repo.path().join("local.yml"), "job: {script: cargo test}\n").unwrap();
        std::fs::write(repo.path().join("lib.rs"), vec![b' '; FILE_LIMIT + 1]).unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "oversized source refused"]);
        let oversized_sha = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        assert!(inventory(repo.path(), oversized_sha.trim()).is_err());
    }
    // trace:TASK-3232 | ai:codex
    #[test]
    fn unrelated_yaml_is_ignored_but_included_yaml_is_validated() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        for gitlab in [false, true] {
            let repo = tempfile::tempdir().unwrap();
            let git = |args: &[&str]| {
                let mut guarded = aida_core::git_ops::NO_AUTO_MAINTENANCE.to_vec();
                guarded.extend_from_slice(args);
                intent_intake::git(repo.path(), &guarded, &[]).unwrap()
            };
            git(&["init", "-q", "--template=", "-b", "main"]);
            if gitlab {
                std::fs::write(repo.path().join(".gitlab-ci.yml"), "job: {script: test}\n")
                    .unwrap();
            } else {
                std::fs::create_dir_all(repo.path().join(".github/workflows")).unwrap();
                std::fs::write(
                    repo.path().join(".github/workflows/test.yml"),
                    "jobs: {job: {steps: [{run: test}]}}\n",
                )
                .unwrap();
            }
            std::fs::write(repo.path().join("lib.rs"), "#[test] fn known() {}\n").unwrap();
            symlink("lib.rs", repo.path().join("mkdocs.yml")).unwrap();
            std::fs::write(repo.path().join("openapi.yaml"), vec![b' '; FILE_LIMIT + 1]).unwrap();
            std::fs::write(repo.path().join("executable.yml"), "job: {script: test}\n").unwrap();
            std::fs::set_permissions(
                repo.path().join("executable.yml"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            git(&["add", "."]);
            git(&["commit", "-qm", "unrelated YAML"]);
            let sha = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
            let inv = inventory(repo.path(), sha.trim()).unwrap();
            assert_eq!(inv.jobs, ["job".into()].into());
            assert_eq!(inv.tests, ["known".into()].into());
            for path in [
                "mkdocs.yml",
                "openapi.yaml",
                "executable.yml",
                "missing.yml",
            ] {
                std::fs::write(
                    repo.path().join(".gitlab-ci.yml"),
                    format!("include: {path}\n"),
                )
                .unwrap();
                git(&["add", ".gitlab-ci.yml"]);
                git(&["commit", "-qm", "unsafe include"]);
                let sha = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
                assert!(inventory(repo.path(), sha.trim()).is_err(), "{path}");
            }
        }
    }
    #[test]
    fn committed_aida_templates_select_only_relay_merge_jobs() {
        let mut files = files(include_str!("../../.gitlab-ci.yml"));
        files.insert(
            "aida-core/templates/gitlab-ci-merge-hold-gate.yml".into(),
            include_bytes!("../../aida-core/templates/gitlab-ci-merge-hold-gate.yml").to_vec(),
        );
        // The seven literal adapter names must be admitted by the production
        // relay reader, including its local-include and template expansion.
        // trace:STORY-2606 | ai:codex
        files.insert(
            ".gitlab/ci/gdma.yml".into(),
            include_bytes!("../../.gitlab/ci/gdma.yml").to_vec(),
        );
        let mut expected: BTreeSet<String> =
            ["bwrap-confinement".into(), "test".into(), "verify".into()].into();
        for lane in [
            "lib-0",
            "lib-1",
            "cli-0",
            "cli-1",
            "rest",
            "gates",
            "contracts",
        ] {
            expected.insert(format!("gdma-ci: [{lane}]"));
        }
        assert_eq!(gitlab_jobs(&files).unwrap(), expected);
    }
    #[test]
    fn local_includes_merge_before_root_and_extends_uses_reverse_deep_merge() {
        let mut files = files("include: [{local: ci/one.yml}, {local: ci/two.yml}]\n.base:\n  script: cargo test\n  rules: [{if: '$CI_PIPELINE_SOURCE == \"push\"'}]\njob:\n  extends: [.other, .base]\n");
        files.insert(
            "ci/one.yml".into(),
            b".other:\n  script: ignored\n  rules: [{when: never}]\njob:\n  script: original\n"
                .to_vec(),
        );
        files.insert("ci/two.yml".into(), b"job:\n  script: replaced\n".to_vec());
        assert_eq!(gitlab_jobs(&files).unwrap(), ["job".into()].into());
        let config = Includes {
            read: |path: &str| files.get(path).cloned().context("Missing test include"),
            stack: BTreeSet::new(),
            count: 0,
            bytes: 0,
        }
        .load(".gitlab-ci.yml")
        .unwrap();
        assert_eq!(
            key(
                &expanded("job", &config, &mut BTreeSet::new(), &mut 0).unwrap(),
                "script"
            )
            .unwrap()
            .as_str(),
            Some("replaced")
        );
    }
    #[test]
    fn includes_refuse_remote_commit_path_cycle_and_expansion_escapes() {
        for include in [
            "{remote: 'https://example.invalid/config.yml'}",
            "{project: other, ref: main, file: ci.yml}",
            "{local: '../escape.yml'}",
            "{local: '/ci.yml'}",
            "{local: 'HEAD:ci.yml'}",
            "{local: '$CONFIG.yml'}",
            "{local: '*.yml'}",
            "{local: 'missing.yml'}",
            "{local: '.gitlab-ci.yml'}",
            "{local: 'ci.yml', rules: []}",
        ] {
            assert!(
                gitlab_jobs(&files(&format!(
                    "include: {include}\njob: {{script: test}}\n"
                )))
                .is_err(),
                "{include}"
            );
        }
        let mut files = files("include: ci.yml\njob: {script: test}\n");
        files.insert("ci.yml".into(), b"include: .gitlab-ci.yml\n".to_vec());
        assert!(gitlab_jobs(&files).is_err());
        files.insert(
            "ci.yml".into(),
            format!("include: [{}]\n", vec!["other.yml"; 65].join(",")).into_bytes(),
        );
        files.insert("other.yml".into(), b"{}".to_vec());
        assert!(gitlab_jobs(&files).is_err());
    }
    #[test]
    fn unsupported_selection_and_template_cycles_fail_closed() {
        for yaml in [
            "job: {script: test, rules: [{changes: ['*.rs']}]}",
            "job: {script: test, rules: [{if: '$UNKNOWN'}]}",
            "job: {script: test, rules: [{if: '$CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH'}]}",
            "job: {script: test, rules: [{when: manual}]}",
            "job: {script: test, parallel: 2}",
            "job: {script: test, variables: {CI_PIPELINE_SOURCE: web}}",
            "workflow: {rules: [{when: never}]}\njob: {script: test}",
            ".base: {extends: job}\njob: {extends: .base, script: test}",
            "job: {extends: .missing, script: test}",
            "job: {script: test}\njob: {script: other}",
        ] {
            assert!(gitlab_jobs(&files(yaml)).is_err(), "{yaml}");
        }
        assert!(
            !condition("$CI_PIPELINE_SOURCE == \"web\" && $PUBLISH_PAGES == \"true\"").unwrap()
        );
        let jobs = gitlab_jobs(&files(
            "job: {script: test, rules: [{when: never}, {when: always}]}\nother: {script: test}",
        ))
        .unwrap();
        assert_eq!(jobs, ["other".into()].into());
    }
}
