use std::path::Path;

use aida_core::scaffolding::agent_pack::{check_portable_pack, sync_portable_pack, PackDrift};
use aida_core::scaffolding::inventory::PORTABLE_PACK;
use aida_core::scaffolding::ScaffoldConfig;

// trace:TASK-1520 | ai:codex
fn main() {
    let Some(command) = std::env::args().nth(1) else {
        usage();
    };
    let root = Path::new(".");
    let config = ScaffoldConfig::default();
    match command.as_str() {
        "sync" => match sync_portable_pack(root, PORTABLE_PACK, &config) {
            Ok(report) => {
                for path in report.written {
                    println!("  wrote: {}", path.display());
                }
                for path in report.removed {
                    println!("  removed: {}", path.display());
                }
                println!("  unchanged: {}", report.unchanged.len());
            }
            Err(error) => fail(error),
        },
        "check" => match check_portable_pack(root, PORTABLE_PACK, &config) {
            Ok(drift) if drift.is_empty() => {
                println!("  OK: .agents/skills (aida-* pack matches aida-core/templates)");
            }
            Ok(drift) => {
                for item in drift {
                    let (kind, path) = match item {
                        PackDrift::Modified(path) => ("modified", path),
                        PackDrift::Missing(path) => ("missing", path),
                        PackDrift::Symlink(path) => ("symlink", path),
                        PackDrift::Orphan(path) => ("orphan", path),
                    };
                    println!("  DRIFT ({kind}): {}", path.display());
                }
                println!("  Run 'make sync-templates' to fix.");
                std::process::exit(1);
            }
            Err(error) => fail(error),
        },
        _ => usage(),
    }
}

// trace:TASK-1520 | ai:codex
fn usage() -> ! {
    eprintln!("usage: agent_skill_pack <sync|check>");
    std::process::exit(2)
}

// trace:TASK-1520 | ai:codex
fn fail(error: std::io::Error) -> ! {
    eprintln!("agent_skill_pack: {error}");
    std::process::exit(1)
}
