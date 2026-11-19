---
name: spool-data
description: Run a Spool project's server and work with its data from the command line - spool dev, spool load for CSV rows, spool extract for text, spool match, spool show for records, identities, runs, documents, review items and the log, spool explain, spool retract and spool reset - and read their output as JSON. Use when loading or matching data in a Spool project, looking up an identity or a record, or asking why two records were or were not matched.
---

# Load, match and look

## The server

```sh
spool dev
```

It runs the project's server on `http://127.0.0.1:7765`, with its data in `.spool/dev.duckdb`, and applies each project file as it is saved. It runs until Ctrl-C, so start it in the background and stop it when done. Nobody signs in. Every other command here calls it: `--url` or `SPOOL_URL` points them at another, and `--token` or `SPOOL_TOKEN` gives a token where one is needed. If the port is taken, `spool dev --listen 127.0.0.1:7766` and `SPOOL_URL=http://127.0.0.1:7766`.

## Load and match

```sh
spool load samples/crm-1.csv
spool load export.csv --source crm --type company
spool match
```

- `spool load` sends a CSV file's rows; the first line names the columns, each one the source lists. Every value is text, an empty field is missing. `.tsv` files have tabs, and `-` reads standard input. The source defaults to the one the file is named after (`crm-1.csv` is `crm`).
- The same rows again add nothing. A row under a key already held, with other values, is a new version.
- Loading does not match. `spool match` places every new record, of every type: a record that matches one identity joins it, records that match only each other make a new one, and a record that matches two identities waits for review. **Identities that exist are never merged or split by a match.**
- `spool extract <path>` stores text files, or every file in a folder, as documents and runs the project's extractor (the spool-extractor skill). Match after it too.

## Look

```sh
spool show
spool show records --source crm --attributes
spool show records crm:c-1
spool show identities --sort records --order desc --limit 10
spool show identities --type company
spool show runs --limit 5
spool show attributes
spool show documents
spool show review
spool show log
```

With an ID after the list, each shows one: `spool show identities <id>` with its records, constraints and history, `spool show runs <run>` with its report, `spool show review <item>` with its records. A record is named by its ID or as `<source>:<key>`; one found in a document as the lists name it, `notes:notes-1.txt[54-69]` (quote it in a shell). `--after <id>` continues a list.

```sh
spool explain crm:c-1 crm:c-2
spool explain crm:c-1
```

Two records: why they share an identity or do not, which run and rule decided it, and what the rules say now. One: what holds its identity together and where it would come apart.

## Output for scripts

Every command that prints a result takes `-o table|json|ndjson|csv`, and `--json`. Standard output is the result only; hints and progress go to standard error, and `-q` silences them. Parse JSON, not tables.

```sh
spool show identities -o ndjson | jq -r '.id + "  " + .label'
spool show records crm:c-1 --json | jq -r .record.identity
spool show records --unassigned -o csv > waiting.csv
```

A failure prints `error:` and often `hint:`, and exits 1; wrong arguments exit 2.

## Take back and start over

```sh
spool retract --source crm --type company c-6 --reason "deleted in the CRM"
spool retract --source crm --type company --keys gone.txt
```

Retracting takes rows a source no longer holds out of their identities. They stay in the store, and loading the key again brings them back.

`spool reset` deletes the development store: every record, identity and its ID. Only with the user's say-so, after stopping `spool dev`; `--yes` skips its question.

## Not on the command line

Merge, split, undo, constraints, deciding review items and approving drafts are HTTP calls or MCP tools (the spool-api skill), or the studio at `http://127.0.0.1:7765/`.
