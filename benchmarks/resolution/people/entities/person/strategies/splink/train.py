# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "splink==5.0.0",
#     "duckdb==2.0.0",
#     "pyarrow==26.0.1",
#     "sqlglot==30.32.0",
# ]
# ///
"""Trains the model that `../splink.py` judges with, and writes `model.json`.

    uv run benchmarks/resolution/people/entities/person/strategies/splink/train.py \
        benchmarks/.cache/fake_1000.csv

The file is a CSV of person records. Its rows go through this entity type's
`features.sql`, so the model is trained on the columns the strategy is given.
`spool bench benchmarks/resolution/people/fake-1000.toml` downloads the file
the committed model was trained on.

Training reads no truth column. The share of matching pairs is estimated from
two deterministic rules, the `u` probabilities from random pairs, and the `m`
probabilities by expectation maximisation.
"""

import sys
from pathlib import Path

import duckdb
import splink.comparison_library as cl
from splink import DuckDBAPI, Linker, SettingsCreator, block_on

HERE = Path(__file__).resolve().parent
FEATURES = HERE.parent.parent / "features.sql"


def settings():
    return SettingsCreator(
        link_type="dedupe_only",
        unique_id_column_name="observation_id",
        # Named, so that training again on the same file writes the same model.
        linker_uid="spool-people",
        # No comparison adjusts for how common a value is. Splink would count
        # that in the records of each run, and the score of a pair would then
        # depend on what else had arrived.
        comparisons=[
            cl.JaroWinklerAtThresholds("first_name", [0.92, 0.88, 0.7]),
            cl.JaroWinklerAtThresholds("surname", [0.92, 0.88, 0.7]),
            cl.DateOfBirthComparison("dob", input_is_string=False),
            cl.ExactMatch("city"),
            cl.LevenshteinAtThresholds("email", 2),
        ],
        blocking_rules_to_generate_predictions=[
            block_on("first_name"),
            block_on("surname"),
            block_on("dob"),
            block_on("email"),
        ],
    )


def train(csv, out):
    connection = duckdb.connect()
    # What the engine's `observation` relation looks like to features.sql:
    # an ID, and the row as it was ingested. An empty field is left out.
    connection.execute(
        """
        CREATE TABLE observation AS
        SELECT row_number() OVER ()::VARCHAR AS observation_id, to_json(csv) AS payload
        FROM read_csv(?, all_varchar = true) AS csv
        """,
        [str(csv)],
    )
    connection.execute(f"CREATE TABLE person AS {FEATURES.read_text(encoding='utf-8')}")

    linker = Linker(DuckDBAPI(connection).register("person"), settings())
    linker.training.estimate_probability_two_random_records_match(
        [block_on("first_name", "surname", "dob"), block_on("email")], recall=0.7
    )
    linker.training.estimate_u_using_random_sampling(max_pairs=1e6, seed=1)
    # Two passes, so that every comparison is estimated by a pass that does
    # not block on its column.
    linker.training.estimate_parameters_using_expectation_maximisation(block_on("dob"))
    linker.training.estimate_parameters_using_expectation_maximisation(
        block_on("first_name", "surname")
    )
    linker.misc.save_model_to_json(str(out), overwrite=True)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {Path(sys.argv[0]).name} <people.csv>")
    train(Path(sys.argv[1]), HERE / "model.json")
