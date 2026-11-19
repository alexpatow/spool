//! The `spool` command.

mod commands;

use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

use commands::{
    bench, extract, init, load, matching, project, recompute, reset, retract, serve, show, token,
};

/// Extract, resolve and track entities
#[derive(Parser)]
#[command(
    name = "spool",
    version,
    after_help = "Run `spool <command> --help` for a command's options, or see https://spool.alexpatow.com/docs/cli"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Init(init::InitArgs),
    Info(project::InfoArgs),
    Dev(serve::DevArgs),
    Load(load::LoadArgs),
    Extract(extract::ExtractArgs),
    Match(matching::MatchArgs),
    Recompute(recompute::RecomputeArgs),
    Show(show::ShowArgs),
    Explain(show::ExplainArgs),
    Retract(retract::RetractArgs),
    Reset(reset::ResetArgs),
    Build(project::BuildArgs),
    Apply(project::ApplyArgs),
    Serve(serve::ServeArgs),
    Token(token::TokenArgs),
    Bench(bench::BenchArgs),
    Schema(commands::SchemaArgs),
}

/// How a command ended, other than well.
pub enum Failure {
    /// The arguments were wrong. Printed with the command's usage; exit 2.
    Usage(String),
    /// The command failed. Printed; exit 1.
    Failed(String),
    /// The command was stopped with Ctrl-C, and says what that left.
    /// Printed; exit 130, as for a command the signal ended.
    Stopped(String),
}

impl Failure {
    /// What the failure says, without the usage a wrong command is shown.
    pub fn message(self) -> String {
        match self {
            Failure::Usage(message) | Failure::Failed(message) | Failure::Stopped(message) => {
                message
            }
        }
    }
}

impl From<spool::Error> for Failure {
    fn from(error: spool::Error) -> Self {
        Failure::Failed(error.to_string())
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        let _ = Cli::command().print_help();
        return ExitCode::SUCCESS;
    };
    if !matches!(command, Command::Serve(_) | Command::Dev(_)) {
        end_quietly_when_the_output_closes();
    }
    let (path, ended) = run(command);
    match ended {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            let mut cli = Cli::command();
            cli.build();
            let mut command = &mut cli;
            for name in path {
                command = command
                    .find_subcommand_mut(name)
                    .expect("a command that ran is one the parser knows");
            }
            let _ = command
                .error(clap::error::ErrorKind::ValueValidation, message)
                .print();
            ExitCode::from(2)
        }
        Err(Failure::Failed(message)) => {
            eprintln!("spool: {message}");
            ExitCode::FAILURE
        }
        Err(Failure::Stopped(message)) => {
            eprintln!("spool: {message}");
            ExitCode::from(130)
        }
    }
}

/// Runs a command, and says which it was: the names of the command and of
/// its subcommand, for the usage a mistake is shown with.
fn run(command: Command) -> (Vec<&'static str>, Result<(), Failure>) {
    match command {
        Command::Init(args) => (vec!["init"], init::init(args)),
        Command::Info(args) => (vec!["info"], project::info(args)),
        Command::Dev(args) => (vec!["dev"], serve::dev(args)),
        Command::Load(args) => (vec!["load"], load::load(args)),
        Command::Extract(args) => (vec!["extract"], extract::extract(args)),
        Command::Match(args) => (vec!["match"], matching::run(args)),
        Command::Recompute(args) => (vec!["recompute"], recompute::run(args)),
        Command::Show(args) => {
            let mut path = vec!["show"];
            path.extend(args.what.as_ref().map(show::What::name));
            (path, show::show(args))
        }
        Command::Explain(args) => (vec!["explain"], show::explain(args)),
        Command::Retract(args) => (vec!["retract"], retract::retract(args)),
        Command::Reset(args) => (vec!["reset"], reset::reset(args)),
        Command::Build(args) => (vec!["build"], project::build(args)),
        Command::Apply(args) => (vec!["apply"], project::apply(args)),
        Command::Serve(args) => (vec!["serve"], serve::serve(args)),
        Command::Token(args) => (vec!["token", args.action.name()], token::token(args)),
        Command::Bench(args) => (vec!["bench"], bench::bench(args)),
        Command::Schema(args) => (vec!["schema"], commands::schema(args)),
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn the_parser_is_consistent() {
        Cli::command().debug_assert();
    }

    /// Every command's `-h`, against the snapshot in `tests/snapshots/help`.
    /// `SNAPSHOTS=overwrite cargo test` writes them again.
    #[test]
    fn the_help_of_every_command_is_its_snapshot() {
        fn visit(command: &mut clap::Command, path: &str, found: &mut Vec<String>) {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/help");
            let help = command.render_help().to_string();
            snapbox::Assert::new().action_env("SNAPSHOTS").eq(
                help,
                snapbox::Data::read_from(&dir.join(format!("{path}.txt")), None).raw(),
            );
            found.push(format!("{path}.txt"));
            for sub in command.get_subcommands_mut() {
                if sub.get_name() != "help" {
                    let name = format!("{path}-{}", sub.get_name());
                    visit(sub, &name, found);
                }
            }
        }
        let mut command = Cli::command().term_width(100);
        command.build();
        let mut found = Vec::new();
        visit(&mut command, "spool", &mut found);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/help");
        let mut written: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        written.sort();
        found.sort();
        assert_eq!(written, found, "a snapshot of a command that is gone");
    }
}
