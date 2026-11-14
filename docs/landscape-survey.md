# Spool landscape survey

Survey of open-source entity resolution as of 2026-11-13, written to ground the [architecture proposal](architecture-proposal.md).

## Summary of conclusions

1. **Matching is a solved-enough problem; identity is not.** Splink, Zingg Community, dedupe, recordlinkage and pyJedAI all stop at "pairs, then clusters". Their cluster IDs are a by-product of one run and change when the data or the model changes.
2. **Persistent identity is not an unoccupied space.** Spool started from the assumption that persistent identity is what separates it from existing open-source tools. Three projects already do part of it: nomenklatura (in production behind OpenSanctions), GoldenMatch (open source, created in 2026) and Zingg Enterprise (closed source). Each has a recognisable design, and none separates identity semantics into a small engine that other matchers can plug into.
3. **Four identity designs exist in the wild.** (a) IDs derived from cluster contents, (b) IDs carried over by cluster overlap after each batch run, (c) a persisted graph of pairwise judgements from which canonical IDs are computed, (d) a stateful engine that resolves each record against existing entities. Only (c) and (d) keep manual decisions safe, and only (c) is simple enough to copy. Spool should take (c)'s evidence model and add explicit, validated state transitions on top.
4. **The data shape at the matcher boundary has converged.** Every tool can be described as: records in, candidate pairs, scored pairs out. Nearly all of them then cluster with connected components. That makes "scored pairs" the natural interchange contract and makes clustering the place where Spool must differ.
5. **Evaluation practice is pairwise and static.** FEBRL, the Leipzig and Magellan sets and WDC Products measure pair or cluster quality on a frozen snapshot. I found no public benchmark for ID stability, churn, order sensitivity or split recovery. Spool will have to define those, and doing so is a contribution in itself.
6. **The real gap** is a small, embeddable identity engine with: stable IDs that survive merges, splits and model upgrades; changes expressed as auditable changesets; one implementation behind Rust, TypeScript and Python; a matcher-neutral contract; and benchmarks that measure identity behaviour over time.

## How this was verified

I cloned each repository at the commit listed below and read the code and docs directly, read the Zingg documentation export, fetched vendor pages for Senzing and OpenSanctions, and checked paper titles against arXiv and Crossref. Package versions come from PyPI and crates.io on 2026-11-13. Performance and accuracy numbers quoted below are the projects' own claims; I reproduced none of them. Section 8 lists what I could not verify.

