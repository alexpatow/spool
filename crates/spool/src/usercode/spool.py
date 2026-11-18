"""What a Spool strategy or extractor written in Python imports.

A strategy is a file under an entity type's `strategies/` directory, and an
extractor is a file under the project's `extractors/` directory. Spool runs
each as a process of its own, hands it a directory of Parquet files, and
reads what it leaves there. This module hides the files.

# Strategies

A strategy says which pairs of records are the same thing:

    from spool import strategy

    @strategy
    def judge(candidates):
        return [
            {"left_id": pair["left_id"], "right_id": pair["right_id"], "verdict": "match"}
            for pair in candidates.to_pylist()
            if pair["left_name"] == pair["right_name"]
        ]

The function is called as soon as it is decorated, so define what it uses
above it. It is given what its parameters ask for by name:

    candidates  the pairs the project's block rules found, as a pyarrow
                Table: `left_id`, `right_id`, and every feature column of
                both sides as `left_<name>` and `right_<name>`
    features    one row per record of the entity type, as a pyarrow
                Table: `record_id`, `is_new`, `identity_id` (empty for
                a record that has no identity) and the feature
                columns. A strategy that finds its own pairs starts here.
    task        the `Task`, for anything else

It returns judgements: `left_id`, `right_id` and `verdict` (`match`,
`no_match` or `unsure`), and optionally `score` (a number on the strategy's
own scale) and `detail` (anything JSON can hold). At least one side of every
pair must be new to the run. Rows may be a list of dicts, a pyarrow Table,
or a pandas, polars or DuckDB result. Returning nothing means no judgements.

A strategy says whether pairs are the same thing. It cannot change an
identity: Spool reconciles the judgements and commits the result itself.

# Extractors

An extractor finds the things a document mentions:

    import re
    from spool import extractor

    @extractor
    def find(document):
        for found in re.finditer("[A-Z][a-z]+ (Inc|Ltd)", document.text):
            yield document.mention("company", found.start(), found.end(), name=found.group())

It is given what its parameters ask for by name:

    document    one `Document`. The function is called once for each
                document of the task.
    documents   every `Document` of the task, as a list. The function is
                called once.
    task        the `ExtractTask`: the entity types the project declares,
                and what each type's `extract.md` says to look for

It returns mentions, or yields them. A mention says which entity type was
found where, and what is known about it there: `document.mention(entity_type,
start, end, **attributes)`. `start` and `end` are positions in
`document.text` as Python counts them, and `end` is the position after the
last character, so `document.text[start:end]` is the text of the mention.
`document.find(quote)` gives the positions of a quote, for an extractor that
knows what it found and not where. An attribute is text, a boolean or a
number.

A model that runs on this machine reads only so many words at once, and
ignores the rest of a longer text without saying so. `document.pieces(words)`
gives the text in pieces it can read whole, one at a time:

    for piece in document.pieces(300):
        for start, end in model.find(piece.text):
            yield piece.mention("company", start, end, name=piece.text[start:end])

`piece.mention` takes positions in `piece.text`. Each piece shares its last
words with the next, so that a name cut off at the end of one is whole in the
other, and what is found in those words is found twice. `piece.mention` gives
`None` for one of the two, and an extractor may yield `None`.

Each mention becomes a record of its entity type, whose source is the
document's. An extractor says what a document mentions. It cannot say that
two mentions are the same thing: the type's rules and strategies decide that.

What an extractor writes to its standard error is shown to whoever waits for
the run, a line at a time. A function that is called once for each document
has a line written for it every ten seconds, such as `read 400 of 3,053
documents`. Spool stops a run that takes longer than it allows, which is 600
seconds unless `--task-timeout` says otherwise where Spool is started.

Without `SPOOL_TASK_DIR` in the environment the decorators do nothing, so
the file can be imported, and its function called, in a test.
"""

from __future__ import annotations

import inspect
import itertools
import json
import os
import re
import sys
import time
from pathlib import Path

__all__ = ["PROTOCOL", "Document", "ExtractTask", "Piece", "Task", "extractor", "strategy"]

