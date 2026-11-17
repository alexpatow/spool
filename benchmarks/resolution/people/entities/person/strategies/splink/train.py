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

    spool build benchmarks/resolution/people
    uv run benchmarks/resolution/people/entities/person/strategies/splink/train.py \
        benchmarks/.cache/fake_1000.csv

The file is a CSV of person records. Its rows become features the way the
engine makes them, with the query and the transforms that `spool build`
compiled from `entity.yaml` into the manifest, so the model is trained on the
columns the strategy is given. `spool bench
benchmarks/resolution/people/fake-1000.toml` downloads the file the committed
model was trained on.

Training reads no truth column. The share of matching pairs is estimated from
two deterministic rules, the `u` probabilities from random pairs, and the `m`
probabilities by expectation maximisation.
"""

import json
import sys
from pathlib import Path

import duckdb
import splink.comparison_library as cl
from splink import DuckDBAPI, Linker, SettingsCreator, block_on

HERE = Path(__file__).resolve().parent
MANIFEST = HERE.parents[3] / ".spool" / "manifest.json"
# The source whose mapping the rows go through.
SOURCE = "fake_1000"


def settings():
    return SettingsCreator(
        link_type="dedupe_only",
        unique_id_column_name="record_id",
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
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    declaration = next(
        entity_type["declaration"]
        for entity_type in manifest["entity_types"]
        if entity_type["name"] == "person"
    )
    # What the engine's `record` relation looks like to the features query:
    # an ID, the source, and the row as it was ingested. An empty field is
    # left out.
    connection.execute(
        """
        CREATE TABLE record AS
        SELECT row_number() OVER ()::VARCHAR AS record_id, ? AS source_id, to_json(csv) AS payload
        FROM read_csv(?, all_varchar = true) AS csv
        """,
        [SOURCE, str(csv)],
    )
    for transform in declaration.get("transforms", []):
        connection.execute(
            f"CREATE MACRO spool_transform_{transform['name']}({', '.join(transform['parameters'])}) "
            f"AS ({transform['body']})"
        )
    connection.execute(f"CREATE TABLE person AS {declaration['features']}")

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
