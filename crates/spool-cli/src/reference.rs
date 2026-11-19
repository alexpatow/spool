//! The command reference of the documentation, written from the definition
//! of the commands: `site/content/docs/cli-reference.mdx`. A test compares
//! the page with what this writes, so the page says what the binary does.

use clap::{Arg, Command};

/// The headings of options that many commands share. The page says them
/// once, and each command says which it takes.
const SHARED: [(&str, &str); 3] = [
    (
        "Output options",
        "These go with every command that prints a result. [Output](/docs/cli#output) says what \
         each format prints.",
    ),
    (
        "Server options",
        "These go with every command that calls a running server.",
    ),
    ("Global options", "These go with every command."),
];

pub fn page(cli: &mut Command) -> String {
    cli.build();
    let mut page = String::from(
        "---\n\
         title: Command reference\n\
         description: Every spool command and option, as the binary's help says them.\n\
         ---\n\n\
         {/* Written from the definition of the commands by `SNAPSHOTS=overwrite cargo test -p \
         spool-cli --bin spool`. To change it, change the doc comments in crates/spool-cli/src. */}\n\n\
         Every command of `spool` and every option it takes, as `spool <command> --help` says \
         them. [Command line](/docs/cli) says what the commands do, with what they print.\n\n\
         ## Options of many commands\n",
    );
    for (heading, about) in SHARED {
        page.push_str(&format!("\n### {heading}\n\n{about}\n\n"));
        let options: Vec<&Arg> = shared(cli, heading);
        for arg in options {
            page.push_str(&option(arg));
        }
    }
    for sub in cli.get_subcommands() {
        command(sub, "spool", &mut page);
    }
    page
}

/// The options under `heading` of the first command that has them.
fn shared<'a>(command: &'a Command, heading: &str) -> Vec<&'a Arg> {
    let here: Vec<&Arg> = command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && arg.get_help_heading() == Some(heading))
        .collect();
    if !here.is_empty() {
        return here;
    }
    command
        .get_subcommands()
        .map(|sub| shared(sub, heading))
        .find(|found| !found.is_empty())
        .unwrap_or_default()
}

fn command(command: &Command, parent: &str, page: &mut String) {
    if command.is_hide_set() || command.get_name() == "help" {
        return;
    }
    let name = format!("{parent} {}", command.get_name());
    page.push_str(&format!("\n## {name}\n\n"));
    let about = command
        .get_long_about()
        .or(command.get_about())
        .map(ToString::to_string)
        .unwrap_or_default();
    page.push_str(&prose(&about));
    page.push_str("\n\n");
    let usage = command.clone().render_usage().to_string();
    let usage = usage.trim_start_matches("Usage: ").trim();
    page.push_str(&format!("```text\n{usage}\n```\n"));

    let shown: Vec<&Arg> = command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && arg.get_id() != "help" && arg.get_id() != "version")
        .collect();
    let positional: Vec<&&Arg> = shown.iter().filter(|arg| arg.is_positional()).collect();
    if !positional.is_empty() {
        page.push_str("\nArguments:\n\n");
        for arg in positional {
            page.push_str(&option(arg));
        }
    }
    let mut headings: Vec<Option<&str>> = Vec::new();
    for arg in shown.iter().filter(|arg| !arg.is_positional()) {
        let heading = arg.get_help_heading();
        if !headings.contains(&heading) {
            headings.push(heading);
        }
    }
    let mut takes = Vec::new();
    for heading in headings {
        if let Some(heading) = heading
            && SHARED.iter().any(|(shared, _)| *shared == heading)
        {
            if heading != "Global options" {
                takes.push(heading);
            }
            continue;
        }
        page.push_str(&format!("\n{}:\n\n", heading.unwrap_or("Options")));
        for arg in shown
            .iter()
            .filter(|arg| !arg.is_positional() && arg.get_help_heading() == heading)
        {
            page.push_str(&option(arg));
        }
    }
    if !takes.is_empty() {
        let links: Vec<String> = takes
            .iter()
            .map(|heading| {
                format!(
                    "[{}](#{})",
                    heading.to_lowercase(),
                    heading.to_lowercase().replace(' ', "-")
                )
            })
            .collect();
        page.push_str(&format!("\nIt takes the {}.\n", links.join(" and the ")));
    }
    if let Some(after) = command.get_after_help() {
        let after = after.to_string();
        if let Some(examples) = after.strip_prefix("Examples:\n") {
            let lines: Vec<&str> = examples.lines().map(str::trim).collect();
            page.push_str(&format!(
                "\nExamples:\n\n{{/* sample: skip examples of the options, not steps that follow \
                 each other */}}\n\n```sh\n{}\n```\n",
                lines.join("\n")
            ));
        }
    }
    for sub in command.get_subcommands() {
        self::command(sub, &name, page);
    }
}

