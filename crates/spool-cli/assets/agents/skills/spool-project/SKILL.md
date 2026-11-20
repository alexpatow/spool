---
name: spool-project
description: Create a Spool project and declare what it holds - sources of rows or documents, entity types with their features and mappings, and SQL transforms - then check it with spool info. Use when starting a Spool project, adding a source, a column, a feature or an entity type, or when spool info or spool dev reports an error in a project file.
---

# Declare a Spool project

A project is a directory kept in Git. A file's path is its name: `sources/crm.yaml` is the source `crm`, `entities/company/` is the entity type `company`, `transforms/name_key.sql` is the transform `name_key`. Names are lowercase letters, digits and underscores, starting with a letter.

## Make one

```sh
spool init my-data
spool init companies --example companies
```

The first writes the folders, `.gitignore`, `.env.example` and these agent files. The second is a whole working project: copy its files when unsure of a shape. `spool init` refuses a folder that holds anything but `.git` and `.DS_Store`.

```text
sources/<source>.yaml               a source of rows or of documents
entities/<type>/entity.yaml         an entity type: its features and where each comes from
entities/<type>/rules/<kind>/*.sql  its rules: see the spool-rules skill
entities/<type>/extract.md          optional: what an extractor looks for
transforms/<name>.sql               a SQL function that features are made with
extractors/<name>.py or .ts         see the spool-extractor skill
.env                                secrets for extractors; Git ignores it
.spool/                             what Spool writes; Git ignores it
```

## A source of rows

```yaml
# sources/crm.yaml
key: id
columns: [id, name, country, website]
```

- `columns`: every column a row may have. A row with another column is refused.
- `key`: the column that tells one row from another. Loading a row under a key the store holds, with other values, makes a new version of that record.
- `evidence`: optional, a column of JSON text that says where the row came from. It is kept and not compared.

There are no other keys.

## A source of documents

```yaml
# sources/notes.yaml
columns: [name]
```

No `key`. `columns` are the attributes an extractor may give what it finds.

## An entity type

```yaml
# entities/company/entity.yaml
label: name
features: [name, name_key, country, domain]

mapping:
  crm:
    name: name
    name_key: { from: name, transform: name_key }
    country: { from: country, transform: upper }
    domain: { from: website, transform: domain_of }
  notes:
    name: name
    name_key: { from: name, transform: name_key }
```

- `features`: the columns of the type's feature table, which is what rules compare.
- `label`: the feature a record is shown by.
- `mapping`: for each source, where each feature comes from: a column as it is, or `{ from: <column>, transform: <name> }`. `from` takes a list of columns for a transform of several arguments.
- A feature a source does not map is empty for its records. Every feature is mapped by at least one source.
- A transform with no file is the DuckDB function of that name, such as `upper` or `lower`.

## A transform

```sql
-- transforms/name_key.sql
lambda name: regexp_replace(lower(trim(name)), '^the ', '')
```

```sql
-- transforms/domain_of.sql
lambda website: nullif(regexp_extract(lower(website), '^(?:https?://)?(?:www\.)?([^/:?#]+)', 1), '')
```

`lambda`, the arguments, and one DuckDB expression. Every value arrives as text, so cast it (`try_cast(founded AS DATE)`). A transform reads one record's values and no table.

## Check it

```sh
spool info
```

It needs no server. It lists every source with the features each column becomes, every entity type with its features and rules, and every extractor, and checks every SQL expression on the way. A mistake is an error that names the file and the field:

```text
error: ./entities/company/entity.yaml: mapping of `name` from `registry`: `legal_nmae` is not a column of sources/registry.yaml
```

With `spool dev` running, a saved file is applied at once, and a file that does not compile is reported in its terminal while the last good version keeps running. A changed `entity.yaml` or transform computes the features again from the stored rows. No identity changes: to see what new features would change, use `spool recompute` (the spool-rules skill).

## Not possible

- A feature is one SQL expression. A normaliser that needs a library or a lookup table runs before the rows are loaded.
- Entity types are files, not API calls. Rules compare records of one type only.
