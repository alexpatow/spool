# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Extraction through to resolution on Linked-DocRED.

    cargo build --release -p spool-cli
    uv run benchmarks/extraction/linked-docred/run.py

It needs Python 3.11 or later and no package, so `python3` runs it too.

Linked-DocRED is 5,051 opening sections of English Wikipedia articles in
which every mention of a person, an organisation, a place and other named
things is marked, with the Wikidata entry it is a mention of. That is the
truth twice over: where the mentions are, and which of them are the same
thing, within a document and across documents.

The dataset is under the GPL, version 3, so none of it is in this
repository. This script downloads it, checks it against the checksums in
`../../datasets.toml`, and writes it in the shape `../company-news` has, into
`benchmarks/.cache/linked-docred`, which Git ignores:

    documents/<batch>/<key>.txt   a document: its sentences as one text
    labels/<type>.csv             the labelled mentions of one entity type
    scenarios/<type>-<extractor>.toml
    models/                       what a model found, kept from one scenario
                                  to the next so that it reads a document once

It then runs each scenario with `spool bench` and prints what the reports
say, side by side. A scenario is one entity type and one extractor:

    gold     reads the labels, so extraction is perfect and what is left is
             what the type's rules make of the mentions: resolution alone
    suffix   finds organisations by the word their name ends in: the least
             an extractor can do without a model
    spacy    finds all four types with spaCy's small English model
    gliner   finds all four types with GLiNER small, the model the extractor
             of a new project starts with

The first two run by default, and take half a minute. The other two are
models that run on this machine, and are asked for by name:

    uv run benchmarks/extraction/linked-docred/run.py --extractors gold,suffix,spacy,gliner

`uv` fetches each the first time, 170 MB for spaCy and 330 MB for GLiNER.
spaCy reads the dataset in a minute and a half, and GLiNER in a quarter of
an hour.

Each has three batches, the dataset's `train_annotated`, `dev` and `test`
files in that order, so a report also says whether identities stayed put as
documents arrived.

`sample/` holds five invented documents in the dataset's format, for trying
the script without the dataset: `--data benchmarks/extraction/linked-docred/sample`.

