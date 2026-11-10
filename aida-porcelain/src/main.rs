//! First-party read-only report tenants. trace:STORY-2704 | ai:codex
mod flow;
mod flow_runtime;
use std::{ffi::OsString, io::IsTerminal, path::Path};

fn parse(argv: Vec<OsString>) -> Result<clap::ArgMatches, clap::Error> {
    let name = argv
        .first()
        .and_then(|s| Path::new(s).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let flow = flow::command();
    let root = clap::Command::new("aida-porcelain")
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            clap::Arg::new("format")
                .long("format")
                .value_parser(["human", "toon", "json"])
                .global(true),
        )
        .subcommand_required(true)
        .subcommand(flow);
    match name {
        "aida-porcelain" => root.try_get_matches_from(argv),
        "aida-flow" => {
            let mut expanded = vec![OsString::from("aida-porcelain"), OsString::from("flow")];
            expanded.extend(argv.into_iter().skip(1));
            root.try_get_matches_from(expanded)
        }
        _ => Err(clap::Error::raw(clap::error::ErrorKind::InvalidValue,
            "unknown invocation name; usage: aida-porcelain flow report ... | aida-flow report ...\n")),
    }
}
fn main() {
    let matches = parse(std::env::args_os().collect()).unwrap_or_else(|e| e.exit());
    let environment = aida_core::exec_context::Environment::capture();
    let format = matches
        .get_one::<String>("format")
        .map(String::as_str)
        .or_else(|| environment.var("AIDA_OUTPUT_FORMAT").ok())
        .map(|value| value.trim().to_ascii_lowercase());
    let output_json = format.as_deref() == Some("json");
    let agent = match format.as_deref() {
        Some("human") => false,
        Some("toon" | "json") => true,
        _ => environment.var("AIDA_AGENT_OUTPUT").map_or_else(
            |_| !std::io::stdout().is_terminal(),
            |value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "" | "0" | "false" | "no" | "off"
                )
            },
        ),
    };
    if let Err(e) = flow::run(
        matches.subcommand_matches("flow").expect("required tenant"),
        output_json,
        if agent { "toon" } else { "human" },
    ) {
        if let Some(failure) = e.downcast_ref::<flow_runtime::ReaderFailure>() {
            use std::io::Write;
            use std::os::unix::process::ExitStatusExt;
            let _ = std::io::stdout().write_all(&failure.0.stdout);
            let _ = std::io::stderr().write_all(&failure.0.stderr);
            std::process::exit(
                failure
                    .0
                    .status
                    .code()
                    .unwrap_or_else(|| 128 + failure.0.status.signal().unwrap_or(1)),
            );
        }
        // Preserve the built-in's runtime-error channel and TOON scalar even
        // for --format json. Report JSON itself remains unchanged.
        let message = format!("{e:?}");
        if agent {
            let summary = message.lines().next().unwrap_or("").trim();
            let summary = summary
                .strip_prefix("Error: ")
                .or_else(|| summary.strip_prefix("error: "))
                .unwrap_or(summary);
            let quoted = summary
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            println!("error: \"{quoted}\"");
        } else {
            eprintln!("Error: {message}");
        }
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lexical_multicall_and_unknown_names() {
        for argv in [
            vec![
                "/bin/aida-porcelain",
                "flow",
                "report",
                "--since",
                "2026-11-09",
            ],
            vec!["/bin/aida-flow", "report", "--since", "2026-11-09"],
        ] {
            assert!(parse(argv.into_iter().map(Into::into).collect()).is_ok());
        }
        assert_eq!(parse(vec!["aida-other".into()]).unwrap_err().exit_code(), 2);
    }
}
