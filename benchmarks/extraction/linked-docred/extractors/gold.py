"""Finds exactly the mentions the dataset says are there.

This is no extractor anyone would deploy: it reads the answers. It exists so
that resolution can be measured on its own. With it, every labelled mention
becomes a record and nothing else does, so what a scenario then reports
about identities is what the type's rules make of perfect extraction.

`run.py` writes the labels of each scenario to a file of its own, and says
which one in `LINKED_DOCRED_LABELS`.
"""

import csv
import os

from spool import extractor

LABELS = os.environ.get("LINKED_DOCRED_LABELS")


@extractor
def find(documents):
    if not LABELS:
        raise SystemExit(
            "LINKED_DOCRED_LABELS does not say where the labelled mentions are: "
            "run.py, beside this project, sets it"
        )
    by_key = {(document.source, document.key): document for document in documents}
    with open(LABELS, newline="", encoding="utf-8") as file:
        for label in csv.DictReader(file):
            document = by_key.get((label["source"], label["document"]))
            if document is None:
                continue
            start, end = int(label["start"]), int(label["end"])
            yield document.mention(
                label["entity_type"],
                start,
                end,
                name=document.text[start:end],
                document=document.key,
            )