Nothing here sends a document anywhere: the model runs on this machine. The
network is used to download the dataset and, the first time, the model.
"""

import argparse
import csv
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCHMARKS = HERE.parent.parent
SOURCE = "wikipedia"

# The dataset's files, as `datasets.toml` names them, and the batch each is.
FILES = [
    ("train", "linked_docred_train", "train_annotated.json"),
    ("dev", "linked_docred_dev", "dev.json"),
    ("test", "linked_docred_test", "test.json"),
]

# The project's entity types, and what DocRED calls each. Its dates and
# numbers have no Wikidata entry and are left out.
TYPES = {"organisation": "ORG", "person": "PER", "place": "LOC", "other": "MISC"}

# The extractors, and the types each finds. `gold` finds whatever the labels
# it is given hold, and a model the one type it is asked for.
EXTRACTORS = {
    "gold": list(TYPES),
    "suffix": ["organisation"],
    "spacy": list(TYPES),
    "gliner": list(TYPES),
}
# Those that run when none is named: the ones with nothing to fetch.
DEFAULT_EXTRACTORS = ["gold", "suffix"]
# A model may take longer over a batch than the ten minutes `spool bench`
# gives an extractor.
TASK_TIMEOUT_SECONDS = 3600


def main():
    arguments = parse_arguments()
    work = arguments.work
    spool = arguments.spool
    if not spool.is_file():
        raise SystemExit(
            f"there is no {spool}: build it with `cargo build --release -p spool-cli`, "
            "or name another with --spool"
        )

    batches = read(arguments.data, work)
    counts = convert(batches, work, arguments.types, arguments.documents)
    print(
        f"{counts['documents']:,} documents in {len(batches)} batches, "
        f"{counts['mentions']:,} labelled mentions of {counts['entities']:,} entities"
    )
    if counts["repeated"]:
        print(
            f"{counts['repeated']:,} labels were left out: "
            "another label of the same type is at the same place"
        )

    runs = []
    for extractor in arguments.extractors:
        for entity_type in arguments.types:
            if entity_type not in EXTRACTORS[extractor]:
                continue
            name = f"{entity_type}-{extractor}"
            scenario = write_scenario(work, name, entity_type, extractor, list(batches))
            print(f"running   {name}", flush=True)
            runs.append(bench(spool, work, name, scenario, entity_type, extractor))

    summary = summarise(runs)
    results = work / "results"
    (results / "summary.txt").write_text(summary, encoding="utf-8")
    (results / "summary.json").write_text(json.dumps(runs, indent=2) + "\n", encoding="utf-8")
    print()
    print(summary, end="")
    print(f"\nreports   {results}")


def parse_arguments():
    parser = argparse.ArgumentParser(
        description="Runs extraction and resolution over Linked-DocRED with `spool bench`."
    )
    parser.add_argument(
        "--types",
        default=",".join(TYPES),
        help=f"the entity types to run, separated by commas (default: {','.join(TYPES)})",
    )
    parser.add_argument(
        "--extractors",
        default=",".join(DEFAULT_EXTRACTORS),
        help=f"the extractors to run, separated by commas: some of {', '.join(EXTRACTORS)} "
        f"(default: {','.join(DEFAULT_EXTRACTORS)})",
    )
    parser.add_argument(
        "--documents",
        type=int,
        metavar="N",
        help="only the first N documents of each batch, for a quick try (default: all)",
    )
    parser.add_argument(
        "--spool",
        type=Path,
        default=BENCHMARKS.parent / "target" / "release" / "spool",
        help="the spool binary (default: target/release/spool)",
    )
    parser.add_argument(
        "--data",
        type=Path,
        metavar="DIR",
        help="a directory that holds train_annotated.json, dev.json and test.json "
        "already: nothing is downloaded or checked",
    )
    parser.add_argument(
        "--work",
        type=Path,
        default=BENCHMARKS / ".cache" / "linked-docred",
        metavar="DIR",
        help="where the documents, scenarios and reports are written "
        "(default: benchmarks/.cache/linked-docred)",
    )
    arguments = parser.parse_args()
    arguments.types = [name for name in arguments.types.split(",") if name]
    unknown = [name for name in arguments.types if name not in TYPES]
    if unknown or not arguments.types:
        parser.error(f"--types takes some of {', '.join(TYPES)}")
    arguments.extractors = [name for name in arguments.extractors.split(",") if name]
    unknown = [name for name in arguments.extractors if name not in EXTRACTORS]
    if unknown or not arguments.extractors:
        parser.error(f"--extractors takes some of {', '.join(EXTRACTORS)}")
    if arguments.documents is not None and arguments.documents < 1:
        parser.error("--documents takes a number of documents, 1 or more")
    arguments.work = arguments.work.resolve()
    return arguments


def read(data, work):
    """The dataset's files, each as the list of documents it holds, by batch."""
    if data is not None:
        paths = {batch: data / file for batch, _, file in FILES}
    else:
        paths = download(work / "download")
    batches = {}
    for batch, path in paths.items():
        try:
            with open(path, encoding="utf-8") as file:
                batches[batch] = json.load(file)
        except OSError as error:
            raise SystemExit(f"{path}: {error.strerror}") from None
    return batches


