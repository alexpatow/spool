//! Command-line arguments, parsed by hand: the commands are few and their
//! options are plain.

use std::collections::HashMap;
use std::path::PathBuf;

/// What a command accepts.
pub struct Spec {
    /// Options that take a value, such as `--store <path>`.
    pub values: &'static [&'static str],
    /// Options that take none, such as `--json`.
    pub flags: &'static [&'static str],
    /// The most arguments that are not options.
    pub positional: usize,
}

#[derive(Debug, Default)]
pub struct Arguments {
    pub positional: Vec<String>,
    values: HashMap<&'static str, String>,
    flags: Vec<&'static str>,
}

/// A mistake in the arguments. The command prints it with its usage and exits
/// with status 2.
pub type Mistake = String;

pub fn parse(arguments: &[String], spec: &Spec) -> Result<Arguments, Mistake> {
    let mut parsed = Arguments::default();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        if let Some(name) = spec.values.iter().find(|name| *name == argument) {
            let value = rest
                .next()
                .ok_or_else(|| format!("`{argument}` needs a value"))?;
            parsed.values.insert(name, value.clone());
        } else if let Some(name) = spec.flags.iter().find(|name| *name == argument) {
            parsed.flags.push(name);
        } else if argument.starts_with('-') && argument.len() > 1 {
            return Err(format!("unknown option `{argument}`"));
        } else if parsed.positional.len() < spec.positional {
            parsed.positional.push(argument.clone());
        } else {
            return Err(format!("unexpected argument `{argument}`"));
        }
    }
    Ok(parsed)
}

impl Arguments {
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.value(name).map(PathBuf::from)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }

    pub fn number(&self, name: &str) -> Result<Option<u64>, Mistake> {
        self.value(name)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| format!("`{name}` needs a whole number, not `{value}`"))
            })
            .transpose()
    }

    /// The project directory: the first argument, or the current directory.
    pub fn project_dir(&self) -> PathBuf {
        self.positional
            .first()
            .map_or_else(|| PathBuf::from("."), PathBuf::from)
    }
}