#: The version of the task directory's layout this module reads and writes.
PROTOCOL = 1

#: How often, in seconds, a long extract run says how many documents it has read.
_SAY_EVERY = 10

_COLUMNS = ("left_id", "right_id", "verdict", "score", "detail")
_REQUIRED = _COLUMNS[:3]


def _pyarrow():
    try:
        import pyarrow
        import pyarrow.parquet  # noqa: F401
    except ImportError as error:
        raise ImportError(
            "a Spool strategy reads and writes Parquet with pyarrow, which is not "
            "installed here. Spool adds it when it runs a strategy with `uv`."
        ) from error
    return pyarrow


def _info(directory, kind, decorator):
    """Reads `task.json` and checks that the task is one for `decorator`."""
    info = json.loads((Path(directory) / "task.json").read_text(encoding="utf-8"))
    protocol = info.get("protocol")
    if protocol != PROTOCOL:
        raise RuntimeError(
            f"this task was written with protocol {protocol}, "
            f"and this helper speaks protocol {PROTOCOL}"
        )
    found = info.get("kind", "match")
    if found != kind:
        raise RuntimeError(
            f"`@{decorator}` was given a task of kind `{found}`: a strategy belongs under an "
            "entity type's `strategies/` directory and an extractor under the project's "
            "`extractors/` directory"
        )
    return info


def _write(pyarrow, table, path):
    # Written under another name and then renamed, so that Spool never
    # reads half a file.
    partial = path.with_name(path.name + ".part")
    pyarrow.parquet.write_table(table, partial)
    os.replace(partial, path)


class Task:
    """One match run's task: what Spool wrote, and where the judgements go."""

    def __init__(self, directory):
        self.dir = Path(directory)
        self.info = _info(directory, "match", "strategy")
        #: The match run the judgements belong to.
        self.run = self.info["run"]
        self.entity_type = self.info["entity_type"]
        #: The strategy's name: this file's name without `.py`.
        self.strategy = self.info["strategy"]["name"]
        #: True when the run examines every record again, not only
        #: those that are new.
        self.rematch = bool(self.info.get("rematch", False))
        self._tables = {}

    def _read(self, name):
        if name not in self._tables:
            pyarrow = _pyarrow()
            self._tables[name] = pyarrow.parquet.read_table(self.dir / self.info["inputs"][name])
        return self._tables[name]

    @property
    def candidates(self):
        """The candidate pairs with both sides' features, as a pyarrow Table."""
        return self._read("candidates")

    @property
    def features(self):
        """Every record of the entity type with its features, as a pyarrow Table."""
        return self._read("features")

    def write(self, judgements):
        """Writes the run's judgements. Whatever was written before is replaced."""
        pyarrow = _pyarrow()
        _write(pyarrow, _judgements(pyarrow, judgements), self.dir / self.info["output"])


def strategy(function):
    """Runs `function` as this file's strategy. See the module's description."""
    directory = os.environ.get("SPOOL_TASK_DIR")
    if not directory:
        return function
    task = Task(directory)
    given = {}
    for name in inspect.signature(function).parameters:
        if name == "task":
            given[name] = task
        elif name in ("candidates", "features"):
            given[name] = getattr(task, name)
        else:
            raise TypeError(
                f"`{function.__name__}` asks for `{name}`, and a strategy is given "
                "`candidates`, `features` and `task`"
            )
    task.write(function(**given))
    return function


