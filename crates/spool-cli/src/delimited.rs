//! Rows from delimited text: CSV, or the same with another character
//! between the fields.
//!
//! The first line names the columns. Every value is read as text, and an
//! empty field is a missing value, which is how the engine reads the CSV of
//! a benchmark. So a row gets the same record ID whichever way it came.
//!
//! What is wrong with a file is said with the line it is on, because the
//! person who reads it is the one who has to open the file.

use std::io::Read;
use std::sync::Arc;

use arrow::array::{ArrayRef, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;

/// What is wrong with text that has nothing in it.
pub const EMPTY: &str = "it is empty: the first line has to name the columns";

/// Why the text could not be read as rows, in words for the person who
/// sent it.
#[derive(Debug, PartialEq, Eq)]
pub struct Malformed(pub String);

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Malformed {}

/// Reads rows a part at a time.
pub struct Reader<R> {
    records: csv::Reader<R>,
    schema: SchemaRef,
    record: csv::ByteRecord,
}

impl<R: Read> Reader<R> {
    /// Reads the first line, which names the columns.
    pub fn new(input: R, delimiter: u8) -> Result<Self, Malformed> {
        let mut records = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .trim(csv::Trim::Headers)
            .from_reader(input);
        let header = records.byte_headers().map_err(|error| describe(&error))?;
        if header.is_empty() {
            return Err(Malformed(EMPTY.to_owned()));
        }
        let mut names: Vec<&str> = Vec::with_capacity(header.len());
        for (index, name) in header.iter().enumerate() {
            let name =
                std::str::from_utf8(name).map_err(|_| Malformed(format!("line 1: {NOT_UTF8}")))?;
            if name.is_empty() {
                return Err(Malformed(format!(
                    "line 1: column {} has no name. The first line has to name the columns",
                    index + 1
                )));
            }
            if names.contains(&name) {
                return Err(Malformed(format!("line 1: two columns are named `{name}`")));
            }
            names.push(name);
        }
        let schema = Arc::new(Schema::new(
            names
                .iter()
                .map(|name| Field::new(*name, DataType::Utf8, true))
                .collect::<Vec<_>>(),
        ));
        Ok(Self {
            records,
            schema,
            record: csv::ByteRecord::new(),
        })
    }

    /// The columns, each of them text.
    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    /// The next rows: at most `most_rows` of them, and no more after the
    /// one that takes them to `most_bytes`, counted as Arrow holds them:
    /// the text of each value and four bytes more. `None` when no row is
    /// left.
    pub fn next_batch(
        &mut self,
        most_rows: usize,
        most_bytes: usize,
    ) -> Result<Option<RecordBatch>, Malformed> {
        let columns = self.schema.fields().len();
        let mut builders: Vec<StringBuilder> = (0..columns).map(|_| StringBuilder::new()).collect();
        let (mut rows, mut bytes) = (0, 0);
        while rows < most_rows && bytes < most_bytes {
            match self.records.read_byte_record(&mut self.record) {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => return Err(describe(&error)),
            }
            for (builder, field) in builders.iter_mut().zip(self.record.iter()) {
                bytes += field.len() + 4;
                if field.is_empty() {
                    builder.append_null();
                    continue;
                }
                let text = std::str::from_utf8(field).map_err(|_| {
                    let line = self.record.position().map_or(0, csv::Position::line);
                    Malformed(format!("line {line}: {NOT_UTF8}"))
                })?;
                builder.append_value(text);
            }
            rows += 1;
        }
        if rows == 0 {
            return Ok(None);
        }
        let arrays: Vec<ArrayRef> = builders
            .iter_mut()
            .map(|builder| Arc::new(builder.finish()) as ArrayRef)
            .collect();
        RecordBatch::try_new(self.schema.clone(), arrays)
            .map(Some)
            .map_err(|error| Malformed(format!("the rows cannot be held: {error}")))
    }
}

const NOT_UTF8: &str = "the text is not UTF-8. Save the file as UTF-8 and send it again";

fn describe(error: &csv::Error) -> Malformed {
    Malformed(match error.kind() {
        csv::ErrorKind::UnequalLengths {
            pos,
            expected_len,
            len,
        } => {
            let line = pos.as_ref().map_or(0, csv::Position::line);
            format!(
                "line {line} has {}, and the first line names {}",
                count(*len, "field"),
                count(*expected_len, "column")
            )
        }
        csv::ErrorKind::Io(error) => format!("it cannot be read: {error}"),
        _ => error.to_string(),
    })
}

fn count(number: u64, what: &str) -> String {
    match number {
        1 => format!("1 {what}"),
        _ => format!("{number} {what}s"),
    }
}

#[cfg(test)]
mod tests {
    use arrow::array::{Array, AsArray};

    use super::*;

    type Row = Vec<Option<String>>;

    /// The names of the columns and every row of the text, as the values of
    /// its columns.
    fn read(text: &[u8], delimiter: u8) -> Result<(Vec<String>, Vec<Row>), Malformed> {
        let mut reader = Reader::new(text, delimiter)?;
        let names = reader
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        let mut rows = Vec::new();
        while let Some(batch) = reader.next_batch(2, usize::MAX)? {
            assert!(batch.num_rows() <= 2);
            for row in 0..batch.num_rows() {
                rows.push(
                    (0..batch.num_columns())
                        .map(|column| {
                            let values = batch.column(column).as_string::<i32>();
                            values.is_valid(row).then(|| values.value(row).to_owned())
                        })
                        .collect(),
                );
            }
        }
        Ok((names, rows))
    }

    fn refused(text: &[u8]) -> String {
        read(text, b',').unwrap_err().0
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn rows_are_text_and_an_empty_field_is_missing() {
        let (names, rows) = read(
            b"id,name,country\n1,\"Acorn, Ltd\",GB\n2,,\"\"\n\n3,\"say \"\"hi\"\"\", padded \n",
            b',',
        )
        .unwrap();
        assert_eq!(names, ["id", "name", "country"]);
        assert_eq!(
            rows,
            [
                vec![some("1"), some("Acorn, Ltd"), some("GB")],
                vec![some("2"), None, None],
                vec![some("3"), some("say \"hi\""), some(" padded ")],
            ]
        );
    }

    #[test]
    fn the_ways_a_file_was_saved_do_not_show() {
        // A byte order mark, Windows line ends, no line end after the last
        // row, and spaces around the names of the columns.
        let (names, rows) = read(b"\xef\xbb\xbfid , name\r\n1,anvil\r\n2,bolt", b',').unwrap();
        assert_eq!(names, ["id", "name"]);
        assert_eq!(
            rows,
            [
                vec![some("1"), some("anvil")],
                vec![some("2"), some("bolt")]
            ]
        );
    }

    #[test]
    fn another_character_can_stand_between_the_fields() {
        let (names, rows) = read(b"id\tname\n1\tanvil, large\n", b'\t').unwrap();
        assert_eq!(names, ["id", "name"]);
        assert_eq!(rows, [vec![some("1"), some("anvil, large")]]);
    }

    #[test]
    fn a_first_line_alone_has_no_rows() {
        let (names, rows) = read(b"id,name\n", b',').unwrap();
        assert_eq!(names, ["id", "name"]);
        assert!(rows.is_empty());
    }

    #[test]
    fn a_part_ends_after_the_row_that_fills_it() {
        let mut reader = Reader::new(&b"id\naaaa\nbbbb\ncccc\n"[..], b',').unwrap();
        let sizes: Vec<usize> = std::iter::from_fn(|| reader.next_batch(100, 16).unwrap())
            .map(|batch| batch.num_rows())
            .collect();
        assert_eq!(sizes, [2, 1]);
    }

    #[test]
    fn what_is_wrong_is_said_with_its_line() {
        assert_eq!(
            refused(b""),
            "it is empty: the first line has to name the columns"
        );
        assert_eq!(
            refused(b"id,name\n1,anvil\n2,bolt,red\n"),
            "line 3 has 3 fields, and the first line names 2 columns"
        );
        assert_eq!(
            refused(b"id,name\n1\n"),
            "line 2 has 1 field, and the first line names 2 columns"
        );
        assert_eq!(
            refused(b"id,name,name\n1,anvil,bolt\n"),
            "line 1: two columns are named `name`"
        );
        assert_eq!(
            refused(b"id,,name\n1,anvil,bolt\n"),
            "line 1: column 2 has no name. The first line has to name the columns"
        );
        assert_eq!(
            refused(b"id,name\n1,anvil\n2,caf\xe9\n"),
            "line 3: the text is not UTF-8. Save the file as UTF-8 and send it again"
        );
        assert_eq!(
            refused(b"id,caf\xe9\n1,anvil\n"),
            "line 1: the text is not UTF-8. Save the file as UTF-8 and send it again"
        );
    }

    #[test]
    fn a_line_is_counted_as_the_file_has_it() {
        // The value in quotes holds a line break, so the row after it is on
        // line 4 of the file.
        assert_eq!(
            refused(b"id,note\n1,\"two\nlines\"\n2,x,y\n"),
            "line 4 has 3 fields, and the first line names 2 columns"
        );
    }
}
