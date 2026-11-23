# Working on Spool

Spool is one Rust binary with an embedded DuckDB store, a web studio compiled
into it, and TypeScript packages around it. This file says where code goes and
what the checks expect. `CLAUDE.md` is a link to it.

## Commands

Run these from the repository root. `just` with no arguments lists them.

| Command | What it does |
|---|---|
| `just check` | Everything CI checks that needs no test run: format, lint, unused exports, types, file sizes, generated files |
| `just test` | The Rust tests (`cargo test`). `just test-all` adds the ones that need `uv` and `bun` |
| `just regen` | Rewrites every generated file: schema, client types, studio bundle, strategy helper, help snapshots |
| `just fmt` | Formats the Rust (`cargo fmt`) and the TypeScript (`bun fmt`) |
| `just lint` | Clippy and oxlint |
| `just unused` | TypeScript exports, files and dependencies nothing uses (knip) |

Rust is tested with `cargo test` and TypeScript with `bun test`, in the
package. `cargo test` also runs the studio's, the client's and the stress
tests' `bun test` against the binary it built, which is how CI runs them.

A commit runs `scripts/check-generated.sh` when the hook is installed
(`scripts/check-generated.sh --install-hook`). If it fails, run `just regen`
and stage what changed.

## Where code goes

| Path | What lives there |
|---|---|
| `crates/spool-core` | What an identity is and which changes to it are valid. No database, few dependencies |
| `crates/spool` | The engine: DuckDB store, project files, load, match, review, explain |
| `crates/spool-bench` | Benchmark scenarios and the metrics they score |
| `crates/spool-cli` | The `spool` command, the HTTP server, MCP, sign-in |
| `crates/spool-testing` | What the crates' integration tests share. Not published |
| `crates/*/assets` | Files compiled into a binary: built bundles, templates, helpers. Not edited by hand when generated |
| `packages/client` | The typed TypeScript client. `src/types.ts` is generated |
| `packages/ui` | Tokens, themes and components the site and the studio share. Shelf's files, checked by `shelf check` |
| `packages/studio` | The studio, built into `crates/spool-cli/assets/studio` |
| `packages/strategy` | What a TypeScript strategy or extractor imports |
| `packages/testing` | Starts a real `spool` for the packages' tests |
| `packages/stress` | Stress tests of a running server |
| `site` | The website and the documentation |
| `benchmarks` | Scenario projects and their data |

The TypeScript packages and the site are one bun workspace with one
`bun.lock` at the root. A package imports another by its name, never by a
path out of its own directory.

A Rust module that outgrows one file keeps its file as the root and puts its
parts in a directory of the same name beside it: `engine.rs` and `engine/`.

A concept has the same name in every layer. Review items are
`crates/spool/src/review.rs`, `crates/spool-cli/src/server/handlers/review.rs`,
`crates/spool-cli/src/server/mcp/tools/review.rs`,
`crates/spool-cli/src/commands/show/review.rs` and
`packages/studio/src/screens/review/`. A new concept gets a file of that name
in each layer it reaches, and an existing one is extended where it already is.

## Rules

1. **Look before you write.** Search for a helper before adding one. Shared
   helpers live in a few named places:
   - `spool_core::DisjointSets` for grouping records that are joined in
     pairs.
   - `crates/spool-cli/src/text.rs` for the words the commands print: counts
     and lists of names.
   - `packages/studio/src/format.ts` for how the studio writes values.
   A helper that two files need moves to the shared place; it is not copied.

2. **Do not hand-roll what a crate does.** Encodings (`hex`, `base64`), dates
   (`time`), comparisons that must take constant time (`subtle`), parsers of
   standard formats (`dotenvy`, `csv`, `url`) and signal handling
   (`signal-hook`) come from a dependency. Prefer one that is already in
   `Cargo.lock` or `bun.lock`.

3. **Name a function for what it does to what.** `hash_password`,
   `validate_email`, `format_timestamp`: a verb and a noun, so that a search
   finds it. One-word names such as `named`, `listed` or `text` mean something
   different in every file and hide the helper that already exists. A method
   whose type gives the object (`Accounts::list`) and a module's own verbs
   called with the module in front (`runs::get`) are fine as they are.

4. **Keep files under 800 lines.** `scripts/check-file-size.sh` fails a
   source file over the budget. The files that were over it when the check was
   added are listed in `scripts/file-size-allowlist.txt`, each with its size,
   and may only get smaller. Split a file by what it is about, not into
   `part1` and `part2`.

5. **One declaration per API operation.** An operation is a typed method of
   `App` in `crates/spool-cli/src/server/handlers/<concept>.rs`. The HTTP
   route is a thin function beside it, and the MCP tool is one
   `declare_tool(...)` in `server/mcp/tools/<concept>.rs`: its name, title,
   description, arguments and what it does, together, calling the same
   method. Request and response types are in `api.rs` and in the `Wire` list
   there, which the schema and the client's types are generated from.

6. **Generated files are regenerated, not edited.** They are marked
   `linguist-generated` in `.gitattributes`.

7. **An exception to a check says why, where it is.** The workspace denies
   `unsafe`, and a function over 120 lines fails clippy; oxlint's correctness
   rules fail the TypeScript. The few places that are excused carry
   `#[allow(unsafe_code)]` with a `SAFETY` comment,
   `#[expect(clippy::too_many_lines)]`, or an `oxlint-disable-next-line` with
   the reason on the line above. Do not add one to get past a check: shorten
   the function, or fix what the rule found.

8. **Tests say what they check.** A test is named for the behaviour, not for
   a milestone or a ticket. Each crate has one integration-test binary,
   `tests/it/`, and a new test file is a module of it. (`spool-cli` also
   has `tests/stress.rs` and `tests/studio.rs`, each one long test that
   runs a TypeScript package's tests and that CI runs in jobs of its own.)

## The studio

Routes are files under `packages/studio/src/routes/` (TanStack Router); the
route tree, `src/routeTree.gen.ts`, is generated by the build and committed.
A screen's filters are its route's search params, read through the
validators in `src/address.ts`. An address is written the way it always was,
so a link someone kept still opens the same screen.

A screen reads with TanStack Query through the client in `packages/client`.
The keys are all written in `src/queries.ts`, which also lists what each kind
of change makes stale. A change is made through `Act`, which is given the
keys it made stale and invalidates them; nothing reads everything again.

The studio is static files served by the binary under a policy that allows no
inline script, and the build refuses any file but the nine the binary serves.
So it has no server rendering, no code splitting, and no framework that needs
either.