def _judgements(pyarrow, result):
    """Whatever a strategy returned, as a table of the judgement columns."""
    table = _table(pyarrow, result)
    unknown = [name for name in table.column_names if name not in _COLUMNS]
    missing = [name for name in _REQUIRED if name not in table.column_names]
    if missing or unknown:
        problems = []
        if missing:
            problems.append("no " + ", ".join(f"`{name}`" for name in missing))
        if unknown:
            problems.append("also " + ", ".join(f"`{name}`" for name in unknown))
        raise ValueError(
            "judgements have the columns `left_id`, `right_id` and `verdict`, and "
            f"optionally `score` and `detail`; these have {' and '.join(problems)}"
        )

    columns = {}
    for name in _REQUIRED:
        columns[name] = table.column(name).cast(pyarrow.string())
    if "score" in table.column_names:
        columns["score"] = table.column("score").cast(pyarrow.float64())
    if "detail" in table.column_names:
        detail = table.column("detail")
        if not (pyarrow.types.is_string(detail.type) or pyarrow.types.is_large_string(detail.type)):
            detail = pyarrow.array(
                [None if value is None else _json(value) for value in detail.to_pylist()],
                pyarrow.string(),
            )
        columns["detail"] = detail.cast(pyarrow.string())
    return pyarrow.table(columns)


def _json(value):
    return json.dumps(value, sort_keys=True, default=str)


def _table(pyarrow, result):
    if result is None:
        return pyarrow.table({name: pyarrow.array([], pyarrow.string()) for name in _REQUIRED})
    if isinstance(result, pyarrow.Table):
        return result
    if isinstance(result, pyarrow.RecordBatch):
        return pyarrow.Table.from_batches([result])
    # DuckDB relations and polars frames convert themselves.
    for convert in ("to_arrow_table", "to_arrow"):
        if callable(getattr(result, convert, None)):
            converted = getattr(result, convert)()
            if isinstance(converted, pyarrow.Table):
                return converted
    if type(result).__module__.split(".")[0] == "pandas":
        return pyarrow.Table.from_pandas(result, preserve_index=False)
    if hasattr(result, "__arrow_c_stream__"):
        return pyarrow.table(result)

    rows = list(result)
    for row in rows:
        if not isinstance(row, dict):
            raise TypeError(
                "a strategy returns a table or a list of dicts, and this one "
                f"returned a row that is a {type(row).__name__}"
            )
    names = [name for name in _COLUMNS if any(name in row for row in rows)]
    names += sorted({name for row in rows for name in row} - set(_COLUMNS))
    columns = {}
    for name in names:
        values = [row.get(name) for row in rows]
        if name == "detail":
            values = [
                value if value is None or isinstance(value, str) else _json(value)
                for value in values
            ]
            columns[name] = pyarrow.array(values, pyarrow.string())
        elif name == "score":
            columns[name] = pyarrow.array(values, pyarrow.float64())
        elif name in _REQUIRED:
            columns[name] = pyarrow.array(
                [None if value is None else str(value) for value in values], pyarrow.string()
            )
        else:
            columns[name] = pyarrow.array([None if value is None else str(value) for value in values])
    if not rows:
        return pyarrow.table({name: pyarrow.array([], pyarrow.string()) for name in _REQUIRED})
    return pyarrow.table(columns)


