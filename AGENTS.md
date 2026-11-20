# Working on Spool

Spool is one Rust binary with an embedded DuckDB store, a web studio compiled
into it, and TypeScript packages around it. This file says where code goes and
what the checks expect. `CLAUDE.md` is a link to it.

## Commands

Run these from the repository root. `just` with no arguments lists them.

| Command | What it does |
|---|---|
| `just check` | Everything CI checks that needs no test run: format, lint, types, file sizes, generated files |
| `just test` | The Rust tests, which also run the TypeScript packages' tests against the binary |
| `just regen` | Rewrites every generated file: schema, client types, studio bundle, strategy helper, help snapshots |
| `just fmt` | Formats Rust and TypeScript |

A commit runs `scripts/check-generated.sh` when the hook is installed
(`scripts/check-generated.sh --install-hook`). If it fails, run `just regen`
and stage what changed.

## Where code goes

| Path | What lives there |
|---|---|
| `crates/spool-core` | What an identity is and which changes to it are valid. No database, few dependencies |
| `crates/spool` | The engine: DuckDB store, project files, load, match, review, explain |
| `crates/spool-bench` | Benchmark scenarios and the metrics they score |
| `crates/spool-testing` | What the crates' integration tests share. Not published |
| `crates/spool-cli` | The `spool` command, the HTTP server, MCP, sign-in |
| `crates/*/assets` | Files compiled into a binary: built bundles, templates, helpers. Not edited by hand when generated |
| `packages/client` | The typed TypeScript client. `src/types.ts` is generated |
| `packages/ui` | Tokens, themes and components the site and the studio share |
| `packages/studio` | The studio, built into `crates/spool-cli/assets/studio` |
| `packages/strategy` | What a TypeScript strategy or extractor imports |
| `packages/testing` | Starts a real `spool` for the packages' tests |
| `packages/stress` | Stress tests of a running server |
| `site` | The website and the documentation |
| `benchmarks` | Scenario projects and their data |

A concept has the same name in every layer. Review items are
`crates/spool/src/review.rs`, `crates/spool-cli/src/server/handlers/review.rs`,
`crates/spool-cli/src/commands/show/review.rs` and
`packages/studio/src/screens/review/`. A new concept gets a file of that name
in each layer it reaches, and an existing one is extended where it already is.

## Rules

1. **Look before you write.** Search for a helper before adding one. Shared
   helpers live in a few named places:
   - `spool_core::DisjointSets` for grouping records that are joined in
     pairs.
   - `crates/spool-cli/src/text.rs` for the words the commands print: lists,
     plurals, shares.
   - `packages/studio/src/format.ts` for how the studio writes values.
   A helper that two files need moves to the shared place; it is not copied.

2. **Do not hand-roll what a crate does.** Encodings (`hex`, `base64`), dates,
   comparisons that must take constant time, parsers of standard formats
   (`.env`, CSV, URLs) and signal handling come from a dependency. Prefer one
   that is already in `Cargo.lock` or `bun.lock`.

3. **Name a function for what it does to what.** `hash_password`,
   `validate_email`, `join_names`: a verb and a noun, so that a search finds
   it. One-word names such as `named`, `listed` or `text` mean something
   different in every file and hide the helper that already exists.

4. **Keep files under 800 lines.** `scripts/check-file-size.sh` fails a
   source file over the budget. The files that were over it when the check was
   added are listed in the script, each with its size, and may only get
   smaller. Split a file by what it is about, not into `part1` and `part2`.

5. **One declaration per API operation.** An operation is a typed function in
   `crates/spool-cli/src/server/handlers/`. The HTTP route and the MCP tool
   both call it. Its request and response types are in `api.rs` and in the
   `Wire` list there, which the schema and the client's types are generated
   from.

6. **Generated files are regenerated, not edited.** They are marked
   `linguist-generated` in `.gitattributes`.

7. **No `unsafe` without a reason at the site.** The workspace denies it; the
   few places that need it carry `#[allow(unsafe_code)]` and a `SAFETY`
   comment.

8. **Tests say what they check.** A test is named for the behaviour, not for
   a milestone or a ticket. Each crate has one integration-test binary,
   `tests/it/`, and a new test file is a module of it. (`spool-cli` also
   has `tests/stress.rs` and `tests/studio.rs`, each one long test that
   runs a TypeScript package's tests and that CI runs in jobs of its own.)

## The studio

Routes are files under `packages/studio/src/routes/` (TanStack Router). A
screen reads with TanStack Query through the client in `packages/client`, and
its filters are the route's validated search params. After a change, a screen
invalidates the queries it made stale; nothing refetches everything.

The studio is static files served by the binary under a policy that allows no
inline script, so it has no server rendering and no framework that needs one.
