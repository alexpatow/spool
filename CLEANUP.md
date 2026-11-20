# Cleanup checklist

Working list for the cleanup branch. One commit per item, ticked as it lands.
This file is removed in the last commit; the conventions that outlive it are
in `AGENTS.md`.

Every step keeps behaviour as it is: the checks are `cargo fmt`, `cargo clippy
-D warnings`, the Rust tests, the packages' tests and `scripts/check-generated.sh`.

## A. Conventions and guardrails

- [x] A1. `AGENTS.md` at the root, with `CLAUDE.md` a symlink to it: where code
      goes, where shared helpers live, what is never hand-rolled, naming.
- [x] A2. `justfile`: `check`, `fmt`, `lint`, `test`, `regen`, one entry point
      for what `scripts/check-generated.sh` and CI spell out by hand.
- [x] A3. `[workspace.lints]`: `unsafe_code = "deny"` with an `allow` at each
      of the three places that need it; `clippy::too_many_lines` with the
      functions that are over it today marked, so new ones fail.
- [x] A4. `scripts/check-file-size.sh`: a line budget for source files with an
      allowlist of today's offenders that may only shrink. Run in CI.

## B. Files out of `src/`

- [x] B1. `crates/spool-cli/src/studio/*` (built bundle, fonts) to
      `crates/spool-cli/assets/studio/`.
- [x] B2. `crates/spool-cli/src/agents/` and `src/examples/` (embedded
      templates) to `crates/spool-cli/assets/`.
- [x] B3. `crates/spool/src/usercode/spool.{js,py}` to
      `crates/spool/assets/usercode/`.

## C. Crates in place of hand-rolled code

- [x] C1. `base64` (already a dependency) for `base64url`, `unbase64` and the
      two copies in the tests.
- [x] C2. `hex` for the thirteen `format!("{byte:02x}")` loops, three `fn hex`
      and `unhex`.
- [x] C3. `subtle` for the constant-time compare in `server.rs`.
- [x] C4. `time` for the calendar arithmetic in `spool/src/time.rs`.
- [x] C5. `dotenvy` for `parse_env`. Errors still name a line by its number
      alone.
- [x] C6. `signal-hook` for the `unsafe` `sigaction` in `commands/extract.rs`.

## D. One copy of each helper

- [ ] D1. Union-find: one in `spool-core`, used by `reconcile.rs`,
      `recompute.rs` and `possible_split.rs`.
- [ ] D2. `spool-cli/src/text.rs`: `listed`, `plural`/`count`, `share`, which
      are written two to four times across the commands and the server.
- [ ] D3. `is_zero`, `uuid_list`, `lock`: one each.

## E. Engine crate structure

- [ ] E1. `engine.rs` (3,435 lines) into `engine/{mod, read, load, matching,
      changes, recompute}.rs`.
- [ ] E2. One path for "who started this run": no more `Engine::x`,
      `Engine::x_by` and `Acting::x` for the same call.
- [ ] E3. `lookup.rs` into `lookup/{records, identities, log, info}.rs`.
- [ ] E4. `explain.rs` into `explain/{pair, identity, prose}.rs`.
- [ ] E5. `recompute.rs` into `recompute/{types, compute, outcome}.rs`.
- [ ] E6. `usercode.rs` into `usercode/{mod, env, task}.rs`.
- [ ] E7. `bench.rs` (and what only it uses) into a crate of its own,
      `spool-bench`.

## F. Server and CLI structure

- [ ] F1. `auth/` module: `accounts.rs`, `accounts/oauth.rs`, `google.rs`,
      `server/oauth.rs` and the sign-in section of `server.rs` in one place.
- [ ] F2. `server.rs` (2,972 lines) into `server/{mod, error, request,
      handlers/*}.rs`, handlers named after the engine module they call.
- [ ] F3. `commands/show.rs` (2,600 lines) into `commands/show/*.rs`, one per
      subcommand, and `commands/explain.rs`.
- [ ] F4. `server/mcp.rs` into `server/mcp/{mod, tools, limits}.rs`.
- [ ] F5. MCP tools call typed functions, not handlers through strings.
- [ ] F6. Try rmcp's `macros` feature for the tool table and dispatch; keep
      the table if the macros cannot carry the annotations and size limits.
- [ ] F7. Greppable names: verb-and-noun names for the private helpers whose
      one-word names collide (`named`, `listed`, `list`, `text`, ...).

## G. Tests

- [ ] G1. One integration-test binary per crate (`tests/it/`), with the shared
      helpers as a module of it and not a `#[path]` into another crate.
- [ ] G2. CI: the test jobs divide one binary with nextest partitions; the
      "every test file is in one job" check goes.
- [ ] G3. `milestone_1.rs` and `milestone_3.rs` named for what they test.

## H. JavaScript

- [ ] H1. One bun workspace at the root: one lockfile, packages import each
      other by name.
- [ ] H2. `packages/ui`: the Shelf tokens, themes, components and the mark,
      used by the site and the studio. The studio's copy step goes.
- [ ] H3. One harness that starts a real `spool` for the client's tests, the
      studio's and the stress tests.
- [ ] H4. Studio on TanStack Router: file routes, typed search params;
      `router.ts` goes.
- [ ] H5. Studio on TanStack Query: `useLoad`, `usePages`, `useNames`,
      `useLabels` and the global `version` counter go.
- [ ] H6. `components.tsx` into `components/{layout, table, refs, filters}.tsx`.
- [ ] H7. `screens/review.tsx` into `screens/review/*.tsx`; `app.tsx` into
      `app/{door, shell}.tsx`.
- [ ] H8. One linter for TypeScript (Biome) at the workspace root, and `knip`
      for exports, files and dependencies nothing uses.

## I. Repository

- [ ] I1. `examples/extractors/model.ts` and `release/by-legal-form.py` to
      where the tests that use them look.
- [ ] I2. `docs/`: planning documents under `docs/design/`.
- [ ] I3. CI: file-size check, `cargo-machete`, `jscpd`.
- [ ] I4. `AGENTS.md` brought up to date with the tree as it ends up; this
      file removed.
