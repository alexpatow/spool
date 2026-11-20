---
name: spool-extractor
description: Write a Spool extractor - a Python or TypeScript file in extractors/ that reads documents and says where each names a thing, such as a company or a person, so that each place becomes a record - and run it with spool extract. Use when asked to find entities in text, notes, emails or articles, to call a language model or a local model on documents, or to fix an extractor that fails or finds the wrong things.
---

# Write an extractor

An extractor reads documents and returns **mentions**: an entity type, where in the text it starts and ends, and attributes such as `name`. Each mention becomes a record, which `spool match` places like a row. The server runs `extractors/<name>.py` with `uv` and `extractors/<name>.ts` with `bun`, as a child process, in the project directory.

## What the project needs

1. A source of documents, whose columns are the attributes a mention may have:

   ```yaml
   # sources/notes.yaml
   columns: [name]
   ```

2. A mapping of that source in the entity type, beside the others:

   ```yaml
   # entities/company/entity.yaml, under mapping:
     notes:
       name: name
       name_key: { from: name, transform: name_key }
   ```

3. Optional: `entities/company/extract.md`, prose that says what a company is and how to quote it. The extractor reads it as `task.instructions("company")`, to give to a model.

4. The extractor, `extractors/<name>.py` or `.ts`. The file's name is the extractor's, and `spool info` lists it.

## Python

```python
"""Finds the companies a note names."""

import re

from spool import extractor

LEGAL_NAME = re.compile(r"\b[A-Z][\w&.-]*(?: [A-Z][\w&.-]*)* (?:Ltd|Limited|Inc|GmbH|plc)\b")


@extractor
def find(document):
    for found in LEGAL_NAME.finditer(document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
```

- `@extractor` marks the function. It asks for what it needs by parameter name: `document` (called once per document), or `documents` (called once with a list), and optionally `task`.
- `document.text`, `document.key`, `document.source`, `document.id`.
- `document.mention(entity_type, start, end, **attributes)`: `start` and `end` are Python string positions in `document.text`, `end` exclusive. An attribute the source's `columns` do not list fails the whole batch.
- `document.find(quote)` gives `(start, end)` for each place the exact quote is. **Ask a model for quotes, never for positions**, and place them with `find`. A quote not in the text letter for letter is found nowhere.
- `document.pieces(300)` gives the text in pieces of at most 300 words, for a model that reads only so much; use `piece.text` and `piece.mention(...)`, which may give `None` (yield it anyway).
- `task.instructions(type)` is the type's `extract.md` or `None`; `task.entity_types`, `task.features(type)`, `task.run`.
- `spool` is not a package to install: the server provides it. Other dependencies go in a script block at the top, which `uv` installs:

  ```python
  # /// script
  # requires-python = ">=3.10,<3.13"
  # dependencies = ["httpx>=0.27"]
  # ///
  ```

- One file: `import helpers` does not find `extractors/helpers.py`.

## A hosted model

The key goes in `.env` (`OPENAI_API_KEY=...`), which Spool hands to the extractor and never prints. Read it with `os.environ`. Never print it or write it in the file.

```python
@extractor
def find(document, task):
    try:
        names = ask_model(task.instructions("company") or "List the companies.", document.text)
    except Exception as error:
        yield document.failed(f"the model's API: {error}")
        return
    for name in set(names):
        for start, end in document.find(name):
            yield document.mention("company", start, end, name=name)
```

`ask_model` is yours: ask for JSON of a fixed shape, such as `{"names": [...]}`. `document.failed(reason)` marks one document unread; it is read again by the next `spool extract`. An exception in a per-document function does the same. An exception with `documents`, an exit status other than 0, or a run over `--task-timeout` (600 s) fails the whole batch, and nothing from it is kept. https://spool.alexpatow.com/docs/concepts/extraction#with-a-hosted-model has a whole extractor that calls an OpenAI-compatible API with the standard library only.

## TypeScript

```ts
import { extractor } from "spool-strategy";

const LEGAL_NAME = /\b[A-Z][\w&.-]*(?: [A-Z][\w&.-]*)* (?:Ltd|Limited|Inc|GmbH|plc)\b/g;

await extractor(async (task) => {
  const documents = await task.documents();
  return documents.flatMap((document) =>
    [...document.text.matchAll(LEGAL_NAME)].map((found) =>
      document.mention("company", found.index, found.index + found[0].length, { name: found[0] }),
    ),
  );
});
```

`document.find(quote)`, `document.failed(reason)`, `task.instructions(type)` and `task.entityTypes` are as in Python. Positions are JavaScript string indices.

## Run it and check what it found

With `spool dev` running (the spool-data skill):

```sh
spool extract samples/notes-1.txt --extractor names
spool show records --source notes
spool show records 'notes:notes-1.txt[54-69]'
spool match
```

`spool extract` stores each file as a document, keyed by its name, and runs the extractor over every document it has not read. It prints each record it found, as `<source>:<file>[<start>-<end>]`, and what the extractor wrote to standard error. A folder stands for every file in it. Text only: turn PDF or HTML into text first.

After changing the extractor, read every document again:

```sh
spool extract --again --extractor names
```

A mention the new version no longer finds is taken back, and `spool extract` says how many. A run that finds nothing in documents that had mentions takes them all back: check its output before matching.

Print to standard error to debug: it reaches `spool extract` and the terminal of `spool dev`. Standard output is shown nowhere.
