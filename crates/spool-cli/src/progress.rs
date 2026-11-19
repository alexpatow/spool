//! Progress while a command waits: a spinner or a bar on standard error,
//! redrawn in place.
//!
//! They are drawn on a terminal only, and not under `-q`. Elsewhere a bar is
//! hidden and nothing is drawn: what a command prints when its output is
//! piped is the same however long it took.

use std::borrow::Cow;
use std::io::IsTerminal;
use std::time::Duration;

use anstream::eprintln;
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};

use crate::output::quiet;
use crate::style::{self, paint};

/// Whether progress is drawn.
fn drawn() -> bool {
    !quiet() && std::io::stderr().is_terminal()
}

/// A spinner that says what is being waited for, and for how long.
pub fn spinner(message: impl Into<Cow<'static, str>>) -> ProgressBar {
    if !drawn() {
        return ProgressBar::hidden();
    }
    let bar = ProgressBar::new_spinner()
        .with_message(message)
        .with_finish(ProgressFinish::AndClear);
    bar.set_style(
        ProgressStyle::with_template("{spinner} {msg}  {elapsed}")
            .expect("the template is valid")
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
    );
    bar.enable_steady_tick(Duration::from_millis(100));
    bar
}

/// A bar of `length` steps, `done` of them done. `verb` is said before it,
/// as cargo says `Compiling`, and `of` and `message` after the count:
/// `32/40 documents, 96 records`.
pub fn bar(verb: &str, length: u64, of: &str, done: u64, message: String) -> ProgressBar {
    if !drawn() {
        return ProgressBar::hidden();
    }
    let template = format!(
        "{{prefix:>12}} [{{bar:30}}] {{pos}}/{{len}}{of}{{msg}}  {{eta}} left",
        of = of.replace('{', "{{").replace('}', "}}")
    );
    let bar = ProgressBar::new(length)
        .with_style(
            ProgressStyle::with_template(&template)
                .expect("the template is valid")
                .progress_chars("=> "),
        )
        .with_prefix(verb.to_owned())
        .with_position(done)
        .with_message(message)
        .with_finish(ProgressFinish::AndClear);
    bar.enable_steady_tick(Duration::from_millis(200));
    bar
}

/// A bar of bytes, for a file that is read as it is sent.
pub fn bytes(verb: &str, length: u64) -> ProgressBar {
    let bar = match drawn() {
        true => ProgressBar::new(length).with_finish(ProgressFinish::AndClear),
        false => return ProgressBar::hidden(),
    };
    bar.set_style(
        ProgressStyle::with_template(
            "{prefix:>12} [{bar:30}] {bytes}/{total_bytes}{msg}  {eta} left",
        )
        .expect("the template is valid")
        .progress_chars("=> "),
    );
    bar.set_prefix(verb.to_owned());
    bar.enable_steady_tick(Duration::from_millis(200));
    bar
}

/// A line above the bar on a terminal, and on standard error where no bar
/// is drawn. Nothing under `-q`.
pub fn say(bar: &ProgressBar, line: &str) {
    if quiet() {
        return;
    }
    match bar.is_hidden() {
        true => eprintln!("{line}"),
        false => bar.println(line),
    }
}

/// A step that is done, as cargo says one: the verb in a column of its own,
/// then what was done.
pub fn done(bar: &ProgressBar, verb: &str, what: &str) {
    say(
        bar,
        &format!("{} {what}", paint(style::DONE, &format!("{verb:>12}"))),
    );
}

/// A step that stops or changes course, said as [`done`] says one.
pub fn note(bar: &ProgressBar, verb: &str, what: &str) {
    say(
        bar,
        &format!("{} {what}", paint(style::WARNING, &format!("{verb:>12}"))),
    );
}

/// A time a person reads at a glance: `4s`, `1m14s`, `2h5m`.
pub fn took(time: Duration) -> String {
    let seconds = time.as_secs();
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, seconds) => format!("{minutes}m{seconds}s"),
        (hours, minutes, _) => format!("{hours}h{minutes}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_said_at_a_glance() {
        assert_eq!(took(Duration::from_millis(400)), "0s");
        assert_eq!(took(Duration::from_secs(59)), "59s");
        assert_eq!(took(Duration::from_secs(74)), "1m14s");
        assert_eq!(took(Duration::from_secs(2 * 3600 + 5 * 60 + 9)), "2h5m");
    }
}
