# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "spacy==3.8.18",
#     "en-core-web-sm @ https://github.com/explosion/spacy-models/releases/download/en_core_web_sm-3.8.0/en_core_web_sm-3.8.0-py3-none-any.whl#sha256=1932429db727d4bff3deed6b34cfc05df17794f4a52eeb26cf8928f7c1a0fb85",
# ]
# ///
"""Finds named things with spaCy's small English model.

`en_core_web_sm` is 12 MB, runs on a processor, and is what most people try
first. `uv` fetches it and spaCy the first time this runs. Nothing is sent
anywhere: the model reads the documents on this machine.

The model was trained on OntoNotes, whose kinds of things are not DocRED's.
Each is given the entity type whose description in the DocRED paper it fits:

    PERSON                                          person
    ORG                                             organisation
    GPE, LOC, FAC                                   place
    PRODUCT, EVENT, WORK_OF_ART, LAW, LANGUAGE      other

`NORP`, a nationality or a religious or political group, fits none of them
alone and is left out, and so are dates and numbers, which this project has
no type for.

A mention is where the model says it is, and nothing is done to it
afterwards. The model often takes the article before a name to be part of
it, "the Senate", and a mention is scored by its place, so those count as
wrong. Leaving the article out is not the answer: the dataset's own labels
take it in for some names and not for others.

`run.py` says which entity type a scenario scores in `LINKED_DOCRED_TYPE`,
and only mentions of that type are returned. Without it, all four are.

A scenario has the model read every document twice, once more for the
batches in reverse order, and there is a scenario for each type. `run.py`
names a directory in `LINKED_DOCRED_CACHE`, and what the model found in a
text is kept there, so that it reads each document once. Without the
variable nothing is kept.
"""

import hashlib
import json
import os
from pathlib import Path

import en_core_web_sm

from spool import extractor

ENTITY_TYPES = {
    "PERSON": "person",
    "ORG": "organisation",
    "GPE": "place",
    "LOC": "place",
    "FAC": "place",
    "PRODUCT": "other",
    "EVENT": "other",
    "WORK_OF_ART": "other",
    "LAW": "other",
    "LANGUAGE": "other",
}

ONLY = os.environ.get("LINKED_DOCRED_TYPE")
CACHE = os.environ.get("LINKED_DOCRED_CACHE")


def entities(texts):
    """What the model finds in each text: a label, and where it starts and ends."""
    kept, path = {}, None
    if CACHE:
        path = Path(CACHE) / f"en_core_web_sm-{en_core_web_sm.__version__}.json"
        if path.is_file():
            kept = json.loads(path.read_text(encoding="utf-8"))
    names = [hashlib.sha256(text.encode()).hexdigest() for text in texts]
    unread = {name: text for name, text in zip(names, texts) if name not in kept}
    if unread:
        # The whole of it is run, as it is when someone tries it. The part
        # that finds names alone is three times as fast, and finds names
        # across the ends of sentences, which this does not.
        model = en_core_web_sm.load()
        for name, read in zip(unread, model.pipe(unread.values(), batch_size=64)):
            kept[name] = [[found.label_, found.start_char, found.end_char] for found in read.ents]
        if path:
            path.parent.mkdir(parents=True, exist_ok=True)
            partial = path.with_name(f"{path.name}.{os.getpid()}.part")
            partial.write_text(json.dumps(kept), encoding="utf-8")
            os.replace(partial, path)
    return [kept[name] for name in names]


# The extractor is run where it is declared, so what it calls is above it.
@extractor
def find(documents):
    for document, found in zip(documents, entities([document.text for document in documents])):
        for label, start, end in found:
            entity_type = ENTITY_TYPES.get(label)
            if entity_type is None or (ONLY and entity_type != ONLY):
                continue
            yield document.mention(
                entity_type,
                start,
                end,
                name=document.text[start:end],
                document=document.key,
            )