def download(directory):
    """Fetches the files `datasets.toml` lists, and keeps those with its checksums."""
    index = BENCHMARKS / "datasets.toml"
    with open(index, "rb") as file:
        listed = {entry["name"]: entry for entry in tomllib.load(file)["dataset"]}
    directory.mkdir(parents=True, exist_ok=True)
    paths = {}
    for batch, name, file in FILES:
        entry = listed.get(name)
        if entry is None:
            raise SystemExit(f"{index}: no dataset named `{name}`")
        path = directory / file
        if not (path.is_file() and sha256_of(path) == entry["sha256"]):
            print(f"fetching  {entry['url']}", flush=True)
            partial = path.with_name(f"{file}.{os.getpid()}.part")
            try:
                with urllib.request.urlopen(entry["url"], timeout=120) as response:
                    with open(partial, "wb") as out:
                        shutil.copyfileobj(response, out)
                found = sha256_of(partial)
                if found != entry["sha256"]:
                    raise SystemExit(
                        f"dataset `{name}`: the download has SHA-256 {found}, "
                        f"and datasets.toml expects {entry['sha256']}"
                    )
                os.replace(partial, path)
            except OSError as error:
                raise SystemExit(
                    f"dataset `{name}`: downloading {entry['url']} failed: {error}"
                ) from None
            finally:
                partial.unlink(missing_ok=True)
        paths[batch] = path
    return paths


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        while chunk := file.read(1 << 16):
            digest.update(chunk)
    return digest.hexdigest()


# The dataset holds a document as sentences of tokens, and a mention as a
# run of tokens. A document's text is its tokens with the spaces a writer
# would leave: none before what closes, none after what opens, none around a
# hyphen or a slash. A straight double quote opens and closes by turns
# within its sentence.
CLOSING = set(", . ; : ! ? ) ] } % ' ’ ” ... … 's 'S ’s n't 'm 're 've 'll 'd".split())
OPENING = set("( [ { $ £ € # “ ‘".split())
JOINING = {"-", "/"}


def text_of(sentences):
    """A document's text, and where each token of each sentence is in it."""
    parts, places, length = [], [], 0
    joined = True
    for sentence in sentences:
        quoted = False
        row = []
        for token in sentence:
            before, after = True, False
            if not token.strip():
                # A token that is white space is the space.
                before, after = False, True
            elif token in CLOSING:
                before = False
            elif token in JOINING:
                before, after = False, True
            elif token in OPENING:
                after = True
            elif token == '"':
                before, after = not quoted, not quoted
                quoted = not quoted
            if before and not joined:
                parts.append(" ")
                length += 1
            row.append((length, length + len(token)))
            parts.append(token)
            length += len(token)
            joined = after
        places.append(row)
    return "".join(parts), places


def entity_of(entity, key):
    """What a labelled entity is: its Wikidata entry, where it has one."""
    linking = entity.get("entity_linking") or {}
    wikidata = linking.get("wikidata_resource") or ""
    if re.fullmatch(r"Q\d+", wikidata):
        return wikidata
    # Some have a Wikipedia page and no Wikidata entry.
    page = linking.get("wikipedia_resource") or ""
    if page and not page.startswith("#"):
        return f"wikipedia:{page}"
    # What is in neither has a number of the dataset's own, `#DocRED-<n>#`,
    # and the dataset gives two such entities the same number where they
    # are the same thing.
    if wikidata.startswith("#DocRED-"):
        return wikidata
    return f"{key}#{entity.get('id')}"


def convert(batches, work, types, limit):
    """Writes the documents and the labels of each entity type, and counts them."""
    for name in ("documents", "labels", "scenarios", "results", "stores", "models"):
        shutil.rmtree(work / name, ignore_errors=True)
        (work / name).mkdir(parents=True)
    wanted = {TYPES[entity_type]: entity_type for entity_type in types}
    labels = {entity_type: [] for entity_type in types}
    entities = set()
    counts = {"documents": 0, "mentions": 0, "repeated": 0}
    for batch, documents in batches.items():
        directory = work / "documents" / batch
        directory.mkdir()
        for index, document in enumerate(documents[:limit]):
            key = f"{batch}-{index:04}.txt"
            text, places = text_of(document["sents"])
            (directory / key).write_text(text, encoding="utf-8")
            counts["documents"] += 1
            # A place holds one mention of a type, as it does in a store.
            taken = set()
            for entity in document["entities"]:
                entity_type = wanted.get(entity["type"])
                if entity_type is None:
                    continue
                true_entity = entity_of(entity, key)
                for mention in entity["mentions"]:
                    tokens = places[mention["sent_id"]]
                    first, after = mention["pos"]
                    start, end = tokens[first][0], tokens[after - 1][1]
                    if (entity_type, start, end) in taken:
                        counts["repeated"] += 1
                        continue
                    taken.add((entity_type, start, end))
                    entities.add((entity_type, true_entity))
                    labels[entity_type].append(
                        (SOURCE, key, start, end, entity_type, true_entity, text[start:end])
                    )
    columns = ["source", "document", "start", "end", "entity_type", "true_entity", "text"]
    for entity_type, rows in labels.items():
        path = work / "labels" / f"{entity_type}.csv"
        with open(path, "w", newline="", encoding="utf-8") as file:
            writer = csv.writer(file)
            writer.writerow(columns)
            writer.writerows(rows)
        counts["mentions"] += len(rows)
    counts["entities"] = len(entities)
    return counts


