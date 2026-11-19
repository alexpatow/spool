---
name: spool-api
description: Call a Spool server over its HTTP API under /v1 or its MCP endpoint at /mcp - load rows, store documents, run extractions and matches, look up identities, and commit changesets that merge, split, undo or constrain identities - and deploy one with spool build, spool apply, spool serve and spool token. Use when writing code or a pipeline against Spool, connecting an agent to it over MCP, correcting identities, or running Spool on a server.
---

# Call the server

`spool dev` serves `http://127.0.0.1:7765` with no token. `spool serve` wants `Authorization: Bearer <token>`. JSON in and out; a field the server does not know is refused. A call that starts work answers with a run at once; `wait_ms` waits for it. `spool schema` prints the JSON Schema of every body.

## Load and match

To store one row and find its identity in one call, send `source`, `entity_type` and one `row` to `POST /v1/records` or the `add_record` MCP tool. If the record needs review, `identity` is null and `review` names the item and the possible identities. Set `replace` to true only if the person asked to change the row under that key.

```sh
curl -s -X POST http://127.0.0.1:7765/v1/load -d '{"source": "crm", "entity_type": "company", "wait_ms": 5000, "rows": [{"id": "c-1", "name": "Acorn Tools", "country": "GB"}]}'
curl -s -X POST 'http://127.0.0.1:7765/v1/load?source=crm&entity_type=company&wait_ms=5000' -H 'content-type: text/csv' --data-binary @samples/crm-2.csv
curl -s -X POST http://127.0.0.1:7765/v1/match -d '{"entity_type": "company", "wait_ms": 5000}'
curl -s 'http://127.0.0.1:7765/v1/runs/3?wait_ms=5000'
```

`POST /v1/match` takes `rematch`, `strategy` and `"commit": false`, which leaves the result as a draft for a person to approve.

## Documents

```sh
curl -s -X POST 'http://127.0.0.1:7765/v1/documents?source=notes&key=notes-1.txt&wait_ms=5000' -H 'content-type: text/plain' --data-binary @samples/notes-1.txt
curl -s -X POST http://127.0.0.1:7765/v1/extractions -d '{"extractor": "companies", "wait_ms": 60000}'
```

## Look up

| Call | Answers |
| --- | --- |
| `GET /v1/info` | entity types, features, sources, extractors, counts |
| `GET /v1/identities?q=acorn` | identities by words, `entity_type` or `ids` |
| `GET /v1/identities/{id}` | one identity and its records; a merged-away ID answers with its successor |
| `GET /v1/records?source=crm&key=c-5` | records by `source`, `key`, `identity`, `q`, `unassigned` |
| `POST /v1/records/identities` | the identity of each of many records |
| `GET /v1/explain?a=crm:c-5&b=crm:c-6` | why two records share an identity or not |
| `GET /v1/review` | review items |
| `GET /v1/changesets?after=<seq>` | the log, for a consumer that follows changes |

Lists answer 100 entries and `next`; pass it back as `after`.

## Change identities

Every change is a changeset with a `reason`, recorded under the caller:

```sh
A=$(spool show records crm:c-5 --json | jq -r .record.id)
B=$(spool show records crm:c-6 --json | jq -r .record.id)
curl -s -X POST http://127.0.0.1:7765/v1/changesets -d '{"reason": "different company numbers", "ops": [{"op": "constrain", "kind": "cannot_link", "a": "'$A'", "b": "'$B'"}]}'
```

Operations: `merge` (`from`, `into`), `assign` (`record`, `from`, `to`), `create_identity` (`identity` from `POST /v1/ids`, `entity_type`), `constrain` and `unconstrain` (`kind` `must_link` or `cannot_link`, `a`, `b`). Also `POST /v1/identities/{id}/split` with `{"records": [...], "reason": "..."}`, `POST /v1/changesets/{seq}/undo` with `{"reason": "..."}` (and `"commit": false` to preview), and `POST /v1/review/{item}/dismiss`. Never change identities without the user's agreement.

## MCP

The same server answers MCP at `/mcp`:

```sh
claude mcp add --transport http spool http://127.0.0.1:7765/mcp
codex mcp add spool --url http://127.0.0.1:7765/mcp
```

Read with `describe_store`, `search_identities`, `get_identity`, `get_identity_history`, `search_records`, `get_record`, `explain_pair`, `list_review_items`, `get_review_case`, `list_constraints`, `list_changesets`, `list_changeset_ops`, `get_run` and `mint_identity_ids`. Write with `add_record`, `load_rows`, `load_documents`, `run_extraction`, `run_match`, `retract_rows`, `commit_changeset`, `split_identity`, `undo_changeset` and `dismiss_review_item`. Retractions and manual identity or review decisions need a `reason`. `add_record` and `load_rows` refuse a changed row unless `"replace": true`. An agent cannot approve drafts, make tokens or change project files over MCP.

`list_changesets` gives summaries with operation counts. Pass `next` as `after` to read the next page. To inspect a changeset, pass its `seq` to `list_changeset_ops`, then pass `next` as `offset` until it is null.

## Deploy

```sh
spool build
spool apply --store /srv/spool/spool.duckdb
spool serve --store /srv/spool/spool.duckdb --public-url https://spool.example.com
spool token add pipeline --store /srv/spool/spool.duckdb
```

`spool build` checks the project and writes `.spool/manifest.json`, and a Dockerfile if there is none. `spool apply` applies the manifest to a store while no server runs on it; after a change of project files, build and apply again. `spool serve` listens on `0.0.0.0:7765`. `spool token add <name>` prints a token once; a caller with it is recorded as `agent:<name>`. https://spool.alexpatow.com/docs/deploying has the rest.
