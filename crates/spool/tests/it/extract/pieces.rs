//! A long document read in pieces.

use super::*;

/// A model reads only so much at once, so an extractor that asks one reads
/// a long document in pieces that share some words, and what is found in
/// the words two pieces share is a mention once.
#[test]
#[ignore = "runs uv"]
fn a_long_document_is_read_in_pieces_and_what_two_pieces_share_is_found_once() {
    let news = News::new();
    news.extractor(
        "names.py",
        r#"
import re
from spool import extractor

WORD = re.compile(r"\w+(?:[-_]\w+)*|\S")

@extractor
def find(document):
    text = document.text
    if document.key == "short":
        # A text that fits is one piece, and it is the text.
        whole = list(document.pieces(300))
        assert [(piece.start, piece.text) for piece in whole] == [(0, text)]
        assert whole[0].document is document
        # One with no words in it is no piece, and a piece has a length.
        assert list(type(document)(document.id, "news", "none", None, " \n").pieces(5)) == []
        for words, shared in [(0, None), (True, None), (5, 5), (5, -1), ("5", None)]:
            try:
                document.pieces(words, shared)
            except ValueError as error:
                assert "`words` is" in str(error) or "`shared` is" in str(error), error
            else:
                raise AssertionError(f"pieces({words!r}, {shared!r}) was taken")
        # Without shared words, each word is in one piece.
        apart = list(document.pieces(3, 0))
        assert [piece.text for piece in apart] == ["🚀 Acme Inc", "opened an office", "."]
        assert [piece.start for piece in apart] == [0, 11, 27]

    # Pieces of twelve words, of which each shares four with the next.
    pieces = list(document.pieces(12, 4))
    words = [found.span() for found in WORD.finditer(text)]
    assert len(pieces) == max(1, -(-(len(words) - 4) // 8)), (len(pieces), len(words))
    assert pieces[0].start == 0
    assert pieces[-1].start + len(pieces[-1].text) == len(text)
    for index, piece in enumerate(pieces):
        # A piece is a part of the text, of no more words than was asked,
        # and it begins and ends with a word.
        assert text[piece.start : piece.start + len(piece.text)] == piece.text
        assert len(WORD.findall(piece.text)) <= 12
        if index:
            assert piece.start == words[index * 8][0]
        if index + 1 < len(pieces):
            assert piece.start + len(piece.text) == words[index * 8 + 11][1]
        for found in re.finditer(r"[A-Z]\w+ (?:Inc|Ltd)", piece.text):
            # Positions are the piece's, and the mention's are the document's.
            mention = piece.mention("company", found.start(), found.end(), name=found.group())
            if mention is not None:
                assert text[mention["start"] : mention["end"]] == found.group()
            # One that the next piece has too is `None` here or there, and
            # the helper leaves it out.
            yield mention
"#,
    );
    // Names every five words, so that they fall at every place of a piece:
    // inside it, in the words it shares, and across where a piece ends.
    let long: String = (0..40)
        .map(|at| match at % 2 {
            0 => format!("Société{at} Ltd said: «née» 🚀. "),
            _ => format!("Acme{at} Inc was not there, "),
        })
        .collect();
    let short = "🚀 Acme Inc opened an office.";
    news.add(&[("long", &long), ("short", short)]);
    let report = news.extract("names").unwrap();
    assert_eq!((report.documents, report.mentions), (2, 41));

    // Each name is a record once, at its place in the document.
    let characters: Vec<char> = long.chars().collect();
    let found = news.mentions(id("long", &long));
    let names: Vec<String> = found
        .iter()
        .map(|record| record.payload["name"].as_str().unwrap().to_owned())
        .collect();
    let expected: Vec<String> = (0..40)
        .map(|at| match at % 2 {
            0 => format!("Société{at} Ltd"),
            _ => format!("Acme{at} Inc"),
        })
        .collect();
    assert_eq!(names, expected);
    for record in &found {
        let evidence = record.evidence.as_ref().unwrap();
        let (start, end) = (
            evidence["start"].as_u64().unwrap() as usize,
            evidence["end"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            characters[start..end].iter().collect::<String>(),
            record.payload["name"].as_str().unwrap()
        );
        assert_eq!(evidence["text"], record.payload["name"]);
    }
    assert_eq!(news.mentions(id("short", short)).len(), 1);
}

/// Anyone who may add a document decides how long it is and what is in it.
/// A text of nothing but words is read in pieces without a place being kept
/// for each of its words, which would be many times the text.
#[test]
#[ignore = "runs uv"]
fn a_text_of_millions_of_words_is_read_in_pieces_in_little_memory() {
    let news = News::new();
    news.extractor(
        "names.py",
        r#"
import resource
import sys
from spool import extractor

def peak():
    """The most memory this process has had, in MiB."""
    most = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return most / (1024 * 1024 if sys.platform == "darwin" else 1024)

@extractor
def find(document):
    words = len(document.text) // 2
    before = peak()
    pieces = document.pieces(300)
    # They are made one at a time, as they are asked for.
    assert iter(pieces) is pieces, type(pieces)
    count, last = 0, None
    for piece in pieces:
        count, last = count + 1, piece
        assert piece.text.count("a") <= 300, (count, len(piece.text))
    grown = peak() - before
    assert count == -(-(words - 37) // 263), (count, words)
    assert last.start + len(last.text) == len(document.text)
    # A place for each word would be some 70 bytes for each two of the text.
    assert grown < 64, f"{grown:.0f} MiB more for a text of {len(document.text) >> 20} MiB"
    yield last.mention("company", len(last.text) - 2, len(last.text) - 1)
"#,
    );
    // Eight MiB in which every other character is a word.
    let dense = "a ".repeat(4 << 20);
    news.add(&[("dense", &dense)]);
    let report = news.extract("names").unwrap();
    assert_eq!((report.documents, report.mentions), (1, 1));
}
