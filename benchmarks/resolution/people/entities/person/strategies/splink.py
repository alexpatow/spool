# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "splink==5.0.0",
#     "duckdb==2.0.0",
#     "pyarrow==26.0.1",
#     "sqlglot==30.32.0",
# ]
# ///
"""Splink as a strategy for people.

    spool bench benchmarks/resolution/people/fake-1000.toml --strategy splink

Splink does what it is good at and nothing else. It finds its own pairs with
the blocking rules of its model, compares them, and scores them. The model
is `splink/model.json`, which `splink/train.py` wrote. It is not trained
here: a run scores with the model as it is, so the same records get the same
score whenever they arrive.

Three things of Splink's stop in this file:

  its clusters   Splink's clustering is never called. The pairs go to Spool,
                 which decides what an identity is.
  its scale      The thresholds below turn a match probability into a
                 verdict. The probability is kept as the score.
  its reach      Only pairs with a new record on one side are scored, unless
                 the run is a rematch, where every record is new.
"""

import logging
from pathlib import Path

from spool import strategy

MODEL = Path(__file__).with_suffix("") / "model.json"

# At or above MATCH the pair is the same person. From UNSURE up to MATCH the
# pair is kept as `unsure`, which changes no identity and is there to be
# looked at. Below UNSURE Splink has nothing to say, and a pair it has
# nothing to say about is not a `no_match`.
MATCH = 0.95
UNSURE = 0.5


@strategy
def judge(features):
    import duckdb
    from splink import DuckDBAPI, Linker

    connection = duckdb.connect()
    connection.register("features", features)
    columns = ", ".join(
        f'"{name}"' for name in features.column_names if name not in ("is_new", "identity_id")
    )
    connection.execute(f"CREATE TABLE fresh AS SELECT {columns} FROM features WHERE is_new")
    connection.execute(f"CREATE TABLE known AS SELECT {columns} FROM features WHERE NOT is_new")

    def count(table):
        return connection.sql(f"SELECT count(*) FROM {table}").fetchall()[0][0]

    if count("fresh") == 0:
        return None

    api = DuckDBAPI(connection)
    fresh = api.register("fresh")
    linker = Linker(fresh, str(MODEL), log_level=logging.WARNING)
    scored = [linker.inference.predict_within(fresh, threshold_match_probability=UNSURE)]
    if count("known") > 0:
        scored.append(
            linker.inference.predict_between(
                api.register("known"), fresh, threshold_match_probability=UNSURE
            )
        )

    # Which level of each comparison the pair fell into, by the comparison's
    # column: the model file says what each level is.
    levels = ", ".join(
        f"{name[len('gamma_'):]} := {name}"
        for name in connection.table(scored[0].physical_name).columns
        if name.startswith("gamma_")
    )
    judged = " UNION ALL ".join(
        f"""
        SELECT record_id_l AS left_id,
               record_id_r AS right_id,
               CASE WHEN match_probability >= {MATCH} THEN 'match' ELSE 'unsure' END AS verdict,
               match_probability AS score,
               to_json(struct_pack(match_weight := match_weight,
                                   levels := struct_pack({levels})))::VARCHAR AS detail
        FROM {table.physical_name}
        """
        for table in scored
    )
    return connection.sql(f"{judged} ORDER BY left_id, right_id").to_arrow_table()
