"""What a Spool strategy written in Python imports.

A strategy is a file under a project's `strategies/` directory. Spool runs
it as a process of its own, hands it a directory of Parquet files, and reads
the judgements it leaves there. This module hides the files:

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
    features    one row per observation of the entity type, as a pyarrow
                Table: `observation_id`, `is_new`, `identity_id` (empty for
                an observation that has no identity) and the feature
                columns. A strategy that finds its own pairs starts here.
    task        the `Task`, for anything else

It returns judgements: `left_id`, `right_id` and `verdict` (`match`,
`no_match` or `unsure`), and optionally `score` (a number on the strategy's
own scale) and `detail` (anything JSON can hold). At least one side of every
pair must be new to the run. Rows may be a list of dicts, a pyarrow Table,
or a pandas, polars or DuckDB result. Returning nothing means no judgements.

A strategy says whether pairs are the same thing. It cannot change an
identity: Spool reconciles the judgements and commits the result itself.

Without `SPOOL_TASK_DIR` in the environment the decorator does nothing, so
the file can be imported, and its function called, in a test.
"""

from __future__ import annotations

import inspect
import json
import os
from pathlib import Path

__all__ = ["PROTOCOL", "Task", "strategy"]

#: The version of the task directory's layout this module reads and writes.
PROTOCOL = 1

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


class Task:
    """One run's task: what Spool wrote, and where the judgements go."""

    def __init__(self, directory):
        self.dir = Path(directory)
        self.info = json.loads((self.dir / "task.json").read_text(encoding="utf-8"))
        protocol = self.info.get("protocol")
        if protocol != PROTOCOL:
            raise RuntimeError(
                f"this task was written with protocol {protocol}, "
                f"and this helper speaks protocol {PROTOCOL}"
            )
        #: The match run the judgements belong to.
        self.run = self.info["run"]
        self.entity_type = self.info["entity_type"]
        #: The strategy's name: this file's name without `.py`.
        self.strategy = self.info["strategy"]["name"]
        #: True when the run examines every observation again, not only
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
        """Every observation of the entity type with its features, as a pyarrow Table."""
        return self._read("features")

    def write(self, judgements):
        """Writes the run's judgements. Whatever was written before is replaced."""
        pyarrow = _pyarrow()
        table = _judgements(pyarrow, judgements)
        path = self.dir / self.info["output"]
        partial = path.with_name(path.name + ".part")
        pyarrow.parquet.write_table(table, partial)
        os.replace(partial, path)


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