def write_scenario(work, name, entity_type, extractor, batches):
    """A scenario in the shape of `../company-news/scenario.toml`: a batch a step."""
    directory = work / "scenarios"

    def relative(path):
        return json.dumps(os.path.relpath(path, directory))

    lines = [
        f"project = {relative(HERE)}",
        "",
        "[truth]",
        f"mentions = {relative(work / 'labels' / f'{entity_type}.csv')}",
    ]
    for batch in batches:
        lines += [
            "",
            "[[step]]",
            f"name = {json.dumps(batch)}",
            "",
            "[[step.documents]]",
            f"dir = {relative(work / 'documents' / batch)}",
            f"source = {json.dumps(SOURCE)}",
            "",
            "[[step.extract]]",
            f"extractor = {json.dumps(extractor)}",
        ]
    path = directory / f"{name}.toml"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def bench(spool, work, name, scenario, entity_type, extractor):
    """Runs one scenario with `spool bench`, and returns what its report says."""
    results = work / "results"
    report, log = results / f"{name}.json", results / f"{name}.log"
    environment = dict(os.environ)
    environment["LINKED_DOCRED_LABELS"] = str(work / "labels" / f"{entity_type}.csv")
    environment["LINKED_DOCRED_TYPE"] = entity_type
    environment["LINKED_DOCRED_CACHE"] = str(work / "models")
    command = [spool, "bench", scenario, "--list", "0"]
    command += ["--db", work / "stores" / f"{name}.duckdb"]
    command += ["--out", report, "--tasks", work / "tasks"]
    command += ["--task-timeout", str(TASK_TIMEOUT_SECONDS)]
    started = time.monotonic()
    with open(log, "wb") as output:
        process = subprocess.Popen(
            command, stdout=output, stderr=subprocess.STDOUT, env=environment
        )
        # What the process and those it started used, which `wait` alone
        # does not say.
        _, status, usage = os.wait4(process.pid, 0)
    seconds = time.monotonic() - started
    if os.waitstatus_to_exitcode(status) != 0:
        tail = log.read_text(encoding="utf-8", errors="replace").splitlines()[-20:]
        raise SystemExit("\n".join([f"`spool bench` failed on {scenario}:", *tail]))
    with open(report, encoding="utf-8") as file:
        report = json.load(file)
    # Linux counts the peak in kilobytes and macOS in bytes.
    peak = usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024)
    return scores(name, entity_type, extractor, report, seconds, peak)


