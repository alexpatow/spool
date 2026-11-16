//! The `spool` command.

mod args;
mod commands;

use std::process::ExitCode;

const USAGE: &str = "\
Usage: spool <command> [options]

Commands:
  init     scaffold a project directory
  info     list what a project directory declares
  build    check a project and compile it into a manifest
  apply    apply a manifest to a store
  serve    run the server from a manifest and a store
  dev      run the server on localhost and apply file changes as they are saved
  bench    run a benchmark scenario
  schema   print the JSON Schema of the server's API

`spool <command> --help` describes a command.
";

/// How a command ended, other than well.
pub enum Failure {
    /// The arguments were wrong. Printed with the command's usage; exit 2.
    Usage(String),
    /// The command failed. Printed; exit 1.
    Failed(String),
}

impl From<spool::Error> for Failure {
    fn from(error: spool::Error) -> Self {
        Failure::Failed(error.to_string())
    }
}

type Command = fn(&[String]) -> Result<(), Failure>;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = arguments.split_first() else {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    };
    if !matches!(command.as_str(), "serve" | "dev") {
        end_quietly_when_the_output_closes();
    }
    let (usage, run): (&str, Command) = match command.as_str() {
        "--help" | "-h" | "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "--version" | "-V" => {
            println!("spool {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "init" => (commands::project::INIT_USAGE, commands::project::init),
        "info" => (commands::project::INFO_USAGE, commands::project::info),
        "build" => (commands::project::BUILD_USAGE, commands::project::build),
        "apply" => (commands::project::APPLY_USAGE, commands::project::apply),
        "serve" => (commands::serve::SERVE_USAGE, commands::serve::serve),
        "dev" => (commands::serve::DEV_USAGE, commands::serve::dev),
        "bench" => (commands::bench::USAGE, commands::bench::bench),
        "schema" => (commands::SCHEMA_USAGE, commands::schema),
        other => {
            eprintln!("spool: unknown command `{other}`\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if rest
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{usage}");
        return ExitCode::SUCCESS;
    }
    match run(rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("spool: {message}\n\n{usage}");
            ExitCode::from(2)
        }
        Err(Failure::Failed(message)) => {
            eprintln!("spool: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Rust ignores SIGPIPE, so printing to a pipe whose reader has gone, as in
/// `spool info | head`, panics. A command that prints and exits takes the
/// signal's default instead and ends there. The server does not: it must not
/// die because whatever was reading its output went away.
fn end_quietly_when_the_output_closes() {
    #[cfg(unix)]
    // SAFETY: this sets a signal's disposition to its default, before any
    // other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}
