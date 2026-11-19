//! STORY-2903 S1c: one production merge boundary. No production code calls
//! `Forge::merge_change` except `landing_merge::merge`, so a new merge path
//! cannot skip the landing checks.
// trace:STORY-2903 | ai:claude

/// The production `merge_change` calls in one source file, as line
/// numbers: method calls (`x.merge_change(..)`) and path calls
/// (`Forge::merge_change(..)`), outside items gated `#[cfg(test)]` or marked
/// `#[test]`. Parsed with `syn`, so a test module anywhere in the file hides
/// nothing after it (finding F6).
fn merge_change_calls(source: &str) -> Result<Vec<usize>, String> {
    use syn::visit::Visit;
    struct Calls(Vec<usize>);
    fn test_only(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            a.path().is_ident("test")
                || (a.path().is_ident("cfg")
                    && a.parse_args::<syn::Meta>()
                        .is_ok_and(|m| m.path().is_ident("test")))
        })
    }
    impl<'ast> Visit<'ast> for Calls {
        fn visit_item(&mut self, item: &'ast syn::Item) {
            let attrs = match item {
                syn::Item::Fn(i) => &i.attrs,
                syn::Item::Mod(i) => &i.attrs,
                syn::Item::Impl(i) => &i.attrs,
                syn::Item::Static(i) => &i.attrs,
                syn::Item::Const(i) => &i.attrs,
                syn::Item::Use(i) => &i.attrs,
                syn::Item::Struct(i) => &i.attrs,
                syn::Item::Trait(i) => &i.attrs,
                _ => return syn::visit::visit_item(self, item),
            };
            if !test_only(attrs) {
                syn::visit::visit_item(self, item);
            }
        }
        fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
            if !test_only(&f.attrs) {
                syn::visit::visit_impl_item_fn(self, f);
            }
        }
        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            if call.method == "merge_change" {
                self.0.push(call.method.span().start().line);
            }
            syn::visit::visit_expr_method_call(self, call);
        }
        // A call inside a macro's tokens (`ensure!(f.merge_change(..))`) is
        // a call too; a string literal (`".merge_change("`) is not.
        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            let tokens = mac.tokens.to_string();
            for _ in tokens.matches("merge_change (") {
                self.0.push(mac.path.segments[0].ident.span().start().line);
            }
            syn::visit::visit_macro(self, mac);
        }
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if let syn::Expr::Path(p) = &*call.func {
                if p.path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "merge_change")
                {
                    self.0
                        .push(p.path.segments.last().unwrap().ident.span().start().line);
                }
            }
            syn::visit::visit_expr_call(self, call);
        }
    }
    let file = syn::parse_file(source).map_err(|e| e.to_string())?;
    let mut calls = Calls(Vec::new());
    calls.visit_file(&file);
    Ok(calls.0)
}

/// Whether `path` is a test-only source by the repository's convention
/// (a mounted test module or an integration test).
fn test_source(rel: &std::path::Path) -> bool {
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    rel.components().any(|c| c.as_os_str() == "tests")
        || name == "tests.rs"
        || name.ends_with("_tests.rs")
}

