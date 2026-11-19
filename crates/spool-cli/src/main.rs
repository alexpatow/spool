//! The `spool` command.

mod commands;
mod output;
mod progress;
#[cfg(test)]
mod reference;
mod style;

use std::ffi::OsString;
use std::process::ExitCode;

use anstream::eprintln;
use clap::{ColorChoice, CommandFactory, FromArgMatches, Parser, Subcommand};

use commands::{
    bench, extract, init, load, matching, project, recompute, reset, retract, serve, show, token,
    update,
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
    /// When to use colour
    ///
    /// `auto` uses it on a terminal, unless NO_COLOR is set, and where
    /// CLICOLOR_FORCE is. JSON, NDJSON and CSV never have colour.
    #[arg(
        long,
        value_name = "WHEN",
        default_value_t = ColorChoice::Auto,
        global = true,
        help_heading = "Global options"
    )]
    color: ColorChoice,
    /// Print only the result and errors: no hints or progress
    #[arg(short, long, global = true, help_heading = "Global options")]
    quiet: bool,
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
    Update(update::UpdateArgs),
    Build(project::BuildArgs),
    Apply(project::ApplyArgs),
    Serve(serve::ServeArgs),
    Token(token::TokenArgs),
    Bench(bench::BenchArgs),
    Schema(commands::SchemaArgs),
    Completions(CompletionsArgs),
    #[command(hide = true)]
    Man(ManArgs),
}

/// Print a shell completion script
///
/// Prints the script that completes `spool`'s commands, options and their
/// values in a shell. The release puts the ones for bash, zsh and fish where
/// those shells find them; this prints them for another place, or another
/// shell.
#[derive(clap::Args)]
#[command(
    after_help = "Examples:\n  spool completions bash > ~/.local/share/bash-completion/completions/spool\n  spool completions zsh > ~/.zfunc/_spool\n  spool completions fish > ~/.config/fish/completions/spool.fish"
)]
struct CompletionsArgs {
    /// The shell
    shell: clap_complete::Shell,
}

/// Write a man page for `spool` and for each of its commands
#[derive(clap::Args)]
struct ManArgs {
    /// The directory to write them in, made if it is not there
    dir: std::path::PathBuf,
}

/// How a command ended, other than well: what went wrong, and what to do
/// about it where there is something.
pub struct Failure {
    kind: Kind,
    message: String,
    hint: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The arguments were wrong. Printed with the command's usage; exit 2.
    Usage,
    /// The command failed. Printed; exit 1.
    Failed,
    /// The command was stopped with Ctrl-C, and says what that left.
    /// Printed; exit 130, as for a command the signal ended.
    Stopped,
}

impl Failure {
    pub fn usage(message: impl Into<String>) -> Failure {
        Failure {
            kind: Kind::Usage,
            message: message.into(),
            hint: None,
        }
    }

    pub fn failed(message: impl Into<String>) -> Failure {
        Failure {
            kind: Kind::Failed,
            message: message.into(),
            hint: None,
        }
    }

    pub fn stopped(message: impl Into<String>) -> Failure {
        Failure {
            kind: Kind::Stopped,
            message: message.into(),
            hint: None,
        }
    }

    /// More that the failure says, on a line of its own after it.
    pub fn and(mut self, more: impl AsRef<str>) -> Failure {
        self.message.push('\n');
        self.message.push_str(more.as_ref());
        self
    }