/// An argument or an option, as an item of a list: its name, what it is,
/// and the default, the possible values and the variable clap knows of.
fn option(arg: &Arg) -> String {
    let mut named = Vec::new();
    if let Some(short) = arg.get_short() {
        named.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        named.push(format!("--{long}"));
    }
    let mut name = named.join(", ");
    let takes_value = arg.get_action().takes_values();
    if takes_value {
        let values: Vec<String> = arg
            .get_value_names()
            .map(|names| names.iter().map(ToString::to_string).collect())
            .unwrap_or_else(|| vec![arg.get_id().to_string().to_uppercase()]);
        let mut value = values
            .iter()
            .map(
                |value| match arg.is_positional() && !arg.is_required_set() {
                    true => format!("[{value}]"),
                    false => format!("<{value}>"),
                },
            )
            .collect::<Vec<_>>()
            .join(" ");
        if arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1)
        {
            value.push_str("...");
        }
        name = match name.is_empty() {
            true => value,
            false => format!("{name} {value}"),
        };
    }
    let help = arg
        .get_long_help()
        .or(arg.get_help())
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut paragraphs: Vec<String> = help.split("\n\n").map(prose).collect();
    let mut first = paragraphs.remove(0);
    let mut facts = Vec::new();
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| format!("`{}`", value.to_string_lossy()))
        .collect();
    if takes_value && !defaults.is_empty() {
        facts.push(format!("Default {}.", defaults.join(", ")));
    }
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| format!("`{}`", value.get_name()))
        .collect();
    if takes_value && !possible.is_empty() {
        facts.push(format!("One of {}.", possible.join(", ")));
    }
    if let Some(variable) = arg.get_env() {
        facts.push(format!(
            "Read from `{}` when it is not given.",
            variable.to_string_lossy()
        ));
    }
    if !facts.is_empty() {
        if !first.ends_with('.') {
            first.push('.');
        }
        first = format!("{first} {}", facts.join(" "));
    }
    let mut item = format!("- `{name}`: {first}\n");
    for paragraph in paragraphs {
        item.push_str(&format!("\n  {paragraph}\n"));
    }
    item
}

/// Help text as Markdown that MDX reads as text: a word with a
/// placeholder such as `<DIR>` in it in code, and no braces, which MDX would
/// read as an expression.
fn prose(text: &str) -> String {
    let mut out = String::new();
    let mut code = false;
    for (at, word) in text.trim().split(' ').enumerate() {
        if at > 0 {
            out.push(' ');
        }
        let ticks = word.matches('`').count();
        if code || ticks > 0 {
            out.push_str(word);
            code ^= ticks % 2 == 1;
            continue;
        }
        let placeholder = word
            .find('<')
            .is_some_and(|open| word[open..].contains('>'));
        if placeholder {
            let start = word.len() - word.trim_start_matches(['(', '[']).len();
            let end = word.trim_end_matches(['.', ',', ';', ':', ')', ']']).len();
            out.push_str(&format!(
                "{}`{}`{}",
                &word[..start],
                &word[start..end],
                &word[end..]
            ));
        } else {
            out.push_str(
                &word
                    .replace('{', "\\{")
                    .replace('}', "\\}")
                    .replace('<', "\\<"),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::prose;

    #[test]
    fn help_is_markdown_that_mdx_reads_as_text() {
        assert_eq!(
            prose("the folder <DIR>/.spool/ holds it"),
            "the folder `<DIR>/.spool/` holds it"
        );
        assert_eq!(
            prose("named as <source>:<key>, which"),
            "named as `<source>:<key>`, which"
        );
        assert_eq!(
            prose("[default: <DIR>/.spool/dev.duckdb]"),
            "[default: `<DIR>/.spool/dev.duckdb`]"
        );
        assert_eq!(
            prose("in sources/<name>.yaml."),
            "in `sources/<name>.yaml`."
        );
        assert_eq!(
            prose("in code `<a> b` and {c}"),
            "in code `<a> b` and \\{c\\}"
        );
        assert_eq!(prose("a < b"), "a \\< b");
    }
}
