# Benchmarks

Each directory here is a Spool project with one or more scenarios: files that say what to ingest in which order, and what the truth is. `spool bench` runs one and scores the identities it ends with.

```sh
cargo build --release -p spool-cli
target/release/spool bench benchmarks/resolution/music/200k.toml
```

| Project | Records | What it is for |
| --- | --- | --- |
| `identity/companies-small` | 39 invented company records in two batches | The hard cases of company identity, small enough to read, with scripted corrections |
| `resolution/people` | Splink's `fake_1000`, and a committed sample of ten rows | A second entity type, and a Splink strategy beside the rules |
| `resolution/voters` | North Carolina voters, five sources of up to 1,000,000 | Scale: millions of records, few attributes, light errors. Sources that take rows back and send them again |
| `resolution/music` | MusicBrainz songs, five sources, up to 1,937,500 records | Sources that write the same thing in five ways, and heavy errors |
| `extraction/company-news` | Ten invented news items | Extraction from text, scored apart from resolution |

## Public data

The voter and song sets are public, and are not in this repository. `datasets.toml` lists where each is from and the checksum it must have. A scenario that names one downloads it into `benchmarks/.cache/` the first time, and refuses a file with another checksum.

Both are from the [benchmark datasets for entity resolution](https://dbs.uni-leipzig.de/research/projects/benchmark-datasets-for-entity-resolution) of the database group at Leipzig University, which publishes them under the Creative Commons Attribution 4.0 licence. Results on them should cite Saeedi, Peukert and Rahm, "Scalable Matching and Clustering of Entities with FAMER", ADBIS 2017. The records are real and the duplicates are made: a generator copied each record into some of the five sources and typed errors into the copies. `resolution/music/sample.csv` is 26 of the song records, committed so that the project can be tried without a download.

- **North Carolina voters.** A given name, a surname, a suburb and a postcode, and nothing else. 5,000,000 records of 3,500,840 people. No source holds a person twice. Half of the people are in every source unchanged, and the rest have errors in two of the four values.
- **MusicBrainz songs.** A title, an artist, an album, a track number, a length, a year and a language. One source writes the album into the title, another the artist, a third puts the track number before it. Each has its own unit for a length and its own way to write a year. 19,375, 193,750 and 1,937,500 records, of which half are songs that are in one source only.

A scenario takes part of a file with `where`, a condition in SQL on the file's columns, which are all text. `resolution/voters/5x500k.toml` takes the people with an even register number, which is the same half of the people in each of the five sources:

```toml
[[step.ingest]]
dataset = "ncvoters_5"
member = "5Party-ocp20/ncvr_numrec_1000000_modrec_2_ocp_20_myp_0_nump_5.csv"
source = "party_0"
entity_type = "person"
where = "recid::BIGINT % 2 = 0"
```

## What was measured

On four cores and 16 GB, with the release build, in one process:

| Scenario | Records | Candidate pairs | Seconds | Peak memory | Pair precision | Pair recall | False merges | False splits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `voters/5x100k.toml` | 500,323 | 1,038,709 | 15 | 1.4 GB | 0.988 | 0.881 | 1,682 | 11,093 |
| `voters/5x500k.toml` | 2,501,091 | 19,765,295 | 82 | 5.9 GB | 0.938 | 0.881 | 36,569 | 54,855 |
| `voters/5x1m.toml` | 5,000,000 | 76,148,339 | 210 | 13.4 GB | 0.880 | 0.879 | 128,355 | 109,304 |
| `music/20k.toml` | 19,375 | 15,981 | 1.5 | 0.2 GB | 1.000 | 0.857 | 0 | 989 |
| `music/200k.toml` | 193,750 | 215,812 | 8 | 0.7 GB | 0.997 | 0.862 | 112 | 9,633 |
| `music/2m.toml` | 1,937,500 | 7,985,984 | 67 | 4.3 GB | 0.959 | 0.863 | 8,392 | 96,276 |

Recall stays where it is as a set grows, and precision falls. Twice the people in the same towns is four times the pairs of different people who look alike, and one wrong pair joins two whole identities. The voter rules also leave 201, 3,124 and 13,755 records for review, where a pair the `relatives` rule keeps apart was matched through a third record.

The rules of both projects were written against the truth of a small part and then run on all of it. The first rules for the songs, "four of the six values agree", had precision 0.998 on 19,375 records, 0.955 on 193,750 and 0.157 on 1,937,500, where 2,532 different songs called "Intro" at track 1 became one identity. Rules that look right on a sample have to be measured at the size they will run at.

What the rules cannot say is in the numbers too. A voter source holds no person twice, and a rule cannot use that, because it sees the features of two records and not their sources.

## Data that changes

A source does not only add rows. `resolution/voters/changing.toml` begins with five sources of about 100,000 voters each, adds 100,000 rows, has two of the sources take 50,000 rows back, matches everything again, and has 10,000 of the rows sent again. A step takes rows back with `[[step.retract]]`, which names them by their keys, or by the rows of a file or a dataset as an ingest reads them:

```toml
[[step]]
name = "50,000 taken back"
rebuild = true

[[step.retract]]
dataset = "ncvoters_5"
member = "5Party-ocp20/ncvr_numrec_1000000_modrec_2_ocp_20_myp_0_nump_5.csv"
source = "party_0"
entity_type = "person"
where = "recid::BIGINT % 40 = 0"
```

The store is verified after every step, and the scenario fails at the first step that leaves it inconsistent. From a retraction on, the truth is scored on the rows the sources still hold. A step with `rebuild = true` also runs the steps so far in a second store that is never sent the rows that are gone, and reports how far the two agree: the share of records grouped with the same records in both, and the pairs that are together in one and not in the other.

On the same machine, in 216 seconds and 2.2 GB:

| Step | Records the sources hold | Pair precision | Pair recall | Kept their identity | Same group as resolved afresh | Pairs only here | Of them, put to a reviewer as a possible split | Pairs only there |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| five sources | 500,323 | 0.988 | 0.881 | | | | | |
| 100,000 more | 600,094 | 0.986 | 0.882 | 500,122 of 500,122 | | | | |
| 50,000 taken back | 549,978 | 0.986 | 0.882 | 549,757 of 599,854 | 549,799 of 549,978 | 142 | 142 | 0 |
| everything matched again | 549,978 | 0.986 | 0.882 | 549,787 of 549,787 | | | | |
| 10,000 come back | 559,990 | 0.986 | 0.883 | 549,787 of 549,787 | 559,837 of 559,990 | 125 | 119 | 0 |

Taking back 50,116 records took 50,097 of them out of an identity and changed nothing else: the records that kept their identity are all the others. Of the 10,012 records that came back, 10,009 went back to the identity they had left, and the other three had none when they were taken back. No identity was created for a row that came back.

The 142 pairs are what a retraction leaves behind. Two records that were matched only through a third stay in one identity when the third is taken back, because the engine does not take an identity apart on its own. Resolved afresh, they would be two identities. Matching everything again does not find them: it suggested no split, since no rule says the two differ, only that nothing joins them any more.

The run that takes the rows back does find them. For each identity it took a record out of, it works out which of the records that are left still hold together, and where they fall into more than one group it leaves a `possible_split` review item that names the identity and the groups. The two retractions left 70 items about 179 records, and the pairs of records in different groups of an item are the 142 pairs and no others. On this data 111 of the 142 are pairs of one person, so the store that kept them has the higher recall (0.8824 against 0.8821), and 31 are wrong merges, in 6 of the 70 identities. Nothing is split until a reviewer says so, and all 31 are in front of one.

When 10,000 of the rows come back, 13 of the items close on their own, because the record that held their groups together is in the identity again. One item is opened: a record went back to an identity that it had been joined to through a record that is still gone. The 58 items open at the end ask about 119 of the 125 pairs that only this store has. The other six are one identity of four records, and they are not a join that went stale. The four match each other, and were left for review in the first step because a fifth record matched two of them and not a third. They got their identity while the fifth was taken back. The fifth came back, and is in an open `conflict` item that names the identity.

The order of arrival is tried at the end on the rows the sources still hold: with the steps in reverse, 559,952 of 559,990 records are grouped as they are with the steps as written.

## Over HTTP

`spool bench` uses the engine in its own process. `packages/stress/src/replay.ts` runs a scenario that way, then sends the same rows to a server in the same runs, reads both stores from end to end, and fails unless the two agree on who is with whom:

```sh
cd packages/stress
bun src/replay.ts ../../benchmarks/resolution/voters/5x500k.toml
```

`music/20k.toml` is replayed in the tests. The larger scenarios are run by hand. A scenario that takes rows back is not replayed: the store keeps which records were taken back, and not which keys each call named.
