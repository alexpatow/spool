//! The words the commands and the server put numbers and names into, so
//! that every one of them writes a count or a list the same way.

/// A number of things, as a sentence says it: `1 record`, `2 records`,
/// `3 identities`. `what` is the word for one of them, and has a regular
/// plural.
pub fn count(number: u64, what: &str) -> String {
    let plural = match what.strip_suffix('y') {
        _ if number == 1 => what.to_owned(),
        Some(stem) => format!("{stem}ies"),
        None => format!("{what}s"),
    };
    format!("{number} {plural}")
}

/// Names between backticks, as a sentence lists them: `a`, `b` and `c`.
pub fn listed(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::{count, listed};

    #[test]
    fn a_count_says_one_thing_or_several() {
        assert_eq!(count(1, "record"), "1 record");
        assert_eq!(count(0, "record"), "0 records");
        assert_eq!(count(2, "new version"), "2 new versions");
        assert_eq!(count(1, "identity"), "1 identity");
        assert_eq!(count(3, "identity"), "3 identities");
    }

    #[test]
    fn names_are_listed_as_a_sentence_lists_them() {
        assert_eq!(listed(&[]), "");
        assert_eq!(listed(&["crm"]), "`crm`");
        assert_eq!(listed(&["crm", "notes"]), "`crm` and `notes`");
        assert_eq!(listed(&["a", "b", "c"]), "`a`, `b` and `c`");
    }
}