/// Every production `merge_change` call in the workspace's crates, as
/// `path:line`.
fn merge_change_callers() -> Vec<String> {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut found = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = std::fs::read_dir(&workspace)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("src"))
        .filter(|p| p.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path.strip_prefix(&workspace).unwrap().to_path_buf();
            if path.extension().is_none_or(|e| e != "rs") || test_source(&rel) {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // A file that is not a Rust module on its own (an `include!`d
            // fragment) is scanned as text, conservatively.
            let lines = merge_change_calls(&text).unwrap_or_else(|_| {
                text.lines()
                    .enumerate()
                    .filter(|(_, l)| l.contains("merge_change("))
                    .filter(|(_, l)| !l.trim_start().starts_with("//"))
                    .filter(|(_, l)| !l.contains("fn merge_change"))
                    .map(|(n, _)| n + 1)
                    .collect()
            });
            for line in lines {
                found.push(format!("{}:{line}", rel.display()));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn only_the_landing_boundary_calls_merge_change() {
    let callers = merge_change_callers();
    assert_eq!(callers.len(), 1, "{callers:?}");
    assert!(
        callers[0].starts_with("aida-cli-lib/src/landing_merge.rs:"),
        "every merge path goes through landing_merge::merge: {callers:?}"
    );
}

/// F6: a direct forge merge added to any existing merge caller, even after
/// its test modules, is caught; test items and definitions are not calls.
#[test]
fn a_bypass_in_any_merge_caller_is_caught() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in [
        "pr_cmd.rs",
        "phase_driver_trait.rs",
        "burndown_run.rs",
        "upgrade_release.rs",
        "pr_train.rs",
    ] {
        let text = std::fs::read_to_string(src.join(file)).unwrap();
        assert_eq!(merge_change_calls(&text), Ok(Vec::new()), "{file} is clean");
        let bypassed = format!(
            "{text}\nfn bypass(f: &dyn crate::forge::Forge) {{ let _ = f.merge_change(a, b, c); }}\n"
        );
        assert_eq!(
            merge_change_calls(&bypassed).map(|l| l.len()),
            Ok(1),
            "a bypass in {file} must be caught"
        );
        let ufcs = format!("{text}\nfn bypass() {{ let _ = Forge::merge_change(f, a, b, c); }}\n");
        assert_eq!(merge_change_calls(&ufcs).map(|l| l.len()), Ok(1), "{file}");
        let wrapped = format!(
            "{text}\nfn bypass() {{ anyhow::ensure!(f.merge_change(a, b, c).is_ok()); }}\n"
        );
        assert_eq!(
            merge_change_calls(&wrapped).map(|l| l.len()),
            Ok(1),
            "{file}"
        );
    }
    let test_only = "#[cfg(test)]\nmod t { fn x() { f.merge_change(a, b, c); } }\n\
                     #[test]\nfn y() { f.merge_change(a, b, c); }\n\
                     impl F for G { fn merge_change(&self) {} }\n";
    assert_eq!(merge_change_calls(test_only), Ok(Vec::new()));
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repo whose local origin's `main` now carries `[landing] merger =
/// "advisor"` while the caller started under the defaults.
fn repo_whose_policy_changed() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["branch", "feat"]);
    let origin = root.join("origin.git");
    git(
        &root,
        &["clone", "-q", "--bare", "repo", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let other = root.join("other");
    git(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "t@example.com"]);
    git(&other, &["config", "user.name", "T"]);
    std::fs::create_dir_all(other.join(".aida")).unwrap();
    std::fs::write(
        other.join(".aida/config.toml"),
        "[landing]\nmerger = \"advisor\"\n",
    )
    .unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "policy"]);
    git(&other, &["push", "-q", "origin", "main"]);
    (temp, repo)
}

fn change() -> crate::forge::ChangeRef {
    crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: "feat".into(),
        base: "main".into(),
        title: None,
    }
}

fn opts() -> crate::forge::MergeOptions {
    crate::forge::MergeOptions {
        method: crate::forge::MergeMethod::Squash,
        squash_subject: None,
        squash_body: None,
        delete_branch: false,
        match_head: None,
    }
}

/// Every non-train caller is refused at the boundary when the policy
/// changed under it, before the forge is asked to merge (the recording
/// forge errors if it is). The refusal is a review refusal (exit 23).
#[test]
fn every_caller_is_refused_at_the_boundary_before_the_forge_merges() {
    use super::{merge, Caller, Entry, MergeError};
    let (_temp, repo) = repo_whose_policy_changed();
    let forge = crate::forge::fake::RecordingForge::new();
    let entry = Entry {
        policy: crate::landing_policy::LandingPolicy::default(),
        floor: false,
    };
    for caller in [
        Caller::Ship,
        Caller::Drain,
        Caller::Burndown,
        Caller::Release,
    ] {
        let got = merge(
            &forge,
            &repo,
            caller,
            &entry,
            &change(),
            &[],
            &opts(),
            &mut crate::network_retry::NoopSink,
        );
        match got {
            Err(MergeError::Refused(kind, why)) => {
                assert_eq!(kind, crate::gate::ship::ShipRefusal::Review, "{caller:?}");
                assert!(why.contains("landing policy changed"), "{caller:?}: {why}");
            }
            other => panic!("{caller:?} must be refused before the forge: {other:?}"),
        }
    }
    // The train is checked per car by S1t; it reaches the forge.
    let got = merge(
        &forge,
        &repo,
        Caller::Train,
        &entry,
        &change(),
        &[],
        &opts(),
        &mut crate::network_retry::NoopSink,
    );
    assert!(matches!(got, Err(MergeError::Forge(_))), "{got:?}");
}