class Document:
    """A document an extractor reads."""

    __slots__ = ("id", "source", "key", "uri", "text")

    def __init__(self, id, source, key, uri, text):
        #: The ID of this version of the document.
        self.id = id
        #: The source the document came from.
        self.source = source
        #: The document's key within its source.
        self.key = key
        #: Where the source keeps the document, if it said.
        self.uri = uri
        self.text = text

    def __repr__(self):
        return f"Document(source={self.source!r}, key={self.key!r}, {len(self.text)} characters)"

    def mention(self, entity_type, start, end, **attributes):
        """A mention of an `entity_type` at `text[start:end]`."""
        return {
            "document_id": self.id,
            "entity_type": entity_type,
            "start": start,
            "end": end,
            "attributes": attributes,
        }

    def find(self, quote):
        """Where `quote` is in the text: a `(start, end)` for each place, in order.

        For an extractor that is told what a document mentions and not
        where, as a language model tells it. A quote that is not in the
        text, letter for letter, is found nowhere.
        """
        places = []
        if not quote:
            return places
        at = self.text.find(quote)
        while at != -1:
            places.append((at, at + len(quote)))
            at = self.text.find(quote, at + len(quote))
        return places

    def pieces(self, words, shared=None):
        """The text in pieces of at most `words` words, one `Piece` at a time.

        For a model that reads only so much at once. A punctuation mark
        counts as a word, which is how such models count. Each piece shares
        its last `shared` words with the next: an eighth of `words` unless
        it is given. A text that fits is one piece, and one with no words in
        it is none.

        The pieces are made as they are asked for, so a long text takes no
        more memory in pieces than one piece does: `list(...)` holds them all.
        """
        if isinstance(words, bool) or not isinstance(words, int) or words < 1:
            raise ValueError(f"`words` is how many words a piece may have, and it is {words!r}")
        if shared is None:
            shared = words // 8
        if isinstance(shared, bool) or not isinstance(shared, int) or not 0 <= shared < words:
            raise ValueError(
                f"`shared` is how many of its {words} words a piece shares with the next, "
                f"and it is {shared!r}"
            )
        return self._pieces(words, shared)

    def _pieces(self, words, shared):
        # Where the words are is asked for as the pieces are made, and only
        # the words of one piece are kept: a text can be nothing but words,
        # and a place for each of them is many times the text.
        spans = (found.span() for found in _WORD.finditer(self.text))
        piece = list(itertools.islice(spans, words))
        first = True
        while piece:
            # What is found in shared words belongs to the piece that has
            # more of the text around it: the halves meet in the middle.
            start = 0 if first else piece[0][0]
            own = 0 if first else piece[shared // 2][0]
            following = next(spans, None)
            if following is None:
                yield Piece(self, start, len(self.text), own, len(self.text) + 1)
                return
            until = (piece + [following])[words - (shared - shared // 2)][0]
            yield Piece(self, start, piece[-1][1], own, until)
            # The next piece begins with the last `shared` words of this one.
            piece = piece[words - shared :] + [following]
            piece += itertools.islice(spans, words - shared - 1)
            first = False


#: A word, or a mark that is not part of one.
_WORD = re.compile(r"\w+(?:[-_]\w+)*|\S")


class Piece:
    """A part of a document's text: what `Document.pieces` gives."""

    __slots__ = ("document", "start", "text", "_own")

    def __init__(self, document, start, end, own, until):
        self.document = document
        #: Where the piece starts in the document's text.
        self.start = start
        self.text = document.text[start:end]
        self._own = (own, until)

    def __repr__(self):
        return f"Piece(start={self.start}, {len(self.text)} characters)"

    def mention(self, entity_type, start, end, **attributes):
        """A mention of an `entity_type` at `piece.text[start:end]`.

        `None` if it starts in words this piece shares with another, on the
        side of the other: that piece has the same mention, with more of
        the text around it.
        """
        start, end = self.start + start, self.start + end
        if not self._own[0] <= start < self._own[1]:
            return None
        return self.document.mention(entity_type, start, end, **attributes)


class ExtractTask:
    """One extract run's task: the documents, and where the mentions go."""

    def __init__(self, directory):
        self.dir = Path(directory)
        self.info = _info(directory, "extract", "extractor")
        #: The extract run the mentions belong to.
        self.run = self.info["run"]
        #: The extractor's name: this file's name without `.py`.
        self.extractor = self.info["extractor"]["name"]
        #: The names of the entity types the project declares. A mention
        #: names one of them.
        self.entity_types = [entry["name"] for entry in self.info["entity_types"]]
        self._documents = None

    def instructions(self, entity_type):
        """What the type's `extract.md` says to look for, or `None`."""
        for entry in self.info["entity_types"]:
            if entry["name"] == entity_type:
                return entry.get("instructions")
        raise KeyError(f"the project declares no entity type `{entity_type}`")

    def features(self, entity_type):
        """The type's feature columns, as `{"name", "data_type"}` dicts."""
        for entry in self.info["entity_types"]:
            if entry["name"] == entity_type:
                return entry.get("features", [])
        raise KeyError(f"the project declares no entity type `{entity_type}`")

    @property
    def documents(self):
        """The documents to read, as a list of `Document`."""
        if self._documents is None:
            pyarrow = _pyarrow()
            table = pyarrow.parquet.read_table(self.dir / self.info["inputs"]["documents"])
            self._documents = [
                Document(row["document_id"], row["source"], row["key"], row["uri"], row["text"])
                for row in table.to_pylist()
            ]
        return self._documents

    def write(self, mentions):
        """Writes the run's mentions. Whatever was written before is replaced."""
        pyarrow = _pyarrow()
        _write(pyarrow, _mentions(pyarrow, mentions), self.dir / self.info["output"])


def extractor(function):
    """Runs `function` as this file's extractor. See the module's description."""
    directory = os.environ.get("SPOOL_TASK_DIR")
    if not directory:
        return function
    task = ExtractTask(directory)
    names = list(inspect.signature(function).parameters)
    for name in names:
        if name not in ("document", "documents", "task"):
            raise TypeError(
                f"`{function.__name__}` asks for `{name}`, and an extractor is given "
                "`document` or `documents`, and `task`"
            )
    if "document" in names and "documents" in names:
        raise TypeError(
            f"`{function.__name__}` asks for `document` and for `documents`: it is called "
            "once for each document, or once with all of them"
        )
    given = {"task": task} if "task" in names else {}
    found = []
    if "document" in names:
        documents = task.documents
        said = time.monotonic()
        for read, document in enumerate(documents):
            # A run that takes long says how far it is, so that whoever
            # waits for it can tell when it will be done.
            if time.monotonic() - said >= _SAY_EVERY:
                said = time.monotonic()
                progress = f"read {read:,} of {len(documents):,} documents"
                print(progress, file=sys.stderr, flush=True)
            for mention in function(document=document, **given) or ():
                if mention is None:
                    continue
                # A mention made by hand need not say which document it is in.
                if isinstance(mention, dict) and mention.get("document_id") is None:
                    mention = {**mention, "document_id": document.id}
                found.append(mention)
    else:
        if "documents" in names:
            given["documents"] = task.documents
        found = [mention for mention in function(**given) or () if mention is not None]
    task.write(found)
    return function


_MENTION = ("document_id", "entity_type", "start", "end", "attributes")


def _mentions(pyarrow, rows):
    """What an extractor returned, as a table of the mention columns."""
    columns = {name: [] for name in _MENTION}
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise TypeError(
                "an extractor returns mentions, as `document.mention(...)` makes them, "
                f"and mention {index} is a {type(row).__name__}"
            )
        unknown = sorted(set(row) - set(_MENTION))
        missing = [name for name in _MENTION[:4] if row.get(name) is None]
        if unknown or missing:
            problems = []
            if missing:
                problems.append("no " + ", ".join(f"`{name}`" for name in missing))
            if unknown:
                problems.append("also " + ", ".join(f"`{name}`" for name in unknown))
            raise ValueError(
                "a mention has `document_id`, `entity_type`, `start` and `end`, and "
                f"optionally `attributes`; mention {index} has {' and '.join(problems)}"
            )
        for name in ("start", "end"):
            if isinstance(row[name], bool) or not isinstance(row[name], int):
                raise TypeError(
                    f"`{name}` is a position in the document's text, and mention {index} "
                    f"has {row[name]!r}"
                )
        attributes = row.get("attributes")
        if attributes is not None and not isinstance(attributes, (dict, str)):
            raise TypeError(
                f"`attributes` is a dict, and mention {index} has a {type(attributes).__name__}"
            )
        columns["document_id"].append(str(row["document_id"]))
        columns["entity_type"].append(str(row["entity_type"]))
        columns["start"].append(row["start"])
        columns["end"].append(row["end"])
        columns["attributes"].append(
            attributes if attributes is None or isinstance(attributes, str) else _json(attributes)
        )
    return pyarrow.table(
        {
            "document_id": pyarrow.array(columns["document_id"], pyarrow.string()),
            "entity_type": pyarrow.array(columns["entity_type"], pyarrow.string()),
            "start": pyarrow.array(columns["start"], pyarrow.int64()),
            "end": pyarrow.array(columns["end"], pyarrow.int64()),
            "attributes": pyarrow.array(columns["attributes"], pyarrow.string()),
        }
    )