def scores(name, entity_type, extractor, report, seconds, peak):
    """The numbers of one report that the summary prints."""
    steps = report["steps"]
    last = steps[-1]
    order = report.get("order_sensitivity") or {}
    return {
        "scenario": name,
        "entity_type": entity_type,
        "extractor": extractor,
        "documents": sum(stored["received"] for step in steps for stored in step["documents"]),
        "extraction": last["extraction"],
        "resolution": last["metrics"],
        "end_to_end": last["end_to_end"],
        # Of the mentions that had an identity before a batch, the share
        # that kept it, for each batch after the first.
        "kept_identity": [step["id_stability"]["strict"] for step in steps[1:]],
        # A mention that would join two identities joins neither: it is left
        # without one, and the two are put to a reviewer.
        "left_for_review": last["metrics"]["unassigned"],
        "review_items": sum(len(resolved["review"]) for resolved in last["resolve"]["resolved"]),
        "same_group_in_reverse_order": order.get("agreement"),
        "candidate_pairs": sum(
            resolved["candidates"] for step in steps for resolved in step["resolve"]["resolved"]
        ),
        "seconds": round(seconds, 1),
        "peak_memory_bytes": peak,
    }


def share(value):
    return "n/a" if value is None else f"{value:.3f}"


def table(title, columns, rows):
    """A title, the names of the columns, and a line a row."""
    rows = [[str(cell) for cell in row] for row in rows]
    widths = [max(len(row[at]) for row in [columns, *rows]) for at in range(len(columns))]

    def line(row):
        # The type and the extractor to the left, and numbers to the right.
        cells = [
            cell.ljust(width) if at < 2 else cell.rjust(width)
            for at, (cell, width) in enumerate(zip(row, widths))
        ]
        return "  ".join(cells).rstrip()

    return "\n".join([title, line(columns), *map(line, rows)]) + "\n"


def summarise(runs):
    """The reports side by side, as text."""

    def named(run):
        return [run["entity_type"], run["extractor"]]

    def against_truth(found):
        return [
            share(found["pairs"]["precision"]),
            share(found["pairs"]["recall"]),
            share(found["b_cubed"]["precision"]),
            share(found["b_cubed"]["recall"]),
        ]

    quality = ["pair precision", "pair recall", "B-cubed precision", "B-cubed recall"]
    extraction = table(
        "extraction: the mentions an extractor found against the labelled ones",
        ["type", "extractor", "labelled", "extracted", "correct", "precision", "recall", "F1"],
        [
            named(run)
            + [f"{found['labelled']:,}", f"{found['extracted']:,}", f"{found['correct']:,}"]
            + [share(found["precision"]), share(found["recall"]), share(found["f1"])]
            for run in runs
            for found in [run["extraction"]]
        ],
    )
    resolution = table(
        "resolution: identities against entities, "
        "over the mentions that were found and are labelled",
        ["type", "extractor", "mentions", "entities", "identities"]
        + quality
        + ["false merges", "false splits"],
        [
            named(run)
            + [f"{found['records']:,}", f"{found['true_entities']:,}", f"{found['identities']:,}"]
            + against_truth(found)
            + [f"{found['false_merges']:,}", f"{found['false_splits']:,}"]
            for run in runs
            for found in [run["resolution"]]
        ],
    )
    end_to_end = table(
        "end to end: identities against entities, over every labelled mention and everything found",
        ["type", "extractor", "entities", "with a mention found"] + quality,
        [
            named(run)
            + [f"{found['true_entities']:,}", f"{found['entities_found']:,}"]
            + against_truth(found)
            for run in runs
            for found in [run["end_to_end"]]
        ],
    )
    running = table(
        "the run: identities as batches arrive, and what it took",
        ["type", "extractor", "documents", "candidate pairs", "kept their identity"]
        + ["left for review", "review items", "same group in reverse order"]
        + ["seconds", "peak memory"],
        [
            named(run)
            + [f"{run['documents']:,}", f"{run['candidate_pairs']:,}"]
            + [" ".join(share(kept) for kept in run["kept_identity"]) or "n/a"]
            + [f"{run['left_for_review']:,}", f"{run['review_items']:,}"]
            + [share(run["same_group_in_reverse_order"])]
            + [f"{run['seconds']:.0f}", f"{run['peak_memory_bytes'] / (1 << 20):,.0f} MB"]
            for run in runs
        ],
    )
    return "\n".join([extraction, resolution, end_to_end, running])


if __name__ == "__main__":
    main()
