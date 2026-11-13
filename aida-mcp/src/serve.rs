use aida_core::Storage;
// trace:TASK-3429 | ai:codex
use aida_cli_lib::mcp_serve::McpServeArgs;

pub fn run_mcp_serve(
    matches: &clap::ArgMatches,
    ctx: &aida_cli_lib::cmdkit::DispatchCtx,
) -> anyhow::Result<aida_cli_lib::cmdkit::Flow> {
    use aida_cli_lib::find_project_root;
    let args = <McpServeArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(aida_cli_lib::cmdkit::Flow::Continue),
        Some(aida_cli_lib::cmdkit::StoreCtx::Distributed {
            store_path,
            project_root_hint,
        }) => {
            let (store_path, project_root_hint) = (*store_path, *project_root_hint);
            // MCP server reads/writes through the same canonical git store the CLI
            // uses. The previous YAML-snapshot approach wrote to
            // `.aida/mcp-cache.yaml` only — invisible to the CLI and overwritten on
            // every MCP restart. trace:BUG-310 | ai:claude
            // trace:BUG-2017 | ai:codex — attach lifecycle allocation only in a
            // clone. Raw directories retain read-only MCP startup; backend
            // admission still refuses any allocation without a Git identity.
            let storage = if aida_core::allocation_authority::in_git_clone(store_path)? {
                Storage::new(store_path).with_dispenser(aida_cli_lib::load_dispenser(store_path)?)
            } else {
                Storage::new(store_path)
            };
            let project_root = aida_cli_lib::store_sync::mcp_serve_project_root(
                store_path,
                project_root_hint,
                find_project_root().ok(),
            );
            aida_cli_lib::cmdkit::returned(crate::run_mcp_server(&storage, project_root))
        }
        Some(aida_cli_lib::cmdkit::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            let (store_path, backend) = (*store_path, *backend);
            let _ = (store_path, backend, &args);
            aida_cli_lib::git_backend_cmd::exit_unsupported_on_git_backend()
        }
        Some(aida_cli_lib::cmdkit::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, _requirements_path) = (*storage, *requirements_path);
            match &args {
                McpServeArgs {} => {
                    // STORY-361: project root for coordination tools.
                    let project_root = find_project_root()
                        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
                    crate::run_mcp_server(&storage, project_root)?;
                }
            }
            Ok(aida_cli_lib::cmdkit::Flow::Continue)
        }
    }
}

// trace:TASK-3429 | ai:codex
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn serve_without_store_continues() {
        let matches = McpServeArgs::command().get_matches_from(["mcp-serve"]);
        let ctx = aida_cli_lib::cmdkit::DispatchCtx {
            globals: None,
            store: None,
        };
        assert!(matches!(
            run_mcp_serve(&matches, &ctx).unwrap(),
            aida_cli_lib::cmdkit::Flow::Continue
        ));
    }
}
