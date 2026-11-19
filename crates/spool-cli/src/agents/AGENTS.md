# Spool

This directory is a [Spool](https://spool.alexpatow.com) project. Its files declare where records come from (`sources/`), what kinds of thing they describe and how records are matched (`entities/`, `transforms/`), and the code that finds records in text (`extractors/`). The data is not in these files: it is in the server, which `spool dev` runs for this project.

## Skills

Each is a file in `.agents/skills/`, linked from `.claude/skills/`. Read the one a task needs:

- `spool-project`: declare sources, entity types, features and transforms, and check them with `spool info`.
- `spool-rules`: write block, match and no_match rules, and see what a change to them would do with `spool recompute`.
- `spool-extractor`: write an extractor, a Python or TypeScript file that finds records in text.
- `spool-data`: load rows and documents, match them, and read the result with `spool show` and `spool explain`.
- `spool-api`: call the server over HTTP or MCP, make tokens, and deploy with `spool build`, `spool apply` and `spool serve`.

## Working here

- `spool dev` runs the server on `http://127.0.0.1:7765` and applies each file as it is saved. Ask whether one is running before you start one: a second one on this project is refused. One you start runs in the background, and you stop it when you are done.
- After you change a file, run `spool info`. It checks every file and every SQL expression without a server, and an error names the file and the field.
- Read `-o json` or `-o ndjson` in scripts, not the table: its columns may change between releases.
- A record is named as `<source>:<key>`, such as `crm:c-1`, by every command that takes one.
- Other systems store the IDs of identities. Do not run `spool reset` or delete `.spool/` to get a better match: change the rules, show what that would change with `spool recompute`, and ask before `--apply`.
- Merging, splitting, undoing and deciding review items are calls of the HTTP API or MCP tools, each with a `reason`. The command line has none of them.
- `.env` holds the secrets of extractors and strategies. Never print it, and never commit it. Never edit `.spool/`: Spool writes it.
- The skills are those of the `spool` that wrote them. After spool is upgraded, `spool update` brings them to its version.
- `spool <command> --help` has every option. The documentation is at https://spool.alexpatow.com/docs, and https://spool.alexpatow.com/llms.txt lists its pages.
