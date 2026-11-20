//! A transform's file: the lambda it is written as, and the words of
//! its SQL.

use super::{SqlFile, Transform, is_sql_identifier, trim_sql};

/// A transform's file: `lambda` and the names of its arguments, a colon, and
/// an expression over them, as DuckDB writes a lambda.
pub(super) fn transform_of(file: &SqlFile) -> Result<Transform, String> {
    const FORM: &str = "a transform is `lambda`, the names of its arguments, `:` and an expression, as in `lambda name: lower(trim(name))`";
    if !is_sql_identifier(&file.name) {
        return Err(
            "a transform's file name may contain only lowercase letters, digits and `_`".to_owned(),
        );
    }
    let text = after_comments(&file.sql);
    let rest = text
        .strip_prefix("lambda")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .ok_or(FORM)?;
    let (header, body) = rest.split_once(':').ok_or(FORM)?;
    let mut parameters: Vec<String> = Vec::new();
    for parameter in header.split(',').map(str::trim) {
        if !is_sql_identifier(parameter) {
            return Err(format!(
                "`{parameter}` cannot name an argument: use lowercase letters, digits and `_`"
            ));
        }
        if parameters.iter().any(|other| other == parameter) {
            return Err(format!("the argument `{parameter}` is named twice"));
        }
        parameters.push(parameter.to_owned());
    }
    let body = trim_sql(body);
    if body.is_empty() {
        return Err(FORM.to_owned());
    }
    if scan_sql_words(&body).any(|word| word.eq_ignore_ascii_case("select")) {
        return Err(
            "a transform works on the values it is given from one record, and cannot `select` from anything"
                .to_owned(),
        );
    }
    Ok(Transform {
        name: file.name.clone(),
        parameters,
        body,
    })
}

/// SQL text from its first character that is not whitespace or a comment.
fn after_comments(sql: &str) -> &str {
    let mut text = sql.trim_start();
    loop {
        if let Some(rest) = text.strip_prefix("--") {
            text = rest.split_once('\n').map_or("", |(_, rest)| rest);
        } else if let Some(rest) = text.strip_prefix("/*") {
            text = rest.split_once("*/").map_or("", |(_, rest)| rest);
        } else {
            return text;
        }
        text = text.trim_start();
    }
}

/// The words of SQL text that are not inside a string, a quoted name or a
/// comment.
fn scan_sql_words(sql: &str) -> impl Iterator<Item = &str> {
    // Every byte of a character outside ASCII counts as part of a word, so
    // the scan only ever stops between characters.
    let in_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii();
    let bytes = sql.as_bytes();
    let mut at = 0;
    std::iter::from_fn(move || {
        while at < bytes.len() {
            let byte = bytes[at];
            let rest = &bytes[at..];
            if byte == b'\'' || byte == b'"' {
                at = skip_past(bytes, at + 1, &[byte]);
            } else if rest.starts_with(b"--") {
                at = skip_past(bytes, at, b"\n");
            } else if rest.starts_with(b"/*") {
                at = skip_past(bytes, at + 2, b"*/");
            } else if let Some(quote) = dollar_quote(rest) {
                at = skip_past(bytes, at + quote.len(), quote);
            } else if in_word(byte) && !byte.is_ascii_digit() {
                let start = at;
                while at < bytes.len() && in_word(bytes[at]) {
                    at += 1;
                }
                return Some(&sql[start..at]);
            } else {
                at += 1;
            }
        }
        None
    })
}

/// Where the first `needle` at or after `from` ends, or the end of the text.
fn skip_past(bytes: &[u8], from: usize, needle: &[u8]) -> usize {
    bytes
        .get(from..)
        .and_then(|rest| {
            rest.windows(needle.len())
                .position(|window| window == needle)
        })
        .map_or(bytes.len(), |found| from + found + needle.len())
}

/// The `$$` or `$tag$` that opens a dollar-quoted string, if the text begins
/// with one.
fn dollar_quote(text: &[u8]) -> Option<&[u8]> {
    let tag = text.strip_prefix(b"$")?;
    let length = tag
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_' || !byte.is_ascii())
        .count();
    let closed = tag.get(length) == Some(&b'$');
    let numbered = tag.first().is_some_and(u8::is_ascii_digit);
    (closed && !numbered).then(|| &text[..length + 2])
}
