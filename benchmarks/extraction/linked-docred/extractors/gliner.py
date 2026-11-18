# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = [
#     "fast-gliner>=0.4.1,<0.5",
# ]
# ///
"""Finds named things with GLiNER small, the model `spool init` starts with.

GLiNER is told what to look for in words, so the entity types are a list
here and not what the model was trained on. `uv` fetches the package the
first time this runs, and the package fetches the model: 330 MB together.
Nothing is sent anywhere: the model reads the documents on this machine,
about five of them a second on four cores.

Each word the model is given is the entity type whose description in the
DocRED paper it fits:

    person                                          person
    organization                                    organisation
    location                                        place
    product, event, work of art, law, language      other

The model is asked for all of them at once, and gives a place one of them.

Three things about the model are handled here:

- It reads 384 words and ignores the rest without saying so. A longer
  document is given to it in pieces, of whole sentences where they fit.
- It counts a place in bytes, and Spool in characters.
- It is one file of several in its repository, and all of them would be
  fetched if the file were not named.

A mention is where the model says it is, and nothing is done to it
afterwards.

`run.py` says which entity type a scenario scores in `LINKED_DOCRED_TYPE`,
and only mentions of that type are returned. Without it, all four are. It
names a directory in `LINKED_DOCRED_CACHE`, and what the model found in a
text is kept there, so that it reads each document once and not once for
every scenario. Without the variable nothing is kept.
"""

import hashlib
import json
import os
import re
from pathlib import Path

# The package draws a progress bar for the download, which is not a log.
os.environ.setdefault("HF_HUB_DISABLE_PROGRESS_BARS", "1")

from fast_gliner import FastGLiNER  # noqa: E402

from spool import extractor  # noqa: E402

ENTITY_TYPES = {
    "person": "person",
    "organization": "organisation",
    "location": "place",
    "product": "other",
    "event": "other",
    "work of art": "other",
    "law": "other",
    "language": "other",
}

# The weights: the repository, the commit of it, and the file. This file is
# 245 MB and gives the answers of the full one, which is 611 MB.
REPOSITORY = "onnx-community/gliner_small-v2.1"
REVISION = "8142fb00740ccea973e64b1272949ff48653df5e"
WEIGHTS = "onnx/model_q4f16.onnx"

# What the model counts as a word, and how many a piece may hold. The model
# takes 384, and a piece stays under that in case the two count differently.
WORD = re.compile(r"\w+(?:[-_]\w+)*|\S")
WORDS = 320
SENTENCE_END = re.compile(r"[.!?][\"')\]]*\s+")

ONLY = os.environ.get("LINKED_DOCRED_TYPE")
CACHE = os.environ.get("LINKED_DOCRED_CACHE")


def pieces(text):
    """Where the pieces of a text start and end: each has at most `WORDS` words."""
    words = [found.span() for found in WORD.finditer(text)]
    if len(words) <= WORDS:
        return [(0, len(text))]
    # A piece ends where a sentence does, if one ends within it, and after
    # its last word if not.
    ends = [found.start() + 1 for found in SENTENCE_END.finditer(text)]
    cut, first = [], 0
    while len(words) - first > WORDS:
        limit = words[first + WORDS - 1][1]
        fitting = [end for end in ends if words[first][0] < end <= limit]
        end = fitting[-1] if fitting else limit
        cut.append((words[first][0], end))
        first = next(at for at in range(first, len(words)) if words[at][0] >= end)
    cut.append((words[first][0], len(text)))
    return cut


def characters(text):
    """For each byte of a text in UTF-8 that begins a character, which character."""
    places, at = {}, 0
    for index, character in enumerate(text):
        places[at] = index
        at += len(character.encode())
    places[at] = len(text)
    return places


def entities(texts):
    """What the model finds in each text: a label, and where it starts and ends."""
    kept, path = {}, None
    if CACHE:
        path = Path(CACHE) / f"gliner_small-v2.1-{REVISION[:12]}.json"
        if path.is_file():
            kept = json.loads(path.read_text(encoding="utf-8"))
    names = [hashlib.sha256(text.encode()).hexdigest() for text in texts]
    unread = {name: text for name, text in zip(names, texts) if name not in kept}
    if unread:
        model = FastGLiNER.from_pretrained(
            REPOSITORY,
            onnx_path=WEIGHTS,
            revision=REVISION,
            allow_patterns=["*.json", "*.model", WEIGHTS],
        )
        for name, text in unread.items():
            found = []
            for start, end in pieces(text):
                piece = text[start:end]
                places = characters(piece)
                for entity in model.predict_entities(piece, list(ENTITY_TYPES)):
                    found.append(
                        [
                            entity["label"],
                            start + places[entity["start"]],
                            start + places[entity["end"]],
                        ]
                    )
            kept[name] = found
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