| Project | Commit read | Latest release | Licence |
| --- | --- | --- | --- |
| [Splink](https://github.com/moj-analytical-services/splink) | `f319070` (2026-11-11) | 5.0.0 (2026-09-28) | MIT |
| [Zingg](https://github.com/zinggAI/zingg) | `9d1a7f4` (2026-10-27) | 0.7.0 (2026-08-10) | AGPL-3.0 |
| [dedupe](https://github.com/dedupeio/dedupe) | `3f61e79` (2025-07-28) | 3.0.3 (2024-08-15) | MIT |
| [recordlinkage](https://github.com/J535D165/recordlinkage) | `b93d976` (2023-07-20) | 0.16 (2023-07-20) | BSD-3-Clause |
| [pyJedAI](https://github.com/AI-team-UoA/pyJedAI) | `eb44609` (2026-03-22) | 0.3.6 (2025-11-03) | Apache-2.0 |
| [Ditto](https://github.com/megagonlabs/ditto) | `5298556` (2021-09-24) | none on PyPI | Apache-2.0 |
| [nomenklatura](https://github.com/opensanctions/nomenklatura) | `0ef7913` (2026-11-12) | 4.17.6 (2026-11-06) | MIT |
| [matchbox](https://github.com/uktrade/matchbox) | `d2c7aa4` (2026-09-03) | 0.10.8, archived | MIT |
| [matchlab](https://github.com/uktrade/matchlab) | `906fb12` (2026-11-05) | alpha | MIT |
| [GoldenMatch](https://github.com/benseverndev-oss/goldenmatch) | `11e3f93` (2026-11-13) | 3.22.0 | MIT |
| [zer](https://github.com/ZAL-Analytics/zer) | `9ed85b1` (2026-09-12) | published on crates.io | Apache-2.0 |
| [Senzing](https://senzing.com/how-entity-resolution-works-with-senzing/) | docs only | SDK v4 | proprietary |

## 1. At a glance

| | Matching approach | Execution | Clustering | Incremental | Stable entity ID | Merge and split history | Provenance |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Splink 5 | Fellegi-Sunter, unsupervised | SQL generated for DuckDB, Spark, Postgres, SQLite | Connected components, one-to-one variant | New pairs only | No | No | Per-comparison match weights on each edge |
| Zingg Community | Learned blocking tree plus classifier, active learning | Spark | Connected components | No | No | No | Cluster min and max score |
| Zingg Enterprise | Same plus deterministic rules | Spark, Snowflake | Same | Yes | Yes, with caveats | Automatic, not documented as a log | `explain` phase |
| dedupe | Learned blocking predicates plus logistic regression, active learning | In-process Python | Hierarchical, centroid linkage | No | No | No | Cluster confidence |
| recordlinkage | Toolkit: indexers, comparers, classifiers | pandas | None | No | No | No | None |
| pyJedAI | Schema-agnostic blocking, meta-blocking, embeddings, LLM | In-process Python | Twelve algorithms | No | No | No | None |
| Ditto and LLM matchers | Fine-tuned or prompted language models on serialised pairs | GPU or API | Usually none | No | No | No | Free-text rationale, where given |
| nomenklatura | Rule and regression matchers over FollowTheMoney entities | Python, SQL store for decisions | Connected components over judgements | Yes | Yes | Edge log with soft delete | Statement-level |
| GoldenMatch | Fuzzy, Fellegi-Sunter, LLM, auto-configured | Python with Rust kernels, separate TypeScript port | Its own | Yes | Yes | Append-only events, hash-chained | Per-field |
| Senzing | Principle-based, entity-centric | Proprietary engine on a SQL database | Stateful per record | Yes, real time | No, entity IDs move | Self-correcting, no ordered change log | `why` and `how` commands |

## 2. Splink

Splink is the reference point for local, SQL-first matching, and Spool's default strategy should look a lot like it.

**Algorithm.** A Fellegi-Sunter model: each column comparison is discretised into levels, each level has an m and a u probability, and a pair's match weight is the sum of per-level log odds ([theory guide](https://moj-analytical-services.github.io/splink/topic_guides/theory/fellegi_sunter.html), [paper](https://doi.org/10.23889/ijpds.v7i3.1794)). Training needs no labels: the prior comes from deterministic rules with an assumed recall, u from random sampling, m from expectation maximisation under a blocking rule. Term-frequency adjustments handle common values. Splink 5 switched its outputs from Bayes factors to match weights for numerical stability ([release note](https://moj-analytical-services.github.io/splink/blog/2026/09/28/splink-500-released.html)).

**Blocking.** Hand-written SQL rules, unioned, with tooling to count the comparisons each rule generates. There is no learned blocking.

**Execution.** Splink is a SQL generator. Python builds a pipeline of CTEs and submits it to a backend; DuckDB is the recommended default and the docs put it at around 5 million records on a 4-core laptop ([backends guide](https://github.com/moj-analytical-services/splink/blob/master/docs/topic_guides/splink_fundamentals/backends/backends.md)). Version 5 depends only on `duckdb`, `pyarrow` and `sqlglot`, adds chunked prediction, and dropped Athena. The team reports a 1 billion record, 10 billion comparison prediction in 8.5 minutes on 192 vCPUs with a pre-release DuckDB 2.0.

**Clustering.** Connected components at a threshold, implemented as iterative SQL. The cluster ID is the minimum node ID in the component ([connected_components.py](https://github.com/moj-analytical-services/splink/blob/master/splink/internals/connected_components.py)), so it is a function of membership and changes whenever a lower-ID record joins. A second method, `cluster_using_single_best_links`, enforces at most one record per duplicate-free source. Graph metrics such as bridge edges and cluster density are offered as quality checks ([clustering.py](https://github.com/moj-analytical-services/splink/blob/master/splink/internals/linker_components/clustering.py)).

**Benchmarks and evaluation.** Built-in datasets are `fake_1000` (1,000 rows, 250 entities, plus 3,176 clerical pair labels), `historical_50k` (50,000 rows, 5,156 entities, from Wikidata), FEBRL3 and FEBRL4a/4b, and a synthetic transactions pair ([metadata.py](https://github.com/moj-analytical-services/splink/blob/master/splink/internals/datasets/metadata.py)). Evaluation is against a labels table or column: accuracy analysis across thresholds, a list of prediction errors, a labelling tool, a comparison viewer and a cluster studio ([evaluation guide](https://moj-analytical-services.github.io/splink/topic_guides/evaluation/overview.html)). It is organised as model, edge and cluster evaluation, which is a structure worth copying.

**Identity, incremental, provenance.** None of this is in the library. Version 5 adds `predict_within` and `predict_between`, marked experimental in the changelog, which score only the new pairs when records arrive ([inference.py](https://github.com/moj-analytical-services/splink/blob/master/splink/internals/linker_components/inference.py)). They return edges and leave identity to the caller. The Ministry of Justice's own [production write-up](https://moj-analytical-services.github.io/splink/blog/2026/01/29/running-splink-in-production.html) describes a weekly batch run that publishes a lookup table partitioned by `linking_snapshot_date`, with a `person_id` "prefixed with the primary dataset's ID", models retrained about yearly, and lineage kept by the pipeline around Splink. They combine edges from several models and cluster once at the end, under the rule that "an edge is scored exactly once", because a pair scored by two models is hard to explain.

**For Spool.** Borrow the SQL-in-DuckDB execution model, comparison levels as the unit of explanation, the three-level evaluation structure and the datasets. Do not borrow the cluster ID, and do not build a multi-backend SQL abstraction.

## 3. Zingg

Zingg is most relevant here for the difference between its two editions: persistent identity is a feature of Zingg Enterprise and is not part of the open-source Community edition.

**Algorithm.** Two models learned from the same labelled pairs. The blocking model is a tree of hash functions chosen to eliminate as few labelled matches as possible ([block package](https://github.com/zinggAI/zingg/tree/main/common/core/src/main/java/zingg/common/core/block)). The similarity model is Spark ML logistic regression over a degree-3 polynomial expansion of per-field similarity features ([SparkModel.java](https://github.com/zinggAI/zingg/blob/main/spark/core/src/main/java/zingg/spark/core/model/SparkModel.java)). Labels come from an active-learning loop; the docs suggest 30 to 50 labelled matches as a starting point. Clustering is GraphFrames connected components ([GraphUtil.java](https://github.com/zinggAI/zingg/blob/main/common/core/src/main/java/zingg/common/core/util/GraphUtil.java)).

**Execution.** Java on Spark 3.5 or 4.2, run as phases (`findTrainingData`, `label`, `train`, `match`, `link`). Enterprise adds a native Snowflake path. The licence is AGPL-3.0, so Spool can call Zingg as an external process and must never link it.

**Benchmarks.** CI runs performance tests on FEBRL 120k and North Carolina voters 5M. The repository ships examples for FEBRL, Amazon-Google, iTunes-Amazon and BeerAdvo-RateBeer. The "Hardware Sizing and Benchmarks" section of the [docs export](https://docs.zingg.ai/zingg/llms-full.txt) reports 120k records in 5 minutes and 5M in about 4 hours on 4 cores with 10 GB of memory. I found runtime figures and no published accuracy table.

**Identity.** Community writes `Z_CLUSTER`, which the docs describe as non-persistent: "Each time you run the match job, Zingg rebuilds the identity graph from scratch" ([Z Cluster and Zingg ID](https://docs.zingg.ai/zingg/zingg-concepts/z-cluster-and-zingg-id)). Enterprise writes `ZINGG_ID`, a GUID, and adds:

- `runIncremental`: new, changed and deleted records are matched against existing clusters; merges and unmerges happen automatically; human-approved decisions are not overridden ([incremental matching](https://docs.zingg.ai/zingg/running-zingg/run-incremental-matching)).
- `reassignZinggId`: after a model change, new clusters are mapped back to old ones through primary-key overlap and inherit the old ID where they overlap ([reassign](https://docs.zingg.ai/zingg/running-zingg/reassign-zingg-id)).
- A stated limit: if a cluster splits, "the `Zingg ID` may be reassigned", and downstream systems should treat it "as stable within a deployment lifecycle, not as an indefinitely immutable identifier".

So the Enterprise edition, as documented, carries IDs over by overlap; identity is not modelled as state with defined transitions. I found no description in the docs of a change log, a redirect from a retired ID or a rule for which side of a split keeps the ID.

**For Spool.** Borrow the list of lifecycle scenarios in Zingg's ID table as test cases, and the idea that a model upgrade must not disturb IDs. Spool does not take a Spark dependency, does not link AGPL-licensed code, and does not use overlap-based ID carry-over as its primary mechanism.

## 4. Other projects

### dedupe

Active learning over record pairs, a regularised logistic regression classifier ([labeler.py](https://github.com/dedupeio/dedupe/blob/main/dedupe/labeler.py)), learned blocking predicates, and hierarchical clustering with centroid linkage rather than connected components ([clustering.py](https://github.com/dedupeio/dedupe/blob/main/dedupe/clustering.py)). It runs in-process and returns clusters with a confidence per record. The last release was in August 2024 and the most recent commit is from July 2025. It has no notion of identity across runs. Its use of a clustering method that resists chaining is the main thing to note.

### recordlinkage

A pandas toolkit with indexers (full, block, sorted neighbourhood), comparers and classifiers including an ECM Fellegi-Sunter implementation, plus loaders for FEBRL 1 to 4 and the Krebsregister set ([docs](https://recordlinkage.readthedocs.io/en/latest/ref-datasets.html)). No release since July 2023. It is useful as a clear statement of the index, compare, classify decomposition and as a source of dataset loaders. It does no clustering and no identity.

### pyJedAI

The Python successor to JedAI from the University of Athens. Its contribution is the schema-agnostic end of the field: token and q-gram blocking, meta-blocking that prunes the blocking graph, embedding-based nearest-neighbour blocking, similarity joins, an Ollama-backed LLM matcher, and twelve clustering algorithms including unique-mapping, correlation and Markov clustering ([clustering.py](https://github.com/AI-team-UoA/pyJedAI/blob/main/src/pyjedai/clustering.py)). It ships the clean-clean ER datasets D1 to D10 with ground truth. It is in-memory and batch, with no identity. The blocking survey by the same group is the best single reference on candidate generation ([Papadakis et al., 2020](https://doi.org/10.1145/3377455)).

### Ditto and LLM-based matchers

[Ditto](https://arxiv.org/abs/2004.00584) serialises a record pair into text and fine-tunes a pre-trained language model as a binary classifier. The repository has had no commits since September 2021 and has no PyPI release, so Spool treats it as a research baseline and not as a dependency. The line of work it started has moved to prompted models:

- [Peeters, Steiner and Bizer](https://arxiv.org/abs/2310.11244) show zero-shot and few-shot LLM matching is competitive with fine-tuned models.
- [Wang et al. (ComEM)](https://arxiv.org/abs/2405.16884) show that asking the model to select the match from a candidate list beats independent pair classification, at lower cost.
- [Fu et al. (LLM-CER)](https://doi.org/10.1145/3749170) have the model cluster small record sets in context, with a guardrail step for hallucinated clusters, to cut the number of calls.
- [AnyMatch](https://arxiv.org/abs/2409.04073) and [ChatMatcher](https://cgi.di.uoa.gr/~koubarak/publications/2026/ChatMatcher26.pdf) target small or quantised models; ChatMatcher's abstract argues clustering is needed to reconcile inconsistent LLM pair decisions.

Three consequences for Spool's contract. Matchers do not all return a comparable score: some return a verdict and a rationale. Some return a selection or a small cluster instead of independent pairs. And LLM pair decisions are not transitive, so whatever consumes them has to tolerate contradiction. None of these systems keeps state between runs.

### Senzing

Proprietary, included as the reference for stateful identity. Each record is resolved on arrival against existing entities, not against other records ("entity-centric learning"). Decisions come from "principles" built on three declared behaviours of each attribute: frequency (how many entities share a value), exclusivity (how many values an entity has) and stability (whether it changes over a lifetime). Values shared too widely are marked generic and stop generating candidates. The engine is "sequence neutral": new data causes earlier decisions to be re-evaluated, so the result is meant not to depend on load order ([how it works](https://senzing.com/how-entity-resolution-works-with-senzing/)).

Two details are instructive. First, `ENTITY_ID` is not a stable identifier: the docs say values "depend on load order" ([audit tutorial](https://senzing.com/docs/tutorials/eda/eda_audit/)), and if an entity no longer exists "it was moved or deleted" ([replication guide](https://senzing.com/docs/tutorials/advanced_replication/)). The stable key is the pair of data source and record ID. Second, Senzing deliberately does not emit an ordered change log. Mutating calls return the list of affected entity IDs and the consumer re-reads them, because the engine is parallel and cannot promise an order.

**For Spool.** Borrow attribute behaviours as the vocabulary for domain constraints, generic-value detection, sequence neutrality as a measurable property, and `why` as a required capability. Spool makes two different choices: its identity IDs do not depend on load order, and it keeps an ordered change log. Spool's audit, review and undo need ordered, replayable changes, which is affordable because Spool has a single writer.

### nomenklatura and OpenSanctions

The closest working precedent for Spool's identity model, and it is small enough to read in an afternoon.

- **Evidence.** FollowTheMoney represents every property value as a [statement](https://followthemoney.tech/docs/statements/): entity ID, property, value, dataset, first seen, last seen, original value, and a `canonical_id` column. Entities are assembled from statements, so the source of every value is retained and deduplication is a relabelling of `canonical_id`.
- **Decisions.** The `Resolver` is a graph whose edges are judgements between two IDs: positive, negative, unsure or none, with a score, a user and timestamps including a soft-delete time ([resolver.py](https://github.com/opensanctions/nomenklatura/blob/main/nomenklatura/resolver/resolver.py), [judgement.py](https://github.com/opensanctions/nomenklatura/blob/main/nomenklatura/judgement.py)). Negative judgements are first-class and block transitive merges.
- **Identity.** Connected components over positive edges give clusters. The canonical ID is the highest-ranked identifier in the cluster, where minted `NK-` IDs outrank source IDs and Wikidata QIDs outrank both ([identifier.py](https://github.com/opensanctions/nomenklatura/blob/main/nomenklatura/resolver/identifier.py)).
- **Published contract.** OpenSanctions documents what consumers can rely on: retired IDs remain in a `referents` array for six months, merges appear in deltas as deletes plus an add, splits "are always manual" and the existing ID "stays with the larger portion of the cluster" ([identifiers](https://www.opensanctions.org/docs/identifiers/)).

Where it differs from what Spool needs: it is Python and tied to the FollowTheMoney schema; identity is recomputed from the graph, so there is no explicit state transition to validate; canonical IDs can change when a higher-ranked identifier joins; and I found no changeset or sync concept.

### matchbox and matchlab

The UK Department for Business and Trade built matchbox as a server that orchestrates dedupers and linkers over warehouse data. It was archived in 2026 in favour of [matchlab](https://github.com/uktrade/matchlab), "a local-first library for building, running and evaluating entity resolution pipelines". Matchlab is a lazy plan of steps with content-addressed caching, DuckDB underneath, Splink 5 as a dependency, a materialised `(root, leaf, key, source)` resolver table, and evaluation by sampling clusters for human judgement. Record identity is a content hash ("leaf"), which anchors judgements to data and not to keys.

This is a data point for two of Spool's initial design goals, local-first execution and evaluation as a first-class capability: a team with a governed server product chose to go local-first, and they treat measurement as a first-class job. The architecture proposal keeps the second goal and departs from the first: Spool runs as one server, because persistent identity needs one owner of the evidence, and it stays a single process on one machine with DuckDB underneath. I did not find durable entity IDs across changing data in matchlab's docs; it is marked alpha.

### GoldenMatch

The surveyed project closest in scope to Spool's goals, created in March 2026. Its README describes zero-configuration matching, a "durable identity layer" with stable `entity_id`s, per-field provenance, merge and split, a hash-chained audit log, Python and TypeScript packages, Rust kernels and DuckDB and Postgres surfaces.

What I confirmed in the code ([identity/store.py](https://github.com/benseverndev-oss/goldenmatch/blob/main/packages/python/goldenmatch/goldenmatch/identity/store.py), [identity/resolve.py](https://github.com/benseverndev-oss/goldenmatch/blob/main/packages/python/goldenmatch/goldenmatch/identity/resolve.py), [identity/model.py](https://github.com/benseverndev-oss/goldenmatch/blob/main/packages/python/goldenmatch/goldenmatch/identity/model.py)):

- The identity store is a Python module over SQLite or Postgres with tables for identity nodes (status `active`, `merged_into`, `split`, `retired`), source records keyed `{source}:{source_pk}` with a content hash, evidence edges between records (`same_as`, `possible_same_as`, `conflicts_with`), an append-only events table, aliases, a persisted block-key index and a runs table holding the config fingerprint.
- IDs are UUIDv7-shaped and minted, not derived.
- Identity is assigned after clustering: for each run-local cluster, `resolve_clusters` decides create, absorb or merge "based on which existing identities cover the cluster's records". That is overlap carry-over, the same family as Zingg's reassign.
- A separate stabilisation pass merges entities that keep being linked across several runs.
- The compute-to-identity handoff is being turned into a versioned `ResolutionBatch` contract, and the README draws a line between a stateless "compute engine" and a transactional "control plane".

Where it differs from Spool's design: the identity logic is in Python, the TypeScript package is a second implementation held at parity by conformance tests, and the Postgres extension embeds CPython through pyo3 ([extensions README](https://github.com/benseverndev-oss/goldenmatch/blob/main/packages/rust/extensions/README.md)). Its accuracy and scale figures, including a comparison with a hand-tuned Splink on `historical_50k`, are the project's own; I did not run them.

**For Spool.** Treat it as independent confirmation of the table design (records, evidence edges, events, runs, redirect status on the identity) and of the compute versus control split. Spool chooses a narrower scope: one small identity engine behind a server and thin clients, with matching left to interchangeable strategies.

### zer

A Rust workspace (Apache-2.0, created May 2026) implementing Fellegi-Sunter with unsupervised EM, an optional neural cross-encoder for borderline pairs, and a pipeline of schema, blocker, comparator, scorer, clusterer and entity store, each a trait. Its `EntityStore` trait has four methods: upsert, get, record-to-entity lookup and list ([traits.rs](https://github.com/ZAL-Analytics/zer/blob/main/crates/zer-core/src/traits.rs)). It is evidence that a Rust-native matcher is practical. The workspace has thirteen crates and defines a trait for each pipeline stage, and the store trait carries no identity history. Spool's design goals take the other route on extension points: interfaces are discovered through implementation, so Spool adds a trait only when a second implementation needs one.

### Adjacent: knowledge-graph memory for agents

[Graphiti](https://github.com/getzep/graphiti) builds temporal knowledge graphs for agents: facts carry validity windows and are invalidated, not deleted, and every fact traces to the "episode" it came from. Entity deduplication there is LLM-driven and is not the project's main subject. It is relevant as a neighbouring product category, because Spool's goals include agent-driven workflows, and for the bi-temporal fact model, not for resolution.

## 5. Benchmark datasets and evaluation practice

| Dataset | What it is | Truth | Used by | Fit for Spool |
| --- | --- | --- | --- | --- |
| FEBRL 1 to 4 | Synthetic person records with injected errors ([Christen, 2008](https://doi.org/10.1145/1401890.1402020)) | Encoded in record IDs | Splink, Zingg, recordlinkage, GoldenMatch | Harness sanity check; low difficulty and not companies |
| `fake_1000`, `historical_50k` | Splink's small synthetic and Wikidata-derived person sets | Cluster column, plus clerical pair labels for `fake_1000` | Splink, GoldenMatch | Day-one fixtures |
| North Carolina voters | Real person records with voter IDs | Voter ID | Zingg CI | Scale test later |
| Magellan and DeepMatcher sets | Product, citation and restaurant pairs such as Amazon-Google, DBLP-ACM, iTunes-Amazon ([list](https://github.com/anhaidgroup/deepmatcher/blob/master/Datasets.md)) | Labelled pairs | Ditto, LLM papers, Zingg examples | Pairwise only, already blocked |
| Leipzig sets | Abt-Buy, Amazon-Google, DBLP-ACM, DBLP-Scholar ([page](https://dbs.uni-leipzig.de/research/projects/benchmark-datasets-for-entity-resolution)) | Perfect mapping | pyJedAI, many papers | Clean-clean linkage |
| [WDC Products](https://arxiv.org/abs/2301.09521) | Web product offers, with controlled shares of corner cases and unseen entities | Pairs and multi-class | LLM papers | The model for building a benchmark with deliberate hard cases |
| pyJedAI D1 to D10 | Ten clean-clean sets shipped in the repo | Ground-truth files | pyJedAI | Blocking recall tests |

**Metrics in use.** Pair precision, recall and F1 dominate. Blocking is measured by pair completeness and reduction ratio. Cluster quality uses B-cubed ([Bagga and Baldwin, 1998](https://doi.org/10.3115/980845.980859)) and occasionally generalised merge distance ([Menestrina et al., 2010](https://doi.org/10.14778/1920841.1920871)), which prices merges and splits separately and so maps directly onto "incorrect merges".

**Known problems.** [Hand and Christen (2017)](https://doi.org/10.1007/s11222-017-9746-6) show the F-measure's implicit weighting of precision against recall depends on the classifier being evaluated, so precision and recall should be reported separately. [Binette et al. (2024)](https://arxiv.org/abs/2404.05622) show that pair metrics computed on benchmark samples misestimate real performance and propose entity-centric estimation from sampled clusters, implemented in [er-evaluation](https://github.com/OlivierBinette/er-evaluation) (AGPL-3.0). Most pair benchmarks are also pre-blocked, so they say nothing about candidate recall.

**What is missing.** None of the datasets above has a time dimension: batches arriving in order, entities that rename, merge or split, or scripted corrections. Research on incremental resolution exists ([Gruenheid, Dong and Srivastava, 2014](https://doi.org/10.14778/2732939.2732943); [Whang and Garcia-Molina, 2013](https://doi.org/10.1007/s00778-013-0315-0); [Saeedi, Peukert and Rahm, 2020](https://doi.org/10.1007/978-3-030-49461-2_23)) and compares incremental against batch quality, but I found no shared benchmark with ID stability as a metric. I also found no open company dataset with ground-truth clusters of the kind Spool's first use case, resolving companies across venture capital data sources, needs.

**A candidate source for a company benchmark.** GLEIF publishes the global LEI register under CC0 ([terms](https://www.gleif.org/en/meta/lei-data-terms-of-use), [download](https://www.gleif.org/en/lei-data/gleif-golden-copy/download-the-golden-copy)). Checked against the live API on 2026-11-13, the register holds 3,493,341 records. Each has a legal name, `otherNames` typed as previous legal or trading names, transliterations, addresses, registry numbers, `successorEntity` fields, event groups with types such as `MERGERS_AND_ACQUISITIONS` and `CHANGE_LEGAL_NAME`, and direct and ultimate parent relationships. 6,521 records have registration status `DUPLICATE` and 259,541 have `RETIRED`; the duplicate record I sampled pointed to its surviving LEI. The LEI is ground truth; previous and trading names supply realistic variation; parents and subsidiaries supply hard negatives; duplicates and successors supply labelled merge events. It has no domains, only 1 of 30 sampled records carried other names, and it is skewed toward regulated entities, so it would need synthetic CRM-style noise on top.

## 6. How persistent identity is done today

| Design | Examples | How the ID is chosen | Limitation |
| --- | --- | --- | --- |
| Derived from members | Splink, Zingg Community, dedupe | Minimum member ID or a run-local counter | Any membership change can change the ID |
| Overlap carry-over after a batch run | Zingg Enterprise, GoldenMatch, most in-house pipelines | New cluster inherits the ID of the old cluster it overlaps | Splits and many-to-many overlaps need tie-break rules; history is implicit |
| Canonical ID computed from a judgement graph | nomenklatura | Highest-ranked identifier in the connected component | Stable under added evidence, but the ID can change when a higher-ranked one joins; no validated transitions |
| Stateful per-record engine | Senzing | Internal counter, reassigned as entities re-form | Entity ID is explicitly not durable |
| Minted ID with redirects | [MusicBrainz](https://musicbrainz.org/doc/MusicBrainz_Identifier), [Wikidata](https://www.wikidata.org/wiki/Help:Redirects), OpenSanctions | Assigned once; a merged ID redirects to the survivor | Splits need a human or a policy to say who keeps the ID |

The last row is the oldest design, in long-standing use by public identifier systems. MusicBrainz: "When an entity is merged into another, its MBIDs redirect to the other entity." Wikidata: "Under no circumstances should redirects be deleted or repurposed." None of the matching libraries surveyed implements it as an engine feature.

The theoretical basis for separating match decisions from merge semantics is [Swoosh](https://doi.org/10.1007/s00778-008-0098-x), which treats resolution as black-box match and merge functions and states the properties under which the result is order-independent. Real matchers rarely satisfy those properties, which is why order sensitivity has to be measured, not assumed.

## 7. Borrow, avoid, and the gap

**Borrow**

- From Splink: SQL executed inside DuckDB for blocking and comparison; comparison levels as the explanation unit; model, edge and cluster evaluation as separate layers; the built-in datasets; the rule that one model scores a pair once.
- From nomenklatura and OpenSanctions: persisted pairwise judgements including negative and unsure; content kept as source-attributed evidence with the canonical ID as a label over it; minted IDs; a written ID contract for consumers.
- From MusicBrainz and Wikidata: merged IDs redirect forever and are never reused.
- From Senzing: attribute behaviours (frequency, exclusivity, stability), generic-value detection, a `why` answer for every decision, and order-independence as a goal to measure.
- From GoldenMatch: the compute versus control split, a run table that fingerprints the configuration behind every decision, and a persisted blocking index for incremental candidates.
- From matchlab: content-hashed record identity, and DuckDB on one machine as the default.
- From dedupe and pyJedAI: clustering choices other than connected components, kept behind the reconciliation step.
- From the LLM literature: verdicts with rationales, selection among candidates, and guardrails on model-proposed clusters.

**Avoid**

These are choices about Spool's own design, given its goals. Where a project is named, it is as the place the technique was observed.

- Cluster IDs derived from membership, and overlap carry-over as the main identity mechanism.
- Plain connected components as the commit rule. One wrong edge merges two entities, and several surveyed tools add or substitute something to limit that: Splink's `cluster_using_single_best_links`, nomenklatura's negative judgements, dedupe's centroid-linkage clustering.
- A plugin trait per pipeline stage before a second implementation exists (zer and pyJedAI structure their pipelines per stage).
- A multi-backend SQL abstraction (Splink uses `sqlglot` and per-dialect code to do it).
- A second implementation per language held at parity by tests (as GoldenMatch does for TypeScript). Spool's goal is one engine behind every language.
- Embedding a Python interpreter in the engine.
- Linking AGPL code (Zingg, er-evaluation). Call them out of process or reimplement the metric.
- Treating a comparable score as part of the contract. Scores from different matchers are not on one scale.
- F1 as a headline number.

**The gap**

Matching quality is contested ground with strong incumbents. The open space is narrower and more specific than "persistent identity":

1. Identity as explicit state with a small set of validated transitions, where history is the log of those transitions and any matcher can feed it.
2. One engine with thin Rust, TypeScript and Python surfaces, so a changeset means the same thing everywhere.
3. A local sandbox that produces changesets a shared store can accept or reject, which I did not find in any surveyed project.
4. Benchmarks for identity behaviour over time, which do not exist publicly.
5. A company-centred dataset with hard negatives such as subsidiaries and renames.

The [architecture proposal](architecture-proposal.md) is built around items 1, 2 and 4: its model section and sections 3 and 4 cover the first, sections 5 and 6 the second, and section 9 the fourth and fifth. It does not pursue item 3. Spool runs as one server that owns its store, so nothing is synchronised between copies.

## 8. Not verified

- Zingg Enterprise behaviour comes from its documentation only; the code is closed. That includes how `runIncremental` decides merges and unmerges.
- Senzing internals come from vendor documentation. I did not confirm the current free-evaluation record limit; a third-party page says 500 records and a 2019 article says 10,000.
- GoldenMatch's accuracy and scale numbers, its automatic split behaviour, and whether the Rust kernels or the Python code are authoritative for matching. I read the identity module and the READMEs only.
- Splink's and Zingg's performance figures are quoted from their own posts and docs.
- The Leipzig, DeepMatcher and WDC dataset descriptions rest on the linked pages being reachable and on prior knowledge; I did not download the data.
- matchlab: I read the README, glossary and query guide, not the resolver code, so the absence of durable entity IDs is a "did not find".
- GitHub star counts are omitted because I could confirm them for only two of the projects.
- For the LLM papers I read abstracts, not full experiments.
