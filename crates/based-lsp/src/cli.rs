//! Process identity/help commands; no protocol or compiler behavior.
use std::process::ExitCode;

pub(super) fn handle() -> Option<ExitCode> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        return None;
    }
    if args.len() != 1 {
        return Some(usage_error());
    }
    match args[0].to_str() {
        Some("--stdio") => None,
        Some("--version" | "-V") => {
            println!("based-lsp {}", based_version::LONG);
            Some(ExitCode::SUCCESS)
        }
        Some("--help" | "-h") => {
            println!("based-lsp: Based DSL language server\n\nUsage: based-lsp [--stdio | --version | --help]\nWithout arguments or with --stdio, serves the Language Server Protocol over stdin/stdout.");
            Some(ExitCode::SUCCESS)
        }
        _ => Some(usage_error()),
    }
}

fn usage_error() -> ExitCode {
    eprintln!("based-lsp: unexpected arguments; run based-lsp --help");
    ExitCode::FAILURE
}
