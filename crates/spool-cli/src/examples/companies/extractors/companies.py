# /// script
# requires-python = ">=3.10,<3.13"
# dependencies = ["fast-gliner>=0.4.1,<0.5"]
# ///
"""Finds the companies a document names, with a small model that runs here.

An extractor reads documents and says where they name something, and each
thing it finds becomes a record. Spool runs this file with `uv`, which
installs what the lines above list. The first run fetches about 330 MB, most
of it the model, and later runs fetch nothing. No document leaves this
machine and there is no key to set.
"""

import sys

from fast_gliner import FastGLiNER
from spool import extractor

# What to look for. On the left is the word the model is given, and the word
# decides what is found: "company" misses most universities, parties and
# charities, and "organization" finds them. Any word will do, "person" or
# "medicine" as well. On the right is the entity type of this project that
# such a find becomes a record of.
LOOK_FOR = {"company": "company"}

# GLiNER small: 245 MB, Apache-2.0, at the version it was tried with.
MODEL = "onnx-community/gliner_small-v2.1"
WEIGHTS = "onnx/model_q4f16.onnx"
VERSION = "8142fb00740ccea973e64b1272949ff48653df5e"


def load():
    """The model: read from this machine if it is here, and fetched if not."""
    files = {"onnx_path": WEIGHTS, "allow_patterns": ["*.json", WEIGHTS], "revision": VERSION}
    try:
        return FastGLiNER.from_pretrained(MODEL, local_files_only=True, **files)
    except FileNotFoundError:
        print(f"Fetching the model {MODEL} (245 MB), once.", file=sys.stderr)
        return FastGLiNER.from_pretrained(MODEL, **files)


# Loaded as the file starts, so that `spool build --install` fetches it too.
model = load()


@extractor
def find(document):
    # The model reads 384 words at a time and ignores the rest, so a long
    # document is read in pieces.
    for piece in document.pieces(300):
        for found in model.predict_entities(piece.text, list(LOOK_FOR)):
            # The model counts in bytes, and Spool in characters.
            start = len(piece.text.encode()[: found["start"]].decode())
            end = start + len(found["text"])
            # `name` is a column of sources/notes.yaml.
            yield piece.mention(LOOK_FOR[found["label"]], start, end, name=found["text"])
