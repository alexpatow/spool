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
| `extraction/linked-docred` | Linked-DocRED: the openings of 5,051 Wikipedia articles, with 103,888 labelled mentions | Text through to identities on public data: extraction, resolution, and the two end to end |

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

Linked-DocRED is public too, and under another licence: the GNU General Public License, version 3. None of it is in this repository, and nothing made from it but the scores below. `extraction/linked-docred/run.py` downloads it when the benchmark is run. It is from [alteca/Linked-DocRED](https://github.com/alteca/Linked-DocRED), and results on it should cite Genest, Portier, Egyed-Zsigmond and Lovisetto, "Linked-DocRED: Enhancing DocRED with Entity-Linking to Evaluate End-To-End Document-Level Information Extraction Pipelines", SIGIR 2023. [From text to identities](#from-text-to-identities) says what is in it.

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

## From text to identities

A scenario whose steps hold documents is scored three times. Extraction is the mentions an extractor found against the labelled ones: a mention counts when its type and its place are both right. Resolution is the identities against the true entities, over the mentions that were found and are labelled. End to end is the same question over every labelled mention and everything that was found: a mention that was missed is with nothing, and a mention that was made up is wrong wherever it is put. It is the score of what a reader of the store gets.

`extraction/company-news` is ten invented items, small enough to read. `extraction/linked-docred` asks the same of public data. Linked-DocRED is the opening section of 5,051 English Wikipedia articles, a million tokens with the punctuation counted. People marked every mention of a person, an organisation, a place and other named things in them, and the Wikidata entry each is a mention of. The entries are the truth about identity: 46,625 entities, of which 4,870 are mentioned in more than one article.

```sh
cargo build --release -p spool-cli
uv run benchmarks/extraction/linked-docred/run.py
```

The script downloads the dataset's three files, 45 MB, into `benchmarks/.cache/`, and checks them against `datasets.toml`. It writes each article as a text file and the labels as files in the shape of `company-news/mentions.csv`, and a scenario for each entity type and extractor. It runs each with `spool bench` and prints the reports side by side. The three files are three batches, so a report also says whether identities stayed put as documents arrived. `--documents 200` takes the first 200 articles of each batch, and `--types person` one type.

There are four extractors. Two are the benchmark, and run unless more are asked for:

- `gold` reads the labels, so its scenarios measure resolution alone.
- `suffix` takes a run of capitalised words that ends in "Ltd", "University", "Party" or one of 67 such words to be an organisation, as the extractor of `company-news` does. It is the least an extractor can do without a model.
The other two are a worked example: a model put in as an extractor and scored end to end, as you would score your own.

- `spacy` is spaCy's small English model, `en_core_web_sm`.
- `gliner` is GLiNER small. It is told what to find in words: "person", "organization", "location", and "product", "event", "work of art", "law" and "language" for the other things.

```sh
uv run benchmarks/extraction/linked-docred/run.py --extractors gold,suffix,spacy,gliner
```

Both models run on the machine the benchmark runs on, and no document leaves it. `uv` fetches each the first time, about 170 MB for spaCy and 330 MB for GLiNER. On four cores and 16 GB, with the release build, `gold` and `suffix` take about half a minute together. spaCy reads the articles in a minute and a half and GLiNER in seventeen minutes, and the script keeps what a model found, so that each reads an article once and not once for every scenario.

Each of the two is one Python file under `extractors/`, with its packages named at the top of the file, and a line in `EXTRACTORS` in `run.py` that says which entity types it finds. Another extractor is put in the same way, and either file is a place to start from. Read the two models' rows below as what the three scores look like for an extractor, and how to read them. They do not measure Spool, and they are not advice on which model to use.

With `gold`, every row is 1.000.

| Type | Extractor | Labelled mentions | Extracted | Correct | Precision | Recall |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| organisation | `suffix` | 19,021 | 4,597 | 3,070 | 0.668 | 0.161 |
| organisation | `spacy` | 19,021 | 25,895 | 6,939 | 0.268 | 0.365 |
| organisation | `gliner` | 19,021 | 13,912 | 10,511 | 0.756 | 0.553 |
| person | `spacy` | 24,239 | 25,062 | 15,943 | 0.636 | 0.658 |
| person | `gliner` | 24,239 | 18,223 | 15,875 | 0.871 | 0.655 |
| place | `spacy` | 40,640 | 28,457 | 20,729 | 0.728 | 0.510 |
| place | `gliner` | 40,640 | 24,046 | 19,681 | 0.818 | 0.484 |
| other | `spacy` | 19,988 | 5,569 | 2,550 | 0.458 | 0.128 |
| other | `gliner` | 19,988 | 7,158 | 5,308 | 0.742 | 0.266 |

**A list of endings finds a sixth of the organisations.** 3,609 of the 19,021 mentions end in one of the words, so no such list finds more than a fifth. Of the 1,527 mentions `suffix` has wrong, 980 are part of a labelled organisation, or a labelled organisation and more: the place is wrong and the thing is right. The other 547 are not organisations by the labels, and most of those are labelled as places.

**Precision and recall say what an extractor found.** `gliner` has precision 0.74 to 0.87 for every type. It finds two thirds of the people, half of the organisations and the places, and a quarter of the other things. `spacy` finds as many people and places with lower precision, and for organisations it is wrong three times in four.

**A wrong mention is the wrong thing, or the right thing with the wrong edges.** Of `spacy`'s 25,895 organisations, 8,426 are exactly where a person, a place or another thing is labelled, and 4,593 are a labelled organisation and more. The more is "the" and nothing else for 3,964 of them. The model takes the article into a name, and the labels mostly do not, though they do for some names: "the" is inside a tenth of the labels that follow one. Counting a mention as right when it overlaps a label of its type, `spacy`'s organisations have precision 0.476 and recall 0.641, and `gliner`'s 0.823 and 0.600. spaCy's model also calls a nationality or a religious or political group one kind of thing, which this project has no type for, and the labels spread over three: it found 8,637, and they are left out.

Resolution, over the mentions that were found and are labelled:

| Type | Extractor | Mentions | Entities | Identities | Pair precision | Pair recall | B-cubed precision | B-cubed recall | False merges | False splits |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| organisation | `gold` | 19,021 | 8,953 | 11,098 | 0.736 | 0.522 | 0.934 | 0.756 | 361 | 1,923 |
| person | `gold` | 24,239 | 13,199 | 14,004 | 0.841 | 0.783 | 0.954 | 0.914 | 456 | 1,027 |
| place | `gold` | 40,640 | 12,808 | 13,847 | 0.788 | 0.373 | 0.860 | 0.734 | 886 | 1,943 |
| other | `gold` | 19,988 | 11,665 | 13,403 | 0.803 | 0.642 | 0.954 | 0.838 | 352 | 1,524 |
| organisation | `suffix` | 3,070 | 1,954 | 1,972 | 0.866 | 0.832 | 0.954 | 0.940 | 57 | 97 |
| organisation | `spacy` | 6,939 | 3,625 | 3,956 | 0.769 | 0.772 | 0.927 | 0.865 | 150 | 481 |
| organisation | `gliner` | 10,511 | 5,610 | 6,388 | 0.887 | 0.701 | 0.966 | 0.849 | 141 | 778 |
| person | `spacy` | 15,943 | 10,416 | 10,733 | 0.852 | 0.785 | 0.966 | 0.941 | 250 | 501 |
| person | `gliner` | 15,875 | 11,725 | 12,352 | 0.907 | 0.731 | 0.986 | 0.935 | 122 | 634 |
| place | `spacy` | 20,729 | 5,634 | 5,093 | 0.816 | 0.609 | 0.863 | 0.910 | 441 | 335 |
| place | `gliner` | 19,681 | 8,118 | 8,204 | 0.875 | 0.849 | 0.929 | 0.931 | 403 | 537 |
| other | `spacy` | 2,550 | 1,721 | 1,720 | 0.793 | 0.934 | 0.942 | 0.971 | 37 | 30 |
| other | `gliner` | 5,308 | 2,845 | 2,950 | 0.820 | 0.748 | 0.952 | 0.930 | 66 | 149 |

End to end. With `gold` it is the row above:

| Type | Extractor | Entities | With a mention found | Pair precision | Pair recall | B-cubed precision | B-cubed recall |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| organisation | `suffix` | 8,953 | 1,954 | 0.370 | 0.054 | 0.615 | 0.119 |
| organisation | `spacy` | 8,953 | 3,625 | 0.300 | 0.222 | 0.228 | 0.252 |
| organisation | `gliner` | 8,953 | 5,610 | 0.542 | 0.204 | 0.703 | 0.408 |
| person | `spacy` | 13,199 | 10,416 | 0.584 | 0.355 | 0.600 | 0.550 |
| person | `gliner` | 13,199 | 11,725 | 0.694 | 0.220 | 0.845 | 0.523 |
| place | `spacy` | 12,808 | 5,634 | 0.636 | 0.207 | 0.588 | 0.372 |
| place | `gliner` | 12,808 | 8,118 | 0.589 | 0.049 | 0.703 | 0.358 |
| other | `spacy` | 11,665 | 1,721 | 0.336 | 0.249 | 0.402 | 0.099 |
| other | `gliner` | 11,665 | 2,845 | 0.493 | 0.231 | 0.666 | 0.209 |

The rules are short. An organisation, a place and a thing of another kind are one identity when their names are the same once case, accents, punctuation and a leading "the" are gone, and for an organisation a legal form at the end. Nothing else is known of a mention but its name and the document it is in.

**A name alone gets most mentions right and most pairs wrong.** B-cubed precision is 0.86 to 0.95: a mention's identity mostly holds mentions of the same thing. Pair recall is 0.37 to 0.64, because pairs count the largest entities many times over. One country has 2,309 mentions and is written in six common ways, among them its adjective and two abbreviations, and it alone is 1.9 million of the 2.6 million pairs the places miss. No rule about spelling joins a country to its adjective. That takes a list of names, or a model that knows them.

**The same name is not the same thing.** The wrong pairs are nearly all across articles: the same one or two words for two things. A word like the name of a parliament is a different body in each country's articles, and an adjective is a country in one place and a language in another. A quarter to two thirds of each type's wrong pairs come from its five most common such names. The context that tells them apart is in the article and not in the name.

**People are where the document helps.** An article names a person in full once and by surname after that, and 72% of the pairs of mentions of one person are within one article. With "the same name" as the only rule, people had pair precision 0.702 and recall 0.501. The rules here are three. A name of two words or more is one person wherever it is written. Within one document, the same name is one person, and so is a name that is the first or the last words of a longer one. That is the row above: 0.841 and 0.783. A rule sees the features of two mentions, so the extractor gives each mention the key of its document as an attribute, and the rules compare it. The other three types keep the one rule. Worked out from the labels, the rule about parts of a name would take the organisations' pair precision from 0.736 to 0.447 for a recall of 0.702, and it does no better for places and other things.

**Extraction decides the end-to-end score.** The mentions `suffix` finds resolve better than the labelled ones do, 0.866 and 0.832, because a name that ends in "University" is a full name. End to end it is 0.370 and 0.054. It finds a mention of 1,954 of the 8,953 organisations, and every mention at the wrong place counts against precision once it is in an identity. Resolution cannot give back what extraction did not find.

**End to end is what would be in the store.** With `gliner`, B-cubed precision is 0.67 to 0.85, where the labels themselves give 0.86 to 0.95: a mention in the store is mostly with mentions of the same thing. Recall is 0.21 to 0.52, where they give 0.73 to 0.91: half the labelled mentions are not in the store. The mentions it does find resolve with higher pair precision than the labelled ones, in every type.

**Pairs follow the largest entities, and one country decides the places.** `gliner` finds a mention of 8,118 of the 12,808 places and `spacy` of 5,634, and `spacy`'s pair recall is four times `gliner`'s, 0.207 against 0.049. `spacy` found 1,297 of the 2,309 mentions of the most mentioned country and `gliner` 264. This is a name the labels take the article into more often than not, and `gliner` never does. It also passes over the country's abbreviations, which `spacy` finds. B-cubed, which counts each mention once, has the recall of the two level, 0.372 and 0.358.

**Identities stay put.** The articles arrive in three batches, and no mention changed identity when a later batch arrived, in any scenario. For people, a later batch brought mentions that match two identities that were already there: four times with `gold` and `spacy`, and twice with `gliner`. The engine does not join identities on its own. With `gold` it left those 32 mentions without one and put the four pairs of identities to a reviewer. Read in the reverse order, 99.6% of people's mentions are grouped as they are in this order, 99.8% with `gliner`, and 100% of the other types', whose one rule cannot depend on the order.

What these numbers are not: the rows of `spacy` and `gliner` are an example of scoring an extractor, and neither a measure of Spool nor a ranking of models. A mention whose place is off by a word counts as missed and as made up, which is hard on an extractor that finds the right thing. Neither model was trained on these labels, and each was given the entity type that fits its own kinds by their descriptions, before anything was measured. An entity with no Wikidata entry, 8% of the mentions, is nearly always a thing of its own in each article, as the dataset has it, though two articles may mean the same one. And the rules for people were chosen after the first run showed where a name alone fell short, on the same articles.

`sample/` holds five invented documents in the dataset's format. A test runs the script on them with `gold` and `suffix`, so that the script and the project stay in step with `spool bench`. The benchmark itself is run by hand.

## Over HTTP

`spool bench` uses the engine in its own process. `packages/stress/src/replay.ts` runs a scenario that way, then sends the same rows to a server in the same runs, reads both stores from end to end, and fails unless the two agree on who is with whom:

```sh
cd packages/stress
bun src/replay.ts ../../benchmarks/resolution/voters/5x500k.toml
```

`music/20k.toml` is replayed in the tests. The larger scenarios are run by hand. A scenario that takes rows back is not replayed: the store keeps which records were taken back, and not which keys each call named.