    /// What to do about it, said after it as `hint:`.
    pub fn hint(mut self, hint: impl Into<String>) -> Failure {
        self.hint = Some(hint.into());
        self
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// What the failure says, with its hint if it has one.
    pub fn message(self) -> String {
        match self.hint {
            Some(hint) => format!("{}\nhint: {hint}", self.message),
            None => self.message,
        }
    }
}

impl From<spool::Error> for Failure {
    fn from(error: spool::Error) -> Self {
        Failure::failed(error.to_string())
    }
}

/// The colour `--color` asks for, read before the arguments are parsed, so
/// that the help and the usage errors follow it as well.
fn color(arguments: &[OsString]) -> ColorChoice {
    let mut given = None;
    let mut words = arguments.iter().skip(1).map(|word| word.to_string_lossy());
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if let Some(value) = word.strip_prefix("--color=") {
            given = Some(value.to_owned());
        } else if word == "--color" {
            given = words.next().map(|value| value.into_owned());
        }
    }
    match given.as_deref() {
        Some("always") => ColorChoice::Always,
        Some("never") => ColorChoice::Never,
        _ => ColorChoice::Auto,
    }
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let choice = color(&arguments);
    anstream::ColorChoice::write_global(match choice {
        ColorChoice::Auto => anstream::ColorChoice::Auto,
        ColorChoice::Always => anstream::ColorChoice::Always,
        ColorChoice::Never => anstream::ColorChoice::Never,
    });
    let command = || Cli::command().color(choice).styles(style::clap());
    let cli = match Cli::from_arg_matches(&command().get_matches_from(&arguments)) {
        Ok(cli) => cli,
        Err(error) => error.format(&mut command()).exit(),
    };
    let Some(command) = cli.command else {
        let _ = Cli::command()
            .color(choice)
            .styles(style::clap())
            .print_help();
        return ExitCode::SUCCESS;
    };
    output::set_quiet(cli.quiet);
    if !matches!(command, Command::Serve(_) | Command::Dev(_)) {
        end_quietly_when_the_output_closes();
    }
    let (path, ended) = run(command);
    let Err(failure) = ended else {
        return ExitCode::SUCCESS;
    };
    if failure.kind == Kind::Usage {
        let mut cli = Cli::command().color(choice).styles(style::clap());
        cli.build();
        let mut command = &mut cli;
        for name in path {
            command = command
                .find_subcommand_mut(name)
                .expect("a command that ran is one the parser knows");
        }
        let message = match &failure.hint {
            Some(hint) => format!("{}\n\n  tip: {hint}", failure.message),
            None => failure.message,
        };
        let _ = command
            .error(clap::error::ErrorKind::ValueValidation, message)
            .print();
        return ExitCode::from(2);
    }
    eprintln!(
        "{} {}",
        style::paint(style::ERROR, "error:"),
        failure.message
    );
    if let Some(hint) = &failure.hint {
        eprintln!("{} {hint}", style::paint(style::HINT, "hint:"));
    }
    match failure.kind {
        Kind::Stopped => ExitCode::from(130),
        _ => ExitCode::FAILURE,
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
        Command::Update(args) => (vec!["update"], update::update(args)),
        Command::Build(args) => (vec!["build"], project::build(args)),
        Command::Apply(args) => (vec!["apply"], project::apply(args)),
        Command::Serve(args) => (vec!["serve"], serve::serve(args)),
        Command::Token(args) => (vec!["token", args.action.name()], token::token(args)),
        Command::Bench(args) => (vec!["bench"], bench::bench(args)),
        Command::Schema(args) => (vec!["schema"], commands::schema(args)),
        Command::Completions(args) => {
            let mut command = Cli::command();
            let mut out = std::io::stdout().lock();
            clap_complete::generate(args.shell, &mut command, "spool", &mut out);
            (vec!["completions"], Ok(()))
        }
        Command::Man(args) => (vec!["man"], man(&args.dir)),
    }
}

fn man(dir: &std::path::Path) -> Result<(), Failure> {
    let wrote =
        std::fs::create_dir_all(dir).and_then(|()| clap_mangen::generate_to(Cli::command(), dir));
    wrote.map_err(|error| Failure::failed(format!("{}: {error}", dir.display())))
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

    /// The command reference of the documentation, against what the
    /// commands say. `SNAPSHOTS=overwrite cargo test` writes it again.
    #[test]
    fn the_command_reference_is_what_the_commands_say() {
        let page = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../site/content/docs/cli-reference.mdx");
        snapbox::Assert::new().action_env("SNAPSHOTS").eq(
            super::reference::page(&mut Cli::command()),
            snapbox::Data::read_from(&page, None).raw(),
        );
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
