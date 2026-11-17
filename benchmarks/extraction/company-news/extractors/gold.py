"""Finds exactly the mentions the scenario's labels say are there.

This is no extractor anyone would deploy: it reads the answers. It exists so
that resolution can be measured on its own. With it, every labelled mention
becomes a record and nothing else does, so what the scenario then
reports about identities is what the type's rules make of perfect extraction.
"""

import csv
from pathlib import Path

from spool import extractor

LABELS = Path(__file__).resolve().parent.parent / "mentions.csv"


@extractor
def find(documents):
    by_key = {(document.source, document.key): document for document in documents}
    with open(LABELS, newline="", encoding="utf-8") as file:
        for label in csv.DictReader(file):
            document = by_key.get((label["source"], label["document"]))
            if document is None:
                continue
            start, end = int(label["start"]), int(label["end"])
            yield document.mention(
                label["entity_type"], start, end, name=document.text[start:end]
            )
