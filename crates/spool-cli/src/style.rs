//! The colours of what the commands print. They are written whatever the
//! output is, and taken out again where it is not a terminal, where
//! `--color never` or NO_COLOR says so, and in JSON, NDJSON and CSV, which
//! are never given any.

use anstyle::{AnsiColor, Effects, Style};

pub const HEADER: Style = Style::new().bold();
pub const ID: Style = Style::new().dimmed();
pub const LABEL: Style = Style::new().bold();
pub const GOOD: Style = AnsiColor::Green.on_default();
pub const BAD: Style = AnsiColor::Red.on_default();
pub const WAITING: Style = AnsiColor::Yellow.on_default();
pub const ERROR: Style = AnsiColor::BrightRed.on_default().effects(Effects::BOLD);
pub const WARNING: Style = AnsiColor::Yellow.on_default().effects(Effects::BOLD);
pub const HINT: Style = AnsiColor::BrightCyan.on_default().effects(Effects::BOLD);
pub const DONE: Style = AnsiColor::BrightGreen.on_default().effects(Effects::BOLD);

/// Text in a style.
pub fn paint(style: Style, text: &str) -> String {
    format!("{style}{text}{style:#}")
}

/// The style of a status, by what it says: done, gone wrong, or waiting.
pub fn status(text: &str) -> Style {
    match text {
        "succeeded" | "committed" | "resolved" | "current" => GOOD,
        "failed" | "rejected" => BAD,
        "running" | "open" | "drafted" => WAITING,
        _ => Style::new(),
    }
}

/// The help and the usage errors in the colours cargo gives them.
pub fn clap() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .header(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
        .usage(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
        .literal(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
        .placeholder(AnsiColor::Cyan.on_default())
        .error(ERROR)
        .valid(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
        .invalid(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
}
