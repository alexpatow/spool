---
name: spool-rules
description: Write and change Spool's matching rules - block, match and no_match SQL conditions over two records - test them with spool explain, and see and apply what a better rule would change in identities that exist with spool match --rematch and spool recompute. Use when records that are one thing are not matched, when different things were merged, or when asked to tune matching.
---

# Write matching rules

A rule is a file holding one SQL condition over two rows of the entity type's feature table, `l` and `r`. The directory is its kind, and the file's name is its name:

| File | Says |
| --- | --- |
| `entities/<type>/rules/block/<name>.sql` | This pair is worth comparing. A pair no block rule finds is never compared |
| `entities/<type>/rules/match/<name>.sql` | This pair is the same thing |
| `entities/<type>/rules/no_match/<name>.sql` | This pair is not. It wins over every match rule |

A pair is judged `no_match` if any no_match rule holds, else `match` if any match rule holds, else not judged. The columns are the features of `entity.yaml`, plus `record_id`.

```sql
-- entities/company/rules/block/name_prefix.sql
left(l.name_key, 4) = left(r.name_key, 4)
```

```sql
-- entities/company/rules/match/similar_name.sql
jaro_winkler_similarity(l.name_key, r.name_key) >= 0.92
```

```sql
-- entities/company/rules/no_match/different_country.sql
l.country <> r.country
```

## Write them well

- **A block rule is one equality**, so the database finds pairs with a join. One file per equality: a block rule with `OR` compares every record with every other.
- **Empty values.** A comparison with an empty feature is neither true nor false, so `l.country <> r.country` does not keep apart two records of which one has no country. Inside `AND`, `OR` or `NOT`, say what empty means: `coalesce(l.country = r.country, false)`.
- **Normalise in a transform, not in every rule.** Compare a feature such as `name_key` that a transform made (the spool-project skill).
- DuckDB has `jaro_winkler_similarity`, `levenshtein`, `damerau_levenshtein`, `jaccard`, `regexp_replace`, `regexp_extract` and `strip_accents`.

## Check a rule

```sh
spool info
spool explain crm:c-5 crm:c-6
```

`spool info` prepares every rule against the feature table and names the one that fails. `spool explain` takes two records and says what each run concluded about them, and which rules hold for them as the rules are now:

```text
rules as they are now
  block     name_prefix        holds
  no_match  different_country  holds
  match     similar_name       holds
```

`spool explain <record>` alone says what holds that record's identity together, and which rule joined its parts. A wrong merge shows there.

## What a change does

A rule changes no identity by itself, and placed records are not matched again: identities keep their IDs and members. New records are matched by the new rules at the next `spool match`. For records that are placed:

```sh
spool match --rematch
spool recompute
spool recompute --detail
```

`--rematch` judges every record again and lists suggested merges and splits, moving nothing. `spool recompute` shows what the identities would be if every record were matched again, all at once, against those there are: the summary, the largest changes, and the command that applies it, which names a plan. It changes nothing. Show the user what it says before applying:

```sh
PLAN=$(spool recompute --json | jq -r .plan)
spool recompute --apply "$PLAN" --reason "names without a leading the"
```

Applying refuses if anything changed since the preview. An identity keeps its ID with the group that gets most of its records, and an identity merged into another has its ID answer for it. What people decided holds: `must_link`, `cannot_link`, and records a person put together.

## Measure

- `spool show identities --sort records --order desc` lists the largest identities first. `--sort names --order desc` lists those with the most different names: dozens of names is many things merged.
- `spool show records --unassigned` lists the records without an identity: new ones no match has placed, and ones a match left for review because they matched two identities. `spool show review` says why.
- `spool show attributes` says how full each column is and its most common values, before you write a rule that depends on it.

A rule cannot say `unsure`, read the record's source, or compare two entity types. For matching in code, put a Python or TypeScript strategy in `entities/<type>/strategies/` and run `spool match --strategy <name>`; https://spool.alexpatow.com/docs/project-structure#strategies has the shape.
